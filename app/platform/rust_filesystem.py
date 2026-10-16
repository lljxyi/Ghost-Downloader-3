"""Rust 文件系统工具适配器（影子模式，默认关闭）。

`GD3_RUST_FILENAME=1` 时，文件名净化与拆分交给 `gd3-core` 的 `filename` 模块；
`app/platform/filesystem.py` 里各留一个开关点，调用方不用改。

与 Python 实现的已知差异：无。规格由 `tests/fixtures/filename.jsonl` 固定——
那份金文件是 Python 生成的，Rust 侧逐字节对比（`crates/gd3-core/tests/filename.rs`），
`tests/test_filesystem.py` 再用同一批断言把两侧都跑一遍。
"""

from __future__ import annotations

DEFAULT_MAX_LENGTH = 200


def isEnabled() -> bool:
    import os

    return os.environ.get("GD3_RUST_FILENAME") == "1"


def toSafeFilename(name: str, fallback: str = "file", maxLength: int = DEFAULT_MAX_LENGTH) -> str:
    import gd3_py

    return gd3_py.to_safe_filename(name, fallback, maxLength)


def splitStemExt(name: str) -> tuple[str, str]:
    import gd3_py

    return gd3_py.split_stem_ext(name)
