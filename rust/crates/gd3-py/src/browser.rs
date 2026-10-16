//! 浏览器扩展的 WebSocket 监听桥（影子模式）。
//!
//! 边界：Rust 拥有套接字、WebSocket 握手与帧、会话登记与发送队列；
//! Python 保留全部领域决策——握手鉴权、配对、任务创建、任务动作、快照内容。
//!
//! 为什么这么切：协议里的每条消息都要读 Task 状态或走 TaskService，
//! 把决策搬到 Rust 只会把同一件事拆成两处；套接字与帧才是与领域无关的部分。

use crate::listener::{dispatch_state, state_dict, to_error, wait_for_state};
use futures_util::{SinkExt, StreamExt};
use gd3_servers::{ConnectionHandler, PortListener, websocket::WebSocket};
use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio_tungstenite::tungstenite::Message;

/// 单条消息上限（Python 侧 `maxSize=2**20`）。
const MAX_MESSAGE_SIZE: usize = 1024 * 1024;

/// 一条已建立的会话：发送队列按顺序泵进 WebSocket。
struct Session {
    sender: UnboundedSender<Message>,
}

type Sessions = Arc<Mutex<HashMap<u64, Session>>>;

/// `(on_connected, on_message, on_disconnected)`——包进 `Arc` 后可以廉价克隆给守卫。
type Callbacks = (Py<PyAny>, Py<PyAny>, Py<PyAny>);

fn notify_connected(callback: &Py<PyAny>, id: u64, peer: &str) {
    let _ = Python::attach(|py| -> PyResult<()> {
        callback.bind(py).call1((id, peer))?;
        Ok(())
    });
}

fn notify_message(callback: &Py<PyAny>, id: u64, text: &str) {
    let _ = Python::attach(|py| -> PyResult<()> {
        callback.bind(py).call1((id, text))?;
        Ok(())
    });
}

fn notify_disconnected(callback: &Py<PyAny>, id: u64) {
    let _ = Python::attach(|py| -> PyResult<()> {
        callback.bind(py).call1((id,))?;
        Ok(())
    });
}

/// 会话收尾守卫。
///
/// Python 的 `handle()` 用 `finally` 保证「断开一定要通知」，而 `gd3-servers` 的
/// `close_all` 是 abort 连接任务——写在循环后面的收尾代码会被直接丢掉，
/// 所以收尾必须挂在 Drop 上。
struct SessionGuard {
    sessions: Sessions,
    id: u64,
    callbacks: Arc<Callbacks>,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.sessions.lock().expect("会话锁").remove(&self.id);
        notify_disconnected(&self.callbacks.2, self.id);
    }
}

/// 由 Rust 拥有的浏览器扩展监听器。
///
/// Python 侧回调：`on_connected(sessionId, peerAddress)` / `on_message(sessionId, text)` /
/// `on_disconnected(sessionId)`；可选的 `on_state(dict)` 报告监听状态变化。
/// 并由 Python 主动调用 `send(sessionId, text)` 与 `close(sessionId)`。
#[pyclass]
pub struct BrowserListener {
    /// `Option` 是为了在 `Drop` 里把运行时取出来关掉——`Drop` 只拿到 `&mut self`。
    runtime: Option<tokio::runtime::Runtime>,
    listener: Arc<PortListener>,
    sessions: Sessions,
}

impl BrowserListener {
    /// 运行时句柄；只有析构开始后才会是 `None`。
    fn runtime(&self) -> &tokio::runtime::Runtime {
        self.runtime.as_ref().expect("运行时")
    }
}

impl Drop for BrowserListener {
    fn drop(&mut self) {
        let Some(runtime) = self.runtime.take() else {
            return;
        };
        // 关运行时前必须放掉 GIL：工作线程可能正卡在 `Python::attach` 上等 GIL
        // （断开通知、状态回调都会回调 Python），拿着 GIL 等它们退出就是死锁。
        // `try_attach` 失败时闭包被丢弃，运行时照样会关掉。
        let _ = Python::try_attach(|py| py.detach(move || drop(runtime)));
    }
}

#[pymethods]
impl BrowserListener {
    #[new]
    #[pyo3(signature = (on_connected, on_message, on_disconnected, on_state=None))]
    fn new(
        on_connected: Py<PyAny>,
        on_message: Py<PyAny>,
        on_disconnected: Py<PyAny>,
        on_state: Option<Py<PyAny>>,
    ) -> PyResult<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(to_error)?;

        let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));
        let next_id = Arc::new(AtomicU64::new(1));

        // 处理器是 `Fn`：闭包会被反复调用，所以每次都要克隆句柄。
        // 克隆必须在闭包外做——写在闭包体里会让外层变量被整体移动。
        let callbacks = Arc::new((on_connected, on_message, on_disconnected));
        let handler_sessions = Arc::clone(&sessions);
        let handler_ids = Arc::clone(&next_id);
        let handler_callbacks = Arc::clone(&callbacks);

        let handler: ConnectionHandler = Arc::new(move |stream: TcpStream| {
            let session_map = Arc::clone(&handler_sessions);
            let session_ids = Arc::clone(&handler_ids);
            let handlers = Arc::clone(&handler_callbacks);
            Box::pin(async move {
                let peer = stream
                    .peer_addr()
                    .map(|address| address.to_string())
                    .unwrap_or_default();
                run_session(stream, peer, session_map, session_ids, handlers).await;
            })
        });

        let listener = Arc::new(PortListener::new(handler));
        if let Some(on_state) = on_state {
            let mut changes = listener.subscribe();
            let _guard = runtime.enter();
            tokio::spawn(async move {
                while let Ok(state) = changes.recv().await {
                    dispatch_state(&on_state, state);
                }
            });
        }

        Ok(BrowserListener {
            runtime: Some(runtime),
            listener,
            sessions,
        })
    }

    fn request(&self, port: Option<u16>) {
        let listener = Arc::clone(&self.listener);
        self.runtime().spawn(async move { listener.request(port) });
    }

    fn stop(&self, py: Python<'_>) {
        let listener = Arc::clone(&self.listener);
        py.detach(|| self.runtime().block_on(listener.stop()));
    }

    fn state<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        state_dict(py, self.listener.state())
    }

    fn wait_for<'py>(
        &self,
        py: Python<'py>,
        status: &str,
        timeout_ms: u64,
    ) -> PyResult<Bound<'py, PyDict>> {
        wait_for_state(py, &self.listener, status, timeout_ms)
    }

    /// 按顺序把文本发给某个会话；会话不存在就丢弃。
    fn send(&self, session_id: u64, text: &str) {
        let sessions = self.sessions.lock().expect("会话锁");
        if let Some(session) = sessions.get(&session_id) {
            let _ = session.sender.send(Message::Text(text.into()));
        }
    }

    fn close(&self, session_id: u64) {
        let session = self.sessions.lock().expect("会话锁").remove(&session_id);
        if let Some(session) = session {
            let _ = session.sender.send(Message::Close(None));
        }
    }

    fn session_count(&self) -> usize {
        self.sessions.lock().expect("会话锁").len()
    }
}

async fn run_session(
    mut stream: TcpStream,
    peer: String,
    sessions: Sessions,
    next_id: Arc<AtomicU64>,
    callbacks: Arc<Callbacks>,
) {
    // 握手要读 head，所以先自己读一次再交给 WebSocket
    let Ok(Some(head)) =
        gd3_servers::websocket::read_head(&mut stream, gd3_servers::MAX_HEAD_SIZE).await
    else {
        return;
    };
    let Some(request) = gd3_servers::parse_head(&head) else {
        return;
    };

    let Ok(Some(websocket)) = WebSocket::accept(stream, &request.headers, MAX_MESSAGE_SIZE).await
    else {
        return;
    };
    let (mut sink, mut source) = websocket.into_inner().split();

    let id = next_id.fetch_add(1, Ordering::Relaxed);
    let (sender, mut receiver) = unbounded_channel::<Message>();
    sessions
        .lock()
        .expect("会话锁")
        .insert(id, Session { sender });

    // 会话登记之后断开就必须通知 Python，所以收尾交给守卫，不写在函数末尾
    let _guard = SessionGuard {
        sessions: Arc::clone(&sessions),
        id,
        callbacks: Arc::clone(&callbacks),
    };

    notify_connected(&callbacks.0, id, &peer);

    let pump = tokio::spawn(async move {
        while let Some(message) = receiver.recv().await {
            if sink.send(message).await.is_err() {
                break;
            }
        }
    });

    while let Some(message) = source.next().await {
        match message {
            Ok(Message::Text(text)) => notify_message(&callbacks.1, id, &text),
            Ok(Message::Binary(data)) => {
                if let Ok(text) = String::from_utf8(data.to_vec()) {
                    notify_message(&callbacks.1, id, &text);
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(Message::Ping(payload)) => {
                let sessions = sessions.lock().expect("会话锁");
                if let Some(session) = sessions.get(&id) {
                    let _ = session.sender.send(Message::Pong(payload));
                }
            }
            Ok(_) => continue,
            Err(_) => break,
        }
    }

    pump.abort();
}
