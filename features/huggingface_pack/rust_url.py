"""Rust HuggingFace URL 解析适配器（影子模式，默认关闭）。

`GD3_RUST_HF_URL=1` 时，`pack.py::parseHuggingFaceUrl` delegates 给
`gd3_net::hf::parse_url_entry`；代理站（`selectedProxySite()`）由 Python 传入。
返回 dict 或 None，键名与 Python 实现完全同构（org/repo/repoType/revision/filePath）。
"""

from __future__ import annotations

import os


def isEnabled() -> bool:
    return os.environ.get("GD3_RUST_HF_URL") == "1"


def parseHuggingFaceUrl(url: str, proxySite: str) -> dict | None:
    import gd3_py

    result = gd3_py.hf_parse_url(url, proxySite or "")
    if result is None:
        return None
    return dict(result)
