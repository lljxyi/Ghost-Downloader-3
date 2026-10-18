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
//! 为什么不做成 `actions::TaskService` 的 `impl`：那个 trait 的方法（`pause`/
//! `start`/`delete`/`redownload`/`startChecksum`）每一个都要先「取消协程 / 停做种 /
//! 删文件」，没有引擎就只能假装。等 `app/engine.py` 迁过来再实现那个 trait，
//! 这里先把不依赖引擎的记账与判定做扎实。
//!
//! # 已知差异
//!
//! - `add()` 不做**重名冲突解决**（`TaskStore.add` 的 `FileExistsError` 重试循环 +
//!   `deleteRecoverably` + `nameConflicted`/`overwriteFailed` 两个信号）；
//! - **不自己算磁盘剩余空间**：`disk_usage(task.outputFolder)` 是文件系统事实，
//!   由调用方算好后走 `add_with_free_space(..., free_space)`；`free_space == None`
//!   等价于 Python 的 `except OSError: pass`（跳过检查），`add()` 是它的简写；
//! - 不建输出目录、不建占位文件（Python 的 `TaskStore.add` 会 `mkdir` + 建 `.ghd`）；
//! - `pump()` 派发时用**空 workId** 占位（Python 在 `_dispatch` 里拿到协程 id 才
//!   `queue.run(taskId, workId)`）：这里先 `run(id, "")` 把 `runningCount` 顶上去，
//!   否则 `pump` 的循环条件与 `tasksAllCompleted` 的判定都不成立；引擎起来后由
//!   `on_run_started(id, work_id)` 回填真实 workId；
//! - 不做 `_rebalance`（依赖 `Task.canPause` 这个运行时事实）、不做做种/校验/
//!   文件监听/速度表（`_seed`/`_onSeeding*`/`startChecksum`/`_watchFile`/
//!   `_refreshSpeedMeter` 全部留给引擎层）。

use std::io;
use std::path::Path;

use gd3_core::queue::TaskQueue;
use gd3_core::store;
use gd3_core::task::{TaskRecord, TaskStatus};

use crate::AppWindow;
use crate::card::CardRuntime;

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

/// `_onRunDone` / `_onRunFailed` 末尾那一段的记账结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOutcome {
    /// `_pump()` 决定派发的任务 id，按 `nextWaiting()` 顺序。
    pub started: Vec<String>,
    /// 派发**之后** `runningCount() == 0` → 调用方发 `tasksAllCompleted`。
    pub all_completed: bool,
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
}

impl TaskService {
    /// 空服务、未 load（`flush()` 不会写盘）。
    pub fn new() -> Self {
        Self::default()
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
        if !auto_start {
            return AddOutcome::Added { queued: false };
        }
        if file_size > 0
            && let Some(free) = free_space
            && free < file_size
        {
            return AddOutcome::DiskSpaceInsufficient { free };
        }
        self.queue.wait(&task_id);
        AddOutcome::Added { queued: true }
    }

    /// `delete(task, ...)` 的记账部分：`_cancelWork` → 从列表移除 → 待落盘。
    ///
    /// 文件删除/占位符清理由调用方做（`Task.deleteFiles` + `deletePlaceholders`），
    /// 「没有权限删」的提示走 `note_delete_denied()`。
    pub fn remove(&mut self, task_id: &str) -> Option<TaskRecord> {
        self.cancel_work(task_id);
        let index = self
            .records
            .iter()
            .position(|record| record.task_id == task_id)?;
        self.pending_flush = true;
        Some(self.records.remove(index))
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

    /// `pause(task)` 的记账部分：`_cancelWork` + `setStatus(PAUSED)` + 待落盘。
    ///
    /// 返回任务是否存在（Python 对不存在的任务会 `AttributeError`，这里退化）。
    pub fn pause(&mut self, task_id: &str) -> bool {
        if self.task(task_id).is_none() {
            return false;
        }
        self.cancel_work(task_id);
        if let Some(record) = self.task_mut(task_id) {
            record.status = TaskStatus::Paused;
        }
        self.pending_flush = true;
        true
    }

    /// `_cancelWork(task)` 的队列部分（协程取消/停做种归引擎）。
    pub fn cancel_work(&mut self, task_id: &str) {
        self.queue.cancel(task_id);
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

    /// `resumeSaved()`：启动时把持久化里的 `WAITING`/`RUNNING` 重新排进队列。
    ///
    /// Python 的 `resumeSaved()` 还做两件引擎侧的事：已完成任务的
    /// `_watchFile`/`_seed`，以及每个任务发 `taskAdded`。这里只做
    /// 「`RUNNING` → `WAITING` + `queue.wait`」，返回重新入队的 id。
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

    /// 把当前记录推成 UI 行（运行时事实走 `CardRuntime::default()`）。
    ///
    /// `isSeeding`/`outputExists`/`lastError` 这些记录里没有的事实，
    /// 等引擎迁过来后由调用方换成真实取值。
    pub fn refresh(&self, window: &AppWindow) {
        crate::push_task_records(window, &self.records, |_| CardRuntime::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(service.pause("a"));
        assert_eq!(service.running_count(), 0);
        assert_eq!(
            service.task("a").expect("任务在").status,
            TaskStatus::Paused
        );
        assert!(service.pending_flush());
        assert!(!service.pause("missing"));
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
}
