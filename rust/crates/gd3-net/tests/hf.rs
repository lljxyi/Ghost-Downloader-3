//! HuggingFace 树枚举集成测试：对照 `features/huggingface_pack/pack.py` 的
//! `_fetchTree` 在 Python 侧的逐条语义（该函数没有 Python 测试，用例是按
//! 分支写出来的）。服务器是本文件自带的极简 JSON HTTP/1.1 服务器。

use gd3_net::hf::{HfTreeEntry, HfTreeError, fetch_tree};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// 一条文件条目的简写。
fn file(path: &str, size: u64) -> HfTreeEntry {
    HfTreeEntry {
        path: path.to_string(),
        size,
        lfs_size: None,
    }
}

/// 极简 JSON API：`routes: 路径 → JSON 正文`；401 / 403 / 非 JSON 由特殊路径提供。
struct JsonServer {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl JsonServer {
    async fn start(routes: BTreeMap<String, String>) -> JsonServer {
        let requests: Arc<Mutex<Vec<String>>> = Arc::default();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(serve(listener, routes, Arc::clone(&requests)));
        JsonServer {
            url: format!("http://127.0.0.1:{port}"),
            requests,
        }
    }

    fn served(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

async fn serve(
    listener: TcpListener,
    routes: BTreeMap<String, String>,
    requests: Arc<Mutex<Vec<String>>>,
) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        tokio::spawn(handle(stream, routes.clone(), Arc::clone(&requests)));
    }
}

async fn handle(
    mut stream: TcpStream,
    routes: BTreeMap<String, String>,
    requests: Arc<Mutex<Vec<String>>>,
) {
    let mut buffer = [0u8; 16 * 1024];
    let Ok(read) = stream.read(&mut buffer).await else {
        return;
    };
    let head = String::from_utf8_lossy(&buffer[..read]);
    let mut parts = head.split_whitespace();
    let _method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");

    requests.lock().unwrap().push(path.to_string());

    let (status, body) = if let Some(json) = routes.get(path) {
        (200, json.clone())
    } else if path == "/unauthorized" {
        (401, "{}".to_string())
    } else if path == "/forbidden" {
        (403, "{}".to_string())
    } else if path == "/broken" {
        (200, "not json".to_string())
    } else {
        (404, "{}".to_string())
    };

    let response = format!(
        "HTTP/1.1 {status} T\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",
        body.len(),
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

fn client() -> wreq::Client {
    // `raw`：不模拟指纹；测试服务器是普通 HTTP
    gd3_net::build_client(None, None, "raw")
}

#[tokio::test]
async fn flat_tree_keeps_only_files_in_page_order() {
    let mut routes = BTreeMap::new();
    routes.insert(
        "/tree".to_string(),
        r#"[{"type":"file","path":"a.bin","size":10},
            {"type":"directory","path":"sub"},
            {"type":"file","path":"b.txt","size":20}]"#
            .to_string(),
    );
    routes.insert("/tree/sub".to_string(), "[]".to_string());

    let server = JsonServer::start(routes).await;
    let entries = fetch_tree(&client(), &[], &format!("{}/tree", server.url))
        .await
        .unwrap();
    assert_eq!(entries, vec![file("a.bin", 10), file("b.txt", 20)]);
}

#[tokio::test]
async fn directories_expand_and_keep_order_and_lfs_override() {
    let mut routes = BTreeMap::new();
    routes.insert(
        "/tree".to_string(),
        r#"[{"type":"file","path":"root.bin","size":1},
            {"type":"directory","path":"sub"},
            {"type":"file","path":"tail.bin","size":2}]"#
            .to_string(),
    );
    routes.insert(
        "/tree/sub".to_string(),
        r#"[{"type":"file","path":"sub/deep/model.safetensors","size":5,"lfs":{"size":50}}]"#
            .to_string(),
    );

    let server = JsonServer::start(routes).await;
    let entries = fetch_tree(&client(), &[], &format!("{}/tree", server.url))
        .await
        .unwrap();

    // 目录条目展开在原位置：root → sub 的内容 → tail
    assert_eq!(
        entries,
        vec![
            file("root.bin", 1),
            HfTreeEntry {
                path: "sub/deep/model.safetensors".to_string(),
                size: 5,
                lfs_size: Some(50),
            },
            file("tail.bin", 2),
        ]
    );
}

#[tokio::test]
async fn nested_directory_uses_last_segment_like_python() {
    // Python：subUrl = f"{apiUrl}/{path 最后一段}"——现状照抄
    let mut routes = BTreeMap::new();
    routes.insert(
        "/tree".to_string(),
        r#"[{"type":"directory","path":"assets/images"},
            {"type":"file","path":"keep.txt","size":3}]"#
            .to_string(),
    );
    routes.insert(
        "/tree/images".to_string(),
        r#"[{"type":"file","path":"img.png","size":7}]"#.to_string(),
    );
    routes.insert("/tree/assets/images".to_string(), "[]".to_string());

    let server = JsonServer::start(routes).await;
    let entries = fetch_tree(&client(), &[], &format!("{}/tree", server.url))
        .await
        .unwrap();
    assert_eq!(entries, vec![file("img.png", 7), file("keep.txt", 3)]);
    // 请求的是 …/tree/images，不是 …/tree/assets/images
    assert_eq!(
        server.served(),
        vec!["/tree".to_string(), "/tree/images".to_string()]
    );
}

#[tokio::test]
async fn page_shapes_match_python() {
    let mut routes = BTreeMap::new();
    routes.insert(
        "/nope".to_string(),
        r#"{"error":"Repository not found"}"#.to_string(),
    );

    let server = JsonServer::start(routes).await;
    // 200 但不是 list → 返回已收集（空）条目（Python: isinstance(page, list) 为假）
    let outcome = fetch_tree(&client(), &[], &format!("{}/nope", server.url)).await;
    assert!(outcome.unwrap().is_empty());

    // 404 → Status(404)（Python: raise_for_status 抛错）
    let outcome = fetch_tree(&client(), &[], &format!("{}/page404", server.url)).await;
    assert!(
        matches!(outcome, Err(HfTreeError::Status(404))),
        "{outcome:?}"
    );

    // 非 JSON 正文 → 传输层失败（Python 的 json 解析异常）
    let outcome = fetch_tree(&client(), &[], &format!("{}/broken", server.url)).await;
    assert!(
        matches!(outcome, Err(HfTreeError::Transport(_))),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn auth_failures_map_to_errors() {
    let server = JsonServer::start(BTreeMap::new()).await;

    let outcome = fetch_tree(&client(), &[], &format!("{}/unauthorized", server.url)).await;
    assert_eq!(outcome.unwrap_err(), HfTreeError::Auth);

    let outcome = fetch_tree(&client(), &[], &format!("{}/forbidden", server.url)).await;
    assert_eq!(outcome.unwrap_err(), HfTreeError::Status(403));
}

#[tokio::test]
async fn headers_are_accepted() {
    let mut routes = BTreeMap::new();
    routes.insert(
        "/tree".to_string(),
        r#"[{"type":"file","path":"a","size":1}]"#.to_string(),
    );
    let server = JsonServer::start(routes).await;
    let headers = vec![("authorization".to_string(), "Bearer token".to_string())];
    let entries = fetch_tree(&client(), &headers, &format!("{}/tree", server.url))
        .await
        .unwrap();
    assert_eq!(entries, vec![file("a", 1)]);
}
