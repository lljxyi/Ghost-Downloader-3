//! HuggingFace 单文件 HEAD 探测（`gd3_net::hf::head_file`）的集成测试。
//!
//! 规格对应 `features/huggingface_pack/pack.py:89-98`（Python 侧 `client.head` 那段）。
//! 服务器实现见 `tests/support/test_server.rs`。
//!
//! 重点是把 Python 那几处**看起来像 bug 但必须照抄**的语义钉住：
//! 不检查状态码、`accept-ranges` 只看键、`content-length` 的 `int()` 语义。

#[path = "support/test_server.rs"]
mod test_server;

use gd3_net::hf::{HfTreeError, head_file};
use test_server::{Mode, Step, TestServer};

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

fn client() -> wreq::Client {
    gd3_net::build_client(None, None, "raw")
}

/// `Mode::Range` 的 200 全量响应带 `Content-Length` 与 `Accept-Ranges: bytes`。
#[tokio::test]
async fn content_length_and_accept_ranges_are_read() {
    let server = TestServer::start(content(1024)).await;
    let client = client();

    let head = head_file(&client, &[], &server.url("/file"))
        .await
        .expect("HEAD 应当成功");

    assert_eq!(head.file_size, 1024);
    assert!(head.can_use_range);
    assert_eq!(server.requests()[0].method, "HEAD");
}

/// 没有 `accept-ranges` 键 → `can_use_range = false`。
///
/// 用 `Mode::Forbidden` 是因为它回 403 + `Content-Length`，且**不带**
/// `Accept-Ranges`；顺带钉住「Python 不检查状态码」这条。
#[tokio::test]
async fn forbidden_responses_still_report_headers() {
    let server = TestServer::start_with(content(1024), Mode::Forbidden).await;
    let client = client();

    let head = head_file(&client, &[], &server.url("/file"))
        .await
        .expect("Python 侧没有 raise_for_status，403 也要正常返回");

    assert_eq!(head.file_size, b"forbidden\n".len() as i64);
    assert!(!head.can_use_range);
}

/// `Accept-Ranges: none` 也算「有」——只看键在不在。
#[tokio::test]
async fn accept_ranges_value_is_ignored() {
    let server = TestServer::start(content(1024)).await;
    server.set_headers(vec![("Accept-Ranges".to_string(), "none".to_string())]);
    let client = client();

    let head = head_file(&client, &[], &server.url("/file"))
        .await
        .expect("HEAD 应当成功");

    assert_eq!(head.file_size, 1024);
    assert!(head.can_use_range);
}

/// 没有 `content-length` 头 → 0（不是错误）。
#[tokio::test]
async fn missing_content_length_is_zero() {
    let server =
        TestServer::start_with(content(1024), Mode::Script(vec![Step::FullNoLength])).await;
    let client = client();

    let head = head_file(&client, &[], &server.url("/file"))
        .await
        .expect("HEAD 应当成功");

    assert_eq!(head.file_size, 0);
    assert!(!head.can_use_range);
}

/// `content-length` 为负数：Python `int("-5") == -5`，所以 `file_size` 也能是负的。
#[tokio::test]
async fn negative_content_length_is_kept() {
    let server = TestServer::start(content(1024)).await;
    server.set_headers(vec![("Content-Length".to_string(), "-5".to_string())]);
    let client = client();

    let head = head_file(&client, &[], &server.url("/file"))
        .await
        .expect("HEAD 应当成功");

    assert_eq!(head.file_size, -5);
    // `Mode::Range` 的 200 响应照旧带 `Accept-Ranges`，这里只关心 `file_size`。
    assert!(head.can_use_range);
}

/// 非整数的 `content-length` → `int()` 的 `ValueError`，文案照抄 CPython。
#[tokio::test]
async fn non_numeric_content_length_is_an_error() {
    let server = TestServer::start(content(1024)).await;
    server.set_headers(vec![("Content-Length".to_string(), "abc".to_string())]);
    let client = client();

    let error = head_file(&client, &[], &server.url("/file"))
        .await
        .expect_err("abc 不是整数");

    assert_eq!(
        error,
        HfTreeError::Transport("invalid literal for int() with base 10: 'abc'".to_string())
    );
}
