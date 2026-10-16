//! aria2 JSON-RPC 规格：镜像 `tests/test_aria2_rpc.py`。
//!
//! 用裸 socket 客户端（与 Python 测试同构）验证 HTTP 与 WebSocket 两条传输路径。

use futures_util::{SinkExt, StreamExt};
use gd3_servers::{Aria2Action, Aria2Config, Aria2Service, ListenStatus, PortListener};
use serde_json::{Value, json};
use std::net::{Ipv4Addr, SocketAddr, TcpListener as StdTcpListener};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

const VERSION: &str = "4.4.0";

struct Harness {
    listener: PortListener,
    port: u16,
    actions: Arc<Mutex<Vec<Aria2Action>>>,
    config: Arc<Mutex<Aria2Config>>,
}

impl Harness {
    async fn start(download_folder: &str, token: &str) -> Harness {
        let config = Arc::new(Mutex::new(Aria2Config {
            version: VERSION.to_string(),
            download_folder: download_folder.to_string(),
            token: token.to_string(),
            emulate_fingerprint: true,
        }));
        let actions: Arc<Mutex<Vec<Aria2Action>>> = Arc::new(Mutex::new(Vec::new()));

        let service = Arc::new(Aria2Service::new(
            {
                let config = Arc::clone(&config);
                Arc::new(move || config.lock().expect("配置锁").clone())
            },
            {
                let actions = Arc::clone(&actions);
                Arc::new(move |action| actions.lock().expect("动作锁").push(action))
            },
        ));

        let listener = PortListener::new(Arc::new(move |stream| {
            let service = Arc::clone(&service);
            Box::pin(async move { service.handle(stream).await })
        }));

        let port = free_port();
        listener.request(Some(port));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while listener.state().status != ListenStatus::Listening {
            assert!(tokio::time::Instant::now() < deadline, "监听未就绪");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        Harness {
            listener,
            port,
            actions,
            config,
        }
    }

    fn set_token(&self, token: &str) {
        self.config.lock().expect("配置锁").token = token.to_string();
    }

    fn set_emulate_fingerprint(&self, value: bool) {
        self.config.lock().expect("配置锁").emulate_fingerprint = value;
    }

    fn take_actions(&self) -> Vec<Aria2Action> {
        std::mem::take(&mut *self.actions.lock().expect("动作锁"))
    }

    fn address(&self) -> SocketAddr {
        SocketAddr::from((Ipv4Addr::LOCALHOST, self.port))
    }

    /// 发一段原始请求，读回完整响应；对端不回时返回 `None`。
    async fn send_raw(&self, raw: &[u8]) -> Option<(u16, Vec<(String, String)>, Vec<u8>)> {
        let mut stream = TcpStream::connect(self.address()).await.expect("连接失败");
        stream.write_all(raw).await.expect("写失败");

        let mut buffer = Vec::new();
        let mut chunk = vec![0u8; 4096];
        loop {
            let read = tokio::time::timeout(Duration::from_secs(3), stream.read(&mut chunk)).await;
            match read {
                Ok(Ok(0)) => break,
                Ok(Ok(n)) => buffer.extend_from_slice(&chunk[..n]),
                Ok(Err(_)) => break,
                Err(_) => break,
            }
        }
        if buffer.is_empty() {
            return None;
        }

        let split = buffer
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("没有响应头结束标记");
        let head = String::from_utf8_lossy(&buffer[..split]).to_string();
        let body = buffer[split + 4..].to_vec();

        let mut lines = head.split("\r\n");
        let status: u16 = lines
            .next()
            .expect("没有状态行")
            .split(' ')
            .nth(1)
            .expect("没有状态码")
            .parse()
            .expect("状态码不是数字");
        let headers = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_lowercase(), value.trim().to_string()))
            .collect();

        Some((status, headers, body))
    }

    async fn post(&self, payload: &[u8], path: &str) -> (u16, Vec<(String, String)>, Value) {
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            payload.len()
        );
        let mut raw = request.into_bytes();
        raw.extend_from_slice(payload);
        let (status, headers, body) = self.send_raw(&raw).await.expect("没有响应");
        (
            status,
            headers,
            serde_json::from_slice(&body).expect("响应不是 JSON"),
        )
    }

    async fn call(&self, method: &str, params: Value) -> Value {
        let payload = json!({"jsonrpc": "2.0", "id": "t", "method": method, "params": params});
        let (_, _, body) = self.post(payload.to_string().as_bytes(), "/jsonrpc").await;
        body
    }

    async fn stop(&self) {
        self.listener.stop().await;
    }
}

fn free_port() -> u16 {
    StdTcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .expect("绑端口")
        .local_addr()
        .expect("取端口")
        .port()
}

fn is_gid(value: &Value) -> bool {
    value.as_str().is_some_and(|text| {
        text.len() == 16
            && text
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    })
}

#[tokio::test]
async fn get_version_over_http() {
    let harness = Harness::start("C:/downloads", "").await;
    let payload = json!({"jsonrpc": "2.0", "id": "a", "method": "aria2.getVersion", "params": []});

    let (status, headers, body) = harness
        .post(payload.to_string().as_bytes(), "/jsonrpc")
        .await;

    assert_eq!(status, 200);
    let header = |name: &str| {
        headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };
    assert_eq!(
        header("content-type").as_deref(),
        Some("application/json-rpc")
    );
    assert_eq!(header("access-control-allow-origin").as_deref(), Some("*"));
    assert_eq!(
        body,
        json!({"jsonrpc": "2.0", "id": "a",
               "result": {"version": VERSION, "enabledFeatures": ["HTTPS"]}})
    );

    harness.stop().await;
}

#[tokio::test]
async fn get_global_option_over_http() {
    let harness = Harness::start("C:/downloads", "").await;
    let body = harness
        .call("aria2.getGlobalOption", json!(["token:"]))
        .await;

    assert_eq!(
        body,
        json!({"jsonrpc": "2.0", "id": "t", "result": {"dir": "C:/downloads"}})
    );

    harness.stop().await;
}

#[tokio::test]
async fn get_version_over_websocket() {
    let harness = Harness::start("C:/downloads", "").await;
    let url = format!("ws://127.0.0.1:{}/jsonrpc", harness.port);
    let (mut websocket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("握手失败");

    websocket
        .send(Message::Text(
            json!({"jsonrpc": "2.0", "id": "w", "method": "aria2.getVersion", "params": []})
                .to_string()
                .into(),
        ))
        .await
        .expect("发送失败");

    let reply = websocket.next().await.expect("没有回复").expect("回复出错");
    let body: Value = serde_json::from_str(reply.to_text().expect("不是文本")).expect("不是 JSON");
    assert_eq!(body["result"]["version"], VERSION);

    websocket.close(None).await.ok();
    harness.stop().await;
}

#[tokio::test]
async fn preflight_is_answered_without_token() {
    let harness = Harness::start("C:/downloads", "secret").await;

    let (status, headers, body) = harness
        .send_raw(
            b"OPTIONS /jsonrpc HTTP/1.1\r\nHost: x\r\nOrigin: https://pdpb.cn\r\n\
              Access-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: content-type\r\n\r\n",
        )
        .await
        .expect("没有响应");

    assert_eq!(status, 200);
    assert!(body.is_empty());
    let header = |name: &str| {
        headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };
    assert_eq!(header("access-control-allow-origin").as_deref(), Some("*"));
    assert_eq!(
        header("access-control-allow-methods").as_deref(),
        Some("POST, GET, OPTIONS")
    );
    assert_eq!(
        header("access-control-allow-headers").as_deref(),
        Some("content-type")
    );

    harness.stop().await;
}

#[tokio::test]
async fn other_path_is_not_found() {
    let harness = Harness::start("C:/downloads", "").await;

    // 404 没有响应体，所以不能用要求 JSON 的 post()
    let (status, _, body) = harness
        .send_raw(b"POST /rpc HTTP/1.1\r\nHost: x\r\nContent-Length: 2\r\n\r\n{}")
        .await
        .expect("没有响应");
    assert_eq!(status, 404);
    assert!(body.is_empty(), "404 不该有响应体");

    harness.stop().await;
}

#[tokio::test]
async fn websocket_on_other_path_is_not_found() {
    let harness = Harness::start("C:/downloads", "").await;
    let url = format!("ws://127.0.0.1:{}/other", harness.port);

    // 升级请求在路径判断之前就被拒，所以客户端拿不到 101
    let error = tokio_tungstenite::connect_async(&url).await;
    assert!(error.is_err(), "非 /jsonrpc 路径不该握手成功");

    harness.stop().await;
}

#[tokio::test]
async fn client_leaving_mid_head_does_not_stall_listener() {
    let harness = Harness::start("C:/downloads", "").await;

    let mut stream = TcpStream::connect(harness.address())
        .await
        .expect("连接失败");
    stream
        .write_all(b"POST /jsonrpc HTTP/1.1\r\nHost: x\r\n")
        .await
        .expect("写失败");
    drop(stream);

    let body = tokio::time::timeout(
        Duration::from_secs(2),
        harness.call("aria2.getVersion", json!([])),
    )
    .await
    .expect("监听器被卡住");
    assert_eq!(body["result"]["version"], VERSION);

    harness.stop().await;
}

#[tokio::test]
async fn body_over_2m_is_dropped_without_response() {
    let harness = Harness::start("C:/downloads", "").await;

    let response = harness
        .send_raw(b"POST /jsonrpc HTTP/1.1\r\nHost: x\r\nContent-Length: 3000000\r\n\r\n{}")
        .await;
    assert!(response.is_none(), "超限请求不该有响应");

    harness.stop().await;
}

#[tokio::test]
async fn malformed_requests_map_to_the_same_status_and_code() {
    let harness = Harness::start("C:/downloads", "").await;

    let cases: [(&[u8], u16, i64); 4] = [
        (b"{not json", 500, -32700),
        (b"[]", 400, -32600),
        (
            b"{\"jsonrpc\":\"2.0\",\"method\":\"aria2.getVersion\"}",
            400,
            -32600,
        ),
        (
            b"{\"jsonrpc\":\"2.0\",\"id\":\"x\",\"method\":\"aria2.getVersion\",\"params\":{}}",
            500,
            -32602,
        ),
    ];

    for (payload, status, code) in cases {
        let (actual_status, _, body) = harness.post(payload, "/jsonrpc").await;
        assert_eq!(
            actual_status,
            status,
            "payload={:?}",
            String::from_utf8_lossy(payload)
        );
        assert_eq!(
            body["error"]["code"],
            code,
            "payload={:?}",
            String::from_utf8_lossy(payload)
        );
    }

    harness.stop().await;
}

#[tokio::test]
async fn unknown_method_is_an_aria2_error() {
    let harness = Harness::start("C:/downloads", "").await;
    let payload =
        json!({"jsonrpc": "2.0", "id": "x", "method": "aria2.tellStatus", "params": ["g"]});

    let (status, _, body) = harness
        .post(payload.to_string().as_bytes(), "/jsonrpc")
        .await;

    assert_eq!(status, 400);
    assert_eq!(
        body["error"],
        json!({"code": 1, "message": "No such method: aria2.tellStatus"})
    );

    harness.stop().await;
}

#[tokio::test]
async fn missing_token_is_unauthorized_after_a_second() {
    let harness = Harness::start("C:/downloads", "").await;
    harness.set_token("secret");

    let started = std::time::Instant::now();
    let body = harness.call("aria2.addUri", json!([[]])).await;

    assert!(
        started.elapsed() >= Duration::from_secs(1),
        "未授权应当延迟一秒：{:?}",
        started.elapsed()
    );
    assert_eq!(body["error"], json!({"code": 1, "message": "Unauthorized"}));

    harness.stop().await;
}

#[tokio::test]
async fn wrong_and_right_tokens() {
    let harness = Harness::start("C:/downloads", "").await;
    harness.set_token("secret");

    let wrong = harness
        .call("aria2.getVersion", json!(["token:guess"]))
        .await;
    assert_eq!(wrong["error"]["message"], "Unauthorized");

    let right = harness
        .call("aria2.getVersion", json!(["token:secret"]))
        .await;
    assert_eq!(right["result"]["version"], VERSION);

    harness.stop().await;
}

#[tokio::test]
async fn token_param_is_stripped_when_no_token_is_set() {
    let harness = Harness::start("C:/downloads", "").await;
    let body = harness
        .call("aria2.addUri", json!(["token:", ["https://a.test/f.zip"]]))
        .await;

    assert!(is_gid(&body["result"]), "应返回 gid：{body}");
    harness.stop().await;
}

#[tokio::test]
async fn add_uri_builds_task_options_from_aria2_options() {
    let harness = Harness::start("C:/downloads", "").await;

    let body = harness
        .call(
            "aria2.addUri",
            json!([
                ["https://a.test/f.zip", "https://mirror.test/f.zip"],
                {
                    "dir": "D:/sub",
                    "out": "renamed.zip",
                    "header": ["Cookie: a=b", "User-Agent: from-header"],
                    "user-agent": "from-option",
                    "referer": "https://a.test/"
                }
            ]),
        )
        .await;

    assert!(is_gid(&body["result"]), "应返回 gid：{body}");

    let actions = harness.take_actions();
    let [Aria2Action::AddTask { gid, options }] = actions.as_slice() else {
        panic!("应产生一个 AddTask：{actions:?}");
    };
    assert_eq!(gid, body["result"].as_str().expect("gid"));
    assert_eq!(options.url, "https://a.test/f.zip");
    assert_eq!(options.output_folder, "D:/sub");
    // 已带 User-Agent 时不被 user-agent 选项覆盖
    assert_eq!(
        options.headers,
        vec![
            ("Cookie".to_string(), "a=b".to_string()),
            ("User-Agent".to_string(), "from-header".to_string()),
            ("Referer".to_string(), "https://a.test/".to_string()),
        ]
    );
    assert_eq!(options.name, "renamed.zip");
    assert_eq!(options.client_profile, "");

    harness.stop().await;
}

#[tokio::test]
async fn header_may_be_a_single_string() {
    let harness = Harness::start("C:/downloads", "").await;
    harness
        .call(
            "aria2.addUri",
            json!([["https://a.test/f"], {"header": "Cookie: a=b"}]),
        )
        .await;

    let actions = harness.take_actions();
    let [Aria2Action::AddTask { options, .. }] = actions.as_slice() else {
        panic!("应产生一个 AddTask");
    };
    assert_eq!(
        options.headers,
        vec![("Cookie".to_string(), "a=b".to_string())]
    );

    harness.stop().await;
}

#[tokio::test]
async fn wrongly_typed_options_are_ignored() {
    let harness = Harness::start("C:/downloads", "").await;
    let url = format!("ws://127.0.0.1:{}/jsonrpc", harness.port);
    let (mut websocket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("握手失败");

    websocket
        .send(Message::Text(
            json!({"jsonrpc": "2.0", "id": 1, "method": "aria2.addUri", "params": [
                ["https://a.test/f"], {"dir": 123, "out": ["x"], "header": 5, "user-agent": null}]})
            .to_string()
            .into(),
        ))
        .await
        .expect("发送失败");
    let reply = websocket.next().await.expect("没有回复").expect("回复出错");
    let body: Value = serde_json::from_str(reply.to_text().expect("不是文本")).expect("不是 JSON");
    assert!(is_gid(&body["result"]), "应返回 gid：{body}");

    websocket
        .send(Message::Text(
            json!({"jsonrpc": "2.0", "id": 2, "method": "aria2.addUri", "params": [
                ["https://a.test/g"], {"dir": null}]})
            .to_string()
            .into(),
        ))
        .await
        .expect("发送失败");
    let reply = websocket.next().await.expect("没有回复").expect("回复出错");
    let body: Value = serde_json::from_str(reply.to_text().expect("不是文本")).expect("不是 JSON");
    assert!(is_gid(&body["result"]), "应返回 gid：{body}");

    let actions = harness.take_actions();
    assert_eq!(actions.len(), 2);
    let Aria2Action::AddTask { options, .. } = &actions[0];
    // 类型不对的选项当空：落到默认目录，headers 为空，name 为空
    assert_eq!(options.output_folder, "C:/downloads");
    assert!(options.headers.is_empty());
    assert_eq!(options.name, "");

    websocket.close(None).await.ok();
    harness.stop().await;
}

#[tokio::test]
async fn invalid_uris_or_options_are_rejected() {
    let harness = Harness::start("C:/downloads", "").await;

    let cases: [(Value, &str); 5] = [
        (json!([]), "The parameter at 0 is required but missing."),
        (
            json!(["https://a.test/f"]),
            "The parameter at 0 has wrong type.",
        ),
        (json!([[]]), "URI is not provided."),
        (json!([[1, null]]), "URI is not provided."),
        (
            json!([["https://a.test/f"], "dir"]),
            "The parameter at 1 has wrong type.",
        ),
    ];

    for (params, message) in cases {
        let payload =
            json!({"jsonrpc": "2.0", "id": "x", "method": "aria2.addUri", "params": params});
        let (status, _, body) = harness
            .post(payload.to_string().as_bytes(), "/jsonrpc")
            .await;
        assert_eq!(status, 400, "params={params}");
        assert_eq!(
            body["error"],
            json!({"code": 1, "message": message}),
            "params={params}"
        );
    }

    assert!(harness.take_actions().is_empty(), "非法请求不该产生动作");
    harness.stop().await;
}

#[tokio::test]
async fn emulate_fingerprint_off_sends_raw_profile() {
    let harness = Harness::start("C:/downloads", "").await;
    harness.set_emulate_fingerprint(false);

    harness
        .call("aria2.addUri", json!([["https://a.test/f"]]))
        .await;

    let actions = harness.take_actions();
    let [Aria2Action::AddTask { options, .. }] = actions.as_slice() else {
        panic!("应产生一个 AddTask");
    };
    assert_eq!(options.client_profile, "raw");

    harness.stop().await;
}

#[tokio::test]
async fn gid_is_sixteen_lowercase_hex_chars() {
    let first = gd3_servers::aria2::new_gid();
    let second = gd3_servers::aria2::new_gid();

    assert!(
        is_gid(&Value::String(first.clone())),
        "gid 格式不对：{first}"
    );
    assert!(
        is_gid(&Value::String(second.clone())),
        "gid 格式不对：{second}"
    );
    assert_ne!(first, second, "两个 gid 不该相同");
}
