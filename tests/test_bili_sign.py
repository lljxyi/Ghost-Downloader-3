"""GD3_RUST_BILI_SIGN 双模式 parity 测试。

覆盖：mixinKey 派生（表映射/缺 URL/短 key 的 IndexError）、signParams
（wts 覆盖/排序/清洗/urlencode/w_rid）、toCookie（分号整理/重名取后者/
COOKIE_ORDER 排前/空白集）。不依赖网络；对照实现用 Python 标准库直算。
"""

import copy
import hashlib
import urllib.parse

import pytest

from features.bili_pack import account as account_mod
from features.bili_pack import rust_sign

IMG_KEY, SUB_KEY = "a" * 32, "b" * 32
IMG_URL, SUB_URL = f"/{IMG_KEY}.png", f"/{SUB_KEY}.png"
FROZEN_NOW = 1_700_000_000.75  # 定住 time.time()，两侧 wts 必然相同


def _mixin(imgKey: str, subKey: str) -> str:
    orig = imgKey + subKey
    return "".join(orig[i] for i in account_mod.MIXIN_KEY_ENC_TAB)[:32]


def _py_sign(params: dict, mixin_key: str, wts: int) -> dict:
    params = {**params, "wts": wts}
    params = dict(sorted(params.items()))
    params = {k: "".join(c for c in str(v) if c not in "!'()*") for k, v in params.items()}
    query = urllib.parse.urlencode(params)
    return {**params, "w_rid": hashlib.md5((query + mixin_key).encode()).hexdigest()}


@pytest.fixture
def frozen_time(monkeypatch):
    monkeypatch.setattr(rust_sign.time, "time", lambda: FROZEN_NOW)
    monkeypatch.setattr(account_mod.time, "time", lambda: FROZEN_NOW)
    return round(FROZEN_NOW)


def _account() -> account_mod.BilibiliAccount:
    account = account_mod.BilibiliAccount.__new__(account_mod.BilibiliAccount)
    account._mixinKey = ""
    return account


# ── mixinKey（_updateWbiKeys） ──


def test_mixin_key_parity(monkeypatch):
    monkeypatch.setenv("GD3_RUST_BILI_SIGN", "1")
    assert rust_sign.mixinKeyFrom(IMG_URL, SUB_URL) == _mixin(IMG_KEY, SUB_KEY)


def test_mixin_key_missing_urls_returns_none(monkeypatch):
    monkeypatch.setenv("GD3_RUST_BILI_SIGN", "1")
    assert rust_sign.mixinKeyFrom("", SUB_URL) is None
    assert rust_sign.mixinKeyFrom(IMG_URL, "") is None
    assert rust_sign.mixinKeyFrom("", "") is None


def test_update_wbi_keys_wiring_is_the_same_on_both_sides(monkeypatch):
    data = {"wbi_img": {"img_url": IMG_URL, "sub_url": SUB_URL}}
    monkeypatch.setenv("GD3_RUST_BILI_SIGN", "1")
    rust_account = _account()
    rust_account._updateWbiKeys(copy.deepcopy(data))
    monkeypatch.delenv("GD3_RUST_BILI_SIGN")
    py_account = _account()
    py_account._updateWbiKeys(copy.deepcopy(data))
    assert rust_account._mixinKey == py_account._mixinKey == _mixin(IMG_KEY, SUB_KEY)


@pytest.mark.parametrize("wbi_img", [{}, {"img_url": ""}, {"sub_url": ""}, {"img_url": None}])
def test_update_wbi_keys_keeps_the_old_key_when_a_url_is_missing(monkeypatch, wbi_img):
    for enabled in ("1", None):
        if enabled:
            monkeypatch.setenv("GD3_RUST_BILI_SIGN", enabled)
        else:
            monkeypatch.delenv("GD3_RUST_BILI_SIGN", raising=False)
        account = _account()
        account._mixinKey = "keep-me"
        account._updateWbiKeys({"wbi_img": wbi_img})
        assert account._mixinKey == "keep-me"


def test_short_keys_raise_index_error_on_both_sides(monkeypatch):
    data = {"wbi_img": {"img_url": "/a.jpg", "sub_url": "/b.png"}}
    messages = []
    for enabled in ("1", None):
        if enabled:
            monkeypatch.setenv("GD3_RUST_BILI_SIGN", enabled)
        else:
            monkeypatch.delenv("GD3_RUST_BILI_SIGN", raising=False)
        with pytest.raises(IndexError) as caught:
            _account()._updateWbiKeys(copy.deepcopy(data))
        messages.append(str(caught.value))
    assert messages[0] == messages[1] == "string index out of range"


# ── signParams ──


SIGN_CASES = [
    {"cid": 11, "qn": 80, "fnval": 16},
    {"cid": "11", "bvid": "BV1xx411c7mD", "avid": 2},
    {"qn": 0, "fnver": False, "fourk": True},
    {"name": "a b+c&d=e", "weird": "!'()*x!'()*"},
    {"tilde": "~*", "plus": "+", "pct": "%2A", "space": "  "},
    {"cn": "中文标题", "emoji": "🎬"},
    {"ratio": 1.5, "none": None, "big": 10**20},
    {"empty": ""},
    {"": "empty-key"},
]


@pytest.mark.parametrize("params", SIGN_CASES)
def test_sign_params_parity(monkeypatch, frozen_time, params):
    monkeypatch.setenv("GD3_RUST_BILI_SIGN", "1")
    mixin = _mixin(IMG_KEY, SUB_KEY)
    got = rust_sign.signParams(copy.deepcopy(params), mixin)
    assert dict(got) == _py_sign(params, mixin, frozen_time)


def test_sign_params_overwrites_the_callers_wts(monkeypatch, frozen_time):
    monkeypatch.setenv("GD3_RUST_BILI_SIGN", "1")
    mixin = _mixin(IMG_KEY, SUB_KEY)
    params = {"wts": 1, "cid": 11}
    assert dict(rust_sign.signParams(copy.deepcopy(params), mixin)) == _py_sign(
        params, mixin, frozen_time
    )


def test_sign_params_wiring_is_the_same_on_both_sides(monkeypatch, frozen_time):
    params = {"cid": 11, "qn": 80}
    mixin = _mixin(IMG_KEY, SUB_KEY)
    monkeypatch.setenv("GD3_RUST_BILI_SIGN", "1")
    rust_account = _account()
    rust_account._mixinKey = mixin
    rust_params = rust_account.signParams(copy.deepcopy(params))
    monkeypatch.delenv("GD3_RUST_BILI_SIGN")
    py_account = _account()
    py_account._mixinKey = mixin
    py_params = py_account.signParams(copy.deepcopy(params))
    assert dict(rust_params) == py_params


def test_disabled_mode_is_python(monkeypatch, frozen_time):
    monkeypatch.delenv("GD3_RUST_BILI_SIGN", raising=False)
    params = {"b": 1, "a": "!"}
    mixin = _mixin(IMG_KEY, SUB_KEY)
    account = _account()
    account._mixinKey = mixin
    assert account.signParams(copy.deepcopy(params)) == _py_sign(params, mixin, frozen_time)


def test_empty_mixin_key_leaves_params_untouched(monkeypatch):
    monkeypatch.setenv("GD3_RUST_BILI_SIGN", "1")
    params = {"cid": 11}
    assert _account().signParams(copy.deepcopy(params)) == params


# ── toCookie ──


COOKIE_CASES = [
    (" junk=0;  sid=9; SESSDATA=abc; sid=1; bili_jct=f  ", "SESSDATA=abc; bili_jct=f; sid=1; junk=0"),
    ("", ""),
    ("  ; junk=  ; ", ""),
    ("a=1\r\nb=2\na=3", "a=3; b=2"),
    ("SESSDATA=x", "SESSDATA=x"),
    ("nosemicolons", ""),
    ("noequals; bili_jct=f", "bili_jct=f"),
    ("DedeUserID=1; DedeUserID__ckMd5=2", "DedeUserID=1; DedeUserID__ckMd5=2"),
    ("sid=1\x1c", "sid=1"),
    ("\x1csid=1\x1d", "sid=1"),
    ("\xa0sid\xa0=\xa01\xa0", "sid=1"),
    ("b=2; a=1; SESSDATA=s", "SESSDATA=s; b=2; a=1"),
]


@pytest.mark.parametrize("raw,expected", COOKIE_CASES)
def test_to_cookie_parity(monkeypatch, raw, expected):
    for enabled in ("1", None):
        if enabled:
            monkeypatch.setenv("GD3_RUST_BILI_SIGN", enabled)
        else:
            monkeypatch.delenv("GD3_RUST_BILI_SIGN", raising=False)
        assert account_mod.toCookie(raw) == expected


def test_to_cookie_wiring_matches_adapter(monkeypatch):
    for raw, _expected in COOKIE_CASES:
        monkeypatch.setenv("GD3_RUST_BILI_SIGN", "1")
        assert account_mod.toCookie(raw) == rust_sign.toCookie(raw)
        monkeypatch.delenv("GD3_RUST_BILI_SIGN")
        assert account_mod.toCookie(raw) == rust_sign.toCookie(raw)
