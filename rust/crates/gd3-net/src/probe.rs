//! URL 探测：镜像 `features/http_pack/pack.py` 的 `probe`。
//!
//! 职责：探出文件大小、是否支持范围请求、文件名、content-type、last-modified。
//!
//! 两处刻意留在 Python 侧的东西：
//!
//! - **`guess_extension`**：Python 的 `mimetypes` 是**平台相关**的——Linux 上读
//!   `/etc/mime.types`（本机 1608 条），Windows 上读注册表，同一个 content-type 可能
//!   给出不同扩展名。所以由调用方传一个解析回调进来，表留在 Python。
//! - **兜底文件名**：Python 用 `f"file_{time_ns()}"`，本身就不可复现；由调用方传进来。

use crate::NetError;
use crate::engine::{PROBE_TIMEOUT, build_client};
use std::sync::Arc;
use wreq::header::{HeaderMap, HeaderName, HeaderValue};

/// 文件大小未知（Python `SpecialFileSize.UNKNOWN`）。
pub const SIZE_UNKNOWN: u64 = 0;

/// content-type → 扩展名（Python `mimetypes.guess_extension`）。
pub type ExtensionResolver = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFile {
    pub name: String,
    pub file_size: u64,
    pub can_use_range_requests: bool,
    pub last_modified: String,
    pub content_type: String,
}

#[derive(Debug, Clone)]
pub struct ProbeOptions {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub user_agent: String,
    /// 应用的 client profile 名，决定 TLS/HTTP2 指纹。
    pub client_profile: String,
    /// 名字彻底拿不到时用的兜底名（Python 传 `f"file_{time_ns()}"`）。
    pub fallback_name: String,
}

/// 一次探测请求的结果。
struct ProbeResponse {
    status: u16,
    headers: HeaderMap,
    final_url: String,
}

/// 取头部的文本值。
///
/// 非 ASCII 的头值 `to_str()` 会失败，而 Python 的 httpx 是按 latin-1 解的，
/// 所以这里也退回 latin-1 —— 否则同一份响应两侧会解析出不同的文件名。
fn header_text(value: &HeaderValue) -> String {
    match value.to_str() {
        Ok(text) => text.to_string(),
        Err(_) => value
            .as_bytes()
            .iter()
            .map(|byte| *byte as char)
            .collect::<String>(),
    }
}

impl ProbeResponse {
    /// 头名小写后的单值查询（Python 把 headers 转成小写 dict）。
    fn header(&self, name: &str) -> String {
        self.headers.get(name).map(header_text).unwrap_or_default()
    }
}

/// 从 `content-range` 里取总大小（Python `rangeTotal`）。
fn range_total(headers: &HeaderMap) -> u64 {
    let raw = headers
        .get("content-range")
        .map(header_text)
        .unwrap_or_default();
    if raw.is_empty() {
        return SIZE_UNKNOWN;
    }
    // Python 用 rpartition("/")，Rust 的 rsplit_once 只给两段
    let Some((_, total)) = raw.rsplit_once('/') else {
        return SIZE_UNKNOWN;
    };
    if total.is_empty() || total == "*" {
        return SIZE_UNKNOWN;
    }
    match total.trim().parse::<u64>() {
        Ok(size) if size > 0 => size,
        _ => SIZE_UNKNOWN,
    }
}

/// 从 `content-length` 里取大小（Python `bodyLength`）。
fn body_length(headers: &HeaderMap) -> u64 {
    let raw = headers
        .get("content-length")
        .map(header_text)
        .unwrap_or_default()
        .trim()
        .to_string();
    if raw.is_empty() {
        return SIZE_UNKNOWN;
    }
    match raw.parse::<u64>() {
        Ok(length) if length > 0 => length,
        _ => SIZE_UNKNOWN,
    }
}

fn has_header(headers: &HeaderMap, name: &str) -> bool {
    headers.contains_key(name)
}

fn to_header_map(headers: &[(String, String)]) -> Result<HeaderMap, NetError> {
    let mut map = HeaderMap::new();
    for (name, value) in headers {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(NetError::io)?;
        let value = HeaderValue::from_str(value).map_err(NetError::io)?;
        map.insert(name, value);
    }
    Ok(map)
}

/// 一次探测请求。非 200/206/416 直接报错（Python 的 `raise_for_status`）。
async fn request(
    client: &wreq::Client,
    options: &ProbeOptions,
    range_value: Option<&str>,
) -> Result<ProbeResponse, NetError> {
    let mut headers = options.headers.clone();
    headers.push(("accept-encoding".to_string(), "identity".to_string()));
    if let Some(range) = range_value {
        headers.push(("range".to_string(), range.to_string()));
    }
    if !options.user_agent.is_empty()
        && !headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("user-agent"))
    {
        headers.push(("user-agent".to_string(), options.user_agent.clone()));
    }

    let response = client
        .get(&options.url)
        .headers(to_header_map(&headers)?)
        .send()
        .await
        .map_err(NetError::io)?;

    let status = response.status().as_u16();
    if !matches!(status, 200 | 206 | 416) {
        return Err(NetError::server_error(status));
    }
    Ok(ProbeResponse {
        status,
        headers: response.headers().clone(),
        final_url: response.uri().to_string(),
    })
}

/// 镜像 `features/bili_pack/stream.py::probeSize`：
/// `bytes=0-0` + `accept-encoding: identity` 去 GET，取 `content-range` 的总数。
///
/// 与 Python 一致的宽松语义：**不检查状态码**（404 也照读头）、任何异常
/// （含网络错误、非法头名）都返回 0，绝不抛出。
pub async fn probe_size(client: &wreq::Client, headers: &[(String, String)], url: &str) -> u64 {
    // Python 用 `{**headers, range: ..., accept-encoding: identity}`，同名键被覆盖
    let mut merged: Vec<(String, String)> = headers.to_vec();
    merged.retain(|(name, _)| {
        !name.eq_ignore_ascii_case("range") && !name.eq_ignore_ascii_case("accept-encoding")
    });
    merged.push(("range".to_string(), "bytes=0-0".to_string()));
    merged.push(("accept-encoding".to_string(), "identity".to_string()));

    let map = match to_header_map(&merged) {
        Ok(map) => map,
        Err(_) => return 0,
    };
    match client.get(url).headers(map).send().await {
        Ok(response) => range_total(response.headers()),
        Err(_) => 0,
    }
}

/// 探测一个 URL。
pub async fn probe(
    options: &ProbeOptions,
    resolve_extension: Option<ExtensionResolver>,
) -> Result<RemoteFile, NetError> {
    let client = build_client(Some(PROBE_TIMEOUT), None, &options.client_profile);

    // ── 大小与范围支持 ──────────────────────────────────────
    let first = request(&client, options, Some("bytes=1-1")).await?;
    let mut file_size = range_total(&first.headers);
    let mut can_use_range_requests =
        first.status == 206 && has_header(&first.headers, "content-range");

    if !can_use_range_requests {
        file_size = body_length(&first.headers);

        // Python：200 且大小未知或为 1 时，再用 bytes=0-0 兜一次
        if first.status == 200 && (file_size == SIZE_UNKNOWN || file_size == 1) {
            let fallback = request(&client, options, Some("bytes=0-0")).await?;
            let fallback_size = range_total(&fallback.headers);
            if fallback.status == 206 && has_header(&fallback.headers, "content-range") {
                file_size = fallback_size;
                can_use_range_requests = true;
            } else if file_size == SIZE_UNKNOWN {
                file_size = body_length(&fallback.headers);
                if file_size == SIZE_UNKNOWN && fallback.status == 416 {
                    file_size = range_total(&fallback.headers);
                }
            }
        }
    }

    // ── 文件名：四级来源 ────────────────────────────────────
    let mut name = content_disposition_filename(&first.header("content-disposition"));

    if name.is_empty() && has_header(&first.headers, "content-location") {
        name = unquote(&url_path_last_segment(&first.header("content-location")));
    }

    if name.is_empty() {
        // 查询参数里的 response-content-disposition
        if let Some(value) = query_param(&first.final_url, "response-content-disposition") {
            if value.to_lowercase().contains("filename=") {
                if let Some(found) = extract_filename_param(&value) {
                    name = unquote(&found);
                }
            }
        }
    }

    if name.is_empty() {
        let path = url_path(&first.final_url);
        if !path.is_empty() && path.contains('/') {
            let clean = path.split(';').next().unwrap_or("");
            name = unquote(clean.rsplit('/').next().unwrap_or(""));
        }
    }

    // ── 扩展名兜底 ──────────────────────────────────────────
    let content_type = first
        .header("content-type")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();

    let standard_ext = if content_type.is_empty() {
        String::new()
    } else {
        resolve_extension
            .as_ref()
            .and_then(|resolve| resolve(&content_type))
            .unwrap_or_default()
    };

    if name.is_empty() {
        name = format!("{}{}", options.fallback_name, standard_ext);
    } else if !name.contains('.') && !standard_ext.is_empty() {
        name = format!("{name}{standard_ext}");
    }

    let name = gd3_core::to_safe_filename(
        &name,
        &options.fallback_name,
        gd3_core::DEFAULT_MAX_FILENAME_LENGTH,
    );

    Ok(RemoteFile {
        name,
        file_size,
        can_use_range_requests,
        last_modified: first.header("last-modified"),
        content_type,
    })
}

// ── content-disposition ─────────────────────────────────────

/// 从 `content-disposition` 里取文件名（镜像 Python 那段 email 解析）。
///
/// Python 走的是 `Message.get_params` + `collapse_rfc2231_value`，语义有几点不直觉
/// （都对着 `tests/fixtures/content_disposition.jsonl` 实测确认过）：
///
/// - `filename*`（RFC 2231）会**覆盖**同名的 `filename`，与出现顺序无关；
/// - 普通值会被 `unquote` **两次**（一次在 `collapse_rfc2231_value` 里，一次在调用处）；
/// - `filename*` 的值先百分号解码成「每字符一字节」的文本、再按 charset 解码；
/// - charset 不认识时退回「百分号解码后的原样文本」；
/// - 参数名大小写不敏感，同名的取**最后一个**。
pub fn content_disposition_filename(header: &str) -> String {
    if header.is_empty() {
        return String::new();
    }

    let mut plain: Option<String> = None;
    let mut encoded: Option<(String, String)> = None; // (charset, 已百分号解码的文本)

    for (key, value) in parse_params(header) {
        let lowered = key.to_lowercase();
        if let Some(base) = lowered.strip_suffix('*') {
            if base == "filename" {
                encoded = Some(split_rfc2231(&value));
            }
        } else if lowered == "filename" {
            plain = Some(value);
        }
    }

    let text = match encoded {
        Some((charset, decoded)) => decode_with_charset(&charset, &decoded),
        None => match plain {
            Some(value) => unquote(&value),
            None => return String::new(),
        },
    };

    // 调用处还会再 unquote 一次，并去掉包裹的引号与空格
    unquote(&text)
        .trim_matches(|ch| ch == '"' || ch == '\'' || ch == ' ')
        .to_string()
}

/// 按 `;` 拆参数，尊重引号；返回 `(键, 值)`（值已去掉包裹引号）。
fn parse_params(header: &str) -> Vec<(String, String)> {
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in header.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                current.push(ch);
            }
            ';' if !in_quotes => parts.push(std::mem::take(&mut current)),
            _ => current.push(ch),
        }
    }
    parts.push(current);

    // 第一段是 disposition 类型，跳过
    parts
        .into_iter()
        .skip(1)
        .filter_map(|part| {
            let (key, value) = part.split_once('=')?;
            let key = key.trim().to_string();
            if key.is_empty() {
                return None;
            }
            let value = value.trim();
            let unquoted = if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
                unescape_quoted(&value[1..value.len() - 1])
            } else {
                value.to_string()
            };
            Some((key, unquoted))
        })
        .collect()
}

/// 处理引号里的转义。
///
/// 只把 `\"` 还原成 `"`——Python 的 email 解析就是这样：`"a\"b.mp4"` → `a"b.mp4`，
/// 而 `"..\..\evil.mp4"` 里的反斜杠**原样保留**（实测确认，别用「去掉所有反斜杠」的写法）。
fn unescape_quoted(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut out = String::with_capacity(value.len());
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '\\' && index + 1 < chars.len() && chars[index + 1] == '"' {
            out.push('"');
            index += 2;
            continue;
        }
        out.push(chars[index]);
        index += 1;
    }
    out
}

/// 拆 `charset'language'value`；不是这个形状就当作普通文本（charset 为空）。
fn split_rfc2231(value: &str) -> (String, String) {
    let mut pieces = value.splitn(3, '\'');
    let charset = pieces.next().unwrap_or("");
    match (pieces.next(), pieces.next()) {
        (Some(_language), Some(rest)) => (charset.to_string(), percent_decode_to_text(rest)),
        _ => (String::new(), percent_decode_to_text(value)),
    }
}

/// 按 charset 把「百分号解码出来的字节」还原成文本。
///
/// charset 不认识时退回原样文本（Python 的 `except LookupError: return unquote(text)`）。
fn decode_with_charset(charset: &str, decoded: &str) -> String {
    let bytes: Vec<u8> = decoded.chars().map(|ch| ch as u32 as u8).collect();
    match charset.to_ascii_lowercase().as_str() {
        "" | "us-ascii" | "ascii" | "utf-8" | "utf8" => {
            String::from_utf8_lossy(&bytes).into_owned()
        }
        "iso-8859-1" | "latin-1" | "latin1" | "l1" | "cp819" | "iso8859-1" => {
            bytes.iter().map(|byte| *byte as char).collect()
        }
        _ => decoded.to_string(),
    }
}

/// `urllib.parse.unquote`：只解 `%XX`，非法序列原样保留，结果按 UTF-8 解码（失败则替换）。
fn unquote(value: &str) -> String {
    let bytes = percent_decode_bytes(value);
    String::from_utf8_lossy(&bytes).into_owned()
}

/// 百分号解码成字节，非法 `%XX` 原样保留。
fn percent_decode_bytes(value: &str) -> Vec<u8> {
    let raw = value.as_bytes();
    let mut out = Vec::with_capacity(raw.len());
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'%' && index + 2 < raw.len() {
            let high = (raw[index + 1] as char).to_digit(16);
            let low = (raw[index + 2] as char).to_digit(16);
            if let (Some(high), Some(low)) = (high, low) {
                out.push((high * 16 + low) as u8);
                index += 3;
                continue;
            }
        }
        out.push(raw[index]);
        index += 1;
    }
    out
}

/// 解码后仍按「每个字符一个字节」表示（给 RFC 2231 那条路径用）。
fn percent_decode_to_text(value: &str) -> String {
    percent_decode_bytes(value)
        .iter()
        .map(|byte| *byte as char)
        .collect()
}

// ── URL 小工具 ──────────────────────────────────────────────

/// 取 URL 的 path 部分（含开头的 `/`，去掉 query/fragment）。
pub fn url_path(url: &str) -> String {
    let after_scheme = match url.find("://") {
        Some(position) => &url[position + 3..],
        None => url,
    };
    let start = after_scheme.find('/').unwrap_or(after_scheme.len());
    let rest = &after_scheme[start..];
    let end = rest.find(['?', '#']).unwrap_or(rest.len());
    rest[..end].to_string()
}

fn url_path_last_segment(url: &str) -> String {
    url_path(url).rsplit('/').next().unwrap_or("").to_string()
}

/// 取查询参数（Python `parse_qs(...).get(name, [""])[0]`）。
pub fn query_param(url: &str, name: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    let query = query.split('#').next().unwrap_or(query);
    for pair in query.split('&') {
        let (key, value) = match pair.split_once('=') {
            Some(parts) => parts,
            None => continue,
        };
        // Python 的 parse_qs 对键和值都做 unquote_plus（`+` 当空格 + 百分号解码）
        if unquote_plus(key) == name {
            return Some(unquote_plus(value));
        }
    }
    None
}

/// `urllib.parse.unquote_plus`：先把 `+` 当空格，再百分号解码。
fn unquote_plus(value: &str) -> String {
    unquote(&value.replace('+', " "))
}

/// Python `re.search(r'filename\s*=\s*["\']?([^"\';]+)["\']?', rcd, re.IGNORECASE)` 的等价实现。
fn extract_filename_param(text: &str) -> Option<String> {
    let lowered = text.to_lowercase();
    let mut search_from = 0;
    while let Some(offset) = lowered[search_from..].find("filename") {
        let start = search_from + offset + "filename".len();
        let rest_trimmed = text[start..].trim_start();
        if let Some(after_eq) = rest_trimmed.strip_prefix('=') {
            let after_eq = after_eq.trim_start();
            let after_eq = after_eq
                .strip_prefix('"')
                .or_else(|| after_eq.strip_prefix('\''))
                .unwrap_or(after_eq);
            let end = after_eq.find(['"', '\'', ';']).unwrap_or(after_eq.len());
            let value = &after_eq[..end];
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
        search_from = start;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_disposition_matches_python_fixture() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/content_disposition.jsonl");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("读金文件失败 {}：{error}", path.display()));

        let mut checked = 0;
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let case: serde_json::Value =
                serde_json::from_str(line).expect("金文件行不是合法 JSON");
            let cd = case["cd"].as_str().expect("cd");
            let expected = case["name"].as_str().expect("name");
            assert_eq!(
                content_disposition_filename(cd),
                expected,
                "content-disposition 解析与 Python 不一致：{cd:?}"
            );
            checked += 1;
        }
        assert!(checked >= 35, "金文件里的用例太少：{checked}");
    }

    #[test]
    fn range_total_and_body_length_follow_python() {
        let mut headers = HeaderMap::new();
        headers.insert("content-range", HeaderValue::from_static("bytes 1-1/12345"));
        assert_eq!(range_total(&headers), 12345);

        headers.insert("content-range", HeaderValue::from_static("bytes 1-1/*"));
        assert_eq!(range_total(&headers), SIZE_UNKNOWN);

        headers.insert("content-range", HeaderValue::from_static("bytes 1-1/0"));
        assert_eq!(range_total(&headers), SIZE_UNKNOWN);

        headers.insert("content-range", HeaderValue::from_static("garbage"));
        assert_eq!(range_total(&headers), SIZE_UNKNOWN);

        let mut headers = HeaderMap::new();
        headers.insert("content-length", HeaderValue::from_static("4096"));
        assert_eq!(body_length(&headers), 4096);

        headers.insert("content-length", HeaderValue::from_static("0"));
        assert_eq!(body_length(&headers), SIZE_UNKNOWN);

        headers.insert("content-length", HeaderValue::from_static(" 2048 "));
        assert_eq!(body_length(&headers), 2048);
    }

    #[test]
    fn url_helpers_follow_python() {
        assert_eq!(url_path("https://a.test/x/y.mp4?q=1"), "/x/y.mp4");
        assert_eq!(url_path("https://a.test/x;y.mp4"), "/x;y.mp4");
        assert_eq!(url_path("https://a.test"), "");
        assert_eq!(
            query_param("https://a.test/f?b=2&a=1", "a"),
            Some("1".into())
        );
        assert_eq!(
            query_param("https://a.test/f?a=x+y", "a"),
            Some("x y".into())
        );
        assert_eq!(query_param("https://a.test/f", "a"), None);
        assert_eq!(
            extract_filename_param("attachment; filename=\"a.mp4\""),
            Some("a.mp4".into())
        );
        assert_eq!(extract_filename_param("attachment; nope=1"), None);
    }
}
