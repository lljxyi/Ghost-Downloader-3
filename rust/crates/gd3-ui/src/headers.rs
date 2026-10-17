//! 请求标头预设镜像层（cfg.py Network/HeadersPresets +
//! Network/CurrentHeadersPreset；第 42 步）。
//!
//! 实证：
//! - `BASE_HEADERS`（cfg.py:29-38）有序 8 对字段；
//! - `HeadersPresetListValidator.correct`（cfg.py:208-221）：逐项校验
//!   {name: str, headers: dict<str,str>}，合法项过滤，无合法项 →
//!   `[{"name": "默认", "headers": BASE_HEADERS}]`；
//! - 落盘为 `json.dumps(list, ensure_ascii=False)`（JsonConfigSerializer
//!   cfg.py:151-165，expected=list 失败 fallback `[]`，再被 correct 兜底）；
//! - `cfg.currentHeadersPreset`（Range 0-99 缺省 0）落盘是 python str
//!   串；`currentHeadersPresetIndex()` = min(value, len-1)（cfg.py:347），
//!   这里双端夹取属容错（负值/越界不复刻崩溃面）。
//!
//! 预设的添加/编辑对话框（HeadersPresetEditDialog + HeadersEditor）
//! 与当前标头读取（currentHeaders）留欠账步。

/// cfg.py:29-38 逐字镜像（键序保持 Python dict 插入序）。
pub const BASE_HEADERS: [(&str, &str); 8] = [
    ("accept-encoding", "deflate, br, gzip"),
    ("accept-language", "zh-CN,zh;q=0.9"),
    ("cookie", "down_ip=1"),
    ("sec-fetch-dest", "document"),
    ("sec-fetch-mode", "navigate"),
    ("sec-fetch-site", "none"),
    ("sec-fetch-user", "?1"),
    ("upgrade-insecure-requests", "1"),
];

pub const DEFAULT_PRESET_NAME: &str = "默认";

pub struct HeaderPreset {
    pub name: String,
    pub headers: Vec<(String, String)>,
}

fn default_presets() -> Vec<HeaderPreset> {
    vec![HeaderPreset {
        name: DEFAULT_PRESET_NAME.into(),
        headers: BASE_HEADERS
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    }]
}

/// 镜像 JsonConfigSerializer.deserialize + HeadersPresetListValidator.correct：
/// 解析 JSON 列表 → 逐项过滤合法预设（name 为 str、headers 为全字符串
/// 值的 dict）；无合法项 → `默认` + BASE_HEADERS 兜底（对齐 correct
/// 的 `presets or [默认]`，不复刻 Python 崩溃面）。
pub fn parse_presets(raw: &str) -> Vec<HeaderPreset> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return default_presets();
    };
    let Some(items) = value.as_array() else {
        return default_presets();
    };
    let mut presets = Vec::new();
    for item in items {
        let Some(name) = item.get("name").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Some(headers_map) = item.get("headers").and_then(serde_json::Value::as_object) else {
            continue;
        };
        if !headers_map.values().all(serde_json::Value::is_string) {
            continue;
        }
        let headers = headers_map
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
            .collect();
        presets.push(HeaderPreset {
            name: name.into(),
            headers,
        });
    }
    if presets.is_empty() {
        default_presets()
    } else {
        presets
    }
}

/// 镜像 `currentHeadersPresetIndex()`（cfg.py:347）的 min 表达式；负值、
/// 非整串按 0 夹取（容错，对齐 RangeValidator 不可产出负值的口径）。
pub fn current_index(raw: Option<&str>, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let value = raw.and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
    value.clamp(0, len as i64 - 1) as usize
}

/// 仅测试用的 (k, v) 转换小助手。
#[cfg(test)]
trait ToPair {
    fn to_pair(&self) -> (String, String);
}

#[cfg(test)]
impl ToPair for (&str, &str) {
    fn to_pair(&self) -> (String, String) {
        (self.0.to_string(), self.1.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_preserves_valid_presets_in_order() {
        let raw = r#"[
            {"name": "A", "headers": {"x-a": "1"}},
            {"name": 7, "headers": {}},
            {"name": "B", "headers": {"y-b": "2", "z-c": "3"}},
            {"name": "C", "headers": {"not-str": 4}}
        ]"#;
        let presets = parse_presets(raw);
        assert_eq!(presets.len(), 2, "非法项应整条过滤");
        assert_eq!(presets[0].name, "A");
        assert_eq!(presets[0].headers, vec![("x-a".into(), "1".into())]);
        assert_eq!(presets[1].name, "B");
        assert_eq!(presets[1].headers.len(), 2);
    }

    #[test]
    fn empty_or_invalid_fallbacks_to_default_preset() {
        for raw in ["", "not json", "{", "[]", "{}", "[\"x\"]"] {
            let presets = parse_presets(raw);
            assert_eq!(presets.len(), 1, "输入：{raw}");
            assert_eq!(presets[0].name, DEFAULT_PRESET_NAME);
            assert_eq!(presets[0].headers.len(), 8);
            assert_eq!(presets[0].headers[0], BASE_HEADERS[0].to_pair());
            assert_eq!(presets[0].headers[7], BASE_HEADERS[7].to_pair());
        }
    }

    #[test]
    fn current_index_mirrors_min_and_clamps_faults() {
        assert_eq!(current_index(None, 3), 0);
        assert_eq!(current_index(Some("1"), 3), 1);
        assert_eq!(current_index(Some("abc"), 3), 0);
        assert_eq!(current_index(Some("-3"), 3), 0);
        assert_eq!(current_index(Some("99"), 1), 0);
        assert_eq!(current_index(Some("2"), 3), 2);
        // 空预设列表不会出现（correct 兜底），夹取防越界。
        assert_eq!(current_index(Some("5"), 0), 0);
    }
}
