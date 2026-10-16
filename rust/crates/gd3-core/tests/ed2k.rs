//! ed2k `Link.parse` 与 Python 实现的逐条对比。
//!
//! 金文件 `tests/fixtures/ed2k.jsonl` 由 `tests/fixtures/generate_ed2k_fixture.py`
//! 用 `features.ed2k_pack.kelpie.models.Link.parse` 生成；Rust 侧只读。

use gd3_core::ed2k::{MAX_SIZE, parse};
use serde_json::Value;

fn fixture() -> Vec<Value> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ed2k.jsonl");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读金文件失败 {}：{error}", path.display()));
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("金文件行不是合法 JSON"))
        .collect()
}

#[test]
fn link_parse_matches_python() {
    let cases = fixture();
    let mut checked_ok = 0;
    let mut checked_error = 0;

    for case in &cases {
        let text = case["text"].as_str().expect("text");
        let actual = parse(text);

        if let Some(expected_error) = case.get("error") {
            let error = actual.expect_err("Python 侧解析失败，Rust 侧也应失败");
            assert_eq!(
                format!("INVALID_LINK: {}", error.message),
                expected_error.as_str().expect("error 是字符串"),
                "{text:?} 的错误文本与 Python 不一致"
            );
            checked_error += 1;
            continue;
        }

        let link = actual.expect("Python 侧解析成功，Rust 侧也应成功");
        assert_eq!(link.name, case["name"].as_str().expect("name"), "{text:?}");
        assert_eq!(link.size, case["size"].as_u64().expect("size"), "{text:?}");
        assert_eq!(link.hash, case["hash"].as_str().expect("hash"), "{text:?}");
        checked_ok += 1;
    }

    assert!(checked_ok >= 20, "金文件里的成功用例太少：{checked_ok}");
    assert!(
        checked_error >= 20,
        "金文件里的失败用例太少：{checked_error}"
    );
    assert_eq!(checked_ok + checked_error, cases.len());
}

#[test]
fn max_size_matches_emule_limit() {
    assert_eq!(MAX_SIZE, 274_877_906_944);
}
