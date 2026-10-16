"""m3u8 清单 parse 扫描面的 Rust 影子适配器（默认关闭）。

`GD3_RUST_M3U8_PARSE=1` 时，`m3u8_pack.pack::M3U8Parser` 里三处
`m3u8.loads(body)` 的**清单扫描面**交给 `rust/crates/gd3-core/src/hls.rs`
（`gd3_py.m3u8_probe`）：is_endlist、消费到的段数、变体列表
（uri/width/height/codecs/frame_rate）。

不迁移的部分（留在 Python）：

- 变体 `absolute_uri`（urllib urljoin + scheme 判定，这里用同一逻辑内联，见
  `absoluteUri`）；点名判活抓取的网络请求本身；
- `#EXT-X-Media`/`TILES`/`PROGRAM-DATE-TIME`/`RENDITION` 系的静默偏差
  （扫描层不镜像它们的错误路径，正常路径观测面一致），详见
  `rust/crates/gd3-core/src/hls.rs` 模块头。

异常翻译：Rust 不抛 `Err`，回 `("error", "Kind: message", ...)`。
这里按 `Kind` 挑同型异常类重抛，使 `str(e)` / `repr(e)` /
`f"{type(e).__name__}: {e}"` 与 Python 原生 `m3u8.loads` 的异常逐字节一致。
`KeyError` 的 `str(e)` 是参数的 `repr`（`KeyError('bandwidth')` →
`'bandwidth'`），所以参数要先解一层引号。
"""

from __future__ import annotations

import ast
import os
from urllib.parse import urljoin, urlsplit


def isEnabled() -> bool:
    return os.environ.get("GD3_RUST_M3U8_PARSE") == "1"


def loadsSnapshot(
    body: str,
) -> tuple[
    bool,  # isEndlist
    int,  # segmentCount（= playlist.segments 的长度真值）
    list[
        tuple[
            str | None,   # uri
            int | None,   # width
            int | None,   # height
            str | None,   # codecs
            float | None, # frame_rate
        ]
    ],
]:
    """`m3u8.loads(body)` 成功时的观测面；失败时重抛同型异常。"""
    import gd3_py

    tag, message, isEndlist, segmentCount, variants = gd3_py.m3u8_probe(body)
    if tag == "error":
        _reraise(message)
    return (
        isEndlist,
        segmentCount,
        [tuple(variant) for variant in variants],
    )


def absoluteUri(uri: str | None, baseUri: str) -> str | None:
    """镜像 `m3u8.mixins.BasePathMixin.absolute_uri`（逐字，含报错文案）。"""
    if uri is None:
        return None
    ret = urljoin(baseUri, uri)
    if baseUri and not urlsplit(baseUri).scheme:
        return ret
    if not urlsplit(ret).scheme:
        raise ValueError("There can not be `absolute_uri` with no `base_uri` set")
    return ret


_EXCEPTION_KINDS = {
    "ValueError": ValueError,
    "OverflowError": OverflowError,
    "TypeError": TypeError,
    "IndexError": IndexError,
}


def _reraise(message: str):
    kind, _, text = message.partition(": ")
    if kind == "KeyError":
        try:
            inner = ast.literal_eval(text)
        except Exception:
            inner = text
        raise KeyError(inner)
    exceptionKind = _EXCEPTION_KINDS.get(kind, ValueError)
    raise exceptionKind(text)
