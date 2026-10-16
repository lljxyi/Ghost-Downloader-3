"""Rust 持久化层适配器（影子模式，默认关闭）。

设置 `GD3_RUST_PERSIST=1` 后，`tasks.jsonl` 的序列化与读写交给 `rust/crates/gd3-py`
编译出的 `gd3_py` 模块；否则走纯 Python 实现。两条路径产出的文件逐字节一致
（规格见 `rust/crates/gd3-core/tests/parity.rs`）。

与 Python 实现的已知差异：单条记录序列化失败时，Python 跳过该记录继续写盘，
Rust 直接报错——不静默丢数据。
"""

from __future__ import annotations

import os
from pathlib import Path


def isEnabled() -> bool:
    return os.environ.get("GD3_RUST_PERSIST") == "1"


def loadLines(path: Path) -> tuple[list[dict], list[str]]:
    import gd3_py

    return gd3_py.load_lines(str(path))


def saveLines(path: Path, records: list[dict]) -> None:
    import gd3_py

    gd3_py.save_lines(str(path), records)
