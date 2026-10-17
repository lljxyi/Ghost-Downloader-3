"""ed2k `parseProgress` 双实现 parity（`GD3_RUST_ED2K_PROGRESS`）。

对同一份语料 `rust/crates/gd3-core/tests/fixtures/ed2k_progress.jsonl`
（40 条，由 `generate_ed2k_progress_fixture.py` 用 Python 参考实现生成）：

- 成功路径：原实现的 `repr(Progress)` 与 Rust 影子分支拼回的 `Progress`
  `repr` 逐字节一致（repr 消歧 1 vs 1.0 / None / bool / nan / inf）；
- 失败路径：异常**类型名 / str** 逐字节一致（桥直接抛
  `KeyError` / `TypeError` 原型，由 `onMessage` 的兜底 catch 接收）。

每个用例都走被接线的 `kelpieModule.parseProgress` 入口（与 `onMessage`
生产路径一致），开关随写随验；最后补开关默认值与适配器直达入口。
"""
from __future__ import annotations

import json
import os
from contextlib import contextmanager
from pathlib import Path

from features.ed2k_pack.kelpie import kelpie as kelpieModule
from features.ed2k_pack.kelpie import rust_progress

SWITCH = "GD3_RUST_ED2K_PROGRESS"
FIXTURE = (
    Path(__file__).resolve().parent.parent
    / "rust/crates/gd3-core/tests/fixtures/ed2k_progress.jsonl"
)

CASES = [json.loads(line) for line in FIXTURE.read_text(encoding="utf-8").splitlines()]


@contextmanager
def rustParse(enabled: bool):
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


def parse(case: dict):
    """按语料记录的期望路径直接调用被接线的 `parseProgress`。"""
    with rustParse("error" not in case):
        return kelpieModule.parseProgress(case["message"])


def test_switch_defaults_off(monkeypatch):
    monkeypatch.delenv(SWITCH, raising=False)
    assert rust_progress.isEnabled() is False


def test_parity_against_python_reference():
    mismatches: list[str] = []
    for index, case in enumerate(CASES):
        try:
            if "error" in case:
                try:
                    parse(case)
                    mismatches.append(f"#{index} 期望异常却成功：{case['error']}")
                except Exception as error:  # noqa: BLE001
                    if type(error).__name__ != case["error"]["type"]:
                        mismatches.append(
                            f"#{index} 异常类型 {type(error).__name__!r}"
                            f" ≠ {case['error']['type']!r}"
                        )
                    if str(error) != case["error"]["str"]:
                        mismatches.append(
                            f"#{index} 异常 str {str(error)!r} ≠ {case['error']['str']!r}"
                        )
            else:
                parsed = parse(case)
                if repr(parsed) != case["repr"]:
                    mismatches.append(
                        f"#{index} repr\n  Rust: {repr(parsed)!r}\n  原生: {case['repr']!r}"
                    )
        except Exception as error:  # 语料期待成功却炸了等结构化事故
            mismatches.append(f"#{index} 测试骨架异常 {type(error).__name__}: {error}")
    assert not mismatches, f"{len(mismatches)} 条不一致：\n" + "\n".join(mismatches)
