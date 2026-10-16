"""huggingface_pack files 变换层的双实现对表（pyRef 防两实现一起错）。

`GD3_RUST_HF_FILES=1` 时走 Rust；否则走 Python 参考实现。两种路径都必须与
独立 pyRef 一致。镜像 `HuggingFaceParser.parse` 里 entries 循环与 `_fileUrl`。
"""

from __future__ import annotations

import pytest

from features.huggingface_pack.rust_files import isEnabled

BASE = "https://hf-mirror.com"


def pyRef(entries: list[dict], repoId: str, repoType: str, revision: str) -> list[dict]:
    typePrefix = {"dataset": "datasets/", "space": "spaces/"}.get(repoType, "")
    files = []
    for i, entry in enumerate(entries):
        path = entry.get("path", "")
        size = entry.get("size", 0)
        lfs = entry.get("lfs")
        if lfs and isinstance(lfs, dict):
            size = lfs.get("size", size)
        files.append(
            {
                "index": i,
                "relativePath": path,
                "size": size,
                "downloadUrl": f"{BASE}/{typePrefix}{repoId}/resolve/{revision}/{path}",
            }
        )
    return files


def underTest(entries: list[dict], repoId: str, repoType: str, revision: str) -> list[dict]:
    if isEnabled():
        from features.huggingface_pack.rust_files import buildFiles

        return buildFiles(entries, base=BASE, repoId=repoId, repoType=repoType, revision=revision)

    # Python 路径：镜像 pack.py 里的原循环
    typePrefix = {"dataset": "datasets/", "space": "spaces/"}.get(repoType, "")
    files = []
    for i, entry in enumerate(entries):
        path = entry.get("path", "")
        size = entry.get("size", 0)
        lfs = entry.get("lfs")
        if lfs and isinstance(lfs, dict):
            size = lfs.get("size", size)
        files.append(
            {
                "index": i,
                "relativePath": path,
                "size": size,
                "downloadUrl": f"{BASE}/{typePrefix}{repoId}/resolve/{revision}/{path}",
            }
        )
    return files


CASES = [
    ([], "org/repo", "model", "main"),
    ([{"path": "a.bin", "size": 5}], "org/repo", "model", "main"),
    ([{"path": "b/c.d", "size": 7, "lfs": {"size": 12}}], "org/repo", "model", "main"),
    ([{"path": "e.txt"}], "org/repo", "model", "latest"),
    ([{"size": 3}], "o/r", "dataset", "main"),
    ([{"path": "f", "lfs": {}}], "o/r", "dataset", "main"),
    ([{"path": "f", "lfs": {"size": 0}}], "o/r", "space", "v1"),
    (
        [{"path": "a", "size": 1}, {"path": "b", "size": 2, "lfs": {"size": 99}}],
        "o/(r+2)",
        "model",
        "main",
    ),
    ([{"path": "x", "lfs": "not-a-dict", "size": 4}], "o/r", "space", "main"),
    ([{"path": "", "size": 0}, {"path": "no-size"}], "o/r", "model", "main"),
]


@pytest.mark.parametrize(("entries", "repoId", "repoType", "revision"), CASES)
def testDualImpl(entries, repoId, repoType, revision):
    assert underTest(entries, repoId, repoType, revision) == pyRef(entries, repoId, repoType, revision)


def testEdgeCasesAsRef():
    entries = [
        {"path": "a.bin", "size": 5},
        {"path": "b/c.d", "size": 7, "lfs": {"size": 12}},
        {"path": "e.txt"},
        {"size": 3},
        {"path": "f", "lfs": {}},
        {"path": "g", "lfs": "str", "size": 4},
    ]
    files = underTest(entries, "o/r", "dataset", "main")
    assert [f["index"] for f in files] == list(range(6))
    assert [f["size"] for f in files] == [5, 12, 0, 3, 0, 4]
    assert files[0]["downloadUrl"] == f"{BASE}/datasets/o/r/resolve/main/a.bin"
    assert files[2]["relativePath"] == "e.txt"
