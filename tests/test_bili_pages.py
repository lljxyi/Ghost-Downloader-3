"""bili `buildPages` 双模式 parity 测试。

GD3_RUST_BILI_PAGES 关闭走 Python 原实现（parse.py），开启走
`features/bili_pack/rust_pages.py` → Rust；同一输入两实现输出必须逐字段一致。
"""

from __future__ import annotations

import copy

import pytest

from bili_pack.parse import BiliUrl, parseBiliUrl
from bili_pack.rust_pages import isEnabled


def _callBuildPages(viewData: dict, biliUrl: BiliUrl, rustEnabled: bool):
    """开关翻转后调用 parse.py::buildPages（内部每次调用都重新读 isEnabled）。"""
    import os

    from bili_pack import parse as parseMod

    saved = os.environ.get("GD3_RUST_BILI_PAGES")
    try:
        os.environ["GD3_RUST_BILI_PAGES"] = "1" if rustEnabled else "0"
        return parseMod.buildPages(copy.deepcopy(viewData), copy.deepcopy(biliUrl))
    finally:
        if saved is None:
            os.environ.pop("GD3_RUST_BILI_PAGES", None)
        else:
            os.environ["GD3_RUST_BILI_PAGES"] = saved


# ---- 样本数据 -----------------------------------------------------------

VIEW_NORMAL = {
    "bvid": "BV1aaa",
    "pic": "http://i0.hdslb.com/bfs/archive/normal.jpg",
    "title": "普通视频",
    "pages": [
        {"cid": 11, "page": 1, "part": "P1", "duration": 100},
        {"cid": 22, "part": "P2", "duration": 0},
        {"cid": "33", "part": " P3 ", "page": 5},
    ],
}

VIEW_SEASON = {
    "bvid": "BV3ccc",
    "pic": "http://i0.hdslb.com/pic.jpg",
    "title": "",
    "ugc_season": {
        "title": "合集名",
        "cover": "http://i0.hdslb.com/cover.jpg",
        "sections": [
            {"title": "第一分区", "episodes": [{"pic": "http://i0.hdslb.com/a.jpg",
                                                  "pages": [{"cid": 401, "part": "EP1a", "duration": 5}]}]},
            {"title": "第二分区", "episodes": [
                {"arc": {"pic": "http://i0.hdslb.com/arc.jpg"},
                 "pages": [{"cid": 501, "part": "EP2a"}, {"cid": 502, "page": 9, "duration": 60}]}]},
        ],
    },
}

VIEW_SEASON_ONGOING = {
    "bvid": "BV3ccc",
    "pic": "http://i0.hdslb.com/pic.jpg",
    "ugc_season": {
        "title": "连载合集",
        "sections": [
            {"title": "S1", "episodes": [{"bvid": "BV8x", "cover": "http://i0.hdslb.com/c.jpg",
                                           "page": {"cid": 601, "part": "EP1"}}]},
        ],
    },
}


# ---- 参数化 parity 断言 -------------------------------------------------

CASES = [
    ("normal_video_no_selection", VIEW_NORMAL, BiliUrl(videoId="BV1aaa", selectedPages=None)),
    ("normal_video_selected__2", VIEW_NORMAL, BiliUrl(videoId="BV1aaa", selectedPages=[2])),
    ("normal_video_selected_miss", VIEW_NORMAL, BiliUrl(videoId="BV1aaa", selectedPages=[9])),
    ("normal_selected_from_spec", VIEW_NORMAL, parseBiliUrl("https://www.bilibili.com/video/BV1aaa?p=1, 3")),
    ("season_multi_section_view", VIEW_SEASON, BiliUrl(videoId="BV3ccc", selectedPages=[2])),
    ("season_seasonid_all_selected", VIEW_SEASON, BiliUrl(mid=1, seasonId=9)),
    ("season_single_section_ongoing", VIEW_SEASON_ONGOING, BiliUrl(videoId="BV3ccc")),
    ("empty_viewdata_default_title", {}, BiliUrl(videoId="BV1zzz")),
    ("empty_pages_list", {"title": "T", "pic": "p", "pages": []}, BiliUrl(videoId="BV1y")),
]


def _fmt(result):
    pages, title, coverUrl = result
    return (
        [
            (
                p, (page.index, page.relativePath, page.cid, page.pagePart, page.pageNumber,
                    page.bvid, page.episodeTitle, page.sectionTitle, page.coverUrl,
                    page._duration, page.selected),
            )
            for p, page in enumerate(pages)
        ],
        title,
        coverUrl,
    )


@pytest.mark.parametrize("name,viewData,biliUrl", CASES, ids=[c[0] for c in CASES])
def test_buildPages_parity(name, viewData, biliUrl):
    pyOut = _fmt(_callBuildPages(viewData, biliUrl, rustEnabled=False))
    rustOut = _fmt(_callBuildPages(viewData, biliUrl, rustEnabled=True))
    assert pyOut == rustOut, f"双实现分歧：{name}\nPY: {pyOut}\nRS: {rustOut}"


def test_missing_cid_keyerror():
    viewData = {"bvid": "BV1", "pic": "", "title": "t", "pages": [{"part": "x"}]}
    for rustEnabled in (False, True):
        with pytest.raises(KeyError) as excInfo:
            _callBuildPages(viewData, BiliUrl(videoId="BV1"), rustEnabled)
        assert "cid" in str(excInfo.value)


def test_disabled_mode_is_python(monkeypatch):
    monkeypatch.delenv("GD3_RUST_BILI_PAGES", raising=False)
    assert not isEnabled()

