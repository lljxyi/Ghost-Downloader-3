"""生成 HeadersEditor 解析层的对比金文件（`headers_parse.jsonl`）。

第 48 步要镜像 `app/view/components/headers_editor.py` 的四个纯函数与两处
`HeadersEditor` 方法：

- `shlex.split(s, comments=False, posix=True)`（`parseCurl` 用它拆命令行）
- `parseCurl` / `parseHeaders` / `toHeaderRows` / `toHeadersText`
- `HeadersEditor.headers()`（预设真正落盘的内容）
- `HeadersEditor._refreshDuplicates()`（重复名高亮）

`shlex` 的 posix 分词器细节多（引号内 `\\$` 保留反斜杠、单引号内反斜杠
是字面量、`whitespace_split` 让 wordchars 失效、`\\r` 算空白），所以先把
规格用 Python 固定下来，Rust 侧逐条对比。

`headers()` 与 `_refreshDuplicates()` 直接用真代码跑，只把 `_rows()` /
`_currentRows()` 这两个「从控件里取值」的钩子换成桩——这样镜像的是 Python
本身，而不是抄一遍算法。

    python3 rust/crates/gd3-ui/tests/fixtures/generate_headers_fixture.py
"""

from __future__ import annotations

import json
import shlex
from pathlib import Path

from app.view.components.headers_editor import (
    HeadersEditor,
    parseCurl,
    parseHeaderLine,
    parseHeaders,
    toHeaderRows,
    toHeadersText,
    toLogicalLines,
)

FIXTURE = Path(__file__).with_name("headers_parse.jsonl")


# --- 桩：把「从控件里取值」换成给定数据，其余跑真代码 -------------------


class _FakeRow:
    def __init__(self, name: str, value: str) -> None:
        self._header = (name, value)
        self.isDuplicate = False

    def header(self) -> tuple[str, str]:
        return self._header

    def setDuplicate(self, isDuplicate: bool) -> None:
        self.isDuplicate = isDuplicate


class _FakeRowsEditor:
    def __init__(self, rows: list[tuple[str, str]]) -> None:
        self._widgets = [_FakeRow(name, value) for name, value in rows]

    def _rows(self) -> list[_FakeRow]:
        return self._widgets


class _FakeCurrentRowsEditor:
    def __init__(self, rows: list[tuple[str, str]]) -> None:
        self._rows_data = rows

    def _currentRows(self) -> list[tuple[str, str]]:
        return self._rows_data


def headersOf(rows: list[tuple[str, str]]) -> list[tuple[str, str]]:
    """跑真的 `HeadersEditor.headers()`（只桩掉 `_currentRows`）。"""
    return list(HeadersEditor.headers(_FakeCurrentRowsEditor(rows)).items())


def duplicatesOf(rows: list[tuple[str, str]]) -> list[bool]:
    """跑真的 `HeadersEditor._refreshDuplicates()`（只桩掉 `_rows`）。"""
    editor = _FakeRowsEditor(rows)
    HeadersEditor._refreshDuplicates(editor)
    return [row.isDuplicate for row in editor._rows()]


# --- 语料 --------------------------------------------------------------

# shlex.split 直接喂（含 parseCurl 会先做 `$'` → `'` 替换，这里测原文）
SPLIT_CASES: list[str] = [
    "",
    "   ",
    "\t\r\n ",
    "a",
    " a ",
    "a b",
    "a  b",
    "a\tb",
    "a\rb",
    "a\nb",
    "a\r\nb",
    "a\u00a0b",  # NBSP 不是 shlex 空白
    "a\u001cb",  # \x1c 也不是
    "a\u001fb",
    "a#b",
    "#a b",
    "''",
    '""',
    "'' \"\"",
    "'' '' ''",
    "'a b' c",
    '"a b" c',
    "'a'\"b\"c",
    "a'b'c",
    "a\"b\"c",
    '"a\\"b"',
    "'a\\'b'",  # 单引号内反斜杠是字面量，`\'` 关引号后 b 再开引号 → 见金文件
    "'a\\b'",
    '"a\\\\b"',
    '"a\\$b"',  # 双引号内 \$ 保留反斜杠
    '"a\\`b"',
    '"a\\nb"',
    '"a\\tb"',
    '"a\\ b"',
    '"a\\"',
    "'a\\",
    "a\\ b",
    "a\\\\ b",
    "a\\",
    "\\",
    "\\a",
    "a\\'b",
    'a\\"b',
    "a\\$b",
    "a\\`b",
    # 词外转义的边界：CPython 在 `' '` 态见反斜杠写 escapedstate='a'，
    # 转义字符本身进 token（空白/制表符都能被「带进」词里）。
    "\\ x",
    "\\\tx",
    "a \\ b",
    "'' \\ ",
    "curl 'https://example.com/a b'",
    "curl \"https://example.com/a b\" -H 'x: 1'",
    "curl --data-raw '{\"a\":1}' https://x",
    "curl -H $'x: 1' https://x",
    "curl 'a' && echo hi",
    "curl 'a' && echo hi | grep x",
    "curl 'a' & curl 'b'",
    "curl 'a' ; curl 'b'",
    "curl 'a' || curl 'b'",
    "curl\t'https://x'",
    "curl 'https://x' \\\n -H 'x: 1'",
    "curl 'a b' 'c\"d' \"e'f\"",
    "  curl  'a'  ",
    "curl ''",
    "curl '' ''",
    "curl '\\u4e2d\\u6587'",
    "curl '\u4e2d\u6587: \u503c'",
]

# parseCurl 直接喂（单逻辑行）
CURL_CASES: list[str] = [
    "",
    "curl",
    "curl ",
    "curl 'https://example.com/f.zip'",
    "curl 'https://example.com/f.zip' -H 'accept: text/html'",
    "curl 'https://example.com/f.zip' -H 'accept: text/html' -H 'x-a:1'",
    "curl -H 'x-a:1' 'https://example.com/f.zip'",
    "curl --header 'x-b: 2' https://x",
    "curl -H 'x-c: ' https://x",
    "curl -H 'x-d:' https://x",
    "curl -H 'x-e;' https://x",  # curl 的空值语法，没有冒号 → 丢
    "curl -H 'a b: 1' https://x",  # 名含空白 → 丢
    "curl -H ': v' https://x",  # 空名 → 丢
    "curl -H 'host: example.com' -H 'x-f: a: b' https://x",
    "curl -H 'X-Upper: V' https://x",
    "curl -b 'a=b; c=d' -A 'UA/1' -e 'https://r' https://x",
    "curl --cookie 'a=b' --user-agent 'UA/2' --referer 'https://r2' https://x",
    "curl -b ' padded ' https://x",
    "curl -A ' ' https://x",
    "curl $'https://x' -H $'x-g: 7'",
    "curl 'a' && curl 'b' -H 'x-h: 8'",
    "curl 'a' & curl 'b' -H 'x-i: 9'",
    "curl 'a' ; -H 'x-j: 10'",
    "curl 'a' | grep x -H 'x-k: 11'",
    "curl 'a' || echo -H 'x-l: 12'",
    "curl 'a' -H 'x-m: 13' -H",
    "curl 'a' -H",
    "curl 'a' -H 'x-n: 14' -o out.bin -H 'x-o: 15'",
    "curl 'a' -X POST --compressed -H 'x-p: 16'",
    "curl 'a' --data 'a=1&b=2' -H 'x-q: 17'",
    "curl 'a' -H 'x-r: 18' 'https://y' -H 'x-s: 19'",
    "curl 'a -H 'x-t: 20'",  # 未闭合引号 → ValueError → []
    "curl 'a' -H 'x-u: 21' \\",  # 行尾反斜杠吞掉 EOF → ValueError → []
    "curl 'a' -H 'x-v: 22' -H 'x-v: 23'",
    "curl 'a' -H \"x-w: 24\" -H 'x-x: 25'",
    "curl 'a' -H 'x-y: \\'quoted\\''",
    "curl 'a' -H $'x-z: 26'",
    "CURL 'a' -H 'x-aa: 27'",
    " curl 'a' -H 'x-ab: 28'",
    "curlfoo 'a' -H 'x-ac: 29'",
    "-H 'x-ad: 30'",
    "curl 'a' -H 'x-ae: 31' -H 'x-ae: 32'",
    "curl 'a' -H 'x-af: 33' ; curl 'b' -H 'x-ag: 34'",
]

# parseHeaders 直接喂（多逻辑行）
HEADERS_CASES: list[str] = [
    "",
    "   ",
    "\n\n\n",
    "a: 1",
    "a: 1\nb: 2",
    "a: 1\r\nb: 2",
    "a: 1\rb: 2",
    "a: 1\u000bb: 2",
    "a: 1\u2028b: 2",
    "  a :  1  \n\n  \n",
    "no-colon\nb: 2",
    ": v\nb: 2",
    "a b: 1\nc: 2",
    "a: 1\ncurl 'https://x' -H 'b: 2'",
    "curl 'https://x' -H 'b: 2'\nc: 3",
    "curl 'https://x' -H 'b: 2'\ncurl 'https://y' -H 'd: 4'",
    "curl 'https://x' -H 'b: 2' && curl 'https://y' -H 'd: 4'",
    "CURL 'https://x' -H 'b: 2'",
    "curl\t'https://x' -H 'b: 2'",
    "curlfoo: 1",
    "curl : 1",
    "curl 'https://x' -H 'b: 2' \\\n  -H 'c: 3'",
    "curl 'https://x' -H 'b: 2' ^\n  -H 'c: 3'",
    "curl 'https://x' -H 'a b: 2'",
    "curl 'https://x' -H 'b: 2'\n\n\nd: 4\n",
    "\u001f",
    "a: 1\u001fb: 2",
]

# toHeaderRows 直接喂（视图切换必须零丢弃）
ROWS_CASES: list[str] = [
    "",
    "   ",
    "a: 1",
    "a: 1\nb: 2",
    "a: 1\r\nb: 2",
    "a: 1\rb: 2",
    "a: 1\u000bb: 2",
    "no-colon",
    "no-colon\nb: 2",
    ": v",
    "a: 1: 2",
    "a :  1  ",
    "  a  ",
    "a\n\n\nb",
    "curl 'https://x' -H 'b: 2'",
    "a b: 1",
    "a:",
    ": ",
    " \t ",
    "\u001f",
]

# toHeadersText 直接喂（值空 → 只写名，Python 口径）
TEXT_CASES: list[list[tuple[str, str]]] = [
    [],
    [("a", "1")],
    [("a", "")],
    [("a", "1"), ("b", "")],
    [("a", ""), ("b", "2")],
    [("accept-encoding", "deflate, br, gzip")],
    [("a", " 1 ")],
    [("", "1")],
    [("a", "1\n2")],
    [("\u4e2d\u6587", "\u503c")],
]

# headers() 直接喂（rows 是 _currentRows 的产物）
MAP_CASES: list[list[tuple[str, str]]] = [
    [],
    [("a", "1")],
    [("A", "1")],
    [("  A  ", " 1 ")],
    [("a", "")],
    [("", "1")],
    [("a", "1"), ("a", "2")],
    [("A", "1"), ("a", "2")],
    [("a", "2"), ("A", "1")],
    [("b", "1"), ("a", "2"), ("b", "3")],
    [("a", " "), ("b", "2")],
    [("no-colon", "")],
    [("\u4e2d\u6587", "\u503c")],
    [("\u0130", "1")],  # İ 小写后是两个码点
]

# _refreshDuplicates 直接喂（只看名称列）
DUP_CASES: list[list[tuple[str, str]]] = [
    [],
    [("a", "1")],
    [("a", "1"), ("b", "2")],
    [("a", "1"), ("a", "2")],
    [("A", "1"), ("a", "2")],
    [(" a ", "1"), ("a", "2")],
    [("a", "1"), ("b", "2"), ("a", "3")],
    [("", "1"), ("", "2")],
    [("a", ""), ("a", "")],
    [("a", "1"), ("b", "2"), ("c", "3"), ("B", "4")],
]


def main() -> None:
    lines: list[str] = []

    def emit(kind: str, **fields: object) -> None:
        payload = {"kind": kind, **fields}
        lines.append(json.dumps(payload, ensure_ascii=False))

    for text in SPLIT_CASES:
        try:
            tokens: list[str] | None = shlex.split(text)
        except ValueError:
            tokens = None
        emit("split", input=text, tokens=tokens)

    for line in CURL_CASES:
        emit(
            "curl",
            input=line,
            normalized=line.replace("$'", "'"),
            rows=[[name, value] for name, value in parseCurl(line)],
        )

    for text in HEADERS_CASES:
        emit(
            "headers",
            input=text,
            lines=toLogicalLines(text),
            rows=[[name, value] for name, value in parseHeaders(text)],
        )

    for text in ROWS_CASES:
        emit(
            "rows",
            input=text,
            rows=[[name, value] for name, value in toHeaderRows(text)],
        )

    for pairs in TEXT_CASES:
        emit(
            "text",
            pairs=[[name, value] for name, value in pairs],
            text=toHeadersText(pairs),
        )

    for pairs in MAP_CASES:
        emit(
            "map",
            pairs=[[name, value] for name, value in pairs],
            rows=[[name, value] for name, value in headersOf(pairs)],
        )

    for pairs in DUP_CASES:
        emit(
            "dup",
            pairs=[[name, value] for name, value in pairs],
            flags=duplicatesOf(pairs),
        )

    # parseHeaderLine 单独覆盖（parseCurl / parseHeaders 都会走它）
    for line in [
        "",
        "a",
        "a:",
        "a: ",
        ":",
        ": 1",
        " a : 1 ",
        "a b: 1",
        "a\tb: 1",
        "a: b: c",
        "a:b",
        "\u001fa: 1",
        "\u4e2d\u6587: \u503c",
    ]:
        row = parseHeaderLine(line)
        emit("header_line", input=line, row=None if row is None else [row[0], row[1]])

    FIXTURE.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"{FIXTURE.name}: {len(lines)} 条")


if __name__ == "__main__":
    main()
