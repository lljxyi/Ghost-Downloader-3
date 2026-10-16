"""用 Python 实现生成 tasks.jsonl 金文件，供 Rust 侧做逐字节对比。

用法（仓库根目录）：uv run python rust/crates/gd3-core/tests/fixtures/generate_fixture.py
"""

from __future__ import annotations

import json
import os
import sys
from dataclasses import dataclass, field
from pathlib import Path

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")


def repo_root() -> Path:
    for parent in Path(__file__).resolve().parents:
        if (parent / "pyproject.toml").exists():
            return parent
    raise SystemExit("找不到仓库根目录")


ROOT = repo_root()
sys.path.insert(0, str(ROOT))

from app.models.serialization import toDict  # noqa: E402
from app.models.task import Task, TaskFile, TaskStatus, TaskStep  # noqa: E402


@dataclass(kw_only=True)
class DemoStep(TaskStep):
    """模拟 FeaturePack 自己的 Step 子类：额外字段会写进 JSON 并带 type 标签。"""

    partIndex: int = 0


@dataclass(kw_only=True, eq=False)
class DemoTask(Task):
    """模拟 FeaturePack 自己的 Task 子类。"""

    demoFlag: bool = False


def build_records() -> list[Task]:
    plain = Task(name="ubuntu.iso", url="https://example.com/ubuntu.iso", packId="http")
    plain.steps.append(
        TaskStep(stepIndex=0, fileIndex=None, stepId="stp_0123456789abcdef0123456789abcdef")
    )

    multi = DemoTask(
        name="示例合集 1080P",
        url="magnet:?xt=urn:btih:0123456789abcdef",
        packId="bittorrent",
        taskId="tsk_aaaabbbbccccddddeeeeffff00001111",
        status=TaskStatus.COMPLETED,
        createdAt=1792000000,
        completedAt=1792003600,
        outputFolder="D:\\下载",
        fileSize=4294967296,
        files=[
            TaskFile(index=0, relativePath="第 1 集.mkv", size=2147483648),
            TaskFile(index=1, relativePath="第 2 集.mkv", size=2147483648, selected=False),
        ],
        category="video",
        shouldSeed=True,
        checksums={"sha256": "e3b0c44298fc1c149afbf4c8996fb924"},
        demoFlag=True,
    )
    multi.steps.extend(
        [
            DemoStep(
                stepIndex=0,
                fileIndex=0,
                stepId="stp_11111111111111111111111111111111",
                status=TaskStatus.COMPLETED,
                progress=100,
                receivedBytes=2147483648,
                speed=0,
                partIndex=0,
            ),
            DemoStep(
                stepIndex=1,
                fileIndex=1,
                stepId="stp_22222222222222222222222222222222",
                status=TaskStatus.RUNNING,
                progress=42.5,
                receivedBytes=913571840,
                speed=1048576,
                partIndex=7,
            ),
        ]
    )

    fresh = Task(name="report.pdf", url="ftp://example.com/report.pdf", packId="ftp")
    fresh.steps.append(
        TaskStep(
            stepIndex=0,
            fileIndex=None,
            stepId="stp_33333333333333333333333333333333",
            status=TaskStatus.PAUSED,
            progress=0.0,
            receivedBytes=0,
            speed=0,
        )
    )
    return [plain, multi, fresh]


def main() -> None:
    lines = [json.dumps(toDict(task), ensure_ascii=False) + "\n" for task in build_records()]
    target = Path(__file__).with_name("tasks.jsonl")
    target.write_text("".join(lines), encoding="utf-8")
    print(f"wrote {target} ({target.stat().st_size} bytes, {len(lines)} records)")


if __name__ == "__main__":
    main()
