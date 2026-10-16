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

/// `HuggingFaceParser.parse` 里 entries → files 纯变换层的桥
/// （`GD3_RUST_HF_FILES=1` 影子模式）。`entries` 是 Python 传入的 JSON 可序列化
/// 条目列表（dict）；返回按顺序的 (path, size, downloadUrl)。
#[pyfunction]
pub fn hf_build_files(
    py: Python<'_>,
    entries: Vec<Bound<'_, PyAny>>,
    base: String,
    repo_id: String,
    repo_type: String,
    revision: String,
) -> PyResult<Vec<(String, u64, String)>> {
    let mut values = Vec::with_capacity(entries.len());
    for entry in &entries {
        values
            .push(pythonize::depythonize(entry).map_err(|e| PyValueError::new_err(e.to_string()))?);
    }
    let _ = py;
    Ok(
        gd3_net::hf::build_files(&values, &base, &repo_id, &repo_type, &revision)
            .into_iter()
            .map(|f| (f.path, f.size, f.download_url))
            .collect(),
    )
}

/// 单文件 HEAD 探测的桥（`GD3_RUST_HF_HEAD=1` 影子模式）：镜像
/// `features/huggingface_pack/pack.py:89-98`。返回 `(fileSize, canUseRange)`。
///
/// 注意 Python 侧**不检查状态码**，所以非 2xx 也照常返回头部信息；
/// `content-length` 不是整数时抛 `ValueError`（文案照抄 CPython `int()`）。
#[pyfunction]
pub fn hf_head_file(
    py: Python<'_>,
    url: String,
    headers: Vec<(String, String)>,
    client_profile: String,
) -> PyResult<(i64, bool)> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(to_error)?;

    let client = gd3_net::build_client(None, None, &client_profile);
    let head = py
        .detach(|| runtime.block_on(gd3_net::hf::head_file(&client, &headers, &url)))
        .map_err(err_to_py)?;
    Ok((head.file_size, head.can_use_range))
}
