//! `app/config/cfg.py` 的 typed 语义层（第 79 步）。
//!
//! ## 这一层解决什么
//!
//! 第 78 步的 `config::UserConfig` 只懂「组 → 键 → 裸 JSON 值」，**不懂每一项的类型、
//! 缺省值、取值范围和落盘序列化方式**。本模块把 `cfg.py` 里 40 个 `ConfigItem`
//! （外加 `QConfig` 自带的 2 个）逐项搬成一张表 [`SPECS`]，提供：
//!
//! - [`default_config`]：**纯 Rust 造出与 Python `cfg` 逐字节相同的 `UserConfig.json`**
//!   （由 `tests/fixtures/user_config_default_linux.json` 钉住）；
//! - [`read`] / [`write`]：按 python 属性名（`maxTaskNum`、`isSpeedLimitEnabled` …）
//!   读写，写的时候跑 `validator.correct()` + `serializer.serialize()`；
//! - [`to_proxy_url`]、[`current_headers_preset_index`]、[`current_headers`]：
//!   `cfg.py` 里三个模块级函数。
//!
//! ## 顺序是协议的一部分
//!
//! Python 的 `toDict()` 遍历 `dir(self._cfg.__class__)`，也就是**属性名字母序**，
//! 组在首次出现时创建。所以 [`SPECS`] 的顺序 = 属性名字母序，`default_config`
//! 按这个顺序 `set`，落盘键序才与 Python 一致（`serde_json` 开了 `preserve_order`）。
//! 组序是「各组首项的出现次序」，由此自然推出：`Aria2Rpc`、`GeneralDownload`、
//! `Browser`、`Category`、`Network`、`Software`、`Personalization`、`UI`、
//! `QFluentWidgets`。
//!
//! ## 缺省值必须过 validator
//!
//! `ConfigItem.__init__` 里 `self.value = default` 会走 `validator.correct()`，
//! 所以磁盘上的缺省值**不是**源码里的字面量：
//!
//! - `HeadersPresets` 源码缺省 `[]`，`HeadersPresetListValidator.correct([])`
//!   （`validate` 要求 `bool(value)`）→ 落盘是「默认 + `BASE_HEADERS`」那一项；
//! - `CategoryRules` 缺省 `[]` → `CategoryListValidator.validate([])` 为真（`all()` 空真）
//!   → 仍是 `[]`，再被 `JsonConfigSerializer` 转成串 `"[]"`；
//! - `DownloadFolder` 缺省 `str(DOWNLOAD_DIR)` → `FolderValidator.correct`
//!   取绝对路径（本机 `//Downloads`）；
//! - `ThemeColor` 缺省 `#009faa` → `ColorSerializer` 的 `name(QColor.HexArgb)`
//!   → 落盘 `#ff009faa`（**带 alpha**）。
//!
//! ## 反直觉但已实证的结论
//!
//! （`2026-10-18` 在 venv 里逐条跑过 `validator.correct()`）
//!
//! - `BoolValidator` 继承 `OptionsValidator([True, False])`，非法值取 `options[0]`
//!   —— 磁盘上写字符串 `"False"` 读回来是 **`true`**；而 `0` / `1` 因为
//!   `0 == False` / `1 == True` 命中白名单，会**原样保留数字形态**。
//! - `OptionsValidator(Language)` 的 options 是 **enum 成员**，`options[0]` 是
//!   `Language.CHINESE_SIMPLIFIED` → 非法 `Language` 落回 `"zh_CN"`（不是 `"Auto"`）；
//!   `Theme` 同理落回 `"Light"`，`CloseMode` 落回 `"Ask"`。
//! - `RangeValidator.correct(0.0)` 返回 **int `0`**（`max(0, 0.0)` 相等时返回第一个
//!   参数，而边界是 int 字面量），`correct(2.0)` 仍是 float `2.0` ——
//!   `DpiScale` 缺省落盘是 `0` 而不是 `0.0`。
//! - `Path(...).absolute()` 只做 `PurePath` 归一（丢空段与 `.`、保留**恰好两个**
//!   前导斜杠），**不解析 `..`**，也不检查存在性。
//!
//! ## 与 Python 的已知差异
//!
//! - **不建目录**：`FolderValidator.correct` 会 `mkdir(parents=True)`，
//!   `FolderListValidator.correct` 会丢掉不存在的路径。Rust 侧
//!   [`Kind::Folder`] 只做「反斜杠换正斜杠 + `PurePath` 归一 + 绝对化」，
//!   **不落盘副作用**；
//!   [`Kind::FolderList`] 保留「只留存在的路径」但同样不建目录。
//! - **不抛异常**：`RangeValidator.correct("7")` 在 Python 里抛 `TypeError`
//!   （然后被 `@exceptionHandler()` 吞掉、整个 `load` 中断），Rust 侧退回缺省值。
//! - `Color` 只认 `#rgb` / `#rrggbb` / `#aarrggbb`；Qt 的颜色名（`"red"`、
//!   `"transparent"`）一律退回缺省。app 里没有任何代码写颜色名。
//! - `ProxyValidator` 用手写解析近似 CPython `re` 的那条正则（见 [`proxy_valid`]）。

use serde_json::{Map, Value, json};

use crate::config::{UserConfig, py_json_dumps};

/// `app/config/paths.py::DOWNLOAD_DIR` 在 Linux 真机上的实测值。
pub const DEFAULT_DOWNLOAD_FOLDER: &str = "//Downloads";

/// `cfg.py` 里 Android 分支的 `DownloadFolder` 缺省。
pub const ANDROID_DOWNLOAD_FOLDER: &str = "/storage/emulated/0/Download";

/// `cfg.py::BASE_HEADERS`（键序即 Python dict 字面量顺序）。
pub const BASE_HEADERS: &[(&str, &str)] = &[
    ("accept-encoding", "deflate, br, gzip"),
    ("accept-language", "zh-CN,zh;q=0.9"),
    ("cookie", "down_ip=1"),
    ("sec-fetch-dest", "document"),
    ("sec-fetch-mode", "navigate"),
    ("sec-fetch-site", "none"),
    ("sec-fetch-user", "?1"),
    ("upgrade-insecure-requests", "1"),
];

/// `cfg.py::IdentityPresets` 的缺省项（百度网盘客户端）。
const DEFAULT_IDENTITY_PRESETS: &str = r#"[{"name": "百度网盘客户端", "clientProfile": "raw", "userAgent": "pan.baidu.com", "hosts": ["*.pcs.baidu.com"], "isEnabled": true}]"#;

/// `cfg.py::Language` 的取值（`EnumSerializer` → enum 的 value）。
pub const LANGUAGE_VALUES: &[&str] = &[
    "zh_CN", "zh_TW", "yue_HK", "en_US", "ja_JP", "ru_RU", "pt_BR", "es_ES", "Auto",
];

/// `cfg.py::CloseMode` 的取值。
pub const CLOSE_MODE_VALUES: &[&str] = &["Ask", "Background", "Quit"];

/// `qfluentwidgets.common.config::Theme` 的取值（`ThemeMode` 用）。
pub const THEME_VALUES: &[&str] = &["Light", "Dark", "Auto"];

/// `cfg.py::nameConflict` 的取值。
pub const NAME_CONFLICT_VALUES: &[&str] = &["keepBoth", "overwrite", "ask"];

/// `QFluentWidgets/FontFamilies` 缺省。
const DEFAULT_FONT_FAMILIES: &[&str] = &["Segoe UI", "Microsoft YaHei", "PingFang SC"];

/// `JsonConfigSerializer` 三项各自的 list validator。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonListKind {
    /// `CategoryListValidator`：每项是带字符串 `name` 的 dict，否则整体 `[]`。
    CategoryRules,
    /// `HeadersPresetListValidator`：过滤非法项，**空则回落**「默认 + BASE_HEADERS」。
    HeadersPresets,
    /// `IdentityPresetListValidator`：过滤非法项，**没有回落**。
    IdentityPresets,
}

/// 一项配置的类型 + 缺省值 + 约束（`cfg.py` 里一个 `ConfigItem` 的三要素）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// `ConfigItem(..., BoolValidator())`。
    Bool { default: bool },
    /// `RangeConfigItem(..., RangeValidator(min, max))`，整数项。
    Int { default: i64, min: i64, max: i64 },
    /// `RangeConfigItem(..., RangeValidator(0, 5))`，浮点项（`DpiScale`）。
    Float { default: f64, min: f64, max: f64 },
    /// `ConfigItem(..., str)`，无 validator（`Token`、`BrowserExtensionPairToken`）。
    Text { default: &'static str },
    /// `OptionsConfigItem(..., OptionsValidator([...]))`，落盘 enum 的 value。
    Options {
        default: &'static str,
        options: &'static [&'static str],
    },
    /// `ConfigItem(..., ProxyValidator())`：`Auto`/`Off` 或合法代理 URL，否则 `Auto`。
    Proxy,
    /// `ConfigItem(..., ClientProfileValidator())`：非空串，否则 `auto`。
    Profile,
    /// `ConfigItem(..., FolderValidator())`：绝对路径、正斜杠。
    Folder { default: &'static str },
    /// `ConfigItem(..., FolderListValidator())`：只留存在的路径。
    FolderList,
    /// `ConfigItem(..., StringListValidator())`：只留字符串项（`UI` 组两项）。
    StringList,
    /// `ConfigItem`，**没有 validator**（`QFluentWidgets/FontFamilies`）：恒等。
    StrList { default: &'static [&'static str] },
    /// `JsonConfigSerializer(list, list)`：逻辑值是 list，落盘是 `json.dumps` 串。
    JsonList(JsonListKind),
    /// `GeometrySerializer`：逻辑值是 `[x, y, w, h]`，落盘是 `"x,y,w,h"`。
    Geometry,
    /// `ColorConfigItem`：落盘是 `name(QColor.HexArgb)`，即 `#aarrggbb`。
    Color { default: &'static str },
}

/// 一项配置：python 属性名 + 组 + 键 + 类型。
///
/// `attr` 是 `cfg.py` 里的**属性名**（不是键名）：它决定 `dir()` 顺序，
/// 也用作 [`read`]/[`write`] 的标识。
#[derive(Debug, Clone, Copy)]
pub struct Spec {
    pub attr: &'static str,
    pub group: &'static str,
    pub key: &'static str,
    pub kind: Kind,
}

const fn entry(attr: &'static str, group: &'static str, key: &'static str, kind: Kind) -> Spec {
    Spec {
        attr,
        group,
        key,
        kind,
    }
}

/// 全部 42 项，**顺序 = `dir(Config)` 的属性名字母序**（见模块头）。
pub static SPECS: &[Spec] = &[
    entry(
        "aria2RpcEmulateFingerprint",
        "Aria2Rpc",
        "EmulateFingerprint",
        Kind::Bool { default: false },
    ),
    entry(
        "aria2RpcPort",
        "Aria2Rpc",
        "Port",
        Kind::Int {
            default: 16800,
            min: 1024,
            max: 65535,
        },
    ),
    entry(
        "aria2RpcToken",
        "Aria2Rpc",
        "Token",
        Kind::Text { default: "" },
    ),
    entry(
        "autoSpeedUp",
        "GeneralDownload",
        "AutoSpeedUp",
        Kind::Bool { default: true },
    ),
    entry(
        "browserExtensionPairToken",
        "Browser",
        "BrowserExtensionPairToken",
        Kind::Text { default: "" },
    ),
    entry(
        "browserExtensionPort",
        "Browser",
        "Port",
        Kind::Int {
            default: 14370,
            min: 1024,
            max: 65535,
        },
    ),
    entry(
        "categoryRules",
        "Category",
        "CategoryRules",
        Kind::JsonList(JsonListKind::CategoryRules),
    ),
    entry("clientProfile", "Network", "ClientProfile", Kind::Profile),
    entry(
        "closeMode",
        "Software",
        "CloseMode",
        Kind::Options {
            default: "Ask",
            options: CLOSE_MODE_VALUES,
        },
    ),
    entry(
        "currentHeadersPreset",
        "Network",
        "CurrentHeadersPreset",
        Kind::Int {
            default: 0,
            min: 0,
            max: 99,
        },
    ),
    entry(
        "downloadFolder",
        "GeneralDownload",
        "DownloadFolder",
        Kind::Folder {
            default: DEFAULT_DOWNLOAD_FOLDER,
        },
    ),
    entry(
        "dpiScale",
        "Personalization",
        "DpiScale",
        Kind::Float {
            default: 0.0,
            min: 0.0,
            max: 5.0,
        },
    ),
    entry(
        "expandedSettingGroups",
        "UI",
        "ExpandedSettingGroups",
        Kind::StringList,
    ),
    entry(
        "fontFamilies",
        "QFluentWidgets",
        "FontFamilies",
        Kind::StrList {
            default: DEFAULT_FONT_FAMILIES,
        },
    ),
    entry("geometry", "Software", "Geometry", Kind::Geometry),
    entry(
        "hasCompletedOobe",
        "Software",
        "HasCompletedOobe",
        Kind::Bool { default: false },
    ),
    entry(
        "headersPresets",
        "Network",
        "HeadersPresets",
        Kind::JsonList(JsonListKind::HeadersPresets),
    ),
    entry(
        "identityPresets",
        "Network",
        "IdentityPresets",
        Kind::JsonList(JsonListKind::IdentityPresets),
    ),
    entry(
        "isAria2RpcEnabled",
        "Aria2Rpc",
        "Enabled",
        Kind::Bool { default: false },
    ),
    entry(
        "isBrowserExtensionEnabled",
        "Browser",
        "EnableBrowserExtension",
        Kind::Bool { default: true },
    ),
    entry(
        "isCategoryEnabled",
        "Category",
        "EnableCategory",
        Kind::Bool { default: false },
    ),
    entry(
        "isClipboardListenerEnabled",
        "Software",
        "ClipboardListener",
        Kind::Bool { default: true },
    ),
    entry(
        "isSpeedLimitEnabled",
        "GeneralDownload",
        "isSpeedLimitEnabled",
        Kind::Bool { default: false },
    ),
    entry(
        "isUrlSchemeRegistered",
        "Browser",
        "UrlSchemeRegistered",
        Kind::Bool { default: true },
    ),
    entry(
        "language",
        "Personalization",
        "Language",
        Kind::Options {
            default: "Auto",
            options: LANGUAGE_VALUES,
        },
    ),
    entry(
        "maxReassignSize",
        "GeneralDownload",
        "MaxReassignSize",
        Kind::Int {
            default: 512,
            min: 64,
            max: 102400,
        },
    ),
    entry(
        "maxTaskNum",
        "GeneralDownload",
        "MaxTaskNum",
        Kind::Int {
            default: 3,
            min: 1,
            max: 10,
        },
    ),
    entry(
        "memoryDownloadFolders",
        "GeneralDownload",
        "HistoryDownloadFolder",
        Kind::FolderList,
    ),
    entry(
        "nameConflict",
        "GeneralDownload",
        "NameConflict",
        Kind::Options {
            default: "keepBoth",
            options: NAME_CONFLICT_VALUES,
        },
    ),
    entry(
        "preBlockNum",
        "GeneralDownload",
        "PreBlockNum",
        Kind::Int {
            default: 8,
            min: 1,
            max: 256,
        },
    ),
    entry("proxyServer", "GeneralDownload", "ProxyServer", Kind::Proxy),
    entry(
        "settingGroupOrder",
        "UI",
        "SettingGroupOrder",
        Kind::StringList,
    ),
    entry(
        "shouldCheckUpdateAtStartup",
        "Software",
        "CheckUpdateAtStartUp",
        Kind::Bool { default: true },
    ),
    entry(
        "shouldDeleteFilesOnRemove",
        "GeneralDownload",
        "DeleteFilesOnRemove",
        Kind::Bool { default: false },
    ),
    entry(
        "shouldDraftTakenDownload",
        "Browser",
        "EnableRaiseWindowWhenReceiveMsg",
        Kind::Bool { default: false },
    ),
    entry(
        "shouldPreserveLastModified",
        "GeneralDownload",
        "PreserveLastModified",
        Kind::Bool { default: false },
    ),
    entry(
        "shouldRunAtLogin",
        "Software",
        "AutoRun",
        Kind::Bool { default: false },
    ),
    entry(
        "shouldUseSystemDns",
        "Network",
        "ShouldUseSystemDns",
        Kind::Bool { default: true },
    ),
    entry(
        "shouldVerifySsl",
        "GeneralDownload",
        "shouldVerifySsl",
        Kind::Bool { default: false },
    ),
    entry(
        "speedLimitation",
        "GeneralDownload",
        "SpeedLimitation",
        Kind::Int {
            default: 4_194_304,
            min: 1024,
            max: 104_857_600,
        },
    ),
    entry(
        "themeColor",
        "QFluentWidgets",
        "ThemeColor",
        Kind::Color { default: "#009faa" },
    ),
    entry(
        "themeMode",
        "Personalization",
        "ThemeMode",
        Kind::Options {
            default: "Auto",
            options: THEME_VALUES,
        },
    ),
];

// ---------------------------------------------------------------------------
// 缺省值 + validator 镜像
// ---------------------------------------------------------------------------

impl Kind {
    /// 源码里的字面量缺省值（**未过 validator**，见模块头）。
    pub fn raw_default(&self) -> Value {
        match *self {
            Kind::Bool { default } => Value::Bool(default),
            Kind::Int { default, .. } => Value::from(default),
            Kind::Float { default, .. } => Value::from(default),
            Kind::Text { default } => Value::from(default),
            Kind::Options { default, .. } => Value::from(default),
            Kind::Proxy => Value::from("Auto"),
            Kind::Profile => Value::from("auto"),
            Kind::Folder { default } => Value::from(default),
            Kind::FolderList | Kind::StringList => Value::Array(Vec::new()),
            Kind::StrList { default } => {
                Value::Array(default.iter().map(|s| Value::from(*s)).collect())
            }
            Kind::JsonList(JsonListKind::IdentityPresets) => {
                serde_json::from_str(DEFAULT_IDENTITY_PRESETS)
                    .expect("DEFAULT_IDENTITY_PRESETS 是合法 JSON")
            }
            Kind::JsonList(_) => Value::Array(Vec::new()),
            Kind::Geometry => json!([0, 0, 0, 0]),
            Kind::Color { default } => Value::from(default),
        }
    }

    /// `validator.correct()` 的镜像。
    ///
    /// `ConfigItem.__init__` 的 `self.value = default` 与 `deserializeFrom`
    /// 的 `self.value = ...` 都会跑它，所以**磁盘上的缺省值也过了这一层**。
    pub fn correct(&self, value: &Value) -> Value {
        match *self {
            Kind::Bool { .. } => match value {
                Value::Bool(b) => Value::Bool(*b),
                // Python: `0 in [True, False]` / `1 in [True, False]` 都为真 → 原样保留
                Value::Number(n) if n.as_f64() == Some(0.0) || n.as_f64() == Some(1.0) => {
                    value.clone()
                }
                // `BoolValidator` 继承 `OptionsValidator([True, False])` → `options[0]`
                _ => Value::Bool(true),
            },
            Kind::Int { default, min, max } => {
                clamp_number(value, min as f64, max as f64).unwrap_or(Value::from(default))
            }
            Kind::Float { default, min, max } => {
                clamp_number(value, min, max).unwrap_or_else(|| bound_value(default))
            }
            // 没有 validator → 恒等（连类型都不校验，与 Python 一致）
            Kind::Text { .. } => value.clone(),
            Kind::Options { options, .. } => match value.as_str() {
                Some(s) if options.contains(&s) => value.clone(),
                _ => Value::from(options[0]),
            },
            Kind::Proxy => match value.as_str() {
                Some(s) if proxy_valid(s) => value.clone(),
                _ => Value::from("Auto"),
            },
            Kind::Profile => match value.as_str() {
                Some(s) if !s.is_empty() => value.clone(),
                _ => Value::from("auto"),
            },
            Kind::Folder { default } => match value.as_str() {
                Some(s) => Value::from(folder_stored(s)),
                None => Value::from(folder_stored(default)),
            },
            Kind::FolderList => folder_list_correct(value),
            Kind::StringList => str_list_correct(value),
            Kind::StrList { .. } => value.clone(),
            Kind::JsonList(kind) => json_list_correct(kind, value),
            // 无 validator（`serializer=GeometrySerializer()` 是唯一参数）
            Kind::Geometry => value.clone(),
            Kind::Color { default } => Value::from(
                color_hex_argb(value)
                    .or_else(|| color_hex_argb(&Value::from(default)))
                    .unwrap_or_else(|| "#ff009faa".to_string()),
            ),
        }
    }

    /// `serializer.deserialize()` 的镜像（`correct()` 之前那一步）。
    pub fn deserialize(&self, stored: &Value) -> Value {
        match *self {
            // `JsonConfigSerializer.deserialize`：只认字符串，`json.loads` 出来不是
            // list 就回落 `[]`；非字符串（含已经是 list 的）→ `TypeError` → 同样 `[]`
            Kind::JsonList(_) => match stored.as_str() {
                Some(text) => match serde_json::from_str::<Value>(text) {
                    Ok(parsed) if parsed.is_array() => parsed,
                    _ => Value::Array(Vec::new()),
                },
                None => Value::Array(Vec::new()),
            },
            Kind::Geometry => geometry_parse(stored),
            _ => stored.clone(),
        }
    }

    /// `serializer.serialize()` 的镜像（落盘形态）。
    pub fn serialize(&self, logical: &Value) -> Value {
        match *self {
            Kind::JsonList(_) => Value::from(py_json_dumps(logical)),
            Kind::Geometry => geometry_format(logical),
            _ => logical.clone(),
        }
    }
}

/// `min(max(min, value), max)`。
///
/// Python 的 `max`/`min` 在相等时返回**第一个**参数，而 `RangeValidator` 的
/// 边界是 int 字面量 —— 所以 `0.0` 会被夹成 int `0`（`DpiScale` 缺省落盘是
/// `0` 而不是 `0.0` 就是这个原因）。这里用 `Value` 保留 int/float 形态。
fn clamp_number(value: &Value, min: f64, max: f64) -> Option<Value> {
    let n = value.as_f64()?;
    let after_max = if n > min {
        value.clone()
    } else {
        bound_value(min)
    };
    let m = after_max.as_f64()?;
    Some(if m < max { after_max } else { bound_value(max) })
}

/// `RangeValidator` 的边界在 Python 里是 int 字面量，夹到边界时结果是 int。
fn bound_value(bound: f64) -> Value {
    if bound.fract() == 0.0 {
        Value::from(bound as i64)
    } else {
        Value::from(bound)
    }
}

/// `ProxyValidator.validate` 的镜像（正则 + `1 <= port <= 65535`）。
///
/// 密码字符集 `[\w!@#$%^&*()]` **包含 `@`**，所以 `u:p@ss@1.2.3.4:80` 是合法的。
/// CPython 正则会回溯到最后一个能匹配的 `@`，这里用 `rsplit_once('@')` 近似。
pub fn proxy_valid(value: &str) -> bool {
    // Python 的 `$` 也匹配结尾的单个换行
    let value = value.strip_suffix('\n').unwrap_or(value);
    if value == "Auto" || value == "Off" {
        return true;
    }
    let Some((scheme, rest)) = value.split_once("://") else {
        return false;
    };
    if !matches!(scheme, "http" | "https" | "socks4" | "socks5" | "socks5h") {
        return false;
    }
    let host_port = match rest.rsplit_once('@') {
        Some((userinfo, host_port)) => {
            if !proxy_creds_valid(userinfo) {
                return false;
            }
            host_port
        }
        None => rest,
    };
    proxy_host_valid(host_port)
}

fn proxy_creds_valid(userinfo: &str) -> bool {
    let Some((user, password)) = userinfo.split_once(':') else {
        return false;
    };
    !user.is_empty()
        && user.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !password.is_empty()
        && password
            .chars()
            .all(|c| c.is_alphanumeric() || "_!@#$%^&*()".contains(c))
}

fn proxy_host_valid(host_port: &str) -> bool {
    let Some((host, port)) = host_port.rsplit_once(':') else {
        return false;
    };
    if port.is_empty() || port.len() > 5 || !port.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    match port.parse::<u32>() {
        Ok(p) if (1..=65535).contains(&p) => {}
        _ => return false,
    }
    is_ipv4_literal(host) || is_domain(host)
}

fn is_ipv4_literal(host: &str) -> bool {
    let parts: Vec<&str> = host.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.len() <= 3
                && part.chars().all(|c| c.is_ascii_digit())
                && part.parse::<u16>().is_ok_and(|v| v <= 255)
        })
}

fn is_domain(host: &str) -> bool {
    let labels: Vec<&str> = host.split('.').collect();
    let Some((tld, rest)) = labels.split_last() else {
        return false;
    };
    if rest.is_empty()
        || tld.len() < 2
        || tld.len() > 6
        || !tld.chars().all(|c| c.is_ascii_alphabetic())
    {
        return false;
    }
    rest.iter().all(|label| {
        let chars: Vec<char> = label.chars().collect();
        !chars.is_empty()
            && chars.len() <= 63
            && chars[0].is_ascii_alphanumeric()
            && chars[chars.len() - 1].is_ascii_alphanumeric()
            && chars.iter().all(|c| c.is_ascii_alphanumeric() || *c == '-')
    })
}

fn folder_list_correct(value: &Value) -> Value {
    let Some(items) = value.as_array() else {
        return Value::Array(Vec::new());
    };
    let mut kept = Vec::new();
    for item in items {
        let Some(path) = item.as_str() else {
            // Python `Path(非 str)` 抛 TypeError；Rust 跳过该项
            continue;
        };
        if std::path::Path::new(path).exists() {
            kept.push(Value::from(folder_stored(path)));
        }
    }
    Value::Array(kept)
}

fn str_list_correct(value: &Value) -> Value {
    match value.as_array() {
        Some(items) => Value::Array(items.iter().filter(|i| i.is_string()).cloned().collect()),
        None => Value::Array(Vec::new()),
    }
}

fn json_list_correct(kind: JsonListKind, value: &Value) -> Value {
    match kind {
        JsonListKind::CategoryRules => {
            // `CategoryListValidator`：`all()` 空真 → 空 list 通过
            let valid = value.as_array().is_some_and(|items| {
                items
                    .iter()
                    .all(|i| i.get("name").is_some_and(Value::is_string))
            });
            if valid {
                value.clone()
            } else {
                Value::Array(Vec::new())
            }
        }
        JsonListKind::HeadersPresets => {
            let mut kept: Vec<Value> = match value.as_array() {
                Some(items) => items
                    .iter()
                    .filter(|i| headers_preset_valid(i))
                    .cloned()
                    .collect(),
                None => Vec::new(),
            };
            // `presets or [{"name": "默认", "headers": dict(BASE_HEADERS)}]`
            if kept.is_empty() {
                kept.push(default_headers_preset());
            }
            Value::Array(kept)
        }
        JsonListKind::IdentityPresets => {
            let kept: Vec<Value> = match value.as_array() {
                Some(items) => items
                    .iter()
                    .filter(|i| identity_preset_valid(i))
                    .cloned()
                    .collect(),
                None => Vec::new(),
            };
            // 没有回落：过滤后为空就是空
            Value::Array(kept)
        }
    }
}

fn headers_valid(headers: &Value) -> bool {
    headers
        .as_object()
        .is_some_and(|m| m.values().all(Value::is_string))
}

fn headers_preset_valid(item: &Value) -> bool {
    item.as_object().is_some_and(|m| {
        m.get("name").is_some_and(Value::is_string) && m.get("headers").is_some_and(headers_valid)
    })
}

fn identity_preset_valid(item: &Value) -> bool {
    let Some(m) = item.as_object() else {
        return false;
    };
    for key in ["name", "clientProfile", "userAgent"] {
        if !m.get(key).is_some_and(Value::is_string) {
            return false;
        }
    }
    m.get("hosts")
        .and_then(Value::as_array)
        .is_some_and(|hosts| hosts.iter().all(Value::is_string))
}

fn geometry_parse(stored: &Value) -> Value {
    let Some(text) = stored.as_str() else {
        return json!([0, 0, 0, 0]);
    };
    let parts: Vec<&str> = text.split(',').collect();
    if parts.len() != 4 {
        return json!([0, 0, 0, 0]);
    }
    let mut out = Vec::with_capacity(4);
    for part in parts {
        match part.trim().parse::<i64>() {
            Ok(v) => out.push(Value::from(v)),
            Err(_) => return json!([0, 0, 0, 0]),
        }
    }
    Value::Array(out)
}

fn geometry_format(logical: &Value) -> Value {
    let Some(items) = logical.as_array() else {
        return Value::from("0,0,0,0");
    };
    if items.len() != 4 {
        return Value::from("0,0,0,0");
    }
    let parts: Vec<String> = items
        .iter()
        .map(|item| match item {
            Value::Number(n) => n.to_string(),
            _ => "0".to_string(),
        })
        .collect();
    Value::from(parts.join(","))
}

/// `ColorSerializer.serialize` 的口径：`QColor.name(QColor.HexArgb)`，即 `#aarrggbb`。
///
/// 只认 Qt 也认的 `#rgb` / `#rrggbb` / `#aarrggbb`，外加 `ColorSerializer.deserialize`
/// 支持的 `[r, g, b]` / `[r, g, b, a]`。Qt 的颜色名（`"red"`）→ `None`（见模块头）。
pub fn color_hex_argb(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            let text = text.trim();
            let hex = text.strip_prefix('#')?;
            if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            let lower = hex.to_ascii_lowercase();
            match lower.len() {
                3 => Some(format!(
                    "#ff{0}{0}{1}{1}{2}{2}",
                    &lower[0..1],
                    &lower[1..2],
                    &lower[2..3]
                )),
                6 => Some(format!("#ff{lower}")),
                8 => Some(format!("#{lower}")),
                _ => None,
            }
        }
        Value::Array(items) => {
            if items.len() != 3 && items.len() != 4 {
                return None;
            }
            let mut channels = Vec::with_capacity(4);
            for item in items {
                let v = item.as_f64()?;
                if !(0.0..=255.0).contains(&v) {
                    return None;
                }
                channels.push(v as u8);
            }
            let alpha = if channels.len() == 4 {
                channels[3]
            } else {
                255
            };
            Some(format!(
                "#{alpha:02x}{:02x}{:02x}{:02x}",
                channels[0], channels[1], channels[2]
            ))
        }
        _ => None,
    }
}

/// `FolderValidator.correct` 的路径部分：绝对路径 + 正斜杠 + normpath（**不建目录**）。
pub fn folder_stored(value: &str) -> String {
    let slashed = value.replace('\\', "/");
    let mut parts: Vec<&str> = Vec::new();
    for segment in slashed.split('/') {
        match segment {
            // `PurePath` 会丢掉空段与 `.`，但**保留 `..`**
            "" | "." => {}
            other => parts.push(other),
        }
    }
    let body = parts.join("/");
    if slashed.starts_with('/') {
        // POSIX 只特殊对待「恰好两个」前导斜杠
        let double = slashed.starts_with("//") && !slashed.starts_with("///");
        return if double {
            format!("//{body}")
        } else {
            format!("/{body}")
        };
    }
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    if body.is_empty() {
        cwd
    } else {
        format!("{cwd}/{body}")
    }
}

/// `cfg.py::BASE_HEADERS` 的一份新 map（键序与字面量一致）。
pub fn base_headers() -> Map<String, Value> {
    let mut map = Map::new();
    for (key, value) in BASE_HEADERS {
        map.insert((*key).to_string(), Value::from(*value));
    }
    map
}

/// `HeadersPresetListValidator` 的回落项。
pub fn default_headers_preset() -> Value {
    json!({ "name": "默认", "headers": base_headers() })
}

// ---------------------------------------------------------------------------
// 公共 API
// ---------------------------------------------------------------------------

impl Spec {
    /// 磁盘上的缺省值（= `serialize(correct(raw_default))`）。
    pub fn default_stored(&self) -> Value {
        self.kind.serialize(&self.default_value())
    }

    /// 逻辑值形态的缺省值（= `correct(raw_default)`）。
    pub fn default_value(&self) -> Value {
        self.kind.correct(&self.kind.raw_default())
    }

    /// 从 `UserConfig` 读一项，返回**逻辑值**（已过 `deserialize` + `correct`）。
    ///
    /// 键缺失 → 缺省值（Python 侧同样保持 `ConfigItem` 的缺省）。
    pub fn read(&self, config: &UserConfig) -> Value {
        match config.get(self.group, self.key) {
            Some(stored) => self.kind.correct(&self.kind.deserialize(stored)),
            None => self.default_value(),
        }
    }

    /// 写一项：`correct` → `serialize` → 落盘，返回落盘形态。
    pub fn write(&self, config: &mut UserConfig, value: &Value) -> Value {
        let stored = self.kind.serialize(&self.kind.correct(value));
        config.set(self.group, self.key, stored.clone());
        stored
    }
}

/// 按 python 属性名查一项。
pub fn spec(attr: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.attr == attr)
}

/// 纯 Rust 造出与 Python `cfg` 逐字节相同的 `UserConfig`。
pub fn default_config() -> UserConfig {
    default_config_with_download_folder(DEFAULT_DOWNLOAD_FOLDER)
}

/// 同上，但 `GeneralDownload/DownloadFolder` 用给定路径（Android 分支用）。
pub fn default_config_with_download_folder(folder: &str) -> UserConfig {
    let mut config = UserConfig::new();
    for s in SPECS {
        let logical = if s.attr == "downloadFolder" {
            Value::from(folder)
        } else {
            s.kind.raw_default()
        };
        let stored = s.kind.serialize(&s.kind.correct(&logical));
        config.set(s.group, s.key, stored);
    }
    config
}

/// 按属性名读一项的逻辑值。
pub fn read(config: &UserConfig, attr: &str) -> Option<Value> {
    spec(attr).map(|s| s.read(config))
}

/// 按属性名写一项，返回落盘形态。
pub fn write(config: &mut UserConfig, attr: &str, value: &Value) -> Option<Value> {
    spec(attr).map(|s| s.write(config, value))
}

pub fn read_bool(config: &UserConfig, attr: &str) -> Option<bool> {
    Some(match read(config, attr)? {
        Value::Bool(b) => b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => matches!(s.as_str(), "true" | "True" | "1"),
        _ => false,
    })
}

pub fn read_i64(config: &UserConfig, attr: &str) -> Option<i64> {
    Some(match read(config, attr)? {
        Value::Number(n) => n.as_f64().map(|f| f as i64)?,
        Value::String(s) => s.trim().parse().ok()?,
        _ => return None,
    })
}

pub fn read_f64(config: &UserConfig, attr: &str) -> Option<f64> {
    Some(match read(config, attr)? {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().parse().ok()?,
        _ => return None,
    })
}

pub fn read_text(config: &UserConfig, attr: &str) -> Option<String> {
    Some(match read(config, attr)? {
        Value::String(s) => s,
        Value::Bool(b) => if b { "True" } else { "False" }.to_string(),
        Value::Number(n) => match n.as_i64() {
            Some(i) => i.to_string(),
            None => crate::config::py_float_repr(n.as_f64()?),
        },
        _ => return None,
    })
}

/// `cfg.py::toProxyUrl`。
pub fn to_proxy_url(url: &str) -> String {
    if !url.contains("://") {
        return format!("http://{url}");
    }
    if let Some(rest) = url.strip_prefix("socks://") {
        return format!("socks5://{rest}");
    }
    url.to_string()
}

/// `cfg.py::currentHeadersPresetIndex`：`min(current, len(presets) - 1)`。
///
/// 返回 `i64` 是因为列表为空时 Python 会得到 `-1`（随后索引 `[-1]` 取最后一项）。
pub fn current_headers_preset_index(config: &UserConfig) -> i64 {
    let current = read_i64(config, "currentHeadersPreset").unwrap_or(0);
    let count = read(config, "headersPresets")
        .and_then(|v| v.as_array().map(|a| a.len()))
        .unwrap_or(0) as i64;
    current.min(count - 1)
}

/// `cfg.py::currentHeaders`：当前预设的 headers 的一份拷贝（键序保留）。
pub fn current_headers(config: &UserConfig) -> Option<Map<String, Value>> {
    let presets = read(config, "headersPresets")?;
    let items = presets.as_array()?;
    if items.is_empty() {
        return None;
    }
    let index = current_headers_preset_index(config);
    let item = if index < 0 {
        items.last()?
    } else {
        items.get(index as usize)?
    };
    item.get("headers")?.as_object().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真机 `UserConfig.json` 的缺省内容（第 78 步由 Python 生成，不手写）。
    const DEFAULT_JSON: &str = include_str!("../tests/fixtures/user_config_default_linux.json");

    /// `dir(Config)` 的实测顺序（42 项，`2026-10-18` 在 venv 里跑出来）。
    const DIR_ORDER: &[&str] = &[
        "aria2RpcEmulateFingerprint",
        "aria2RpcPort",
        "aria2RpcToken",
        "autoSpeedUp",
        "browserExtensionPairToken",
        "browserExtensionPort",
        "categoryRules",
        "clientProfile",
        "closeMode",
        "currentHeadersPreset",
        "downloadFolder",
        "dpiScale",
        "expandedSettingGroups",
        "fontFamilies",
        "geometry",
        "hasCompletedOobe",
        "headersPresets",
        "identityPresets",
        "isAria2RpcEnabled",
        "isBrowserExtensionEnabled",
        "isCategoryEnabled",
        "isClipboardListenerEnabled",
        "isSpeedLimitEnabled",
        "isUrlSchemeRegistered",
        "language",
        "maxReassignSize",
        "maxTaskNum",
        "memoryDownloadFolders",
        "nameConflict",
        "preBlockNum",
        "proxyServer",
        "settingGroupOrder",
        "shouldCheckUpdateAtStartup",
        "shouldDeleteFilesOnRemove",
        "shouldDraftTakenDownload",
        "shouldPreserveLastModified",
        "shouldRunAtLogin",
        "shouldUseSystemDns",
        "shouldVerifySsl",
        "speedLimitation",
        "themeColor",
        "themeMode",
    ];

    fn assert_same_text(actual: &str, expected: &str) {
        if actual == expected {
            return;
        }
        let a: Vec<&str> = actual.lines().collect();
        let e: Vec<&str> = expected.lines().collect();
        for i in 0..a.len().max(e.len()) {
            let left = a.get(i).copied().unwrap_or("<缺行>");
            let right = e.get(i).copied().unwrap_or("<缺行>");
            if left != right {
                panic!("第 {} 行不同：\n  实际: {left}\n  期望: {right}", i + 1);
            }
        }
        panic!(
            "行内容相同但字节不同（实际 {} 字节 / 期望 {} 字节）",
            actual.len(),
            expected.len()
        );
    }

    #[test]
    fn specs_are_in_python_dir_order() {
        let attrs: Vec<&str> = SPECS.iter().map(|s| s.attr).collect();
        assert_eq!(attrs.len(), 42);
        assert_eq!(attrs, DIR_ORDER);
    }

    #[test]
    fn defaults_are_byte_identical_to_python() {
        // 这一条同时钉住 42 项缺省值、每个 validator 的 correct()、序列化方式和落盘顺序
        assert_same_text(&default_config().to_json_text(), DEFAULT_JSON);
    }

    #[test]
    fn specs_cover_every_group_and_key_of_the_fixture() {
        let fixture: Value = serde_json::from_str(DEFAULT_JSON).unwrap();
        let groups = fixture.as_object().unwrap();
        // 组序 = 各组首项在 SPECS 里的出现次序
        let mut spec_groups: Vec<&str> = Vec::new();
        for s in SPECS {
            if !spec_groups.contains(&s.group) {
                spec_groups.push(s.group);
            }
        }
        let fixture_groups: Vec<&str> = groups.keys().map(String::as_str).collect();
        assert_eq!(spec_groups, fixture_groups);

        // 组内键序 = 属性名字母序（即 SPECS 里的相对次序）
        for (group, entries) in groups {
            let fixture_keys: Vec<&str> = entries
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            let spec_keys: Vec<&str> = SPECS
                .iter()
                .filter(|s| s.group == group)
                .map(|s| s.key)
                .collect();
            assert_eq!(spec_keys, fixture_keys, "组 {group} 的键序不一致");
        }
    }

    #[test]
    fn default_config_reads_back_typed() {
        let cfg = default_config();
        assert_eq!(read_bool(&cfg, "shouldVerifySsl"), Some(false));
        assert_eq!(read_bool(&cfg, "isClipboardListenerEnabled"), Some(true));
        assert_eq!(read_bool(&cfg, "shouldUseSystemDns"), Some(true));
        assert_eq!(read_i64(&cfg, "maxTaskNum"), Some(3));
        assert_eq!(read_i64(&cfg, "currentHeadersPreset"), Some(0));
        assert_eq!(read_i64(&cfg, "speedLimitation"), Some(4_194_304));
        assert_eq!(read_f64(&cfg, "dpiScale"), Some(0.0));
        assert_eq!(read_text(&cfg, "aria2RpcToken").as_deref(), Some(""));
        assert_eq!(read_text(&cfg, "themeColor").as_deref(), Some("#ff009faa"));
        assert_eq!(read_text(&cfg, "closeMode").as_deref(), Some("Ask"));
        assert_eq!(read_text(&cfg, "language").as_deref(), Some("Auto"));
        assert_eq!(read_text(&cfg, "nameConflict").as_deref(), Some("keepBoth"));
        assert_eq!(read_text(&cfg, "clientProfile").as_deref(), Some("auto"));
        assert_eq!(read_text(&cfg, "proxyServer").as_deref(), Some("Auto"));
        assert_eq!(
            read_text(&cfg, "downloadFolder").as_deref(),
            Some(DEFAULT_DOWNLOAD_FOLDER)
        );
        assert_eq!(read(&cfg, "geometry"), Some(json!([0, 0, 0, 0])));
        assert_eq!(
            read(&cfg, "fontFamilies"),
            Some(json!(["Segoe UI", "Microsoft YaHei", "PingFang SC"]))
        );
        assert_eq!(
            read(&cfg, "headersPresets"),
            Some(json!([default_headers_preset()]))
        );
        assert_eq!(
            read(&cfg, "identityPresets"),
            Some(json!([{
                "name": "百度网盘客户端",
                "clientProfile": "raw",
                "userAgent": "pan.baidu.com",
                "hosts": ["*.pcs.baidu.com"],
                "isEnabled": true
            }]))
        );
        assert_eq!(read(&cfg, "categoryRules"), Some(json!([])));
        assert_eq!(read(&cfg, "expandedSettingGroups"), Some(json!([])));
    }

    #[test]
    fn range_values_are_clamped_to_bounds() {
        let mut cfg = default_config();
        assert_eq!(write(&mut cfg, "maxTaskNum", &json!(0)), Some(json!(1)));
        assert_eq!(write(&mut cfg, "maxTaskNum", &json!(999)), Some(json!(10)));
        assert_eq!(write(&mut cfg, "preBlockNum", &json!(-5)), Some(json!(1)));
        assert_eq!(
            write(&mut cfg, "speedLimitation", &json!(1)),
            Some(json!(1024))
        );
        assert_eq!(
            write(&mut cfg, "speedLimitation", &json!(1_000_000_000i64)),
            Some(json!(104_857_600))
        );
        assert_eq!(
            write(&mut cfg, "maxReassignSize", &json!(63)),
            Some(json!(64))
        );
        assert_eq!(
            write(&mut cfg, "currentHeadersPreset", &json!(100)),
            Some(json!(99))
        );
        assert_eq!(write(&mut cfg, "dpiScale", &json!(7.0)), Some(json!(5)));
        assert_eq!(write(&mut cfg, "dpiScale", &json!(-1.0)), Some(json!(0)));
    }

    #[test]
    fn range_non_number_falls_back_to_default() {
        // Python: `max(1, "7")` 抛 TypeError → `@exceptionHandler()` 中断整个 load，
        // 该项保持缺省。Rust 侧同样退回缺省值。
        let mut cfg = default_config();
        assert_eq!(write(&mut cfg, "maxTaskNum", &json!("7")), Some(json!(3)));
        assert_eq!(write(&mut cfg, "dpiScale", &json!("1.5")), Some(json!(0)));
    }

    #[test]
    fn range_keeps_float_shape_when_python_does() {
        let mut cfg = default_config();
        // `max(0, 2.0)` → 2.0（float 保留）→ 落盘 `2.0`
        assert_eq!(write(&mut cfg, "dpiScale", &json!(2.0)), Some(json!(2.0)));
        // `max(0, 0.0)` 相等 → 返回第一个参数（int 0）→ 落盘 `0`
        assert_eq!(write(&mut cfg, "dpiScale", &json!(0.0)), Some(json!(0)));
    }

    #[test]
    fn bool_fallback_is_options_first() {
        let mut cfg = default_config();
        // `BoolValidator` = `OptionsValidator([True, False])` → 非法值取 `options[0]`
        assert_eq!(
            write(&mut cfg, "isSpeedLimitEnabled", &json!("False")),
            Some(json!(true))
        );
        assert_eq!(
            write(&mut cfg, "isSpeedLimitEnabled", &json!(null)),
            Some(json!(true))
        );
        // `0 in [True, False]` 为真 → 原样保留数字形态
        assert_eq!(
            write(&mut cfg, "isSpeedLimitEnabled", &json!(0)),
            Some(json!(0))
        );
        assert_eq!(
            write(&mut cfg, "isSpeedLimitEnabled", &json!(false)),
            Some(json!(false))
        );
    }

    #[test]
    fn options_fall_back_to_first_option() {
        let mut cfg = default_config();
        // `OptionsValidator(Language)` 的 options 是 enum 成员 → `options[0]` 是
        // `Language.CHINESE_SIMPLIFIED`，序列化后是 "zh_CN"（不是 "Auto"）
        assert_eq!(
            write(&mut cfg, "language", &json!("klingon")),
            Some(json!("zh_CN"))
        );
        // `OptionsValidator(Theme)` 同理 → "Light"
        assert_eq!(
            write(&mut cfg, "themeMode", &json!("Sepia")),
            Some(json!("Light"))
        );
        assert_eq!(
            write(&mut cfg, "language", &json!("ja_JP")),
            Some(json!("ja_JP"))
        );
        // `CloseMode.QUIT.value == "Quit"`，不是 "QUIT"
        assert_eq!(
            write(&mut cfg, "closeMode", &json!("QUIT")),
            Some(json!("Ask"))
        );
        assert_eq!(
            write(&mut cfg, "closeMode", &json!("Quit")),
            Some(json!("Quit"))
        );
        assert_eq!(
            write(&mut cfg, "nameConflict", &json!("keep")),
            Some(json!("keepBoth"))
        );
        assert_eq!(
            write(&mut cfg, "themeMode", &json!("Dark")),
            Some(json!("Dark"))
        );
    }

    #[test]
    fn proxy_validator_matches_python_pattern() {
        for ok in [
            "Auto",
            "Off",
            "http://1.2.3.4:8080",
            "https://example.com:443",
            "socks4://a-b.co:80",
            "socks5://user:pass@127.0.0.1:1080",
            // 密码字符集含 `@` → 最后一个 `@` 才是分隔符
            "socks5h://user:p@ss@1.2.3.4:1080",
            "http://u:p%41!@example.com:80",
            "http://example.com:080",
            "http://0.0.0.0:1",
            "http://255.255.255.255:65535",
        ] {
            assert!(proxy_valid(ok), "应当合法：{ok}");
        }
        for bad in [
            "",
            "1.2.3.4:80",
            "ftp://1.2.3.4:80",
            "HTTP://1.2.3.4:80",
            "http://1.2.3.4",
            "http://1.2.3.4:0",
            "http://1.2.3.4:65536",
            "http://1.2.3.4:70000",
            "http://256.1.1.1:80",
            "http://host:80",
            "http://example.c:80",
            "http://example.toolongtld:80",
            "http://example.com:80/x",
            "http://u:pa ss@example.com:80",
            "http://:pass@example.com:80",
        ] {
            assert!(!proxy_valid(bad), "应当非法：{bad}");
        }
    }

    #[test]
    fn proxy_write_falls_back_to_auto() {
        let mut cfg = default_config();
        assert_eq!(
            write(&mut cfg, "proxyServer", &json!("ftp://1.2.3.4:80")),
            Some(json!("Auto"))
        );
        assert_eq!(
            write(&mut cfg, "proxyServer", &json!("socks5://127.0.0.1:1080")),
            Some(json!("socks5://127.0.0.1:1080"))
        );
        assert_eq!(
            write(&mut cfg, "proxyServer", &json!(5)),
            Some(json!("Auto"))
        );
    }

    #[test]
    fn headers_presets_filter_and_fall_back() {
        let mut cfg = default_config();
        let fallback = json!([default_headers_preset()]);
        assert_eq!(
            write(&mut cfg, "headersPresets", &json!([])),
            Some(json!(py_json_dumps(&fallback)))
        );
        // 逐项过滤；过滤后为空 → 回落默认项
        let mixed = json!([
            {"name": "甲", "headers": {"a": "b"}},
            {"name": "乙"},
            {"headers": {}},
            {"name": "丙", "headers": {"a": 1}},
        ]);
        let stored = write(&mut cfg, "headersPresets", &mixed).unwrap();
        let kept = json!([{"name": "甲", "headers": {"a": "b"}}]);
        assert_eq!(read(&cfg, "headersPresets"), Some(kept.clone()));
        assert_eq!(stored, json!(py_json_dumps(&kept)));
    }

    #[test]
    fn garbage_stored_headers_presets_fall_back() {
        let mut cfg = default_config();
        // 磁盘上是非法 JSON 串 → `json.loads` 失败 → `[]` → 回落默认项
        cfg.set("Network", "HeadersPresets", json!("not json"));
        assert_eq!(
            read(&cfg, "headersPresets"),
            Some(json!([default_headers_preset()]))
        );
        // 磁盘上直接是 list（Python `json.loads(list)` 抛 TypeError）→ 同样回落
        cfg.set(
            "Network",
            "HeadersPresets",
            json!([{"name": "甲", "headers": {}}]),
        );
        assert_eq!(
            read(&cfg, "headersPresets"),
            Some(json!([default_headers_preset()]))
        );
    }

    #[test]
    fn identity_presets_have_no_fallback() {
        let mut cfg = default_config();
        assert_eq!(
            write(&mut cfg, "identityPresets", &json!([])),
            Some(json!("[]"))
        );
        let mixed = json!([
            {"name": "a", "clientProfile": "raw", "userAgent": "x", "hosts": ["h"]},
            {"name": "b", "clientProfile": "raw", "userAgent": "x"},
            {"name": "c", "clientProfile": "raw", "userAgent": "x", "hosts": [1]},
        ]);
        write(&mut cfg, "identityPresets", &mixed);
        assert_eq!(
            read(&cfg, "identityPresets"),
            Some(json!([
                {"name": "a", "clientProfile": "raw", "userAgent": "x", "hosts": ["h"]}
            ]))
        );
    }

    #[test]
    fn category_rules_are_all_or_nothing() {
        let mut cfg = default_config();
        let good = json!([{"name": "视频", "rules": []}]);
        assert_eq!(
            write(&mut cfg, "categoryRules", &good),
            Some(json!(py_json_dumps(&good)))
        );
        // 任一项不合法 → 整个 list 清空（`CategoryListValidator` 用 `all()`）
        write(
            &mut cfg,
            "categoryRules",
            &json!([{"name": "视频"}, {"rules": []}]),
        );
        assert_eq!(read(&cfg, "categoryRules"), Some(json!([])));
    }

    #[test]
    fn geometry_round_trips_through_string() {
        let mut cfg = default_config();
        assert_eq!(
            write(&mut cfg, "geometry", &json!([10, 20, 300, 400])),
            Some(json!("10,20,300,400"))
        );
        assert_eq!(read(&cfg, "geometry"), Some(json!([10, 20, 300, 400])));
        cfg.set("Software", "Geometry", json!("1,2,3"));
        assert_eq!(read(&cfg, "geometry"), Some(json!([0, 0, 0, 0])));
        cfg.set("Software", "Geometry", json!("1,2,x,4"));
        assert_eq!(read(&cfg, "geometry"), Some(json!([0, 0, 0, 0])));
        // Python `int(" 2 ")` 合法
        cfg.set("Software", "Geometry", json!(" 1 , 2 ,3, 4 "));
        assert_eq!(read(&cfg, "geometry"), Some(json!([1, 2, 3, 4])));
    }

    #[test]
    fn color_is_normalized_to_argb() {
        let mut cfg = default_config();
        assert_eq!(
            write(&mut cfg, "themeColor", &json!("#009faa")),
            Some(json!("#ff009faa"))
        );
        assert_eq!(
            write(&mut cfg, "themeColor", &json!("#ABC")),
            Some(json!("#ffaabbcc"))
        );
        assert_eq!(
            write(&mut cfg, "themeColor", &json!("#80FF0000")),
            Some(json!("#80ff0000"))
        );
        // Qt 颜色名不认 → 退回缺省
        assert_eq!(
            write(&mut cfg, "themeColor", &json!("red")),
            Some(json!("#ff009faa"))
        );
        // `ColorSerializer.deserialize` 支持 list
        assert_eq!(
            write(&mut cfg, "themeColor", &json!([255, 0, 0])),
            Some(json!("#ffff0000"))
        );
        assert_eq!(
            write(&mut cfg, "themeColor", &json!([255, 0, 0, 128])),
            Some(json!("#80ff0000"))
        );
        assert_eq!(color_hex_argb(&json!("#12345")), None);
        assert_eq!(color_hex_argb(&json!(42)), None);
    }

    #[test]
    fn string_lists_filter_non_strings() {
        let mut cfg = default_config();
        assert_eq!(
            write(
                &mut cfg,
                "expandedSettingGroups",
                &json!([1, "a", true, "b"])
            ),
            Some(json!(["a", "b"]))
        );
        assert_eq!(
            write(&mut cfg, "expandedSettingGroups", &json!("x")),
            Some(json!([]))
        );
        // `fontFamilies` 没有 validator → 恒等
        assert_eq!(
            write(&mut cfg, "fontFamilies", &json!(["X"])),
            Some(json!(["X"]))
        );
    }

    #[test]
    fn folder_is_absolute_and_normalized() {
        let mut cfg = default_config();
        assert_eq!(
            write(&mut cfg, "downloadFolder", &json!("//Downloads")),
            Some(json!("//Downloads"))
        );
        // `Path.absolute()` 只做 `PurePath` 归一（丢空段与 `.`），**不解析 `..`**
        let stored = write(&mut cfg, "downloadFolder", &json!("Downloads/./x/../y/")).unwrap();
        let text = stored.as_str().unwrap();
        assert!(text.starts_with('/'), "{text}");
        assert!(text.ends_with("/Downloads/x/../y"), "{text}");

        assert_eq!(folder_stored("/a//b/./c/../d/"), "/a/b/c/../d");
        assert_eq!(folder_stored("///a"), "/a");
        assert_eq!(folder_stored("//Downloads"), "//Downloads");
        assert_eq!(folder_stored("/a/b/"), "/a/b");
        assert_eq!(folder_stored("/"), "/");
        assert_eq!(folder_stored("/.."), "/..");
        let cwd = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert_eq!(folder_stored("../a"), format!("{cwd}/../a"));
        assert_eq!(folder_stored(""), cwd);
    }

    #[test]
    fn android_download_folder_override() {
        let cfg = default_config_with_download_folder(ANDROID_DOWNLOAD_FOLDER);
        assert_eq!(
            read_text(&cfg, "downloadFolder").as_deref(),
            Some(ANDROID_DOWNLOAD_FOLDER)
        );
    }

    #[test]
    fn folder_list_keeps_only_existing_paths() {
        let mut cfg = default_config();
        let tmp = std::env::temp_dir().to_string_lossy().replace('\\', "/");
        let expected = folder_stored(&tmp);
        assert_eq!(
            write(
                &mut cfg,
                "memoryDownloadFolders",
                &json!([tmp, "/definitely/not/here-xyz"])
            ),
            Some(json!([expected]))
        );
        assert_eq!(
            write(
                &mut cfg,
                "memoryDownloadFolders",
                &json!(["/definitely/not/here-xyz"])
            ),
            Some(json!([]))
        );
    }

    #[test]
    fn to_proxy_url_mirrors_python() {
        assert_eq!(to_proxy_url("1.2.3.4:80"), "http://1.2.3.4:80");
        assert_eq!(to_proxy_url("socks://h:1080"), "socks5://h:1080");
        assert_eq!(to_proxy_url("socks5://h:1080"), "socks5://h:1080");
        assert_eq!(to_proxy_url("http://h:1"), "http://h:1");
        assert_eq!(to_proxy_url(""), "http://");
    }

    #[test]
    fn current_headers_follows_the_preset_index() {
        let mut cfg = default_config();
        assert_eq!(current_headers_preset_index(&cfg), 0);
        assert_eq!(current_headers(&cfg), Some(base_headers()));

        let presets = json!([
            {"name": "甲", "headers": {"a": "1"}},
            {"name": "乙", "headers": {"b": "2"}},
        ]);
        write(&mut cfg, "headersPresets", &presets);
        write(&mut cfg, "currentHeadersPreset", &json!(1));
        assert_eq!(current_headers_preset_index(&cfg), 1);
        assert_eq!(current_headers(&cfg).unwrap().get("b"), Some(&json!("2")));

        // 越界 → `min(current, len - 1)`
        write(&mut cfg, "currentHeadersPreset", &json!(99));
        assert_eq!(current_headers_preset_index(&cfg), 1);
    }

    #[test]
    fn legacy_stored_strings_fall_back_to_defaults() {
        let mut cfg = default_config();
        cfg.set("GeneralDownload", "MaxTaskNum", json!("7"));
        assert_eq!(read(&cfg, "maxTaskNum"), Some(json!(3)));
        // `BoolValidator.correct("False")` → `options[0]` → True（与 Python 一致）
        cfg.set("GeneralDownload", "isSpeedLimitEnabled", json!("False"));
        assert_eq!(read(&cfg, "isSpeedLimitEnabled"), Some(json!(true)));
    }

    #[test]
    fn write_then_reload_round_trips() {
        let mut cfg = default_config();
        write(&mut cfg, "maxTaskNum", &json!(7));
        write(&mut cfg, "shouldVerifySsl", &json!(true));
        write(&mut cfg, "language", &json!("ja_JP"));
        write(&mut cfg, "geometry", &json!([1, 2, 3, 4]));
        write(
            &mut cfg,
            "headersPresets",
            &json!([{"name": "自定义", "headers": {"a": "b"}}]),
        );
        write(&mut cfg, "currentHeadersPreset", &json!(0));

        let text = cfg.to_json_text();
        let reloaded = UserConfig::from_json(&text).unwrap();
        assert_eq!(read(&reloaded, "maxTaskNum"), Some(json!(7)));
        assert_eq!(read_bool(&reloaded, "shouldVerifySsl"), Some(true));
        assert_eq!(read_text(&reloaded, "language").as_deref(), Some("ja_JP"));
        assert_eq!(read(&reloaded, "geometry"), Some(json!([1, 2, 3, 4])));
        assert_eq!(
            current_headers(&reloaded).unwrap().get("a"),
            Some(&json!("b"))
        );
        assert_eq!(text, reloaded.to_json_text());
    }

    #[test]
    fn spec_lookup_by_python_attribute() {
        let s = spec("maxTaskNum").unwrap();
        assert_eq!((s.group, s.key), ("GeneralDownload", "MaxTaskNum"));
        assert_eq!(spec("speedLimitation").unwrap().key, "SpeedLimitation");
        assert_eq!(spec("themeColor").unwrap().group, "QFluentWidgets");
        assert!(spec("MaxTaskNum").is_none());
        assert!(spec("nope").is_none());
    }
}
