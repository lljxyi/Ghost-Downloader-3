"""Rust GitHub 判定层适配器（影子模式，默认关闭）。

`GD3_RUST_GITHUB=1` 时，GitHub pack 的三个纯判定交给 `gd3_core::github`：
`isGitHubFileUrl`（URL 判定）、`toProxyHeaders`（代理头过滤）、`matchFile`
（响应匹配）；否则走本 pack 自己的实现。三者都是纯函数，两侧逐断言一致。
"""

from __future__ import annotations

import os


def isEnabled() -> bool:
    return os.environ.get("GD3_RUST_GITHUB") == "1"


def isGitHubFileUrl(url: str) -> bool:
    import gd3_py
    return gd3_py.github_is_file_url(url)


def toProxyHeaders(headers: dict[str, str]) -> dict[str, str]:
    import gd3_py
    return dict(gd3_py.github_proxy_headers(list(headers.items())))


def matchFile(originalUrl: str, file) -> bool:
    import gd3_py
    return gd3_py.github_match_file(
        originalUrl,
        file.contentType,
        file.canUseRangeRequests,
        max(file.fileSize, 0),
    )
