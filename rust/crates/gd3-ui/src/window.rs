//! 主窗口派生层（第 56 步）：镜像 `app/view/windows/main_window.py` 里
//! 「不依赖 Qt 部件」的那部分——导航表、搜索框口径、拖放判定、关闭模式
//! 分派、通知文案、更新提示过滤。
//!
//! **未镜像**（都是 Qt/平台外壳，Slint 侧要另想办法）：
//!
//! - `DropOverlay` 自绘半透明遮罩（`paintEvent`）；
//! - `_refreshBackgroundEffect` / `_setBackgroundEffectWin` /
//!   `_setBackgroundEffectMac` / `_removeMacAcrylicEffect` /
//!   `_setMacAcrylicAppearance`（Mica/Acrylic 是 Win/Mac 专有 API）；
//! - `refreshThemeColor` / `_setTheme`（主题色走 qfluentwidgets 的
//!   `setThemeColor` + `updateStyleSheet`，Slint 侧用 `Palette`）；
//! - `systemTitleBarRect`、`changeEvent`、`resizeEvent`、`showEvent` 里的
//!   窗口几何持久化（`cfg.geometry` 由调用方管）；
//! - `setupPacks`（pack 提供的页面类由 featureService 给，Rust 侧暂无
//!   pack 页面）。
//!
//! 这里的判定顺序**本身有语义**（Python 是短路链），所以每个函数都按
//! 原顺序逐条写，不做等价改写。

use crate::close_mode;
use crate::pystr::{py_strip, split_lines};

// ---------------------------------------------------------------------------
// 导航表（`_initLayout` / `_addPage`）
// ---------------------------------------------------------------------------

/// `TaskPage` 的路由键（`pageClass.__name__`）。
pub const PAGE_TASK: &str = "TaskPage";
/// `SettingPage` 的路由键。
pub const PAGE_SETTINGS: &str = "SettingPage";
/// 「新建任务」不是页面，只是一个不可选中的导航项（`selectable=False`）。
pub const ROUTE_ADD_TASK: &str = "addTaskButton";

/// 导航项位置（镜像 `NavigationItemPosition.TOP/BOTTOM`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavPosition {
    Top,
    Bottom,
}

/// 一个导航项。图标只有名字——矢量资源在主题对齐步再换。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NavItem {
    /// `routeKey`：页面用类名，按钮用固定串。
    pub route_key: &'static str,
    /// `text`（`self.tr(...)` 的源串；没装 translator 时原样）。
    pub title: &'static str,
    /// `FluentIcon` 的名字。
    pub icon: &'static str,
    pub position: NavPosition,
    /// `False` = 点它不切页、也不进选中态（「新建任务」）。
    pub selectable: bool,
}

/// 导航栏顺序：`_initLayout` 里 TOP 的 `TaskPage` → `addTaskButton`，
/// 再 BOTTOM 的 `SettingPage`。`setupPacks` 追加的页面插在 `SettingPage` 之前，
/// 由调用方自行插入。
pub const NAV_ITEMS: [NavItem; 3] = [
    NavItem {
        route_key: PAGE_TASK,
        title: "下载任务",
        icon: "DOWNLOAD",
        position: NavPosition::Top,
        selectable: true,
    },
    NavItem {
        route_key: ROUTE_ADD_TASK,
        title: "新建任务",
        icon: "ADD",
        position: NavPosition::Top,
        selectable: false,
    },
    NavItem {
        route_key: PAGE_SETTINGS,
        title: "设置",
        icon: "SETTING",
        position: NavPosition::Bottom,
        selectable: true,
    },
];

// ---------------------------------------------------------------------------
// 搜索框（`_updateSearchTarget` / `_onSearchTextChanged` / `_refreshSearchEditGeometry`）
// ---------------------------------------------------------------------------

/// 切页时搜索框该怎么变。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchTarget {
    /// `None` = 页面没有 `searchPlaceholder` → 搜索框 `hide()`。
    pub placeholder: Option<&'static str>,
    /// 页面有 `setSearchText` → 键入时把文本转发过去。
    pub accepts_text: bool,
}

/// 镜像 `_updateSearchTarget` 的 `hasattr(page, 'searchPlaceholder')` 判定。
///
/// 两个内置页面都有 `searchPlaceholder` + `setSearchText`；未知路由（pack
/// 页面 / 未实现页面）按「没有」处理 → 搜索框隐藏。`_updateSearchTarget`
/// 无论如何都会先 `self.searchEdit.clear()`，这一步由调用方做。
pub fn search_target(route_key: &str) -> SearchTarget {
    match route_key {
        PAGE_TASK => SearchTarget {
            placeholder: Some("搜索任务"),
            accepts_text: true,
        },
        PAGE_SETTINGS => SearchTarget {
            placeholder: Some("搜索设置"),
            accepts_text: true,
        },
        _ => SearchTarget {
            placeholder: None,
            accepts_text: false,
        },
    }
}

/// `_refreshSearchEditGeometry` 的三个输出。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchGeometry {
    pub width: i32,
    pub x: i32,
    pub y: i32,
}

/// 镜像 `_refreshSearchEditGeometry`：
///
/// ```python
/// w = min(360, tb.width() - 300)
/// w = max(200, w)
/// x = (tb.width() - w) // 2
/// y = (tb.height() - self.searchEdit.height()) // 2
/// ```
///
/// `//` 是**向下取整**，不是向零截断：`(5 - 2) // 2 == 1`、
/// `(-5) // 2 == -3`（Rust `/` 会给 `-2`），所以用 `div_euclid`。
pub fn search_edit_geometry(
    title_bar_width: i32,
    title_bar_height: i32,
    edit_height: i32,
) -> SearchGeometry {
    let width = (title_bar_width - 300).clamp(200, 360);
    SearchGeometry {
        width,
        x: (title_bar_width - width).div_euclid(2),
        y: (title_bar_height - edit_height).div_euclid(2),
    }
}

// ---------------------------------------------------------------------------
// 拖放（`dragEnterEvent` / `dropEvent`）
// ---------------------------------------------------------------------------

/// `dragEnterEvent` 需要的输入。
#[derive(Clone, Copy, Debug)]
pub struct DropInput<'a> {
    /// `QApplication.instance().isFileDragActive`（应用内拖文件时置真）。
    pub is_file_drag_active: bool,
    /// `mime.hasUrls()`。
    pub has_urls: bool,
    /// `mime.hasText()`。
    pub has_text: bool,
    /// `mime.text()`（`has_text` 为假时给空串）。
    pub text: &'a str,
    /// 有可见的 `MaskDialogBase`（模态对话框盖着）。
    pub mask_dialog_visible: bool,
}

/// `dragEnterEvent` 的结论（`event.acceptProposedAction()` 与否）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropDecision {
    Accept,
    Ignore,
}

/// 把文本按 CPython `splitlines()` 切行、`strip()` 去空白、丢空行。
fn text_lines(text: &str) -> Vec<String> {
    split_lines(text)
        .iter()
        .map(|line| py_strip(line))
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// 镜像 `dragEnterEvent` 的短路链：
///
/// 1. 应用内拖文件 → `ignore`；
/// 2. `hasUrls() or (hasText() and 任一行 match)` 不成立 → `ignore`；
/// 3. 有可见模态框 → `ignore`；
/// 4. 否则 `acceptProposedAction()`。
pub fn drop_decision(input: &DropInput<'_>, matches: impl Fn(&str) -> bool) -> DropDecision {
    if input.is_file_drag_active {
        return DropDecision::Ignore;
    }
    let acceptable = input.has_urls
        || (input.has_text && text_lines(input.text).iter().any(|line| matches(line)));
    if !acceptable {
        return DropDecision::Ignore;
    }
    if input.mask_dialog_visible {
        return DropDecision::Ignore;
    }
    DropDecision::Accept
}

/// 镜像 `dropEvent` 的取址：有 URL 就取 URL，否则取「能 match 的文本行」。
///
/// 注意 `dropEvent` **不**检查 `isFileDragActive` 与模态框（那些在
/// `dragEnterEvent` 里已经拦过），这里也不检查。
pub fn drop_urls(
    has_urls: bool,
    urls: &[String],
    text: &str,
    matches: impl Fn(&str) -> bool,
) -> Vec<String> {
    if has_urls {
        return urls.to_vec();
    }
    text_lines(text)
        .into_iter()
        .filter(|line| matches(line))
        .collect()
}

// ---------------------------------------------------------------------------
// 关闭（`_onCloseClicked`）
// ---------------------------------------------------------------------------

/// 平台标识（与 `sys.platform` 同口径，只有 `darwin` 分支与别不同）。
pub const PLATFORM_WINDOWS: &str = "win32";
pub const PLATFORM_MACOS: &str = "darwin";
pub const PLATFORM_LINUX: &str = "linux";

/// `CloseMode` 的三个配置字面量（与 `close_mode::OPTIONS` 同源）。
pub const MODE_ASK: &str = close_mode::OPTIONS[0];
pub const MODE_BACKGROUND: &str = close_mode::OPTIONS[1];
pub const MODE_QUIT: &str = close_mode::OPTIONS[2];

/// `_onCloseClicked` 的分派结论。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseAction {
    /// 有可见的 `MaskDialogBase` → 只 `reject()` 那一层，窗口不动。
    RejectTopDialog,
    /// macOS 全屏 → 先 `showNormal()`，1 秒后重试关闭。
    LeaveFullScreen,
    /// macOS 非退出模式 → 记几何后 `hide()`（留托盘）。
    HideToTray,
    /// 需要弹「是否完全退出程序？」确认框。
    Ask,
    /// `close()` 后留在后台（`Background`）。
    Close,
    /// `close()` 后 `QApplication.quit()`（`Quit`）。
    CloseAndQuit,
}

/// 镜像 `_onCloseClicked` 的前半段（不含 ASK 对话框本身）。
///
/// 顺序：可见模态框 → macOS 全屏 → macOS 非退出模式 → ASK → QUIT/Background。
pub fn close_action(
    platform: &str,
    mode: &str,
    has_visible_dialog: bool,
    is_full_screen: bool,
) -> CloseAction {
    if has_visible_dialog {
        return CloseAction::RejectTopDialog;
    }
    let is_macos = platform == PLATFORM_MACOS;
    if is_macos && is_full_screen {
        return CloseAction::LeaveFullScreen;
    }
    if is_macos && mode != MODE_QUIT {
        return CloseAction::HideToTray;
    }
    if mode == MODE_ASK {
        return CloseAction::Ask;
    }
    if mode == MODE_QUIT {
        return CloseAction::CloseAndQuit;
    }
    CloseAction::Close
}

/// ASK 确认框的返回：先 `exec()` 拿布尔，再决定要不要记住选择。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AskOutcome {
    pub action: CloseAction,
    /// `Some(mode)` = 用户勾了「记住我的选择」，调用方应 `cfg.set(closeMode, mode)`。
    pub remember: Option<&'static str>,
}

/// 镜像 `_onCloseClicked` 的后半段：`mode = QUIT if dialog.exec() else
/// BACKGROUND`，勾了「记住」就落盘，然后 `close()` + （QUIT 时）`quit()`。
pub fn resolve_ask(quit: bool, remember: bool) -> AskOutcome {
    let mode = if quit { MODE_QUIT } else { MODE_BACKGROUND };
    AskOutcome {
        action: if quit {
            CloseAction::CloseAndQuit
        } else {
            CloseAction::Close
        },
        remember: remember.then_some(mode),
    }
}

// ---------------------------------------------------------------------------
// 通知文案
// ---------------------------------------------------------------------------

/// `onPairRequested` 的标题与按钮文案。
pub const PAIR_REQUEST_TITLE: &str = "浏览器扩展配对请求";
pub const PAIR_REQUEST_ALLOW: &str = "允许配对";
pub const PAIR_REQUEST_DENY: &str = "拒绝";
/// `request.clientKind or self.tr("浏览器扩展")` 的兜底。
pub const PAIR_CLIENT_FALLBACK: &str = "浏览器扩展";
/// `request.extensionVersion or self.tr("未知")` 的兜底。
pub const PAIR_VERSION_FALLBACK: &str = "未知";

/// 镜像 `onPairRequested` 的 `content`。
///
/// `request.clientKind or ...` / `request.extensionVersion or ...` 是**假值**
/// 判断：`None` 与空串都走兜底。
pub fn pair_request_text(
    peer: &str,
    client_kind: Option<&str>,
    extension_version: Option<&str>,
) -> String {
    let client = client_kind
        .filter(|value| !value.is_empty())
        .unwrap_or(PAIR_CLIENT_FALLBACK);
    let version = extension_version
        .filter(|value| !value.is_empty())
        .unwrap_or(PAIR_VERSION_FALLBACK);
    format!(
        "浏览器扩展正在请求连接到 Ghost Downloader。\n\n\
         来源: {peer}\n客户端: {client}\n扩展版本: {version}\n\n\
         仅在你刚刚点击扩展里的\"自动配对\"时允许。"
    )
}

/// `onBrowserListenFailed` / `onAria2ListenFailed` 的 InfoBar 标题。
pub const LISTEN_FAILED_BROWSER_TITLE: &str = "浏览器扩展服务无法启动";
pub const LISTEN_FAILED_ARIA2_TITLE: &str = "Aria2 RPC 兼容服务无法启动";
/// `_onListenFailed` 里那个「前往设置」按钮的文案。
pub const LISTEN_FAILED_SETTINGS_BUTTON: &str = "前往设置";

/// `onUpdateAvailable` 的标题、正文模板与三个按钮文案。
pub const UPDATE_AVAILABLE_TITLE: &str = "检测到新版本";
pub const UPDATE_DOWNLOAD_BUTTON: &str = "立即下载";
pub const UPDATE_DETAIL_BUTTON: &str = "查看详情";
pub const UPDATE_SPONSOR_BUTTON: &str = "请作者喝咖啡";

/// 镜像 `onUpdateAvailable` 的 `content`：`self.tr("最新版本: {0}")`。
pub fn update_available_content(latest_version: &str) -> String {
    format!("最新版本: {latest_version}")
}

/// `onExceptionCaught` 的标题、正文模板与「查看日志」提示。
pub const EXCEPTION_TITLE: &str = "程序发生异常";
pub const EXCEPTION_LOG_TOOLTIP: &str = "查看日志";

/// 镜像 `onExceptionCaught` 的正文。
pub fn exception_text(message: &str) -> String {
    format!("点击\"确定\"后将复制错误信息并打开反馈页面。\n\n{message}")
}

/// `_onCloseClicked` 的 ASK 确认框文案。
pub const CLOSE_ASK_TITLE: &str = "是否完全退出程序？";
pub const CLOSE_ASK_CONTENT: &str = "后台运行时可通过系统托盘图标重新打开。";
pub const CLOSE_ASK_YES: &str = "退出程序";
pub const CLOSE_ASK_NO: &str = "继续在后台运行";
pub const CLOSE_ASK_REMEMBER: &str = "记住我的选择";

// ---------------------------------------------------------------------------
// 更新进度提示（`_onUpdateChanged`）
// ---------------------------------------------------------------------------

/// `UpdateState` 里会触发 `ProgressToast` 的三个状态（`IntEnum` 名字口径）。
pub const PROGRESS_TOAST_STATES: [&str; 3] = ["DOWNLOADING", "READY", "FAILED"];
/// 更新服务只关心应用自身，`targetId` 固定是 `"app"`。
pub const UPDATE_TARGET_APP: &str = "app";

/// 镜像 `_onUpdateChanged` 的两个提前 `return`。
///
/// `ProgressToast` 一旦建出来就复用（`if self._progressToast is None`），
/// 所以这里只回答「这次事件要不要更新 toast」。
pub fn wants_progress_toast(target_id: &str, state: &str) -> bool {
    target_id == UPDATE_TARGET_APP && PROGRESS_TOAST_STATES.contains(&state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(text: &'a str) -> DropInput<'a> {
        DropInput {
            is_file_drag_active: false,
            has_urls: false,
            has_text: true,
            text,
            mask_dialog_visible: false,
        }
    }

    #[test]
    fn nav_table_matches_python_init_layout() {
        assert_eq!(NAV_ITEMS.len(), 3);
        assert_eq!(NAV_ITEMS[0].route_key, PAGE_TASK);
        assert_eq!(NAV_ITEMS[0].position, NavPosition::Top);
        assert!(NAV_ITEMS[0].selectable);
        // 「新建任务」不可选中。
        assert_eq!(NAV_ITEMS[1].route_key, ROUTE_ADD_TASK);
        assert!(!NAV_ITEMS[1].selectable);
        assert_eq!(NAV_ITEMS[2].route_key, PAGE_SETTINGS);
        assert_eq!(NAV_ITEMS[2].position, NavPosition::Bottom);
    }

    #[test]
    fn search_target_hides_for_unknown_pages() {
        assert_eq!(search_target(PAGE_TASK).placeholder, Some("搜索任务"));
        assert_eq!(search_target(PAGE_SETTINGS).placeholder, Some("搜索设置"));
        assert!(search_target(PAGE_TASK).accepts_text);
        let unknown = search_target("PackPage");
        assert_eq!(unknown.placeholder, None);
        assert!(!unknown.accepts_text);
    }

    #[test]
    fn search_geometry_floors_like_python() {
        // w = min(360, 960-300) = 360; x = (960-360)//2 = 300; y = (48-28)//2 = 10
        assert_eq!(
            search_edit_geometry(960, 48, 28),
            SearchGeometry {
                width: 360,
                x: 300,
                y: 10
            }
        );
        // 窄标题栏：w = min(360, 400-300)=100 → max(200,100)=200；
        // x = (400-200)//2 = 100。
        assert_eq!(search_edit_geometry(400, 48, 28).width, 200);
        assert_eq!(search_edit_geometry(400, 48, 28).x, 100);
        // 奇数差向下取整：y = (49-28)//2 = 10（10.5 向下）。
        assert_eq!(search_edit_geometry(960, 49, 28).y, 10);
        // 负差也向下取整：(-5)//2 == -3，不是 Rust `/` 的 -2。
        assert_eq!(search_edit_geometry(195, 48, 53).y, -3);
    }

    #[test]
    fn drop_decision_follows_the_short_circuit() {
        let match_all = |_: &str| true;
        let never = |_: &str| false;

        // 1. 应用内拖文件优先于一切。
        let mut active = input("https://e/1");
        active.is_file_drag_active = true;
        assert_eq!(drop_decision(&active, match_all), DropDecision::Ignore);

        // 2. 有 URL 即接受，不必 match。
        let mut urls = input("");
        urls.has_urls = true;
        assert_eq!(drop_decision(&urls, never), DropDecision::Accept);

        // 3. 纯文本要任一行 match。
        assert_eq!(
            drop_decision(&input("https://e/1"), match_all),
            DropDecision::Accept
        );
        assert_eq!(
            drop_decision(&input("https://e/1"), never),
            DropDecision::Ignore
        );
        // 全是空白行 → 没有可 match 的行。
        assert_eq!(
            drop_decision(&input("  \n\t\n"), match_all),
            DropDecision::Ignore
        );

        // 4. 模态框盖着 → 忽略。
        let mut masked = input("https://e/1");
        masked.mask_dialog_visible = true;
        assert_eq!(drop_decision(&masked, match_all), DropDecision::Ignore);
    }

    #[test]
    fn drop_urls_prefers_mime_urls_over_text() {
        let urls = vec!["file:///a".to_string(), "file:///b".to_string()];
        assert_eq!(drop_urls(true, &urls, "https://e/1", |_| true), urls);
        // 无 URL 时只留能 match 的行，且已 strip。
        assert_eq!(
            drop_urls(false, &urls, " https://e/1 \nhttps://e/2\n", |line| {
                line.ends_with('1')
            }),
            vec!["https://e/1".to_string()]
        );
        // 无 URL 也无匹配行 → 空（`if urls:` 为假，不会打开草稿框）。
        assert!(drop_urls(false, &urls, "junk", |_| false).is_empty());
    }

    #[test]
    fn close_action_matches_python_branches() {
        // 可见模态框优先。
        assert_eq!(
            close_action(PLATFORM_MACOS, MODE_QUIT, true, true),
            CloseAction::RejectTopDialog
        );
        // macOS 全屏。
        assert_eq!(
            close_action(PLATFORM_MACOS, MODE_QUIT, false, true),
            CloseAction::LeaveFullScreen
        );
        // macOS 非退出模式 → 留托盘。
        assert_eq!(
            close_action(PLATFORM_MACOS, MODE_ASK, false, false),
            CloseAction::HideToTray
        );
        assert_eq!(
            close_action(PLATFORM_MACOS, MODE_QUIT, false, false),
            CloseAction::CloseAndQuit
        );
        // 非 macOS：ASK 弹框、QUIT 退出、其余留后台。
        assert_eq!(
            close_action(PLATFORM_LINUX, MODE_ASK, false, false),
            CloseAction::Ask
        );
        assert_eq!(
            close_action(PLATFORM_WINDOWS, MODE_ASK, false, true),
            CloseAction::Ask
        );
        assert_eq!(
            close_action(PLATFORM_LINUX, MODE_QUIT, false, false),
            CloseAction::CloseAndQuit
        );
        assert_eq!(
            close_action(PLATFORM_LINUX, MODE_BACKGROUND, false, false),
            CloseAction::Close
        );
    }

    #[test]
    fn resolve_ask_returns_the_mode_to_remember() {
        let quit = resolve_ask(true, true);
        assert_eq!(quit.action, CloseAction::CloseAndQuit);
        assert_eq!(quit.remember, Some(MODE_QUIT));
        let background = resolve_ask(false, true);
        assert_eq!(background.action, CloseAction::Close);
        assert_eq!(background.remember, Some(MODE_BACKGROUND));
        // 没勾「记住」→ 不落盘。
        assert_eq!(resolve_ask(false, false).remember, None);
    }

    #[test]
    fn pair_request_text_falls_back_on_falsy_fields() {
        assert_eq!(
            pair_request_text("127.0.0.1", Some("chrome"), Some("1.2.3")),
            "浏览器扩展正在请求连接到 Ghost Downloader。\n\n\
             来源: 127.0.0.1\n客户端: chrome\n扩展版本: 1.2.3\n\n\
             仅在你刚刚点击扩展里的\"自动配对\"时允许。"
        );
        // `or` 是假值判断：None 与空串都兜底。
        let fallback = pair_request_text("::1", None, Some(""));
        assert!(fallback.contains("客户端: 浏览器扩展\n"));
        assert!(fallback.contains("扩展版本: 未知\n"));
    }

    #[test]
    fn notification_texts_match_python_templates() {
        assert_eq!(update_available_content("3.2.0"), "最新版本: 3.2.0");
        assert_eq!(
            exception_text("boom"),
            "点击\"确定\"后将复制错误信息并打开反馈页面。\n\nboom"
        );
    }

    #[test]
    fn progress_toast_only_for_app_target_and_three_states() {
        assert!(wants_progress_toast("app", "DOWNLOADING"));
        assert!(wants_progress_toast("app", "READY"));
        assert!(wants_progress_toast("app", "FAILED"));
        assert!(!wants_progress_toast("app", "IDLE"));
        assert!(!wants_progress_toast("app", "CHECKING"));
        assert!(!wants_progress_toast("app", "AVAILABLE"));
        // 非 app 目标一律忽略。
        assert!(!wants_progress_toast("pack", "READY"));
    }

    // -----------------------------------------------------------------
    // 金文件逐条比对（`tests/fixtures/window.jsonl`，由
    // `tests/fixtures/generate_window_fixture.py` 跑真 Python 方法生成）
    // -----------------------------------------------------------------

    fn fixture_cases() -> Vec<serde_json::Value> {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/window.jsonl");
        let raw = std::fs::read_to_string(&path).expect("window.jsonl 应存在");
        raw.split('\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_str(line).expect("每行一个 JSON 对象"))
            .collect()
    }

    fn strings(value: &serde_json::Value) -> Vec<&str> {
        value
            .as_array()
            .expect("数组")
            .iter()
            .map(|item| item.as_str().expect("字符串"))
            .collect()
    }

    #[test]
    fn matches_python_main_window_fixture() {
        let cases = fixture_cases();
        let (mut nav, mut search, mut geometry) = (0, 0, 0);
        let (mut drop, mut close, mut pair) = (0, 0, 0);
        let (mut listen, mut update, mut exception, mut changed) = (0, 0, 0, 0);

        for case in &cases {
            match case["kind"].as_str().expect("kind") {
                "nav" => {
                    nav += 1;
                    let items = case["items"].as_array().expect("items");
                    assert_eq!(items.len(), NAV_ITEMS.len());
                    for (item, expected) in items.iter().zip(NAV_ITEMS.iter()) {
                        assert_eq!(item["routeKey"].as_str().unwrap(), expected.route_key);
                        assert_eq!(item["text"].as_str().unwrap(), expected.title);
                        assert_eq!(item["icon"].as_str().unwrap(), expected.icon);
                        let position = match expected.position {
                            NavPosition::Top => "TOP",
                            NavPosition::Bottom => "BOTTOM",
                        };
                        assert_eq!(item["position"].as_str().unwrap(), position);
                        assert_eq!(item["selectable"].as_bool().unwrap(), expected.selectable);
                        // 三个导航项都挂了 onClick（页面切页 / 新建任务弹草稿）。
                        assert!(item["hasOnClick"].as_bool().unwrap());
                    }
                    assert_eq!(strings(&case["shown"]), vec![PAGE_TASK]);
                }
                "search" => {
                    search += 1;
                    let route = case["routeKey"].as_str().unwrap();
                    let target = search_target(route);
                    if route == "PlaceholderOnly" {
                        // 探针页面：Python 靠 `hasattr` 判定，两个 flag 互相独立。
                        // Rust 侧按 routeKey 查表，pack 页面一律按「没有」处理，
                        // 所以这里只钉住「Python 里两个 flag 确实独立」这一事实。
                        assert!(case["hasPlaceholder"].as_bool().unwrap());
                        assert!(!case["hasSetSearchText"].as_bool().unwrap());
                        assert!(target.placeholder.is_none());
                        continue;
                    }
                    assert_eq!(target.placeholder, case["placeholder"].as_str());
                    assert_eq!(
                        target.accepts_text,
                        case["hasSetSearchText"].as_bool().unwrap()
                    );
                    let expected = match target.placeholder {
                        Some(text) => vec![
                            "clear".to_string(),
                            format!("placeholder:{text}"),
                            "show".to_string(),
                        ],
                        None => vec!["clear".to_string(), "hide".to_string()],
                    };
                    assert_eq!(strings(&case["log"]), expected);
                }
                "geometry" => {
                    geometry += 1;
                    let got = search_edit_geometry(
                        case["titleBarWidth"].as_i64().unwrap() as i32,
                        case["titleBarHeight"].as_i64().unwrap() as i32,
                        case["editHeight"].as_i64().unwrap() as i32,
                    );
                    assert_eq!(got.width, case["width"].as_i64().unwrap() as i32);
                    assert_eq!(got.x, case["x"].as_i64().unwrap() as i32);
                    assert_eq!(got.y, case["y"].as_i64().unwrap() as i32);
                    // 真方法只做 setFixedWidth + move 两步。
                    let log = case["log"].as_array().unwrap();
                    assert_eq!(log.len(), 2);
                    assert_eq!(log[0][0].as_str().unwrap(), "width");
                    assert_eq!(log[1][0].as_str().unwrap(), "move");
                }
                "drop" => {
                    drop += 1;
                    let input = DropInput {
                        is_file_drag_active: case["isFileDragActive"].as_bool().unwrap(),
                        has_urls: case["hasUrls"].as_bool().unwrap(),
                        has_text: case["hasText"].as_bool().unwrap(),
                        text: case["text"].as_str().unwrap(),
                        mask_dialog_visible: case["maskDialogVisible"].as_bool().unwrap(),
                    };
                    let matches = |line: &str| line.starts_with("http");
                    let decision = drop_decision(&input, matches);
                    let expected_enter = match decision {
                        DropDecision::Accept => vec!["accept"],
                        DropDecision::Ignore => vec!["ignore"],
                    };
                    assert_eq!(strings(&case["enterActions"]), expected_enter);

                    // 遮罩层：接住时 setGeometry/show/raise_，随后 dropEvent 隐藏。
                    let expected_overlay = match decision {
                        DropDecision::Accept => {
                            vec!["geometry", "show", "raise", "hide"]
                        }
                        DropDecision::Ignore => vec!["hide"],
                    };
                    assert_eq!(strings(&case["overlayLog"]), expected_overlay);

                    let urls: Vec<String> = case["urls"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|url| url.as_str().unwrap().to_string())
                        .collect();
                    let expected_urls = drop_urls(input.has_urls, &urls, input.text, matches);
                    let expected_added: Vec<Vec<String>> = if expected_urls.is_empty() {
                        Vec::new()
                    } else {
                        vec![expected_urls]
                    };
                    let added: Vec<Vec<String>> = case["added"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|call| {
                            call.as_array()
                                .unwrap()
                                .iter()
                                .map(|url| url.as_str().unwrap().to_string())
                                .collect()
                        })
                        .collect();
                    assert_eq!(added, expected_added);
                }
                "close" => {
                    close += 1;
                    let platform = case["platform"].as_str().unwrap();
                    let mode = case["mode"].as_str().unwrap();
                    let action = close_action(
                        platform,
                        mode,
                        case["dialogVisible"].as_bool().unwrap(),
                        case["fullScreen"].as_bool().unwrap(),
                    );
                    let mut actions: Vec<&str> = Vec::new();
                    let mut rejects = 0;
                    let mut quits = 0;
                    let mut timers: Vec<i64> = Vec::new();
                    let mut geometry_sets = 0;
                    let mut close_mode_sets = 0;
                    let mut expected_mode: Option<&str> = None;
                    let mut message: Option<[&str; 5]> = None;

                    match action {
                        CloseAction::RejectTopDialog => rejects += 1,
                        CloseAction::LeaveFullScreen => {
                            actions.push("showNormal");
                            timers.push(1000);
                        }
                        CloseAction::HideToTray => {
                            actions.push("hide");
                            geometry_sets += 1;
                        }
                        CloseAction::Ask => {
                            let outcome = resolve_ask(
                                case["exec"].as_bool().unwrap(),
                                case["remember"].as_bool().unwrap(),
                            );
                            actions.push("close");
                            if outcome.action == CloseAction::CloseAndQuit {
                                quits += 1;
                            }
                            if let Some(remember) = outcome.remember {
                                close_mode_sets += 1;
                                expected_mode = Some(remember);
                            }
                            message = Some([
                                CLOSE_ASK_TITLE,
                                CLOSE_ASK_CONTENT,
                                CLOSE_ASK_YES,
                                CLOSE_ASK_NO,
                                CLOSE_ASK_REMEMBER,
                            ]);
                        }
                        CloseAction::Close => actions.push("close"),
                        CloseAction::CloseAndQuit => {
                            actions.push("close");
                            quits += 1;
                        }
                    }

                    assert_eq!(strings(&case["actions"]), actions);
                    assert_eq!(case["rejects"].as_array().unwrap().len(), rejects);
                    assert_eq!(case["quits"].as_i64().unwrap() as i32, quits);
                    let delays: Vec<i64> = case["timerDelays"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|delay| delay.as_i64().unwrap())
                        .collect();
                    assert_eq!(delays, timers);

                    // `cfg.set` 的两类键：几何（调用方管）与 closeMode（这里管）。
                    let remembered = case["remembered"].as_array().unwrap();
                    assert_eq!(
                        remembered.len(),
                        geometry_sets + close_mode_sets,
                        "remembered = {remembered:?}"
                    );
                    let mut seen_close_mode = 0;
                    for entry in remembered {
                        match entry[0].as_str().unwrap() {
                            "geometry" => {}
                            "closeMode" => {
                                seen_close_mode += 1;
                                assert_eq!(
                                    entry[1].as_str().unwrap(),
                                    expected_mode.expect("closeMode 只在 ASK 分支写")
                                );
                            }
                            other => panic!("意外的配置键 {other}"),
                        }
                    }
                    assert_eq!(seen_close_mode, close_mode_sets);

                    match (&case["message"], message) {
                        (serde_json::Value::Null, None) => {}
                        (serde_json::Value::Object(fields), Some(texts)) => {
                            assert_eq!(fields["title"].as_str().unwrap(), texts[0]);
                            assert_eq!(fields["content"].as_str().unwrap(), texts[1]);
                            assert_eq!(fields["yes"].as_str().unwrap(), texts[2]);
                            assert_eq!(fields["no"].as_str().unwrap(), texts[3]);
                            assert_eq!(fields["checkbox"].as_str().unwrap(), texts[4]);
                        }
                        (other, expected) => {
                            panic!("message 不匹配：{other:?} vs {expected:?}")
                        }
                    }
                }
                "pair" => {
                    pair += 1;
                    let exec = case["exec"].as_bool().unwrap();
                    assert_eq!(case["title"].as_str().unwrap(), PAIR_REQUEST_TITLE);
                    assert_eq!(case["allow"].as_str().unwrap(), PAIR_REQUEST_ALLOW);
                    assert_eq!(case["deny"].as_str().unwrap(), PAIR_REQUEST_DENY);
                    assert_eq!(
                        pair_request_text(
                            case["peerAddress"].as_str().unwrap(),
                            case["clientKind"].as_str(),
                            case["extensionVersion"].as_str(),
                        ),
                        case["content"].as_str().unwrap()
                    );
                    // `dialog.exec()` 为真 → approve，否则 reject。
                    let expected = if exec {
                        "approve:req-1"
                    } else {
                        "reject:req-1"
                    };
                    assert_eq!(strings(&case["log"]), vec![expected]);
                }
                "listen-failed" => {
                    listen += 1;
                    let title = match case["target"].as_str().unwrap() {
                        "browser" => LISTEN_FAILED_BROWSER_TITLE,
                        "aria2" => LISTEN_FAILED_ARIA2_TITLE,
                        other => panic!("未知监听目标 {other}"),
                    };
                    assert_eq!(case["title"].as_str().unwrap(), title);
                    assert_eq!(
                        strings(&case["buttonTexts"]),
                        vec![LISTEN_FAILED_SETTINGS_BUTTON]
                    );
                    assert_eq!(case["shown"].as_i64().unwrap(), 1);
                }
                "update-available" => {
                    update += 1;
                    assert_eq!(case["title"].as_str().unwrap(), UPDATE_AVAILABLE_TITLE);
                    assert_eq!(
                        case["content"].as_str().unwrap(),
                        update_available_content(case["latestVersion"].as_str().unwrap())
                    );
                    assert_eq!(
                        strings(&case["buttonTexts"]),
                        vec![
                            UPDATE_DOWNLOAD_BUTTON,
                            UPDATE_DETAIL_BUTTON,
                            UPDATE_SPONSOR_BUTTON
                        ]
                    );
                    // 三个平台都先清掉旧的进度提示再弹 InfoBar。
                    assert!(case["toastCleared"].as_bool().unwrap());
                    assert_eq!(case["shown"].as_i64().unwrap(), 1);
                }
                "exception" => {
                    exception += 1;
                    let message = case["message"].as_str().unwrap();
                    let exec = case["exec"].as_bool().unwrap();
                    assert_eq!(case["title"].as_str().unwrap(), EXCEPTION_TITLE);
                    assert_eq!(case["content"].as_str().unwrap(), exception_text(message));
                    assert_eq!(case["logTooltip"].as_str().unwrap(), EXCEPTION_LOG_TOOLTIP);
                    // 把 titleLabel 摘出来、塞进带「查看日志」按钮的一行。
                    assert_eq!(strings(&case["layoutOps"]), vec!["remove", "insert"]);
                    let clipboard = strings(&case["clipboard"]);
                    let opened = strings(&case["opened"]);
                    if exec {
                        assert_eq!(clipboard, vec![message]);
                        assert_eq!(opened.len(), 1);
                    } else {
                        assert!(clipboard.is_empty());
                        assert!(opened.is_empty());
                    }
                }
                "update-changed" => {
                    changed += 1;
                    let want = wants_progress_toast(
                        case["targetId"].as_str().unwrap(),
                        case["state"].as_str().unwrap(),
                    );
                    let expected = if want { 1 } else { 0 };
                    assert_eq!(case["created"].as_i64().unwrap(), expected);
                    assert_eq!(case["updated"].as_i64().unwrap(), expected);
                }
                other => panic!("未知用例类型 {other}"),
            }
        }

        assert_eq!(cases.len(), 119);
        assert_eq!((nav, search, geometry), (1, 4, 9));
        assert_eq!((drop, close, pair), (48, 26, 10));
        assert_eq!((listen, update, exception, changed), (2, 3, 4, 12));
    }

    #[test]
    fn mode_literals_share_close_mode_options() {
        assert_eq!([MODE_ASK, MODE_BACKGROUND, MODE_QUIT], close_mode::OPTIONS);
    }
}
