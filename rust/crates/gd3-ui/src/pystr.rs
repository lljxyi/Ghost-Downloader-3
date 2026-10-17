//! CPython 字符串口径小工具（第 47 步从 headers.rs 抽出，供 headers /
//! identity 共用）。
//!
//! 实证（`.venv` CPython 3.13 逐条实测）：
//! - `str.strip()` / `str.isspace()` 把 `\x1c`-`\x1f`（信息分隔符）也算
//!   空白，Rust `char::is_whitespace` 不含这四个码位；
//! - `str.splitlines()` 的行界比 `str::lines` 宽：`\r`、`\v`、`\f`、
//!   `\x1c`-`\x1e`、`\u{85}`、`\u{2028}`、`\u{2029}`，`\r\n` 视为单个；
//!   注意 `\x1f` 是空白但**不是**行界（`'a\x1fb'.splitlines()` →
//!   `['a\x1fb']`）。

/// CPython `str.isspace()` 口径。
pub fn py_isspace(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

/// 镜像 CPython `str.strip()`（两端 py_isspace 字符）。
pub fn py_strip(text: &str) -> &str {
    text.trim_matches(py_isspace)
}

/// 镜像 CPython `str.splitlines()` 的行界集合（`\r\n` 视为单个行界）。
pub fn split_lines(text: &str) -> Vec<String> {
    fn is_boundary(c: char) -> bool {
        matches!(
            c,
            '\n' | '\r'
                | '\x0b'
                | '\x0c'
                | '\x1c'
                | '\x1d'
                | '\x1e'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        )
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if is_boundary(c) {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            lines.push(std::mem::take(&mut current));
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_matches_cpython_isspace() {
        // \x1c-\x1f 在 Python 是空白、Rust is_whitespace 不是。
        assert_eq!(py_strip(" \u{1f}a\u{1c} "), "a");
        assert_eq!(py_strip("\u{1e}\u{1d}"), "");
        // U+00A0（NBSP）两侧都算空白，同样被 strip。
        assert_eq!(py_strip("\u{a0}x"), "x");
    }

    #[test]
    fn split_lines_matches_cpython_boundaries() {
        assert_eq!(split_lines("a\r\nb"), vec!["a", "b"]);
        assert_eq!(split_lines("a\u{b}b\u{c}c"), vec!["a", "b", "c"]);
        assert_eq!(
            split_lines("a\u{1c}b\u{1d}c\u{1e}d"),
            vec!["a", "b", "c", "d"]
        );
        assert_eq!(
            split_lines("a\u{85}b\u{2028}c\u{2029}d"),
            vec!["a", "b", "c", "d"]
        );
        // \x1f 不是行界。
        assert_eq!(split_lines("a\u{1f}b"), vec!["a\u{1f}b"]);
        // 尾随行界不产生空尾行；纯空串 → []。
        assert_eq!(split_lines("a\n"), vec!["a"]);
        assert_eq!(split_lines(""), Vec::<String>::new());
        // 连续行界产生空行（与 Python 一致）。
        assert_eq!(split_lines("a\n\nb"), vec!["a", "", "b"]);
    }
}
