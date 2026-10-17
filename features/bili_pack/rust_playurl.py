"""Rust Bilibili 取流响应判定层适配器（影子模式，默认关闭）。

`GD3_RUST_BILI_PLAYURL=1` 时，`stream.py::playurlGate`（`fetchPlayurl`
`await response.json()` 之后的纯判定层：code 兜底 / qn=80 递归 /
pageData / dash / videoStreams / acceptQuality）交给
`gd3_net::bili::playurl_gate`。

返回协议与 Python 原生纯函数一致：
  ('fallback',) | ('error', message) | ('ok', dash, acceptQuality)
AttributeError / TypeError 由桥按原型直接抛出（文案逐字一致）。
"""

from __future__ import annotations

import os


def isEnabled() -> bool:
    return os.environ.get("GD3_RUST_BILI_PLAYURL") == "1"


def playurlGate(playPayload, qn: int):
    import gd3_py

    outcome = gd3_py.bili_playurl_gate(playPayload, qn)
    kind = outcome[0]
    if kind == "ok":
        return ("ok", outcome[1], outcome[2])
    if kind == "fallback":
        return ("fallback",)
    return ("error", outcome[1])
