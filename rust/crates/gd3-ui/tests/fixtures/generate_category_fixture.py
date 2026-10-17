"""生成分类规则写路径的对比金文件（`category_rules.jsonl`，第 49 步）。

要钉住三件事：

1. `kind="dump"`：`CategoryService._save()` 的落盘串 =
   `cfg.set(cfg.categoryRules, [asdict(c) for c in categories])`，其中
   `categoryRules` 的 `JsonConfigSerializer.serialize` 是
   `json.dumps(value, ensure_ascii=False)`（默认分隔符 `", "` / `": "`），
   且 `QConfig.set` 先过 `CategoryListValidator.correct`。所以期望串是
   **过完 correct 之后** 的 `json.dumps`——键序即 dataclass 字段序
   （`categoryId/name/icon/extensions/folder`），`folder=None` → `null`。
2. `kind="correct"`：`CategoryListValidator.correct` 的整表口径
   （非 list / 项非 dict / 项 `name` 非 str → 整表 `[]`；其余原样）。
3. `kind="draft"`：`CategoryEditDialog.category()`（category_edit.py:93-110）
   ——**直接构造真对话框**（offscreen + 一个 parent QWidget，`MessageBoxBase`
   要求 parent 有尺寸），设控件值后调真方法，不抄算法。
   `extensions` 的输入给的是**原始文本**，脚本按 `,` 切分后 `setTokens`
   ——Slint 侧用逗号分隔串代替 `TokenLineEdit` 的逐 token 输入，切分口径
   在这里对齐。

注意：`category()` 里「显示名未改动则保留原文名」走的是
`toCategoryName()` = `QCoreApplication.translate`，生成脚本没有装
translator，`translate` 原样返回源串，故编辑态与新增态结果一致；
装了译文的运行期差异已在 Rust 侧 doc 注释记录。

    python3 rust/crates/gd3-ui/tests/fixtures/generate_category_fixture.py
"""

from __future__ import annotations

import json
import os
from dataclasses import asdict
from pathlib import Path

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

FIXTURE = Path(__file__).with_name("category_rules.jsonl")


def _app():
    from PySide6.QtWidgets import QApplication

    return QApplication.instance() or QApplication([])


def dump_cases() -> list[dict]:
    from app.config.cfg import CategoryListValidator
    from app.services.category_service import CATEGORY_PRESETS, Category

    validator = CategoryListValidator()

    def case(label: str, categories: list) -> dict:
        payload = [asdict(c) for c in categories]
        return {
            "kind": "dump",
            "label": label,
            "rules": payload,
            "text": json.dumps(validator.correct(payload), ensure_ascii=False),
        }

    # 八预设：与 `CategoryService._buildDefaults` 同一表达式；这里不构造
    # 服务实例，免得 `_load` 真的去读写 cfg。
    presets = [Category.fromDict(preset) for preset in CATEGORY_PRESETS]
    return [
        case("八预设", presets),
        case("空表", []),
        case(
            "转义与 Unicode",
            [
                Category(
                    categoryId='cat_"引号"',
                    name='前\\缀\t带"引号"',
                    icon="MUSIC",
                    extensions=["mp4", "tar.gz"],
                    folder="D:\\下载\\{default}\b\f",
                )
            ],
        ),
        case(
            "控制字符与行分隔符",
            [
                Category(
                    categoryId="cat_c",
                    name="\x00\x1f\u2028\u2029😀",
                    icon="DOCUMENT",
                    extensions=["a"],
                    folder=None,
                )
            ],
        ),
        # `Category.fromDict` 会把空串 folder 归一成 None；直接构造才保留
        # 空串形态——两种来源都留，写侧 `asdict` 分别落 `""` 与 `null`。
        case(
            "folder 空串形态",
            [
                Category(
                    categoryId="cat_e",
                    name="空 folder",
                    icon="DOCUMENT",
                    extensions=[],
                    folder="",
                )
            ],
        ),
    ]


def correct_cases() -> list[dict]:
    from app.config.cfg import CategoryListValidator

    validator = CategoryListValidator()
    inputs = [
        [],
        [{"name": "a"}],
        [{"name": "a", "icon": 5, "extensions": "nope"}],
        [{"name": 1}],
        [{"icon": "DOCUMENT"}],
        [{"name": "a"}, {"name": 2}],
        [{"name": "a"}, "not a dict"],
        "not a list",
        {"name": "a"},
        None,
        3,
    ]
    return [
        {"kind": "correct", "input": value, "output": validator.correct(value)}
        for value in inputs
    ]


DRAFTS: list[dict] = [
    {
        "label": "常规归一化",
        "id": "",
        "name": "  视频  ",
        "icon": "VIDEO",
        "extensions": ".MP4, mkv ,mkv,",
        "folder": "  {default}/V  ",
    },
    {
        "label": "空名空目录",
        "id": "",
        "name": "   ",
        "icon": "",
        "extensions": "",
        "folder": "   ",
    },
    {
        "label": "空 token 丢弃",
        "id": "cat_audio",
        "name": "音频",
        "icon": "MUSIC",
        "extensions": "mp3,,flac",
        "folder": "",
    },
    {
        "label": "信息分隔符算空白（CPython strip 口径）",
        "id": "",
        "name": "\x1c视频\x1f",
        "icon": "DOCUMENT",
        "extensions": "\x1cmp3\x1f, flac",
        "folder": "\x1c/default/A\x1f",
    },
    {
        "label": "去前导点与大小写",
        "id": "",
        "name": "  a.b  ",
        "icon": "CODE",
        "extensions": "...mp4, .MKV",
        "folder": "/abs/path",
    },
    {
        "label": "制表与换行也算空白",
        "id": "cat_t",
        "name": "\n\t视频\t\n",
        "icon": "DOCUMENT",
        "extensions": "\t srt \n",
        "folder": "\t{default}/S\t",
    },
    {
        "label": "未知图标回落 DOCUMENT",
        "id": "",
        "name": "未知图标",
        "icon": "NOT_AN_ICON",
        "extensions": "bin",
        "folder": "",
    },
]


def draft_cases() -> list[dict]:
    from PySide6.QtWidgets import QWidget

    from app.services.category_service import Category
    from app.view.dialogs.category_edit import CategoryEditDialog

    _app()
    parent = QWidget()
    parent.resize(800, 600)
    cases: list[dict] = []
    for spec in DRAFTS:
        existing = None
        if spec["id"]:
            # 编辑态：原名的两端无空白，`toCategoryName` 无 translator 时原样
            # 返回，所以「未改动保留原名」与「strip 后同名」同值，可与 Rust
            # 的纯 strip 口径直接对比。
            existing = Category(
                categoryId=spec["id"],
                name="原始名",
                icon="DOCUMENT",
                extensions=[],
                folder=None,
            )
        dialog = CategoryEditDialog(parent, category=existing)
        dialog.nameEdit.setText(spec["name"])
        dialog.extensionsEdit.setTokens(spec["extensions"].split(","))
        dialog.folderEdit.setText(spec["folder"])
        index = dialog.iconCombo.findData(spec["icon"])
        dialog.iconCombo.setCurrentIndex(index if index >= 0 else 0)
        category = dialog.category()
        assert category.categoryId == (spec["id"] or category.categoryId)
        cases.append(
            {
                "kind": "draft",
                "label": spec["label"],
                "id": spec["id"],
                "name": spec["name"],
                "icon": spec["icon"],
                "extensions": spec["extensions"],
                "folder": spec["folder"],
                "expected": {
                    "name": category.name,
                    "icon": category.icon,
                    "extensions": category.extensions,
                    "folder": category.folder,
                },
            }
        )
        dialog.deleteLater()
    parent.deleteLater()
    return cases


def main() -> None:
    lines = [
        json.dumps(case, ensure_ascii=False)
        for case in dump_cases() + correct_cases() + draft_cases()
    ]
    FIXTURE.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"写入 {FIXTURE}：{len(lines)} 条")


if __name__ == "__main__":
    main()
