"""Rust Bilibili 字幕字典变换适配器（影子模式，默认关闭）。

`GD3_RUST_BILI_SUBTITLES=1` 时，`stream.py::subtitleList`
（`fetchSubtitles` 拿到 payload 之后的纯字典变换：rawList or 链、
过滤 lan/subtitle_url、lan_doc/subtitle_url 缺省、isAi=type==1）
交给 `gd3_net::bili::subtitle_list`。

成功返回字典列表（键序 lan/lan_doc/subtitle_url/isAi）；
AttributeError / TypeError 由桥按原型直接抛出（文案逐字一致），
`fetchSubtitles` 的 try/except 会按原逻辑吞掉并返回 `[]`。
"""

from __future__ import annotations

import os


def isEnabled() -> bool:
    return os.environ.get("GD3_RUST_BILI_SUBTITLES") == "1"


def subtitleList(payload):
    import gd3_py

    return gd3_py.bili_subtitle_list(payload)
