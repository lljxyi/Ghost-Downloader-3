//! Phase 8e 的端到端验收：真起本地 HTTP 服务 → 派发任务 → 逐字节校验。
//!
//! 这里用的是**生产实现**（[`TaskRunner::new`] = tokio 工作线程 +
//! [`gd3_net::HttpEngine`]），只有 HTTP 服务是 `gd3-net` 的测试替身
//! （`tests/support/test_server.rs`，手写 TcpListener，无 hyper/reqwest）。
//!
//! 覆盖两条链路：
//!
//! 1. 完整下载 + `tasks.jsonl` 落盘再读回（第 8e 步的「真下载」验收）；
//! 2. 暂停（保留 `.ghd`）→ 恢复 → 完成（第 8g 步的「暂停恢复」验收）。

#[path = "../../gd3-net/tests/support/test_server.rs"]
mod test_server;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use gd3_core::task::{Progress, TaskRecord, TaskStatus, TaskStep};
use gd3_ui::task_runner::{RunnerSettings, TaskRunner};
use gd3_ui::task_service::TaskService;
use serde_json::{Map, Value, json};

use test_server::{Mode, TestServer};

struct Harness {
    /// 只为「让 accept 循环活着」：字段被读不读无所谓，但**不能提前 drop**。
    _runtime: tokio::runtime::Runtime,
    server: TestServer,
}

impl Harness {
    fn start(content: Vec<u8>, mode: Mode) -> Harness {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("建测试运行时");
        let server = runtime.block_on(TestServer::start_with(content, mode));
        Harness {
            _runtime: runtime,
            server,
        }
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "gd3-ui-runner-e2e-{}-{tag}-{seq}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

/// 一条单步 HTTP 任务（字段口径与 `tasks.jsonl` 里的平铺写法一致）。
fn record(
    task_id: &str,
    folder: &Path,
    url: &str,
    file_size: usize,
    subworkers: u64,
) -> TaskRecord {
    let mut extra = Map::new();
    extra.insert("url".to_string(), Value::String(url.to_string()));
    extra.insert("fileSize".to_string(), json!(file_size));
    extra.insert("canUseRangeRequests".to_string(), json!(true));
    extra.insert("subworkerCount".to_string(), json!(subworkers));

    let step = TaskStep {
        step_index: 0,
        file_index: None,
        step_id: "step-0".to_string(),
        status: TaskStatus::Waiting,
        progress: Progress::Int(0),
        received_bytes: 0,
        speed: 0,
        extra,
        type_name: None,
    };

    TaskRecord {
        name: format!("{task_id}.bin"),
        url: url.to_string(),
        pack_id: "http".to_string(),
        task_id: task_id.to_string(),
        status: TaskStatus::Waiting,
        steps: vec![step],
        created_at: 0,
        completed_at: 0,
        output_folder: folder.to_string_lossy().to_string(),
        file_size: file_size as i64,
        files: None,
        category: None,
        should_seed: false,
        checksums: Map::new(),
        extra: Map::new(),
        type_name: None,
    }
}

/// 反复 drain，直到没有在跑的任务。返回 tick 次数。
fn drive(runner: &mut TaskRunner, service: &mut TaskService, timeout: Duration) -> u32 {
    let deadline = Instant::now() + timeout;
    let mut ticks = 0;
    loop {
        runner.drain(service);
        if runner.running_ids().is_empty() {
            return ticks;
        }
        assert!(Instant::now() < deadline, "下载没有在 {timeout:?} 内收工");
        ticks += 1;
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn part_path(dir: &Path, name: &str) -> PathBuf {
    PathBuf::from(format!("{}/{name}.ghd", dir.display()))
}

#[test]
fn a_real_http_download_completes_and_reaches_tasks_jsonl() {
    let content: Vec<u8> = (0..200_000u32).map(|index| (index % 251) as u8).collect();
    let harness = Harness::start(content.clone(), Mode::Range);
    let dir = temp_dir("download");
    let tasks_path = dir.join("tasks.jsonl");

    let (mut service, failures) = TaskService::load(&tasks_path).expect("读缺失的 tasks.jsonl");
    assert!(failures.is_empty());
    let url = harness.server.url("/file.bin");
    let _ = service.add(record("dl", &dir, &url, content.len(), 2), true);

    let mut runner = TaskRunner::new(RunnerSettings::default());
    assert_eq!(runner.pump(&mut service), vec!["dl".to_string()]);
    drive(&mut runner, &mut service, Duration::from_secs(120));

    assert_eq!(
        std::fs::read(dir.join("dl.bin")).expect("读下载结果"),
        content,
        "下载结果要逐字节一致"
    );
    let task = service.task("dl").expect("任务还在");
    assert_eq!(task.status, TaskStatus::Completed);
    assert_eq!(task.steps[0].status, TaskStatus::Completed);
    assert!(!part_path(&dir, "dl.bin").exists(), "完成后删 .ghd");
    assert!(
        !harness.server.requests().is_empty(),
        "服务器应该真的收到请求"
    );

    assert!(service.flush(&tasks_path).expect("落盘"), "有改动就要写");
    let (reloaded, failures) = TaskService::load(&tasks_path).expect("再读回来");
    assert!(failures.is_empty());
    let task = reloaded.task("dl").expect("jsonl 里能读回任务");
    assert_eq!(task.status, TaskStatus::Completed);
    assert_eq!(task.url, url);
    assert_eq!(task.file_size, content.len() as i64);

    assert!(runner.shutdown(Duration::from_secs(5)).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pausing_keeps_the_part_record_and_resuming_finishes_the_download() {
    let content: Vec<u8> = (0..4_000_000u32).map(|index| (index % 253) as u8).collect();
    let harness = Harness::start(
        content.clone(),
        Mode::Slow {
            piece: 16 * 1024,
            gap: Duration::from_millis(10),
        },
    );
    let dir = temp_dir("pause");
    let url = harness.server.url("/file.bin");

    let mut service = TaskService::new();
    let _ = service.add(record("dl", &dir, &url, content.len(), 2), true);
    let mut runner = TaskRunner::new(RunnerSettings::default());
    assert_eq!(runner.pump(&mut service), vec!["dl".to_string()]);

    // 等到第一次进度回调（证明真的在下载），再暂停。
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        runner.drain(&mut service);
        if service.task("dl").expect("任务").steps[0].received_bytes > 0 {
            break;
        }
        assert!(Instant::now() < deadline, "30 秒内没有收到任何进度");
        std::thread::sleep(Duration::from_millis(5));
    }
    let part = part_path(&dir, "dl.bin");
    assert!(part.exists(), "开跑之后就该有 .ghd");

    let max_task_num = runner.settings().max_task_num;
    let report = runner
        .pause(&mut service, "dl", max_task_num)
        .expect("任务在服务里");
    assert!(report.cancel.work_id.is_some(), "要能取消到工作线程");
    drive(&mut runner, &mut service, Duration::from_secs(30));

    let task = service.task("dl").expect("任务还在");
    assert_eq!(task.status, TaskStatus::Paused);
    assert!(task.steps[0].received_bytes > 0);
    assert!(task.steps[0].progress.percent() < 100.0);
    assert!(part.exists(), "暂停要保留 .ghd 以便续传");
    assert!(runner.running_ids().is_empty());

    // 恢复：换回全速模式，重新入队 + 派发。
    harness.server.set_mode(Mode::Range);
    assert!(service.start("dl"), "暂停的任务可以重新入队");
    assert_eq!(runner.pump(&mut service), vec!["dl".to_string()]);
    drive(&mut runner, &mut service, Duration::from_secs(120));

    assert_eq!(
        std::fs::read(dir.join("dl.bin")).expect("读下载结果"),
        content,
        "续传之后仍要逐字节一致"
    );
    assert_eq!(
        service.task("dl").expect("任务").status,
        TaskStatus::Completed
    );
    assert!(!part.exists(), "全部完成后删 .ghd");

    assert!(runner.shutdown(Duration::from_secs(5)).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn preserving_last_modified_stamps_the_real_download() {
    // 第 133 步：`cfg.shouldPreserveLastModified` 打开时，跑完要把输出文件的 mtime
    // 设成 `step.lastModified`（`rust_engine.py:103-110` 的 `_restoreMtime`）。
    let content: Vec<u8> = (0..120_000u32).map(|index| (index % 241) as u8).collect();
    let harness = Harness::start(content.clone(), Mode::Range);
    let dir = temp_dir("mtime");
    let url = harness.server.url("/file.bin");

    let mut service = TaskService::new();
    let mut task = record("dl", &dir, &url, content.len(), 2);
    task.steps[0].extra.insert(
        "lastModified".to_string(),
        json!("Sun, 06 Nov 1994 08:49:37 GMT"),
    );
    let _ = service.add(task, true);

    let mut runner = TaskRunner::new(RunnerSettings {
        preserve_last_modified: true,
        ..Default::default()
    });
    assert_eq!(runner.pump(&mut service), vec!["dl".to_string()]);
    drive(&mut runner, &mut service, Duration::from_secs(120));

    let output = dir.join("dl.bin");
    assert_eq!(
        std::fs::read(&output).expect("读下载结果"),
        content,
        "下载结果要逐字节一致（改时间戳不能碰内容）"
    );
    assert_eq!(
        service.task("dl").expect("任务还在").status,
        TaskStatus::Completed
    );
    let modified = std::fs::metadata(&output)
        .expect("stat")
        .modified()
        .expect("mtime");
    assert_eq!(
        modified
            .duration_since(std::time::UNIX_EPOCH)
            .expect("1970 之后")
            .as_secs(),
        784_111_777,
        "mtime 要变成 Last-Modified 里的时间"
    );

    assert!(runner.shutdown(Duration::from_secs(5)).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_default_settings_leave_the_mtime_alone() {
    // 开关关着（`cfg.py:254` 的缺省）时不能碰 mtime。
    let content: Vec<u8> = (0..40_000u32).map(|index| (index % 239) as u8).collect();
    let harness = Harness::start(content.clone(), Mode::Range);
    let dir = temp_dir("mtime-off");
    let url = harness.server.url("/file.bin");

    let mut service = TaskService::new();
    let mut task = record("dl", &dir, &url, content.len(), 2);
    task.steps[0].extra.insert(
        "lastModified".to_string(),
        json!("Sun, 06 Nov 1994 08:49:37 GMT"),
    );
    let _ = service.add(task, true);

    let mut runner = TaskRunner::new(RunnerSettings::default());
    assert_eq!(runner.pump(&mut service), vec!["dl".to_string()]);
    drive(&mut runner, &mut service, Duration::from_secs(120));

    let output = dir.join("dl.bin");
    assert_eq!(std::fs::read(&output).expect("读下载结果"), content);
    let modified = std::fs::metadata(&output)
        .expect("stat")
        .modified()
        .expect("mtime");
    assert!(
        modified
            .duration_since(std::time::UNIX_EPOCH)
            .expect("1970 之后")
            .as_secs()
            > 784_111_777,
        "缺省不该把 mtime 拉回 1994 年"
    );

    assert!(runner.shutdown(Duration::from_secs(5)).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
