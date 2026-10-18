//! 剪贴板监听（第 77 步）：`app/view/shell/clipboard_listener.py`(67) 的语义镜像。
//!
//! 为什么单独一层：`draft.rs::clipboard_urls` 只镜像了
//! `task_draft.py::_clipboardUrls`（逐行过滤 + matcher），而 `ClipboardListener`
//! 的**状态机**是另一套东西——自复制闩锁、Wayland 去重、启停只连一次。这三条
//! 决定「复制任务链接后会不会自己弹一个建任务框」，必须逐条钉死。
//!
//! ## 实证口径（2026-10-18 实读源码 + `.venv/bin/python` 核 `urlparse`）
//!
//! - `setUrls(urls)`：**只有 `_enabled` 时才**置 `_selfCopied = True`，然后
//!   `QApplication.clipboard().setText("\n".join(urls))`（:23-26）。所以关监听时
//!   程序自己写剪贴板不会被闩锁吃掉——但那时也没接 `dataChanged`，结果一样。
//! - `setEnabled(enabled)`（:28-37）：只在**状态翻转**时连/断信号——
//!   `enabled and not _enabled` → connect，`not enabled and _enabled` → disconnect，
//!   其余情况**不碰信号**（重复 setEnabled(True) 不会二次 connect）。
//! - `_onDataChanged`（:39-53）顺序是契约：先消费自复制闩锁（**即使没有链接也消费**），
//!   再取链接、空则返回；Wayland 下与上次快照相同则**不再发**信号，不同则记下再发；
//!   其它平台**完全不用** `_lastUrls`（同一段文本连续两次 dataChanged 会发两次）。
//! - `_downloadableUrls`（:55-67）：逐行 `splitlines()` + `strip()`，空行跳过；
//!   `urlparse` 抛 `ValueError`（仅非法 IPv6 括号）→ 记日志跳过；
//!   **`not parsed.scheme or parsed.geturl() != url` 才跳过**——注意是 `geturl()`
//!   与原串逐字符相等，不是「看起来像 URL」。实测 `urlparse` 的口径：
//!   `"HTTP://x/"` → scheme 被小写成 `http`，`geturl()` 为 `"http://x/"` ≠ 原串
//!   → **丢弃**；`"http:/x"` → `geturl()` 为 `"http:///x"` → 丢弃；
//!   `"http://a/1?"` → 空 query 在 `geturl()` 里被丢掉 → 丢弃；
//!   `"http://a b/1"`（路径带空格）、`"http://A/1"`（主机大写）、
//!   `"http://a/./b/../c"`（不规范化）→ **都保留**；
//!   `"not-a-url"` / `"//a/1"` → scheme 为空 → 丢弃。
//! - `geturl()` 走 `urlunsplit`：**只有 `uses_netloc` 里的 scheme**（`''`、`http`、
//!   `https`、`ftp`、`file`、`ws`、`git`…）在 netloc 为空且 path 以 `/` 开头（或为空）
//!   时才补 `//`。所以 `"http:/x"` → `"http:///x"`（丢弃），而 `"a:/b"` →
//!   `"a:/b"`（保留）——**同一个形状，结果相反**，只能按表实现。
//! - `urlsplit` 先 lstrip 掉 C0 控制符与空格，再**删掉 `\t`/`\r`/`\n`**：
//!   被删过字符的串 `geturl()` 必然与原串不等 → 一律丢弃。
//! - 方括号主机还要过 `_check_bracketed_netloc`（只允许 `0-9a-fA-F:.%`），
//!   非法字符同样算 `ValueError`。
//! - 最后才 `self._matchPassive(url)`（:66），matcher 只看到通过 `geturl()` 校验的串。
//!
//! ## 刻意的偏离
//!
//! - **scheme 的字符判定按 ASCII**：Python 用 `str.isalpha()` / `isalnum()`（Unicode
//!   语义），`"例:x"` 会被当成合法 scheme；本模块要求 ASCII 字母开头，这类串退化为
//!   「scheme 为空 → 丢弃」。真实剪贴板里不会出现非 ASCII scheme。
//! - **不建模 `QApplication.clipboard()` 对象**：`set_urls` 只返回该写进剪贴板的文本，
//!   写与读都由调用方做（无头测试不需要真剪贴板）。

use crate::pystr::{py_strip, split_lines};

/// 监听器状态机。`wayland` 是 `QApplication.platformName() == "wayland"` 的实测值。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClipboardListener {
    enabled: bool,
    self_copied: bool,
    last_urls: Vec<String>,
    wayland: bool,
}

impl ClipboardListener {
    pub fn new(wayland: bool) -> Self {
        Self {
            enabled: false,
            self_copied: false,
            last_urls: Vec::new(),
            wayland,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// `_selfCopied` 当前值（测试与装配层可见）。
    pub fn self_copied(&self) -> bool {
        self.self_copied
    }

    /// `_lastUrls` 当前快照（Wayland 去重口径）。
    pub fn last_urls(&self) -> &[String] {
        &self.last_urls
    }

    /// `setEnabled`：返回 `Some(true)` 表示要 `connect`，`Some(false)` 表示要
    /// `disconnect`，`None` 表示状态没翻转、**不动信号**。
    pub fn set_enabled(&mut self, enabled: bool) -> Option<bool> {
        let action = if enabled && !self.enabled {
            Some(true)
        } else if !enabled && self.enabled {
            Some(false)
        } else {
            None
        };
        self.enabled = enabled;
        action
    }

    /// `setUrls`：返回该写进剪贴板的文本（`"\n".join(urls)`）。
    ///
    /// 只有监听开启时才置自复制闩锁——与 Python 一致。
    pub fn set_urls(&mut self, urls: &[String]) -> String {
        if self.enabled {
            self.self_copied = true;
        }
        urls.join("\n")
    }

    /// `_onDataChanged`：`Some(urls)` = 该发 `urlsDetected(urls)`；`None` = 不发。
    pub fn on_data_changed(
        &mut self,
        text: &str,
        matcher: impl Fn(&str) -> bool,
    ) -> Option<Vec<String>> {
        if self.self_copied {
            self.self_copied = false;
            return None;
        }

        let urls = downloadable_urls(text, matcher);
        if urls.is_empty() {
            return None;
        }

        if self.wayland {
            if urls == self.last_urls {
                return None;
            }
            self.last_urls = urls.clone();
        }

        Some(urls)
    }
}

/// `_downloadableUrls`：逐行去空白，`geturl()` 往返校验，最后过 matcher。
pub fn downloadable_urls(text: &str, matcher: impl Fn(&str) -> bool) -> Vec<String> {
    let mut urls = Vec::new();
    for line in split_lines(text) {
        let url = py_strip(&line);
        if url.is_empty() {
            continue;
        }
        if !url_roundtrip_ok(url) {
            continue;
        }
        if matcher(url) {
            urls.push(url.to_string());
        }
    }
    urls
}

/// `urlparse(url)` 不抛错 **且** `parsed.geturl() == url` **且** scheme 非空。
///
/// 逐条镜像 CPython `urllib.parse.urlsplit` + `SplitResult.geturl`：
/// scheme 必须已是小写、空 query/fragment 会被 `geturl` 吃掉、
/// `//` 之后的主机段非法 IPv6 括号算解析失败。
pub fn url_roundtrip_ok(url: &str) -> bool {
    // `urlsplit` 会 lstrip C0/空格、删掉 `\t\r\n`；动过字符就不可能往返相等。
    if url.chars().next().is_some_and(|c| (c as u32) < 0x21) || url.contains(['\t', '\r', '\n']) {
        return false;
    }
    match split_url(url) {
        Some(parts) => !parts.scheme.is_empty() && geturl(&parts) == url,
        None => false,
    }
}

struct UrlParts<'a> {
    scheme: String,
    netloc: &'a str,
    path: &'a str,
    query: &'a str,
    fragment: &'a str,
}

fn is_scheme_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')
}

/// `_check_bracketed_netloc` 允许的字符集。
fn is_bracket_char(c: char) -> bool {
    c.is_ascii_hexdigit() || matches!(c, ':' | '.' | '%')
}

/// `urlsplit` 的降级实现：只覆盖 `downloadable_urls` 会遇到的输入面。
///
/// 返回 `None` 对应 Python 的 `ValueError`（非法 IPv6 括号）。
fn split_url(url: &str) -> Option<UrlParts<'_>> {
    let mut scheme = String::new();
    let mut rest = url;

    if let Some(colon) = url.find(':') {
        let candidate = &url[..colon];
        let starts_with_alpha = candidate
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic());
        if colon > 0 && starts_with_alpha && candidate.chars().all(is_scheme_char) {
            scheme = candidate.to_ascii_lowercase();
            rest = &url[colon + 1..];
        }
    }

    let mut netloc = "";
    if let Some(after) = rest.strip_prefix("//") {
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        netloc = &after[..end];
        rest = &after[end..];
        // CPython：`('[' in netloc and ']' not in netloc)` 或反向 → ValueError。
        if netloc.contains('[') != netloc.contains(']') {
            return None;
        }
        if let (Some(open), Some(close)) = (netloc.find('['), netloc.find(']')) {
            // CPython 用 `netloc.index('[') + 1 : netloc.index(']')`；`]` 在 `[` 前时
            // 切片为空 → 通过。
            if close > open && !netloc[open + 1..close].chars().all(is_bracket_char) {
                return None;
            }
        }
    }

    // Python 先切 `#` 再切 `?`：fragment 里的 `?` 不会被当成 query 分隔符。
    let (rest, fragment) = match rest.find('#') {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, ""),
    };
    let (path, query) = match rest.find('?') {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, ""),
    };

    Some(UrlParts {
        scheme,
        netloc,
        path,
        query,
        fragment,
    })
}

/// `uses_netloc`（CPython `urllib.parse.uses_netloc`）的完整快照。
///
/// 只影响 `geturl()` 补不补 `//`：不在表里的 scheme（`a`、`magnet`、`mailto`…）
/// 一律不加。
fn uses_netloc(scheme: &str) -> bool {
    matches!(
        scheme,
        "" | "ftp"
            | "http"
            | "gopher"
            | "nntp"
            | "telnet"
            | "imap"
            | "wais"
            | "file"
            | "mms"
            | "https"
            | "shttp"
            | "snews"
            | "prospero"
            | "rtsp"
            | "rtsps"
            | "rtspu"
            | "rsync"
            | "svn"
            | "svn+ssh"
            | "sftp"
            | "nfs"
            | "git"
            | "git+ssh"
            | "ws"
            | "wss"
            | "itms-services"
    )
}

/// `SplitResult.geturl()`（= `urlunsplit`）：空 query / 空 fragment 不输出分隔符，
/// netloc 为空时按 `uses_netloc` 决定要不要补 `//`。
fn geturl(parts: &UrlParts<'_>) -> String {
    let mut body = String::new();
    if !parts.netloc.is_empty() {
        if !parts.path.is_empty() && !parts.path.starts_with('/') {
            body.push('/');
        }
        body.push_str("//");
        body.push_str(parts.netloc);
        body.push_str(parts.path);
    } else if parts.path.starts_with("//")
        || (uses_netloc(&parts.scheme) && (parts.path.is_empty() || parts.path.starts_with('/')))
    {
        // 两条分支合并：`url[:2] == "//"` 与 `uses_netloc` 都只是补 `//` 再拼 path。
        body.push_str("//");
        body.push_str(parts.path);
    } else {
        body.push_str(parts.path);
    }

    let mut out = if parts.scheme.is_empty() {
        body
    } else {
        format!("{}:{}", parts.scheme, body)
    };
    if !parts.query.is_empty() {
        out.push('?');
        out.push_str(parts.query);
    }
    if !parts.fragment.is_empty() {
        out.push('#');
        out.push_str(parts.fragment);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    fn any_url(_: &str) -> bool {
        true
    }

    #[test]
    fn set_urls_joins_with_newline_and_latches_only_when_enabled() {
        let mut listener = ClipboardListener::new(false);
        assert_eq!(listener.set_urls(&urls(&["a", "b"])), "a\nb");
        assert!(!listener.self_copied(), "关监听时不该置自复制闩锁");

        assert_eq!(listener.set_enabled(true), Some(true));
        assert_eq!(listener.set_urls(&urls(&["c"])), "c");
        assert!(listener.self_copied());
    }

    #[test]
    fn set_enabled_only_signals_on_transitions() {
        let mut listener = ClipboardListener::new(false);
        assert_eq!(listener.set_enabled(true), Some(true));
        assert_eq!(listener.set_enabled(true), None, "重复开启不该二次 connect");
        assert_eq!(listener.set_enabled(false), Some(false));
        assert_eq!(
            listener.set_enabled(false),
            None,
            "重复关闭不该二次 disconnect"
        );
        assert!(!listener.is_enabled());
    }

    #[test]
    fn self_copied_latch_is_consumed_even_without_urls() {
        let mut listener = ClipboardListener::new(false);
        listener.set_enabled(true);
        listener.set_urls(&urls(&["https://a/1"]));
        // 剪贴板里是无关文本：闩锁照样被消费。
        assert_eq!(listener.on_data_changed("hello", any_url), None);
        assert!(!listener.self_copied());
        // 下一次 dataChanged 才真正识别。
        assert_eq!(
            listener.on_data_changed("https://a/1", any_url),
            Some(urls(&["https://a/1"]))
        );
    }

    #[test]
    fn non_wayland_emits_every_time_but_wayland_dedupes() {
        let mut plain = ClipboardListener::new(false);
        assert_eq!(
            plain.on_data_changed("https://a/1", any_url),
            Some(urls(&["https://a/1"]))
        );
        assert_eq!(
            plain.on_data_changed("https://a/1", any_url),
            Some(urls(&["https://a/1"]))
        );
        assert!(plain.last_urls().is_empty(), "非 Wayland 不维护快照");

        let mut wayland = ClipboardListener::new(true);
        assert_eq!(
            wayland.on_data_changed("https://a/1", any_url),
            Some(urls(&["https://a/1"]))
        );
        assert_eq!(wayland.on_data_changed("https://a/1", any_url), None);
        assert_eq!(wayland.last_urls(), ["https://a/1"]);
        assert_eq!(
            wayland.on_data_changed("https://b/2", any_url),
            Some(urls(&["https://b/2"]))
        );
    }

    #[test]
    fn wayland_dedupe_compares_the_whole_list() {
        let mut wayland = ClipboardListener::new(true);
        assert_eq!(
            wayland.on_data_changed("https://a/1\nhttps://b/2", any_url),
            Some(urls(&["https://a/1", "https://b/2"]))
        );
        // 顺序变了就算不同（tuple 比较）。
        assert_eq!(
            wayland.on_data_changed("https://b/2\nhttps://a/1", any_url),
            Some(urls(&["https://b/2", "https://a/1"]))
        );
    }

    #[test]
    fn downloadable_urls_strips_lines_and_skips_blanks() {
        let text = "  https://a/1  \n\n\thttps://b/2\n";
        assert_eq!(
            downloadable_urls(text, any_url),
            urls(&["https://a/1", "https://b/2"])
        );
    }

    #[test]
    fn matcher_sees_only_roundtrip_safe_urls() {
        let text = "not-a-url\nHTTP://x/\nhttps://ok/1\nmagnet:?xt=1\n";
        let picked = downloadable_urls(text, |u| u.starts_with("https") || u.starts_with("magnet"));
        assert_eq!(picked, urls(&["https://ok/1", "magnet:?xt=1"]));
    }

    #[test]
    fn url_roundtrip_matches_cpython_on_the_empirical_corpus() {
        // 全部来自 `.venv/bin/python` 对 urlparse().geturl() 的实测输出。
        for ok in [
            "http://a/1",
            "https://example.com",
            "http://example.com:80/",
            "magnet:?xt=1",
            "http://a/1?b=2#c",
            "ftp://u:p@h/p",
            "http://a/./b/../c",
            "http://A/1",
            "file:///tmp/a",
            "http://a/%41",
            "http://a/1/",
            "mailto:a@b",
            "http://a b/1",
            "http://a/1 2",
            "https://例子.测试/路径",
            "http://a",
            "http://a?b",
            "http://a/1#f?q",
            "a:b",
            "a:/b",
            "x:",
            "http://",
            "http://?x",
            "a1:b",
            "a+b:c",
            "http://a/1#c?d",
        ] {
            assert!(url_roundtrip_ok(ok), "{ok} 应通过 geturl 往返校验");
        }
        for bad in [
            "HTTP://x/",
            "http:/x",
            "not-a-url",
            "//a/1",
            "http://a/1?",
            "http://a/1#",
            "http://[::1",
            ":x",
            "http:/x",
            "http://a\t/1",
            "http://[zz]/",
            "1a:b",
        ] {
            assert!(!url_roundtrip_ok(bad), "{bad} 应被 geturl 往返校验拒掉");
        }
    }

    #[test]
    fn empty_query_and_fragment_markers_are_dropped_like_cpython() {
        assert!(!url_roundtrip_ok("http://a/1?"));
        assert!(!url_roundtrip_ok("http://a/1#"));
        // 非空则保留。
        assert!(url_roundtrip_ok("http://a/1?b"));
        assert!(url_roundtrip_ok("http://a/1#c"));
    }

    #[test]
    fn uses_netloc_decides_the_double_slash() {
        // 同为「scheme + 单斜杠路径」，http 补 `//`、a 不补 —— 结果相反。
        assert!(!url_roundtrip_ok("http:/x"));
        assert!(url_roundtrip_ok("a:/b"));
        assert!(url_roundtrip_ok("file:///tmp/a"));
        assert!(url_roundtrip_ok("ws://a/1"));
        assert!(url_roundtrip_ok("itms-services://?action=download"));
    }

    #[test]
    fn unsafe_bytes_and_leading_controls_never_round_trip() {
        // `urlsplit` 会删掉 \t\r\n，删过就不可能与原串相等。
        assert!(!url_roundtrip_ok("http://a\t/1"));
        assert!(!url_roundtrip_ok("http://a/1\r"));
        // 行内前导控制符会被 lstrip 掉。
        assert!(!url_roundtrip_ok("\u{1}http://a/1"));
        assert!(url_roundtrip_ok("http://a/1"));
    }

    #[test]
    fn bracket_mismatch_is_the_only_value_error() {
        assert!(!url_roundtrip_ok("http://[::1"));
        assert!(!url_roundtrip_ok("http://::1]"));
        assert!(url_roundtrip_ok("http://[::1]/"));
    }
}
