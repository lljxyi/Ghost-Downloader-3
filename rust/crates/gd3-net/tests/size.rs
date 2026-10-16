//! probeSize 集成测试：对照 `features/bili_pack/stream.py::probeSize` 的分支
//! （该函数有 Python 测试 `tests/test_bili_parse.py::test_probeSize_*`，这里镜像
//! 逐条语义）。服务器是本文件自带的极简 HTTP/1.1 服务器（可完全自定义响应头）。

use gd3_net::build_client;
use std::sync::{Arc, Mutex};

/// raw：不模拟指纹；测试服务器是普通 HTTP
fn client() -> wreq::Client {
    build_client(None, None, "raw")
}

/// `route → status + headers + body`（headers 为原始行，body 为空 = 不含 Content-Length，靠关连接结束正文）。
///
/// 全 tokio 异步实现：accept 循环与读请求头都不阻塞 runtime。
/// 不能用 std `TcpListener` + 阻塞 accept——那会把 worker 线程卡死，测试 runtime 无法关停（实测死锁）。
struct RawServer {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl RawServer {
    async fn start(routes: Vec<(&'static str, u16, Vec<&'static str>, &'static str)>) -> RawServer {
        use std::collections::HashMap;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let map: HashMap<&'static str, (u16, String, &'static str)> = routes
            .into_iter()
            .map(|(path, status, headers, body)| (path, (status, headers.join("\r\n"), body)))
            .collect();
        let requests: Arc<Mutex<Vec<String>>> = Arc::default();
        let value = Arc::clone(&requests);

        tokio::spawn(async move {
            let mut conns = tokio::task::JoinSet::new();
            loop {
                while conns.try_join_next().is_some() {}
                let Ok((mut stream, _)) = listener.accept().await else {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    continue;
                };
                let map = map.clone();
                let requests = Arc::clone(&value);
                conns.spawn(async move {
                    let mut head = Vec::new();
                    let mut chunk = [0u8; 4096];
                    loop {
                        match stream.read(&mut chunk).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                head.extend_from_slice(&chunk[..n]);
                                if head.windows(4).any(|w| w == b"\r\n\r\n") {
                                    break;
                                }
                            }
                        }
                    }
                    let head = String::from_utf8_lossy(&head);
                    let mut parts = head.split_whitespace();
                    let _method = parts.next().unwrap_or("");
                    let path = parts.next().unwrap_or("");
                    requests.lock().unwrap().push(path.to_string());

                    let (status, other, body) = map.get(path).cloned().unwrap_or((
                        404,
                        "Content-Type: text/plain".to_string(),
                        "not found",
                    ));
                    let explanation = if status == 200 { "OK" } else { "T" };
                    let response = if body.is_empty() {
                        // 无正文：不发 Content-Length，靠关连接结束正文
                        format!("HTTP/1.1 {status} {explanation}\r\n{other}\r\nConnection: close\r\n\r\n")
                    } else {
                        format!(
                            "HTTP/1.1 {status} {explanation}\r\n{other}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                            body.len(),
                        )
                    };
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });

        RawServer {
            url: format!("http://{addr}"),
            requests,
        }
    }
}

/// Python probeSize 的默认头。
/// 服务器已改为全 tokio 异步实现，accept/read 都不阻塞 runtime。
fn base_headers() -> Vec<(String, String)> {
    vec![
        ("range".to_string(), "bytes=0-0".to_string()),
        ("accept-encoding".to_string(), "identity".to_string()),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn content_range_total_is_read() {
    let server = RawServer::start(vec![(
        "/file",
        206,
        vec!["Content-Range: bytes 0-0/415000000"],
        "",
    )])
    .await;
    assert_eq!(
        gd3_net::probe::probe_size(&client(), &base_headers(), &format!("{}/file", server.url))
            .await,
        415000000
    );
    assert_eq!(
        server.requests.lock().unwrap().as_slice(),
        ["/file".to_string()]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_or_star_total_returns_zero() {
    // 没有 content-range → 0
    let server = RawServer::start(vec![("/none", 200, vec!["Content-Length: 12"], "")]).await;
    assert_eq!(
        gd3_net::probe::probe_size(&client(), &base_headers(), &format!("{}/none", server.url))
            .await,
        0
    );
    // total = * → 0
    let server =
        RawServer::start(vec![("/star", 206, vec!["Content-Range: bytes 0-0/*"], "")]).await;
    assert_eq!(
        gd3_net::probe::probe_size(&client(), &base_headers(), &format!("{}/star", server.url))
            .await,
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_status_still_parses_headers() {
    // Python 不检查状态码：404 也照读 content-range
    let server = RawServer::start(vec![(
        "/lost",
        404,
        vec!["Content-Range: bytes 0-0/77"],
        "",
    )])
    .await;
    assert_eq!(
        gd3_net::probe::probe_size(&client(), &base_headers(), &format!("{}/lost", server.url))
            .await,
        77
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn garbage_total_returns_zero() {
    let server = RawServer::start(vec![(
        "/junk",
        206,
        vec!["Content-Range: bytes 0-0/abc"],
        "",
    )])
    .await;
    assert_eq!(
        gd3_net::probe::probe_size(&client(), &base_headers(), &format!("{}/junk", server.url))
            .await,
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incoming_range_and_encoding_are_overridden() {
    // 覆盖与 Python 同款：调用方传入的 range 被替换为 bytes=0-0，请求仍能用
    let server =
        RawServer::start(vec![("/file", 206, vec!["Content-Range: bytes 0-0/9"], "")]).await;
    let overridden: Vec<(String, String)> =
        vec![("range".to_string(), "bytes=999-999".to_string())];
    assert_eq!(
        gd3_net::probe::probe_size(&client(), &overridden, &format!("{}/file", server.url)).await,
        9
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unreachable_host_returns_zero() {
    // 连不上的端口 → 0（Python 捕获一切异常）
    assert_eq!(
        gd3_net::probe::probe_size(&client(), &base_headers(), "http://127.0.0.1:9/none").await,
        0
    );
}
