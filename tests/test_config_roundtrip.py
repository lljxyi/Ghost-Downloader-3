"""Phase 8c：`UserConfig.json` 的 Python ↔ Rust 往返（逐字节一致 + 改一项只动一行）。

完工判定 2 的验收：用 **Python 自己的写盘路径**（`QConfig.save` → `json.dump(indent=4)`）
写出一份配置 → 让 **Rust 前端**（`gd3-ui`，走 `settings_writer` / `ConfigStore`）
读进来再整文件回写 → 两侧字节一致；再用 `--set` 改一项，断言**只有那一行变**，
最后 Python 侧能读回新值。

这条测试同时钉住「单一写者」口径：Rust 回写时**不能**顺手改别的字段（比如把
`HistoryDownloadFolder` 重排、把空数组写成 `[]`、改缩进），也**不能**丢掉它不认识的
组 —— 第 130 步之前它确实丢过：`ConfigStore::rebuild` 照 `SPECS` 重建整份文件，
把 feature pack 的组（`Bilibili` / `BitTorrent` / `ED2k` / `FFmpeg` / `FTP` /
`GitHub` / `HuggingFace` / `M3U8` / `YtDlp`）整段删掉（真配置 163 行 → 66 行）。

所以 fixture 会把 `features/*/config.py` 全导一遍（`PackConfig.__init_subclass__`
把它们的 `ConfigItem` 挂到全局 `cfg` 上，`app/models/pack.py:62-71`），让写出的档
**与真机一致**，并且**与测试执行顺序无关** —— 否则只有「别的测试先导入过某个 pack」
时才会出现那些组（第 130 步就是这样在全量跑里红了）。
"""

from __future__ import annotations

import importlib
import subprocess
import sys
from pathlib import Path

import pytest
from qfluentwidgets import qconfig

from app.config.cfg import cfg
from app.platform.ui_backend import findRustUiBinary

CONFIG_NAME = "UserConfig.json"
ROOT = Path(__file__).parents[1]
PACK_GROUPS = (
    "Bilibili",
    "BitTorrent",
    "ED2k",
    "FFmpeg",
    "FTP",
    "GitHub",
    "HuggingFace",
    "M3U8",
    "YtDlp",
)


def rustBinary() -> Path:
    binary = findRustUiBinary()
    if binary is None:
        pytest.skip("没有构建 gd3-ui（cargo build -p gd3-ui）")
    return binary


def runRust(data_dir: Path, *extra: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [str(rustBinary()), "--headless", "--frames", "0", "--data-dir", str(data_dir), *extra],
        capture_output=True,
        text=True,
        timeout=180,
    )


def loadPackConfigs() -> None:
    """把 `features/*/config.py` 全导进来，让 `cfg` 里出现 feature pack 的组。

    真机上这件事发生在 `app/startup.py:65-78`（`importPackConfigs` + `PackConfig.load`）；
    测试里没有那条启动路径，所以要自己导，否则写出的档只有 9 个基础组。
    """
    for path in sorted((ROOT / "features").glob("*/config.py")):
        importlib.import_module(f"features.{path.parent.name}.config")


def changedLines(before: str, after: str) -> list[tuple[str, str]]:
    """逐行比（行数必须一样，否则说明整文件被重排了）。"""
    old = before.splitlines()
    new = after.splitlines()
    assert len(old) == len(new), f"行数变了：{len(old)} → {len(new)}"
    return [(a, b) for a, b in zip(old, new, strict=True) if a != b]


@pytest.fixture
def python_written_config(tmp_path, qapp, monkeypatch):
    """用 Python 的写盘路径产出一份 `UserConfig.json`（不写进真实数据目录）。"""
    if str(ROOT) not in sys.path:
        sys.path.insert(0, str(ROOT))
    loadPackConfigs()

    path = tmp_path / CONFIG_NAME
    qconfig.load(str(path), cfg)
    # 令牌非空：Rust 侧只在为空时才生成（`browser_service.py:190`），
    # 否则这条测试会被那一次「正常」的生成干扰。
    monkeypatch.setattr(cfg.browserExtensionPairToken, "value", "roundtrip-token")
    monkeypatch.setattr(cfg.maxTaskNum, "value", 3)
    monkeypatch.setattr(cfg.downloadFolder, "value", str(tmp_path / "dl"))
    qconfig.save()
    assert path.exists(), "Python 侧没写出配置"
    return path


def test_the_python_side_writes_the_pack_groups(python_written_config):
    """先确认 fixture 自己没退化：写出的档里确实有 feature pack 的组。"""
    import json

    data = json.loads(python_written_config.read_text(encoding="utf-8"))

    missing = [group for group in PACK_GROUPS if group not in data]
    assert not missing, f"fixture 少了这些组：{missing}"


def test_rust_rewrites_a_python_written_config_byte_for_byte(python_written_config):
    before = python_written_config.read_text(encoding="utf-8")

    completed = runRust(python_written_config.parent)

    assert completed.returncode == 0, completed.stderr
    after = python_written_config.read_text(encoding="utf-8")
    assert after == before, changedLines(before, after)


def test_set_changes_exactly_one_line(python_written_config):
    before = python_written_config.read_text(encoding="utf-8")

    completed = runRust(python_written_config.parent, "--set", "maxTaskNum=7")

    assert completed.returncode == 0, completed.stderr
    after = python_written_config.read_text(encoding="utf-8")
    assert changedLines(before, after) == [('        "MaxTaskNum": 3,', '        "MaxTaskNum": 7,')]


def test_rust_does_not_drop_the_pack_groups(python_written_config):
    """`--set` 之后，它不认识的组要一字不动（第 130 步的回归点）。"""
    import json

    before = json.loads(python_written_config.read_text(encoding="utf-8"))

    completed = runRust(python_written_config.parent, "--set", "maxTaskNum=7")

    assert completed.returncode == 0, completed.stderr
    after = json.loads(python_written_config.read_text(encoding="utf-8"))

    assert list(after) == list(before), "组的顺序被改了"
    for group in PACK_GROUPS:
        assert after[group] == before[group], f"{group} 组被改了"


def test_python_reads_the_value_rust_wrote_back(python_written_config):
    completed = runRust(python_written_config.parent, "--set", "maxTaskNum=7")
    assert completed.returncode == 0, completed.stderr

    qconfig.load(str(python_written_config), cfg)

    assert cfg.maxTaskNum.value == 7
    # 其他项一个都不能被顺手改掉。
    assert cfg.browserExtensionPairToken.value == "roundtrip-token"
    assert cfg.downloadFolder.value == str(python_written_config.parent / "dl")


def test_rust_applies_a_bare_string_and_a_json_value(python_written_config):
    completed = runRust(
        python_written_config.parent,
        "--set",
        "clientProfile=chrome",
        "--set",
        "autoSpeedUp=false",
    )
    assert completed.returncode == 0, completed.stderr

    qconfig.load(str(python_written_config), cfg)

    assert cfg.clientProfile.value == "chrome", "裸写当字符串"
    assert cfg.autoSpeedUp.value is False, "`false` 按 JSON 解析成布尔"
