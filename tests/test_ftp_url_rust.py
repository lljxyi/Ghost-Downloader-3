"""ftp URL 纯变换层双实现对表（`GD3_RUST_FTP_URL` 影子模式）。

对同一批 URL 输入，逐条断言 Python 原生实现
（`ftp_pack.pack.parseFtpSource`，各测试先 delenv 强制原生分支，
与 `parse` 原实现的 urlparse + unquote + PurePosixPath + `or` 兜底
行序一致）与 Rust 实现（`rust_url.source` 适配器 →
`gd3_py.ftp_source` → `gd3_net::ftp::parse_ftp_source`）：

- 成功路径：七元组 `(scheme, host, port, username, password,
  sourcePathStr, hasPort)` 完全相等（want 值已用 Python 原生先验实测固化）；
- 失败路径：`ValueError` 的文案逐字一致（Invalid IPv6 URL、
  bracketed host 校验、port cast/range、NFKC netloc 等）。
"""

from __future__ import annotations

import pytest

import ftp_pack.pack as pack
from ftp_pack.rust_url import isEnabled as rustEnabled, source as rustSource

DEFAULT_PORT = 21
ANON = ("anonymous", "anon@")


def runBoth(monkeypatch, url):
    """同一入口双模式：delenv→Python 原生分支；setenv→Rust 分派（含 or 兜底）。"""
    monkeypatch.delenv("GD3_RUST_FTP_URL", raising=False)
    got_py = pack.parseFtpSource(url, DEFAULT_PORT)
    monkeypatch.setenv("GD3_RUST_FTP_URL", "1")
    got_rust = pack.parseFtpSource(url, DEFAULT_PORT)
    monkeypatch.delenv("GD3_RUST_FTP_URL", raising=False)
    return got_py, got_rust


def test_switch_defaults_off(monkeypatch):
    monkeypatch.delenv("GD3_RUST_FTP_URL", raising=False)
    assert not rustEnabled()


@pytest.mark.parametrize(
    "url,want",
    [
        # 常规：scheme lower、port 缺省 21、hasPort False
    ('ftp://host/x', ('ftp', 'host', 21, 'anonymous', 'anon@', '/x', False)),
        ("ftp://HOST:2121/X", ("ftp", "host", 2121, *ANON, "/X", True)),
    ('FTP://HOST/X Y#Z', ('ftp', 'host', 21, 'anonymous', 'anon@', '/X Y', False)),
    ('ftps://host/x', ('ftps', 'host', 21, 'anonymous', 'anon@', '/x', False)),
        # userinfo：user/pw 透传；h%6Fst 不 unquote（host 仅头段 lower）
        ("ftp://u%40ser:p%40w@h%6Fst/pa%2Fth", ("ftp", "h%6Fst", 21, "u@ser", "p@w", "/pa/th", False)),
    ('ftp://u@h/x', ('ftp', 'h', 21, 'u', 'anon@', '/x', False)),
    ('ftp://:pw@h/x', ('ftp', 'h', 21, 'anonymous', 'pw', '/x', False)),
    ('ftp://user:@h/x', ('ftp', 'h', 21, 'user', 'anon@', '/x', False)),
    ('ftp://@h/x', ('ftp', 'h', 21, 'anonymous', 'anon@', '/x', False)),
    ('ftp://h@/x', ('ftp', '', 21, 'h', 'anon@', '/x', False)),
    ('ftp://user:p:w@host/x', ('ftp', 'host', 21, 'user', 'p:w', '/x', False)),
        # '@' 取最后一段分 host（query/fragment 中的 @ 不算）
        ("ftp://u@h/x?y#z@w", ("ftp", "h", 21, "u", *ANON[1:], "/x", False)),
        # 空串归一为 anonymous/anon@（sink 端 or 兜底，双分支一致）
        ("ftp://host", ("ftp", "host", 21, *ANON, "/", False)),
        # port `or` 兜底：显式 0 回退但 hasPort 仍 True；※ host:/x hasPort False
    ('ftp://host:0/x', ('ftp', 'host', 21, 'anonymous', 'anon@', '/x', True)),
    ('ftp://host:09/x', ('ftp', 'host', 9, 'anonymous', 'anon@', '/x', True)),
    ('ftp://host:65535/x', ('ftp', 'host', 65535, 'anonymous', 'anon@', '/x', True)),
    ('ftp://host:/x', ('ftp', 'host', 21, 'anonymous', 'anon@', '/x', False)),
        # IPv6：bracketed 内容 lower（zone 保留原样）；userinfo 空串保留
        ("ftp://[2001:DB8::1]:21/p", ("ftp", "2001:db8::1", 21, *ANON, "/p", True)),
    ('ftp://[FE80::1%ETH0]/x', ('ftp', 'fe80::1%ETH0', 21, 'anonymous', 'anon@', '/x', False)),
        # IPvFuture 的 bracketed 内容原样取 host
    ('ftp://[v1.addr]/x', ('ftp', 'v1.addr', 21, 'anonymous', 'anon@', '/x', False)),
        # urlsplit 只左端去空格：头部空白删、尾部空格保留；\t\r\n 全删
        ("  ftp://host:21/x\tx", ("ftp", "host", 21, *ANON, "/xx", True)),
        ("  ftp://host\tx", ("ftp", "hostx", 21, *ANON, "/", False)),
    ('ftp:// host/x', ('ftp', ' host', 21, 'anonymous', 'anon@', '/x', False)),
        # fragment/query 截断在第一个 /?# 之前；空格不做归一化
        ("ftp://host/x y#z", ("ftp", "host", 21, *ANON, "/x y", False)),
        # 相对/无 netloc：sourcePath 原样保留、username 仍兜底为 anonymous
    ('ftp:host/path', ('ftp', '', 21, 'anonymous', 'anon@', 'host/path', False)),
    ('ftp:/x', ('ftp', '', 21, 'anonymous', 'anon@', '/x', False)),
    ('ftp://', ('ftp', '', 21, 'anonymous', 'anon@', '/', False)),
        # 非 ftp scheme 输入由调用方保证（这里以无 scheme 路径形态通过）
    ('host/x', ('', '', 21, 'anonymous', 'anon@', 'host/x', False)),
        # PurePosixPath 归一化：dot 段合并非空段、双斜杠保留
        ("ftp://host/a/./b", ("ftp", "host", 21, *ANON, "/a/b", False)),
        ("ftp://host//x", ("ftp", "host", 21, *ANON, "//x", False)),
        ("ftp://host/..", ("ftp", "host", 21, *ANON, "/..", False)),
        # 控制字符/非 ASCII：unquote('replace') 只发生在 %-escape 段
    ('ftp://u%1bser@h/x', ('ftp', 'h', 21, 'u\x1bser', 'anon@', '/x', False)),
        ("ftp://%C3%A4/%ee%", ("ftp", "%C3%A4", 21, *ANON, "/\ufffd%", False)),
        # 病态 %：'%' 原样保留（占位字符不吞其后字符）
        ("ftp://host/%", ("ftp", "host", 21, *ANON, "/%", False)),
        ("ftp://%", ("ftp", "%", 21, *ANON, "/", False)),
        ("ftp://x/a%ame2", ("ftp", "x", 21, *ANON, "/a%ame2", False)),
        # 非法 ftp: scheme 字符 → 无 scheme 落到路径形态（不等 ipaddress 流）
    ('ftp℄://host/x', ('', '', 21, 'anonymous', 'anon@', 'ftp℄:/host/x', False)),
    ],
)
def test_outcome_parity(monkeypatch, url, want):
    got_py, got_rust = runBoth(monkeypatch, url)
    assert tuple(got_py) == want, f"python 原生锚点不符: url={url!r} got={got_py!r} want={want!r}"
    assert tuple(got_rust) == want, f"url={url!r}: rust={got_rust!r} py={want!r}"


@pytest.mark.parametrize(
    "url,text",
    [
        # bracketed host：非法字符双校验的文案逐字（IPv4 or IPv6 措辞实测修正）
        ("ftp://[ ]/x", "' ' does not appear to be an IPv4 or IPv6 address"),
        ("ftp://[%41]", "'%41' does not appear to be an IPv4 or IPv6 address"),
        ("ftp://[2001:0:0:0:0:0:0:0:0]/x", "'2001:0:0:0:0:0:0:0:0' does not appear to be an IPv4 or IPv6 address"),
        ("ftp://[1:2:3:4:5:6:7:8:9]/x", "'1:2:3:4:5:6:7:8:9' does not appear to be an IPv4 or IPv6 address"),
        ("ftp://[1:2:3:4:5:6:7:8:]/x", "'1:2:3:4:5:6:7:8:' does not appear to be an IPv4 or IPv6 address"),
        ("ftp://[1:2:3:4:5:6:1.2.3.256]/x", "'1:2:3:4:5:6:1.2.3.256' does not appear to be an IPv4 or IPv6 address"),
        ("ftp://[1:2:3:4:5:6:1.2.3]/x", "'1:2:3:4:5:6:1.2.3' does not appear to be an IPv4 or IPv6 address"),
        ("ftp://[11111:1111::]/x", "'11111:1111::' does not appear to be an IPv4 or IPv6 address"),
        ("ftp://[fe80::1%]/x", "'fe80::1%' does not appear to be an IPv4 or IPv6 address"),
        ("ftp://[fe80::1%a%b]", "'fe80::1%a%b' does not appear to be an IPv4 or IPv6 address"),
        # 括号失衡：netloc 内有数据 → Invalid IPv6 URL
        ("ftp://u:[2001:DB8::1]:21/p", "Invalid IPv6 URL"),
        ("ftp://[::]%/x", "Invalid IPv6 URL"),
        ("ftp://[fe80::1%zone]%/x", "Invalid IPv6 URL"),
        # IPvFuture 拒收
        ("ftp://[v1]", "IPvFuture address is invalid"),
        # port：strconv/范围/非数字逐字（rstrip 后取斜杠前）
        ("ftp://h::21/x", "Port could not be cast to integer value as ':21'"),
        ("ftp://host:-1/x", "Port could not be cast to integer value as '-1'"),
        ("ftp://host:port/x", "Port could not be cast to integer value as 'port'"),
        ("ftp://host:1.5/x", "Port could not be cast to integer value as '1.5'"),
        ("ftp://host:65536/x", "Port out of range 0-65535"),
        # 非 ASCII netloc：NFKC 展开含 /?#@: 时才报错；其余走到 cast 报错
        ("ftp://host:21\u2100/x", "netloc 'host:21\u2100' contains invalid characters under NFKC normalization"),
        ("ftp://host:21\u2104/x", "Port could not be cast to integer value as '21\u2104'"),
    ],
)
def test_error_parity(monkeypatch, url, text):
    monkeypatch.delenv("GD3_RUST_FTP_URL", raising=False)

    try:
        pack.parseFtpSource(url, DEFAULT_PORT)
        raise AssertionError(f"Python 侧未抛出: url={url!r}")
    except ValueError as e:
        pyStr = str(e)
        assert pyStr == text, f"python 原生锚点不符: url={url!r} got={pyStr!r} want={text!r}"

    monkeypatch.setenv("GD3_RUST_FTP_URL", "1")
    try:
        pack.parseFtpSource(url, DEFAULT_PORT)
        raise AssertionError(f"Rust 侧未抛出: url={url!r}")
    except ValueError as e:
        assert str(e) == text, f" rust 侧: url={url!r} got={str(e)!r} want={text!r}"


def test_rust_dispatch_wiring(monkeypatch):
    """开关开：parseFtpSource 经适配器走 Rust 分支且结果一致。"""
    monkeypatch.setenv("GD3_RUST_FTP_URL", "1")
    assert rustEnabled()
    import ftp_pack.pack as pack

    want = ("ftp", "host", 2121, "user", "pw", "/dir/file.txt", True)
    got = pack.parseFtpSource("ftp://user:pw@host:2121/dir/file.txt", DEFAULT_PORT)
    assert tuple(got) == want

    # 错误路径同样经适配器抛 ValueError（文案逐字）
    with pytest.raises(ValueError, match="^Port could not be cast"):
        pack.parseFtpSource("ftp://host:TYPE/x", DEFAULT_PORT)


def test_bridge_raw_contract():
    """桥原样契约：userinfo 空串保留 ''、anonymous 兜底归 dispatch 的 sink。"""
    assert rustSource("ftp://host/x", DEFAULT_PORT) == ("ftp", "host", 21, "", "", "/x", False)
    assert rustSource("ftp://:pw@h/x", DEFAULT_PORT) == ("ftp", "h", 21, "", "pw", "/x", False)
    assert rustSource("ftp://h@/x", DEFAULT_PORT) == ("ftp", "", 21, "h", "", "/x", False)
