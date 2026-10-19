"""Phase 8c：`UserConfig.json` 的 Python ↔ Rust 往返（逐字节一致 + 改一项只动一行）。

完工判定 2 的验收：用 **Python 自己的写盘路径**（`QConfig.save` → `json.dump(indent=4)`）
写出一份配置 → 让 **Rust 前端**（`gd3-ui`，走 `settings_writer` / `ConfigStore`）
读进来再整文件回写 → 两侧字节一致；再用 `--set` 改一项，断言**只有那一行变**，
最后 Python 侧能读回新值。

这条测试同时钉住「单一写者」口径：Rust 回写时**不能**顺手改别的字段（比如把
`HistoryDownloadFolder` 重排、把空数组写成 `[]`、改缩进）。
"""

from __future__ import annotations

import subprocess
from pathlib import Path

import pytest
from qfluentwidgets import qconfig

from app.config.cfg import cfg
from app.platform.ui_backend import findRustUiBinary

CONFIG_NAME = "UserConfig.json"


def rustBinary() -> Path:
    binary = findRustUiBinary()
    if binary is None:
        pytest.skip("没有构建 gd3-ui（cargo build -p gd3-ui）")
    return binary


def runRust(data_dir: Path, *extra: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [str(rustBinary()), "--headless", "--frames", "0", "--data-dir", str(data_dir), *extra],
        capture_output=True,
        text=True,
        timeout=180,
    )


def changed_lines(before: str, after: str) -> list[tuple[str, str]]:
    """逐行比（行数必须一样，否则说明整文件被重排了）。"""
    old = before.splitlines()
    new = after.splitlines()
    assert len(old) == len(new), f"行数变了：{len(old)} → {len(new)}"
    return [(a, b) for a, b in zip(old, new, strict=True) if a != b]


@pytest.fixture
def python_written_config(tmp_path, qapp, monkeypatch):
    """用 Python 的写盘路径产出一份 `UserConfig.json`（不写进真实数据目录）。"""
    path = tmp_path / CONFIG_NAME
    qconfig.load(str(path), cfg)
    # 令牌非空：Rust 侧只在为空时才生成（`browser_service.py:190`），
    # 否则这条测试会被那一次「正常」的生成干扰。
    monkeypatch.setattr(cfg.browserExtensionPairToken, "value", "roundtrip-token")
    monkeypatch.setattr(cfg.maxTaskNum, "value", 3)
    monkeypatch.setattr(cfg.downloadFolder, "value", str(tmp_path / "dl"))
    qconfig.save()
    assert path.exists(), "Python 侧没写出配置"
    return path


def test_rust_rewrites_a_python_written_config_byte_for_byte(python_written_config):
    before = python_written_config.read_text(encoding="utf-8")

    completed = runRust(python_written_config.parent)

    assert completed.returncode == 0, completed.stderr
    after = python_written_config.read_text(encoding="utf-8")
    assert after == before, changed_lines(before, after)


def test_set_changes_exactly_one_line(python_written_config):
    before = python_written_config.read_text(encoding="utf-8")

    completed = runRust(python_written_config.parent, "--set", "maxTaskNum=7")

    assert completed.returncode == 0, completed.stderr
    after = python_written_config.read_text(encoding="utf-8")
    assert changed_lines(before, after) == [('        "MaxTaskNum": 3,', '        "MaxTaskNum": 7,')]


def test_python_reads_the_value_rust_wrote_back(python_written_config):
    completed = runRust(python_written_config.parent, "--set", "maxTaskNum=7")
    assert completed.returncode == 0, completed.stderr

    qconfig.load(str(python_written_config), cfg)

    assert cfg.maxTaskNum.value == 7
    # 其他项一个都不能被顺手改掉。
    assert cfg.browserExtensionPairToken.value == "roundtrip-token"
    assert cfg.downloadFolder.value == str(python_written_config.parent / "dl")


def test_rust_applies_a_bare_string_and_a_json_value(python_written_config):
    completed = runRust(
        python_written_config.parent,
        "--set",
        "clientProfile=chrome",
        "--set",
        "autoSpeedUp=false",
    )
    assert completed.returncode == 0, completed.stderr

    qconfig.load(str(python_written_config), cfg)

    assert cfg.clientProfile.value == "chrome", "裸写当字符串"
    assert cfg.autoSpeedUp.value is False, "`false` 按 JSON 解析成布尔"
