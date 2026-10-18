//! 组件库（P4，第 69 步）：镜像 `app/view/components/install_button.py`（72 行）
//! 的 [`InstallButton`] 状态机。
//!
//! ## 实证口径（2026-10-18 实读源码）
//!
//! - 两个指示器都是 16×16、`setStrokeWidth(2)`、`WA_TransparentForMouseEvents`、
//!   **初始隐藏**：`ring = ProgressRing(useAni=False)`、`spinner =
//!   IndeterminateProgressRing(start=False)`。
//! - `setInstalling(isInstalling)`：**状态相同就 return**（幂等，不重复改尺寸）；
//!   进入安装态时 `setFixedWidth(sizeHint().width())` + `setText("")`（用宽度
//!   锁住按钮，文字让位给指示器）；退出时 `setMinimumWidth(0)`、
//!   `setMaximumWidth(QWIDGETSIZE_MAX)`、`ring.hide()`、`spinner.stop()`、
//!   `spinner.hide()`。**退出时不恢复文字**（源码没有这一步）。
//! - `setProgress(progress)`：`isDownloading = 0 < progress < 100`；
//!   `ring.setValue(progress)`；`ring.setVisible(isDownloading)`；下载中则
//!   `spinner.stop()+hide()`，否则**仅当 spinner 已隐藏**才 `show()+start()`
//!   （已可见就不重复启动）。
//! - `paintEvent`：安装态时在按钮中心画 6×6、圆角 1.5 的方块（深色主题黑、
//!   浅色主题白）——即「停止」图标。
//!
//! ## 与 Qt 的差异
//!
//! `sizeHint().width()` 与按钮自身 `width()` 是布局实测值，本模块把
//! `size_hint_width` 当入参；`isDarkTheme()` 当布尔入参。

/// `RING_SIZE = 16`
pub const RING_SIZE: i32 = 16;
/// `STOP_SIZE = 6`
pub const STOP_SIZE: i32 = 6;
/// `QWIDGETSIZE_MAX = (1 << 24) - 1`
pub const QWIDGETSIZE_MAX: i32 = (1 << 24) - 1;
/// `ring.setStrokeWidth(2)`
pub const RING_STROKE_WIDTH: i32 = 2;
/// `drawRoundedRect(rect, 1.5, 1.5)`
pub const STOP_RADIUS: f64 = 1.5;

/// 停止方块的填充色（`QColor(0, 0, 0) if isDarkTheme() else QColor(255, 255, 255)`）。
pub fn stop_color(dark: bool) -> (u8, u8, u8) {
    if dark { (0, 0, 0) } else { (255, 255, 255) }
}

/// 方块居中矩形：`(x, y, STOP_SIZE, STOP_SIZE)`。
pub fn stop_rect(width: i32, height: i32) -> (f64, f64, i32, i32) {
    (
        (width - STOP_SIZE) as f64 / 2.0,
        (height - STOP_SIZE) as f64 / 2.0,
        STOP_SIZE,
        STOP_SIZE,
    )
}

/// 按钮状态。字段即「外壳要同步给 Slint 的属性」。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallButtonState {
    installing: bool,
    progress: i32,
    ring_visible: bool,
    spinner_visible: bool,
    spinner_running: bool,
    /// 安装态锁定的宽度（`setFixedWidth(sizeHint().width())`）
    locked_width: Option<i32>,
    min_width: i32,
    max_width: i32,
    text: String,
}

impl InstallButtonState {
    /// `__init__` + `_initWidget`：两个指示器初始隐藏，宽度不受限。
    pub fn new(text: &str) -> Self {
        Self {
            installing: false,
            progress: 0,
            ring_visible: false,
            spinner_visible: false,
            spinner_running: false,
            locked_width: None,
            min_width: 0,
            max_width: QWIDGETSIZE_MAX,
            text: text.to_string(),
        }
    }

    /// `setInstalling(isInstalling)`；`size_hint_width` = `sizeHint().width()`。
    pub fn set_installing(&mut self, installing: bool, size_hint_width: i32) {
        if installing == self.installing {
            return;
        }
        self.installing = installing;
        if installing {
            self.locked_width = Some(size_hint_width);
            self.min_width = size_hint_width;
            self.max_width = size_hint_width;
            self.text.clear();
            return;
        }
        self.locked_width = None;
        self.min_width = 0;
        self.max_width = QWIDGETSIZE_MAX;
        self.ring_visible = false;
        self.spinner_running = false;
        self.spinner_visible = false;
    }

    /// `setProgress(progress)`（不碰 `installing`）。
    pub fn set_progress(&mut self, progress: i32) {
        self.progress = progress;
        let downloading = 0 < progress && progress < 100;
        self.ring_visible = downloading;
        if downloading {
            self.spinner_running = false;
            self.spinner_visible = false;
        } else if !self.spinner_visible {
            self.spinner_visible = true;
            self.spinner_running = true;
        }
    }

    pub fn is_installing(&self) -> bool {
        self.installing
    }

    pub fn progress(&self) -> i32 {
        self.progress
    }

    pub fn ring_visible(&self) -> bool {
        self.ring_visible
    }

    pub fn spinner_visible(&self) -> bool {
        self.spinner_visible
    }

    pub fn spinner_running(&self) -> bool {
        self.spinner_running
    }

    pub fn locked_width(&self) -> Option<i32> {
        self.locked_width
    }

    pub fn min_width(&self) -> i32 {
        self.min_width
    }

    pub fn max_width(&self) -> i32 {
        self.max_width
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// 安装态才画停止方块（`paintEvent` 的 `if not self._isInstalling: return`）。
    pub fn draws_stop_square(&self) -> bool {
        self.installing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_python() {
        assert_eq!((RING_SIZE, STOP_SIZE), (16, 6));
        assert_eq!(QWIDGETSIZE_MAX, (1 << 24) - 1);
        assert_eq!(RING_STROKE_WIDTH, 2);
        assert_eq!(STOP_RADIUS, 1.5);
    }

    /// 初始态：文字在、指示器都隐藏、宽度不限。
    #[test]
    fn initial_state_hides_indicators() {
        let state = InstallButtonState::new("安装");
        assert_eq!(state.text(), "安装");
        assert!(!state.ring_visible());
        assert!(!state.spinner_visible());
        assert!(!state.spinner_running());
        assert_eq!(state.locked_width(), None);
        assert_eq!(state.min_width(), 0);
        assert_eq!(state.max_width(), QWIDGETSIZE_MAX);
        assert!(!state.draws_stop_square());
    }

    /// 进入安装态锁宽度 + 清文字；退出时解锁并清指示器，**文字不回来**。
    #[test]
    fn installing_locks_width_and_clears_text() {
        let mut state = InstallButtonState::new("安装");
        state.set_installing(true, 96);
        assert!(state.is_installing());
        assert_eq!(state.locked_width(), Some(96));
        assert_eq!((state.min_width(), state.max_width()), (96, 96));
        assert_eq!(state.text(), "");
        assert!(state.draws_stop_square());

        state.set_progress(50);
        assert!(state.ring_visible());
        state.set_installing(false, 96);
        assert!(!state.is_installing());
        assert_eq!(state.locked_width(), None);
        assert_eq!((state.min_width(), state.max_width()), (0, QWIDGETSIZE_MAX));
        assert!(!state.ring_visible());
        assert!(!state.spinner_visible());
        assert!(!state.spinner_running());
        assert_eq!(state.text(), "");
        assert!(!state.draws_stop_square());
    }

    /// 重复设同一状态是空操作（Python 的 `if isInstalling == self._isInstalling: return`）。
    #[test]
    fn repeated_set_installing_is_a_noop() {
        let mut state = InstallButtonState::new("安装");
        state.set_installing(true, 96);
        state.set_progress(30);
        // 再次「进入」不该把已经动起来的指示器重置。
        state.set_installing(true, 200);
        assert_eq!(state.locked_width(), Some(96));
        assert!(state.ring_visible());

        // 未进入过就「退出」也是空操作。
        let mut fresh = InstallButtonState::new("安装");
        fresh.set_installing(false, 96);
        assert_eq!(fresh.text(), "安装");
        assert_eq!(fresh.max_width(), QWIDGETSIZE_MAX);
    }

    /// 0 与 100 都算「非下载中」→ 显示并启动 spinner。
    #[test]
    fn progress_bounds_toggle_ring_and_spinner() {
        let mut state = InstallButtonState::new("安装");
        state.set_progress(0);
        assert!(!state.ring_visible());
        assert!(state.spinner_visible());
        assert!(state.spinner_running());

        state.set_progress(100);
        assert!(!state.ring_visible());
        // spinner 已经可见 → 不重复 show/start，状态保持。
        assert!(state.spinner_visible());
        assert!(state.spinner_running());

        state.set_progress(1);
        assert!(state.ring_visible());
        assert!(!state.spinner_visible());
        assert!(!state.spinner_running());

        state.set_progress(99);
        assert!(state.ring_visible());
        assert!(!state.spinner_visible());
    }

    /// 退出安装态把 spinner 关掉后，再设进度会重新 show+start。
    #[test]
    fn progress_restarts_spinner_after_leaving_install_state() {
        let mut state = InstallButtonState::new("安装");
        state.set_installing(true, 96);
        state.set_progress(50);
        state.set_installing(false, 96);
        assert!(!state.spinner_visible());
        state.set_progress(0);
        assert!(state.spinner_visible());
        assert!(state.spinner_running());
    }

    /// 停止方块：6×6 居中，颜色随主题。
    #[test]
    fn stop_square_geometry_and_color() {
        assert_eq!(stop_rect(96, 32), (45.0, 13.0, 6, 6));
        assert_eq!(stop_rect(7, 7), (0.5, 0.5, 6, 6));
        assert_eq!(stop_color(true), (0, 0, 0));
        assert_eq!(stop_color(false), (255, 255, 255));
    }
}
