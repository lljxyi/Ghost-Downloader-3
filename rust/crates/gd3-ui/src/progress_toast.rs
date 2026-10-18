//! 组件库（P4，第 71 步）：镜像 `app/view/components/progress_toast.py`（159 行）
//! —— 更新进度的 InfoBar：底部 3px 进度条 + 重试/重启按钮。
//!
//! ## 实证口径（2026-10-18 实读源码）
//!
//! - 常量：`BAR_HEIGHT = 3`、`CORNER_RADIUS = 6`、`FILL_EASING = 0.25`、
//!   `_fillTimer = QTimer(self, interval=16)`。
//! - `COLORS` 只有三态（DOWNLOADING / READY / FAILED），各带浅色+深色两色；
//!   `barColor(state)` 查不到就**兜 DOWNLOADING**（`COLORS.get(state, COLORS[DOWNLOADING])`），
//!   再按 `isDarkTheme()` 取。故 `IDLE` / `CHECKING` / `AVAILABLE` 也是下载中配色。
//! - `setInfo`：
//!   - DOWNLOADING：`_isDismissed` 直接 return（用户关掉后不再弹）；否则写
//!     target、下载配色、WARNING 图标、标题「正在下载更新」、
//!     内容 `f"{latestVersion} ({progress:.0f}%)"`、两个按钮都藏、启动填色表。
//!   - READY：`_isDismissed = False`、target=100、READY 配色、SUCCESS 图标、
//!     标题「更新已就绪」、内容「重启后生效」、重试藏、**重启显示**、启动填色表。
//!   - FAILED：`_isDismissed = False`、target=100 且 **display 直接置 100**、
//!     FAILED 配色、ERROR 图标、标题「下载更新失败」、内容 =
//!     `toLocalizedError(error)` 或「未知错误」、**重试显示**、重启藏、**停表**。
//!   - 其余状态直接 return（不改任何字段）。
//!   - 收尾：`_adjustText()` + 不可见则 `show()`；`_displayProgress <= 0` 时
//!     `paintEvent` 直接 return（不画条）。
//! - `_onFillTick`：`diff = target - display`；`abs(diff) < 0.5` 时
//!   `display = target` 并**停表**，否则 `display += diff * 0.25`。
//! - `_onUserClose`：`_isDismissed = True` 后 `close()`；`closeEvent` 停表、
//!   发 `closedSignal`、`hide()`、`e.ignore()`。
//! - 进度条几何：`barW = int(width * display / 100)`，矩形
//!   `(0, height - 3, barW, 3)`，圆角 6 裁剪。
//!
//! ## 与 Qt 的差异
//!
//! `toLocalizedError(info.error)` 属于 i18n 层，本模块收已本地化的字符串
//! （`error_text`）；`isDarkTheme()` 当布尔入参；窗口定位（`InfoBarManager`）
//! 由外壳负责。

use crate::pack_info::UpdateState;

/// `BAR_HEIGHT = 3`
pub const BAR_HEIGHT: i32 = 3;
/// `CORNER_RADIUS = 6`
pub const CORNER_RADIUS: i32 = 6;
/// `FILL_EASING = 0.25`
pub const FILL_EASING: f64 = 0.25;
/// `QTimer(self, interval=16)`
pub const FILL_INTERVAL_MS: u64 = 16;
/// `abs(diff) < 0.5` 即吸附
pub const FILL_SNAP_EPSILON: f64 = 0.5;
/// `_retryButton.setFixedSize(36, 36)` / `setIconSize(QSize(12, 12))`
pub const ACTION_BUTTON_SIZE: (i32, i32) = (36, 36);
pub const ACTION_ICON_SIZE: (i32, i32) = (12, 12);

/// `QColor` 的 RGB 三元组。
pub type Rgb = (u8, u8, u8);

/// `InfoBarIcon`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastIcon {
    Warning,
    Success,
    Error,
}

/// `barColor(state)`：三态各双色，未知状态兜 DOWNLOADING。
pub fn bar_color(state: UpdateState, dark: bool) -> Rgb {
    let (light, dark_color) = match state {
        UpdateState::Ready => ((0x0F, 0x7B, 0x0F), (0x6C, 0xCB, 0x5F)),
        UpdateState::Failed => ((0xC4, 0x2B, 0x1C), (0xFF, 0x99, 0xA4)),
        // DOWNLOADING + COLORS.get 的兜底（IDLE/CHECKING/AVAILABLE）
        _ => ((0x9D, 0x5D, 0x00), (0xFC, 0xE1, 0x00)),
    };
    if dark { dark_color } else { light }
}

/// `app/services/update_service.py::UpdateInfo`（只保留 toast 用到的字段）。
#[derive(Clone, Debug, PartialEq)]
pub struct UpdateInfo {
    pub target_id: String,
    pub label: String,
    pub current_version: String,
    pub latest_version: String,
    pub state: UpdateState,
    pub progress: f64,
    pub error: Option<String>,
}

impl UpdateInfo {
    /// 只填 toast 真正读的字段，其余留空。
    pub fn new(state: UpdateState, latest_version: &str, progress: f64) -> Self {
        Self {
            target_id: String::new(),
            label: String::new(),
            current_version: String::new(),
            latest_version: latest_version.to_string(),
            state,
            progress,
            error: None,
        }
    }
}

/// `ProgressToast` 的可测状态。
#[derive(Clone, Debug, PartialEq)]
pub struct ProgressToastState {
    target_progress: f64,
    display_progress: f64,
    bar_color: Rgb,
    dismissed: bool,
    fill_running: bool,
    icon: ToastIcon,
    title: String,
    content: String,
    retry_visible: bool,
    restart_visible: bool,
    visible: bool,
}

impl ProgressToastState {
    /// `__init__`：空标题/内容、WARNING 图标、下载中配色、两个按钮藏。
    pub fn new(dark: bool) -> Self {
        Self {
            target_progress: 0.0,
            display_progress: 0.0,
            bar_color: bar_color(UpdateState::Downloading, dark),
            dismissed: false,
            fill_running: false,
            icon: ToastIcon::Warning,
            title: String::new(),
            content: String::new(),
            retry_visible: false,
            restart_visible: false,
            visible: false,
        }
    }

    /// `setInfo(info)`；`error_text` = `toLocalizedError(info.error)`。
    /// 返回是否真的应用了这次更新（未处理的状态返回 `false`）。
    pub fn apply(&mut self, info: &UpdateInfo, error_text: Option<&str>, dark: bool) -> bool {
        match info.state {
            UpdateState::Downloading => {
                if self.dismissed {
                    return false;
                }
                self.target_progress = info.progress;
                self.bar_color = bar_color(UpdateState::Downloading, dark);
                self.icon = ToastIcon::Warning;
                self.title = "正在下载更新".to_string();
                self.content = format!("{} ({:.0}%)", info.latest_version, info.progress);
                self.retry_visible = false;
                self.restart_visible = false;
                self.fill_running = true;
            }
            UpdateState::Ready => {
                self.dismissed = false;
                self.target_progress = 100.0;
                self.bar_color = bar_color(UpdateState::Ready, dark);
                self.icon = ToastIcon::Success;
                self.title = "更新已就绪".to_string();
                self.content = "重启后生效".to_string();
                self.retry_visible = false;
                self.restart_visible = true;
                self.fill_running = true;
            }
            UpdateState::Failed => {
                self.dismissed = false;
                self.target_progress = 100.0;
                self.display_progress = 100.0;
                self.bar_color = bar_color(UpdateState::Failed, dark);
                self.icon = ToastIcon::Error;
                self.title = "下载更新失败".to_string();
                self.content = error_text.unwrap_or("未知错误").to_string();
                self.retry_visible = true;
                self.restart_visible = false;
                self.fill_running = false;
            }
            UpdateState::Idle | UpdateState::Checking | UpdateState::Available => return false,
        }
        self.visible = true;
        true
    }

    /// `_onFillTick`：返回定时器是否还在跑。
    pub fn fill_tick(&mut self) -> bool {
        let diff = self.target_progress - self.display_progress;
        if diff.abs() < FILL_SNAP_EPSILON {
            self.display_progress = self.target_progress;
            self.fill_running = false;
        } else {
            self.display_progress += diff * FILL_EASING;
        }
        self.fill_running
    }

    /// `_onUserClose` + `closeEvent`：置 dismissed、停表、隐藏。
    pub fn user_close(&mut self) {
        self.dismissed = true;
        self.fill_running = false;
        self.visible = false;
    }

    /// `paintEvent` 的进度条宽度：`int(width * display / 100)`；
    /// `display <= 0` 不画（`None`）。
    pub fn bar_width(&self, width: i32) -> Option<i32> {
        if self.display_progress <= 0.0 {
            return None;
        }
        Some((width as f64 * self.display_progress / 100.0) as i32)
    }

    /// 进度条的 y 坐标（`height - BAR_HEIGHT`）。
    pub fn bar_top(height: i32) -> i32 {
        height - BAR_HEIGHT
    }

    pub fn target_progress(&self) -> f64 {
        self.target_progress
    }

    pub fn display_progress(&self) -> f64 {
        self.display_progress
    }

    pub fn bar_color(&self) -> Rgb {
        self.bar_color
    }

    pub fn is_dismissed(&self) -> bool {
        self.dismissed
    }

    pub fn is_fill_running(&self) -> bool {
        self.fill_running
    }

    pub fn icon(&self) -> ToastIcon {
        self.icon
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn retry_visible(&self) -> bool {
        self.retry_visible
    }

    pub fn restart_visible(&self) -> bool {
        self.restart_visible
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 常量与两套主题色；未知状态兜 DOWNLOADING。
    #[test]
    fn constants_and_colors() {
        assert_eq!(BAR_HEIGHT, 3);
        assert_eq!(CORNER_RADIUS, 6);
        assert_eq!(FILL_EASING, 0.25);
        assert_eq!(FILL_INTERVAL_MS, 16);
        assert_eq!(ACTION_BUTTON_SIZE, (36, 36));
        assert_eq!(ACTION_ICON_SIZE, (12, 12));

        assert_eq!(
            bar_color(UpdateState::Downloading, false),
            (0x9D, 0x5D, 0x00)
        );
        assert_eq!(
            bar_color(UpdateState::Downloading, true),
            (0xFC, 0xE1, 0x00)
        );
        assert_eq!(bar_color(UpdateState::Ready, false), (0x0F, 0x7B, 0x0F));
        assert_eq!(bar_color(UpdateState::Ready, true), (0x6C, 0xCB, 0x5F));
        assert_eq!(bar_color(UpdateState::Failed, false), (0xC4, 0x2B, 0x1C));
        assert_eq!(bar_color(UpdateState::Failed, true), (0xFF, 0x99, 0xA4));
        for state in [
            UpdateState::Idle,
            UpdateState::Checking,
            UpdateState::Available,
        ] {
            assert_eq!(
                bar_color(state, false),
                bar_color(UpdateState::Downloading, false)
            );
        }
    }

    /// 初始态：空文本、WARNING、下载配色、未显示、未启动填色。
    #[test]
    fn initial_state() {
        let toast = ProgressToastState::new(false);
        assert_eq!(toast.title(), "");
        assert_eq!(toast.content(), "");
        assert_eq!(toast.icon(), ToastIcon::Warning);
        assert_eq!(toast.target_progress(), 0.0);
        assert_eq!(toast.display_progress(), 0.0);
        assert_eq!(toast.bar_color(), (0x9D, 0x5D, 0x00));
        assert!(!toast.is_dismissed());
        assert!(!toast.is_fill_running());
        assert!(!toast.retry_visible());
        assert!(!toast.restart_visible());
        assert!(!toast.is_visible());
    }

    /// DOWNLOADING：标题/内容/配色/按钮/填色表。
    #[test]
    fn apply_downloading() {
        let mut toast = ProgressToastState::new(true);
        let info = UpdateInfo::new(UpdateState::Downloading, "1.2.3", 42.0);
        assert!(toast.apply(&info, None, true));
        assert_eq!(toast.title(), "正在下载更新");
        assert_eq!(toast.content(), "1.2.3 (42%)");
        assert_eq!(toast.target_progress(), 42.0);
        assert_eq!(toast.bar_color(), (0xFC, 0xE1, 0x00));
        assert_eq!(toast.icon(), ToastIcon::Warning);
        assert!(!toast.retry_visible());
        assert!(!toast.restart_visible());
        assert!(toast.is_fill_running());
        assert!(toast.is_visible());
    }

    /// 内容格式 `f"{latest} ({progress:.0f}%)"`：Python 与 Rust 都是半值取偶。
    #[test]
    fn content_rounds_half_to_even() {
        let mut toast = ProgressToastState::new(false);
        let mut info = UpdateInfo::new(UpdateState::Downloading, "9.9", 42.5);
        toast.apply(&info, None, false);
        assert_eq!(toast.content(), "9.9 (42%)");
        info.progress = 43.5;
        toast.apply(&info, None, false);
        assert_eq!(toast.content(), "9.9 (44%)");
        info.progress = 0.4;
        toast.apply(&info, None, false);
        assert_eq!(toast.content(), "9.9 (0%)");
        info.progress = 100.0;
        toast.apply(&info, None, false);
        assert_eq!(toast.content(), "9.9 (100%)");
    }

    /// 用户关闭后 DOWNLOADING 被忽略；READY 会重置 dismissed。
    #[test]
    fn dismissed_blocks_downloading_but_ready_resets() {
        let mut toast = ProgressToastState::new(false);
        toast.apply(
            &UpdateInfo::new(UpdateState::Downloading, "1.0", 10.0),
            None,
            false,
        );
        toast.user_close();
        assert!(toast.is_dismissed());
        assert!(!toast.is_visible());
        assert!(!toast.is_fill_running());

        let before = toast.clone();
        assert!(!toast.apply(
            &UpdateInfo::new(UpdateState::Downloading, "1.0", 55.0),
            None,
            false
        ));
        assert_eq!(toast, before);

        assert!(toast.apply(
            &UpdateInfo::new(UpdateState::Ready, "1.0", 100.0),
            None,
            false
        ));
        assert!(!toast.is_dismissed());
        assert_eq!(toast.title(), "更新已就绪");
    }

    /// READY：target=100、重启按钮显示、SUCCESS 图标。
    #[test]
    fn apply_ready() {
        let mut toast = ProgressToastState::new(false);
        assert!(toast.apply(
            &UpdateInfo::new(UpdateState::Ready, "2.0", 0.0),
            None,
            false
        ));
        assert_eq!(toast.title(), "更新已就绪");
        assert_eq!(toast.content(), "重启后生效");
        assert_eq!(toast.target_progress(), 100.0);
        assert_eq!(toast.bar_color(), (0x0F, 0x7B, 0x0F));
        assert_eq!(toast.icon(), ToastIcon::Success);
        assert!(!toast.retry_visible());
        assert!(toast.restart_visible());
        assert!(toast.is_fill_running());
        assert!(toast.is_visible());
    }

    /// FAILED：display 直接 100、重试显示、停表；错误文案两分支。
    #[test]
    fn apply_failed() {
        let mut toast = ProgressToastState::new(false);
        let mut info = UpdateInfo::new(UpdateState::Failed, "2.0", 0.0);
        info.error = Some("boom".into());
        assert!(toast.apply(&info, Some("下载失败：boom"), false));
        assert_eq!(toast.title(), "下载更新失败");
        assert_eq!(toast.content(), "下载失败：boom");
        assert_eq!(toast.target_progress(), 100.0);
        assert_eq!(toast.display_progress(), 100.0);
        assert_eq!(toast.bar_color(), (0xC4, 0x2B, 0x1C));
        assert_eq!(toast.icon(), ToastIcon::Error);
        assert!(toast.retry_visible());
        assert!(!toast.restart_visible());
        assert!(!toast.is_fill_running());

        // 无 error → 「未知错误」
        let plain = UpdateInfo::new(UpdateState::Failed, "2.0", 0.0);
        assert!(toast.apply(&plain, None, false));
        assert_eq!(toast.content(), "未知错误");
    }

    /// 未处理的状态（IDLE/CHECKING/AVAILABLE）不改任何字段。
    #[test]
    fn other_states_are_ignored() {
        let mut toast = ProgressToastState::new(false);
        let before = toast.clone();
        for state in [
            UpdateState::Idle,
            UpdateState::Checking,
            UpdateState::Available,
        ] {
            assert!(!toast.apply(&UpdateInfo::new(state, "3.0", 50.0), None, false));
        }
        assert_eq!(toast, before);
    }

    /// 填色缓动：`display += diff * 0.25`，`|diff| < 0.5` 吸附并停表。
    #[test]
    fn fill_tick_eases_toward_target() {
        let mut toast = ProgressToastState::new(false);
        toast.apply(
            &UpdateInfo::new(UpdateState::Downloading, "1.0", 50.0),
            None,
            false,
        );
        assert!(toast.fill_tick());
        assert_eq!(toast.display_progress(), 12.5);
        assert!(toast.fill_tick());
        assert_eq!(toast.display_progress(), 21.875);

        // 一路跑到吸附
        let mut guard = 0;
        while toast.fill_tick() {
            guard += 1;
            assert!(guard < 100, "填色未收敛");
        }
        assert_eq!(toast.display_progress(), 50.0);
        assert!(!toast.is_fill_running());
    }

    /// `|diff| < 0.5` 的边界：49.8 → 直接吸附到 50，不再乘缓动。
    #[test]
    fn fill_tick_snaps_within_epsilon() {
        let mut toast = ProgressToastState::new(false);
        toast.apply(
            &UpdateInfo::new(UpdateState::Downloading, "1.0", 50.0),
            None,
            false,
        );
        while toast.display_progress() < 49.8 && toast.fill_tick() {}
        assert!(toast.display_progress() >= 49.8);
        // 再走一步必定进入吸附分支（diff 已 < 0.5）
        let stopped = !toast.fill_tick();
        assert!(stopped || toast.display_progress() == 50.0);
        assert_eq!(toast.display_progress(), 50.0);
        assert!(!toast.is_fill_running());
    }

    /// 进度条：`display <= 0` 不画；否则 `int(width * display / 100)`。
    #[test]
    fn bar_geometry() {
        let mut toast = ProgressToastState::new(false);
        assert_eq!(toast.bar_width(200), None);
        assert_eq!(ProgressToastState::bar_top(60), 57);

        toast.apply(
            &UpdateInfo::new(UpdateState::Downloading, "1.0", 50.0),
            None,
            false,
        );
        while toast.fill_tick() {}
        assert_eq!(toast.bar_width(200), Some(100));
        assert_eq!(toast.bar_width(201), Some(100));

        toast.apply(
            &UpdateInfo::new(UpdateState::Failed, "1.0", 0.0),
            None,
            false,
        );
        assert_eq!(toast.bar_width(200), Some(200));
        assert_eq!(toast.bar_width(150), Some(150));
    }

    /// 用户关闭：dismissed + 停表 + 隐藏。
    #[test]
    fn user_close_sets_dismissed_and_hides() {
        let mut toast = ProgressToastState::new(false);
        toast.apply(
            &UpdateInfo::new(UpdateState::Downloading, "1.0", 10.0),
            None,
            false,
        );
        toast.user_close();
        assert!(toast.is_dismissed());
        assert!(!toast.is_fill_running());
        assert!(!toast.is_visible());
        // 标题/内容保留（closeEvent 只停表+隐藏）
        assert_eq!(toast.title(), "正在下载更新");
    }
}
