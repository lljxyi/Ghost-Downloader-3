"""生成任务卡片**交互层**的对比金文件（`card_menu.jsonl`，第 58 步）。

第 50 步的 `card.jsonl` 钉住了 `refresh()` 那条「按状态分发」的表；本文件钉住
`task_cards.py` 里剩下那些**只在鼠标/菜单里发生**的判定：

- `createContextMenu()`：菜单项顺序、分隔符、`canEdit and status != COMPLETED`
  这个条件、「移动到分类」子菜单（`cfg.isCategoryEnabled.value` 开关 +
  `未分类` + 每个分类），以及**每项触发后的真实动作**（把回调真跑一遍，记下
  `taskService` / 剪贴板 / `openFile` / `TaskDrawer` 的调用）；
- `_onChecksumClicked()`：三条短路（校验进行中 → 取消；目录或文件不存在 →
  状态行「文件不存在，无法校验」；否则弹菜单），以及菜单里
  `ALGORITHMS` / `COMMON_ALGORITHMS` 的分配与 `checked`（`algorithm in
  task.checksums`）；
- `_canDrag()`：`COMPLETED and hasOutputFile and isExisting(outputPath)`；
- `mouseReleaseEvent()`：名字区 → 打开文件、信息区 → 抽屉、其余 → 选择
  （`checked = True if extend or not selectionMode else not isChecked`），
  以及 `button != Left` / `_hasDragged` 的提前返回。

做法与第 48/49/50 步一致：**桩替换外部依赖，跑真方法**。

- `TaskCard.__init__` 不调用（跳过整套 widget 搭建），改用
  `task_cards.CardWidget.__init__(self)` 建出真 QObject——于是
  `selectionChanged.emit(...)`、`super().mouseReleaseEvent(e)` 走的是真 Qt 路径；
  卡片用到的 widget（`checkBox` / `openFileButton` / `statusLabel` /
  `checksumButton`）换成记录型 `Stub`；
- `Action` / `RoundMenu` 换成记录型替身，**`triggered` 回调真跑**，动作语义由
  真 lambda 决定，脚本只记录服务收到的调用；
- `isExisting` / `isFolder` / `openFile` / `revealInFolder` / `cfg` /
  `QApplication.instance().clipboardListener` / `TaskDrawer` 全部 monkeypatch；
- `ALGORITHMS` 是 `sorted(hashlib.algorithms_available)`（平台相关），所以
  **算法表由金文件带出来**，Rust 侧当输入收（`checksum_menu(algorithms, …)`），
  不硬编码。

两处「实证踩坑」写在这里，免得后人再踩：

1. `qfluentwidgets.Action` 是 `singledispatchmethod`，按**第一个位置参数的类型**
   分派：`Action(text: str, parent)` 与 `Action(icon, text, parent)` 两种。
   `task_cards.py` 两种都用（右键菜单三参、校验菜单 `Action(algorithm.upper(),
   self)` 两参），按固定 `(icon, text, parent)` 写桩会把算法名当图标、把卡片本身
   当文本。
2. 一次回调可能**先后**打到多个服务（`_onEditClicked` = `featureService` 弹窗
   → `refresh()` → `taskService.checksumProgress`），所以调用记录是**一个按发生
   顺序排列的共享日志**（`_CALL_LOG`），不是每个服务各记一份。

    python3 rust/crates/gd3-ui/tests/fixtures/generate_card_menu_fixture.py
"""

from __future__ import annotations

import json
import os
import sys
import types
from pathlib import Path

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

HERE = Path(__file__).resolve()
REPO = HERE.parents[5]
if str(REPO) not in sys.path:
    sys.path.insert(0, str(REPO))

FIXTURE = HERE.with_name("card_menu.jsonl")
ICONS = HERE.with_name("fluent_icons.json")

# 所有替身的调用按**发生顺序**记进这一个列表（见模块 docstring 第 2 条）。
_CALL_LOG: list[dict] = []


def _app():
    from PySide6.QtWidgets import QApplication

    return QApplication.instance() or QApplication([])


def _plain(value):
    """把回调参数压成可 JSON 化的形状。"""
    if value is None or isinstance(value, (bool, int, float, str)):
        return value
    if isinstance(value, os.PathLike):
        return str(value)
    if isinstance(value, (list, tuple)):
        return [_plain(v) for v in value]
    for attr in ("taskId", "categoryId", "name"):
        if hasattr(value, attr):
            return getattr(value, attr)
    # 其余（如 `self.window()` 传进来的卡片本身）只留类名——
    # `repr` 带内存地址，金文件会不稳定。
    return f"<{type(value).__name__}>"


class Stub:
    """只记录调用序列的 widget 替身。"""

    def __init__(self, name: str = "") -> None:
        self.name = name
        self.log: list[tuple[str, tuple]] = []

    def __getattr__(self, name: str):
        if name.startswith("_"):
            raise AttributeError(name)

        def record(*args, **_kwargs):
            self.log.append((name, args))
            if name in {"height", "width"}:
                return 20
            if name == "mapToGlobal":
                from PySide6.QtCore import QPoint

                return QPoint(0, 0)
            return None

        return record


class Recorder:
    """`_taskService` / `_featureService` / 剪贴板替身：调用记进共享日志。"""

    def __init__(self, sink: str = "service", **fixed) -> None:
        self.sink = sink
        self._fixed = fixed

    def __getattr__(self, name: str):
        if name.startswith("_"):
            raise AttributeError(name)

        def record(*args, **_kwargs):
            _CALL_LOG.append(
                {"sink": self.sink, "call": name, "args": [_plain(a) for a in args]}
            )
            if name in self._fixed:
                value = self._fixed[name]
                return value(*args) if callable(value) else value
            return None

        return record


class _Action:
    """镜像 `qfluentwidgets.Action` 的两个 `singledispatchmethod` 重载。

    * `Action(text: str, parent=None)` —— 两参形式，第 1 位是文本；
    * `Action(icon: QIcon | FluentIconBase, text: str, parent=None)` —— 三参形式。

    `task_cards.py` 两种都用到了：右键菜单走三参（带图标），校验菜单
    （`Action(algorithm.upper(), self)`）走两参（**无图标**，`self` 是 parent）。
    按固定 `(icon, text, parent)` 形参写桩会把 `algorithm.upper()` 当图标、
    把卡片本身当文本，金文件里就会出现 `<_Card>` 这种脏值。
    """

    def __init__(self, *args) -> None:
        self.icon = None
        self.text = ""
        self.parent = None
        if args:
            first = args[0]
            if isinstance(first, str):
                self.text = first
                self.parent = args[1] if len(args) > 1 else None
            else:
                self.icon = first
                self.text = args[1] if len(args) > 1 else ""
                self.parent = args[2] if len(args) > 2 else None
        self.callbacks: list = []
        self.checked = None
        self.checkable = None

    @property
    def iconName(self):
        return getattr(self.icon, "name", None) if self.icon is not None else None

    def setCheckable(self, value: bool) -> None:
        self.checkable = value

    def setChecked(self, value: bool) -> None:
        self.checked = value

    @property
    def triggered(self):
        action = self

        class _Sig:
            def connect(self, fn):
                action.callbacks.append(fn)

        return _Sig()


class _Menu:
    def __init__(self, title: str | None = None, parent=None) -> None:
        self.title = title
        self.icon = None
        self.items: list = []

    def setIcon(self, icon) -> None:
        self.icon = icon

    def addAction(self, action) -> None:
        self.items.append(("action", action))

    def addSeparator(self) -> None:
        self.items.append(("separator", None))

    def addMenu(self, menu) -> None:
        self.items.append(("submenu", menu))

    def exec(self, *args, **kwargs) -> None:
        self.items.append(("exec", args))


def _invoke(action: _Action) -> dict:
    """真跑 `triggered` 回调，返回它造成的调用序列（按发生顺序）。"""
    _CALL_LOG.clear()
    for callback in action.callbacks:
        # `triggered` 传 checked，但 `_openDrawer` 这类槽不带参数。
        try:
            callback(False)
        except TypeError:
            callback()
    return {"calls": list(_CALL_LOG)}


def _flatten_menu(menu: _Menu, *, with_invoke: bool) -> list:
    out = []
    for kind, payload in menu.items:
        if kind == "separator":
            out.append({"type": "separator"})
        elif kind == "action":
            entry = {
                "type": "action",
                "text": payload.text,
                "icon": payload.iconName,
            }
            if payload.checked is not None:
                entry["checked"] = bool(payload.checked)
            if with_invoke:
                entry["invoke"] = _invoke(payload)
            out.append(entry)
        elif kind == "submenu":
            out.append(
                {
                    "type": "submenu",
                    "text": payload.title or "",
                    "icon": getattr(payload.icon, "name", None),
                    "items": _flatten_menu(payload, with_invoke=with_invoke),
                }
            )
        else:
            out.append({"type": "exec"})
    return out


def _phantom_task(Task, status, *, can_edit, has_output_file=True):
    """真属性、绕过 `__init__`（`Task.__init__` 会挂 steps/IO）。"""
    task = object.__new__(Task)
    task.name = "a.bin"
    task.url = "https://example/a.bin"
    task.taskId = "tsk_1"
    task.packId = "pack"
    task.status = status
    task.fileSize = 1024
    task.outputFolder = Path("/tmp")
    task.completedAt = 0
    task.files = []
    task.steps = []
    task.checksums = []
    task.category = ""
    task.canEdit = can_edit
    task.hasOutputFile = has_output_file
    task.isLive = False
    return task


def _fake_card(task_cards, task, *, services):
    class _Card(task_cards.TaskCard):
        def __init__(self) -> None:
            # `TaskCard` 的直接基类是 `CardWidget`（`QWidget` 不是直接基类，
            # 不能直接 `QWidget.__init__(self)`）。
            task_cards.CardWidget.__init__(self)
            self._task = task
            self._taskService = services["task"]
            self._featureService = services["feature"]
            self._categoryService = services["category"]
            self._isSelectionMode = False
            self._isFileMissing = False
            self._isDragPending = False
            self._hasDragged = False
            self._lastStatus = None
            # `self.window()` 在真 Qt 里返回父窗口；卡片没父窗口时是它自己。
            # 用字符串占位，免得金文件里出现 `<_Card>` 这种实现细节。
            self.window = lambda: "window"
            self.checkBox = Stub("checkBox")
            self.statusLabel = Stub("statusLabel")
            self.progressBar = Stub("progressBar")
            self.iconLabel = Stub("iconLabel")
            self.nameLabel = Stub("nameLabel")
            # 信息行与按钮全部按真类属性建桩（名字由 `FieldSpec` /
            # `ButtonSpec` 决定，脚本不抄）。
            for field_spec in self.infoFields:
                setattr(self, f"{field_spec.name}Label", Stub(field_spec.name))
            for button_spec in self.buttons:
                setattr(self, f"{button_spec.name}Button", Stub(button_spec.name))
            self._status_log: list[str] = []

        def _setStatus(self, text: str) -> None:  # 记录状态行，不碰 widget
            self._status_log.append(text)

    return _Card()


def _category_stub(categories):
    class _CategoryService:
        def categories(self):
            return categories

        def categoryById(self, category_id):
            for category in categories:
                if category.categoryId == category_id:
                    return category
            return None

    return _CategoryService()


def category_settings_category(category_id, name, icon):
    """用真 `Category` dataclass 造分类（字段名与磁盘一致）。"""
    from app.services.category_service import Category

    return Category(categoryId=category_id, name=name, icon=icon,
                    extensions=[], folder=None)


def main() -> None:
    _app()

    from app.models.task import Task, TaskStatus
    import app.view.cards.task_cards as task_cards
    from qfluentwidgets import FluentIcon

    # ---- 外部依赖替身 ----
    task_cards.Action = _Action
    task_cards.RoundMenu = _Menu
    task_cards.isExisting = lambda path: _STATE["exists"]
    task_cards.isFolder = lambda path: _STATE["folder"]
    task_cards.openFile = lambda path: _STATE["opened"].append(path)
    task_cards.revealInFolder = lambda path: None

    clipboard = Recorder(sink="clipboard")
    task_cards.QApplication = types.SimpleNamespace(
        instance=lambda: types.SimpleNamespace(clipboardListener=clipboard)
    )

    drawer_module = types.ModuleType("app.view.dialogs.task_drawer")

    class _Drawer:
        def __init__(self, task, *args):
            _CALL_LOG.append(
                {"sink": "drawer", "call": "TaskDrawer", "args": [task.taskId]}
            )

        def show(self):
            _CALL_LOG.append({"sink": "drawer", "call": "show", "args": []})

    drawer_module.TaskDrawer = _Drawer
    sys.modules["app.view.dialogs.task_drawer"] = drawer_module

    opened: list[str] = []
    _STATE = {"exists": True, "folder": False, "opened": opened}
    globals()["_STATE"] = _STATE

    def services_for(*, task=None, feature=None):
        return {
            "task": task or Recorder(sink="task"),
            "feature": feature or Recorder(sink="feature"),
            "category": _category_stub([]),
        }

    cases: list[dict] = []

    # ---- 1. 右键菜单 ----
    statuses = [TaskStatus.WAITING, TaskStatus.RUNNING, TaskStatus.COMPLETED, TaskStatus.FAILED]
    cats = [
        category_settings_category("cat_video", "视频", "VIDEO"),
        category_settings_category("cat_other", "杂项", "NOT_A_REAL_ICON"),
    ]
    for can_edit in (False, True):
        for status in statuses:
            for enabled in (False, True):
                task = _phantom_task(Task, status, can_edit=can_edit)
                services = services_for(
                    feature=Recorder(
                        sink="feature",
                        createEditDialog=lambda *a: types.SimpleNamespace(exec=lambda: True),
                    )
                )
                services["category"] = _category_stub(cats)
                task_cards.cfg = types.SimpleNamespace(
                    isCategoryEnabled=types.SimpleNamespace(value=enabled)
                )
                card = _fake_card(task_cards, task, services=services)
                menu = card.createContextMenu()
                cases.append(
                    {
                        "kind": "context-menu",
                        "canEdit": can_edit,
                        "status": status.name,
                        "categoryEnabled": enabled,
                        "categories": [
                            {"categoryId": c.categoryId, "name": c.name, "icon": c.icon}
                            for c in cats
                        ],
                        "menu": _flatten_menu(menu, with_invoke=True),
                    }
                )

    # ---- 2. 校验按钮点击 ----
    algorithms = list(task_cards.ALGORITHMS)
    for in_progress, folder, exists, checksums in [
        (True, False, True, []),
        (False, True, True, []),
        (False, False, False, []),
        (False, False, True, ["md5", "sha512"]),
        (False, False, True, ["blake2b"]),
    ]:
        task = _phantom_task(Task, TaskStatus.COMPLETED, can_edit=False)
        task.checksums = checksums
        _STATE["folder"] = folder
        _STATE["exists"] = exists
        services = services_for(
            task=Recorder(
                sink="task", checksumProgress=lambda *a: 42 if in_progress else None
            )
        )
        card = _fake_card(task_cards, task, services=services)
        # `_onChecksumClicked` 里的 menu 是局部变量，靠替换 `exec` 把它捞出来。
        holder: dict = {}
        original_exec = _Menu.exec

        def capture(self, *args, **kwargs):
            holder["menu"] = self

        _Menu.exec = capture
        try:
            _CALL_LOG.clear()
            card._onChecksumClicked()
            calls = list(_CALL_LOG)
        finally:
            _Menu.exec = original_exec
        if in_progress:
            outcome = {"kind": "cancel"}
        elif card._status_log:
            outcome = {"kind": "status", "text": card._status_log[-1]}
        elif "menu" in holder:
            outcome = {
                "kind": "menu",
                "menu": _flatten_menu(holder["menu"], with_invoke=True),
            }
        else:
            raise AssertionError("校验按钮既没取消、没改状态、也没弹菜单")
        cases.append(
            {
                "kind": "checksum-click",
                "inProgress": in_progress,
                "folder": folder,
                "exists": exists,
                "checksums": checksums,
                "algorithms": algorithms,
                "calls": calls,
                "outcome": outcome,
            }
        )
    _STATE["folder"] = False
    _STATE["exists"] = True

    # ---- 3. 拖拽资格 ----
    for status in statuses:
        for has_output in (True, False):
            for exists in (True, False):
                task = _phantom_task(Task, status, can_edit=False, has_output_file=has_output)
                _STATE["exists"] = exists
                card = _fake_card(task_cards, task, services=services_for())
                cases.append(
                    {
                        "kind": "can-drag",
                        "status": status.name,
                        "hasOutputFile": has_output,
                        "exists": exists,
                        "result": bool(card._canDrag()),
                    }
                )
    _STATE["exists"] = True

    # ---- 4. 松开鼠标的手势分派 ----
    from PySide6.QtCore import QEvent, QPointF, Qt
    from PySide6.QtGui import QMouseEvent

    def mouse_event(button, modifiers):
        return QMouseEvent(
            QEvent.Type.MouseButtonRelease,
            QPointF(5, 5),
            QPointF(5, 5),
            button,
            Qt.MouseButton.NoButton,
            modifiers,
        )

    for button in (Qt.MouseButton.LeftButton, Qt.MouseButton.RightButton):
        for has_dragged in (False, True):
            for selection_mode in (False, True):
                for checked in (False, True):
                    for match_name, match_info in ((True, False), (False, True), (False, False)):
                        for extend in (False, True):
                            task = _phantom_task(Task, TaskStatus.COMPLETED, can_edit=False)
                            card = _fake_card(task_cards, task, services=services_for())
                            card._isSelectionMode = selection_mode
                            card._hasDragged = has_dragged
                            card.checkBox.isChecked = lambda c=checked: c
                            card._matchNameText = lambda pos, v=match_name: v
                            card._matchInfoText = lambda pos, v=match_info: v
                            selections: list[tuple] = []
                            card.selectionChanged.connect(
                                lambda c, e: selections.append((bool(c), bool(e)))
                            )
                            opened.clear()
                            _CALL_LOG.clear()
                            card.mouseReleaseEvent(
                                mouse_event(
                                    button,
                                    Qt.KeyboardModifier.NoModifier
                                    if not extend
                                    else Qt.KeyboardModifier.ShiftModifier,
                                )
                            )
                            if opened:
                                outcome = {"gesture": "openFile", "path": opened[-1]}
                            elif _CALL_LOG:
                                outcome = {
                                    "gesture": "openDrawer",
                                    "calls": list(_CALL_LOG),
                                }
                            elif selections:
                                outcome = {
                                    "gesture": "select",
                                    "checked": selections[-1][0],
                                    "extend": selections[-1][1],
                                }
                            else:
                                outcome = {"gesture": "none"}
                            cases.append(
                                {
                                    "kind": "release",
                                    "button": "left"
                                    if button == Qt.MouseButton.LeftButton
                                    else "right",
                                    "hasDragged": has_dragged,
                                    "selectionMode": selection_mode,
                                    "checked": checked,
                                    "matchName": match_name,
                                    "matchInfo": match_info,
                                    "extend": extend,
                                    "outcome": outcome,
                                }
                            )

    lines = [json.dumps(case, ensure_ascii=False) for case in cases]
    FIXTURE.write_text("\n".join(lines) + "\n", encoding="utf-8")
    kinds: dict[str, int] = {}
    for case in cases:
        kinds[case["kind"]] = kinds.get(case["kind"], 0) + 1
    print(f"写入 {FIXTURE}：{len(lines)} 条 {kinds}")

    # FluentIcon 成员表：Rust 侧镜像 `getattr(FluentIcon, name, FluentIcon.TAG)`。
    names = sorted(member.name for member in FluentIcon)
    ICONS.write_text(json.dumps(names, ensure_ascii=False, indent=0) + "\n", encoding="utf-8")
    print(f"写入 {ICONS}：{len(names)} 个图标名")


if __name__ == "__main__":
    main()
