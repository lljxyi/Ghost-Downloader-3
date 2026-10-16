"""GitHub 判定层双实现一致性。金数据从 `tests/test_github_probe.py` 的表抄来。

`GD3_RUST_GITHUB` 影子开关切的是实现路径；这里直接把 `gd3_py` 的 Rust 实现与本
pack 的 Python 实现对表（开关开/关结果都应一致），协程用例留给 `test_github_probe.py`。
"""

from __future__ import annotations

import pytest

import gd3_py

from github_pack.pack import GITHUB_HOSTS, isGitHubFileUrl
from github_pack.probe import matchFile, toProxyHeaders
from http_pack.pack import RemoteFile

ASSET_URL = "https://github.com/o/r/releases/download/v1/a.tar.gz"
RAW_URL = "https://raw.githubusercontent.com/o/r/main/README.md"
ARCHIVE_URL = "https://github.com/o/r/archive/refs/heads/main.zip"
CODELOAD_URL = "https://codeload.github.com/o/r/zip/refs/heads/main"

MATCH_TABLE = [
    # (originalUrl, fileSize, canUseRangeRequests, contentType, expected)
    (ASSET_URL, 100, True, "", True),
    (RAW_URL, 100, True, "text/plain", True),
    (ASSET_URL, 100, False, "", False),
    (ASSET_URL, 0, True, "", False),
    (ARCHIVE_URL, 0, False, "application/zip", True),
    (CODELOAD_URL, 0, False, "application/zip", True),
    (ARCHIVE_URL, 100, True, "application/zip", True),
    (ARCHIVE_URL, 0, False, "text/html", False),
    (ARCHIVE_URL, 0, False, "text/plain", False),
    (ASSET_URL, -1, True, "", False),
]

URL_TABLE = [
    (ASSET_URL, True),
    (RAW_URL, True),
    (ARCHIVE_URL, True),
    (CODELOAD_URL, True),
    ("https://www.github.com/o/r/archive/main.zip", True),
    ("https://www.raw.GitHubUserContent.com/o/r/main/a", True),
    ("https://example.com/a.zip", False),
    ("https://gitlab.com/o/r/releases/download/v1/a", False),
    ("https://github.com/o/r/tree/main", False),
    ("ftp://github.com/o/r/releases/download/v1/a", False),
]


def buildFile(fileSize: int, canUseRangeRequests: bool, contentType: str) -> RemoteFile:
    return RemoteFile(name="a", fileSize=fileSize, canUseRangeRequests=canUseRangeRequests, contentType=contentType)


@pytest.mark.parametrize("url, expected", URL_TABLE)
def testIsGitHubFileUrlSame(url: str, expected: bool):
    assert isGitHubFileUrl(url) is expected is gd3_py.github_is_file_url(url)


@pytest.mark.parametrize("url, fileSize, canRange, contentType, expected", MATCH_TABLE)
def testMatchFileSame(url: str, fileSize: int, canRange: bool, contentType: str, expected: bool):
    file = buildFile(fileSize, canRange, contentType)
    for _ in range(2):
        assert matchFile(url, file) is expected is gd3_py.github_match_file(
            url, contentType, canRange, max(fileSize, 0)
        )


def testHostsMatch():
    # Rust 侧的判定不需要 Python 的 host 表，但行为必须一致：表内的每个 host 都命中
    for host in GITHUB_HOSTS:
        assert gd3_py.github_is_file_url(f"https://{host}/anything")


def testProxyHeadersSame():
    headers = {"Authorization": "token t", "Cookie": "c=1", "X-Other": "1"}
    assert toProxyHeaders(headers) == {"X-Other": "1"}
    assert dict(gd3_py.github_proxy_headers(list(headers.items()))) == toProxyHeaders(headers)
