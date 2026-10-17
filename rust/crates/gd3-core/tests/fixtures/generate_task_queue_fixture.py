"""生成任务调度队列的对比金文件（`task_queue.jsonl`，第 54 步）。

要钉住的是 `app/services/task_service.py` 里的两块纯逻辑：

- `TaskQueue`（`:243`）——`_waiting: list[str]` + `_running: dict[str, str]`
  的增删查改与顺序语义；
- `TaskService._rebalance` / `_pump`（`:641` / `:650`）里**只跟队列有关**的
  那一半（`runningIds()[maxTaskNum:]` 与「还有几个并发位」）。

做法与前面几步一致：**跑真 `TaskQueue`**，逐 op 记录返回值与末态。服务层
`_rebalance` / `_pump` 里「取消协程」「起任务」的动作没法脱离 Qt 协程跑，
所以脚本只把那两段循环体照抄成 `_pump()` / `_rebalance()` 两个本地函数，
其中「起任务」用一个只调 `queue.run(tid, workId)` 的假派发器顶替——
Rust 侧的 `next_to_dispatch` / `rebalance` 对应的正是这两段。

两条口径说明：

1. `pump` / `rebalance` 的返回值是**派发/退回的 taskId 列表**，不是队列快照；
   快照另外记在 `final` 里。Rust 测试回放时按同样的顺序调 `run()` 才能对上。
2. `moveToFront` 的目标顺序取**等待队列自身的顺序**，不是参数顺序
   （`[tid for tid in self._waiting if tid in set(taskIds)]`），所以
   `moveToFront(["d", "b"])` 得到 `["b", "d", ...]`。金文件里专门有一条用例。

    python3 rust/crates/gd3-core/tests/fixtures/generate_task_queue_fixture.py
"""

from __future__ import annotations

import json
from pathlib import Path

FIXTURE = Path(__file__).with_name("task_queue.jsonl")


def _queue():
    from app.services.task_service import TaskQueue

    return TaskQueue()


def _pump(queue, max_task_num: int, dispatch) -> list[str]:
    """照抄 `TaskService._pump()` 的循环体（`_dispatch` 换成假派发器）。"""
    dispatched: list[str] = []
    while queue.runningCount() < max_task_num:
        task_id = queue.nextWaiting()
        if task_id is None:
            break
        dispatch(task_id)
        dispatched.append(task_id)
    return dispatched


def _rebalance(queue, max_task_num: int, can_pause: set[str], dispatch) -> dict:
    """照抄 `TaskService._rebalance()`：先退超额，再 `_pump()`。"""
    moved: list[str] = []
    for task_id in queue.runningIds()[max_task_num:]:
        if task_id in can_pause:
            queue.cancel(task_id)
            queue.wait(task_id)
            moved.append(task_id)
    return {"moved": moved, "dispatched": _pump(queue, max_task_num, dispatch)}


def _snapshot(queue) -> dict:
    return {
        "waiting": queue.waitingOrder(),
        "running": [[task_id, queue.workIdOf(task_id)] for task_id in queue.runningIds()],
    }


def _run_case(case: dict) -> dict:
    queue = _queue()
    serial = [0]

    def dispatch(task_id: str) -> None:
        serial[0] += 1
        queue.run(task_id, f"w{serial[0]}")

    results = []
    for op in case["ops"]:
        name = op["op"]
        if name == "wait":
            results.append(queue.wait(op["taskId"]))
        elif name == "cancel":
            results.append(queue.cancel(op["taskId"]))
        elif name == "run":
            results.append(queue.run(op["taskId"], op["workId"]))
        elif name == "done":
            results.append(queue.done(op["taskId"]))
        elif name == "workIdOf":
            results.append(queue.workIdOf(op["taskId"]))
        elif name == "isRunning":
            results.append(queue.isRunning(op["taskId"]))
        elif name == "isWaiting":
            results.append(queue.isWaiting(op["taskId"]))
        elif name == "waitingOrder":
            results.append(queue.waitingOrder())
        elif name == "moveToFront":
            results.append(queue.moveToFront(op["taskIds"]))
        elif name == "runningCount":
            results.append(queue.runningCount())
        elif name == "runningIds":
            results.append(queue.runningIds())
        elif name == "nextWaiting":
            results.append(queue.nextWaiting())
        elif name == "pump":
            results.append(_pump(queue, op["maxTaskNum"], dispatch))
        elif name == "rebalance":
            can_pause = set(op.get("canPause", []))
            results.append(_rebalance(queue, op["maxTaskNum"], can_pause, dispatch))
        else:
            raise SystemExit(f"unknown op: {name}")
    return {
        "name": case["name"],
        "ops": case["ops"],
        "results": results,
        "final": _snapshot(queue),
    }


CASES = [
    {
        "name": "empty",
        "ops": [
            {"op": "waitingOrder"},
            {"op": "runningIds"},
            {"op": "runningCount"},
            {"op": "nextWaiting"},
            {"op": "moveToFront", "taskIds": ["a"]},
            {"op": "workIdOf", "taskId": "a"},
        ],
    },
    {
        "name": "wait-dedupes",
        "ops": [
            {"op": "wait", "taskId": "a"},
            {"op": "wait", "taskId": "b"},
            {"op": "wait", "taskId": "a"},
            {"op": "waitingOrder"},
            {"op": "isWaiting", "taskId": "a"},
            {"op": "isWaiting", "taskId": "c"},
        ],
    },
    {
        "name": "cancel-from-waiting",
        "ops": [
            {"op": "wait", "taskId": "a"},
            {"op": "wait", "taskId": "b"},
            {"op": "wait", "taskId": "c"},
            {"op": "cancel", "taskId": "b"},
            {"op": "waitingOrder"},
            {"op": "cancel", "taskId": "zzz"},
            {"op": "waitingOrder"},
        ],
    },
    {
        "name": "cancel-from-running",
        "ops": [
            {"op": "run", "taskId": "a", "workId": "w1"},
            {"op": "run", "taskId": "b", "workId": "w2"},
            {"op": "cancel", "taskId": "a"},
            {"op": "runningIds"},
            {"op": "runningCount"},
            {"op": "isRunning", "taskId": "a"},
        ],
    },
    {
        "name": "cancel-both",
        "ops": [
            {"op": "wait", "taskId": "a"},
            {"op": "run", "taskId": "a", "workId": "w1"},
            {"op": "cancel", "taskId": "a"},
            {"op": "waitingOrder"},
            {"op": "runningIds"},
        ],
    },
    {
        "name": "run-keeps-insertion-order",
        "ops": [
            {"op": "run", "taskId": "a", "workId": "w1"},
            {"op": "run", "taskId": "b", "workId": "w2"},
            {"op": "run", "taskId": "c", "workId": "w3"},
            {"op": "run", "taskId": "a", "workId": "w4"},
            {"op": "runningIds"},
            {"op": "workIdOf", "taskId": "a"},
            {"op": "workIdOf", "taskId": "b"},
        ],
    },
    {
        "name": "done-removes-only-running",
        "ops": [
            {"op": "wait", "taskId": "a"},
            {"op": "run", "taskId": "b", "workId": "w1"},
            {"op": "done", "taskId": "b"},
            {"op": "runningIds"},
            {"op": "waitingOrder"},
            {"op": "done", "taskId": "b"},
            {"op": "runningCount"},
        ],
    },
    {
        "name": "move-to-front-uses-waiting-order",
        "ops": [
            {"op": "wait", "taskId": "a"},
            {"op": "wait", "taskId": "b"},
            {"op": "wait", "taskId": "c"},
            {"op": "wait", "taskId": "d"},
            {"op": "moveToFront", "taskIds": ["d", "b"]},
            {"op": "waitingOrder"},
            {"op": "moveToFront", "taskIds": ["zzz"]},
            {"op": "waitingOrder"},
            {"op": "moveToFront", "taskIds": []},
        ],
    },
    {
        "name": "move-to-front-ignores-running",
        "ops": [
            {"op": "wait", "taskId": "a"},
            {"op": "wait", "taskId": "b"},
            {"op": "run", "taskId": "c", "workId": "w1"},
            {"op": "moveToFront", "taskIds": ["c", "b"]},
            {"op": "waitingOrder"},
            {"op": "runningIds"},
        ],
    },
    {
        "name": "next-waiting-pops-head",
        "ops": [
            {"op": "wait", "taskId": "a"},
            {"op": "wait", "taskId": "b"},
            {"op": "nextWaiting"},
            {"op": "waitingOrder"},
            {"op": "nextWaiting"},
            {"op": "nextWaiting"},
            {"op": "waitingOrder"},
        ],
    },
    {
        "name": "pump-under-limit",
        "ops": [
            {"op": "wait", "taskId": "a"},
            {"op": "wait", "taskId": "b"},
            {"op": "wait", "taskId": "c"},
            {"op": "pump", "maxTaskNum": 3},
            {"op": "runningIds"},
            {"op": "waitingOrder"},
            {"op": "pump", "maxTaskNum": 3},
        ],
    },
    {
        "name": "pump-over-limit",
        "ops": [
            {"op": "wait", "taskId": "a"},
            {"op": "wait", "taskId": "b"},
            {"op": "wait", "taskId": "c"},
            {"op": "wait", "taskId": "d"},
            {"op": "pump", "maxTaskNum": 2},
            {"op": "runningIds"},
            {"op": "waitingOrder"},
            {"op": "pump", "maxTaskNum": 2},
            {"op": "pump", "maxTaskNum": 0},
        ],
    },
    {
        "name": "pump-empty-waiting",
        "ops": [
            {"op": "pump", "maxTaskNum": 4},
            {"op": "wait", "taskId": "a"},
            {"op": "pump", "maxTaskNum": 1},
            {"op": "pump", "maxTaskNum": 1},
        ],
    },
    {
        "name": "rebalance-no-overflow",
        "ops": [
            {"op": "run", "taskId": "a", "workId": "w1"},
            {"op": "run", "taskId": "b", "workId": "w2"},
            {"op": "rebalance", "maxTaskNum": 2},
            {"op": "runningIds"},
            {"op": "waitingOrder"},
        ],
    },
    {
        "name": "rebalance-pushes-excess-back",
        "ops": [
            {"op": "run", "taskId": "a", "workId": "w1"},
            {"op": "run", "taskId": "b", "workId": "w2"},
            {"op": "run", "taskId": "c", "workId": "w3"},
            {"op": "run", "taskId": "d", "workId": "w4"},
            {"op": "wait", "taskId": "e"},
            {"op": "rebalance", "maxTaskNum": 2, "canPause": ["a", "b", "c", "d"]},
            {"op": "runningIds"},
            {"op": "waitingOrder"},
        ],
    },
    {
        "name": "rebalance-skips-unpausable",
        "ops": [
            {"op": "run", "taskId": "a", "workId": "w1"},
            {"op": "run", "taskId": "b", "workId": "w2"},
            {"op": "run", "taskId": "c", "workId": "w3"},
            {"op": "run", "taskId": "d", "workId": "w4"},
            {"op": "wait", "taskId": "e"},
            {"op": "rebalance", "maxTaskNum": 2, "canPause": ["b", "d"]},
            {"op": "runningIds"},
            {"op": "waitingOrder"},
            {"op": "rebalance", "maxTaskNum": 2, "canPause": ["c"]},
            {"op": "runningIds"},
            {"op": "waitingOrder"},
        ],
    },
    {
        "name": "rebalance-from-zero",
        "ops": [
            {"op": "run", "taskId": "a", "workId": "w1"},
            {"op": "run", "taskId": "b", "workId": "w2"},
            {"op": "wait", "taskId": "c"},
            {"op": "rebalance", "maxTaskNum": 0, "canPause": ["a", "b"]},
            {"op": "runningIds"},
            {"op": "waitingOrder"},
        ],
    },
    {
        "name": "long-session",
        "ops": [
            {"op": "wait", "taskId": "t1"},
            {"op": "wait", "taskId": "t2"},
            {"op": "wait", "taskId": "t3"},
            {"op": "wait", "taskId": "t4"},
            {"op": "pump", "maxTaskNum": 2},
            {"op": "wait", "taskId": "t5"},
            {"op": "moveToFront", "taskIds": ["t5"]},
            {"op": "waitingOrder"},
            {"op": "done", "taskId": "t1"},
            {"op": "pump", "maxTaskNum": 2},
            {"op": "waitingOrder"},
            {"op": "run", "taskId": "t1", "workId": "manual"},
            {"op": "runningIds"},
            {"op": "rebalance", "maxTaskNum": 2, "canPause": ["t2", "t5", "t1"]},
            {"op": "runningIds"},
            {"op": "waitingOrder"},
            {"op": "cancel", "taskId": "t3"},
            {"op": "waitingOrder"},
            {"op": "done", "taskId": "t2"},
            {"op": "pump", "maxTaskNum": 2},
            {"op": "runningIds"},
            {"op": "waitingOrder"},
        ],
    },
]


def main() -> None:
    lines = []
    for case in CASES:
        lines.append(json.dumps(_run_case(case), ensure_ascii=False))
    FIXTURE.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"wrote {len(lines)} cases to {FIXTURE}")


if __name__ == "__main__":
    main()
