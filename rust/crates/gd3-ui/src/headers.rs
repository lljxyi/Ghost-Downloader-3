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
//! 第 46 步已镜像添加/编辑对话框（HeadersPresetEditDialog）的预设
//! 归一化；HeadersEditor 的 cURL 粘贴解析（parseCurl）与重复行高亮
//! 留欠账步。

use crate::pystr::{py_isspace, py_strip, split_lines};

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

#[derive(Clone)]
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

/// 镜像 HeadersEditor.parseHeaderLine（headers_editor.py:33-40）：
/// partition(":")——分隔符缺失、名称 strip 后为空或含空白字符的行
/// 丢弃（HTTP/2 伪头/请求行不是标头——原样发出会破坏请求）；值
/// strip（空值合法）。
fn parse_header_line(line: &str) -> Option<(String, String)> {
    let idx = line.find(':')?;
    let name = py_strip(&line[..idx]).to_string();
    if name.is_empty() || name.chars().any(py_isspace) {
        return None;
    }
    Some((name, py_strip(&line[idx + 1..]).to_string()))
}

/// 镜像 HeadersEditor.toLogicalLines（headers_editor.py:28-31）：CRLF
/// 归一化、行尾续行的反斜杠/「^」接空格，逐行 strip 去空。
fn to_logical_lines(text: &str) -> Vec<String> {
    let merged = text
        .replace("\r\n", "\n")
        .replace("\\\n", " ")
        .replace("^\n", " ");
    split_lines(&merged)
        .iter()
        .map(|line| py_strip(line).to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

/// 第 46 步：编辑覆盖层的标头文本 → (名, 值) 列表（HeadersEditor 行
/// 部件的简版镜像——逐行 parseHeaderLine；cURL 块解析与重复行高亮
/// 留欠账步）。
pub fn parse_header_pairs(text: &str) -> Vec<(String, String)> {
    to_logical_lines(text)
        .iter()
        .filter_map(|line| parse_header_line(line))
        .collect()
}

/// 镜像 HeadersPresetEditDialog.preset()（headers_preset_edit.py:58-62）：
/// 名称 strip 空兜「未命名预设」；标头成对列表原样。空串在覆盖层
/// 校验拒绝（不回发）；空白串放行由本层兜底（Python text().strip()
/// 为空同样兜未命名预设——同口径）。
pub fn draft_to_header_preset(name: &str, pairs: &str) -> HeaderPreset {
    let name = py_strip(name);
    HeaderPreset {
        name: if name.is_empty() {
            "未命名预设".to_string()
        } else {
            name.to_string()
        },
        headers: parse_header_pairs(pairs),
    }
}

/// 编辑覆盖层回显文本（「名: 值」逐行）——HeadersEditor 行部件的
/// 简版镜像；与 parse_header_pairs 互逆。
pub fn headers_to_text(pairs: &[(String, String)]) -> String {
    let mut text = String::new();
    for (name, value) in pairs {
        text.push_str(name);
        text.push_str(": ");
        text.push_str(value);
        text.push('\n');
    }
    text
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
    #[test]
    fn parse_header_pairs_mirrors_editor_rules() {
        let text = "accept-encoding: deflate, br, gzip\r\nx-a: 1\n\n  \nc:\nno-colon\nb c: 5";
        let pairs = parse_header_pairs(text);
        // 合法行保留（含空值/值含逗号分号）；无冒号、名称含空白
        // （「b c: 5」如请求行）丢弃——与 parseHeaderLine 一致。
        assert_eq!(
            pairs,
            vec![
                (
                    "accept-encoding".to_string(),
                    "deflate, br, gzip".to_string()
                ),
                ("x-a".to_string(), "1".to_string()),
                ("c".to_string(), String::new()),
            ]
        );
        // 续行镜像：反斜杠行尾接空格（「^」同规则——不重复测）。
        let merged = parse_header_pairs("user-agent: A\\\nx-b: 2");
        assert_eq!(
            merged,
            vec![("user-agent".to_string(), "A x-b: 2".to_string())]
        );
    }

    #[test]
    fn to_logical_lines_mirrors_cpython_splitlines_and_strip() {
        // 期望值由 .venv CPython 3.13 的
        // headers_editor.toLogicalLines 逐条实测得出。
        for (input, want) in [
            ("a: 1\rb: 2", vec!["a: 1", "b: 2"]),
            ("a: 1\u{b}b: 2", vec!["a: 1", "b: 2"]),
            ("a: 1\u{c}b: 2", vec!["a: 1", "b: 2"]),
            ("a: 1\u{1c}b: 2", vec!["a: 1", "b: 2"]),
            ("a: 1\u{2028}b: 2", vec!["a: 1", "b: 2"]),
            ("a: 1\u{85}b: 2", vec!["a: 1", "b: 2"]),
            ("a: 1\r\nb: 2", vec!["a: 1", "b: 2"]),
            ("x: 1\\\ny: 2", vec!["x: 1 y: 2"]),
            ("x: 1^\ny: 2", vec!["x: 1 y: 2"]),
            ("  \n\t\n", Vec::<&str>::new()),
        ] {
            assert_eq!(to_logical_lines(input), want, "input {input:?}");
        }
        // 行内 \x1f 不是行界、但被 strip 掉；名含空白/空名丢行。
        let lines = to_logical_lines("a b: 1\nc: 2\n: x\n d :  e \n\u{1f}");
        assert_eq!(lines, vec!["a b: 1", "c: 2", ": x", "d :  e"]);
        assert_eq!(
            lines
                .iter()
                .map(|line| parse_header_line(line))
                .collect::<Vec<_>>(),
            vec![
                None,
                Some(("c".to_string(), "2".to_string())),
                None,
                Some(("d".to_string(), "e".to_string())),
            ]
        );
    }

    #[test]
    fn draft_to_header_preset_mirrors_dialog_preset() {
        let preset = draft_to_header_preset("  A  ", "x-a: 1");
        assert_eq!(preset.name, "A");
        assert_eq!(preset.headers, vec![("x-a".to_string(), "1".to_string())]);
        // 空白串兜「未命名预设」（空串在校验层拒绝、不回发）。
        let blank = draft_to_header_preset(" ", "");
        assert_eq!(blank.name, "未命名预设");
        assert!(blank.headers.is_empty());
    }

    #[test]
    fn headers_to_text_round_trips_with_parse() {
        let pairs = vec![
            ("upgrade-insecure-requests".to_string(), "1".to_string()),
            ("sec-fetch-dest".to_string(), "document".to_string()),
        ];
        assert_eq!(parse_header_pairs(&headers_to_text(&pairs)), pairs);
    }
}
