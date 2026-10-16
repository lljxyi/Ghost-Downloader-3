//! GitHub pack 的桥。
//!
//! - `GD3_RUST_GITHUB=1` 影子模式：URL 判定、代理头过滤与响应匹配交给
//!   `gd3_core::github`。纯函数，无网络，无状态。
//! - `GD3_RUST_GITHUB_PROBE=1` 影子模式：候选竞速（`probeUrls`）交给
//!   `gd3_net::github`。会发网络请求，`matchFile` 判定也在里面完成。

use crate::probe::{ProbeOutcome, extension_resolver};
use crate::to_error;
use gd3_core::github;
use gd3_net::github::{CandidateError, probe_urls};
use gd3_net::probe::ProbeOptions;
use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;

create_exception!(
    gd3_py,
    GithubUnmatchedError,
    PyException,
    "候选响应未通过 matchFile 判定（对应 Python 的 TaskError(\"代理站响应不合格\")）。"
);

#[pyfunction]
pub fn github_is_file_url(url: &str) -> bool {
    github::is_github_file_url(url)
}

#[pyfunction]
pub fn github_match_file(
    original_url: &str,
    content_type: &str,
    can_use_range_requests: bool,
    file_size: u64,
) -> bool {
    github::match_file(
        original_url,
        content_type,
        can_use_range_requests,
        file_size,
    )
}

#[pyfunction]
pub fn github_proxy_headers(headers: Vec<(String, String)>) -> Vec<(String, String)> {
    github::proxy_headers(&headers)
}

/// 竞速一批候选 URL（镜像 `features/github_pack/probe.py::probeUrls`）。
///
/// `headers` / `user_agent` / `client_profile` / `fallback_name` 是**模板**：
/// 每个候选共用，只有「等于 `original_url` 的那个候选」拿原样头，其余剥凭据。
///
/// 返回 `(获胜者, 探测结果, 可回退的其余 URL)`。全部失败时抛异常：
/// `matchFile` 不过 → `GithubUnmatchedError`（调用方翻成 `TaskError("代理站响应不合格")`），
/// 其余（网络错、超时、非法头名）→ `ValueError`。
#[pyfunction]
#[pyo3(signature = (original_url, urls, headers, user_agent, client_profile, fallback_name, guess_extension = None))]
#[allow(clippy::too_many_arguments)]
pub fn github_probe_urls(
    py: Python<'_>,
    original_url: String,
    urls: Vec<String>,
    headers: Vec<(String, String)>,
    user_agent: String,
    client_profile: String,
    fallback_name: String,
    guess_extension: Option<Py<PyAny>>,
) -> PyResult<(String, ProbeOutcome, Vec<String>)> {
    let resolver = extension_resolver(guess_extension);

    // `url` 由 `probe_urls` 按候选填，这里只是把 `ProbeOptions` 的字段凑齐。
    let template = ProbeOptions {
        url: String::new(),
        headers,
        user_agent,
        client_profile,
        fallback_name,
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(to_error)?;

    let (winner, file, fallback) = py
        .detach(|| runtime.block_on(probe_urls(&original_url, &urls, &template, resolver)))
        .map_err(|error| {
            if error == CandidateError::Unmatched {
                GithubUnmatchedError::new_err(error.to_string())
            } else {
                to_error(error)
            }
        })?;

    Ok((winner, ProbeOutcome::from_file(file), fallback))
}
