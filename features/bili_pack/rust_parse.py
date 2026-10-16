"""Rust Bilibili 解析层适配器（影子模式，默认关闭）。

`GD3_RUST_BILI_PARSE=1` 时，`stream.py` 的纯函数 `buildFnval` / `parseDash`
 delegates 给 `gd3_net::bili`；配置值（acceptQuality、qn、audioQn、
`alternativeQuality`、HDR/Dolby 开关）由调用方传入，`BiliPage` 的字段赋值
留在 Python。两侧 `ValueError` 文案逐字一致。
"""

from __future__ import annotations

import os


def isEnabled() -> bool:
    return os.environ.get("GD3_RUST_BILI_PARSE") == "1"


def parseDash(
    dash: dict,
    duration: int,
    qn: int,
    acceptQuality: list[int],
    audioQn: int,
    alternative: str,
    includeHdr: bool = False,
    includeDolby: bool = False,
) -> tuple[str, str, int, int]:
    """返回 `(videoUrl, audioUrl, videoSize, audioSize)`；失败抛 ValueError（文案与 Python 一致）。"""
    import gd3_py
    return gd3_py.bili_parse_dash(dash, duration, qn, list(acceptQuality or []), audioQn, alternative)


def buildFnval(qn: int, includeHdr: bool, includeDolby: bool) -> int:
    import gd3_py
    return gd3_py.bili_build_fnval(qn, includeHdr, includeDolby)
