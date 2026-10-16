"""m3u8 输出解析层双实现 parity（`GD3_RUST_M3U8_OUTPUT`）。

对同一份语料，分别用「纯 Python 参考实现」与「Rust 实现」跑
`parseDiagnostic` / `parseError` / `M3U8TaskStep._parseOutputLine`，
断言两侧的可观测结果**逐字段一致**（含 params 的值类型）。

语料复用 `tests/test_m3u8_progress.py` / `tests/test_m3u8_error.py`，
并补上两侧都容易走偏的边界：空行、纯空白、unicode、超长行截断、
多行拼接取末条、`-` 速度、重试后缀 `(9)`、`totalSize == 0`。
"""
from __future__ import annotations

import os
from contextlib import contextmanager
from pathlib import Path

import pytest

from features.m3u8_pack.task import (
    M3U8Task,
    M3U8TaskStep,
    parseDiagnostic,
    parseDiagnosticPython,
    parseError,
    parseErrorPython,
)

SWITCH = "GD3_RUST_M3U8_OUTPUT"


@contextmanager
def rustOutput(enabled: bool):
    """临时把影子开关掰到指定状态（不改调用方的环境）。"""
    previous = os.environ.get(SWITCH)
    if enabled:
        os.environ[SWITCH] = "1"
    else:
        os.environ.pop(SWITCH, None)
    try:
        yield
    finally:
        if previous is None:
            os.environ.pop(SWITCH, None)
        else:
            os.environ[SWITCH] = previous


def makeStep() -> M3U8TaskStep:
    task = M3U8Task(
        name="test.mp4",
        url="http://test",
        fileSize=1,
        outputFolder=Path("."),
    )
    step = M3U8TaskStep(stepIndex=1)
    task.addStep(step)
    return step


def snapshotLine(line: str) -> dict:
    step = makeStep()
    step._parseOutputLine(line)
    return {
        "progress": step.progress,
        "speed": step.speed,
        "receivedBytes": step.receivedBytes,
        "fileSize": step.task.fileSize,
        "lastMessage": step.lastMessage,
        "liveElapsed": step.liveElapsed,
        "liveTotal": step.liveTotal,
        "liveStatus": step.liveStatus,
    }


def bothLine(line: str) -> tuple[dict, dict]:
    with rustOutput(False):
        expected = snapshotLine(line)
    with rustOutput(True):
        actual = snapshotLine(line)
    return expected, actual


def snapshotError(output: str, returncode: int):
    try:
        error = parseError(output, returncode)
    except Exception as exc:  # noqa: BLE001 - 两侧都要把异常类型与文案对齐
        return ("raise", type(exc).__name__, str(exc))
    return (
        "error",
        error.message,
        str(error),
        dict(error.params),
        tuple(sorted((name, type(value).__name__) for name, value in error.params.items())),
    )


def bothError(output: str, returncode: int):
    with rustOutput(False):
        expected = snapshotError(output, returncode)
    with rustOutput(True):
        actual = snapshotError(output, returncode)
    return expected, actual


def snapshotDiagnostic(output: str):
    with rustOutput(False):
        expected = parseDiagnostic(output)
    with rustOutput(True):
        actual = parseDiagnostic(output)
    return expected, actual


# ── 语料 ────────────────────────────────────────────────────

CONCATENATED_VOD = (
    "Vid 1280x720 | 1650 Kbps | 30 ━━━━━━ 3/155 1.94% -0.00Bps00:00:37"
    "Vid 1280x720 | 1650 Kbps | 30 ━━━━━━ 35/155 22.58% 30.99MB/137.26MB28.45MBps00:00:06"
    "Vid 1280x720 | 1650 Kbps | 30 ━━━━━━ 100/155 64.52% 71.82MB/148.43MB40.83MBps00:00:02"
)

LINES = [
    ("empty", ""),
    ("whitespace_only", "   \t  "),
    ("vod_v06_with_sizes", "Vid 1280x720 | 2000 Kbps ━━━━━━ 6/6857 0.09% 3.12MB/3.48GB1.55MBps01:27:07"),
    ("vod_negative_speed", "Vid 1280x720 | 2000 Kbps ━━━━━━ 0/6857 0.00% -191.58KBps --:--:--"),
    ("vod_negative_speed_retry", "Vid 1280x720 | 2000 Kbps ━━━━━━ 0/6857 0.00% -0.00Bps(9) --:--:--"),
    ("vod_zero_bps", "Vid 1920x1080 | 6221 Kbps ━━━━━━ 0/64 0.00% -0.00Bps --:--:--"),
    ("vod_legacy_spaces", "12/64 18.75% 12.34MB/56.78MB 1.23MBps 00:01:30"),
    ("vod_dash_placeholder", "6/6857 0.09% -1.55MBps"),
    ("vod_gbps", "6/6857 0.09% 1.55GBps"),
    ("vod_zero_total_size", "6/6857 0.09% 0.00MB/0.00B 1.55MBps"),
    ("vod_concatenated_takes_last", CONCATENATED_VOD),
    ("vod_leading_trailing_space", "  12/64 18.75% 12.34MB/56.78MB 1.23MBps 00:01:30  "),
    ("live_progress", "Vid ━━━━━━ 01m30s/02m00s 125/250 Recording 50% 1.55MBps"),
    ("live_progress_over_one_hour", "Vid ━━━━━━ 01h05m30s/01h30m00s 3000/4500 Recording 66% 2.10MBps"),
    ("live_progress_waiting_dash", "Vid ━━━━━━ 00m00s/00m00s 0/0 Waiting 0% -"),
    ("live_record", "Vid ━━━━━━ 01m30s/02m00s 130.00MB 125/250 Recording 1.55MBps"),
    ("live_record_over_one_hour", "Vid ━━━━━━ 02h10m00s/02h15m30s 5.12GB 9000/9500 Recording 30.00MBps"),
    ("live_record_dash_speed", "Vid ━━━━━━ 00m10s/00m30s 0.50MB 5/15 Recording -"),
    ("unmatched_unicode", "纯中文没有任何进度信息"),
    ("unmatched_ascii", "Loading URL: http://example.com/index.m3u8"),
    ("vod_wins_over_live", "Vid ━━━━━━ 01m30s/02m00s 125/250 Recording 50% 1.55MBps 12/64 18.75% 1.23MBps"),
    ("tail_truncation_ascii", "x" * 2000),
    ("tail_truncation_multibyte", "━" * 1500),
    ("tail_exactly_1000", "y" * 1000),
    ("tail_1001", "y" * 1001),
]

DIAGNOSTIC_OUTPUTS = [
    ("empty", ""),
    ("info_only", "22:17:47.595 INFO : N_m3u8DL-RE started\n"),
    ("last_warn_wins", (
        "22:17:47.595 INFO : N_m3u8DL-RE (Beta version) 20260628\n"
        "22:17:48.730 WARN : Response status code does not indicate success: 403 (Forbidden). (1/10)\n"
        "22:17:50.235 INFO : Loading URL: http://example.com\n"
        "22:17:51.306 WARN : Response status code does not indicate success: 403 (Forbidden). (2/10)\n"
    )),
    ("error_beats_earlier_warn", (
        "22:17:48.730 WARN : some warning\n"
        "22:18:00.000 ERROR: ffmpeg not found\n"
    )),
    ("error_without_space_after_colon", "22:18:00.000 ERROR: Failed\n"),
    ("stack_trace_ignored", (
        "22:18:14.893 WARN : 403 (Forbidden). (10/10)\n"
        "Unhandled exception: System.Exception: Failed\n"
        " ---> System.Net.Http.HttpRequestException: 403\n"
        "   at System.Net.Http.EnsureSuccessStatusCode()\n"
    )),
    ("concatenated_warn_and_unhandled", (
        "22:18:14.893 WARN : 403 (Forbidden). (10/10)"
        "Unhandled exception: System.Exception: Failed\n"
        " ---> System.Net.Http.HttpRequestException: 403\n"
    )),
    ("unhandled_only", (
        "17:07:01.414 INFO : N_m3u8DL-RE started\n"
        "Unhandled exception: System.Exception: something went wrong\n"
        "   at SomeNamespace.SomeMethod()\n"
    )),
    ("unhandled_not_supported_garbled", (
        "17:07:01.414 INFO : \xd8\xb6, ANSI\xc9\xab\n"
        "17:07:01.419 INFO : N_m3u8DL-RE (Beta version) 20260628\n"
        "Unhandled exception: System.NotSupportedException: garbled text\n"
        "   at N_m3u8DL_RE.Parser.StreamExtractor.LoadSourceFromText(String) + 0x282\n"
    )),
    ("unhandled_no_message", "Unhandled exception: System.Exception:\n"),
    ("unhandled_no_type", "Unhandled exception: 没有类型名\n"),
    ("info_line_looks_like_log", "22:18:00.000 INFO : 403 (Forbidden). (10/10)\n"),
    ("crlf_output", "22:17:48.730 WARN : first\r\n22:18:00.000 ERROR: second\r\n"),
]

ERROR_OUTPUTS = [
    ("empty_output_rc1", "", 1),
    ("empty_output_rc0", "", 0),
    ("empty_output_rc_negative", "", -9),
    ("info_only_rc1", "22:17:47.595 INFO : N_m3u8DL-RE started\n", 1),
    ("forbidden_403", (
        "22:18:14.893 WARN : Response status code does not indicate success: "
        "403 (Forbidden). (10/10)\n"
    ), 1),
    ("not_found_404", (
        "22:18:14.893 WARN : Response status code does not indicate success: "
        "404 (Not Found). (3/10)\n"
    ), 1),
    ("generic_http_500", (
        "22:18:14.893 WARN : Response status code does not indicate success: "
        "500 (Internal Server Error). (1/10)\n"
    ), 1),
    ("http_403_in_info_line_only", "22:18:00.000 INFO : 403 (Forbidden). (10/10)\n", 1),
    ("not_supported_exception", (
        "17:07:01.414 INFO : \xd8\xb6, ANSI\xc9\xab\n"
        "17:07:01.419 INFO : N_m3u8DL-RE (Beta version) 20260628\n"
        "Unhandled exception: System.NotSupportedException: garbled text\n"
        "   at N_m3u8DL_RE.Parser.StreamExtractor.LoadSourceFromText(String) + 0x282\n"
    ), 1),
    ("file_not_found_exception", (
        "17:07:01.414 INFO : N_m3u8DL-RE (Beta version) 20260628\n"
        "Unhandled exception: System.IO.FileNotFoundException: "
        "ffmpeg not found, please download at: https://ffmpeg.org\n"
        "   at N_m3u8DL_RE.Program.<DoWorkAsync>d__3.MoveNext()\n"
    ), 1),
    ("dns_failure", "22:18:00.000 WARN : No such host is known. (1/10)\n", 1),
    ("dns_failure_uppercase", "22:18:00.000 WARN : NO SUCH HOST IS KNOWN. (1/10)\n", 1),
    ("connection_refused", "22:18:00.000 WARN : Connection refused (1/10)\n", 1),
    ("timeout", (
        "22:18:00.000 WARN : The request was canceled due to the configured "
        "HttpClient.Timeout (1/10)\n"
    ), 1),
    ("ssl_failure", "22:18:00.000 WARN : The SSL connection could not be established (1/10)\n", 1),
    ("fallback_with_diagnostic", "22:18:00.000 ERROR: some unknown error\n", 1),
    ("fallback_with_unhandled", (
        "17:07:01.414 INFO : N_m3u8DL-RE started\n"
        "Unhandled exception: System.Exception: something went wrong\n"
        "   at SomeNamespace.SomeMethod()\n"
    ), 1),
    ("fallback_with_stack_trace", (
        "22:18:14.893 WARN : 403 (Forbidden). (10/10)\n"
        "Unhandled exception: System.Exception: Failed\n"
        "   at SomeNamespace.SomeMethod()\n"
    ), 1),
    ("hint_order_403_before_generic", (
        "22:18:14.893 WARN : Response status code does not indicate success: "
        "403 (Forbidden). (10/10)\n"
        "22:18:15.000 WARN : Response status code does not indicate success: 500\n"
    ), 1),
]


# ── parseDiagnostic ────────────────────────────────────────

@pytest.mark.parametrize("name,output", DIAGNOSTIC_OUTPUTS, ids=[case[0] for case in DIAGNOSTIC_OUTPUTS])
def test_parse_diagnostic_parity(name: str, output: str):
    expected, actual = snapshotDiagnostic(output)
    assert actual == expected


# ── parseError ─────────────────────────────────────────────

@pytest.mark.parametrize(
    "name,output,returncode",
    ERROR_OUTPUTS,
    ids=[case[0] for case in ERROR_OUTPUTS],
)
def test_parse_error_parity(name: str, output: str, returncode: int):
    expected, actual = bothError(output, returncode)
    assert actual == expected


# ── _parseOutputLine ───────────────────────────────────────

@pytest.mark.parametrize("name,line", LINES, ids=[case[0] for case in LINES])
def test_parse_output_line_parity(name: str, line: str):
    expected, actual = bothLine(line)
    assert actual == expected


def test_parse_output_line_parity_keeps_state_across_lines():
    """连续多行（模拟 `_readOutput` 的逐行喂入）也要逐行一致。"""
    lines = [
        "Vid 1280x720 | 2000 Kbps ━━━━━━ 0/6857 0.00% -191.58KBps --:--:--",
        "",
        "Vid 1280x720 | 2000 Kbps ━━━━━━ 6/6857 0.09% 3.12MB/3.48GB1.55MBps01:27:07",
        "Loading URL: http://example.com/index.m3u8",
        "Vid ━━━━━━ 01m30s/02m00s 125/250 Recording 50% 1.55MBps",
        "Vid ━━━━━━ 01m30s/02m00s 130.00MB 125/250 Recording 1.55MBps",
    ]

    def run() -> list[dict]:
        step = makeStep()
        states = []
        for line in lines:
            step._parseOutputLine(line)
            states.append(
                {
                    "progress": step.progress,
                    "speed": step.speed,
                    "receivedBytes": step.receivedBytes,
                    "fileSize": step.task.fileSize,
                    "lastMessage": step.lastMessage,
                    "liveElapsed": step.liveElapsed,
                    "liveTotal": step.liveTotal,
                    "liveStatus": step.liveStatus,
                }
            )
        return states

    with rustOutput(False):
        expected = run()
    with rustOutput(True):
        actual = run()
    assert actual == expected


# ── 接线本身 ───────────────────────────────────────────────

def test_switch_off_uses_python_reference(monkeypatch):
    """开关关闭时不能碰 Rust 桥（`gd3_py` 被换成会炸的替身）。"""
    import gd3_py

    from features.m3u8_pack import task as m3u8Task

    def boom(*args, **kwargs):
        raise AssertionError("开关关闭时不应调用 Rust 桥")

    monkeypatch.setattr(gd3_py, "m3u8_parse_diagnostic", boom)
    monkeypatch.setattr(gd3_py, "m3u8_classify_error", boom)
    monkeypatch.setattr(gd3_py, "m3u8_parse_output_line", boom)
    monkeypatch.delenv(SWITCH, raising=False)

    output = (
        "22:18:14.893 WARN : Response status code does not indicate success: "
        "403 (Forbidden). (10/10)\n"
    )
    assert m3u8Task.parseDiagnostic(output).startswith("Response status code")
    assert m3u8Task.parseError(output, 1).params["detail"].startswith("Response status code")
    step = makeStep()
    step._parseOutputLine("Vid ━━━━━━ 01m30s/02m00s 125/250 Recording 50% 1.55MBps")
    assert step.liveElapsed == "01m30s"


def test_switch_on_routes_to_rust(monkeypatch):
    """开关打开时三个入口都要经过适配器（用 spy 记录调用）。"""
    from features.m3u8_pack import rust_output, task as m3u8Task

    calls: list[tuple] = []

    def spyParseDiagnostic(output):
        calls.append(("diagnostic", output))
        return "RUST-DIAGNOSTIC"

    def spyParseError(output, returncode):
        calls.append(("error", output, returncode))
        return rust_output.TaskError("RUST-ERROR", detail="d")

    def spyApplyOutputLine(step, line):
        calls.append(("line", line))
        step.lastMessage = "RUST-LINE"

    monkeypatch.setattr(rust_output, "parseDiagnostic", spyParseDiagnostic)
    monkeypatch.setattr(rust_output, "parseError", spyParseError)
    monkeypatch.setattr(rust_output, "applyOutputLine", spyApplyOutputLine)
    monkeypatch.setenv(SWITCH, "1")

    assert m3u8Task.parseDiagnostic("x") == "RUST-DIAGNOSTIC"
    assert m3u8Task.parseError("x", 3).message == "RUST-ERROR"
    step = makeStep()
    step._parseOutputLine("x")
    assert step.lastMessage == "RUST-LINE"
    assert calls == [("diagnostic", "x"), ("error", "x", 3), ("line", "x")]


def test_python_reference_never_calls_rust(monkeypatch):
    """`parseErrorPython` / `parseDiagnosticPython` / `_parseOutputLinePython`
    必须自洽，不能混用 Rust 结果（否则 parity 会自我实现）。"""
    import gd3_py

    def boom(*args, **kwargs):
        raise AssertionError("纯 Python 参考实现不应调用 Rust 桥")

    monkeypatch.setattr(gd3_py, "m3u8_parse_diagnostic", boom)
    monkeypatch.setattr(gd3_py, "m3u8_classify_error", boom)
    monkeypatch.setattr(gd3_py, "m3u8_parse_output_line", boom)

    output = (
        "22:18:14.893 WARN : Response status code does not indicate success: "
        "403 (Forbidden). (10/10)\n"
    )
    assert parseDiagnosticPython(output).startswith("Response status code")
    error = parseErrorPython(output, 1)
    assert error.params["detail"].startswith("Response status code")
    assert "服务器拒绝了请求" in error.message
    assert parseErrorPython("", 7).params["code"] == 7

    step = makeStep()
    step._parseOutputLinePython("Vid ━━━━━━ 01m30s/02m00s 125/250 Recording 50% 1.55MBps")
    assert step.liveElapsed == "01m30s"
    assert step.progress == pytest.approx(50)
