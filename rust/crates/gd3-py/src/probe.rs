//! URL 探测的桥（`GD3_RUST_PROBE=1` 影子模式）：
//! `features/http_pack/pack.py::probe` 的网络与解析交给 `gd3_net::probe`。
//!
//! 扩展名解析留给 Python：`mimetypes.guess_extension` 平台相关（Linux 读
//! `/etc/mime.types`，Windows 读注册表），由调用方传回调进来。

use crate::to_error;
use gd3_net::probe::{ExtensionResolver, ProbeOptions};
use pyo3::prelude::*;
use std::sync::Arc;

/// 一次探测的结果，字段与 `RemoteFile` dataclass 对应。
#[pyclass]
pub struct ProbeOutcome {
    #[pyo3(get)]
    name: String,
    #[pyo3(get)]
    file_size: u64,
    #[pyo3(get)]
    can_use_range_requests: bool,
    #[pyo3(get)]
    last_modified: String,
    #[pyo3(get)]
    content_type: String,
}

impl ProbeOutcome {
    pub(crate) fn from_file(file: gd3_net::probe::RemoteFile) -> Self {
        ProbeOutcome {
            name: file.name,
            file_size: file.file_size,
            can_use_range_requests: file.can_use_range_requests,
            last_modified: file.last_modified,
            content_type: file.content_type,
        }
    }
}

/// 把 Python 的 `guess_extension(content_type) -> 扩展名或 None` 包成 Rust 回调。
pub(crate) fn extension_resolver(guess_extension: Option<Py<PyAny>>) -> Option<ExtensionResolver> {
    guess_extension.map(|callback| {
        Arc::new(move |content_type: &str| -> Option<String> {
            Python::attach(|py| {
                callback
                    .bind(py)
                    .call1((content_type,))
                    .ok()
                    .and_then(|value| value.extract::<String>().ok())
            })
        }) as ExtensionResolver
    })
}

/// 镜像 `bili_pack.stream.probeSize`：任何失败（网络错、非法头名）都返回 0。
#[pyfunction]
#[pyo3(signature = (url, headers, client_profile))]
pub fn probe_size(
    py: Python<'_>,
    url: String,
    headers: Vec<(String, String)>,
    client_profile: String,
) -> PyResult<u64> {
    let client = gd3_net::engine::build_client(None, None, &client_profile);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(to_error)?;
    Ok(py.detach(|| runtime.block_on(gd3_net::probe::probe_size(&client, &headers, &url))))
}

/// 探测一个 URL。`guess_extension(content_type) -> 扩展名或 None` 由 Python 提供。
///
/// 非 200/206/416 的响应会抛 `ValueError`（镜像 Python 的 `raise_for_status`）。
#[pyfunction]
#[pyo3(signature = (url, headers, user_agent, client_profile, fallback_name, guess_extension = None))]
pub fn probe(
    py: Python<'_>,
    url: String,
    headers: Vec<(String, String)>,
    user_agent: String,
    client_profile: String,
    fallback_name: String,
    guess_extension: Option<Py<PyAny>>,
) -> PyResult<ProbeOutcome> {
    let resolver = extension_resolver(guess_extension);

    let options = ProbeOptions {
        url,
        headers,
        user_agent,
        client_profile,
        fallback_name,
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(to_error)?;

    let file = py
        .detach(|| runtime.block_on(gd3_net::probe::probe(&options, resolver)))
        .map_err(to_error)?;

    Ok(ProbeOutcome::from_file(file))
}
