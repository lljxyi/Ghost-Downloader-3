//! 极简 `urlparse` 子集：`hostname` 与 lowercase 之后的 `path`。
//!
//! 第 108 步从 `github.rs` 抽出来：`github` / `http_pack` 判 host 用它，
//! `gd3-ui::feature_service` 的预设前置也要从 URL 里抠 host（`feature_service.py:79`
//! 的 `urlparse(options.url).hostname or ""`），三处共用一份，免得各写一遍再漂移。
//!
//! 跟 Python 的对应关系（照 `urlparse` 的现状抄，别去"修正"）：
//! - `hostname` 是 lowercase、去掉端口与 userinfo 的 host（`_hostinfo` 在**第一个**
//!   `:` 处切端口，且端口不要求是数字）；
//! - `path` 不含 query / fragment；**本模块额外 lowercase 了 path**（Python 保留原样）
//!   ——两个消费者都只做 lowercase 比较，这个降级写在 [`ParsedUrl::path`] 上；
//! - scheme 只认 ASCII 字母数字与 `+ - .`，且后面必须跟 `//` 才可能有 host。

/// 极简 `urlparse` 子集：`hostname` 与 lowercase 之后的 `path`。
pub(crate) struct ParsedUrl {
    pub(crate) scheme: String,
    pub(crate) host: String,
    pub(crate) path: String,
}

/// `urlparse(url).hostname or ""`：取不到 host 时给空串（Python 给 `None`）。
///
/// `feature_service.py:79` 与 `app/client.py:83` 的 `matchIdentityPreset` 都走这条
/// 口径；`urlparse` 在非法 IPv6 上会抛 `ValueError`，这里一律当「没有 host」。
pub fn host_of(url: &str) -> String {
    parse_url(url).host
}

/// 与 Python `urlparse(url)` 在三个字段上等价的最小解析（`pub(crate)`：
/// `http_pack` 的 `HttpParser.match` / `matchPassive`、`github` 的
/// `is_github_file_url` / `match_file` 复用它）。
///
/// 没有 `://` 的输入不会产生 host（Python 里这种输入的 `netloc` 为空）。
pub(crate) fn parse_url(url: &str) -> ParsedUrl {
    let colon = url.find(':');
    let (scheme, rest) = match colon {
        Some(idx) => {
            let prefix = &url[..idx];
            let valid = !prefix.is_empty()
                && prefix
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.');
            if valid {
                (prefix.to_ascii_lowercase(), &url[idx + 1..])
            } else {
                return ParsedUrl {
                    scheme: String::new(),
                    host: String::new(),
                    path: url.to_ascii_lowercase(),
                };
            }
        }
        None => {
            return ParsedUrl {
                scheme: String::new(),
                host: String::new(),
                path: url.to_ascii_lowercase(),
            };
        }
    };

    let (host, path) = match rest.strip_prefix("//") {
        Some(after) => {
            let end = after.find(['/', '?', '#']).unwrap_or(after.len());
            let authority = &after[..end];
            let path = after[end..]
                .split(['?', '#'])
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            (authority.to_string(), path)
        }
        None => {
            let path = rest
                .split(['?', '#'])
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            (String::new(), path)
        }
    };

    ParsedUrl {
        scheme,
        host: hostname(&host),
        path,
    }
}

/// 对齐 Python `parsed.hostname`（`_hostinfo` + `hostname.lower()`）：
/// 去 userinfo（在**最后一个** `@` 处切）、IPv6 去前后括号、在**第一个** `:` 处切端口、
/// lowercase。
///
/// 端口不要求是数字：`host:notaport` 的 hostname 仍是 `host`（CPython 实测）。
fn hostname(authority: &str) -> String {
    let without_userinfo = match authority.rfind('@') {
        Some(idx) => &authority[idx + 1..],
        None => authority,
    };
    if let Some(stripped) = without_userinfo.strip_prefix('[') {
        // IPv6：`[::1]:8080` → `::1`
        return stripped
            .split(']')
            .next()
            .unwrap_or(stripped)
            .to_ascii_lowercase();
    }
    without_userinfo
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 期望值来自 CPython 3.13 的 `urlparse(u).hostname or ""`（第 108 步实取）。
    #[test]
    fn host_of_mirrors_urlparse_hostname() {
        let cases = [
            ("https://example.com/a.zip", "example.com"),
            ("HTTPS://Example.COM/A.zip", "example.com"),
            ("https://user:pw@host.example:8443/x", "host.example"),
            ("https://[::1]:8080/x", "::1"),
            ("https://[::1]/x", "::1"),
            // 端口不是数字时 CPython 仍按第一个 ':' 切（`_hostinfo` 不校验端口）
            ("https://host.example:notaport/x", "host.example"),
            ("http://host.example", "host.example"),
            ("https://host.example?q=1", "host.example"),
            ("https://host.example#frag", "host.example"),
            ("https://host.example:8080", "host.example"),
            ("https://:8080/x", ""),
            ("https://@/x", ""),
            ("ftp://ftp.example/dir/file.bin", "ftp.example"),
            ("https://pan.baidu.com/s/1abc", "pan.baidu.com"),
            // 没有 `://` 就没有 host（Python 的 netloc 为空）
            ("pcs.baidu.com/x", ""),
            ("/local/path", ""),
            ("", ""),
            ("mailto:a@b.com", ""),
            ("magnet:?xt=urn:btih:abc", ""),
            // ed2k 的 netloc 是 `|file|...|`，`_hostinfo` 照样返回它
            ("ed2k://|file|x|1|h|/", "|file|x|1|h|"),
        ];
        for (url, expected) in cases {
            assert_eq!(host_of(url), expected, "host of {url:?}");
        }
    }

    #[test]
    fn parse_url_splits_scheme_host_path() {
        let parsed = parse_url("https://host.example/a/b?q=1#frag");
        assert_eq!(parsed.scheme, "https");
        assert_eq!(parsed.host, "host.example");
        assert_eq!(parsed.path, "/a/b");

        let parsed = parse_url("HTTP://Host.Example");
        assert_eq!(parsed.scheme, "http");
        assert_eq!(parsed.host, "host.example");
        assert_eq!(parsed.path, "");
    }

    #[test]
    fn parse_url_lowercases_path_as_documented() {
        // Python 保留 `/A.zip`；本模块降级成小写（两个消费者都只做小写比较）
        assert_eq!(parse_url("https://host/A.zip").path, "/a.zip");
    }

    #[test]
    fn parse_url_rejects_illegal_schemes() {
        // 前缀不是 ASCII 字母数字 / `+-.` → 整条当 path，scheme 与 host 都为空
        let parsed = parse_url("ht tp://host/x");
        assert_eq!(parsed.scheme, "");
        assert_eq!(parsed.host, "");
        assert_eq!(parsed.path, "ht tp://host/x");
    }
}
