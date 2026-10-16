"""bili 解析层双实现对表（`GD3_RUST_BILI_PARSE` 影子模式的两侧一致性）。

对同一批 dash 输入，逐条断言 Python 实现（`bili_pack.stream.parseDash`）
与 Rust 实现（`gd3_py.bili_parse_dash`）完全一致：返回四元组、错误文案、
`buildFnval` 位标志（配置值由调用方传给 Rust）。
"""

from __future__ import annotations

import pytest

from bili_pack.config import bilibiliConfig


def rustParse(dash, duration, qn, acceptQuality, audioQn, alternative=None):
    import gd3_py

    return gd3_py.bili_parse_dash(
        dash,
        duration,
        qn,
        acceptQuality,
        audioQn,
        alternative or bilibiliConfig.alternativeQuality.value,
    )


def rustFnval(qn, includeHdr=False, includeDolby=False):
    import gd3_py

    return gd3_py.bili_build_fnval(qn, includeHdr, includeDolby)


def pyFnval(qn, includeHdr=False, includeDolby=False):
    fnval = 16
    if includeHdr:
        fnval |= 64
    if includeDolby:
        fnval |= 256 | 512
    if qn == 128:
        fnval |= 1024
    if qn == 120:
        fnval |= 128
    return fnval


@pytest.mark.parametrize(
    "qn,includeHdr,includeDolby",
    [
        (80, False, False),
        (80, True, False),
        (80, False, True),
        (128, False, False),
        (128, True, True),
        (120, True, True),
        (16, False, False),
        (112, True, False),
    ],
)
def test_buildFnval_parity(qn, includeHdr, includeDolby):
    assert rustFnval(qn, includeHdr, includeDolby) == pyFnval(qn, includeHdr, includeDolby)


DASHES = [
    # 正常 video+audio
    {
        "video": [{"id": 80, "baseUrl": "https://v", "bandwidth": 8000}],
        "audio": [{"id": 30216, "baseUrl": "https://a", "bandwidth": 32000}],
    },
    # base_url 备选 + backupUrl
    {
        "video": [{"id": 64, "base_url": "", "backup_url": ["https://v2"], "bandwidth": 4000}],
        "audio": [
            {"id": 30280, "bandwidth": 192000, "baseUrl": "https://a192"},
            {"id": 30216, "bandwidth": 64000, "baseUrl": "https://a64"},
        ],
    },
    # id 是浮点 / bandwidth 是字符串
    {
        "video": [{"id": 80.0, "baseUrl": "https://v", "bandwidth": "9000"}],
        "audio": [{"id": "30216", "baseUrl": "https://a"}],
    },
    # video-only（允许）
    {
        "video": [{"id": 80, "baseUrl": "https://v", "bandwidth": 8000}],
        "audio": [],
    },
]

CASES = [
    (10, 80, [80, 64], 0, "max"),
    (10, 64, [64], 30280, "max"),
    (10, 32, [80, 64], 0, "max"),
    (10, 32, [80, 64], 30216, "min"),
    (8, 80, [], 0, "max"),
    (0, 80, [80], 0, "max"),
]


@pytest.mark.parametrize("dash", DASHES, ids=["normal", "backup", "float-id", "video-only"])
@pytest.mark.parametrize("duration,qn,acceptQuality,audioQn,alternative", CASES)
def test_parseDash_parity(dash, duration, qn, acceptQuality, audioQn, alternative):
    from bili_pack.stream import parseDash as pyParseDash
    from bili_pack.task import BiliPage

    pyPage = BiliPage(index=0, relativePath="p", _duration=duration)
    try:
        pyParseDash(pyPage, dash, qn=qn, acceptQuality=acceptQuality, audioQn=audioQn)
        pyResult = (pyPage.videoUrl, pyPage.audioUrl, pyPage.videoSize, pyPage.audioSize)
    except ValueError as error:
        pyResult = error

    try:
        rustResult = rustParse(dash, duration, qn, acceptQuality, audioQn, alternative)
    except ValueError as error:
        assert str(error) == str(pyResult), f"Rust 抛错但文案不同: {error!r} vs {pyResult!r}"
        return
    assert rustResult == pyResult, f"dash={dash} duration={duration} qn={qn}"


def test_parseDash_no_video_stream_verbatim_message():
    with pytest.raises(ValueError, match="Bilibili 返回结果中不存在可用的媒体流"):
        rustParse({"video": [], "audio": [{"id": 30216}]}, 10, 80, [80], 0, "max")


def test_parseDash_no_usable_stream_verbatim_message():
    # 两侧：所有流都没有 url 时，Python 与 Rust 都抛 "未找到可用的媒体流"
    pySideMessage = None
    try:
        from bili_pack.stream import parseDash as pyParseDash
        from bili_pack.task import BiliPage

        page = BiliPage(index=0, relativePath="p", _duration=10)
        pyParseDash(
            page,
            {"video": [{"id": 80, "baseUrl": ""}, {"id": 64, "baseUrl": ""}]},
            qn=64,
            acceptQuality=[64],
            audioQn=0,
        )
    except ValueError as error:
        pySideMessage = str(error)
    with pytest.raises(ValueError) as excInfo:
        rustParse(
            {"video": [{"id": 80, "baseUrl": ""}, {"id": 64, "baseUrl": ""}]},
            10, 64, [64], 0, "max",
        )
    assert str(excInfo.value) == str(pySideMessage)


def test_quality_screening_respects_python_id_equality():
    # Python 语义：字符串 id "64" 不等于 int 64，筛选必失败 → 兜底取第一个有 url 的流
    import gd3_py

    dash = {
        "video": [{"id": "64", "baseUrl": "https://a-string"}, {"id": 80, "baseUrl": "https://b"}],
        "audio": [],
    }
    videoUrl, audioUrl, _, _ = gd3_py.bili_parse_dash(dash, 10, 64, [64], 0, "max")
    assert audioUrl == ""
    assert videoUrl == "https://a-string"
