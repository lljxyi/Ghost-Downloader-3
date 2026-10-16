//! HuggingFace 树枚举的桥（`GD3_RUST_HF_TREE=1` 影子模式）：
//! `features/huggingface_pack/pack.py::_fetchTree` 的网络与 JSON 解析交给
//! `gd3_net::hf::fetch_tree`。
//!
//! 401 → `PermissionError`；其余网络 / JSON 失败 → `ValueError`。

use crate::to_error;
use gd3_net::hf::{HfTreeEntry, HfTreeError};
use pyo3::exceptions::{PyPermissionError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyDict;

/// 一条树条目的结果，对应 Python 侧会拼回的 dict。
#[pyclass]
pub struct HfEntryOutcome {
    #[pyo3(get)]
    path: String,
    #[pyo3(get)]
    size: u64,
    /// `lfs.size`；None 表示没有或不是对象。
    #[pyo3(get)]
    lfs_size: Option<u64>,
}

/// 枚举一棵 HuggingFace 仓库树。
#[pyfunction]
pub fn hf_fetch_tree(
    py: Python<'_>,
    api_url: String,
    headers: Vec<(String, String)>,
    client_profile: String,
) -> PyResult<Vec<HfEntryOutcome>> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(to_error)?;

    let client = gd3_net::build_client(None, None, &client_profile);
    let result =
        py.detach(|| runtime.block_on(gd3_net::hf::fetch_tree(&client, &headers, &api_url)));
    let entries = result.map_err(err_to_py)?;
    Ok(entries
        .into_iter()
        .map(|entry: HfTreeEntry| HfEntryOutcome {
            path: entry.path,
            size: entry.size,
            lfs_size: entry.lfs_size,
        })
        .collect())
}

fn err_to_py(error: HfTreeError) -> PyErr {
    match error {
        HfTreeError::Auth => {
            PyPermissionError::new_err("该仓库需要授权，请在设置中配置 HuggingFace Access Token")
        }
        HfTreeError::Status(status) => PyValueError::new_err(status.to_string()),
        HfTreeError::Transport(message) => PyValueError::new_err(message),
    }
}

/// `parseHuggingFaceUrl` 的桥（`GD3_RUST_HF_URL=1` 影子模式）。
/// 返回 None 表示不匹配；成功时返回带四个字符串键的 dict。
#[pyfunction]
pub fn hf_parse_url(
    py: Python<'_>,
    url: String,
    proxy_site: String,
) -> PyResult<Option<Py<PyAny>>> {
    match gd3_net::hf::parse_url_entry(&url, &proxy_site) {
        None => Ok(None),
        Some(info) => {
            let dict = PyDict::new(py);
            dict.set_item("org", info.org)?;
            dict.set_item("repo", info.repo)?;
            dict.set_item("repoType", info.repo_type)?;
            dict.set_item("revision", info.revision)?;
            dict.set_item("filePath", info.file_path)?;
            Ok(Some(dict.into_any().unbind()))
        }
    }
}
