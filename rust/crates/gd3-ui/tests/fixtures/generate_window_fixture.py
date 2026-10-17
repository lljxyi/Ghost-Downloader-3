"""生成主窗口派生层的对比金文件（`window.jsonl`，第 56 步）。

要钉住的是 `app/view/windows/main_window.py` 里**不依赖 Qt 部件**的那几段：

- `_initLayout` / `_addPage` 建出来的导航表（routeKey / text / icon / position /
  selectable）；
- `_updateSearchTarget` 的 `hasattr(page, "searchPlaceholder")` 分派与 placeholder 文案；
- `_refreshSearchEditGeometry` 的三元几何（`//` 向下取整）；
- `dragEnterEvent` / `dropEvent` 的短路链；
- `_onCloseClicked` 的平台/模式分派；
- `onPairRequested` / `_onListenFailed` / `onUpdateAvailable` / `onExceptionCaught`
  的标题与按钮文案；
- `_onUpdateChanged` 的 `ProgressToast` 过滤。

做法与前面几步一致：**真方法绑到桩宿主上跑**，Qt 部件（`MessageBox`、
`InfoBar`、`ProgressToast`…）全部替换成记录型替身。这样脚本不抄任何算法，
Rust 侧改错就会在逐条比对时炸出来。

    python3 rust/crates/gd3-ui/tests/fixtures/generate_window_fixture.py
"""

from __future__ import annotations

import json
import os
import sys
import types
from pathlib import Path

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

FIXTURE = Path(__file__).with_name("window.jsonl")


def _app():
    from PySide6.QtWidgets import QApplication

    return QApplication.instance() or QApplication([])


# ---------------------------------------------------------------------------
# 记录型替身
# ---------------------------------------------------------------------------


class FakeNav:
    def __init__(self) -> None:
        self.items: list[dict] = []

    def addItem(self, **kwargs) -> None:
        self.items.append(kwargs)


class FakeSearchEdit:
    def __init__(self) -> None:
        self.log: list[str] = []

    def clear(self) -> None:
        self.log.append("clear")

    def setPlaceholderText(self, text: str) -> None:
        self.log.append(f"placeholder:{text}")

    def show(self) -> None:
        self.log.append("show")

    def hide(self) -> None:
        self.log.append("hide")


class FakeTitleBar:
    def __init__(self, width: int, height: int) -> None:
        self._width = width
        self._height = height

    def width(self) -> int:
        return self._width

    def height(self) -> int:
        return self._height


class FakeGeometryEdit:
    def __init__(self, height: int) -> None:
        self._height = height
        self.log: list[list] = []

    def setFixedWidth(self, width: int) -> None:
        self.log.append(["width", width])

    def height(self) -> int:
        return self._height

    def move(self, x: int, y: int) -> None:
        self.log.append(["move", x, y])


class FakeMime:
    def __init__(self, has_urls: bool, has_text: bool, text: str, urls=()) -> None:
        self._hasUrls = has_urls
        self._hasText = has_text
        self._text = text
        self._urls = list(urls)

    def hasUrls(self) -> bool:
        return self._hasUrls

    def hasText(self) -> bool:
        return self._hasText

    def text(self) -> str:
        return self._text

    def urls(self):
        return self._urls


class FakeDropEvent:
    def __init__(self, mime: FakeMime) -> None:
        self._mime = mime
        self.actions: list[str] = []

    def mimeData(self) -> FakeMime:
        return self._mime

    def ignore(self) -> None:
        self.actions.append("ignore")

    def acceptProposedAction(self) -> None:
        self.actions.append("accept")


class FakeOverlay:
    def __init__(self) -> None:
        self.log: list[str] = []

    def setGeometry(self, _rect) -> None:
        self.log.append("geometry")

    def show(self) -> None:
        self.log.append("show")

    def raise_(self) -> None:
        self.log.append("raise")

    def hide(self) -> None:
        self.log.append("hide")


class FakeDialog:
    def __init__(self, log: list[str], visible: bool) -> None:
        self._log = log
        self._visible = visible

    def isVisible(self) -> bool:
        return self._visible

    def reject(self) -> None:
        self._log.append("reject")


class FakeButton:
    def __init__(self, log: list[str] | None = None) -> None:
        self.text = ""
        self._log = log

    def setText(self, text: str) -> None:
        self.text = text


class FakeLayout:
    def __init__(self) -> None:
        self.ops: list[str] = []

    def removeWidget(self, _widget) -> None:
        self.ops.append("remove")

    def insertLayout(self, _index, _layout) -> None:
        self.ops.append("insert")

    def addWidget(self, _widget, _stretch=None, _alignment=None) -> None:
        self.ops.append("addWidget")


class FakeMsgBox:
    """替换 `MessageBox`：记录标题/正文/按钮文案。"""

    exec_result = False
    last: FakeMsgBox | None = None

    def __init__(self, title: str, content: str, parent=None) -> None:
        self.title = title
        self.content = content
        self.yesButton = FakeButton()
        self.cancelButton = FakeButton()
        self.contentLabel = types.SimpleNamespace(setTextInteractionFlags=lambda _f: None)
        self.textLayout = FakeLayout()
        self.titleLabel = object()
        FakeMsgBox.last = self

    def exec(self) -> bool:
        return FakeMsgBox.exec_result


class FakeCheckBox:
    checked = False
    last: FakeCheckBox | None = None

    def __init__(self, text: str, parent=None) -> None:
        self.text = text
        FakeCheckBox.last = self

    def isChecked(self) -> bool:
        return FakeCheckBox.checked


class FakeInfoBar:
    last: FakeInfoBar | None = None

    def __init__(self, **kwargs) -> None:
        self.kwargs = kwargs
        self.widgets: list = []
        self.shown = 0
        FakeInfoBar.last = self

    def addWidget(self, widget) -> None:
        self.widgets.append(widget)

    def show(self) -> None:
        self.shown += 1

    def close(self) -> None:
        pass


class FakePushButton:
    def __init__(self, icon=None, text: str = "") -> None:
        self.icon = icon
        self.text = text
        self.clicked = types.SimpleNamespace(connect=lambda _fn: None)


class FakeToolButton:
    def __init__(self, icon=None, parent=None) -> None:
        self.icon = icon
        self.tooltip = ""
        self.clicked = types.SimpleNamespace(connect=lambda _fn: None)
        self.filters: list = []

    def setToolTip(self, text: str) -> None:
        self.tooltip = text

    def installEventFilter(self, obj) -> None:
        self.filters.append(obj)


class FakeQApplication:
    is_file_drag_active = False
    quit_calls = 0

    @classmethod
    def instance(cls):
        return types.SimpleNamespace(isFileDragActive=cls.is_file_drag_active)

    @classmethod
    def quit(cls) -> None:
        cls.quit_calls += 1


class FakeTimer:
    calls: list[int] = []

    @staticmethod
    def singleShot(delay, _fn) -> None:
        FakeTimer.calls.append(delay)


class FakeCfg:
    def __init__(self, mode) -> None:
        self.closeMode = types.SimpleNamespace(value=mode)
        self.geometry = "geometry"
        self.sets: list[tuple] = []

    def set(self, key, value) -> None:
        name = "closeMode" if key is self.closeMode else "geometry"
        self.sets.append((name, getattr(value, "value", value)))


class FakeUpdateService:
    def __init__(self) -> None:
        self.downloads: list[str] = []

    def download(self, target: str) -> None:
        self.downloads.append(target)


class FakeProgressToast:
    created = 0
    last: FakeProgressToast | None = None

    def __init__(self, onRetry=None, parent=None) -> None:
        self.infos: list = []
        FakeProgressToast.created += 1
        FakeProgressToast.last = self

    def setInfo(self, info) -> None:
        self.infos.append(info)

    def deleteLater(self) -> None:
        pass


# ---------------------------------------------------------------------------
# 导航表
# ---------------------------------------------------------------------------


class NavHost:
    def __init__(self) -> None:
        self.navigationInterface = FakeNav()
        self.shown: list[str] = []

    def tr(self, text: str) -> str:
        return text

    def addUrls(self, _urls) -> None:
        pass

    def _showPage(self, pageClass) -> None:
        self.shown.append(pageClass.__name__)


NavHost._addPage = None  # 占位，main() 里绑真方法


def _nav_case(MainWindow) -> dict:
    from qfluentwidgets import FluentIcon

    host = NavHost()
    host._addPage = types.MethodType(MainWindow._addPage, host)
    MainWindow._initLayout(host)

    items = []
    for kwargs in host.navigationInterface.items:
        icon = kwargs.get("icon")
        items.append(
            {
                "routeKey": kwargs["routeKey"],
                "text": kwargs["text"],
                "icon": getattr(icon, "name", None),
                "position": kwargs["position"].name,
                "selectable": kwargs.get("selectable", True),
                "hasOnClick": kwargs.get("onClick") is not None,
            }
        )
    assert FluentIcon.DOWNLOAD.name == "DOWNLOAD"
    return {"kind": "nav", "items": items, "shown": host.shown}


# ---------------------------------------------------------------------------
# 搜索框
# ---------------------------------------------------------------------------


class SearchHost:
    def __init__(self) -> None:
        self.searchEdit = FakeSearchEdit()

    def tr(self, text: str) -> str:
        return text


SearchHost._updateSearchTarget = None


def _probe_page(name: str, source, with_set_text: bool):
    namespace = {"tr": lambda self, text: text}
    namespace["searchPlaceholder"] = source.searchPlaceholder
    if with_set_text:
        namespace["setSearchText"] = source.setSearchText
    return name, type(name, (), namespace)()


def _search_cases(MainWindow) -> list[dict]:
    from app.view.pages.setting_page import SettingPage
    from app.view.pages.task_page import TaskPage

    pages = [
        _probe_page("TaskPage", TaskPage, True),
        _probe_page("SettingPage", SettingPage, True),
        _probe_page("PlaceholderOnly", TaskPage, False),
        ("addTaskButton", types.SimpleNamespace()),
    ]

    host = SearchHost()
    host._updateSearchTarget = types.MethodType(MainWindow._updateSearchTarget, host)
    cases = []
    for route_key, page in pages:
        host.searchEdit.log.clear()
        MainWindow._updateSearchTarget(host, page)
        has_placeholder = hasattr(page, "searchPlaceholder")
        cases.append(
            {
                "kind": "search",
                "routeKey": route_key,
                "hasPlaceholder": has_placeholder,
                "placeholder": getattr(page, "searchPlaceholder", None),
                "hasSetSearchText": hasattr(page, "setSearchText"),
                "log": list(host.searchEdit.log),
            }
        )
    return cases


# ---------------------------------------------------------------------------
# 搜索框几何
# ---------------------------------------------------------------------------


class GeometryHost:
    def __init__(self, title_bar, search_edit) -> None:
        self.titleBar = title_bar
        self.searchEdit = search_edit


GeometryHost._refreshSearchEditGeometry = None


def _geometry_cases(MainWindow) -> list[dict]:
    sizes = [
        (960, 48, 32),
        (2000, 48, 32),
        (400, 40, 33),
        (100, 40, 32),
        (500, 41, 32),
        (800, 21, 32),
        (801, 48, 32),
        (0, 0, 32),
        (660, 48, 32),
    ]
    cases = []
    for width, height, edit_height in sizes:
        host = GeometryHost(FakeTitleBar(width, height), FakeGeometryEdit(edit_height))
        MainWindow._refreshSearchEditGeometry(host)
        log = host.searchEdit.log
        fixed = next(op for op in log if op[0] == "width")
        moved = next(op for op in log if op[0] == "move")
        cases.append(
            {
                "kind": "geometry",
                "titleBarWidth": width,
                "titleBarHeight": height,
                "editHeight": edit_height,
                "width": fixed[1],
                "x": moved[1],
                "y": moved[2],
                "log": log,
            }
        )
    return cases


# ---------------------------------------------------------------------------
# 拖放
# ---------------------------------------------------------------------------


class DragHost:
    def __init__(self, dialogs, overlay) -> None:
        self._dialogs = dialogs
        self._dropOverlay = overlay
        self.added: list[list[str]] = []

    def _matches(self, line: str) -> bool:
        return line.startswith("http")

    def _match(self, line: str) -> bool:
        return self._matches(line)

    def findChildren(self, _cls):
        return self._dialogs

    def rect(self):
        return "RECT"

    def addUrls(self, urls) -> None:
        self.added.append(list(urls))


DragHost.dragEnterEvent = None
DragHost.dropEvent = None

TEXTS = [
    "https://a.com/x\n  https://b.com/y  \n\n",
    "hello\nhttps://c.com/z",
    "hello",
    "",
    "  \n\t\n",
    "https://a.com/1\r\nhttps://a.com/2",
]


def _drag_cases(MainWindow) -> list[dict]:
    from PySide6.QtCore import QUrl

    cases = []
    for is_file_drag in (False, True):
        for has_urls in (False, True):
            for text in TEXTS:
                for mask in (False, True):
                    FakeQApplication.is_file_drag_active = is_file_drag
                    main_window.QApplication = FakeQApplication
                    urls = [QUrl("https://u.com/1"), QUrl("https://u.com/2")]
                    mime = FakeMime(has_urls, True, text, urls)
                    overlay = FakeOverlay()
                    dialogs = [FakeDialog([], mask)]
                    host = DragHost(dialogs, overlay)
                    host._featureService = types.SimpleNamespace(match=host._match)

                    enter = FakeDropEvent(mime)
                    MainWindow.dragEnterEvent(host, enter)

                    drop = FakeDropEvent(mime)
                    MainWindow.dropEvent(host, drop)

                    cases.append(
                        {
                            "kind": "drop",
                            "isFileDragActive": is_file_drag,
                            "hasUrls": has_urls,
                            "hasText": True,
                            "text": text,
                            "maskDialogVisible": mask,
                            "urls": [u.toString() for u in urls],
                            "enterActions": enter.actions,
                            "overlayLog": overlay.log,
                            "dropActions": drop.actions,
                            "added": host.added,
                        }
                    )
    return cases


# ---------------------------------------------------------------------------
# 关闭
# ---------------------------------------------------------------------------


class CloseHost:
    def __init__(self, dialogs, full_screen: bool, maximized: bool) -> None:
        self._dialogs = dialogs
        self._full = full_screen
        self._max = maximized
        self.log: list[str] = []

    def tr(self, text: str) -> str:
        return text

    def findChildren(self, _cls):
        return self._dialogs

    def isFullScreen(self) -> bool:
        return self._full

    def isMaximized(self) -> bool:
        return self._max

    def geometry(self):
        return types.SimpleNamespace(
            x=lambda: 1, y=lambda: 2, width=lambda: 3, height=lambda: 4
        )

    def showNormal(self) -> None:
        self.log.append("showNormal")

    def hide(self) -> None:
        self.log.append("hide")

    def close(self) -> None:
        self.log.append("close")


CloseHost._onCloseClicked = None


def _close_case(MainWindow, platform, mode, dialog_visible, full_screen, exec_result,
                remember) -> dict:
    from app.config.cfg import CloseMode

    reject_log: list[str] = []
    dialogs = [FakeDialog(reject_log, dialog_visible)]
    host = CloseHost(dialogs, full_screen, False)
    fake_cfg = FakeCfg(mode)

    FakeMsgBox.exec_result = exec_result
    FakeMsgBox.last = None
    FakeCheckBox.checked = remember
    FakeCheckBox.last = None
    FakeQApplication.quit_calls = 0
    FakeTimer.calls.clear()

    original = (sys.platform, main_window.cfg, main_window.MessageBox,
                main_window.CheckBox, main_window.QApplication, main_window.QTimer)
    sys.platform = platform
    main_window.cfg = fake_cfg
    main_window.MessageBox = FakeMsgBox
    main_window.CheckBox = FakeCheckBox
    main_window.QApplication = FakeQApplication
    main_window.QTimer = FakeTimer
    try:
        MainWindow._onCloseClicked(host)
    finally:
        (sys.platform, main_window.cfg, main_window.MessageBox,
         main_window.CheckBox, main_window.QApplication, main_window.QTimer) = original

    box = FakeMsgBox.last
    check = FakeCheckBox.last
    return {
        "kind": "close",
        "platform": platform,
        "mode": mode.value,
        "dialogVisible": dialog_visible,
        "fullScreen": full_screen,
        "exec": exec_result,
        "remember": remember,
        "actions": host.log,
        "rejects": reject_log,
        "quits": FakeQApplication.quit_calls,
        "timerDelays": list(FakeTimer.calls),
        "remembered": list(fake_cfg.sets),
        "message": None
        if box is None
        else {
            "title": box.title,
            "content": box.content,
            "yes": box.yesButton.text,
            "no": box.cancelButton.text,
            "checkbox": None if check is None else check.text,
        },
    }


def _close_cases(MainWindow) -> list[dict]:
    from app.config.cfg import CloseMode

    cases = []
    for platform in ("win32", "darwin", "linux"):
        for mode in (CloseMode.ASK, CloseMode.BACKGROUND, CloseMode.QUIT):
            for dialog_visible in (False, True):
                cases.append(
                    _close_case(MainWindow, platform, mode, dialog_visible, False, False, False)
                )
    # macOS 全屏分支
    for mode in (CloseMode.ASK, CloseMode.QUIT):
        cases.append(
            _close_case(MainWindow, "darwin", mode, False, True, False, False)
        )
    # ASK 的四种返回组合
    for exec_result in (True, False):
        for remember in (True, False):
            cases.append(
                _close_case(MainWindow, "win32", CloseMode.ASK, False, False,
                            exec_result, remember)
            )
    # macOS 非 QUIT 模式的「收进托盘」分支
    for mode in (CloseMode.ASK, CloseMode.BACKGROUND):
        cases.append(
            _close_case(MainWindow, "darwin", mode, False, False, False, False)
        )
    return cases


# ---------------------------------------------------------------------------
# 通知文案
# ---------------------------------------------------------------------------


class NotifyHost:
    def __init__(self) -> None:
        self.log: list[str] = []
        self._progressToast = None
        self._updateService = FakeUpdateService()
        self._browserService = types.SimpleNamespace(
            approvePair=lambda rid: self.log.append(f"approve:{rid}"),
            rejectPair=lambda rid: self.log.append(f"reject:{rid}"),
        )

    def tr(self, text: str) -> str:
        return text

    def _openLogFolder(self) -> None:
        self.log.append("openLogFolder")

    def _showReleaseDetails(self) -> None:
        self.log.append("releaseDetails")

    def _downloadBestAsset(self) -> None:
        self.log.append("bestAsset")

    def _showPage(self, pageClass) -> None:
        self.log.append(f"page:{pageClass.__name__}")


# `onBrowserListenFailed` / `onAria2ListenFailed` 都转发到 `self._onListenFailed`，
# 所以宿主要在 `_listen_failed_cases` 里绑上真方法。


def _pair_cases(MainWindow) -> list[dict]:
    cases = []
    combos = [
        (None, None),
        ("", ""),
        ("chrome", "1.2.3"),
        ("firefox", None),
        (None, "2.0"),
    ]
    for client_kind, version in combos:
        for exec_result in (True, False):
            host = NotifyHost()
            request = types.SimpleNamespace(
                requestId="req-1",
                peerAddress="127.0.0.1:5000",
                clientKind=client_kind,
                extensionVersion=version,
            )
            FakeMsgBox.exec_result = exec_result
            FakeMsgBox.last = None
            original = main_window.MessageBox
            main_window.MessageBox = FakeMsgBox
            try:
                MainWindow.onPairRequested(host, request)
            finally:
                main_window.MessageBox = original
            box = FakeMsgBox.last
            cases.append(
                {
                    "kind": "pair",
                    "peerAddress": request.peerAddress,
                    "clientKind": client_kind,
                    "extensionVersion": version,
                    "exec": exec_result,
                    "title": box.title,
                    "content": box.content,
                    "allow": box.yesButton.text,
                    "deny": box.cancelButton.text,
                    "log": list(host.log),
                }
            )
    return cases


def _listen_failed_cases(MainWindow) -> list[dict]:
    cases = []
    for method, key in (("onBrowserListenFailed", "browser"), ("onAria2ListenFailed", "aria2")):
        host = NotifyHost()
        host._onListenFailed = types.MethodType(MainWindow._onListenFailed, host)
        FakeInfoBar.last = None
        original = (main_window.InfoBar, main_window.PushButton, main_window.toLocalizedError)
        main_window.InfoBar = FakeInfoBar
        main_window.PushButton = FakePushButton
        main_window.toLocalizedError = lambda error: f"ERR:{error}"
        try:
            getattr(MainWindow, method)(host, "boom")
        finally:
            main_window.InfoBar, main_window.PushButton, main_window.toLocalizedError = original
        bar = FakeInfoBar.last
        cases.append(
            {
                "kind": "listen-failed",
                "target": key,
                "title": bar.kwargs.get("title"),
                "content": bar.kwargs.get("content"),
                "buttonTexts": [w.text for w in bar.widgets],
                "shown": bar.shown,
            }
        )
    return cases


def _update_available_cases(MainWindow) -> list[dict]:
    cases = []
    for platform in ("linux", "win32", "darwin"):
        host = NotifyHost()
        host._progressToast = FakeProgressToast()
        FakeInfoBar.last = None
        FakeProgressToast.created = 0
        original = (sys.platform, main_window.InfoBar, main_window.PrimaryPushButton,
                    main_window.PushButton)
        sys.platform = platform
        main_window.InfoBar = FakeInfoBar
        main_window.PrimaryPushButton = FakePushButton
        main_window.PushButton = FakePushButton
        try:
            MainWindow.onUpdateAvailable(host, types.SimpleNamespace(latestVersion="3.2.0"))
        finally:
            (sys.platform, main_window.InfoBar, main_window.PrimaryPushButton,
             main_window.PushButton) = original
        bar = FakeInfoBar.last
        cases.append(
            {
                "kind": "update-available",
                "platform": platform,
                "latestVersion": "3.2.0",
                "title": bar.kwargs.get("title"),
                "content": bar.kwargs.get("content"),
                "buttonTexts": [w.text for w in bar.widgets],
                "toastCleared": host._progressToast is None,
                "shown": bar.shown,
            }
        )
    return cases


def _exception_cases(MainWindow) -> list[dict]:
    cases = []
    for message in ("boom", "多行\n错误"):
        for exec_result in (False, True):
            host = NotifyHost()
            FakeMsgBox.exec_result = exec_result
            FakeMsgBox.last = None
            clipboard: list[str] = []
            opened: list[str] = []
            original = (main_window.MessageBox, main_window.TransparentToolButton,
                        main_window.ToolTipFilter, main_window.QApplication,
                        main_window.QDesktopServices, main_window.QHBoxLayout)
            main_window.MessageBox = FakeMsgBox
            main_window.TransparentToolButton = FakeToolButton
            main_window.ToolTipFilter = lambda widget: ("filter", widget)
            main_window.QHBoxLayout = FakeLayout
            main_window.QApplication = types.SimpleNamespace(
                clipboard=lambda: types.SimpleNamespace(setText=clipboard.append)
            )
            main_window.QDesktopServices = types.SimpleNamespace(
                openUrl=lambda url: opened.append(url.toString())
            )
            try:
                MainWindow.onExceptionCaught(host, message)
            finally:
                (main_window.MessageBox, main_window.TransparentToolButton,
                 main_window.ToolTipFilter, main_window.QApplication,
                 main_window.QDesktopServices, main_window.QHBoxLayout) = original
            box = FakeMsgBox.last
            button = box.textLayout.ops
            cases.append(
                {
                    "kind": "exception",
                    "message": message,
                    "exec": exec_result,
                    "title": box.title,
                    "content": box.content,
                    "logTooltip": None,
                    "layoutOps": list(button),
                    "clipboard": clipboard,
                    "opened": opened,
                }
            )
    return cases


def _exception_tooltip(MainWindow) -> str:
    host = NotifyHost()
    captured: list[str] = []

    class RecordingButton(FakeToolButton):
        def setToolTip(self, text: str) -> None:
            captured.append(text)
            super().setToolTip(text)

    FakeMsgBox.exec_result = False
    original = (main_window.MessageBox, main_window.TransparentToolButton,
                main_window.ToolTipFilter, main_window.QHBoxLayout)
    main_window.MessageBox = FakeMsgBox
    main_window.TransparentToolButton = RecordingButton
    main_window.ToolTipFilter = lambda widget: ("filter", widget)
    main_window.QHBoxLayout = FakeLayout
    try:
        MainWindow.onExceptionCaught(host, "boom")
    finally:
        (main_window.MessageBox, main_window.TransparentToolButton,
         main_window.ToolTipFilter, main_window.QHBoxLayout) = original
    assert len(captured) == 1, captured
    return captured[0]


def _update_changed_cases(MainWindow) -> list[dict]:
    from app.services.update_service import UpdateState

    states = [
        UpdateState.IDLE,
        UpdateState.CHECKING,
        UpdateState.AVAILABLE,
        UpdateState.DOWNLOADING,
        UpdateState.READY,
        UpdateState.FAILED,
    ]
    cases = []
    for target in ("app", "other"):
        for state in states:
            host = NotifyHost()
            FakeProgressToast.created = 0
            FakeProgressToast.last = None
            original = main_window.ProgressToast
            main_window.ProgressToast = FakeProgressToast
            try:
                MainWindow._onUpdateChanged(
                    host, types.SimpleNamespace(targetId=target, state=state)
                )
            finally:
                main_window.ProgressToast = original
            toast = FakeProgressToast.last
            cases.append(
                {
                    "kind": "update-changed",
                    "targetId": target,
                    "state": state.name,
                    "created": FakeProgressToast.created,
                    "updated": 0 if toast is None else len(toast.infos),
                }
            )
    return cases


# ---------------------------------------------------------------------------
# 入口
# ---------------------------------------------------------------------------


def main() -> None:
    global main_window

    _app()
    from app.view.windows import main_window

    MainWindow = main_window.MainWindow

    lines: list[str] = []
    lines.append(json.dumps(_nav_case(MainWindow), ensure_ascii=False))
    for case in _search_cases(MainWindow):
        lines.append(json.dumps(case, ensure_ascii=False))
    for case in _geometry_cases(MainWindow):
        lines.append(json.dumps(case, ensure_ascii=False))
    for case in _drag_cases(MainWindow):
        lines.append(json.dumps(case, ensure_ascii=False))
    for case in _close_cases(MainWindow):
        lines.append(json.dumps(case, ensure_ascii=False))
    for case in _pair_cases(MainWindow):
        lines.append(json.dumps(case, ensure_ascii=False))
    for case in _listen_failed_cases(MainWindow):
        lines.append(json.dumps(case, ensure_ascii=False))
    for case in _update_available_cases(MainWindow):
        lines.append(json.dumps(case, ensure_ascii=False))
    tooltip = _exception_tooltip(MainWindow)
    for case in _exception_cases(MainWindow):
        case["logTooltip"] = tooltip
        lines.append(json.dumps(case, ensure_ascii=False))
    for case in _update_changed_cases(MainWindow):
        lines.append(json.dumps(case, ensure_ascii=False))

    FIXTURE.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"wrote {len(lines)} cases to {FIXTURE}")


if __name__ == "__main__":
    main()
