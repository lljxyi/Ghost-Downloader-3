//! 状态归一与进度汇总的移植规格：镜像 `app/models/task.py` 的 `updateStatus` 家族。

use gd3_core::{Progress, TaskFile, TaskRecord, TaskStatus, TaskStep, load_records};
use serde_json::Map;
use std::path::PathBuf;

fn step(
    index: i64,
    file_index: Option<i64>,
    status: TaskStatus,
    received: i64,
    progress: Progress,
) -> TaskStep {
    TaskStep {
        step_index: index,
        file_index,
        step_id: format!("stp_{index:032}"),
        status,
        progress,
        received_bytes: received,
        speed: 0,
        extra: Map::new(),
        type_name: None,
    }
}

fn file(index: i64, selected: bool, size: i64) -> TaskFile {
    TaskFile {
        index,
        relative_path: format!("file{index}.bin"),
        size,
        selected,
        downloaded_bytes: 0,
        completed: false,
        extra: Map::new(),
    }
}

fn record(steps: Vec<TaskStep>, files: Option<Vec<TaskFile>>) -> TaskRecord {
    TaskRecord {
        name: "demo.bin".to_string(),
        url: "https://example.com/demo.bin".to_string(),
        pack_id: "http".to_string(),
        task_id: "tsk_test".to_string(),
        status: TaskStatus::Waiting,
        steps,
        created_at: 1_800_000_000,
        completed_at: 0,
        output_folder: "C:\\downloads".to_string(),
        file_size: 0,
        files,
        category: None,
        should_seed: false,
        checksums: Map::new(),
        extra: Map::new(),
        type_name: None,
    }
}

#[test]
fn loading_then_updating_status_normalizes_like_python() {
    // 金文件第 3 条：磁盘上 Task 是 WAITING、Step 是 PAUSED；Python 加载后 Task 变 PAUSED。
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("tasks.jsonl");
    let mut records = load_records(&fixture).expect("读取失败").records;

    assert_eq!(records[2].status, TaskStatus::Waiting, "读盘不改变状态");
    assert_eq!(
        records[2].update_status(),
        TaskStatus::Paused,
        "归一后与 Python 一致"
    );
}

#[test]
fn failed_step_wins_over_completed() {
    let mut task = record(
        vec![
            step(0, None, TaskStatus::Completed, 1024, Progress::Int(100)),
            step(1, None, TaskStatus::Failed, 0, Progress::Int(0)),
        ],
        None,
    );
    assert_eq!(task.update_status(), TaskStatus::Failed);
}

#[test]
fn completed_sets_completed_at_and_file_size() {
    let mut task = record(
        vec![step(
            0,
            None,
            TaskStatus::Completed,
            1024,
            Progress::Int(100),
        )],
        None,
    );
    assert_eq!(task.update_status(), TaskStatus::Completed);
    assert!(task.completed_at > 0, "完成时间应被补上");
    assert_eq!(task.file_size, 1024, "fileSize 为 0 时取实际收到的字节数");
}

#[test]
fn unselected_file_step_is_ignored() {
    let mut task = record(
        vec![step(0, Some(0), TaskStatus::Failed, 0, Progress::Int(0))],
        Some(vec![file(0, false, 100)]),
    );
    assert_eq!(
        task.update_status(),
        TaskStatus::Waiting,
        "取消选择的文件不参与状态推导"
    );
}

#[test]
fn files_aggregate_bytes_from_steps() {
    let mut task = record(
        vec![
            step(0, Some(0), TaskStatus::Completed, 100, Progress::Int(100)),
            step(1, Some(0), TaskStatus::Running, 200, Progress::Float(66.5)),
            step(2, Some(1), TaskStatus::Completed, 100, Progress::Int(100)),
        ],
        Some(vec![file(0, true, 300), file(1, true, 100)]),
    );

    assert_eq!(task.update_status(), TaskStatus::Running);

    let files = task.files.as_ref().expect("files 丢失");
    assert_eq!(files[0].downloaded_bytes, 300, "同一文件的多个 Step 累加");
    assert!(!files[0].completed, "有一个 Step 未完成即未完成");
    assert_eq!(files[1].downloaded_bytes, 100);
    assert!(files[1].completed);
}

#[test]
fn current_snapshot_averages_selected_steps() {
    let task = record(
        vec![
            step(0, Some(0), TaskStatus::Running, 300, Progress::Float(50.0)),
            step(1, Some(1), TaskStatus::Running, 100, Progress::Int(100)),
        ],
        Some(vec![file(0, true, 600), file(1, false, 100)]),
    );

    let (progress, speed, received) = task.current_snapshot();
    assert_eq!(progress, 50.0, "只统计选中文件的 Step");
    assert_eq!(speed, 0);
    assert_eq!(received, 300);
}

#[test]
fn set_status_running_resets_failed_step() {
    let mut task = record(
        vec![step(
            0,
            None,
            TaskStatus::Failed,
            500,
            Progress::Float(42.5),
        )],
        None,
    );

    let status = task.set_status(TaskStatus::Running);
    assert_eq!(status, TaskStatus::Running);
    // FAILED 的 Step 先被清零（丢掉上次失败留下的进度）再推进到目标状态
    assert_eq!(task.steps[0].status, TaskStatus::Running);
    assert_eq!(task.steps[0].progress, Progress::Int(0));
    assert_eq!(task.steps[0].received_bytes, 0);
}

#[test]
fn set_status_completed_finishes_steps() {
    let mut task = record(
        vec![step(
            0,
            None,
            TaskStatus::Paused,
            512,
            Progress::Float(12.5),
        )],
        None,
    );

    assert_eq!(
        task.set_status(TaskStatus::Completed),
        TaskStatus::Completed
    );
    assert_eq!(task.steps[0].progress, Progress::Int(100));
    assert!(task.completed_at > 0);
}

#[test]
fn reset_clears_progress_checksums_and_marks_seeding() {
    let mut task = record(
        vec![step(
            0,
            None,
            TaskStatus::Completed,
            1024,
            Progress::Int(100),
        )],
        None,
    );
    task.checksums
        .insert("sha256".to_string(), "e3b0".to_string().into());
    task.completed_at = 1_800_000_999;

    assert_eq!(task.reset(), TaskStatus::Waiting);
    assert_eq!(task.completed_at, 0);
    assert!(task.should_seed, "重新下载后默认继续做种");
    assert!(task.checksums.is_empty());
    assert_eq!(task.steps[0].progress, Progress::Int(0));
    assert_eq!(task.steps[0].received_bytes, 0);
}
