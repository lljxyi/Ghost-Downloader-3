//! 探测（probe）集成测试：在本地 HTTP 服务器上验证大小推断、范围支持与文件名解析。
//!
//! 规格对应 `features/http_pack/pack.py` 的 `probe`。服务器实现见
//! `tests/support/test_server.rs`。
//!
//! 注意：`http_pack.probe` 在 Python 侧**没有任何直接测试**，所以这里的用例是照着
//! Python 实现的分支逐条写出来的，不是从既有测试翻译的。

#[path = "support/test_server.rs"]
mod test_server;

use gd3_net::probe::{ExtensionResolver, ProbeOptions, SIZE_UNKNOWN, probe};
use std::sync::Arc;
use test_server::{Mode, Step, TestServer};

fn options(server: &TestServer, path: &str) -> ProbeOptions {
    ProbeOptions {
        url: server.url(path),
        headers: Vec::new(),
        user_agent: "gd3-test/1.0".to_string(),
        client_profile: "raw".to_string(),
        fallback_name: "file_1700000000000000000".to_string(),
    }
}

/// 确定性伪随机内容，断言逐字节相等时才有意义。
fn content(size: usize) -> Vec<u8> {
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    (0..size)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

fn resolver(mapping: &'static [(&'static str, &'static str)]) -> Option<ExtensionResolver> {
    Some(Arc::new(move |content_type: &str| {
        mapping
            .iter()
            .find(|(key, _)| *key == content_type)
            .map(|(_, value)| (*value).to_string())
    }))
}

#[tokio::test]
async fn range_support_reads_size_from_content_range() {
    let server = TestServer::start_with(content(4096), Mode::Range).await;
    let file = probe(&options(&server, "/file"), None)
        .await
        .expect("探测失败");

    assert_eq!(file.file_size, 4096);
    assert!(file.can_use_range_requests);
    assert_eq!(file.name, "file");
}

#[tokio::test]
async fn ignored_range_falls_back_to_content_length() {
    let server = TestServer::start_with(content(2048), Mode::IgnoreRange).await;
    let file = probe(&options(&server, "/file.bin"), None)
        .await
        .expect("探测失败");

    assert_eq!(file.file_size, 2048);
    assert!(!file.can_use_range_requests);
    assert_eq!(file.name, "file.bin");
}

#[tokio::test]
async fn unknown_length_triggers_the_zero_zero_probe() {
    // 第一个请求 200 但不给 Content-Length，第二个请求才回 206。
    let server = TestServer::start_with(
        content(1024),
        Mode::Script(vec![Step::FullNoLength, Step::Ranged]),
    )
    .await;
    let file = probe(&options(&server, "/file.bin"), None)
        .await
        .expect("探测失败");

    assert_eq!(file.file_size, 1024);
    assert!(file.can_use_range_requests);

    let requests = server.requests();
    assert_eq!(requests.len(), 2, "应当只发两次请求");
    assert_eq!(requests[0].range.as_deref(), Some("bytes=1-1"));
    assert_eq!(requests[1].range.as_deref(), Some("bytes=0-0"));
}

#[tokio::test]
async fn one_byte_content_length_also_triggers_the_probe() {
    // 服务器把 range 请求当成普通请求、只回了 1 字节 —— Python 也会再兜一次。
    let server = TestServer::start_with(
        content(512),
        Mode::Script(vec![Step::OneByte, Step::Ranged]),
    )
    .await;
    let file = probe(&options(&server, "/file.bin"), None)
        .await
        .expect("探测失败");

    assert_eq!(file.file_size, 512);
    assert!(file.can_use_range_requests);
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn unsatisfiable_fallback_reads_size_from_content_range_star() {
    // 兜底请求拿到 416 + `bytes */N`，Python 从这里取总大小。
    let server = TestServer::start_with(
        content(777),
        Mode::Script(vec![Step::FullNoLength, Step::Unsatisfiable]),
    )
    .await;
    let file = probe(&options(&server, "/file.bin"), None)
        .await
        .expect("探测失败");

    assert_eq!(file.file_size, 777);
    assert!(!file.can_use_range_requests);
}

#[tokio::test]
async fn content_disposition_wins_over_everything_else() {
    // 用 RFC 2231 形式：HTTP 头里直接放非 ASCII 字节不合法，真实服务器也这么发。
    // （若真发了裸 UTF-8，两侧都会按 latin-1 解成乱码，见 probe.rs 的 header_text。）
    let server = TestServer::start_with(content(64), Mode::Range).await;
    server.set_headers(vec![
        (
            "Content-Disposition".to_string(),
            "attachment; filename*=UTF-8''%E8%A7%86%E9%A2%91.mp4".to_string(),
        ),
        (
            "Content-Location".to_string(),
            "/ignored/other.bin".to_string(),
        ),
    ]);

    let file = probe(&options(&server, "/from/url/path.bin"), None)
        .await
        .expect("探测失败");
    assert_eq!(file.name, "视频.mp4");
}

#[tokio::test]
async fn content_location_is_used_when_disposition_is_missing() {
    let server = TestServer::start_with(content(64), Mode::Range).await;
    server.set_headers(vec![(
        "Content-Location".to_string(),
        "/cdn/%E8%A7%86%E9%A2%91.mp4".to_string(),
    )]);

    let file = probe(&options(&server, "/from/url/path.bin"), None)
        .await
        .expect("探测失败");
    assert_eq!(file.name, "视频.mp4");
}

#[tokio::test]
async fn url_path_is_the_last_resort_for_the_name() {
    let server = TestServer::start_with(content(64), Mode::Range).await;
    let file = probe(&options(&server, "/a/b/from-url.bin"), None)
        .await
        .expect("探测失败");
    assert_eq!(file.name, "from-url.bin");
}

#[tokio::test]
async fn query_parameter_disposition_is_understood() {
    let server = TestServer::start_with(content(64), Mode::Range).await;
    let path = "/a/b/file?response-content-disposition=attachment%3B%20filename%3D%22q.mp4%22";
    let file = probe(&options(&server, path), None)
        .await
        .expect("探测失败");
    assert_eq!(file.name, "q.mp4");
}

#[tokio::test]
async fn missing_extension_is_filled_from_content_type() {
    let server = TestServer::start_with(content(64), Mode::Range).await;
    server.set_headers(vec![
        (
            "Content-Disposition".to_string(),
            "attachment; filename=\"movie\"".to_string(),
        ),
        ("Content-Type".to_string(), "video/mp4".to_string()),
    ]);

    let file = probe(&options(&server, "/x"), resolver(&[("video/mp4", ".mp4")]))
        .await
        .expect("探测失败");
    assert_eq!(file.name, "movie.mp4");
    assert_eq!(file.content_type, "video/mp4");
}

#[tokio::test]
async fn existing_extension_is_not_replaced() {
    let server = TestServer::start_with(content(64), Mode::Range).await;
    server.set_headers(vec![(
        "Content-Disposition".to_string(),
        "attachment; filename=\"movie.avi\"".to_string(),
    )]);

    let file = probe(&options(&server, "/x"), resolver(&[("video/mp4", ".mp4")]))
        .await
        .expect("探测失败");
    assert_eq!(file.name, "movie.avi");
}

#[tokio::test]
async fn fallback_name_is_used_when_nothing_is_available() {
    // 根路径（没有 `/` 之后的片段）+ 没有任何文件名头 -> 用调用方给的兜底名 + 扩展名
    let server = TestServer::start_with(content(64), Mode::Range).await;
    server.set_headers(vec![("Content-Type".to_string(), "video/mp4".to_string())]);

    let file = probe(&options(&server, "/"), resolver(&[("video/mp4", ".mp4")]))
        .await
        .expect("探测失败");
    assert_eq!(file.name, "file_1700000000000000000.mp4");
}

#[tokio::test]
async fn last_modified_is_reported() {
    let server = TestServer::start_with(content(64), Mode::Range).await;
    server.set_headers(vec![(
        "Last-Modified".to_string(),
        "Wed, 21 Oct 2015 07:28:00 GMT".to_string(),
    )]);

    let file = probe(&options(&server, "/x.bin"), None)
        .await
        .expect("探测失败");
    assert_eq!(file.last_modified, "Wed, 21 Oct 2015 07:28:00 GMT");
}

#[tokio::test]
async fn server_error_status_is_reported_as_an_error() {
    let server = TestServer::start_with(content(64), Mode::Forbidden).await;
    let error = probe(&options(&server, "/file"), None)
        .await
        .expect_err("403 应当报错");
    assert!(
        error.to_string().contains("403"),
        "错误信息里应当带上状态码：{error}"
    );
}

#[tokio::test]
async fn unknown_size_stays_unknown_when_nothing_helps() {
    // 两个请求都不给任何长度信息 -> 大小保持未知
    let server = TestServer::start_with(
        content(64),
        Mode::Script(vec![Step::FullNoLength, Step::FullNoLength]),
    )
    .await;
    let file = probe(&options(&server, "/file.bin"), None)
        .await
        .expect("探测失败");

    assert_eq!(file.file_size, SIZE_UNKNOWN);
    assert!(!file.can_use_range_requests);
}
