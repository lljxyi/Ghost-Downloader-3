//! `gd3-servers` 的 Python 桥：Rust 拥有套接字，Python 提供配置与动作回调。
//!
//! 边界：Rust 负责监听、HTTP/1.1 与 JSON-RPC 解析、WebSocket 帧；
//! Python 负责「读当前配置」与「把 Aria2Action 变成任务」。这两件事都是 Python 侧
//! 的既有能力（`cfg` 与 `featureService.parse` + `taskService.add`），所以不搬家。

use gd3_servers::{
    Aria2Action, Aria2Config, Aria2Service, ListenState, PortListener, aria2::Aria2TaskOptions,
};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::sync::Arc;
use std::time::Duration;

pub(crate) fn to_error(error: impl std::fmt::Display) -> PyErr {
    PyValueError::new_err(error.to_string())
}

/// 读当前配置；失败时给一份「未授权」配置，宁可拒绝也不放行。
fn read_config(callback: &Py<PyAny>) -> Aria2Config {
    Python::attach(|py| -> PyResult<Aria2Config> {
        let value = callback.bind(py).call0()?;
        let dict = value.cast::<PyDict>()?;
        let text = |key: &str| -> String {
            dict.get_item(key)
                .ok()
                .flatten()
                .and_then(|item| item.extract::<String>().ok())
                .unwrap_or_default()
        };
        let flag = |key: &str| -> bool {
            dict.get_item(key)
                .ok()
                .flatten()
                .and_then(|item| item.extract::<bool>().ok())
                .unwrap_or(false)
        };
        Ok(Aria2Config {
            version: text("version"),
            download_folder: text("downloadFolder"),
            token: text("token"),
            emulate_fingerprint: flag("emulateFingerprint"),
        })
    })
    .unwrap_or_else(|_| Aria2Config {
        version: String::new(),
        download_folder: String::new(),
        // 读配置失败时留一个不可能匹配的 token：宁可不干活，也不绕过鉴权
        token: "\u{0}config-unavailable".to_string(),
        emulate_fingerprint: false,
    })
}

/// 把动作交给 Python（`Aria2Action::AddTask` → dict）。
fn dispatch_action(callback: &Py<PyAny>, action: Aria2Action) {
    let Aria2Action::AddTask { gid, options } = action;
    let Aria2TaskOptions {
        url,
        output_folder,
        headers,
        client_profile,
        name,
    } = options;

    let _ = Python::attach(|py| -> PyResult<()> {
        let dict = PyDict::new(py);
        dict.set_item("kind", "addTask")?;
        dict.set_item("gid", gid)?;
        dict.set_item("url", url)?;
        dict.set_item("outputFolder", output_folder)?;
        dict.set_item("headers", headers)?;
        dict.set_item("clientProfile", client_profile)?;
        dict.set_item("name", name)?;
        callback.bind(py).call1((dict,))?;
        Ok(())
    });
}

/// 把监听状态交给 Python（四态 + 端口 + 失败原因）。
pub(crate) fn dispatch_state(callback: &Py<PyAny>, state: ListenState) {
    let _ = Python::attach(|py| -> PyResult<()> {
        let dict = PyDict::new(py);
        dict.set_item("status", state.status.as_str())?;
        dict.set_item("port", state.port)?;
        match &state.error {
            Some(error) => {
                dict.set_item("message", error.message)?;
                dict.set_item("params", error.params.clone())?;
            }
            None => {
                dict.set_item("message", py.None())?;
                dict.set_item("params", py.None())?;
            }
        }
        callback.bind(py).call1((dict,))?;
        Ok(())
    });
}

/// 一个由 Rust 拥有的回环监听器 + aria2 JSON-RPC 服务。
///
/// Python 侧只提供两个回调：
/// - `get_config()` → dict（version / downloadFolder / token / emulateFingerprint），每次请求读一次；
/// - `on_action(dict)` → 把 addUri 变成任务（解析失败也要在这里自己消化）。
///
/// 可选的第三个回调 `on_state(dict)` 会在监听状态变化时被调用。
#[pyclass]
pub struct Aria2Listener {
    /// `Option` 是为了在 `Drop` 里把运行时取出来关掉——`Drop` 只拿到 `&mut self`。
    runtime: Option<tokio::runtime::Runtime>,
    listener: Arc<PortListener>,
}

impl Aria2Listener {
    /// 运行时句柄；只有析构开始后才会是 `None`。
    fn runtime(&self) -> &tokio::runtime::Runtime {
        self.runtime.as_ref().expect("运行时")
    }
}

impl Drop for Aria2Listener {
    fn drop(&mut self) {
        let Some(runtime) = self.runtime.take() else {
            return;
        };
        // 与 `BrowserListener` 同理：关运行时前必须放掉 GIL，否则工作线程里
        // 正在等 GIL 的状态回调会让 `Runtime::drop` 永远等下去。
        let _ = Python::try_attach(|py| py.detach(move || drop(runtime)));
    }
}

#[pymethods]
impl Aria2Listener {
    #[new]
    #[pyo3(signature = (get_config, on_action, on_state=None))]
    fn new(
        get_config: Py<PyAny>,
        on_action: Py<PyAny>,
        on_state: Option<Py<PyAny>>,
    ) -> PyResult<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(to_error)?;

        let service = Arc::new(Aria2Service::new(
            Arc::new(move || read_config(&get_config)),
            Arc::new(move |action| dispatch_action(&on_action, action)),
        ));

        let listener = Arc::new(PortListener::new(Arc::new(move |stream| {
            let service = Arc::clone(&service);
            Box::pin(async move { service.handle(stream).await })
        })));

        if let Some(on_state) = on_state {
            let mut changes = listener.subscribe();
            let _guard = runtime.enter();
            tokio::spawn(async move {
                while let Ok(state) = changes.recv().await {
                    dispatch_state(&on_state, state);
                }
            });
        }

        Ok(Aria2Listener {
            runtime: Some(runtime),
            listener,
        })
    }

    /// `port=None` 关闭监听，`port=N` 监听。
    fn request(&self, port: Option<u16>) {
        let listener = Arc::clone(&self.listener);
        self.runtime().spawn(async move { listener.request(port) });
    }

    /// 关闭并等待收尾。阻塞期间释放 GIL，否则连接任务里的回调会卡死。
    fn stop(&self, py: Python<'_>) {
        let listener = Arc::clone(&self.listener);
        py.detach(|| self.runtime().block_on(listener.stop()));
    }

    /// 当前状态 dict（与 `on_state` 同形）。
    fn state<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        state_dict(py, self.listener.state())
    }

    /// 等状态变成 `status`，最多等 `timeoutMs`。给测试与启动流程用。
    fn wait_for<'py>(
        &self,
        py: Python<'py>,
        status: &str,
        timeout_ms: u64,
    ) -> PyResult<Bound<'py, PyDict>> {
        wait_for_state(py, &self.listener, status, timeout_ms)
    }
}

/// 监听状态 → dict（`status` / `port` / `message` / `params`）。
pub(crate) fn state_dict<'py>(py: Python<'py>, state: ListenState) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("status", state.status.as_str())?;
    dict.set_item("port", state.port)?;
    match &state.error {
        Some(error) => {
            dict.set_item("message", error.message)?;
            dict.set_item("params", error.params.clone())?;
        }
        None => {
            dict.set_item("message", py.None())?;
            dict.set_item("params", py.None())?;
        }
    }
    Ok(dict)
}

/// 等状态变成 `status`，最多等 `timeoutMs`。阻塞期间释放 GIL。
pub(crate) fn wait_for_state<'py>(
    py: Python<'py>,
    listener: &PortListener,
    status: &str,
    timeout_ms: u64,
) -> PyResult<Bound<'py, PyDict>> {
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let current = listener.state();
        if current.status.as_str() == status {
            return state_dict(py, current);
        }
        if std::time::Instant::now() >= deadline {
            return Err(to_error(format!(
                "等待状态 {status} 超时，当前是 {}",
                current.status.as_str()
            )));
        }
        py.detach(|| std::thread::sleep(Duration::from_millis(10)));
    }
}
