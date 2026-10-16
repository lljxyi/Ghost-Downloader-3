// 自包含的本地 HTTP/1.1 测试服务器 / Self-contained local HTTP/1.1 test server.
//
// 用 `tokio::net::TcpListener` 手写，不依赖 hyper/axum/reqwest，
// 只依赖 tokio（rt-multi-thread, macros, net, io-util, time, sync）。
//
// Hand-rolled on top of `tokio::net::TcpListener`; the only dependency is tokio.
// Intended for gd3-net tests: it records every request, can switch response
// modes at runtime and stops as soon as the `TestServer` value is dropped.
//
// 顶层用普通注释而不是 `//!`，这样 `mod test_server;`、`#[path = "..."] mod
// test_server;` 和 `include!(...)` 三种引入方式都能编译。
// Plain `//` header (not `//!`) so `mod test_server;`, `#[path = "..."] mod
// test_server;` and `include!(...)` all compile.
//
// 用法 / usage:
//     #[path = "support/test_server.rs"]
//     mod test_server;
//
//     let server = test_server::TestServer::start(content).await;
//     let resp = reqwest::get(server.url("/file.bin")).await.unwrap();

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// 单个连接最多读取的请求头字节数 / max request-head bytes read from one connection.
const MAX_HEAD_BYTES: usize = 64 * 1024;

/// `Stall` 模式先写出的字节数 / bytes written before `Stall` goes quiet.
const STALL_PREFIX_BYTES: usize = 64;

/// `Stall` 模式保持连接不结束的时长 / how long `Stall` keeps the socket open.
const STALL_HOLD: Duration = Duration::from_secs(3600);

/// 响应模式 / response mode of the test server.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// 支持 Range：206 + Content-Range
    Range,
    /// 忽略 Range：总是 200 + 全量
    IgnoreRange,
    /// 403
    Forbidden,
    /// 前 N 个请求回 500，之后按 Range 正常服务
    FailTimes(u32),
    /// 发头部与少量数据后长时间不结束
    Stall,
    /// 按 Range 服务，但正文按 `piece` 字节一片慢慢发、每片之间停 `gap`。
    /// 用来证明读超时是「空闲超时」而不是「总超时」。
    Slow { piece: usize, gap: Duration },
    /// 脚本化：第 N 个请求用第 N 步，用完后重复最后一步。
    Script(Vec<Step>),
}

/// `Mode::Script` 里的一步 / one scripted response step.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// 206 + Content-Range（按请求区间；区间不可解析则 200 全量）
    Ranged,
    /// 200 + 全量正文（带 Content-Length）
    Full,
    /// 200 + 全量正文，**不带** Content-Length
    FullNoLength,
    /// 200 + 只有 1 字节（Content-Length: 1）
    OneByte,
    /// 416 + `Content-Range: bytes */{total}`
    Unsatisfiable,
}

/// 服务器收到的一次请求 / one request seen by the server.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedRequest {
    pub method: String,
    pub path: String,
    /// 原始 Range 头的值，如 "bytes=0-99"
    pub range: Option<String>,
}

/// 服务器可变状态 / mutable state shared between the accept loop and connections.
#[allow(dead_code)]
struct State {
    content: Vec<u8>,
    mode: Mode,
    requests: Vec<RecordedRequest>,
    /// 已处理的请求数（跨连接累计，供 `Mode::FailTimes` 使用）
    served: u32,
    /// 附加到每个响应上的额外头部（探测类测试用来注入 Content-Disposition 等）
    headers: Vec<(String, String)>,
}

/// 本地测试服务器 / local test HTTP server.
///
/// Drop 时会 abort accept 循环并中止所有在途连接。
#[allow(dead_code)]
pub struct TestServer {
    port: u16,
    state: Arc<Mutex<State>>,
    accept_task: JoinHandle<()>,
}

#[allow(dead_code)]
impl TestServer {
    /// 等价于 `start_with(content, Mode::Range)`。
    pub async fn start(content: Vec<u8>) -> TestServer {
        TestServer::start_with(content, Mode::Range).await
    }

    /// 在 `127.0.0.1:0` 上启动服务器并返回句柄。
    pub async fn start_with(content: Vec<u8>, mode: Mode) -> TestServer {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("test server: bind 127.0.0.1:0 failed");
        let port = listener
            .local_addr()
            .expect("test server: local_addr failed")
            .port();

        let state = Arc::new(Mutex::new(State {
            content,
            mode,
            requests: Vec::new(),
            served: 0,
            headers: Vec::new(),
        }));

        let accept_state = Arc::clone(&state);
        let accept_task = tokio::spawn(async move {
            // JoinSet 被 drop 时会 abort 所有连接任务（TestServer::drop -> abort accept 任务）。
            let mut conns = tokio::task::JoinSet::new();
            loop {
                // 回收已结束的连接任务，避免 JoinSet 无限增长。
                while conns.try_join_next().is_some() {}

                match listener.accept().await {
                    Ok((stream, _peer)) => {
                        let state = Arc::clone(&accept_state);
                        conns.spawn(async move { handle_connection(stream, state).await });
                    }
                    Err(_) => {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }
            }
        });

        TestServer {
            port,
            state,
            accept_task,
        }
    }

    /// 运行中切换响应模式。
    pub fn set_mode(&self, mode: Mode) {
        lock(&self.state).mode = mode;
    }

    /// 设置附加到每个响应上的额外头部（替换上一次的设置）。
    pub fn set_headers(&self, headers: Vec<(String, String)>) {
        lock(&self.state).headers = headers;
    }

    /// 监听端口。
    pub fn port(&self) -> u16 {
        self.port
    }

    /// 形如 `http://127.0.0.1:{port}{path}`。
    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }

    /// 已收到的请求（按到达顺序）。
    pub fn requests(&self) -> Vec<RecordedRequest> {
        lock(&self.state).requests.clone()
    }

    /// 服务器持有的内容。
    pub fn content(&self) -> Vec<u8> {
        lock(&self.state).content.clone()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        // 中止 accept 循环；JoinSet 随之 drop，在途连接（含 Stall）也会被 abort。
        self.accept_task.abort();
    }
}

/// 取锁，忽略 poisoning（测试中断言失败不应让服务器卡死）。
fn lock(state: &Arc<Mutex<State>>) -> std::sync::MutexGuard<'_, State> {
    match state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// 处理单个连接：读请求头 -> 记录请求 -> 规划响应 -> 写出。
async fn handle_connection(mut stream: TcpStream, state: Arc<Mutex<State>>) {
    let head = match read_head(&mut stream).await {
        Some(head) => head,
        None => return,
    };
    let (method, path, headers) = parse_head(&head);
    let range = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("range"))
        .map(|(_, value)| value.clone());

    // 先记录请求（在响应之前），再规划响应；锁不跨 await 持有。
    let (response, mode) = {
        let mut guard = lock(&state);
        guard.requests.push(RecordedRequest {
            method,
            path,
            range: range.clone(),
        });
        guard.served = guard.served.saturating_add(1);
        let mut response = plan_response(&guard, range.as_deref());
        let extra = guard.headers.clone();
        let mode = guard.mode.clone();
        drop(guard);
        inject_headers(&mut response.head, &extra);
        (response, mode)
    };

    // 所有写错误一律忽略：测试里客户端经常中途断开。
    let _ = stream.write_all(response.head.as_bytes()).await;
    if let Mode::Slow { piece, gap } = mode {
        // 慢慢发正文：每片之间停 `gap`，用来测读超时是不是空闲超时。
        for piece in response.body.chunks(piece.max(1)) {
            let _ = stream.write_all(piece).await;
            let _ = stream.flush().await;
            tokio::time::sleep(gap).await;
        }
    } else if !response.body.is_empty() {
        let _ = stream.write_all(&response.body).await;
    }
    if response.stall {
        // 故意不结束连接：先发头 + 少量数据，然后长时间挂起。
        tokio::time::sleep(STALL_HOLD).await;
    }
    let _ = stream.flush().await;
    let _ = stream.shutdown().await;
}

/// 读取请求头直到 `\r\n\r\n`（不含），返回头部文本；连接提前关闭/出错返回 `None`。
async fn read_head(stream: &mut TcpStream) -> Option<String> {
    let mut buf: Vec<u8> = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    loop {
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            buf.truncate(pos);
            break;
        }
        if buf.len() > MAX_HEAD_BYTES {
            return None;
        }
        match stream.read(&mut chunk).await {
            Ok(0) => {
                // 对端关闭：头部不完整就没有可回应的请求。
                if buf.is_empty() {
                    return None;
                }
                break;
            }
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => return None,
        }
    }
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// 解析请求行与头部；请求体被忽略（测试只发 GET）。
fn parse_head(head: &str) -> (String, String, Vec<(String, String)>) {
    let mut lines = head.split('\n').map(|line| line.trim_end_matches('\r'));

    let request_line = lines.next().unwrap_or("").trim();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();

    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    (method, path, headers)
}

/// 一次响应的完整规划结果（头部 + 已拷贝的正文）。
struct Response {
    head: String,
    body: Vec<u8>,
    stall: bool,
}

/// Range 解析结果。
enum RangeSpec {
    /// 可满足的闭区间 `[start, end]`
    Satisfiable(usize, usize),
    /// `start >= total`（或空内容）-> 416
    Unsatisfiable,
    /// 缺失/无法解析 -> 200 全量
    Unparsable,
}

fn plan_response(state: &State, range: Option<&str>) -> Response {
    match &state.mode {
        Mode::Forbidden => plain(403, "Forbidden", b"forbidden\n".to_vec()),
        Mode::FailTimes(n) if state.served <= *n => {
            plain(500, "Internal Server Error", b"server error\n".to_vec())
        }
        // `Stall` 也要按 Range 回 206：回 200 会让客户端判成「不支持范围请求」而去降级，
        // 那就测不到读超时了。头部照给完整长度，正文只发前 64 字节然后挂住。
        Mode::Stall => {
            let mut response = match range {
                None => full(state),
                Some(value) => ranged(state, value),
            };
            response.body.truncate(STALL_PREFIX_BYTES);
            response.stall = true;
            response
        }
        Mode::IgnoreRange => full(state),
        // `Slow` 与 `Range` 同服务，节奏在写回时按模式处理（见 `handle_connection`）
        Mode::Range | Mode::FailTimes(_) | Mode::Slow { .. } => match range {
            None => full(state),
            Some(value) => ranged(state, value),
        },
        Mode::Script(steps) => {
            let index = state.served.saturating_sub(1) as usize;
            let step = steps
                .get(index)
                .or_else(|| steps.last())
                .cloned()
                .unwrap_or(Step::Ranged);
            scripted(state, &step, range)
        }
    }
}

/// 按脚本步骤产出响应。
fn scripted(state: &State, step: &Step, range: Option<&str>) -> Response {
    match step {
        Step::Full => full(state),
        Step::FullNoLength => Response {
            head: build_head_without_length(200, "application/octet-stream", &[]),
            body: state.content.clone(),
            stall: false,
        },
        Step::OneByte => Response {
            head: build_head(200, "application/octet-stream", 1, &[]),
            body: state
                .content
                .first()
                .copied()
                .map(|b| vec![b])
                .unwrap_or_default(),
            stall: false,
        },
        Step::Unsatisfiable => Response {
            head: build_head(
                416,
                "application/octet-stream",
                0,
                &[("Content-Range", format!("bytes */{}", state.content.len()))],
            ),
            body: Vec::new(),
            stall: false,
        },
        Step::Ranged => match range {
            None => full(state),
            Some(value) => ranged(state, value),
        },
    }
}

/// 组装状态行 + 头部，但**不写 Content-Length**（靠 `Connection: close` 定界）。
fn build_head_without_length(status: u16, content_type: &str, extra: &[(&str, String)]) -> String {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\n",
        status,
        reason(status),
        content_type
    );
    for (name, value) in extra {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("Connection: close\r\n\r\n");
    head
}

/// 把额外头部合并进头部块：同名的**替换**，没有的追加。
///
/// 必须是替换语义：`build_head` 已经写过 `Content-Type`，若直接追加会出现两条，
/// 而客户端只会看到第一条。
fn inject_headers(head: &mut String, extra: &[(String, String)]) {
    if extra.is_empty() {
        return;
    }
    let Some(body_start) = head.find("\r\n\r\n") else {
        return;
    };
    let block = head[..body_start].to_string();
    let split = block
        .find("\r\n")
        .map(|position| position + 2)
        .unwrap_or(block.len());
    let (status_line, rest) = block.split_at(split);

    let mut lines: Vec<String> = rest
        .split("\r\n")
        .filter(|line| !line.is_empty())
        .map(|line| line.to_string())
        .collect();

    for (name, value) in extra {
        let lowered = name.to_lowercase();
        let replacement = format!("{name}: {value}");
        let found = lines.iter().position(|line| {
            line.split(':')
                .next()
                .map(|key| key.trim().to_lowercase() == lowered)
                .unwrap_or(false)
        });
        match found {
            Some(index) => lines[index] = replacement,
            None => lines.push(replacement),
        }
    }

    let mut rebuilt = String::from(status_line);
    for line in &lines {
        rebuilt.push_str(line);
        rebuilt.push_str("\r\n");
    }
    rebuilt.push_str("\r\n");
    *head = rebuilt;
}

/// 按 Range 头给 206 / 416 / 200 全量；`Range` / `FailTimes` / `Slow` / `Stall` 共用。
fn ranged(state: &State, value: &str) -> Response {
    let total = state.content.len();
    match parse_range(value, total) {
        RangeSpec::Unparsable => full(state),
        RangeSpec::Unsatisfiable => Response {
            head: build_head(
                416,
                "application/octet-stream",
                0,
                &[
                    ("Content-Range", format!("bytes */{}", total)),
                    ("Accept-Ranges", "bytes".to_string()),
                ],
            ),
            body: Vec::new(),
            stall: false,
        },
        RangeSpec::Satisfiable(start, end) => Response {
            head: build_head(
                206,
                "application/octet-stream",
                end - start + 1,
                &[
                    (
                        "Content-Range",
                        format!("bytes {}-{}/{}", start, end, total),
                    ),
                    ("Accept-Ranges", "bytes".to_string()),
                ],
            ),
            body: state.content[start..=end].to_vec(),
            stall: false,
        },
    }
}

/// 200 + 全量正文。
fn full(state: &State) -> Response {
    Response {
        head: build_head(
            200,
            "application/octet-stream",
            state.content.len(),
            &[("Accept-Ranges", "bytes".to_string())],
        ),
        body: state.content.clone(),
        stall: false,
    }
}

/// 短正文错误响应。
fn plain(status: u16, content_type: &str, body: Vec<u8>) -> Response {
    Response {
        head: build_head(status, content_type, body.len(), &[]),
        body,
        stall: false,
    }
}

/// 组装状态行 + 头部（含 `Connection: close`）。
fn build_head(
    status: u16,
    content_type: &str,
    content_length: usize,
    extra: &[(&str, String)],
) -> String {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n",
        status,
        reason(status),
        content_type,
        content_length
    );
    for (name, value) in extra {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("Connection: close\r\n\r\n");
    head
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        206 => "Partial Content",
        403 => "Forbidden",
        416 => "Range Not Satisfiable",
        _ => "Internal Server Error",
    }
}

/// 解析 Range 头。
///
/// 支持 `bytes=start-end`（含端点）与 `bytes=start-`（开放结尾）；
/// 额外支持标准后缀形式 `bytes=-N`（最后 N 字节）以及逗号多段时取第一段。
/// 其他情况一律视为无法解析 -> 200 全量。
fn parse_range(value: &str, total: usize) -> RangeSpec {
    let value = value.trim();
    let rest = match value.get(..6) {
        Some(prefix) if prefix.eq_ignore_ascii_case("bytes=") => &value[6..],
        _ => return RangeSpec::Unparsable,
    };
    // 多段 Range 只取第一段。
    let spec = match rest.split(',').next() {
        Some(spec) => spec.trim(),
        None => return RangeSpec::Unparsable,
    };
    let (start_raw, end_raw) = match spec.split_once('-') {
        Some(pair) => pair,
        None => return RangeSpec::Unparsable,
    };
    let start_raw = start_raw.trim();
    let end_raw = end_raw.trim();

    if start_raw.is_empty() {
        // `bytes=-N`：最后 N 字节。
        let suffix: usize = match end_raw.parse() {
            Ok(suffix) => suffix,
            Err(_) => return RangeSpec::Unparsable,
        };
        if suffix == 0 {
            return RangeSpec::Unparsable;
        }
        if total == 0 {
            return RangeSpec::Unsatisfiable;
        }
        let suffix = suffix.min(total);
        return RangeSpec::Satisfiable(total - suffix, total - 1);
    }

    let start: usize = match start_raw.parse() {
        Ok(start) => start,
        Err(_) => return RangeSpec::Unparsable,
    };
    if start >= total {
        return RangeSpec::Unsatisfiable;
    }
    let end: usize = if end_raw.is_empty() {
        total - 1
    } else {
        match end_raw.parse() {
            Ok(end) => end,
            Err(_) => return RangeSpec::Unparsable,
        }
    };
    if end < start {
        return RangeSpec::Unparsable;
    }
    RangeSpec::Satisfiable(start, end.min(total - 1))
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
