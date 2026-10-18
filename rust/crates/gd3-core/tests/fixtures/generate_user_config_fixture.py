"""生成 `UserConfig.json` 与 python JSON writer 的金文件（第 78 步）。

第 78 步推翻了第 49 步对磁盘格式的假设：qfluentwidgets 1.18 的
`ConfigItem.serialize()` 只是 `serializer.serialize(value)`，而
`RangeConfigItem`/`ConfigItem(BoolValidator)`/`ConfigItem(FolderValidator)`
**没有** serializer —— 落盘就是裸的 JSON 数字 / 布尔 / 数组，**不是**
`str(True)` 那样的字符串。真机实证（本脚本跑出的文件）：

    "EmulateFingerprint": false,
    "MaxTaskNum": 3,
    "HistoryDownloadFolder": [],
    "CategoryRules": "[]",          <- JsonConfigSerializer 才转字符串
    "Language": "Auto",             <- EnumSerializer
    "Geometry": "0,0,0,0",          <- GeometrySerializer

产出三个金文件：

- `user_config_default_linux.json`：空目录 `qconfig.load()` 后 `cfg.save()`
  的**逐字节**输出（Linux 平台分支：无 `backgroundEffect`、无 darwin 三键；
  `DownloadFolder` 取 `QStandardPaths.DownloadLocation` 经
  `FolderValidator.correct` 后的绝对路径）。
- `user_config_mutated_expected.json`：在上一份基础上按 `MUTATIONS` 赋值
  （走 `qconfig.set`，与 app 的写入路径一致）后 `save()` 的输出。
  **Rust 侧 `config.rs::tests::rust_mutation_matches_python_resave` 用同一组
  改动**，两侧逐字节比对。
- `py_json_writer.jsonl`：`json.dumps(value, ensure_ascii=False)`（紧凑）与
  `json.dumps(..., indent=4)`（落盘）两种口径的对照，钉住
  `py_json_dumps` / `UserConfig::to_json_text`。
- `py_float_repr.jsonl`：`repr(float)` 口径（python 在 |x| >= 1e16 或
  < 1e-4 时切指数写法、指数至少两位、整值补 `.0`），钉住 `py_float_repr`。

    QT_QPA_PLATFORM=offscreen .venv/bin/python \
        rust/crates/gd3-core/tests/fixtures/generate_user_config_fixture.py
"""

from __future__ import annotations

import json
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve()
ROOT = HERE.parents[5]
sys.path.insert(0, str(ROOT))

from qfluentwidgets import qconfig  # noqa: E402

from app.config.cfg import BASE_HEADERS, CloseMode, cfg  # noqa: E402



def fresh_default() -> str:
    """空目录 load（文件不存在 → 全缺省）后 save 的文本。"""
    tmp = Path(tempfile.mkdtemp()) / "UserConfig.json"
    qconfig.load(tmp, cfg)
    assert not tmp.exists(), "缺省路径不该写盘"
    cfg.save()
    return tmp.read_text(encoding="utf-8")


def mutated(default_text: str) -> str:
    # Rust 侧 `config.rs::tests::rust_mutation_matches_python_resave` 镜像下面这组改动。
    tmp = Path(tempfile.mkdtemp()) / "UserConfig.json"
    tmp.write_text(default_text, encoding="utf-8")
    qconfig.load(tmp, cfg)

    qconfig.set(cfg.isCategoryEnabled, True)
    qconfig.set(cfg.maxTaskNum, 5)
    qconfig.set(cfg.dpiScale, 0.25)
    qconfig.set(
        cfg.categoryRules,
        [{"name": "视频", "extensions": [".mp4", ".mkv"], "folder": "video"}],
    )
    qconfig.set(cfg.clientProfile, "manual")
    qconfig.set(cfg.closeMode, CloseMode.QUIT)
    qconfig.set(
        cfg.headersPresets,
        [
            {"name": "默认", "headers": dict(BASE_HEADERS)},
            {"name": "自定义", "headers": {"a": "b", "x-trace": "1"}},
        ],
    )
    qconfig.set(cfg.currentHeadersPreset, 1)
    qconfig.set(cfg.downloadFolder, "/tmp")
    qconfig.set(cfg.geometry, (10, 20, 800, 600))
    qconfig.set(cfg.proxyServer, "socks5://u:p@1.2.3.4:1080")
    qconfig.set(cfg.isSpeedLimitEnabled, True)
    qconfig.set(cfg.speedLimitation, 8388608)
    qconfig.set(cfg.memoryDownloadFolders, ["/tmp", "/var/tmp"])
    qconfig.set(cfg.expandedSettingGroups, ["download", "network"])

    return tmp.read_text(encoding="utf-8")


WRITER_CASES: list[object] = [
    {},
    [],
    {"a": 1},
    {"a": [1, 2, 3]},
    {"a": {"b": {"c": [True, False, None]}}},
    {"中文": "值", "emoji": "🎉"},
    {"quote": 'a"b', "backslash": "a\\b", "slash": "a/b"},
    {"ctrl": "a\u0001b\u001fc", "tab": "a\tb", "nl": "a\nb", "cr": "a\rb"},
    {"bs": "a\bb", "ff": "a\fb"},
    {"del": "\u007f", "ls": "\u2028", "ps": "\u2029"},
    {"empty_str": "", "empty_list": [], "empty_dict": {}},
    [[], {}, [[]]],
    {"nested": [{"k": [1, [2, [3]]]}]},
    {"float": 0.25, "int": 0, "neg": -17, "big": 104857600},
    {"float": 1.75, "whole": 3.0, "half": 0.5},
    {"dup_like": {"a": "b"}, "arr": ["x", "y"]},
    {"url": "http://a/1?b=2#c", "magnet": "magnet:?xt=1"},
    {"tabs": "\t", "spaces": "  "},
    ["单元素"],
    {"trailing": "x "},
]

FLOAT_CASES: list[float] = [
    0.0,
    -0.0,
    0.25,
    0.5,
    1.75,
    2.0,
    3.0,
    100.0,
    4194304.0,
    0.1,
    0.0001,
    0.00001,
    1e-5,
    1e15,
    1e16,
    1e300,
    5e-324,
    1.2345678901234567,
    -1234.5678,
    1 / 3,
]


def main() -> None:
    default_text = fresh_default()
    (HERE.parent / "user_config_default_linux.json").write_text(
        default_text, encoding="utf-8"
    )

    mutated_text = mutated(default_text)
    (HERE.parent / "user_config_mutated_expected.json").write_text(
        mutated_text, encoding="utf-8"
    )

    lines = []
    for value in WRITER_CASES:
        lines.append(
            json.dumps(
                {
                    "value": value,
                    "dumps": json.dumps(value, ensure_ascii=False),
                    "indent4": json.dumps(value, ensure_ascii=False, indent=4),
                },
                ensure_ascii=False,
            )
        )
    (HERE.parent / "py_json_writer.jsonl").write_text(
        "\n".join(lines) + "\n", encoding="utf-8"
    )

    lines = [
        json.dumps({"value": value, "repr": repr(value)}, ensure_ascii=False)
        for value in FLOAT_CASES
    ]
    (HERE.parent / "py_float_repr.jsonl").write_text(
        "\n".join(lines) + "\n", encoding="utf-8"
    )

    print(f"default={len(default_text)}B mutated={len(mutated_text)}B")
    print(f"writer_cases={len(WRITER_CASES)} float_cases={len(FLOAT_CASES)}")


if __name__ == "__main__":
    main()
