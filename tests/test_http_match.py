"""http_pack 判定层双实现对表（`GD3_RUST_HTTP_MATCH` 影子模式）。

对同一批 URL（含 scheme 解析的边界情况），逐条断言 Python 实现
（`HttpParser.match` / `matchPassive`）与 Rust 实现
（`gd3_py.http_parser_match` / `http_parser_match_passive`）结果完全一致，
且与手写预期一致。
"""

from __future__ import annotations

import os
from urllib.parse import urlparse

import pytest

from features.http_pack.pack import DOWNLOADABLE_EXTENSIONS, HttpParser
from features.http_pack.task import HttpTaskStep
from app.models.task import TaskOptions

# 破坏性边界（scheme 解析、大小写、query/fragment、分号 path、非 ASCII scheme）
URLS = [
    "http://a/b.mp4",
    "https://A.COM/x.TS?y=1",
    "HTTP://a/",
    "hTtPs://a/b.mkv#frag",
    "ftp://a/b",
    "ftp://a/b.mp4",
    "a.mp4",
    "",
    "http:",
    "//a/b.mkv",
    "hTtP+2://a/b.rar",
    "https://a/b.mp4;",
    "文件://a/b.mp4",
    "http:/a",
    "http://a/b.mp4",
    "https://host/p/文件.mp4",
    "https://a/b.MP4",
    "https://a/b.zst?x",
    "https://a/b.pdf",
    "https://a/README.md",
    "https://a/b.img",
    "https://a/b.appimage",
    "https://a/b.m2ts?download",
]


def pyMatch(url: str) -> bool:
    parser = HttpParser()
    return parser.match(TaskOptions(url=url))


def pyMatchPassive(url: str) -> bool:
    parser = HttpParser()
    return parser.matchPassive(TaskOptions(url=url))


def rustMatch(url: str) -> bool:
    import gd3_py
    return gd3_py.http_parser_match(url)


def rustMatchPassive(url: str) -> bool:
    import gd3_py
    return gd3_py.http_parser_match_passive(url)


def pyPassiveReference(url: str) -> bool:
    """独立于 HttpParser 的参考实现，防止「两个实现一起错」。"""
    parsed = urlparse(url)
    if parsed.scheme.lower() not in {"http", "https"}:
        return False
    return any(parsed.path.lower().endswith(ext) for ext in DOWNLOADABLE_EXTENSIONS)


@pytest.mark.parametrize("url", URLS)
def test_match_and_passive_parity(url):
    assert pyMatch(url) == rustMatch(url)
    assert pyMatchPassive(url) == rustMatchPassive(url)
    assert pyPassiveReference(url) == rustMatchPassive(url)


def test_downloadable_extension_table_intact():
    # Rust 只读这份表的镜像；表变化时这里的对照测试会立刻红灯
    assert len(DOWNLOADABLE_EXTENSIONS) == 38
    import gd3_py

    for ext in DOWNLOADABLE_EXTENSIONS:
        assert rustMatchPassive(f"https://a/x{ext}")
