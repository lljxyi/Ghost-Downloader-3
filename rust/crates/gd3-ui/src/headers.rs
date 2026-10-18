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

use crate::pyshlex;
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

#[derive(Clone, Debug, PartialEq, Eq)]
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

/// 第 46 步：编辑覆盖层的标头文本 → (名, 值) 列表（只认「名: 值」行，
/// 逐行 `parse_header_line`；cURL 块解析见 `parse_headers`）。保留空值与
/// 重名，落盘前的归一化在 `headers_map`。
pub fn parse_header_pairs(text: &str) -> Vec<(String, String)> {
    to_logical_lines(text)
        .iter()
        .filter_map(|line| parse_header_line(line))
        .collect()
}

/// `parseCurl` 认的标头旗标（headers_editor.py:20）。
const CURL_HEADER_FLAGS: [&str; 2] = ["-H", "--header"];

/// Windows「复制全部为 cURL」把多条命令用这些符号连在同一行
/// （headers_editor.py:21）——见到就停，只认第一条命令。
fn is_command_separator(token: &str) -> bool {
    matches!(token, ";" | "&" | "&&" | "|" | "||")
}

/// `-b`/`-A`/`-e` 这类「值就是一个标头」的旗标 → 标头名
/// （headers_editor.py:22-26）。
fn curl_value_flag(token: &str) -> Option<&'static str> {
    match token {
        "-b" | "--cookie" => Some("cookie"),
        "-A" | "--user-agent" => Some("user-agent"),
        "-e" | "--referer" => Some("referer"),
        _ => None,
    }
}

/// 镜像 `parseCurl`（headers_editor.py:43-64）：
/// `shlex.split(line.replace("$'", "'"))`——分词失败（引号/转义未闭合）
/// 返回空列表；从第 2 个 token 起扫，`-H`/`--header` 取下一个 token 走
/// `parse_header_line`，`-b`/`-A`/`-e` 取下一个 token 作值（`strip`），
/// 其余 token 只是跳过（不消费值）。Python 的 `while index < len(tokens) - 1`
/// 让最后一个 token 永远只当值、不当旗标——照抄。
pub fn parse_curl(line: &str) -> Vec<(String, String)> {
    let normalized = line.replace("$'", "'");
    let Some(tokens) = pyshlex::split(&normalized) else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    let mut index = 1usize;
    while index + 1 < tokens.len() {
        let token = tokens[index].as_str();
        if is_command_separator(token) {
            break;
        }
        if CURL_HEADER_FLAGS.contains(&token) {
            if let Some(row) = parse_header_line(&tokens[index + 1]) {
                rows.push(row);
            }
        } else if let Some(name) = curl_value_flag(token) {
            rows.push((name.to_string(), py_strip(&tokens[index + 1]).to_string()));
        } else {
            index += 1;
            continue;
        }
        index += 2;
    }
    rows
}

/// 镜像 `line[:5].lower() == "curl "`（headers_editor.py:69）。只有 ASCII
/// 字符能小写成 `curl ` 里的字母，所以逐字符 ASCII 小写比较等价。
fn starts_with_curl_command(line: &str) -> bool {
    let mut chars = line.chars();
    for expected in ['c', 'u', 'r', 'l', ' '] {
        match chars.next() {
            Some(c) if c.to_ascii_lowercase() == expected => {}
            _ => return false,
        }
    }
    true
}

/// 镜像 `parseHeaders`（headers_editor.py:67-82）：逐逻辑行分派而非整段
/// 二选一（否则 cURL 后面手加的裸行会被静默吞掉）。`curl ` 开头的行走
/// `parse_curl` 且**只认第一条**——一份标头属于一个请求，「复制全部为
/// cURL」混多条产出的标头集不对应任何真实请求；其余行走
/// `parse_header_line`。
pub fn parse_headers(text: &str) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    let mut has_curl = false;
    for line in to_logical_lines(text) {
        if starts_with_curl_command(&line) {
            if has_curl {
                continue;
            }
            has_curl = true;
            rows.extend(parse_curl(&line));
            continue;
        }
        if let Some(row) = parse_header_line(&line) {
            rows.push(row);
        }
    }
    rows
}

/// 镜像 `toHeaderRows`（headers_editor.py:93-101）：字面切分、零丢弃——
/// 表格/文本视图切换必须无损，清洗只发生在粘贴时。与 `parse_header_line`
/// 的差别：不校验名称（含空白、空名都留着），无冒号的行留 `(行, "")`。
pub fn to_header_rows(text: &str) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    for line in split_lines(text) {
        if py_strip(&line).is_empty() {
            continue;
        }
        match line.find(':') {
            Some(idx) => rows.push((
                py_strip(&line[..idx]).to_string(),
                py_strip(&line[idx + 1..]).to_string(),
            )),
            None => rows.push((py_strip(&line).to_string(), String::new())),
        }
    }
    rows
}

/// 镜像 `HeadersEditor.headers()`（headers_editor.py:290-297）：名称
/// `strip().lower()`、值 `strip()`，键或值为空的行丢弃，同名后者覆盖前者
/// （键序取首次出现的位置，与 Python dict 一致）。这是预设真正落盘的形状。
pub fn headers_map(rows: &[(String, String)]) -> Vec<(String, String)> {
    let mut result: Vec<(String, String)> = Vec::new();
    for (name, value) in rows {
        let key = py_strip(name).to_lowercase();
        let value = py_strip(value);
        if key.is_empty() || value.is_empty() {
            continue;
        }
        match result.iter_mut().find(|(seen, _)| *seen == key) {
            Some(slot) => slot.1 = value.to_string(),
            None => result.push((key, value.to_string())),
        }
    }
    result
}

/// 镜像 `_refreshDuplicates`（headers_editor.py:377-383）：按行序标记该行
/// 名称是否**已经出现过**（即「重名行」），判定用 `name.strip().lower()`，
/// 空名不进 seen、也不会被标重。
pub fn duplicate_flags(rows: &[(String, String)]) -> Vec<bool> {
    let mut seen: Vec<String> = Vec::new();
    let mut flags = Vec::with_capacity(rows.len());
    for (name, _) in rows {
        let key = py_strip(name).to_lowercase();
        flags.push(!key.is_empty() && seen.contains(&key));
        if !key.is_empty() {
            seen.push(key);
        }
    }
    flags
}

/// 覆盖层汇总警告用：重名集合（按首次被重复的顺序、去重）。Slint 的
/// `TextEdit` 无法逐行染色，重复行高亮降级成一行汇总文字。
pub fn duplicate_names(rows: &[(String, String)]) -> Vec<String> {
    let flags = duplicate_flags(rows);
    let mut names: Vec<String> = Vec::new();
    for ((name, _), is_duplicate) in rows.iter().zip(&flags) {
        if !is_duplicate {
            continue;
        }
        let key = py_strip(name).to_lowercase();
        if !names.contains(&key) {
            names.push(key);
        }
    }
    names
}

/// 覆盖层汇总警告文案（Slint 唯一能给的重名提示）：Python 是逐行
/// `row.setDuplicate` 红框，`TextEdit` 无法逐行染色，降级成一行汇总。
/// 空串=无重复（与「无警告」同一判定）。
pub fn duplicate_warning(text: &str) -> String {
    let names = duplicate_names(&parse_headers(text));
    if names.is_empty() {
        String::new()
    } else {
        format!("重复标头：{}（后一条覆盖前一条）", names.join(", "))
    }
}

/// 镜像 HeadersPresetEditDialog.preset()（headers_preset_edit.py:58-62）：
/// 名称 strip 空兜「未命名预设」；标头走 `HeadersEditor.headers()` 口径
/// （`headers_map`：名称小写、空名/空值行丢弃、同名后者覆盖）。空串在
/// 覆盖层校验拒绝（不回发）；空白串放行由本层兜底（Python
/// `text().strip()` 为空同样兜未命名预设——同口径）。
///
/// 输入文本先过 `parse_headers`（cURL 感知）：Python 在**粘贴时**就把
/// cURL 归一化成「名: 值」行（`HeadersTextEdit.insertFromMimeData`），
/// Slint 的 `TextEdit` 拦不到粘贴，只能在确定时做等价归一化。纯手输
/// 标头行的结果与 Python 文本视图（`toHeaderRows` + `headers()`）一致；
/// 唯一已知差异是名称含空白的行——Python 文本视图会保留、表格视图被
/// `HeaderNameValidator` 拒绝，这里取更安全的一侧丢弃。
pub fn draft_to_header_preset(name: &str, pairs: &str) -> HeaderPreset {
    let name = py_strip(name);
    HeaderPreset {
        name: if name.is_empty() {
            "未命名预设".to_string()
        } else {
            name.to_string()
        },
        headers: headers_map(&parse_headers(pairs)),
    }
}

/// 镜像 `toHeadersText`（headers_editor.py:85-86）：值非空写「名: 值」，
/// 值空只写名；行间 `\n`、**末尾不带换行**。与 `to_header_rows` 互逆
/// （空值行走单名回来是 `(名, "")`）；与 `parse_header_line` 不互逆——
/// 单名行没有冒号会被它丢弃，写盘路径用 `headers_map` 滤掉空值行，
/// 所以不会触发。
pub fn headers_to_text(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(name, value)| {
            if value.is_empty() {
                name.clone()
            } else {
                format!("{name}: {value}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
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
        // 第 48 步：标头走 `headers()` 口径——名称小写、空值行丢弃、
        // 同名后者覆盖（键序仍是首次出现的位置）。
        let normalized = draft_to_header_preset("B", "X-A: 1\r\nx-a: 2\nempty:\nb: 3");
        assert_eq!(
            normalized.headers,
            vec![
                ("x-a".to_string(), "2".to_string()),
                ("b".to_string(), "3".to_string()),
            ]
        );
        // cURL 文本在确定时归一化（Python 是在粘贴时做的，Slint 拦不到）。
        let curl = draft_to_header_preset(
            "C",
            "curl 'https://example.com/f.zip' -H 'accept: text/html' -A 'UA/1'",
        );
        assert_eq!(
            curl.headers,
            vec![
                ("accept".to_string(), "text/html".to_string()),
                ("user-agent".to_string(), "UA/1".to_string()),
            ]
        );
    }

    #[test]
    fn headers_to_text_mirrors_python_and_round_trips_through_rows() {
        let pairs = vec![
            ("upgrade-insecure-requests".to_string(), "1".to_string()),
            ("sec-fetch-dest".to_string(), "document".to_string()),
        ];
        // 末尾不带换行（Python `"\n".join(...)`）。
        assert_eq!(
            headers_to_text(&pairs),
            "upgrade-insecure-requests: 1\nsec-fetch-dest: document"
        );
        assert_eq!(parse_header_pairs(&headers_to_text(&pairs)), pairs);
        // 空值只写名；经 `to_header_rows` 回来仍是 `(名, "")`（无损视图），
        // 经 `parse_header_line` 则丢行——写盘路径用 `headers_map` 兜。
        let with_empty = vec![("c".to_string(), String::new())];
        assert_eq!(headers_to_text(&with_empty), "c");
        assert_eq!(
            to_header_rows(&headers_to_text(&with_empty)),
            vec![("c".to_string(), String::new())]
        );
        assert!(parse_header_pairs(&headers_to_text(&with_empty)).is_empty());
    }

    #[test]
    fn duplicate_warning_lists_repeated_names_in_first_repeat_order() {
        // 无重复 → 空串（Slint 侧 `!= ""` 即显隐判定）。
        assert_eq!(duplicate_warning("x-a: 1\nx-b: 2"), "");
        // 重名按「首次被重复」的顺序去重（大小写/首尾空白归一后比较）。
        assert_eq!(
            duplicate_warning("X-A: 1\nx-a: 2\nb: 3\nB: 4\nx-a: 5"),
            "重复标头：x-a, b（后一条覆盖前一条）"
        );
        // cURL 文本同样先过 parse_headers：`-H` 两条同名也算重复。
        assert_eq!(
            duplicate_warning("curl 'https://e.com' -H 'a: 1' -H 'A: 2'"),
            "重复标头：a（后一条覆盖前一条）"
        );
        // 空名不进 seen、也不会被标重（镜像 `if name: seen.add(name)`）。
        assert_eq!(duplicate_warning(": 1\n: 2"), "");
    }

    fn fixture_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/headers_parse.jsonl")
    }

    fn json_pairs(value: &serde_json::Value) -> Vec<(String, String)> {
        value
            .as_array()
            .expect("pairs")
            .iter()
            .map(|item| {
                let pair = item.as_array().expect("pair");
                (
                    pair[0].as_str().expect("name").to_string(),
                    pair[1].as_str().expect("value").to_string(),
                )
            })
            .collect()
    }

    /// 第 48 步：解析层逐条对比 Python 金文件
    /// （`tests/fixtures/generate_headers_fixture.py` 生成，不要手改）。
    #[test]
    fn parse_layer_matches_python_fixture() {
        let path = fixture_path();
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("读金文件失败 {}：{error}", path.display()));
        let mut counts = std::collections::BTreeMap::<String, usize>::new();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let case: serde_json::Value =
                serde_json::from_str(line).expect("金文件行不是合法 JSON");
            let kind = case["kind"].as_str().expect("kind");
            *counts.entry(kind.to_string()).or_insert(0) += 1;
            match kind {
                "split" => {
                    let input = case["input"].as_str().expect("input");
                    let want = case["tokens"].as_array().map(|items| {
                        items
                            .iter()
                            .map(|token| token.as_str().expect("token").to_string())
                            .collect::<Vec<_>>()
                    });
                    assert_eq!(
                        pyshlex::split(input),
                        want,
                        "shlex.split 与 Python 不一致：{input:?}"
                    );
                }
                "curl" => {
                    let input = case["input"].as_str().expect("input");
                    let normalized = input.replace("$'", "'");
                    assert_eq!(
                        case["normalized"].as_str(),
                        Some(normalized.as_str()),
                        "金文件的 normalized 与 `$'` 替换不一致：{input:?}"
                    );
                    assert_eq!(
                        parse_curl(input),
                        json_pairs(&case["rows"]),
                        "parseCurl 与 Python 不一致：{input:?}"
                    );
                }
                "headers" => {
                    let input = case["input"].as_str().expect("input");
                    let want_lines = case["lines"]
                        .as_array()
                        .expect("lines")
                        .iter()
                        .map(|line| line.as_str().expect("line").to_string())
                        .collect::<Vec<_>>();
                    assert_eq!(
                        to_logical_lines(input),
                        want_lines,
                        "toLogicalLines 与 Python 不一致：{input:?}"
                    );
                    assert_eq!(
                        parse_headers(input),
                        json_pairs(&case["rows"]),
                        "parseHeaders 与 Python 不一致：{input:?}"
                    );
                }
                "rows" => {
                    let input = case["input"].as_str().expect("input");
                    assert_eq!(
                        to_header_rows(input),
                        json_pairs(&case["rows"]),
                        "toHeaderRows 与 Python 不一致：{input:?}"
                    );
                }
                "text" => {
                    let pairs = json_pairs(&case["pairs"]);
                    assert_eq!(
                        headers_to_text(&pairs),
                        case["text"].as_str().expect("text"),
                        "toHeadersText 与 Python 不一致：{pairs:?}"
                    );
                }
                "map" => {
                    let pairs = json_pairs(&case["pairs"]);
                    assert_eq!(
                        headers_map(&pairs),
                        json_pairs(&case["rows"]),
                        "headers() 与 Python 不一致：{pairs:?}"
                    );
                }
                "dup" => {
                    let pairs = json_pairs(&case["pairs"]);
                    let want = case["flags"]
                        .as_array()
                        .expect("flags")
                        .iter()
                        .map(|flag| flag.as_bool().expect("flag"))
                        .collect::<Vec<_>>();
                    assert_eq!(
                        duplicate_flags(&pairs),
                        want,
                        "_refreshDuplicates 与 Python 不一致：{pairs:?}"
                    );
                }
                "header_line" => {
                    let input = case["input"].as_str().expect("input");
                    let want = if case["row"].is_null() {
                        None
                    } else {
                        Some(json_pairs(&serde_json::json!([case["row"].clone()])).remove(0))
                    };
                    assert_eq!(
                        parse_header_line(input),
                        want,
                        "parseHeaderLine 与 Python 不一致：{input:?}"
                    );
                }
                other => panic!("未知的金文件 kind：{other}"),
            }
        }
        // 语料下限：防止金文件被误清空导致「全绿但什么都没测」。
        for (kind, floor) in [
            ("split", 50),
            ("curl", 40),
            ("headers", 25),
            ("rows", 15),
            ("text", 10),
            ("map", 14),
            ("dup", 10),
            ("header_line", 13),
        ] {
            let got = counts.get(kind).copied().unwrap_or(0);
            assert!(
                got >= floor,
                "金文件 {kind} 用例只有 {got} 条（下限 {floor}）"
            );
        }
    }
}
