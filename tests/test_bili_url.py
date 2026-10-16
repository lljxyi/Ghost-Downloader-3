"""`parseBiliUrl` 的双实现对表测试。

对同一批 URL，逐条断言 Python 实现（`bili_pack.parse`）与 Rust 实现
（`gd3_py.bili_parse_url`）结果完全一致——成功字段、`ValueError` 文案
（含 `int()` 的英文报错）均逐字比较。`parsePageSpec` 不单独导出，
其语义（逗号/范围展开、`%2C` 编码、空段报错）通过带 `p` 参数的 URL 覆盖。

默认未开启 `GD3_RUST_BILI_URL`，`parseBiliUrl` 走纯 Python；
Rust 侧直调 `gd3_py.bili_parse_url`，开关本身留给双模式回归验证。
"""

from __future__ import annotations

import os
import re

import pytest
from bili_pack.parse import BiliUrl, parseBiliUrl

URLS = [
    "https://www.bilibili.com/video/BV1abc123/?p=2",
    "https://www.bilibili.com/video/BV1abc123",
    "https://b23.tv/video/av170001/doc",
    "https://a/video/BV1av12",
    "https://a/video/BV1?p=1,3-5",
    "https://a/video/BV1?p=1%2C2+3,4-a",
    "https://a/video/BV1?p=x",
    "https://a/video/BV1?p=5-3",
    "https://a/video/av12?p=1,",
    "https://a/video/av",
    "https://a/video/BV",
    "https://a/video/BVx?x",
    "https://space.bilibili.com/487/lists/920",
    "https://space.bilibili.com/487/channel/collectiondetail?sid=920",
    "https://space.bilibili.com/487/channel/collectiondetail?sid=x",
    "https://space.bilibili.com/487/channel/collectiondetail",
    "https://space.bilibili.com/x/lists/920",
    "https://example.com/foo",
    "",
    "ftp://a/video/BV1",
]


def asTuple(url: BiliUrl) -> tuple:
    return (url.videoId, url.mid, url.seasonId, url.selectedPages)


def rustParse(url: str) -> BiliUrl:
    import gd3_py

    fields = dict(gd3_py.bili_parse_url(url))
    return BiliUrl(
        videoId=fields["videoId"],
        mid=fields["mid"],
        seasonId=fields["seasonId"],
        selectedPages=fields["selectedPages"],
    )


@pytest.mark.parametrize("url", URLS)
def test_parity(url: str) -> None:
    try:
        expected = asTuple(parseBiliUrl(url))
    except ValueError as error:
        with pytest.raises(ValueError, match=re.escape(str(error))):
            rustParse(url)
        return
    assert asTuple(rustParse(url)) == expected


def test_switch_default_off() -> None:
    from bili_pack.rust_url import isEnabled

    if "GD3_RUST_BILI_URL" in os.environ:
        assert isEnabled()
    else:
        assert not isEnabled()
