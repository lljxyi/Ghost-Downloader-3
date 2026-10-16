"""生成 ed2k Link.parse 的对比金文件（`ed2k.jsonl`）。

规格来自 `features/ed2k_pack/kelpie/models.py::Link.parse`：
Python 的 `str.strip` / `str.lower` / `urllib.parse.unquote` /
`re.fullmatch` / `int`（任意精度）联手决定行为，Rust 要逐条还原。

运行：`python3 rust/crates/gd3-core/tests/fixtures/generate_ed2k_fixture.py`
（需要项目 venv 或同 Python 版本环境，用到 `features.ed2k_pack`）。
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[5]))

from features.ed2k_pack.kelpie.errors import Error  # noqa: E402
from features.ed2k_pack.kelpie.models import Link  # noqa: E402

FIXTURE = Path(__file__).with_name("ed2k.jsonl")

GOOD_HASH = "0123456789abcdef0123456789abcdef"
GOOD_HASH_UPPER = GOOD_HASH.upper()
SHORT_SIZE = "1" * 40  # 40 位数，远超 eMule 上限


def link(name: str, size: str, hash: str = GOOD_HASH, scheme="ed2k:", keyword="file") -> str:
    return f"{scheme}//|{keyword}|{name}|{size}|{hash}|/"


CASES: list[str] = [
    # 基本
    link("video.mp4", "1234"),
    link("video.mp4", "1234", hash=GOOD_HASH_UPPER),
    link("video.mp4", "0"),
    link("video.mp4", "00"),
    link("video.mp4", str(256 << 30)),
    link("video.mp4", str((256 << 30) + 1)),
    link("video.mp4", SHORT_SIZE),
    # scheme 与关键字大小写
    link("video.mp4", "1", scheme="ED2K:"),
    link("video.mp4", "1", scheme="Ed2k:"),
    link("video.mp4", "1", keyword="FILE"),
    link("video.mp4", "1", keyword="File"),
    link("video.mp4", "1", keyword=" File"),
    "ed2k://|folder|name|",
    # 首尾空白（CPython str.strip，含 \x1c-\x1f）
    link("video.mp4", "1").join([" ", " "]),
    link("video.mp4", "1").join(["\t", "\n"]),
    link("video.mp4", "1").join(["\x1c", "\x1d"]),
    # 浏览器把分隔符整体编码
    f"ed2k://%7Cfile%7Cname%2Bspace%7C333%7C{GOOD_HASH}%7C/",
    f"ed2k://%7cfile%7cpl.txt%7c2%7c{GOOD_HASH}%7c/",
    f"ED2K://%7CFILE%7C%D1%86.txt%7C4%7C{GOOD_HASH}%7C/",
    f"ed2k://%7Cfile%7C%E8%A7%86%E9%A2%91%20.mp4%7C9%7C{GOOD_HASH}%7C/",
    # 普通链接里的 %7C 属于 name
    link("a%7Cb.mp4", "1"),
    link("%7Clead.mp4", "1"),
    # name 的百分号编码语义
    link("100%25.mp4", "1"),
    link("a%20b.mp4", "1"),
    link("a%FFb.mp4", "1"),
    link("%C3%A9.mp4", "1"),
    link("%C3.mp4", "1"),
    link("%E0%80%A0.mp4", "1"),
    link("%F0%9F%98%80%F0.mp4", "1"),
    link("x%zz.mp4", "1"),
    link("a%%b.mp4", "1"),
    link("%2.mp4", "1"),
    link("emoji😀.mp4", "1"),
    # name 直接含非 ASCII
    link("视频.mp4", "1"),
    # name 为空
    link("", "1"),
    # size 错误
    link("n", "-1"),
    link("n", "abc"),
    link("n", " "),
    link("n", "+1"),
    link("n", "1.0"),
    link("n", "０"),
    # hash 错误
    link("n", "1", hash="0123456789ABCDEF0123456789ABCDEF"),
    link("n", "1", hash=GOOD_HASH[:-1]),
    link("n", "1", hash=GOOD_HASH + "0"),
    link("n", "1", hash="0123456789abcdef0123456789abcdeg"),
    link("n", "1", hash=""),
    # 结构错误
    "ed2k://",
    "ed2k://|file|",
    "ed2k://|file|n|1",
    "http://example.com",
    "",
    "   ",
    # 字段富余（多于 5 段照样合法，多余段被忽略）
    f"ed2k://|file|n|1|{GOOD_HASH}|/|v2",
]


def to_line(case: str) -> dict:
    try:
        parsed = Link.parse(case)
        return {"text": case, "name": parsed.name, "size": parsed.size, "hash": parsed.hash}
    except Error as error:
        return {"text": case, "error": str(error)}


def main() -> None:
    lines = [json.dumps(to_line(case), ensure_ascii=False) for case in CASES]
    FIXTURE.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"写了 {len(lines)} 条到 {FIXTURE}")

    if "--show" in sys.argv:
        for line in lines:
            print("  ", line)


if __name__ == "__main__":
    main()
