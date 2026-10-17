//! 设置页卡片清单（第 62 步）：`app/view/pages/setting_page.py::_initCards`
//! （949 行 `setting_cards.py` 的控件实现 + 191 行 `card_groups.py` 的容器）
//! 的**声明式镜像**。
//!
//! 目的不是重画控件，而是把「哪张卡在哪个分组、第几位、什么标题副标题、
//! 绑哪个配置键、缺省值多少、范围多少、下拉候选是什么」抽成一张可断言
//! 的表——这样 Slint 侧可以数据驱动渲染，`UserConfig.json` 两侧也能互相
//! 读写（配置键名/缺省值/取值范围必须一致）。
//!
//! 实证口径（2026-10-17 实读源码 + 在 venv 里跑出枚举序）：
//!
//! - 卡片标题/副标题取 `.tr` 字面量（setting_page.py:83-388）。
//! - `RangeConfigItem` 的范围是**配置口径**；`SpinBoxSettingCard` 用
//!   `division` 换算成显示口径（:66-67 `setRange(int(r[0]*division), ...)`），
//!   `PercentSpinBoxSettingCard` 固定 ×100（:105-107）。本表存**显示口径**
//!   的 `min/max`（Slint 直接可用）+ `Scale` 双向换算。
//! - `OptionsConfigItem` 存**枚举字面量**，UI 发索引（qfluentwidgets
//!   ComboBoxSettingCard 实证：索引 i ⇄ options[i]）；缺省值实测：
//!   `themeMode.options == [Light, Dark, Auto]` 且 `value == Auto`（索引 2）、
//!   `language.options == [zh_CN, zh_TW, yue_HK, en_US, ja_JP, ru_RU, pt_BR,
//!   es_ES, Auto]` 且 `value == Auto`（索引 8）、`LANGUAGE_TEXTS` 无 AUTO 项
//!   → 标签回退 `self.tr("使用系统设置")`。
//! - `backgroundEffect` 是**平台分支键**：win32 候选
//!   `[Acrylic, Mica, MicaAlt, Aero, None]`（Win10 缺省 None，否则 Mica）、
//!   darwin 候选 `[Acrylic, None]` 缺省 Acrylic；Linux/Android 上**该键根本
//!   不存在**（cfg.py:283-294 的 if/elif 无 else）。
//! - 与平台相关的卡：`association` 组整体在 darwin 上不创建
//!   （setting_page.py:64-65）、`closeMode`/`storage_mode` 在 Android 上不创建
//!   （:344-363）、三个 dock/菜单栏速度卡仅 darwin（:291-307）。
//!
//! 本表**不含**功能包动态卡（关联组的「关联 X 文件 / 处理 X 链接」由
//! `_featureService.packs` 逐包生成，setting_page.py:225-246）与运行时卡
//! （`RuntimeCard`，feature_service.py:142-144 按包创建）——它们的数据源是
//! 功能包而非配置，见 [`pack_association_card`]。

use crate::settings_group::{self, CardText, GroupCards};

/// 配置键（`ConfigItem(section, key, ...)` 的前两个实参）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigKey {
    pub section: &'static str,
    pub key: &'static str,
}

impl ConfigKey {
    const fn new(section: &'static str, key: &'static str) -> Self {
        Self { section, key }
    }
}

/// 配置值 ⇄ 显示值的换算（镜像 `SpinBoxSettingCard.division` 与
/// `PercentSpinBoxSettingCard` 的固定 ×100）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    /// `division=1`：显示值即配置值。
    One,
    /// `division=1/1024`：配置存字节、显示 KB（SpeedLimitation）。
    BytesPerKb,
    /// `PercentSpinBoxSettingCard`：配置是小数、显示是百分数（DpiScale）。
    FractionToPercent,
}

impl Scale {
    /// 配置值 → 显示值。Python 侧是 `int(value * division)` / `value * 100`
    /// （`1/1024` 在二进制里精确，无浮点尾差）。
    pub fn to_display(self, config: f64) -> i64 {
        match self {
            Scale::One => config as i64,
            Scale::BytesPerKb => (config * (1.0 / 1024.0)) as i64,
            Scale::FractionToPercent => (config * 100.0).round() as i64,
        }
    }

    /// 显示值 → 配置值。Python 侧是 `int(spin.value() / division)` /
    /// `spin.value() / 100`（后者保持 float，写盘走 `str(float)`）。
    pub fn to_config(self, display: i64) -> f64 {
        match self {
            Scale::One => display as f64,
            Scale::BytesPerKb => (display as f64) * 1024.0,
            Scale::FractionToPercent => (display as f64) / 100.0,
        }
    }
}

/// 控件种类（决定 Slint 侧渲染哪个 delegate）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Control {
    /// 整数 SpinBox：`min/max` 为显示口径，`step` 为 `singleStep`。
    Range {
        min: i64,
        max: i64,
        step: i64,
        unit: &'static str,
        scale: Scale,
    },
    /// 开关。
    Switch,
    /// 下拉框：`literals` 是配置字面量、`labels` 是 UI 文案（等长）。
    Combo {
        literals: &'static [&'static str],
        labels: &'static [&'static str],
    },
    /// 单行输入框（`isPassword` 走 `PasswordLineEdit`）。
    Line {
        placeholder: &'static str,
        password: bool,
    },
    /// 目录选择（`FolderPicker` + 「恢复默认路径」按钮）。
    Folder,
    /// 纯动作卡（PushSettingCard / PrimaryPushSettingCard / HyperlinkCard）。
    Button,
    /// 已单独实现的复合卡（Proxy / 身份 / 标头预设 / 分类规则 / 配对令牌 /
    /// 商店安装 / Chromium 安装 / 数据存储模式 / 关于组四卡）。
    Custom,
}

/// 平台可见性（Python 侧是 `if sys.platform == ...` / `if not IS_ANDROID`
/// 决定**是否创建**，不是隐藏）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platforms {
    All,
    NotDarwin,
    NotAndroid,
    Win32,
    Darwin,
}

impl Platforms {
    pub fn applies(self, platform: Platform) -> bool {
        match self {
            Platforms::All => true,
            Platforms::NotDarwin => platform != Platform::MacOs,
            Platforms::NotAndroid => platform != Platform::Android,
            Platforms::Win32 => platform == Platform::Windows,
            Platforms::Darwin => platform == Platform::MacOs,
        }
    }
}

/// 目标平台。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Linux,
    Windows,
    MacOs,
    Android,
}

impl Platform {
    /// 编译目标对应的平台（`cfg!` 而非运行时探测：设置页的平台分支在
    /// Python 侧也是 import 期决定）。
    pub fn current() -> Self {
        if cfg!(target_os = "windows") {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(target_os = "android") {
            Platform::Android
        } else {
            Platform::Linux
        }
    }
}

/// 一张设置卡。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CardSpec {
    /// Rust 侧稳定标识（用于回调分派；Python 侧对应 `self.<name>Card` 属性名）。
    pub id: &'static str,
    pub group: &'static str,
    pub title: &'static str,
    pub content: &'static str,
    pub control: Control,
    /// 绑定的配置键（纯动作卡为 `None`）。
    pub config: Option<ConfigKey>,
    /// 配置口径缺省值（`RangeConfigItem`/`ConfigItem` 的第三实参）。
    pub default_config: f64,
    pub platforms: Platforms,
}

impl CardSpec {
    /// 该卡在 `platform` 上是否创建。
    pub fn applies(&self, platform: Platform) -> bool {
        self.platforms.applies(platform)
    }

    /// 搜索文本（`isSearchMatch` 只看标题与副标题）。
    pub fn text(&self) -> CardText {
        CardText::new(self.title, self.content)
    }
}

// ── 常量（app/config/constants.py:1-13） ───────────────────────────────
pub const YEAR: u32 = 2026;
pub const AUTHOR: &str = "XiaoYouChR";
pub const VERSION: &str = "4.4.0";
pub const AUTHOR_URL: &str = "https://space.bilibili.com/437313511";
pub const FEEDBACK_URL: &str = "https://github.com/XiaoYouChR/Ghost-Downloader-3/issues";
pub const FIREFOX_ADDONS_URL: &str = "https://addons.mozilla.org/firefox/addon/ghost-downloader/";
pub const EDGE_ADDONS_URL: &str = "https://microsoftedge.microsoft.com/addons/detail/ghost-downloader-browser/odaohmfjjbompdkmfbambadnagplcmce";
pub const CHROME_WEBSTORE_URL: &str = "https://chromewebstore.google.com/detail/ghost-downloader-for-brow/lagbjgkmaafnlinaeonbhjchnjinjpeh";

/// 「关于」卡副标题：`f"© Copyright {YEAR}, {AUTHOR}. Version {VERSION}"`。
pub fn about_content() -> String {
    format!("© Copyright {YEAR}, {AUTHOR}. Version {VERSION}")
}

/// 「了解作者」卡副标题：`self.tr("发现更多 {} 的作品").format(AUTHOR)`。
pub fn author_content() -> String {
    format!("发现更多 {AUTHOR} 的作品")
}

/// 语言下拉候选（`cfg.language.options` 实测顺序）。
pub const LANGUAGE_LITERALS: [&str; 9] = [
    "zh_CN", "zh_TW", "yue_HK", "en_US", "ja_JP", "ru_RU", "pt_BR", "es_ES", "Auto",
];

/// 语言下拉标签（`LANGUAGE_TEXTS` + AUTO 回退 `self.tr("使用系统设置")`）。
pub const LANGUAGE_LABELS: [&str; 9] = [
    "简体中文 (中国大陆)",
    "正體中文 (台灣)",
    "粤语 (香港)",
    "English (US)",
    "日本語 (日本)",
    "Русский (Россия)",
    "Português (Brasil)",
    "Español (España)",
    "使用系统设置",
];

const THEME_LITERALS: [&str; 3] = ["Light", "Dark", "Auto"];
const THEME_LABELS: [&str; 3] = ["浅色", "深色", "跟随系统设置"];
const CONFLICT_LITERALS: [&str; 3] = ["keepBoth", "overwrite", "ask"];
const CONFLICT_LABELS: [&str; 3] = ["保留两者", "覆盖", "询问"];
const CLOSE_LITERALS: [&str; 3] = ["Ask", "Background", "Quit"];
const CLOSE_LABELS: [&str; 3] = ["关闭时询问", "继续在后台运行", "退出程序"];
const BG_WIN_LITERALS: [&str; 5] = ["Acrylic", "Mica", "MicaAlt", "Aero", "None"];
const BG_MAC_LITERALS: [&str; 2] = ["Acrylic", "None"];

const fn card(
    id: &'static str,
    group: &'static str,
    title: &'static str,
    content: &'static str,
    control: Control,
    config: ConfigKey,
    default_config: f64,
) -> CardSpec {
    CardSpec {
        id,
        group,
        title,
        content,
        control,
        config: Some(config),
        default_config,
        platforms: Platforms::All,
    }
}

const fn button(
    id: &'static str,
    group: &'static str,
    title: &'static str,
    content: &'static str,
    config: Option<ConfigKey>,
) -> CardSpec {
    CardSpec {
        id,
        group,
        title,
        content,
        control: Control::Button,
        config,
        default_config: 0.0,
        platforms: Platforms::All,
    }
}

const fn with_platform(mut spec: CardSpec, platforms: Platforms) -> CardSpec {
    spec.platforms = platforms;
    spec
}

const SPIN_ONE: Scale = Scale::One;

/// 全部设置卡（组内顺序 = Python `addSettingCards` 的列表顺序）。
pub static CARDS: &[CardSpec] = &[
    // ── 综合下载设置（setting_page.py:106-140） ──────────────────────
    card(
        "max_task_num",
        settings_group::GROUP_GENERAL,
        "最大任务数",
        "最多能同时进行的任务数量",
        Control::Range {
            min: 1,
            max: 10,
            step: 1,
            unit: "",
            scale: SPIN_ONE,
        },
        ConfigKey::new("GeneralDownload", "MaxTaskNum"),
        3.0,
    ),
    card(
        "pre_block_num",
        settings_group::GROUP_GENERAL,
        "预分配线程数",
        "线程越多，下载越快。线程数大于 64 时，有触发反爬导致文件损坏的风险",
        Control::Range {
            min: 1,
            max: 256,
            step: 1,
            unit: "",
            scale: SPIN_ONE,
        },
        ConfigKey::new("GeneralDownload", "PreBlockNum"),
        8.0,
    ),
    card(
        "auto_speed_up",
        settings_group::GROUP_GENERAL,
        "自动提速",
        "AI 实时检测各线程效率并自动增加线程数以提高下载速度",
        Control::Switch,
        ConfigKey::new("GeneralDownload", "AutoSpeedUp"),
        1.0,
    ),
    card(
        "max_reassign_size",
        settings_group::GROUP_GENERAL,
        "最小再分配大小",
        "每线程剩余量大于此值时, 有线程完成或自动提速条件满足会触发重新分配",
        Control::Range {
            min: 64,
            max: 102400,
            step: 64,
            unit: " KB",
            scale: SPIN_ONE,
        },
        ConfigKey::new("GeneralDownload", "MaxReassignSize"),
        512.0,
    ),
    card(
        "speed_limitation",
        settings_group::GROUP_GENERAL,
        "下载限速",
        "当下载任务界面限速开关开启时，所有任务将根据此值进行限速",
        Control::Range {
            min: 1,
            max: 102400,
            step: 512,
            unit: " KB/s",
            scale: Scale::BytesPerKb,
        },
        ConfigKey::new("GeneralDownload", "SpeedLimitation"),
        4_194_304.0,
    ),
    card(
        "preserve_last_modified",
        settings_group::GROUP_GENERAL,
        "保留文件修改时间",
        "下载完成后将文件的修改时间设为服务器提供的 Last-Modified 值",
        Control::Switch,
        ConfigKey::new("GeneralDownload", "PreserveLastModified"),
        0.0,
    ),
    card(
        "verify_ssl",
        settings_group::GROUP_GENERAL,
        "下载时验证 SSL 证书",
        "文件无法下载时，可尝试关闭该选项",
        Control::Switch,
        // 注意小写 s：cfg.py:250 `ConfigItem("GeneralDownload", "shouldVerifySsl", ...)`。
        ConfigKey::new("GeneralDownload", "shouldVerifySsl"),
        0.0,
    ),
    card(
        "download_folder",
        settings_group::GROUP_GENERAL,
        "下载路径",
        "文件默认保存位置",
        Control::Folder,
        // 缺省值平台相关（`str(DOWNLOAD_DIR)` / Android 的 Download 目录），
        // 由调用方经 `default_download_folder` 传入，这里不编造。
        ConfigKey::new("GeneralDownload", "DownloadFolder"),
        0.0,
    ),
    card(
        "name_conflict",
        settings_group::GROUP_GENERAL,
        "同名文件",
        "下载目录中已有同名文件时的处理方式",
        Control::Combo {
            literals: &CONFLICT_LITERALS,
            labels: &CONFLICT_LABELS,
        },
        ConfigKey::new("GeneralDownload", "NameConflict"),
        0.0,
    ),
    card(
        "use_system_dns",
        settings_group::GROUP_GENERAL,
        "使用系统 DNS",
        "使用操作系统的 DNS 解析，兼容 TUN、VPN 和代理等网络环境",
        Control::Switch,
        ConfigKey::new("Network", "ShouldUseSystemDns"),
        1.0,
    ),
    CardSpec {
        control: Control::Custom,
        ..card(
            "proxy",
            settings_group::GROUP_GENERAL,
            "代理",
            "设置下载时希望使用的代理",
            Control::Custom,
            ConfigKey::new("GeneralDownload", "ProxyServer"),
            0.0,
        )
    },
    card(
        "client_profile",
        settings_group::GROUP_GENERAL,
        "模拟身份",
        "TLS 指纹与 User-Agent 预设",
        Control::Custom,
        ConfigKey::new("Network", "ClientProfile"),
        0.0,
    ),
    card(
        "headers_presets",
        settings_group::GROUP_GENERAL,
        "请求标头预设",
        "新建任务的标头起点",
        Control::Custom,
        ConfigKey::new("Network", "HeadersPresets"),
        0.0,
    ),
    // ── 下载分类（:143-148） ─────────────────────────────────────────
    card(
        "category_enabled",
        settings_group::GROUP_CATEGORY,
        "启用下载分类",
        "根据扩展名将下载任务归类，便于筛选与分发到指定文件夹",
        Control::Switch,
        ConfigKey::new("Category", "EnableCategory"),
        0.0,
    ),
    card(
        "category_rules",
        settings_group::GROUP_CATEGORY,
        "下载分类规则",
        "根据扩展名自动归类，可为分类指定下载文件夹",
        Control::Custom,
        ConfigKey::new("Category", "CategoryRules"),
        0.0,
    ),
    // ── 关联设置（:222-246，darwin 无此组） ──────────────────────────
    with_platform(
        card(
            "url_scheme",
            settings_group::GROUP_ASSOCIATION,
            "允许浏览器扩展唤醒",
            "浏览器扩展可通过 ghostdownloader:// 协议启动桌面端",
            Control::Switch,
            ConfigKey::new("Browser", "UrlSchemeRegistered"),
            1.0,
        ),
        Platforms::NotDarwin,
    ),
    // ── 浏览器扩展（:160-219） ──────────────────────────────────────
    card(
        "browser_enabled",
        settings_group::GROUP_BROWSER,
        "启用浏览器扩展",
        "",
        Control::Switch,
        ConfigKey::new("Browser", "EnableBrowserExtension"),
        1.0,
    ),
    card(
        "browser_draft_taken",
        settings_group::GROUP_BROWSER,
        "接管下载时进入草稿模式",
        "自动接管浏览器下载时先进入草稿，方便您调整下载路径和文件名",
        Control::Switch,
        // cfg.py:270 键名与文案语义不一致（历史键名），以配置为准。
        ConfigKey::new("Browser", "EnableRaiseWindowWhenReceiveMsg"),
        0.0,
    ),
    button(
        "browser_pair_token",
        settings_group::GROUP_BROWSER,
        "配对令牌",
        "",
        Some(ConfigKey::new("Browser", "BrowserExtensionPairToken")),
    ),
    button(
        "store_install",
        settings_group::GROUP_BROWSER,
        "从商店安装扩展",
        "商店版扩展需等待审核后才能获得更新",
        None,
    ),
    button(
        "chromium_install",
        settings_group::GROUP_BROWSER,
        "安装到 Chromium 浏览器",
        "自动解包扩展并引导加载（Chrome / Brave 等），扩展随桌面端更新自动升级",
        None,
    ),
    card(
        "browser_port",
        settings_group::GROUP_BROWSER,
        "服务端口",
        "浏览器扩展连接使用的端口",
        Control::Range {
            min: 1024,
            max: 65535,
            step: 1,
            unit: "",
            scale: SPIN_ONE,
        },
        ConfigKey::new("Browser", "Port"),
        14_370.0,
    ),
    // ── Aria2 RPC 兼容（:250-272） ─────────────────────────────────
    card(
        "aria2_enabled",
        settings_group::GROUP_ARIA2RPC,
        "启用 Aria2 RPC 兼容",
        "",
        Control::Switch,
        ConfigKey::new("Aria2Rpc", "Enabled"),
        0.0,
    ),
    card(
        "aria2_port",
        settings_group::GROUP_ARIA2RPC,
        "监听端口",
        "Aria2 RPC 默认端口为 16800",
        Control::Range {
            min: 1024,
            max: 65535,
            step: 1,
            unit: "",
            scale: SPIN_ONE,
        },
        ConfigKey::new("Aria2Rpc", "Port"),
        16_800.0,
    ),
    card(
        "aria2_token",
        settings_group::GROUP_ARIA2RPC,
        "令牌",
        "若设置，客户端需传入 token 才可创建任务",
        Control::Line {
            placeholder: "可选",
            password: true,
        },
        ConfigKey::new("Aria2Rpc", "Token"),
        0.0,
    ),
    card(
        "aria2_emulate_fingerprint",
        settings_group::GROUP_ARIA2RPC,
        "模拟浏览器指纹",
        "为通过 Aria2 RPC 接收的任务附加浏览器 TLS 指纹与请求头",
        Control::Switch,
        ConfigKey::new("Aria2Rpc", "EmulateFingerprint"),
        0.0,
    ),
    // ── 个性化（:275-312） ─────────────────────────────────────────
    card(
        "theme_mode",
        settings_group::GROUP_PERSONALIZATION,
        "应用主题",
        "更改应用程序的外观",
        Control::Combo {
            literals: &THEME_LITERALS,
            labels: &THEME_LABELS,
        },
        ConfigKey::new("Personalization", "ThemeMode"),
        0.0,
    ),
    with_platform(
        card(
            "background_effect_win",
            settings_group::GROUP_PERSONALIZATION,
            "窗口背景透明材质",
            "设置窗口背景透明效果和透明材质",
            Control::Combo {
                literals: &BG_WIN_LITERALS,
                labels: &BG_WIN_LITERALS,
            },
            ConfigKey::new("Personalization", "BackgroundEffect"),
            0.0,
        ),
        Platforms::Win32,
    ),
    with_platform(
        card(
            "background_effect_mac",
            settings_group::GROUP_PERSONALIZATION,
            "窗口背景透明材质",
            "设置窗口背景透明效果和透明材质",
            Control::Combo {
                literals: &BG_MAC_LITERALS,
                labels: &BG_MAC_LITERALS,
            },
            ConfigKey::new("Personalization", "BackgroundEffect"),
            0.0,
        ),
        Platforms::Darwin,
    ),
    card(
        "dpi_scale",
        settings_group::GROUP_PERSONALIZATION,
        "界面缩放",
        "改变应用程序界面的缩放比例, 0% 为自动",
        Control::Range {
            min: 0,
            max: 500,
            step: 25,
            unit: " %",
            scale: Scale::FractionToPercent,
        },
        ConfigKey::new("Personalization", "DpiScale"),
        0.0,
    ),
    card(
        "language",
        settings_group::GROUP_PERSONALIZATION,
        "语言",
        "设置界面的首选语言",
        Control::Combo {
            literals: &LANGUAGE_LITERALS,
            labels: &LANGUAGE_LABELS,
        },
        ConfigKey::new("Personalization", "Language"),
        0.0,
    ),
    with_platform(
        card(
            "show_dock_icon",
            settings_group::GROUP_PERSONALIZATION,
            "在 Dock 栏中显示程序",
            "关闭后可通过菜单栏图标继续使用程序",
            Control::Switch,
            ConfigKey::new("Personalization", "ShowDockIcon"),
            1.0,
        ),
        Platforms::Darwin,
    ),
    with_platform(
        card(
            "show_dock_speed",
            settings_group::GROUP_PERSONALIZATION,
            "在 Dock 图标上显示实时速度",
            "下载时在程序坞图标上叠加当前速度",
            Control::Switch,
            ConfigKey::new("Personalization", "ShowDockSpeed"),
            1.0,
        ),
        Platforms::Darwin,
    ),
    with_platform(
        card(
            "show_menu_bar_speed",
            settings_group::GROUP_PERSONALIZATION,
            "在菜单栏显示实时速度",
            "下载时在菜单栏图标旁显示当前速度",
            Control::Switch,
            ConfigKey::new("Personalization", "ShowMenuBarSpeed"),
            1.0,
        ),
        Platforms::Darwin,
    ),
    // ── 应用（:316-364） ───────────────────────────────────────────
    card(
        "check_update_at_startup",
        settings_group::GROUP_SOFTWARE,
        "在应用程序启动时检查更新",
        "新版本将更稳定，并具有更多功能",
        Control::Switch,
        ConfigKey::new("Software", "CheckUpdateAtStartUp"),
        1.0,
    ),
    card(
        "run_at_login",
        settings_group::GROUP_SOFTWARE,
        "开机启动",
        "在系统启动时静默运行 Ghost Downloader",
        Control::Switch,
        ConfigKey::new("Software", "AutoRun"),
        0.0,
    ),
    with_platform(
        card(
            "close_mode",
            settings_group::GROUP_SOFTWARE,
            "关闭主窗口时",
            "设置关闭主窗口后程序继续在后台运行还是退出",
            Control::Combo {
                literals: &CLOSE_LITERALS,
                labels: &CLOSE_LABELS,
            },
            ConfigKey::new("Software", "CloseMode"),
            0.0,
        ),
        Platforms::NotAndroid,
    ),
    card(
        "clipboard_listener",
        settings_group::GROUP_SOFTWARE,
        "剪贴板监听",
        "剪贴板监听器将自动检测剪贴板中的链接并添加下载任务",
        Control::Switch,
        ConfigKey::new("Software", "ClipboardListener"),
        1.0,
    ),
    with_platform(
        button(
            "storage_mode",
            settings_group::GROUP_SOFTWARE,
            "数据存储模式",
            "",
            None,
        ),
        Platforms::NotAndroid,
    ),
    // ── 关于（:366-388） ──────────────────────────────────────────
    button(
        "author_link",
        settings_group::GROUP_ABOUT,
        "了解作者",
        "发现更多 XiaoYouChR 的作品",
        None,
    ),
    button(
        "feedback",
        settings_group::GROUP_ABOUT,
        "提供反馈",
        "通过提供反馈来帮助我们改进 Ghost Downloader，也可查看日志排查问题",
        None,
    ),
    button(
        "pack_info",
        settings_group::GROUP_ABOUT,
        "功能包",
        "管理已安装的功能包",
        None,
    ),
    button(
        "about",
        settings_group::GROUP_ABOUT,
        "关于",
        "© Copyright 2026, XiaoYouChR. Version 4.4.0",
        None,
    ),
];

/// 按 id 查卡。
pub fn card_by_id(id: &str) -> Option<&'static CardSpec> {
    CARDS.iter().find(|spec| spec.id == id)
}

/// 某分组在 `platform` 上的卡片（保持表内顺序）。
pub fn cards_in_group(group: &str, platform: Platform) -> Vec<&'static CardSpec> {
    CARDS
        .iter()
        .filter(|spec| spec.group == group && spec.applies(platform))
        .collect()
}

/// 某分组在该平台是否有卡（darwin 上 `association` 组不创建）。
pub fn group_has_cards(group: &str, platform: Platform) -> bool {
    CARDS
        .iter()
        .any(|spec| spec.group == group && spec.applies(platform))
}

/// 页面分组顺序（创建顺序，`about` 恒在最后；`settings_group::restore_order`
/// 会按 `UI/SettingGroupOrder` 再排一次）。
pub fn page_groups(platform: Platform) -> Vec<&'static str> {
    settings_group::PAGE_GROUPS
        .iter()
        .copied()
        .filter(|group| group_has_cards(group, platform))
        .collect()
}

/// 搜索层输入：把清单摊成 `settings_group::GroupCards`。
pub fn search_groups(platform: Platform) -> Vec<GroupCards> {
    page_groups(platform)
        .into_iter()
        .map(|group| GroupCards {
            key: group.to_string(),
            cards: cards_in_group(group, platform)
                .into_iter()
                .map(CardSpec::text)
                .collect(),
        })
        .collect()
}

/// 功能包关联卡（setting_page.py:225-246）：文件类型与协议各生成一张开关卡。
/// 返回 `(标题, 副标题)`；`extensions` 是 `"/".join(ft.extensions)` 的结果、
/// `scheme_text` 是 `"/".join(s.displayName)` 的结果，拼接由调用方完成。
pub fn pack_association_card(kind: PackAssociation, names: &str) -> (String, String) {
    match kind {
        PackAssociation::FileTypes => (
            format!("关联 {names} 文件"),
            format!("双击 {names} 文件时用 Ghost Downloader 打开"),
        ),
        PackAssociation::UriSchemes => (
            format!("处理 {names} 链接"),
            format!("点击 {names} 链接时唤起 Ghost Downloader"),
        ),
    }
}

/// 功能包关联卡种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackAssociation {
    FileTypes,
    UriSchemes,
}

/// 下拉框的当前索引（配置字面量 → 索引；未知字面量兜底 0，同 conflict.rs 惯例）。
pub fn combo_index(literals: &[&str], value: &str) -> usize {
    literals.iter().position(|item| *item == value).unwrap_or(0)
}

/// 下拉框索引 → 配置字面量（越界兜底第 0 项）。
///
/// 返回值的生命周期跟随入参表：调用方传的是 `&'static` 表，取到的就是
/// `&'static str`；传临时切片也不会泄漏。
pub fn combo_literal<'a>(literals: &[&'a str], index: i64) -> &'a str {
    literals
        .get(index.max(0) as usize)
        .copied()
        .or_else(|| literals.first().copied())
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linux_cards(group: &str) -> Vec<&'static str> {
        cards_in_group(group, Platform::Linux)
            .into_iter()
            .map(|spec| spec.id)
            .collect()
    }

    #[test]
    fn general_group_order_matches_python() {
        assert_eq!(
            linux_cards(settings_group::GROUP_GENERAL),
            vec![
                "max_task_num",
                "pre_block_num",
                "auto_speed_up",
                "max_reassign_size",
                "speed_limitation",
                "preserve_last_modified",
                "verify_ssl",
                "download_folder",
                "name_conflict",
                "use_system_dns",
                "proxy",
                "client_profile",
                "headers_presets",
            ]
        );
    }

    #[test]
    fn other_group_orders_match_python() {
        assert_eq!(
            linux_cards(settings_group::GROUP_CATEGORY),
            vec!["category_enabled", "category_rules"]
        );
        assert_eq!(
            linux_cards(settings_group::GROUP_BROWSER),
            vec![
                "browser_enabled",
                "browser_draft_taken",
                "browser_pair_token",
                "store_install",
                "chromium_install",
                "browser_port",
            ]
        );
        assert_eq!(
            linux_cards(settings_group::GROUP_ARIA2RPC),
            vec![
                "aria2_enabled",
                "aria2_port",
                "aria2_token",
                "aria2_emulate_fingerprint"
            ]
        );
        assert_eq!(
            linux_cards(settings_group::GROUP_SOFTWARE),
            vec![
                "check_update_at_startup",
                "run_at_login",
                "close_mode",
                "clipboard_listener",
                "storage_mode"
            ]
        );
        assert_eq!(
            linux_cards(settings_group::GROUP_ABOUT),
            vec!["author_link", "feedback", "pack_info", "about"]
        );
    }

    #[test]
    fn platform_gating_matches_python_conditions() {
        // 关联组在 darwin 上整组不创建（url_scheme 卡 NotDarwin）。
        assert!(!group_has_cards(
            settings_group::GROUP_ASSOCIATION,
            Platform::MacOs
        ));
        assert!(group_has_cards(
            settings_group::GROUP_ASSOCIATION,
            Platform::Linux
        ));
        // Linux 个性化组：theme/dpi/language 三张（背景材质与 dock 卡都不在）。
        assert_eq!(
            linux_cards(settings_group::GROUP_PERSONALIZATION),
            vec!["theme_mode", "dpi_scale", "language"]
        );
        // Windows 有背景材质（win 版候选），macOS 有 mac 版 + 三张 dock 卡。
        let win = cards_in_group(settings_group::GROUP_PERSONALIZATION, Platform::Windows);
        assert!(win.iter().any(|spec| spec.id == "background_effect_win"));
        assert!(!win.iter().any(|spec| spec.id == "background_effect_mac"));
        let mac = cards_in_group(settings_group::GROUP_PERSONALIZATION, Platform::MacOs);
        assert!(mac.iter().any(|spec| spec.id == "background_effect_mac"));
        assert!(mac.iter().any(|spec| spec.id == "show_dock_icon"));
        assert!(mac.iter().any(|spec| spec.id == "show_menu_bar_speed"));
        // Android 无 closeMode / 数据存储模式。
        let android = cards_in_group(settings_group::GROUP_SOFTWARE, Platform::Android);
        assert!(!android.iter().any(|spec| spec.id == "close_mode"));
        assert!(!android.iter().any(|spec| spec.id == "storage_mode"));
        assert!(android.iter().any(|spec| spec.id == "clipboard_listener"));
    }

    #[test]
    fn page_groups_follow_creation_order() {
        assert_eq!(
            page_groups(Platform::Linux),
            vec![
                settings_group::GROUP_GENERAL,
                settings_group::GROUP_CATEGORY,
                settings_group::GROUP_ASSOCIATION,
                settings_group::GROUP_BROWSER,
                settings_group::GROUP_ARIA2RPC,
                settings_group::GROUP_PERSONALIZATION,
                settings_group::GROUP_SOFTWARE,
                settings_group::GROUP_ABOUT,
            ]
        );
        // darwin 少一个 association 组。
        assert_eq!(page_groups(Platform::MacOs).len(), 7);
    }

    #[test]
    fn config_keys_match_cfg_py() {
        let key = |id: &str| card_by_id(id).and_then(|spec| spec.config).expect(id);
        // 逐条对照 cfg.py:242-333 的 (section, key) 实参。
        assert_eq!(
            key("max_task_num"),
            ConfigKey::new("GeneralDownload", "MaxTaskNum")
        );
        assert_eq!(
            key("pre_block_num"),
            ConfigKey::new("GeneralDownload", "PreBlockNum")
        );
        assert_eq!(
            key("auto_speed_up"),
            ConfigKey::new("GeneralDownload", "AutoSpeedUp")
        );
        assert_eq!(
            key("max_reassign_size"),
            ConfigKey::new("GeneralDownload", "MaxReassignSize")
        );
        assert_eq!(
            key("speed_limitation"),
            ConfigKey::new("GeneralDownload", "SpeedLimitation")
        );
        assert_eq!(
            key("preserve_last_modified"),
            ConfigKey::new("GeneralDownload", "PreserveLastModified")
        );
        // 唯一一个小写开头的键名。
        assert_eq!(
            key("verify_ssl"),
            ConfigKey::new("GeneralDownload", "shouldVerifySsl")
        );
        assert_eq!(
            key("download_folder"),
            ConfigKey::new("GeneralDownload", "DownloadFolder")
        );
        assert_eq!(
            key("name_conflict"),
            ConfigKey::new("GeneralDownload", "NameConflict")
        );
        assert_eq!(
            key("use_system_dns"),
            ConfigKey::new("Network", "ShouldUseSystemDns")
        );
        assert_eq!(
            key("proxy"),
            ConfigKey::new("GeneralDownload", "ProxyServer")
        );
        assert_eq!(
            key("client_profile"),
            ConfigKey::new("Network", "ClientProfile")
        );
        assert_eq!(
            key("headers_presets"),
            ConfigKey::new("Network", "HeadersPresets")
        );
        assert_eq!(
            key("category_enabled"),
            ConfigKey::new("Category", "EnableCategory")
        );
        assert_eq!(
            key("url_scheme"),
            ConfigKey::new("Browser", "UrlSchemeRegistered")
        );
        assert_eq!(
            key("browser_enabled"),
            ConfigKey::new("Browser", "EnableBrowserExtension")
        );
        assert_eq!(
            key("browser_draft_taken"),
            ConfigKey::new("Browser", "EnableRaiseWindowWhenReceiveMsg")
        );
        assert_eq!(key("browser_port"), ConfigKey::new("Browser", "Port"));
        assert_eq!(key("aria2_enabled"), ConfigKey::new("Aria2Rpc", "Enabled"));
        assert_eq!(key("aria2_port"), ConfigKey::new("Aria2Rpc", "Port"));
        assert_eq!(key("aria2_token"), ConfigKey::new("Aria2Rpc", "Token"));
        assert_eq!(
            key("aria2_emulate_fingerprint"),
            ConfigKey::new("Aria2Rpc", "EmulateFingerprint")
        );
        assert_eq!(
            key("theme_mode"),
            ConfigKey::new("Personalization", "ThemeMode")
        );
        assert_eq!(
            key("dpi_scale"),
            ConfigKey::new("Personalization", "DpiScale")
        );
        assert_eq!(
            key("language"),
            ConfigKey::new("Personalization", "Language")
        );
        assert_eq!(
            key("check_update_at_startup"),
            ConfigKey::new("Software", "CheckUpdateAtStartUp")
        );
        assert_eq!(key("run_at_login"), ConfigKey::new("Software", "AutoRun"));
        assert_eq!(key("close_mode"), ConfigKey::new("Software", "CloseMode"));
        assert_eq!(
            key("clipboard_listener"),
            ConfigKey::new("Software", "ClipboardListener")
        );
        // 纯动作卡不绑配置键。
        assert!(card_by_id("about").unwrap().config.is_none());
        assert!(card_by_id("feedback").unwrap().config.is_none());
        assert!(card_by_id("store_install").unwrap().config.is_none());
        assert!(card_by_id("storage_mode").unwrap().config.is_none());
    }

    #[test]
    fn range_defaults_are_in_display_range_and_round_trip() {
        for spec in CARDS {
            if let Control::Range {
                min,
                max,
                step,
                scale,
                ..
            } = spec.control
            {
                let display = scale.to_display(spec.default_config);
                assert!(
                    (min..=max).contains(&display),
                    "{}: 缺省显示值 {display} 越界 [{min}, {max}]",
                    spec.id
                );
                // 显示值 → 配置值 → 显示值 幂等（配置存的就是显示口径的整数倍）。
                assert_eq!(
                    scale.to_display(scale.to_config(display)),
                    display,
                    "{}: 换算不幂等",
                    spec.id
                );
                assert!(step > 0, "{}: step 必须为正", spec.id);
                // **不要**断言缺省值落在 step 网格上：qfluentwidgets 的
                // `SpinBox.setSingleStep()` 只影响方向键/滚轮步进，
                // `setValue()` 不做网格吸附（setting_cards.py:63-70）。
                // speedLimitation 缺省 4194304 字节 → 4096 KB/s，在
                // min=1 / step=512 下本来就不是网格点。
            }
        }
    }

    #[test]
    fn speed_limitation_display_matches_cfg_range() {
        // cfg.py:247-249 RangeValidator(1024, 104857600) 字节 ÷1024 = 显示 1..102400。
        let spec = card_by_id("speed_limitation").unwrap();
        let Control::Range {
            min,
            max,
            step,
            scale,
            unit,
        } = spec.control
        else {
            panic!("speed_limitation 必须是 Range");
        };
        assert_eq!((min, max, step), (1, 102400, 512));
        assert_eq!(unit, " KB/s");
        assert_eq!(scale.to_display(1024.0), 1);
        assert_eq!(scale.to_display(104_857_600.0), 102_400);
        // Python `int(spin.value() / (1/1024))`：4096 KB/s → 4194304 字节。
        assert_eq!(scale.to_config(4096), 4_194_304.0);
    }

    #[test]
    fn dpi_scale_uses_percent_scale() {
        let spec = card_by_id("dpi_scale").unwrap();
        let Control::Range {
            min,
            max,
            step,
            scale,
            unit,
        } = spec.control
        else {
            panic!("dpi_scale 必须是 Range");
        };
        // cfg.py:296 RangeValidator(0, 5) ×100 → 0..500 %，singleStep 缺省 25。
        assert_eq!((min, max, step), (0, 500, 25));
        assert_eq!(unit, " %");
        assert_eq!(scale.to_display(1.5), 150);
        // PercentSpinBoxSettingCard.leaveEvent: spin.value() / 100 保持 float。
        assert_eq!(scale.to_config(150), 1.5);
        assert_eq!(scale.to_display(0.0), 0);
    }

    #[test]
    fn combo_literals_and_labels_are_consistent() {
        for spec in CARDS {
            if let Control::Combo { literals, labels } = spec.control {
                assert_eq!(
                    literals.len(),
                    labels.len(),
                    "{}: 候选与标签不等长",
                    spec.id
                );
                assert!(!literals.is_empty(), "{}: 候选为空", spec.id);
                // 缺省值必须能在候选里找到（Python 侧 OptionsConfigItem 同理）。
                let default = match spec.id {
                    "name_conflict" => "keepBoth",
                    "theme_mode" => "Auto",
                    "language" => "Auto",
                    "close_mode" => "Ask",
                    "background_effect_win" => "Mica",
                    "background_effect_mac" => "Acrylic",
                    other => panic!("未覆盖的下拉卡 {other}"),
                };
                assert!(
                    literals.contains(&default),
                    "{}: 缺省 {default} 不在候选里",
                    spec.id
                );
                assert_eq!(
                    combo_index(literals, default),
                    combo_index(literals, default)
                );
                assert_eq!(
                    combo_literal(literals, combo_index(literals, default) as i64),
                    default
                );
            }
        }
        // 实测枚举序（venv 跑出）：Theme [Light, Dark, Auto] 缺省 Auto=索引 2。
        assert_eq!(combo_index(&THEME_LITERALS, "Auto"), 2);
        assert_eq!(THEME_LABELS[2], "跟随系统设置");
        // Language 九项、Auto 在末位，标签回退「使用系统设置」。
        assert_eq!(combo_index(&LANGUAGE_LITERALS, "Auto"), 8);
        assert_eq!(LANGUAGE_LABELS[8], "使用系统设置");
        assert_eq!(LANGUAGE_LITERALS.len(), 9);
    }

    #[test]
    fn combo_fallback_is_first_item() {
        assert_eq!(combo_index(&CLOSE_LITERALS, "Nope"), 0);
        assert_eq!(combo_literal(&CLOSE_LITERALS, 9), "Ask");
        assert_eq!(combo_literal(&CLOSE_LITERALS, -1), "Ask");
    }

    #[test]
    fn search_text_uses_title_and_content() {
        // 代理卡：标题「代理」、副标题含「代理」→ 搜「代理」必命中。
        let groups = search_groups(Platform::Linux);
        let (results, has_match) = settings_group::search_all(&groups, "代理", &[]);
        assert!(has_match);
        let general = results
            .iter()
            .find(|result| result.key == settings_group::GROUP_GENERAL)
            .unwrap();
        assert!(general.visible);
        // 组内命中两张：use_system_dns（索引 9，副标题「兼容 TUN、VPN 和代理等
        // 网络环境」）与 proxy（索引 10）。**不是**只有代理卡——搜索口径是
        // 标题 + 副标题子串（setting_card_group.py `isSearchMatch`）。
        let hits: Vec<usize> = general
            .card_visible
            .iter()
            .enumerate()
            .filter(|(_, visible)| **visible)
            .map(|(index, _)| index)
            .collect();
        assert_eq!(hits, vec![9, 10]);
        // 搜「SSL」→ verify_ssl 卡（索引 6）。
        let (results, _) = settings_group::search_all(&groups, "ssl", &[]);
        let general = results
            .iter()
            .find(|result| result.key == settings_group::GROUP_GENERAL)
            .unwrap();
        let hits: Vec<usize> = general
            .card_visible
            .iter()
            .enumerate()
            .filter(|(_, visible)| **visible)
            .map(|(index, _)| index)
            .collect();
        assert_eq!(hits, vec![6]);
    }

    #[test]
    fn about_and_author_content_are_formatted() {
        assert_eq!(
            about_content(),
            "© Copyright 2026, XiaoYouChR. Version 4.4.0"
        );
        assert_eq!(author_content(), "发现更多 XiaoYouChR 的作品");
        // 表里的静态文案与格式化结果一致（VERSION/AUTHOR 改动时测试会提醒）。
        assert_eq!(card_by_id("about").unwrap().content, about_content());
        assert_eq!(card_by_id("author_link").unwrap().content, author_content());
    }

    #[test]
    fn pack_association_cards_format_like_python() {
        assert_eq!(
            pack_association_card(PackAssociation::FileTypes, "torrent/ddl"),
            (
                "关联 torrent/ddl 文件".to_string(),
                "双击 torrent/ddl 文件时用 Ghost Downloader 打开".to_string()
            )
        );
        assert_eq!(
            pack_association_card(PackAssociation::UriSchemes, "哔哩哔哩"),
            (
                "处理 哔哩哔哩 链接".to_string(),
                "点击 哔哩哔哩 链接时唤起 Ghost Downloader".to_string()
            )
        );
    }

    #[test]
    fn card_ids_are_unique() {
        let mut ids: Vec<&str> = CARDS.iter().map(|spec| spec.id).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count, "卡片 id 重复");
    }
}
