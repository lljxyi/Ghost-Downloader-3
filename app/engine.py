from __future__ import annotations

from app.config.cfg import cfg
from app.services import rust_aria2, rust_browser
from app.services.aria2_rpc import Aria2RpcService
from app.services.browser_service import BrowserService
from app.services.category_service import CategoryService
from app.services.feature_service import FeatureService
from app.services.port_listener import PortListener
from app.services.name_conflict_queue import NameConflictQueue
from app.services.runtime_status import RuntimeStatusService
from app.services.speed_meter import SpeedMeter
from app.services.task_service import NameConflictChoice, TaskService


class Engine:
    def __init__(self, coroutineRunner, fileWatcher, loadCrx, extensionFolder, deleteRecoverably) -> None:
        self.coroutineRunner = coroutineRunner
        self.categoryService = CategoryService()
        self.speedMeter = SpeedMeter(coroutineRunner)
        self.taskService = TaskService(coroutineRunner, self.categoryService, self.speedMeter, fileWatcher,
                                       deleteRecoverably, lambda: NameConflictChoice(cfg.nameConflict.value))
        self.nameConflictQueue = NameConflictQueue(self.taskService.probeConflict, self.taskService.add,
                                                   coroutineRunner.post)
        self.taskService.nameConflicted.connect(self.nameConflictQueue.add)
        self.runtimeStatusService = RuntimeStatusService(coroutineRunner)
        self.featureService = FeatureService(self.taskService, self.categoryService, coroutineRunner,
                                             self.runtimeStatusService)

        self.browserService = BrowserService(coroutineRunner, self.taskService, self.speedMeter.speedChanged,
                                             self.featureService.parse, loadCrx, extensionFolder)
        self.browserListener = createBrowserListener(coroutineRunner, self.browserService)
        self.aria2RpcService = Aria2RpcService(coroutineRunner, self.featureService.parse, self.taskService.add)
        self.aria2RpcListener = createAria2Listener(coroutineRunner, self.aria2RpcService)

    def start(self) -> None:
        self.taskService.resumeSaved()
        self.featureService.activate()
        self.browserListener.start()
        self.aria2RpcListener.start()

    def stop(self) -> None:
        self.taskService.stop()
        self.taskService.flush()
        self.speedMeter.stop()
        self.browserListener.stop()
        self.aria2RpcListener.stop()
        self.featureService.deactivate()
        self.taskService.flush()


def createBrowserListener(coroutineRunner, service):
    """扩展监听器的唯一开关点：`GD3_RUST_BROWSER=1` 时套接字与 WebSocket 帧交给 Rust。"""
    if rust_browser.isEnabled():
        return rust_browser.RustBrowserListener(
            coroutineRunner, service,
            isEnabled=cfg.isBrowserExtensionEnabled, port=cfg.browserExtensionPort,
        )
    return PortListener(coroutineRunner, service.handle,
                        isEnabled=cfg.isBrowserExtensionEnabled, port=cfg.browserExtensionPort)


def createAria2Listener(coroutineRunner, service):
    """aria2 监听器的唯一开关点：`GD3_RUST_ARIA2=1` 时交给 Rust。

    状态回调来自 Rust 线程，所以派发器要用线程安全的 `post`。
    """
    if rust_aria2.isEnabled():
        return rust_aria2.RustAria2Listener(
            service, isEnabled=cfg.isAria2RpcEnabled, port=cfg.aria2RpcPort,
            dispatcher=coroutineRunner.post,
        )
    return PortListener(coroutineRunner, service.handle,
                        isEnabled=cfg.isAria2RpcEnabled, port=cfg.aria2RpcPort)
