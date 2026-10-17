"""bili 字幕字典变换层双实现对表（`GD3_RUST_BILI_SUBTITLES` 影子模式）。

对同一批 payload 输入，逐条断言 Python 原生实现
（`bili_pack.stream.subtitleList`，各测试先 delenv 强制原生分支）
与 Rust 实现（`rust_subtitles.subtitleList` 适配器 →
`gd3_py.bili_subtitle_list`）完全一致：字典列表值相等（含键序）、
AttributeError/TypeError 的类型与文案逐字一致。
"""

from __future__ import annotations

import pytest

from bili_pack.rust_subtitles import isEnabled as rustEnabled, subtitleList as rustList
from bili_pack.stream import subtitleList as pyList


def pyNative(payload):
    """Python 原生字幕列表（各测试先 delenv，原生分支必然可达）。"""
    return pyList(payload)


def test_switch_defaults_off(monkeypatch):
    monkeypatch.delenv("GD3_RUST_BILI_SUBTITLES", raising=False)
    assert not rustEnabled()


@pytest.mark.parametrize(
    "payload,want",
    [
        # 常规两条：type=1 → isAi True（lan_doc 缺省=lan）；type 缺省 0 → False
        (
            {"data": {"subtitle": {"subtitles": [
                {"lan": "ai-zh", "type": 1, "subtitle_url": "http://x"},
                {"lan": "zh-CN", "lan_doc": "中文", "subtitle_url": "http://y"},
            ]}}},
            [
                {"lan": "ai-zh", "lan_doc": "ai-zh", "subtitle_url": "http://x", "isAi": True},
                {"lan": "zh-CN", "lan_doc": "中文", "subtitle_url": "http://y", "isAi": False},
            ],
        ),
        # 过滤：lan/subtitle_url 假值（空串 / None / 缺失）剔除
        (
            {"data": {"subtitle": {"subtitles": [
                {"lan": "", "subtitle_url": "u"},
                {"lan": "zh"},
                {"lan": None, "subtitle_url": "u"},
                {"lan": "en", "subtitle_url": None},
                {"lan": "en", "subtitle_url": True, "type": 2},
            ]}}},
            [{"lan": "en", "lan_doc": "en", "subtitle_url": True, "isAi": False}],
        ),
        # or 链在 data/subtitle/subtitles 任意环节缺失/假值 → []
        ({}, []),
        ({"data": None}, []),
        ({"data": {}}, []),
        ({"data": {"subtitle": None}}, []),
        ({"data": {"subtitle": {"subtitles": None}}}, []),
        ({"data": {"subtitle": {"subtitles": []}}}, []),
        ({"data": {"subtitle": {"subtitles": 0}}}, []),
        ({"data": {"subtitle": {"subtitles": ""}}}, []),
        ({"data": {"subtitle": {"subtitles": {}}}}, []),
        # 空串/空 dict 是假值 → or [] → 空；值透传 lan_doc=Null 优先于 default
        (
            {"data": {"subtitle": {"subtitles": [
                {"lan": "zh", "lan_doc": None, "subtitle_url": "u", "type": 0},
            ]}}},
            [{"lan": "zh", "lan_doc": None, "subtitle_url": "u", "isAi": False}],
        ),
        # subtitle_url 键存在取值（可为空串），仅缺省用 ""
        (
            {"data": {"subtitle": {"subtitles": [
                {"lan": "zh", "subtitle_url": ""},
            ]}}},
            [],
        ),
         # isAi 相等语义：1/1.0/True → True；"1"/2/0.5/False/None → False
         ({"data": {"subtitle": {"subtitles": [{"lan": "zh", "subtitle_url": "u", "type": 1}]}}},
          [{"lan": "zh", "lan_doc": "zh", "subtitle_url": "u", "isAi": True}]),
         ({"data": {"subtitle": {"subtitles": [{"lan": "zh", "subtitle_url": "u", "type": 1.0}]}}},
          [{"lan": "zh", "lan_doc": "zh", "subtitle_url": "u", "isAi": True}]),
         ({"data": {"subtitle": {"subtitles": [{"lan": "zh", "subtitle_url": "u", "type": True}]}}},
          [{"lan": "zh", "lan_doc": "zh", "subtitle_url": "u", "isAi": True}]),
         ({"data": {"subtitle": {"subtitles": [{"lan": "zh", "subtitle_url": "u", "type": "1"}]}}},
          [{"lan": "zh", "lan_doc": "zh", "subtitle_url": "u", "isAi": False}]),
         ({"data": {"subtitle": {"subtitles": [{"lan": "zh", "subtitle_url": "u", "type": 2}]}}},
          [{"lan": "zh", "lan_doc": "zh", "subtitle_url": "u", "isAi": False}]),
         ({"data": {"subtitle": {"subtitles": [{"lan": "zh", "subtitle_url": "u", "type": 0.5}]}}},
          [{"lan": "zh", "lan_doc": "zh", "subtitle_url": "u", "isAi": False}]),
         ({"data": {"subtitle": {"subtitles": [{"lan": "zh", "subtitle_url": "u", "type": False}]}}},
          [{"lan": "zh", "lan_doc": "zh", "subtitle_url": "u", "isAi": False}]),
         ({"data": {"subtitle": {"subtitles": [{"lan": "zh", "subtitle_url": "u", "type": None}]}}},
          [{"lan": "zh", "lan_doc": "zh", "subtitle_url": "u", "isAi": False}]),
        # 值容器透传：lan 为列表等非字符串真值原样进输出
        (
            {"data": {"subtitle": {"subtitles": [
                {"lan": ["a"], "lan_doc": {"d": 1}, "subtitle_url": ["u"]},
            ]}}},
            [{"lan": ["a"], "lan_doc": {"d": 1}, "subtitle_url": ["u"], "isAi": False}],
        ),
    ],
)
def test_list_outcome_parity(monkeypatch, payload, want):
    monkeypatch.delenv("GD3_RUST_BILI_SUBTITLES", raising=False)
    pyWant = pyNative(payload)
    assert pyWant == want, f"python 原生锚点不符: {pyWant!r}"
    rustGot = rustList(payload)
    assert rustGot == want, f"payload={payload!r}: rust={rustGot!r} py={want!r}"


@pytest.mark.parametrize(
    "payload,etype,text",
    [
        # 根真值非 dict（含假值）→ payload.get 先抛 AttributeError
        (5, AttributeError, "'int' object has no attribute 'get'"),
        (1.5, AttributeError, "'float' object has no attribute 'get'"),
        ("x", AttributeError, "'str' object has no attribute 'get'"),
        ([1], AttributeError, "'list' object has no attribute 'get'"),
        (None, AttributeError, "'NoneType' object has no attribute 'get'"),
        (True, AttributeError, "'bool' object has no attribute 'get'"),
        (False, AttributeError, "'bool' object has no attribute 'get'"),
        # or 链环节真值非 dict
        ({"data": 5}, AttributeError, "'int' object has no attribute 'get'"),
        ({"data": "x"}, AttributeError, "'str' object has no attribute 'get'"),
        ({"data": {"subtitle": 5}}, AttributeError, "'int' object has no attribute 'get'"),
        # rawList 不可迭代（真值数字/bool）
        ({"data": {"subtitle": {"subtitles": 5}}}, TypeError, "'int' object is not iterable"),
        ({"data": {"subtitle": {"subtitles": 1.5}}}, TypeError, "'float' object is not iterable"),
        ({"data": {"subtitle": {"subtitles": True}}}, TypeError, "'bool' object is not iterable"),
        # rawList str/dict：逐元素（字符/键）走 s.get → 'str' 无 get
        ({"data": {"subtitle": {"subtitles": "xy"}}}, AttributeError, "'str' object has no attribute 'get'"),
        ({"data": {"subtitle": {"subtitles": {"k": 1}}}}, AttributeError, "'str' object has no attribute 'get'"),
        # 列表元素非 dict → AttributeError
        ({"data": {"subtitle": {"subtitles": [5]}}}, AttributeError, "'int' object has no attribute 'get'"),
        ({"data": {"subtitle": {"subtitles": [None]}}}, AttributeError, "'NoneType' object has no attribute 'get'"),
        ({"data": {"subtitle": {"subtitles": [True]}}}, AttributeError, "'bool' object has no attribute 'get'"),
        ({"data": {"subtitle": {"subtitles": [1.5]}}}, AttributeError, "'float' object has no attribute 'get'"),
        ({"data": {"subtitle": {"subtitles": [[1]]}}}, AttributeError, "'list' object has no attribute 'get'"),
        ({"data": {"subtitle": {"subtitles": ["a"]}}}, AttributeError, "'str' object has no attribute 'get'"),
        # 过滤后的元素构造期错误仍按类型逐字
        ({"data": {"subtitle": {"subtitles": [True]}}, "x": 1}, AttributeError, "'bool' object has no attribute 'get'"),
    ],
)
def test_list_error_parity(monkeypatch, payload, etype, text):
    monkeypatch.delenv("GD3_RUST_BILI_SUBTITLES", raising=False)
    try:
        pyNative(payload)
        raise AssertionError(f"Python 侧未抛出: payload={payload!r}")
    except etype as e:
        pyStr = str(e)
        assert pyStr == text, (payload, pyStr, text)
    except AssertionError:
        raise
    except Exception as e:  # noqa: BLE001
        raise AssertionError(
            f"Python 锚点异常类型不符 {type(e).__name__}（期望 {etype.__name__}: {text}）"
        ) from e

    try:
        rustList(payload)
        raise AssertionError(f"Rust 侧未抛出: payload={payload!r}")
    except etype as e:
        assert str(e) == text, (payload, str(e), text)


def test_rust_dispatch_wiring(monkeypatch):
    """开关开：stream.subtitleList 经适配器走 Rust 分支且结果一致。"""
    monkeypatch.setenv("GD3_RUST_BILI_SUBTITLES", "1")
    import bili_pack.stream as stream

    payload = {"data": {"subtitle": {"subtitles": [
        {"lan": "zh", "type": 1, "subtitle_url": "u"},
    ]}}}
    want = [{"lan": "zh", "lan_doc": "zh", "subtitle_url": "u", "isAi": True}]
    assert stream.subtitleList(payload) == want
