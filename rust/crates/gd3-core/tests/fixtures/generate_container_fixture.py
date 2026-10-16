"""用 Python 实现生成 fMP4 切片金文件，供 Rust 侧逐字节对比。

用法（仓库根目录）：uv run python rust/crates/gd3-core/tests/fixtures/generate_container_fixture.py
"""

from __future__ import annotations

import sys
import json
from pathlib import Path

def repo_root() -> Path:
    for parent in Path(__file__).resolve().parents:
        if (parent / "pyproject.toml").exists():
            return parent
    raise SystemExit("找不到仓库根目录")


ROOT = repo_root()
sys.path.insert(0, str(ROOT))

from app.container import buildMp4SegmentRange, parseMp4Boxes  # noqa: E402
FIXTURES = Path(__file__).parent
RANGES = [(60, 65), (0, 5), (726, 731), (0, 731), (120, 121)]


def main() -> None:
    header = (ROOT / "tests/fixtures/youtube_video_header.bin").read_bytes()

    golden = buildMp4SegmentRange(header, 60, 65)
    (FIXTURES / "mp4_patched_header_60_65.bin").write_bytes(golden.patchedHeader)

    segments = []
    for start, end in RANGES:
        segment = buildMp4SegmentRange(header, start, end)
        segments.append(
            {
                "start": start,
                "end": end,
                "segStart": segment.segStart,
                "segEnd": segment.segEnd,
                "segStartTime": segment.segStartTime,
                "segDuration": segment.segDuration,
                "patchedHeaderLength": len(segment.patchedHeader),
            }
        )

    boxes = parseMp4Boxes(header)
    (FIXTURES / "mp4_segments.json").write_text(
        json.dumps(
            {
                "boxes": {name: [offset, size] for name, (offset, size) in boxes.items()},
                "segments": segments,
            },
            ensure_ascii=False,
            indent=2,
        ),
        encoding="utf-8",
    )
    print(f"wrote mp4_patched_header_60_65.bin ({len(golden.patchedHeader)} bytes) 与 {len(segments)} 条区间")


if __name__ == "__main__":
    main()
