//! 草稿对话框的后台解析（第 138 步）：`gd3-ui::draft_worker`。
//!
//! Python 侧草稿对话框靠 `coroutineRunner.submit(coro, done=…, failed=…)` 把
//! `featureService.parse(options)` 丢进事件循环，**不阻塞 UI**；回调再回到
//! [`crate::task_draft::TaskDraft::on_parsed`] / `on_parse_failed`。Rust 里
//! `ParserChain::parse` 是同步的（[`crate::task_create::HttpParser::parse`] 会
//! 在 [`crate::task_create::NetProber`] 里 `block_on` 一次真实探测，最长到
//! `SIZE_UNKNOWN` 的请求超时），所以这里用**一根工作线程 + 两个通道**顶上：
//!
//! ```text
//! UI 线程                          工作线程
//! ───────                          ────────
//! TaskDraft::set_urls
//!   → DraftEffect::Parse{work_id, options}
//!     → ParseWorker::submit ──Job──▶ recv() → 取消过？跳过
//!                                     HttpParser::parse(probe 30s 级)
//!                                     → 取消过？丢弃
//!                        ◀─Outcome── send(Parsed|Failed)
//!   （UI tick，1 秒一次）
//!   ParseWorker::drain → TaskDraftUi::on_parsed
//!     → TaskDraft::on_parsed → 效果队列（Succeeded / Confirmed / …）
//! ```
//!
//! ## 与 Python 的差异（刻意，别当 bug）
//!
//! 1. **串行而不是并发**：工作线程一次只跑一个 job（FIFO）。Python 的
//!    `asyncio` 会把所有 URL 的探测并发起来。串行换来的是「一个
//!    [`crate::task_create::NetProber`] 头上一个 current-thread runtime」，
//!    不用为每个 URL 建 runtime；批量粘贴 20 条链接时是慢一点，不是错。
//! 2. **取消是协作式的**：`probe` 已经进了 `block_on` 就没法打断，只能
//!    「结果回来时丢掉」（提交前取消、结果落地前取消都会丢）。因此项上的
//!    `parse_id` 清空后，迟到的结果不会有任何副作用。
//! 3. **`work_id` 由 [`crate::task_draft::TaskDraft`] 发号**（`draft-1`…），
//!    宿主把它当不透明句柄原样交回；这里不做解析、不校验格式。
//! 4. **`cancelled` 集合只增不减**（除被 `drain` 回收的 id）：取消一项后
//!    该 id 会一直留着，量级是「用户点过的取消次数」，可忽略。

use std::collections::HashSet;
use std::io;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use gd3_core::task::TaskRecord;
use gd3_core::task_error::TaskError;
use serde_json::{Map, Value};

use crate::feature_service::TaskParser as _;
use crate::task_create::{CreateDefaults, HttpParser};
use crate::task_draft_ui::{DraftHost, TaskDraftUi, UiEffect};

/// 这个 `work_id` 是否被取消过（锁中毒时按「没取消」处理，宁可多跑一次）。
fn is_cancelled(cancelled: &Arc<Mutex<HashSet<String>>>, work_id: &str) -> bool {
    cancelled
        .lock()
        .map(|set| set.contains(work_id))
        .unwrap_or(false)
}

/// 一次解析任务（UI 线程 → 工作线程）。
struct Job {
    work_id: String,
    /// 原始 options 字典（含 `"url"`），原样喂给 `HttpParser::parse`。
    options: Map<String, Value>,
}

/// 一次解析的结果（工作线程 → UI 线程）。
#[derive(Debug, Clone)]
pub enum ParseOutcome {
    /// `parseSucceeded` 的那一半：探测 + 造记录成功。
    ///
    /// `TaskRecord` 有几百字节，`Box` 一层让这个枚举过通道时只搬指针
    /// （`clippy::large_enum_variant`）。
    Parsed {
        work_id: String,
        task: Box<TaskRecord>,
    },
    /// `parseFailed` 的那一半。
    Failed { work_id: String, error: TaskError },
}

impl ParseOutcome {
    /// 宿主把它当不透明句柄原样交回 [`crate::task_draft::TaskDraft`]。
    pub fn work_id(&self) -> &str {
        match self {
            ParseOutcome::Parsed { work_id, .. } | ParseOutcome::Failed { work_id, .. } => work_id,
        }
    }
}

/// 后台解析器：持有工作线程、提交端、结果端与取消集合。
///
/// **不做 `Drop` join**：`probe` 可能正卡在 `block_on` 里，等它会把宿主拖死。
/// 丢掉 [`ParseWorker`] 时提交端一起消失，工作线程跑完手上这一条自然退出。
/// 想等到线程真的结束（测试收尾）就调 [`ParseWorker::shutdown`]。
pub struct ParseWorker {
    // `Sender` / `Receiver` 都是 `Send` 但**不是** `Sync`，而宿主侧必然要
    // `Arc<ParseWorker>`（[`WorkerSubmitter`] 与 UI 线程各持一份）。包一层
    // `Mutex` 之后 `ParseWorker: Send + Sync`，两条线程共用同一份 worker。
    tx: Mutex<Option<Sender<Job>>>,
    rx: Mutex<Receiver<ParseOutcome>>,
    cancelled: Arc<Mutex<HashSet<String>>>,
    thread: Option<JoinHandle<()>>,
}

impl ParseWorker {
    /// 用真网络探测建一个后台解析器（[`HttpParser::with_defaults`]）。
    pub fn spawn(defaults: CreateDefaults) -> io::Result<ParseWorker> {
        ParseWorker::with_parser(HttpParser::with_defaults(defaults)?)
    }

    /// 用现成的 [`HttpParser`] 建一个后台解析器（单测注入桩探测器就看这里）。
    pub fn with_parser(parser: HttpParser) -> io::Result<ParseWorker> {
        let (job_tx, job_rx) = channel::<Job>();
        let (out_tx, out_rx) = channel::<ParseOutcome>();
        let cancelled: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));

        let thread_cancelled = Arc::clone(&cancelled);
        let thread = thread::Builder::new()
            .name("gd3-draft-parse".to_string())
            .spawn(move || {
                while let Ok(job) = job_rx.recv() {
                    if is_cancelled(&thread_cancelled, &job.work_id) {
                        continue;
                    }
                    let outcome = match parser.parse(&job.options) {
                        Ok(task) => ParseOutcome::Parsed {
                            work_id: job.work_id.clone(),
                            task: Box::new(task),
                        },
                        Err(error) => ParseOutcome::Failed {
                            work_id: job.work_id.clone(),
                            error,
                        },
                    };
                    // 探测期间被取消：结果丢掉，别去动已经清空的草稿项。
                    if is_cancelled(&thread_cancelled, &job.work_id) {
                        continue;
                    }
                    if out_tx.send(outcome).is_err() {
                        break;
                    }
                }
            })?;

        Ok(ParseWorker {
            tx: Mutex::new(Some(job_tx)),
            rx: Mutex::new(out_rx),
            cancelled,
            thread: Some(thread),
        })
    }

    /// 提交一次解析。返回 `false` = 工作线程已经没了（调用方照旧等结果即可，
    /// 只是永远等不到）。
    pub fn submit(&self, work_id: impl Into<String>, options: Map<String, Value>) -> bool {
        let Ok(tx) = self.tx.lock() else {
            return false;
        };
        let Some(tx) = tx.as_ref() else {
            return false;
        };
        tx.send(Job {
            work_id: work_id.into(),
            options,
        })
        .is_ok()
    }

    /// `coroutineRunner.cancel(parseId)`：提交前取消 → 直接跳过；跑着取消 →
    /// 结果丢弃（见模块头的差异 2）。
    pub fn cancel(&self, work_id: &str) {
        if let Ok(mut set) = self.cancelled.lock() {
            set.insert(work_id.to_string());
        }
    }

    /// 把已经回来的结果全取走（UI tick 每轮调一次，非阻塞）。
    pub fn drain(&self) -> Vec<ParseOutcome> {
        let mut outcomes = Vec::new();
        if let Ok(rx) = self.rx.lock() {
            while let Ok(outcome) = rx.try_recv() {
                outcomes.push(outcome);
            }
        }
        // 已经落地的 id 不必再记着取消状态。
        if let Ok(mut set) = self.cancelled.lock() {
            for outcome in &outcomes {
                set.remove(outcome.work_id());
            }
        }
        outcomes
    }

    /// 等一条结果或超时（宿主不用它，测试用它少写轮询）。
    pub fn recv_timeout(&self, timeout: Duration) -> Option<ParseOutcome> {
        let outcome = self.rx.lock().ok()?.recv_timeout(timeout).ok()?;
        if let Ok(mut set) = self.cancelled.lock() {
            set.remove(outcome.work_id());
        }
        Some(outcome)
    }

    /// 主动收摊：关掉提交端并等线程跑完（手上那条会先跑完）。
    pub fn shutdown(mut self) {
        if let Ok(mut tx) = self.tx.lock() {
            *tx = None;
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// 把回来的结果灌进对话框状态机，返回要派发/落库的效果（`handle` 的上游）。
///
/// 宿主 tick 里的用法：
///
/// ```ignore
/// let effects = draft_worker::drain_into(&worker, &mut ui.borrow_mut(), host.as_ref());
/// task_draft_ui::handle(&window, &ui, host.as_ref(), effects);
/// ```
pub fn drain_into(
    worker: &ParseWorker,
    ui: &mut TaskDraftUi,
    host: &dyn DraftHost,
) -> Vec<UiEffect> {
    let mut effects = Vec::new();
    for outcome in worker.drain() {
        match outcome {
            ParseOutcome::Parsed { work_id, task } => {
                effects.extend(ui.on_parsed(&work_id, *task, host));
            }
            ParseOutcome::Failed { work_id, error } => {
                effects.extend(ui.on_parse_failed(&work_id, error, host));
            }
        }
    }
    effects
}

/// 宿主侧的最小粘合：把 [`TaskDraftUi`] 的 `DraftEffect::Parse` 转成
/// [`ParseWorker::submit`]、`DraftEffect::Cancel` 转成 [`ParseWorker::cancel`]。
///
/// 真正的宿主还要实现 `can_edit` / `has_conflict` / `confirmed` 等钩子，所以
/// 这里是**独立结构**而不是 `DraftHost` 实现：宿主持有它并按需转调（见模块头的
/// 接线示例）。
pub struct WorkerSubmitter {
    worker: Arc<ParseWorker>,
}

impl WorkerSubmitter {
    pub fn new(worker: Arc<ParseWorker>) -> Self {
        Self { worker }
    }

    pub fn worker(&self) -> &Arc<ParseWorker> {
        &self.worker
    }

    /// [`DraftHost::submit_parse`] 的实现体。
    pub fn submit(&self, work_id: &str, options: &Map<String, Value>) -> bool {
        self.worker.submit(work_id.to_string(), options.clone())
    }

    /// [`DraftHost::cancel_parse`] 的实现体。
    pub fn cancel(&self, work_id: &str) {
        self.worker.cancel(work_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_create::{HttpParser, Prober};
    use gd3_net::probe::{ProbeOptions, RemoteFile};
    use std::cell::RefCell;

    /// 可控闸门：让桩探测器卡在 `probe` 里，好把「取消」精确地插进解析途中。
    ///
    /// `entered` 在探测开始时发一条；`release` 上等着，直到测试放手。
    struct Gate {
        entered: Sender<()>,
        release: Mutex<Receiver<()>>,
    }

    impl Gate {
        fn new() -> (Arc<Gate>, Receiver<()>, Sender<()>) {
            let (entered_tx, entered_rx) = channel::<()>();
            let (release_tx, release_rx) = channel::<()>();
            let gate = Arc::new(Gate {
                entered: entered_tx,
                release: Mutex::new(release_rx),
            });
            (gate, entered_rx, release_tx)
        }

        /// 探测一开始就报到，然后卡住等放行（`sender` 掉了也不会卡死）。
        fn block(&self) {
            let _ = self.entered.send(());
            if let Ok(release) = self.release.lock() {
                let _ = release.recv();
            }
        }
    }

    /// 桩探测器：URL 里带 `fail` 就失败；带 `gate` 就先过闸门；其余回固定大小。
    struct StubProber {
        gate: Option<Arc<Gate>>,
    }

    impl Prober for StubProber {
        fn probe(&self, options: &ProbeOptions) -> Result<RemoteFile, TaskError> {
            if let (true, Some(gate)) = (options.url.contains("gate"), &self.gate) {
                gate.block();
            }
            if options.url.contains("fail") {
                return Err(TaskError::new("探测失败", Vec::new()));
            }
            Ok(RemoteFile {
                name: options.url.rsplit('/').next().unwrap_or("x").to_string(),
                file_size: 42,
                can_use_range_requests: true,
                last_modified: String::new(),
                content_type: "application/octet-stream".to_string(),
            })
        }
    }

    fn worker_with(prober: StubProber) -> ParseWorker {
        let parser = HttpParser::new(Box::new(prober), CreateDefaults::default());
        ParseWorker::with_parser(parser).expect("spawn worker")
    }

    fn worker() -> ParseWorker {
        worker_with(StubProber { gate: None })
    }

    fn options(url: &str) -> Map<String, Value> {
        let mut options = Map::new();
        options.insert("url".to_string(), Value::String(url.to_string()));
        options
    }

    fn parsed(outcome: ParseOutcome) -> TaskRecord {
        match outcome {
            ParseOutcome::Parsed { task, .. } => *task,
            ParseOutcome::Failed { error, .. } => panic!("expected parsed, got {error:?}"),
        }
    }

    #[test]
    fn a_submitted_parse_comes_back_as_a_record() {
        let worker = worker();
        assert!(worker.submit("draft-1", options("http://example.com/a.bin")));
        let task = parsed(
            worker
                .recv_timeout(Duration::from_secs(10))
                .expect("outcome"),
        );
        assert_eq!(task.name, "a.bin");
        assert_eq!(task.file_size, 42);
        assert_eq!(task.url, "http://example.com/a.bin");
        worker.shutdown();
    }

    #[test]
    fn a_failing_probe_comes_back_as_an_error() {
        let worker = worker();
        assert!(worker.submit("draft-1", options("http://example.com/fail.bin")));
        match worker
            .recv_timeout(Duration::from_secs(10))
            .expect("outcome")
        {
            ParseOutcome::Failed { work_id, error } => {
                assert_eq!(work_id, "draft-1");
                assert_eq!(error.message, "探测失败");
            }
            ParseOutcome::Parsed { task, .. } => panic!("expected failure, got {task:?}"),
        }
        worker.shutdown();
    }

    #[test]
    fn a_cancelled_work_id_never_reports_back() {
        let worker = worker();
        assert!(worker.submit("draft-1", options("http://example.com/a.bin")));
        worker.cancel("draft-1");
        assert!(worker.recv_timeout(Duration::from_millis(500)).is_none());
        assert!(worker.drain().is_empty());
        worker.shutdown();
    }

    #[test]
    fn cancelled_ids_are_forgotten_once_they_land() {
        let worker = worker();
        assert!(worker.submit("draft-1", options("http://example.com/a.bin")));
        let _ = worker
            .recv_timeout(Duration::from_secs(10))
            .expect("outcome");
        assert!(worker.cancelled.lock().unwrap().is_empty());
        worker.shutdown();
    }

    #[test]
    fn the_worker_runs_jobs_one_at_a_time_in_order() {
        let worker = worker();
        assert!(worker.submit("draft-1", options("http://example.com/a.bin")));
        assert!(worker.submit("draft-2", options("http://example.com/b.bin")));
        let first = worker.recv_timeout(Duration::from_secs(10)).expect("first");
        let second = worker
            .recv_timeout(Duration::from_secs(10))
            .expect("second");
        assert_eq!(first.work_id(), "draft-1");
        assert_eq!(second.work_id(), "draft-2");
        assert_eq!(parsed(first).name, "a.bin");
        assert_eq!(parsed(second).name, "b.bin");
        worker.shutdown();
    }

    /// 提交端没了以后 `submit` 返回 `false`（宿主据此发现 worker 没了）。
    #[test]
    fn submit_without_a_sender_reports_false() {
        let mut worker = worker();
        *worker.tx.lock().expect("tx") = None;
        assert!(!worker.submit("draft-1", options("http://example.com/a.bin")));
        if let Some(thread) = worker.thread.take() {
            let _ = thread.join();
        }
    }

    // -----------------------------------------------------------------
    // 灌进对话框状态机
    // -----------------------------------------------------------------

    /// 异步宿主：`submit_parse` 直接丢给 worker，解析结果由 `drain_into` 回灌。
    struct AsyncHost {
        submitter: WorkerSubmitter,
        log: RefCell<Vec<String>>,
    }

    impl AsyncHost {
        fn new(worker: Arc<ParseWorker>) -> Self {
            Self {
                submitter: WorkerSubmitter::new(worker),
                log: RefCell::new(Vec::new()),
            }
        }
    }

    impl DraftHost for AsyncHost {
        fn submit_parse(
            &self,
            work_id: &str,
            options: &Map<String, Value>,
        ) -> crate::task_draft_ui::ParseSubmit {
            assert!(self.submitter.submit(work_id, options));
            crate::task_draft_ui::ParseSubmit::Async
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
    }

    fn ui() -> TaskDraftUi {
        TaskDraftUi::new(Default::default(), "/tmp", &[], 8)
    }

    /// 等 worker 出结果，回灌进对话框；返回最后一条效果列表。
    fn settle(worker: &ParseWorker, ui: &mut TaskDraftUi, host: &dyn DraftHost) -> Vec<UiEffect> {
        let outcome = worker
            .recv_timeout(Duration::from_secs(10))
            .expect("outcome");
        let mut effects = Vec::new();
        match outcome {
            ParseOutcome::Parsed { work_id, task } => {
                effects.extend(ui.on_parsed(&work_id, *task, host));
            }
            ParseOutcome::Failed { work_id, error } => {
                effects.extend(ui.on_parse_failed(&work_id, error, host));
            }
        }
        effects
    }

    fn succeeded(effects: &[UiEffect]) -> Option<&TaskRecord> {
        effects.iter().find_map(|effect| match effect {
            UiEffect::Draft(draft) => match &**draft {
                crate::task_draft::DraftEffect::Succeeded { task, .. } => Some(task),
                _ => None,
            },
            _ => None,
        })
    }

    #[test]
    fn drain_into_feeds_the_draft_dialog() {
        let worker = Arc::new(worker());
        let host = AsyncHost::new(Arc::clone(&worker));
        let mut ui = ui();
        ui.text_changed("http://example.com/a.bin");

        let effects = ui.parse_needed(&host);
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, UiEffect::ParseSubmitted { .. })),
            "异步提交应该报一条 ParseSubmitted：{effects:?}"
        );
        assert!(ui.draft().is_parsing());

        let effects = settle(&worker, &mut ui, &host);
        let task = succeeded(&effects).expect("Succeeded 效果");
        assert_eq!(task.name, "a.bin");
        assert_eq!(task.file_size, 42);
        assert!(!ui.draft().is_parsing());

        // 快照里能看到这一行（名字 + 可读尺寸）。
        let view = ui.view(&host);
        assert_eq!(view.rows.len(), 1);
        assert_eq!(view.rows[0].name, "a.bin");
        assert!(view.can_confirm);
    }

    #[test]
    fn drain_into_reports_the_failure_to_the_host() {
        let worker = Arc::new(worker());
        let host = AsyncHost::new(Arc::clone(&worker));
        let mut ui = ui();
        ui.text_changed("http://example.com/fail.bin");
        ui.parse_needed(&host);

        let effects = settle(&worker, &mut ui, &host);
        // `DraftEffect::Failed` 在 `TaskDraftUi::run` 里就被翻成一条 InfoBar 文案。
        let failed = effects.iter().any(|effect| {
            matches!(effect, UiEffect::ParseError { title, .. } if title == "链接解析失败")
        });
        assert!(failed, "应该有一条 ParseError：{effects:?}");
        assert_eq!(ui.draft().fail_count(), 1);
    }

    #[test]
    fn a_cancelled_parse_leaves_the_dialog_alone() {
        let worker = Arc::new(worker());
        let host = AsyncHost::new(Arc::clone(&worker));
        let mut ui = ui();
        ui.text_changed("http://example.com/a.bin");
        ui.parse_needed(&host);
        assert!(ui.draft().is_parsing());

        // 清空 URL 文本 → `set_urls(&[])` → 取消旧的解析。
        ui.text_changed("");
        let effects = ui.parse_needed(&host);
        assert!(!ui.draft().is_parsing(), "取消后不该还在解析：{effects:?}");

        // 桩探测是毫秒级，「取消」和「结果落地」本来就在赛跑：两种结局都合法，
        // 契约是**对话框不受影响**（真正「跑着取消 → 结果丢弃」见
        // `a_cancelled_inflight_parse_is_dropped`，那条用闸门钉住了时序）。
        let _ = worker.recv_timeout(Duration::from_millis(100));
        assert!(drain_into(&worker, &mut ui, &host).is_empty());
        assert!(ui.draft().items().is_empty());
        assert_eq!(ui.draft().fail_count(), 0);
    }

    /// 解析跑到一半被取消：结果必须丢掉，别去动已经清空的草稿项。
    #[test]
    fn a_cancelled_inflight_parse_is_dropped() {
        let (gate, entered, release) = Gate::new();
        let worker = Arc::new(worker_with(StubProber { gate: Some(gate) }));
        let host = AsyncHost::new(Arc::clone(&worker));
        let mut ui = ui();
        ui.text_changed("http://example.com/gate.bin");
        ui.parse_needed(&host);
        // 等探测真的进到 `probe` 里，再取消——这样取消一定发生在解析途中。
        entered
            .recv_timeout(Duration::from_secs(10))
            .expect("探测应该已经开跑");
        ui.text_changed("");
        ui.parse_needed(&host);
        assert!(!ui.draft().is_parsing(), "取消后立刻不算在解析");

        let _ = release.send(());
        assert!(
            worker.recv_timeout(Duration::from_millis(500)).is_none(),
            "取消掉的那一条不该再有结果回来"
        );
        assert!(drain_into(&worker, &mut ui, &host).is_empty());
        assert!(ui.draft().items().is_empty());
        assert_eq!(ui.draft().fail_count(), 0);
    }

    #[test]
    fn confirmed_tasks_keep_the_probed_record() {
        let worker = Arc::new(worker());
        let host = AsyncHost::new(Arc::clone(&worker));
        let mut ui = ui();
        ui.text_changed("http://example.com/a.bin");
        ui.parse_needed(&host);
        let _ = settle(&worker, &mut ui, &host);

        let effects = ui.accept(true, &host);
        let confirmed = effects.iter().find_map(|effect| match effect {
            UiEffect::Draft(draft) => match &**draft {
                crate::task_draft::DraftEffect::Confirmed { task, auto_start } => {
                    Some((task.clone(), *auto_start))
                }
                _ => None,
            },
            _ => None,
        });
        let (task, auto_start) = confirmed.expect("Confirmed 效果");
        assert!(auto_start);
        assert_eq!(task.name, "a.bin");
        assert_eq!(task.file_size, 42);
        // 探测出来的 `fileSize` 在 step.extra 里也是 42（落库形状由第 137 步钉死）。
        assert_eq!(task.steps[0].extra.get("fileSize"), Some(&Value::from(42)));
        assert!(ui.draft().items().is_empty());
    }

    #[test]
    fn a_late_result_for_a_cleared_draft_is_harmless() {
        let worker = Arc::new(worker());
        let host = AsyncHost::new(Arc::clone(&worker));
        let mut ui = ui();
        ui.text_changed("http://example.com/a.bin");
        ui.parse_needed(&host);
        let task = succeeded(&settle(&worker, &mut ui, &host))
            .expect("Succeeded 效果")
            .clone();

        // 同一个 work_id 再回灌一次：`TaskDraft::on_parsed` 查不到（parseId 已清空）。
        let late = ui.on_parsed("draft-1", task, &host);
        assert!(late.is_empty(), "迟到结果应该无声丢弃：{late:?}");
        assert_eq!(ui.draft().items().len(), 1);
        assert_eq!(ui.draft().fail_count(), 0);
    }
}
