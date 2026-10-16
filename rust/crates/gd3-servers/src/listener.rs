//! 回环端口监听：镜像 `app/services/port_listener.py`。
//!
//! 一个 actor 拥有「在本机回环端口上按配置监听」这件事：关闭、启动中、监听中、失败四态。
//! 与 Python 一致的行为：
//! - 先绑 `127.0.0.1`（失败即 FAILED），再试 `::1`（失败只降级，不影响状态）；
//! - Windows 上不设 `SO_REUSEADDR`——否则占用检测会失效；
//! - IPv6 套接字设 `IPV6_V6ONLY`，两个栈各管各的；
//! - 状态变化去重后才广播；停止会关闭已建立的连接。

use crate::ListenState;
use gd3_core::TaskError;
use socket2::{Domain, Protocol, Socket, Type};
use std::future::Future;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener as StdTcpListener};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// 每个连接的处理器：拿到的流已就绪，读写完自己关闭。
pub type ConnectionHandler =
    Arc<dyn Fn(tokio::net::TcpStream) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

struct Inner {
    handler: ConnectionHandler,
    state: Mutex<ListenState>,
    changes: broadcast::Sender<ListenState>,
    /// 当前一代的取消令牌；换代即取消旧代的接受循环与连接。
    cancel: Mutex<Option<CancellationToken>>,
    accept_loops: Mutex<Vec<JoinHandle<()>>>,
    connections: Mutex<Vec<JoinHandle<()>>>,
    /// 串行化「关闭 + 打开」，快速反复切换时最后一次请求说了算。
    transition: tokio::sync::Mutex<()>,
}

pub struct PortListener {
    inner: Arc<Inner>,
}

impl PortListener {
    pub fn new(handler: ConnectionHandler) -> Self {
        let (changes, _) = broadcast::channel(64);
        PortListener {
            inner: Arc::new(Inner {
                handler,
                state: Mutex::new(ListenState::off()),
                changes,
                cancel: Mutex::new(None),
                accept_loops: Mutex::new(Vec::new()),
                connections: Mutex::new(Vec::new()),
                transition: tokio::sync::Mutex::new(()),
            }),
        }
    }

    pub fn state(&self) -> ListenState {
        self.inner.state.lock().expect("状态锁").clone()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ListenState> {
        self.inner.changes.subscribe()
    }

    /// 请求目标状态：`Some(port)` 监听，`None` 关闭。
    ///
    /// `Some` 会先进入 STARTING（这样调用方在 bind 完成前也不会看到 OFF）。
    pub fn request(&self, port: Option<u16>) {
        if let Some(port) = port {
            self.set_state(ListenState::starting(port));
        }
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            let _guard = inner.transition.lock().await;
            close_all(&inner).await;
            match port {
                None => set_state(&inner, ListenState::off()),
                Some(port) => set_state(&inner, open(&inner, port).await),
            }
        });
    }

    /// 关闭监听并等待收尾完成（Python `stop()`）。
    pub async fn stop(&self) {
        let inner = Arc::clone(&self.inner);
        let _guard = inner.transition.lock().await;
        close_all(&inner).await;
        set_state(&inner, ListenState::off());
    }

    fn set_state(&self, state: ListenState) {
        set_state(&self.inner, state);
    }
}

fn set_state(inner: &Arc<Inner>, state: ListenState) {
    {
        let mut current = inner.state.lock().expect("状态锁");
        if *current == state {
            return;
        }
        *current = state.clone();
    }
    let _ = inner.changes.send(state);
}

/// 建一个已 bind + listen 的回环套接字；与 Python `createSocket` 同参数。
fn create_socket(host: &str, port: u16) -> std::io::Result<StdTcpListener> {
    let is_v6 = host.contains(':');
    let domain = if is_v6 { Domain::IPV6 } else { Domain::IPV4 };
    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;

    // Windows 上设 SO_REUSEADDR 会允许抢占别人的端口，占用检测就失效了
    if !cfg!(windows) {
        socket.set_reuse_address(true)?;
    }
    if is_v6 {
        socket.set_only_v6(true)?;
    }

    let addr = if is_v6 {
        SocketAddr::from((host.parse::<Ipv6Addr>().expect("回环 IPv6 地址"), port))
    } else {
        SocketAddr::from((host.parse::<Ipv4Addr>().expect("回环 IPv4 地址"), port))
    };
    socket.bind(&addr.into())?;
    socket.listen(1024)?;
    socket.set_nonblocking(true)?;
    Ok(socket.into())
}

/// 绑定失败 → `TaskError`（Python `toListenError`）。
pub fn to_listen_error(error: &std::io::Error, port: u16) -> TaskError {
    match error.raw_os_error() {
        // EADDRINUSE / WSAEADDRINUSE
        Some(98) | Some(10048) => TaskError::port_in_use(port),
        // EACCES / WSAEACCES
        Some(13) | Some(10013) => TaskError::port_denied(port),
        _ => TaskError::port_unavailable(port),
    }
}

async fn open(inner: &Arc<Inner>, port: u16) -> ListenState {
    let listener = match create_socket("127.0.0.1", port) {
        Ok(listener) => listener,
        Err(error) => {
            return ListenState::failed(port, to_listen_error(&error, port));
        }
    };

    let cancel = CancellationToken::new();
    {
        let mut current = inner.cancel.lock().expect("取消锁");
        *current = Some(cancel.clone());
    }

    // ::1 是尽力而为：占用或不可用时只降级，不影响整体状态
    if let Ok(v6) = create_socket("::1", port) {
        spawn_accept_loop(inner, v6, cancel.clone());
    }
    spawn_accept_loop(inner, listener, cancel);

    ListenState::listening(port)
}

fn spawn_accept_loop(inner: &Arc<Inner>, listener: StdTcpListener, cancel: CancellationToken) {
    let loop_inner = Arc::clone(inner);
    let handle = tokio::spawn(async move {
        let listener = match TcpListener::from_std(listener) {
            Ok(listener) => listener,
            Err(_) => return,
        };
        loop {
            let accepted = tokio::select! {
                _ = cancel.cancelled() => return,
                accepted = listener.accept() => accepted,
            };
            let Ok((stream, _)) = accepted else {
                if cancel.is_cancelled() {
                    return;
                }
                continue;
            };

            let handler = Arc::clone(&loop_inner.handler);
            let connection_cancel = cancel.clone();
            let task = tokio::spawn(async move {
                tokio::select! {
                    _ = connection_cancel.cancelled() => {}
                    _ = handler(stream) => {}
                }
            });
            let mut connections = loop_inner.connections.lock().expect("连接锁");
            connections.retain(|task| !task.is_finished());
            connections.push(task);
        }
    });

    let mut loops = inner.accept_loops.lock().expect("接受循环锁");
    loops.push(handle);
}

/// 关闭接受循环与所有在途连接（Python `_close`）。
async fn close_all(inner: &Arc<Inner>) {
    if let Some(cancel) = inner.cancel.lock().expect("取消锁").take() {
        cancel.cancel();
    }

    let loops: Vec<JoinHandle<()>> =
        std::mem::take(&mut *inner.accept_loops.lock().expect("接受循环锁"));
    let connections: Vec<JoinHandle<()>> =
        std::mem::take(&mut *inner.connections.lock().expect("连接锁"));

    for handle in &loops {
        handle.abort();
    }
    for handle in &connections {
        handle.abort();
    }

    // 让被 abort 的任务真正退出，端口才来得及释放
    for handle in loops.into_iter().chain(connections) {
        let _ = tokio::time::timeout(Duration::from_millis(500), handle).await;
    }
}
