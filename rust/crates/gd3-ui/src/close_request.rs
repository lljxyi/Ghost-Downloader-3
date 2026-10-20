//! 关窗语义的宿主落点（第 132 步）：`Software/CloseMode` → 窗口事件循环怎么处置。
//!
//! ## 为什么要这一层
//!
//! Slint 的 `on_close_requested` **缺省返回 `CloseRequestResponse::HideWindow`**：
//! 窗口隐藏，事件循环**继续跑**（`i-slint-core-1.18.1/window.rs:2421` 实证）。
//! `gd3-ui` 有 `Runtime` 装的 `slint::Timer`，只要定时器还在，事件循环就不会自己结束。
//! 第 132 步实测（Xvfb + `xdotool windowclose`）：窗口消失了，**进程还活着**、
//! 下载还在继续、`UserConfig.json` 的写者还在 —— 用户再也看不到它，只能 kill。
//!
//! Python 侧 `main_window.py:440-475` 的 `_onCloseClicked` 是三条分支（+ 一条前置）：
//!
//! | `Software/CloseMode` | Python | 这里 |
//! |---|---|---|
//! | 任意，但**有消息框打开** | `for dialog in findChildren(MaskDialogBase): dialog.reject(); return` | [`CloseDecision::RejectDialog`] |
//! | `Ask`（缺省） | 弹 `MessageBox`：退出程序 / 继续在后台运行 + 「记住我的选择」 | [`CloseDecision::ShowAsk`] → [`crate::window::close_ask_dialog`] |
//! | `Background` | `self.close()` 隐藏窗口，托盘图标留着 | [`CloseDecision::Background`] → `Window::hide()` |
//! | `Quit` | `self.close()` + `QApplication.quit()` | [`CloseDecision::Quit`] → `quit_event_loop()` |
//!
//! 未知字面量按 `Ask` 兜底（`close_mode.rs` 的既有口径：不复刻 Python 的 KeyError 崩溃）。
//!
//! ## 与 Python 的已知差异
//!
//! - **`Background` 没有托盘可回**：Python 靠 `QSystemTrayIcon` 让用户点回来，Slint 1.18
//!   没有原生托盘/状态项（`README.md`「平台外壳」已记录降级方案）。这里照
//!   `CloseMode` 的**字面语义**隐藏窗口继续跑（那是用户显式选的），并在**首次**降级时
//!   往 stderr 打一条提示，说明窗口要用托盘才找得回来。`NullTray` 接线后这句提示可以删。
//! - **darwin 的两个特例未镜像**：`sys.platform == "darwin" and mode != QUIT` 时
//!   Python 只 `hide()`（并先存几何）、全屏时先 `showNormal()` 再 1 秒后重来。
//!   这两支依赖 AppKit 的窗口状态，Rust 侧没有对应能力，等 8f 平台外壳那一步一起接。
//! - **`dialogButtonClicked` 是单槽回调**：Slint 的 `on_dialogButtonClicked` 只能有一个
//!   处理函数，后装的会覆盖先装的。所以第 140 步起由外壳动作集（[`crate::shell_host`]）
//!   独占这个槽，再用 [`CloseHooks::handle_button`] 把点击**先问一遍**本模块的关窗链
//!   （返回 `false` 表示不是关窗框，交回调用方）。`boot` 因此调 [`install_shared`]
//!   而不是 [`install`]。

use std::cell::Cell;
use std::rc::Rc;

use serde_json::Value;
use slint::{CloseRequestResponse, ComponentHandle};

use crate::AppWindow;
use crate::settings_writer::SettingsWriter;

/// `Software/CloseMode` 的属性名（`cfg.py:309-312`）。
pub const CLOSE_MODE_ATTR: &str = "closeMode";

/// 一次关窗请求的落点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseDecision {
    /// 先关掉开着的消息框，窗口留着。
    RejectDialog,
    /// 弹「是否完全退出程序？」。
    ShowAsk,
    /// 隐藏窗口，进程继续跑（后台下载）。
    Background,
    /// 退出事件循环。
    Quit,
}

/// 纯判定：一条 `CloseMode` 字面量 + 「有没有消息框开着」→ 落点。
///
/// `mode` 为 `None`（缺键）或未知字面量时按 `Ask` 处理。
pub fn decide(mode: Option<&str>, dialog_visible: bool) -> CloseDecision {
    if dialog_visible {
        return CloseDecision::RejectDialog;
    }
    match mode {
        Some("Quit") => CloseDecision::Quit,
        Some("Background") => CloseDecision::Background,
        _ => CloseDecision::ShowAsk,
    }
}

/// 关窗流程里要跨回调保持的两个小状态。
#[derive(Debug, Default)]
pub struct CloseState {
    /// 「记住我的选择」当前勾选态（`dialogCheckboxToggled` 写、按钮分派读）。
    remember: Cell<bool>,
    /// 后台降级的 stderr 提示只打一次。
    warned: Cell<bool>,
}

impl CloseState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn remember(&self) -> bool {
        self.remember.get()
    }

    pub fn set_remember(&self, checked: bool) {
        self.remember.set(checked);
    }
}

/// 处理一次关窗请求（闭包本体抽出来，便于单测）。
pub fn handle_close(
    window: &AppWindow,
    cfg: &SettingsWriter,
    state: &CloseState,
) -> CloseRequestResponse {
    let mode = cfg.get(CLOSE_MODE_ATTR).and_then(|value| match value {
        Value::String(text) => Some(text),
        _ => None,
    });

    match decide(mode.as_deref(), window.get_dialogVisible()) {
        CloseDecision::RejectDialog => {
            crate::hide_dialog(window);
            CloseRequestResponse::KeepWindowShown
        }
        CloseDecision::ShowAsk => {
            // 每次都是新对话框对象（Python `_onCloseClicked` 里 `MessageBox(...)`），
            // 勾选态不残留。
            state.set_remember(false);
            crate::show_dialog(window, &crate::window::close_ask_dialog());
            CloseRequestResponse::KeepWindowShown
        }
        CloseDecision::Background => {
            hide_to_background(window, state);
            CloseRequestResponse::HideWindow
        }
        CloseDecision::Quit => {
            crate::hide_dialog(window);
            quit();
            CloseRequestResponse::HideWindow
        }
    }
}

/// 处理关窗询问框的按钮点击：`true` = 这个框归本模块管（已处理）。
///
/// 按**按钮文案**分派（`lib.rs::dialog_button_text`，等价 Qt 侧的按钮身份），
/// 否则说明这是别的消息框，交回调用方。
pub fn handle_dialog_button(
    window: &AppWindow,
    cfg: &SettingsWriter,
    state: &CloseState,
    index: usize,
) -> bool {
    if !window.get_dialogVisible() {
        return false;
    }
    let Some(text) = crate::dialog_button_text(window, index) else {
        return false;
    };
    let should_quit = match text.as_str() {
        crate::window::CLOSE_ASK_YES => true,
        crate::window::CLOSE_ASK_NO => false,
        // 别的框（异常 / 配对 / 删除确认）：不归这里管。
        _ => return false,
    };

    let remember = state.remember.get();
    crate::hide_dialog(window);
    if remember {
        // Python：`if checkbox.isChecked(): cfg.set(cfg.closeMode, mode)`。
        let mode = if should_quit { "Quit" } else { "Background" };
        cfg.set_named(CLOSE_MODE_ATTR, &Value::from(mode));
    }
    if should_quit {
        quit();
    } else {
        hide_to_background(window, state);
    }
    true
}

/// 关窗链的句柄：装完返回，让**别的**分派链（外壳动作集）能复用它，
/// 而不去抢 `on_dialogButtonClicked` 那个单槽。
pub struct CloseHooks {
    cfg: Rc<SettingsWriter>,
    state: Rc<CloseState>,
}

impl CloseHooks {
    /// [`handle_close`] 的句柄版。
    pub fn handle_close(&self, window: &AppWindow) -> CloseRequestResponse {
        handle_close(window, &self.cfg, &self.state)
    }

    /// [`handle_dialog_button`] 的句柄版：`true` = 这是关窗框，已经处理完。
    pub fn handle_button(&self, window: &AppWindow, index: usize) -> bool {
        handle_dialog_button(window, &self.cfg, &self.state, index)
    }

    /// 「记住我的选择」复选框。
    pub fn set_remember(&self, checked: bool) {
        self.state.set_remember(checked);
    }

    pub fn remember(&self) -> bool {
        self.state.remember()
    }
}

/// 只装 `on_close_requested`，把按钮 / 勾选槽**留给调用方**。
///
/// `boot` 走这条：`dialogButtonClicked` 由 [`crate::shell_host`] 独占，它的分派链里
/// 第一站就是 [`CloseHooks::handle_button`]。
pub fn install_shared(window: &AppWindow, cfg: Rc<SettingsWriter>) -> Rc<CloseHooks> {
    let hooks = Rc::new(CloseHooks {
        cfg,
        state: Rc::new(CloseState::new()),
    });

    let close_window = window.as_weak();
    let close_hooks = Rc::clone(&hooks);
    window
        .window()
        .on_close_requested(move || match close_window.upgrade() {
            Some(window) => close_hooks.handle_close(&window),
            None => CloseRequestResponse::HideWindow,
        });

    hooks
}

/// 把关窗闭环**整份**装到窗口上（`on_close_requested` + 两个对话框槽）。
///
/// 两个槽也被本模块占住，所以只适合「没有别家对话框分派」的场景（单测 / 将来的
/// 独立窗口）；`boot` 用的是 [`install_shared`]。
pub fn install(window: &AppWindow, cfg: Rc<SettingsWriter>) -> Rc<CloseHooks> {
    let hooks = install_shared(window, cfg);

    let button_window = window.as_weak();
    let button_hooks = Rc::clone(&hooks);
    window.on_dialogButtonClicked(move |index| {
        if let Some(window) = button_window.upgrade() {
            button_hooks.handle_button(&window, index.max(0) as usize);
        }
    });

    let checkbox_hooks = Rc::clone(&hooks);
    window.on_dialogCheckboxToggled(move |checked| checkbox_hooks.set_remember(checked));

    hooks
}

/// 隐藏窗口但留着进程（`CloseMode=Background`）。
fn hide_to_background(window: &AppWindow, state: &CloseState) {
    if !state.warned.replace(true) {
        eprintln!(
            "gd3-ui：按 CloseMode=Background 隐藏窗口继续在后台运行。\
             托盘 / 状态项尚未接线（见 rust/README.md「平台外壳」），窗口暂时无法从 UI 找回。"
        );
    }
    let _ = window.window().hide();
}

/// `QApplication.quit()` 的镜像。
///
/// 失败必须**说出来**：这里吞掉错误就等于「窗口没了、进程还在」，用户只能 kill
/// （第 132 步实测过一次）。没有事件循环在跑（单测 / 无头冒烟）时
/// `NoEventLoopProvider` 是预期结果。
fn quit() {
    if let Err(error) = slint::quit_event_loop() {
        eprintln!("gd3-ui: 请求退出事件循环失败：{error:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use crate::config_store::ConfigStore;
    use crate::window;

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "gd3-ui-close-request-{}-{tag}-{seq}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    fn writer(tag: &str) -> (PathBuf, SettingsWriter) {
        let dir = temp_dir(tag);
        let path = dir.join("UserConfig.json");
        let store = ConfigStore::new(&path);
        (path, SettingsWriter::new(store))
    }

    fn set_mode(writer: &SettingsWriter, mode: &str) {
        writer.set_named(CLOSE_MODE_ATTR, &Value::from(mode));
    }

    #[test]
    fn decide_follows_close_mode_and_dialog_precedence() {
        // 有消息框开着 → 一律先关框（Python 的第一条分支）。
        assert_eq!(decide(Some("Quit"), true), CloseDecision::RejectDialog);
        assert_eq!(decide(None, true), CloseDecision::RejectDialog);

        assert_eq!(decide(Some("Quit"), false), CloseDecision::Quit);
        assert_eq!(decide(Some("Background"), false), CloseDecision::Background);
        assert_eq!(decide(Some("Ask"), false), CloseDecision::ShowAsk);
        // 缺键 / 未知字面量兜底 Ask（`close_mode.rs` 的既有口径）。
        assert_eq!(decide(None, false), CloseDecision::ShowAsk);
        assert_eq!(decide(Some("Weird"), false), CloseDecision::ShowAsk);
    }

    #[test]
    fn a_visible_dialog_is_rejected_first() {
        let (_path, writer) = writer("reject");
        let window = crate::create_headless();
        crate::show_dialog(&window, &window::close_ask_dialog());
        let state = CloseState::new();

        let response = handle_close(&window, &writer, &state);

        assert_eq!(response, CloseRequestResponse::KeepWindowShown);
        assert!(!window.get_dialogVisible(), "消息框应当被关掉");
    }

    #[test]
    fn quit_mode_closes_without_asking() {
        let (_path, writer) = writer("quit");
        set_mode(&writer, "Quit");
        let window = crate::create_headless();
        let state = CloseState::new();

        let response = handle_close(&window, &writer, &state);

        assert_eq!(response, CloseRequestResponse::HideWindow);
        assert!(!window.get_dialogVisible(), "Quit 不该弹框");
    }

    #[test]
    fn background_mode_hides_without_asking() {
        let (_path, writer) = writer("background");
        set_mode(&writer, "Background");
        let window = crate::create_headless();
        let state = CloseState::new();

        let response = handle_close(&window, &writer, &state);

        assert_eq!(response, CloseRequestResponse::HideWindow);
        assert!(!window.get_dialogVisible(), "Background 不该弹框");
    }

    #[test]
    fn ask_mode_shows_the_close_dialog() {
        let (_path, writer) = writer("ask");
        let window = crate::create_headless();
        let state = CloseState::new();
        state.set_remember(true); // 上一次的勾选态不能漏进来

        let response = handle_close(&window, &writer, &state);

        assert_eq!(response, CloseRequestResponse::KeepWindowShown);
        assert!(window.get_dialogVisible());
        assert_eq!(
            crate::dialog_button_text(&window, 0).as_deref(),
            Some(window::CLOSE_ASK_YES)
        );
        assert_eq!(
            crate::dialog_button_text(&window, 1).as_deref(),
            Some(window::CLOSE_ASK_NO)
        );
        assert!(!state.remember(), "每次弹框都从「未勾选」开始");
    }

    #[test]
    fn remembering_the_choice_writes_close_mode() {
        let (_path, writer) = writer("remember-quit");
        let window = crate::create_headless();
        let state = CloseState::new();
        crate::show_dialog(&window, &window::close_ask_dialog());
        state.set_remember(true);

        assert!(handle_dialog_button(&window, &writer, &state, 0));

        assert!(!window.get_dialogVisible());
        assert_eq!(writer.get(CLOSE_MODE_ATTR), Some(Value::from("Quit")));

        // 另一半：选「继续在后台运行」→ Background。
        let window = crate::create_headless();
        crate::show_dialog(&window, &window::close_ask_dialog());
        assert!(handle_dialog_button(&window, &writer, &state, 1));
        assert_eq!(writer.get(CLOSE_MODE_ATTR), Some(Value::from("Background")));
    }

    #[test]
    fn not_remembering_leaves_close_mode_untouched() {
        let (_path, writer) = writer("remember-off");
        let window = crate::create_headless();
        let state = CloseState::new();
        crate::show_dialog(&window, &window::close_ask_dialog());

        assert!(handle_dialog_button(&window, &writer, &state, 0));

        assert_eq!(
            writer.get(CLOSE_MODE_ATTR),
            Some(Value::from("Ask")),
            "缺省没变"
        );
    }

    #[test]
    fn other_dialogs_are_left_to_the_caller() {
        let (_path, writer) = writer("other");
        let window = crate::create_headless();
        let state = CloseState::new();
        crate::show_dialog(&window, &window::exception_dialog("炸了"));

        assert!(
            !handle_dialog_button(&window, &writer, &state, 0),
            "不归这里管"
        );
        assert!(window.get_dialogVisible(), "不该被关掉");

        // 没有框开着 → 也不算处理。
        crate::hide_dialog(&window);
        assert!(!handle_dialog_button(&window, &writer, &state, 0));
    }

    /// `install` 装的是真的 `on_close_requested`：派发一次关窗事件，看事件循环拿到的答复
    /// （`Rejected` = 窗口留着，`Accepted` = 准予隐藏）。这条覆盖「接线」本身，
    /// 上面几条只覆盖判定函数。
    #[test]
    fn installed_handler_answers_the_real_close_event() {
        use slint::platform::{WindowEvent, WindowEventDispatchResult};

        // Ask（缺省）：留住窗口 + 弹框。
        let (_ask_path, ask_writer) = writer("installed-ask");
        let window = crate::create_headless();
        install(&window, Rc::new(ask_writer));
        let result = window
            .window()
            .dispatch_event_with_result(WindowEvent::CloseRequested)
            .expect("派发关窗事件");
        assert_eq!(
            result,
            WindowEventDispatchResult::Rejected,
            "Ask 必须留住窗口等用户答复"
        );
        assert!(window.get_dialogVisible());

        // Quit：放行隐藏（随后 `quit_event_loop` 让事件循环收工）。
        let (_quit_path, quit_writer) = writer("installed-quit");
        set_mode(&quit_writer, "Quit");
        let window = crate::create_headless();
        install(&window, Rc::new(quit_writer));
        let result = window
            .window()
            .dispatch_event_with_result(WindowEvent::CloseRequested)
            .expect("派发关窗事件");
        assert_eq!(
            result,
            WindowEventDispatchResult::Accepted,
            "Quit 不该弹框，直接放行"
        );
        assert!(!window.get_dialogVisible());
    }

    /// 按钮槽 / 勾选槽也走 `install` 装的真回调：勾「记住我的选择」+ 点「退出程序」
    /// → `closeMode` 落盘成 `Quit`（Python 的 `cfg.set(cfg.closeMode, mode)`）。
    #[test]
    fn installed_button_slot_resolves_the_ask_dialog() {
        let (path, writer) = writer("installed-buttons");
        let window = crate::create_headless();
        install(&window, Rc::new(writer));
        crate::show_dialog(&window, &window::close_ask_dialog());

        window.invoke_dialogCheckboxToggled(true);
        window.invoke_dialogButtonClicked(0);

        assert!(!window.get_dialogVisible(), "点完按钮框要关掉");
        let text = std::fs::read_to_string(&path).expect("读回配置");
        assert!(
            text.contains("\"CloseMode\": \"Quit\""),
            "closeMode 应落盘成 Quit：{text}"
        );
    }
}
