"""文件名工具的行为规格。

这些断言同时是**开关两侧的规格**：`GD3_RUST_FILENAME=1` 时同一批断言跑的是
`gd3-core` 的实现，所以两种模式必须给出完全一样的结果。

金文件 `rust/crates/gd3-core/tests/fixtures/filename.jsonl` 是 Python 生成的、
Rust 侧逐字节对比的那一份；这里挑的是同一批语义里最容易踩坏的那些。
"""

from __future__ import annotations

import pytest

from app.platform.filesystem import splitStemExt, toSafeFilename


class TestToSafeFilename:
    def test_ordinary_names_are_untouched(self):
        assert toSafeFilename("a.bin") == "a.bin"
        assert toSafeFilename("视频 名字.mp4") == "视频 名字.mp4"
        assert toSafeFilename("a😀b.mp4") == "a😀b.mp4"

    @pytest.mark.parametrize("name", ["", "   ", ".", "..", "...", "\t\n "])
    def test_degenerate_names_fall_back(self, name):
        # 注意 "\t\n " 不是退化的：控制字符先被折成 "_"，所以留下的是 "_"
        if name == "\t\n ":
            assert toSafeFilename(name, fallback="兜底") == "_"
        else:
            assert toSafeFilename(name, fallback="兜底") == "兜底"

    def test_invalid_characters_collapse_into_one_underscore(self):
        assert toSafeFilename('a/b\\c:d*e?f"g<h>i|j') == "a_b_c_d_e_f_g_h_i_j"
        assert toSafeFilename("a\x00b\x1fc\x7fd") == "a_b_c_d"
        assert toSafeFilename('<>:"/\\|?*') == "_"

    def test_trailing_dots_and_spaces_are_stripped(self):
        assert toSafeFilename("name.") == "name"
        assert toSafeFilename("name. ") == "name"
        assert toSafeFilename("name . . ") == "name"
        assert toSafeFilename("  name  ") == "name"

    @pytest.mark.parametrize("name, expected", [
        ("CON", "_CON"),
        ("con", "_con"),
        ("Con.txt", "_Con.txt"),
        ("COM9.log", "_COM9.log"),
        ("COM10.log", "COM10.log"),   # COM10 不是保留名
        ("NUL.tar.gz", "_NUL.tar.gz"),
        ("PRN", "_PRN"),
    ])
    def test_windows_reserved_names_get_a_prefix(self, name, expected):
        assert toSafeFilename(name) == expected

    @pytest.mark.parametrize("name, maxLength, expected", [
        ("abcdef.gh", 5, "ab.gh"),     # 保住扩展名，主干缩到 2
        ("abcdef.gh", 4, "a.gh"),
        ("abcdef.gh", 3, "abc"),       # 扩展名都放不下，直接截断
        ("abcdef", 3, "abc"),
        ("abcdef.gh", 0, "abcdef.gh"),  # 0 表示不截断
        ("abcdef.gh", -1, "abcdef.gh"),
        ("名字名字名字.mkv", 6, "名字.mkv"),  # 按码点算，不是字节
        ("a.b", 2, "a."),
        ("a.b", 1, "a"),
    ])
    def test_truncation_keeps_the_extension_when_it_can(self, name, maxLength, expected):
        assert toSafeFilename(name, maxLength=maxLength) == expected

    def test_long_name_is_cut_to_the_limit(self):
        assert len(toSafeFilename("a" * 300 + ".mp4")) == 200
        assert toSafeFilename("a" * 300 + ".mp4").endswith(".mp4")
        assert len(toSafeFilename("a" * 300)) == 200


class TestSplitStemExt:
    @pytest.mark.parametrize("name, stem, ext", [
        ("a.tar.gz", "a", ".tar.gz"),
        ("a.tar.bz2", "a", ".tar.bz2"),
        ("a.tar.xz", "a", ".tar.xz"),
        ("a.tar.zst", "a", ".tar.zst"),
        ("a.TAR.GZ", "a", ".TAR.GZ"),
        ("archive.001.gz", "archive", ".001.gz"),
        ("a.mp4.gz", "a", ".mp4.gz"),
        ("a.tar.bz", "a", ".tar.bz"),
        ("a.tar.7z", "a.tar", ".7z"),        # 7z 不是复合扩展名的一部分
        ("a.tar.gz.asc", "a.tar.gz", ".asc"),  # 结尾不是归档扩展名就不算复合
        ("视频.tar.gz", "视频", ".tar.gz"),
    ])
    def test_compound_extensions_stay_whole(self, name, stem, ext):
        assert splitStemExt(name) == (stem, ext)

    @pytest.mark.parametrize("name, stem, ext", [
        ("a.bin", "a", ".bin"),
        ("a.b.c", "a.b", ".c"),
        ("noext", "noext", ""),
        (".hidden", ".hidden", ""),
        ("a.", "a", "."),
        ("", "", ""),
    ])
    def test_plain_extensions(self, name, stem, ext):
        assert splitStemExt(name) == (stem, ext)
