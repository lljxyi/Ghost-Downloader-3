//! GitHub 候选竞速（`gd3_net::github::probe_urls`）的集成测试。
//!
//! 规格对应 `features/github_pack/probe.py::probeUrls`。服务器实现见
//! `tests/support/test_server.rs`。
//!
//! 竞速本身**不保证赢家是谁**（两侧都取决于完成顺序），所以每个用例都构造成
//! 「只有一个候选能赢」或「断言与顺序无关」，别写靠时间取胜的断言。

#[path = "support/test_server.rs"]
mod test_server;

use gd3_net::github::{CandidateError, probe_urls};
use gd3_net::probe::ProbeOptions;
use test_server::{Mode, TestServer};

/// 典型的 GitHub release 资产地址：`match_file` 走「支持范围且大小 > 0」分支。
const ASSET_URL: &str = "https://github.com/o/r/releases/download/v1/a.tar.gz";

/// 归档地址：`match_file` 只要求 content-type 不是 `text/*`。
const ARCHIVE_URL: &str = "https://github.com/o/r/archive/refs/heads/main.zip";

/// 连不上的地址：127.0.0.1:1 上没有服务，`connect` 立刻被拒（不依赖 DNS）。
const UNREACHABLE_URL: &str = "http://127.0.0.1:1/a.tar.gz";

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

fn template(headers: Vec<(String, String)>) -> ProbeOptions {
    ProbeOptions {
        url: ASSET_URL.to_string(),
        headers,
        user_agent: "gd3-test/1.0".to_string(),
        client_profile: "raw".to_string(),
        fallback_name: "file_1700000000000000000".to_string(),
    }
}

fn header_value(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

/// 探测成功但 `match_file` 不过的候选不算赢家，也不进回退列表。
#[tokio::test]
async fn unmatched_candidates_are_dropped_from_the_fallback_list() {
    // `IgnoreRange` 回 200 + Content-Length，`can_use_range_requests` 为 false。
    let unmatched = TestServer::start_with(content(4096), Mode::IgnoreRange).await;
    let good = TestServer::start_with(content(4096), Mode::Range).await;

    let urls = vec![unmatched.url("/file"), good.url("/file")];
    let (winner, file, fallback) = probe_urls(ASSET_URL, &urls, &template(Vec::new()), None)
        .await
        .expect("至少有一个候选应当胜出");

    assert_eq!(winner, good.url("/file"));
    assert_eq!(file.file_size, 4096);
    assert!(file.can_use_range_requests);
    assert!(fallback.is_empty(), "不合格的候选不该出现在回退列表里");
}

/// 归档地址只要求 content-type 不是 `text/*`：两个候选都能赢，回退列表是另一个。
#[tokio::test]
async fn archive_urls_accept_any_non_text_content_type() {
    let first = TestServer::start_with(content(1024), Mode::Range).await;
    let second = TestServer::start_with(content(1024), Mode::Range).await;

    let urls = vec![first.url("/file"), second.url("/file")];
    let (winner, _, fallback) = probe_urls(ARCHIVE_URL, &urls, &template(Vec::new()), None)
        .await
        .expect("两个候选都应当合格");

    assert!(urls.contains(&winner));
    assert_eq!(fallback.len(), 1);
    assert_ne!(fallback[0], winner);
}

/// 全部候选都不合格 -> `CandidateError::Unmatched`（Python `TaskError("代理站响应不合格")`）。
#[tokio::test]
async fn all_unmatched_reports_unmatched() {
    let first = TestServer::start_with(content(4096), Mode::IgnoreRange).await;
    let second = TestServer::start_with(content(4096), Mode::IgnoreRange).await;

    let urls = vec![first.url("/file"), second.url("/file")];
    let error = probe_urls(ASSET_URL, &urls, &template(Vec::new()), None)
        .await
        .expect_err("两个候选都不该合格");

    assert_eq!(error, CandidateError::Unmatched);
    assert_eq!(error.to_string(), "代理站响应不合格");
}

/// 传输失败（403）-> `CandidateError::Transport`，文案取自底层错误。
#[tokio::test]
async fn forbidden_candidates_report_a_transport_error() {
    let first = TestServer::start_with(content(64), Mode::Forbidden).await;
    let second = TestServer::start_with(content(64), Mode::Forbidden).await;

    let urls = vec![first.url("/file"), second.url("/file")];
    let error = probe_urls(ASSET_URL, &urls, &template(Vec::new()), None)
        .await
        .expect_err("403 不该被当成合格响应");

    assert!(matches!(error, CandidateError::Transport(_)), "{error:?}");
    assert!(!error.to_string().is_empty());
}

/// 代理候选（URL ≠ 原始 URL）剥掉 `authorization` / `cookie`，其余头照发。
#[tokio::test]
async fn proxy_candidates_lose_credentials_but_keep_other_headers() {
    let proxy = TestServer::start_with(content(4096), Mode::Range).await;
    let headers = vec![
        ("Authorization".to_string(), "token t".to_string()),
        ("Cookie".to_string(), "c=1".to_string()),
        ("X-Other".to_string(), "1".to_string()),
    ];

    let urls = vec![proxy.url("/file"), UNREACHABLE_URL.to_string()];
    let (winner, _, fallback) = probe_urls(UNREACHABLE_URL, &urls, &template(headers), None)
        .await
        .expect("代理候选应当胜出");

    assert_eq!(winner, proxy.url("/file"));
    assert!(fallback.is_empty(), "连不上的原始 URL 不该进回退列表");

    let requests = proxy.requests();
    assert_eq!(requests.len(), 1);
    let seen = &requests[0].headers;
    assert_eq!(header_value(seen, "x-other").as_deref(), Some("1"));
    assert!(header_value(seen, "authorization").is_none());
    assert!(header_value(seen, "cookie").is_none());
}

/// 原始 URL 那个候选保留全部头（含凭据）。
#[tokio::test]
async fn the_original_candidate_keeps_credentials() {
    let direct = TestServer::start_with(content(4096), Mode::Range).await;
    let headers = vec![
        ("Authorization".to_string(), "token t".to_string()),
        ("Cookie".to_string(), "c=1".to_string()),
    ];

    let original = direct.url("/file");
    let urls = vec![original.clone(), UNREACHABLE_URL.to_string()];
    let (winner, _, _) = probe_urls(&original, &urls, &template(headers), None)
        .await
        .expect("原始 URL 应当胜出");

    assert_eq!(winner, original);

    let requests = direct.requests();
    assert_eq!(requests.len(), 1);
    let seen = &requests[0].headers;
    assert_eq!(
        header_value(seen, "authorization").as_deref(),
        Some("token t")
    );
    assert_eq!(header_value(seen, "cookie").as_deref(), Some("c=1"));
}
