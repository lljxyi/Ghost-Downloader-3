"""生成文件名工具的对比金文件（`filename.jsonl`）。

Rust 侧只读这份金文件、逐字节对比，所以规格的**唯一来源是 Python 实现**
（`app/platform/filesystem.py`）。改了那边就跑这个脚本重新生成：

    .venv\\Scripts\\python.exe rust/crates/gd3-core/tests/fixtures/generate_filename_fixture.py

语料覆盖的是「真会出问题」的地方：非法字符、Windows 保留名、结尾的点与空格、
多段扩展名、超长名截断、Unicode/emoji。
"""

from __future__ import annotations

import json
import sys
from pathlib import Path


def repo_root() -> Path:
    for parent in Path(__file__).resolve().parents:
        if (parent / "pyproject.toml").exists():
            return parent
    raise SystemExit("找不到仓库根目录")


sys.path.insert(0, str(repo_root()))

from app.platform.filesystem import splitStemExt, toSafeFilename  # noqa: E402

FIXTURE = Path(__file__).with_name("filename.jsonl")

# `(name, fallback, maxLength)`；maxLength 为 None 时用默认值 200
SAFE_CASES: list[tuple[str, str, int | None]] = [
    # 空 / 退化输入
    ("", "file", None),
    ("   ", "file", None),
    (".", "file", None),
    ("..", "file", None),
    ("...", "file", None),
    ("\t\n ", "file", None),
    ("///", "file", None),
    ("///", "兜底名", None),
    # 非法字符：整段折叠成一个下划线
    ("a/b\\c:d*e?f\"g<h>i|j", "file", None),
    ("a\x00b\x1fc\x7fd", "file", None),
    ("<>:\"/\\|?*", "file", None),
    ("a\nb", "file", None),
    # 结尾的点与空格会被去掉
    ("name.", "file", None),
    ("name. ", "file", None),
    ("name . . ", "file", None),
    ("  name  ", "file", None),
    # Windows 保留名（只看第一个点之前的部分，大小写不敏感）
    ("CON", "file", None),
    ("con", "file", None),
    ("Con.txt", "file", None),
    ("COM1", "file", None),
    ("COM9.log", "file", None),
    ("COM10.log", "file", None),
    ("LPT1", "file", None),
    ("NUL.tar.gz", "file", None),
    ("AUX.mp4", "file", None),
    ("PRN", "file", None),
    # 正常名字：不该被动
    ("a.bin", "file", None),
    (".hidden", "file", None),
    ("a.b.c.d", "file", None),
    ("视频 名字.mp4", "file", None),
    ("a😀b.mp4", "file", None),
    ("with space.txt", "file", None),
    ("trailing.multiple...dots", "file", None),
    # 截断：优先保住最后一段扩展名
    ("a" * 300 + ".mp4", "file", 200),
    ("a" * 300, "file", 200),
    ("abcdef.gh", "file", 5),
    ("abcdef.gh", "file", 3),
    ("abcdef.gh", "file", 4),
    ("abcdef", "file", 3),
    ("abcdef.gh", "file", 0),          # 0 表示不截断
    ("abcdef.gh", "file", -1),         # 负数同样不截断
    ("名字" * 100 + ".mkv", "file", 20),
    ("exactly200", "file", 11),        # 正好等于上限
    ("a.b", "file", 2),
    ("a.b", "file", 1),
]

SPLIT_CASES: list[str] = [
    # 复合扩展名：整段算扩展名
    "a.tar.gz",
    "a.tar.bz2",
    "a.tar.xz",
    "a.tar.zst",
    "a.tar.lzma",
    "a.tar.lzo",
    "a.tar.z",
    "a.TAR.GZ",
    "archive.001.gz",
    "a.mp4.gz",
    "a.tar.bz",
    "a.tar.7z",
    # 普通扩展名
    "a.bin",
    "a.mp4",
    "a.b.c",
    "noext",
    ".hidden",
    "a.",
    "",
    "a.tar.gz.asc",
    "视频.tar.gz",
]


def main() -> None:
    lines: list[str] = []
    for name, fallback, maxLength in SAFE_CASES:
        if maxLength is None:
            safe = toSafeFilename(name, fallback=fallback)
        else:
            safe = toSafeFilename(name, fallback=fallback, maxLength=maxLength)
        lines.append(json.dumps(
            {"kind": "safe", "name": name, "fallback": fallback,
             "maxLength": maxLength, "safe": safe},
            ensure_ascii=False,
        ))

    for name in SPLIT_CASES:
        stem, ext = splitStemExt(name)
        lines.append(json.dumps(
            {"kind": "split", "name": name, "stem": stem, "ext": ext},
            ensure_ascii=False,
        ))

    FIXTURE.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"写了 {len(lines)} 条到 {FIXTURE}")


if __name__ == "__main__":
    main()
