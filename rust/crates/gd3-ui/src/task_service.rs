//! 任务服务的「引擎无关半边」：镜像 `app/services/task_service.py` 的 `TaskService`。
//!
//! Python 的 `TaskService`（705 行）把三件事揉在一个类里：
//!
//! 1. **存储/队列记账**：`TaskStore`（`tasks.jsonl` + 占位文件）与 `TaskQueue`
//!    （等待队列 + 运行中映射）——纯逻辑，已经在 `gd3_core::store` 与
//!    `gd3_core::queue` 里镜像过；
//! 2. **引擎交互**：`coroutineRunner.submit/cancel`、`speedMeter`、`fileWatcher`、
//!    做种（libtorrent）与校验（checksum）——依赖尚未迁移的 `app/engine.py`；
//! 3. **信号**：`taskAdded`/`taskRemoved`/`taskStarted`/`taskCompleted`/
//!    `tasksAllCompleted`/`queueChanged`/… 共 19 个。
//!
//! 本模块只做 **1 + 3 的判定**，不碰 2：
//!
//! - 任务列表用 `Vec<TaskRecord>` 保**插入序**（Python 的 `self._store._tasks`
//!   是 `dict`，迭代序即插入序，`tasks()` 直接 `list(...values())`）；
//! - 队列记账全部转调 `gd3_core::queue::TaskQueue`（`wait`/`cancel`/`run`/`done`/
//!   `moveToFront`/`runningCount`/`waitingOrder` 语义已在那边逐条对齐）；
//! - 信号退化成**返回值**：`pump()` 返回「该派发哪些 id」、`on_run_finished()`
//!   返回 `RunOutcome { started, all_completed }`，由调用方决定发
//!   `taskStarted`/`tasksAllCompleted`（Python 在 `_pump` 里发前者、在
//!   `_onRunDone`/`_onRunFailed` 末尾按 `runningCount() == 0` 发后者）；
//! - `_flushSoon()` 的 200ms 防抖退化成 `pending_flush` 标记 + 调用方 tick
//!   （Python 用 `asyncio.sleep(0.2)` + `coroutineRunner.post(self.flush)`）。
//!
//! 为什么还没做 `actions::TaskService` 的 `impl`：那个 trait 收 `&self`，而这里的
//! 方法全要 `&mut self`；且每个动作的第一步都是「取消协程 / 停做种 / 删文件」，
//! 没有引擎就只能假装。第 97–99 步已经把 `pause` / `start` / `delete` /
//! `redownload` / `startChecksum` 的**记账半边**补齐，接引擎时只需一层
//! `RefCell` 适配器。
//!
//! # 已知差异
//!
//! - `add()` / `add_with_free_space()` 不做**重名冲突解决**：不建输出目录、不建
//!   占位文件。要那套语义用 `add_with_conflict()`（第 93 步接上
//!   `gd3_core::placeholder`）；
//! - **不自己算磁盘剩余空间**：`disk_usage(task.outputFolder)` 是文件系统事实，
//!   由调用方算好后走 `add_with_free_space(..., free_space)`；`free_space == None`
//!   等价于 Python 的 `except OSError: pass`（跳过检查），`add()` 是它的简写；
//! - **分类服务是克隆**（第 95 步）：`add_with_conflict()` 的第一步就是
//!   `self.categories.output_folder_of(&record)` 并写回 `category` /
//!   `output_folder`（与 `task_service.py:361` 同序），但 Python 的
//!   `TaskService` 拿的是 `Engine` 里**同一个** `CategoryService` 实例，Rust 侧
//!   是克隆——改规则/设置后要 [`TaskService::set_categories`] 重新推
//!   （引擎侧 [`crate::engine::Engine::push_categories`]）；
//! - `probe_conflict()` 只 `stat()` **一次**（Python 是 `isExisting` 一次、队列
//!   取 `st_size`/`st_mtime` 再一次，中间文件可能被删）；四列都来自同一次
//!   `metadata`，所以不会出现「探测到存在、stat 时没了」的 `OSError`；
//! - `pump()` 派发时用**空 workId** 占位（Python 在 `_dispatch` 里拿到协程 id 才
//!   `queue.run(taskId, workId)`）：这里先 `run(id, "")` 把 `runningCount` 顶上去，
//!   否则 `pump` 的循环条件与 `tasksAllCompleted` 的判定都不成立；引擎起来后由
//!   `on_run_started(id, work_id)` 回填真实 workId；
//! - **做种只做记账**（第 97 步）：`_seed`/`_onSeeding*`/`startSeeding`/
//!   `stopSeeding` 的状态机（`_seeding` 字典 + `isSeeding` + `shouldSeed`）已对齐，
//!   但 `task.runSeeding()` 本体（libtorrent）与 `_watchFile` / 速度表计数仍归引擎层；
//! - **校验也只做记账**（第 98 步）：`_checksums` + `checksumProgress` + 三个信号的
//!   判定已对齐（`startChecksum` / `cancelChecksum` / `_onChecksumDone` /
//!   `_onChecksumFailed`），`toChecksum` 本体在 `gd3_core::checksum`、协程提交归引擎；
//!   `_rebalance`（依赖 `Task.canPause`）同样只做记账；
//! - **删除/重下也只做记账**（第 99 步）：`delete` / `redownload` / `setCategory` /
//!   `startAll` / `pauseAll` 的记账与顺序已对齐，`_unwatchFile`（文件监听器）、
//!   `Task.deleteFiles`（截断已下载内容）与协程取消仍归引擎层；
//!   `Task.deletePlaceholders` 里「未完成步骤的 `step.outputPath`」那一段没法镜像
//!   ——`TaskStep` 基类的 `outputPath` 恒为 `""`，真实路径来自 `HttpStep` /
//!   `yt_dlp` / `bittorrent` 等步骤子类，Rust 侧的 `TaskStep` 只是数据行；所以只做
//!   占位符文件那半边（与 `placeholder_paths()` 只返回 `[outputPath]` 是同一个缺口）。

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use gd3_core::filename::{DEFAULT_MAX_FILENAME_LENGTH, to_safe_filename};
use gd3_core::filesystem::{delete_path, move_path};
use gd3_core::path::{posix_join, posix_normpath, posix_relative_to};
use gd3_core::placeholder::{
    CreateError, NameConflictChoice, PlaceholderRegistry, create_placeholders, delete_placeholder,
    resolve_conflict, sidecar_path,
};
use gd3_core::queue::TaskQueue;
use gd3_core::store;
use gd3_core::task::{TaskRecord, TaskStatus};
use gd3_core::task_error::TaskError;
use serde_json::Value;

use crate::AppWindow;
use crate::card::CardRuntime;
use crate::category::CategoryService;
use crate::facts;
use crate::name_conflict::ProbeTarget;

/// `st_mtime` 的浮点形态（Python 的 `stat_result.st_mtime`）。
///
/// 取不到时间戳（理论上不会：`metadata` 已经成功了）时退回 `0.0`——Python 那边
/// 是 `AttributeError`，不值得为它引入一条错误路径。
fn modified_seconds(info: &fs::Metadata) -> f64 {
    info.modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0.0, |delta| delta.as_secs_f64())
}

/// `TaskService.add()` 的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddOutcome {
    /// 已在服务里：Python `if task.taskId in self._store.tasks: return True`。
    Duplicate,
    /// 已加入但磁盘不够：Python 发 `diskSpaceInsufficient` 后 `return True`，
    /// **不入队**（记录已经在列表里了，顺序是 `store.add` 先于检查）。
    DiskSpaceInsufficient { free: i64 },
    /// 已加入。`queued` = 是否进了等待队列（`auto_start == true` 即入队，
    /// 与 `fileSize` 无关——`_schedule` 在磁盘检查块之后**无条件**调用）。
    Added { queued: bool },
}

/// `add_with_conflict()` 的结果。
///
/// 对应 Python `TaskService.add` 的两处额外信号：`overwriteFailed`（选了覆盖但
/// 回收站移入失败，已退化成改名）与 `taskRemoved`（被同一个占位符替换掉的旧任务）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddReport {
    /// 最终采用的名字（`Overwrite` 成功时与入参相同，`KeepBoth` 时带 `(n)`）。
    pub name: String,
    /// 被替换掉的旧任务 id，按占位符顺序去重（Python `AddResult.replacedTasks`）。
    pub replaced: Vec<String>,
    /// `overwriteFailed` 信号的条件。
    pub overwrite_failed: bool,
    /// `None` = Python 的 `nameConflicted` 分支（`return False`，等用户重新选）。
    pub outcome: Option<AddOutcome>,
}

/// `update_placeholders()` 的结果。
///
/// Python 的 `updatePlaceholders` 只改对象、不发信号；这里把「实际动了什么」返回出来，
/// 方便调用方决定要不要重绘卡片。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateReport {
    /// 最终名字（撞名时会被继续改名，与调用方传进来的可能不同）。
    pub name: String,
    /// 最终占位符列表（= `record.placeholder_paths()`）。
    pub paths: Vec<PathBuf>,
    /// 本次新建成占位符的路径。
    pub created: Vec<PathBuf>,
    /// 被搬走的 `(旧, 新)` 对，含 `<path>.ghd`。
    pub moved: Vec<(PathBuf, PathBuf)>,
    /// 被 `delete_placeholder` 清掉的旧占位符（调用前确实存在的）。
    pub removed: Vec<PathBuf>,
}

/// `_onRunDone` / `_onRunFailed` 末尾那一段的记账结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOutcome {
    /// `_pump()` 决定派发的任务 id，按 `nextWaiting()` 顺序。
    pub started: Vec<String>,
    /// 派发**之后** `runningCount() == 0` → 调用方发 `tasksAllCompleted`。
    pub all_completed: bool,
}

/// `seedingStarted` / `seedingStopped`（`Signal(object)`）的返回值替身。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedingEvent {
    /// `seedingStarted.emit(task)`
    Started,
    /// `seedingStopped.emit(task)`
    Stopped,
}

/// `_cancelWork(task)` 的产物（协程取消本身归引擎）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CancelWork {
    /// 要取消的协程 id：`queue.workIdOf(id) or self._seeding.pop(id)`。
    pub work_id: Option<String>,
    /// `if task.isSeeding: …; seedingStopped.emit(task)`。
    pub stopped_seeding: bool,
}

/// `ChecksumWork`（`task_service.py:27-30`）：一次校验的协程 id 与进度。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChecksumWork {
    /// `coroutineRunner.submit(toChecksum(...))` 的返回值。
    pub work_id: String,
    /// `onProgress` 回写的百分比（`done * 100 // size`），初值 `0`。
    pub progress: i64,
}

/// `checksumStarted` / `checksumCompleted` / `checksumStopped` 的返回值替身。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChecksumEvent {
    /// `checksumStarted.emit(task)`（`Signal(object)`）。
    Started,
    /// `checksumCompleted.emit(task, algorithm)`（`Signal(object, str)`）。
    Completed { algorithm: String },
    /// `checksumStopped.emit(task, error)`（`Signal(object, object)`）：
    /// 取消时 `error is None`，失败时是
    /// `TaskError("无法读取文件：{detail}", detail=str(error))`。
    Stopped { error: Option<TaskError> },
}

/// `_rebalance(*_args)` 的产物。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RebalanceReport {
    /// 被取消的任务（按 `runningIds()` 顺序）：`(taskId, _cancelWork 的结果)`。
    pub cancelled: Vec<(String, CancelWork)>,
    /// 放回等待队列的 id（`canPause` 为真的那些，与 `cancelled` 同序）。
    pub requeued: Vec<String>,
    /// 尾部 `_pump()` 派发的 id。
    pub started: Vec<String>,
}

/// `pause(task)` 的产物。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PauseReport {
    /// `_cancelWork(task)` 的结果（协程取消归引擎）。
    pub cancel: CancelWork,
    /// 尾部 `_pump()` 派发的 id（`taskPaused` 之后让出的空位）。
    pub started: Vec<String>,
}

/// `delete(task, shouldDeleteFiles)` 的产物。
///
/// 不 `derive(PartialEq)`：`removed` 是 `TaskRecord`，`gd3-core` 侧没给它
/// `PartialEq`（含 `Value`，只可能到 `PartialEq` 不到 `Eq`）。
#[derive(Debug, Clone)]
pub struct DeleteReport {
    /// `_cancelWork(task)` 的结果。
    pub cancel: CancelWork,
    /// 被移除的记录（`taskRemoved` 的载荷；占位符文件已经删过了）。
    pub removed: TaskRecord,
    /// `cancelChecksum(task)` 摘掉的 workId：`Some` 时调用方还要发
    /// `checksumStopped(task, None)`。
    pub checksum_work: Option<String>,
    /// `canDelete = shouldDeleteFiles and _canDeleteIn(outputFolder)`：为真时
    /// 调用方还要执行 `Task.deleteFiles`（截断/清空已下载内容）。
    pub delete_files: bool,
    /// 本次首次触发 `fileDeleteDenied`（Python 只提示一次）。
    pub delete_denied: bool,
    /// 尾部 `_pump()` 派发的 id。
    pub started: Vec<String>,
}

/// `redownload(task)` 的产物。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedownloadReport {
    /// `cancelChecksum(task)` 摘掉的 workId（同 [`DeleteReport::checksum_work`]）。
    pub checksum_work: Option<String>,
    /// `_cancelWork(task)` 的结果。
    pub cancel: CancelWork,
    /// `Task.reset()` 之后的状态（= Python `reset()` 里 `updateStatus()` 的返回）。
    pub status: TaskStatus,
    /// `_schedule` 尾部 `_pump()` 派发的 id。
    pub started: Vec<String>,
}

/// 任务列表 + 队列记账（不含引擎）。
#[derive(Debug, Clone, Default)]
pub struct TaskService {
    /// 插入序 = Python `dict` 的迭代序。
    records: Vec<TaskRecord>,
    queue: TaskQueue,
    /// `TaskStore._loaded`：`loadSaved()` 之前 `flush()` 是 no-op。
    loaded: bool,
    /// `_flushSoon()` 留下的防抖标记（调用方 tick 到点后调 `flush`）。
    pending_flush: bool,
    /// `_hasNotifiedDeleteDenied`：只提示一次。
    delete_denied_notified: bool,
    /// `TaskStore._placeholders` + `_taskByPlaceholder`。
    placeholders: PlaceholderRegistry,
    /// `TaskService._categoryService`：`add` 的第一步就是
    /// `task.category, task.outputFolder = outputFolderOf(task)`。
    ///
    /// `Default` = 空规则 + 分类功能关闭 → 不改 `category` / `outputFolder`
    /// （与 Python 里 `isCategoryEnabled` 缺省 `False` 的行为一致）。
    categories: CategoryService,
    /// `TaskService._seeding`：taskId → 做种协程的 workId。
    ///
    /// 同时充当 `Task.isSeeding`（Python 的非持久化字段，`init=False, repr=False`）。
    seeding: BTreeMap<String, String>,
    /// `TaskService._checksums`：taskId → 校验协程的 workId + 进度。
    ///
    /// `ChecksumWork` 整体入库（不是只存 workId）：`checksumProgress(task)` 要读
    /// `progress`，而 `onProgress` 回调只拿得到 taskId。
    checksums: BTreeMap<String, ChecksumWork>,
}

/// `_canDeleteIn(folder)`（`task_service.py:606-615`）：macOS 上「删不掉已下载内容」
/// 的情形是「目录里一个条目都读不出来（`PermissionError`）」——那是沙箱/权限问题，
/// 其余平台恒真。
///
/// Python 用 `next(folder.iterdir(), None)` 只取一个条目；`read_dir` 打开目录时才
/// 可能 `PermissionDenied`，等价。
#[cfg(target_os = "macos")]
fn can_delete_in(folder: &Path) -> bool {
    match fs::read_dir(folder) {
        Ok(_) => true,
        Err(error) => error.kind() != io::ErrorKind::PermissionDenied,
    }
}

#[cfg(not(target_os = "macos"))]
fn can_delete_in(_folder: &Path) -> bool {
    true
}

impl TaskService {
    /// 空服务、未 load（`flush()` 不会写盘）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 分类服务（`TaskService._categoryService`）。
    pub fn categories(&self) -> &CategoryService {
        &self.categories
    }

    /// 换上分类服务。Python 是构造时传同一个实例；Rust 侧由
    /// [`crate::engine::Engine::push_categories`] 在改完规则/设置后重新推。
    pub fn set_categories(&mut self, categories: CategoryService) {
        self.categories = categories;
    }

    /// 从 `tasks.jsonl` 读出的记录建服务（相当于 `loadSaved()` 之后）。
    pub fn from_records(records: Vec<TaskRecord>) -> Self {
        Self {
            records,
            loaded: true,
            ..Self::default()
        }
    }

    /// `loadSaved()`：读 `tasks.jsonl`，坏行只计数（Python 记日志后跳过）。
    ///
    /// 文件不存在 → 空服务且 `loaded == true`（Python 里 `_loaded = True` 之后
    /// 才 `return`，所以之后 `flush()` 会真的写盘）。
    pub fn load(path: &Path) -> io::Result<(Self, Vec<String>)> {
        let outcome = store::load_records(path)?;
        Ok((Self::from_records(outcome.records), outcome.failures))
    }

    /// `loadSaved()` 的就地版本（引擎启动时用）：读盘后**替换**整个服务。
    ///
    /// Python 的 `TaskStore.loadSaved()` 会把记录并进已有的 `_tasks`，但引擎只在
    /// 启动时调一次，那时服务必然是空的，所以直接替换与合并等价（有单测钉住）。
    pub fn load_into(&mut self, path: &Path) -> io::Result<Vec<String>> {
        let (service, failures) = TaskService::load(path)?;
        *self = service;
        Ok(failures)
    }

    /// `list(self._store.tasks.values())`。
    pub fn records(&self) -> &[TaskRecord] {
        &self.records
    }

    /// `taskById(taskId)`。
    pub fn task(&self, task_id: &str) -> Option<&TaskRecord> {
        self.records.iter().find(|record| record.task_id == task_id)
    }

    /// 可变借用（改完记得 `mark_dirty()`，Python 的 `cfg.set` 等价物是 `_flushSoon`）。
    pub fn task_mut(&mut self, task_id: &str) -> Option<&mut TaskRecord> {
        self.records
            .iter_mut()
            .find(|record| record.task_id == task_id)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// 占位符登记表（`TaskStore._placeholders` + `_taskByPlaceholder`）。
    pub fn placeholders(&self) -> &PlaceholderRegistry {
        &self.placeholders
    }

    /// 标记「有未落盘改动」（`_flushSoon()` 的立即部分）。
    pub fn mark_dirty(&mut self) {
        self.pending_flush = true;
    }

    pub fn pending_flush(&self) -> bool {
        self.pending_flush
    }

    /// `add(task, autoStart)` 的记账部分（跳过磁盘检查，= `disk_usage` 抛 OSError）。
    ///
    /// - 已存在 → `Duplicate`（Python 直接 `return True`，不重复入队）；
    /// - `shouldSeed = True`（Python 无条件置位）；
    /// - `autoStart` → 进等待队列（`_schedule` 只做 `queue.wait`，真正的派发在
    ///   `_pump`，由调用方调 `pump()`）。
    pub fn add(&mut self, record: TaskRecord, auto_start: bool) -> AddOutcome {
        self.add_with_free_space(record, auto_start, None)
    }

    /// 带磁盘检查的 `add`：`free_space` = `disk_usage(output_folder).free`，
    /// `None` 代表 `disk_usage` 抛了 `OSError`（Python 吞掉后照常入队）。
    pub fn add_with_free_space(
        &mut self,
        mut record: TaskRecord,
        auto_start: bool,
        free_space: Option<i64>,
    ) -> AddOutcome {
        if self.task(&record.task_id).is_some() {
            return AddOutcome::Duplicate;
        }
        record.should_seed = true;
        let task_id = record.task_id.clone();
        let file_size = record.file_size;
        self.records.push(record);
        self.pending_flush = true;
        self.schedule_add(&task_id, file_size, auto_start, free_space)
    }

    /// `add` 的尾段：入队 + 磁盘检查（`_flushSoon()` 之后的全部记账）。
    ///
    /// 记录必须**已经在** `self.records` 里 —— Python 的 `TaskStore.add` 也是先把
    /// 任务放进 `_tasks`，再回到 `TaskService.add` 做磁盘检查与 `_schedule`。
    fn schedule_add(
        &mut self,
        task_id: &str,
        file_size: i64,
        auto_start: bool,
        free_space: Option<i64>,
    ) -> AddOutcome {
        if !auto_start {
            return AddOutcome::Added { queued: false };
        }
        if file_size > 0
            && let Some(free) = free_space
            && free < file_size
        {
            return AddOutcome::DiskSpaceInsufficient { free };
        }
        self.queue.wait(task_id);
        AddOutcome::Added { queued: true }
    }

    /// 带**重名冲突解决**的 `add`：镜像 `TaskService.add` 里 `TaskStore.add` 那一段
    /// （`app/services/task_service.py:62-90` + `:358-383`）。
    ///
    /// 与 `add_with_free_space` 的区别：
    ///
    /// - 先 `mkdir -p` 输出目录（Python 的 `task.outputFolder.mkdir(parents=True,
    ///   exist_ok=True)`）；
    /// - 建占位符（输出是目录时建目录，否则建 0 字节文件），撞上已存在就走
    ///   `gd3_core::placeholder::resolve_conflict` 的决策循环；
    /// - 用 `PlaceholderRegistry` 记账，`replaced` 里的旧任务会被移出服务；
    /// - `Ok(AddReport { outcome: None })` = Python 的 `nameConflicted` +
    ///   `return False`（任务**没**加进去，等用户重新选）。
    ///
    /// `delete_recoverably` 由调用方注入，生产环境传
    /// `gd3_core::trash::delete_recoverably`（`Err` 一律按 `false` 处理即可）。
    ///
    /// `Err(CreateError)` = Python 里会冒泡打断 `add` 的 `OSError`：
    /// `CreateError::Io` 是建占位符/建目录的真实失败，`CreateError::Taken`
    /// 是改名循环用尽 `MAX_RENAME_ATTEMPTS` 仍未成功。
    pub fn add_with_conflict<D>(
        &mut self,
        mut record: TaskRecord,
        choice: NameConflictChoice,
        auto_start: bool,
        free_space: Option<i64>,
        delete_recoverably: D,
    ) -> Result<AddReport, CreateError>
    where
        D: FnMut(&Path) -> bool,
    {
        if self.task(&record.task_id).is_some() {
            return Ok(AddReport {
                name: record.name,
                replaced: Vec::new(),
                overwrite_failed: false,
                outcome: Some(AddOutcome::Duplicate),
            });
        }
        // `task_service.py:361`：`task.category, task.outputFolder =
        // categoryService.outputFolderOf(task)`——分类功能开启时，落在默认下载
        // 目录的任务会被改道到分类目录（`record.category` 为 `None` 才按名字猜）。
        let resolved = self.categories.output_folder_of(&record);
        record.output_folder = resolved.folder_string();
        record.category = resolved.category;
        // `Task.__post_init__` 的口径：`toSafeFilename(name, fallback="download")`。
        // 这里补一次，避免调用方塞进来的名字带分隔符把占位符建到输出目录之外。
        record.name = to_safe_filename(&record.name, "download", DEFAULT_MAX_FILENAME_LENGTH);
        record.should_seed = true;
        // Python 的 `mkdir(parents=True, exist_ok=True)` 抛 `OSError`，冒泡打断 `add`。
        fs::create_dir_all(&record.output_folder).map_err(|error| CreateError::Io {
            path: PathBuf::from(&record.output_folder),
            error,
        })?;
        let is_output_folder = record.is_output_folder();
        let probe = record.clone();
        let registry = &self.placeholders;
        let records = &self.records;
        let resolved = resolve_conflict(
            &record.name,
            choice,
            |name| {
                // 每个候选名字都要重算 outputPath：`BilibiliTask` 多文件时取
                // `Path(name).stem`，不是简单换路径最后一段。
                let paths = probe.placeholder_paths_at(&probe.output_folder, name);
                create_placeholders(&paths, &[], is_output_folder).map(|_| ())
            },
            |path| {
                registry.is_unfinished(path, |task_id| {
                    records
                        .iter()
                        .find(|record| record.task_id == task_id)
                        .map(|record| record.status)
                })
            },
            delete_recoverably,
        )?;
        let Some(resolved) = resolved else {
            return Ok(AddReport {
                name: record.name,
                replaced: Vec::new(),
                overwrite_failed: false,
                outcome: None,
            });
        };
        record.name = resolved.name.clone();
        let paths = record.placeholder_paths();
        let replaced = self.placeholders.replaced_tasks(&paths);
        for task_id in &replaced {
            self.remove(task_id);
        }
        let task_id = record.task_id.clone();
        let file_size = record.file_size;
        let output_folder = record.output_folder.clone();
        self.records.push(record);
        self.placeholders
            .set(&task_id, Path::new(&output_folder), &resolved.name, &paths);
        self.pending_flush = true;
        let outcome = self.schedule_add(&task_id, file_size, auto_start, free_space);
        Ok(AddReport {
            name: resolved.name,
            replaced,
            overwrite_failed: resolved.overwrite_failed,
            outcome: Some(outcome),
        })
    }

    /// 带**改名/移动**的占位符更新：镜像 `TaskStore.updatePlaceholders`
    /// （`app/services/task_service.py:91-121`）。
    ///
    /// 调用方先通过 `task_mut(task_id)` 改好 `output_folder` / `name`，再调这里：
    ///
    /// 1. 占位符没变（`placeholder_paths() == 登记值`）→ 直接返回，什么都不做；
    /// 2. `mkdir -p` 输出目录 → 按 `KeepBoth` 走 `resolve_conflict`（Python 这里只
    ///    `except FileExistsError` 改名，既不查「未完成」也不进回收站）；
    /// 3. 名字或目录变了 → 把旧占位符与 `<path>.ghd` 逐个搬到新位置
    ///    （先 `delete_path(新)` 再 `move_path`）；
    /// 4. 旧占位符里没搬走的，`delete_placeholder` 清掉；
    /// 5. 重新登记并置 `pending_flush`（Python 由调用方 `_flushSoon()`）。
    ///
    /// 与 Python 的差异：任务没有登记项时返回 `Ok(None)`（Python 是 `KeyError`）；
    /// 改名重试仍有 `MAX_RENAME_ATTEMPTS` 上限。
    pub fn update_placeholders(
        &mut self,
        task_id: &str,
    ) -> Result<Option<UpdateReport>, CreateError> {
        let Some(old) = self.placeholders.get(task_id).cloned() else {
            return Ok(None);
        };
        let Some(mut record) = self.task(task_id).cloned() else {
            return Ok(None);
        };
        let current = record.placeholder_paths();
        if current == old.paths {
            return Ok(Some(UpdateReport {
                name: record.name.clone(),
                paths: current,
                ..Default::default()
            }));
        }
        fs::create_dir_all(&record.output_folder).map_err(|error| CreateError::Io {
            path: PathBuf::from(&record.output_folder),
            error,
        })?;
        let is_output_folder = record.is_output_folder();
        let existing = old.paths.clone();
        let probe = record.clone();
        let mut created: Vec<PathBuf> = Vec::new();
        let resolved = resolve_conflict(
            &record.name,
            NameConflictChoice::KeepBoth,
            |name| {
                let paths = probe.placeholder_paths_at(&probe.output_folder, name);
                create_placeholders(&paths, &existing, is_output_folder)
                    .map(|done| created.extend(done))
            },
            |_| false,
            |_| false,
        )?;
        let Some(resolved) = resolved else {
            return Ok(None);
        };
        record.name = resolved.name.clone();
        let new_paths = record.placeholder_paths();
        let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
        let renamed = record.name != old.name;
        let moved_folder = Path::new(&record.output_folder) != old.output_folder.as_path();
        if renamed || moved_folder {
            // `placeholderPathsAt(旧目录, 旧名字)`：注意其余字段仍取**当前** record
            // （`BilibiliTask` 的 `isOutputFolder` 可能已经变了），Python 也是这么算的。
            let old_folder = old.output_folder.to_string_lossy().into_owned();
            let old_paths = record.placeholder_paths_at(&old_folder, &old.name);
            for (old_path, new_path) in old_paths.iter().zip(new_paths.iter()) {
                if !old.paths.contains(old_path) {
                    continue;
                }
                for (from, to) in [
                    (old_path.clone(), new_path.clone()),
                    (sidecar_path(old_path), sidecar_path(new_path)),
                ] {
                    if !from.exists() {
                        continue;
                    }
                    delete_path(&to);
                    move_path(&from, &to).map_err(|error| CreateError::Io {
                        path: to.clone(),
                        error,
                    })?;
                    moved.push((from, to));
                }
            }
        }
        let mut removed: Vec<PathBuf> = Vec::new();
        for path in &old.paths {
            if new_paths.contains(path) {
                continue;
            }
            let existed = path.is_dir() || path.is_file();
            delete_placeholder(path).map_err(|error| CreateError::Io {
                path: path.clone(),
                error,
            })?;
            if existed {
                removed.push(path.clone());
            }
        }
        let new_folder = PathBuf::from(&record.output_folder);
        let name = record.name.clone();
        self.placeholders
            .set(task_id, &new_folder, &name, &new_paths);
        if let Some(slot) = self.task_mut(task_id) {
            *slot = record;
        }
        self.pending_flush = true;
        Ok(Some(UpdateReport {
            name,
            paths: new_paths,
            created,
            moved,
            removed,
        }))
    }

    /// `delete(task, ...)` 的记账部分：`_cancelWork` → 从列表移除 → 待落盘。
    ///
    /// 文件删除/占位符清理由调用方做（`Task.deleteFiles` + `deletePlaceholders`），
    /// 「没有权限删」的提示走 `note_delete_denied()`。
    pub fn remove(&mut self, task_id: &str) -> Option<TaskRecord> {
        // `_cancelWork` 的协程取消归引擎；记账部分已由 cancel_work 做完。
        let _ = self.cancel_work(task_id);
        self.placeholders.remove(task_id);
        let index = self
            .records
            .iter()
            .position(|record| record.task_id == task_id)?;
        self.pending_flush = true;
        Some(self.records.remove(index))
    }

    /// `TaskService.probeConflict(task)`（`task_service.py:388-390`）：
    /// `_, folder = categoryService.outputFolderOf(task)` 之后再探测。
    ///
    /// 供 `NameConflictQueue` 的 `probeConflict` 注入点使用（[`crate::name_conflict`]）。
    pub fn probe_conflict(&self, record: &TaskRecord) -> Option<ProbeTarget> {
        let folder = self.categories.output_folder_of(record).folder;
        self.probe_conflict_in(record, &folder)
    }

    /// `TaskStore.probeConflict(task, folder)`（`task_service.py:126-128`）：
    ///
    /// ```python
    /// paths = sorted(folder / path.relative_to(task.outputFolder)
    ///                for path in task.placeholderPaths)
    /// return next((path for path in paths
    ///              if isExisting(path) and not self._isUnfinishedPlaceholder(path)), None)
    /// ```
    ///
    /// 三处口径：
    /// - **相对路径再拼到 `folder`**：占位符登记在 `task.outputFolder` 下，探测时
    ///   要按「新的输出目录」重新定位（`add` 的第一步刚把分类目录算出来）；
    /// - `sorted(...)`：`Path` 之间**按组件**比较（`/a/b` < `/a.b`），不是字符串比较；
    /// - `isExisting(path) and not _isUnfinishedPlaceholder(path)`：先看 `stat()`
    ///   成不成，再看登记表里该路径的**所属任务**是不是 `COMPLETED`——未完成的
    ///   占位符是自己人，不算冲突。
    pub fn probe_conflict_in(&self, record: &TaskRecord, folder: &Path) -> Option<ProbeTarget> {
        let folder_text = folder.to_string_lossy();
        let mut paths: Vec<PathBuf> = record
            .placeholder_paths()
            .iter()
            .filter_map(|path| {
                let text = path.to_string_lossy();
                // `pathlib` 在不可相对时抛 `ValueError`；这里的占位符都是
                // `outputFolder / name` 拼出来的，走不到 `None`。
                let relative = posix_relative_to(&text, &record.output_folder)?;
                // `Path(folder) / "."` 在 pathlib 里就是 `Path(folder)`。
                Some(if relative == "." {
                    PathBuf::from(posix_normpath(&folder_text))
                } else {
                    PathBuf::from(posix_join(&folder_text, &relative))
                })
            })
            .collect();
        // `Path` 的 `Ord` 是逐组件比较，与 CPython `PurePath._cparts` 同口径。
        paths.sort();
        paths.into_iter().find_map(|path| {
            let info = fs::metadata(&path).ok()?;
            if self
                .placeholders
                .is_unfinished(&path, |task_id| self.task(task_id).map(|task| task.status))
            {
                return None;
            }
            Some(ProbeTarget {
                path: path.to_string_lossy().into_owned(),
                is_folder: info.is_dir(),
                existing_size: info.len() as i64,
                modified_at: modified_seconds(&info),
            })
        })
    }

    /// [`TaskService::probe_conflict`]，按 id 取任务（`ConflictActions::probe` 只
    /// 拿得到 `ConflictTask.task_id`）。任务不在列表里 → `None`。
    pub fn probe_conflict_by_id(&self, task_id: &str) -> Option<ProbeTarget> {
        self.probe_conflict(self.task(task_id)?)
    }

    /// `start(task)`：已在运行/等待则不动（Python 两条短路），否则入队等派发。
    pub fn start(&mut self, task_id: &str) -> bool {
        if self.queue.is_running(task_id) || self.queue.is_waiting(task_id) {
            return false;
        }
        if self.task(task_id).is_none() {
            return false;
        }
        self.queue.wait(task_id);
        true
    }

    /// `startSeeding(task)`（`task_service.py:397-400`）：`shouldSeed = True` +
    /// 落盘 + `_seed(task, isManual=True)`。
    ///
    /// 与 Python 的差别只在「谁提交协程」：`task.runSeeding(isManual)` 的 workId
    /// 由引擎给，这里只登记。任务不在列表里 → `None`（Python 会 `AttributeError`）。
    pub fn start_seeding(&mut self, task_id: &str, work_id: &str) -> Option<SeedingEvent> {
        let record = self.task_mut(task_id)?;
        record.should_seed = true;
        // `_flushSoon()`：`shouldSeed` 是持久化字段（`isSeeding` 不是）。
        self.pending_flush = true;
        self.seed(task_id, work_id)
    }

    /// `stopSeeding(task)`（`task_service.py:402-406`）：`shouldSeed = False` +
    /// 落盘 + `_cancelWork(task)` + `_refreshSpeedMeter()`。
    ///
    /// `_refreshSpeedMeter()` 退化成「调用方读 [`TaskService::speed_meter_should_run`]」；
    /// `seedingStopped` 事件在 [`CancelWork::stopped_seeding`] 里。
    pub fn stop_seeding(&mut self, task_id: &str) -> CancelWork {
        if let Some(record) = self.task_mut(task_id) {
            record.should_seed = false;
            self.pending_flush = true;
        }
        self.cancel_work(task_id)
    }

    /// `pause(task)`（`task_service.py:429-436`）的记账部分。
    ///
    /// ```python
    /// self._cancelWork(task)
    /// task.setStatus(TaskStatus.PAUSED)
    /// self._flushSoon()
    /// self.taskPaused.emit(task)
    /// self._pump()
    /// ```
    ///
    /// 尾部 `_pump()` 不能省：让出空位后要立刻派发下一个等待任务（`started`）。
    /// 任务不在列表里 → `None`（Python 会 `AttributeError`）。
    pub fn pause(&mut self, task_id: &str, max_task_num: usize) -> Option<PauseReport> {
        self.task(task_id)?;
        let cancel = self.cancel_work(task_id);
        if let Some(record) = self.task_mut(task_id) {
            record.status = TaskStatus::Paused;
        }
        self.pending_flush = true;
        let started = self.pump(max_task_num);
        Some(PauseReport { cancel, started })
    }

    /// `delete(task, shouldDeleteFiles)`（`task_service.py:438-452`）的记账部分。
    ///
    /// ```python
    /// self.cancelChecksum(task)
    /// self._unwatchFile(task)
    /// canDelete = shouldDeleteFiles and self._canDeleteIn(task.outputFolder)
    /// if shouldDeleteFiles and not canDelete and not self._hasNotifiedDeleteDenied:
    ///     self._hasNotifiedDeleteDenied = True
    ///     self.fileDeleteDenied.emit()
    /// def onStopped():
    ///     if canDelete: task.deleteFiles()
    ///     task.deletePlaceholders()
    /// self._cancelWork(task, finished=onStopped)
    /// self._store.remove(task.taskId)
    /// self._flushSoon()
    /// self.taskRemoved.emit(task.taskId)
    /// self._pump()
    /// ```
    ///
    /// 顺序照抄：**先摘校验、再判能不能删、再取消协程、再移除记录、最后 pump**。
    /// 占位符文件在这里就地删（`deletePlaceholder` + `<path>.ghd`，只删「空目录 /
    /// 零字节文件」，正在下载的内容不动）；**已下载内容**留给调用方
    /// （[`DeleteReport::delete_files`] 为真时执行 `Task.deleteFiles` 的语义），
    /// 因为它要等协程真的停下（Python 放在 `finished` 回调里）。
    /// `_unwatchFile` 同样归引擎。
    ///
    /// 任务不在列表里 → `None`（Python 在 `task.outputFolder` 就 `AttributeError`）。
    pub fn delete(
        &mut self,
        task_id: &str,
        should_delete_files: bool,
        max_task_num: usize,
    ) -> Option<DeleteReport> {
        let folder = PathBuf::from(self.task(task_id)?.output_folder.clone());
        let checksum_work = self.cancel_checksum(task_id);
        let delete_files = should_delete_files && can_delete_in(&folder);
        let delete_denied = should_delete_files && !delete_files && self.note_delete_denied();
        let cancel = self.cancel_work(task_id);
        let removed = self.remove(task_id)?;
        for path in removed.placeholder_paths() {
            let _ = delete_placeholder(&path);
            // `delete_path` 返回 `()`，不能再套 `let _ =`（clippy::let_unit_value）。
            delete_path(&sidecar_path(&path));
        }
        let started = self.pump(max_task_num);
        Some(DeleteReport {
            cancel,
            removed,
            checksum_work,
            delete_files,
            delete_denied,
            started,
        })
    }

    /// `redownload(task)`（`task_service.py:454-463`）的记账部分。
    ///
    /// ```python
    /// self.cancelChecksum(task)
    /// self._unwatchFile(task)
    /// def onStopped():
    ///     task.deleteFiles()
    ///     task.reset()
    ///     self._flushSoon()
    ///     self._schedule(task)
    ///     self._cancelWork(task, finished=onStopped)
    /// ```
    ///
    /// Python 把 `reset()` + `_schedule()` 放在 `finished` 回调里；这里直接做完，
    /// 因为 `_pump()` 只**返回**该派发的 id，真正的派发归调用方——调用方拿到
    /// `started` 之后、提交协程之前删掉旧文件即可（顺序与 Python 一致）。
    /// `_unwatchFile` 归引擎。
    ///
    /// 任务不在列表里 → `None`。
    pub fn redownload(&mut self, task_id: &str, max_task_num: usize) -> Option<RedownloadReport> {
        self.task(task_id)?;
        let checksum_work = self.cancel_checksum(task_id);
        let cancel = self.cancel_work(task_id);
        let status = self.task_mut(task_id)?.reset();
        // `_flushSoon()`：`reset()` 动了持久化字段（`completedAt` / `shouldSeed` /
        // `checksums` / 每个 step 的进度）。
        self.pending_flush = true;
        // `_schedule(task)`：`queue.wait(taskId)` + `_pump()`。
        self.queue.wait(task_id);
        let started = self.pump(max_task_num);
        Some(RedownloadReport {
            checksum_work,
            cancel,
            status,
            started,
        })
    }

    /// `setCategory(task, categoryId)`（`task_service.py:478-480`）：只改
    /// `task.category` + 落盘，**不重算 `outputFolder`**、也不动占位符。
    ///
    /// Python 的 `categoryId` 恒为 `str`（卡片菜单的「未分类」传 `""`），所以
    /// 空串落成 `Some("")` 而不是 `None`——`outputFolderOf` 用 `categoryId is None`
    /// 判断「要不要按名字猜分类」，两种空值语义不同。
    pub fn set_category(&mut self, task_id: &str, category_id: &str) -> bool {
        match self.task_mut(task_id) {
            Some(record) => record.category = Some(category_id.to_string()),
            None => return false,
        }
        self.pending_flush = true;
        true
    }

    /// `startAll()`（`task_service.py:530-534`）：对 `PAUSED` / `WAITING` / `FAILED`
    /// 的任务逐个 `_schedule`（= `queue.wait` + `_pump()`）。
    ///
    /// 返回所有被派发的 id（Python 是逐任务 pump，这里同序聚合）。
    /// **不改状态**：`_dispatch` 才 `setStatus(RUNNING)`，排队期间还是原状态。
    pub fn start_all(&mut self, max_task_num: usize) -> Vec<String> {
        let ids: Vec<String> = self
            .records
            .iter()
            .filter(|record| {
                matches!(
                    record.status,
                    TaskStatus::Paused | TaskStatus::Waiting | TaskStatus::Failed
                )
            })
            .map(|record| record.task_id.clone())
            .collect();
        let mut started = Vec::new();
        for task_id in ids {
            self.queue.wait(&task_id);
            started.extend(self.pump(max_task_num));
        }
        started
    }

    /// `pauseAll()`（`task_service.py:536-539`）：对**运行中或等待中**的任务逐个
    /// `pause`（`_cancelWork` + `PAUSED` + `_pump`）。
    ///
    /// 先快照 id 列表再遍历——Python 用 `list(self._store.tasks.values())` 同理，
    /// 否则 `pause` 里的 `_pump` 会改动正在迭代的队列。
    pub fn pause_all(&mut self, max_task_num: usize) -> Vec<String> {
        let ids: Vec<String> = self
            .records
            .iter()
            .filter(|record| {
                self.queue.is_running(&record.task_id) || self.queue.is_waiting(&record.task_id)
            })
            .map(|record| record.task_id.clone())
            .collect();
        let mut started = Vec::new();
        for task_id in ids {
            if let Some(report) = self.pause(&task_id, max_task_num) {
                started.extend(report.started);
            }
        }
        started
    }

    /// `_cancelWork(task)`（`task_service.py:629-639`）的记账部分。
    ///
    /// ```python
    /// workId = self._queue.workIdOf(task.taskId) or self._seeding.pop(task.taskId, None)
    /// self._queue.cancel(task.taskId)
    /// if task.isSeeding:
    ///     task.isSeeding = False
    ///     self.seedingStopped.emit(task)
    /// if workId is not None:
    ///     self._coroutineRunner.cancel(workId, finished=finished)
    /// ```
    ///
    /// 三处口径：
    /// - **`or` 是短路的**：队列里有 workId 时 `_seeding.pop` 根本不会执行，
    ///   `_seeding` 里那条会留下（而 `isSeeding` 已被清成 `False`）。这是 Python
    ///   的既有缺陷，Rust 侧照抄——`speed_meter_should_run` 因此可能因为残留项
    ///   恒为真，与 `_refreshSpeedMeter` 同病（单测
    ///   `cancel_work_prefers_the_queue_work_id` 钉住）；
    /// - `stopped_seeding` 取**入口处**的 `isSeeding`（= `_seeding` 里有这个 id），
    ///   与 `pop` 是否真的摘掉无关；
    /// - 协程取消本身归引擎（[`CancelWork::work_id`]）。
    pub fn cancel_work(&mut self, task_id: &str) -> CancelWork {
        let stopped_seeding = self.seeding.contains_key(task_id);
        let work_id = match self.queue.work_id_of(task_id) {
            Some(work_id) => Some(work_id.to_string()),
            None => self.seeding.remove(task_id),
        };
        self.queue.cancel(task_id);
        CancelWork {
            work_id,
            stopped_seeding,
        }
    }

    /// `Task.isSeeding`（Python 的非持久化字段）：`_seeding` 里有这个 id。
    pub fn is_seeding(&self, task_id: &str) -> bool {
        self.seeding.contains_key(task_id)
    }

    /// `_seeding[taskId]`：做种协程的 workId。
    pub fn seeding_work_id(&self, task_id: &str) -> Option<&str> {
        self.seeding.get(task_id).map(String::as_str)
    }

    /// `len(self._seeding)`。
    pub fn seeding_count(&self) -> usize {
        self.seeding.len()
    }

    /// `_refreshSpeedMeter()`（`task_service.py:659-663`）的判定：
    /// `if self._queue.runningCount() or self._seeding: start() else: stop()`。
    pub fn speed_meter_should_run(&self) -> bool {
        self.queue.running_count() > 0 || !self.seeding.is_empty()
    }

    /// `_seed(task, isManual)`（`task_service.py:585-593`）：登记 workId +
    /// `seedingStarted` + `_refreshSpeedMeter()`。
    ///
    /// ```python
    /// task.isSeeding = True
    /// self._seeding[task.taskId] = self._coroutineRunner.submit(task.runSeeding(isManual), ...)
    /// self.seedingStarted.emit(task)
    /// self._refreshSpeedMeter()
    /// ```
    ///
    /// 不检查 `canSeed` / `shouldSeed`——Python 的 `_seed` 也不检查（调用点
    /// [`TaskService::should_start_seeding`] 已经判过）；`isManual` 只影响
    /// `runSeeding` 本体，不落账。任务不在列表里 → `None`（Python 那边 `_seed`
    /// 只从 `_tasks` 里取到过的 `task` 调用，走不到这条）。
    pub fn seed(&mut self, task_id: &str, work_id: &str) -> Option<SeedingEvent> {
        self.task(task_id)?;
        self.seeding
            .insert(task_id.to_string(), work_id.to_string());
        Some(SeedingEvent::Started)
    }

    /// `_onSeedingDone(task)`（`task_service.py:595-597`）：做种协程正常结束 →
    /// `shouldSeed = False` + `_onSeedingEnded(task)`。
    ///
    /// 不单独 `_flushSoon()`：`_onSeedingEnded` 里已经有一次。
    pub fn on_seeding_done(&mut self, task_id: &str) -> SeedingEvent {
        if let Some(record) = self.task_mut(task_id) {
            record.should_seed = false;
        }
        self.on_seeding_ended(task_id)
    }

    /// `_onSeedingEnded(task)`（`task_service.py:599-604`）：摘 `_seeding` +
    /// `isSeeding = False` + 落盘 + `seedingStopped` + `_refreshSpeedMeter()`。
    ///
    /// Python 无条件 `emit`（哪怕 `_seeding` 里本来就没有这条），所以返回的不是
    /// `Option`；`pop(taskId, None)` 的缺省在这里等价于 `remove` 的忽略。
    pub fn on_seeding_ended(&mut self, task_id: &str) -> SeedingEvent {
        self.seeding.remove(task_id);
        self.pending_flush = true;
        SeedingEvent::Stopped
    }

    /// 引擎真的开始跑了（`_dispatch` 里 `queue.run(taskId, workId)` 的 workId 回填）。
    pub fn on_run_started(&mut self, task_id: &str, work_id: &str) {
        self.queue.run(task_id, work_id);
    }

    /// `_onRunDone` / `_onRunFailed` 的记账部分，**含末尾的 `_pump()`**。
    ///
    /// Python 顺序：`queue.done` → `_flushSoon` → 发 `taskCompleted`/`taskFailed`
    /// → 做种/监听 → `_pump()` → `if runningCount() == 0: tasksAllCompleted`。
    /// 所以 `all_completed` 必须在派发**之后**算——否则「刚做完一个、正好接上
    /// 下一个等待任务」会被误判成全完成。
    pub fn on_run_finished(&mut self, task_id: &str, max_task_num: usize) -> RunOutcome {
        self.queue.done(task_id);
        self.pending_flush = true;
        let started = self.pump(max_task_num);
        RunOutcome {
            started,
            all_completed: self.queue.running_count() == 0,
        }
    }

    /// `_pump()`：`while runningCount() < maxTaskNum: nextWaiting() → dispatch`。
    ///
    /// 派发 = `queue.run(id, "")` 占位 + 返回 id 列表；`taskStarted` 与协程提交
    /// 由调用方做。列表里的任务若已不在服务里（Python 的 `task is None` 分支）
    /// 直接跳过，但**仍然被 `nextWaiting()` 弹出**，与 Python 一致。
    pub fn pump(&mut self, max_task_num: usize) -> Vec<String> {
        let mut started = Vec::new();
        while self.queue.running_count() < max_task_num {
            let Some(task_id) = self.queue.next_waiting() else {
                break;
            };
            if self.task(&task_id).is_none() {
                continue;
            }
            self.queue.run(&task_id, "");
            started.push(task_id);
        }
        started
    }

    /// `_rebalance(*_args)`（`task_service.py:639-647`）：`maxTaskNum` 变小时把超出
    /// 限额的运行中任务取消并放回等待队列，然后 `_pump()`。
    ///
    /// ```python
    /// for taskId in self._queue.runningIds()[cfg.maxTaskNum.value:]:
    ///     task = self._store.taskById(taskId)
    ///     if task is not None and task.canPause:
    ///         self._cancelWork(task)
    ///         task.setStatus(TaskStatus.WAITING)
    ///         self._queue.wait(taskId)
    /// self._pump()
    /// ```
    ///
    /// 三处口径：
    /// - `runningIds()[max:]` 是**切片**：`max` 大于运行数时一个都不动；
    /// - `canPause` 走 `facts::can_pause`（要读第一个 `RUNNING` 步骤的规则），
    ///   不可暂停的任务**原样留在运行中**（Python 同样跳过）；
    /// - **不置 `pending_flush`**：Python 的 `setStatus` 不落盘，靠下一次真实改动写。
    pub fn rebalance(&mut self, max_task_num: usize) -> RebalanceReport {
        let mut report = RebalanceReport::default();
        let overflow: Vec<String> = self
            .queue
            .running_ids()
            .into_iter()
            .skip(max_task_num)
            .collect();
        for task_id in overflow {
            if !self.task(&task_id).is_some_and(facts::can_pause) {
                continue;
            }
            let work = self.cancel_work(&task_id);
            if let Some(record) = self.task_mut(&task_id) {
                record.status = TaskStatus::Waiting;
            }
            self.queue.wait(&task_id);
            report.cancelled.push((task_id.clone(), work));
            report.requeued.push(task_id);
        }
        report.started = self.pump(max_task_num);
        report
    }

    /// `resumeSaved()`：启动时把持久化里的 `WAITING`/`RUNNING` 重新排进队列。
    ///
    /// Python 的 `resumeSaved()` 还做两件引擎侧的事：已完成任务的
    /// `_watchFile`/`_seed`，以及每个任务发 `taskAdded`。这里只做
    /// 「`RUNNING` → `WAITING` + `queue.wait`」，返回重新入队的 id；
    /// 「该做种」的那半边见 [`TaskService::resume_seeding_candidates`]。
    ///
    /// Python 的 `_schedule` 会顺带 `_pump()`，但没有引擎时「派发」无意义，
    /// 所以**不在这里 pump**——调用方在引擎就绪后自己 `pump(maxTaskNum)`。
    /// 状态从 `RUNNING` 改回 `WAITING` 也不主动置 `pending_flush`：
    /// Python 的 `setStatus` 同样不落盘，要等下一次真实改动才写。
    pub fn resume_saved(&mut self) -> Vec<String> {
        let mut resumed = Vec::new();
        for record in &mut self.records {
            if matches!(record.status, TaskStatus::Waiting | TaskStatus::Running) {
                record.status = TaskStatus::Waiting;
                resumed.push(record.task_id.clone());
            }
        }
        for task_id in &resumed {
            self.queue.wait(task_id);
        }
        resumed
    }

    /// `_onRunDone` 尾部（`task_service.py:672-673`）：`status == COMPLETED and
    /// task.canSeed and task.shouldSeed` → 该 `_seed(task)`。
    ///
    /// 判定顺序与 Python 一致（`status` 先判，短路掉 `canSeed`/`shouldSeed`）；
    /// 协程 workId 由调用方补。
    pub fn should_start_seeding(&self, task_id: &str) -> bool {
        let Some(record) = self.task(task_id) else {
            return false;
        };
        record.status == TaskStatus::Completed
            && facts::task_kind(record.type_name.as_deref()).can_seed
            && record.should_seed
    }

    /// `resumeSaved()` 里「已完成且文件还在 → 该做种」的那半边
    /// （`task_service.py:529-531`）。
    ///
    /// ```python
    /// if task.status == COMPLETED and task.hasOutputFile and isExisting(task.outputPath):
    ///     self._watchFile(task)
    ///     if task.canSeed and task.shouldSeed:
    ///         self._seed(task)
    /// ```
    ///
    /// `_watchFile` 归引擎；这里只返回「该 `_seed` 的 taskId」（workId 由调用方补）。
    /// `hasOutputFile` 恒为真（`facts::task_kind`），留着是为了跟 Python 的形状对齐。
    pub fn resume_seeding_candidates(&self) -> Vec<String> {
        self.records
            .iter()
            .filter(|record| {
                let kind = facts::task_kind(record.type_name.as_deref());
                record.status == TaskStatus::Completed
                    && kind.has_output_file
                    && facts::is_existing(&record.output_path())
                    && kind.can_seed
                    && record.should_seed
            })
            .map(|record| record.task_id.clone())
            .collect()
    }

    /// `checksumProgress(task)`（`task_service.py:408-410`）：没有登记项 → `None`。
    pub fn checksum_progress(&self, task_id: &str) -> Option<i64> {
        self.checksums.get(task_id).map(|work| work.progress)
    }

    /// `self._checksums[taskId]`（`workId` + `progress`）。
    pub fn checksum_work(&self, task_id: &str) -> Option<&ChecksumWork> {
        self.checksums.get(task_id)
    }

    /// `len(self._checksums)`。
    pub fn checksum_count(&self) -> usize {
        self.checksums.len()
    }

    /// `startChecksum(task, algorithm)` 的记账部分（`task_service.py:412-420`）。
    ///
    /// ```python
    /// work = ChecksumWork()
    /// self._checksums[task.taskId] = work
    /// work.workId = self._coroutineRunner.submit(toChecksum(...), done=..., failed=...)
    /// self.checksumStarted.emit(task)
    /// ```
    ///
    /// `algorithm` 只用于引擎构造 `toChecksum(path, algorithm, onProgress)`，**不落账**
    /// （`ChecksumWork` 只有 `workId` / `progress` 两个字段）；workId 由引擎提交协程后
    /// 回填。任务不在列表里 → `None`（Python 不校验，因为调用点拿的一定是真任务）。
    pub fn start_checksum(&mut self, task_id: &str, work_id: &str) -> Option<ChecksumEvent> {
        self.task(task_id)?;
        self.checksums.insert(
            task_id.to_string(),
            ChecksumWork {
                work_id: work_id.to_string(),
                progress: 0,
            },
        );
        Some(ChecksumEvent::Started)
    }

    /// `toChecksum` 的 `onProgress` 回调（`lambda p: setattr(work, "progress", p)`）。
    ///
    /// Python 的闭包持有 `work` 对象：即使 `cancelChecksum` 已经把它从 `_checksums`
    /// 里摘掉，迟到的进度仍会写进那个孤儿对象（界面上看不见）。这里只写登记表里的
    /// 那条，返回是否真的写了。
    pub fn set_checksum_progress(&mut self, task_id: &str, progress: i64) -> bool {
        match self.checksums.get_mut(task_id) {
            Some(work) => {
                work.progress = progress;
                true
            }
            None => false,
        }
    }

    /// `cancelChecksum(task)`（`task_service.py:422-427`）：摘掉登记项 + 取消协程 +
    /// `checksumStopped.emit(task, None)`。
    ///
    /// 返回要取消的 workId：`Some` 意味着调用方还要发 `checksumStopped(task, None)`；
    /// 没有登记项时 Python 直接 `return`（不发信号、不取消协程）。
    pub fn cancel_checksum(&mut self, task_id: &str) -> Option<String> {
        self.checksums.remove(task_id).map(|work| work.work_id)
    }

    /// `_onChecksumDone`（`task_service.py:617-621`）：`del _checksums[taskId]` →
    /// `task.checksums[algorithm] = checksum` → `_flushSoon()` →
    /// `checksumCompleted.emit(task, algorithm)`。
    ///
    /// Python 的 `del` 在登记项不存在时抛 `KeyError`：这里退化成 `None`（不写、不发）。
    /// 任务不在列表里也 `None`（Python 会 `AttributeError`）——但登记项仍被摘掉。
    pub fn on_checksum_done(
        &mut self,
        task_id: &str,
        algorithm: &str,
        checksum: &str,
    ) -> Option<ChecksumEvent> {
        self.checksums.remove(task_id)?;
        let record = self.task_mut(task_id)?;
        record
            .checksums
            .insert(algorithm.to_string(), Value::String(checksum.to_string()));
        // `_flushSoon()`：`checksums` 是持久化字段。
        self.pending_flush = true;
        Some(ChecksumEvent::Completed {
            algorithm: algorithm.to_string(),
        })
    }

    /// `_onChecksumFailed`（`task_service.py:623-626`）：`del` +
    /// `checksumStopped.emit(task, TaskError("无法读取文件：{detail}", detail=str(error)))`。
    ///
    /// 与 Python 一致：**不置脏、不写 `task.checksums`**（失败没有结果可写）。
    /// 任务不在列表里也照样发信号——Python 拿的是活对象，`del` 只碰 `_checksums`。
    pub fn on_checksum_failed(&mut self, task_id: &str, detail: &str) -> ChecksumEvent {
        self.checksums.remove(task_id);
        ChecksumEvent::Stopped {
            error: Some(TaskError::new(
                "无法读取文件：{detail}",
                vec![("detail", detail.to_string())],
            )),
        }
    }

    /// `stop()`：把在跑/等待的任务标成 `PAUSED`。
    ///
    /// Python 的 `TaskService.stop()` 只 `setStatus(PAUSED)`，**不动队列**、
    /// 也不 `_flushSoon()`——落盘靠 `Engine.stop()` 紧接着的那次 `flush()`
    /// （而 `TaskStore.flush()` 无脏标记，每次都写）。Rust 的 `flush()` 有
    /// `pending_flush` 短路，所以这里必须显式置脏，否则停机时的 `PAUSED`
    /// 永远不会落盘（有单测 `stop_flush_persists_paused_status` 钉住）。
    ///
    /// 队列不动是**有意保留的缺陷**：停机后 `running_count()` 仍可能 >0，
    /// 真正的协程取消归引擎层（`_cancelWork`）。
    pub fn stop(&mut self) {
        let mut changed = false;
        for record in &mut self.records {
            if matches!(record.status, TaskStatus::Running | TaskStatus::Waiting) {
                record.status = TaskStatus::Paused;
                changed = true;
            }
        }
        if changed {
            self.pending_flush = true;
        }
    }

    /// `runningCount()`。
    pub fn running_count(&self) -> usize {
        self.queue.running_count()
    }

    /// `waitingOrder()`。
    pub fn waiting_order(&self) -> Vec<String> {
        self.queue.waiting_order()
    }

    /// `moveToFront(taskIds)`：返回是否真的动了（`queueChanged` 的触发条件）。
    pub fn move_to_front(&mut self, task_ids: &[String]) -> bool {
        self.queue.move_to_front(task_ids)
    }

    /// `runningProgress()`：只统计 `RUNNING` 且 `fileSize > 0` 的任务。
    ///
    /// `totalSize == 0` → `-1.0`（Python 的哨兵值，UI 侧据此显示「—」）。
    pub fn running_progress(&self) -> f64 {
        let mut total_received: i64 = 0;
        let mut total_size: i64 = 0;
        for record in &self.records {
            if record.status != TaskStatus::Running {
                continue;
            }
            let (_, _, received) = record.current_snapshot();
            if record.file_size > 0 {
                total_received += received;
                total_size += record.file_size;
            }
        }
        if total_size == 0 {
            return -1.0;
        }
        (total_received as f64 / total_size as f64 * 100.0).min(100.0)
    }

    /// `fileDeleteDenied` 只提示一次：首次调用返回 `true`。
    pub fn note_delete_denied(&mut self) -> bool {
        if self.delete_denied_notified {
            return false;
        }
        self.delete_denied_notified = true;
        true
    }

    /// `TaskStore.flush()`：写 `tasks.jsonl`，返回是否真的写了。
    ///
    /// 未 load 或没有待落盘改动 → `Ok(false)`（Python 两处短路）。
    pub fn flush(&mut self, path: &Path) -> io::Result<bool> {
        if !self.loaded || !self.pending_flush {
            return Ok(false);
        }
        store::save_records(path, &self.records)?;
        self.pending_flush = false;
        Ok(true)
    }

    /// 把当前记录推成 UI 行。
    ///
    /// `isSeeding` 走 [`TaskService::is_seeding`]（第 97 步的做种记账），
    /// `canSeed`/`hasOutputFile` 走 `facts::task_kind`（类属性，按 `type` 反查）；
    /// `canPause`（要读步骤状态）、`completedAtText`（要时区）、`outputExists`
    /// （要 stat）、`lastError`（要挑失败步骤）仍由调用方换成真实取值——
    /// `push_task_records` 的注入点就是为它们留的。
    pub fn refresh(&self, window: &AppWindow) {
        crate::push_task_records(window, &self.records, |record| {
            let kind = facts::task_kind(record.type_name.as_deref());
            CardRuntime {
                can_seed: kind.can_seed,
                has_output_file: kind.has_output_file,
                is_seeding: self.is_seeding(&record.task_id),
                ..CardRuntime::default()
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use gd3_core::task::{Progress, TaskStep};
    use serde_json::Map;

    fn record(task_id: &str, status: TaskStatus, file_size: i64) -> TaskRecord {
        TaskRecord {
            name: format!("{task_id}.bin"),
            url: format!("https://example.com/{task_id}"),
            pack_id: String::new(),
            task_id: task_id.to_string(),
            status,
            steps: Vec::new(),
            created_at: 0,
            completed_at: 0,
            output_folder: String::new(),
            file_size,
            files: None,
            category: None,
            should_seed: false,
            checksums: Map::new(),
            extra: Map::new(),
            type_name: None,
        }
    }

    fn step(received: i64) -> TaskStep {
        TaskStep {
            step_index: 0,
            file_index: None,
            step_id: format!("step-{received}"),
            status: TaskStatus::Running,
            progress: Progress::Int(0),
            received_bytes: received,
            speed: 0,
            extra: Map::new(),
            type_name: None,
        }
    }

    #[test]
    fn new_service_is_not_loaded_so_flush_is_a_noop() {
        let mut service = TaskService::new();
        service.mark_dirty();
        let path = std::env::temp_dir().join("gd3-ui-task-service-unloaded.jsonl");
        let _ = std::fs::remove_file(&path);
        assert!(!service.flush(&path).expect("flush 不应报错"));
        assert!(!path.exists(), "未 load 时不得写盘");
    }

    #[test]
    fn load_of_missing_file_is_empty_and_loaded() {
        let path = std::env::temp_dir().join("gd3-ui-task-service-missing-dir/tasks.jsonl");
        let _ = std::fs::remove_file(&path);
        let (service, failures) = TaskService::load(&path).expect("缺失文件不算错误");
        assert!(failures.is_empty());
        assert!(
            service.is_loaded(),
            "Python 里 _loaded = True 之后才 return"
        );
        assert!(service.is_empty());
        // 已 load → 之后真的会写盘。
        let mut service = service;
        service.mark_dirty();
        assert!(service.flush(&path).expect("写盘"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn add_rejects_duplicate_task_id() {
        let mut service = TaskService::new();
        assert_eq!(
            service.add(record("a", TaskStatus::Waiting, 10), true),
            AddOutcome::Added { queued: true }
        );
        assert_eq!(
            service.add(record("a", TaskStatus::Waiting, 10), true),
            AddOutcome::Duplicate
        );
        assert_eq!(service.len(), 1);
        assert_eq!(service.waiting_order(), vec!["a".to_string()]);
    }

    #[test]
    fn add_marks_dirty_and_forces_should_seed() {
        let mut service = TaskService::new();
        let mut input = record("a", TaskStatus::Waiting, 10);
        input.should_seed = false;
        service.add(input, true);
        assert!(service.pending_flush());
        assert!(service.task("a").expect("任务在").should_seed);
    }

    #[test]
    fn add_queues_whenever_auto_start_even_without_known_size() {
        let mut service = TaskService::new();
        // fileSize == 0 也入队：Python 的磁盘检查块之后是**无条件** `_schedule`。
        assert_eq!(
            service.add(record("a", TaskStatus::Waiting, 0), true),
            AddOutcome::Added { queued: true }
        );
        // autoStart=False 不入队。
        assert_eq!(
            service.add(record("b", TaskStatus::Waiting, 10), false),
            AddOutcome::Added { queued: false }
        );
        assert_eq!(service.waiting_order(), vec!["a".to_string()]);
    }

    #[test]
    fn add_with_free_space_reports_insufficient_disk_and_skips_queue() {
        let mut service = TaskService::new();
        assert_eq!(
            service.add_with_free_space(record("a", TaskStatus::Waiting, 10), true, Some(5)),
            AddOutcome::DiskSpaceInsufficient { free: 5 }
        );
        // 记录仍在列表里（Python 先 store.add 再查磁盘），但不入队。
        assert_eq!(service.len(), 1);
        assert!(service.waiting_order().is_empty());
        assert!(service.pending_flush());
    }

    #[test]
    fn add_with_free_space_ignores_check_when_size_unknown_or_oserror() {
        let mut service = TaskService::new();
        // fileSize == 0 → Python 根本不查磁盘。
        assert_eq!(
            service.add_with_free_space(record("a", TaskStatus::Waiting, 0), true, Some(0)),
            AddOutcome::Added { queued: true }
        );
        // disk_usage 抛 OSError → None → 跳过检查照常入队。
        assert_eq!(
            service.add_with_free_space(record("b", TaskStatus::Waiting, 10), true, None),
            AddOutcome::Added { queued: true }
        );
        // 空间够 → 入队。
        assert_eq!(
            service.add_with_free_space(record("c", TaskStatus::Waiting, 10), true, Some(10)),
            AddOutcome::Added { queued: true }
        );
    }

    #[test]
    fn remove_cancels_queue_entry_and_returns_record() {
        let mut service = TaskService::new();
        service.add(record("a", TaskStatus::Waiting, 10), true);
        service.on_run_started("a", "work-1");
        let removed = service.remove("a").expect("应移除");
        assert_eq!(removed.task_id, "a");
        assert_eq!(service.len(), 0);
        assert_eq!(service.running_count(), 0);
        assert!(service.waiting_order().is_empty());
        assert!(service.remove("a").is_none());
    }

    #[test]
    fn start_is_noop_when_waiting_or_running() {
        let mut service = TaskService::new();
        service.add(record("a", TaskStatus::Waiting, 10), true);
        assert!(!service.start("a"), "已在等待队列");
        service.cancel_work("a");
        assert!(service.start("a"), "已出队 → 可以重新入队");
        service.cancel_work("a");
        service.on_run_started("a", "work-1");
        assert!(!service.start("a"), "已在运行");
        assert!(!service.start("missing"), "任务不存在");
    }

    #[test]
    fn pause_cancels_queue_and_marks_record_paused() {
        let mut service = TaskService::new();
        service.add(record("a", TaskStatus::Running, 10), true);
        service.on_run_started("a", "work-1");
        let report = service.pause("a", 1).expect("任务在");
        assert_eq!(report.cancel.work_id.as_deref(), Some("work-1"));
        assert!(report.started.is_empty(), "等待队列为空");
        assert_eq!(service.running_count(), 0);
        assert_eq!(
            service.task("a").expect("任务在").status,
            TaskStatus::Paused
        );
        assert!(service.pending_flush());
        assert!(service.pause("missing", 1).is_none());
    }

    #[test]
    fn pump_dispatches_up_to_max_task_num() {
        let mut service = TaskService::new();
        for id in ["a", "b", "c"] {
            service.add(record(id, TaskStatus::Waiting, 10), true);
        }
        assert_eq!(
            service.pump(2),
            vec!["a".to_string(), "b".to_string()],
            "按 nextWaiting() 顺序派发到上限为止"
        );
        assert_eq!(service.running_count(), 2);
        assert_eq!(service.waiting_order(), vec!["c".to_string()]);
        assert!(service.pump(2).is_empty(), "已达上限，不再派发");
    }

    #[test]
    fn pump_skips_waiting_ids_without_a_record() {
        let mut service = TaskService::new();
        service.add(record("a", TaskStatus::Waiting, 10), true);
        // 直接把一个不存在的 id 塞进等待队列（Python 里也会被 nextWaiting 弹出）。
        service.queue.wait("ghost");
        assert_eq!(service.pump(4), vec!["a".to_string()]);
        assert!(service.waiting_order().is_empty());
        assert_eq!(service.running_count(), 1);
    }

    #[test]
    fn on_run_finished_reports_all_completed_only_after_pump() {
        let mut service = TaskService::new();
        service.add(record("a", TaskStatus::Running, 10), true);
        service.add(record("b", TaskStatus::Running, 10), true);
        assert_eq!(service.pump(2), vec!["a".to_string(), "b".to_string()]);
        assert_eq!(service.running_count(), 2);

        let first = service.on_run_finished("a", 2);
        assert!(first.started.is_empty(), "没有等待中的任务");
        assert!(!first.all_completed, "还剩一个在跑");

        let second = service.on_run_finished("b", 2);
        assert!(second.all_completed, "运行中清零 → tasksAllCompleted");
        assert!(service.pending_flush(), "完成要落盘");
    }

    #[test]
    fn on_run_finished_pumps_a_waiting_task_so_not_all_completed() {
        let mut service = TaskService::new();
        for id in ["a", "b"] {
            service.add(record(id, TaskStatus::Running, 10), true);
        }
        assert_eq!(service.pump(1), vec!["a".to_string()]);

        let outcome = service.on_run_finished("a", 1);
        assert_eq!(outcome.started, vec!["b".to_string()]);
        assert!(!outcome.all_completed, "刚接上一个等待任务，不算全完成");
        assert_eq!(service.running_count(), 1);
    }

    #[test]
    fn resume_saved_requeues_waiting_and_running_only() {
        let mut service = TaskService::from_records(vec![
            record("a", TaskStatus::Waiting, 10),
            record("b", TaskStatus::Running, 10),
            record("c", TaskStatus::Completed, 10),
            record("d", TaskStatus::Paused, 10),
        ]);
        assert_eq!(
            service.resume_saved(),
            vec!["a".to_string(), "b".to_string()],
            "只重排 WAITING/RUNNING，且保插入序"
        );
        assert_eq!(
            service.task("b").expect("任务在").status,
            TaskStatus::Waiting,
            "RUNNING 回落到 WAITING"
        );
        assert_eq!(
            service.task("c").expect("任务在").status,
            TaskStatus::Completed
        );
        assert_eq!(
            service.task("d").expect("任务在").status,
            TaskStatus::Paused
        );
        assert_eq!(
            service.waiting_order(),
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(service.running_count(), 0, "不 pump：没有引擎就不假装派发");
        assert!(
            !service.pending_flush(),
            "与 Python 的 setStatus 一样不主动落盘"
        );
    }

    #[test]
    fn running_progress_returns_minus_one_without_known_size() {
        let mut service = TaskService::new();
        service.add(record("a", TaskStatus::Running, 0), true);
        assert_eq!(service.running_progress(), -1.0);
    }

    #[test]
    fn running_progress_only_counts_running_tasks() {
        let mut service = TaskService::new();
        let mut running = record("a", TaskStatus::Running, 200);
        running.steps = vec![step(50)];
        let mut paused = record("b", TaskStatus::Paused, 1000);
        paused.steps = vec![step(1000)];
        service.add(running, true);
        service.add(paused, true);
        // 50/200 = 25%；暂停的 1000 字节不计入。
        assert!((service.running_progress() - 25.0).abs() < 1e-9);
    }

    #[test]
    fn running_progress_caps_at_one_hundred() {
        let mut service = TaskService::new();
        let mut running = record("a", TaskStatus::Running, 100);
        running.steps = vec![step(300)];
        service.add(running, true);
        assert_eq!(service.running_progress(), 100.0);
    }

    #[test]
    fn move_to_front_reorders_waiting_queue() {
        let mut service = TaskService::new();
        for id in ["a", "b", "c"] {
            service.add(record(id, TaskStatus::Waiting, 10), true);
        }
        assert!(service.move_to_front(&["c".to_string(), "a".to_string()]));
        assert_eq!(
            service.waiting_order(),
            vec!["a".to_string(), "c".to_string(), "b".to_string()]
        );
        assert!(!service.move_to_front(&["zzz".to_string()]));
    }

    #[test]
    fn delete_denied_notifies_only_once() {
        let mut service = TaskService::new();
        assert!(service.note_delete_denied());
        assert!(!service.note_delete_denied());
    }

    #[test]
    fn flush_writes_jsonl_and_clears_pending_flag() {
        let dir = std::env::temp_dir().join(format!("gd3-ui-task-service-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let path = dir.join("tasks.jsonl");

        let mut service = TaskService::from_records(vec![record("a", TaskStatus::Running, 10)]);
        assert!(!service.pending_flush());
        assert!(!service.flush(&path).expect("空 flush"), "无改动不写盘");
        assert!(!path.exists());

        service.mark_dirty();
        assert!(service.flush(&path).expect("写盘"));
        assert!(!service.pending_flush());
        let outcome = store::load_records(&path).expect("读回");
        assert!(outcome.failures.is_empty());
        assert_eq!(outcome.records.len(), 1);
        assert_eq!(outcome.records[0].task_id, "a");
        assert_eq!(outcome.records[0].status, TaskStatus::Running);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refresh_pushes_one_row_per_record() {
        let window = crate::create_headless();
        let mut service = TaskService::new();
        service.add(record("a", TaskStatus::Waiting, 10), true);
        service.add(record("b", TaskStatus::Completed, 20), false);
        service.refresh(&window);
        assert_eq!(slint::Model::row_count(&window.get_tasks()), 2);
        let first = slint::Model::row_data(&window.get_tasks(), 0).expect("首行");
        assert_eq!(first.taskId.as_str(), "a");
        let second = slint::Model::row_data(&window.get_tasks(), 1).expect("次行");
        assert_eq!(second.taskId.as_str(), "b");
    }

    /// 临时目录（`gd3-ui` 的 dev-dependencies 里没有 `tempfile`）。
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(tag: &str) -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let index = NEXT.fetch_add(1, Ordering::SeqCst);
            let path =
                std::env::temp_dir().join(format!("gd3ui-ts-{tag}-{}-{index}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("建临时目录");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// 输出目录在 `dir` 里的记录（`fileSize` 默认 10）。
    fn record_in(dir: &Path, task_id: &str, status: TaskStatus) -> TaskRecord {
        let mut record = record(task_id, status, 10);
        record.output_folder = dir.to_string_lossy().into_owned();
        record
    }

    /// 占位符一律「删除失败」：单测不动真回收站。
    fn never_trash(_path: &Path) -> bool {
        false
    }

    fn placeholder(dir: &Path, name: &str) -> PathBuf {
        dir.join(name)
    }

    fn file(index: i64, relative_path: &str) -> gd3_core::task::TaskFile {
        gd3_core::task::TaskFile {
            index,
            relative_path: relative_path.to_string(),
            size: 0,
            selected: true,
            downloaded_bytes: 0,
            completed: false,
            extra: Map::new(),
        }
    }

    #[test]
    fn add_with_conflict_creates_placeholder_and_registers_it() {
        let dir = TempDir::new("create");
        let mut service = TaskService::new();
        // 输出目录还不存在：Python 先 `mkdir(parents=True, exist_ok=True)`。
        let output = dir.path().join("nested/deep");
        let mut input = record_in(&output, "a", TaskStatus::Waiting);
        input.name = "a.bin".to_string();
        let report = service
            .add_with_conflict(input, NameConflictChoice::KeepBoth, true, None, never_trash)
            .expect("建占位符");
        assert_eq!(report.name, "a.bin");
        assert!(report.replaced.is_empty());
        assert!(!report.overwrite_failed);
        assert_eq!(report.outcome, Some(AddOutcome::Added { queued: true }));
        assert!(output.is_dir(), "输出目录应被建出来");
        let path = placeholder(&output, "a.bin");
        assert!(path.is_file(), "单文件任务的占位符是 0 字节文件");
        assert_eq!(std::fs::metadata(&path).expect("metadata").len(), 0);
        assert_eq!(service.placeholders().owner(&path), Some("a"));
        assert_eq!(service.len(), 1);
    }

    #[test]
    fn add_with_conflict_renames_when_placeholder_is_taken() {
        let dir = TempDir::new("rename");
        let mut service = TaskService::new();
        let mut first = record_in(dir.path(), "a", TaskStatus::Waiting);
        first.name = "a.bin".to_string();
        service
            .add_with_conflict(
                first,
                NameConflictChoice::KeepBoth,
                false,
                None,
                never_trash,
            )
            .expect("第一个任务");
        let mut second = record_in(dir.path(), "b", TaskStatus::Waiting);
        second.name = "a.bin".to_string();
        let report = service
            .add_with_conflict(
                second,
                NameConflictChoice::KeepBoth,
                false,
                None,
                never_trash,
            )
            .expect("第二个任务");
        assert_eq!(report.name, "a(1).bin");
        assert!(report.replaced.is_empty());
        assert!(!report.overwrite_failed);
        let renamed = placeholder(dir.path(), "a(1).bin");
        assert!(renamed.is_file());
        assert_eq!(service.placeholders().owner(&renamed), Some("b"));
        assert_eq!(service.len(), 2);
    }

    #[test]
    fn add_with_conflict_ask_leaves_the_task_out() {
        let dir = TempDir::new("ask");
        // 磁盘上的无主占位符：`_isUnfinishedPlaceholder` 查不到任务 → 视为已完成。
        let taken = placeholder(dir.path(), "a.bin");
        std::fs::write(&taken, b"x").expect("写已存在文件");
        let mut service = TaskService::new();
        let mut input = record_in(dir.path(), "a", TaskStatus::Waiting);
        input.name = "a.bin".to_string();
        let report = service
            .add_with_conflict(input, NameConflictChoice::Ask, true, None, never_trash)
            .expect("Ask 不是错误");
        assert_eq!(report.outcome, None, "Ask 撞上无主占位符 → 放弃本次添加");
        assert_eq!(report.name, "a.bin");
        assert!(report.replaced.is_empty());
        assert!(!report.overwrite_failed);
        assert_eq!(service.len(), 0, "任务没加进去");
        assert!(service.waiting_order().is_empty());
        assert!(service.placeholders().is_empty());
        assert!(
            !service.pending_flush(),
            "nameConflicted 分支不碰 _flushSoon"
        );
        assert_eq!(std::fs::metadata(&taken).expect("metadata").len(), 1);
    }

    #[test]
    fn add_with_conflict_overwrite_reuses_the_name() {
        let dir = TempDir::new("overwrite");
        let taken = placeholder(dir.path(), "a.bin");
        std::fs::write(&taken, b"old").expect("写已存在文件");
        let mut trashed: Vec<PathBuf> = Vec::new();
        let mut service = TaskService::new();
        let mut input = record_in(dir.path(), "a", TaskStatus::Waiting);
        input.name = "a.bin".to_string();
        let report = service
            .add_with_conflict(
                input,
                NameConflictChoice::Overwrite,
                true,
                None,
                |path: &Path| {
                    trashed.push(path.to_path_buf());
                    std::fs::remove_file(path).is_ok()
                },
            )
            .expect("覆盖");
        assert_eq!(report.name, "a.bin", "覆盖成功不改名");
        assert!(!report.overwrite_failed);
        assert_eq!(trashed, vec![taken.clone()], "只对撞上的那一个路径动手");
        assert_eq!(report.outcome, Some(AddOutcome::Added { queued: true }));
        assert_eq!(std::fs::metadata(&taken).expect("metadata").len(), 0);
        assert_eq!(service.placeholders().owner(&taken), Some("a"));
    }

    #[test]
    fn add_with_conflict_overwrite_failure_degrades_to_keep_both() {
        let dir = TempDir::new("overwrite-failed");
        let taken = placeholder(dir.path(), "a.bin");
        std::fs::write(&taken, b"old").expect("写已存在文件");
        let mut service = TaskService::new();
        let mut input = record_in(dir.path(), "a", TaskStatus::Waiting);
        input.name = "a.bin".to_string();
        let report = service
            .add_with_conflict(
                input,
                NameConflictChoice::Overwrite,
                true,
                None,
                never_trash,
            )
            .expect("删除失败不是错误");
        assert_eq!(report.name, "a(1).bin", "删除失败 → 退化成 KeepBoth");
        assert!(report.overwrite_failed, "overwriteFailed 信号");
        assert_eq!(report.outcome, Some(AddOutcome::Added { queued: true }));
        assert_eq!(std::fs::metadata(&taken).expect("metadata").len(), 3);
    }

    #[test]
    fn add_with_conflict_reports_replaced_task() {
        let dir = TempDir::new("replaced");
        let mut service = TaskService::new();
        let mut first = record_in(dir.path(), "a", TaskStatus::Completed);
        first.name = "a.bin".to_string();
        service
            .add_with_conflict(
                first,
                NameConflictChoice::KeepBoth,
                false,
                None,
                never_trash,
            )
            .expect("旧任务");
        let mut second = record_in(dir.path(), "b", TaskStatus::Waiting);
        second.name = "a.bin".to_string();
        let report = service
            .add_with_conflict(
                second,
                NameConflictChoice::Overwrite,
                false,
                None,
                |path: &Path| std::fs::remove_file(path).is_ok(),
            )
            .expect("新任务");
        assert_eq!(report.replaced, vec!["a".to_string()]);
        assert_eq!(report.name, "a.bin");
        assert!(service.task("a").is_none(), "被替换的旧任务应移出服务");
        assert!(service.task("b").is_some());
        assert_eq!(service.len(), 1);
        assert_eq!(
            service
                .placeholders()
                .owner(&placeholder(dir.path(), "a.bin")),
            Some("b")
        );
    }

    #[test]
    fn add_with_conflict_short_circuits_on_duplicate() {
        let dir = TempDir::new("duplicate");
        let mut service = TaskService::new();
        let mut first = record_in(dir.path(), "a", TaskStatus::Waiting);
        first.name = "a.bin".to_string();
        service
            .add_with_conflict(
                first,
                NameConflictChoice::KeepBoth,
                false,
                None,
                never_trash,
            )
            .expect("首个");
        let mut calls = 0;
        let mut again = record_in(dir.path(), "a", TaskStatus::Waiting);
        again.name = "a.bin".to_string();
        let report = service
            .add_with_conflict(
                again,
                NameConflictChoice::Overwrite,
                true,
                None,
                |_: &Path| {
                    calls += 1;
                    true
                },
            )
            .expect("重复不是错误");
        assert_eq!(report.outcome, Some(AddOutcome::Duplicate));
        assert!(report.replaced.is_empty());
        assert!(!report.overwrite_failed);
        assert_eq!(calls, 0, "重复任务不进冲突循环");
        assert_eq!(service.len(), 1);
    }

    #[test]
    fn add_with_conflict_never_overwrites_unfinished_task() {
        let dir = TempDir::new("unfinished");
        let mut service = TaskService::new();
        let mut first = record_in(dir.path(), "a", TaskStatus::Running);
        first.name = "a.bin".to_string();
        service
            .add_with_conflict(
                first,
                NameConflictChoice::KeepBoth,
                false,
                None,
                never_trash,
            )
            .expect("运行中的任务");
        let mut calls = 0;
        let mut second = record_in(dir.path(), "b", TaskStatus::Waiting);
        second.name = "a.bin".to_string();
        let report = service
            .add_with_conflict(
                second,
                NameConflictChoice::Overwrite,
                false,
                None,
                |_: &Path| {
                    calls += 1;
                    true
                },
            )
            .expect("未完成占位符只能改名");
        assert_eq!(report.name, "a(1).bin");
        assert!(!report.overwrite_failed);
        assert_eq!(calls, 0, "未完成任务的占位符不许覆盖");
        assert_eq!(service.len(), 2);
    }

    #[test]
    fn add_with_conflict_creates_a_directory_for_multi_file_tasks() {
        let dir = TempDir::new("folder");
        let mut service = TaskService::new();
        let mut input = record_in(dir.path(), "a", TaskStatus::Waiting);
        input.name = "a.bin".to_string();
        input.files = Some(vec![file(0, "a.mp4"), file(1, "b.mp4")]);
        let report = service
            .add_with_conflict(
                input,
                NameConflictChoice::KeepBoth,
                false,
                None,
                never_trash,
            )
            .expect("多文件任务");
        assert_eq!(report.name, "a.bin");
        let path = placeholder(dir.path(), "a.bin");
        assert!(path.is_dir(), "isOutputFolder → 占位符是目录");
        assert_eq!(service.placeholders().owner(&path), Some("a"));
    }

    #[test]
    fn add_with_conflict_uses_the_same_tail_as_plain_add() {
        let dir = TempDir::new("tail");
        let mut service = TaskService::new();
        let mut low = record_in(dir.path(), "a", TaskStatus::Waiting);
        low.name = "a.bin".to_string();
        let report = service
            .add_with_conflict(
                low,
                NameConflictChoice::KeepBoth,
                true,
                Some(5),
                never_trash,
            )
            .expect("磁盘不足");
        assert_eq!(
            report.outcome,
            Some(AddOutcome::DiskSpaceInsufficient { free: 5 })
        );
        assert_eq!(service.len(), 1, "记录仍在列表里（先 add 再查磁盘）");
        assert!(service.waiting_order().is_empty(), "磁盘不足不入队");
        let mut idle = record_in(dir.path(), "b", TaskStatus::Waiting);
        idle.name = "b.bin".to_string();
        let report = service
            .add_with_conflict(idle, NameConflictChoice::KeepBoth, false, None, never_trash)
            .expect("不自动开始");
        assert_eq!(report.outcome, Some(AddOutcome::Added { queued: false }));
        assert!(service.pending_flush());
    }

    #[test]
    fn update_placeholders_returns_none_for_unknown_task() {
        let dir = TempDir::new("update-unknown");
        let mut service = TaskService::new();
        assert_eq!(
            service.update_placeholders("nope").expect("不是错误"),
            None,
            "Python 是 KeyError，Rust 退化成 Ok(None)"
        );
        let mut input = record_in(dir.path(), "a", TaskStatus::Waiting);
        input.name = "a.bin".to_string();
        service
            .add_with_conflict(
                input,
                NameConflictChoice::KeepBoth,
                false,
                None,
                never_trash,
            )
            .expect("建占位符");
        assert!(
            service
                .update_placeholders("nope")
                .expect("不是错误")
                .is_none(),
            "没有登记项的任务同样返回 None"
        );
    }

    #[test]
    fn update_placeholders_is_a_noop_when_paths_match() {
        let dir = TempDir::new("update-noop");
        let log = dir.path().join("tasks.jsonl");
        let (mut service, _) = TaskService::load(&log).expect("load");
        let mut input = record_in(dir.path(), "a", TaskStatus::Waiting);
        input.name = "a.bin".to_string();
        service
            .add_with_conflict(
                input,
                NameConflictChoice::KeepBoth,
                false,
                None,
                never_trash,
            )
            .expect("建占位符");
        service.flush(&log).expect("flush");
        assert!(!service.pending_flush());

        let report = service
            .update_placeholders("a")
            .expect("不是错误")
            .expect("有登记项");
        assert_eq!(report.name, "a.bin");
        assert_eq!(report.paths, vec![placeholder(dir.path(), "a.bin")]);
        assert!(report.created.is_empty());
        assert!(report.moved.is_empty());
        assert!(report.removed.is_empty());
        assert!(!service.pending_flush(), "无变化不落盘");
    }

    #[test]
    fn update_placeholders_renames_and_moves_the_file() {
        let dir = TempDir::new("update-rename");
        let mut service = TaskService::new();
        let mut input = record_in(dir.path(), "a", TaskStatus::Completed);
        input.name = "a.bin".to_string();
        service
            .add_with_conflict(
                input,
                NameConflictChoice::KeepBoth,
                false,
                None,
                never_trash,
            )
            .expect("建占位符");
        // 模拟下载完成：占位符被写满
        let old = placeholder(dir.path(), "a.bin");
        std::fs::write(&old, b"data").expect("写内容");
        service.task_mut("a").expect("任务在").name = "b.bin".to_string();

        let report = service
            .update_placeholders("a")
            .expect("不是错误")
            .expect("有登记项");
        let new = placeholder(dir.path(), "b.bin");
        assert_eq!(report.name, "b.bin");
        assert_eq!(report.paths, vec![new.clone()]);
        assert_eq!(report.created, vec![new.clone()], "新名字先建成占位符");
        assert_eq!(report.moved, vec![(old.clone(), new.clone())]);
        assert!(report.removed.is_empty());
        assert!(!old.exists());
        assert_eq!(std::fs::read(&new).expect("读回"), b"data");
        assert_eq!(service.task("a").expect("任务在").name, "b.bin");
        assert_eq!(service.placeholders().owner(&new), Some("a"));
        assert_eq!(
            service
                .placeholders()
                .get("a")
                .expect("登记项")
                .output_folder,
            dir.path()
        );
        assert!(service.pending_flush());
    }

    #[test]
    fn update_placeholders_moves_the_ghd_sidecar() {
        let dir = TempDir::new("update-ghd");
        let mut service = TaskService::new();
        let mut input = record_in(dir.path(), "a", TaskStatus::Running);
        input.name = "a.bin".to_string();
        service
            .add_with_conflict(
                input,
                NameConflictChoice::KeepBoth,
                false,
                None,
                never_trash,
            )
            .expect("建占位符");
        let old = placeholder(dir.path(), "a.bin");
        let old_part = sidecar_path(&old);
        std::fs::write(&old, b"data").expect("写内容");
        std::fs::write(&old_part, b"part").expect("写分片");
        service.task_mut("a").expect("任务在").name = "b.bin".to_string();

        let report = service
            .update_placeholders("a")
            .expect("不是错误")
            .expect("有登记项");
        let new = placeholder(dir.path(), "b.bin");
        let new_part = sidecar_path(&new);
        assert_eq!(report.moved, vec![(old, new), (old_part, new_part.clone())]);
        assert_eq!(std::fs::read(&new_part).expect("读回"), b"part");
    }

    #[test]
    fn update_placeholders_moves_across_folders() {
        let dir = TempDir::new("update-folder");
        let other = dir.path().join("other");
        let mut service = TaskService::new();
        let mut input = record_in(dir.path(), "a", TaskStatus::Completed);
        input.name = "a.bin".to_string();
        service
            .add_with_conflict(
                input,
                NameConflictChoice::KeepBoth,
                false,
                None,
                never_trash,
            )
            .expect("建占位符");
        let old = placeholder(dir.path(), "a.bin");
        std::fs::write(&old, b"data").expect("写内容");
        service.task_mut("a").expect("任务在").output_folder = other.to_string_lossy().into_owned();

        let report = service
            .update_placeholders("a")
            .expect("不是错误")
            .expect("有登记项");
        let new = other.join("a.bin");
        assert_eq!(report.name, "a.bin");
        assert_eq!(report.moved, vec![(old.clone(), new.clone())]);
        assert!(!old.exists(), "旧目录里的文件被搬走");
        assert_eq!(std::fs::read(&new).expect("读回"), b"data");
        assert_eq!(
            service
                .placeholders()
                .get("a")
                .expect("登记项")
                .output_folder,
            other
        );
    }

    #[test]
    fn update_placeholders_renames_again_on_collision() {
        let dir = TempDir::new("update-collision");
        let mut service = TaskService::new();
        let mut input = record_in(dir.path(), "a", TaskStatus::Waiting);
        input.name = "a.bin".to_string();
        service
            .add_with_conflict(
                input,
                NameConflictChoice::KeepBoth,
                false,
                None,
                never_trash,
            )
            .expect("建占位符");
        // 别人的文件占着目标名字：`updatePlaceholders` 只改名，不覆盖、不进回收站。
        let occupied = placeholder(dir.path(), "b.bin");
        std::fs::write(&occupied, b"occupied").expect("写占用文件");
        service.task_mut("a").expect("任务在").name = "b.bin".to_string();

        let report = service
            .update_placeholders("a")
            .expect("不是错误")
            .expect("有登记项");
        let renamed = placeholder(dir.path(), "b(1).bin");
        assert_eq!(report.name, "b(1).bin");
        assert_eq!(report.paths, vec![renamed.clone()]);
        assert_eq!(
            report.moved,
            vec![(placeholder(dir.path(), "a.bin"), renamed.clone())]
        );
        assert!(report.removed.is_empty());
        assert_eq!(std::fs::read(&occupied).expect("读回"), b"occupied");
        assert_eq!(service.task("a").expect("任务在").name, "b(1).bin");
        assert_eq!(service.placeholders().owner(&renamed), Some("a"));
    }
    // -----------------------------------------------------------------------
    // 第 95 步：`add` 第一步的 `outputFolderOf` 接线
    // -----------------------------------------------------------------------

    /// 预设规则 + 开启分类，下载目录 = `folder`。
    fn categories(folder: &Path) -> CategoryService {
        CategoryService::new(
            crate::category::parse_rules(None),
            crate::category::CategoryOutputSettings {
                is_category_enabled: true,
                download_folder: folder.to_string_lossy().into_owned(),
            },
        )
    }

    #[test]
    fn add_with_conflict_moves_the_task_into_the_category_folder() {
        let dir = TempDir::new("category-default");
        let mut service = TaskService::new();
        service.set_categories(categories(dir.path()));

        let mut record = record_in(dir.path(), "a", TaskStatus::Waiting);
        record.name = "movie.mkv".to_string();
        let report = service
            .add_with_conflict(
                record,
                NameConflictChoice::KeepBoth,
                true,
                Some(1024),
                never_trash,
            )
            .expect("不是错误");
        assert_eq!(report.outcome, Some(AddOutcome::Added { queued: true }));

        let task = service.task("a").expect("任务在");
        assert_eq!(task.category.as_deref(), Some("cat_video"));
        assert_eq!(
            PathBuf::from(&task.output_folder),
            dir.path().join("Video"),
            "默认下载目录 → 分类目录"
        );
        // 占位符也建在分类目录里。
        assert_eq!(
            service
                .placeholders()
                .get("a")
                .map(|entry| entry.paths.clone()),
            Some(vec![dir.path().join("Video/movie.mkv")])
        );
        assert!(dir.path().join("Video/movie.mkv").exists());
    }

    #[test]
    fn add_with_conflict_keeps_a_custom_folder_but_sets_the_category() {
        let dir = TempDir::new("category-custom");
        let custom = dir.path().join("custom");
        let mut service = TaskService::new();
        service.set_categories(categories(dir.path()));

        let mut record = record_in(&custom, "a", TaskStatus::Waiting);
        record.name = "movie.mkv".to_string();
        service
            .add_with_conflict(
                record,
                NameConflictChoice::KeepBoth,
                true,
                Some(1024),
                never_trash,
            )
            .expect("不是错误");

        let task = service.task("a").expect("任务在");
        assert_eq!(task.category.as_deref(), Some("cat_video"));
        assert_eq!(
            PathBuf::from(&task.output_folder),
            custom,
            "非默认目录不动，但分类照样定"
        );
        assert!(custom.join("movie.mkv").exists());
    }

    #[test]
    fn add_with_conflict_respects_an_explicit_category() {
        let dir = TempDir::new("category-explicit");
        let mut service = TaskService::new();
        service.set_categories(categories(dir.path()));

        let mut record = record_in(dir.path(), "a", TaskStatus::Waiting);
        record.name = "movie.mkv".to_string();
        record.category = Some("cat_audio".to_string());
        service
            .add_with_conflict(
                record,
                NameConflictChoice::KeepBoth,
                true,
                Some(1024),
                never_trash,
            )
            .expect("不是错误");

        let task = service.task("a").expect("任务在");
        assert_eq!(task.category.as_deref(), Some("cat_audio"));
        assert_eq!(PathBuf::from(&task.output_folder), dir.path().join("Audio"));
    }

    #[test]
    fn add_with_conflict_leaves_the_folder_alone_when_categories_are_disabled() {
        let dir = TempDir::new("category-disabled");
        // `TaskService::default()` 的分类服务 = 空规则 + 关闭。
        let mut service = TaskService::new();
        let mut record = record_in(dir.path(), "a", TaskStatus::Waiting);
        record.name = "movie.mkv".to_string();
        service
            .add_with_conflict(
                record,
                NameConflictChoice::KeepBoth,
                true,
                Some(1024),
                never_trash,
            )
            .expect("不是错误");

        let task = service.task("a").expect("任务在");
        assert_eq!(task.category, None, "关闭时连分类都不猜");
        assert_eq!(PathBuf::from(&task.output_folder), dir.path());
        assert!(dir.path().join("movie.mkv").exists());
    }

    // -----------------------------------------------------------------------
    // 第 96 步：`probeConflict`（重名冲突探测）
    // -----------------------------------------------------------------------

    #[test]
    fn probe_conflict_returns_none_when_nothing_exists() {
        let dir = TempDir::new("probe-empty");
        let service = TaskService::new();
        let record = record_in(dir.path(), "a", TaskStatus::Waiting);
        assert_eq!(service.probe_conflict(&record), None);
        assert_eq!(service.probe_conflict_by_id("a"), None, "任务不在列表里");
    }

    #[test]
    fn probe_conflict_reports_the_existing_file() {
        let dir = TempDir::new("probe-file");
        let service = TaskService::new();
        let record = record_in(dir.path(), "a", TaskStatus::Waiting);
        let path = dir.path().join("a.bin");
        std::fs::write(&path, b"hello").expect("写文件");

        let target = service.probe_conflict(&record).expect("有冲突");
        assert_eq!(PathBuf::from(&target.path), path);
        assert!(!target.is_folder);
        assert_eq!(target.existing_size, 5);
        assert!(target.modified_at > 0.0, "st_mtime 是正数");
    }

    #[test]
    fn probe_conflict_reports_a_folder_placeholder() {
        let dir = TempDir::new("probe-dir");
        let service = TaskService::new();
        let record = record_in(dir.path(), "a", TaskStatus::Waiting);
        let path = dir.path().join("a.bin");
        std::fs::create_dir(&path).expect("建目录占位符");

        let target = service.probe_conflict(&record).expect("有冲突");
        assert!(target.is_folder, "多文件任务的占位符是目录");
    }

    #[test]
    fn probe_conflict_skips_unfinished_placeholders() {
        let dir = TempDir::new("probe-unfinished");
        let mut service = TaskService::new();
        // `add_with_conflict` 会真的建出 0 字节占位符并登记，正好当「自己人」。
        service
            .add_with_conflict(
                record_in(dir.path(), "a", TaskStatus::Running),
                NameConflictChoice::KeepBoth,
                false,
                None,
                never_trash,
            )
            .expect("不是错误");
        let record = service.task("a").expect("任务在").clone();
        assert!(
            service.probe_conflict(&record).is_none(),
            "未完成任务的占位符不算冲突"
        );

        service.task_mut("a").expect("任务在").status = TaskStatus::Completed;
        let target = service.probe_conflict(&record).expect("完成之后才算冲突");
        assert_eq!(target.existing_size, 0, "占位符是 0 字节");
    }

    #[test]
    fn probe_conflict_looks_inside_the_category_folder() {
        let dir = TempDir::new("probe-category");
        let mut service = TaskService::new();
        service.set_categories(categories(dir.path()));
        let mut record = record_in(dir.path(), "a", TaskStatus::Waiting);
        record.name = "movie.mkv".to_string();
        std::fs::create_dir(dir.path().join("Video")).expect("建分类目录");
        let taken = dir.path().join("Video/movie.mkv");
        std::fs::write(&taken, b"x").expect("写文件");

        let target = service.probe_conflict(&record).expect("分类目录里有冲突");
        assert_eq!(PathBuf::from(&target.path), taken);
        assert_eq!(
            service.probe_conflict_in(&record, dir.path()),
            None,
            "显式给默认目录时那边没有文件"
        );
    }

    /// 可做种的任务（`BTTask.canSeed = True`，`features/bittorrent_pack/task.py:28`）。
    fn seedable(task_id: &str, status: TaskStatus) -> TaskRecord {
        let mut record = record(task_id, status, 10);
        record.type_name = Some("features.bittorrent_pack.task.BTTask".to_string());
        record
    }

    #[test]
    fn start_seeding_marks_should_seed_and_registers_the_work() {
        // `from_records` 而不是 `add`：Python 的 `add` 会强制 `shouldSeed = True`，
        // 那样就测不出 `startSeeding` 自己那一句。
        let mut input = seedable("a", TaskStatus::Completed);
        input.should_seed = false;
        let mut service = TaskService::from_records(vec![input]);
        assert!(!service.is_seeding("a"));

        assert_eq!(
            service.start_seeding("a", "w1"),
            Some(SeedingEvent::Started)
        );
        assert!(service.task("a").expect("任务在").should_seed);
        assert!(service.pending_flush(), "shouldSeed 是持久化字段");
        assert!(service.is_seeding("a"), "`_seed` 里置 isSeeding = True");
        assert_eq!(service.seeding_work_id("a"), Some("w1"));
        assert_eq!(service.seeding_count(), 1);
        assert!(service.speed_meter_should_run(), "做种中速度表要跑");

        // 任务不在列表里 → `None`，且不登记。
        assert_eq!(service.start_seeding("nope", "w2"), None);
        assert_eq!(service.seeding_count(), 1);
    }

    #[test]
    fn seed_requires_the_task_to_exist() {
        let mut service = TaskService::new();
        assert_eq!(service.seed("a", "w1"), None);
        assert_eq!(service.seeding_count(), 0);
        assert!(!service.is_seeding("a"));

        // 任务在 → 直接登记：`_seed` 自己不看 canSeed / shouldSeed。
        let mut service = TaskService::from_records(vec![record("a", TaskStatus::Completed, 10)]);
        assert_eq!(service.seed("a", "w1"), Some(SeedingEvent::Started));
        assert!(service.is_seeding("a"));
        assert!(!service.task("a").expect("任务在").should_seed);
    }

    #[test]
    fn stop_seeding_clears_should_seed_and_returns_the_work() {
        let mut input = seedable("a", TaskStatus::Completed);
        input.should_seed = true;
        let mut service = TaskService::from_records(vec![input]);
        service.seed("a", "w1");
        assert!(!service.pending_flush(), "`_seed` 不落盘");

        let cancelled = service.stop_seeding("a");
        assert!(!service.task("a").expect("任务在").should_seed);
        assert!(service.pending_flush(), "`shouldSeed` 改了要 `_flushSoon`");
        // 队列里没有 workId → `_seeding.pop` 兜底。
        assert_eq!(cancelled.work_id.as_deref(), Some("w1"));
        assert!(cancelled.stopped_seeding);
        assert!(!service.is_seeding("a"));
        assert_eq!(service.seeding_count(), 0);
        assert!(!service.speed_meter_should_run());
    }

    #[test]
    fn on_seeding_ended_flushes_and_clears() {
        let mut input = seedable("a", TaskStatus::Completed);
        input.should_seed = true;
        let mut service = TaskService::from_records(vec![input]);
        service.seed("a", "w1");
        assert!(!service.pending_flush());
        assert_eq!(service.on_seeding_ended("a"), SeedingEvent::Stopped);
        assert!(!service.is_seeding("a"));
        assert!(service.pending_flush(), "`_onSeedingEnded` 里 `_flushSoon`");
        // 再调一次也不炸（Python 的 `pop(taskId, None)` 带缺省）。
        assert_eq!(service.on_seeding_ended("a"), SeedingEvent::Stopped);

        // `_onSeedingDone` 先清 `shouldSeed` 再走 `_onSeedingEnded`。
        let mut input = seedable("b", TaskStatus::Completed);
        input.should_seed = true;
        let mut service = TaskService::from_records(vec![input]);
        service.seed("b", "w2");
        assert_eq!(service.on_seeding_done("b"), SeedingEvent::Stopped);
        assert!(!service.task("b").expect("任务在").should_seed);
        assert!(!service.is_seeding("b"));
        assert!(service.pending_flush());
    }

    #[test]
    fn cancel_work_prefers_the_queue_work_id() {
        let mut input = seedable("a", TaskStatus::Completed);
        input.should_seed = true;
        let mut service = TaskService::from_records(vec![input]);
        service.seed("a", "seed-1");
        // 极端情形：同一个任务既挂着队列 workId、又在做种。
        assert!(service.start("a"));
        service.on_run_started("a", "run-1");

        let cancelled = service.cancel_work("a");
        assert_eq!(
            cancelled.work_id.as_deref(),
            Some("run-1"),
            "`queue.workIdOf(id) or …` 里队列优先"
        );
        assert!(cancelled.stopped_seeding, "入口处 isSeeding 为真");
        assert_eq!(service.running_count(), 0);

        // Python 的 `or` 短路：`_seeding.pop` 没执行，那条留在字典里
        // （`isSeeding` 已经清了）——既有缺陷，Rust 照抄。
        assert!(service.is_seeding("a"));
        assert_eq!(service.seeding_count(), 1);
        assert!(
            service.speed_meter_should_run(),
            "残留项让速度表一直跑，与 Python 的 `_refreshSpeedMeter` 同病"
        );
    }

    #[test]
    fn speed_meter_runs_while_seeding() {
        let mut service = TaskService::from_records(vec![
            seedable("a", TaskStatus::Completed),
            record("b", TaskStatus::Waiting, 10),
        ]);
        assert!(!service.speed_meter_should_run());

        // 队列在跑 → 跑。派发走 `pump`：`queue.run` 本身不出等待队列。
        assert!(service.start("b"));
        assert_eq!(service.pump(8), vec!["b".to_string()]);
        assert!(service.speed_meter_should_run());
        let outcome = service.on_run_finished("b", 8);
        assert_eq!(outcome.started, Vec::<String>::new());
        assert!(!service.speed_meter_should_run(), "队列空了、也没做种");

        // 只有做种 → 也跑。
        service.seed("a", "w2");
        assert!(service.speed_meter_should_run());
        service.on_seeding_ended("a");
        assert!(!service.speed_meter_should_run());
    }

    #[test]
    fn should_start_seeding_requires_completed_and_seedable_type() {
        let mut service = TaskService::from_records(vec![
            seedable("bt", TaskStatus::Completed),
            seedable("bt-waiting", TaskStatus::Waiting),
            record("plain", TaskStatus::Completed, 10),
        ]);
        // `from_records` 不强制 `shouldSeed`，与 `add` 不同。
        assert!(!service.should_start_seeding("bt"));

        service.task_mut("bt").expect("任务在").should_seed = true;
        assert!(service.should_start_seeding("bt"));

        // 状态不是 COMPLETED。
        service.task_mut("bt-waiting").expect("任务在").should_seed = true;
        assert!(!service.should_start_seeding("bt-waiting"));

        // 类型不可做种（`type_name` 为 None → `canSeed = False`）。
        service.task_mut("plain").expect("任务在").should_seed = true;
        assert!(!service.should_start_seeding("plain"));

        assert!(!service.should_start_seeding("nope"));
    }

    #[test]
    fn resume_seeding_candidates_needs_an_existing_output_file() {
        let dir = TempDir::new("resume-seed");
        // 1. 该做种：已完成 + 可做种 + shouldSeed + 文件在。
        let mut present = record_in(dir.path(), "present", TaskStatus::Completed);
        present.type_name = Some("features.bittorrent_pack.task.BTTask".to_string());
        present.should_seed = true;
        std::fs::write(dir.path().join("present.bin"), b"data").expect("写文件");
        // 2. 文件不在 → `isExisting` 为假。
        let mut missing = record_in(dir.path(), "missing", TaskStatus::Completed);
        missing.type_name = Some("features.bittorrent_pack.task.BTTask".to_string());
        missing.should_seed = true;
        // 3. 文件在但没开 `shouldSeed`。
        let mut no_seed = record_in(dir.path(), "no-seed", TaskStatus::Completed);
        no_seed.type_name = Some("features.bittorrent_pack.task.BTTask".to_string());
        std::fs::write(dir.path().join("no-seed.bin"), b"data").expect("写文件");
        // 4. 文件在、shouldSeed 也真，但类型不可做种。
        let mut plain = record_in(dir.path(), "plain", TaskStatus::Completed);
        plain.should_seed = true;
        std::fs::write(dir.path().join("plain.bin"), b"data").expect("写文件");

        let service = TaskService::from_records(vec![present, missing, no_seed, plain]);
        assert_eq!(
            service.resume_seeding_candidates(),
            vec!["present".to_string()]
        );
    }

    #[test]
    fn refresh_reflects_the_seeding_state() {
        use slint::Model as _;

        let window = crate::create_headless();
        let mut service = TaskService::from_records(vec![seedable("bt", TaskStatus::Completed)]);

        service.refresh(&window);
        let row = window.get_tasks().row_data(0).expect("首行缺失");
        assert_eq!(row.toggleIcon, "PLAY", "COMPLETED 未做种 → 沿用 PLAY");
        assert!(row.toggleEnabled, "BTTask.canSeed 为真");

        service.seed("bt", "w1");
        service.refresh(&window);
        let row = window.get_tasks().row_data(0).expect("首行缺失");
        assert_eq!(row.toggleIcon, "PAUSE", "做种中 → PAUSE");
    }

    #[test]
    fn start_checksum_registers_the_work() {
        let mut service = TaskService::from_records(vec![record("t", TaskStatus::Completed, 10)]);
        assert_eq!(
            service.start_checksum("t", "w1"),
            Some(ChecksumEvent::Started)
        );
        assert_eq!(service.checksum_progress("t"), Some(0));
        assert_eq!(
            service.checksum_work("t").map(|work| work.work_id.as_str()),
            Some("w1")
        );
        assert_eq!(service.checksum_count(), 1);
        assert_eq!(service.checksum_progress("nope"), None);

        // 任务不在列表里 → 不登记（Python 走不到这条）。
        assert_eq!(service.start_checksum("nope", "w2"), None);
        assert_eq!(service.checksum_count(), 1);
    }

    #[test]
    fn set_checksum_progress_updates_the_registered_work() {
        let mut service = TaskService::from_records(vec![record("t", TaskStatus::Completed, 10)]);
        service.start_checksum("t", "w1");
        assert!(service.set_checksum_progress("t", 42));
        assert_eq!(service.checksum_progress("t"), Some(42));

        // 取消之后迟到的进度写不进去（Python 会写进孤儿对象，看不见）。
        service.cancel_checksum("t");
        assert!(!service.set_checksum_progress("t", 99));
        assert_eq!(service.checksum_progress("t"), None);
    }

    #[test]
    fn cancel_checksum_returns_the_work_id_once() {
        let mut service = TaskService::from_records(vec![record("t", TaskStatus::Completed, 10)]);
        service.start_checksum("t", "w1");
        assert_eq!(service.cancel_checksum("t"), Some("w1".to_string()));
        assert_eq!(service.checksum_count(), 0);
        // 第二次：Python `pop(..., None)` → `return`，不发信号也不取消协程。
        assert_eq!(service.cancel_checksum("t"), None);
        assert_eq!(service.cancel_checksum("nope"), None);
    }

    #[test]
    fn on_checksum_done_records_the_digest_and_flushes() {
        let mut service = TaskService::from_records(vec![record("t", TaskStatus::Completed, 10)]);
        service.start_checksum("t", "w1");
        assert!(!service.pending_flush(), "startChecksum 本身不落盘");
        assert_eq!(
            service.on_checksum_done("t", "sha256", "abc123"),
            Some(ChecksumEvent::Completed {
                algorithm: "sha256".to_string()
            })
        );
        assert_eq!(service.checksum_count(), 0);
        assert!(service.pending_flush(), "`checksums` 是持久化字段");
        let record = service.task("t").expect("任务在");
        assert_eq!(
            record.checksums.get("sha256"),
            Some(&Value::String("abc123".to_string()))
        );
    }

    #[test]
    fn on_checksum_done_is_idempotent_and_needs_the_task() {
        let mut service = TaskService::from_records(vec![record("t", TaskStatus::Completed, 10)]);
        service.start_checksum("t", "w1");
        assert!(service.on_checksum_done("t", "md5", "aaa").is_some());
        // Python 的 `del` 在这里抛 KeyError；Rust 幂等。
        assert_eq!(service.on_checksum_done("t", "md5", "bbb"), None);
        // 任务不在列表里 → 不写（Python 会 AttributeError）。
        assert_eq!(service.on_checksum_done("nope", "md5", "ccc"), None);
    }

    #[test]
    fn on_checksum_failed_reports_the_task_error() {
        let mut service = TaskService::from_records(vec![record("t", TaskStatus::Completed, 10)]);
        service.start_checksum("t", "w1");
        assert_eq!(
            service.on_checksum_failed("t", "boom"),
            ChecksumEvent::Stopped {
                error: Some(TaskError::new(
                    "无法读取文件：{detail}",
                    vec![("detail", "boom".to_string())]
                ))
            }
        );
        assert_eq!(service.checksum_count(), 0);
        // 失败不落盘、不写 `checksums`。
        assert!(!service.pending_flush());
        assert!(service.task("t").expect("任务在").checksums.is_empty());
        // 任务不在列表里也照样发信号（Python 拿的是活对象）。
        assert!(matches!(
            service.on_checksum_failed("nope", "gone"),
            ChecksumEvent::Stopped { error: Some(_) }
        ));
    }

    #[test]
    fn rebalance_cancels_tasks_beyond_the_limit() {
        let mut service = TaskService::from_records(vec![
            record("a", TaskStatus::Running, 10),
            record("b", TaskStatus::Running, 10),
            record("c", TaskStatus::Running, 10),
        ]);
        service.on_run_started("a", "wa");
        service.on_run_started("b", "wb");
        service.on_run_started("c", "wc");

        let report = service.rebalance(2);
        assert_eq!(
            report.cancelled,
            vec![(
                "c".to_string(),
                CancelWork {
                    work_id: Some("wc".to_string()),
                    stopped_seeding: false,
                }
            )]
        );
        assert_eq!(report.requeued, vec!["c".to_string()]);
        assert!(report.started.is_empty(), "运行数已到上限，不派发");
        assert_eq!(
            service.task("c").expect("任务在").status,
            TaskStatus::Waiting
        );
        assert_eq!(service.waiting_order(), vec!["c".to_string()]);
        assert_eq!(service.running_count(), 2);
    }

    #[test]
    fn rebalance_skips_tasks_that_cannot_pause() {
        let mut blocked = record("b", TaskStatus::Running, 10);
        blocked.steps.push(TaskStep {
            step_index: 0,
            file_index: None,
            step_id: "step".to_string(),
            status: TaskStatus::Running,
            progress: Progress::Int(0),
            received_bytes: 0,
            speed: 0,
            extra: Map::new(),
            type_name: Some("FFmpegStep".to_string()),
        });
        let mut service =
            TaskService::from_records(vec![record("a", TaskStatus::Running, 10), blocked]);
        service.on_run_started("a", "wa");
        service.on_run_started("b", "wb");

        let report = service.rebalance(1);
        assert!(report.cancelled.is_empty(), "canPause 为假 → 不动");
        assert!(report.requeued.is_empty());
        assert_eq!(service.running_count(), 2);
        assert_eq!(
            service.task("b").expect("任务在").status,
            TaskStatus::Running
        );
    }

    #[test]
    fn rebalance_pumps_after_the_limit_grows() {
        let mut service = TaskService::from_records(vec![
            record("a", TaskStatus::Running, 10),
            record("b", TaskStatus::Waiting, 10),
        ]);
        service.on_run_started("a", "wa");
        assert!(service.start("b"), "b 进等待队列");

        let report = service.rebalance(2);
        assert!(report.cancelled.is_empty(), "运行数没超限");
        assert_eq!(report.started, vec!["b".to_string()]);
        assert_eq!(service.running_count(), 2);
        assert!(service.waiting_order().is_empty());
    }
    #[test]
    fn pause_pumps_the_freed_slot() {
        let mut service = TaskService::new();
        service.add(record("a", TaskStatus::Running, 10), true);
        service.add(record("b", TaskStatus::Waiting, 10), false);
        service.on_run_started("a", "work-1");
        service.queue.wait("b");

        let report = service.pause("a", 1).expect("任务在");
        assert_eq!(report.cancel.work_id.as_deref(), Some("work-1"));
        assert_eq!(report.started, vec!["b".to_string()], "让出空位立刻补位");
        assert_eq!(service.running_count(), 1);
        assert!(service.waiting_order().is_empty());
    }

    #[test]
    fn delete_reports_checksum_cancel_and_pumps() {
        let mut service = TaskService::new();
        service.add(record("a", TaskStatus::Running, 10), true);
        service.add(record("b", TaskStatus::Waiting, 10), false);
        service.on_run_started("a", "work-1");
        service.start_checksum("a", "check-1").expect("任务在");
        service.queue.wait("b");

        let report = service.delete("a", false, 1).expect("任务在");
        assert_eq!(report.checksum_work.as_deref(), Some("check-1"));
        assert_eq!(report.cancel.work_id.as_deref(), Some("work-1"));
        assert_eq!(report.removed.task_id, "a");
        assert!(!report.delete_files, "shouldDeleteFiles=False");
        assert!(!report.delete_denied, "非 macOS 恒可删");
        assert_eq!(report.started, vec!["b".to_string()]);
        assert_eq!(service.len(), 1);
        assert_eq!(service.checksum_count(), 0, "校验登记项要摘掉");
        assert_eq!(service.running_count(), 1);
        assert!(service.pending_flush());
        assert!(service.delete("missing", false, 1).is_none());
    }

    #[test]
    fn delete_removes_placeholders_but_keeps_content() {
        let dir = TempDir::new("delete");
        let mut service = TaskService::new();
        service.add(record_in(dir.path(), "a", TaskStatus::Completed), false);
        service.add(record_in(dir.path(), "b", TaskStatus::Completed), false);
        let empty = placeholder(dir.path(), "a.bin");
        std::fs::write(&empty, b"").expect("写占位符");
        std::fs::write(sidecar_path(&empty), b"{}").expect("写 sidecar");
        let content = placeholder(dir.path(), "b.bin");
        std::fs::write(&content, b"payload").expect("写内容");

        let report = service.delete("a", true, 1).expect("任务在");
        assert!(report.delete_files, "非 macOS 恒可删");
        assert!(!report.delete_denied);
        assert!(!empty.exists(), "零字节占位符要删");
        assert!(!sidecar_path(&empty).exists(), "sidecar 也要删");

        service.delete("b", true, 1).expect("任务在");
        assert!(content.exists(), "有内容的文件是真实数据，不能删");
    }

    #[test]
    fn redownload_resets_and_requeues() {
        let mut service = TaskService::new();
        let mut done = record("a", TaskStatus::Completed, 10);
        done.completed_at = 1234;
        done.should_seed = false;
        done.checksums
            .insert("sha256".to_string(), serde_json::json!("abc"));
        done.steps.push(step(10));
        service.add(done, false);
        service.start_checksum("a", "check-1").expect("任务在");

        let report = service.redownload("a", 1).expect("任务在");
        assert_eq!(report.checksum_work.as_deref(), Some("check-1"));
        assert!(report.cancel.work_id.is_none(), "不在队列里");
        assert_eq!(report.status, TaskStatus::Waiting);
        assert_eq!(report.started, vec!["a".to_string()]);
        assert_eq!(service.checksum_count(), 0);
        assert!(service.pending_flush());
        assert_eq!(service.running_count(), 1);
        let record = service.task("a").expect("任务在");
        assert_eq!(record.completed_at, 0);
        assert!(record.should_seed);
        assert!(record.checksums.is_empty());
        assert_eq!(record.steps[0].status, TaskStatus::Waiting);
        assert_eq!(record.steps[0].received_bytes, 0);
        assert!(service.redownload("missing", 1).is_none());
    }

    #[test]
    fn set_category_writes_the_raw_string_and_flushes() {
        let dir = TempDir::new("category");
        let path = dir.path().join("tasks.jsonl");
        let (mut service, failures) = TaskService::load(&path).expect("缺失文件不算错误");
        assert!(failures.is_empty());
        service.add(record("a", TaskStatus::Completed, 10), false);
        assert!(service.flush(&path).expect("写盘"));
        assert!(!service.pending_flush());

        assert!(service.set_category("a", ""));
        assert_eq!(
            service.task("a").expect("任务在").category.as_deref(),
            Some(""),
            "「未分类」传空串，落成 Some(\"\") 而非 None"
        );
        assert!(service.pending_flush());

        assert!(service.set_category("a", "video"));
        assert_eq!(
            service.task("a").expect("任务在").category.as_deref(),
            Some("video")
        );
        assert!(!service.set_category("missing", "video"));
    }

    #[test]
    fn start_all_queues_paused_waiting_and_failed() {
        let mut service = TaskService::from_records(vec![
            record("p", TaskStatus::Paused, 10),
            record("w", TaskStatus::Waiting, 10),
            record("f", TaskStatus::Failed, 10),
            record("d", TaskStatus::Completed, 10),
        ]);

        assert_eq!(service.start_all(1), vec!["p".to_string()]);
        assert_eq!(
            service.waiting_order(),
            vec!["w".to_string(), "f".to_string()],
            "只排 PAUSED/WAITING/FAILED，保持记录顺序"
        );
        assert_eq!(
            service.task("w").expect("任务在").status,
            TaskStatus::Waiting,
            "排队期间不改状态（_dispatch 才 setStatus(RUNNING)）"
        );
        assert_eq!(
            service.task("d").expect("任务在").status,
            TaskStatus::Completed
        );
        assert_eq!(service.running_count(), 1);
    }

    #[test]
    fn pause_all_pauses_running_and_waiting() {
        let mut service = TaskService::new();
        service.add(record("a", TaskStatus::Running, 10), true);
        service.add(record("b", TaskStatus::Waiting, 10), false);
        service.add(record("c", TaskStatus::Completed, 10), false);
        service.on_run_started("a", "wa");
        service.queue.wait("b");

        let started = service.pause_all(1);
        assert_eq!(
            started,
            vec!["b".to_string()],
            "pause(a) 的 pump 先把 b 派发出去，随后 b 自己也被暂停"
        );
        assert_eq!(
            service.task("a").expect("任务在").status,
            TaskStatus::Paused
        );
        assert_eq!(
            service.task("b").expect("任务在").status,
            TaskStatus::Paused
        );
        assert_eq!(
            service.task("c").expect("任务在").status,
            TaskStatus::Completed,
            "已完成的不动"
        );
        assert_eq!(service.running_count(), 0);
        assert!(service.waiting_order().is_empty());
    }
}
