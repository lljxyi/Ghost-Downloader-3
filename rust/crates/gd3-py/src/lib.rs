//! `gd3-core` / `gd3-net` 的 Python 桥（PyO3）。
//!
//! 只在影子模式下被 Python 侧调用：
//!
//! - `GD3_RUST_PERSIST=1` → `tasks.jsonl` 的序列化与读写交给 Rust；
//! - `GD3_RUST_HTTP=1` → HTTP Step 的传输交给 Rust 引擎。
//!
//! Python 仍然拥有领域对象与调度，产出的文件与纯 Python 路径一致。

mod bili;
mod browser;
mod github;
mod hf;
mod http;
mod listener;
mod probe;
use gd3_core::{TaskRecord, load_records, save_records, to_line as record_to_line};
use gd3_net::{HttpEngine, ProgressCallback, TransferProgress};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyList;
use pythonize::{depythonize, pythonize};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn to_error(error: impl std::fmt::Display) -> PyErr {
    PyValueError::new_err(error.to_string())
}

// ── tasks.jsonl ─────────────────────────────────────────────

/// 把一个 Task Record dict 序列化成一行（与 Python `json.dumps` 逐字节一致）。
#[pyfunction]
fn to_line(record: &Bound<'_, PyAny>) -> PyResult<String> {
    let value: serde_json::Value = depythonize(record).map_err(to_error)?;
    let parsed: TaskRecord = serde_json::from_value(value).map_err(to_error)?;
    record_to_line(&parsed).map_err(to_error)
}

/// 读 `tasks.jsonl`，返回 `(每行的 dict 列表, 坏行原因列表)`。
#[pyfunction]
fn load_lines(py: Python<'_>, path: &str) -> PyResult<(Py<PyAny>, Vec<String>)> {
    let outcome = load_records(std::path::Path::new(path)).map_err(to_error)?;

    let mut items: Vec<Py<PyAny>> = Vec::with_capacity(outcome.records.len());
    for record in &outcome.records {
        let value = serde_json::to_value(record).map_err(to_error)?;
        items.push(pythonize(py, &value).map_err(to_error)?.unbind());
    }

    let list = PyList::new(py, items).map_err(to_error)?;
    Ok((list.into_any().unbind(), outcome.failures))
}

/// 整表写入 `tasks.jsonl`（`.tmp` + 替换）。
#[pyfunction]
fn save_lines(path: &str, records: &Bound<'_, PyList>) -> PyResult<()> {
    let mut parsed = Vec::with_capacity(records.len());
    for record in records.iter() {
        let value: serde_json::Value = depythonize(&record).map_err(to_error)?;
        parsed.push(serde_json::from_value::<TaskRecord>(value).map_err(to_error)?);
    }
    save_records(std::path::Path::new(path), &parsed).map_err(to_error)
}

// ── HTTP 传输 ───────────────────────────────────────────────

/// 一次 HTTP Step 的结果。任务级错误不抛异常，由 Python 侧决定怎么变成 `TaskError`。
#[pyclass]
struct HttpOutcome {
    /// `completed` / `cancelled` / `failed`。
    #[pyo3(get)]
    status: String,
    /// 失败时的 `TaskError` 文案（i18n 键）。
    #[pyo3(get)]
    message: String,
    /// 失败时的格式化参数。
    #[pyo3(get)]
    params: Vec<(String, String)>,
    #[pyo3(get)]
    received_bytes: u64,
    /// 引擎最终状态：服务器拒绝范围请求时会降级成 false。
    #[pyo3(get)]
    can_use_range_requests: bool,
    /// 各分片的最终位置 `(start, position, end)`，供 Python 侧回写 step.subworkers。
    #[pyo3(get)]
    subworkers: Vec<(u64, u64, u64)>,
}

impl HttpOutcome {
    fn new(
        status: &str,
        message: String,
        params: Vec<(String, String)>,
        received: &AtomicU64,
        can_use_range_requests: bool,
        subworkers: Vec<(u64, u64, u64)>,
    ) -> Self {
        HttpOutcome {
            status: status.to_string(),
            message,
            params,
            received_bytes: received.load(Ordering::Relaxed),
            can_use_range_requests,
            subworkers,
        }
    }
}

/// 跑一个 HTTP Step。
///
/// `on_progress(received, total, chunk_delta)` 每个数据块调用一次（每秒采样时 `chunk_delta` 为 0），
/// 返回假值表示请求取消（暂停）。调用期间会释放 GIL，其他 Python 线程照常运行。
#[pyfunction]
#[pyo3(signature = (url, output_path, file_size, can_use_range_requests, subworker_count, headers, user_agent, client_profile, http_byte_offset, retry_delay_ms, on_progress, max_reassign_size = 512 * 1024, auto_speed_up = true))]
#[allow(clippy::too_many_arguments)]
fn http_run(
    py: Python<'_>,
    url: String,
    output_path: String,
    file_size: u64,
    can_use_range_requests: bool,
    subworker_count: usize,
    headers: Vec<(String, String)>,
    user_agent: String,
    client_profile: String,
    http_byte_offset: u64,
    retry_delay_ms: u64,
    on_progress: Py<PyAny>,
    max_reassign_size: u64,
    auto_speed_up: bool,
) -> PyResult<HttpOutcome> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(to_error)?;

    let cancel = CancellationToken::new();
    let received = Arc::new(AtomicU64::new(0));

    let callback = Arc::new(on_progress);
    let cancel_for_progress = cancel.clone();
    let received_for_progress = Arc::clone(&received);
    let progress: ProgressCallback = Arc::new(move |update: TransferProgress| {
        received_for_progress.store(update.received_bytes, Ordering::Relaxed);
        let keep_going = Python::attach(|py| {
            callback
                .bind(py)
                .call1((update.received_bytes, update.file_size, update.chunk_delta))
                .and_then(|value| value.is_truthy())
                .unwrap_or(true)
        });
        if !keep_going {
            cancel_for_progress.cancel();
        }
    });

    let mut engine = HttpEngine::new(url, output_path, file_size);
    engine.can_use_range_requests = can_use_range_requests;
    engine.subworker_count = subworker_count;
    engine.headers = headers;
    engine.user_agent = user_agent;
    engine.client_profile = client_profile;
    engine.http_byte_offset = http_byte_offset;
    engine.retry_delay = Duration::from_millis(retry_delay_ms);
    engine.max_reassign_size = max_reassign_size;
    engine.auto_speed_up = auto_speed_up;
    engine.on_progress = Some(progress);

    let result = py.detach(|| runtime.block_on(engine.run(cancel)));
    let can_use_range_requests = engine.can_use_range_requests;

    Ok(match result {
        Ok(()) => HttpOutcome::new(
            "completed",
            String::new(),
            Vec::new(),
            &received,
            can_use_range_requests,
            engine.finished_subworkers.clone(),
        ),
        Err(error) if error.is_cancelled() => HttpOutcome::new(
            "cancelled",
            String::new(),
            Vec::new(),
            &received,
            can_use_range_requests,
            engine.finished_subworkers.clone(),
        ),
        Err(error) => HttpOutcome::new(
            "failed",
            error.message.to_string(),
            error
                .params
                .iter()
                .map(|(name, value)| (name.to_string(), value.clone()))
                .collect(),
            &received,
            can_use_range_requests,
            engine.finished_subworkers.clone(),
        ),
    })
}

// ── 文件名工具（`app/platform/filesystem.py`） ──────────────

/// 净化文件名（Python `toSafeFilename`）。
///
/// `max_length` 用有符号：Python 侧允许负数，含义是「不截断」。
#[pyfunction]
#[pyo3(signature = (name, fallback = "file", max_length = 200))]
fn to_safe_filename(name: &str, fallback: &str, max_length: i64) -> String {
    let max_length = usize::try_from(max_length).unwrap_or(0);
    gd3_core::to_safe_filename(name, fallback, max_length)
}

/// 拆出主干与扩展名（Python `splitStemExt`）。
#[pyfunction]
fn split_stem_ext(name: &str) -> (String, String) {
    gd3_core::split_stem_ext(name)
}

#[pymodule]
fn gd3_py(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(to_line, module)?)?;
    module.add_function(wrap_pyfunction!(load_lines, module)?)?;
    module.add_function(wrap_pyfunction!(save_lines, module)?)?;
    module.add_function(wrap_pyfunction!(http_run, module)?)?;
    module.add_function(wrap_pyfunction!(probe::probe, module)?)?;
    module.add_function(wrap_pyfunction!(probe::probe_size, module)?)?;
    module.add_function(wrap_pyfunction!(hf::hf_fetch_tree, module)?)?;
    module.add_function(wrap_pyfunction!(hf::hf_parse_url, module)?)?;
    module.add_function(wrap_pyfunction!(hf::hf_build_files, module)?)?;
    module.add_function(wrap_pyfunction!(github::github_is_file_url, module)?)?;
    module.add_function(wrap_pyfunction!(github::github_match_file, module)?)?;
    module.add_function(wrap_pyfunction!(github::github_proxy_headers, module)?)?;
    module.add_function(wrap_pyfunction!(http::http_parser_match, module)?)?;
    module.add_function(wrap_pyfunction!(http::http_parser_match_passive, module)?)?;
    module.add_function(wrap_pyfunction!(bili::bili_build_fnval, module)?)?;
    module.add_function(wrap_pyfunction!(bili::bili_parse_url, module)?)?;
    module.add_function(wrap_pyfunction!(bili::bili_parse_dash, module)?)?;
    module.add_function(wrap_pyfunction!(bili::bili_build_pages, module)?)?;
    module.add_function(wrap_pyfunction!(bili::bili_fetch_view, module)?)?;
    module.add_function(wrap_pyfunction!(to_safe_filename, module)?)?;
    module.add_function(wrap_pyfunction!(split_stem_ext, module)?)?;
    module.add_class::<HttpOutcome>()?;
    module.add_class::<probe::ProbeOutcome>()?;
    module.add_class::<listener::Aria2Listener>()?;
    module.add_class::<browser::BrowserListener>()?;
    module.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
