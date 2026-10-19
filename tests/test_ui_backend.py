"""`GHOST_UI` 前端分派（`app/platform/ui_backend.py` + 仓库根入口）。

Phase 8 的共存口径：`GHOST_UI=slint` 走 Rust/Slint 前端，缺省（或 `qt`）走原
PySide6 入口。这里钉三件事：

1. `uiBackend()` 的取值规则（缺省 / 大小写 / 未知值）；
2. `findRustUiBinary()` 的找法（`GD3_UI_BIN` 优先，找不到返回 `None`）；
3. **入口真的把进程换掉了** —— 用子进程跑 `Ghost-Downloader-3.py`，断言 Rust 前端
   收到了 `sys.argv[1:]`，而 Qt 侧一点都没启动。
"""

from __future__ import annotations

import os
import stat
import subprocess
import sys
from pathlib import Path

import pytest

from app.platform.ui_backend import (
    BINARY_ENV_VAR,
    DEFAULT_BACKEND,
    ENV_VAR,
    SLINT_BACKEND,
    dispatchRustUi,
    findRustUiBinary,
    uiBackend,
)

ROOT = Path(__file__).parents[1]


@pytest.fixture(autouse=True)
def _clean_env(monkeypatch):
    """两个开关都清掉，免得外层环境（比如 CI 的 `GHOST_UI=slint`）影响断言。"""
    monkeypatch.delenv(ENV_VAR, raising=False)
    monkeypatch.delenv(BINARY_ENV_VAR, raising=False)


def _fakeBinary(path: Path) -> Path:
    """一个只打印自己收到的参数、然后退出 0 的假前端。"""
    path.write_text(
        "#!/bin/sh\n"
        'echo "fake-gd3-ui $*"\n'
        'echo "QT_STARTED=no"\n'
        "exit 0\n",
        encoding="utf-8",
    )
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return path


# ---------------------------------------------------------------------
# 取值规则
# ---------------------------------------------------------------------


def test_missing_env_means_qt():
    assert uiBackend() == DEFAULT_BACKEND == "qt"


@pytest.mark.parametrize("value", ["qt", "QT", " qt ", ""])
def test_qt_spellings_all_mean_qt(monkeypatch, value):
    monkeypatch.setenv(ENV_VAR, value)
    assert uiBackend() == "qt"


@pytest.mark.parametrize("value", ["slint", "Slint", " SLINT "])
def test_slint_spellings_all_mean_slint(monkeypatch, value):
    monkeypatch.setenv(ENV_VAR, value)
    assert uiBackend() == SLINT_BACKEND


def test_unknown_value_warns_and_falls_back_to_qt(monkeypatch, capsys):
    monkeypatch.setenv(ENV_VAR, "sailfish")

    assert uiBackend() == "qt"
    assert "sailfish" in capsys.readouterr().err


# ---------------------------------------------------------------------
# 找二进制
# ---------------------------------------------------------------------


def test_binary_env_var_wins(monkeypatch, tmp_path):
    path = _fakeBinary(tmp_path / "gd3-ui")
    monkeypatch.setenv(BINARY_ENV_VAR, str(path))

    assert findRustUiBinary() == path


def test_missing_binary_env_var_means_none(monkeypatch, tmp_path):
    monkeypatch.setenv(BINARY_ENV_VAR, str(tmp_path / "not-there"))

    assert findRustUiBinary() is None


def test_auto_discovery_only_returns_real_files():
    found = findRustUiBinary()
    if found is not None:
        assert found.is_file()
        assert found.name.startswith("gd3-ui")


# ---------------------------------------------------------------------
# 分派
# ---------------------------------------------------------------------


def test_qt_backend_never_execs(monkeypatch):
    def boom(*args, **kwargs):  # pragma: no cover - 不该被调用
        raise AssertionError("qt 后端不该 exec")

    monkeypatch.setattr(os, "execv", boom)

    assert dispatchRustUi(["--version"]) is False


def test_slint_backend_hands_over_argv_and_binary(monkeypatch, tmp_path):
    path = _fakeBinary(tmp_path / "gd3-ui")
    monkeypatch.setenv(ENV_VAR, SLINT_BACKEND)
    monkeypatch.setenv(BINARY_ENV_VAR, str(path))
    recorded: list[tuple] = []

    def fake_execv(file, args):
        recorded.append((file, list(args)))
        raise SystemExit(0)

    monkeypatch.setattr(os, "execv", fake_execv)

    with pytest.raises(SystemExit):
        dispatchRustUi(["--headless", "--frames", "2"])

    assert recorded == [
        (str(path), [str(path), "--headless", "--frames", "2"]),
    ]


def test_slint_backend_without_a_binary_falls_back_to_qt(monkeypatch, tmp_path, capsys):
    monkeypatch.setenv(ENV_VAR, SLINT_BACKEND)
    monkeypatch.setenv(BINARY_ENV_VAR, str(tmp_path / "not-there"))

    assert dispatchRustUi([]) is False
    assert "回退到 Qt" in capsys.readouterr().err


def test_exec_failure_falls_back_to_qt(monkeypatch, tmp_path, capsys):
    """文件在、但不是可执行文件：`execv` 抛 `OSError`，要能捡起来走 Qt。"""
    path = tmp_path / "gd3-ui"
    path.write_text("not a program", encoding="utf-8")
    monkeypatch.setenv(ENV_VAR, SLINT_BACKEND)
    monkeypatch.setenv(BINARY_ENV_VAR, str(path))

    assert dispatchRustUi([]) is False
    assert "回退到 Qt" in capsys.readouterr().err


# ---------------------------------------------------------------------
# 真入口（子进程）
# ---------------------------------------------------------------------


def test_entry_point_replaces_itself_with_the_rust_frontend(tmp_path):
    """`GHOST_UI=slint` + 真入口脚本 → Rust 前端拿到 `sys.argv[1:]`，Qt 不启动。"""
    path = _fakeBinary(tmp_path / "gd3-ui")
    env = dict(os.environ)
    env[ENV_VAR] = SLINT_BACKEND
    env[BINARY_ENV_VAR] = str(path)

    completed = subprocess.run(
        [sys.executable, str(ROOT / "Ghost-Downloader-3.py"), "--silence", "https://example.com/a"],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=120,
    )

    assert completed.returncode == 0, completed.stderr
    assert "fake-gd3-ui --silence https://example.com/a" in completed.stdout, completed.stdout
    assert "QT_STARTED=no" in completed.stdout
