"""Rust Bilibili URL 解析适配器（影子模式，默认关闭）。

`GD3_RUST_BILI_URL=1` 时，`parse.py::parseBiliUrl` delegates 给
`gd3_net::bili::parse_url`；返回与 `BiliUrl` dataclass 字段同构的 dict
（videoId/mid/seasonId/selectedPages），由调用方直接构造 `BiliUrl`；
`ValueError`（含 `int()` 的英文文案）与 Python 逐字一致。
"""

from __future__ import annotations

import os


def isEnabled() -> bool:
    return os.environ.get("GD3_RUST_BILI_URL") == "1"


def parseBiliUrl(url: str) -> dict:
    import gd3_py

    # 失败时 Rust 侧已抛同文案的 ValueError
    return dict(gd3_py.bili_parse_url(url))
