"""Rust probeSize 适配器（影子模式，默认关闭）。

`GD3_RUST_PROBE_SIZE=1` 时，`bili_pack.stream.probeSize` 的范围请求与
`content-range` 解析交给 `rust/crates/gd3-net::probe::probe_size`；
两侧输入输出都是 `int`，任何失败都归 0。

与 Python 实现的差异（见 `rust/README.md`）：Rust 侧 client 不带应用代理，
profile 名退回 Chrome 家族。
"""

from __future__ import annotations

import asyncio
import os


def isEnabled() -> bool:
    return os.environ.get("GD3_RUST_PROBE_SIZE") == "1"


async def probeSize(url: str, headers: dict[str, str] | None = None) -> int:
    import gd3_py

    from app.config.cfg import cfg as appCfg

    merged = dict(headers or {})
    merged["range"] = "bytes=0-0"
    merged["accept-encoding"] = "identity"
    return await asyncio.to_thread(
        gd3_py.probe_size,
        url,
        [(name, value) for name, value in merged.items()],
        appCfg.clientProfile.value,
    )
