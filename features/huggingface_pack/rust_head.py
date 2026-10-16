"""Rust HuggingFace 单文件 HEAD 探测适配器（影子模式，默认关闭）。

`GD3_RUST_HF_HEAD=1` 时，`HuggingFaceParser.parse` 单文件分支里的
`client.head(downloadUrl)` 探测（`content-length` → `fileSize`、
`accept-ranges` 键是否存在 → `canUseRange`）交给
`rust/crates/gd3-net::hf::head_file`；否则走本 pack 自己的实现。

与 Python 实现的差异：Rust 侧 client 不带应用代理与重定向细节（engine/probe
同款限制），profile 名同样退回 Chrome 家族（见 `rust/README.md`）。

语义上刻意照抄 Python 的几处「看起来像 bug」的地方：**不检查状态码**
（404/403 也照常读头部）、`accept-ranges` 只看键在不在（`none` 也算有）、
`content-length` 走 `int()` 语义（可以为负，非整数抛 `ValueError`）。
"""

from __future__ import annotations

import asyncio
import os


def isEnabled() -> bool:
    return os.environ.get("GD3_RUST_HF_HEAD") == "1"


async def headFile(downloadUrl: str, headers: dict[str, str]) -> tuple[int, bool]:
    """返回 `(fileSize, canUseRange)`，对应 `pack.py` 里那段 HEAD 探测的结果。"""
    import gd3_py

    from app.config.cfg import cfg as appCfg

    profile = appCfg.clientProfile.value

    fileSize, canUseRange = await asyncio.to_thread(
        gd3_py.hf_head_file,
        downloadUrl,
        [(name, value) for name, value in headers.items()],
        profile,
    )
    return fileSize, canUseRange
