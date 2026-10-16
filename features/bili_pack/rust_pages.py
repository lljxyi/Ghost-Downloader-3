"""Rust Bilibili `buildPages` 适配器（影子模式，默认关闭）。

`GD3_RUST_BILI_PAGES=1` 时，`parse.py::buildPages` 的 JSON 变换层
（season/pages 遍历、真值兜底、选页规则、toHttps）交给 `gd3_net::bili::build_pages`；
Python 拿扁平 dict 重建 `BiliPage` 对象。

与 Python 实现的已知差异（Rust 侧已记录）：
- `int(x)` 语义一致（int/float 截断/bool/十进制字符串），失败抛相同文案 `ValueError`；
- `raw["cid"]` 缺失抛同型 `KeyError`；
- 非 list 的 `pages` / `sections` 在 Python 里会以稀奇方式被 `list()` 消化（如 dict 取 keys），
  Rust 侧按缺失处理——现实 JSON 不会出现。
"""

from __future__ import annotations

import os


def isEnabled() -> bool:
    return os.environ.get("GD3_RUST_BILI_PAGES") == "1"


def buildPages(viewData: dict, biliUrl) -> tuple[list, str, str]:
    import gd3_py

    from .task import BiliPage

    if isinstance(biliUrl.selectedPages, int):
        selectedPages = [biliUrl.selectedPages]
    elif biliUrl.selectedPages:
        selectedPages = list(biliUrl.selectedPages)
    else:
        selectedPages = None

    outcome = gd3_py.bili_build_pages(
        viewData,
        biliUrl.videoId or "",
        0,
        1 if biliUrl.seasonId else 0,
        selectedPages,
    )
    pages = []
    for item in outcome["pages"]:
        pages.append(BiliPage(
            index=item["index"],
            relativePath=item["relativePath"],
            cid=item["cid"],
            pagePart=item["pagePart"],
            pageNumber=item["pageNumber"],
            bvid=item["bvid"],
            episodeTitle=item["episodeTitle"],
            sectionTitle=item["sectionTitle"],
            coverUrl=item["coverUrl"],
            _duration=item["_duration"],
        ))
        pages[-1].selected = item["selected"]
    return pages, outcome["title"], outcome["coverUrl"]
