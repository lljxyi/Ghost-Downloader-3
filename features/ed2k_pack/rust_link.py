"""Rust ed2k 链接解析适配器（影子模式，默认关闭）。

`GD3_RUST_ED2K_LINK=1` 时，`features/ed2k_pack/kelpie/models.py::Link.parse`
交给 `rust/crates/gd3-core/src/ed2k.rs`（桥函数 `gd3_py.ed2k_parse_link`）。

- 失败：`Ed2kInvalidLinkError` 的 `args[0]` 就是 Python `Error.message`
  的原文，这里翻成 `Error(ErrorCode.INVALID_LINK, message)` 抛回，
  上游 `toTaskError` 的错误语义保持不变（`"不是有效的 eD2k 链接"`）。
- 成功：桥返回 `(name, size, hash)`，`hash` 已是大写（与 Python `hash.upper()`
  一致），直接构造 kelpie `Link`。

金文件 `rust/crates/gd3-core/tests/fixtures/ed2k.jsonl`（53 条）由
`tests/fixtures/generate_ed2k_fixture.py` 用 Python 原实现生成，Rust 只读。
语义细节见 `rust/crates/gd3-core/src/ed2k.rs` 的模块文档。
"""

from __future__ import annotations

import os
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from .kelpie import Link


def isEnabled() -> bool:
    return os.environ.get("GD3_RUST_ED2K_LINK") == "1"


def parse(text: str) -> Link:
    """`features/ed2k_pack/kelpie/models.py::Link.parse` 的 Rust 实现。"""
    from .kelpie import Error, ErrorCode, Link

    import gd3_py

    try:
        name, size, hash = gd3_py.ed2k_parse_link(text)
    except gd3_py.Ed2kInvalidLinkError as error:
        raise Error(ErrorCode.INVALID_LINK, error.args[0]) from error
    return Link(name=name, size=size, hash=hash)
