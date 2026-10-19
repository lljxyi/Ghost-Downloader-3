//! GitHub pack 的判定层（镜像 `features/github_pack/pack.py::isGitHubFileUrl`、
//! `features/github_pack/probe.py::matchFile` / `toProxyHeaders`）。
//!
//! 纯解析、无网络、无状态：输入 URL / 头列表 / RemoteFile 字段，输出与 Python
//! 实现逐断言一致。选串行三件套一起搬是因为它们共享 `is_github_file_url`
//! 的 URL 解析细节，拆开反而容易漂移。
//!
//! 跟 Python 的对应关系（照 `urlparse` 的现状抄，别去"修正"）：
//! - `hostname` 是 lowercase、去掉端口与 userinfo 的 host；
//! - `path` 不含 query / fragment；
//! - scheme 必须是 `http` / `https` 且 host 非空才可能命中。
//!
//! URL 解析本身在 `crate::url`（第 108 步抽出：`http_pack` 与 `gd3-ui` 的
//! 身份预设前置共用同一份 `urlparse` 子集）。

use crate::url::parse_url;

const GITHUB_HOSTS: [&str; 10] = [
    "api.github.com",
    "codeload.github.com",
    "gist.github.com",
    "gist.githubusercontent.com",
    "github-releases.githubusercontent.com",
    "media.githubusercontent.com",
    "objects.githubusercontent.com",
    "raw.githubusercontent.com",
    "raw.github.com",
    "release-assets.githubusercontent.com",
];

fn strip_www(host: &str) -> &str {
    host.strip_prefix("www.").unwrap_or(host)
}

/// 镜像 `GitHubParser.match` 使用的判定：`isGitHubFileUrl(url)`。
pub fn is_github_file_url(url: &str) -> bool {
    let parsed = parse_url(url);
    if parsed.scheme != "http" && parsed.scheme != "https" {
        return false;
    }
    let host = strip_www(&parsed.host);
    let is_github = host == "github.com"
        && (parsed.path.contains("/archive/")
            || parsed.path.contains("/raw/")
            || parsed.path.contains("/releases/download/")
            || parsed.path.contains("/releases/latest/download/"));
    !host.is_empty() && (GITHUB_HOSTS.contains(&host) || is_github)
}

/// 镜像 `toProxyHeaders(headers)`：小写化比较，去掉 `authorization` / `cookie`，保序。
pub fn proxy_headers(headers: &[(String, String)]) -> Vec<(String, String)> {
    headers
        .iter()
        .filter(|(name, _)| {
            let lower = name.to_ascii_lowercase();
            lower != "authorization" && lower != "cookie"
        })
        .cloned()
        .collect()
}

/// 镜像 `matchFile(originalUrl, file)`。
///
/// `content_type` 是 probe 侧已归一的值（`content-type` 去掉 `;` 参数、lowercase、trim）。
pub fn match_file(
    original_url: &str,
    content_type: &str,
    can_use_range_requests: bool,
    file_size: u64,
) -> bool {
    let parsed = parse_url(original_url);
    let is_archive = parsed.host == "codeload.github.com" || parsed.path.contains("/archive/");
    if is_archive {
        return !content_type.starts_with("text/");
    }
    can_use_range_requests && file_size > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASSET_URL: &str = "https://github.com/o/r/releases/download/v1/a.tar.gz";
    const RAW_URL: &str = "https://raw.githubusercontent.com/o/r/main/README.md";
    const ARCHIVE_URL: &str = "https://github.com/o/r/archive/refs/heads/main.zip";
    const CODELOAD_URL: &str = "https://codeload.github.com/o/r/zip/refs/heads/main";

    fn is_match(
        original_url: &str,
        file_size: u64,
        can_use_range_requests: bool,
        content_type: &str,
    ) -> bool {
        match_file(
            original_url,
            content_type,
            can_use_range_requests,
            file_size,
        )
    }

    #[test]
    fn match_file_mirrors_python_table() {
        assert!(is_match(ASSET_URL, 100, true, ""));

        // RAW_URL：GITHUB_HOSTS 命中，path 不含 /archive/，走 range 分支
        assert!(is_match(RAW_URL, 100, true, "text/plain"));

        assert!(!is_match(ASSET_URL, 100, false, ""));
        assert!(!is_match(ASSET_URL, 0, true, ""));

        // archive 分支：codeload / /archive/ 只看 content_type
        assert!(is_match(ARCHIVE_URL, 0, false, "application/zip"));
        assert!(is_match(CODELOAD_URL, 0, false, "application/zip"));
        assert!(is_match(ARCHIVE_URL, 100, true, "application/zip"));
        assert!(!is_match(ARCHIVE_URL, 0, false, "text/html"));
        assert!(!is_match(ARCHIVE_URL, 0, false, "text/plain"));
    }

    #[test]
    fn is_github_file_url_mirrors_python() {
        for url in [
            ASSET_URL,
            RAW_URL,
            "https://api.github.com/repos/o/r",
            "https://github.com/o/r/raw/main/x.bin",
            "https://github.com/o/r/releases/latest/download/a.zip",
            "http://raw.githubusercontent.com/o/r/main/a",
            "https://www.github.com/o/r/archive/main.zip",
        ] {
            assert!(is_github_file_url(url), "should match: {url}");
        }
        for url in [
            "https://example.com/a.zip",
            "ftp://github.com/o/r/releases/download/v1/a",
            "https://gitlab.com/o/r/releases/download/v1/a",
            "https://github.com/o/r/tree/main",
            "github.com/o/r/releases/download/v1/a",
            "",
        ] {
            assert!(!is_github_file_url(url), "should not match: {url}");
        }
    }

    #[test]
    fn proxy_headers_only_drops_credentials() {
        let headers = vec![
            ("Authorization".to_string(), "token t".to_string()),
            ("Cookie".to_string(), "c=1".to_string()),
            ("X-Other".to_string(), "1".to_string()),
        ];
        assert_eq!(
            proxy_headers(&headers),
            vec![("X-Other".to_string(), "1".to_string())]
        );
    }
}
