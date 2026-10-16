"""Rust GitHub 候选竞速适配器（影子模式，默认关闭）。

`GD3_RUST_GITHUB_PROBE=1` 时，`github_pack.probe.probeUrls` 的整段竞速交给
`gd3_net::github::probe_urls`：逐候选并发探测（每个候选整体 5 秒超时）、
`matchFile` 判定、取首个合格者并取消其余、拼出可回退的候选列表。

两侧一致的语义（细节见 `rust/crates/gd3-net/src/github.rs` 的模块文档）：
等于 `originalUrl` 的候选拿原样头，其余候选剥掉 `authorization` / `cookie`；
不合格的候选既不获胜也不进回退列表；全部失败时抛「最后完成者」的异常。

与 Python 实现的两点已知差异：

- 竞速内部的探测固定走 Rust（`gd3_net::probe::probe`），**不受** `GD3_RUST_PROBE`
  影响 —— 这是影子模式的老规矩：一个开关只决定它自己那段走谁。
- 兜底文件名 `file_{time_ns()}` 整场竞速只生成一次（Python 是每个候选各生成一次）。
  该名字只在响应里彻底拿不到文件名时才会用到，而它本来就不可复现。

`probeProxyLatencies` **不迁移**：它返回的是实测耗时，两侧不可比。
"""

from __future__ import annotations

import asyncio
import os
import time
from mimetypes import guess_extension

from app.models.task import TaskError


def isEnabled() -> bool:
    return os.environ.get("GD3_RUST_GITHUB_PROBE") == "1"


async def probeUrls(options, urls: list[str]) -> tuple[str, "RemoteFile", list[str]]:
    import gd3_py

    from http_pack.pack import RemoteFile

    from app.config.cfg import cfg as appCfg

    def guessExtension(contentType: str) -> str:
        return guess_extension(contentType) or ""

    profile = options.clientProfile or appCfg.clientProfile.value

    try:
        winner, outcome, fallbackUrls = await asyncio.to_thread(
            gd3_py.github_probe_urls,
            options.url,
            list(urls),
            list(options.headers.items()),
            options.userAgent,
            profile,
            f"file_{time.time_ns()}",
            guessExtension,
        )
    except gd3_py.GithubUnmatchedError as error:
        raise TaskError("代理站响应不合格") from error

    return winner, RemoteFile(
        name=outcome.name,
        fileSize=outcome.file_size,
        canUseRangeRequests=outcome.can_use_range_requests,
        lastModified=outcome.last_modified,
        contentType=outcome.content_type,
    ), fallbackUrls
