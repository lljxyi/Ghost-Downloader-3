"""Rust probe 适配器（影子模式，默认关闭）。

`GD3_RUST_PROBE=1` 时，`http_pack.probe` 的网络请求与头解析交给 `rust/crates/gd3-net`
的 `probe` 模块；否则走本 pack 自己的实现。两者输入输出都是 `RemoteFile`。

与 Python 实现的已知差异（engine 同款）：Rust 侧 profile 解析没有 Edge 与 Android
系列身份，这类名字退回 Chrome 最新版；`auto` 依赖 `sourceUserAgent` 匹配的分支
退回 Chrome（见 `rust/README.md` 的「与 Python 的已知差异」）。
"""

from __future__ import annotations

import asyncio
import os
import time
from mimetypes import guess_extension


def isEnabled() -> bool:
    return os.environ.get("GD3_RUST_PROBE") == "1"


async def probe(options) -> "RemoteFile":
    import gd3_py

    from .pack import RemoteFile

    def guessExtension(contentType: str) -> str:
        return guess_extension(contentType) or ""

    from app.config.cfg import cfg as appCfg

    profile = options.clientProfile or appCfg.clientProfile.value

    outcome = await asyncio.to_thread(
        gd3_py.probe,
        options.url,
        list(options.headers.items()),
        options.userAgent,
        profile,
        f"file_{time.time_ns()}",
        guessExtension,
    )
    return RemoteFile(
        name=outcome.name,
        fileSize=outcome.file_size,
        canUseRangeRequests=outcome.can_use_range_requests,
        lastModified=outcome.last_modified,
        contentType=outcome.content_type,
    )
