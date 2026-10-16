//! ed2k pack 的链接解析层（镜像 `features/ed2k_pack/kelpie/models.py::Link.parse`）。
//!
//! 纯文本解析：输入一段 URL 文本，输出 `(name, size, hash)` 或
//! `INVALID_LINK` 错误（消息逐字抄 Python；Python 侧 `Error.__init__` 会再拼成
//! `"INVALID_LINK: <message>"`，桥在异常参数里给出原样 message）。
//!
//! 镜像到的标准库语义（都在本模块内实现并配单测）：
//! - `str.strip()`：Unicode White_Space **加** `\x1c`–`\x1f`（CPython 特有
//!   的四个文件分隔符）；
//! - `urllib.parse.unquote`（默认 `encoding='utf-8', errors='replace'`）：
//!   按**字节**扫 `%XX`，两个 ASCII 十六进制位才是转义；解码时每个无法并入
//!   完整 UTF-8 序列的字节产生一个 U+FFFD（逐字节跳过，见
//!   `decode_utf8_replacing_per_byte` 的单测）；
//! - `str.lower()`：Unicode 简单小写映射（Rust `to_lowercase` 同源）。

use serde_json::Value;

/// Python 侧 `models.MAX_SIZE`：eMule 的 `MAX_EMULE_FILE_SIZE`。
pub const MAX_SIZE: u64 = 256 << 30;

/// Python 侧 `models.Link` 的有用字段（`Session` / 卡片要从 Link 取的三个）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub name: String,
    pub size: u64,
    pub hash: String,
}

/// `Error(ErrorCode.INVALID_LINK, message)` 的等价物。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidLink {
    pub message: String,
}

/// `Link.parse(text)`。
pub fn parse(text: &str) -> Result<Link, InvalidLink> {
    let text = python_strip(text);
    let error = |message: &str| InvalidLink {
        message: message.to_string().replace("{text}", text),
    };
    if text.get(..7).map(python_lower) != Some("ed2k://".to_string()) {
        return Err(error("not an eD2k link: {text}"));
    }
    let mut body = text[7..].to_string();
    // Browsers may encode every separator, as aMule's TextClient.cpp:537-540
    // repairs. Only such a link is decoded: in a plain link %7C is part of
    // the name.
    if body.get(..3).map(python_lower).as_deref() == Some("%7c") {
        body = body.replace("%7C", "|").replace("%7c", "|");
    }
    let fields: Vec<&str> = body.split('|').collect();
    if fields.len() < 5 || !fields[0].is_empty() || python_lower(fields[1]) != "file" {
        return Err(error("not an eD2k file link: {text}"));
    }
    let name = python_unquote(fields[2]);
    if name.is_empty() {
        return Err(error("link has no name: {text}"));
    }
    // `[0-9]+` fullmatch；Python 的 int 是任意精度，所以先去前导零按位数封顶
    // （MAX_SIZE 是 12 位数，位数更多必然超界），短值再让 u128 承接。
    // 这些路径的报错消息在 Python 侧完全相同，判序不影响行为。
    let size_field = fields[3];
    if !is_ascii_digits(size_field) {
        return Err(error("link has an invalid size: {text}"));
    }
    let digits = size_field.trim_start_matches('0');
    if digits.len() > 12 {
        return Err(error("link has an invalid size: {text}"));
    }
    let size = size_field.parse::<u128>().expect("有效位数必能放下");
    if size == 0 || size > u128::from(MAX_SIZE) {
        return Err(error("link has an invalid size: {text}"));
    }
    if !is_ed2k_hash(fields[4]) {
        return Err(error("link has an invalid hash: {text}"));
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "size <= MAX_SIZE < u64::MAX"
    )]
    Ok(Link {
        name,
        size: size as u64,
        hash: fields[4].to_uppercase(),
    })
}

/// `str(text[:7]).lower()`。
fn python_lower(text: &str) -> String {
    text.to_lowercase()
}

/// `str.strip()`：Unicode White_Space + `\x1c`–`\x1f`（CPython 特有）。
fn python_strip(text: &str) -> &str {
    text.trim_matches(is_python_space)
}

fn is_python_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

fn is_ascii_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// `[0-9A-Fa-f]{32}` fullmatch。
fn is_ed2k_hash(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `urllib.parse.unquote`（`encoding='utf-8', errors='replace'`）。
///
/// `%` 后面跟不满两个 ASCII 十六进制位时按字面处理；解码字节串时每个
/// 「没能并入完整序列」的字节各产生一个 U+FFFD（实测对齐 CPython：
/// `%E0%80%A0` → 三个 `\ufffd`，`%F0%9F%98%80%F0` → 😀 + 一个 `\ufffd`）。
fn python_unquote(text: &str) -> String {
    if !text.contains('%') {
        return text.to_string();
    }
    let bytes = text.as_bytes();
    let mut decoded: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit()
        {
            let high = (bytes[i + 1] as char)
                .to_digit(16)
                .expect("已判断是十六进制位") as u8;
            let low = (bytes[i + 2] as char)
                .to_digit(16)
                .expect("已判断是十六进制位") as u8;
            decoded.push(high * 16 + low);
            i += 3;
        } else {
            let c = text[i..].chars().next().expect("按字节对齐扫描");
            let mut buffer = [0u8; 4];
            decoded.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
            i += c.len_utf8();
        }
    }
    decode_utf8_replacing_per_byte(&decoded)
}

/// CPython `bytes.decode('utf-8', 'replace')`：每个无法并入完整序列的字节
/// 各 Producing 一个 U+FFFD。
fn decode_utf8_replacing_per_byte(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match std::str::from_utf8(&bytes[i..]) {
            Ok(rest) => {
                out.push_str(rest);
                return out;
            }
            Err(e) => {
                let valid = e.valid_up_to();
                out.push_str(
                    std::str::from_utf8(&bytes[i..i + valid]).expect("valid_len 必是合法 UTF-8"),
                );
                out.push('\u{FFFD}');
                i += valid + 1;
            }
        }
    }
    out
}

/// progress 消息变换层。镜像 `features/ed2k_pack/kelpie/kelpie.py::parseProgress`：
/// 字段原样透传（Python 对出现即用，不做类型收窄）；只实现取键/迭代/下标语义——
/// 缺字段 → `KeyError("字段名")`；`sources` 不可迭代 / 元素不可下标 → 对应的
/// Python `TypeError` 文案；空 sources / 空字符串 / 空 dict（循环零次）按零个
/// 来源成功返回。`Type(message)` 的文案已按 Python 逐字拼好。
#[derive(Debug, Clone, PartialEq)]
pub struct ProgressShape {
    pub hash: Value,
    pub size: Value,
    pub received: Value,
    pub download_rate: Value,
    pub upload_rate: Value,
    pub uploaded: Value,
    pub peers: Value,
    pub active_peers: Value,
    pub held_sources: Value,
    pub held_until: Value,
    pub sources: Vec<[Value; 6]>,
}

/// 失败：`Key(k)` → Python `KeyError(k)`，`Type(message)` → Python `TypeError`（文案已拼好）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgressParseError {
    Key(&'static str),
    Type(String),
}

/// Python 侧 `kelpie.py::parseProgress` 的字段与顺序。
const PROGRESS_FIELDS: [&str; 10] = [
    "hash",
    "size",
    "received",
    "downloadRate",
    "uploadRate",
    "uploaded",
    "peers",
    "activePeers",
    "heldSources",
    "heldUntil",
];

/// Python 侧 `kelpie.py::parseSource` 的字段与顺序。
const SOURCE_FIELDS: [&str; 6] = [
    "address",
    "software",
    "status",
    "rank",
    "downloadRate",
    "channel",
];

fn field_of(
    object: &serde_json::Map<String, Value>,
    key: &'static str,
) -> Result<Value, ProgressParseError> {
    object.get(key).cloned().ok_or(ProgressParseError::Key(key))
}

/// Python `type(value)` 的名字（这是我们需要的全部差异）。
fn py_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(number) => {
            if number.is_f64() {
                "float"
            } else {
                "int"
            }
        }
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// Python `value[字段名]` 在非 dict 上的 TypeError 文案
/// （`str`/`list` 有各自的特殊文案，其余走 `'<type>' object is not subscriptable`）。
fn type_error_subscript(value: &Value) -> ProgressParseError {
    match value {
        Value::Array(_) => {
            ProgressParseError::Type("list indices must be integers or slices, not str".into())
        }
        Value::String(_) => {
            ProgressParseError::Type("string indices must be integers, not 'str'".into())
        }
        other => ProgressParseError::Type(format!(
            "'{}' object is not subscriptable",
            py_type_name(other)
        )),
    }
}

/// Python `for source in message["sources"]`。
fn parse_sources(sources: &Value) -> Result<Vec<[Value; 6]>, ProgressParseError> {
    match sources {
        Value::Array(items) => items.iter().map(parse_source).collect(),
        // Python 迭代 dict 得到键（字符串）；首个元素下标取值即失败，空 dict 不进循环。
        Value::Object(map) => {
            if map.keys().next().is_some() {
                return Err(ProgressParseError::Type(
                    "string indices must be integers, not 'str'".into(),
                ));
            }
            Ok(Vec::new())
        }
        // 空字符串一次循环都不进，不报错。
        Value::String(text) if text.is_empty() => Ok(Vec::new()),
        // 非空字符串：Python 迭代得到字符，首个元素下标取值即失败。
        Value::String(_) => Err(ProgressParseError::Type(
            "string indices must be integers, not 'str'".into(),
        )),
        other => Err(ProgressParseError::Type(format!(
            "'{}' object is not iterable",
            py_type_name(other)
        ))),
    }
}

/// Python `kelpie.py::parseSource(source)`。
fn parse_source(element: &Value) -> Result<[Value; 6], ProgressParseError> {
    let object = match element {
        Value::Object(object) => object,
        other => return Err(type_error_subscript(other)),
    };
    let source: [Value; 6] = SOURCE_FIELDS
        .iter()
        .map(|key| field_of(object, key))
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .expect("SOURCE_FIELDS 是 6 个字段");
    Ok(source)
}

/// `kelpie.py::parseProgress(message)`。
pub fn parse_progress(message: &Value) -> Result<ProgressShape, ProgressParseError> {
    let object = match message {
        Value::Object(object) => object,
        other => return Err(type_error_subscript(other)),
    };
    // Python 原实现按关键字求值顺序：10 个普通字段先于 `sources=`，
    // 所以两者同时缺坏时 KeyError/TypeError 必须先报字段再报 sources。
    let mut fields = PROGRESS_FIELDS
        .iter()
        .map(|key| field_of(object, key))
        .collect::<Result<Vec<_>, _>>()?;
    let sources = parse_sources(&field_of(object, "sources")?)?;
    let held_until = fields.pop().expect("10 个字段");
    let held_sources = fields.pop().expect("10 个字段");
    let active_peers = fields.pop().expect("10 个字段");
    let peers = fields.pop().expect("10 个字段");
    let uploaded = fields.pop().expect("10 个字段");
    let upload_rate = fields.pop().expect("10 个字段");
    let download_rate = fields.pop().expect("10 个字段");
    let received = fields.pop().expect("10 个字段");
    let size = fields.pop().expect("10 个字段");
    let hash = fields.pop().expect("10 个字段");
    debug_assert!(fields.is_empty());
    Ok(ProgressShape {
        hash,
        size,
        received,
        download_rate,
        upload_rate,
        uploaded,
        peers,
        active_peers,
        held_sources,
        held_until,
        sources,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_cpython_control_bytes() {
        // CPython: "\x1cabc\x1c" 的 strip（Rust 自带的 trim 不认识这四个）
        assert_eq!(python_strip("\u{1c}abc\u{1c}"), "abc");
        assert_eq!(python_strip("\ta b\n"), "a b");
    }

    #[test]
    fn unquote_follows_cpython_replacement_count() {
        assert_eq!(python_unquote("%C3%A9"), "é");
        assert_eq!(python_unquote("a%FFb"), "a\u{FFFD}b");
        assert_eq!(python_unquote("%E0%80%A0"), "\u{FFFD}\u{FFFD}\u{FFFD}");
        assert_eq!(python_unquote("x%zz"), "x%zz");
        assert_eq!(python_unquote("100%25"), "100%");
        assert_eq!(python_unquote("plain+plus"), "plain+plus");
        assert_eq!(python_unquote("a%%b"), "a%%b");
        assert_eq!(python_unquote("%"), "%");
        assert_eq!(python_unquote("a%2"), "a%2");
        assert_eq!(python_unquote("%ED%A0%80"), "\u{FFFD}\u{FFFD}\u{FFFD}");
        assert_eq!(python_unquote("%F0%9F%98%80%F0"), "😀\u{FFFD}");
        assert_eq!(python_unquote("%C3%A9%C3"), "é\u{FFFD}");
    }

    #[test]
    fn parses_plain_links() {
        let link = parse("ed2k://|file|video.mp4|1234|abcdef0123456789abcdef0123456789|/")
            .expect("合法链接");
        assert_eq!(link.name, "video.mp4");
        assert_eq!(link.size, 1234);
        assert_eq!(link.hash, "ABCDEF0123456789ABCDEF0123456789");
    }

    #[test]
    fn decodes_percent_encoded_separators_but_not_in_plain_links() {
        // 浏览器把分隔符全编码：整体解码成 '|'
        let link = parse("ed2k://|file|%7Cname%7C|1|0123456789abcdef0123456789abcdef|/")
            .expect("合法链接");
        assert_eq!(link.name, "|name|");
        // 普通链接里 name 的 %7C 由 unquote 解码（实测 CPython：name == "a|b.mp4"）；
        // 注释「%7C is part of the name」只是说明整体解码分支不触发。
        let link =
            parse("ed2k://|file|a%7Cb.mp4|1|0123456789abcdef0123456789abcdef|/").expect("合法链接");
        assert_eq!(link.name, "a|b.mp4");
    }

    #[test]
    fn rejects_malformed_links_with_python_messages() {
        let message = |text: &str| parse(text).expect_err("应失败").message;
        assert_eq!(
            message("http://example.com"),
            "not an eD2k link: http://example.com"
        );
        assert_eq!(
            message("ed2k://|folder|name|"),
            "not an eD2k file link: ed2k://|folder|name|"
        );
        assert_eq!(
            message("ed2k://|file||1|0123456789abcdef0123456789abcdef|/"),
            "link has no name: ed2k://|file||1|0123456789abcdef0123456789abcdef|/"
        );
        assert_eq!(
            message("ed2k://|file|n|0|0123456789abcdef0123456789abcdef|/"),
            "link has an invalid size: ed2k://|file|n|0|0123456789abcdef0123456789abcdef|/"
        );
        assert_eq!(
            message("ed2k://|file|n|-1|0123456789abcdef0123456789abcdef|/"),
            "link has an invalid size: ed2k://|file|n|-1|0123456789abcdef0123456789abcdef|/"
        );
        // HASH_PATTERN 是 [0-9A-Fa-f]{32}：hash 全大写同样合法
        assert!(parse("ed2k://|file|n|1|0123456789ABCDEF0123456789ABCDEF|/").is_ok());
        // 空哈希 / 31 位哈希
        assert_eq!(
            message("ed2k://|file|n|1|0123456789abcdef0123456789abcde|/"),
            "link has an invalid hash: ed2k://|file|n|1|0123456789abcdef0123456789abcde|/"
        );
    }
    #[test]
    fn size_bounds_follow_emule_limit() {
        assert!(parse("ed2k://|file|n|274877906944|0123456789abcdef0123456789abcdef|/").is_ok());
        assert_eq!(
            parse("ed2k://|file|n|274877906945|0123456789abcdef0123456789abcdef|/")
                .expect_err("超上限")
                .message,
            "link has an invalid size: ed2k://|file|n|274877906945|0123456789abcdef0123456789abcdef|/"
        );
    }

    #[test]
    fn wellformed_progress() {
        let message: Value = serde_json::json!({
            "type": "progress", "run": 3,
            "hash": "0123456789abcdef0123456789abcdef", "size": 1024, "received": 10,
            "downloadRate": 1, "uploadRate": 2, "uploaded": 20,
            "peers": 4, "activePeers": 2, "heldSources": 0, "heldUntil": 99,
            "sources": [
                {"address": "1.2.3.4:4662", "software": "kelpie", "status": "active",
                 "rank": 7, "downloadRate": 5, "channel": "tcp"},
                {"address": "", "software": "", "status": "handshake", "rank": 1,
                 "downloadRate": 0, "channel": "udp"},
            ],
        });
        let shape = parse_progress(&message).expect("should parse");
        assert_eq!(shape.hash, Value::from("0123456789abcdef0123456789abcdef"));
        assert_eq!(shape.download_rate, Value::from(1));
        assert_eq!(shape.held_until, Value::from(99));
        assert_eq!(shape.sources.len(), 2);
        assert_eq!(
            shape.sources[0],
            [
                Value::from("1.2.3.4:4662"),
                Value::from("kelpie"),
                Value::from("active"),
                Value::from(7),
                Value::from(5),
                Value::from("tcp"),
            ]
        );
    }

    #[test]
    fn progress_failures_follow_python_messages() {
        let full = serde_json::json!({
            "hash": "h", "size": 1, "received": 2, "downloadRate": 3, "uploadRate": 4,
            "uploaded": 5, "peers": 6, "activePeers": 7, "heldSources": 8, "heldUntil": 9,
            "sources": [{"address": "a:1", "software": "s", "status": "ok"}],
        });
        let fields_of = |sources| {
            serde_json::json!({
                "hash": "h", "size": 1, "received": 2, "downloadRate": 3, "uploadRate": 4,
                "uploaded": 5, "peers": 6, "activePeers": 7, "heldSources": 8, "heldUntil": 9,
                "sources": sources,
            })
        };

        // 字段阶段先于 sources 阶段（Python 关键字求值顺序，2026-XX 实证：
        // {"hash":"h"} → KeyError 'size'，{"sources":[1]} → KeyError 'hash'）
        assert_eq!(
            parse_progress(&serde_json::json!({"hash": "h"})),
            Err(ProgressParseError::Key("size"))
        );
        assert_eq!(
            parse_progress(&serde_json::json!({})),
            Err(ProgressParseError::Key("hash"))
        );
        let mut missing_sources = full.clone();
        missing_sources.as_object_mut().unwrap().remove("sources");
        assert_eq!(
            parse_progress(&missing_sources),
            Err(ProgressParseError::Key("sources"))
        );

        // sources 阶段（10 字段齐全，屏蔽字段阶段干扰）
        let expect_type = |sources, message: &str| {
            assert_eq!(
                parse_progress(&fields_of(sources)),
                Err(ProgressParseError::Type(message.to_string())),
                "sources case"
            );
        };
        // 元素下标错误：int / bool / NoneType / float → … is not subscriptable
        expect_type(serde_json::json!([1]), "'int' object is not subscriptable");
        expect_type(
            serde_json::json!([serde_json::Value::Null]),
            "'NoneType' object is not subscriptable",
        );
        // str 与 list 元素 → indices 文案（str 带 ", not 'str'" 尾巴，CPython 3.13 实测）
        expect_type(
            serde_json::json!(["ab"]),
            "string indices must be integers, not 'str'",
        );
        expect_type(
            serde_json::json!([[1]]),
            "list indices must be integers or slices, not str",
        );
        // sources 不可迭代
        expect_type(serde_json::json!(5), "'int' object is not iterable");
        expect_type(serde_json::json!(true), "'bool' object is not iterable");
        // 空 sources（零循环）成功返回零来源；dict 迭代得键并立即下标失败
        assert!(
            parse_progress(&fields_of(serde_json::json!("")))
                .expect("空串零循环")
                .sources
                .is_empty()
        );
        assert!(
            parse_progress(&fields_of(serde_json::json!({})))
                .expect("空 dict 零循环")
                .sources
                .is_empty()
        );
        expect_type(
            serde_json::json!({"a": 1}),
            "string indices must be integers, not 'str'",
        );
        // source 缺字段按 SOURCE_FIELDS 顺序报第一个缺失
        let mut missing_software = full.clone();
        let first = missing_software
            .as_object_mut()
            .unwrap()
            .get_mut("sources")
            .unwrap()
            .as_array_mut()
            .unwrap()
            .get_mut(0)
            .unwrap()
            .as_object_mut()
            .unwrap();
        first.remove("software");
        assert_eq!(
            parse_progress(&missing_software),
            Err(ProgressParseError::Key("software"))
        );

        // 根值不是 dict
        let not_dict = serde_json::json!([1]);
        assert_eq!(
            parse_progress(&not_dict),
            Err(ProgressParseError::Type(
                "list indices must be integers or slices, not str".to_string()
            ))
        );
    }
}
