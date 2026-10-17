"""bili 取流响应判定层双实现对表（`GD3_RUST_BILI_PLAYURL` 影子模式）。

对同一批 playPayload / qn 输入，逐条断言 Python 原生实现
（`bili_pack.stream.playurlGate`，开关关）与 Rust 实现
（`rust_playurl.playurlGate` 适配器 → `gd3_py.bili_playurl_gate`，
开关开）完全一致：返回元组协议、AttributeError/TypeError 的类型
与文案逐字一致。ok 路径比对完整 tuple（dash / acceptQuality 值相等）。
"""

from __future__ import annotations

import pytest

from bili_pack.rust_playurl import isEnabled as rustEnabled, playurlGate as rustGate
from bili_pack.stream import playurlGate as pyGate

OK_DASH = {"video": [{"id": 80, "baseUrl": "https://example/m4s"}], "audio": [{"id": 30280}]}


def pyNative(payload, qn):
    """Python 原生判定层（各测试先 delenv，原生分支必然可达）。"""
    return pyGate(payload, qn)


def test_switch_defaults_off(monkeypatch):
    monkeypatch.delenv("GD3_RUST_BILI_PLAYURL", raising=False)
    assert not rustEnabled()


@pytest.mark.parametrize(
    "payload,qn",
    [
        # 正常 ok：code=0 / None / False，qn=120 与 qn=80
        ({"code": 0, "data": {"dash": OK_DASH, "accept_quality": [80, 64]},
          "accept_description": ["1080P 高清", "720P"]}, 120),
        ({"code": None, "data": {"dash": OK_DASH}}, 80),
        ({"code": False, "data": {"dash": OK_DASH, "accept_quality": []}}, 120),
        ({}, 80),
        # code 失败：非 80 → fallback；80 → error message
        ({"code": -404, "message": "咋了", "data": {"dash": OK_DASH}}, 120),
        ({"code": -404, "message": "咋了"}, 80),
        ({"code": -400, "message": ""}, 80),
        ({"code": -400}, 80),
        ({"code": -400, "message": 0}, 80),
        # qn=80 兜底：dash 无 video / data 缺失
        ({"code": 0, "data": {"dash": {}}}, 120),
        ({"code": 0, "data": {}}, 120),
        ({"code": 0}, 120),
        # dash.video 为空串 → list() 为空 → 兜底（qn=120）
        ({"code": 0, "data": {"dash": {"video": ""}}}, 120),
        # 假值 video（0/False）→ or [] → 兜底；真值 int → TypeError（error 表）
        ({"code": 0, "data": {"dash": {"video": 0}}}, 120),
        ({"code": 0, "data": {"dash": {"video": False}}}, 120),
        # video 是 dict → list(键) 非空 → 通过（键列表原样透传）
        ({"code": 0, "data": {"dash": {"video": {"v": 1}}}, "accept_quality": ()}, 120),
        # video 是字符串 → 非空即通过（ok）
        ({"code": 0, "data": {"dash": {"video": "xy"}}}, 120),
        # acceptQuality 字符串逐字符 list
        ({"code": 0, "data": {"dash": OK_DASH, "accept_quality": "xy"}}, 80),
        # message 透传非 str 真值
        ({"code": 3, "message": -404}, 80),
    ],
)
def test_gate_outcome_parity(monkeypatch, payload, qn):
    monkeypatch.delenv("GD3_RUST_BILI_PLAYURL", raising=False)
    want = pyNative(payload, qn)
    got = rustGate(payload, qn)
    assert got == want, f"payload={payload!r} qn={qn!r}: rust={got!r} py={want!r}"


@pytest.mark.parametrize(
    "payload,qn,etype",
    [
        # code 不可哈希成员测试 → TypeError（先于 qn 兜底）。
        # tuple 同样不可达：JSON 序列化不报 tuple；Rust depythonize
        # 把 tuple 当 array 报 'list'，与 Python（可哈希 → 走兜底）分流
        # 不一致，线上 payload 只来自 response.json() 无此形态，剔除。
        # NaN/Infinity 同理（serde_json 值模型不可表达）。
        ({"code": []}, 120, TypeError),
        ({"code": [], "message": "m"}, 80, TypeError),
        ({"code": {}}, 120, TypeError),
        # 真值非 dict → AttributeError 'X' object has no attribute 'get'
        ({"code": 0, "data": 5}, 80, AttributeError),
        ({"code": 0, "data": "abc"}, 80, AttributeError),
        ({"code": 0, "data": {"dash": 3}}, 80, AttributeError),
        # video/accept_quality 不可迭代（真值 int/float/bool；
        # 假值 0/False 走兜底，归 outcome 用例）
        ({"code": 0, "data": {"dash": {"video": 1}}}, 120, TypeError),
        ({"code": 0, "data": {"dash": OK_DASH, "accept_quality": 80}}, 80, TypeError),
        ({"code": 0, "data": {"dash": OK_DASH, "accept_quality": True}}, 80, TypeError),
    ],
)
def test_gate_error_parity(monkeypatch, payload, qn, etype):
    monkeypatch.delenv("GD3_RUST_BILI_PLAYURL", raising=False)
    pyErr = None
    rustErr = None
    try:
        pyNative(payload, qn)
    except pytest.skip.Exception:
        raise
    except Exception as e:  # noqa: BLE001
        pyErr = e
    try:
        rustGate(payload, qn)
    except Exception as e:  # noqa: BLE001
        rustErr = e

    assert pyErr is not None, f"Python 侧未抛出: payload={payload!r}"
    assert rustErr is not None, f"Rust 侧未抛出: payload={payload!r}"
    assert type(rustErr).__name__ == type(pyErr).__name__ == etype.__name__, (
        payload, type(pyErr), type(rustErr))
    assert str(pyErr) == str(rustErr), (payload, str(pyErr), str(rustErr))


def test_rust_dispatch_wiring(monkeypatch):
    """开关开：stream.playurlGate 经适配器走 Rust 分支且结果一致。"""
    monkeypatch.setenv("GD3_RUST_BILI_PLAYURL", "1")
    import bili_pack.stream as stream

    assert stream.playurlGate(
        {"code": 0, "data": {"dash": OK_DASH, "accept_quality": [80]}}, 120
    ) == ("ok", OK_DASH, [80])
    assert stream.playurlGate({"code": -404, "message": "错"}, 80) == ("error", "错")
