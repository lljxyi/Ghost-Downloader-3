"""Rust ftp URL 纯变换适配器（影子模式，默认关闭）。

`GD3_RUST_FTP_URL=1` 时，`pack.py::parseFtpSource`
（urlparse + unquote + PurePosixPath 归一化 → 连接信息七元组）
交给 `gd3_net::ftp::parse_ftp_source`。

返回 `(scheme, host, port, username, password, sourcePathStr, hasPort)`，
userinfo/path 已 unquote、sourcePathStr 已 PurePosixPath 归一；
urlparse 会抛的 `ValueError`（Invalid IPv6 URL、bracketed host 校验、
port cast 等）由桥按原型直接抛出（文案逐字一致）。
username/password 为 urlparse 原样值（可能为空串，`anonymous`/`anon@`
兜底由 parseFtpSource 的 sink 端 `or` 完成，与原生分支行序一致）。
"""

from __future__ import annotations

import os


def isEnabled() -> bool:
    return os.environ.get("GD3_RUST_FTP_URL") == "1"


def source(url: str, defaultPort: int):
    import gd3_py

    return gd3_py.ftp_source(url, defaultPort)
