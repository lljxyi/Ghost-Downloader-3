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
}
