"""生成任务卡片派生层的对比金文件（`card.jsonl`，第 50 步）。

要钉住的是 `app/view/cards/task_cards.py` 里那张「按状态分发」的表：

- `TaskCard.infoFields`（`SPEED_FIELD` / `ETA_FIELD` / `SIZE_FIELD`）在
  `refresh()` 里的 `formats.get(status) or formats.get(None)` 分发；
- `_refreshForStatus`（状态行文案 / 告警色 / 进度条三态）；
- `MultiFileTaskCard._refreshForStatus` 的多文件计数覆盖；
- `_refreshButtons`（`states.get(status) or states.get(None)` + 图标回落）。

做法与第 48/49 步一致：**桩替换 widget，跑真方法**。具体：

- 用 `_FakeMixin + TaskCard` / `_FakeMixin + MultiFileTaskCard` 两个桩类，
  **不调用 `__init__`**（QWidget 初始化跳过），类属性 `infoFields` /
  `buttons` / `nameFormats` 仍是真的；
- 所有 widget 换成 `Stub`：只记录调用序列（`setText` / `show` / `hide` /
  `setVisible` / `setEnabled` / `setIcon` / `setValue` / `setError` /
  `pause`），事后回放得出「文本 + 可见性」；
- `_createProgressBar` **也是真方法**：把模块里的 `IndeterminateProgressBar`
  / `ProgressBar` 换成记录型桩类，于是 `fileSize in {UNKNOWN, NOT_SUPPORTED}`
  这个分支由真代码判定，脚本不抄；
- `isExisting` 用 monkeypatch 固定（文件系统事实由用例给）；
- `Task` / `TaskFile` / `TaskStep` / `TaskError` 全是真对象，于是
  `task.canPause`（遍历 steps 找 RUNNING）、`task.lastError`（倒序两轮）、
  `task.currentSnapshot()`（选中步骤求和平均）都由真代码算出，脚本只记录。

两条口径说明：

1. `completedAtText` 由脚本用与 `_refreshForStatus` 同一表达式
   （`datetime.fromtimestamp(...).strftime("%Y-%m-%d %H:%M:%S")`，**本地时区**）
   算出，并断言状态行确实是「完成于 {text}」——Rust 侧只拼模板，不碰时区。
2. 进度条可见性记录的是**动作**（`show` / `hide` / `keep`）而不是最终状态：
   Python 只在 RUNNING 里 `show()`、在 COMPLETED 里 `hide()`，FAILED 与
   PAUSED/WAITING 分支根本不碰可见性（沿用上一次的值）。无状态重渲染时
   Rust 侧按「非 COMPLETED 即可见」兜底。

    python3 rust/crates/gd3-ui/tests/fixtures/generate_card_fixture.py
"""

from __future__ import annotations

import json
import os
from datetime import datetime
from pathlib import Path

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

FIXTURE = Path(__file__).with_name("card.jsonl")

# 由 `main()` 填充的桩类（模块级变量，避免 `global` 声明散落）。
_STATE: dict[str, object] = {}


def _app():
    from PySide6.QtWidgets import QApplication

    return QApplication.instance() or QApplication([])


# --------------------------------------------------------------------------
# 记录型 widget 桩
# --------------------------------------------------------------------------


class Stub:
    """只记录调用序列的 widget 替身。"""

    def __init__(self) -> None:
        self.log: list[tuple[str, tuple]] = []

    def __getattr__(self, name: str):
        if name.startswith("_"):
            raise AttributeError(name)

        def record(*args, **_kwargs):
            self.log.append((name, args))

        return record


class BarStub(Stub):
    """`ProgressBar` / `IndeterminateProgressBar` 的替身（带形态标记）。"""

    def __init__(self, kind: str) -> None:
        super().__init__()
        self.kind = kind

    def setCustomBackgroundColor(self, *_args) -> None:  # 真代码会调它
        pass


def _last(stub: Stub, name: str):
    for call, args in reversed(stub.log):
        if call == name:
            return args
    return None


def _text(stub: Stub):
    args = _last(stub, "setText")
    return args[0] if args else None


def _visible(stub: Stub):
    """回放可见性动作；从未触碰过返回 None。"""
    for call, args in reversed(stub.log):
        if call == "show":
            return True
        if call == "hide":
            return False
        if call == "setVisible":
            return bool(args[0])
    return None


def _visibility(stub: Stub) -> str:
    """进度条**最后一次**可见性动作：`show` / `hide` / `keep`（没碰过）。"""
    for call, _args in reversed(stub.log):
        if call == "show":
            return "show"
        if call == "hide":
            return "hide"
        if call == "setVisible":
            return "show" if _args[0] else "hide"
    return "keep"


def _enabled(stub: Stub) -> bool:
    args = _last(stub, "setEnabled")
    return bool(args[0]) if args else True


def _icon(stub: Stub):
    args = _last(stub, "setIcon")
    if not args:
        return None
    icon = args[0]
    # `button.setIcon(state.icon if state.icon is not None else spec.icon)`
    # 永远是 FluentIcon 枚举；取 `.name` 与磁盘/金文件口径一致。
    return getattr(icon, "name", str(icon))


# --------------------------------------------------------------------------
# 卡片桩：不跑 QWidget 初始化，但方法全是真的
# --------------------------------------------------------------------------


class _FakeMixin:
    def __init__(self, task) -> None:
        self._task = task
        self._taskService = _FakeTaskService()
        self._lastStatus = None
        self._isFileMissing = False
        self.nameLabel = Stub()
        self.iconLabel = Stub()
        self.statusLabel = Stub()
        self.speedLabel = Stub()
        self.etaLabel = Stub()
        self.sizeLabel = Stub()
        self.toggleButton = Stub()
        self.selectFilesButton = Stub()
        self.checksumButton = Stub()
        self.openFileButton = Stub()
        self.openFolderButton = Stub()
        self.deleteButton = Stub()
        # 真方法决定形态（fileSize in {UNKNOWN, NOT_SUPPORTED}）。
        self.progressBar = type(self)._createProgressBar(self)

    def tr(self, text: str, *_args, **_kwargs) -> str:
        # 生成脚本不装 translator，与运行期「无译文回落源串」同口径。
        return text

    def _refreshIcon(self) -> None:
        # 图标 pixmap 不在派生层范围内（需要真实文件系统 + QFileIconProvider）。
        pass


class _FakeTaskService:
    def checksumProgress(self, _task):
        return None


# --------------------------------------------------------------------------
# 用例
# --------------------------------------------------------------------------

SERVER_ERROR = ("服务器返回了错误（{status}）", {"status": 500})
PORT_IN_USE = ("端口 {port} 被占用，请更换端口", {"port": 16800})
DISK_FULL = ("磁盘空间不足", {})
RANGE_REJECTED = ("服务器拒绝了范围请求，状态码：{status}", {"status": 416})
UNEXPECTED = ("服务器返回了异常状态码：{status}", {"status": 502})

COMPLETED_AT = 1767225845  # 2026-01-01 00:04:05 UTC 附近，具体值不重要


def _step(spec: dict, index: int):
    from app.models.task import TaskError, TaskStep, TaskStatus

    cls = TaskStep if spec.get("can_pause", True) else _STATE["NoPauseStep"]
    step = cls(
        stepIndex=index,
        fileIndex=spec.get("file_index"),
        status=spec.get("status", TaskStatus.WAITING),
        progress=spec.get("progress", 0.0),
        receivedBytes=spec.get("received", 0),
        speed=spec.get("speed", 0),
    )
    if spec.get("error") is not None:
        message, params = spec["error"]
        step.error = TaskError(message, **params)
    return step


def _build_task(spec: dict):
    from app.models.task import Task, TaskFile, TaskStatus

    files = spec.get("files")
    if files is not None:
        files = [
            TaskFile(index=index, relativePath=f"part{index}.bin", selected=selected)
            for index, selected in files
        ]
    steps = [_step(step, index) for index, step in enumerate(spec.get("steps", []))]
    task = Task(
        name=spec.get("name", "a.bin"),
        url="https://example.invalid/1",
        packId="pack",
        steps=steps,
        files=files,
    )
    # `Task.__post_init__` 会按 steps 推状态 / 补 completedAt / 补 fileSize，
    # 这里覆盖回用例要的值（本层只关心最终状态）。
    task.status = spec.get("status", TaskStatus.WAITING)
    task.fileSize = spec.get("file_size", 4096)
    task.completedAt = spec.get("completed_at", 0)
    task.isSeeding = spec.get("is_seeding", False)
    if spec.get("has_output_file") is not None:
        task.hasOutputFile = spec["has_output_file"]
    if spec.get("can_seed") is not None:
        task.canSeed = spec["can_seed"]
    return task


def _last_error(task):
    error = task.lastError
    if error is None:
        return None
    return {
        "message": error.message,
        "params": {key: str(value) for key, value in error.params.items()},
    }


def _run_case(spec: dict) -> dict:
    from app.models.task import TaskStatus

    import app.view.cards.task_cards as task_cards

    task = _build_task(spec)
    card_cls = _STATE["MultiCard" if spec.get("multi") else "Card"]
    card = card_cls(task)
    assert isinstance(card.progressBar, BarStub)

    output_exists = spec.get("output_exists", True)
    task_cards.isExisting = lambda _path, value=output_exists: value
    card_cls.refresh(card, force=True)

    # 信息行（顺序即 `infoFields`）。
    fields = []
    for field in card_cls.infoFields:
        label = getattr(card, f"{field.name}Label")
        visible = _visible(label)
        fields.append(
            {
                "name": field.name,
                "text": _text(label) if visible else None,
                "visible": bool(visible),
            }
        )

    # 状态行。
    status_visible = _visible(card.statusLabel)
    color_args = _last(card.statusLabel, "setTextColor")
    status_line = (
        {
            "text": _text(card.statusLabel),
            "warn": bool(color_args) and len(color_args) > 0,
        }
        if status_visible
        else None
    )

    # 完成时间：与 `_refreshForStatus` 同一表达式，并交叉验证状态行。
    completed_at_text = None
    if task.completedAt:
        completed_at_text = datetime.fromtimestamp(task.completedAt).strftime(
            "%Y-%m-%d %H:%M:%S"
        )
    if (
        task.status == TaskStatus.COMPLETED
        and not card._isFileMissing
        and completed_at_text is not None
        and len(task.files or []) <= 1
    ):
        assert status_line is not None
        assert status_line["text"] == f"完成于 {completed_at_text}", status_line
    # 进度条。
    bar = card.progressBar
    visibility = _visibility(bar)
    error_args = _last(bar, "setError")
    value_args = _last(bar, "setValue")
    progress_bar = {
        "value": int(value_args[0]) if value_args else 0,
        "indeterminate": bar.kind == "INDETERMINATE",
        "visibility": visibility,
        "error": bool(error_args[0]) if error_args else False,
        "paused": _last(bar, "pause") is not None,
    }

    # 四个带 states 的按钮（`_refreshButtons` 只碰这四个；openFolder/delete
    # 的 spec.states 是 None，直接 continue）。
    buttons = {}
    for name in ("toggle", "selectFiles", "checksum", "openFile"):
        button = getattr(card, f"{name}Button")
        buttons[name] = {
            "icon": _icon(button),
            "visible": bool(_visible(button)),
            "enabled": _enabled(button),
        }

    snapshot = task.currentSnapshot()
    return {
        "label": spec["label"],
        "status": task.status.name,
        "fileSize": task.fileSize,
        "completedAtText": completed_at_text,
        "canPause": task.canPause,
        "canSeed": task.canSeed,
        "isSeeding": task.isSeeding,
        "hasOutputFile": task.hasOutputFile,
        "outputExists": output_exists,
        "lastError": _last_error(task),
        "selectedFiles": sum(1 for f in (task.files or []) if f.selected),
        "totalFiles": len(task.files or []),
        "progress": snapshot[0],
        "speed": snapshot[1],
        "received": snapshot[2],
        "fields": fields,
        "statusLine": status_line,
        "fileMissing": card._isFileMissing,
        "progressBar": progress_bar,
        "buttons": buttons,
    }


def specs() -> list[dict]:
    from app.models.task import TaskStatus

    W, R, P, C, F = (
        TaskStatus.WAITING,
        TaskStatus.RUNNING,
        TaskStatus.PAUSED,
        TaskStatus.COMPLETED,
        TaskStatus.FAILED,
    )
    single = [(0, True)]
    return [
        # 1-2 基本状态
        {"label": "等待-单文件", "status": W, "files": single,
         "steps": [{"status": W}]},
        {"label": "运行中-单文件", "status": R, "files": single,
         "steps": [{"status": R, "progress": 25.5, "speed": 1024, "received": 512}]},
        # 3-8 运行中的特殊总量 / 速度 / 进度截断
        {"label": "运行中-总量未知", "status": R, "file_size": 0, "files": single,
         "steps": [{"status": R, "progress": 10.0, "speed": 1024, "received": 512}]},
        {"label": "运行中-总量不支持", "status": R, "file_size": -1, "files": single,
         "steps": [{"status": R, "progress": 10.0, "speed": 1024, "received": 512}]},
        {"label": "运行中-速度为零", "status": R, "files": single,
         "steps": [{"status": R, "progress": 0.0, "speed": 0, "received": 0}]},
        {"label": "运行中-不可暂停", "status": R, "files": single,
         "steps": [{"status": R, "progress": 5.0, "speed": 100, "received": 5,
                    "can_pause": False}]},
        {"label": "运行中-进度向下截断", "status": R, "files": single,
         "steps": [{"status": R, "progress": 42.9, "speed": 1024, "received": 512}]},
        {"label": "运行中-进度 99.99", "status": R, "files": single,
         "steps": [{"status": R, "progress": 99.99, "speed": 1024, "received": 4095}]},
        # 9-10 暂停
        {"label": "暂停", "status": P, "files": single,
         "steps": [{"status": P, "progress": 50.0, "received": 2048}]},
        {"label": "暂停-总量为零", "status": P, "file_size": 0, "files": single,
         "steps": [{"status": P, "progress": 50.0, "received": 512}]},
        # 11-16 失败：错误文案与 lastError 的两轮倒序
        {"label": "失败-无错误", "status": F, "files": single,
         "steps": [{"status": F, "progress": 37.5}]},
        {"label": "失败-服务器错误", "status": F, "files": single,
         "steps": [{"status": F, "progress": 12.0, "error": SERVER_ERROR}]},
        {"label": "失败-端口占用", "status": F, "files": single,
         "steps": [{"status": F, "error": PORT_IN_USE}]},
        {"label": "失败-磁盘不足", "status": F, "files": single,
         "steps": [{"status": F, "error": DISK_FULL}]},
        {"label": "失败-两步骤取最后一个失败", "status": F, "files": single,
         "steps": [{"status": F, "error": RANGE_REJECTED},
                   {"status": F, "error": UNEXPECTED}]},
        {"label": "失败-非失败步骤的错误也认", "status": F, "files": single,
         "steps": [{"status": P, "error": PORT_IN_USE}]},
        # 17-26 完成
        {"label": "完成-无完成时间", "status": C, "files": single,
         "steps": [{"status": C, "progress": 100.0, "received": 4096}]},
        {"label": "完成-有完成时间", "status": C, "files": single,
         "completed_at": COMPLETED_AT,
         "steps": [{"status": C, "progress": 100.0, "received": 4096}]},
        {"label": "完成-文件不存在", "status": C, "files": single,
         "completed_at": COMPLETED_AT, "output_exists": False,
         "steps": [{"status": C, "progress": 100.0, "received": 4096}]},
        {"label": "完成-无输出文件", "status": C, "files": single,
         "has_output_file": False, "output_exists": False,
         "steps": [{"status": C, "progress": 100.0, "received": 4096}]},
        {"label": "完成-做种中", "status": C, "files": single,
         "can_seed": True, "is_seeding": True,
         "steps": [{"status": C, "progress": 100.0, "received": 4096}]},
        {"label": "完成-可做种未做种", "status": C, "files": single,
         "can_seed": True, "is_seeding": False,
         "steps": [{"status": C, "progress": 100.0, "received": 4096}]},
        {"label": "完成-可做种但文件不存在", "status": C, "files": single,
         "can_seed": True, "is_seeding": False, "output_exists": False,
         "steps": [{"status": C, "progress": 100.0, "received": 4096}]},
        {"label": "完成-总量为零", "status": C, "file_size": 0, "files": single,
         "steps": [{"status": C, "progress": 100.0, "received": 512}]},
        {"label": "完成-总量不支持", "status": C, "file_size": -1, "files": single,
         "steps": [{"status": C, "progress": 100.0, "received": 512}]},
        {"label": "完成-进度 100", "status": C, "files": single,
         "steps": [{"status": C, "progress": 100.0, "received": 4096}]},
        # 27 等待时已收为零
        {"label": "等待-已收为零", "status": W, "file_size": 1048576, "files": single,
         "steps": [{"status": W, "progress": 0.0, "received": 0}]},
        # 28-35 多文件卡片
        {"label": "等待-多文件2选1", "status": W, "multi": True,
         "files": [(0, True), (1, False)], "steps": [{"status": W}]},
        {"label": "等待-多文件3选2", "status": W, "multi": True,
         "files": [(0, True), (1, True), (2, False)], "steps": [{"status": W}]},
        {"label": "运行中-多文件", "status": R, "multi": True,
         "files": [(0, True), (1, True), (2, True)],
         "steps": [{"status": R, "progress": 33.0, "speed": 300, "received": 990}]},
        {"label": "暂停-多文件", "status": P, "multi": True,
         "files": [(0, True), (1, True), (2, False)],
         "steps": [{"status": P, "progress": 40.0, "received": 1200}]},
        {"label": "失败-多文件", "status": F, "multi": True,
         "files": [(0, True), (1, True), (2, True)],
         "steps": [{"status": F, "error": DISK_FULL}]},
        {"label": "完成-多文件全选", "status": C, "multi": True,
         "files": [(0, True), (1, True), (2, True)], "completed_at": COMPLETED_AT,
         "steps": [{"status": C, "progress": 100.0, "received": 4096}]},
        {"label": "完成-多文件文件不存在", "status": C, "multi": True,
         "files": [(0, True), (1, True), (2, True)], "completed_at": COMPLETED_AT,
         "output_exists": False,
         "steps": [{"status": C, "progress": 100.0, "received": 4096}]},
        {"label": "完成-单元素文件列表", "status": C, "multi": True,
         "files": [(0, True)], "completed_at": COMPLETED_AT,
         "steps": [{"status": C, "progress": 100.0, "received": 4096}]},
        # 36-37 文件列表为空
        {"label": "等待-文件列表为空", "status": W, "files": [],
         "steps": [{"status": W}]},
        {"label": "等待-文件列表为 None", "status": W, "files": None,
         "steps": [{"status": W}]},
        # 38-44 字段对非相关状态的「无影响」
        {"label": "等待-可做种字段无影响", "status": W, "files": single,
         "can_seed": True, "is_seeding": True, "steps": [{"status": W}]},
        {"label": "暂停-不可暂停字段无影响", "status": P, "files": single,
         "steps": [{"status": P, "can_pause": False}]},
        {"label": "运行中-无输出文件", "status": R, "files": single,
         "has_output_file": False, "output_exists": False,
         "steps": [{"status": R, "progress": 8.0, "speed": 256, "received": 64}]},
        {"label": "失败-总量未知", "status": F, "file_size": 0, "files": single,
         "steps": [{"status": F, "error": DISK_FULL}]},
        {"label": "完成-无输出文件且可做种", "status": C, "files": single,
         "has_output_file": False, "can_seed": True, "is_seeding": True,
         "output_exists": False,
         "steps": [{"status": C, "progress": 100.0, "received": 4096}]},
        {"label": "等待-进度非零", "status": W, "files": single,
         "steps": [{"status": W, "progress": 12.5, "received": 512}]},
        {"label": "运行中-极速", "status": R, "file_size": 1048576, "files": single,
         "steps": [{"status": R, "progress": 0.0, "speed": 1048576, "received": 0}]},
    ]


def main() -> None:
    from dataclasses import dataclass

    from app.models.task import TaskStep

    _app()

    import app.view.cards.task_cards as task_cards

    task_cards.IndeterminateProgressBar = lambda parent=None: BarStub("INDETERMINATE")
    task_cards.ProgressBar = lambda parent=None: BarStub("PROGRESS")

    @dataclass
    class _NoPauseStep(TaskStep):
        # `canPause` 是 ClassVar（无注解即不是 dataclass 字段）。
        canPause = False

    class _FakeCard(_FakeMixin, task_cards.TaskCard):
        pass

    class _FakeMultiCard(_FakeMixin, task_cards.MultiFileTaskCard):
        pass

    _STATE["NoPauseStep"] = _NoPauseStep
    _STATE["Card"] = _FakeCard
    _STATE["MultiCard"] = _FakeMultiCard

    cases = [_run_case(spec) for spec in specs()]
    lines = [json.dumps(case, ensure_ascii=False) for case in cases]
    FIXTURE.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"写入 {FIXTURE}：{len(lines)} 条")


if __name__ == "__main__":
    main()
