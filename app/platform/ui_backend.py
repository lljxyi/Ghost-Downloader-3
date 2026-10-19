"""前端分派：`GHOST_UI` 环境变量决定用哪套 UI（Qt 是缺省，随时可回退）。

Phase 8 的落地方式：

=================  ====================================================
`GHOST_UI`         行为
=================  ====================================================
未设 / `qt`        走原 PySide6 入口（缺省，可回退）
`slint`            `os.execv` 把进程换成 `gd3-ui`（Rust/Slint 前端）
其他值             当作 `qt`，只打一条警告
=================  ====================================================

`gd3-ui` 二进制的找法（按顺序）：`GD3_UI_BIN` → 可执行文件同目录 →
仓库里的 `rust/target/release` → `rust/target/debug`。

## 为什么必须**在任何 Qt 侧初始化之前**分派

Rust 侧（`rust/crates/gd3-ui/src/boot.rs`）自己会：

- 读写 `UserConfig.json`；
- 读写 `tasks.jsonl`；
- 上自己的单实例锁（`APP_DATA_DIR/instance.sock`）。

所以不能先跑 Python 的 `setupEnvironment()`（`qconfig.load` 读同一个
`UserConfig.json`，而 QConfig 在主题 / 缩放变化时会**整文件回写**），也不能先
建 `SingletonApplication`（`QSharedMemory` 是另一个锁）。**单一写者**口径：

- `GHOST_UI=slint` → 写者是 Rust；
- `GHOST_UI=qt`（缺省）→ 写者是 Python。
"""

from __future__ import annotations

import os
import sys
from pathlib import Path

ENV_VAR = "GHOST_UI"
BINARY_ENV_VAR = "GD3_UI_BIN"
BINARY_NAME = "gd3-ui.exe" if sys.platform == "win32" else "gd3-ui"

DEFAULT_BACKEND = "qt"
SLINT_BACKEND = "slint"


def uiBackend() -> str:
    """读 `GHOST_UI`（大小写与空白都容错）。未知取值按缺省处理。"""
    value = os.environ.get(ENV_VAR, "").strip().lower()
    if value in ("", DEFAULT_BACKEND):
        return DEFAULT_BACKEND
    if value == SLINT_BACKEND:
        return SLINT_BACKEND
    print(
        f"gd3: {ENV_VAR}={value!r} 不是已知取值（qt / slint），按 {DEFAULT_BACKEND} 处理",
        file=sys.stderr,
    )
    return DEFAULT_BACKEND


def findRustUiBinary() -> Path | None:
    """`gd3-ui` 二进制的绝对路径；找不到返回 `None`（调用方回退 Qt）。"""
    override = os.environ.get(BINARY_ENV_VAR, "").strip()
    if override:
        path = Path(override)
        return path if path.is_file() else None

    candidates = [Path(sys.executable).parent / BINARY_NAME]
    # `app/platform/ui_backend.py` → parents[2] 是仓库根。
    repo_root = Path(__file__).resolve().parents[2]
    for profile in ("release", "debug"):
        candidates.append(repo_root / "rust" / "target" / profile / BINARY_NAME)

    for candidate in candidates:
        if candidate.is_file():
            return candidate
    return None


def dispatchRustUi(argv: list[str] | None = None) -> bool:
    """`GHOST_UI=slint` 时把进程换成 Rust 前端。

    返回 `True` 表示已经交给 Rust（正常路径下 `os.execv` 成功，**不会返回**）；
    返回 `False` 表示继续走 Qt（没选 slint，或者二进制找不到 / 起不来）。
    """
    if uiBackend() != SLINT_BACKEND:
        return False

    binary = findRustUiBinary()
    if binary is None:
        print(
            f"gd3: {ENV_VAR}={SLINT_BACKEND} 但没找到 {BINARY_NAME}"
            f"（可用 {BINARY_ENV_VAR} 指定路径）；回退到 Qt",
            file=sys.stderr,
        )
        return False

    args = list(sys.argv[1:] if argv is None else argv)
    try:
        # 用 execv 而不是 subprocess：窗口 / 信号 / 退出码都直接是 Rust 那边的，
        # Python 不留在进程树里。
        os.execv(str(binary), [str(binary), *args])
    except OSError as error:
        print(
            f"gd3: 启动 {binary} 失败：{error}；回退到 Qt",
            file=sys.stderr,
        )
        return False
    return False  # pragma: no cover - execv 成功时进程已经换掉了
