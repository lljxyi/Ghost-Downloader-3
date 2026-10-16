"""Rust HuggingFace 树枚举适配器（影子模式，默认关闭）。

`GD3_RUST_HF_TREE=1` 时，`HuggingFaceParser._fetchTree` 的网络请求与 JSON 解析
交给 `rust/crates/gd3-net::hf`；否则走本 pack 自己的实现。两者输入输出都是
「`type == "file"` 的条目 dict 列表」，顺序一致。

与 Python 实现的差异：Rust 侧 client 不带应用代理与重定向细节（engine/probe
同款限制），profile 名同样退回 Chrome 家族（见 `rust/README.md`）。
"""

from __future__ import annotations

import asyncio
import os


def isEnabled() -> bool:
    return os.environ.get("GD3_RUST_HF_TREE") == "1"


async def fetchTree(apiUrl: str, headers: dict[str, str]) -> list[dict]:
    import gd3_py

    from app.config.cfg import cfg as appCfg

    profile = appCfg.clientProfile.value

    outcome = await asyncio.to_thread(
        gd3_py.hf_fetch_tree,
        apiUrl,
        [(name, value) for name, value in headers.items()],
        profile,
    )
    entries: list[dict] = []
    for entry in outcome:
        item: dict = {"path": entry.path, "size": entry.size}
        if entry.lfs_size is not None:
            item["lfs"] = {"size": entry.lfs_size}
        entries.append(item)
    return entries
