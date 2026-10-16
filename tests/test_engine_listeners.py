"""应用层接线：`app/engine.py` 选到哪个监听器，那个监听器就得真的能服务。

`tests/test_browser_service.py` / `tests/test_aria2_rpc.py` 直接构造监听器，绕过了
`Engine` 的选择逻辑；这里补上那一段——两套监听器都要能用**应用里真实的**
`BrowserService` / `Aria2RpcService` 跑通一次往返，并且能干净地停掉。
"""

from __future__ import annotations

import asyncio
import io
import json
import socket
import struct
import zipfile

import pytest
import websockets

from app.config.cfg import cfg
from app.config.constants import VERSION
from app.engine import Engine
from app.services import rust_aria2 as rustAria2
from app.services import rust_browser as rustBrowser
from app.services.browser_service import PROTOCOL_VERSION
from app.services.coroutine_runner import CoroutineRunner
from app.services.port_listener import ListenStatus, PortListener
from tests.test_aria2_rpc import Aria2Client
from tests.test_port_listener import findFreePort, waitFor

pytestmark = pytest.mark.asyncio(loop_factories=["asyncio", "uvloop"])

TOKEN = "engine-pair-token"


class StubFileWatcher:
    class _Signal:
        def connect(self, _):
            pass

    fileChanged = _Signal()

    def addPath(self, _):
        pass

    def removePath(self, _):
        pass


def buildCrx() -> bytes:
    archive = io.BytesIO()
    with zipfile.ZipFile(archive, "w") as zf:
        zf.writestr("manifest.json", "{}")
    header = b"signed-header"
    return b"Cr24" + struct.pack("<II", 3, len(header)) + header + archive.getvalue()


def isPortFree(port: int) -> bool:
    with socket.socket(socket.AF_INET) as probe:
        try:
            probe.bind(("127.0.0.1", port))
        except OSError:
            return False
    return True


@pytest.fixture
async def engine(monkeypatch, tmp_path):
    import app.services.task_service as taskServiceModule

    monkeypatch.setattr(taskServiceModule, "APP_DATA_DIR", tmp_path / "data")
    monkeypatch.setattr(cfg, "set", lambda item, value, save=True: monkeypatch.setattr(item, "value", value))
    monkeypatch.setattr(cfg.browserExtensionPairToken, "value", TOKEN)
    monkeypatch.setattr(cfg.aria2RpcToken, "value", "")
    monkeypatch.setattr(cfg.shouldDraftTakenDownload, "value", False)
    monkeypatch.setattr(cfg.downloadFolder, "value", str(tmp_path))
    monkeypatch.setattr(cfg.isBrowserExtensionEnabled, "value", True)
    monkeypatch.setattr(cfg.isAria2RpcEnabled, "value", True)
    monkeypatch.setattr(cfg.browserExtensionPort, "value", findFreePort())
    monkeypatch.setattr(cfg.aria2RpcPort, "value", findFreePort())

    loop = asyncio.get_running_loop()
    # Rust 侧的回调来自 Rust 线程，派发器必须线程安全（应用里是 QTimer.singleShot）
    runner = CoroutineRunner(loop.call_soon_threadsafe, loop=loop)
    built = Engine(runner, StubFileWatcher(), buildCrx, tmp_path / "ext", lambda path: None)
    built.start()
    await waitFor(lambda: built.browserListener.state.status == ListenStatus.LISTENING)
    await waitFor(lambda: built.aria2RpcListener.state.status == ListenStatus.LISTENING)

    yield built

    # `Engine.stop()` 里的 `FeatureService.deactivate()` 会阻塞等事件循环，
    # 所以在测试里从线程调用它，别把事件循环本身堵住。
    # 用例自己停过就不再停一次（重复停会重复 disconnect 信号）。
    if built.browserListener.state.status != ListenStatus.OFF:
        await asyncio.to_thread(built.stop)


async def test_listener_choice_follows_the_switch(engine):
    """开关决定用哪套实现——这是 `Engine` 唯一的接线点。"""
    if rustBrowser.isEnabled():
        assert isinstance(engine.browserListener, rustBrowser.RustBrowserListener)
    else:
        assert isinstance(engine.browserListener, PortListener)
        assert engine.browserListener.state.port == cfg.browserExtensionPort.value

    if rustAria2.isEnabled():
        assert isinstance(engine.aria2RpcListener, rustAria2.RustAria2Listener)
    else:
        assert isinstance(engine.aria2RpcListener, PortListener)


class TestBrowserRoundTrip:
    async def test_handshake_over_the_app_service(self, engine):
        async with websockets.connect(f"ws://127.0.0.1:{engine.browserListener.state.port}/") as ws:
            await ws.send(json.dumps({"type": "hello", "protocolVersion": PROTOCOL_VERSION,
                                      "token": TOKEN, "extensionVersion": "2.2.0",
                                      "installType": "normal"}))
            ack = json.loads(await asyncio.wait_for(ws.recv(), 3))
            await waitFor(lambda: engine.browserService.connectionSummary == ("normal", "2.2.0"))

        assert ack["type"] == "hello_ack"
        assert ack["appVersion"] == VERSION

    async def test_bad_token_is_refused(self, engine):
        async with websockets.connect(f"ws://127.0.0.1:{engine.browserListener.state.port}/") as ws:
            await ws.send(json.dumps({"type": "hello", "protocolVersion": PROTOCOL_VERSION,
                                      "token": "guess", "extensionVersion": "2.2.0"}))
            error = json.loads(await asyncio.wait_for(ws.recv(), 3))

        assert error["code"] == "unauthorized"


class TestAria2RoundTrip:
    async def test_getVersion_over_the_app_service(self, engine):
        client = Aria2Client(engine.aria2RpcListener.state.port, [], [], [])
        response = await client.post({"jsonrpc": "2.0", "id": "a", "method": "aria2.getVersion",
                                      "params": []})

        assert response.status == 200
        assert response.json() == {"jsonrpc": "2.0", "id": "a",
                                   "result": {"version": VERSION, "enabledFeatures": ["HTTPS"]}}


async def test_stopping_the_engine_releases_both_ports(engine):
    ports = (engine.browserListener.state.port, engine.aria2RpcListener.state.port)
    await asyncio.to_thread(engine.stop)

    assert engine.browserListener.state.status == ListenStatus.OFF
    assert engine.aria2RpcListener.state.status == ListenStatus.OFF
    for port in ports:
        await waitFor(lambda port=port: isPortFree(port))
