//! `UserConfig.json` 存取（第 78 步按真机磁盘格式重写）。
//!
//! ## 磁盘协议（实证，不是推测）
//!
//! 第 49 步按「每个项经 validator `format()` 后都是字符串」实现，**是错的**：
//! qfluentwidgets 1.18 的 `ConfigItem.serialize()` 只有一句
//! `self.serializer.serialize(self.value)`，而 `ConfigItem` 的默认 serializer
//! 是恒等 —— 没显式传 serializer 的项，落盘就是**裸的 JSON 值**：
//!
//! ```json
//! "EmulateFingerprint": false,     // BoolValidator，无 serializer
//! "MaxTaskNum": 3,                 // RangeConfigItem，无 serializer
//! "HistoryDownloadFolder": [],     // FolderListValidator，无 serializer
//! "CategoryRules": "[]",           // JsonConfigSerializer → json.dumps 串
//! "Language": "Auto",              // EnumSerializer → enum.value
//! "Geometry": "0,0,0,0"            // GeometrySerializer → "x,y,w,h"
//! ```
//!
//! 旧实现写 `"False"`/`"3"` 这类字符串，Python 侧读回是**灾难**：
//! `BoolValidator.correct("False")` → `OptionsValidator.correct` →
//! 不在 `[True, False]` 里 → 取 `options[0]` → **变成 True**；
//! `RangeValidator.correct("7")` → `max(1, "7")` → `TypeError`，
//! `QConfig.load` 被 `@exceptionHandler()` 兜住 → **整个 load 中断**，
//! 后面所有项保持缺省（实测：`{"MaxTaskNum": "7", "PreBlockNum": 16}` 读入后
//! `MaxTaskNum` 仍是缺省 3、`PreBlockNum` 仍是缺省 8）。所以本模块只写
//! 真机同构的值类型。
//!
//! 其余协议不变：
//! - 写盘 = `json.dump(toDict(), f, ensure_ascii=False, indent=4)`，无尾随换行；
//! - 组/键序 = `dir(Config)` 的**属性名字母序**（组按首次出现排），
//!   `serde_json` 开了 `preserve_order` → 本模块 `load` 保序、写回逐字节同构；
//! - `QFluentWidgets` 组（`ThemeColor`/`FontFamilies`，继承自 `QConfig`）
//!   也在 Python 的 `toDict()` 里，必须原样保留；
//! - 平台分支键（`backgroundEffect` 仅 win32/darwin、三个 dock 键仅 darwin）
//!   不在这里补 —— 缺键即缺省，读写都不造键。
//!
//! ## 与 Python 的已知差异
//!
//! - **容错策略**：Python 在首个非法值处中断整个 load（见上），本模块逐键跳过
//!   非法/不可解析值、保留其余项。Rust 侧不该因为一个脏键丢掉整份配置。
//! - `get_*` 接受「数字/布尔的字符串形式」这一遗留形态（手改文件、旧版
//!   Rust 写出的档），Python 会拒收；写回时按 set 的入参类型落盘，不做批量归一。
//!
//! 金文件由 `tests/fixtures/generate_user_config_fixture.py` 在 venv 里跑出：
//! `user_config_default_linux.json`（空目录 load 后 save）、
//! `user_config_mutated_expected.json`（同一份再按固定改动 `qconfig.set` 后
//! save）、`py_json_writer.jsonl`、`py_float_repr.jsonl`。

use std::path::Path;

use serde_json::{Map, Value};

/// group → (key → 落盘值)。值类型是 JSON 原生标量/数组/对象，
/// **不是**统一字符串（见模块头）。
pub type GroupMap = Map<String, Value>;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct UserConfig {
    pub groups: GroupMap,
}

impl UserConfig {
    pub fn new() -> Self {
        Self::default()
    }

    /// 从磁盘读。文件不存在/不可读 → `io::Error`（调用方决定缺省）。
    pub fn load(path: &Path) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Self::from_json(&text).map_err(|error| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
        })
    }

    /// 从同态 JSON 文本解析：保序（`preserve_order`），组层级非对象的一律跳过，
    /// 值原样保留（标量/数组/对象都收）。
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        let value: Value = serde_json::from_str(text)?;
        let mut groups = GroupMap::new();
        if let Value::Object(map) = value {
            for (group, items) in map {
                if matches!(items, Value::Object(_)) {
                    groups.insert(group, items);
                }
            }
        }
        Ok(Self { groups })
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, self.to_json_text())
    }

    /// `json.dump(..., ensure_ascii=False, indent=4)` 的逐字节镜像。
    pub fn to_json_text(&self) -> String {
        let mut out = String::new();
        write_indented(&Value::Object(self.groups.clone()), 0, &mut out);
        out
    }

    pub fn entries(&self, group: &str) -> Option<&Map<String, Value>> {
        match self.groups.get(group)? {
            Value::Object(map) => Some(map),
            _ => None,
        }
    }

    pub fn get(&self, group: &str, key: &str) -> Option<&Value> {
        self.entries(group)?.get(key)
    }

    /// 只认 `Value::String`（`DownloadFolder`/`ProxyServer`/`CategoryRules`
    /// 这类本来就是字符串的项）。
    pub fn get_str(&self, group: &str, key: &str) -> Option<&str> {
        match self.get(group, key)? {
            Value::String(text) => Some(text.as_str()),
            _ => None,
        }
    }

    /// python `str(value)` 口径：布尔 → `"True"`/`"False"`，数字 → 十进制文本，
    /// 字符串原样；数组/对象 → `None`。给「磁盘存数字、调用方要文本」的项用
    /// （`Network.CurrentHeadersPreset` 落盘是 `0`，不是 `"0"`）。
    pub fn get_py_str(&self, group: &str, key: &str) -> Option<String> {
        match self.get(group, key)? {
            Value::String(text) => Some(text.clone()),
            Value::Bool(flag) => Some(if *flag { "True" } else { "False" }.to_string()),
            Value::Number(number) => Some(py_number_text(number)),
            _ => None,
        }
    }

    /// `BoolValidator` 项。容忍遗留的字符串布尔（`"True"`/`"False"`/小写），
    /// 其余形态 `None`。
    pub fn get_bool(&self, group: &str, key: &str) -> Option<bool> {
        match self.get(group, key)? {
            Value::Bool(flag) => Some(*flag),
            Value::String(text) => match text.as_str() {
                "True" | "true" => Some(true),
                "False" | "false" => Some(false),
                _ => None,
            },
            _ => None,
        }
    }

    /// `RangeConfigItem` 项（磁盘是数字）。整数串也收（遗留档）。
    pub fn get_i64(&self, group: &str, key: &str) -> Option<i64> {
        match self.get(group, key)? {
            Value::Number(number) => number.as_i64().or_else(|| {
                number
                    .as_f64()
                    .filter(|value| value.fract() == 0.0)
                    .map(|value| value as i64)
            }),
            Value::String(text) => text.parse().ok(),
            _ => None,
        }
    }

    /// 浮点项（`Personalization.DpiScale` 落盘 `0` 或 `0.25`）。
    pub fn get_f64(&self, group: &str, key: &str) -> Option<f64> {
        match self.get(group, key)? {
            Value::Number(number) => number.as_f64(),
            Value::String(text) => text.parse().ok(),
            _ => None,
        }
    }

    pub fn set(&mut self, group: &str, key: &str, value: Value) {
        let entry = self
            .groups
            .entry(group.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(map) = entry {
            map.insert(key.to_string(), value);
        }
    }

    pub fn set_str(&mut self, group: &str, key: &str, value: &str) {
        self.set(group, key, Value::String(value.to_string()));
    }

    pub fn set_bool(&mut self, group: &str, key: &str, value: bool) {
        self.set(group, key, Value::Bool(value));
    }

    pub fn set_i64(&mut self, group: &str, key: &str, value: i64) {
        self.set(group, key, Value::Number(value.into()));
    }

    /// `RangeConfigItem` 落盘浮点（DpiScale 的 0.25 步进）。NaN/Inf 不在项值域内
    /// （validator 侧守卫），`serde_json::Number` 表示不了 → 落 `null`。
    pub fn set_f64(&mut self, group: &str, key: &str, value: f64) {
        let number = serde_json::Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or(Value::Null);
        self.set(group, key, number);
    }

    /// `JsonConfigSerializer` 项：先 `json.dumps(value, ensure_ascii=False)`
    /// （紧凑、`", "`/`": "` 分隔），再当字符串存。
    pub fn set_py_json(&mut self, group: &str, key: &str, value: &Value) {
        self.set_str(group, key, &py_json_dumps(value));
    }
}

/// 手写 JSON 字符串转义，镜像 `json.dumps(..., ensure_ascii=False)`。
///
/// 口径来自 CPython 的 `json.encoder.ESCAPE_DCT`：`"` / `\` 反斜杠转义，
/// `\b`(0x08) / `\f`(0x0c) / `\n` / `\r` / `\t` 五个**短转义**，其余
/// `< 0x20` 的控制字符走 `\uXXXX`（小写十六进制、四位补零）；`>= 0x20`
/// 的一律原样（含 `\x7f`、`\u{2028}`、`\u{2029}`、CJK、emoji——只有
/// `ensure_ascii=True` 才会转义非 ASCII）。
///
/// 金文件 `tests/fixtures/json_escape.jsonl`（28 条，CPython 生成）
/// 钉住逐字节一致。
pub fn py_json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", ch as u32));
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// `repr(float)` 口径：最短往返表示；`|x| >= 1e16` 或 `< 1e-4` 切指数写法
/// （指数至少两位、带符号），整值补 `.0`，`-0.0` 保留符号。
///
/// 金文件 `tests/fixtures/py_float_repr.jsonl`（20 条，CPython `repr`）。
pub fn py_float_repr(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_string();
    }
    if value.is_infinite() {
        return if value > 0.0 { "inf" } else { "-inf" }.to_string();
    }
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0".to_string()
        } else {
            "0.0".to_string()
        };
    }
    let magnitude = value.abs();
    if !(1e-4..1e16).contains(&magnitude) {
        // Rust `{:e}` → "1e16"/"1.5e-7"；python → "1e+16"/"1.5e-07"。
        let text = format!("{:e}", value);
        let (mantissa, exponent) = text.split_once('e').unwrap_or((text.as_str(), "0"));
        let (sign, digits) = match exponent.strip_prefix('-') {
            Some(rest) => ('-', rest),
            None => ('+', exponent),
        };
        let digits = if digits.len() < 2 {
            format!("{:0>2}", digits)
        } else {
            digits.to_string()
        };
        return format!("{mantissa}e{sign}{digits}");
    }
    let mut text = format!("{}", value);
    if !text.contains('.') {
        text.push_str(".0");
    }
    text
}

/// `json.dumps(value, ensure_ascii=False)`（紧凑，默认分隔符 `", "` / `": "`）。
pub fn py_json_dumps(value: &Value) -> String {
    let mut out = String::new();
    write_compact(value, &mut out);
    out
}

fn py_number_text(number: &serde_json::Number) -> String {
    if let Some(value) = number.as_i64() {
        return value.to_string();
    }
    if let Some(value) = number.as_u64() {
        return value.to_string();
    }
    py_float_repr(number.as_f64().unwrap_or(0.0))
}

fn write_compact(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            out.push('{');
            for (index, (key, item)) in map.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                out.push_str(&py_json_string(key));
                out.push_str(": ");
                write_compact(item, out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                write_compact(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&py_json_scalar(other)),
    }
}

fn write_indented(value: &Value, level: usize, out: &mut String) {
    match value {
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (index, (key, item)) in map.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push('\n');
                push_indent(level + 1, out);
                out.push_str(&py_json_string(key));
                out.push_str(": ");
                write_indented(item, level + 1, out);
            }
            out.push('\n');
            push_indent(level, out);
            out.push('}');
        }
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push('\n');
                push_indent(level + 1, out);
                write_indented(item, level + 1, out);
            }
            out.push('\n');
            push_indent(level, out);
            out.push(']');
        }
        other => out.push_str(&py_json_scalar(other)),
    }
}

fn push_indent(level: usize, out: &mut String) {
    for _ in 0..level {
        out.push_str("    ");
    }
}

fn py_json_scalar(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(true) => "true".to_string(),
        Value::Bool(false) => "false".to_string(),
        Value::Number(number) => py_number_text(number),
        Value::String(text) => py_json_string(text),
        // write_indented / write_compact 已吃掉容器形态；这里防御性兜底。
        Value::Array(_) | Value::Object(_) => "null".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(name);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("读不到金文件 {}：{error}", path.display()))
    }

    fn jsonl(name: &str) -> Vec<Value> {
        fixture(name)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("金文件行不是合法 JSON"))
            .collect()
    }

    /// `cfg.py` 的 `BASE_HEADERS`（键序即 Python dict 字面量顺序，
    /// `json.dumps` 保序 → 金文件里能逐字节对上）。
    fn base_headers() -> Value {
        json!({
            "accept-encoding": "deflate, br, gzip",
            "accept-language": "zh-CN,zh;q=0.9",
            "cookie": "down_ip=1",
            "sec-fetch-dest": "document",
            "sec-fetch-mode": "navigate",
            "sec-fetch-site": "none",
            "sec-fetch-user": "?1",
            "upgrade-insecure-requests": "1",
        })
    }

    /// 空目录 `qconfig.load()` + `cfg.save()` 的逐字节回写。
    ///
    /// 这条测试同时钉住三件事：组序（`dir(Config)` 首次出现）、组内键序
    /// （属性名字母序）、以及**值类型**（布尔/数字不落成字符串）。
    #[test]
    fn default_fixture_round_trips_byte_exact() {
        let text = fixture("user_config_default_linux.json");
        let config = UserConfig::from_json(&text).expect("缺省金文件不是合法 JSON");
        assert_eq!(config.to_json_text(), text);

        let groups: Vec<&str> = config.groups.keys().map(String::as_str).collect();
        assert_eq!(
            groups,
            [
                "Aria2Rpc",
                "GeneralDownload",
                "Browser",
                "Category",
                "Network",
                "Software",
                "Personalization",
                "UI",
                "QFluentWidgets",
            ],
            "组序必须与 Python toDict() 一致"
        );
        let aria2: Vec<&str> = config
            .entries("Aria2Rpc")
            .expect("缺 Aria2Rpc 组")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(aria2, ["EmulateFingerprint", "Port", "Token", "Enabled"]);
        // 值类型是磁盘协议的核心：旧实现写 "False"/"3"，Python 读回会炸。
        assert_eq!(
            config.get("Aria2Rpc", "EmulateFingerprint"),
            Some(&Value::Bool(false))
        );
        assert_eq!(config.get("GeneralDownload", "MaxTaskNum"), Some(&json!(3)));
        assert_eq!(
            config.get("GeneralDownload", "HistoryDownloadFolder"),
            Some(&json!([]))
        );
        // JsonConfigSerializer 的三项才是字符串。
        assert_eq!(config.get_str("Category", "CategoryRules"), Some("[]"));
        assert_eq!(config.get_str("Personalization", "ThemeMode"), Some("Auto"));
        assert_eq!(config.get_str("Software", "Geometry"), Some("0,0,0,0"));
        // QFluentWidgets 组（继承自 QConfig）也在 toDict() 里。
        assert_eq!(
            config.get("QFluentWidgets", "ThemeColor"),
            Some(&json!("#ff009faa"))
        );
    }

    #[test]
    fn mutated_fixture_round_trips_byte_exact() {
        let text = fixture("user_config_mutated_expected.json");
        let config = UserConfig::from_json(&text).expect("改动后金文件不是合法 JSON");
        assert_eq!(config.to_json_text(), text);
        assert_eq!(config.get_f64("Personalization", "DpiScale"), Some(0.25));
        // 0.25 不是整数 → `get_i64` 不该给出截断值。
        assert_eq!(config.get_i64("Personalization", "DpiScale"), None);
        assert_eq!(
            config.get("GeneralDownload", "HistoryDownloadFolder"),
            Some(&json!(["/tmp", "/var/tmp"]))
        );
    }

    /// Rust 侧同一组改动 → 逐字节等于 Python `qconfig.set` 后 `save()`。
    ///
    /// 改动清单镜像 `generate_user_config_fixture.py::mutated`。
    #[test]
    fn rust_mutation_matches_python_resave() {
        let default_text = fixture("user_config_default_linux.json");
        let mut config = UserConfig::from_json(&default_text).expect("缺省金文件不是合法 JSON");

        config.set_bool("Category", "EnableCategory", true);
        config.set_i64("GeneralDownload", "MaxTaskNum", 5);
        config.set_f64("Personalization", "DpiScale", 0.25);
        config.set_py_json(
            "Category",
            "CategoryRules",
            &json!([{"name": "视频", "extensions": [".mp4", ".mkv"], "folder": "video"}]),
        );
        config.set_str("Network", "ClientProfile", "manual");
        // `CloseMode.QUIT.value == "Quit"`（不是枚举成员名 "QUIT"）。
        config.set_str("Software", "CloseMode", "Quit");
        config.set_py_json(
            "Network",
            "HeadersPresets",
            &json!([
                {"name": "默认", "headers": base_headers()},
                {"name": "自定义", "headers": {"a": "b", "x-trace": "1"}},
            ]),
        );
        config.set_i64("Network", "CurrentHeadersPreset", 1);
        config.set_str("GeneralDownload", "DownloadFolder", "/tmp");
        // GeometrySerializer：元组 → "x,y,w,h"。
        config.set_str("Software", "Geometry", "10,20,800,600");
        config.set_str(
            "GeneralDownload",
            "ProxyServer",
            "socks5://u:p@1.2.3.4:1080",
        );
        config.set_bool("GeneralDownload", "isSpeedLimitEnabled", true);
        config.set_i64("GeneralDownload", "SpeedLimitation", 8_388_608);
        config.set(
            "GeneralDownload",
            "HistoryDownloadFolder",
            json!(["/tmp", "/var/tmp"]),
        );
        config.set(
            "UI",
            "ExpandedSettingGroups",
            json!(["download", "network"]),
        );

        let expected = fixture("user_config_mutated_expected.json");
        assert_eq!(config.to_json_text(), expected);
    }

    /// `get_*` 的类型口径必须跟磁盘一致：数字项不能当字符串读，
    /// 反之亦然；`get_py_str` 才是 python `str()` 口径。
    #[test]
    fn typed_accessors_follow_python_scalar_kinds() {
        let text = fixture("user_config_default_linux.json");
        let config = UserConfig::from_json(&text).expect("缺省金文件不是合法 JSON");

        assert_eq!(
            config.get_bool("GeneralDownload", "AutoSpeedUp"),
            Some(true)
        );
        assert_eq!(
            config.get_bool("Aria2Rpc", "EmulateFingerprint"),
            Some(false)
        );
        // 布尔项不是字符串。
        assert_eq!(config.get_str("Aria2Rpc", "EmulateFingerprint"), None);
        assert_eq!(
            config.get_py_str("Aria2Rpc", "EmulateFingerprint"),
            Some("False".to_string())
        );
        // 数字项：CurrentHeadersPreset 落盘是 0，不是 "0"。
        assert_eq!(config.get_i64("Network", "CurrentHeadersPreset"), Some(0));
        assert_eq!(config.get_str("Network", "CurrentHeadersPreset"), None);
        assert_eq!(
            config.get_py_str("Network", "CurrentHeadersPreset"),
            Some("0".to_string())
        );
        // DpiScale 落盘是整数 0，`get_f64` 照样读得出。
        assert_eq!(config.get_f64("Personalization", "DpiScale"), Some(0.0));
        assert_eq!(config.get_i64("Personalization", "DpiScale"), Some(0));
        // 字符串项。
        assert_eq!(
            config.get_str("GeneralDownload", "DownloadFolder"),
            Some("//Downloads")
        );
        assert_eq!(
            config.get_str("GeneralDownload", "NameConflict"),
            Some("keepBoth")
        );
        assert_eq!(config.get_str("Network", "ClientProfile"), Some("auto"));
        // 数组/对象 → `get_py_str` 无值（python `str()` 会给出 "[...]"，
        // 但没有调用方需要，不实现）。
        assert_eq!(
            config.get_py_str("GeneralDownload", "HistoryDownloadFolder"),
            None
        );
        // 缺键/缺组一律 None。
        assert_eq!(config.get_bool("Nope", "Nope"), None);
        assert_eq!(config.get_i64("GeneralDownload", "Nope"), None);
        assert_eq!(config.get_str("GeneralDownload", "Nope"), None);
    }

    /// 遗留档（手改过的文件、旧版 Rust 写出的字符串档）不该让读方拿到 None。
    /// 注意：`get_str` 对字符串一律原样返回，`"False"` 就是 `"False"`。
    #[test]
    fn legacy_string_values_are_tolerated() {
        let text = r#"{
            "GeneralDownload": {
                "isSpeedLimitEnabled": "False",
                "MaxTaskNum": "7",
                "SpeedLimitation": "0.5",
                "DownloadFolder": 3,
                "ProxyServer": "Auto"
            },
            "Personalization": {"DpiScale": "0.25"}
        }"#;
        let mut config = UserConfig::from_json(text).expect("遗留档不是合法 JSON");
        assert_eq!(
            config.get_bool("GeneralDownload", "isSpeedLimitEnabled"),
            Some(false)
        );
        // 小写形态也收。
        config.set_str("GeneralDownload", "isSpeedLimitEnabled", "false");
        assert_eq!(
            config.get_bool("GeneralDownload", "isSpeedLimitEnabled"),
            Some(false)
        );
        assert_eq!(config.get_i64("GeneralDownload", "MaxTaskNum"), Some(7));
        assert_eq!(
            config.get_f64("GeneralDownload", "SpeedLimitation"),
            Some(0.5)
        );
        assert_eq!(config.get_f64("Personalization", "DpiScale"), Some(0.25));
        // 反向：数字项被当字符串读不到（`get_str` 只认 `Value::String`）。
        assert_eq!(config.get_str("GeneralDownload", "DownloadFolder"), None);
        assert_eq!(
            config.get_py_str("GeneralDownload", "DownloadFolder"),
            Some("3".to_string())
        );
        // 布尔项的字符串形态不会被 `get_str` 拦下——这是已知形态歧义。
        assert_eq!(
            config.get_str("GeneralDownload", "isSpeedLimitEnabled"),
            Some("false")
        );
    }

    /// `py_json_dumps` 与 `write_indented` 逐字节对齐 python 的两种 dump 口径。
    #[test]
    fn py_json_writer_matches_python_dumps() {
        let cases = jsonl("py_json_writer.jsonl");
        assert_eq!(cases.len(), 20);
        for case in cases {
            let value = &case["value"];
            let dumps = case["dumps"].as_str().expect("dumps 不是字符串");
            let indent4 = case["indent4"].as_str().expect("indent4 不是字符串");
            assert_eq!(py_json_dumps(value), dumps, "紧凑口径不一致：{value}");
            let mut out = String::new();
            write_indented(value, 0, &mut out);
            assert_eq!(out, indent4, "indent=4 口径不一致：{value}");
        }
    }

    #[test]
    fn py_float_repr_matches_python() {
        let cases = jsonl("py_float_repr.jsonl");
        assert_eq!(cases.len(), 20);
        for case in cases {
            let value = case["value"].as_f64().expect("value 不是数字");
            let expected = case["repr"].as_str().expect("repr 不是字符串");
            assert_eq!(py_float_repr(value), expected, "repr({value})");
        }
    }

    #[test]
    fn py_json_string_matches_json_escape_fixture() {
        let cases = jsonl("json_escape.jsonl");
        assert_eq!(cases.len(), 28);
        for case in cases {
            let input = case["input"].as_str().expect("input 不是字符串");
            let expected = case["expected"].as_str().expect("expected 不是字符串");
            assert_eq!(py_json_string(input), expected, "转义不一致：{input:?}");
        }
    }

    #[test]
    fn save_load_round_trip() {
        let text = fixture("user_config_mutated_expected.json");
        let config = UserConfig::from_json(&text).expect("金文件不是合法 JSON");
        let path = std::env::temp_dir().join(format!(
            "gd3-user-config-{}-{:?}.json",
            std::process::id(),
            std::thread::current().id()
        ));
        config.save(&path).expect("写盘失败");
        let raw = std::fs::read_to_string(&path).expect("读回失败");
        assert_eq!(raw, text, "写盘要逐字节等于 python json.dump");
        let loaded = UserConfig::load(&path).expect("load 失败");
        assert_eq!(loaded, config);
        std::fs::remove_file(&path).expect("清理失败");
        // 文件不存在 → io::Error（调用方决定缺省），不是空配置。
        assert!(UserConfig::load(&path).is_err());
    }

    #[test]
    fn unknown_groups_and_non_object_values_are_skipped() {
        let config = UserConfig::from_json(
            r#"{"A": {"k": 1}, "B": 3, "C": [1], "D": null, "E": {"n": {"deep": true}}}"#,
        )
        .expect("合法 JSON");
        let groups: Vec<&str> = config.groups.keys().map(String::as_str).collect();
        assert_eq!(groups, ["A", "E"]);
        assert_eq!(config.entries("B"), None);
        assert_eq!(config.entries("C"), None);
        assert_eq!(config.entries("D"), None);
        assert_eq!(config.get("E", "n"), Some(&json!({"deep": true})));
        // 空配置写出 `{}`（python `json.dump({})` 无换行）。
        assert_eq!(UserConfig::new().to_json_text(), "{}");
    }
}
