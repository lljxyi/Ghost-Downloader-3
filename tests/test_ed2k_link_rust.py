"""ed2k 链接解析层双实现 parity（`GD3_RUST_ED2K_LINK`）。

同一批链接文本，分别用「纯 Python 参考实现」
（`features/ed2k_pack/kelpie/models.py::Link.parse`）与「Rust 实现」
（`features/ed2k_pack/rust_link.py::parse`）直接对跑，断言两侧可观测
结果逐字段一致：成功 → `(name, size, hash)`；失败 →
`Error` 的 `code` 与 `message`。

语料直接引用金文件 `rust/crates/gd3-core/tests/fixtures/ed2k.jsonl`
（53 条，由 Python 原实现生成），另外补 strip / 大小写 / unicode /
带后缀尾巴这几类接线层容易走偏的组合。
"""
from __future__ import annotations

import json
from pathlib import Path

import pytest

from features.ed2k_pack.kelpie import Error, Link
from features.ed2k_pack.rust_link import isEnabled, parse as rustParse

FIXTURE = (
    Path(__file__).resolve().parents[1]
    / "rust" / "crates" / "gd3-core" / "tests" / "fixtures" / "ed2k.jsonl"
)


def snapshot(text: str, *, rust: bool) -> tuple:
    """可观测结果的公共快照（两侧用同一形状，逐字段比较）。"""
    try:
        link = rustParse(text) if rust else Link.parse(text)
    except Error as error:
        return ("error", str(error.code), error.message)
    return ("ok", link.name, link.size, link.hash)


def cases() -> list[str]:
    rows = [
        json.loads(line)
        for line in FIXTURE.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    return [row["text"] for row in rows]


# pack.py 的 `ED2kParser.parse` 会先 strip 再解析；这里直接喂原文，
# 保证 strip 语义交给 Rust 之后的可观测行为与 Python 相同。
EXTRA_CASES = [
    "",
    "   ",
    " ed2k://|file|video.mp4|1|0123456789abcdef0123456789abcdef|/ ",
    "ED2K://|FILE|video.mp4|1|0123456789abcdef0123456789abcdef|/",
    "ed2k://|file|视频.mp4|1|0123456789abcdef0123456789abcdef|/",
    "ed2k://|file|%E8%A7%86%E9%A2%91.mp4|1|0123456789abcdef0123456789abcdef|/",
    "ed2k://|file|a%2Fb.mp4|1|0123456789abcdef0123456789abcdef|/",
    "ed2k://|file|name|1|0123456789abcdef0123456789abcdef|/extra",
]

ALL = EXTRA_CASES + cases()


def test_switch_defaults_to_off():
    assert not isEnabled()


def test_real_link_matches_reference_fields():
    assert snapshot(
        "ed2k://|file|video.mp4|1|0123456789abcdef0123456789abcdef|/", rust=False
    ) == ("ok", "video.mp4", 1, "0123456789ABCDEF0123456789ABCDEF")


@pytest.mark.parametrize("text", ALL)
def test_parity(text: str):
    assert snapshot(text, rust=True) == snapshot(text, rust=False)
