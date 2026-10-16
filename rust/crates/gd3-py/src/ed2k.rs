//! ed2k 链接解析的桥（`GD3_RUST_ED2K_LINK=1` 影子模式）：
//! `features/ed2k_pack/kelpie/models.py::Link.parse` 交给 `gd3_core::ed2k::parse`。
//! progress 消息变换的桥（`GD3_RUST_ED2K_PROGRESS=1` 影子模式）：
//! `features/ed2k_pack/kelpie/kelpie.py::parseProgress` 交给 `gd3_core::ed2k::parse_progress`。

use gd3_core::ed2k;
use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyKeyError, PyTypeError};
use pyo3::prelude::*;
use pythonize::{depythonize, pythonize};

create_exception!(
    gd3_py,
    Ed2kInvalidLinkError,
    PyException,
    "不是有效的 eD2k 链接（对应 Python 的 Error(ErrorCode.INVALID_LINK, message)）。"
);

/// `Link.parse(text)` → `(name, size, hash)`。
/// 解析失败抛 `Ed2kInvalidLinkError`，参数是 Python `Error.message` 的原文。
#[pyfunction]
pub fn ed2k_parse_link(text: &str) -> PyResult<(String, i64, String)> {
    let link = ed2k::parse(text).map_err(|error| Ed2kInvalidLinkError::new_err(error.message))?;
    #[expect(clippy::cast_possible_wrap, reason = "size <= MAX_SIZE < i64::MAX")]
    Ok((link.name, link.size as i64, link.hash))
}

/// `kelpie.parseProgress(message)` → `(hash, size, received, downloadRate,
/// uploadRate, uploaded, peers, activePeers, heldSources, heldUntil, sources)`，
/// sources 是 `(address, software, status, rank, downloadRate, channel)` 元组列表。
/// 字段值原样透传；缺字段抛 `KeyError`，`sources` 不可迭代 / 元素不可下标抛
/// `TypeError`（文案在核心层逐字拼好，与 CPython 报错一致）。
#[pyfunction]
pub fn ed2k_parse_progress(
    py: Python<'_>,
    message: &Bound<'_, PyAny>,
) -> PyResult<Py<pyo3::types::PyTuple>> {
    let message: serde_json::Value = depythonize(message)?;
    match ed2k::parse_progress(&message) {
        Ok(shape) => {
            let pairs = [
                &shape.hash,
                &shape.size,
                &shape.received,
                &shape.download_rate,
                &shape.upload_rate,
                &shape.uploaded,
                &shape.peers,
                &shape.active_peers,
                &shape.held_sources,
                &shape.held_until,
            ];
            let mut items = Vec::with_capacity(11);
            for value in pairs {
                items.push(pythonize(py, value)?.unbind());
            }
            let mut sources = Vec::with_capacity(shape.sources.len());
            for source in &shape.sources {
                let mut tuple = Vec::with_capacity(6);
                for value in source {
                    tuple.push(pythonize(py, value)?.unbind());
                }
                sources.push(pyo3::types::PyTuple::new(py, tuple)?.unbind());
            }
            items.push(sources.into_pyobject(py)?.unbind().into_any());
            Ok(pyo3::types::PyTuple::new(py, items)?.unbind())
        }
        Err(ed2k::ProgressParseError::Key(key)) => Err(PyKeyError::new_err(key)),
        Err(ed2k::ProgressParseError::Type(text)) => Err(PyTypeError::new_err(text)),
    }
}
