"""Rust ed2k progress 消息变换适配器（影子模式，默认关闭）。

`GD3_RUST_ED2K_PROGRESS=1` 时，`kelpie/kelpie.py::parseProgress`
交给 `rust/crates/gd3-core/src/ed2k.rs`（桥函数 `gd3_py.ed2k_parse_progress`）。

- 字段值原样透传（Python 对出现即用，不做类型收窄），返回值为
  `(hash, size, received, downloadRate, uploadRate, uploaded, peers,
  activePeers, heldSources, heldUntil, sources)`，`sources` 是
  `(address, software, status, rank, downloadRate, channel)` 元组列表，
  这里负责拼回 `Progress` / `Source` 数据类。
- 缺字段时桥抛 `KeyError`，`sources` 不可迭代 / 元素不可下标时抛 `TypeError`，
  文案与 Python 原实现逐字一致，由 `onMessage` 的兜底 catch 原样接收，
  不需要任何翻译。
"""

from __future__ import annotations

import os
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from .kelpie.models import Progress

FIELD_NAMES = (
    "hash size received downloadRate uploadRate uploaded peers activePeers "
    "heldSources heldUntil"
).split()


def isEnabled() -> bool:
    return os.environ.get("GD3_RUST_ED2K_PROGRESS") == "1"

def parseProgress(message: dict) -> Progress:
    """`features/ed2k_pack/kelpie/kelpie.py::parseProgress` 的 Rust 实现。"""
    from .models import Progress, Source

    import gd3_py

    values = gd3_py.ed2k_parse_progress(message)
    return Progress(
        **dict(zip(FIELD_NAMES, values[:-1])),
        sources=tuple(Source(*source) for source in values[-1]),
    )
