"""生成 ed2k `parseProgress` 的对比金文件（`ed2k_progress.jsonl`）。

规格来自 `features/ed2k_pack/kelpie/kelpie.py::parseProgress`：
Python 的关键字求值顺序（10 个普通字段先于 `sources`）、dict 迭代得键、
str 迭代得字符、CPython 3.13 的下标错误文案联手决定行为，
Rust 影子分支（`GD3_RUST_ED2K_PROGRESS`）要逐条还原。

每条语料一个 JSON 行：

- 成功路径：`{"message": …, "repr": "Progress(hash=…, …)"}`
  （原实现返回值的 `repr`，含类型与引号形态，消歧 1 vs 1.0 / None)；
- 失败路径：`{"message": …, "error": {"type": "KeyError", "str": "'sources'"}}`。

运行：`python3 rust/crates/gd3-core/tests/fixtures/generate_ed2k_progress_fixture.py`
（需要项目 venv 或同 Python 版本环境，用到 `features.ed2k_pack`）。
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[5]))

from features.ed2k_pack.kelpie.kelpie import parseProgress as nativeParseProgress  # noqa: E402

FIXTURE = Path(__file__).with_name("ed2k_progress.jsonl")

GOOD_HASH = "0123456789abcdef0123456789abcdef"


def source(address: str = "1.2.3.4:4662", software: str = "kelpie", status: str = "active",
           rank: int = 7, downloadRate=5, channel: str = "tcp") -> dict:
    return dict(address=address, software=software, status=status,
                rank=rank, downloadRate=downloadRate, channel=channel)


def message(**override) -> dict:
    base = dict(
        hash=GOOD_HASH, size=1024, received=512, downloadRate=1.5, uploadRate=0.0,
        uploaded=42, peers=7, activePeers=5, heldSources=0, heldUntil=None,
        sources=[source()],
    )
    base.update(override)
    return base


def record(message_, ok_or_error):
    if isinstance(ok_or_error, BaseException):
        return {"message": message_, "error": {"type": type(ok_or_error).__name__, "str": str(ok_or_error)}}
    return {"message": message_, "repr": repr(ok_or_error)}


# 约束：语料值必须能被 JSON 值模型表达（serde_json 无 NaN/Infinity）。
# 真实线上不可达：kelpie 引擎侧用严格 JSON 序列化（serde 遇 NaN 直接报错），
# progress 的 rate 字段实际均为 int。
CASES: list[dict] = [
    # 成功路径
    message(),
    message(sources=[]),
    message(sources=[source(address="ÄÖÜ:4662", software="eMule", status="connecting")]),
    message(sources=[source(), source(address="5.6.7.8:4661", software="kelpie")]),  # 重复地址
    message(sources=[source(downloadRate=0.125)]),
    message(size=2 ** 53 + 1, received=0),  # 大整数（u64 内）
    message(peers=True, activePeers=False),  # bool 透传
    message(peers="7", hash=GOOD_HASH),  # 字符串字段原样透传（Python 对出现即用）
    message(downloadRate=1056 / 3),  # 无限小数 float
    message(uploadRate=-0.0, received=-1),  # -0.0 / 负号
    message(heldUntil=0),
    message(sources=[dict(source(), status="high" > "low")]),  # bool status 透传
    message(
        unknownField={"x": [1, 2]},  # 消息多出的字段被无视
        sources=[dict(source(), extra=None)],
    ),
    # 成功路径：sources 空退路（零循环）
    message(sources=""),
    message(sources={}),
    # 字段阶段先于 sources 阶段（关键字求值顺序，报第一个缺失字段）
    {"hash": GOOD_HASH},
    {},
    {"hash": GOOD_HASH, "size": 1},
    {k: v for k, v in message().items() if k != "received"},
    # sources 缺失：10 字段齐全之后才暴露
    {k: v for k, v in message().items() if k != "sources"},
    # source 缺字段（按 SOURCE_FIELDS 顺序报第一个）
    message(sources=[{k: v for k, v in source().items() if k != "address"}]),
    message(sources=[{k: v for k, v in source().items() if k != "software"}]),
    message(sources=[{k: v for k, v in source().items() if k != "rank"}]),
    message(sources=[{k: v for k, v in source().items() if k != "channel"}]),
    # source 不是 dict：下标错误文案
    message(sources=[1]),
    message(sources=[None]),
    message(sources=[True]),
    message(sources=[1.5]),
    message(sources=["ab"]),
    message(sources=[[1]]),
    message(sources=[[1, 2], 3]),  # 第二个元素才坏
    message(sources=["abc"]),  # 非空字符串：字符下标
    # sources 不是序列：迭代错误
    message(sources=5),
    message(sources=True),
    message(sources=None),
    message(sources={"a": 1}),  # dict 迭代得键，键下标
    # message 根值不是 dict
    [1, "2"],
    "plain text",
    99,
    None,
]


def main() -> None:
    assert os.environ.get("GD3_RUST_ED2K_PROGRESS") != "1", "生成金文件必须用 Python 参考实现"
    lines = []
    for case in CASES:
        try:
            native = nativeParseProgress(case)
            lines.append(json.dumps(record(case, native), ensure_ascii=False))
        except Exception as error:  # noqa: BLE001
            lines.append(json.dumps(record(case, error), ensure_ascii=False))
    FIXTURE.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"{len(lines)} 条 → {FIXTURE}")


if __name__ == "__main__":
    main()
