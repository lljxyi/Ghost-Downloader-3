//! 分片划分规格：与 Python `_buildSubworkers` / `_splitSlowest` 的金文件对比。
//!
//! 金文件由 `tests/fixtures/generate_split_fixture.py` 用真实 Python 实现生成。

use gd3_net::{Subworker, split_ranges, split_slowest, top_up_subworkers};
use serde_json::Value;
use std::path::PathBuf;

fn golden() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("subworker_splits.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读不到 {}：{error}", path.display()));
    serde_json::from_str(&text).expect("解析金文件失败")
}

fn as_tuples(subworkers: &[Subworker]) -> Vec<(usize, u64, i64)> {
    subworkers
        .iter()
        .map(|subworker| (subworker.index, subworker.start, subworker.end))
        .collect()
}

fn from_json(value: &Value) -> Vec<(usize, u64, i64)> {
    value
        .as_array()
        .expect("期望数组")
        .iter()
        .map(|subworker| {
            (
                subworker["index"].as_u64().expect("index") as usize,
                subworker["start"].as_u64().expect("start"),
                subworker["end"].as_i64().expect("end"),
            )
        })
        .collect()
}

#[test]
fn split_ranges_matches_python() {
    for case in golden()["splits"].as_array().expect("缺 splits") {
        let file_size = case["fileSize"].as_u64().expect("fileSize");
        let count = case["subworkerCount"].as_u64().expect("subworkerCount") as usize;
        let can_use_range = case["canUseRangeRequests"]
            .as_bool()
            .expect("canUseRangeRequests");

        assert_eq!(
            as_tuples(&split_ranges(file_size, count, can_use_range)),
            from_json(&case["subworkers"]),
            "fileSize={file_size} count={count} range={can_use_range}"
        );
    }
}

#[test]
fn split_slowest_matches_python() {
    for case in golden()["splitSlowest"]
        .as_array()
        .expect("缺 splitSlowest")
    {
        let file_size = case["fileSize"].as_u64().expect("fileSize");
        let count = case["subworkerCount"].as_u64().expect("subworkerCount") as usize;
        let mut subworkers = split_ranges(file_size, count, true);

        for state in case["states"].as_array().expect("缺 states") {
            let new_subworker = split_slowest(&mut subworkers);
            let expected = &state["newSubworker"];

            if expected.is_null() {
                assert!(new_subworker.is_none(), "Python 没切，Rust 却切了");
                continue;
            }

            let actual = new_subworker.expect("Python 切了，Rust 没切");
            assert_eq!(
                (actual.index, actual.start, actual.end),
                (
                    expected["index"].as_u64().expect("index") as usize,
                    expected["start"].as_u64().expect("start"),
                    expected["end"].as_i64().expect("end"),
                )
            );
            assert_eq!(
                as_tuples(&subworkers),
                from_json(&state["subworkers"]),
                "切完之后的分片列表不一致"
            );
        }
    }
}

#[test]
fn top_up_subworkers_matches_python() {
    for case in golden()["topUp"].as_array().expect("缺 topUp") {
        let target = case["target"].as_u64().expect("target") as usize;
        let file_size = case["fileSize"].as_u64().expect("fileSize");

        let mut subworkers: Vec<Subworker> = case["restored"]
            .as_array()
            .expect("缺 restored")
            .iter()
            .map(|subworker| Subworker {
                index: subworker["index"].as_u64().expect("index") as usize,
                start: subworker["start"].as_u64().expect("start"),
                position: subworker["position"].as_u64().expect("position"),
                end: subworker["end"].as_i64().expect("end"),
            })
            .collect();

        top_up_subworkers(&mut subworkers, file_size, target);
        assert_eq!(
            as_tuples(&subworkers),
            from_json(&case["subworkers"]),
            "补齐后的分片列表与 Python 不一致"
        );
    }
}

#[test]
fn split_ranges_covers_file_without_overlap() {
    let subworkers = split_ranges(1000, 8, true);
    let mut covered = 0u64;
    for (index, subworker) in subworkers.iter().enumerate() {
        assert_eq!(subworker.index, index);
        assert!(subworker.start <= subworker.end as u64);
        assert_eq!(subworker.position, subworker.start);
        covered += subworker.end as u64 - subworker.start + 1;
    }
    assert_eq!(covered, 1000, "分片必须无缝覆盖整个文件");
}
