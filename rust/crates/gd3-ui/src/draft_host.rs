//! 草稿对话框的**真宿主**（第 139 步）：把 [`DraftHost`] 接到引擎 / 后台解析
//! 线程 / 窗口通知条上。
//!
//! ## 接线（对照 `app/view/dialogs/task_draft.py` + `main_window.py`）
//!
//! | 钩子 | Python | 这里 |
//! |---|---|---|
//! | `submit_parse` | `coroutineRunner.submit(parse(options), …)` | [`WorkerSubmitter::submit`] → [`ParseSubmit::Async`] |
//! | `cancel_parse` | `coroutineRunner.cancel(parseId)` | [`WorkerSubmitter::cancel`] |
//! | `confirmed` | `taskService.addTask(task)`（`main_window.py` 收 `taskConfirmed`） | `TaskService::add`，派发交给下一轮 `Runtime::tick` 的 `pump` |
//! | `has_conflict` | `taskService.probeConflict(task)`（`draft_cards.py:87`） | `TaskService::probe_conflict` |
//! | `notify_parse_failure` | `InfoBar.error(title, body)` | 窗口通知条（`infoBar*` 三个属性） |
//!
//! `can_edit` 不覆盖：Python 的 `Task.canEdit` 是 `ClassVar[bool] = False`
//! （`models/task.py:187`），HTTP 包没改过它，所以草稿卡片上的「编辑」按钮本来就
//! 是隐藏的（`draft_cards.py:62`）——与 [`DraftHost`] 的缺省实现一致。
//!
//! ## 与 Python 的已知差异
//!
//! 1. **没有原生对话框的那些钩子**：`pick_folder`（`QFileDialog`）、`edit_task`
//!    （`DraftEditDialog`）、`batch_url`（`BatchUrlDialog`）、`import_files`
//!    （`QFileDialog`）、`category_menu`（`AcrylicMenu`）在 Slint 侧要么没有原生
//!    文件对话框、要么对应组件还没接进窗口。这里统一**记一行 stderr 并返回**，
//!    对话框本身照常可用（下载路径能直接在输入框里改）。
//! 2. **通知条不自动消失**：Python 的 `InfoBar` 带关闭按钮与计时器；这里只置位
//!    `infoBarVisible`，由窗口那条通知条自己的关闭按钮收起。
//! 3. **`confirmed` 不弹「磁盘空间不足」以外的提示**：Python 的 `addTask` 返回
//!    `bool`，重复任务静默跳过；这里打一行 stderr，因为 Rust 侧没有全局 InfoBar
//!    队列。

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gd3_core::task::TaskRecord;
use serde_json::{Map, Value};
use slint::ComponentHandle;

use crate::AppWindow;
use crate::draft_worker::{ParseWorker, WorkerSubmitter};
use crate::engine::Engine;
use crate::task_draft_ui::{DraftHost, ParseSubmit, TaskDraftUi, UiEffect};
use crate::task_service::AddOutcome;

/// 生产环境的 [`DraftHost`]：引擎 + 后台解析线程 + 窗口通知条。
pub struct UiDraftHost {
    engine: Rc<RefCell<Engine>>,
    submitter: WorkerSubmitter,
    window: slint::Weak<AppWindow>,
}

impl UiDraftHost {
    /// 宿主只持窗口的**弱**引用：窗口回调里存着宿主的 `Rc`，强引用会绕成环。
    pub fn new(engine: Rc<RefCell<Engine>>, worker: Arc<ParseWorker>, window: &AppWindow) -> Self {
        Self {
            engine,
            submitter: WorkerSubmitter::new(worker),
            window: window.as_weak(),
        }
    }

    /// 后台解析器（测试收尾要 `shutdown`）。
    pub fn worker(&self) -> &Arc<ParseWorker> {
        self.submitter.worker()
    }

    /// 往窗口的通知条上写一对标题 / 正文并显示（`InfoBar.error` 的替身）。
    fn show_info_bar(&self, title: &str, body: &str) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        window.set_infoBarTitle(title.into());
        window.set_infoBarText(body.into());
        window.set_infoBarVisible(true);
    }
}

impl DraftHost for UiDraftHost {
    /// 甩给后台线程；工作线程已经没了就只能等一条永远不来的结果。
    fn submit_parse(&self, work_id: &str, options: &Map<String, Value>) -> ParseSubmit {
        if !self.submitter.submit(work_id, options) {
            eprintln!("后台解析线程已退出，这条 URL 解析不了：{work_id}");
        }
        ParseSubmit::Async
    }

    fn cancel_parse(&self, work_id: &str) {
        self.submitter.cancel(work_id);
    }

    fn has_conflict(&self, task: &TaskRecord) -> bool {
        self.engine.borrow().tasks().probe_conflict(task).is_some()
    }

    fn notify_parse_failure(&self, title: &str, body: &str) {
        eprintln!("{title}：{body}");
        self.show_info_bar(title, body);
    }

    fn confirmed(&self, task: &TaskRecord, auto_start: bool) {
        let outcome = self
            .engine
            .borrow_mut()
            .tasks_mut()
            .add(task.clone(), auto_start);
        match outcome {
            // 入队与否由 `TaskService` 记账，真正的派发在下一轮 `Runtime::tick` 的
            // `TaskRunner::pump`（与 `task_service.rs` 的口径一致）。
            AddOutcome::Added { .. } => {}
            AddOutcome::Duplicate => eprintln!("任务已在列表里：{}", task.task_id),
            AddOutcome::DiskSpaceInsufficient { free } => {
                let body = format!("磁盘剩余空间不足（{free} 字节），无法开始下载。");
                eprintln!("{body}");
                self.show_info_bar("磁盘空间不足", &body);
            }
        }
    }

    fn pick_folder(&self) {
        eprintln!("目录选择框还没接（Slint 无原生文件对话框），请直接编辑下载路径。");
    }

    fn edit_task(&self, url: &str) {
        eprintln!("编辑任务对话框还没接进窗口：{url}");
    }

    fn batch_url(&self) {
        eprintln!("批量添加对话框还没接进窗口，可以直接往输入框里粘多行链接。");
    }

    fn import_files(&self, title: &str, _filters: &[String]) {
        eprintln!("导入文件对话框还没接进窗口：{title}");
    }

    fn category_menu(&self, url: &str, _entries: &[crate::draft_card::DraftMenuEntry]) {
        eprintln!("分类菜单还没接进窗口：{url}");
    }
}

/// `main_window.addUrls(urls)`：有 URL 就**立刻**解析并弹框（不走 1 秒防抖）。
///
/// `urlEdit.appendPlainText` + `self._parseTimer.stop()` + `self._onParseNeeded()`
/// （`task_draft.py:193-202`）在这里就是「追加文本 → 提交解析」。
pub fn seed_urls(ui: &mut TaskDraftUi, urls: &[String], host: &dyn DraftHost) -> Vec<UiEffect> {
    if urls.is_empty() {
        return Vec::new();
    }
    ui.set_open(true);
    ui.add_urls(urls, host)
}

/// 后台结果回灌的轮询间隔（毫秒）。
///
/// Python 的 `coroutineRunner` 是「探测完直接回调」，没有轮询；Slint 侧只能在 UI
/// 线程的定时器里收，100ms 对「粘贴后最多多等一眨眼」是可接受的取舍。
pub const DRAIN_INTERVAL_MS: u64 = 100;

/// 装好一整套草稿对话框的装配体：UI 状态机 + 宿主 + 后台线程 + 回灌定时器。
///
/// 拆出来是为了让 `boot` 只写一行 `build(...)`；`_timer` 一被 drop 定时器就停，
/// 所以调用方必须把它留到事件循环结束。
pub struct DraftUi {
    ui: Rc<RefCell<TaskDraftUi>>,
    host: Rc<UiDraftHost>,
    opener: Rc<dyn Fn()>,
    window: slint::Weak<AppWindow>,
    /// 只为了活着（`slint::Timer` 一 drop 就停），故意不读。
    _timer: slint::Timer,
}

impl DraftUi {
    /// 建对话框状态机 + 宿主 + 回灌定时器，并把回调装到窗口上。
    ///
    /// `default_folder` = `cfg.downloadFolder`、`subworker_default` =
    /// `cfg.preBlockNum`；`folder_history` 目前传空（Qt 的 `ConfigItem.history`
    /// 还没建模，见模块头的差异 1）。
    pub fn build(
        window: &AppWindow,
        engine: Rc<RefCell<Engine>>,
        worker: Arc<ParseWorker>,
        default_folder: &str,
        folder_history: &[String],
        subworker_default: i32,
    ) -> DraftUi {
        let mut ui = TaskDraftUi::new(
            crate::task_draft_ui::Texts::default(),
            default_folder,
            folder_history,
            subworker_default,
        );
        {
            let engine = engine.borrow();
            let rules = engine.category().rules().to_vec();
            let enabled = engine.category().settings().is_category_enabled;
            ui.set_categories(rules, enabled);
        }
        let ui = Rc::new(RefCell::new(ui));
        let host = Rc::new(UiDraftHost::new(Rc::clone(&engine), worker, window));
        let host_dyn: Rc<dyn DraftHost> = Rc::clone(&host) as Rc<dyn DraftHost>;
        let opener = crate::task_draft_ui::install(window, Rc::clone(&ui), host_dyn);

        let timer = slint::Timer::default();
        {
            let ui = Rc::clone(&ui);
            let host = Rc::clone(&host);
            let worker = Arc::clone(host.worker());
            let weak = window.as_weak();
            timer.start(
                slint::TimerMode::Repeated,
                std::time::Duration::from_millis(DRAIN_INTERVAL_MS),
                move || {
                    let Some(window) = weak.upgrade() else {
                        return;
                    };
                    pump(&window, &ui, host.as_ref(), &worker);
                },
            );
        }

        DraftUi {
            ui,
            host,
            opener,
            window: window.as_weak(),
            _timer: timer,
        }
    }

    pub fn ui(&self) -> &Rc<RefCell<TaskDraftUi>> {
        &self.ui
    }

    pub fn host(&self) -> &Rc<UiDraftHost> {
        &self.host
    }

    /// `addUrls([])`：只把对话框打开（`showMask`）。
    pub fn open(&self) {
        (self.opener)();
    }

    /// `addUrls(urls)`：开框 + **立刻**解析（CLI 位置参数 / 单实例交接 / 扩展）。
    pub fn open_with_urls(&self, urls: &[String]) {
        if urls.is_empty() {
            self.open();
            return;
        }
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let effects = seed_urls(&mut self.ui.borrow_mut(), urls, self.host.as_ref());
        crate::task_draft_ui::handle(&window, &self.ui, self.host.as_ref(), effects);
    }
}

/// 宿主定时器的收口：把后台回来的结果灌进对话框，再按常规路径派发效果 + 刷新快照。
///
/// 没有效果时**不**刷新：`refresh` 会重灌整份 `DraftRowData` 模型，白跑一遍会让
/// 正在编辑的输入框闪烁。
pub fn pump(
    window: &AppWindow,
    ui: &Rc<RefCell<TaskDraftUi>>,
    host: &dyn DraftHost,
    worker: &ParseWorker,
) {
    let effects = crate::draft_worker::drain_into(worker, &mut ui.borrow_mut(), host);
    if effects.is_empty() {
        return;
    }
    crate::task_draft_ui::handle(window, ui, host, effects);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_create::{CreateDefaults, HttpParser, Prober};
    use crate::task_draft_ui::Texts;
    use gd3_core::task::TaskStatus;
    use gd3_core::task_error::TaskError;
    use gd3_net::probe::{ProbeOptions, RemoteFile};
    use std::time::{Duration, Instant};

    /// 桩探测器：只回固定大小，一条网络都不发。
    struct StubProber;

    impl Prober for StubProber {
        fn probe(&self, options: &ProbeOptions) -> Result<RemoteFile, TaskError> {
            Ok(RemoteFile {
                name: options.url.rsplit('/').next().unwrap_or("x").to_string(),
                file_size: 64,
                can_use_range_requests: true,
                last_modified: String::new(),
                content_type: "application/octet-stream".to_string(),
            })
        }
    }

    fn record(name: &str, folder: &str) -> TaskRecord {
        TaskRecord {
            name: name.to_string(),
            url: format!("http://example.com/{name}"),
            pack_id: "http".to_string(),
            task_id: format!("tsk_{name}"),
            status: TaskStatus::Waiting,
            steps: Vec::new(),
            created_at: 0,
            completed_at: 0,
            output_folder: folder.to_string(),
            file_size: 64,
            files: None,
            category: None,
            should_seed: false,
            checksums: Map::new(),
            extra: Map::new(),
            type_name: None,
        }
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("gd3-ui-draft-host-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    /// 一台装了桩解析器 worker 的宿主 + 引擎 + 无头窗口。
    struct Harness {
        window: AppWindow,
        engine: Rc<RefCell<Engine>>,
        worker: Arc<ParseWorker>,
        host: Rc<UiDraftHost>,
        dir: std::path::PathBuf,
    }

    impl Harness {
        fn new(tag: &str) -> Harness {
            let dir = temp_dir(tag);
            let config =
                gd3_core::settings::default_config_with_download_folder(&dir.to_string_lossy());
            let mut engine = Engine::new(&config, dir.join("tasks.jsonl"));
            engine.start().expect("装载任务列表");
            let parser = HttpParser::new(Box::new(StubProber), CreateDefaults::default());
            let worker = Arc::new(ParseWorker::with_parser(parser).expect("worker"));
            let engine = Rc::new(RefCell::new(engine));
            let window = crate::create_headless();
            let host = Rc::new(UiDraftHost::new(
                Rc::clone(&engine),
                Arc::clone(&worker),
                &window,
            ));
            Harness {
                window,
                engine,
                worker,
                host,
                dir,
            }
        }

        fn ui(&self) -> Rc<RefCell<TaskDraftUi>> {
            Rc::new(RefCell::new(TaskDraftUi::new(
                Texts::default(),
                &self.dir.to_string_lossy(),
                &[],
                8,
            )))
        }
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn confirmed_adds_the_task_and_ignores_the_duplicate() {
        let harness = Harness::new("confirm");
        let task = record("a.bin", &harness.dir.to_string_lossy());
        harness.host.confirmed(&task, true);
        harness.host.confirmed(&task, true);
        let engine = harness.engine.borrow();
        assert_eq!(engine.tasks().records().len(), 1, "重复确认不该加出第二条");
        assert_eq!(engine.tasks().records()[0].task_id, task.task_id);
    }

    #[test]
    fn submit_parse_hands_the_job_to_the_worker() {
        let harness = Harness::new("submit");
        let mut options = Map::new();
        options.insert(
            "url".to_string(),
            Value::String("http://example.com/real.bin".to_string()),
        );
        let submit = harness.host.submit_parse("draft-1", &options);
        assert!(matches!(submit, ParseSubmit::Async), "生产宿主必然是异步");
        let outcome = harness
            .worker
            .recv_timeout(Duration::from_secs(10))
            .expect("后台应该回一条结果");
        match outcome {
            crate::draft_worker::ParseOutcome::Parsed { work_id, task } => {
                assert_eq!(work_id, "draft-1", "work_id 要原样交回");
                assert_eq!(task.name, "real.bin");
            }
            other => panic!("应该解析成功：{other:?}"),
        }
    }

    #[test]
    fn has_conflict_sees_a_file_that_is_already_there() {
        let harness = Harness::new("conflict");
        let task = record("taken.bin", &harness.dir.to_string_lossy());
        assert!(!harness.host.has_conflict(&task), "还没落盘，不算冲突");
        std::fs::write(harness.dir.join("taken.bin"), b"x").expect("造冲突文件");
        assert!(harness.host.has_conflict(&task), "同名文件在就该报冲突");
    }

    #[test]
    fn seed_urls_opens_the_dialog_and_starts_parsing() {
        let harness = Harness::new("seed");
        let mut ui = TaskDraftUi::new(Texts::default(), "/dl", &[], 8);
        let effects = seed_urls(
            &mut ui,
            &["http://example.com/one.bin".to_string()],
            harness.host.as_ref(),
        );
        assert!(ui.is_open(), "有 URL 就把对话框打开（`showMask`）");
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, UiEffect::ParseSubmitted { .. })),
            "应该立刻提交解析，不走 1 秒防抖：{effects:?}"
        );
        // 空列表什么都不做（`addUrls` 的第一行 `if not urls: return`）。
        let effects = seed_urls(&mut ui, &[], harness.host.as_ref());
        assert!(effects.is_empty());
    }

    #[test]
    fn pump_feeds_the_background_result_into_the_dialog() {
        let harness = Harness::new("pump");
        let ui = harness.ui();
        let effects = {
            let mut ui = ui.borrow_mut();
            seed_urls(
                &mut ui,
                &["http://example.com/pumped.bin".to_string()],
                harness.host.as_ref(),
            )
        };
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, UiEffect::ParseSubmitted { .. }))
        );

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            pump(&harness.window, &ui, harness.host.as_ref(), &harness.worker);
            if !ui.borrow().view(harness.host.as_ref()).rows.is_empty() {
                break;
            }
            assert!(Instant::now() < deadline, "后台结果没在期内回来");
            std::thread::sleep(Duration::from_millis(5));
        }
        let view = ui.borrow().view(harness.host.as_ref());
        assert_eq!(view.rows.len(), 1);
        assert_eq!(view.rows[0].name, "pumped.bin");
        assert!(!view.parsing, "结果回来后不该还显示解析中");
        assert!(view.open, "对话框仍然开着");
    }

    #[test]
    fn a_parse_failure_lands_on_the_notification_bar() {
        let harness = Harness::new("fail");
        // 桩探测器不会失败，所以直接走宿主的提示钩子：它只该碰通知条三个属性。
        harness
            .host
            .notify_parse_failure("链接解析失败", "服务器返回 500");
        assert!(harness.window.get_infoBarVisible());
        assert_eq!(harness.window.get_infoBarTitle(), "链接解析失败");
        assert_eq!(harness.window.get_infoBarText(), "服务器返回 500");
    }
}
