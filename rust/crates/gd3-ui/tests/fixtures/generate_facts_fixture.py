"""生成任务运行时事实层的对比金文件（第 55 步）。

产出两个文件（与本脚本同目录）：

1. `task_registry.jsonl` —— `Task._registry` / `TaskStep._registry` 的**全量**导出：
   - `Task` 每行 `{kind:"task", name, type, canSeed, hasOutputFile}`；
   - `TaskStep` 每行 `{kind:"step", name, type, rule}`，`rule` 是
     `always` / `never` / `range` / `notLive`。

   `facts.rs` 里的 `SEEDING_TASKS` / `NEVER_PAUSE_STEPS` / `RANGE_PAUSE_STEPS`
   是**手抄**的 Python 类属性表。Python 侧新增一个步骤类之后 Rust 会静默按默认值
   算，所以这里把登记表全量导出来逐条比对——新增类会立刻报红。

   `rule` 的判定不靠抄名字：`inspect.getattr_static(cls, "canPause")` 拿到的是
   类属性原样（`bool` 或 `property`）；`property` 再读 `fget.__code__.co_names`
   判断它读的是 `canUseRangeRequests` 还是 `isLive`。都不匹配就抛异常——宁可
   生成脚本炸掉，也不要静默把新规则算成 `always`。

2. `task_facts.jsonl` —— 每条 case：
   `{name, record, live, utcOffset, expected{canPause, canSeed, hasOutputFile,
   outputPath, completedAtText}}`。

   `record` **由真对象 `toDict()` 出来**，`expected` **由真属性算出**，脚本不抄
   算法：

   - `canPause` 走 `task.canPause`（`Task.canPause` 遍历 steps 找第一个 RUNNING）；
   - `canSeed` / `hasOutputFile` 走类属性；
   - `outputPath` 走 `task.outputPath`（`pathlib` 的 `/` 运算 + `BilibiliTask`
     的多文件覆盖）；
   - `completedAtText` 走 `datetime.fromtimestamp(...).strftime("%Y-%m-%d %H:%M:%S")`
     （**本地时区**，脚本用 `TZ=UTC` + `time.tzset()` 把偏移钉成 0；带非零偏移的
     两条 case 显式用 `timezone(timedelta(...))` 算，因为 Rust 侧的
     `to_time_text(ts, offset)` 把偏移当参数收）。

   `isExisting` 不进金文件——它取决于跑测试的机器。Rust 侧
   `facts::is_existing` 由单测覆盖（`/`、不存在的路径、空串）。

构造策略（避开各 pack 的 `__post_init__` 陷阱）：

- **步骤级规则用基类 `Task`**：`canPause` 只看步骤类型（`Task.canPause` 对所有
  任务类都是同一个方法），所以不需要真任务子类。`FtpStep` 需要一个
  `remotePath`，其余步骤只要求 `stepIndex`。
- **`M3U8TaskStep` 必须用 `M3U8Task`**：它的 `canPause` 读 `self.task.isLive`。
  且 `M3U8Task.__post_init__` 会把「live + RUNNING」归一成 COMPLETED，所以
  live 那条 case 要**先 `fromDict`（步骤 WAITING）再手工把步骤置 RUNNING**。
- **`BilibiliTask` 的 `files` 全部用 `.mp4`/`.m4a`**：`__post_init__` 的
  `self.update()`（网络）只在存在非 mp4/m4a 文件时才跑。
- **不构造 `BTTask` / `FtpTask` 实例**：前者 `__post_init__` 处理 torrentData，
  后者要求 `connectionInfo` 且会 `addStep`。它们的类属性已由
  `task_registry.jsonl` 钉死。
- **不出现「所有步骤 COMPLETED」的 case**：`updateStatus()` 会往 `completedAt`
  写 `int(time())`，金文件就不稳定了。

    PYTHONPATH=. .venv/bin/python \
        rust/crates/gd3-ui/tests/fixtures/generate_facts_fixture.py
"""

from __future__ import annotations

import importlib
import inspect
import json
import os
import sys
import time
from dataclasses import MISSING, fields
from datetime import datetime, timedelta, timezone
from pathlib import Path

# 金文件必须与机器时区无关。
os.environ["TZ"] = "UTC"
time.tzset()

FIXTURE_DIR = Path(__file__).resolve().parent
REPO = FIXTURE_DIR.parents[4]
if str(REPO) not in sys.path:
    sys.path.insert(0, str(REPO))
# `features/*/task.py` 内部写的是 `from http_pack.task import ...`（不带 `features.`
# 前缀），所以 `features` 目录本身也要在搜索路径上。
FEATURES = REPO / "features"
if str(FEATURES) not in sys.path:
    sys.path.insert(0, str(FEATURES))

# 顺序固定，保证 registry 的遍历顺序稳定（实际输出按短类名排序）。
PACKS = [
    "http_pack",
    "bittorrent_pack",
    "ed2k_pack",
    "m3u8_pack",
    "ffmpeg_pack",
    "ftp_pack",
    "bili_pack",
    "yt_dlp_pack",
    "huggingface_pack",
]


def load_models():
    """导入全部 FeaturePack，再取 `Task` / `TaskStep` / `toDict` / `fromDict`。"""
    for pack in PACKS:
        importlib.import_module(f"{pack}.task")
    from app.models.serialization import fromDict, toDict
    from app.models.task import Task, TaskFile, TaskStep
    from bili_pack.task import BiliPage

    return {
        "Task": Task,
        "TaskStep": TaskStep,
        "TaskFile": TaskFile,
        "BiliPage": BiliPage,
        "fromDict": fromDict,
        "toDict": toDict,
    }


# ---------------------------------------------------------------------------
# 1. 登记表导出
# ---------------------------------------------------------------------------

RULE_BY_FIELD = {
    "canUseRangeRequests": "range",
    "isLive": "notLive",
}


def classify_can_pause(cls) -> str:
    """把 `TaskStep.canPause` 的类属性归成四种规则之一。"""
    raw = inspect.getattr_static(cls, "canPause")
    if isinstance(raw, property):
        names = set(raw.fget.__code__.co_names)
        matched = {RULE_BY_FIELD[name] for name in names if name in RULE_BY_FIELD}
        if len(matched) != 1:
            raise SystemExit(
                f"{cls.__name__}.canPause 是 property，但读的字段 {sorted(names)} "
                "里没有恰好一个已知规则（canUseRangeRequests / isLive）。"
                "请先在 facts.rs 的 StepPauseRule 里补规则。"
            )
        return matched.pop()
    if isinstance(raw, bool):
        return "always" if raw else "never"
    raise SystemExit(f"{cls.__name__}.canPause 既不是 bool 也不是 property：{raw!r}")


def export_registry(models, path: Path) -> None:
    Task = models["Task"]
    TaskStep = models["TaskStep"]

    rows: list[dict] = []
    seen: dict[str, object] = {}

    def remember(name: str, cls: object) -> None:
        # 同一批代码可能被 import 成两个模块名（`http_pack.task` 与
        # `features.http_pack.task`），registry 里就会出现两份。同名必须是同一个
        # 类对象，否则就是真冲突。
        if name in seen and seen[name] is not cls:
            raise SystemExit(f"登记表里出现两个不同的 {name}")
        seen[name] = cls

    for key in sorted(Task._registry):
        cls = Task._registry[key]
        remember(cls.__name__, cls)
        rows.append(
            {
                "kind": "task",
                "name": cls.__name__,
                "type": f"{cls.__module__}.{cls.__name__}",
                "canSeed": bool(cls.canSeed),
                "hasOutputFile": bool(cls.hasOutputFile),
            }
        )

    for key in sorted(TaskStep._registry):
        cls = TaskStep._registry[key]
        remember(cls.__name__, cls)
        rows.append(
            {
                "kind": "step",
                "name": cls.__name__,
                "type": f"{cls.__module__}.{cls.__name__}",
                "rule": classify_can_pause(cls),
            }
        )

    # 基类本身不在 registry 里，但 Rust 侧对「未知类型」走的是基类默认值，
    # 所以把基类的规则也钉一条，避免有人改了 `TaskStep.canPause` 默认值。
    rows.append(
        {
            "kind": "step",
            "name": "TaskStep",
            "type": "app.models.task.TaskStep",
            "rule": classify_can_pause(TaskStep),
        }
    )
    rows.append(
        {
            "kind": "task",
            "name": "Task",
            "type": "app.models.task.Task",
            "canSeed": bool(Task.canSeed),
            "hasOutputFile": bool(Task.hasOutputFile),
        }
    )

    with path.open("w", encoding="utf-8", newline="\n") as handle:
        for row in rows:
            handle.write(json.dumps(row, ensure_ascii=False, sort_keys=True))
            handle.write("\n")


# ---------------------------------------------------------------------------
# 2. 事实用例
# ---------------------------------------------------------------------------


def required_defaults(cls) -> dict:
    """给 `cls` 的必填 dataclass 字段造占位值（只有 `stepIndex` / `remotePath`）。"""
    values: dict = {}
    for field in fields(cls):
        if not field.init:
            continue
        if field.default is not MISSING or field.default_factory is not MISSING:
            continue
        if field.name == "stepIndex":
            values[field.name] = 0
        elif field.name == "remotePath":
            values[field.name] = "/remote/f.bin"
        else:
            raise SystemExit(f"{cls.__name__} 新增了必填字段 {field.name}，请补占位值")
    return values


def make_step(models, type_name: str, status: str, **overrides):
    """用 `fromDict` 造一个真步骤对象（`type` 标签决定类）。"""
    registry = models["TaskStep"]._registry
    cls = registry[type_name]
    data = required_defaults(cls)
    data.update(
        {
            "type": type_name,
            "status": status,
            "stepId": f"stp_{cls.__name__.lower()}",
        }
    )
    data.update(overrides)
    return models["fromDict"](data, models["TaskStep"])


def make_task(models, record: dict):
    """用 `fromDict` 造一个真任务对象（`type` 标签决定类）。"""
    return models["fromDict"](record, models["Task"])


def phantom_task(models, record: dict):
    """只填属性、不走 `__init__` / `__post_init__` 的「幽灵」任务对象。

    用来读 `outputPath` / `canPause` / `canSeed` 这些纯属性——绕开 `BTTask` 与
    `FtpTask` 的 `__post_init__`：前者要求 `files` 不是 None 并会求和，
    后者要求 `connectionInfo` 且会按 `files` 补建步骤。两者都与本步要钉的
    「类属性 + 路径拼接」无关。
    """
    cls = models["Task"]
    type_name = record.get("type")
    if type_name:
        cls = models["Task"]._registry[type_name]
    task = object.__new__(cls)
    task.name = record["name"]
    task.outputFolder = Path(record["outputFolder"])
    task.completedAt = record.get("completedAt", 0)
    task.status = _status_of(record.get("status", "WAITING"))
    task.isLive = record.get("isLive", False)
    task.files = record.get("files")
    task.steps = []
    for raw in record.get("steps") or []:
        step = models["fromDict"](raw, models["TaskStep"])
        step._bindTask(task)
        task.steps.append(step)
    return task


def completed_at_text(completed_at: int, offset_seconds: int) -> str | None:
    if not completed_at:
        return None
    tz = timezone(timedelta(seconds=offset_seconds))
    return datetime.fromtimestamp(completed_at, tz=tz).strftime("%Y-%m-%d %H:%M:%S")


def fact_row(name: str, record: dict, task, utc_offset: int = 0, is_seeding=False) -> dict:
    """把真对象的属性读成期望值。"""
    return {
        "name": name,
        "record": record,
        "live": {"isSeeding": is_seeding},
        "utcOffset": utc_offset,
        "expected": {
            "canPause": bool(task.canPause),
            "canSeed": bool(type(task).canSeed),
            "hasOutputFile": bool(type(task).hasOutputFile),
            "outputPath": task.outputPath,
            "completedAtText": completed_at_text(task.completedAt, utc_offset),
        },
    }


def base_record(models, **overrides) -> dict:
    """基类 `Task` 的规范 record（`toDict` 省略 `type`）。"""
    record = {
        "name": "file.bin",
        "url": "https://example.test/file.bin",
        "packId": "http",
        "taskId": "tsk_fixture",
        "status": "WAITING",
        "steps": [],
        "createdAt": 1700000000,
        "completedAt": 0,
        "outputFolder": "/downloads",
        "fileSize": 0,
        "files": None,
        "category": None,
        "shouldSeed": False,
        "checksums": {},
    }
    record.update(overrides)
    # 走一遍真 `fromDict` + `toDict`，保证写进金文件的 record 就是 Python 自己
    # 会写出的形状（`updateStatus()` 会改写 status / completedAt / fileSize）。
    return models["toDict"](make_task(models, record))


def step_pause_cases(models) -> list[dict]:
    """每个步骤类一条（range 两条、M3U8 两条）。"""
    cases: list[dict] = []

    for key in sorted(models["TaskStep"]._registry):
        type_name = key
        cls = models["TaskStep"]._registry[key]
        rule = classify_can_pause(cls)
        short = cls.__name__

        if rule == "range":
            for flag in (False, True):
                step = make_step(
                    models, type_name, "RUNNING", canUseRangeRequests=flag
                )
                record = base_record(
                    models,
                    name="file.bin",
                    steps=[models["toDict"](step)],
                    status="RUNNING",
                )
                task = make_task(models, record)
                cases.append(
                    fact_row(f"step-{short}-range-{int(flag)}", record, task)
                )
            continue

        if rule == "notLive":
            for live in (False, True):
                step = make_step(models, type_name, "WAITING")
                record = base_record(
                    models,
                    name="live.mp4",
                    steps=[models["toDict"](step)],
                    status="WAITING",
                )
                record["type"] = type_name.rsplit(".", 1)[0] + ".M3U8Task"
                record["isLive"] = live
                record["packId"] = "m3u8"
                task = make_task(models, record)
                if live:
                    # `M3U8Task.__post_init__` 会把「live + RUNNING」归一成
                    # COMPLETED，所以先构造（步骤 WAITING）再手工置 RUNNING。
                    task.steps[0].status = _status_of("RUNNING")
                    task.status = _status_of("RUNNING")
                    record = models["toDict"](task)
                cases.append(fact_row(f"step-{short}-live-{int(live)}", record, task))
            continue

        step = make_step(models, type_name, "RUNNING")
        record = base_record(
            models,
            name="file.bin",
            steps=[models["toDict"](step)],
            status="RUNNING",
        )
        task = make_task(models, record)
        cases.append(fact_row(f"step-{short}-{rule}", record, task))

    return cases


def _status_of(name: str):
    from app.models.task import TaskStatus

    return TaskStatus[name]


def task_kind_cases(models) -> list[dict]:
    """每个任务类的 canSeed / hasOutputFile / outputPath（无 files）。"""
    cases: list[dict] = []
    for key in sorted(models["Task"]._registry):
        cls = models["Task"]._registry[key]
        type_name = f"{cls.__module__}.{cls.__name__}"
        pack = cls.__module__.split(".")[0].removesuffix("_pack")
        record = base_record(
            models,
            name="file.bin",
            packId=pack or "http",
        )
        record["type"] = type_name
        task = phantom_task(models, record)
        cases.append(fact_row(f"task-{cls.__name__}", record, task))
    return cases


OUTPUT_FOLDER_CASES = [
    # (outputFolder, name) —— 与 `gd3-core/src/path.rs` 的单测同一批边界值
    ("", "name.txt"),
    (".", "name.txt"),
    ("./", "name.txt"),
    ("a//b", "name.txt"),
    ("//Downloads", "name.txt"),
    ("///x", "name.txt"),
    ("/a/", "name.txt"),
    ("/a/./b", "name.txt"),
    ("/a/../b", "name.txt"),
    ("rel/", "name.txt"),
    # 空名字 / "." 会被 `Task.__post_init__` 的 `toSafeFilename` 换成 `download`，
    # 所以这两个 case 钉的是「record 里的 name 已经归一过」。
    ("/a/b/", ""),
    ("/a/b/..", "name.txt"),
    ("/a/b", "a.tar.gz"),
]


def output_path_cases(models) -> list[dict]:
    cases: list[dict] = []
    for index, (folder, name) in enumerate(OUTPUT_FOLDER_CASES):
        record = base_record(models, outputFolder=folder, name=name)
        task = make_task(models, record)
        cases.append(fact_row(f"output-path-{index}", record, task))
    return cases


BILI_OUTPUT_CASES = [
    # (outputFolder, name, relativePaths)
    # 名字带扩展名才能区分两条分支：多文件走 `Path(name).stem`（砍掉 `.mp4`），
    # 单文件走基类 `outputFolder / name`（原样保留）。
    ("/downloads", "My Show.mp4", ["1.mp4", "2.m4a"]),
    ("/downloads", "My Show.mp4", ["1.mp4"]),
    ("/downloads", "a.tar.gz", ["1.mp4", "2.m4a"]),
    ("/downloads", "a.tar.gz", ["1.mp4"]),
    ("/downloads", "My Show", []),
]


def bili_output_cases(models) -> list[dict]:
    BiliPage = models["BiliPage"]
    type_name = "bili_pack.task.BilibiliTask"
    cases: list[dict] = []
    for index, (folder, name, paths) in enumerate(BILI_OUTPUT_CASES):
        pages = [
            BiliPage(index=position, relativePath=relative)
            for position, relative in enumerate(paths)
        ]
        record = base_record(
            models,
            name=name,
            url="https://www.bilibili.com/video/BV1xx411c7mD",
            packId="bili",
            outputFolder=folder,
            files=[models["toDict"](page) for page in pages],
        )
        record["type"] = type_name
        task = make_task(models, record)
        cases.append(fact_row(f"bili-output-{index}", record, task))
    return cases


COMPLETED_AT_CASES = [
    (0, 0),
    (1700000000, 0),
    (1700000000, 3600),
    (1700000000, -18000),
]


def completed_at_cases(models) -> list[dict]:
    cases: list[dict] = []
    for index, (completed_at, offset) in enumerate(COMPLETED_AT_CASES):
        # 没有步骤 → `updateStatus()` 提前返回，`completedAt` 原样保留。
        record = base_record(models, completedAt=completed_at, status="COMPLETED")
        task = make_task(models, record)
        cases.append(
            fact_row(
                f"completed-at-{index}",
                record,
                task,
                utc_offset=offset,
            )
        )
    return cases


def export_facts(models, path: Path) -> int:
    rows: list[dict] = []
    rows.extend(step_pause_cases(models))
    rows.extend(task_kind_cases(models))
    rows.extend(output_path_cases(models))
    rows.extend(bili_output_cases(models))
    rows.extend(completed_at_cases(models))

    names = [row["name"] for row in rows]
    duplicates = {name for name in names if names.count(name) > 1}
    if duplicates:
        raise SystemExit(f"用例名重复：{sorted(duplicates)}")

    with path.open("w", encoding="utf-8", newline="\n") as handle:
        for row in rows:
            handle.write(json.dumps(row, ensure_ascii=False, sort_keys=True))
            handle.write("\n")
    return len(rows)


def main() -> None:
    models = load_models()
    export_registry(models, FIXTURE_DIR / "task_registry.jsonl")
    count = export_facts(models, FIXTURE_DIR / "task_facts.jsonl")
    print(f"task_registry.jsonl / task_facts.jsonl 已写出（{count} 条 case）")


if __name__ == "__main__":
    main()
