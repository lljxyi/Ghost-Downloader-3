//! HTTP/1.1 头部解析与响应构造：镜像 `app/services/aria2_rpc.py` 的 `parseHead` / `buildHttpResponse`。

use std::collections::HashMap;

/// 请求头最大字节数（Python `websocket_stream.MAX_HEAD_SIZE`）。
pub const MAX_HEAD_SIZE: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    /// 已去掉 query 与 fragment。
    pub path: String,
    /// 键名小写。
    pub headers: HashMap<String, String>,
}

impl HttpRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    pub fn has_header(&self, name: &str) -> bool {
        self.headers.contains_key(name)
    }
}

/// 解析请求头；格式不对返回 `None`（Python 侧同样返回 `None`，调用方直接断开）。
pub fn parse_head(head: &[u8]) -> Option<HttpRequest> {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.split("\r\n");

    let request_line = lines.next()?;
    let parts: Vec<&str> = request_line.split(' ').collect();
    if parts.len() != 3 {
        return None;
    }

    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = match line.split_once(':') {
            Some((name, value)) => (name, value),
            None => continue,
        };
        headers.insert(name.trim().to_lowercase(), value.trim().to_string());
    }

    let path = parts[1]
        .split('?')
        .next()
        .unwrap_or(parts[1])
        .split('#')
        .next()
        .unwrap_or(parts[1])
        .to_string();

    Some(HttpRequest {
        method: parts[0].to_string(),
        path,
        headers,
    })
}

/// 状态码对应的原因短语（HTTP/1.1 全集，取 Python `http.HTTPStatus` 的同一套）。
pub fn reason_phrase(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        101 => "Switching Protocols",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Payload Too Large",
        416 => "Range Not Satisfiable",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Unknown",
    }
}

/// 构造响应；`Connection: close` 与 `Access-Control-Allow-Origin: *` 与 Python 一致。
pub fn build_response(status: u16, body: &[u8], headers: &[(&str, String)]) -> Vec<u8> {
    let mut head = format!(
        "HTTP/1.1 {status} {}\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\nConnection: close\r\n",
        reason_phrase(status),
        body.len()
    );
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");

    let mut out = head.into_bytes();
    out.extend_from_slice(body);
    out
}

/// CORS 预检头：只有同时带 `origin` 与 `access-control-request-method` 才回（Python `buildPreflightHeaders`）。
pub fn build_preflight_headers(request: &HttpRequest) -> Vec<(&'static str, String)> {
    if !request.has_header("origin") || !request.has_header("access-control-request-method") {
        return Vec::new();
    }
    vec![
        (
            "Access-Control-Allow-Methods",
            "POST, GET, OPTIONS".to_string(),
        ),
        (
            "Access-Control-Allow-Headers",
            request
                .header("access-control-request-headers")
                .unwrap_or("")
                .to_string(),
        ),
        ("Access-Control-Max-Age", "1728000".to_string()),
    ]
}

/// 从 `Range: bytes=a-b` 解出闭区间；不合法返回 `None`。
pub fn parse_byte_range(value: &str) -> Option<(u64, Option<u64>)> {
    let spec = value.strip_prefix("bytes=")?;
    let (start, end) = spec.split_once('-')?;
    let start: u64 = start.trim().parse().ok()?;
    let end = match end.trim() {
        "" => None,
        text => Some(text.parse().ok()?),
    };
    Some((start, end))
}
