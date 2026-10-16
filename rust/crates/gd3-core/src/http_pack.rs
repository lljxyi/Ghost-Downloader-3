//! http_pack 的判定层（镜像 `features/http_pack/pack.py::HttpParser.match` /
//! `HttpParser.matchPassive`）。
//!
//! 纯解析、无网络、无状态：`match` 只看 URL scheme 是否为 `http` / `https`；
//! `matchPassive` 再要求 lowercase 之后的 path 以 `DOWNLOADABLE_EXTENSIONS` 里的
//! 某个扩展名结尾。
//!
//! URL 解析复用 `github::parse_url` 的极简 `urlparse` 子集，语义对 Python 现状照抄
//! （没有 `://` 就没有 scheme；scheme 只认 ASCII 字母数字与 `+. -`）。

use super::github::parse_url;

/// 镜像 `DOWNLOADABLE_EXTENSIONS`（`pack.py` 顶部的一份 frozenset，顺序无关紧要：
/// `matchPassive` 只问「是否以其中之一结尾」）。
const DOWNLOADABLE_EXTENSIONS: [&str; 38] = [
    ".mp4",
    ".mkv",
    ".avi",
    ".mov",
    ".wmv",
    ".flv",
    ".webm",
    ".ts",
    ".m2ts",
    ".mp3",
    ".flac",
    ".wav",
    ".aac",
    ".ogg",
    ".m4a",
    ".wma",
    ".opus",
    ".zip",
    ".rar",
    ".7z",
    ".tar",
    ".gz",
    ".bz2",
    ".xz",
    ".zst",
    ".exe",
    ".msi",
    ".dmg",
    ".deb",
    ".rpm",
    ".appimage",
    ".pkg",
    ".iso",
    ".img",
    ".pdf",
    ".epub",
    ".apk",
    ".ipa",
];

/// 镜像 `HttpParser.match`：`urlparse(options.url).scheme.lower() in {"http","https"}`。
pub fn parser_match(url: &str) -> bool {
    let parsed = parse_url(url);
    parsed.scheme == "http" || parsed.scheme == "https"
}

/// 镜像 `HttpParser.matchPassive`：scheme 合格 + lowercase path 以可下载扩展名结尾。
pub fn parser_match_passive(url: &str) -> bool {
    if !parser_match(url) {
        return false;
    }
    let parsed = parse_url(url);
    DOWNLOADABLE_EXTENSIONS
        .iter()
        .any(|ext| parsed.path.ends_with(ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_follows_urlparse_scheme_semantics() {
        // 正常命中
        assert!(parser_match("http://a/b"));
        assert!(parser_match("HTTP://a/"));
        assert!(parser_match("hTtPs://a/b.mp4"));
        // 其它 scheme
        assert!(!parser_match("ftp://a/b"));
        // 无冒号 → scheme 为空
        assert!(!parser_match("a.mp4"));
        assert!(!parser_match(""));
        // 非法scheme 前缀（含非 ASCII）→ scheme 为空（与 Python `文件://a` 一致）
        assert!(!parser_match("文件://a/b.mp4"));
        // 有 scheme 无 netloc
        assert!(parser_match("http:/a"));
        // 只对 match 有效：scheme 合格但扩展名不在表里
        assert!(!parser_match_passive("http://a/b"));
        assert!(parser_match_passive("http://a/b.mp4"));
        assert!(parser_match_passive("HTTPS://A.COM/x.TS?y=1"));
        // 分号属于 path：'/b.mp4;' 不以 '.mp4' 结尾（Python 同样为 False）
        assert!(!parser_match_passive("https://a/b.mp4;"));
    }
}
