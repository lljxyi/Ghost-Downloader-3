//! 文件名工具与 Python 实现的逐字节对比。
//!
//! 金文件 `tests/fixtures/filename.jsonl` 由 `tests/fixtures/generate_filename_fixture.py`
//! 用 `app/platform/filesystem.py` 生成；Rust 侧只读，改实现就要同步改 Python 那边。

use gd3_core::filename::{split_stem_ext, to_safe_filename};
use serde_json::Value;

fn fixture_lines() -> Vec<Value> {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/filename.jsonl");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读金文件失败 {}：{error}", path.display()));
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("金文件行不是合法 JSON"))
        .collect()
}

#[test]
fn to_safe_filename_matches_python() {
    let cases = fixture_lines();
    let mut checked = 0;

    for case in &cases {
        if case["kind"] != "safe" {
            continue;
        }
        let name = case["name"].as_str().expect("name");
        let fallback = case["fallback"].as_str().expect("fallback");
        let expected = case["safe"].as_str().expect("safe");

        let actual = match case["maxLength"].as_u64() {
            Some(max_length) => to_safe_filename(name, fallback, max_length as usize),
            None => to_safe_filename(
                name,
                fallback,
                gd3_core::filename::DEFAULT_MAX_FILENAME_LENGTH,
            ),
        };

        assert_eq!(
            actual, expected,
            "to_safe_filename({name:?}, {fallback:?}, {:?}) 与 Python 不一致",
            case["maxLength"]
        );
        checked += 1;
    }

    assert!(checked >= 40, "金文件里的净化用例太少：{checked}");
}

#[test]
fn split_stem_ext_matches_python() {
    let cases = fixture_lines();
    let mut checked = 0;

    for case in &cases {
        if case["kind"] != "split" {
            continue;
        }
        let name = case["name"].as_str().expect("name");
        let (stem, ext) = split_stem_ext(name);

        assert_eq!(
            stem,
            case["stem"].as_str().expect("stem"),
            "主干不一致：{name:?}"
        );
        assert_eq!(
            ext,
            case["ext"].as_str().expect("ext"),
            "扩展名不一致：{name:?}"
        );
        checked += 1;
    }

    assert!(checked >= 15, "金文件里的拆解用例太少：{checked}");
}
