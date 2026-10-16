"""Rust m3u8 输出解析适配器（影子模式，默认关闭）。

`GD3_RUST_M3U8_OUTPUT=1` 时，`m3u8_pack.task` 的三段**纯文本解析**交给
`rust/crates/gd3-core/src/m3u8.rs`：

- `parseDiagnostic` → `gd3_py.m3u8_parse_diagnostic`；
- `parseError` → `gd3_py.m3u8_classify_error`（返回 message 模板 + params，
  这里翻成 `TaskError`，键集合与 Python 侧逐字一致：命中 `ERROR_HINTS` 时
  `params` 只有 `detail`，兜底时还有 int 类型的 `code`）；
- `M3U8TaskStep._parseOutputLine` → `gd3_py.m3u8_parse_output_line`
  （返回 `(lastMessage, fields)`，这里只负责把 `fields` 写回 step 属性）。

语义细节与已知偏差见 `rust/crates/gd3-core/src/m3u8.rs` 的模块文档。
这里不做任何二次判断：Rust 说 `lastMessage is None` 就直接返回
（对应 Python 在赋值 `lastMessage` 之前 return），Rust 没给的键就不动。
"""

from __future__ import annotations

import os

from app.models.task import TaskError


def isEnabled() -> bool:
    return os.environ.get("GD3_RUST_M3U8_OUTPUT") == "1"


def parseDiagnostic(output: str) -> str:
    """`features/m3u8_pack/task.py::parseDiagnostic` 的 Rust 实现。"""
    import gd3_py

    return gd3_py.m3u8_parse_diagnostic(output)


def parseError(output: str, returncode: int) -> TaskError:
    """`features/m3u8_pack/task.py::parseError` 的 Rust 实现。"""
    import gd3_py

    message, params = gd3_py.m3u8_classify_error(output, returncode)
    return TaskError(message, **params)


def applyOutputLine(step, line: str) -> None:
    """`M3U8TaskStep._parseOutputLine` 的 Rust 实现：只做属性赋值。"""
    import gd3_py

    lastMessage, fields = gd3_py.m3u8_parse_output_line(line)
    if lastMessage is None:
        return

    step.lastMessage = lastMessage
    if "progress" in fields:
        step.progress = fields["progress"]
    if "speed" in fields:
        step.speed = fields["speed"]
    if "receivedBytes" in fields:
        step.receivedBytes = fields["receivedBytes"]
    if "fileSize" in fields:
        step.task.fileSize = fields["fileSize"]
    if "liveElapsed" in fields:
        step.liveElapsed = fields["liveElapsed"]
    if "liveTotal" in fields:
        step.liveTotal = fields["liveTotal"]
    if "liveStatus" in fields:
        step.liveStatus = fields["liveStatus"]
