"""Rust 浏览器扩展监听器适配器（影子模式，默认关闭）。

`GD3_RUST_BROWSER=1` 时，回环监听、HTTP/1.1 与 WebSocket 握手/帧由
`rust/crates/gd3-servers` + `gd3-py::BrowserListener` 承担；Python 保留全部领域决策
——握手鉴权、配对、任务创建、任务动作、快照内容。

与 Python 实现的已知差异：

- 会话身份是 Rust 的 `sessionId`（整数），但 `BrowserService._sessions` 的键仍是一个与
  `WebSocketStream` 同形的流对象（`RustSessionStream`），所以 `_send` / `_close` /
  `_sendSnapshots` 一行都不用改；
- 消息由 Rust 解好帧后以 `str` 交给 Python（Python 路径给的是 `bytes`），`json.loads` 两者都收；
- 来自 Rust 线程的回调经 `coroutineRunner.post` 搬回主线程，与 `BrowserService.handle()`
  的既有行为一致——因此 `post` 用的派发器必须是线程安全的（应用里是 `QTimer.singleShot`）。
"""

from __future__ import annotations

from app.services.port_listener import ListenState, ListenStatus
from app.signal import Signal


def isEnabled() -> bool:
    import os

    return os.environ.get("GD3_RUST_BROWSER") == "1"


class RustSessionStream:
    """与 `WebSocketStream` 同形的会话面：`send(text)` / `close()`。

    实例本身就是会话句柄，按身份作 `BrowserClientSession` 的键。
    """

    def __init__(self, listener, sessionId: int) -> None:
        self._listener = listener
        self._sessionId = sessionId

    async def send(self, text: str) -> None:
        self._listener.send(self._sessionId, text)

    async def close(self) -> None:
        self._listener.close(self._sessionId)


class RustBrowserListener:
    """与 `PortListener` 同形的公开面：`start()` / `stop()` / `state` / `stateChanged`。"""

    stateChanged = Signal(object)

    def __init__(self, coroutineRunner, service, *, isEnabled, port) -> None:
        self._coroutineRunner = coroutineRunner
        self._service = service
        self._isEnabled = isEnabled
        self._port = port
        self._state = ListenState()
        self._streams: dict[int, RustSessionStream] = {}
        self._listener = None

    @property
    def state(self) -> ListenState:
        return self._state

    def start(self) -> None:
        import gd3_py

        self._listener = gd3_py.BrowserListener(
            self._onConnected, self._onMessage, self._onDisconnected, self._onState,
        )
        self._isEnabled.valueChanged.connect(self._onConfigChanged)
        self._port.valueChanged.connect(self._onConfigChanged)
        self._onConfigChanged()

    def stop(self) -> None:
        self._isEnabled.valueChanged.disconnect(self._onConfigChanged)
        self._port.valueChanged.disconnect(self._onConfigChanged)
        if self._listener is None:
            return
        listener, self._listener = self._listener, None
        listener.stop()

    def _onConfigChanged(self, *_) -> None:
        if self._listener is None:
            return
        port = self._port.value if self._isEnabled.value else None
        if port is not None:
            self._setState(ListenState(ListenStatus.STARTING, port))
        self._listener.request(port)

    # ── 来自 Rust 线程：只做派发，不碰领域状态 ──────────────────

    def _onConnected(self, sessionId: int, peerAddress: str) -> None:
        self._coroutineRunner.post(self._connect, sessionId, peerAddress)

    def _onMessage(self, sessionId: int, text: str) -> None:
        self._coroutineRunner.post(self._receive, sessionId, text)

    def _onDisconnected(self, sessionId: int) -> None:
        self._coroutineRunner.post(self._disconnect, sessionId)

    def _onState(self, state: dict) -> None:
        self._coroutineRunner.post(self._storeState, state)

    def _connect(self, sessionId: int, peerAddress: str) -> None:
        stream = RustSessionStream(self._listener, sessionId)
        self._streams[sessionId] = stream
        self._service.onRustConnected(stream, peerAddress)

    def _receive(self, sessionId: int, text: str) -> None:
        stream = self._streams.get(sessionId)
        if stream is not None:
            self._service.onRustMessage(stream, text)

    def _disconnect(self, sessionId: int) -> None:
        stream = self._streams.pop(sessionId, None)
        if stream is not None:
            self._service.onRustDisconnected(stream)

    def _storeState(self, state: dict) -> None:
        message = state.get("message")
        error = None
        if message is not None:
            from app.models.task import TaskError

            error = TaskError(message, **dict(state.get("params") or {}))
        self._setState(ListenState(ListenStatus(state["status"]), state["port"], error))

    def _setState(self, state: ListenState) -> None:
        if state == self._state:
            return
        self._state = state
        self.stateChanged.emit(state)
