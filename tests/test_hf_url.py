"""HuggingFace URL 解析：Python `parseHuggingFaceUrl` 与 Rust 影子分支等价。

同一批 URL / 代理站组合，`None / (org, repo, repoType, revision, filePath)`
两侧逐条一致。Python 侧是独立参考实现（防两实现一起漂），
Rust 侧直调 `gd3_py.hf_parse_url`，再经开关适配器 `rust_url` 走一遍冒烟。
"""
from __future__ import annotations

import re
from urllib.parse import urlparse

import pytest

from features.huggingface_pack.pack import parseHuggingFaceUrl
from features.huggingface_pack.rust_url import parseHuggingFaceUrl as rustParseUrl

REPO_PATTERN = re.compile(
    r"^/(?P<type>(?:datasets|spaces)/)?(?P<org>[^/]+)/(?P<repo>[^/]+)"
    r"(?:/(?:resolve|blob|tree)/(?P<rev>[^/]+)(?:/(?P<path>.+))?)?$"
)


def pyRef(url: str, proxySite: str) -> tuple | None:
    parsed = urlparse(url)
    host = (parsed.hostname or "").lower().removeprefix("www.")
    if host != "huggingface.co":
        if not proxySite:
            return None
        proxyHost = (urlparse(proxySite).hostname or "").lower()
        if host != proxyHost:
            return None
    m = REPO_PATTERN.match(parsed.path)
    if m is None:
        return None
    typePrefix = m.group("type") or ""
    if typePrefix.startswith("datasets"):
        repoType = "dataset"
    elif typePrefix.startswith("spaces"):
        repoType = "space"
    else:
        repoType = "model"
    return (
        m.group("org"),
        m.group("repo"),
        repoType,
        m.group("rev") or "main",
        m.group("path") or "",
    )


CASES = [
    # 正常命中
    ("https://huggingface.co/org/repo", ""),
    ("https://huggingface.co/org/repo/resolve/main", ""),
    ("https://huggingface.co/org/repo/resolve/main/a/b.model", ""),
    ("https://huggingface.co/org/repo/blob/v1.0/file", ""),
    ("https://huggingface.co/org/repo/tree/dev/dir", ""),
    ("https://huggingface.co/datasets/org/repo", ""),
    ("https://huggingface.co/datasets/org/repo/resolve/v2/x%20y/file.bin", ""),
    ("https://huggingface.co/spaces/org/repo/blob/v1.0/file", ""),
    ("https://www.huggingface.co/org/repo", ""),
    ("https://HF.CO/org/repo", ""),
    ("https://huggingface.co/ORG/repo", ""),
    ("https://user:pass@huggingface.co/org/repo", ""),
    ("https://huggingface.co:443/org/repo", ""),
    ("https://huggingface.co/org/repo?download=true", ""),
    # 不命中
    ("", ""),
    ("not-a-url", ""),
    ("ftp://huggingface.co/org/repo", ""),
    ("https://example.com/org/repo", ""),
    ("https://huggingface.co/org", ""),
    ("https://huggingface.co/org/", ""),
    ("https://huggingface.co/org/repo/", ""),
    ("https://huggingface.co/org/repo/resolve", ""),
    ("https://huggingface.co/org/repo/resolve/", ""),
    ("https://huggingface.co/org/repo/resolve/main/", ""),
    ("https://huggingface.co/org/repo/resolve//x", ""),
    ("https://huggingface.co/org/repo/x/file", ""),
    ("https://huggingface.co//org/repo", ""),
    ("https://huggingface.co/org//repo", ""),
    ("https://huggingface.co/org/repo/RESOLVE/main/f", ""),
    ("https://huggingface.co/datasetX/org/repo", ""),
    ("https://huggingface.co/org/repo/resolve/main", "https://mirror.example"),
    # 代理站
    ("https://hf-mirror.com/org/repo/resolve/main/f", "https://hf-mirror.com"),
    ("https://hf-mirror.com/org/repo", "https://hf-mirror.com/path/to"),
    ("https://hf-mirror.com:8443/org/repo", "https://hf-mirror.com:8443/path"),
    ("https://hf-mirror.com/org/repo", "https://other.site/org/repo"),
    ("https://hf-mirror.com/org/repo", "https://hf-mirror.com:8443"),
    ("https://HF-MIRROR.com/org/repo", "https://hf-mirror.com"),
]


@pytest.mark.parametrize(("url", "proxySite"), CASES)
def test_rust_matches_reference(url: str, proxySite: str) -> None:
    import gd3_py

    expected = pyRef(url, proxySite)
    result = rustParseUrl(url, proxySite)
    assert (result if result is None else (
        result["org"], result["repo"], result["repoType"], result["revision"], result["filePath"],
    )) == expected


@pytest.mark.parametrize("url", [c[0] for c in CASES if not c[1]])
def test_pack_prefixes_rust(url: str) -> None:
    """pack 级入口（无开关时也必须与参考一致；开关开时它就是 rustParseUrl）。"""
    py = parseHuggingFaceUrl(url)
    assert (py if py is None else (
        py["org"], py["repo"], py["repoType"], py["revision"], py["filePath"],
    )) == pyRef(url, "")
