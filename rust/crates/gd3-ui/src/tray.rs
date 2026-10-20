//! 托盘（第 136 步）：把 [`crate::shell`] 的托盘语义接到 Slint 的
//! `SystemTrayIcon` 上。
//!
//! Slint 1.18 自带 `SystemTrayIcon`（Linux 走 `ksni` = StatusNotifierItem，
//! macOS 走 `NSStatusItem`，Windows 走 `Shell_NotifyIconW`），所以 Phase 8f
//! **不需要降级方案**了（旧笔记里的「留 TODO + 记录平台 API 方案」已作废）。
//! 未接的只是像素级还原：`tray.py` 里那三个自绘菜单类
//! （`AcrylicMenu` / `TrayRoundMenu` / `RoundMenu`）连同 `adjustPosition`
//! 的坐标钳制、亚克力底色、`itemHeight`/`iconSize` 都归桌面环境，不再复制。
//!
//! # 分工
//!
//! | 层 | 位置 | 内容 |
//! |---|---|---|
//! | 文案与顺序 | [`crate::shell::tray_menu`] | 五项（仪表盘/全部开始/全部暂停/分隔线/退出程序） |
//! | 气泡文案 | [`crate::shell::tray_tooltip`] | `speed > 0` 才带 `↓ …/s` |
//! | 装配 | `ui/system_tray.slint` | `Gd3TrayIcon inherits SystemTrayIcon` + 五项菜单 |
//! | 接线 | 本模块 | 四个回调 → 窗口 / 运行期 / 退出 |
//!
//! 菜单文案的漂移有测试盯着（[`tests::the_slint_menu_matches_the_semantic_layer`]）：
//! 改 `shell.rs` 却没改 `.slint` 会直接红。
//!
//! # 与 Qt 侧的差异
//!
//! - `QSystemTrayIcon.showMessage`（`messageClicked` → 激活）没有对应物：
//!   Slint 的托盘只有 `clicked`，通知气泡归桌面环境。
//! - `ActivationReason` 只剩「左键 = `clicked`」这一路；中键/双击/右键菜单在
//!   Slint 里各自没有回调（右键固定弹菜单）。
//! - `_refreshMenuItems` 的启用/禁用（macOS 状态项那套，`PAUSED/FAILED/WAITING`
//!   才亮「全部开始」）**没做**：`tray.py` 的 `QMenu` 本来就不改 `enabled`，
//!   而 Slint 的菜单项要逐项绑 `enabled` 就得把任务状态推进 UI —— 那属于
//!   `status_menu` 的活，不在本步范围内。

use std::cell::RefCell;
use std::rc::Rc;

use crate::AppWindow;
use crate::engine::Engine;
use crate::shell::{self, TraySignal};
use crate::task_runner::TaskRunner;
use slint::ComponentHandle;

mod generated {
    // 生成的代码带自己的 `allow`，这里再兜一层：`.slint` 产物不参与 lint 口径。
    #![allow(clippy::all, unused_imports, dead_code)]
    include!(concat!(env!("OUT_DIR"), "/system_tray.rs"));
}

pub use generated::Gd3TrayIcon;

/// 一次托盘动作的产物（`boot` 打日志、测试断言都靠它）。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TrayOutcome {
    /// `全部开始` 派发出去的任务（[`crate::runtime::start_all`]）。
    pub started: Vec<String>,
    /// `全部暂停` 停下来的任务（[`crate::runtime::pause_all`]）。
    pub paused: Vec<String>,
    /// 是否请求了「显示主窗口」（仪表盘 / 左键单击）。
    pub revealed: bool,
    /// 是否请求了退出事件循环（`QApplication.quit`）。
    pub quit: bool,
}

/// 把一个托盘信号落到具体动作上（不碰 Slint 组件的装配部分）。
///
/// `quit` 这一步在测试里也会被调到：`slint::quit_event_loop()` 在没有事件循环
/// 时返回 `Err(NoEventLoop)`，这里按「已经记下退出意图」处理，不向上抛。
pub fn dispatch(
    signal: TraySignal,
    window: &AppWindow,
    engine: &Rc<RefCell<Engine>>,
    runner: &Rc<RefCell<TaskRunner>>,
) -> TrayOutcome {
    let mut outcome = TrayOutcome::default();
    match signal {
        TraySignal::ActivationRequested => {
            outcome.revealed = window.show().is_ok();
        }
        TraySignal::StartAll => {
            outcome.started = crate::runtime::start_all(engine, runner).started;
        }
        TraySignal::PauseAll => {
            outcome.paused = crate::runtime::pause_all(engine, runner).paused;
        }
        TraySignal::Quit => {
            outcome.quit = true;
            let _ = slint::quit_event_loop();
        }
    }
    outcome
}

/// 装配托盘并把四个回调接上。
///
/// 只在**有头**分支调用：无头冒烟没有事件循环，托盘的 `ksni` 服务循环也起不来。
/// 返回的组件必须活到事件循环结束（`Drop` 会把图标从托盘里摘掉）。
pub fn install(
    window: &AppWindow,
    engine: Rc<RefCell<Engine>>,
    runner: Rc<RefCell<TaskRunner>>,
) -> Result<Gd3TrayIcon, slint::PlatformError> {
    let tray = Gd3TrayIcon::new()?;
    tray.set_tooltip_text(shell::tray_tooltip(0).as_str().into());

    // 「仪表盘」与左键单击是同一个信号。左键那一路不需要在这里接：`.slint` 里
    // `clicked => { root.dashboard-clicked(); }` 已经把 `SystemTrayIcon.clicked`
    // （Qt 的 `ActivationReason.Trigger`）转成了 `dashboard-clicked`，与
    // `shell::tray_activated` 的「只有 Trigger 才算激活」一致。
    let window_weak = window.as_weak();
    tray.on_dashboard_clicked(move || {
        if let Some(window) = window_weak.upgrade() {
            let _ = window.show();
        }
    });

    let start_engine = Rc::clone(&engine);
    let start_runner = Rc::clone(&runner);
    tray.on_start_all_clicked(move || {
        crate::runtime::start_all(&start_engine, &start_runner);
    });

    let pause_engine = Rc::clone(&engine);
    let pause_runner = Rc::clone(&runner);
    tray.on_pause_all_clicked(move || {
        crate::runtime::pause_all(&pause_engine, &pause_runner);
    });

    // `QApplication.instance().quit`：Slint 侧就是结束事件循环。这里不做落盘/停机
    // 收尾 —— 那些在 `boot::run` 的停机序列里，事件循环返回之后照跑一遍（同 Qt 的
    // `aboutToQuit`）。
    tray.on_quit_clicked(|| {
        let _ = slint::quit_event_loop();
    });

    Ok(tray)
}

/// 速度表变化时刷新提示气泡（`tray.py:201-205`）。
///
/// `speed == None` 表示这一 tick 速度表没发数（无头 `--frames`、或者 1 秒还没到），
/// 这时保持上一次的文案 —— Python 那边也是只在 `speedChanged` 时才改。
pub fn apply_tooltip(tray: &Gd3TrayIcon, speed: Option<i64>) {
    if let Some(speed) = speed {
        tray.set_tooltip_text(shell::tray_tooltip(speed).as_str().into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::shell::{TrayActionKind, TrayMenuEntry};

    /// `.slint` 里的菜单项（顺序即出现顺序）。
    #[derive(Debug, PartialEq, Eq)]
    enum SlintEntry {
        Item(String),
        Separator,
    }

    fn slint_menu() -> Vec<SlintEntry> {
        let source = include_str!("../ui/system_tray.slint");
        let mut entries = Vec::new();
        // 只认 `MenuItem` 块里的 `title:` —— 组件顶部还有一行 `title: "…";`
        // （`SystemTrayIcon.title`），不能被算进菜单。
        let mut in_item = false;
        for line in source.lines() {
            let line = line.trim();
            if line.starts_with("MenuSeparator") {
                entries.push(SlintEntry::Separator);
            } else if line.starts_with("MenuItem") {
                in_item = true;
            } else if in_item && let Some(rest) = line.strip_prefix("title:") {
                let text = rest.trim().trim_end_matches(';').trim();
                entries.push(SlintEntry::Item(text.trim_matches('"').to_string()));
                in_item = false;
            }
        }
        entries
    }

    /// 改了 `shell.rs` 的菜单却没改 `.slint`（或反过来）就会红。
    #[test]
    fn the_slint_menu_matches_the_semantic_layer() {
        let expected: Vec<SlintEntry> = shell::tray_menu()
            .iter()
            .map(|entry: &TrayMenuEntry| match entry.kind {
                TrayActionKind::Separator => SlintEntry::Separator,
                _ => SlintEntry::Item(entry.text.to_string()),
            })
            .collect();
        assert_eq!(slint_menu(), expected);
    }

    /// 五项菜单都要有回调落点（`tray_action` 给得出信号的都得有人接）。
    #[test]
    fn every_actionable_entry_has_a_signal() {
        for entry in shell::tray_menu() {
            let signal = shell::tray_action(entry.kind);
            assert_eq!(signal.is_none(), entry.kind == TrayActionKind::Separator);
        }
    }

    #[test]
    fn the_tooltip_starts_as_the_plain_name_and_follows_the_speed() {
        let _ = crate::create_headless();
        let tray = Gd3TrayIcon::new().expect("托盘组件能实例化");
        assert_eq!(tray.get_tooltip_text(), shell::TRAY_NAME);

        apply_tooltip(&tray, None);
        assert_eq!(tray.get_tooltip_text(), shell::TRAY_NAME, "没发数就不动");

        apply_tooltip(&tray, Some(1536));
        assert_eq!(tray.get_tooltip_text(), "Ghost Downloader\n↓ 1.50 KB/s");

        apply_tooltip(&tray, Some(0));
        assert_eq!(tray.get_tooltip_text(), shell::TRAY_NAME, "归零就退回名字");
    }

    /// 回调真的接上了：`仪表盘` → 窗口从隐藏变可见；另外三条动作在空任务表上
    /// 也得安静地跑完（`全部开始/暂停` 无事可做，`退出` 没有事件循环时只是
    /// `Err(NoEventLoop)`，不许 panic）。
    #[test]
    fn the_menu_callbacks_are_wired_to_the_window_and_the_runtime() {
        let dir = std::env::temp_dir().join(format!("gd3-ui-tray-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let config =
            gd3_core::settings::default_config_with_download_folder(&dir.to_string_lossy());
        let engine = crate::engine::Engine::new(&config, dir.join("tasks.jsonl"));
        let engine = Rc::new(RefCell::new(engine));
        let runner = Rc::new(RefCell::new(TaskRunner::new(
            crate::task_runner::RunnerSettings::default(),
        )));

        let window = crate::create_headless();
        let _ = window.hide();
        let tray = install(&window, Rc::clone(&engine), Rc::clone(&runner)).expect("建托盘");

        tray.invoke_dashboard_clicked();
        assert!(window.window().is_visible(), "仪表盘要唤出窗口");

        // 空任务表：没有可派发的，也没有可暂停的。
        tray.invoke_start_all_clicked();
        tray.invoke_pause_all_clicked();
        tray.invoke_quit_clicked();

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_tray_component_builds_on_a_headless_platform() {
        // 托盘与窗口一样要平台；`Gd3TrayIcon::new()` 在没有 StatusNotifier
        // watcher 的机器上只是「图标注册失败」（Slint 打一条 debug 日志），
        // 组件本身照样构造成功 —— 所以这里断言的是「不炸」。
        let _ = crate::create_headless();
        // 内建的 `title` / `icon` / `tooltip` 只有 setter 没 getter（生成代码只为
        // 组件自己声明的属性出 getter），所以这里断言导出属性的初值。
        let first = Gd3TrayIcon::new().expect("第一个实例");
        let second = Gd3TrayIcon::new().expect("第二个实例");
        assert_eq!(first.get_tooltip_text(), shell::TRAY_NAME);
        assert_eq!(second.get_tooltip_text(), shell::TRAY_NAME);
    }
}
