//! m3u8 输出解析层的桥（`GD3_RUST_M3U8_OUTPUT=1` 影子模式）：
//! `features/m3u8_pack/task.py` 的 `parseDiagnostic` / `parseError` /
//! `M3U8TaskStep._parseOutputLine` 交给 `gd3_core::m3u8`。
//!
//! 这里只做数据搬运，不解释语义：
//!
//! - `parseError` 返回 `(message 模板, params)`，`params` 的键集合与 Python
//!   `TaskError.params` 逐字一致（命中 hint 时只有 `detail`；兜底时还有 `code`，
//!   且 `code` 是 **int** 而不是 str）；
//! - `_parseOutputLine` 返回 `(lastMessage, fields)`，`fields` 里只放
//!   **该写回**的键，`lastMessage is None` 表示这行 strip 之后是空的、
//!   调用方应当直接返回（Python 在赋值 `lastMessage` 之前就 return 了）。

use gd3_core::m3u8::{self, OutputUpdate};
use pyo3::prelude::*;
use pyo3::types::PyDict;

/// `parseDiagnostic(output)`。
#[pyfunction]
pub fn m3u8_parse_diagnostic(output: &str) -> String {
    m3u8::parse_diagnostic(output)
}

/// `parseError(output, returncode)` → `(message, params)`。
#[pyfunction]
pub fn m3u8_classify_error(
    py: Python<'_>,
    output: &str,
    returncode: i64,
) -> PyResult<(String, Py<PyDict>)> {
    let spec = m3u8::classify_error(output, returncode);
    let params = PyDict::new(py);
    if let Some(code) = spec.code {
        params.set_item("code", code)?;
    }
    params.set_item("detail", spec.detail)?;
    Ok((spec.message.to_string(), params.unbind()))
}

/// `M3U8TaskStep._parseOutputLine(line)` → `(lastMessage, fields)`。
#[pyfunction]
pub fn m3u8_parse_output_line(
    py: Python<'_>,
    line: &str,
) -> PyResult<(Option<String>, Py<PyDict>)> {
    let OutputUpdate {
        last_message,
        progress,
        speed,
        received_bytes,
        file_size,
        live_elapsed,
        live_total,
        live_status,
    } = m3u8::parse_output_line(line);

    let fields = PyDict::new(py);
    if let Some(progress) = progress {
        fields.set_item("progress", progress)?;
    }
    if let Some(speed) = speed {
        fields.set_item("speed", speed)?;
    }
    if let Some(received_bytes) = received_bytes {
        fields.set_item("receivedBytes", received_bytes)?;
    }
    if let Some(file_size) = file_size {
        fields.set_item("fileSize", file_size)?;
    }
    if let Some(live_elapsed) = live_elapsed {
        fields.set_item("liveElapsed", live_elapsed)?;
    }
    if let Some(live_total) = live_total {
        fields.set_item("liveTotal", live_total)?;
    }
    if let Some(live_status) = live_status {
        fields.set_item("liveStatus", live_status)?;
    }
    Ok((last_message, fields.unbind()))
}
