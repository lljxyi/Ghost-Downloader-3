"""HuggingFace 单文件 HEAD 探测的双实现 parity。

`GD3_RUST_HF_HEAD=1` 时走 Rust（`gd3_py.hf_head_file`），否则走 Python
`pack.py:89-98` 里 `client.head` 那段；两侧必须给出同样的
`(fileSize, canUseRange)`，或同样类型 + 同样文案的异常。

用裸 socket 服务器是为了**逐字节控制响应头**——`accept-ranges` 缺键、
`content-length` 非整数 / 负数这些边界，aiohttp 造不出来。
"""

from __future__ import annotations

import socket
import threading
from pathlib import Path

import pytest

from app.models.task import TaskOptions
from features.huggingface_pack import pack
from features.huggingface_pack.pack import HuggingFaceParser


class RawHeadServer:
    """每个连接都回同一份固定字节响应的极简 HTTP 服务器（只为 HEAD 探测用）。"""

    def __init__(self, payload: bytes):
        self._payload = payload
        self._sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self._sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self._sock.bind(("127.0.0.1", 0))
        self._sock.listen(8)
        self._sock.settimeout(0.1)
        self.port = self._sock.getsockname()[1]
        self._closing = threading.Event()
        self._thread = threading.Thread(target=self._serve, daemon=True)
        self._thread.start()

    def _serve(self) -> None:
        while not self._closing.is_set():
            try:
                conn, _ = self._sock.accept()
            except TimeoutError:
                continue
            except OSError:
                return
            try:
                conn.settimeout(5)
                conn.recv(65536)
                conn.sendall(self._payload)
            except OSError:
                pass
            finally:
                conn.close()

    @property
    def base(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    def close(self) -> None:
        self._closing.set()
        self._sock.close()
        self._thread.join(timeout=5)


@pytest.fixture
def headServer():
    """工厂：`start(payload) -> RawHeadServer`，测试结束统一关闭。"""
    started: list[RawHeadServer] = []

    def start(payload: bytes) -> RawHeadServer:
        server = RawHeadServer(payload)
        started.append(server)
        return server

    yield start
    for server in started:
        server.close()


def rawResponse(status: str = "200 OK", extra: tuple[tuple[str, str], ...] = ()) -> bytes:
    lines = [f"HTTP/1.1 {status}"]
    lines += [f"{name}: {value}" for name, value in extra]
    lines.append("Connection: close")
    return ("\r\n".join(lines) + "\r\n\r\n").encode()


async def runCase(server: RawHeadServer, monkeypatch, *, rust: bool) -> tuple:
    if rust:
        monkeypatch.setenv("GD3_RUST_HF_HEAD", "1")
    else:
        monkeypatch.delenv("GD3_RUST_HF_HEAD", raising=False)
    monkeypatch.setattr(pack, "selectedProxySite", lambda: server.base)

    options = TaskOptions(
        url=f"{server.base}/org/repo/resolve/main/f.bin",
        outputFolder=Path("/tmp"),
        headers={},
    )
    try:
        task = await HuggingFaceParser().parse(options)
    except Exception as error:
        return ("error", type(error).__name__, str(error))
    return ("ok", task.fileSize, task.steps[0].canUseRangeRequests)


async def runBoth(server: RawHeadServer, monkeypatch) -> tuple:
    pythonRun = await runCase(server, monkeypatch, rust=False)
    rustRun = await runCase(server, monkeypatch, rust=True)
    assert pythonRun == rustRun, f"两侧不一致：{pythonRun!r} != {rustRun!r}"
    return pythonRun


async def testParityContentLengthAndAcceptRanges(headServer, monkeypatch):
    server = headServer(rawResponse(extra=(("Content-Length", "1024"), ("Accept-Ranges", "bytes"))))
    assert await runBoth(server, monkeypatch) == ("ok", 1024, True)


async def testParityMissingAcceptRangesMeansNoRange(headServer, monkeypatch):
    server = headServer(rawResponse(extra=(("Content-Length", "12"),)))
    assert await runBoth(server, monkeypatch) == ("ok", 12, False)


async def testParityAcceptRangesValueIsIgnored(headServer, monkeypatch):
    """`Accept-Ranges: none` 也算「有」——只看键在不在。"""
    server = headServer(rawResponse(extra=(("Content-Length", "12"), ("Accept-Ranges", "none"))))
    assert await runBoth(server, monkeypatch) == ("ok", 12, True)


async def testParityNegativeContentLengthIsKept(headServer, monkeypatch):
    """Python 是 `int("-5") == -5`，两侧都得给出负数。"""
    server = headServer(rawResponse(extra=(("Content-Length", "-5"),)))
    assert await runBoth(server, monkeypatch) == ("ok", -5, False)


async def testParityMissingContentLengthIsZero(headServer, monkeypatch):
    server = headServer(rawResponse())
    assert await runBoth(server, monkeypatch) == ("ok", 0, False)


async def testParityNon2xxIsNotAnError(headServer, monkeypatch):
    """Python 侧没有 `raise_for_status`，404 也要照常返回头部信息。"""
    server = headServer(rawResponse("404 Not Found", (("Content-Length", "9"),)))
    assert await runBoth(server, monkeypatch) == ("ok", 9, False)


async def testParityNonNumericContentLengthRaisesValueError(headServer, monkeypatch):
    server = headServer(rawResponse(extra=(("Content-Length", "abc"),)))
    result = await runBoth(server, monkeypatch)
    assert result == (
        "error",
        "ValueError",
        "invalid literal for int() with base 10: 'abc'",
    )
