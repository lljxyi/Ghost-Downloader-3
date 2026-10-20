//! 第 138 步的端到端验收：**粘贴 URL → 后台探测 → 确认 → 真的下下来**。
//!
//! 单测（`draft_worker::tests`）用桩探测器把「worker 线程 + 通道 + 取消 + 回灌」
//! 钉死了，但没碰网络。这一条把真件摆齐：真 HTTP 服务器 + 真
//! [`gd3_ui::task_create::NetProber`] 的 [`gd3_ui::draft_worker::ParseWorker`]，
//! 走草稿对话框的公开入口（`text_changed` → `parse_needed` → tick 里
//! `drain_into` → `accept`），最后把确认出来的记录交给 `TaskService`，由
//! `Runtime` 推着下完并逐字节校验。
//!
//! 覆盖的三件事：
//!
//! 1. **UI 线程不被探测阻塞**：`parse_needed` 立刻返回，且返回的是
//!    `ParseSubmitted` 而不是解析结果；
//! 2. **结果从通道回灌**：tick 里 `drain_into` 之后草稿项才有名字/大小/统计；
//! 3. **确认出来的记录是可下的**：任务真下完，文件逐字节一致。

#[path = "../../gd3-net/tests/support/test_server.rs"]
mod test_server;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use gd3_core::settings;
use gd3_core::task::{TaskRecord, TaskStatus};
use gd3_ui::draft_worker::{ParseWorker, WorkerSubmitter, drain_into};
use gd3_ui::engine::Engine;
use gd3_ui::runtime::Runtime;
use gd3_ui::task_create::CreateDefaults;
use gd3_ui::task_draft::DraftEffect;
use gd3_ui::task_draft_ui::{DraftHost, ParseSubmit, TaskDraftUi, Texts, UiEffect};
use gd3_ui::task_runner::{RunnerSettings, TaskRunner};
use gd3_ui::task_service::AddOutcome;
use serde_json::{Map, Value};

use test_server::{Mode, TestServer};

/// 只为让 accept 循环活着：字段不读，但**不能提前 drop**。
struct Harness {
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
        "gd3-ui-draft-worker-{}-{tag}-{seq}",
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

/// 宿主：解析甩给后台 worker，其余钩子只记账。
struct WorkerHost {
    submitter: WorkerSubmitter,
    log: RefCell<Vec<String>>,
}

impl WorkerHost {
    fn new(worker: Arc<ParseWorker>) -> Self {
        Self {
            submitter: WorkerSubmitter::new(worker),
            log: RefCell::new(Vec::new()),
        }
    }
}

impl DraftHost for WorkerHost {
    fn submit_parse(&self, work_id: &str, options: &Map<String, Value>) -> ParseSubmit {
        assert!(
            self.submitter.submit(work_id, options),
            "后台 worker 应该还活着"
        );
        ParseSubmit::Async
    }

    fn cancel_parse(&self, work_id: &str) {
        self.submitter.cancel(work_id);
    }

    fn notify_parse_failure(&self, title: &str, body: &str) {
        self.log.borrow_mut().push(format!("error:{title}:{body}"));
    }

    fn confirmed(&self, task: &TaskRecord, auto_start: bool) {
        self.log
            .borrow_mut()
            .push(format!("confirm:{}:{auto_start}", task.name));
    }

    fn can_edit(&self, _task: &TaskRecord) -> bool {
        true
    }
}

/// 从效果里抽出确认出来的任务（`draft/ui.rs::handle` 里交给 `TaskService` 的那条）。
fn confirmed(effects: &[UiEffect]) -> Option<(TaskRecord, bool)> {
    effects.iter().find_map(|effect| match effect {
        UiEffect::Draft(draft) => match &**draft {
            DraftEffect::Confirmed { task, auto_start } => Some((task.clone(), *auto_start)),
            _ => None,
        },
        _ => None,
    })
}

fn succeeded(effects: &[UiEffect]) -> Option<&TaskRecord> {
    effects.iter().find_map(|effect| match effect {
        UiEffect::Draft(draft) => match &**draft {
            DraftEffect::Succeeded { task, .. } => Some(task),
            _ => None,
        },
        _ => None,
    })
}

#[test]
fn a_pasted_url_is_probed_in_the_background_and_downloads() {
    let content = content_of(120_000, 247);
    let harness = Harness::start(content.clone());
    let dir = temp_dir("chain");
    let url = harness.server.url("/draft.bin");

    let defaults = CreateDefaults {
        output_folder: dir.to_string_lossy().to_string(),
        headers: Vec::new(),
        client_profile: String::new(),
        subworker_count: 2,
    };
    let worker = Arc::new(ParseWorker::spawn(defaults).expect("后台解析 worker"));
    let host = WorkerHost::new(Arc::clone(&worker));

    let mut ui = TaskDraftUi::new(Texts::default(), &dir.to_string_lossy(), &[], 8);
    ui.text_changed(&url);

    // 一：贴 URL 这一步**不阻塞**——只报「已提交」，探测还在后台跑。
    let effects = ui.parse_needed(&host);
    assert!(
        effects
            .iter()
            .any(|effect| matches!(effect, UiEffect::ParseSubmitted { .. })),
        "应该先是 ParseSubmitted：{effects:?}"
    );
    assert!(ui.draft().can_confirm(), "在跑的解析也算可确认");
    assert_eq!(ui.view(&host).rows.len(), 0, "探测还没回来，没有行");

    // 二：tick 循环把结果收回来（真探测，给足时间）。
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut probed: Option<TaskRecord> = None;
    while probed.is_none() {
        let effects = drain_into(&worker, &mut ui, &host);
        if let Some(task) = succeeded(&effects) {
            probed = Some(task.clone());
            break;
        }
        assert!(
            Instant::now() < deadline,
            "后台探测没在期内回来：{:?}",
            host.log.borrow()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let probed = probed.expect("探测结果");

    assert_eq!(probed.name, "draft.bin");
    assert_eq!(probed.file_size, content.len() as i64, "大小来自真探测");
    assert!(
        probed.steps[0].extra["canUseRangeRequests"]
            .as_bool()
            .unwrap(),
        "服务器支持 Range，探测要看得出来"
    );

    // 快照（Slint 那一层灌的就是它）：一行、名字对、可确认。
    let view = ui.view(&host);
    assert_eq!(view.rows.len(), 1);
    assert_eq!(view.rows[0].name, "draft.bin");
    assert!(!view.rows[0].category_visible || view.rows[0].category.is_empty());
    assert!(view.can_confirm);
    assert!(!view.parsing, "结果回来后不该还显示「解析中」");
    assert!(view.size_visible, "统计里有总大小");

    // 三：确认 → 记录进 `TaskService` → `Runtime` 推着下完。
    let effects = ui.accept(true, &host);
    let (task, auto_start) = confirmed(&effects).expect("确认效果");
    assert_eq!(task.task_id, probed.task_id);
    assert!(auto_start);
    // `host.confirmed` 由 `task_draft_ui::handle` 派发（那里要窗口，单测已覆盖），
    // 这里只确认整个过程没有往宿主报错。
    assert!(
        host.log.borrow().is_empty(),
        "探测成功就不该报到宿主：{:?}",
        host.log.borrow()
    );
    assert!(ui.draft().items().is_empty(), "确认后草稿清空");

    let config = settings::default_config_with_download_folder(&dir.to_string_lossy());
    let mut engine = Engine::new(&config, dir.join("tasks.jsonl"));
    engine.start().expect("装载任务列表");
    let task_id = task.task_id.clone();
    let outcome = engine.tasks_mut().add(task, auto_start);
    assert!(matches!(outcome, AddOutcome::Added { .. }), "{outcome:?}");
    assert_eq!(engine.tasks().records().len(), 1);

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
        std::fs::read(dir.join("draft.bin")).expect("读下载结果"),
        content,
        "下载结果要逐字节一致"
    );
    assert!(runtime.shutdown(Duration::from_secs(5)).is_empty());

    // 服务器真的被探测过（`Range: bytes=1-1` + 真正的分片下载）。
    let requests = harness.server.requests();
    assert!(requests.len() >= 2, "至少一次探测 + 一次取块：{requests:?}");

    drop(host);
    let worker = match Arc::try_unwrap(worker) {
        Ok(worker) => worker,
        Err(_) => panic!("宿主放手后应该只剩一份 worker"),
    };
    worker.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
