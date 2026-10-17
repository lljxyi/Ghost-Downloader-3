//! CPython `shlex.split(s, comments=False, posix=True)` 的镜像（第 48 步）。
//!
//! `app/view/components/headers_editor.py:37` 用它把「复制为 cURL」的
//! 命令行拆成 token。`shlex` crate 与 CPython 有三处语义分歧（双引号内
//! `\$`/`` \` `` 的反斜杠保留、空白集含 `\r`、单引号内反斜杠是字面量），
//! 所以这里手写状态机逐字镜像 CPython 3.13 的 `shlex.shlex.read_token`
//! 在 `posix=True, whitespace_split=True, commenters='', punctuation_chars=''`
//! 下的行为。
//!
//! 镜像要点（对应 `/usr/lib/python3.13/shlex.py`）：
//! - 空白集固定 `' ' '\t' '\r' '\n'`；`\u{a0}`、`\x1c`、`\x1f` **不是**
//!   空白（Rust `char::is_whitespace` 会把它们算进去，不能直接用）。
//! - `whitespace_split=True` 让 `wordchars` 失效：词状态里除空白、引号、
//!   反斜杠以外的字符一律进 token。
//! - `commenters=''`：`#` 是普通字符。
//! - `punctuation_chars=''`：`&&`、`||` 之类不会被切开，只有被空白分隔
//!   时才成独立 token。
//! - 转义：词外/词内 `\X` → `X`；双引号内只有 `\` 与 `"` 会被吃掉反斜杠，
//!   其余（`\$`、`` \` ``、`\n`）保留反斜杠；单引号内反斜杠是字面量。
//! - EOF 落在引号内 → `ValueError("No closing quotation")`；落在反斜杠后
//!   → `ValueError("No escaped character")`；两者都返回 `None`。
//! - 引号包裹的空串是合法 token（`''` → `[""]`）。
//!
//! 金文件：`tests/fixtures/headers_parse.jsonl`（`kind == "split"`）。

/// CPython `shlex.shlex.whitespace`。
fn is_shlex_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n')
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    /// `' '`：词间。
    Space,
    /// `'a'`：词内。
    Word,
    /// 引号内（区分单/双引号——只有双引号允许反斜杠转义）。
    Quote(char),
    /// `'\'` 后：`escaped_from` 记着该回到哪个状态。
    Escape,
}

/// 镜像 `shlex.split(s, comments=False, posix=True)`；引号/转义未闭合时
/// 返回 `None`（Python 抛 `ValueError`，`parseCurl` 吞掉返回 `[]`）。
pub fn split(input: &str) -> Option<Vec<String>> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens: Vec<String> = Vec::new();
    let mut pos = 0usize;
    let mut finished = false;
    while !finished {
        let mut token = String::new();
        // CPython 的 `quoted` 是 read_token 的局部变量：引号包裹过的空串
        // 仍是合法 token，未包裹的空串才是 EOF 标记。
        let mut quoted = false;
        let mut state = State::Space;
        let mut escaped_from = State::Space;
        let mut emit = false;
        while !emit && !finished {
            let next = chars.get(pos).copied();
            match state {
                State::Space => match next {
                    None => finished = true,
                    Some(c) if is_shlex_whitespace(c) => {
                        pos += 1;
                        if !token.is_empty() || quoted {
                            emit = true;
                        }
                    }
                    Some('\\') => {
                        // CPython 在 `' '` 状态见反斜杠时写 `escapedstate = 'a'`
                        // （不是 `' '`）——转义后必回词内态；逐字照抄。
                        escaped_from = State::Word;
                        state = State::Escape;
                        pos += 1;
                    }
                    Some(c) if c == '\'' || c == '"' => {
                        state = State::Quote(c);
                        pos += 1;
                    }
                    Some(c) => {
                        token.push(c);
                        state = State::Word;
                        pos += 1;
                    }
                },
                State::Word => match next {
                    None => finished = true,
                    Some(c) if is_shlex_whitespace(c) => {
                        pos += 1;
                        if !token.is_empty() || quoted {
                            emit = true;
                        }
                    }
                    Some(c) if c == '\'' || c == '"' => {
                        state = State::Quote(c);
                        pos += 1;
                    }
                    Some('\\') => {
                        escaped_from = State::Word;
                        state = State::Escape;
                        pos += 1;
                    }
                    Some(c) => {
                        token.push(c);
                        pos += 1;
                    }
                },
                State::Quote(quote) => {
                    // 引号内 EOF：CPython 抛 No closing quotation。
                    let c = next?;
                    quoted = true;
                    if c == quote {
                        state = State::Word;
                        pos += 1;
                    } else if quote == '"' && c == '\\' {
                        escaped_from = State::Quote('"');
                        state = State::Escape;
                        pos += 1;
                    } else {
                        token.push(c);
                        pos += 1;
                    }
                }
                State::Escape => {
                    // 反斜杠后 EOF：CPython 抛 No escaped character。
                    let c = next?;
                    // 双引号内只有 `\` 与 `"` 允许吃掉反斜杠。
                    if let State::Quote(q) = escaped_from
                        && c != '\\'
                        && c != q
                    {
                        token.push('\\');
                    }
                    token.push(c);
                    state = escaped_from;
                    pos += 1;
                }
            }
        }
        if token.is_empty() && !quoted {
            finished = true;
        } else {
            tokens.push(token);
        }
    }
    Some(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split_ok(input: &str) -> Vec<String> {
        split(input).unwrap_or_else(|| panic!("不该报错：{input:?}"))
    }

    #[test]
    fn splits_on_cpython_whitespace_only() {
        assert_eq!(split_ok(""), Vec::<String>::new());
        assert_eq!(split_ok("   "), Vec::<String>::new());
        assert_eq!(split_ok("a b"), vec!["a", "b"]);
        assert_eq!(split_ok("a\rb\nc\td"), vec!["a", "b", "c", "d"]);
        // NBSP / \x1c / \x1f 不是 shlex 空白（char::is_whitespace 会误判）。
        assert_eq!(split_ok("a\u{a0}b"), vec!["a\u{a0}b"]);
        assert_eq!(split_ok("a\u{1c}b"), vec!["a\u{1c}b"]);
        assert_eq!(split_ok("a\u{1f}b"), vec!["a\u{1f}b"]);
        // commenters 为空：`#` 是普通字符。
        assert_eq!(split_ok("#a b"), vec!["#a", "b"]);
    }

    #[test]
    fn honours_quotes_and_escapes() {
        assert_eq!(split_ok("''"), vec![""]);
        assert_eq!(split_ok("'' \"\""), vec!["", ""]);
        assert_eq!(split_ok("'a b' c"), vec!["a b", "c"]);
        assert_eq!(split_ok("a'b'c"), vec!["abc"]);
        // 单引号内反斜杠是字面量。
        assert_eq!(split_ok("'a\\b'"), vec!["a\\b"]);
        // 双引号内只有 `\` 与 `"` 被吃掉反斜杠。
        assert_eq!(split_ok("\"a\\\\b\""), vec!["a\\b"]);
        assert_eq!(split_ok("\"a\\$b\""), vec!["a\\$b"]);
        assert_eq!(split_ok("\"a\\`b\""), vec!["a\\`b"]);
        assert_eq!(split_ok("\"a\\nb\""), vec!["a\\nb"]);
        assert_eq!(split_ok("\"a\\\"b\""), vec!["a\"b"]);
        // 词外/词内转义吞掉反斜杠。
        assert_eq!(split_ok("a\\ b"), vec!["a b"]);
        assert_eq!(split_ok("a\\\\ b"), vec!["a\\", "b"]);
        assert_eq!(split_ok("\\a"), vec!["a"]);
        assert_eq!(split_ok("a\\$b"), vec!["a$b"]);
        // 词外转义把空白带进词里（CPython escapedstate='a' 的可见后果）。
        assert_eq!(split_ok("\\ x"), vec![" x"]);
        assert_eq!(split_ok("\\\tx"), vec!["\tx"]);
        assert_eq!(split_ok("a \\ b"), vec!["a", " b"]);
    }

    #[test]
    fn unterminated_quote_or_escape_is_none() {
        for bad in ["'a", "\"a", "a\\", "\\", "'a\\'b'", "\"a\\\"", "\"a\\$"] {
            assert!(split(bad).is_none(), "应报错：{bad:?}");
        }
    }

    #[test]
    fn punctuation_is_not_split_without_whitespace() {
        assert_eq!(
            split_ok("curl 'a' && echo hi"),
            vec!["curl", "a", "&&", "echo", "hi"]
        );
        assert_eq!(split_ok("a&&b"), vec!["a&&b"]);
        assert_eq!(split_ok("a|b"), vec!["a|b"]);
    }
}
