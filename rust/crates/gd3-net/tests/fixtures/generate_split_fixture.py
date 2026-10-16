"""用 Python 实现生成分片划分金文件，供 Rust 侧对比。

用法（仓库根目录）：uv run python rust/crates/gd3-net/tests/fixtures/generate_split_fixture.py
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")


def repo_root() -> Path:
    for parent in Path(__file__).resolve().parents:
        if (parent / "pyproject.toml").exists():
            return parent
    raise SystemExit("找不到仓库根目录")


ROOT = repo_root()
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / "features"))

from http_pack.task import HttpSubworker, HttpTaskStep  # noqa: E402

SPLIT_CASES = [
    (1000, 2, True),
    (1000, 3, True),
    (1000, 8, True),
    (1, 8, True),
    (0, 8, True),
    (100, 1, True),
    (7, 3, True),
    (999, 4, True),
    (1_000_000_000, 16, True),
    (100, 8, False),
]

SPLIT_SLOWEST_CASES = [(1_000_000, 2, 1), (1_000_000, 2, 2), (1_000_000, 3, 3), (17, 2, 1)]

TOP_UP_CASES = [
    # (恢复出来的分片, 目标线程数, 文件大小)
    ([(0, 0, 100_000), (100_000, 100_000, 999_999)], 4, 1_000_000),
    ([(0, 0, 499_999), (500_000, 500_000, 999_999)], 8, 1_000_000),
    ([(0, 250_000, 499_999), (500_000, 500_000, 999_999)], 4, 1_000_000),
]


def dumpSubworkers(subworkers: list[HttpSubworker]) -> list[dict]:
    return [
        {"index": sw.index, "start": sw.start, "end": sw.end, "position": sw.position}
        for sw in subworkers
    ]


def main() -> None:
    splits = []
    for file_size, subworker_count, can_use_range in SPLIT_CASES:
        step = HttpTaskStep(
            stepIndex=0,
            fileSize=file_size,
            subworkerCount=subworker_count,
            canUseRangeRequests=can_use_range,
        )
        splits.append(
            {
                "fileSize": file_size,
                "subworkerCount": subworker_count,
                "canUseRangeRequests": can_use_range,
                "subworkers": dumpSubworkers(step._buildSubworkers()),
            }
        )

    split_slowest = []
    for file_size, subworker_count, times in SPLIT_SLOWEST_CASES:
        step = HttpTaskStep(
            stepIndex=0, fileSize=file_size, subworkerCount=subworker_count, canUseRangeRequests=True
        )
        step.subworkers = step._buildSubworkers()
        states = []
        for _ in range(times):
            new_subworker = step._splitSlowest()
            states.append(
                {
                    "newSubworker": None if new_subworker is None else dumpSubworkers([new_subworker])[0],
                    "subworkers": dumpSubworkers(step.subworkers),
                }
            )
        split_slowest.append(
            {
                "fileSize": file_size,
                "subworkerCount": subworker_count,
                "times": times,
                "states": states,
            }
        )

    top_up = []
    for restored, target, file_size in TOP_UP_CASES:
        step = HttpTaskStep(
            stepIndex=0, fileSize=file_size, subworkerCount=target, canUseRangeRequests=True
        )
        step.subworkers = [
            HttpSubworker(index=index, start=start, end=end, receivedBytes=position - start)
            for index, (start, position, end) in enumerate(restored)
        ]
        while len(step.subworkers) < min(target, file_size):
            if not step._splitSlowest():
                break
        top_up.append(
            {
                "restored": [
                    {"index": index, "start": start, "position": position, "end": end}
                    for index, (start, position, end) in enumerate(restored)
                ],
                "target": target,
                "fileSize": file_size,
                "subworkers": dumpSubworkers(step.subworkers),
            }
        )

    target_path = Path(__file__).with_name("subworker_splits.json")
    target_path.write_text(
        json.dumps(
            {"splits": splits, "splitSlowest": split_slowest, "topUp": top_up},
            ensure_ascii=False,
            indent=2,
        ),
        encoding="utf-8",
    )
    print(f"wrote {target_path} ({len(splits)} splits, {len(split_slowest)} splitSlowest, {len(top_up)} topUp)")


if __name__ == "__main__":
    main()
