"""Rust Bilibili account 纯函数适配器（影子模式，默认关闭）。

`GD3_RUST_BILI_SIGN=1` 时，`account.py` 的三处纯函数走 Rust：
`_updateWbiKeys`（img/sub URL → mixinKey）、`signParams`（wts 排序清洗 +
urlencode + md5 w_rid）、`toCookie`（分号整理 + COOKIE_ORDER 排序）。

语义照 Python 现状抄，几处容易写错的地方（两侧已对表）：
- `wts` 是**赋值**语义（调用方自带的 `wts` 被覆盖，不是追加第二个）；
- `urlencode` 用 `quote_plus`，安全集是 `_.-~`——`*` 会被编成 `%2A`；
- `toCookie` 的空白集是 CPython `str.strip()`（比 Rust `trim()` 多 `\\x1c`-`\\x1f`）；
- key 合起来不足 64 字符时 Python 的 `orig[i]` 抛 `IndexError`，Rust 侧抛同文案。
"""

from __future__ import annotations

import os
import time


def isEnabled() -> bool:
    return os.environ.get("GD3_RUST_BILI_SIGN") == "1"


def mixinKeyFrom(imgUrl: str, subUrl: str) -> str | None:
    import gd3_py

    return gd3_py.bili_mixin_key(imgUrl, subUrl)


def signParams(params: dict, mixinKey: str) -> dict:
    import gd3_py

    wts = round(time.time())
    return dict(gd3_py.bili_sign_params(params, mixinKey, wts))


def toCookie(raw: str) -> str:
    import gd3_py

    return gd3_py.bili_to_cookie(raw)
