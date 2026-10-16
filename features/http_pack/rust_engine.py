"""Rust HTTP 引擎适配器（影子模式，默认关闭）。

`GD3_RUST_HTTP=1` 时，`HttpTaskStep` 的传输交给 `rust/crates/gd3-net`；否则走本 pack
自己的实现。两者共用同一份成品文件与 `.ghd` 进度记录，可互相续传。

与 Python 实现的已知差异：Rust 侧的重试间隔固定为 `RETRY_DELAY_MS`，
不再受 Python 测试里 `asyncio.sleep` 打桩影响。
"""

from __future__ import annotations

import asyncio
import contextlib
import os
import threading
from email.utils import parsedate_to_datetime
from pathlib import Path

from loguru import logger

RETRY_DELAY_MS = 5000
CANCEL_GRACE_SECONDS = 10


def isEnabled() -> bool:
    return os.environ.get("GD3_RUST_HTTP") == "1"


async def run(step, reportSpeed, waitForSpeedLimit) -> None:
    import gd3_py

    from app.config.cfg import cfg
    from app.models.task import TaskError

    cancelled = threading.Event()
    done = threading.Event()

    def onProgress(received: int, total: int, delta: int) -> bool:
        if delta:
            reportSpeed(delta)
        step.receivedBytes = received
        if total > 0:
            step.progress = received / total * 100
        if cancelled.is_set():
            return False
        asyncio.run_coroutine_threadsafe(waitForSpeedLimit(), loop).result()
        return True

    def blockingRun():
        try:
            return gd3_py.http_run(
                step.url,
                str(step.outputPath),
                step.fileSize,
                step.canUseRangeRequests,
                step.subworkerCount,
                list(step.headers.items()),
                step.userAgent,
                step.clientProfile or cfg.clientProfile.value,
                step.httpByteOffset,
                RETRY_DELAY_MS,
                onProgress,
                max_reassign_size=cfg.maxReassignSize.value * 1024,
                auto_speed_up=cfg.autoSpeedUp.value,
            )
        finally:
            done.set()
    worker = asyncio.ensure_future(asyncio.to_thread(blockingRun))
    try:
        outcome = await worker
    except asyncio.CancelledError:
        cancelled.set()
        with contextlib.suppress(asyncio.CancelledError, TimeoutError):
            await asyncio.wait_for(asyncio.shield(_waitDone(done)), timeout=CANCEL_GRACE_SECONDS)
        raise

    step.canUseRangeRequests = outcome.can_use_range_requests
    step.receivedBytes = outcome.received_bytes
    if outcome.subworkers:
        from .task import HttpSubworker

        step.subworkers = [
            HttpSubworker(index=index, start=start, end=end, receivedBytes=position - start)
            for index, (start, position, end) in enumerate(outcome.subworkers)
        ]

    if outcome.status == "cancelled":
        cancelled.set()
        raise asyncio.CancelledError()
    if outcome.status == "failed":
        raise TaskError(outcome.message, **dict(outcome.params))

    step.progress = 100
    step.speed = 0
    _restoreMtime(step, cfg)


async def _waitDone(done: threading.Event) -> None:
    while not done.is_set():
        await asyncio.sleep(0.05)


def _restoreMtime(step, cfg) -> None:
    if not cfg.shouldPreserveLastModified.value or not step.lastModified:
        return
    try:
        mtime = parsedate_to_datetime(step.lastModified).timestamp()
        os.utime(Path(step.outputPath), (mtime, mtime))
    except Exception as e:
        logger.opt(exception=e).warning("设置文件修改时间失败 {}", step.outputPath)
