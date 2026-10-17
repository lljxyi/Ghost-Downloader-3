//! UserConfig.json 存取（Phase 7c 设置页数据源下沉起步）。
//!
//! 磁盘协议不变（qfluentwidgets `Config.save` 格式）：
//! - 外形 `{group: {key: "serialized 字符串"}}`——每个项的值经
//!   validator `format()` 后全为字符串（`BoolValidator.format`=
//!   `str(True)` → `"True"`，`RangeValidator.format` → `str(value)`，
//!   `JsonConfigSerializer` → `json.dumps`）。
//! - 保存 = `json.dump(toDict(), f, ensure_ascii=False, indent=4)`，
//!   无尾随换行。本文件手写等价 writer（serde_json pretty 是 2 空格、
//!   `bool` 小写，都不兼容）。
//! - 键序不名誉：Python 端 toDict 走 `dir()` 字母序存初档、此后文件
//!   自身顺序往返——不用字段；
//!   本模块用 BTreeMap 稳定排序写盘（语义键集不变、Python 端可读回）。
//!
//! 分层：只做存取与值串镜像（python repr 口径），**不做配置项盘点**
//! ——设置页用到哪项、先验哪项，避免一次性几十项的双漂移浪费。

use std::collections::BTreeMap;
use std::path::Path;

/// group → (key → python 序列化字符串)。
pub type GroupMap = BTreeMap<String, BTreeMap<String, String>>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserConfig {
    pub groups: GroupMap,
}

impl UserConfig {
    pub fn new() -> Self {
        Self::default()
    }

    /// 从磁盘读（qfluentwidgets 格式；非字符串值容错丢弃——磁盘正常
    /// 全为字符串，异常形态不该让设置页崩）。
    pub fn load(path: &Path) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Self::from_json(&text).map_err(|error| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
        })
    }

    /// 从同态 JSON 文本解析。组层级非对象 / 值非字符串的一律跳过。
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        let value: serde_json::Value = serde_json::from_str(text)?;
        let mut groups = GroupMap::new();
        if let serde_json::Value::Object(map) = &value {
            for (group, items) in map {
                let serde_json::Value::Object(item_map) = items else {
                    continue;
                };
                let mut entries: BTreeMap<String, String> = BTreeMap::new();
                for (key, item) in item_map {
                    if let serde_json::Value::String(s) = item {
                        entries.insert(key.clone(), s.clone());
                    }
                }
                groups.insert(group.clone(), entries);
            }
        }
        Ok(Self { groups })
    }

    /// 写出 qfluentwidgets 同构 JSON：`json.dump(..., indent=4,
    /// ensure_ascii=False)`，无尾随换行。手写 writer 以逐字节镜像
    /// Python 输出（serde_json::to_string_pretty 缩进/布尔不符）。
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, self.to_json_text())
    }

    pub fn to_json_text(&self) -> String {
        let mut out = String::new();
        out.push('{');
        let group_count = self.groups.len();
        for (group_index, (group, entries)) in self.groups.iter().enumerate() {
            if group_index > 0 {
                out.push(',');
            }
            out.push('\n');
            out.push_str("    "); // indent 层 1（4 空格）
            out.push_str(&self::json_string(group));
            out.push_str(": {\n");
            let entry_count = entries.len();
            for (entry_index, (key, value)) in entries.iter().enumerate() {
                out.push_str("        "); // indent 层 2
                out.push_str(&self::json_string(key));
                out.push_str(": ");
                out.push_str(&self::json_string(value));
                if entry_index + 1 < entry_count {
                    out.push(',');
                }
                out.push('\n');
            }
            if entry_count > 0 {
                out.push_str("    "); // 闭合缩进回到层 1
            }
            out.push('}');
            let _ = group_index;
        }
        if group_count > 0 {
            out.push('\n');
        }
        out.push('}');
        out
    }

    fn entries(&self, group: &str) -> Option<&BTreeMap<String, String>> {
        self.groups.get(group)
    }

    pub fn get_str(&self, group: &str, key: &str) -> Option<&str> {
        self.entries(group)?.get(key).map(String::as_str)
    }

    /// python 口径字符串写盘（缺组自建）。
    pub fn set_str(&mut self, group: &str, key: &str, value: &str) {
        self.groups
            .entry(group.to_string())
            .or_default()
            .insert(key.to_string(), value.to_string());
    }

    /// BoolValidator.format = str(bool) → `"True"`/`"False"`（python
    /// repr，**不是** JSON 的 true/false）。
    pub fn get_bool(&self, group: &str, key: &str) -> Option<bool> {
        match self.get_str(group, key)? {
            "True" => Some(true),
            "False" => Some(false),
            _ => None,
        }
    }

    pub fn set_bool(&mut self, group: &str, key: &str, value: bool) {
        self.set_str(group, key, if value { "True" } else { "False" });
    }

    /// RangeValidator.format = str(int)；python 可能存 `"3.0"`（float
    /// 项），`"3"` 解析成功才返回值。
    pub fn get_i64(&self, group: &str, key: &str) -> Option<i64> {
        self.get_str(group, key)?.parse().ok()
    }

    pub fn set_i64(&mut self, group: &str, key: &str, value: i64) {
        self.set_str(group, key, &value.to_string());
    }

    /// python `str(float)` 口径读：磁盘串 parse f64（`"0.25"`/`"3"`/
    /// `"3.0"` 均可；非数字/缺键 None）。DpiScale 项（cfg.py:296，值域
    /// 0-5、step 0.25，Python 端 cfg.set(x/100) 写浮点）消费。
    pub fn get_f64(&self, group: &str, key: &str) -> Option<f64> {
        self.get_str(group, key)?.parse().ok()
    }

    /// python `str(float)` 口径写：shortest repr；**整值浮点补 `.0`**
    /// （python `str(2.0)` == `"2.0"`，Rust Display 只给 `"2"`）。本仓库
    /// 数值域（0-5、二分步 0.25）无科学计数形态，Rust Display 也不产
    /// 指数；NaN/Inf 不在项值域内（validator 侧守卫），原样落串。
    pub fn set_f64(&mut self, group: &str, key: &str, value: f64) {
        let mut text = format!("{}", value);
        if value.is_finite() && value.fract() == 0.0 && !text.contains('.') {
            text.push_str(".0");
        }
        self.set_str(group, key, &text);
    }
}

/// 手写 JSON 字符串转义，镜像 `json.dumps(..., ensure_ascii=False)`：
/// 控制字符 `\uXXXX`（小写十六进制）、`"`/`\` 反斜杠转义、换行 `\n`、
/// 制表 `\t`、回车 `\r`、非 ASCII 原样。
fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> UserConfig {
        let path = std::path::Path::new("tests/fixtures/user_config_sample.json");
        UserConfig::load(path).expect("sample 加载失败")
    }

    /// sample 解析：组/键齐、typed 取值对齐磁盘 python 串、缺键与非
    /// 数字串容错返回 None。
    #[test]
    fn load_sample_parses_groups_and_typed_values() {
        let config = sample();
        assert_eq!(config.groups.len(), 3);
        assert_eq!(config.get_bool("Category", "EnableCategory"), Some(true));
        assert_eq!(
            config.get_bool("GeneralDownload", "isSpeedLimitEnabled"),
            Some(false)
        );
        assert_eq!(config.get_i64("GeneralDownload", "MaxTaskNum"), Some(3));
        assert_eq!(config.get_str("Category", "KeyboardCategory"), Some("视频"));
        assert_eq!(config.get_bool("GeneralDownload", "MaxTaskNum"), None);
        assert_eq!(config.get_bool("GeneralDownload", "NoSuchKey"), None);
        assert_eq!(config.entries("NoSuchGroup"), None);
    }

    /// typed 写 + 手写 writer：写盘文本与 `.venv` Python
    /// `json.dumps(..., indent=4, ensure_ascii=False)` 生成的 fixture
    /// 逐字节一致；再读回 typed 断言（round-trip）。
    #[test]
    fn typed_set_and_save_matches_python_dump() {
        let mut config = sample();
        config.set_bool("Category", "EnableCategory", false);
        config.set_str("Category", "KeyboardCategory", "音频");
        config.set_i64("GeneralDownload", "MaxTaskNum", 5);
        config.set_str("Network", "ClientProfile", "manual");

        let expected = std::fs::read_to_string(std::path::Path::new(
            "tests/fixtures/user_config_expected_save.json",
        ))
        .expect("expected fixture 缺失");
        assert_eq!(config.to_json_text(), expected, "写盘文本须逐字节镜像");
        // 两次生成稳定（幂等）。
        assert_eq!(config.to_json_text(), config.to_json_text());

        let path = std::env::temp_dir().join("gd3_user_config_save_test.json");
        config.save(&path).expect("save 失败");
        let reloaded = UserConfig::load(&path).expect("reload 失败");
        assert_eq!(reloaded.get_bool("Category", "EnableCategory"), Some(false));
        assert_eq!(
            reloaded.get_str("Category", "KeyboardCategory"),
            Some("音频")
        );
        assert_eq!(reloaded.get_i64("GeneralDownload", "MaxTaskNum"), Some(5));
        // 未触及键原样保留。
        assert_eq!(
            reloaded.get_str("GeneralDownload", "DownloadFolder"),
            Some("C:/downloads")
        );
        std::fs::remove_file(&path).ok();
    }

    /// python str(float) 口径：整值浮点补 `.0`、小数原样 shortest、
    /// 读回 round-trip；非数字串 get_f64 容错 None。
    #[test]
    fn f64_python_repr_round_trip() {
        let mut config = UserConfig::new();
        config.set_f64("Personalization", "DpiScale", 0.25);
        assert_eq!(config.get_str("Personalization", "DpiScale"), Some("0.25"));
        config.set_f64("Personalization", "DpiScale", 2.0);
        assert_eq!(config.get_str("Personalization", "DpiScale"), Some("2.0"));
        config.set_f64("Personalization", "DpiScale", 5.0);
        assert_eq!(config.get_str("Personalization", "DpiScale"), Some("5.0"));
        config.set_f64("Personalization", "DpiScale", 1.75);
        assert_eq!(config.get_str("Personalization", "DpiScale"), Some("1.75"));
        assert_eq!(config.get_f64("Personalization", "DpiScale"), Some(1.75));
        // 缺键/非数字容错。
        assert_eq!(config.get_f64("Personalization", "NoSuchKey"), None);
        config.set_str("Personalization", "DpiScale", "weird");
        assert_eq!(config.get_f64("Personalization", "DpiScale"), None);
    }
}
