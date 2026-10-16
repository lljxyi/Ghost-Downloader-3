"""Rust aria2 监听器适配器（影子模式，默认关闭）。

`GD3_RUST_ARIA2=1` 时，回环监听、HTTP/1.1 与 JSON-RPC 解析、WebSocket 帧都由
`rust/crates/gd3-servers` 承担；Python 只提供两件事：

- 读当前配置（`cfg` 里的版本、下载目录、token、指纹开关）；
- 把 `addTask` 动作交给 `Aria2RpcService` 既有的解析与加入流程。

与 Python 实现的已知差异：状态变化回调来自 Rust 的线程，必须经 `dispatcher`
（应用里是 `coroutineRunner.post`）搬回 UI 线程；不传就不发信号，只更新 `state`。
"""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path
from threading import Lock

from app.config.cfg import cfg
from app.config.constants import VERSION
from app.models.task import TaskOptions
from app.services.port_listener import ListenState, ListenStatus
from app.signal import Signal


def isEnabled() -> bool:
    import os

    return os.environ.get("GD3_RUST_ARIA2") == "1"


class RustAria2Listener:
    """与 `PortListener` 同形的公开面：`start()` / `stop()` / `state` / `stateChanged`。"""

    stateChanged = Signal(object)

    def __init__(self, rpcService, *, isEnabled, port, dispatcher: Callable | None = None) -> None:
        self._rpc = rpcService
        self._isEnabled = isEnabled
        self._port = port
        self._dispatcher = dispatcher
        self._state = ListenState()
        self._lock = Lock()
        self._listener = None

    @property
    def state(self) -> ListenState:
        with self._lock:
            return self._state

    def start(self) -> None:
        import gd3_py

        self._listener = gd3_py.Aria2Listener(self._getConfig, self._onAction, self._onState)
        self._isEnabled.valueChanged.connect(self._onConfigChanged)
        self._port.valueChanged.connect(self._onConfigChanged)
        self._onConfigChanged()

    def stop(self) -> None:
        self._isEnabled.valueChanged.disconnect(self._onConfigChanged)
        self._port.valueChanged.disconnect(self._onConfigChanged)
        self._request(None)

    def _onConfigChanged(self, *_) -> None:
        self._request(self._port.value if self._isEnabled.value else None)

    def _request(self, port: int | None) -> None:
        if self._listener is None:
            return
        if port is not None:
            self._setState(ListenState(ListenStatus.STARTING, port))
        self._listener.request(port)

    def _getConfig(self) -> dict:
        return {
            "version": VERSION,
            "downloadFolder": str(cfg.downloadFolder.value),
            "token": cfg.aria2RpcToken.value,
            "emulateFingerprint": cfg.aria2RpcEmulateFingerprint.value,
        }

    def _onAction(self, action: dict) -> None:
        if action.get("kind") != "addTask":
            return
        options = TaskOptions(
            url=action["url"],
            outputFolder=Path(action["outputFolder"]),
            headers=dict(action["headers"]),
            clientProfile=action["clientProfile"],
        )
        self._rpc.applyTaskOptions(options, action["name"])

    def _onState(self, state: dict) -> None:
        """在 Rust 的线程上被调用，只做线程安全的更新与派发。"""
        if self._dispatcher is None:
            self._storeState(state)
            return
        self._dispatcher(lambda: self._storeState(state))

    def _storeState(self, state: dict) -> None:
        message = state.get("message")
        error = None
        if message is not None:
            from app.models.task import TaskError

            error = TaskError(message, **dict(state.get("params") or {}))
        self._setState(ListenState(ListenStatus(state["status"]), state["port"], error))

    def _setState(self, state: ListenState) -> None:
        with self._lock:
            if state == self._state:
                return
            self._state = state
        self.stateChanged.emit(state)
