//! 平台外壳（第 75 步）：`app/view/shell/` 三个文件的**语义镜像**。
//!
//! 覆盖 `tray.py`(205)、`dock.py`(141)、`mac_status_item.py`(112)。与 P3/P5
//! 同一策略：不画控件、不调 `QSystemTrayIcon` / AppKit，只把「菜单项顺序与
//! 文案、提示气泡文案、激活原因到信号的映射、Windows 菜单定位钳制、Dock 文字
//! 排版与进度条阈值、状态栏菜单项与启用条件、速度标题格式」抽成可断言的纯
//! 函数与状态机——Slint 侧据此装配托盘/Dock/状态项。
//!
//! 实证口径（2026-10-18 实读源码 + 核对 `app/format.py`）：
//!
//! - `SystemTrayIcon.NAME = "Ghost Downloader"`，`setToolTip(self.NAME)` 是初值
//!   （tray.py:172-178）。
//! - 菜单顺序（tray.py:181-190）：仪表盘（`GhostIcon.GHOST`）→ 全部开始
//!   （`FluentIcon.PLAY`）→ 全部暂停（`FluentIcon.PAUSE`）→ 分隔线 → 退出程序
//!   （`FluentIcon.CLOSE`）；退出接 `QApplication.instance().quit`。
//! - `_onActivated`（:197-199）只认 `ActivationReason.Trigger`；`messageClicked`
//!   直接接 `signalBus.activationRequested`。
//! - `_onSpeedChanged`（:201-205）：`speed > 0` 才显示
//!   `"{NAME}\n↓ {toReadableSize(speed)}/s"`，否则回到纯 `NAME`。
//! - `toReadableSize`（`format.py:17-22`）是**两**位小数 + `B/KB/MB/GB/TB`，
//!   与 `toDockSpeed`（:4-14）的**一**位小数 + `B/s, K/s, M/s, G/s` 是两套口径；
//!   后者小于 1024 时输出**整数** `"{n} B/s"`。
//! - Windows 菜单三选一（:166-168）：`win32` 且 `isGreaterEqualWin11()` →
//!   `AcrylicMenu`；`win32` 否则 → `TrayRoundMenu`；其它平台 → `RoundMenu`。
//! - `AcrylicMenu.adjustPosition`（:120-126）：`w = sizeHint.width + 5`，
//!   `x = min(self.x() - m.left(), rect.right() - w)`，`y = self.y() - 45`。
//! - `TrayRoundMenu.showEvent`（:159-164）：`x = min(x, right - width)`，
//!   `y = min(y, bottom - height)`。
//! - `AcrylicMenu` 的常量：`itemHeight = 28`（:83）、`MenuStyle(iconSize=14)`
//!   （:53）、`timer.setInterval(400)` 且单次（:106-107）、
//!   `paintEvent` 填 `QColor(0,0,0,1)`（:154）、
//!   `showEvent` 的亚克力底色 `"00000030"`/`"FFFFFF30"`（:134-137）。
//! - `dock.update`（:31-110）：基准图缩放到 256×256（KeepAspectRatio +
//!   SmoothTransformation），字号 `int(size * 0.14)`，最大宽度 `size * 0.85`，
//!   `while advance > maxWidth and pixel > 6: pixel -= 1`；`textX =
//!   (size - advance) / 2`，`textY = size * 0.82`；描边
//!   `QColor(0,0,0,180)` 宽 12 圆头圆角，填充纯白；进度条高 `tileHeight * 0.08`、
//!   `style 0`、`min 0`、`max 100`、非不确定态；`progress >= 0` 才显示。
//! - `dock.clear`（:112-121）在 `container is None` 时**直接 return**（无副作用）。
//! - `dock.onSpeedChanged`（:123-129）先查 `shouldShowDockSpeed and
//!   shouldShowDockIcon`，`speed <= 0` 才 clear，否则 `update(toDockSpeed(speed),
//!   runningProgress())`。
//! - `mac_status_item`（:42-112）：图标高 16、按原始宽高比缩放；菜单四项
//!   `仪表盘/全部开始/全部暂停/退出程序`，选择器
//!   `showDashboard:/startAll:/pauseAll:/quitApp:`，仅退出有快捷键 `"q"`；
//!   `setSpeed` 的标题带**前导空格** `" {size}/s"`；`_refreshMenuItems` 里
//!   「全部开始」在存在 `PAUSED/FAILED/WAITING` 时启用、「全部暂停」在存在
//!   `RUNNING` 时启用。
//!
//! 已知偏差：
//!
//! - `toLocalizedError` / `QCoreApplication.translate` 的译文层不存在，
//!   菜单文案按中文原文硬编码（与 Python 缺省语言一致）。
//! - `status_icon_size` 在高度为 0 时返回 `(16, 16)`；Python 会抛
//!   `ZeroDivisionError`。

use gd3_core::task::TaskStatus;

use crate::format::to_readable_size;

// ---------------------------------------------------------------- 托盘：菜单

/// `SystemTrayIcon.NAME`（tray.py:172）。
pub const TRAY_NAME: &str = "Ghost Downloader";

/// 菜单四项文案（tray.py:181-189）。
pub const TRAY_DASHBOARD: &str = "仪表盘";
pub const TRAY_START_ALL: &str = "全部开始";
pub const TRAY_PAUSE_ALL: &str = "全部暂停";
pub const TRAY_QUIT: &str = "退出程序";

/// 菜单项类别。`Separator` 不对应任何动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayActionKind {
    Dashboard,
    StartAll,
    PauseAll,
    Separator,
    Quit,
}

/// 一个菜单项。`icon` 是 FluentIcon 成员名（`Separator` 为 `None`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrayMenuEntry {
    pub kind: TrayActionKind,
    pub icon: Option<&'static str>,
    pub text: &'static str,
}

/// 托盘右键菜单的完整装配顺序（tray.py:180-190）。
pub fn tray_menu() -> [TrayMenuEntry; 5] {
    [
        TrayMenuEntry {
            kind: TrayActionKind::Dashboard,
            icon: Some("GHOST"),
            text: TRAY_DASHBOARD,
        },
        TrayMenuEntry {
            kind: TrayActionKind::StartAll,
            icon: Some("PLAY"),
            text: TRAY_START_ALL,
        },
        TrayMenuEntry {
            kind: TrayActionKind::PauseAll,
            icon: Some("PAUSE"),
            text: TRAY_PAUSE_ALL,
        },
        TrayMenuEntry {
            kind: TrayActionKind::Separator,
            icon: None,
            text: "",
        },
        TrayMenuEntry {
            kind: TrayActionKind::Quit,
            icon: Some("CLOSE"),
            text: TRAY_QUIT,
        },
    ]
}

/// 托盘触发的动作（`signalBus` 事件 + 两个 taskService 调用 + quit）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraySignal {
    ActivationRequested,
    StartAll,
    PauseAll,
    Quit,
}

/// `Action(..., triggered=...)` 的落点（tray.py:181-189）。
pub fn tray_action(kind: TrayActionKind) -> Option<TraySignal> {
    match kind {
        TrayActionKind::Dashboard => Some(TraySignal::ActivationRequested),
        TrayActionKind::StartAll => Some(TraySignal::StartAll),
        TrayActionKind::PauseAll => Some(TraySignal::PauseAll),
        TrayActionKind::Quit => Some(TraySignal::Quit),
        TrayActionKind::Separator => None,
    }
}

/// `QSystemTrayIcon.ActivationReason` 的取值（Qt6 定义）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivationReason {
    Unknown,
    Context,
    DoubleClick,
    Trigger,
    MiddleClick,
}

impl ActivationReason {
    /// Qt 的整数值：Unknown 0 / Context 1 / DoubleClick 2 / Trigger 3 /
    /// MiddleClick 4；未知整数落到 `Unknown`。
    pub fn from_qt(value: i32) -> Self {
        match value {
            1 => ActivationReason::Context,
            2 => ActivationReason::DoubleClick,
            3 => ActivationReason::Trigger,
            4 => ActivationReason::MiddleClick,
            _ => ActivationReason::Unknown,
        }
    }

    pub fn qt_value(self) -> i32 {
        match self {
            ActivationReason::Unknown => 0,
            ActivationReason::Context => 1,
            ActivationReason::DoubleClick => 2,
            ActivationReason::Trigger => 3,
            ActivationReason::MiddleClick => 4,
        }
    }
}

/// `_onActivated`（tray.py:197-199）：只有 `Trigger`（左键单击）唤出主窗口。
pub fn tray_activated(reason: ActivationReason) -> Option<TraySignal> {
    if reason == ActivationReason::Trigger {
        Some(TraySignal::ActivationRequested)
    } else {
        None
    }
}

/// `_onSpeedChanged`（tray.py:201-205）的提示气泡文案。
///
/// `speed > 0` → `"{NAME}\n↓ {toReadableSize(speed)}/s"`；否则纯 `NAME`。
pub fn tray_tooltip(speed: i64) -> String {
    if speed > 0 {
        format!("{}\n↓ {}/s", TRAY_NAME, to_readable_size(speed as f64))
    } else {
        TRAY_NAME.to_string()
    }
}

// ---------------------------------------------------------------- 托盘：菜单定位

/// `AcrylicMenu.adjustPosition`（tray.py:120-126）。
pub const TRAY_MENU_WIDTH_PAD: i32 = 5;

/// `y = self.y() - 45`（tray.py:125）。
pub const ACRYLIC_MENU_Y_OFFSET: i32 = 45;

/// `self.itemHeight = 28`（tray.py:83）。
pub const ACRYLIC_MENU_ITEM_HEIGHT: i32 = 28;

/// `MenuStyle(iconSize=14)`（tray.py:53）。
pub const ACRYLIC_MENU_ICON_SIZE: i32 = 14;

/// `timer.setInterval(400)` + `setSingleShot(True)`（tray.py:106-107）。
pub const ACRYLIC_MENU_HIDE_DELAY_MS: i32 = 400;

/// `painter.setBrush(QColor(0, 0, 0, 1))`（tray.py:154）。
pub const ACRYLIC_MENU_PAINT_COLOR: (u8, u8, u8, u8) = (0, 0, 0, 1);

/// Windows 亚克力底色的 alpha 都是 `0x30`，只是基色不同（tray.py:134-137）。
pub const ACRYLIC_TINT_DARK: &str = "00000030";
pub const ACRYLIC_TINT_LIGHT: &str = "FFFFFF30";

/// 托盘菜单的实现三选一（tray.py:166-168）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayMenuFlavor {
    /// 非 Windows 平台的 `RoundMenu`。
    RoundMenu,
    /// Windows 10 的 `TrayRoundMenu`。
    TrayRoundMenu,
    /// Windows 11 的 `AcrylicMenu`。
    AcrylicMenu,
}

/// `TrayMenu = AcrylicMenu if isGreaterEqualWin11() else TrayRoundMenu`，
/// 非 `win32` 一律 `RoundMenu`。
pub fn tray_menu_flavor(platform: &str, win11: bool) -> TrayMenuFlavor {
    if platform != "win32" {
        TrayMenuFlavor::RoundMenu
    } else if win11 {
        TrayMenuFlavor::AcrylicMenu
    } else {
        TrayMenuFlavor::TrayRoundMenu
    }
}

/// `AcrylicMenu.adjustPosition`：返回新的 `(x, y)`。
pub fn acrylic_position(
    x: i32,
    y: i32,
    margin_left: i32,
    size_hint_width: i32,
    screen_right: i32,
) -> (i32, i32) {
    let width = size_hint_width + TRAY_MENU_WIDTH_PAD;
    let new_x = (x - margin_left).min(screen_right - width);
    (new_x, y - ACRYLIC_MENU_Y_OFFSET)
}

/// `TrayRoundMenu.showEvent`：把菜单钳制在屏幕内。
pub fn round_menu_position(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    screen_right: i32,
    screen_bottom: i32,
) -> (i32, i32) {
    (x.min(screen_right - width), y.min(screen_bottom - height))
}

/// `showEvent` 的亚克力底色：`isDarkTheme()` 当 `Theme.AUTO`，否则比较
/// `cfg.themeMode.value == Theme.DARK`（tray.py:132-137）。
pub fn acrylic_tint(dark: bool) -> &'static str {
    if dark {
        ACRYLIC_TINT_DARK
    } else {
        ACRYLIC_TINT_LIGHT
    }
}

/// 把 `Theme.AUTO` 的明暗解析口径单独留一个入口（tray.py:133）：
/// 非 AUTO 时**不**看系统主题，直接比较配置值是否为 `Dark`。
pub fn acrylic_is_dark(theme_value: &str, system_dark: bool) -> bool {
    if theme_value == "Auto" {
        system_dark
    } else {
        theme_value == "Dark"
    }
}

// ---------------------------------------------------------------- Dock

/// `dock.py:35-39`：基准图缩放到 256×256。
pub const DOCK_ICON_SIZE: i32 = 256;

/// `pixel = int(size * 0.14)`（dock.py:48）。
pub const DOCK_FONT_RATIO: f64 = 0.14;

/// 缩字下限：`pixel > 6`（dock.py:51）。
pub const DOCK_FONT_MIN_PIXEL: i32 = 6;

/// `maxWidth = size * 0.85`（dock.py:50）。
pub const DOCK_TEXT_MAX_WIDTH_RATIO: f64 = 0.85;

/// `textY = size * 0.82`（dock.py:57）。
pub const DOCK_TEXT_Y_RATIO: f64 = 0.82;

/// 描边笔：`QPen(QColor(0, 0, 0, 180), 12, SolidLine, RoundCap, RoundJoin)`
/// （dock.py:61-65）。
pub const DOCK_TEXT_STROKE_WIDTH: i32 = 12;
pub const DOCK_TEXT_STROKE_COLOR: (u8, u8, u8, u8) = (0, 0, 0, 180);
pub const DOCK_TEXT_FILL: (u8, u8, u8) = (255, 255, 255);

/// 进度条：`barHeight = tileSize.height * 0.08`、`style 0`、`min 0`、
/// `max 100`、`setIndeterminate_(False)`（dock.py:90-97）。
pub const DOCK_BAR_HEIGHT_RATIO: f64 = 0.08;
pub const DOCK_BAR_STYLE: i32 = 0;
pub const DOCK_BAR_MIN_VALUE: f64 = 0.0;
pub const DOCK_BAR_MAX_VALUE: f64 = 100.0;
pub const DOCK_BAR_INDETERMINATE: bool = false;

/// `toDockSpeed`（`format.py:4-14`）：与 `toReadableSize` 是两套口径。
///
/// `< 1024` → 整数 `"{n} B/s"`；其后每级**一**位小数，单位 `K/s`/`M/s`/`G/s`。
pub fn to_dock_speed(bytes_per_sec: i64) -> String {
    if bytes_per_sec < 1024 {
        return format!("{} B/s", bytes_per_sec);
    }
    let mut value = bytes_per_sec as f64 / 1024.0;
    if value < 1024.0 {
        return format!("{value:.1} K/s");
    }
    value /= 1024.0;
    if value < 1024.0 {
        return format!("{value:.1} M/s");
    }
    value /= 1024.0;
    format!("{value:.1} G/s")
}

/// `dock.update` 的文字排版结果（字号、左边距、基线、实际宽度）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DockTextLayout {
    pub pixel_size: i32,
    pub text_x: f64,
    pub text_y: f64,
    pub advance: f64,
}

/// `dock.update` 的缩字循环（dock.py:46-57）。
///
/// `advance(text, pixel_size)` 由调用方提供（Slint 侧查字体度量）；Python 用
/// `QFontMetricsF(font).horizontalAdvance(text)`，每轮换字号后重算。
pub fn dock_text_layout(
    size: f64,
    text: &str,
    advance: &dyn Fn(&str, i32) -> f64,
) -> DockTextLayout {
    // `int(size * 0.14)`：正数下 `as i32` 与 `int()` 同为向零截断。
    let mut pixel_size = (size * DOCK_FONT_RATIO) as i32;
    let max_width = size * DOCK_TEXT_MAX_WIDTH_RATIO;
    let mut width = advance(text, pixel_size);
    while width > max_width && pixel_size > DOCK_FONT_MIN_PIXEL {
        pixel_size -= 1;
        width = advance(text, pixel_size);
    }
    DockTextLayout {
        pixel_size,
        text_x: (size - width) / 2.0,
        text_y: size * DOCK_TEXT_Y_RATIO,
        advance: width,
    }
}

/// `dock_bar_height(tileHeight)`。
pub fn dock_bar_height(tile_height: f64) -> f64 {
    tile_height * DOCK_BAR_HEIGHT_RATIO
}

/// `if progress >= 0:`（dock.py:104）：负值表示隐藏进度条。
pub fn dock_bar_visible(progress: f64) -> bool {
    progress >= 0.0
}

/// Dock 侧对外的一次副作用。
#[derive(Clone, Debug, PartialEq)]
pub enum DockEffect {
    /// `app.setActivationPolicy_(Regular|Accessory)`，可选 `activateIgnoringOtherApps`。
    SetActivationPolicy { visible: bool, activate: bool },
    /// `imageView.setImage_(...)` + `progressBar.setDoubleValue_(...)` + `tile.display()`。
    UpdateTile { text: String, progress: f64 },
    /// `tile.setContentView_(None)` + `tile.display()`。
    ClearTile,
}

/// `setupDock` 的闭包状态（`container` 是否已建）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DockState {
    has_container: bool,
}

impl DockState {
    pub fn new() -> Self {
        Self {
            has_container: false,
        }
    }

    pub fn has_container(&self) -> bool {
        self.has_container
    }

    /// `onSpeedChanged`（dock.py:123-129）。
    ///
    /// 先查两个开关（任一关闭就完全不响应），`speed <= 0` 走 `clear()`，
    /// 否则 `update(toDockSpeed(speed), runningProgress())`。
    pub fn on_speed_changed(
        &mut self,
        speed: i64,
        show_dock_speed: bool,
        show_dock_icon: bool,
        running_progress: f64,
    ) -> Vec<DockEffect> {
        if !(show_dock_speed && show_dock_icon) {
            return Vec::new();
        }
        if speed <= 0 {
            return self.clear();
        }
        self.has_container = true;
        vec![DockEffect::UpdateTile {
            text: to_dock_speed(speed),
            progress: running_progress,
        }]
    }

    /// `onShowSpeedChanged`（dock.py:131-133）：只在关闭时清空。
    pub fn on_show_speed_changed(&mut self, enabled: bool) -> Vec<DockEffect> {
        if enabled { Vec::new() } else { self.clear() }
    }

    /// `onShowIconChanged`（dock.py:135-137）：先切激活策略，再 `clear()`。
    pub fn on_show_icon_changed(&mut self, visible: bool) -> Vec<DockEffect> {
        let mut effects = vec![DockEffect::SetActivationPolicy {
            visible,
            activate: true,
        }];
        effects.extend(self.clear());
        effects
    }

    /// `clear()`（dock.py:112-121）：`container is None` 时**直接 return**。
    fn clear(&mut self) -> Vec<DockEffect> {
        if !self.has_container {
            return Vec::new();
        }
        self.has_container = false;
        vec![DockEffect::ClearTile]
    }
}

// ---------------------------------------------------------------- macOS 状态项

/// `MacStatusItem.ICON_SIZE = 16`（mac_status_item.py:43）。
pub const STATUS_ICON_SIZE: f64 = 16.0;

/// `menu.itemAtIndex_(1)` / `(2)`（mac_status_item.py:58-59）。
pub const STATUS_START_ALL_INDEX: usize = 1;
pub const STATUS_PAUSE_ALL_INDEX: usize = 2;

/// 一个 `NSMenuItem`（选择器、快捷键、SF Symbol 名）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusMenuItem {
    pub title: &'static str,
    pub selector: &'static str,
    pub key: &'static str,
    pub symbol: &'static str,
}

/// `_buildMenu` 的四项（mac_status_item.py:77-82）。
pub fn status_menu() -> [StatusMenuItem; 4] {
    [
        StatusMenuItem {
            title: TRAY_DASHBOARD,
            selector: "showDashboard:",
            key: "",
            symbol: "gauge.open.with.lines.needle.33percent",
        },
        StatusMenuItem {
            title: TRAY_START_ALL,
            selector: "startAll:",
            key: "",
            symbol: "play.fill",
        },
        StatusMenuItem {
            title: TRAY_PAUSE_ALL,
            selector: "pauseAll:",
            key: "",
            symbol: "pause.fill",
        },
        StatusMenuItem {
            title: TRAY_QUIT,
            selector: "quitApp:",
            key: "q",
            symbol: "power",
        },
    ]
}

/// `setSpeed`（mac_status_item.py:67-71）：标题带**前导空格**。
pub fn status_speed_title(show_menu_bar_speed: bool, bytes_per_second: i64) -> String {
    if show_menu_bar_speed && bytes_per_second > 0 {
        format!(" {}/s", to_readable_size(bytes_per_second as f64))
    } else {
        String::new()
    }
}

/// `_refreshMenuItems`（mac_status_item.py:91-100）：返回
/// `(start_all_enabled, pause_all_enabled)`。
///
/// 「全部开始」要求存在 `PAUSED/FAILED/WAITING`，「全部暂停」要求存在
/// `RUNNING`；菜单本身 `setAutoenablesItems_(False)`，所以这是唯一开关。
pub fn status_menu_enabled(statuses: &[TaskStatus]) -> (bool, bool) {
    let start_all = statuses.iter().any(|s| {
        matches!(
            s,
            TaskStatus::Paused | TaskStatus::Failed | TaskStatus::Waiting
        )
    });
    let pause_all = statuses.contains(&TaskStatus::Running);
    (start_all, pause_all)
}

/// `_buildMenuBarIcon`（mac_status_item.py:102-108）：
/// `(16 * width / height, 16)`；高度非正时回落 `(16, 16)`（已知偏差）。
pub fn status_icon_size(width: f64, height: f64) -> (f64, f64) {
    if height <= 0.0 {
        return (STATUS_ICON_SIZE, STATUS_ICON_SIZE);
    }
    (STATUS_ICON_SIZE * width / height, STATUS_ICON_SIZE)
}

// `_onShowSpeedChanged`（mac_status_item.py:110-112）在关闭时把标题清空——这与
// [`status_speed_title`] 的 `show_menu_bar_speed == false` 分支完全一致，故不
// 单独建模：调用方把开关新值传进去即可。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_menu_order_matches_python() {
        let menu = tray_menu();
        let kinds: Vec<TrayActionKind> = menu.iter().map(|entry| entry.kind).collect();
        assert_eq!(
            kinds,
            vec![
                TrayActionKind::Dashboard,
                TrayActionKind::StartAll,
                TrayActionKind::PauseAll,
                TrayActionKind::Separator,
                TrayActionKind::Quit,
            ]
        );
        let texts: Vec<&str> = menu.iter().map(|entry| entry.text).collect();
        assert_eq!(
            texts,
            vec!["仪表盘", "全部开始", "全部暂停", "", "退出程序"]
        );
        let icons: Vec<Option<&str>> = menu.iter().map(|entry| entry.icon).collect();
        assert_eq!(
            icons,
            vec![
                Some("GHOST"),
                Some("PLAY"),
                Some("PAUSE"),
                None,
                Some("CLOSE")
            ]
        );
    }

    #[test]
    fn tray_actions_map_to_signals() {
        assert_eq!(
            tray_action(TrayActionKind::Dashboard),
            Some(TraySignal::ActivationRequested)
        );
        assert_eq!(
            tray_action(TrayActionKind::StartAll),
            Some(TraySignal::StartAll)
        );
        assert_eq!(
            tray_action(TrayActionKind::PauseAll),
            Some(TraySignal::PauseAll)
        );
        assert_eq!(tray_action(TrayActionKind::Quit), Some(TraySignal::Quit));
        assert_eq!(tray_action(TrayActionKind::Separator), None);
    }

    #[test]
    fn tray_activation_only_reacts_to_trigger() {
        assert_eq!(
            tray_activated(ActivationReason::Trigger),
            Some(TraySignal::ActivationRequested)
        );
        for reason in [
            ActivationReason::Unknown,
            ActivationReason::Context,
            ActivationReason::DoubleClick,
            ActivationReason::MiddleClick,
        ] {
            assert_eq!(tray_activated(reason), None);
        }
    }

    #[test]
    fn activation_reason_round_trips_qt_values() {
        for value in 0..=4 {
            assert_eq!(ActivationReason::from_qt(value).qt_value(), value);
        }
        assert_eq!(ActivationReason::from_qt(9), ActivationReason::Unknown);
        assert_eq!(ActivationReason::from_qt(-1), ActivationReason::Unknown);
    }

    #[test]
    fn tray_tooltip_switches_on_positive_speed() {
        assert_eq!(tray_tooltip(0), TRAY_NAME);
        assert_eq!(tray_tooltip(-5), TRAY_NAME);
        assert_eq!(tray_tooltip(1), "Ghost Downloader\n↓ 1.00 B/s");
        assert_eq!(tray_tooltip(1024), "Ghost Downloader\n↓ 1.00 KB/s");
        assert_eq!(tray_tooltip(1536), "Ghost Downloader\n↓ 1.50 KB/s");
    }

    #[test]
    fn tray_menu_flavor_depends_on_platform_and_win11() {
        assert_eq!(tray_menu_flavor("linux", true), TrayMenuFlavor::RoundMenu);
        assert_eq!(tray_menu_flavor("darwin", false), TrayMenuFlavor::RoundMenu);
        assert_eq!(tray_menu_flavor("win32", true), TrayMenuFlavor::AcrylicMenu);
        assert_eq!(
            tray_menu_flavor("win32", false),
            TrayMenuFlavor::TrayRoundMenu
        );
    }

    #[test]
    fn acrylic_position_clamps_to_screen_and_lifts_by_45() {
        // 宽 = sizeHint + 5 = 205；屏幕右边界 1920 → 允许的最大 x = 1715。
        assert_eq!(acrylic_position(500, 400, 0, 200, 1920), (500, 355));
        // 左边距参与减法。
        assert_eq!(acrylic_position(500, 400, 8, 200, 1920), (492, 355));
        // 越过右边界 → 钳到 right - width。
        assert_eq!(acrylic_position(1900, 400, 0, 200, 1920), (1715, 355));
    }

    #[test]
    fn round_menu_position_clamps_both_axes() {
        assert_eq!(
            round_menu_position(100, 100, 50, 60, 1920, 1080),
            (100, 100)
        );
        assert_eq!(
            round_menu_position(1900, 1070, 50, 60, 1920, 1080),
            (1870, 1020)
        );
    }

    #[test]
    fn acrylic_tint_follows_theme_resolution() {
        assert_eq!(acrylic_tint(true), "00000030");
        assert_eq!(acrylic_tint(false), "FFFFFF30");
        assert!(acrylic_is_dark("Auto", true));
        assert!(!acrylic_is_dark("Auto", false));
        assert!(acrylic_is_dark("Dark", false), "非 AUTO 不看系统主题");
        assert!(!acrylic_is_dark("Light", true));
    }

    #[test]
    fn acrylic_menu_constants_match_python() {
        const { assert!(TRAY_MENU_WIDTH_PAD == 5) };
        const { assert!(ACRYLIC_MENU_Y_OFFSET == 45) };
        const { assert!(ACRYLIC_MENU_ITEM_HEIGHT == 28) };
        const { assert!(ACRYLIC_MENU_ICON_SIZE == 14) };
        const { assert!(ACRYLIC_MENU_HIDE_DELAY_MS == 400) };
        const { assert!(ACRYLIC_MENU_PAINT_COLOR.3 == 1) };
        assert_eq!(TRAY_NAME, "Ghost Downloader");
    }

    #[test]
    fn to_dock_speed_uses_single_decimal_units() {
        assert_eq!(to_dock_speed(0), "0 B/s");
        assert_eq!(to_dock_speed(1023), "1023 B/s");
        assert_eq!(to_dock_speed(1024), "1.0 K/s");
        assert_eq!(to_dock_speed(1536), "1.5 K/s");
        assert_eq!(to_dock_speed(1024 * 1024), "1.0 M/s");
        assert_eq!(to_dock_speed(1024 * 1024 * 1024), "1.0 G/s");
        assert_eq!(to_dock_speed(1024_i64.pow(4)), "1024.0 G/s");
    }

    #[test]
    fn dock_text_layout_shrinks_until_it_fits() {
        // 字号从 int(256 * 0.14) = 35 起；每像素宽度 2.0，文本 4 字符 → 8 * pixel。
        let advance = |text: &str, pixel: i32| text.chars().count() as f64 * 2.0 * pixel as f64;
        let layout = dock_text_layout(256.0, "1.5M", &advance);
        // maxWidth = 217.6；8*35=280 > 217.6 → 逐级降到 27（8*27=216 ≤ 217.6）。
        assert_eq!(layout.pixel_size, 27);
        assert_eq!(layout.advance, 216.0);
        assert_eq!(layout.text_x, 20.0);
        assert_eq!(layout.text_y, 256.0 * 0.82);
    }

    #[test]
    fn dock_text_layout_stops_at_six_pixels() {
        let advance = |_text: &str, _pixel: i32| 10_000.0;
        let layout = dock_text_layout(100.0, "long", &advance);
        assert_eq!(layout.pixel_size, 6, "pixel > 6 才继续缩");
        assert!(layout.text_x < 0.0);
    }

    #[test]
    fn dock_bar_geometry_and_visibility() {
        assert_eq!(dock_bar_height(100.0), 8.0);
        const { assert!(DOCK_BAR_STYLE == 0) };
        const { assert!(DOCK_BAR_MIN_VALUE == 0.0) };
        const { assert!(DOCK_BAR_MAX_VALUE == 100.0) };
        const { assert!(!DOCK_BAR_INDETERMINATE) };
        assert!(dock_bar_visible(0.0));
        assert!(dock_bar_visible(42.5));
        assert!(!dock_bar_visible(-0.5));
    }

    #[test]
    fn dock_speed_change_requires_both_switches() {
        let mut dock = DockState::new();
        assert!(dock.on_speed_changed(2048, false, true, 10.0).is_empty());
        assert!(dock.on_speed_changed(2048, true, false, 10.0).is_empty());
        assert!(!dock.has_container(), "开关关闭时连 container 都不建");

        let effects = dock.on_speed_changed(2048, true, true, 10.0);
        assert_eq!(
            effects,
            vec![DockEffect::UpdateTile {
                text: "2.0 K/s".to_string(),
                progress: 10.0
            }]
        );
        assert!(dock.has_container());
    }

    #[test]
    fn dock_zero_speed_clears_only_when_container_exists() {
        let mut dock = DockState::new();
        assert!(
            dock.on_speed_changed(0, true, true, 0.0).is_empty(),
            "未建 container 时 clear 直接 return"
        );

        dock.on_speed_changed(4096, true, true, 5.0);
        assert_eq!(
            dock.on_speed_changed(0, true, true, 0.0),
            vec![DockEffect::ClearTile]
        );
        assert!(!dock.has_container());
        assert!(dock.on_speed_changed(0, true, true, 0.0).is_empty());
    }

    #[test]
    fn dock_toggle_handlers_clear_and_switch_policy() {
        let mut dock = DockState::new();
        assert_eq!(
            dock.on_show_icon_changed(false),
            vec![DockEffect::SetActivationPolicy {
                visible: false,
                activate: true
            }]
        );

        dock.on_speed_changed(1024, true, true, 0.0);
        assert_eq!(
            dock.on_show_icon_changed(true),
            vec![
                DockEffect::SetActivationPolicy {
                    visible: true,
                    activate: true
                },
                DockEffect::ClearTile,
            ]
        );

        assert!(dock.on_show_speed_changed(true).is_empty());
        dock.on_speed_changed(1024, true, true, 0.0);
        assert_eq!(
            dock.on_show_speed_changed(false),
            vec![DockEffect::ClearTile]
        );
    }

    #[test]
    fn status_menu_matches_python_items() {
        let menu = status_menu();
        let titles: Vec<&str> = menu.iter().map(|item| item.title).collect();
        assert_eq!(titles, vec!["仪表盘", "全部开始", "全部暂停", "退出程序"]);
        let selectors: Vec<&str> = menu.iter().map(|item| item.selector).collect();
        assert_eq!(
            selectors,
            vec!["showDashboard:", "startAll:", "pauseAll:", "quitApp:"]
        );
        let keys: Vec<&str> = menu.iter().map(|item| item.key).collect();
        assert_eq!(keys, vec!["", "", "", "q"]);
        assert_eq!(menu[0].symbol, "gauge.open.with.lines.needle.33percent");
        assert_eq!(menu[3].symbol, "power");
        assert_eq!(STATUS_START_ALL_INDEX, 1);
        assert_eq!(STATUS_PAUSE_ALL_INDEX, 2);
    }

    #[test]
    fn status_speed_title_has_leading_space() {
        assert_eq!(status_speed_title(true, 1024), " 1.00 KB/s");
        assert_eq!(status_speed_title(true, 0), "");
        assert_eq!(status_speed_title(false, 1024), "");
    }

    #[test]
    fn status_menu_enabled_requires_matching_statuses() {
        assert_eq!(status_menu_enabled(&[]), (false, false));
        assert_eq!(status_menu_enabled(&[TaskStatus::Running]), (false, true));
        assert_eq!(status_menu_enabled(&[TaskStatus::Paused]), (true, false));
        assert_eq!(status_menu_enabled(&[TaskStatus::Failed]), (true, false));
        assert_eq!(status_menu_enabled(&[TaskStatus::Waiting]), (true, false));
        assert_eq!(
            status_menu_enabled(&[TaskStatus::Completed]),
            (false, false)
        );
        assert_eq!(
            status_menu_enabled(&[
                TaskStatus::Completed,
                TaskStatus::Running,
                TaskStatus::Paused
            ]),
            (true, true)
        );
    }

    #[test]
    fn status_icon_size_keeps_aspect_ratio() {
        assert_eq!(status_icon_size(32.0, 32.0), (16.0, 16.0));
        assert_eq!(status_icon_size(32.0, 16.0), (32.0, 16.0));
        assert_eq!(status_icon_size(16.0, 32.0), (8.0, 16.0));
        assert_eq!(status_icon_size(10.0, 0.0), (16.0, 16.0));
    }
}
