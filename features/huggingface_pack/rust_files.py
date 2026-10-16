"""Rust HuggingFace files 变换层适配器（影子模式，默认关闭）。

`GD3_RUST_HF_FILES=1` 时，`HuggingFaceParser.parse` 里
`for i, entry in enumerate(entries)` 循环（path/size/lfs 折叠与 `_fileUrl` 拼接）
交给 `rust/crates/gd3-net::hf::build_files`；返回与 Python 现状逐字段一致的
dict 列表。`HuggingFaceFile` / `HuggingFaceTask` / repoName / steps 仍由 Python 构造。
"""

from __future__ import annotations

import os


def isEnabled() -> bool:
    return os.environ.get("GD3_RUST_HF_FILES") == "1"


def buildFiles(
    entries: list[dict], base: str, repoId: str, repoType: str, revision: str
) -> list[dict]:
    import gd3_py

    files: list[dict] = []
    for i, (path, size, downloadUrl) in enumerate(
        gd3_py.hf_build_files(entries, base, repoId, repoType, revision)
    ):
        files.append(
            {"index": i, "relativePath": path, "size": size, "downloadUrl": downloadUrl}
        )
    return files
