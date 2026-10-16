"""HuggingFace 树枚举：Python `_fetchTree` 与 Rust 影子分支等价。

同一本地 JSON 树，两侧 entries（path/size/lfs）逐条一致；
错误路径（401 → PermissionError、404 → ValueError）语义一致。
"""
from __future__ import annotations

import asyncio
import json

import pytest
from aiohttp import web

from app.client import buildClient
from features.huggingface_pack.pack import HuggingFaceParser
from features.huggingface_pack.rust_tree import fetchTree as rustFetchTree

TREE = {
    "/tree": [
        {"type": "file", "path": "a.png", "size": 100},
        {"type": "file", "path": "b.txt", "size": 7, "lfs": {"size": 555}},
        {"type": "directory", "path": "images", "size": 0},
        {"type": "file", "path": "c.md", "size": 3},
    ],
    "/tree/images": [
        {"type": "file", "path": "images/cover.jpg", "size": 99},
    ],
}


def _normalized(entries: list[dict]) -> list[tuple]:
    return [(e.get("path", ""), e.get("size", 0), (e.get("lfs") or {}).get("size")) for e in entries]


class _TreeParser(HuggingFaceParser):
    async def _fetchTree(self, client, apiUrl: str) -> list[dict]:
        return await HuggingFaceParser._fetchTree(self, client, apiUrl)


async def _start(app: web.Application) -> str:
    runner = web.AppRunner(app)
    await runner.setup()
    site = web.TCPSite(runner, "127.0.0.1", 0)
    await site.start()
    port = site._server.sockets[0].getsockname()[1]
    return f"http://127.0.0.1:{port}"


class TestTreeEquivalence:

    @pytest.mark.asyncio
    async def test_flat_and_nested_match(self):
        app = web.Application()

        async def handler(request):
            payload = TREE.get(request.path)
            if payload is None:
                return web.Response(status=404, text="Repository not found")
            return web.Response(text=json.dumps(payload), content_type="application/json")

        app.router.add_route("*", "/{tail:.*}", handler)
        base = await _start(app)
        try:
            apiUrl = f"{base}/tree"
            headers = {"Authorization": "Bearer x"}
            client = buildClient(headers=headers)
            try:
                py_entries = await _TreeParser()._fetchTree(client, apiUrl)
            finally:
                client.close()
            rust_entries = await rustFetchTree(apiUrl, headers)
            assert _normalized(py_entries) == _normalized(rust_entries)
            # 目录展开与顺序（嵌套目录取最后一段，与 Python 语义一致）
            assert _normalized(rust_entries) == [
                ("a.png", 100, None),
                ("b.txt", 7, 555),
                ("images/cover.jpg", 99, None),
                ("c.md", 3, None),
            ]
        finally:
            await asyncio.get_event_loop().shutdown_asyncgens()

    @pytest.mark.asyncio
    async def test_unauthorized_is_permission_error_on_both_sides(self):
        app = web.Application()

        async def handler(request):
            return web.Response(status=401, text="unauthorized")

        app.router.add_route("*", "/{tail:.*}", handler)
        base = await _start(app)
        apiUrl = f"{base}/tree"
        headers = {}
        client = buildClient(headers=headers)
        try:
            py_err = None
            try:
                await _TreeParser()._fetchTree(client, apiUrl)
            except Exception as exc:
                py_err = exc
        finally:
            client.close()
        rust_err = None
        try:
            await rustFetchTree(apiUrl, headers)
        except Exception as exc:
            rust_err = exc
        assert isinstance(py_err, PermissionError)
        assert isinstance(rust_err, PermissionError)
        assert str(rust_err) == str(py_err)

    @pytest.mark.asyncio
    async def test_not_found_raises_on_both_sides(self):
        app = web.Application()

        async def handler(request):
            return web.Response(status=404, text="Repository not found")

        app.router.add_route("*", "/{tail:.*}", handler)
        base = await _start(app)
        apiUrl = f"{base}/tree"
        headers = {}
        client = buildClient(headers=headers)
        try:
            with pytest.raises(Exception):
                await _TreeParser()._fetchTree(client, apiUrl)
        finally:
            client.close()
        with pytest.raises(Exception):
            await rustFetchTree(apiUrl, headers)
