"""`GD3_RUST_GITHUB_PROBE` 影子开关的双实现 parity 测试。

每个用例都跑两遍 —— 开关关（`github_pack.probe.probeUrls` 的 Python 实现）与
开关开（`github_pack.rust_probe.probeUrls` → `gd3_net::github::probe_urls`）——
比较「获胜者 / 探测结果 / 可回退列表 / 各候选实际收到的请求头」，并额外断言
具体期望值，避免两侧一起错还能通过。

竞速本身有不确定性，所以每个用例要么构造成「只有一个候选能赢」，要么只断言
与顺序无关的部分（比如「谁赢都行，但代理站一定拿不到凭据」）。
"""

from __future__ import annotations

import asyncio

from aiohttp import web

from app.models.task import TaskOptions
from github_pack.probe import probeUrls

ASSET_URL = "https://github.com/o/r/releases/download/v1/a.tar.gz"

# 每个候选都是独立端口，`host` 头必然不同；其余头必须逐个相同。
VOLATILE_HEADERS = {"host"}


def buildSiteHandler(
    *, delay: float = 0.0, status: int = 206, contentType: str = "application/octet-stream",
    requests: list | None = None,
):
    async def handler(request: web.Request) -> web.Response:
        if requests is not None:
            requests.append(dict(request.headers))
        await asyncio.sleep(delay)
        headers = {"Content-Type": contentType}
        if status == 206:
            headers["Content-Range"] = "bytes 1-1/100"
        return web.Response(status=status, body=b"x", headers=headers)

    return handler


def normalize(records: list[list[dict]]) -> list[list[dict]]:
    """把每个候选收到的请求头归一成可比结构（头名小写、去掉易变头）。"""
    return [
        [
            {name.lower(): value for name, value in entry.items() if name.lower() not in VOLATILE_HEADERS}
            for entry in record
        ]
        for record in records
    ]


def describe(winnerIndex: int | None, file, fallbackIndexes: list[int] | None) -> dict:
    return {
        "winner": winnerIndex,
        "file": None if file is None else (
            file.name, file.fileSize, file.canUseRangeRequests, file.contentType, file.lastModified,
        ),
        "fallback": fallbackIndexes,
    }


async def runCase(server, monkeypatch, *, rust: bool, sites: list[dict], originalUrl: str,
                  headers: dict[str, str] | None = None, urlsFactory=None) -> dict:
    """跑一遍竞速，返回与实现无关的可比结构。"""
    if rust:
        monkeypatch.setenv("GD3_RUST_GITHUB_PROBE", "1")
    else:
        monkeypatch.delenv("GD3_RUST_GITHUB_PROBE", raising=False)

    urls, records = [], []
    for spec in sites:
        record: list = []
        records.append(record)
        urls.append(await server(buildSiteHandler(requests=record, **spec)))
    if urlsFactory is not None:
        urls = urlsFactory(urls)

    result = {"winner": None, "file": None, "fallback": None, "error": None}
    try:
        winner, file, fallback = await probeUrls(TaskOptions(url=originalUrl, headers=headers or {}), urls)
    except Exception as error:  # noqa: BLE001 —— 两侧的异常类型正是要比的东西
        result["error"] = (type(error).__name__, str(error))
    else:
        result.update(describe(
            urls.index(winner), file, sorted(urls.index(url) for url in fallback),
        ))

    result["requests"] = normalize(records)
    return result


async def runBoth(server, monkeypatch, **kwargs) -> tuple[dict, dict]:
    pythonRun = await runCase(server, monkeypatch, rust=False, **kwargs)
    rustRun = await runCase(server, monkeypatch, rust=True, **kwargs)
    return pythonRun, rustRun


async def testParityUnmatchedCandidateIsDroppedFromFallback(server, monkeypatch):
    # 不合格的候选（200 + text/html）先完成，合格的候选慢一点 —— 结果确定。
    pythonRun, rustRun = await runBoth(
        server, monkeypatch,
        sites=[{"status": 200, "contentType": "text/html"}, {"delay": 0.5}],
        originalUrl=ASSET_URL,
    )

    assert pythonRun == rustRun
    assert pythonRun["winner"] == 1
    assert pythonRun["fallback"] == []
    assert pythonRun["file"][1] == 100
    assert pythonRun["file"][2] is True
    assert pythonRun["file"][3] == "application/octet-stream"


async def testParityTransportFailureIsDroppedFromFallback(server, monkeypatch):
    # 连不上的候选（端口 1，不做 DNS）会立刻失败，不进回退列表。
    pythonRun, rustRun = await runBoth(
        server, monkeypatch,
        sites=[{"delay": 0.5}],
        originalUrl=ASSET_URL,
        urlsFactory=lambda urls: ["http://127.0.0.1:1/a.tar.gz", *urls],
    )

    assert pythonRun == rustRun
    assert pythonRun["winner"] == 1
    assert pythonRun["fallback"] == []
    assert pythonRun["error"] is None


async def testParityPendingCandidateStaysInFallback(server, monkeypatch):
    # 赢家 0.1 秒完成，另一个还挂着 —— 挂着的没被判定不合格，所以留在回退列表里。
    pythonRun, rustRun = await runBoth(
        server, monkeypatch,
        sites=[{"delay": 0.1}, {"delay": 1.0}],
        originalUrl=ASSET_URL,
    )

    assert pythonRun == rustRun
    assert pythonRun["winner"] == 0
    assert pythonRun["fallback"] == [1]
    assert pythonRun["error"] is None


async def testParitySingleCandidateIsTheOriginalUrl(server, monkeypatch):
    # `github_pack/pack.py` 选定了站点时只传一个候选，且它就是原始 URL。
    pythonRun, rustRun = await runBoth(
        server, monkeypatch,
        sites=[{"delay": 0.0}],
        originalUrl=ASSET_URL,
    )

    assert pythonRun == rustRun
    assert pythonRun["winner"] == 0
    assert pythonRun["fallback"] == []


async def testParityAllUnmatchedRaisesTaskError(server, monkeypatch):
    pythonRun, rustRun = await runBoth(
        server, monkeypatch,
        sites=[{"status": 200, "contentType": "text/html"}, {"status": 200, "contentType": "text/plain"}],
        originalUrl=ASSET_URL,
    )

    assert pythonRun == rustRun
    assert pythonRun["error"] == ("TaskError", "代理站响应不合格")
    assert pythonRun["winner"] is None

async def runCredentialCase(server, monkeypatch, *, rust: bool, headers: dict[str, str]) -> dict:
    """代理候选在前、原始 URL 候选在后的凭据用例（`options.url` 要等于后者）。"""
    if rust:
        monkeypatch.setenv("GD3_RUST_GITHUB_PROBE", "1")
    else:
        monkeypatch.delenv("GD3_RUST_GITHUB_PROBE", raising=False)

    proxyRecord, directRecord = [], []
    # 代理候选快一点，所以它一定是赢家；原始 URL 候选的请求这时也已经发出去了。
    proxyUrl = await server(buildSiteHandler(delay=0.1, requests=proxyRecord))
    directUrl = await server(buildSiteHandler(delay=0.5, requests=directRecord))

    winner, file, fallback = await probeUrls(
        TaskOptions(url=directUrl, headers=headers), [proxyUrl, directUrl],
    )

    result = describe(
        0 if winner == proxyUrl else 1,
        file,
        sorted(1 if url == directUrl else 0 for url in fallback),
    )
    result["error"] = None
    result["requests"] = normalize([proxyRecord, directRecord])
    return result


async def testParityProxyCandidateLosesCredentials(server, monkeypatch):
    headers = {"Authorization": "token t", "Cookie": "c=1", "X-Other": "1"}

    pythonRun = await runCredentialCase(server, monkeypatch, rust=False, headers=headers)
    rustRun = await runCredentialCase(server, monkeypatch, rust=True, headers=headers)

    assert pythonRun == rustRun
    assert pythonRun["error"] is None
    assert pythonRun["winner"] == 0
    assert pythonRun["fallback"] == [1]

    (proxyHeaders,), (directHeaders,) = pythonRun["requests"]
    assert "authorization" not in proxyHeaders
    assert "cookie" not in proxyHeaders
    assert proxyHeaders["x-other"] == "1"
    assert directHeaders["authorization"] == "token t"
    assert directHeaders["cookie"] == "c=1"
    assert directHeaders["x-other"] == "1"
