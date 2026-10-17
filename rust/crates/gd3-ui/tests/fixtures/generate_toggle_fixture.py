"""生成「暂停/继续」按钮分派的对比金文件（`toggle.jsonl`，第 54 步）。

要钉住的是 `app/view/cards/task_cards.py:363 TaskCard._onToggleClicked`：

```python
if self._task.isSeeding:            self._taskService.stopSeeding(self._task)
elif self._task.status == COMPLETED: self._taskService.startSeeding(self._task)
elif self._task.status == RUNNING:   self._taskService.pause(self._task)
else:                                self._taskService.start(self._task)
self.refresh()
```

这是个**顺序敏感的短路链**，不是按状态查表：`isSeeding` 优先于 `COMPLETED`，
所以「已完成且正在做种」点一下是停止做种。金文件把 `status × isSeeding`
十种组合全跑一遍，记录**实际被调用的服务方法名**与 `refresh()` 次数。

做法：`TaskCard._onToggleClicked` 是普通方法，绑到一个桩对象上跑真代码；
`_task` 与 `_taskService` 都是记录型替身，`refresh()` 只计数。

    python3 rust/crates/gd3-ui/tests/fixtures/generate_toggle_fixture.py
"""

from __future__ import annotations

import json
import os
from pathlib import Path

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

FIXTURE = Path(__file__).with_name("toggle.jsonl")


def _app():
    from PySide6.QtWidgets import QApplication

    return QApplication.instance() or QApplication([])


class FakeTask:
    """只带 `_onToggleClicked` 用到的两个属性。"""

    def __init__(self, status, is_seeding: bool) -> None:
        self.status = status
        self.isSeeding = is_seeding


class FakeService:
    """记录被调用的服务方法名（顺序即调用顺序）。"""

    def __init__(self) -> None:
        self.calls: list[str] = []

    def stopSeeding(self, _task) -> None:
        self.calls.append("stopSeeding")

    def startSeeding(self, _task) -> None:
        self.calls.append("startSeeding")

    def pause(self, _task) -> None:
        self.calls.append("pause")

    def start(self, _task) -> None:
        self.calls.append("start")


class FakeCard:
    """`TaskCard` 的最小替身：方法体由 `TaskCard._onToggleClicked` 提供。"""

    def __init__(self, task, service) -> None:
        self._task = task
        self._taskService = service
        self.refreshed = 0

    def refresh(self, _force: bool = False) -> None:
        self.refreshed += 1


def _run(status, is_seeding: bool) -> dict:
    from app.view.cards.task_cards import TaskCard

    service = FakeService()
    card = FakeCard(FakeTask(status, is_seeding), service)
    TaskCard._onToggleClicked(card)
    return {
        "status": status.name,
        "isSeeding": is_seeding,
        "calls": service.calls,
        "refreshed": card.refreshed,
    }


def main() -> None:
    _app()
    from app.models.task import TaskStatus

    lines = []
    for status in (
        TaskStatus.WAITING,
        TaskStatus.RUNNING,
        TaskStatus.PAUSED,
        TaskStatus.COMPLETED,
        TaskStatus.FAILED,
    ):
        for is_seeding in (False, True):
            lines.append(json.dumps(_run(status, is_seeding), ensure_ascii=False))
    FIXTURE.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"wrote {len(lines)} cases to {FIXTURE}")


if __name__ == "__main__":
    main()
