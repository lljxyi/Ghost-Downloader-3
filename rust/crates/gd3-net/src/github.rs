//! GitHub pack 的候选竞速（镜像 `features/github_pack/probe.py::probeUrls`）。
//!
//! 只在 `GD3_RUST_GITHUB_PROBE=1` 影子模式下由 `gd3-py` 调用，默认走 Python。
//!
//! 语义要点（都照 Python 抄，别"优化"）：
//!
//! - 每个候选的请求头：等于 `original_url` 的那个用原样头，其余走
//!   `gd3_core::github::proxy_headers`（剥掉 `authorization` / `cookie`）；
//! - 每个候选整体限时 [`CANDIDATE_TIMEOUT`]（Python 是 `asyncio.wait_for(probe(...), 5)`）；
//! - **谁先完成谁先看**：第一个「探测成功且 `match_file` 通过」的候选获胜，其余立刻取消；
//! - 探测成功但 `match_file` 不过的候选进 `unmatched`，它们**不会**出现在返回的
//!   fallback 列表里；传输失败的候选同样进 `unmatched`（Python 也是这么做的）；
//! - 全部失败时抛出「最后完成的那个候选」的错误 —— 与 Python 一样取决于竞速结果，
//!   不保证是哪一个（Python 里 `done` 是 set，顺序同样不确定）。
//!
//! 空 `urls` 在 Python 里会走到 `raise lastError`（`lastError` 为 `None`）→
//! `TypeError`。这里给一条明确的传输错误，调用方拿到的仍然是异常。

use crate::probe::{ExtensionResolver, ProbeOptions, RemoteFile, probe};
use gd3_core::github;
use std::time::Duration;

/// 单个候选的整体超时（Python `asyncio.wait_for(probe(...), 5)`）。
pub const CANDIDATE_TIMEOUT: Duration = Duration::from_secs(5);

/// 单个候选的失败原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateError {
    /// 探测成功，但 `match_file` 判定不合格（Python `TaskError("代理站响应不合格")`）。
    Unmatched,
    /// 传输失败：网络错、非法头名、非 200/206/416 的状态码。
    Transport(String),
    /// 整体超时（Python `asyncio.wait_for` 抛的 `TimeoutError`，`str()` 为空串）。
    Timeout,
}

impl std::fmt::Display for CandidateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CandidateError::Unmatched => formatter.write_str("代理站响应不合格"),
            CandidateError::Transport(message) => formatter.write_str(message),
            CandidateError::Timeout => formatter.write_str(""),
        }
    }
}

impl std::error::Error for CandidateError {}

/// 竞速一批候选 URL，返回 `(获胜者, 探测结果, 可回退的其余 URL)`。
///
/// `template` 提供除 `url` 之外的全部探测参数；`original_url` 决定哪个候选拿原样头。
pub async fn probe_urls(
    original_url: &str,
    urls: &[String],
    template: &ProbeOptions,
    resolve_extension: Option<ExtensionResolver>,
) -> Result<(String, RemoteFile, Vec<String>), CandidateError> {
    if urls.is_empty() {
        return Err(CandidateError::Transport("没有候选 URL".to_string()));
    }

    let mut candidates = tokio::task::JoinSet::new();

    for url in urls {
        let candidate = url.clone();
        let headers = if candidate == original_url {
            template.headers.clone()
        } else {
            github::proxy_headers(&template.headers)
        };
        let options = ProbeOptions {
            url: candidate.clone(),
            headers,
            user_agent: template.user_agent.clone(),
            client_profile: template.client_profile.clone(),
            fallback_name: template.fallback_name.clone(),
        };
        let resolver = resolve_extension.clone();
        candidates.spawn(async move {
            let outcome = tokio::time::timeout(CANDIDATE_TIMEOUT, probe(&options, resolver)).await;
            (candidate, outcome)
        });
    }

    let mut unmatched: Vec<String> = Vec::new();
    let mut last_error: Option<CandidateError> = None;

    while let Some(joined) = candidates.join_next().await {
        let (url, outcome) = match joined {
            Ok(value) => value,
            // 任务被 abort / panic：Python 里没有对应情形，按传输失败处理。
            Err(error) => {
                last_error = Some(CandidateError::Transport(error.to_string()));
                continue;
            }
        };

        match outcome {
            Ok(Ok(file))
                if github::match_file(
                    original_url,
                    &file.content_type,
                    file.can_use_range_requests,
                    file.file_size,
                ) =>
            {
                let fallback = urls
                    .iter()
                    .filter(|candidate| **candidate != url && !unmatched.contains(candidate))
                    .cloned()
                    .collect();
                candidates.shutdown().await;
                return Ok((url, file, fallback));
            }
            Ok(Ok(_)) => {
                unmatched.push(url);
                last_error = Some(CandidateError::Unmatched);
            }
            Ok(Err(error)) => {
                unmatched.push(url);
                last_error = Some(CandidateError::Transport(error.to_string()));
            }
            Err(_) => {
                unmatched.push(url);
                last_error = Some(CandidateError::Timeout);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| CandidateError::Transport("没有候选 URL".to_string())))
}
