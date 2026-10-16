"""生成 content-disposition 解析的对比金文件（`content_disposition.jsonl`）。

`probe` 里这段是最容易踩坏的地方：Python 走 `email.message.Message.get_params`
加 `collapse_rfc2231_value`，行为细节多（引号、RFC 2231 编码字、优先取 `filename`
而不是 `filename*`）。所以先把规格用 Python 实现固定下来，Rust 侧逐条对比。

    python3 rust/crates/gd3-net/tests/fixtures/generate_cd_fixture.py
"""

from __future__ import annotations

import json
import sys
from email.message import Message
from email.utils import collapse_rfc2231_value
from pathlib import Path
from urllib.parse import unquote

FIXTURE = Path(__file__).with_name("content_disposition.jsonl")

# 直接抄 `features/http_pack/pack.py` 里那段逻辑，保证规格同源
CASES: list[str] = [
    # 基本形式
    'attachment; filename="video.mp4"',
    "attachment; filename=video.mp4",
    "attachment; filename='video.mp4'",
    "inline; filename=\"a b c.bin\"",
    "attachment",
    "",
    "   ",
    # 引号与转义
    'attachment; filename="a\\"b.mp4"',
    'attachment; filename="  spaced.mp4  "',
    'attachment; filename="trailing.mp4" ',
    "attachment; filename=\"\"",
    # 分号、等号、额外参数
    'attachment; filename="a;b.mp4"',
    'attachment; filename="a=b.mp4"',
    'attachment; filename="v.mp4"; size=1234',
    'attachment; size=1234; filename="v.mp4"',
    'attachment; filename="v.mp4"; filename="w.mp4"',
    # RFC 2231 编码字
    "attachment; filename*=UTF-8''%E8%A7%86%E9%A2%91.mp4",
    "attachment; filename*=utf-8''%e8%a7%86%e9%a2%91.mp4",
    "attachment; filename*=UTF-8'en'%E8%A7%86%E9%A2%91.mp4",
    "attachment; filename*=ISO-8859-1''%E9%C5.mp4",
    "attachment; filename*=UTF-8''%2Fetc%2Fpasswd",
    # 同时给两个：Python 优先 filename
    "attachment; filename=\"plain.mp4\"; filename*=UTF-8''%E8%A7%86%E9%A2%91.mp4",
    "attachment; filename*=UTF-8''%E8%A7%86%E9%A2%91.mp4; filename=\"plain.mp4\"",
    # 带百分号编码的普通值
    'attachment; filename="%E8%A7%86%E9%A2%91.mp4"',
    'attachment; filename="a%20b.mp4"',
    # 非法/怪异
    "attachment; filename",
    "attachment; =value",
    "attachment; filename*",
    "attachment; filename*=UTF-8''",
    "attachment; filename*=UTF-8''%ZZ.mp4",
    "attachment; filename*=bogus",
    'attachment; filename="unterminated',
    "attachment;;;",
    "attachment; ;",
    # 大小写
    'ATTACHMENT; FILENAME="Upper.MP4"',
    'attachment; FileName="Mixed.MP4"',
    # 非 ASCII 直接写在头里
    'attachment; filename="视频.mp4"',
    'attachment; filename="😀.mp4"',
    # 路径样式（不应当被当成路径处理，只 strip 引号空格）
    'attachment; filename="/etc/passwd"',
    'attachment; filename="..\\..\\evil.mp4"',
]


def parse_name(cd: str) -> str:
    """完全照抄 `probe` 里那几行。"""
    if not cd:
        return ""
    msg = Message()
    msg["Content-Disposition"] = cd
    params = msg.get_params(header="Content-Disposition")
    param_dict = {k.lower(): v for k, v in params}
    return unquote(
        collapse_rfc2231_value(param_dict.get("filename") or param_dict.get("filename*") or "")
    ).strip("\"' ")


def main() -> None:
    lines = []
    for cd in CASES:
        lines.append(json.dumps({"cd": cd, "name": parse_name(cd)}, ensure_ascii=False))
    FIXTURE.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"写了 {len(lines)} 条到 {FIXTURE}")

    if "--show" in sys.argv:
        for line in lines:
            print("  ", line)


if __name__ == "__main__":
    main()
