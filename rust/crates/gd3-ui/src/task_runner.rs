//! Phase 8e：把 [`gd3_net::HttpEngine`] 接进运行期（任务真的能下载）。
//!
//! 镜像的 Python 三件套：
//!
//! | Python | 本模块 |
//! |---|---|
//! | `TaskService._pump` → `_dispatch`（`task_service.py:649-583`） | [`TaskRunner::pump`] / [`TaskRunner::dispatch`] |
//! | `Task.run` 的步骤循环（`app/models/task.py:455-470`） | 工作线程里的 `run_job`（见 [`ThreadSpawner`]） |
//! | `HttpTask.run` 的传输（`features/http_pack/task.py:375`） | [`gd3_net::HttpEngine`]（Phase 5/6 已迁完，Python 侧同源走 `rust_engine.py`） |
//!
//! # 为什么是「线程 + 通道」而不是 async
//!
//! `gd3-ui` 的其余部分（`task_service` / `settings_bridge` / Slint 回调）全是**同步单线程**
//! 的：状态机是纯函数、宿主事件靠 [`slint::Timer`] 轮询。下载引擎要 tokio，所以这一层
//! 做成「工作线程跑 tokio，UI 线程按 tick 收发事件」——两边不共享任何 `TaskService`，
//! 也就不用给整个 UI 上 `Send + Sync`。
//!
//! ```text
//! UI 线程                         工作线程（tokio runtime）
//! TaskRunner::pump()  ─ job ─▶    run_job：逐步跑 HttpEngine
//! TaskRunner::drain() ◀─ 事件 ─   Progress / StepFinished / TaskFinished / …
//! ```
//!
//! # 账目口径（与 Python 对齐的几处）
//!
//! - **进度不改盘**：Python 的 `HttpTask._supervise` 只改 `step.receivedBytes/progress/speed`，
//!   `_flushSoon` 只出现在 `_onRunDone` / `_onRunFailed` / `pause`。这里同样只在
//!   完成 / 失败 / 暂停时 `mark_dirty`；
//! - **`reportSpeed` 只收数据块增量**：`_runSubworker` 每块调一次 `reportSpeed(delta)`，
//!   每秒采样时为 0。所以 [`TaskRunner`] 只把 `chunk_delta` 交给
//!   [`TaskService::add_speed`]，`step.speed` 用同一份采样值；
//! - **`.ghd` 在整任务成功后才删**：Python 是 `Task.run` 末尾
//!   `if self.status == COMPLETED: deletePath(self.partPath)`；取消时**保留**
//!   （`HttpEngine::run` 被取消时同样保留），失败时也保留（分片记录有用）；
//! - **取消不回调 `_onRunDone`**：`_cancelWork` 走的是 `coroutineRunner.cancel(workId,
//!   finished=...)`，`_queue.cancel` 已经把它从运行集合里摘掉了，所以 [`RunnerEvent::Cancelled`]
//!   只把工作线程的记录删掉，不再做队列记账（否则会 `queue.done` 两次）。
//!
//! # 与 Python 的已知差异
//!
//! - **只支持 HTTP 步骤**：`features/http_pack` 的解析器还是 Python，Rust 侧没有建任务
//!   链路，所以 [`TaskRunner`] 只认「带 `url` 的步骤」。其它 pack 的任务（BT / FTP /
//!   ed2k / m3u8）派发时直接按「没有待跑步骤」处理（等价于 Python 的
//!   `pendingSteps()` 为空：`Task.run` 立刻返回），既不假装成功也不报错；
//! - **限速钩子未接**：Python 每个数据块后 `await waitForSpeedLimit()`。Rust 侧
//!   `SpeedGate` 已在 `gd3-net` 里留好接口（[`HttpEngine::speed_gate`]），但
//!   `SpeedMeter::wait_for_speed_limit` 目前是同步判定，跨线程等待要另起一层队列，
//!   留到 8g 之后；
//! - **`shouldPreserveLastModified` 未接**：Python 在 `rust_engine.py` 里 `os.utime`，
//!   工作线程拿到的是 `lastModified` 字符串（HTTP `Last-Modified`），但
//!   [`gd3_net::HttpEngine`] 目前不回传响应头，所以这一条留 TODO。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, atomic::AtomicU64, atomic::Ordering};
use std::time::{Duration, Instant};

use gd3_core::config::UserConfig;
use gd3_core::settings;
use gd3_core::task::{TaskRecord, TaskStatus, TaskStep};
use gd3_core::task_error::TaskError;
use gd3_net::{CancellationToken, HttpEngine, TransferProgress};
use serde_json::Value;
use tokio::runtime::Runtime;

use crate::speed_limit::SpeedLimit;
use crate::task_service::{PauseReport, TaskService};

/// Python `rust_engine.RETRY_DELAY_MS`。
pub const RETRY_DELAY: Duration = Duration::from_millis(5000);

/// `HttpTaskStep.subworkerCount` 的默认值（`features/http_pack/task.py:69`）。
pub const DEFAULT_SUBWORKER_COUNT: usize = 8;

/// 运行期需要从 `cfg` 现读的九项。
///
/// Python 里这些是全局 `cfg` 的 `ConfigItem`，每次用到时现读；Rust 侧由宿主在
/// 每轮 tick 用 [`RunnerSettings::from_config`] 重新取一份（设置页改完立刻生效）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerSettings {
    /// `cfg.maxTaskNum`（`_pump` 的并发上限）。
    pub max_task_num: usize,
    /// 没有 `subworkerCount` 的旧步骤用它。
    pub subworker_count: usize,
    /// `cfg.clientProfile`（步骤自己没给时用它）。
    pub client_profile: String,
    /// 重试间隔（Python 侧是模块常量 `RETRY_DELAY_MS`）。
    pub retry_delay: Duration,
    /// `cfg.maxReassignSize` × 1024。
    pub max_reassign_size: u64,
    /// `cfg.autoSpeedUp`。
    pub auto_speed_up: bool,
    /// `cfg.shouldPreserveLastModified`（跑完把 mtime 设成响应头里的时间）。
    pub preserve_last_modified: bool,
    /// `cfg.isSpeedLimitEnabled`（限速总开关）。
    pub speed_limit_enabled: bool,
    /// `cfg.speedLimitation`（字节/秒，缺省 4 MiB/s）。
    pub speed_limit_bytes: i64,
}

impl Default for RunnerSettings {
    fn default() -> Self {
        RunnerSettings {
            max_task_num: 3,
            subworker_count: DEFAULT_SUBWORKER_COUNT,
            client_profile: "auto".to_string(),
            retry_delay: RETRY_DELAY,
            max_reassign_size: 512 * 1024,
            auto_speed_up: true,
            preserve_last_modified: false,
            speed_limit_enabled: false,
            speed_limit_bytes: 4_194_304,
        }
    }
}

impl RunnerSettings {
    /// 从配置读六项（每项都过 `Spec::read` 的 `correct()` 口径）。
    pub fn from_config(config: &UserConfig) -> Self {
        let defaults = RunnerSettings::default();
        RunnerSettings {
            max_task_num: crate::engine::max_task_num(config),
            subworker_count: defaults.subworker_count,
            client_profile: settings::read_text(config, "clientProfile")
                .unwrap_or(defaults.client_profile),
            retry_delay: defaults.retry_delay,
            max_reassign_size: settings::read_i64(config, "maxReassignSize")
                .map(|value| (value.max(0) as u64) * 1024)
                .unwrap_or(defaults.max_reassign_size),
            auto_speed_up: settings::read_bool(config, "autoSpeedUp")
                .unwrap_or(defaults.auto_speed_up),
            preserve_last_modified: settings::read_bool(config, "shouldPreserveLastModified")
                .unwrap_or(defaults.preserve_last_modified),
            speed_limit_enabled: settings::read_bool(config, "isSpeedLimitEnabled")
                .unwrap_or(defaults.speed_limit_enabled),
            speed_limit_bytes: settings::read_i64(config, "speedLimitation")
                .unwrap_or(defaults.speed_limit_bytes),
        }
    }
}

// ---------------------------------------------------------------------------
// 计划层：TaskRecord → DownloadJob
// ---------------------------------------------------------------------------

/// 一个待跑的 HTTP 步骤（`HttpTaskStep` 的传输输入）。
#[derive(Debug, Clone, PartialEq)]
pub struct DownloadStep {
    /// 在 `record.steps` 里的**下标**（不是 `stepIndex` 字段）。
    pub position: usize,
    pub url: String,
    pub output_path: PathBuf,
    pub file_size: u64,
    pub headers: Vec<(String, String)>,
    pub user_agent: String,
    pub client_profile: String,
    pub subworker_count: usize,
    pub can_use_range_requests: bool,
    pub http_byte_offset: u64,
    /// 响应头里的 `Last-Modified`（`HttpTaskStep.lastModified`，可能为空/缺失）。
    ///
    /// 只有 `shouldPreserveLastModified` 打开时才会用到（[`DownloadJob::preserve_last_modified`]）。
    pub last_modified: Option<String>,
}

/// 一个 Task 的一次运行：按 `stepIndex` 顺序跑完所有待跑步骤。
#[derive(Debug, Clone, PartialEq)]
pub struct DownloadJob {
    pub work_id: String,
    pub task_id: String,
    pub steps: Vec<DownloadStep>,
    pub retry_delay: Duration,
    pub max_reassign_size: u64,
    pub auto_speed_up: bool,
    /// `cfg.shouldPreserveLastModified` 在派发时的快照。
    pub preserve_last_modified: bool,
}

impl DownloadJob {
    /// 本次运行覆盖的步骤下标（暂停 / 完成时用来对账）。
    pub fn positions(&self) -> Vec<usize> {
        self.steps.iter().map(|step| step.position).collect()
    }
}

/// `Task.pendingSteps()`（`app/models/task.py:420-428`）的下标版。
///
/// 三个口径照抄：按 `stepIndex` **升序**（不是数组序）；任务不是 `RUNNING` 时**一个都不给**
/// （Python 的生成器里是 `break`，去掉状态检查会让暂停中的任务被重新派发）；已完成的步骤跳过。
/// `_isStepSelected` 走 [`TaskRecord::is_step_selected`]。
pub fn pending_positions(record: &TaskRecord) -> Vec<usize> {
    if record.status != TaskStatus::Running {
        return Vec::new();
    }
    let mut ordered: Vec<usize> = (0..record.steps.len()).collect();
    ordered.sort_by_key(|index| record.steps[*index].step_index);
    ordered
        .into_iter()
        .filter(|index| record.is_step_selected(&record.steps[*index]))
        .filter(|index| record.steps[*index].status != TaskStatus::Completed)
        .collect()
}

/// `Task.toFilePath(step.fileIndex)`（`app/models/task.py:220-225`）。
///
/// 多文件任务（`isOutputFolder`）落到 `outputPath/relativePath`，否则就是 `outputPath`。
pub fn step_output_path(record: &TaskRecord, step: &TaskStep) -> PathBuf {
    if record.is_output_folder()
        && let Some(file_index) = step.file_index
        && let Some(files) = &record.files
        && let Some(file) = files.iter().find(|file| file.index == file_index)
    {
        return PathBuf::from(gd3_core::path::posix_join(
            &record.output_path(),
            &file.relative_path,
        ));
    }
    PathBuf::from(record.output_path())
}

/// 把一条记录摊成本次要跑的步骤列表。
///
/// 步骤字段全部来自 `TaskStep.extra`（Python 侧 `HttpTaskStep` 的 dataclass 字段被
/// `toDict` 平铺进同一条 JSON 里）：`url` / `fileSize` / `headers` / `clientProfile` /
/// `userAgent` / `subworkerCount` / `canUseRangeRequests` / `httpByteOffset`。
/// 没有 `url` 的步骤**不算 HTTP 步骤**，直接跳过（见模块头的「已知差异」）。
pub fn plan_job(
    record: &TaskRecord,
    settings: &RunnerSettings,
    work_id: impl Into<String>,
) -> DownloadJob {
    let steps = pending_positions(record)
        .into_iter()
        .filter_map(|position| {
            let step = &record.steps[position];
            let url = step_text(step, "url")?;
            Some(DownloadStep {
                position,
                url,
                output_path: step_output_path(record, step),
                file_size: step_u64(step, "fileSize").unwrap_or(record.file_size.max(0) as u64),
                headers: step_headers(step),
                user_agent: step_text(step, "userAgent").unwrap_or_default(),
                client_profile: step_text(step, "clientProfile")
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| settings.client_profile.clone()),
                subworker_count: step_u64(step, "subworkerCount")
                    .map(|value| (value.max(1) as usize).max(1))
                    .unwrap_or(settings.subworker_count),
                can_use_range_requests: step_bool(step, "canUseRangeRequests").unwrap_or(false),
                http_byte_offset: step_u64(step, "httpByteOffset").unwrap_or(0),
                last_modified: step_text(step, "lastModified").filter(|value| !value.is_empty()),
            })
        })
        .collect();

    DownloadJob {
        work_id: work_id.into(),
        task_id: record.task_id.clone(),
        steps,
        retry_delay: settings.retry_delay,
        max_reassign_size: settings.max_reassign_size,
        auto_speed_up: settings.auto_speed_up,
        preserve_last_modified: settings.preserve_last_modified,
    }
}

fn step_value(step: &TaskStep, key: &str) -> Option<Value> {
    step.extra.get(key).cloned()
}

fn step_text(step: &TaskStep, key: &str) -> Option<String> {
    match step_value(step, key)? {
        Value::String(text) => Some(text),
        Value::Null => None,
        other => other.as_str().map(str::to_string),
    }
}

fn step_u64(step: &TaskStep, key: &str) -> Option<u64> {
    match step_value(step, key)? {
        Value::Number(number) => number.as_u64().or_else(|| {
            number
                .as_f64()
                .filter(|value| *value >= 0.0)
                .map(|value| value as u64)
        }),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

fn step_bool(step: &TaskStep, key: &str) -> Option<bool> {
    match step_value(step, key)? {
        Value::Bool(value) => Some(value),
        Value::Number(number) => Some(number.as_f64().unwrap_or(0.0) != 0.0),
        Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" => Some(true),
            "0" | "false" | "no" | "" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// `headers` 是 `{name: value}` 对象（Python 的 `dict[str, str]`）。
fn step_headers(step: &TaskStep) -> Vec<(String, String)> {
    match step_value(step, "headers") {
        Some(Value::Object(map)) => map
            .into_iter()
            .map(|(name, value)| {
                let text = match value {
                    Value::String(text) => text,
                    other => other.to_string(),
                };
                (name, text)
            })
            .collect(),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// 事件与工作线程
// ---------------------------------------------------------------------------

/// 工作线程发回 UI 线程的事件。
///
/// 每条都带 `work_id`：暂停后同一任务会立刻重新派发（新的 `work_id`），旧协程的尾巴
/// 事件必须能被认出来扔掉，否则会把新运行的进度写回去。
#[derive(Debug, Clone, PartialEq)]
pub enum RunnerEvent {
    /// 一次进度采样（`HttpEngine::on_progress`）。
    Progress {
        work_id: String,
        task_id: String,
        position: usize,
        received_bytes: i64,
        progress: f64,
        speed: i64,
        chunk_delta: i64,
    },
    /// 一个步骤跑完了（`Task.run` 里的 `step.setStatus(COMPLETED)`）。
    StepFinished {
        work_id: String,
        task_id: String,
        position: usize,
        can_use_range_requests: bool,
        received_bytes: i64,
    },
    /// 整个任务跑完了。
    TaskFinished { work_id: String, task_id: String },
    /// 一个步骤失败了（`step.setError(toTaskError(e))`）。
    TaskFailed {
        work_id: String,
        task_id: String,
        position: Option<usize>,
        error: TaskError,
    },
    /// 被取消（暂停 / 停机）。
    Cancelled { work_id: String, task_id: String },
}

impl RunnerEvent {
    fn work_id(&self) -> &str {
        match self {
            RunnerEvent::Progress { work_id, .. }
            | RunnerEvent::StepFinished { work_id, .. }
            | RunnerEvent::TaskFinished { work_id, .. }
            | RunnerEvent::TaskFailed { work_id, .. }
            | RunnerEvent::Cancelled { work_id, .. } => work_id,
        }
    }

    fn task_id(&self) -> &str {
        match self {
            RunnerEvent::Progress { task_id, .. }
            | RunnerEvent::StepFinished { task_id, .. }
            | RunnerEvent::TaskFinished { task_id, .. }
            | RunnerEvent::TaskFailed { task_id, .. }
            | RunnerEvent::Cancelled { task_id, .. } => task_id,
        }
    }
}

/// 跑 job 的东西：生产环境是 tokio 工作线程（[`ThreadSpawner`]），测试里是假实现。
pub trait Spawner {
    /// `gate` 是全局限速闸门（`app/services/speed_meter.py` 的 `waitForSpeedLimit`），
    /// 所有 job 共用一份（Python 也是一个 `speedMeter`）。
    fn spawn(
        &self,
        job: DownloadJob,
        cancel: CancellationToken,
        sink: Sender<RunnerEvent>,
        gate: Arc<SpeedLimit>,
    );
    /// 停机（默认空操作）。[`TaskRunner::shutdown`] 在所有工作线程收工之后调它。
    fn shutdown(&mut self) {}
}

/// 生产实现：一个多线程 tokio runtime，所有任务共用（Python 是每任务一个 runtime）。
///
/// `Default` 给的是**没有 runtime** 的形态：`new()` 建失败时 runner 仍要能用
/// （只是不派发），所以 `Option<Runtime>` 而不是 `Runtime`。
#[derive(Default)]
pub struct ThreadSpawner {
    runtime: Option<Runtime>,
}

impl ThreadSpawner {
    /// 建 runtime（失败时返回 `Err`，调用方退回不派发）。
    pub fn new() -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("下载运行时创建失败：{error}"))?;
        Ok(ThreadSpawner {
            runtime: Some(runtime),
        })
    }
}

impl Spawner for ThreadSpawner {
    fn spawn(
        &self,
        job: DownloadJob,
        cancel: CancellationToken,
        sink: Sender<RunnerEvent>,
        gate: Arc<SpeedLimit>,
    ) {
        let Some(runtime) = &self.runtime else {
            // `new` 失败时 runner 仍然可用（只是不派发），所以这里静默返回。
            return;
        };
        runtime.spawn(async move {
            run_job(job, cancel, sink, gate).await;
        });
    }

    fn shutdown(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(Duration::from_secs(2));
        }
    }
}

/// `features/http_pack/rust_engine.py:103-110` 的 `_restoreMtime`：
/// 开关打开且步骤带 `Last-Modified` 时把输出文件的 mtime 设回去。
///
/// Python 侧只是 `logger.warning`，**绝不让任务失败**（很多文件系统/挂载点不支持
/// 改时间戳，这里失败不影响「文件已下完」这个事实），所以错了只打 stderr。
fn restore_last_modified(job: &DownloadJob, step: &DownloadStep) {
    if !job.preserve_last_modified {
        return;
    }
    let Some(raw) = step.last_modified.as_deref() else {
        return;
    };
    if let Err(error) = gd3_core::apply_last_modified(&step.output_path, raw) {
        eprintln!(
            "设置文件修改时间失败 {}：{error}",
            step.output_path.display()
        );
    }
}

/// 逐步跑完一个 job，把过程发成事件（`Task.run` 的循环 + `HttpTask.run` 的传输）。
async fn run_job(
    job: DownloadJob,
    cancel: CancellationToken,
    sink: Sender<RunnerEvent>,
    gate: Arc<SpeedLimit>,
) {
    for step in &job.steps {
        if cancel.is_cancelled() {
            let _ = sink.send(RunnerEvent::Cancelled {
                work_id: job.work_id.clone(),
                task_id: job.task_id.clone(),
            });
            return;
        }

        // `Task.run`：`if step.outputPath: Path(...).parent.mkdir(parents=True, exist_ok=True)`。
        if let Some(parent) = step.output_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let mut engine =
            HttpEngine::new(step.url.clone(), step.output_path.clone(), step.file_size);
        engine.can_use_range_requests = step.can_use_range_requests;
        engine.subworker_count = step.subworker_count;
        engine.headers = step.headers.clone();
        engine.user_agent = step.user_agent.clone();
        engine.client_profile = step.client_profile.clone();
        engine.http_byte_offset = step.http_byte_offset;
        engine.retry_delay = job.retry_delay;
        engine.max_reassign_size = job.max_reassign_size;
        engine.auto_speed_up = job.auto_speed_up;
        engine.speed_gate = Some(Arc::clone(&gate) as Arc<dyn gd3_net::SpeedGate>);

        let received = Arc::new(AtomicU64::new(0));
        let progress_gate = Arc::clone(&gate);
        let progress_sink = sink.clone();
        let progress_work = job.work_id.clone();
        let progress_task = job.task_id.clone();
        let position = step.position;
        let received_ref = Arc::clone(&received);
        engine.on_progress = Some(Arc::new(move |update: TransferProgress| {
            received_ref.store(update.received_bytes, Ordering::Relaxed);
            // `reportSpeed(delta)` → `speedMeter.addSpeed(delta)`，闸门按这个数判断超限。
            progress_gate.add(update.chunk_delta);
            let _ = progress_sink.send(RunnerEvent::Progress {
                work_id: progress_work.clone(),
                task_id: progress_task.clone(),
                position,
                received_bytes: update.received_bytes as i64,
                progress: update.progress,
                speed: update.speed,
                chunk_delta: update.chunk_delta as i64,
            });
        }));

        let outcome = engine.run(cancel.clone()).await;
        let can_use_range_requests = engine.can_use_range_requests;
        let received_bytes = received.load(Ordering::Relaxed) as i64;

        match outcome {
            Ok(()) => {
                restore_last_modified(&job, step);
                let _ = sink.send(RunnerEvent::StepFinished {
                    work_id: job.work_id.clone(),
                    task_id: job.task_id.clone(),
                    position,
                    can_use_range_requests,
                    received_bytes,
                });
            }
            Err(error) if error.is_cancelled() => {
                let _ = sink.send(RunnerEvent::Cancelled {
                    work_id: job.work_id.clone(),
                    task_id: job.task_id.clone(),
                });
                return;
            }
            Err(error) => {
                let _ = sink.send(RunnerEvent::TaskFailed {
                    work_id: job.work_id.clone(),
                    task_id: job.task_id.clone(),
                    position: Some(position),
                    error,
                });
                return;
            }
        }
    }

    let _ = sink.send(RunnerEvent::TaskFinished {
        work_id: job.work_id.clone(),
        task_id: job.task_id.clone(),
    });
}

// ---------------------------------------------------------------------------
// TaskRunner
// ---------------------------------------------------------------------------

/// 一条在跑的任务（`_queue.run(taskId, workId)` 的那份记账）。
struct Running {
    work_id: String,
    cancel: CancellationToken,
    positions: Vec<usize>,
}

/// [`TaskRunner::drain`] 的产物（UI 决定要不要刷新列表 / 弹提示）。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct DrainReport {
    /// 收到进度更新的任务（去重、保序）。
    pub progressed: Vec<String>,
    /// 完成的任务。
    pub finished: Vec<String>,
    /// 失败的任务与错误。
    pub failed: Vec<(String, TaskError)>,
    /// `on_run_finished` 顺带派发出去的任务（`_pump()` 的结果）。
    pub started: Vec<String>,
    /// 最后一轮「运行数为 0」判定（`tasksAllCompleted`）。
    pub all_completed: bool,
    /// 事务上有没有变化（有就 `flush` + 刷 UI）。
    pub changed: bool,
}

impl DrainReport {
    /// 去重追加（关联函数：`&mut self` 与字段借用会撞在一起）。
    fn note_task(list: &mut Vec<String>, task_id: &str) {
        if !list.iter().any(|existing| existing == task_id) {
            list.push(task_id.to_string());
        }
    }
}

/// 任务运行器：`_pump` / `_dispatch` / 事件回灌 三件事。
pub struct TaskRunner {
    settings: RunnerSettings,
    /// 全局限速闸门（`speed_meter.py:39-41`）。工作线程读、UI 线程写。
    speed_limit: Arc<SpeedLimit>,
    spawner: Box<dyn Spawner>,
    sender: Sender<RunnerEvent>,
    events: Receiver<RunnerEvent>,
    running: HashMap<String, Running>,
    next_work_id: u64,
    shutdown_attempted: bool,
}

impl TaskRunner {
    /// 用给定的 spawner 建一个（测试用假 spawner）。
    pub fn with_spawner(settings: RunnerSettings, spawner: Box<dyn Spawner>) -> Self {
        let (sender, events) = channel();
        let speed_limit = SpeedLimit::new();
        speed_limit.configure(settings.speed_limit_enabled, settings.speed_limit_bytes);
        TaskRunner {
            settings,
            speed_limit,
            spawner,
            sender,
            events,
            running: HashMap::new(),
            next_work_id: 0,
            shutdown_attempted: false,
        }
    }

    /// 生产入口：tokio 工作线程。
    pub fn new(settings: RunnerSettings) -> Self {
        let spawner = Box::new(ThreadSpawner::new().unwrap_or_default());
        TaskRunner::with_spawner(settings, spawner)
    }

    pub fn settings(&self) -> &RunnerSettings {
        &self.settings
    }

    /// 每轮 tick 现读配置（`cfg.maxTaskNum` 之类的改动立刻生效）。
    ///
    /// 限速的两项要顺手推给闸门：那个对象是 `Arc` 共享给工作线程的，
    /// 不同步就只有 UI 见过新值（`waitForSpeedLimit` 每轮直接读 `cfg`，等价于这里推）。
    pub fn set_settings(&mut self, settings: RunnerSettings) {
        self.speed_limit
            .configure(settings.speed_limit_enabled, settings.speed_limit_bytes);
        self.settings = settings;
    }

    /// 全局限速闸门（宿主每秒 `reset` 一次，见 `runtime::tick_host`）。
    pub fn speed_limit(&self) -> &Arc<SpeedLimit> {
        &self.speed_limit
    }

    pub fn is_running(&self, task_id: &str) -> bool {
        self.running.contains_key(task_id)
    }

    /// 某次运行覆盖的步骤下标（[`DownloadJob::positions`] 的存档，诊断用）。
    pub fn running_positions(&self, task_id: &str) -> Option<&[usize]> {
        self.running
            .get(task_id)
            .map(|running| running.positions.as_slice())
    }

    /// 在跑的任务 id（`_queue.runningIds()` 的镜像，顺序不保证）。
    pub fn running_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.running.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// `TaskService._pump()` + 逐个 `_dispatch(task)`。
    ///
    /// 返回**真的派发出去**的任务 id（`taskStarted` 的落点）。`TaskService::pump` 已经
    /// 按 `maxTaskNum` 限额把队列状态改好了，这里只补「建 job + 起工作线程」。
    pub fn pump(&mut self, tasks: &mut TaskService) -> Vec<String> {
        let queued = tasks.pump(self.settings.max_task_num);
        let mut dispatched = Vec::new();
        for task_id in queued {
            if self.dispatch(tasks, &task_id) {
                dispatched.push(task_id);
            }
        }
        dispatched
    }

    /// `_dispatch(task)`：置 `RUNNING` + `queue.run(taskId, workId)` + 起工作线程。
    ///
    /// 任务不在服务里 → `false`（Python 的 `_pump` 同样跳过）。
    pub fn dispatch(&mut self, tasks: &mut TaskService, task_id: &str) -> bool {
        if self.running.contains_key(task_id) {
            return false;
        }
        if tasks.task(task_id).is_none() {
            return false;
        }
        let work_id = self.next_work_id();

        // Python 顺序：`task.setStatus(RUNNING)` → 提交协程 → `queue.run(id, workId)`。
        // 状态必须先置：`Task.run` 里这时才读 `pendingSteps()`，而它在任务不是
        // `RUNNING` 时一条都不给（先算 job 会得到空计划）。
        tasks.dispatch(task_id, &work_id);

        let record = tasks.task(task_id).expect("刚置过状态");
        let job = plan_job(record, &self.settings, work_id.clone());
        let positions = job.positions();

        let cancel = CancellationToken::new();
        self.running.insert(
            task_id.to_string(),
            Running {
                work_id: work_id.clone(),
                cancel: cancel.clone(),
                positions,
            },
        );
        self.spawner.spawn(
            job,
            cancel,
            self.sender.clone(),
            Arc::clone(&self.speed_limit),
        );
        true
    }

    /// 收回所有事件并记账。返回这一轮发生了什么。
    pub fn drain(&mut self, tasks: &mut TaskService) -> DrainReport {
        let mut report = DrainReport::default();
        loop {
            let event = match self.events.try_recv() {
                Ok(event) => event,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            };

            // 过期的运行（暂停后重派发、或已被删掉的任务）：整条扔掉。
            let Some(running) = self.running.get(event.task_id()) else {
                continue;
            };
            if running.work_id != event.work_id() {
                continue;
            }
            let task_id = event.task_id().to_string();

            match event {
                RunnerEvent::Progress {
                    position,
                    received_bytes,
                    progress,
                    speed,
                    chunk_delta,
                    ..
                } => {
                    if let Some(record) = tasks.task_mut(&task_id)
                        && let Some(step) = record.steps.get_mut(position)
                    {
                        step.received_bytes = received_bytes;
                        step.progress = gd3_core::task::Progress::from_percent(progress);
                        step.speed = speed;
                    }
                    // `reportSpeed(delta)` → `speedMeter.addSpeed(delta)`。
                    if chunk_delta != 0 {
                        tasks.add_speed(chunk_delta);
                    }
                    DrainReport::note_task(&mut report.progressed, &task_id);
                }
                RunnerEvent::StepFinished {
                    position,
                    can_use_range_requests,
                    received_bytes,
                    ..
                } => {
                    if let Some(record) = tasks.task_mut(&task_id)
                        && let Some(step) = record.steps.get_mut(position)
                    {
                        step.received_bytes = received_bytes;
                        step.status = TaskStatus::Completed;
                        step.progress = gd3_core::task::Progress::Int(100);
                        step.speed = 0;
                        // `rust_engine.run` 回写降级后的取值。
                        step.extra.insert(
                            "canUseRangeRequests".to_string(),
                            Value::Bool(can_use_range_requests),
                        );
                        step.extra
                            .insert("subworkers".to_string(), Value::Array(Vec::new()));
                    }
                    if let Some(record) = tasks.task_mut(&task_id) {
                        record.update_status();
                    }
                    report.changed = true;
                }
                RunnerEvent::TaskFinished { .. } => {
                    self.running.remove(&task_id);
                    self.delete_part_record(tasks, &task_id);
                    let outcome = tasks.on_run_finished(&task_id, self.settings.max_task_num);
                    DrainReport::note_task(&mut report.finished, &task_id);
                    report.started.extend(outcome.started);
                    report.all_completed = outcome.all_completed;
                    report.changed = true;
                }
                RunnerEvent::TaskFailed {
                    position, error, ..
                } => {
                    if let (Some(position), Some(record)) = (position, tasks.task_mut(&task_id))
                        && let Some(step) = record.steps.get_mut(position)
                    {
                        step.status = TaskStatus::Failed;
                        step.speed = 0;
                    }
                    if let Some(record) = tasks.task_mut(&task_id) {
                        record.update_status();
                        record.set_status(TaskStatus::Failed);
                    }
                    self.running.remove(&task_id);
                    let outcome = tasks.on_run_finished(&task_id, self.settings.max_task_num);
                    report.failed.push((task_id, error));
                    report.started.extend(outcome.started);
                    report.all_completed = outcome.all_completed;
                    report.changed = true;
                }
                RunnerEvent::Cancelled { .. } => {
                    // 暂停 / 停机已经在别处记完账（见模块头第 4 条），这里只收摊。
                    self.running.remove(&task_id);
                    report.changed = true;
                }
            }
        }

        // 事件里带出来的新派发（`_onRunDone` 尾部的 `_pump()`）。
        let started = report.started.clone();
        for task_id in started {
            if !self.running.contains_key(&task_id) {
                self.dispatch(tasks, &task_id);
            }
        }
        report
    }

    /// `pause(task)`：先取消工作线程，再走 [`TaskService::pause`] 的记账。
    ///
    /// 顺序不能反：`TaskService::pause` 会撤销队列里的 workId，之后再取消也只是幂等。
    pub fn pause(
        &mut self,
        tasks: &mut TaskService,
        task_id: &str,
        max_task_num: usize,
    ) -> Option<PauseReport> {
        if let Some(running) = self.running.remove(task_id) {
            running.cancel.cancel();
        }
        tasks.pause(task_id, max_task_num)
    }

    /// 取消所有在跑的工作（停机用；`.ghd` 会保住）。
    pub fn cancel_all(&mut self) {
        for running in self.running.values() {
            running.cancel.cancel();
        }
    }

    /// 停机：取消全部 → 等工作线程收工（最多 `wait`）→ 关运行时。
    ///
    /// 返回值是**超时前没收到收尾事件**的任务 id（正常情况下是空的）。
    pub fn shutdown(&mut self, wait: Duration) -> Vec<String> {
        // 先放开限速闸门：卡在 `wait()` 里的工作线程要靠它返回 —— 停机之后没人再
        // `reset` 那个计数窗口，闸门会把它们一直按住。
        self.speed_limit.close();
        self.cancel_all();
        let deadline = Instant::now() + wait;
        while !self.running.is_empty() && Instant::now() < deadline {
            let mut settled = Vec::new();
            while let Ok(event) = self.events.try_recv() {
                settled.push(event);
            }
            for event in settled {
                if let Some(running) = self.running.get(event.task_id())
                    && running.work_id == event.work_id()
                    && matches!(
                        event,
                        RunnerEvent::Cancelled { .. }
                            | RunnerEvent::TaskFinished { .. }
                            | RunnerEvent::TaskFailed { .. }
                    )
                {
                    self.running.remove(event.task_id());
                }
            }
            if !self.running.is_empty() {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        let stragglers = self.running_ids();
        self.running.clear();
        self.shutdown_attempted = true;
        self.spawner.shutdown();
        stragglers
    }

    /// 已经停机过（重复调用不再等）。
    pub fn is_shutdown(&self) -> bool {
        self.shutdown_attempted
    }

    /// 整任务成功后删掉 `.ghd`（Python `Task.run` 末尾的 `deletePath(self.partPath)`）。
    fn delete_part_record(&self, tasks: &TaskService, task_id: &str) {
        let Some(record) = tasks.task(task_id) else {
            return;
        };
        if record.status != TaskStatus::Completed {
            return;
        }
        let _ = std::fs::remove_file(format!("{}.ghd", record.output_path()));
    }

    fn next_work_id(&mut self) -> String {
        self.next_work_id += 1;
        format!("wrk_{}", self.next_work_id)
    }
}

// ---------------------------------------------------------------------------
// 单测
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use gd3_core::task::{Progress, TaskFile};
    use serde_json::{Map, Value, json};

    // ---- 夹具 ----

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "gd3-ui-task-runner-{}-{tag}-{seq}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    /// 造一条记录：输出目录固定 `/out`，步骤由调用方补。
    fn record(task_id: &str, status: TaskStatus) -> TaskRecord {
        TaskRecord {
            name: format!("{task_id}.bin"),
            url: format!("https://example.com/{task_id}.bin"),
            pack_id: "http".to_string(),
            task_id: task_id.to_string(),
            status,
            steps: Vec::new(),
            created_at: 0,
            completed_at: 0,
            output_folder: "/out".to_string(),
            file_size: 0,
            files: None,
            category: None,
            should_seed: false,
            checksums: Map::new(),
            extra: Map::new(),
            type_name: None,
        }
    }

    /// 一个 HTTP 步骤（`url` 进 `extra`，与 Python 的平铺 dataclass 字段一致）。
    fn http_step(step_index: i64, url: &str) -> TaskStep {
        let mut extra = Map::new();
        extra.insert("url".to_string(), Value::String(url.to_string()));
        TaskStep {
            step_index,
            file_index: None,
            step_id: format!("step-{step_index}"),
            status: TaskStatus::Waiting,
            progress: Progress::Int(0),
            received_bytes: 0,
            speed: 0,
            extra,
            type_name: None,
        }
    }

    fn task_file(index: i64, relative_path: &str, selected: bool) -> TaskFile {
        TaskFile {
            index,
            relative_path: relative_path.to_string(),
            size: 0,
            selected,
            downloaded_bytes: 0,
            completed: false,
            extra: Map::new(),
        }
    }

    /// 一条带单个 HTTP 步骤的记录。
    fn http_record(task_id: &str, status: TaskStatus) -> TaskRecord {
        let mut record = record(task_id, status);
        let url = record.url.clone();
        record.steps.push(http_step(0, &url));
        record
    }

    #[derive(Default, Clone)]
    struct FakeSpawner(Rc<FakeState>);

    #[derive(Default)]
    struct FakeState {
        jobs: RefCell<Vec<DownloadJob>>,
        cancels: RefCell<Vec<CancellationToken>>,
        sink: RefCell<Option<Sender<RunnerEvent>>>,
        shutdowns: Cell<usize>,
        gates: RefCell<Vec<Arc<SpeedLimit>>>,
    }

    impl FakeSpawner {
        fn jobs(&self) -> Vec<DownloadJob> {
            self.0.jobs.borrow().clone()
        }

        fn last_job(&self) -> DownloadJob {
            self.0
                .jobs
                .borrow()
                .last()
                .cloned()
                .expect("没有派发过 job")
        }

        fn cancel_token(&self, index: usize) -> CancellationToken {
            self.0.cancels.borrow()[index].clone()
        }

        fn shutdowns(&self) -> usize {
            self.0.shutdowns.get()
        }

        fn send(&self, event: RunnerEvent) {
            let sink = self.0.sink.borrow().clone().expect("还没有 sink");
            sink.send(event).expect("发事件");
        }
    }

    impl Spawner for FakeSpawner {
        fn spawn(
            &self,
            job: DownloadJob,
            cancel: CancellationToken,
            sink: Sender<RunnerEvent>,
            gate: Arc<SpeedLimit>,
        ) {
            self.0.jobs.borrow_mut().push(job);
            self.0.cancels.borrow_mut().push(cancel);
            *self.0.sink.borrow_mut() = Some(sink);
            self.0.gates.borrow_mut().push(gate);
        }

        fn shutdown(&mut self) {
            self.0.shutdowns.set(self.0.shutdowns.get() + 1);
        }
    }

    fn runner(settings: RunnerSettings, spawner: &FakeSpawner) -> TaskRunner {
        TaskRunner::with_spawner(settings, Box::new(spawner.clone()))
    }

    /// 建一个服务，把记录都按 `autoStart` 加进去（等待队列）。
    fn service_of(records: Vec<TaskRecord>) -> TaskService {
        let mut service = TaskService::new();
        for record in records {
            let _ = service.add(record, true);
        }
        service
    }

    /// 同一个服务，但任务不入队（只给「直接 `dispatch`」的用例）。
    fn unstarted(records: Vec<TaskRecord>) -> TaskService {
        let mut service = TaskService::new();
        for record in records {
            let _ = service.add(record, false);
        }
        service
    }

    /// 走生产路径派发一条任务（`_pump` 弹队列 + `_dispatch`），返回跑着的 runner。
    fn pumped(spawner: &FakeSpawner, service: &mut TaskService, task_id: &str) -> TaskRunner {
        let mut runner = runner(RunnerSettings::default(), spawner);
        assert_eq!(
            runner.pump(service),
            vec![task_id.to_string()],
            "任务应能派发"
        );
        runner
    }

    // ---- 计划层 ----

    #[test]
    fn pending_positions_require_a_running_task() {
        let mut record = record("a", TaskStatus::Waiting);
        record.steps.push(http_step(0, "https://example.com/a"));
        assert!(
            pending_positions(&record).is_empty(),
            "WAITING 不给任何步骤"
        );
        record.set_status(TaskStatus::Running);
        assert_eq!(pending_positions(&record), vec![0]);
    }

    #[test]
    fn pending_positions_follow_step_index_and_skip_completed_steps() {
        let mut record = record("a", TaskStatus::Running);
        record.steps.push(http_step(2, "u2"));
        record.steps.push(http_step(0, "u0"));
        record.steps.push(http_step(1, "u1"));
        record.steps[2].status = TaskStatus::Completed;
        assert_eq!(
            pending_positions(&record),
            vec![1, 0],
            "按 stepIndex 升序，已完成的下标跳过"
        );
    }

    #[test]
    fn pending_positions_skip_steps_of_unselected_files() {
        let mut record = record("a", TaskStatus::Running);
        record.files = Some(vec![
            task_file(0, "one.bin", true),
            task_file(1, "two.bin", false),
        ]);
        let mut first = http_step(0, "u0");
        first.file_index = Some(0);
        let mut second = http_step(1, "u1");
        second.file_index = Some(1);
        record.steps.push(first);
        record.steps.push(second);
        assert_eq!(pending_positions(&record), vec![0]);
    }

    #[test]
    fn a_single_file_task_downloads_to_the_task_path() {
        let record = record("a", TaskStatus::Running);
        let step = http_step(0, "u");
        assert_eq!(
            step_output_path(&record, &step),
            PathBuf::from("/out/a.bin")
        );
    }

    #[test]
    fn a_multi_file_task_downloads_into_the_output_folder() {
        let mut record = record("a", TaskStatus::Running);
        record.files = Some(vec![
            task_file(0, "one.bin", true),
            task_file(1, "sub/two.bin", true),
        ]);
        let mut step = http_step(0, "u");
        step.file_index = Some(1);
        assert_eq!(
            step_output_path(&record, &step),
            PathBuf::from("/out/a.bin/sub/two.bin"),
            "Python 的 toFilePath = Path(self.outputPath, file.relativePath)"
        );
    }

    #[test]
    fn an_unknown_file_index_falls_back_to_the_task_path() {
        let mut record = record("a", TaskStatus::Running);
        record.files = Some(vec![
            task_file(0, "one.bin", true),
            task_file(1, "two.bin", true),
        ]);
        let mut step = http_step(0, "u");
        step.file_index = Some(9);
        assert_eq!(
            step_output_path(&record, &step),
            PathBuf::from("/out/a.bin")
        );
    }

    #[test]
    fn plan_job_reads_every_transport_field_from_the_step() {
        let mut record = record("a", TaskStatus::Running);
        record.file_size = 4096;
        let mut step = http_step(0, "https://example.com/file.bin");
        step.extra.insert("fileSize".to_string(), json!(1234));
        step.extra.insert(
            "headers".to_string(),
            json!({"Referer": "https://example.com/", "X-Count": 3}),
        );
        step.extra
            .insert("userAgent".to_string(), json!("Ghost/4.4.0"));
        step.extra
            .insert("clientProfile".to_string(), json!("chrome"));
        step.extra.insert("subworkerCount".to_string(), json!(16));
        step.extra
            .insert("canUseRangeRequests".to_string(), json!(true));
        step.extra.insert("httpByteOffset".to_string(), json!(128));
        step.extra.insert(
            "lastModified".to_string(),
            json!("Sun, 06 Nov 1994 08:49:37 GMT"),
        );
        record.steps.push(step);

        let settings = RunnerSettings {
            subworker_count: 2,
            client_profile: "auto".to_string(),
            preserve_last_modified: true,
            ..Default::default()
        };
        let job = plan_job(&record, &settings, "wrk_1");

        assert_eq!(job.work_id, "wrk_1");
        assert_eq!(job.task_id, "a");
        assert_eq!(job.steps.len(), 1);
        let planned = &job.steps[0];
        assert_eq!(planned.position, 0);
        assert_eq!(planned.url, "https://example.com/file.bin");
        assert_eq!(planned.output_path, PathBuf::from("/out/a.bin"));
        assert_eq!(planned.file_size, 1234);
        assert_eq!(planned.user_agent, "Ghost/4.4.0");
        assert_eq!(planned.client_profile, "chrome");
        assert_eq!(planned.subworker_count, 16);
        assert!(planned.can_use_range_requests);
        assert_eq!(planned.http_byte_offset, 128);
        assert_eq!(
            planned.last_modified.as_deref(),
            Some("Sun, 06 Nov 1994 08:49:37 GMT")
        );
        assert!(job.preserve_last_modified);
        assert_eq!(
            planned.headers,
            vec![
                ("Referer".to_string(), "https://example.com/".to_string()),
                ("X-Count".to_string(), "3".to_string()),
            ]
        );
    }

    #[test]
    fn an_empty_last_modified_counts_as_absent() {
        // `HttpTaskStep.lastModified` 缺省是 `""`（`task.py:72`），不能当成有值。
        let mut record = record("a", TaskStatus::Running);
        let mut step = http_step(0, "u");
        step.extra.insert("lastModified".to_string(), json!(""));
        record.steps.push(step);
        let job = plan_job(&record, &RunnerSettings::default(), "wrk_1");
        assert_eq!(job.steps[0].last_modified, None);
        assert!(!job.preserve_last_modified, "缺省关（`cfg.py:254`）");
    }

    #[test]
    fn restore_last_modified_sets_the_mtime_only_when_enabled() {
        let dir = temp_dir("mtime");
        let output = dir.join("out.bin");
        std::fs::write(&output, b"payload").expect("写文件");
        let snapshot = std::fs::metadata(&output)
            .expect("stat")
            .modified()
            .expect("mtime");

        let mut record = record("a", TaskStatus::Running);
        let mut step = http_step(0, "u");
        step.extra.insert(
            "lastModified".to_string(),
            json!("Sun, 06 Nov 1994 08:49:37 GMT"),
        );
        record.steps.push(step);
        let mut job = plan_job(&record, &RunnerSettings::default(), "wrk_1");
        let planned = job.steps[0].clone();
        assert_eq!(
            planned.output_path,
            PathBuf::from("/out/a.bin"),
            "计划层用记录里的路径"
        );

        // 开关关着：一个字节都不动
        let mut on_disk = planned.clone();
        on_disk.output_path = output.clone();
        restore_last_modified(&job, &on_disk);
        assert_eq!(
            std::fs::metadata(&output)
                .expect("stat")
                .modified()
                .expect("mtime"),
            snapshot,
            "开关关着不能碰 mtime"
        );

        // 开关开着：mtime 变成 1994-11-06T08:49:37Z
        job.preserve_last_modified = true;
        restore_last_modified(&job, &on_disk);
        let modified = std::fs::metadata(&output)
            .expect("stat")
            .modified()
            .expect("mtime");
        assert_eq!(
            modified
                .duration_since(std::time::UNIX_EPOCH)
                .expect("1970 之后")
                .as_secs(),
            784_111_777
        );
        assert_eq!(
            std::fs::read(&output).expect("读回"),
            b"payload",
            "内容不能动"
        );

        // 解析不出来 / 文件不在：只打告警，不 panic（`logger.warning` 口径）
        let mut bad = on_disk.clone();
        bad.last_modified = Some("not a date".to_string());
        restore_last_modified(&job, &bad);
        let mut missing = on_disk.clone();
        missing.output_path = dir.join("nope.bin");
        restore_last_modified(&job, &missing);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plan_job_falls_back_to_settings_when_the_step_is_sparse() {
        let mut record = record("a", TaskStatus::Running);
        record.file_size = 2048;
        record.steps.push(http_step(0, "u"));
        let settings = RunnerSettings {
            max_task_num: 9,
            subworker_count: 4,
            client_profile: "firefox".to_string(),
            retry_delay: Duration::from_millis(123),
            max_reassign_size: 777,
            auto_speed_up: false,
            preserve_last_modified: true,
            speed_limit_enabled: true,
            speed_limit_bytes: 2048,
        };
        let job = plan_job(&record, &settings, "wrk_2");
        let planned = &job.steps[0];
        assert_eq!(
            planned.file_size, 2048,
            "没有 fileSize 时用 record.fileSize"
        );
        assert_eq!(planned.subworker_count, 4);
        assert_eq!(planned.client_profile, "firefox");
        assert_eq!(planned.user_agent, "");
        assert!(planned.headers.is_empty());
        assert!(!planned.can_use_range_requests);
        assert_eq!(planned.last_modified, None);
        assert!(job.preserve_last_modified, "开关走 settings");
        assert_eq!(planned.http_byte_offset, 0);
        assert_eq!(job.retry_delay, Duration::from_millis(123));
        assert_eq!(job.max_reassign_size, 777);
        assert!(!job.auto_speed_up);
    }

    #[test]
    fn an_empty_step_profile_falls_back_to_the_settings_one() {
        let mut record = record("a", TaskStatus::Running);
        let mut step = http_step(0, "u");
        step.extra.insert("clientProfile".to_string(), json!(""));
        record.steps.push(step);
        let settings = RunnerSettings {
            client_profile: "safari".to_string(),
            ..Default::default()
        };
        let job = plan_job(&record, &settings, "w");
        assert_eq!(job.steps[0].client_profile, "safari");
    }

    #[test]
    fn plan_job_skips_steps_without_a_url() {
        let mut record = record("a", TaskStatus::Running);
        record.steps.push(http_step(0, "u0"));
        let mut other_pack_step = http_step(1, "u1");
        other_pack_step.extra.remove("url");
        record.steps.push(other_pack_step);
        let job = plan_job(&record, &RunnerSettings::default(), "w");
        assert_eq!(job.positions(), vec![0]);
    }

    #[test]
    fn a_task_without_http_steps_plans_nothing_and_reports_no_error() {
        let record = record("bt", TaskStatus::Running);
        let job = plan_job(&record, &RunnerSettings::default(), "w");
        assert!(job.steps.is_empty());
        assert!(job.positions().is_empty());
        assert_eq!(job.task_id, "bt");
    }

    #[test]
    fn runner_settings_come_from_the_config() {
        let mut config = settings::default_config();
        settings::write(&mut config, "maxTaskNum", &json!(5)).expect("已知 attr");
        settings::write(&mut config, "clientProfile", &json!("chrome")).expect("已知 attr");
        settings::write(&mut config, "maxReassignSize", &json!(101)).expect("已知 attr");
        settings::write(&mut config, "autoSpeedUp", &json!(false)).expect("已知 attr");
        settings::write(&mut config, "shouldPreserveLastModified", &json!(true))
            .expect("已知 attr");

        let parsed = RunnerSettings::from_config(&config);

        assert_eq!(parsed.max_task_num, 5);
        assert_eq!(parsed.client_profile, "chrome");
        assert_eq!(parsed.max_reassign_size, 101 * 1024, "cfg 里是 KiB");
        assert!(!parsed.auto_speed_up);
        assert!(parsed.preserve_last_modified);
        assert_eq!(parsed.subworker_count, DEFAULT_SUBWORKER_COUNT);
        assert_eq!(parsed.retry_delay, RETRY_DELAY);
    }

    #[test]
    fn the_settings_page_key_is_the_same_setting_the_runner_reads() {
        // 设置页写的是 `GeneralDownload/PreserveLastModified`（`settings_spec.rs:365`），
        // runner 读的是属性名 `shouldPreserveLastModified` —— 两条必须指向同一个键。
        let mut config = settings::default_config();
        config.set_bool("GeneralDownload", "PreserveLastModified", true);
        assert!(
            RunnerSettings::from_config(&config).preserve_last_modified,
            "设置页勾上之后 runner 必须看得到"
        );
    }

    #[test]
    fn the_speed_limit_gate_is_shared_with_the_spawner_and_follows_the_config() {
        let mut service = service_of(vec![http_record("a", TaskStatus::Waiting)]);
        let spawner = FakeSpawner::default();
        let mut runner = runner(
            RunnerSettings {
                speed_limit_enabled: true,
                speed_limit_bytes: 1024,
                ..Default::default()
            },
            &spawner,
        );
        assert_eq!(runner.pump(&mut service), vec!["a".to_string()]);

        let gate = spawner.0.gates.borrow()[0].clone();
        assert!(
            Arc::ptr_eq(&gate, runner.speed_limit()),
            "工作线程拿到的必须是 runner 那一份（Python 也只有一个 speedMeter）"
        );
        assert!(gate.is_enabled());
        assert_eq!(gate.limit_bytes(), 1024);

        // 设置页改完 → 下一轮 `set_settings` 推给闸门（`waitForSpeedLimit` 的每轮重读）
        runner.set_settings(RunnerSettings {
            speed_limit_enabled: false,
            speed_limit_bytes: 10,
            ..Default::default()
        });
        assert!(!gate.is_enabled());
        assert_eq!(gate.limit_bytes(), 10);
        assert!(!runner.settings().speed_limit_enabled);
    }

    #[test]
    fn shutdown_releases_a_gate_that_is_holding_a_worker() {
        // 停机之后没人再 `reset` 计数窗口，闸门必须自己放行，否则 shutdown 会卡满。
        let mut service = service_of(vec![http_record("a", TaskStatus::Waiting)]);
        let spawner = FakeSpawner::default();
        let mut runner = runner(RunnerSettings::default(), &spawner);
        assert_eq!(runner.pump(&mut service), vec!["a".to_string()]);
        let gate = spawner.0.gates.borrow()[0].clone();
        gate.configure(true, 1);
        gate.add(1024);
        assert!(gate.exceeded());

        let _ = runner.shutdown(Duration::from_millis(50));
        assert!(gate.is_closed());
        assert!(!gate.exceeded(), "停机后不能再拦人");
    }

    // ---- 派发 ----

    #[test]
    fn pump_dispatches_every_waiting_task_and_spawns_one_job_each() {
        let mut service = service_of(vec![
            http_record("a", TaskStatus::Waiting),
            http_record("b", TaskStatus::Waiting),
        ]);
        let spawner = FakeSpawner::default();
        let settings = RunnerSettings {
            max_task_num: 2,
            ..Default::default()
        };
        let mut runner = runner(settings, &spawner);

        let started = runner.pump(&mut service);

        assert_eq!(started, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(service.task("a").expect("a").status, TaskStatus::Running);
        assert_eq!(service.task("b").expect("b").status, TaskStatus::Running);
        assert_eq!(runner.running_ids(), vec!["a".to_string(), "b".to_string()]);
        assert_eq!(runner.running_positions("a"), Some(&[0usize][..]));
        let jobs = spawner.jobs();
        assert_eq!(jobs.len(), 2, "一个任务一个 job");
        assert_eq!(jobs[0].task_id, "a");
        assert_eq!(jobs[0].work_id, "wrk_1");
        assert_eq!(jobs[0].steps.len(), 1);
        assert_eq!(jobs[1].task_id, "b");
        assert_eq!(jobs[1].work_id, "wrk_2");
    }

    #[test]
    fn pump_stops_at_max_task_num() {
        let mut service = service_of(vec![
            http_record("a", TaskStatus::Waiting),
            http_record("b", TaskStatus::Waiting),
        ]);
        let spawner = FakeSpawner::default();
        let settings = RunnerSettings {
            max_task_num: 1,
            ..Default::default()
        };
        let mut runner = runner(settings, &spawner);

        assert_eq!(runner.pump(&mut service), vec!["a".to_string()]);

        assert_eq!(service.task("b").expect("b").status, TaskStatus::Waiting);
        assert_eq!(spawner.jobs().len(), 1);
    }

    #[test]
    fn dispatch_is_idempotent_for_a_running_task() {
        let mut service = unstarted(vec![http_record("a", TaskStatus::Waiting)]);
        let spawner = FakeSpawner::default();
        let mut runner = runner(RunnerSettings::default(), &spawner);

        assert!(runner.dispatch(&mut service, "a"));
        assert!(!runner.dispatch(&mut service, "a"), "同一任务只有一个 job");
        assert_eq!(spawner.jobs().len(), 1);
    }

    #[test]
    fn dispatch_rejects_an_unknown_task() {
        let mut service = TaskService::new();
        let spawner = FakeSpawner::default();
        let mut runner = runner(RunnerSettings::default(), &spawner);

        assert!(!runner.dispatch(&mut service, "ghost"));

        assert!(spawner.jobs().is_empty());
        assert!(runner.running_ids().is_empty());
    }

    // ---- 事件回灌 ----

    #[test]
    fn a_progress_event_updates_the_step_and_feeds_the_speed_meter() {
        let mut service = service_of(vec![http_record("a", TaskStatus::Waiting)]);
        let spawner = FakeSpawner::default();
        let mut runner = pumped(&spawner, &mut service, "a");
        let work_id = spawner.last_job().work_id;

        spawner.send(RunnerEvent::Progress {
            work_id,
            task_id: "a".to_string(),
            position: 0,
            received_bytes: 512,
            progress: 12.5,
            speed: 99,
            chunk_delta: 512,
        });
        let report = runner.drain(&mut service);

        assert_eq!(report.progressed, vec!["a".to_string()]);
        assert!(
            !report.changed,
            "进度不落盘（Python 的 _supervise 不 _flushSoon）"
        );
        let step = &service.task("a").expect("a").steps[0];
        assert_eq!(step.received_bytes, 512);
        assert_eq!(step.progress, Progress::Float(12.5));
        assert_eq!(step.speed, 99);
        assert_eq!(service.speed_meter().pending_bytes(), 512);
    }

    #[test]
    fn a_zero_chunk_delta_does_not_touch_the_speed_meter() {
        let mut service = service_of(vec![http_record("a", TaskStatus::Waiting)]);
        let spawner = FakeSpawner::default();
        let mut runner = pumped(&spawner, &mut service, "a");
        let work_id = spawner.last_job().work_id;

        spawner.send(RunnerEvent::Progress {
            work_id,
            task_id: "a".to_string(),
            position: 0,
            received_bytes: 1024,
            progress: 25.0,
            speed: 0,
            chunk_delta: 0,
        });
        runner.drain(&mut service);

        assert_eq!(service.speed_meter().pending_bytes(), 0, "每秒采样时为 0");
        assert_eq!(service.task("a").expect("a").steps[0].received_bytes, 1024);
    }

    #[test]
    fn progress_from_a_stale_work_id_is_dropped() {
        let mut service = service_of(vec![http_record("a", TaskStatus::Waiting)]);
        let spawner = FakeSpawner::default();
        let mut runner = pumped(&spawner, &mut service, "a");
        let work_id = spawner.last_job().work_id;

        spawner.send(RunnerEvent::Progress {
            work_id: format!("{work_id}-old"),
            task_id: "a".to_string(),
            position: 0,
            received_bytes: 999,
            progress: 99.0,
            speed: 5,
            chunk_delta: 999,
        });
        let report = runner.drain(&mut service);

        assert!(report.progressed.is_empty());
        let step = &service.task("a").expect("a").steps[0];
        assert_eq!(step.received_bytes, 0);
        assert_eq!(service.speed_meter().pending_bytes(), 0);
    }

    #[test]
    fn progress_for_an_out_of_range_position_is_ignored() {
        let mut service = service_of(vec![http_record("a", TaskStatus::Waiting)]);
        let spawner = FakeSpawner::default();
        let mut runner = pumped(&spawner, &mut service, "a");
        let work_id = spawner.last_job().work_id;

        spawner.send(RunnerEvent::Progress {
            work_id,
            task_id: "a".to_string(),
            position: 7,
            received_bytes: 10,
            progress: 1.0,
            speed: 1,
            chunk_delta: 10,
        });
        let report = runner.drain(&mut service);

        assert_eq!(service.task("a").expect("a").steps[0].received_bytes, 0);
        assert_eq!(service.speed_meter().pending_bytes(), 10, "速度照收");
        assert_eq!(report.progressed, vec!["a".to_string()]);
    }

    #[test]
    fn a_finished_step_is_completed_and_writes_the_range_flag_back() {
        let mut service = service_of(vec![http_record("a", TaskStatus::Waiting)]);
        let spawner = FakeSpawner::default();
        let mut runner = pumped(&spawner, &mut service, "a");
        let work_id = spawner.last_job().work_id;

        spawner.send(RunnerEvent::StepFinished {
            work_id,
            task_id: "a".to_string(),
            position: 0,
            can_use_range_requests: false,
            received_bytes: 123,
        });
        let report = runner.drain(&mut service);

        assert!(report.changed);
        assert!(report.finished.is_empty(), "任务收尾由 TaskFinished 处理");
        let record = service.task("a").expect("a");
        assert_eq!(record.status, TaskStatus::Completed);
        assert_eq!(record.steps[0].status, TaskStatus::Completed);
        assert_eq!(record.steps[0].progress, Progress::Int(100));
        assert_eq!(record.steps[0].received_bytes, 123);
        assert_eq!(record.steps[0].speed, 0);
        assert_eq!(
            record.steps[0].extra.get("canUseRangeRequests"),
            Some(&Value::Bool(false)),
            "降级后的取值要回写"
        );
        assert_eq!(record.steps[0].extra.get("subworkers"), Some(&json!([])));
        assert!(record.completed_at > 0, "全部完成时补 completedAt");
        assert_eq!(record.file_size, 123, "fileSize 为 0 时用收到的字节数");
    }

    #[test]
    fn a_finished_task_deletes_the_part_record_and_ends_the_run() {
        let dir = temp_dir("finished");
        let output = dir.join("a.bin");
        std::fs::write(&output, b"payload").expect("建输出文件");
        let part = PathBuf::from(format!("{}.ghd", output.display()));
        std::fs::write(&part, b"subworker state").expect("建 .ghd");

        let mut record = http_record("a", TaskStatus::Waiting);
        record.output_folder = dir.to_string_lossy().to_string();
        let mut service = service_of(vec![record]);
        let spawner = FakeSpawner::default();
        let mut runner = pumped(&spawner, &mut service, "a");
        let work_id = spawner.last_job().work_id;

        spawner.send(RunnerEvent::StepFinished {
            work_id: work_id.clone(),
            task_id: "a".to_string(),
            position: 0,
            can_use_range_requests: true,
            received_bytes: 7,
        });
        spawner.send(RunnerEvent::TaskFinished {
            work_id,
            task_id: "a".to_string(),
        });
        let report = runner.drain(&mut service);

        assert_eq!(report.finished, vec!["a".to_string()]);
        assert!(report.all_completed);
        assert!(runner.running_ids().is_empty());
        assert!(!part.exists(), "整任务完成后删 .ghd");
        assert!(output.exists(), "只删 .ghd，不动输出文件");
        assert!(service.pending_flush(), "收尾要落盘");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_step_marks_the_task_failed_and_keeps_the_part_record() {
        let dir = temp_dir("failed");
        let output = dir.join("a.bin");
        let part = PathBuf::from(format!("{}.ghd", output.display()));
        std::fs::write(&part, b"subworker state").expect("建 .ghd");

        let mut record = http_record("a", TaskStatus::Waiting);
        record.output_folder = dir.to_string_lossy().to_string();
        let mut service = service_of(vec![record]);
        let spawner = FakeSpawner::default();
        let mut runner = pumped(&spawner, &mut service, "a");
        let work_id = spawner.last_job().work_id;

        spawner.send(RunnerEvent::TaskFailed {
            work_id,
            task_id: "a".to_string(),
            position: Some(0),
            error: TaskError::server_error(500),
        });
        let report = runner.drain(&mut service);

        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].0, "a");
        assert!(report.failed[0].1.format().contains("500"));
        let record = service.task("a").expect("a");
        assert_eq!(record.status, TaskStatus::Failed);
        assert_eq!(record.steps[0].status, TaskStatus::Failed);
        assert_eq!(record.steps[0].speed, 0);
        assert!(part.exists(), "失败时保留 .ghd（分片记录还有用）");
        assert!(!runner.is_running("a"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failure_without_a_position_still_fails_the_task() {
        let mut service = service_of(vec![http_record("a", TaskStatus::Waiting)]);
        let spawner = FakeSpawner::default();
        let mut runner = pumped(&spawner, &mut service, "a");
        let work_id = spawner.last_job().work_id;

        spawner.send(RunnerEvent::TaskFailed {
            work_id,
            task_id: "a".to_string(),
            position: None,
            error: TaskError::cancelled(),
        });
        let report = runner.drain(&mut service);

        assert_eq!(report.failed.len(), 1);
        assert_eq!(service.task("a").expect("a").status, TaskStatus::Failed);
    }

    #[test]
    fn finishing_one_task_dispatches_the_next_waiting_one() {
        let mut service = service_of(vec![
            http_record("a", TaskStatus::Waiting),
            http_record("b", TaskStatus::Waiting),
        ]);
        let spawner = FakeSpawner::default();
        let settings = RunnerSettings {
            max_task_num: 1,
            ..Default::default()
        };
        let mut runner = runner(settings, &spawner);
        assert_eq!(runner.pump(&mut service), vec!["a".to_string()]);

        let work_id = spawner.last_job().work_id;
        spawner.send(RunnerEvent::StepFinished {
            work_id: work_id.clone(),
            task_id: "a".to_string(),
            position: 0,
            can_use_range_requests: true,
            received_bytes: 1,
        });
        spawner.send(RunnerEvent::TaskFinished {
            work_id,
            task_id: "a".to_string(),
        });
        let report = runner.drain(&mut service);

        assert_eq!(report.started, vec!["b".to_string()], "让出的空位立刻补上");
        assert!(runner.is_running("b"));
        assert_eq!(service.task("b").expect("b").status, TaskStatus::Running);
        assert_eq!(spawner.jobs().len(), 2);
        assert!(!report.all_completed, "刚接上下一个任务，不算全完成");
    }

    #[test]
    fn a_cancelled_event_only_drops_the_running_entry() {
        let mut service = service_of(vec![http_record("a", TaskStatus::Waiting)]);
        let spawner = FakeSpawner::default();
        let mut runner = pumped(&spawner, &mut service, "a");
        let work_id = spawner.last_job().work_id;

        spawner.send(RunnerEvent::Cancelled {
            work_id,
            task_id: "a".to_string(),
        });
        let report = runner.drain(&mut service);

        assert!(report.changed);
        assert!(runner.running_ids().is_empty());
        assert!(report.finished.is_empty() && report.failed.is_empty());
        assert_eq!(
            service.task("a").expect("a").status,
            TaskStatus::Running,
            "队列记账已在 pause/停机里做过，不能重复 done"
        );
    }

    // ---- 暂停与停机 ----

    #[test]
    fn pause_cancels_the_token_and_pauses_the_task() {
        let mut service = service_of(vec![http_record("a", TaskStatus::Waiting)]);
        let spawner = FakeSpawner::default();
        let mut runner = pumped(&spawner, &mut service, "a");
        let max_task_num = runner.settings().max_task_num;

        let report = runner
            .pause(&mut service, "a", max_task_num)
            .expect("任务在服务里");

        assert!(report.cancel.work_id.is_some());
        assert!(spawner.cancel_token(0).is_cancelled(), "工作线程要收到取消");
        assert!(!runner.is_running("a"));
        assert_eq!(service.task("a").expect("a").status, TaskStatus::Paused);
        assert!(service.pending_flush(), "pause 要落盘");
    }

    #[test]
    fn pause_of_an_unknown_task_reports_nothing() {
        let mut service = TaskService::new();
        let spawner = FakeSpawner::default();
        let mut runner = runner(RunnerSettings::default(), &spawner);

        assert!(runner.pause(&mut service, "ghost", 3).is_none());
        assert_eq!(spawner.shutdowns(), 0);
    }

    #[test]
    fn cancel_all_cancels_every_running_job_without_touching_the_queue() {
        let mut service = service_of(vec![
            http_record("a", TaskStatus::Waiting),
            http_record("b", TaskStatus::Waiting),
        ]);
        let spawner = FakeSpawner::default();
        let settings = RunnerSettings {
            max_task_num: 2,
            ..Default::default()
        };
        let mut runner = runner(settings, &spawner);
        runner.pump(&mut service);

        runner.cancel_all();

        assert!(spawner.cancel_token(0).is_cancelled());
        assert!(spawner.cancel_token(1).is_cancelled());
        assert_eq!(runner.running_ids().len(), 2, "记账留到事件回灌");
        assert_eq!(service.task("a").expect("a").status, TaskStatus::Running);
    }

    #[test]
    fn shutdown_settles_the_running_jobs_and_closes_the_spawner() {
        let mut service = service_of(vec![http_record("a", TaskStatus::Waiting)]);
        let spawner = FakeSpawner::default();
        let mut runner = pumped(&spawner, &mut service, "a");
        let work_id = spawner.last_job().work_id;
        spawner.send(RunnerEvent::Cancelled {
            work_id,
            task_id: "a".to_string(),
        });

        let stragglers = runner.shutdown(Duration::from_millis(500));

        assert!(stragglers.is_empty());
        assert!(spawner.cancel_token(0).is_cancelled());
        assert!(runner.is_shutdown());
        assert!(runner.running_ids().is_empty());
        assert_eq!(spawner.shutdowns(), 1);
    }

    #[test]
    fn shutdown_reports_the_tasks_that_never_settle() {
        let mut service = service_of(vec![http_record("a", TaskStatus::Waiting)]);
        let spawner = FakeSpawner::default();
        let mut runner = pumped(&spawner, &mut service, "a");

        let stragglers = runner.shutdown(Duration::from_millis(20));

        assert_eq!(stragglers, vec!["a".to_string()]);
        assert!(runner.is_shutdown());
        assert_eq!(spawner.shutdowns(), 1);
    }
}
