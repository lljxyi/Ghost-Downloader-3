"""m3u8 清单 parse 扫描面双实现 parity（`GD3_RUST_M3U8_PARSE`）。

对同一份语料，分别用「Python `m3u8.loads` 参考实现」与「Rust
`features.m3u8_pack.rust_playlist.loadsSnapshot`」跑：

- 错误路径：异常**类型名 / str / repr** 逐字节一致（不含 traceback）；
- 成功路径：`is_endlist` / 段数（`playlist.segments` 的长度真值）/
  `_hlsStreams` 观测用的变体字段（uri/width/height/codecs/frame_rate）
  逐条一致；
- 变体 `absolute_uri`：`rust_playlist.absoluteUri` 与库的
  `BasePathMixin.absolute_uri` 逐字节一致（urljoin + scheme 判定含报错）。

语料复用 Rust 侧金文件 `rust/crates/gd3-core/tests/fixtures/hls.jsonl`
（75 条，由 `generate_hls_fixture.py` 用 `m3u8.loads` 生成），并补上
开关默认值与 `_hlsStreams` pack 级入口的双模式冒烟。
"""
from __future__ import annotations

import json
import os
from contextlib import contextmanager
from pathlib import Path

import m3u8
import pytest

from features.m3u8_pack import rust_playlist as rp
from features.m3u8_pack.pack import M3U8Parser

SWITCH = "GD3_RUST_M3U8_PARSE"
FIXTURE = (
    Path(__file__).resolve().parent.parent
    / "rust/crates/gd3-core/tests/fixtures/hls.jsonl"
)


@contextmanager
def rustParse(enabled: bool):
    """临时把影子开关掰到指定状态（不改调用方的环境）。"""
    previous = os.environ.get(SWITCH)
    if enabled:
        os.environ[SWITCH] = "1"
    else:
        os.environ.pop(SWITCH, None)
    try:
        yield
    finally:
        if previous is None:
            os.environ.pop(SWITCH, None)
        else:
            os.environ[SWITCH] = previous


def test_switch_defaults_to_off(monkeypatch):
    monkeypatch.delenv(SWITCH, raising=False)
    assert rp.isEnabled() is False


def corpus() -> list[dict]:
    return [json.loads(line) for line in FIXTURE.read_text(encoding="utf-8").splitlines() if line.strip()]


def pythonError(body: str):
    """Python 参考实现：`m3u8.loads` 的异常形态（type 名 / str / repr）。"""
    try:
        m3u8.loads(body)
    except Exception as exc:  # noqa: BLE001 - 参考实现要抓同型
        return type(exc).__name__, str(exc), repr(exc)
    return None


def rustError(body: str):
    """Rust 实现：`loadsSnapshot` 的异常形态（同样三件套）。"""
    try:
        rp.loadsSnapshot(body)
    except Exception as exc:  # noqa: BLE001 - 与参考实现同型
        return type(exc).__name__, str(exc), repr(exc)
    return None


def pythonSnapshot(body: str) -> dict:
    playlist = m3u8.loads(body)
    streams = []
    for variant in playlist.playlists:
        resolution = variant.stream_info.resolution
        if not resolution:
            continue
        streams.append(
            {
                "width": resolution[0],
                "height": resolution[1],
                "codecs": variant.stream_info.codecs,
                "frameRate": variant.stream_info.frame_rate,
            }
        )
    return {
        "isEndlist": playlist.is_endlist,
        "segmentCount": len(playlist.segments),
        "rawUris": [variant.uri for variant in playlist.playlists],
        "streams": streams,
    }


def rustSnapshot(body: str) -> dict:
    isEndlist, segmentCount, variants = rp.loadsSnapshot(body)
    streams = []
    for _, width, height, codecs, frameRate in variants:
        if width is None:
            continue
        streams.append(
            {
                "width": width,
                "height": height,
                "codecs": codecs,
                "frameRate": frameRate,
            }
        )
    return {
        "isEndlist": isEndlist,
        "segmentCount": segmentCount,
        "rawUris": [variant[0] for variant in variants],
        "streams": streams,
    }


@pytest.mark.parametrize("case", corpus(), ids=lambda case: case["name"])
def test_corpus_parity(case: dict):
    body = case["body"]
    with rustParse(False):
        if "error" in case:
            py = pythonError(body)
            assert py is not None
            with rustParse(True):
                rs = rustError(body)
            assert rs is not None
            assert rs == py, f"错误形态不一致：Rust={rs} Python={py}"
            return
        py = pythonSnapshot(body)
    with rustParse(True):
        rs = rustSnapshot(body)
    assert rs == py, (
        f"观测面不一致：Rust={rs} Python={py}（corpus: {case['name']}）"
    )


@pytest.mark.parametrize(
    ("base", "uri", "expected"),
    [
        ("https://a.example.com/dir/master.m3u8", "v.m3u8", "https://a.example.com/dir/v.m3u8"),
        ("https://a.example.com/dir/master.m3u8", "../x/v.m3u8", "https://a.example.com/x/v.m3u8"),
        ("https://a.example.com/master.m3u8", "https://b.example.com/y.m3u8", "https://b.example.com/y.m3u8"),
        ("/tmp/master.m3u8", "v.m3u8", "/tmp/v.m3u8"),
    ],
)
def test_absolute_uri_matches_mixin(base: str, uri: str, expected: str):
    segment = m3u8.model.Segment(uri=uri, base_uri=base)
    assert rp.absoluteUri(uri, base) == segment.absolute_uri == expected


def test_absolute_uri_raises_like_mixin():
    with pytest.raises(ValueError) as expect:
        m3u8.model.Segment(uri="v.m3u8", base_uri="").absolute_uri
    with pytest.raises(ValueError) as actual:
        rp.absoluteUri("v.m3u8", "")
    assert str(actual.value) == str(expect.value)


def test_hls_streams_pack_entry_matches_both_modes():
    body = (
        "#EXTM3U\n"
        '#EXT-X-STREAM-INF:BANDWIDTH=1,RESOLUTION=1920x1080,CODECS="avc1.4d401f",FRAME-RATE=30.0\n'
        "v.m3u8\n"
        "#EXT-X-STREAM-INF:BANDWIDTH=2,CODECS=\"hev1.1\"\n"
        "w.m3u8\n"
    )
    with rustParse(False):
        off = M3U8Parser()._hlsStreams(body)
    with rustParse(True):
        on = M3U8Parser()._hlsStreams(body)
    assert off == on == [
        {"width": 1920, "height": 1080, "codecs": "avc1.4d401f", "frameRate": 30.0},
    ]
