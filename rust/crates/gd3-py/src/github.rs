//! GitHub pack 判定层的桥（`GD3_RUST_GITHUB` 影子模式）：
//! `features/github_pack` 的 URL 判定、代理头过滤与响应匹配交给
//! `gd3_core::github`。纯函数，无网络，无状态。

use gd3_core::github;
use pyo3::prelude::*;

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
