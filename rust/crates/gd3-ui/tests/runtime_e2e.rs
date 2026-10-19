//! 第 127 步的端到端验收：**宿主层**（[`gd3_ui::runtime::Runtime`] 与
//! [`gd3_ui::boot::run`]）真的会把任务下下来。
//!
//! `tests/runner_e2e.rs` 验的是 `TaskRunner` 自己（手工 pump + drain）；这里验的是
//! 「谁按 tick 推它」——也就是 `cargo run -p gd3-ui` 走的**同一条**路径：
//!
//! ```text
//! tasks.jsonl / service.add → Engine → Runtime.tick → TaskRunner → HttpEngine
//!                                       ↓
//!                                  tasks.jsonl 落盘
//! ```
//!
//! 两条链路：
//!
//! 1. **CLI 全链路**：写好 `tasks.jsonl` → `boot::run(--until-idle)` → 真下载 →
//!    逐字节校验 → 状态回写 `tasks.jsonl`（Phase 8 完工判定 3 + 4 + 5）；
//! 2. **暂停 / 恢复**：慢速服务 → `Runtime::pause`（保住 `.ghd`）→ `Runtime::start`
//!    → 续传完成（完工判定 5 的后半段）。

#[path = "../../gd3-net/tests/support/test_server.rs"]
mod test_server;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use gd3_core::settings;
use gd3_core::task::{Progress, TaskRecord, TaskStatus, TaskStep};
use gd3_ui::boot::{Cli, RunOutcome};
use gd3_ui::engine::Engine;
use gd3_ui::runtime::Runtime;
use gd3_ui::task_runner::{RunnerSettings, TaskRunner};
use serde_json::{Map, Value, json};

use test_server::{Mode, TestServer};

struct Harness {
    /// 只为「让 accept 循环活着」：字段本身不读，但**不能提前 drop**。
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
        "gd3-ui-runtime-e2e-{}-{tag}-{seq}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

/// 一条单步 HTTP 任务（字段口径与 `tasks.jsonl` 里的平铺写法一致）。
fn record(task_id: &str, folder: &Path, url: &str, file_size: usize) -> TaskRecord {
    let mut extra = Map::new();
    extra.insert("url".to_string(), Value::String(url.to_string()));
    extra.insert("fileSize".to_string(), json!(file_size));
    extra.insert("canUseRangeRequests".to_string(), json!(true));
    extra.insert("subworkerCount".to_string(), json!(2));

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

fn content_of(len: usize, modulo: u32) -> Vec<u8> {
    (0..len as u32)
        .map(|index| (index % modulo) as u8)
        .collect()
}

fn part_path(dir: &Path, name: &str) -> PathBuf {
    PathBuf::from(format!("{}/{name}.ghd", dir.display()))
}

/// 造一套「引擎 + 运行器 + 无头窗口」的运行期（生产 `TaskRunner`）。
fn runtime_with(dir: &Path, tasks_path: &Path, task: TaskRecord) -> Runtime {
    let config = settings::default_config_with_download_folder(&dir.to_string_lossy());
    let mut engine = Engine::new(&config, tasks_path);
    engine.start().expect("装载任务列表");
    let _ = engine.tasks_mut().add(task, true);

    let runner = TaskRunner::new(RunnerSettings::from_config(engine.config()));
    let mut runtime = Runtime::new(
        Rc::new(RefCell::new(engine)),
        Rc::new(RefCell::new(runner)),
        gd3_ui::create_headless(),
    );
    // 无头手动推：把 tick 压到 5ms，免得测试白等。
    runtime.set_tick_interval(Duration::from_millis(5));
    runtime
}

/// 一直 tick 到 `done` 为真（或超时 panic）。返回 tick 次数。
fn tick_until(runtime: &Runtime, timeout: Duration, mut done: impl FnMut(&Runtime) -> bool) -> u32 {
    let deadline = Instant::now() + timeout;
    let mut ticks = 0;
    while !done(runtime) {
        runtime.tick();
        ticks += 1;
        assert!(
            Instant::now() < deadline,
            "没有在 {timeout:?} 内达到目标（已经推了 {ticks} 个 tick）"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    ticks
}

fn step_bytes(runtime: &Runtime, task_id: &str) -> i64 {
    runtime
        .engine()
        .borrow()
        .tasks()
        .task(task_id)
        .map(|task| task.steps[0].received_bytes)
        .unwrap_or(0)
}

fn status(runtime: &Runtime, task_id: &str) -> Option<TaskStatus> {
    runtime
        .engine()
        .borrow()
        .tasks()
        .task(task_id)
        .map(|task| task.status)
}

/// 完工判定 3 + 4 + 5（前半）：`--until-idle` 把 `tasks.jsonl` 里的任务真的下完。
#[test]
fn boot_until_idle_downloads_the_task_saved_on_disk() {
    let content = content_of(200_000, 251);
    let harness = Harness::start(content.clone(), Mode::Range);
    let dir = temp_dir("boot-idle");
    let url = harness.server.url("/file.bin");

    // 任务只存在于 `tasks.jsonl` 里（Python 侧写出来的就是这种一行一条的形态）。
    let tasks_path = dir.join("tasks.jsonl");
    let body =
        serde_json::to_string(&record("dl", &dir, &url, content.len())).expect("序列化任务") + "\n";
    std::fs::write(&tasks_path, body.as_bytes()).expect("写任务文件");

    let cli = Cli {
        headless: true,
        until_idle: true,
        idle_timeout: Some(120),
        data_dir: Some(dir.clone()),
        ..Cli::default()
    };
    let summary = match gd3_ui::boot::run(&cli).expect("无头启动应成功") {
        RunOutcome::Ran(summary) => summary,
        RunOutcome::AlreadyRunning(secondary) => panic!("不该变成第二个实例：{secondary:?}"),
    };

    assert!(summary.headless, "--until-idle 隐含无头");
    assert_eq!(summary.tasks, 1);
    assert_eq!(summary.resumed, 1, "WAITING 任务会被 resumeSaved 重新入队");
    assert_eq!(summary.completed, 1, "真的下完了");
    assert!(summary.runner_ticks > 1, "{}", summary.runner_ticks);

    assert_eq!(
        std::fs::read(dir.join("dl.bin")).expect("读下载结果"),
        content,
        "下载结果要逐字节一致"
    );
    assert!(!harness.server.requests().is_empty(), "服务器要真的被请求");
    assert!(!part_path(&dir, "dl.bin").exists(), "完成后删 .ghd");

    // 状态回写 `tasks.jsonl`（Python 的 `TaskStore` 读得回）。
    let text = std::fs::read_to_string(&tasks_path).expect("读回 tasks.jsonl");
    assert!(text.contains("\"COMPLETED\""), "{text}");
    let reloaded: Vec<TaskRecord> = text
        .lines()
        .map(|line| serde_json::from_str(line).expect("每行都是合法任务"))
        .collect();
    assert_eq!(reloaded.len(), 1);
    assert_eq!(reloaded[0].task_id, "dl");
    assert_eq!(reloaded[0].status, TaskStatus::Completed);
    assert_eq!(reloaded[0].steps[0].status, TaskStatus::Completed);
    assert_eq!(reloaded[0].file_size, content.len() as i64);

    let _ = std::fs::remove_dir_all(&dir);
}

/// 完工判定 5（后半）：`Runtime::pause` 保住 `.ghd`，`Runtime::start` 续传后完成。
#[test]
fn pause_keeps_the_part_record_and_start_resumes_it() {
    let content = content_of(2_000_000, 253);
    let harness = Harness::start(
        content.clone(),
        Mode::Slow {
            piece: 16 * 1024,
            gap: Duration::from_millis(10),
        },
    );
    let dir = temp_dir("boot-pause");
    let tasks_path = dir.join("tasks.jsonl");
    let url = harness.server.url("/file.bin");
    let runtime = runtime_with(&dir, &tasks_path, record("dl", &dir, &url, content.len()));

    // 1. 先看到真进度，再暂停。
    tick_until(&runtime, Duration::from_secs(60), |runtime| {
        step_bytes(runtime, "dl") > 0
    });
    let part = part_path(&dir, "dl.bin");
    assert!(part.exists(), "开跑之后就该有 .ghd");

    let report = runtime.pause("dl").expect("任务在服务里");
    assert!(report.cancel.work_id.is_some(), "要能取消到工作线程");
    assert_eq!(status(&runtime, "dl"), Some(TaskStatus::Paused));
    assert!(runtime.is_idle(), "取消之后运行集合里就没有它了");
    let received = step_bytes(&runtime, "dl");
    assert!(
        received > 0 && received < content.len() as i64,
        "{received}"
    );
    assert!(part.exists(), "暂停要保留 .ghd 以便续传");

    // 2. 恢复：换回全速模式 → 重新入队 → 续传完成。
    harness.server.set_mode(Mode::Range);
    assert!(runtime.start("dl"), "暂停的任务可以重新入队");
    tick_until(&runtime, Duration::from_secs(120), |runtime| {
        status(runtime, "dl") == Some(TaskStatus::Completed)
    });

    assert_eq!(
        std::fs::read(dir.join("dl.bin")).expect("读下载结果"),
        content,
        "续传之后仍要逐字节一致"
    );
    assert!(!part.exists(), "全部完成后删 .ghd");

    // 收尾：停机不该留下没收回来的工作线程。
    let mut runtime = runtime;
    assert!(runtime.shutdown(Duration::from_secs(5)).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
