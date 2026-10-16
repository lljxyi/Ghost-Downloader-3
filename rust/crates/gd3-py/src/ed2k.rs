//! ed2k 链接解析的桥（`GD3_RUST_ED2K_LINK=1` 影子模式）：
//! `features/ed2k_pack/kelpie/models.py::Link.parse` 交给 `gd3_core::ed2k::parse`。

use gd3_core::ed2k;
use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;

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
