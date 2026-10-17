"""生成任务抽屉派生层的对比金文件（`drawer.jsonl`，第 52 步）。

要钉住的是 `app/view/dialogs/task_drawer.py` 里五张卡片的 `refresh()`：

- `HeaderCard.refresh`（名称 / 进度条三处 / `_toStatusText`）；
- `ErrorBar.refresh`（FAILED 才看 `lastError`，`toLocalizedError` 后为空则隐藏）；
- `InfoCard.refresh`（六行信息 + 大小/分类/完成时间三处可见性）；
- `ChecksumCard.refresh` + `_refreshMatch`（行排序、比对命中、报错、tooltip）；
- `FileListCard.refresh` + `_toFileStatusText`。

做法与第 50 步一致：**把真方法绑到普通 Python 类上跑**，widget 全换记录型桩。

为什么不用真 widget：`QWidget.isVisible()` 在父窗口未 show 时恒为 False，
`setVisible(True)` 之后读回来仍是 False，判不出可见性；而记录调用序列可以。

两条口径说明：

1. `toTimeText` 是 `datetime.fromtimestamp(t).strftime("%Y-%m-%d %H:%M:%S")`
   （**本地时区**）。脚本把进程时区钉成 `TZ=UTC`，于是 offset=0 的用例直接跑
   真函数；非零偏移的用例把 `task_drawer.toTimeText` 换成
   `datetime.fromtimestamp(t, timezone(timedelta(seconds=off)))`——这就是
   「该偏移下的本地时间」，Rust 侧按 `to_time_text(t, off)` 算，两边可对。
   脚本会断言 offset=0 的替身与真函数逐条相同（证明这个等价建模成立）。
2. `ChecksumCard.refresh` 有 `task.checksums == self._shownChecksums` 的缓存
   短路（相等就不调 `_refreshMatch`）。桩不跑 `_bind`，故脚本在 `refresh()`
   之后**显式**再调一次 `_refreshMatch()`——等价于「用户输入了期望值」，
   两侧口径才可比。

    python3 rust/crates/gd3-ui/tests/fixtures/generate_drawer_fixture.py
"""

from __future__ import annotations

import json
import os
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path
from types import SimpleNamespace

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
os.environ["TZ"] = "UTC"
time.tzset()

FIXTURE = Path(__file__).with_name("drawer.jsonl")


def _app():
    from PySide6.QtWidgets import QApplication

    return QApplication.instance() or QApplication([])


# --------------------------------------------------------------------------
# 记录型 widget 桩
# --------------------------------------------------------------------------


class Stub:
    """记录调用序列的 widget 替身（属性链也是桩，便于 `bar.textLayout.x()`）。"""

    def __init__(self, log: list | None = None) -> None:
        object.__setattr__(self, "_log", log if log is not None else [])
        object.__setattr__(self, "_name", "")

    def __call__(self, *args, **_kwargs):
        self._log.append((self._name, args))
        return None

    def __getattr__(self, name: str):
        if name.startswith("_"):
            raise AttributeError(name)
        child = Stub(self._log)
        object.__setattr__(child, "_name", name)
        object.__setattr__(self, name, child)
        return child


class EditStub(Stub):
    """`expectedEdit`：`text()` 要返回真字符串，否则 `.strip()` 会炸。"""

    def __init__(self, text: str = "") -> None:
        super().__init__()
        object.__setattr__(self, "_text", text)

    def text(self) -> str:
        return self._text


def _logof(stub) -> list:
    """桩与「真方法绑普通类」两种底座共用：`Stub._log` / `FakeBase.log`。"""
    return stub.__dict__.get("_log") or stub.__dict__.get("log") or []


def _last(stub, name: str):
    for call, args in reversed(_logof(stub)):
        if call == name:
            return args
    return None


def _text(stub: Stub, name: str = "setText"):
    args = _last(stub, name)
    return args[0] if args else None


def _visible(stub: Stub) -> bool:
    args = _last(stub, "setVisible")
    return bool(args[0]) if args else True


class FakeBase:
    """真方法的底座：补 Qt 侧必要钩子（`setVisible` / `width` / `tr`）。"""

    def setVisible(self, value) -> None:  # noqa: N802（Qt 命名）
        self.log.append(("setVisible", (value,)))

    def width(self) -> int:
        return 400

    def tr(self, text: str, *_args, **_kwargs) -> str:
        # 生成脚本不装 translator，与运行期「无译文回落源串」同口径。
        return text


def _bind(cls, *names: str):
    """把**真方法**挂到普通 Python 类上（跳过 QWidget 构造）。"""
    namespace = {name: getattr(cls, name) for name in names}
    namespace["__init__"] = lambda self: setattr(self, "log", [])
    return type(f"Fake{cls.__name__}", (FakeBase,), namespace)


# --------------------------------------------------------------------------
# 任务构造
# --------------------------------------------------------------------------

SERVER_ERROR = ("服务器返回了错误（{status}）", {"status": 500})
PORT_IN_USE = ("端口 {port} 被占用，请更换端口", {"port": 16800})
DISK_FULL = ("磁盘空间不足", {})
RANGE_REJECTED = ("服务器拒绝了范围请求，状态码：{status}", {"status": 416})
UNEXPECTED = ("服务器返回了异常状态码：{status}", {"status": 502})

CREATED_AT = 1767225845  # 2026-01-01T00:04:05Z
COMPLETED_AT = 1767312245  # 2026-01-02T00:04:05Z


def _step(spec: dict, index: int):
    from app.models.task import TaskError, TaskStep, TaskStatus

    step = TaskStep(
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
    from app.models.task import Task, TaskFile

    files = spec.get("files")
    if files is not None:
        files = [
            TaskFile(
                index=item["index"],
                relativePath=item["relativePath"],
                size=item.get("size", 0),
                selected=item.get("selected", True),
                downloadedBytes=item.get("downloadedBytes", 0),
                completed=item.get("completed", False),
            )
            for item in files
        ]
    steps = [_step(step, index) for index, step in enumerate(spec.get("steps", []))]
    task = Task(
        name=spec.get("name", "a.bin"),
        url=spec.get("url", "https://example.invalid/1"),
        packId="pack",
        taskId=spec.get("taskId", "tsk_1"),
        steps=steps,
        files=files,
        createdAt=spec.get("createdAt", CREATED_AT),
        # 保存位置钉死：`cfg.downloadFolder` 是机器相关的（本机是 `//Downloads`），
        # 不钉死金文件就无法跨机器复现。
        outputFolder=Path(spec.get("outputFolder", "/downloads")),
        category=spec.get("category"),
        checksums=spec.get("checksums") or {},
    )
    # `Task` 的 dataclass 后置逻辑会按 steps 推状态 / 补 fileSize / 补
    # completedAt，这里覆盖回用例要的值（本层只关心最终状态）。
    task.status = spec["status"]
    task.fileSize = spec.get("fileSize", 4096)
    task.completedAt = spec.get("completedAt", 0)
    task.isSeeding = spec.get("isSeeding", False)
    if spec.get("hasOutputFile") is not None:
        task.hasOutputFile = spec["hasOutputFile"]
    return task


def _last_error(task):
    error = task.lastError
    if error is None:
        return None
    return {
        "message": error.message,
        "params": {key: str(value) for key, value in error.params.items()},
    }


def _record(task) -> dict:
    """喂给 Rust `TaskRecord` 的那部分（磁盘同名键，逐一对齐 tasks.jsonl）。"""
    files = None
    if task.files is not None:
        files = [
            {
                "index": file.index,
                "relativePath": file.relativePath,
                "size": file.size,
                "selected": file.selected,
                "downloadedBytes": file.downloadedBytes,
                "completed": file.completed,
            }
            for file in task.files
        ]
    return {
        "name": task.name,
        "url": task.url,
        "packId": task.packId,
        "taskId": task.taskId,
        "status": task.status.name,
        "fileSize": task.fileSize,
        "createdAt": task.createdAt,
        "completedAt": task.completedAt,
        "steps": [
            {
                "stepIndex": step.stepIndex,
                "fileIndex": step.fileIndex,
                "status": step.status.name,
                "progress": step.progress,
                "receivedBytes": step.receivedBytes,
                "speed": step.speed,
            }
            for step in task.steps
        ],
        "files": files,
        "checksums": dict(task.checksums),
        "category": task.category,
    }


# --------------------------------------------------------------------------
# 各卡片的用例
# --------------------------------------------------------------------------


def _run_header(spec: dict) -> dict:
    import app.view.dialogs.task_drawer as task_drawer

    task = _build_task(spec)
    card = _STATE["Header"]()

    # `_refresh` 由 `TaskDrawer` 定时器驱动；这里直接调卡片自己的 refresh。
    card._task = task
    card.nameText = Stub()
    card.iconLabel = Stub()
    card.statusLabel = Stub()
    card.progressBar = Stub()
    _STATE["Header"].refresh(card)

    value_args = _last(card.progressBar, "setValue")
    error_args = _last(card.progressBar, "setError")
    return {
        "label": spec["label"],
        "card": "header",
        "record": _record(task),
        "runtime": _runtime(spec, task),
        "want": {
            "name": _text(card.nameText),
            "progressValue": int(value_args[0]) if value_args else 0,
            "progressVisible": _visible(card.progressBar),
            "progressError": bool(error_args[0]) if error_args else False,
            "statusText": _text(card.statusLabel),
        },
    }


def _run_error_bar(spec: dict) -> dict:
    import app.view.dialogs.task_drawer as task_drawer

    task = _build_task(spec)
    card = _STATE["ErrorBar"]()
    card.log = []
    card._task = task
    card._shownText = ""
    card.bar = None
    card.vBoxLayout = Stub()
    bars: list[tuple] = []
    task_drawer.InfoBar = lambda *_args, **_kwargs: _capture_bar(bars, _args)
    # 复制按钮的父是 InfoBar（桩，不是 QWidget），真按钮 / ToolTipFilter 构造会炸
    # → 一起换桩（本层不关心按钮本身，只关心标题与正文）。
    task_drawer.TransparentToolButton = lambda *_args, **_kwargs: Stub()
    task_drawer.ToolTipFilter = lambda *_args, **_kwargs: Stub()
    _STATE["ErrorBar"].refresh(card)

    want = {"visible": _visible(card), "title": "", "text": ""}
    if bars:
        want["title"] = bars[-1][0]
        want["text"] = bars[-1][1]
    return {
        "label": spec["label"],
        "card": "errorBar",
        "record": _record(task),
        "runtime": _runtime(spec, task),
        "want": want,
    }


def _capture_bar(bars: list, args: tuple) -> Stub:
    """`InfoBar(icon, title, content, …)`：记下标题与正文，再返回桩。"""
    bars.append((args[1], args[2]))
    return Stub()


def _run_info(spec: dict) -> dict:
    import app.view.dialogs.task_drawer as task_drawer

    task = _build_task(spec)
    card = _STATE["Info"]()
    card.log = []
    card._task = task
    card._categoryService = SimpleNamespace(
        categoryById=lambda _cid: SimpleNamespace(name=spec["categoryName"])
    )
    names = ("urlRow", "pathRow", "sizeRow", "categoryRow", "createdRow", "completedRow")
    for name in names:
        setattr(card, name, Stub())
    _STATE["Info"].refresh(card)

    labels = {
        "urlRow": ("url", "下载链接"),
        "pathRow": ("path", "保存位置"),
        "sizeRow": ("size", "大小"),
        "categoryRow": ("category", "分类"),
        "createdRow": ("created", "创建时间"),
        "completedRow": ("completed", "完成时间"),
    }
    rows = []
    for name in names:
        key, caption = labels[name]
        stub = getattr(card, name)
        rows.append(
            {
                "name": key,
                "caption": caption,
                "text": _text(stub, "setValue") or "",
                "visible": _visible(stub),
            }
        )
    return {
        "label": spec["label"],
        "card": "info",
        "record": _record(task),
        "runtime": _runtime(spec, task),
        "want": {"rows": rows},
    }


def _run_checksum(spec: dict) -> dict:
    import app.view.dialogs.task_drawer as task_drawer

    task = _build_task(spec)
    card = _STATE["Checksum"]()
    card.log = []
    card._task = task
    card._shownChecksums = {}
    card.card = Stub()
    card.rowsLayout = Stub()
    card.expectedEdit = EditStub(spec.get("expected", ""))
    created: list[tuple[str, Stub]] = []
    task_drawer.InfoRow = lambda caption, *_args, **_kwargs: _capture_row(created, caption)
    _STATE["Checksum"].refresh(card)
    # 显式再调一次：桩没跑 `_bind`，等价于「用户输入了期望值」。
    _STATE["Checksum"]._refreshMatch(card)

    edit = card.expectedEdit
    error_args = _last(edit, "setError")
    tooltip = _last(edit, "setToolTip")
    matched = _STATE["matchChecksum"](task.checksums, edit.text())
    return {
        "label": spec["label"],
        "card": "checksum",
        "record": _record(task),
        "runtime": _runtime(spec, task),
        "want": {
            "visible": _visible(card),
            "rows": [
                {"algorithm": caption, "value": _text(stub, "setValue") or ""}
                for caption, stub in created
            ],
            "matched": {
                "matched": matched,
                "error": bool(error_args[0]) if error_args else False,
                "tooltip": tooltip[0] if tooltip else "",
            },
        },
    }


def _capture_row(created: list, caption: str) -> Stub:
    stub = Stub()
    created.append((caption, stub))
    return stub


def _run_file_list(spec: dict) -> dict:
    import app.view.dialogs.task_drawer as task_drawer

    task = _build_task(spec)
    card = _STATE["FileList"]()
    card.log = []
    card._task = task
    card._lastStatus = None
    card.table = Stub()
    _STATE["FileList"].refresh(card)

    rows_args = _last(card.table, "setRows")
    rows = [
        {"path": path, "size": size, "status": status}
        for path, size, status in (rows_args[0] if rows_args else [])
    ]
    last_status = spec.get("lastStatus")
    needs_refresh = not (
        last_status is not None
        and last_status == task.status.name
        and task.status != _TaskStatus().RUNNING
    )
    return {
        "label": spec["label"],
        "card": "fileList",
        "record": _record(task),
        "runtime": _runtime(spec, task),
        "want": {
            "visible": _visible(card),
            "rows": rows,
            "needsRefresh": needs_refresh,
        },
    }


def _TaskStatus():
    from app.models.task import TaskStatus

    return TaskStatus


def _runtime(spec: dict, task) -> dict:
    """记录里没有、由调用方补的抽屉输入。"""
    category_name = None
    if task.category:
        category_name = spec["categoryName"]
    return {
        # 与 Python 的 `task.outputPath` 同源（InfoCard 的「保存位置」行就取它）。
        "outputPath": task.outputPath,
        "utcOffsetSeconds": spec.get("utcOffsetSeconds", 0),
        "categoryEnabled": spec.get("categoryEnabled", False),
        "categoryName": category_name,
        "expectedChecksum": spec.get("expected", ""),
        "isSeeding": task.isSeeding,
        "hasOutputFile": task.hasOutputFile,
        "outputExists": spec.get("outputExists", True),
        "lastError": _last_error(task),
        "lastStatus": spec.get("lastStatus"),
    }


# --------------------------------------------------------------------------
# 用例表
# --------------------------------------------------------------------------


def specs() -> list[dict]:
    S = _TaskStatus()
    W, R, P, C, F = S.WAITING, S.RUNNING, S.PAUSED, S.COMPLETED, S.FAILED
    single = [{"index": 0, "relativePath": "a.bin", "size": 4096, "selected": True}]
    return [
        # --- HeaderCard ---------------------------------------------------
        {"card": "header", "label": "头部-等待", "status": W,
         "steps": [{"status": W}]},
        {"card": "header", "label": "头部-运行中", "status": R,
         "steps": [{"status": R, "progress": 25.5, "speed": 1024, "received": 512}]},
        {"card": "header", "label": "头部-运行中-总量未知", "status": R, "fileSize": 0,
         "steps": [{"status": R, "progress": 10.0, "speed": 1024, "received": 512}]},
        {"card": "header", "label": "头部-运行中-总量不支持", "status": R, "fileSize": -1,
         "steps": [{"status": R, "progress": 10.0, "speed": 1024, "received": 512}]},
        {"card": "header", "label": "头部-运行中-速度为零", "status": R,
         "steps": [{"status": R, "progress": 0.0, "speed": 0, "received": 0}]},
        {"card": "header", "label": "头部-运行中-进度截断", "status": R,
         "steps": [{"status": R, "progress": 42.9, "speed": 1024, "received": 512}]},
        {"card": "header", "label": "头部-运行中-99.99", "status": R,
         "steps": [{"status": R, "progress": 99.99, "speed": 1024, "received": 4095}]},
        {"card": "header", "label": "头部-暂停", "status": P,
         "steps": [{"status": P, "progress": 50.0, "received": 2048}]},
        {"card": "header", "label": "头部-失败", "status": F,
         "steps": [{"status": F, "progress": 12.0, "error": SERVER_ERROR}]},
        {"card": "header", "label": "头部-完成-无完成时间", "status": C,
         "steps": [{"status": C, "progress": 100.0, "received": 4096}]},
        {"card": "header", "label": "头部-完成-做种中", "status": C, "isSeeding": True,
         "completedAt": COMPLETED_AT,
         "steps": [{"status": C, "progress": 100.0, "received": 4096}]},
        {"card": "header", "label": "头部-完成-总量未知", "status": C, "fileSize": 0,
         "steps": [{"status": C, "progress": 100.0, "received": 512}]},
        {"card": "header", "label": "头部-多步骤快照", "status": R,
         "steps": [
             {"status": R, "progress": 30.0, "speed": 512, "received": 384},
             {"status": R, "progress": 70.0, "speed": 512, "received": 128},
         ]},
        # --- ErrorBar -----------------------------------------------------
        {"card": "errorBar", "label": "错误条-失败-无错误", "status": F,
         "steps": [{"status": F, "progress": 37.5}]},
        {"card": "errorBar", "label": "错误条-失败-服务器错误", "status": F,
         "steps": [{"status": F, "error": SERVER_ERROR}]},
        {"card": "errorBar", "label": "错误条-失败-端口占用", "status": F,
         "steps": [{"status": F, "error": PORT_IN_USE}]},
        {"card": "errorBar", "label": "错误条-失败-磁盘不足", "status": F,
         "steps": [{"status": F, "error": DISK_FULL}]},
        {"card": "errorBar", "label": "错误条-失败-取最后一个失败", "status": F,
         "steps": [{"status": F, "error": RANGE_REJECTED},
                   {"status": F, "error": UNEXPECTED}]},
        {"card": "errorBar", "label": "错误条-失败-非失败步骤的错误", "status": F,
         "steps": [{"status": P, "error": PORT_IN_USE}]},
        {"card": "errorBar", "label": "错误条-运行中带错误也隐藏", "status": R,
         "steps": [{"status": P, "error": PORT_IN_USE}]},
        {"card": "errorBar", "label": "错误条-完成带错误也隐藏", "status": C,
         "completedAt": COMPLETED_AT,
         "steps": [{"status": P, "error": PORT_IN_USE}]},
        # --- InfoCard -----------------------------------------------------
        {"card": "info", "label": "信息-完成-有分类", "status": C,
         "completedAt": COMPLETED_AT, "category": "cat_video", "categoryName": "视频",
         "categoryEnabled": True, "steps": [{"status": C, "progress": 100.0}]},
        {"card": "info", "label": "信息-等待-无分类", "status": W,
         "steps": [{"status": W}]},
        {"card": "info", "label": "信息-分类关闭", "status": C,
         "completedAt": COMPLETED_AT, "category": "cat_video", "categoryName": "视频",
         "categoryEnabled": False, "steps": [{"status": C, "progress": 100.0}]},
        {"card": "info", "label": "信息-分类开启但无分类", "status": W,
         "categoryEnabled": True, "steps": [{"status": W}]},
        {"card": "info", "label": "信息-总量为零", "status": W, "fileSize": 0,
         "steps": [{"status": W}]},
        {"card": "info", "label": "信息-总量不支持", "status": W, "fileSize": -1,
         "steps": [{"status": W}]},
        {"card": "info", "label": "信息-东八区时间", "status": C,
         "utcOffsetSeconds": 8 * 3600, "completedAt": COMPLETED_AT,
         "steps": [{"status": C, "progress": 100.0}]},
        {"card": "info", "label": "信息-西五区时间", "status": C,
         "utcOffsetSeconds": -5 * 3600, "completedAt": COMPLETED_AT,
         "steps": [{"status": C, "progress": 100.0}]},
        {"card": "info", "label": "信息-完成时间早于创建时间", "status": C,
         "completedAt": 946684800, "createdAt": COMPLETED_AT,
         "steps": [{"status": C, "progress": 100.0}]},
        # --- ChecksumCard -------------------------------------------------
        {"card": "checksum", "label": "校验-无校验值", "status": C, "checksums": {},
         "steps": [{"status": C, "progress": 100.0}]},
        {"card": "checksum", "label": "校验-单个md5", "status": C,
         "checksums": {"md5": "bb"}, "steps": [{"status": C, "progress": 100.0}]},
        {"card": "checksum", "label": "校验-两个按原始键排序", "status": C,
         "checksums": {"sha256": "aa", "md5": "bb"},
         "steps": [{"status": C, "progress": 100.0}]},
        {"card": "checksum", "label": "校验-大小写混排", "status": C,
         "checksums": {"sha1": "aa", "Sha256": "bb"},
         "steps": [{"status": C, "progress": 100.0}]},
        {"card": "checksum", "label": "校验-命中", "status": C,
         "checksums": {"md5": "bb", "sha256": "aa"}, "expected": " BB \n",
         "steps": [{"status": C, "progress": 100.0}]},
        {"card": "checksum", "label": "校验-命中带文件名", "status": C,
         "checksums": {"md5": "bb"}, "expected": "bb  a.bin",
         "steps": [{"status": C, "progress": 100.0}]},
        {"card": "checksum", "label": "校验-不命中", "status": C,
         "checksums": {"md5": "bb"}, "expected": "cc",
         "steps": [{"status": C, "progress": 100.0}]},
        {"card": "checksum", "label": "校验-纯空白期望值", "status": C,
         "checksums": {"md5": "bb"}, "expected": "  \t",
         "steps": [{"status": C, "progress": 100.0}]},
        {"card": "checksum", "label": "校验-多文件隐藏", "status": C,
         "checksums": {"md5": "bb"},
         "files": [
             {"index": 0, "relativePath": "a.bin", "size": 1},
             {"index": 1, "relativePath": "b.bin", "size": 1},
         ],
         "steps": [{"status": C, "progress": 100.0}]},
        {"card": "checksum", "label": "校验-单文件列表仍显示", "status": C,
         "checksums": {"md5": "bb"},
         "files": [{"index": 0, "relativePath": "a.bin", "size": 1}],
         "steps": [{"status": C, "progress": 100.0}]},
        # --- FileListCard -------------------------------------------------
        {"card": "fileList", "label": "文件-单文件隐藏", "status": W,
         "files": single, "steps": [{"status": W}]},
        {"card": "fileList", "label": "文件-无文件列表", "status": W,
         "steps": [{"status": W}]},
        {"card": "fileList", "label": "文件-三文件混合", "status": R,
         "files": [
             {"index": 0, "relativePath": "a.bin", "size": 200, "selected": True,
              "downloadedBytes": 100},
             {"index": 1, "relativePath": "b/c.bin", "size": 0, "selected": False},
             {"index": 2, "relativePath": "d.bin", "size": 300, "selected": True,
              "downloadedBytes": 300, "completed": True},
         ],
         "steps": [{"status": R, "progress": 50.0, "speed": 100, "received": 400}]},
        {"card": "fileList", "label": "文件-进度向下取整", "status": R,
         "files": [
             {"index": 0, "relativePath": "a.bin", "size": 3, "selected": True,
              "downloadedBytes": 2},
             {"index": 1, "relativePath": "b.bin", "size": 7, "selected": True,
              "downloadedBytes": 1},
         ],
         "steps": [{"status": R, "progress": 10.0}]},
        {"card": "fileList", "label": "文件-状态未变跳过重算", "status": C,
         "lastStatus": "COMPLETED",
         "files": [
             {"index": 0, "relativePath": "a.bin", "size": 1},
             {"index": 1, "relativePath": "b.bin", "size": 1},
         ],
         "steps": [{"status": C, "progress": 100.0}]},
        {"card": "fileList", "label": "文件-运行中不跳过重算", "status": R,
         "lastStatus": "RUNNING",
         "files": [
             {"index": 0, "relativePath": "a.bin", "size": 1},
             {"index": 1, "relativePath": "b.bin", "size": 1},
         ],
         "steps": [{"status": R, "progress": 10.0}]},
        {"card": "fileList", "label": "文件-状态变化要重算", "status": C,
         "lastStatus": "WAITING",
         "files": [
             {"index": 0, "relativePath": "a.bin", "size": 1},
             {"index": 1, "relativePath": "b.bin", "size": 1},
         ],
         "steps": [{"status": C, "progress": 100.0}]},
        {"card": "fileList", "label": "文件-首次刷新要重算", "status": C,
         "files": [
             {"index": 0, "relativePath": "a.bin", "size": 1},
             {"index": 1, "relativePath": "b.bin", "size": 1},
         ],
         "steps": [{"status": C, "progress": 100.0}]},
    ]


# --------------------------------------------------------------------------
# 主流程
# --------------------------------------------------------------------------


def _make_time_text(offset: int):
    def to_time_text(timestamp: int) -> str:
        moment = datetime.fromtimestamp(
            timestamp, timezone(timedelta(seconds=offset))
        )
        return moment.strftime("%Y-%m-%d %H:%M:%S")

    return to_time_text


def main() -> None:
    _app()

    import app.view.dialogs.task_drawer as task_drawer
    from app.checksum import matchChecksum

    # 校验「偏移建模」：offset=0 的替身必须与真 `toTimeText` 逐条相同。
    for sample in (0, CREATED_AT, COMPLETED_AT, -86400, 951782400):
        assert _make_time_text(0)(sample) == task_drawer.toTimeText(sample), sample

    _STATE["Header"] = _bind(task_drawer.HeaderCard, "refresh", "_toStatusText")
    _STATE["ErrorBar"] = _bind(task_drawer.ErrorBar, "refresh")
    _STATE["Info"] = _bind(task_drawer.InfoCard, "refresh")
    _STATE["Checksum"] = _bind(
        task_drawer.ChecksumCard, "refresh", "_refreshMatch"
    )
    _STATE["FileList"] = _bind(task_drawer.FileListCard, "refresh", "_toFileStatusText")
    _STATE["matchChecksum"] = matchChecksum

    runners = {
        "header": _run_header,
        "errorBar": _run_error_bar,
        "info": _run_info,
        "checksum": _run_checksum,
        "fileList": _run_file_list,
    }

    cases = []
    for spec in specs():
        # 每例独立设置时间口径与 cfg 开关（模块级名字，直接替换）。
        task_drawer.toTimeText = _make_time_text(spec.get("utcOffsetSeconds", 0))
        task_drawer.cfg = SimpleNamespace(
            isCategoryEnabled=SimpleNamespace(value=spec.get("categoryEnabled", False))
        )
        cases.append(runners[spec["card"]](spec))

    lines = [json.dumps(case, ensure_ascii=False) for case in cases]
    FIXTURE.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"写入 {FIXTURE}：{len(lines)} 条")


_STATE: dict[str, object] = {}


if __name__ == "__main__":
    main()
