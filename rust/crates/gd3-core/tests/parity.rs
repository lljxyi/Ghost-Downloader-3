//! 逐字节对比：Rust 读出的 tasks.jsonl 必须能原样写回。
//!
//! 金文件由同目录的 `generate_fixture.py` 用 Python 实现生成，
//! 覆盖基础 Task、FeaturePack 风格的子类（额外字段 + `type` 标签）与 int/float 两种进度写法。

use gd3_core::{Progress, TaskStatus, load_records, save_records};
use serde_json::Value;
use std::path::PathBuf;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("tasks.jsonl")
}

#[test]
fn round_trips_python_tasks_jsonl_byte_for_byte() {
    let fixture = fixture_path();
    let expected =
        std::fs::read_to_string(&fixture).expect("读不到金文件，先跑 generate_fixture.py");
    let outcome = load_records(&fixture).expect("读取失败");
    assert!(
        outcome.failures.is_empty(),
        "解析失败：{:?}",
        outcome.failures
    );
    assert_eq!(outcome.records.len(), 3);

    let temp = std::env::temp_dir().join("gd3-parity-tasks.jsonl");
    save_records(&temp, &outcome.records).expect("写入失败");
    let written = std::fs::read_to_string(&temp).expect("读回失败");

    // 行尾由平台决定（Python 文本模式写盘，见 `store.rs` 的 `LINE_ENDING`），
    // 而金文件可能是另一个平台生成的——所以这里分开断言两件事：
    //   1. 写回的行尾统一、且与本平台一致；
    //   2. 逐行内容与金文件逐字节相同。
    // 直接比整串的话，同一份金文件换到另一个平台就红（Windows 生成的是 CRLF）。
    let platform_ending = if cfg!(windows) { "\r\n" } else { "\n" };
    let stripped = written.replace(platform_ending, "");
    assert!(
        !stripped.contains('\r') && !stripped.contains('\n'),
        "写回的行尾必须统一是 {platform_ending:?}"
    );

    assert_eq!(
        split_lines(&expected),
        split_lines(&written),
        "写回的 tasks.jsonl 逐行与 Python 金文件不一致"
    );
}

/// 按行拆开并去掉行尾，用于跨平台比较内容（金文件可能来自另一个平台）。
fn split_lines(text: &str) -> Vec<&str> {
    text.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
}

#[test]
fn keeps_pack_fields_type_tag_and_status() {
    let outcome = load_records(&fixture_path()).expect("读取失败");
    let records = outcome.records;

    let plain = &records[0];
    assert_eq!(plain.status, TaskStatus::Waiting);
    assert!(plain.files.is_none());
    assert!(plain.category.is_none());
    assert!(plain.type_name.is_none());
    assert_eq!(plain.steps[0].progress, Progress::Int(0));
    assert_eq!(plain.steps[0].status, TaskStatus::Waiting);

    let multi = &records[1];
    assert_eq!(multi.type_name.as_deref(), Some("__main__.DemoTask"));
    assert_eq!(multi.status, TaskStatus::Completed);
    assert_eq!(multi.extra.get("demoFlag"), Some(&Value::Bool(true)));
    assert_eq!(
        multi.checksums.get("sha256"),
        Some(&Value::String(
            "e3b0c44298fc1c149afbf4c8996fb924".to_string()
        ))
    );
    assert_eq!(multi.output_folder, "D:\\下载");
    assert!(multi.should_seed);

    let files = multi.files.as_ref().expect("files 丢失");
    assert_eq!(files.len(), 2);
    assert!(files[0].selected);
    assert!(!files[1].selected);
    assert_eq!(files[1].relative_path, "第 2 集.mkv");

    assert_eq!(multi.steps[0].progress, Progress::Int(100));
    assert_eq!(multi.steps[1].progress, Progress::Float(42.5));
    assert_eq!(multi.steps[1].status, TaskStatus::Running);
    assert_eq!(
        multi.steps[1].type_name.as_deref(),
        Some("__main__.DemoStep")
    );
    assert_eq!(
        multi.steps[1].extra.get("partIndex"),
        Some(&Value::Number(7.into()))
    );
    assert_eq!(multi.steps[1].file_index, Some(1));

    let fresh = &records[2];
    assert_eq!(fresh.steps[0].status, TaskStatus::Paused);
    assert_eq!(fresh.steps[0].progress, Progress::Float(0.0));
    assert_eq!(fresh.steps[0].progress.percent(), 0.0);
}
