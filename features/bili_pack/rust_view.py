"""Rust Bilibili `fetchView` 适配器（影子模式，默认关闭）。

`GD3_RUST_BILI_VIEW=1` 时，`pack.py::fetchView` 的两次 HTTP 请求与
JSON 解析（seasons_archives_list → bvid → view API）交给
`gd3_net::bili::fetch_view`；Python 只提供配置值（UA/指纹 profile）。

与 Python 实现的差异：Rust client 不带应用代理与重定向细节（engine/probe
同款限制）；HTTP 失败 / JSON 解析失败在 Python 抛 httpx 异常，Rust 桥抛
`ValueError`（类型差异已接受，非 2xx 与解析错误同样会中断 parse 流程）。
"""

from __future__ import annotations

import asyncio
import os


def isEnabled() -> bool:
    return os.environ.get("GD3_RUST_BILI_VIEW") == "1"


async def fetchView(apiHeaders: dict[str, str], biliUrl) -> dict:
    import gd3_py

    from app.config.cfg import cfg as appCfg

    profile = appCfg.clientProfile.value
    outcome = await asyncio.to_thread(
        gd3_py.bili_fetch_view,
        biliUrl.videoId or "",
        biliUrl.mid or 0,
        biliUrl.seasonId or 0,
        [(name, value) for name, value in apiHeaders.items()],
        profile,
    )
    return dict(outcome)
