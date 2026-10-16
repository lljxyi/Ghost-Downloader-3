"""`parseHuggingFaceUrl` 的双实现对表测试。

对同一批 URL / 代理站组合，逐条断言 Python 实现（`features.huggingface_pack.pack`
的实现逻辑，这里用独立的参考实现防「两个实现一起错」）与 Rust 实现
（`gd3_py.hf_parse_url`）结果完全一致。

默认未开启 `GD3_RUST_HF_URL`，pack 级入口走纯 Python；本测试直接双调
参考实现与适配器，不受开关影响。
"""

from __future__ import annotations

import re
import os

from urllib.parse import urlparse

from features.huggingface_pack.pack import parseHuggingFaceUrl
from features.huggingface_pack.rust_url import parseHuggingFaceUrl as rustParseUrl

REPO_PATTERN = re.compile(
    r"^/(?P<type>(?:datasets|spaces)/)?(?P<org>[^/]+)/(?P<repo>[^/]+)"
    r"(?:/(?:resolve|blob|tree)/(?P<rev>[^/]+)(?:/(?P<path>.+))?)?$"
)


def pyRef(url: str, proxySite: str) -> dict | None:
    parsed = urlparse(url)
    host = (parsed.hostname or "").lower().removeprefix("www.")
    if host not in {"huggingface.co"}:
        if proxySite:
            proxyHost = (urlparse(proxySite).hostname or "").lower()
            if host != proxyHost:
                return None
        else:
            return None

    m = REPO_PATTERN.match(parsed.path)
    if m is None:
        return None

    repoType = "model"
    typePrefix = m.group("type") or ""
    if typePrefix.startswith("datasets"):
        repoType = "dataset"
    elif typePrefix.startswith("spaces"):
        repoType = "space"

    return {
        "org": m.group("org"),
        "repo": m.group("repo"),
        "repoType": repoType,
        "revision": m.group("rev") or "main",
        "filePath": m.group("path") or "",
    }


URLS = [
    "https://huggingface.co/org/repo",
    "https://huggingface.co/org/repo/",
    "https://huggingface.co/org/repo/resolve/main",
    "https://huggingface.co/org/repo/resolve/main/a/b.model?download=true",
    "https://huggingface.co/datasets/org/repo",
    "https://huggingface.co/spaces/org/repo/blob/v1.0/file",
    "https://huggingface.co/org/repo/tree/main",
    "https://HF.Co/org/repo",
    "https://www.huggingface.co/org/repo",
    "https://user:pw@huggingface.co:8443/org/repo",
    "https://a.co/org/repo",
    "ftp://huggingface.co/org/repo",
    "//huggingface.co/org/repo",
    "https://huggingface.co/org",
    "https://huggingface.co/org/repo/x/file",
    "https://huggingface.co/org/repo/resolve",
    "https://huggingface.co/org/repo/resolve/",
    "https://huggingface.co//repo",
    "https://huggingface.co/org//",
    "",
]

PROXY_SITES = ["", "https://a.co", "http://mirror.example.org:8080"]


def test_parity_against_python_reference():
    for url in URLS:
        expected = pyRef(url, "")
        assert parseHuggingFaceUrl(url) == expected, url


def test_proxy_site_parity():
    for url in URLS:
        for site in PROXY_SITES:
            expected = pyRef(url, site)
            assert rustParseUrl(url, site) == expected, (url, site)


def test_disabled_by_default():
    from features.huggingface_pack.rust_url import isEnabled

    if "GD3_RUST_HF_URL" in os.environ:
        assert isEnabled()
    else:
        assert not isEnabled()
