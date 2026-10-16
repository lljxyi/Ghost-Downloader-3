//! m3u8 清单 parse 的桥（`GD3_RUST_M3U8_PARSE=1` 影子模式）：
//! `m3u8.loads(body)` 的扫描面交给 `gd3_core::hls::probe`。
//!
//! 返回「标签元组」而不是抛 `Err`：Python 侧适配器按 `kind` 挑同型异常类
//! 再抛，使 `f"{type(e).__name__}: {e}"` 与 Python 原生逐字节一致。
//!
//! - `("ok", "", is_endlist, segment_count, [(uri, width, height, codecs,
//!   frame_rate), ...])`
//! - `("error", "Kind: message", False, 0, [])`

use gd3_core::hls;
use pyo3::prelude::*;
use pyo3::types::{PyList, PyTuple};

/// `m3u8_probe(body)` → 见模块头。
#[pyfunction]
pub fn m3u8_probe<'py>(
    py: Python<'py>,
    body: &str,
) -> PyResult<(String, String, bool, u64, Py<PyList>)> {
    match hls::probe(body) {
        Ok(probe) => {
            let variants: Vec<Bound<'py, PyTuple>> = probe
                .variants
                .into_iter()
                .map(|variant| {
                    PyTuple::new(
                        py,
                        [
                            variant.uri.into_pyobject(py)?.unbind().into_any(),
                            variant.width.into_pyobject(py)?.unbind().into_any(),
                            variant.height.into_pyobject(py)?.unbind().into_any(),
                            variant
                                .codecs
                                .clone()
                                .into_pyobject(py)?
                                .unbind()
                                .into_any(),
                            variant.frame_rate.into_pyobject(py)?.unbind().into_any(),
                        ],
                    )
                })
                .collect::<Result<_, _>>()?;
            let list = PyList::new(py, variants)?;
            Ok((
                "ok".to_string(),
                String::new(),
                probe.is_endlist,
                probe.segment_count,
                list.unbind(),
            ))
        }
        Err(error) => {
            let message = format!("{}: {}", error.kind, error.message);
            let empty = PyList::empty(py);
            Ok(("error".to_string(), message, false, 0, empty.unbind()))
        }
    }
}
