"""Rust http_pack 判定层适配器（影子模式，默认关闭）。

`GD3_RUST_HTTP_MATCH=1` 时，`HttpParser.match` / `matchPassive` 交给
`gd3_core::http_pack`；否则走 `pack.py` 自己的实现。两者都是纯 URL 判定，
两侧逐断言一致。
"""

from __future__ import annotations

import os


def isEnabled() -> bool:
    return os.environ.get("GD3_RUST_HTTP_MATCH") == "1"


def match(url: str) -> bool:
    import gd3_py
    return gd3_py.http_parser_match(url)


def matchPassive(url: str) -> bool:
    import gd3_py
    return gd3_py.http_parser_match_passive(url)
