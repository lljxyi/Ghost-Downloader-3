//! Bilibili 解析层的桥（`GD3_RUST_BILI_PARSE` 影子模式）：
//! `bili_pack.stream` 的 `parseDash` / `buildFnval` 交给 `gd3_net::bili`。
//! 配置值（alternativeQuality、HDR/Dolby 开关）由 Python 传入。

use gd3_net::bili;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use pythonize::depythonize;

fn error(message: String) -> PyErr {
    PyValueError::new_err(message)
}

/// 镜像 `parseDash`：返回 `(videoUrl, audioUrl, videoSize, audioSize)`。
/// 非 0 出口（videoStreams 空 / matchStream 失败）抛 `ValueError`，文案与 Python 一致。
#[pyfunction]
#[pyo3(signature = (dash, duration, qn, accept_quality, audio_qn, alternative))]
pub fn bili_parse_dash(
    dash: &Bound<'_, PyAny>,
    duration: i64,
    qn: i64,
    accept_quality: Vec<i64>,
    audio_qn: i64,
    alternative: &str,
) -> PyResult<(String, String, i64, i64)> {
    let value: serde_json::Value = depythonize(dash)?;
    let outcome = bili::parse_dash(&value, duration, qn, &accept_quality, audio_qn, alternative)
        .map_err(error)?;
    Ok((
        outcome.video_url,
        outcome.audio_url,
        outcome.video_size,
        outcome.audio_size,
    ))
}

/// 镜像 `buildFnval(qn)`。
#[pyfunction]
#[pyo3(signature = (qn, include_hdr, include_dolby))]
pub fn bili_build_fnval(qn: i64, include_hdr: bool, include_dolby: bool) -> i64 {
    bili::build_fnval(qn, include_hdr, include_dolby)
}

/// `parseBiliUrl` 的桥（`GD3_RUST_BILI_URL=1` 影子模式）。
/// 成功 → 与 `BiliUrl` 字段同构的 dict；失败 → `ValueError`（文案与 Python 逐字一致）。
#[pyfunction]
pub fn bili_parse_url(py: Python<'_>, url: String) -> PyResult<Py<PyAny>> {
    let dict = PyDict::new(py);
    match gd3_net::bili::parse_url(&url) {
        Ok(outcome) => {
            dict.set_item("videoId", outcome.video_id)?;
            dict.set_item("mid", outcome.mid)?;
            dict.set_item("seasonId", outcome.season_id)?;
            dict.set_item("selectedPages", outcome.selected_pages)?;
            Ok(dict.into_any().unbind())
        }
        Err(message) => Err(PyValueError::new_err(message)),
    }
}
