//! 第 137 步的端到端验收：**URL → 记录 → 真的下下来**。
//!
//! `task_create` 的单测把 `buildTask` 的 JSON 形状钉死了，但没碰网络。这一条补上
//! 缺掉的半个环：拿一个真 HTTP 服务器，让 [`gd3_ui::task_create::HttpParser`] 真的
//! 探测（`Range: bytes=1-1`）、真的造记录，然后
//!
//! 1. 记录落进 `tasks.jsonl` → `boot::run(--until-idle)` 读回来下完 → 逐字节校验；
//! 2. 记录直接进 `TaskService` → `Runtime::tick` 推着下完（不走 CLI）。
//!
//! 两条路径验的是同一件事：**Rust 侧的建任务流水线产出的记录，Rust 侧的下载器认**。

#[path = "../../gd3-net/tests/support/test_server.rs"]
mod test_server;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use gd3_core::settings;
use gd3_core::task::TaskStatus;
use gd3_ui::boot::{Cli, RunOutcome};
use gd3_ui::engine::Engine;
use gd3_ui::feature_service::ParserChain;
use gd3_ui::runtime::Runtime;
use gd3_ui::task_create::{CreateDefaults, HttpParser};
use gd3_ui::task_runner::{RunnerSettings, TaskRunner};
use serde_json::{Map, Value};

use test_server::{Mode, TestServer};

struct Harness {
    /// 只为让 accept 循环活着：字段不读，但**不能提前 drop**。
    _runtime: tokio::runtime::Runtime,
    server: TestServer,
}

impl Harness {
    fn start(content: Vec<u8>) -> Harness {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("建测试运行时");
        let server = runtime.block_on(TestServer::start_with(content, Mode::Range));
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
        "gd3-ui-task-create-{}-{tag}-{seq}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

fn content_of(len: usize, modulo: u32) -> Vec<u8> {
    (0..len as u32)
        .map(|index| (index % modulo) as u8)
        .collect()
}

/// 真探测（`NetProber`）+ 建记录，返回记录。
fn create(url: &str, folder: &Path) -> gd3_core::task::TaskRecord {
    let defaults = CreateDefaults {
        output_folder: folder.to_string_lossy().to_string(),
        headers: Vec::new(),
        client_profile: String::new(),
        subworker_count: 2,
    };
    let mut chain = ParserChain::new();
    chain.register(Box::new(
        HttpParser::with_defaults(defaults).expect("建探测运行时"),
    ));

    let mut options = Map::new();
    options.insert("url".to_string(), Value::String(url.to_string()));
    chain.parse(&mut options, &[]).expect("解析 URL 应成功")
}

/// 一：`tasks.jsonl` → `--until-idle`，记录由本步的解析器产出。
#[test]
fn a_url_created_by_the_parser_downloads_through_the_cli() {
    let content = content_of(200_000, 249);
    let harness = Harness::start(content.clone());
    let dir = temp_dir("cli");
    let url = harness.server.url("/file.bin");

    let record = create(&url, &dir);
    assert_eq!(
        record.file_size,
        content.len() as i64,
        "探测出来的大小要和真内容一致"
    );
    assert!(
        record.steps[0].extra["canUseRangeRequests"]
            .as_bool()
            .unwrap(),
        "服务器支持 Range，探测要看得出来"
    );
    assert_eq!(record.name, "file.bin", "名字取自 URL path");

    // 落盘 → 拉起 CLI（Python 侧写出来的就是这种一行一条的形态）。
    let tasks_path = dir.join("tasks.jsonl");
    let body = serde_json::to_string(&record).expect("序列化任务") + "\n";
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
    assert_eq!(summary.tasks, 1);
    assert_eq!(summary.completed, 1, "真的下完了");

    assert_eq!(
        std::fs::read(dir.join("file.bin")).expect("读下载结果"),
        content,
        "下载结果要逐字节一致"
    );
    assert!(!harness.server.requests().is_empty(), "服务器要真的被请求");

    let text = std::fs::read_to_string(&tasks_path).expect("读回 tasks.jsonl");
    assert!(text.contains("\"COMPLETED\""), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 二：不经 CLI，记录直接进服务层，由 `Runtime` 推着下完。
#[test]
fn a_url_created_by_the_parser_downloads_through_the_runtime() {
    let content = content_of(150_000, 241);
    let harness = Harness::start(content.clone());
    let dir = temp_dir("runtime");
    let url = harness.server.url("/payload.bin");

    let record = create(&url, &dir);
    let task_id = record.task_id.clone();

    let config = settings::default_config_with_download_folder(&dir.to_string_lossy());
    let mut engine = Engine::new(&config, dir.join("tasks.jsonl"));
    engine.start().expect("装载任务列表");
    let _ = engine.tasks_mut().add(record, true);

    let runner = TaskRunner::new(RunnerSettings::from_config(engine.config()));
    let mut runtime = Runtime::new(
        Rc::new(RefCell::new(engine)),
        Rc::new(RefCell::new(runner)),
        gd3_ui::create_headless(),
    );
    runtime.set_tick_interval(Duration::from_millis(5));

    let deadline = Instant::now() + Duration::from_secs(120);
    let status = loop {
        runtime.tick();
        let status = runtime
            .engine()
            .borrow()
            .tasks()
            .task(&task_id)
            .map(|task| task.status);
        if status == Some(TaskStatus::Completed) {
            break status;
        }
        assert!(Instant::now() < deadline, "没有在期内下完：{status:?}");
        std::thread::sleep(Duration::from_millis(2));
    };
    assert_eq!(status, Some(TaskStatus::Completed));

    assert_eq!(
        std::fs::read(dir.join("payload.bin")).expect("读下载结果"),
        content,
        "下载结果要逐字节一致"
    );
    assert!(
        !Path::new(&format!("{}/payload.bin.ghd", dir.display())).exists(),
        "完成后删 .ghd"
    );

    assert!(runtime.shutdown(Duration::from_secs(5)).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
