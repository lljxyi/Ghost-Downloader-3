//! aria2 JSON-RPC 兼容层：镜像 `app/services/aria2_rpc.py`。
//!
//! 只是最小兼容协议适配器，行为以 aria2 原始实现为准。协议解析与校验是纯函数
//! （`Aria2Service::dispatch`），需要应用做的事（解析并加入任务）以 `Aria2Action`
//! 交回调用方——这样这块能独立测试，也不把领域逻辑塞进协议层。

use crate::http::{build_preflight_headers, build_response, parse_head};
use crate::websocket::{WebSocket, is_upgrade, read_head};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub const JSONRPC_PARSE_ERROR: i64 = -32700;
pub const JSONRPC_INVALID_REQUEST: i64 = -32600;
pub const JSONRPC_INVALID_PARAMS: i64 = -32602;
pub const ARIA2_ERROR: i64 = 1;
/// 请求体上限（Python `MAX_REQUEST_SIZE`）。
pub const MAX_REQUEST_SIZE: usize = 2 * 1024 * 1024;
/// 未授权时的延迟（Python `UNAUTHORIZED_DELAY`）。
pub const UNAUTHORIZED_DELAY: Duration = Duration::from_secs(1);

/// 解析任务的输入（CONTEXT.md 的 Task Options 里 aria2 能提供的那部分）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Aria2TaskOptions {
    pub url: String,
    pub output_folder: String,
    /// 保留顺序与原始大小写。
    pub headers: Vec<(String, String)>,
    /// `""` 跟随全局指纹设置；`"raw"` 不做模拟（`aria2RpcEmulateFingerprint` 关掉时）。
    pub client_profile: String,
    /// `out` 选项给出的 Task Name。
    pub name: String,
}

/// 协议层交给应用层的后续动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Aria2Action {
    /// 解析并加入任务；`gid` 已先行返回给调用方。
    AddTask {
        gid: String,
        options: Aria2TaskOptions,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Aria2Config {
    pub version: String,
    pub download_folder: String,
    pub token: String,
    pub emulate_fingerprint: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Aria2Response {
    pub status: u16,
    pub body: Value,
    pub action: Option<Aria2Action>,
    /// 返回前要先等这么久（未授权时是 1 秒，与 aria2 一致）。
    pub delay: Duration,
}

impl Aria2Response {
    fn error(id: Value, code: i64, message: &str) -> Self {
        Aria2Response {
            status: to_http_status(code),
            body: json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}),
            action: None,
            delay: Duration::ZERO,
        }
    }

    fn result(id: Value, result: Value) -> Self {
        Aria2Response {
            status: 200,
            body: json!({"jsonrpc": "2.0", "id": id, "result": result}),
            action: None,
            delay: Duration::ZERO,
        }
    }
}

/// Python `toHttpStatus`：aria2 自身错误与非法请求是 400，其它（含解析错误）是 500。
pub fn to_http_status(code: i64) -> u16 {
    if code == ARIA2_ERROR || code == JSONRPC_INVALID_REQUEST {
        400
    } else {
        500
    }
}

/// 16 位十六进制 gid（Python `token_hex(8)`）。
pub fn new_gid() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or_default();
    let mut state = now ^ COUNTER.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);

    let mut out = String::with_capacity(16);
    for _ in 0..2 {
        // splitmix64
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        out.push_str(&format!("{z:016x}"));
    }
    out.truncate(16);
    out
}

/// `header` 选项：单个字符串或字符串数组，逐行 `Name: value`；再补 user-agent / referer。
pub fn to_headers(options: &Value) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = Vec::new();

    let lines: Vec<&str> = match options.get("header") {
        Some(Value::String(line)) => vec![line.as_str()],
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }

    let user_agent = options
        .get("user-agent")
        .and_then(Value::as_str)
        .unwrap_or("");
    let referer = options.get("referer").and_then(Value::as_str).unwrap_or("");
    let has_user_agent = headers
        .iter()
        .any(|(key, _)| key.eq_ignore_ascii_case("user-agent"));
    let has_referer = headers
        .iter()
        .any(|(key, _)| key.eq_ignore_ascii_case("referer"));

    if !user_agent.is_empty() && !has_user_agent {
        headers.push(("User-Agent".to_string(), user_agent.to_string()));
    }
    if !referer.is_empty() && !has_referer {
        headers.push(("Referer".to_string(), referer.to_string()));
    }
    headers
}

/// 按 Python `toText` 的语义取字符串选项：类型不对就当空。
fn to_text(options: &Value, key: &str) -> String {
    options
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

pub struct Aria2Service {
    /// 配置是动态的（用户改设置后立刻生效），所以每次请求都取一次。
    config: Arc<dyn Fn() -> Aria2Config + Send + Sync>,
    on_action: Arc<dyn Fn(Aria2Action) + Send + Sync>,
}

impl Aria2Service {
    pub fn new(
        config: Arc<dyn Fn() -> Aria2Config + Send + Sync>,
        on_action: Arc<dyn Fn(Aria2Action) + Send + Sync>,
    ) -> Self {
        Aria2Service { config, on_action }
    }

    /// 协议分发：纯函数，不碰 socket。
    pub fn dispatch(&self, body: &[u8]) -> Aria2Response {
        let config = (self.config)();

        let data: Value = match serde_json::from_slice(body) {
            Ok(data) => data,
            Err(_) => {
                return Aria2Response::error(Value::Null, JSONRPC_PARSE_ERROR, "Parse error.");
            }
        };
        let Some(object) = data.as_object() else {
            return Aria2Response::error(Value::Null, JSONRPC_INVALID_REQUEST, "Invalid Request.");
        };

        let id = object.get("id").cloned().unwrap_or(Value::Null);
        let method = object.get("method").and_then(Value::as_str);
        if !object.contains_key("id") || method.is_none() {
            return Aria2Response::error(id, JSONRPC_INVALID_REQUEST, "Invalid Request.");
        }
        let method = method.expect("已确认是字符串");

        let empty = Value::Array(Vec::new());
        let params = match object.get("params") {
            None => &empty,
            Some(params) if params.is_array() => params,
            Some(_) => {
                return Aria2Response::error(id, JSONRPC_INVALID_PARAMS, "Invalid params.");
            }
        };
        let mut params: Vec<Value> = params.as_array().expect("已确认是数组").clone();

        // 首参若是 "token:xxx" 就取出来，其余参数顺延
        let mut token = String::new();
        if let Some(first) = params.first().and_then(Value::as_str)
            && let Some(rest) = first.strip_prefix("token:")
        {
            token = rest.to_string();
            params.remove(0);
        }
        if !config.token.is_empty() && token != config.token {
            let mut response = Aria2Response::error(id, ARIA2_ERROR, "Unauthorized");
            response.delay = UNAUTHORIZED_DELAY;
            return response;
        }

        match method {
            "aria2.getVersion" => Aria2Response::result(
                id,
                json!({"version": config.version, "enabledFeatures": ["HTTPS"]}),
            ),
            "aria2.getGlobalOption" => {
                Aria2Response::result(id, json!({"dir": config.download_folder}))
            }
            "aria2.addUri" => match add_uri(&params, &config) {
                Ok(response) => response,
                Err(message) => Aria2Response::error(id, ARIA2_ERROR, &message),
            },
            other => Aria2Response::error(id, ARIA2_ERROR, &format!("No such method: {other}")),
        }
    }

    /// 处理一条连接（PortListener 的 handle）。
    pub async fn handle(&self, mut socket: TcpStream) {
        let Ok(Some(head)) = read_head(&mut socket, crate::http::MAX_HEAD_SIZE).await else {
            return;
        };
        let Some(request) = parse_head(&head) else {
            return;
        };

        // OPTIONS 先于路径判断（与 Python 一致）
        if request.method == "OPTIONS" {
            drain_body(&mut socket, &request.headers).await;
            let headers = build_preflight_headers(&request);
            let _ = socket.write_all(&build_response(200, b"", &headers)).await;
            let _ = socket.flush().await;
            return;
        }
        if request.path != "/jsonrpc" {
            drain_body(&mut socket, &request.headers).await;
            let _ = socket.write_all(&build_response(404, b"", &[])).await;
            let _ = socket.flush().await;
            return;
        }
        if request.method == "GET" && is_upgrade(&request.headers) {
            self.run_websocket(socket, &request.headers).await;
            return;
        }
        if request.method == "POST" {
            self.run_http(socket, &request.headers).await;
            return;
        }
        let _ = socket.write_all(&build_response(404, b"", &[])).await;
        let _ = socket.flush().await;
    }
    async fn run_http(&self, mut socket: TcpStream, headers: &HashMap<String, String>) {
        let length = if headers.contains_key("transfer-encoding") {
            0
        } else {
            match headers
                .get("content-length")
                .map(String::as_str)
                .unwrap_or("0")
                .parse::<usize>()
            {
                Ok(length) => length,
                Err(_) => {
                    finish(&mut socket).await;
                    return;
                }
            }
        };
        // 超限直接丢弃，不回响应（与 Python 一致）
        if length > MAX_REQUEST_SIZE {
            finish(&mut socket).await;
            return;
        }

        let mut body = vec![0u8; length];
        if socket.read_exact(&mut body).await.is_err() {
            return;
        }

        let mut response = self.dispatch(&body);
        if !response.delay.is_zero() {
            tokio::time::sleep(response.delay).await;
        }
        if let Some(action) = response.action.take() {
            (self.on_action)(action);
        }

        let payload = serde_json::to_vec(&response.body).unwrap_or_default();
        let _ = socket
            .write_all(&build_response(
                response.status,
                &payload,
                &[("Content-Type", "application/json-rpc".to_string())],
            ))
            .await;
        let _ = socket.flush().await;
    }

    async fn run_websocket(&self, socket: TcpStream, headers: &HashMap<String, String>) {
        let Ok(Some(mut websocket)) = WebSocket::accept(socket, headers, MAX_REQUEST_SIZE).await
        else {
            return;
        };

        while let Some(message) = websocket.recv().await {
            let mut response = self.dispatch(message.as_bytes());
            if !response.delay.is_zero() {
                tokio::time::sleep(response.delay).await;
            }
            if let Some(action) = response.action.take() {
                (self.on_action)(action);
            }
            let payload = serde_json::to_string(&response.body).unwrap_or_default();
            if websocket.send(&payload).await.is_err() {
                return;
            }
        }
        websocket.close().await;
    }
}

fn add_uri(params: &[Value], config: &Aria2Config) -> Result<Aria2Response, String> {
    let Some(first) = params.first() else {
        return Err("The parameter at 0 is required but missing.".to_string());
    };
    let Some(uris) = first.as_array() else {
        return Err("The parameter at 0 has wrong type.".to_string());
    };
    let Some(uri) = uris.iter().find_map(Value::as_str) else {
        return Err("URI is not provided.".to_string());
    };

    let empty = json!({});
    let options = match params.get(1) {
        None => &empty,
        Some(options) if options.is_object() => options,
        Some(_) => return Err("The parameter at 1 has wrong type.".to_string()),
    };

    let directory = to_text(options, "dir");
    let gid = new_gid();
    let action = Aria2Action::AddTask {
        gid: gid.clone(),
        options: Aria2TaskOptions {
            url: uri.to_string(),
            output_folder: if directory.is_empty() {
                config.download_folder.clone()
            } else {
                directory
            },
            headers: to_headers(options),
            client_profile: if config.emulate_fingerprint {
                String::new()
            } else {
                "raw".to_string()
            },
            name: to_text(options, "out"),
        },
    };

    Ok(Aria2Response {
        status: 200,
        body: json!({"jsonrpc": "2.0", "id": Value::Null, "result": gid}),
        action: Some(action),
        delay: Duration::ZERO,
    })
}

/// 读掉请求体再响应。Windows 上带着未读数据关闭套接字会给对端 RST，
/// 客户端会丢掉已经收到的响应——所以不走 body 的分支也要先排空。
async fn drain_body(socket: &mut TcpStream, headers: &HashMap<String, String>) {
    if headers.contains_key("transfer-encoding") {
        return;
    }
    let Ok(length) = headers
        .get("content-length")
        .map(String::as_str)
        .unwrap_or("0")
        .parse::<usize>()
    else {
        return;
    };
    if length == 0 || length > MAX_REQUEST_SIZE {
        return;
    }
    let mut body = vec![0u8; length];
    let _ = socket.read_exact(&mut body).await;
}

/// 提前放弃请求前的收尾：把对端已发出的字节读掉再优雅关闭。
///
/// Windows 上带着未读数据 `closesocket` 会给对端 RST，客户端会把已经收到的数据
/// 一起丢掉（超限请求的「不回响应」就变成「连接重置」）。先排空再 shutdown 才是干净 FIN。
async fn finish(socket: &mut TcpStream) {
    let mut buffer = vec![0u8; 16 * 1024];
    loop {
        match tokio::time::timeout(Duration::from_millis(100), socket.read(&mut buffer)).await {
            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => break,
            Ok(Ok(_)) => continue,
        }
    }
    let _ = socket.shutdown().await;
}
