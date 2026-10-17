"""生成 `json_escape.jsonl`：`json.dumps(s, ensure_ascii=False)` 的转义口径。

第 49 步把 `gd3-core::config::py_json_string`（手写 JSON 字符串 writer）
与 CPython 的 `json.dumps(..., ensure_ascii=False)` 钉到逐字节一致。
风险点集中在两处：

- CPython 的 `ESCAPE_DCT` 里 `\\b`(0x08) / `\\f`(0x0c) 走的是短转义
  （`"\\b"` / `"\\f"`），**不是** `\\u0008` / `\\u000c`；其余 < 0x20 的
  控制字符才是 `\\uXXXX`（小写十六进制、四位补零）。
- `ensure_ascii=False` 下非 ASCII 原样输出，`\\u2028` / `\\u2029` /
  `\\x7f` 都**不**转义（只有 `ensure_ascii=True` 才会）。

    python3 rust/crates/gd3-core/tests/fixtures/generate_json_escape_fixture.py
"""

from __future__ import annotations

import json
from pathlib import Path

FIXTURE = Path(__file__).with_name("json_escape.jsonl")

CASES: list[str] = [
    "",
    "abc",
    "a b c",
    'a"b',
    "a\\b",
    "a/b",
    "\\\\",
    '""',
    "\n",
    "\r",
    "\t",
    "\b",
    "\f",
    "\x00",
    "\x01",
    "\x0b",
    "\x1f",
    "\x7f",
    "中文",
    "😀",
    "\u00a0",
    "\u2028",
    "\u2029",
    "".join(chr(i) for i in range(0x20)),
    '前缀\n后缀\t"引号"\\反斜杠',
    "D:\\下载\\{default}",
    "cat_video",
    "\x1c\x1d\x1e\x1f",
]


def main() -> None:
    lines: list[str] = []
    for case in CASES:
        expected = json.dumps(case, ensure_ascii=False)
        # 自检：期望串必须能解回原值（避免写出无效金文件）。
        assert json.loads(expected) == case, case
        lines.append(
            json.dumps({"input": case, "expected": expected}, ensure_ascii=False)
        )
    FIXTURE.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"写入 {FIXTURE}：{len(lines)} 条")


if __name__ == "__main__":
    main()
