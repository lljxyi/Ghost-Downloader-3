//! 首次启动向导（第 74 步）：`app/view/windows/oobe_window.py`(971) 的
//! **语义镜像**。
//!
//! 与 P3 对话框同一策略：不画控件，只把「六页顺序、导航按钮可见性、
//! 主题卡选中、语言下拉换算、扩展页横幅状态机、运行时勾选与安装目标、
//! 高级选项落盘计划、窗口关闭/完成幂等」抽成可断言的状态机——Slint 侧
//! 据此数据驱动渲染，配置键名/缺省值/格式化文案与 Python 逐字一致。
//!
//! 实证口径（2026-10-18 实读源码 + 在 venv 里核对 qfluentwidgets）：
//!
//! - `WINDOW_SIZE = QSize(960, 600)`、`PREVIEW_SIZE = QSize(592, 333)`、
//!   `STORE_COLUMN_WIDTH = 280`、`OobeWindow.PAGE_COUNT = 6`（:46-49, :788）。
//! - 页序就是 `_initContent` 的 `addWidget` 顺序（:834-839）：
//!   欢迎 → 基本设置 → 浏览器扩展 → 推荐组件 → 更多选项 → 完成。
//! - `_refreshNavigation`（:904-912）：`isFirst = i == 0`、`isLast = i == 5`；
//!   `back`/`next` 可见性都是 `not isFirst and not isLast`，`skip` 只在首页
//!   可见；`pipsPager.currentIndex = i`。
//! - `_onNextClicked`（:914-927）：`i >= 5` 直接 return；**`i == 3` 时先
//!   `_installSelectedRuntimes()` 再自增**；自增后 `i == 2` →
//!   `refreshListenState()`，`i == 3` → `runtimeInstallPage.mount()`。
//! - `_onBackClicked`（:929-934）：`i <= 0` 直接 return，否则 `i -= 1` 且
//!   `setCurrentIndex(i, isBack=True)`。
//! - `_finish`（:946-953）与 `closeEvent`（:966-971）**共用 `_isFinished`
//!   闸门**：任一先到都会 `cfg.set(cfg.hasCompletedOobe, True)` 并
//!   `emit finished`；区别只在 `_finish` 额外跑 `advancedOptionsPage.save()`
//!   且显式 `close()`，而 `closeEvent` 走「用户点 X」路径，**不**保存
//!   高级选项。
//! - `_onLanguageChanged`（:890-902）是**整块重建 content**：重建后把
//!   `stackedWidget.setCurrentIndex(self._currentIndex)` 恢复（期间关动画），
//!   再 `_refreshNavigation()`。所以语言切换**不**改变当前页。
//! - `BasicSettingsPage._refreshThemeCards`（:378-381）按 `card.theme ==
//!   cfg.themeMode.value` 选卡；`Theme` 枚举值是 `"Light"/"Dark"/"Auto"`
//!   （qfluentwidgets `common/config.py:18-23`），`ThemePreview` 的 mode 映射
//!   是 `{LIGHT: "light", DARK: "dark"}` 其余兜底 `"auto"`（:137）。
//! - `ThemeCard` / `ActionCard` 的选中/推荐描边都是
//!   `drawRoundedRect(rect().adjusted(1,1,-1,-1), borderRadius, borderRadius)`
//!   + 2px 主题色笔；`CardWidget` 的 `borderRadius` 实测 **5**（`card_widget.py:24`）。
//! - `ActionCard` 高度：有 hint → `setMinimumHeight(80)`，无 hint →
//!   `setFixedHeight(48)`（:176-179）；`OptionCard` 恒 `setFixedHeight(64)`（:225）。
//! - `IconChip.paintEvent`：`margin = round(width * 0.25)`（:72），圆角 8。
//! - `ThemePreview._drawMini`（:84-102）：标题条高 14；padding 8；三根条
//!   `y+22 h9` / `y+37 h9 宽 0.55` / `y+h-20 h11 宽 0.34`，圆角 3；
//!   外框 `QPen(QColor(128,128,128,60), 1)` 画在
//!   `outer.adjusted(0.5, 0.5, -0.5, -0.5)`。
//! - `BasicSettingsPage` 的语言下拉：候选 `cfg.language.options`（9 项，见
//!   [`crate::settings_spec::LANGUAGE_LITERALS`]），标签 `LANGUAGE_TEXTS`
//!   缺失时回退 `self.tr("使用系统设置")`；初始
//!   `setCurrentIndex(options.index(cfg.language.value))`（:335-337）。
//! - `_onLanguageChanged`（:383-387）只在 `0 <= index < len` 且
//!   `options[index] != cfg.language.value` 时才写配置并发信号。
//! - 扩展页横幅（:493-577）：`_setBanner` 恒宽 `PREVIEW_SIZE.width()`、
//!   不可关闭、`duration=-1`、`position=NONE`、`contentLabel.hide()`；
//!   `refreshListenState` 在 `_isPaired` 时**直接 return**；
//!   `OFF` → WARNING「浏览器扩展未启用，可稍后在设置中开启」；
//!   `FAILED` → WARNING `toLocalizedError(state.error)`；其余 → INFORMATION
//!   「正在端口 {port} 上等待扩展连接」。
//! - `_setConnectedBanner`（:522-529）：有 version 时
//!   「已连接扩展 v{v}，最新版本为 v{latest}」，否则
//!   「已连接扩展，最新版本为 v{latest}」；`LATEST_EXTENSION_VERSION` 实测
//!   `"2.2.0"`（`app/config/constants.py:7`）。
//! - `_onConnectionChanged`（:531-538）：`connectionSummary` 的
//!   `installType or version` 为真 → `_isPaired = True` + 已连接横幅；
//!   否则 `_isPaired = False` + `refreshListenState()`。
//! - `_onListenStateChanged`（:569-571）：**仅 `isVisible()` 时**刷新。
//! - `RuntimeInstallPage.mount`（:606-639）：`_isMounted` 幂等；候选
//!   `rt.canInstall and rt.title`；`sort(key=lambda rt: not rt.isRecommended)`
//!   ——Python `list.sort` 稳定，故推荐项在前、组内保序；
//!   `runtime.path()` 非空 → 显示「已安装」**且不建复选框**，否则建
//!   复选框且 `checked = rt.isRecommended`。
//! - `_installSelectedRuntimes`（:936-944）：无选中直接 return；逐个
//!   `if runtime.path(): continue` 再 `install(runtime)`。
//! - `AdvancedOptionsPage.save`（:715-739）：`runAtLogin` **仅在与配置不同
//!   时**才 `setRunAtLogin` + `cfg.set`；`clipboard`/`category`/`aria2`
//!   无条件写；关联三卡仅在非 darwin 存在，逐包按
//!   `config.associateFileTypes` / `associateUriSchemes` 是否为 `None`
//!   决定写不写；`urlScheme` 卡勾选 → `registerUrlScheme()`，否则
//!   `unregisterUrlScheme()`，随后写配置。
//!
//! 已知偏差：
//!
//! - Python `cfg.language.options.index(value)` 在值缺失时抛 `ValueError`；
//!   这里沿用 [`crate::settings_spec::combo_index`] 的兜底 0（与既有
//!   `conflict.rs` / `option_cards.rs` 同口径）。
//! - `toLocalizedError` / `QCoreApplication.translate` 的译文层在 Rust 侧
//!   不存在，错误文本与运行时标题/描述一律**原样透传**（入参）。

use crate::editors::py_round;
use crate::settings_spec::{
    CHROME_WEBSTORE_URL, EDGE_ADDONS_URL, FIREFOX_ADDONS_URL, LANGUAGE_LITERALS, combo_index,
};

// ---------------------------------------------------------------- 窗口尺寸

/// `WINDOW_SIZE`（:46）：固定 960×600，`setResizeEnabled(False)`。
pub const WINDOW_WIDTH: i32 = 960;
pub const WINDOW_HEIGHT: i32 = 600;

/// `PREVIEW_SIZE`（:48）：扩展安装动图占位框，也是横幅宽度。
pub const PREVIEW_WIDTH: i32 = 592;
pub const PREVIEW_HEIGHT: i32 = 333;

/// 右栏（手动安装 + 三个商店卡）固定宽（:49）。
pub const STORE_COLUMN_WIDTH: i32 = 280;

/// `OobeWindow.PAGE_COUNT`（:788）。
pub const PAGE_COUNT: usize = 6;

/// `LATEST_EXTENSION_VERSION`（`app/config/constants.py:7`）。
pub const LATEST_EXTENSION_VERSION: &str = "2.2.0";

/// `chrome://extensions`（:550-551 的兜底复制内容）。
pub const CHROMIUM_EXTENSIONS_URL: &str = "chrome://extensions";

/// 窗口标题（:809）。
pub const WINDOW_TITLE: &str = "Ghost Downloader";

/// 内容区外边距：`(36, titleBar.height() + 8, 36, 16)`（:876）。
pub const LAYOUT_MARGIN_H: i32 = 36;
pub const LAYOUT_MARGIN_BOTTOM: i32 = 16;
/// 标题栏高度未知 → 单独暴露那 8px 间隙。
pub const LAYOUT_TITLE_BAR_GAP: i32 = 8;

/// 导航行外边距 `(0, 14, 0, 0)`（:858）。
pub const NAV_MARGIN_TOP: i32 = 14;

/// 首页/完成页的主按钮固定宽（:290、:766）。
pub const PRIMARY_BUTTON_WIDTH: i32 = 200;

/// 首页/完成页的垂直节奏（:292-302、:768-778）。
pub const HERO_SPACING: i32 = 8;
pub const HERO_STRETCH_TOP: i32 = 3;
pub const HERO_STRETCH_BOTTOM: i32 = 4;
pub const HERO_ICON_SIZE: i32 = 88;
pub const HERO_ICON_GAP: i32 = 16;
pub const HERO_BUTTON_GAP: i32 = 28;

/// `systemTitleBarRect`（:820-821）：`QRect(0, 10, 75, size.height())`。
pub const SYSTEM_TITLE_BAR_X: i32 = 0;
pub const SYSTEM_TITLE_BAR_Y: i32 = 10;
pub const SYSTEM_TITLE_BAR_WIDTH: i32 = 75;

// ---------------------------------------------------------------- 图标胶囊

/// `IconChip` 圆角（:71 `drawRoundedRect(rect(), 8, 8)`）。
pub const ICON_CHIP_RADIUS: f64 = 8.0;

/// 中性胶囊四色（:51）：`(浅底, 浅字, 深底, 深字)`。
pub const NEUTRAL_CHIP_COLORS: [&str; 4] = ["#EFEFEF", "#666666", "#3D3D3D", "#AAAAAA"];

/// 成功胶囊四色（:52）。
pub const SUCCESS_CHIP_COLORS: [&str; 4] = ["#E8F5E9", "#0F7B46", "#1A3A1A", "#4ADE80"];

/// `ActionCard.iconChip` 尺寸 30（:180）。
pub const ICON_CHIP_SIZE_ACTION: i32 = 30;
/// `OptionCard.iconChip` 尺寸 34（:226）。
pub const ICON_CHIP_SIZE_OPTION: i32 = 34;
/// `CompletePage.checkIcon` 尺寸 80（:753）。
pub const ICON_CHIP_SIZE_COMPLETE: i32 = 80;

/// `IconChip.paintEvent` 的内缩：`round(width * 0.25)`（:72）。
pub fn icon_chip_margin(width: i32) -> i32 {
    py_round(f64::from(width) * 0.25)
}

/// 按主题挑 `(背景, 前景)`：`(darkBg, darkFg)` / `(lightBg, lightFg)`（:67-68）。
pub fn chip_colors(colors: [&'static str; 4], dark: bool) -> (&'static str, &'static str) {
    if dark {
        (colors[2], colors[3])
    } else {
        (colors[0], colors[1])
    }
}

// ---------------------------------------------------------------- 主题卡

/// `Theme` 枚举值（qfluentwidgets `common/config.py:18-23`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeMode {
    Light,
    Dark,
    Auto,
}

impl ThemeMode {
    /// `BasicSettingsPage` 的建卡顺序（:325-327）。
    pub const ALL: [ThemeMode; 3] = [ThemeMode::Light, ThemeMode::Dark, ThemeMode::Auto];

    /// 配置字面量（`UserConfig.json` 的 `themeMode` 值）。
    pub fn value(self) -> &'static str {
        match self {
            ThemeMode::Light => "Light",
            ThemeMode::Dark => "Dark",
            ThemeMode::Auto => "Auto",
        }
    }

    /// `ThemePreview` 的 mode 字符串（:137）：非 LIGHT/DARK 一律 `"auto"`。
    pub fn preview_mode(self) -> &'static str {
        match self {
            ThemeMode::Light => "light",
            ThemeMode::Dark => "dark",
            ThemeMode::Auto => "auto",
        }
    }

    /// 卡片标签（:325-327）。
    pub fn label(self) -> &'static str {
        match self {
            ThemeMode::Light => "浅色",
            ThemeMode::Dark => "深色",
            ThemeMode::Auto => "跟随系统",
        }
    }

    /// 配置字面量 → 枚举；未知值返回 `None`。
    pub fn from_value(value: &str) -> Option<ThemeMode> {
        ThemeMode::ALL
            .into_iter()
            .find(|mode| mode.value() == value)
    }
}

/// `_refreshThemeCards`：选中索引（未知配置值 → 全不选，返回 `None`）。
pub fn selected_theme_index(config_value: &str) -> Option<usize> {
    ThemeMode::ALL
        .into_iter()
        .position(|mode| mode.value() == config_value)
}

/// 选中/推荐描边矩形：`rect().adjusted(1, 1, -1, -1)`（:162、:213）。
pub fn selection_ring(width: f64, height: f64) -> (f64, f64, f64, f64) {
    (1.0, 1.0, width - 2.0, height - 2.0)
}

/// `CardWidget.borderRadius`（`card_widget.py:24`）。
pub const CARD_BORDER_RADIUS: f64 = 5.0;

/// 描边笔宽（:159、:210 `QPen(themeColor(), 2)`）。
pub const SELECTION_PEN_WIDTH: f64 = 2.0;

/// `ThemePreview` 固定高（:82）。
pub const THEME_PREVIEW_HEIGHT: i32 = 96;
/// `ThemeCard` 布局 `(14, 14, 14, 12)` + 间距 10（:144-145）。
pub const THEME_CARD_MARGINS: (i32, i32, i32, i32) = (14, 14, 14, 12);
pub const THEME_CARD_SPACING: i32 = 10;
/// 预览框圆角 6（:110、:123）。
pub const THEME_PREVIEW_RADIUS: f64 = 6.0;
/// 外框 `QPen(QColor(128, 128, 128, 60), 1)`（:121）。
pub const THEME_PREVIEW_BORDER_WIDTH: f64 = 1.0;
pub const THEME_PREVIEW_BORDER_COLOR: (u8, u8, u8, u8) = (128, 128, 128, 60);

/// 预览小窗矩形（`QRectF` 的四个分量）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// `_drawMini` 里四块区域的几何（:89-102）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MiniLayout {
    /// 顶部标题条：`(x, y, w, 14)`。
    pub title_bar: Rect,
    /// 第一根条：`(x+8, y+22, w-16, 9)`。
    pub bar1: Rect,
    /// 第二根条：`(x+8, y+37, (w-16)*0.55, 9)`。
    pub bar2: Rect,
    /// 主题色按钮：`(x+8, y+h-20, (w-16)*0.34, 11)`。
    pub accent: Rect,
}

/// 小窗几何：padding 固定 8，三根条圆角 3、高 9/9/11。
pub const MINI_PADDING: f64 = 8.0;
pub const MINI_BAR_RADIUS: f64 = 3.0;
pub const MINI_TITLE_BAR_HEIGHT: f64 = 14.0;
pub const MINI_BAR_HEIGHT: f64 = 9.0;
pub const MINI_BAR1_TOP: f64 = 22.0;
pub const MINI_BAR2_TOP: f64 = 37.0;
pub const MINI_BAR2_RATIO: f64 = 0.55;
pub const MINI_ACCENT_HEIGHT: f64 = 11.0;
pub const MINI_ACCENT_BOTTOM: f64 = 20.0;
pub const MINI_ACCENT_RATIO: f64 = 0.34;

pub fn mini_layout(rect: Rect) -> MiniLayout {
    let bar_x = rect.x + MINI_PADDING;
    let bar_width = rect.width - MINI_PADDING * 2.0;
    MiniLayout {
        title_bar: Rect::new(rect.x, rect.y, rect.width, MINI_TITLE_BAR_HEIGHT),
        bar1: Rect::new(bar_x, rect.y + MINI_BAR1_TOP, bar_width, MINI_BAR_HEIGHT),
        bar2: Rect::new(
            bar_x,
            rect.y + MINI_BAR2_TOP,
            bar_width * MINI_BAR2_RATIO,
            MINI_BAR_HEIGHT,
        ),
        accent: Rect::new(
            bar_x,
            rect.y + rect.height - MINI_ACCENT_BOTTOM,
            bar_width * MINI_ACCENT_RATIO,
            MINI_ACCENT_HEIGHT,
        ),
    }
}

/// `paintEvent`（:113-118）：`auto` 左右各画一半（左浅右深），否则整块。
pub fn preview_panes(mode: ThemeMode, width: f64, height: f64) -> Vec<(Rect, bool)> {
    match mode {
        ThemeMode::Auto => {
            let half = width / 2.0;
            vec![
                (Rect::new(0.0, 0.0, half, height), false),
                (Rect::new(half, 0.0, half, height), true),
            ]
        }
        ThemeMode::Light => vec![(Rect::new(0.0, 0.0, width, height), false)],
        ThemeMode::Dark => vec![(Rect::new(0.0, 0.0, width, height), true)],
    }
}

/// `_drawMini` 的底色（:85）。
pub fn preview_background(dark: bool) -> (u8, u8, u8) {
    if dark {
        (0x23, 0x23, 0x23)
    } else {
        (0xF5, 0xF7, 0xFA)
    }
}

/// `_drawMini` 的标题条色（:86）。
pub fn preview_title_bar(dark: bool) -> (u8, u8, u8) {
    if dark {
        (0x2E, 0x2E, 0x2E)
    } else {
        (0xE9, 0xED, 0xF2)
    }
}

/// `_drawMini` 的条色（:87）。
pub fn preview_bar(dark: bool) -> (u8, u8, u8) {
    if dark {
        (0x33, 0x33, 0x33)
    } else {
        (0xFF, 0xFF, 0xFF)
    }
}

// ---------------------------------------------------------------- 卡片规格

/// `ActionCard._initWidget`（:176-179）：有 hint 走 `setMinimumHeight(80)`，
/// 无 hint 走 `setFixedHeight(48)`。
pub const ACTION_CARD_MIN_HEIGHT: i32 = 80;
pub const ACTION_CARD_FIXED_HEIGHT: i32 = 48;

/// `ActionCard._initLayout`（:189-206）。
pub const ACTION_CARD_MARGINS: (i32, i32, i32, i32) = (14, 8, 14, 8);
pub const ACTION_CARD_SPACING: i32 = 12;
pub const ACTION_CARD_TEXT_SPACING: i32 = 2;
pub const ACTION_CARD_CHEVRON_SIZE: i32 = 12;

/// `OptionCard._initWidget`（:225）：`setFixedHeight(64)`，与 hint 无关。
pub const OPTION_CARD_HEIGHT: i32 = 64;

/// `OptionCard._initLayout`（:236-247）：文本列 `setSpacing(0)`（与 ActionCard
/// 的 2 不同）。
pub const OPTION_CARD_MARGINS: (i32, i32, i32, i32) = (18, 8, 18, 8);
pub const OPTION_CARD_SPACING: i32 = 14;
pub const OPTION_CARD_TEXT_SPACING: i32 = 0;

/// `PageHeader`（:252-263）：标题 + 灰色描述，`setSpacing(2)`。
pub const PAGE_HEADER_SPACING: i32 = 2;

/// 一张 `ActionCard` 的静态规格（图标以 FluentIcon 成员名表示，Slint 侧查表）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionCardSpec {
    pub title: &'static str,
    pub hint: &'static str,
    pub icon: &'static str,
    pub recommended: bool,
    pub url: Option<&'static str>,
}

impl ActionCardSpec {
    /// 普通动作卡（无跳转 URL）。
    pub fn new(
        icon: &'static str,
        title: &'static str,
        hint: &'static str,
        recommended: bool,
    ) -> Self {
        Self {
            title,
            hint,
            icon,
            recommended,
            url: None,
        }
    }

    /// 商店卡：点击后 `QDesktopServices.openUrl(QUrl(url))`（:485-486）。
    pub fn with_url(icon: &'static str, title: &'static str, url: &'static str) -> Self {
        Self {
            title,
            hint: "",
            icon,
            recommended: false,
            url: Some(url),
        }
    }

    /// `if hint:`（:177）。
    pub fn has_hint(&self) -> bool {
        !self.hint.is_empty()
    }

    /// 只有无 hint 时才 `setFixedHeight(48)`；有 hint 返回 `None`。
    pub fn fixed_height(&self) -> Option<i32> {
        if self.has_hint() {
            None
        } else {
            Some(ACTION_CARD_FIXED_HEIGHT)
        }
    }

    /// 只有有 hint 时才 `setMinimumHeight(80)`；无 hint 返回 `None`。
    pub fn minimum_height(&self) -> Option<i32> {
        if self.has_hint() {
            Some(ACTION_CARD_MIN_HEIGHT)
        } else {
            None
        }
    }
}

/// `manualCard`（:435-440）：`FluentIcon.DOWNLOAD` + `isRecommended=True`。
pub fn manual_install_card() -> ActionCardSpec {
    ActionCardSpec::new("DOWNLOAD", MANUAL_INSTALL_TITLE, MANUAL_INSTALL_HINT, true)
}

/// `storeCards`（:441-447）：三张无 hint 的 `FluentIcon.GLOBE` 卡。
pub fn store_cards() -> [ActionCardSpec; 3] {
    [
        ActionCardSpec::with_url("GLOBE", STORE_CHROME_TITLE, CHROME_WEBSTORE_URL),
        ActionCardSpec::with_url("GLOBE", STORE_EDGE_TITLE, EDGE_ADDONS_URL),
        ActionCardSpec::with_url("GLOBE", STORE_FIREFOX_TITLE, FIREFOX_ADDONS_URL),
    ]
}

/// 一张 `OptionCard` 的静态规格（开关初值由调用方按配置给）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OptionCardSpec {
    pub icon: &'static str,
    pub title: &'static str,
    pub desc: &'static str,
}

/// `AdvancedOptionsPage._initWidget` 的七张卡（:658-697）。
pub const OPTION_RUN_AT_LOGIN: OptionCardSpec = OptionCardSpec {
    icon: "POWER_BUTTON",
    title: "开机自启",
    desc: "登录系统时自动在后台启动，随时接管下载",
};
pub const OPTION_CLIPBOARD: OptionCardSpec = OptionCardSpec {
    icon: "PASTE",
    title: "剪贴板监听",
    desc: "复制下载链接时自动弹出新任务提示",
};
pub const OPTION_CATEGORY: OptionCardSpec = OptionCardSpec {
    icon: "TAG",
    title: "自动分类保存",
    desc: "按文件类型自动保存到 视频、音频、文档 等子文件夹",
};
pub const OPTION_FILE_ASSOC: OptionCardSpec = OptionCardSpec {
    icon: "DOCUMENT",
    title: "关联文件类型",
    desc: "双击 .torrent 等文件时用 Ghost Downloader 打开",
};
pub const OPTION_URI_SCHEME: OptionCardSpec = OptionCardSpec {
    icon: "LINK",
    title: "处理协议链接",
    desc: "点击 Magnet/eD2k/FTP 链接时唤起 Ghost Downloader",
};
pub const OPTION_URL_SCHEME: OptionCardSpec = OptionCardSpec {
    icon: "GLOBE",
    title: "允许浏览器扩展唤醒",
    desc: "浏览器扩展可通过 ghostdownloader:// 协议启动桌面端",
};
pub const OPTION_ARIA2_RPC: OptionCardSpec = OptionCardSpec {
    icon: "COMMAND_PROMPT",
    title: "Aria2 RPC 兼容",
    desc: "让支持 Aria2 的工具和网站把下载任务发给 Ghost Downloader",
};

// ---------------------------------------------------------------- 文案常量

/// 欢迎页（:280-289）。
pub const WELCOME_TITLE: &str = "欢迎使用 Ghost Downloader";
pub const WELCOME_DESC: &str = "快速、智能的下载管理器。\n接下来的几步将帮助你完成基本配置。";
pub const START_BUTTON: &str = "开始配置";

/// 基本设置页（:320-361）。
pub const BASIC_TITLE: &str = "基本设置";
pub const BASIC_DESC: &str = "选择你喜欢的外观，设置下载文件的保存位置";
pub const PREFERENCES_GROUP: &str = "偏好";
pub const LANGUAGE_ITEM_TITLE: &str = "界面语言";
pub const LANGUAGE_ITEM_DESC: &str = "更改立即生效";
pub const FOLDER_ITEM_TITLE: &str = "下载保存位置";
pub const BROWSE_BUTTON: &str = "浏览...";
pub const FOLDER_DIALOG_TITLE: &str = "选择下载目录";
/// `LANGUAGE_TEXTS` 缺项时的兜底（:336）。
pub const LANGUAGE_FALLBACK_LABEL: &str = "使用系统设置";

/// 浏览器扩展页（:413-449）。
pub const EXTENSION_TITLE: &str = "安装浏览器扩展";
pub const EXTENSION_DESC: &str = "让浏览器中的下载自动接管到 Ghost Downloader";
pub const PREVIEW_FALLBACK_TEXT: &str = "安装教程动图";
pub const MANUAL_INSTALL_TITLE: &str = "手动安装";
pub const MANUAL_INSTALL_HINT: &str = "随桌面端自动更新，适用于所有 Chromium 浏览器";
pub const STORE_CHROME_TITLE: &str = "Chrome 商店";
pub const STORE_EDGE_TITLE: &str = "Edge 商店";
pub const STORE_FIREFOX_TITLE: &str = "Firefox 商店";
pub const EXTENSION_FOOTNOTE: &str = "商店版更新需等待审核，可能滞后于桌面端";

/// 扩展页横幅（:510-577）。
pub const LISTEN_OFF_TEXT: &str = "浏览器扩展未启用，可稍后在设置中开启";
pub const LISTEN_WAITING_TEMPLATE: &str = "正在端口 {} 上等待扩展连接";
pub const CONNECTED_WITH_VERSION_TEMPLATE: &str = "已连接扩展 v{}，最新版本为 v{}";
pub const CONNECTED_WITHOUT_VERSION_TEMPLATE: &str = "已连接扩展，最新版本为 v{}";
pub const PROTOCOL_MISMATCH_TEXT: &str = "协议版本不匹配，商店版可能滞后，请尝试手动安装";

/// 手动安装成功后的兜底提示（:551-556）与失败标题（:560）。
pub const MANUAL_OPEN_TITLE: &str = "请手动打开浏览器";
pub const MANUAL_OPEN_CONTENT: &str = "chrome://extensions 已复制到剪贴板";
pub const EXTRACT_FAILED_TITLE: &str = "解包失败";
/// `InfoBar.info/error(..., duration=5000, ...)`。
pub const TOAST_DURATION_MS: i32 = 5000;

/// 推荐组件页（:592-626）。
pub const RUNTIME_TITLE: &str = "安装推荐组件";
pub const RUNTIME_DESC: &str = "点击下一步将自动安装勾选的组件，稍后可在设置中管理";
pub const RUNTIME_GROUP: &str = "推荐组件";
pub const INSTALLED_LABEL: &str = "已安装";

/// 更多选项页（:655-656）。
pub const ADVANCED_TITLE: &str = "更多选项";
pub const ADVANCED_DESC: &str = "按需开启以下功能，也可以稍后在设置中修改";

/// 完成页（:755-765）。
pub const COMPLETE_TITLE: &str = "一切就绪";
pub const COMPLETE_DESC: &str =
    "Ghost Downloader 已准备好为你工作。\n你可以随时在设置中调整所有选项。";
pub const FINISH_BUTTON: &str = "开始使用";

/// 底部导航（:841-843）。
pub const BACK_BUTTON: &str = "上一步";
pub const SKIP_BUTTON: &str = "跳过全部";
pub const NEXT_BUTTON: &str = "下一步";

// ---------------------------------------------------------------- 语言下拉

/// 下拉候选标签（:335-336）：`LANGUAGE_TEXTS.get(lang, tr("使用系统设置"))`。
///
/// [`crate::settings_spec::LANGUAGE_LABELS`] 的末项（`"Auto"`）就是兜底文案，
/// 所以这里直接复用该表。
pub fn language_items() -> [&'static str; 9] {
    crate::settings_spec::LANGUAGE_LABELS
}

/// `setCurrentIndex(cfg.language.options.index(cfg.language.value))`（:337）。
///
/// Python 在值缺失时抛 `ValueError`；Rust 沿用
/// [`crate::settings_spec::combo_index`] 的兜底 0（已写入模块头的已知偏差）。
pub fn language_index(value: &str) -> i32 {
    combo_index(&LANGUAGE_LITERALS, value) as i32
}

/// `_onLanguageChanged(index)`（:383-387）：越界或与当前值相同 → `None`，
/// 否则返回要写入配置的字面量。
pub fn language_pick(index: i32, current: &str) -> Option<&'static str> {
    let index = usize::try_from(index).ok()?;
    let literal = *LANGUAGE_LITERALS.get(index)?;
    if literal == current {
        None
    } else {
        Some(literal)
    }
}

// ---------------------------------------------------------------- 扩展页

/// `ListenStatus`（`app/browser/listener.py`）：三态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListenStatus {
    Off,
    Failed,
    Listening,
}

/// `InfoBarIcon` 的三档（只用到这三档）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BannerIcon {
    Warning,
    Information,
    Success,
}

impl BannerIcon {
    /// Slint 侧 `InfoBar` 的图标选择键。
    pub fn key(self) -> &'static str {
        match self {
            BannerIcon::Warning => "WARNING",
            BannerIcon::Information => "INFO",
            BannerIcon::Success => "SUCCESS",
        }
    }
}

/// 一条横幅。`_setBanner`（:493-504）恒传 `content=""`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Banner {
    pub icon: BannerIcon,
    pub title: String,
}

impl Banner {
    pub fn new(icon: BannerIcon, title: impl Into<String>) -> Self {
        Self {
            icon,
            title: title.into(),
        }
    }
}

/// `_setBanner` 的固定参数（:499-503）。
pub const BANNER_WIDTH: i32 = PREVIEW_WIDTH;
pub const BANNER_CLOSABLE: bool = false;
pub const BANNER_DURATION_MS: i32 = -1;
pub const BANNER_SHOWS_CONTENT: bool = false;

/// `browserListener.state` 的快照。`error` 只在 `Failed` 时有意义，且原文交给
/// 调用方经 `toLocalizedError` 转换后传入（Rust 无译文层）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListenInfo {
    pub status: ListenStatus,
    pub port: u16,
    pub error: String,
}

impl ListenInfo {
    pub fn off() -> Self {
        Self {
            status: ListenStatus::Off,
            port: 0,
            error: String::new(),
        }
    }

    pub fn listening(port: u16) -> Self {
        Self {
            status: ListenStatus::Listening,
            port,
            error: String::new(),
        }
    }

    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            status: ListenStatus::Failed,
            port: 0,
            error: error.into(),
        }
    }
}

/// `self.tr("正在端口 {} 上等待扩展连接").format(state.port)`（:519）。
pub fn listen_waiting_text(port: u16) -> String {
    LISTEN_WAITING_TEMPLATE.replacen("{}", &port.to_string(), 1)
}

/// `refreshListenState`（:506-521）。
pub fn listen_banner(info: &ListenInfo, error_text: &str) -> Banner {
    match info.status {
        ListenStatus::Off => Banner::new(BannerIcon::Warning, LISTEN_OFF_TEXT),
        ListenStatus::Failed => Banner::new(BannerIcon::Warning, error_text),
        ListenStatus::Listening => {
            Banner::new(BannerIcon::Information, listen_waiting_text(info.port))
        }
    }
}

/// `_setConnectedBanner`（:522-529）。
pub fn connected_banner(version: &str) -> Banner {
    let title = if version.is_empty() {
        CONNECTED_WITHOUT_VERSION_TEMPLATE.replacen("{}", LATEST_EXTENSION_VERSION, 1)
    } else {
        CONNECTED_WITH_VERSION_TEMPLATE
            .replacen("{}", version, 1)
            .replacen("{}", LATEST_EXTENSION_VERSION, 1)
    };
    Banner::new(BannerIcon::Success, title)
}

/// 扩展页的横幅状态机（`_isPaired` + `_banner`）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtensionPageState {
    pub is_paired: bool,
    pub banner: Option<Banner>,
}

impl ExtensionPageState {
    /// `_initWidget` 收尾状态：未配对、无横幅。
    pub fn new() -> Self {
        Self {
            is_paired: false,
            banner: None,
        }
    }

    /// `refreshListenState`：已配对时**直接 return**（:507-508）。
    ///
    /// 返回是否真的换了横幅，便于 Slint 侧判断要不要重建控件。
    pub fn refresh_listen_state(&mut self, info: &ListenInfo, error_text: &str) -> bool {
        if self.is_paired {
            return false;
        }
        self.banner = Some(listen_banner(info, error_text));
        true
    }

    /// `_setConnectedBanner`：只换横幅，不动 `_isPaired`。
    pub fn set_connected_banner(&mut self, version: &str) {
        self.banner = Some(connected_banner(version));
    }

    /// `_onConnectionChanged`（:531-538）：`installType or version` 为真即配对。
    pub fn on_connection_changed(
        &mut self,
        summary: (&str, &str),
        info: &ListenInfo,
        error_text: &str,
    ) {
        let (install_type, version) = summary;
        if !install_type.is_empty() || !version.is_empty() {
            self.is_paired = true;
            self.set_connected_banner(version);
        } else {
            self.is_paired = false;
            self.refresh_listen_state(info, error_text);
        }
    }

    /// `onPairRequested`（:558-561）：先批准配对，再直接置「已连接」。
    pub fn on_pair_requested(&mut self, extension_version: &str) {
        self.is_paired = true;
        self.set_connected_banner(extension_version);
    }

    /// `_onListenStateChanged`（:563-565）：**只有页面可见时**才刷新。
    pub fn on_listen_state_changed(
        &mut self,
        visible: bool,
        info: &ListenInfo,
        error_text: &str,
    ) -> bool {
        if !visible {
            return false;
        }
        self.refresh_listen_state(info, error_text)
    }

    /// `_onProtocolMismatched`（:570-577）。
    pub fn on_protocol_mismatched(&mut self) {
        self.banner = Some(Banner::new(BannerIcon::Warning, PROTOCOL_MISMATCH_TEXT));
    }
}

/// `_onExtensionExtracted` 的兜底提示（:546-556）。
///
/// `open_ok` 是 `openChromiumUrl("chrome://extensions")` 的返回值：打开成功
/// 就没有提示（`None`），失败才复制到剪贴板并弹 5s 的 InfoBar。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManualInstallToast {
    pub title: &'static str,
    pub content: &'static str,
    pub duration_ms: i32,
}

pub fn extension_extracted(open_ok: bool) -> Option<ManualInstallToast> {
    if open_ok {
        return None;
    }
    Some(ManualInstallToast {
        title: MANUAL_OPEN_TITLE,
        content: MANUAL_OPEN_CONTENT,
        duration_ms: TOAST_DURATION_MS,
    })
}

// ---------------------------------------------------------------- 推荐组件

/// `featureService.runtimes()` 的一项。`installed` 镜像 Python 的
/// `bool(runtime.path())`（非空即已安装）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeEntry {
    /// 灰色小字（`runtime.name`），不是标题。
    pub name: String,
    /// `QCoreApplication.translate("BinaryRuntime", runtime.title)`。
    pub title: String,
    /// `QCoreApplication.translate("BinaryRuntime", runtime.description)`。
    pub description: String,
    /// `getattr(FluentIcon, runtime.icon, FluentIcon.APPLICATION)`。
    pub icon: String,
    pub can_install: bool,
    pub is_recommended: bool,
    pub installed: bool,
}

impl RuntimeEntry {
    pub fn new(
        name: impl Into<String>,
        title: impl Into<String>,
        description: impl Into<String>,
        icon: impl Into<String>,
        can_install: bool,
        is_recommended: bool,
        installed: bool,
    ) -> Self {
        Self {
            name: name.into(),
            title: title.into(),
            description: description.into(),
            icon: icon.into(),
            can_install,
            is_recommended,
            installed,
        }
    }
}

/// 一行：`has_checkbox == false` 时 Python 显示「已安装」小字而不是复选框。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeRow {
    pub entry: RuntimeEntry,
    pub has_checkbox: bool,
    pub checked: bool,
}

/// `RuntimeInstallPage.mount`（:613-645）的行装配。
///
/// - 过滤 `rt.canInstall and rt.title`；
/// - `entries.sort(key=lambda rt: not rt.isRecommended)` —— Python `list.sort`
///   稳定，推荐项在前、组内保序（Rust `sort_by_key` 同为稳定排序）；
/// - 已安装 → 无复选框；否则建复选框，初值 `runtime.isRecommended`。
pub fn mount_rows(entries: &[RuntimeEntry]) -> Vec<RuntimeRow> {
    let mut picked: Vec<&RuntimeEntry> = entries
        .iter()
        .filter(|e| e.can_install && !e.title.is_empty())
        .collect();
    picked.sort_by_key(|e| !e.is_recommended);
    picked
        .into_iter()
        .map(|entry| RuntimeRow {
            entry: entry.clone(),
            has_checkbox: !entry.installed,
            checked: !entry.installed && entry.is_recommended,
        })
        .collect()
}

/// `selectedRuntimes()`（:647-648）：勾选且仍有复选框的行。
pub fn selected_runtimes(rows: &[RuntimeRow]) -> Vec<&RuntimeEntry> {
    rows.iter()
        .filter(|row| row.has_checkbox && row.checked)
        .map(|row| &row.entry)
        .collect()
}

/// `_installSelectedRuntimes`（:936-944）：`if runtime.path(): continue`。
pub fn install_targets(rows: &[RuntimeRow]) -> Vec<&RuntimeEntry> {
    selected_runtimes(rows)
        .into_iter()
        .filter(|entry| !entry.installed)
        .collect()
}

/// `_isMounted` 闸门 + 行列表（`_checkBoxes`）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuntimeInstallState {
    mounted: bool,
    rows: Vec<RuntimeRow>,
}

impl RuntimeInstallState {
    pub fn new() -> Self {
        Self {
            mounted: false,
            rows: Vec::new(),
        }
    }

    /// `mount()`：第二次调用直接 return，**不重排也不重建行**。
    pub fn mount(&mut self, entries: &[RuntimeEntry]) -> bool {
        if self.mounted {
            return false;
        }
        self.mounted = true;
        self.rows = mount_rows(entries);
        true
    }

    pub fn is_mounted(&self) -> bool {
        self.mounted
    }

    pub fn rows(&self) -> &[RuntimeRow] {
        &self.rows
    }

    /// `checkBox.setChecked(...)`；无复选框的行返回 `false`（无效果）。
    pub fn set_checked(&mut self, index: usize, checked: bool) -> bool {
        match self.rows.get_mut(index) {
            Some(row) if row.has_checkbox => {
                let changed = row.checked != checked;
                row.checked = checked;
                changed
            }
            _ => false,
        }
    }

    pub fn selected(&self) -> Vec<&RuntimeEntry> {
        selected_runtimes(&self.rows)
    }

    pub fn install_targets(&self) -> Vec<&RuntimeEntry> {
        install_targets(&self.rows)
    }
}

// ---------------------------------------------------------------- 更多选项

/// `save()` 里「关联」两档配置的写入意图。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssocKind {
    FileTypes,
    UriSchemes,
}

impl AssocKind {
    /// `UserConfig.json` 里的键名（`pack.config.associateFileTypes` 等）。
    pub fn key(self) -> &'static str {
        match self {
            AssocKind::FileTypes => "associateFileTypes",
            AssocKind::UriSchemes => "associateUriSchemes",
        }
    }
}

/// 一个包的关联配置项是否存在（Python 判 `config is not None and
/// config.associateFileTypes is not None`）。`Some(v)` 的 `v` 是**当前值**，
/// 保存时被开关值覆盖。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PackAssoc {
    pub file_types: Option<bool>,
    pub uri_schemes: Option<bool>,
}

impl PackAssoc {
    pub fn none() -> Self {
        Self {
            file_types: None,
            uri_schemes: None,
        }
    }

    pub fn both(file_types: bool, uri_schemes: bool) -> Self {
        Self {
            file_types: Some(file_types),
            uri_schemes: Some(uri_schemes),
        }
    }
}

/// 一次 `cfg.set` 的意图。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PackAssocWrite {
    pub pack_index: usize,
    pub kind: AssocKind,
    pub value: bool,
}

/// `urlSchemeCard` 的落盘：先调注册/反注册（副作用），再写配置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UrlSchemePlan {
    /// `registerUrlScheme()` / `unregisterUrlScheme()`。
    pub register: bool,
    /// `cfg.set(cfg.isUrlSchemeRegistered, ...)` 的新值。
    pub registered: bool,
}

/// `AdvancedOptionsPage.save()`（:718-745）的完整落盘计划。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SavePlan {
    /// `None` = 与当前配置相同，Python 不调 `setRunAtLogin` 也不写配置。
    pub set_run_at_login: Option<bool>,
    pub clipboard_listener: bool,
    pub category_enabled: bool,
    /// 按 `packs` 顺序、每个包内「文件类型 → 协议」。
    pub pack_assoc: Vec<PackAssocWrite>,
    /// `None` = macOS（无 `urlSchemeCard`）。
    pub url_scheme: Option<UrlSchemePlan>,
    pub aria2_rpc: bool,
}

/// `AdvancedOptionsPage` 的七张开关卡状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdvancedOptions {
    /// `sys.platform == "darwin"`：为真时后三张卡**不存在**（:682-692）。
    pub is_darwin: bool,
    pub run_at_login: bool,
    pub clipboard_listener: bool,
    pub category_enabled: bool,
    pub file_assoc: bool,
    pub uri_scheme_assoc: bool,
    pub url_scheme: bool,
    pub aria2_rpc: bool,
}

impl AdvancedOptions {
    /// 全关状态；`is_darwin` 决定后三张卡是否装配。
    pub fn new(is_darwin: bool) -> Self {
        Self {
            is_darwin,
            run_at_login: false,
            clipboard_listener: false,
            category_enabled: false,
            file_assoc: false,
            uri_scheme_assoc: false,
            url_scheme: false,
            aria2_rpc: false,
        }
    }

    /// `_initLayout` 的卡片顺序（:710-717）：`None` 项被跳过。
    pub fn cards(&self) -> Vec<(OptionCardSpec, bool)> {
        let mut cards = vec![
            (OPTION_RUN_AT_LOGIN, self.run_at_login),
            (OPTION_CLIPBOARD, self.clipboard_listener),
            (OPTION_CATEGORY, self.category_enabled),
        ];
        if !self.is_darwin {
            cards.push((OPTION_FILE_ASSOC, self.file_assoc));
            cards.push((OPTION_URI_SCHEME, self.uri_scheme_assoc));
            cards.push((OPTION_URL_SCHEME, self.url_scheme));
        }
        cards.push((OPTION_ARIA2_RPC, self.aria2_rpc));
        cards
    }

    pub fn card_titles(&self) -> Vec<&'static str> {
        self.cards()
            .into_iter()
            .map(|(spec, _)| spec.title)
            .collect()
    }

    /// `save()`：只产出计划，副作用（`setRunAtLogin` / 注册协议 / 写配置）
    /// 留给调用方——与 `proxy::validate` 返回 `Result` 同风格。
    pub fn save(&self, current_run_at_login: bool, packs: &[PackAssoc]) -> SavePlan {
        let mut plan = SavePlan {
            set_run_at_login: if self.run_at_login == current_run_at_login {
                None
            } else {
                Some(self.run_at_login)
            },
            clipboard_listener: self.clipboard_listener,
            category_enabled: self.category_enabled,
            pack_assoc: Vec::new(),
            url_scheme: None,
            aria2_rpc: self.aria2_rpc,
        };

        if !self.is_darwin {
            for (pack_index, pack) in packs.iter().enumerate() {
                if pack.file_types.is_some() {
                    plan.pack_assoc.push(PackAssocWrite {
                        pack_index,
                        kind: AssocKind::FileTypes,
                        value: self.file_assoc,
                    });
                }
                if pack.uri_schemes.is_some() {
                    plan.pack_assoc.push(PackAssocWrite {
                        pack_index,
                        kind: AssocKind::UriSchemes,
                        value: self.uri_scheme_assoc,
                    });
                }
            }
            plan.url_scheme = Some(UrlSchemePlan {
                register: self.url_scheme,
                registered: self.url_scheme,
            });
        }

        plan
    }
}

// ---------------------------------------------------------------- 窗口状态机

/// `_initContent` 的 `addWidget` 顺序（:834-839）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Welcome,
    BasicSettings,
    BrowserExtension,
    RuntimeInstall,
    AdvancedOptions,
    Complete,
}

impl Page {
    pub const ALL: [Page; PAGE_COUNT] = [
        Page::Welcome,
        Page::BasicSettings,
        Page::BrowserExtension,
        Page::RuntimeInstall,
        Page::AdvancedOptions,
        Page::Complete,
    ];

    pub fn index(self) -> usize {
        match self {
            Page::Welcome => 0,
            Page::BasicSettings => 1,
            Page::BrowserExtension => 2,
            Page::RuntimeInstall => 3,
            Page::AdvancedOptions => 4,
            Page::Complete => 5,
        }
    }

    pub fn from_index(index: usize) -> Option<Page> {
        Page::ALL.get(index).copied()
    }
}

/// 窗口对外的一次副作用。Slint 侧按序消费；`OobeState` 自己不碰任何外部服务。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OobeEffect {
    /// `stackedWidget.setCurrentIndex(index, isBack=back)`。
    ShowPage { index: usize, back: bool },
    /// `browserExtensionPage.refreshListenState()`。
    RefreshListenState,
    /// `runtimeInstallPage.mount()`。
    MountRuntimes,
    /// `_installSelectedRuntimes()`。
    InstallSelectedRuntimes,
    /// `advancedOptionsPage.save()`。
    SaveAdvancedOptions,
    /// `cfg.set(cfg.hasCompletedOobe, True)`。
    MarkCompleted,
    /// `self.finished.emit()`。
    Finished,
    /// `self.close()`。
    Close,
}

/// `_refreshNavigation`（:904-912）的四个可观察量。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Navigation {
    pub back_visible: bool,
    pub next_visible: bool,
    pub skip_visible: bool,
    pub pager_index: usize,
}

/// `isFirst = i == 0`、`isLast = i == PAGE_COUNT - 1`；`back`/`next` 可见性是
/// `not isFirst and not isLast`，`skip` 只在首页可见。
pub fn navigation(index: usize) -> Navigation {
    let is_first = index == 0;
    let is_last = index == PAGE_COUNT - 1;
    Navigation {
        back_visible: !is_first && !is_last,
        next_visible: !is_first && !is_last,
        skip_visible: is_first,
        pager_index: index,
    }
}

/// `OobeWindow` 的 `_currentIndex` + `_isFinished`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OobeState {
    current_index: usize,
    is_finished: bool,
}

impl Default for OobeState {
    fn default() -> Self {
        Self::new()
    }
}

impl OobeState {
    pub fn new() -> Self {
        Self {
            current_index: 0,
            is_finished: false,
        }
    }

    pub fn current_index(&self) -> usize {
        self.current_index
    }

    pub fn page(&self) -> Page {
        Page::from_index(self.current_index).unwrap_or(Page::Welcome)
    }

    pub fn is_finished(&self) -> bool {
        self.is_finished
    }

    pub fn navigation(&self) -> Navigation {
        navigation(self.current_index)
    }

    /// `_onNextClicked`（:914-927）。欢迎页的「开始配置」也接在这个槽上。
    pub fn go_next(&mut self) -> Vec<OobeEffect> {
        let mut effects = Vec::new();
        if self.current_index >= PAGE_COUNT - 1 {
            return effects;
        }
        // 关键顺序：i == 3 时**先装再自增**。
        if self.current_index == 3 {
            effects.push(OobeEffect::InstallSelectedRuntimes);
        }
        self.current_index += 1;
        effects.push(OobeEffect::ShowPage {
            index: self.current_index,
            back: false,
        });
        if self.current_index == 2 {
            effects.push(OobeEffect::RefreshListenState);
        } else if self.current_index == 3 {
            effects.push(OobeEffect::MountRuntimes);
        }
        effects
    }

    /// `_onBackClicked`（:929-934）：**不**刷新监听、**不**挂载组件。
    pub fn back(&mut self) -> Vec<OobeEffect> {
        if self.current_index == 0 {
            return Vec::new();
        }
        self.current_index -= 1;
        vec![OobeEffect::ShowPage {
            index: self.current_index,
            back: true,
        }]
    }

    /// `_finish`（:946-953）：存高级选项 → 标记完成 → 发信号 → 关窗。
    pub fn finish(&mut self) -> Vec<OobeEffect> {
        if self.is_finished {
            return Vec::new();
        }
        self.is_finished = true;
        vec![
            OobeEffect::SaveAdvancedOptions,
            OobeEffect::MarkCompleted,
            OobeEffect::Finished,
            OobeEffect::Close,
        ]
    }

    /// `closeEvent`（:966-971）：用户点 X。**不**保存高级选项。
    pub fn close(&mut self) -> Vec<OobeEffect> {
        if self.is_finished {
            return Vec::new();
        }
        self.is_finished = true;
        vec![OobeEffect::MarkCompleted, OobeEffect::Finished]
    }

    /// `_onLanguageChanged`（:890-902）：整块重建 content 后恢复**原页码**。
    pub fn on_language_changed(&self) -> Vec<OobeEffect> {
        vec![OobeEffect::ShowPage {
            index: self.current_index,
            back: false,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_runtimes() -> Vec<RuntimeEntry> {
        vec![
            RuntimeEntry::new("ffmpeg", "FFmpeg", "转码", "VIDEO", true, false, false),
            RuntimeEntry::new("aria2", "Aria2", "RPC", "ROBOT", true, true, false),
            RuntimeEntry::new("yt-dlp", "yt-dlp", "解析", "CLOUD", false, true, false),
            RuntimeEntry::new("no-title", "", "缺 title", "APPLICATION", true, true, false),
            RuntimeEntry::new("7z", "7-Zip", "解压", "ZIP_FOLDER", true, false, true),
        ]
    }

    #[test]
    fn window_geometry_matches_python() {
        const { assert!(WINDOW_WIDTH == 960 && WINDOW_HEIGHT == 600) };
        const { assert!(PREVIEW_WIDTH == 592 && PREVIEW_HEIGHT == 333) };
        const { assert!(STORE_COLUMN_WIDTH == 280) };
        const { assert!(BANNER_WIDTH == PREVIEW_WIDTH) };
        const { assert!(PAGE_COUNT == 6) };
        let nav = navigation(0);
        assert!(nav.skip_visible, "首页只显示「跳过全部」");
    }

    #[test]
    fn page_order_matches_init_content() {
        let indexes: Vec<usize> = Page::ALL.iter().map(|page| page.index()).collect();
        assert_eq!(indexes, vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(Page::ALL[0], Page::Welcome);
        assert_eq!(Page::ALL[2], Page::BrowserExtension);
        assert_eq!(Page::ALL[3], Page::RuntimeInstall);
        assert_eq!(Page::from_index(6), None);
    }

    #[test]
    fn navigation_on_first_page() {
        let nav = navigation(0);
        assert_eq!(
            nav,
            Navigation {
                back_visible: false,
                next_visible: false,
                skip_visible: true,
                pager_index: 0
            }
        );
    }

    #[test]
    fn navigation_on_last_page() {
        let nav = navigation(PAGE_COUNT - 1);
        assert!(!nav.back_visible && !nav.next_visible && !nav.skip_visible);
        assert_eq!(nav.pager_index, PAGE_COUNT - 1);
    }

    #[test]
    fn navigation_in_the_middle() {
        let nav = navigation(2);
        assert!(nav.back_visible && nav.next_visible);
        assert!(!nav.skip_visible);
    }

    #[test]
    fn next_from_welcome_lands_on_basic_without_side_effects() {
        let mut state = OobeState::new();
        let effects = state.go_next();
        assert_eq!(state.current_index(), 1);
        assert_eq!(
            effects,
            vec![OobeEffect::ShowPage {
                index: 1,
                back: false
            }]
        );
    }

    #[test]
    fn next_into_extension_refreshes_listen_state() {
        let mut state = OobeState::new();
        state.go_next();
        let effects = state.go_next();
        assert_eq!(
            effects,
            vec![
                OobeEffect::ShowPage {
                    index: 2,
                    back: false
                },
                OobeEffect::RefreshListenState,
            ]
        );
    }

    #[test]
    fn next_into_runtime_mounts_rows() {
        let mut state = OobeState::new();
        state.go_next();
        state.go_next();
        let effects = state.go_next();
        assert_eq!(
            effects,
            vec![
                OobeEffect::ShowPage {
                    index: 3,
                    back: false
                },
                OobeEffect::MountRuntimes
            ]
        );
    }

    #[test]
    fn next_from_runtime_installs_before_advancing() {
        let mut state = OobeState::new();
        for _ in 0..3 {
            state.go_next();
        }
        assert_eq!(state.current_index(), 3);
        let effects = state.go_next();
        assert_eq!(
            effects,
            vec![
                OobeEffect::InstallSelectedRuntimes,
                OobeEffect::ShowPage {
                    index: 4,
                    back: false
                },
            ],
            "Python 在 i == 3 时先 _installSelectedRuntimes() 再自增"
        );
        assert_eq!(state.current_index(), 4);
    }

    #[test]
    fn next_on_last_page_is_noop() {
        let mut state = OobeState::new();
        while state.current_index() < PAGE_COUNT - 1 {
            state.go_next();
        }
        assert!(state.go_next().is_empty());
        assert_eq!(state.current_index(), PAGE_COUNT - 1);
    }

    #[test]
    fn back_on_first_page_is_noop() {
        let mut state = OobeState::new();
        assert!(state.back().is_empty());
        assert_eq!(state.current_index(), 0);
    }

    #[test]
    fn back_does_not_refresh_or_mount() {
        let mut state = OobeState::new();
        for _ in 0..4 {
            state.go_next();
        }
        assert_eq!(state.current_index(), 4);
        assert_eq!(
            state.back(),
            vec![OobeEffect::ShowPage {
                index: 3,
                back: true
            }]
        );
        assert_eq!(
            state.back(),
            vec![OobeEffect::ShowPage {
                index: 2,
                back: true
            }]
        );
    }

    #[test]
    fn finish_saves_marks_finishes_and_closes() {
        let mut state = OobeState::new();
        assert_eq!(
            state.finish(),
            vec![
                OobeEffect::SaveAdvancedOptions,
                OobeEffect::MarkCompleted,
                OobeEffect::Finished,
                OobeEffect::Close,
            ]
        );
        assert!(state.is_finished());
    }

    #[test]
    fn close_skips_save_but_marks_completed() {
        let mut state = OobeState::new();
        assert_eq!(
            state.close(),
            vec![OobeEffect::MarkCompleted, OobeEffect::Finished]
        );
        assert!(state.is_finished());
    }

    #[test]
    fn finish_and_close_share_the_is_finished_gate() {
        let mut state = OobeState::new();
        state.finish();
        assert!(state.close().is_empty());
        assert!(state.finish().is_empty());

        let mut state = OobeState::new();
        state.close();
        assert!(
            state.finish().is_empty(),
            "closeEvent 先到时 _finish 不再保存高级选项"
        );
    }

    #[test]
    fn language_change_keeps_current_page() {
        let mut state = OobeState::new();
        for _ in 0..4 {
            state.go_next();
        }
        let effects = state.on_language_changed();
        assert_eq!(state.current_index(), 4);
        assert_eq!(
            effects,
            vec![OobeEffect::ShowPage {
                index: 4,
                back: false
            }]
        );
    }

    #[test]
    fn theme_cards_are_light_dark_auto() {
        let values: Vec<&str> = ThemeMode::ALL.iter().map(|mode| mode.value()).collect();
        assert_eq!(values, vec!["Light", "Dark", "Auto"]);
        let labels: Vec<&str> = ThemeMode::ALL.iter().map(|mode| mode.label()).collect();
        assert_eq!(labels, vec!["浅色", "深色", "跟随系统"]);
        let modes: Vec<&str> = ThemeMode::ALL
            .iter()
            .map(|mode| mode.preview_mode())
            .collect();
        assert_eq!(modes, vec!["light", "dark", "auto"]);
    }

    #[test]
    fn selected_theme_index_follows_config_value() {
        assert_eq!(selected_theme_index("Light"), Some(0));
        assert_eq!(selected_theme_index("Dark"), Some(1));
        assert_eq!(selected_theme_index("Auto"), Some(2));
        assert_eq!(selected_theme_index("System"), None);
        assert_eq!(ThemeMode::from_value("Dark"), Some(ThemeMode::Dark));
    }

    #[test]
    fn auto_preview_splits_into_two_panes() {
        let panes = preview_panes(ThemeMode::Auto, 100.0, 50.0);
        assert_eq!(
            panes,
            vec![
                (Rect::new(0.0, 0.0, 50.0, 50.0), false),
                (Rect::new(50.0, 0.0, 50.0, 50.0), true),
            ]
        );
        assert_eq!(preview_panes(ThemeMode::Light, 100.0, 50.0).len(), 1);
        assert!(!preview_panes(ThemeMode::Light, 100.0, 50.0)[0].1);
        assert!(preview_panes(ThemeMode::Dark, 100.0, 50.0)[0].1);
    }

    #[test]
    fn action_card_heights_follow_hint() {
        let manual = manual_install_card();
        assert!(manual.has_hint());
        assert_eq!(manual.fixed_height(), None);
        assert_eq!(manual.minimum_height(), Some(80));
        assert!(manual.recommended);
        assert_eq!(manual.url, None);

        let stores = store_cards();
        assert_eq!(stores.len(), 3);
        for card in stores {
            assert!(!card.has_hint());
            assert_eq!(card.fixed_height(), Some(48));
            assert_eq!(card.minimum_height(), None);
            assert!(!card.recommended);
        }
        assert_eq!(stores[0].url, Some(CHROME_WEBSTORE_URL));
        assert_eq!(stores[1].url, Some(EDGE_ADDONS_URL));
        assert_eq!(stores[2].url, Some(FIREFOX_ADDONS_URL));
        assert_eq!(stores[0].title, "Chrome 商店");
    }

    #[test]
    fn option_card_geometry_matches_python() {
        const { assert!(OPTION_CARD_HEIGHT == 64) };
        const {
            assert!(
                OPTION_CARD_MARGINS.0 == 18
                    && OPTION_CARD_MARGINS.1 == 8
                    && OPTION_CARD_MARGINS.2 == 18
                    && OPTION_CARD_MARGINS.3 == 8
            )
        };
        const { assert!(OPTION_CARD_SPACING == 14) };
        const { assert!(OPTION_CARD_TEXT_SPACING == 0) };
        const {
            assert!(
                ACTION_CARD_MARGINS.0 == 14
                    && ACTION_CARD_MARGINS.1 == 8
                    && ACTION_CARD_MARGINS.2 == 14
                    && ACTION_CARD_MARGINS.3 == 8
            )
        };
        const { assert!(ACTION_CARD_TEXT_SPACING == 2) };
        const { assert!(ACTION_CARD_CHEVRON_SIZE == 12) };
        assert_eq!(OPTION_RUN_AT_LOGIN.title, "开机自启");
        assert_eq!(OPTION_ARIA2_RPC.icon, "COMMAND_PROMPT");
    }

    #[test]
    fn listen_banner_off_is_a_warning() {
        let banner = listen_banner(&ListenInfo::off(), "unused");
        assert_eq!(banner.icon, BannerIcon::Warning);
        assert_eq!(banner.title, "浏览器扩展未启用，可稍后在设置中开启");
        assert_eq!(banner.icon.key(), "WARNING");
    }

    #[test]
    fn listen_banner_failed_passes_localized_error_through() {
        let banner = listen_banner(&ListenInfo::failed("端口占用"), "端口 1234 已被占用");
        assert_eq!(banner.icon, BannerIcon::Warning);
        assert_eq!(banner.title, "端口 1234 已被占用");
    }

    #[test]
    fn listen_banner_listening_formats_port() {
        let banner = listen_banner(&ListenInfo::listening(1234), "");
        assert_eq!(banner.icon, BannerIcon::Information);
        assert_eq!(banner.title, "正在端口 1234 上等待扩展连接");
        assert_eq!(listen_waiting_text(80), "正在端口 80 上等待扩展连接");
    }

    #[test]
    fn connected_banner_with_and_without_version() {
        let with = connected_banner("2.1.0");
        assert_eq!(with.icon, BannerIcon::Success);
        assert_eq!(with.title, "已连接扩展 v2.1.0，最新版本为 v2.2.0");

        let without = connected_banner("");
        assert_eq!(without.title, "已连接扩展，最新版本为 v2.2.0");
    }

    #[test]
    fn refresh_listen_state_is_ignored_when_paired() {
        let mut page = ExtensionPageState::new();
        page.on_pair_requested("2.2.0");
        let before = page.banner.clone();
        assert!(!page.refresh_listen_state(&ListenInfo::off(), ""));
        assert_eq!(page.banner, before);
    }

    #[test]
    fn connection_changed_toggles_paired_and_banner() {
        let mut page = ExtensionPageState::new();
        page.on_connection_changed(("", ""), &ListenInfo::listening(16800), "");
        assert!(!page.is_paired);
        assert_eq!(page.banner.as_ref().unwrap().icon, BannerIcon::Information);

        page.on_connection_changed(("store", "2.1.0"), &ListenInfo::off(), "");
        assert!(page.is_paired);
        assert_eq!(
            page.banner.as_ref().unwrap().title,
            "已连接扩展 v2.1.0，最新版本为 v2.2.0"
        );

        page.on_connection_changed(("", ""), &ListenInfo::off(), "");
        assert!(!page.is_paired, "断开后 refreshListenState 重新接管横幅");
        assert_eq!(page.banner.as_ref().unwrap().title, LISTEN_OFF_TEXT);
    }

    #[test]
    fn protocol_mismatch_replaces_banner() {
        let mut page = ExtensionPageState::new();
        page.on_protocol_mismatched();
        let banner = page.banner.unwrap();
        assert_eq!(banner.icon, BannerIcon::Warning);
        assert_eq!(
            banner.title,
            "协议版本不匹配，商店版可能滞后，请尝试手动安装"
        );
    }

    #[test]
    fn listen_state_changed_only_when_visible() {
        let mut page = ExtensionPageState::new();
        assert!(!page.on_listen_state_changed(false, &ListenInfo::off(), ""));
        assert_eq!(page.banner, None);
        assert!(page.on_listen_state_changed(true, &ListenInfo::off(), ""));
        assert!(page.banner.is_some());
    }

    #[test]
    fn extension_extracted_toast_only_when_open_fails() {
        assert_eq!(extension_extracted(true), None);
        let toast = extension_extracted(false).unwrap();
        assert_eq!(toast.title, "请手动打开浏览器");
        assert_eq!(toast.content, "chrome://extensions 已复制到剪贴板");
        assert_eq!(toast.duration_ms, 5000);
        const { assert!(!BANNER_CLOSABLE) };
        const { assert!(BANNER_DURATION_MS == -1) };
        const { assert!(!BANNER_SHOWS_CONTENT) };
    }

    #[test]
    fn mount_rows_put_recommended_first_and_hide_installed_checkbox() {
        let rows = mount_rows(&sample_runtimes());
        let titles: Vec<&str> = rows.iter().map(|row| row.entry.title.as_str()).collect();
        assert_eq!(titles, vec!["Aria2", "FFmpeg", "7-Zip"]);

        assert!(rows[0].has_checkbox && rows[0].checked, "推荐项默认勾选");
        assert!(rows[1].has_checkbox && !rows[1].checked);
        assert!(
            !rows[2].has_checkbox,
            "已安装时 Python 只显示「已安装」小字"
        );
        assert!(!rows[2].checked);
    }

    #[test]
    fn mount_is_idempotent() {
        let mut state = RuntimeInstallState::new();
        assert!(state.mount(&sample_runtimes()));
        assert!(state.is_mounted());
        let count = state.rows().len();
        assert!(!state.mount(&[]), "第二次 mount 直接 return，不重建行");
        assert_eq!(state.rows().len(), count);
    }

    #[test]
    fn install_targets_skip_rows_without_checkbox() {
        let mut state = RuntimeInstallState::new();
        state.mount(&sample_runtimes());
        assert_eq!(state.selected().len(), 1);
        assert!(state.set_checked(1, true));
        assert!(!state.set_checked(1, true), "值未变 → 无效果");
        let targets: Vec<&str> = state
            .install_targets()
            .into_iter()
            .map(|e| e.title.as_str())
            .collect();
        assert_eq!(targets, vec!["Aria2", "FFmpeg"]);
        assert!(!state.set_checked(2, true), "无复选框的行不响应勾选");
        assert!(!state.set_checked(9, true), "越界静默忽略");
    }

    #[test]
    fn advanced_cards_drop_platform_only_entries_on_mac() {
        let linux = AdvancedOptions::new(false);
        assert_eq!(
            linux.card_titles(),
            vec![
                "开机自启",
                "剪贴板监听",
                "自动分类保存",
                "关联文件类型",
                "处理协议链接",
                "允许浏览器扩展唤醒",
                "Aria2 RPC 兼容",
            ]
        );
        let mac = AdvancedOptions::new(true);
        assert_eq!(
            mac.card_titles(),
            vec!["开机自启", "剪贴板监听", "自动分类保存", "Aria2 RPC 兼容"]
        );
    }

    #[test]
    fn save_plan_matches_python_writes() {
        let options = AdvancedOptions {
            is_darwin: false,
            run_at_login: true,
            clipboard_listener: false,
            category_enabled: true,
            file_assoc: true,
            uri_scheme_assoc: false,
            url_scheme: false,
            aria2_rpc: true,
        };
        let packs = [
            PackAssoc::both(true, true),
            PackAssoc::none(),
            PackAssoc {
                file_types: Some(false),
                uri_schemes: None,
            },
        ];
        let plan = options.save(false, &packs);

        assert_eq!(plan.set_run_at_login, Some(true));
        assert!(!plan.clipboard_listener);
        assert!(plan.category_enabled);
        assert_eq!(
            plan.pack_assoc,
            vec![
                PackAssocWrite {
                    pack_index: 0,
                    kind: AssocKind::FileTypes,
                    value: true
                },
                PackAssocWrite {
                    pack_index: 0,
                    kind: AssocKind::UriSchemes,
                    value: false
                },
                PackAssocWrite {
                    pack_index: 2,
                    kind: AssocKind::FileTypes,
                    value: true
                },
            ]
        );
        assert_eq!(
            plan.url_scheme,
            Some(UrlSchemePlan {
                register: false,
                registered: false
            })
        );
        assert!(plan.aria2_rpc);
        assert_eq!(AssocKind::FileTypes.key(), "associateFileTypes");
    }

    #[test]
    fn save_plan_on_darwin_skips_association_writes() {
        let options = AdvancedOptions {
            is_darwin: true,
            url_scheme: true,
            ..AdvancedOptions::new(true)
        };
        let plan = options.save(false, &[PackAssoc::both(true, true)]);
        assert!(
            plan.pack_assoc.is_empty(),
            "macOS 无 fileAssocCard/urlSchemeCard"
        );
        assert_eq!(plan.url_scheme, None);
    }

    #[test]
    fn save_plan_skips_run_at_login_when_unchanged() {
        let options = AdvancedOptions {
            run_at_login: true,
            ..AdvancedOptions::new(false)
        };
        let plan = options.save(true, &[]);
        assert_eq!(
            plan.set_run_at_login, None,
            "值相同 → Python 不调 setRunAtLogin"
        );
        assert_eq!(
            plan.url_scheme,
            Some(UrlSchemePlan {
                register: false,
                registered: false
            })
        );
    }

    #[test]
    fn language_index_and_pick_follow_combo_semantics() {
        assert_eq!(language_index("zh_CN"), 0);
        assert_eq!(language_index("en_US"), 3);
        assert_eq!(language_index("Auto"), 8);
        assert_eq!(
            language_index("nope"),
            0,
            "Python 抛 ValueError，Rust 兜底 0"
        );

        assert_eq!(language_pick(3, "zh_CN"), Some("en_US"));
        assert_eq!(language_pick(3, "en_US"), None, "同值不发信号");
        assert_eq!(language_pick(-1, "zh_CN"), None);
        assert_eq!(language_pick(9, "zh_CN"), None);

        let items = language_items();
        assert_eq!(items.len(), LANGUAGE_LITERALS.len());
        assert_eq!(items[8], LANGUAGE_FALLBACK_LABEL);
    }
}
