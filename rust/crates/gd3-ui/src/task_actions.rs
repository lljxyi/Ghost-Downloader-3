//! 卡片/抽屉动作 → 真 [`TaskService`] 的接线（第 100 步）。
//!
//! [`crate::actions`] 只算「点一下该调哪个服务方法」，[`crate::task_service`]
//! 只做记账半边并把「要取消的协程 / 要删的文件 / 该派发的 id」摊成报告。
//! 这一层把两者接起来，判定标准是 `app/services/task_service.py` 的**调用序列**
//! 与**信号发射顺序**：
//!
//! ```text
//! pause(task)        _cancelWork → setStatus(PAUSED) → _flushSoon → taskPaused → _pump
//! start(task)        (guard) → _schedule = queue.wait + _pump
//! startSeeding(task) shouldSeed=True → _flushSoon → _seed(isManual=True)
//! stopSeeding(task)  shouldSeed=False → _flushSoon → _cancelWork → _refreshSpeedMeter
//! delete(task, df)   cancelChecksum → _unwatchFile → [fileDeleteDenied] → _cancelWork
//!                    → onStopped{deleteFiles, deletePlaceholders} → _store.remove
//!                    → _flushSoon → taskRemoved → _pump
//! redownload(task)   cancelChecksum → _unwatchFile → _cancelWork
//!                    → onStopped{deleteFiles, reset, _flushSoon, _schedule}
//! setCategory(t, c)  task.category = c → _flushSoon
//! moveToFront(ids)   if queue.moveToFront(ids): queueChanged
//! startChecksum(t,a) _checksums[t]=work → submit(toChecksum(...)) → checksumStarted
//! cancelChecksum(t)  if work: cancel(workId) → checksumStopped(t, None)
//! ```
//!
//! # 两个缺口与它们的补法
//!
//! 1. **`&self` vs `&mut self`**：`actions::TaskService` 的方法收 `&self`（Slint
//!    回调是单槽 `Fn`，`TaskCommands` 用 `Rc<dyn actions::TaskService>` 持有），
//!    而 [`TaskService`] 的方法全要 `&mut self`。这里用 [`TaskStore`] 端口把
//!    「怎么拿到 `&mut TaskService`」抽象掉：`RefCell<TaskService>`、
//!    `RefCell<&mut TaskService>`（借引擎那份）与 `Rc<RefCell<TaskService>>`
//!    （UI 长期持有）三种句柄各一个 `impl`；
//! 2. **没有协程运行时**：Python 每个动作都会 `coroutineRunner.submit/cancel`。
//!    Rust 侧退化成[`TaskAction`] 效果队列——[`TaskActions`] 只**登记**要做什么
//!    （提交哪个 workId、取消哪个 workId、删哪个任务的文件），由宿主按序消费。
//!    workId 由 [`TaskActions`] 自己发号（`run-1` / `seed-2` / `checksum-3`，一个
//!    全局计数器 + 前缀区分类别），宿主只需把它当**不透明句柄**原样交回
//!    [`TaskAction::CancelWork`]。
//!
//! # 顺序
//!
//! 效果队列的顺序就是 Python 的**发射顺序**（`taskPaused` 在 `_pump` 之前、
//! `taskRemoved` 在 `taskStarted` 之前……）。唯一的语义差别是 `_cancelWork` 的
//! `finished=` 回调：Python 等协程真的停下才跑 `onStopped`，这里把
//! [`TaskAction::CancelWork`] 排在 [`TaskAction::DeleteFiles`] 之前，**宿主必须
//! 按序应用**（取消完成后再删文件），与第 99 步 `DeleteReport::delete_files`
//! 的约定一致。
//!
//! # 还没接的
//!
//! `actions::UiHooks`（剪贴板 / 抽屉 / 编辑对话框 / 打开文件）不在本模块：
//! 它们落到 `app.platform.desktop`，要等 `gd3-core::desktop` 的 GUI 调用方
//! （Slint 窗口）就位。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::actions;
use crate::task_service::{CancelWork, TaskService};
use gd3_core::task::TaskStatus;

/// `cfg.maxTaskNum.value` 的现读形态（Python 每次 `_pump` 都重新取）。
pub type MaxTaskNum = Rc<dyn Fn() -> usize>;

/// 动作层产生的**副作用**：宿主（引擎 / 外壳）按序消费。
///
/// 每个变体都对应 Python 里的一行具体调用或一次信号发射，注释里给出出处。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskAction {
    /// `_dispatch(task)`：提交 `task.run(...)` 协程 + `taskStarted.emit(task)`。
    ///
    /// 记录状态与队列的 workId 已经在 [`TaskService::dispatch`] 里落好了。
    Started { task_id: String, work_id: String },
    /// `_cancelWork(task)` 里的 `coroutineRunner.cancel(workId)`。
    CancelWork { task_id: String, work_id: String },
    /// `taskPaused.emit(task)`。
    Paused { task_id: String },
    /// `taskRemoved.emit(task.taskId)`。
    Removed { task_id: String },
    /// `fileDeleteDenied.emit()`（`_hasNotifiedDeleteDenied`：整个进程只发一次）。
    DeleteDenied,
    /// `Task.deleteFiles()`：目录清空 / 文件截断 + 删 `<path>.ghd`。
    ///
    /// 排在 [`TaskAction::CancelWork`] 之后——Python 在 `finished` 回调里做。
    DeleteFiles { task_id: String },
    /// `_unwatchFile(task)`：摘掉文件监听（`QFileSystemWatcher`）。
    UnwatchFile { task_id: String },
    /// `_seed(task, isManual)`：提交 `task.runSeeding(isManual)` + `_refreshSpeedMeter()`。
    SeedingStarted {
        task_id: String,
        work_id: String,
        is_manual: bool,
    },
    /// `seedingStopped.emit(task)`。
    SeedingStopped { task_id: String },
    /// `startChecksum`：提交 `toChecksum(path, algorithm, onProgress)` +
    /// `checksumStarted.emit(task)`。
    ChecksumStarted {
        task_id: String,
        work_id: String,
        algorithm: String,
    },
    /// `cancelChecksum`：`coroutineRunner.cancel(workId)` + `checksumStopped.emit(task, None)`。
    ChecksumStopped { task_id: String, work_id: String },
    /// `queueChanged.emit()`（只在 `moveToFront` 真的动了队列时发）。
    QueueChanged,
}

/// 动作层看到的 `TaskService` 句柄。
///
/// 方法收 `&self` 但给的是 `&mut TaskService`：内部一定是 `RefCell` 一类的东西。
/// 用 trait 而不是直接收 `Rc<RefCell<TaskService>>`，是为了让引擎能在**不改变
/// 所有权结构**的前提下临时借出自己那份（`RefCell<&mut TaskService>`）。
pub trait TaskStore {
    fn with_tasks<R>(&self, f: impl FnOnce(&mut TaskService) -> R) -> R;
}

impl TaskStore for RefCell<TaskService> {
    fn with_tasks<R>(&self, f: impl FnOnce(&mut TaskService) -> R) -> R {
        f(&mut self.borrow_mut())
    }
}

impl TaskStore for RefCell<&mut TaskService> {
    fn with_tasks<R>(&self, f: impl FnOnce(&mut TaskService) -> R) -> R {
        f(&mut self.borrow_mut())
    }
}

impl TaskStore for Rc<RefCell<TaskService>> {
    fn with_tasks<R>(&self, f: impl FnOnce(&mut TaskService) -> R) -> R {
        f(&mut self.borrow_mut())
    }
}

/// [`actions::TaskService`] 的真实现：把动作落到 [`TaskService`] 并把副作用
/// 摊成 [`TaskAction`]。
///
/// ```text
/// let mut engine = Engine::new(&cfg, path);
/// let actions = engine.task_actions();          // 借出 engine.tasks
/// actions::apply_toggle(&actions, "t1", status, is_seeding);
/// for action in actions.take_effects() { … }    // 宿主按序应用
/// ```
pub struct TaskActions<S: TaskStore> {
    store: S,
    max_task_num: MaxTaskNum,
    next_work_id: Cell<u64>,
    effects: RefCell<Vec<TaskAction>>,
}

impl<S: TaskStore> TaskActions<S> {
    /// `max_task_num` 走闭包：Python 的 `_pump` 每次都读 `cfg.maxTaskNum.value`。
    pub fn new(store: S, max_task_num: MaxTaskNum) -> Self {
        Self {
            store,
            max_task_num,
            next_work_id: Cell::new(0),
            effects: RefCell::new(Vec::new()),
        }
    }

    /// 固定上限的简写（引擎临时借出时用：一次动作内配置不会变，等价于现读）。
    pub fn with_max(store: S, max_task_num: usize) -> Self {
        Self::new(store, Rc::new(move || max_task_num))
    }

    /// 底下的句柄（测试与调用方回查状态用）。
    pub fn store(&self) -> &S {
        &self.store
    }

    /// 已登记但还没消费的副作用（拷贝一份，不清空）。
    pub fn effects(&self) -> Vec<TaskAction> {
        self.effects.borrow().clone()
    }

    /// 取走已登记的副作用（宿主消费点）。
    pub fn take_effects(&self) -> Vec<TaskAction> {
        std::mem::take(&mut self.effects.borrow_mut())
    }

    fn max_task_num(&self) -> usize {
        (self.max_task_num)()
    }

    fn emit(&self, action: TaskAction) {
        self.effects.borrow_mut().push(action);
    }

    /// `coroutineRunner.submit(...)` 的返回值：`<类>-<序号>`。
    ///
    /// 一个全局计数器（`run-1` / `seed-2` / `checksum-3`）：Python 那边是运行时
    /// 发号、全局唯一，前缀只是给日志看的；本层保证**同一层内不重复**即可。
    fn next_work_id(&self, kind: &str) -> String {
        let seq = self.next_work_id.get() + 1;
        self.next_work_id.set(seq);
        format!("{kind}-{seq}")
    }

    /// `_cancelWork(task)` 的两个信号：`seedingStopped` 先、`cancel(workId)` 后。
    fn emit_cancel(&self, task_id: &str, work: &CancelWork) {
        if work.stopped_seeding {
            self.emit(TaskAction::SeedingStopped {
                task_id: task_id.to_string(),
            });
        }
        if let Some(work_id) = &work.work_id {
            self.emit(TaskAction::CancelWork {
                task_id: task_id.to_string(),
                work_id: work_id.clone(),
            });
        }
    }

    /// `_pump()` 的 `_dispatch` 那一半：给每个该派发的 id 发号 + 置 `RUNNING` +
    /// `queue.run(id, workId)` + 登记 [`TaskAction::Started`]。
    fn dispatch(&self, started: &[String]) {
        for task_id in started {
            let work_id = self.next_work_id("run");
            let dispatched = self
                .store
                .with_tasks(|tasks| tasks.dispatch(task_id, &work_id));
            if dispatched {
                self.emit(TaskAction::Started {
                    task_id: task_id.clone(),
                    work_id,
                });
            }
        }
    }

    /// `_pump()`：派发到上限为止（返回被派发的 id，已登记 `Started`）。
    pub fn pump(&self) -> Vec<String> {
        let max = self.max_task_num();
        let started = self.store.with_tasks(|tasks| tasks.pump(max));
        self.dispatch(&started);
        started
    }

    /// `startAll()`（`task_service.py:530-534`）。
    pub fn start_all(&self) -> Vec<String> {
        let max = self.max_task_num();
        let started = self.store.with_tasks(|tasks| tasks.start_all(max));
        self.dispatch(&started);
        started
    }

    /// `pauseAll()`（`task_service.py:536-539`）。
    ///
    /// ```python
    /// for task in list(self.tasks.values()):
    ///     if self._queue.isRunning(task.taskId) or self._queue.isWaiting(task.taskId):
    ///         self.pause(task)
    /// ```
    ///
    /// 每个命中的任务都走一次 [`actions::TaskService::pause`] 的语义（含
    /// `taskPaused` 与 `_cancelWork`），返回所有被派发的 id。先快照 id 再遍历：
    /// `pause` 内部的 `_pump()` 会改动队列（Python 的 `list(...)` 同理）。
    pub fn pause_all(&self) -> Vec<String> {
        let max = self.max_task_num();
        let snapshot: Vec<String> = self.store.with_tasks(|tasks| {
            tasks
                .records()
                .iter()
                .filter(|record| {
                    tasks.is_running(&record.task_id) || tasks.is_waiting(&record.task_id)
                })
                .map(|record| record.task_id.clone())
                .collect()
        });
        let mut started = Vec::new();
        for task_id in snapshot {
            let report = self.store.with_tasks(|tasks| tasks.pause(&task_id, max));
            let Some(report) = report else { continue };
            self.emit_cancel(&task_id, &report.cancel);
            self.emit(TaskAction::Paused {
                task_id: task_id.clone(),
            });
            self.dispatch(&report.started);
            started.extend(report.started);
        }
        started
    }

    /// `_onRunDone` / `_onRunFailed` 的记账 + 派发（返回 `allCompleted`）。
    pub fn on_run_finished(&self, task_id: &str) -> bool {
        let max = self.max_task_num();
        let outcome = self
            .store
            .with_tasks(|tasks| tasks.on_run_finished(task_id, max));
        self.dispatch(&outcome.started);
        outcome.all_completed
    }

    /// `_rebalance(*_args)`：上限变小时把超出的运行中任务放回等待队列。
    pub fn rebalance(&self) -> Vec<String> {
        let max = self.max_task_num();
        let report = self.store.with_tasks(|tasks| tasks.rebalance(max));
        for (task_id, work) in &report.cancelled {
            self.emit_cancel(task_id, work);
        }
        self.dispatch(&report.started);
        report.started
    }

    /// `TaskStatus` 的只读快照（测试与状态行用）。
    pub fn status_of(&self, task_id: &str) -> Option<TaskStatus> {
        self.store
            .with_tasks(|tasks| tasks.task(task_id).map(|record| record.status))
    }
}

impl<S: TaskStore> actions::TaskService for TaskActions<S> {
    fn pause(&self, task_id: &str) {
        let max = self.max_task_num();
        let report = self.store.with_tasks(|tasks| tasks.pause(task_id, max));
        let Some(report) = report else {
            return;
        };
        self.emit_cancel(task_id, &report.cancel);
        self.emit(TaskAction::Paused {
            task_id: task_id.to_string(),
        });
        self.dispatch(&report.started);
    }

    fn start(&self, task_id: &str) {
        let max = self.max_task_num();
        // Python 的 `start` 先过 `isRunning or isWaiting` 两道短路，命中就直接
        // return（连 `_pump` 都不走）；`TaskService::start` 返回 `false` 即此。
        let started = self.store.with_tasks(|tasks| {
            if tasks.start(task_id) {
                tasks.pump(max)
            } else {
                Vec::new()
            }
        });
        self.dispatch(&started);
    }

    fn start_seeding(&self, task_id: &str) {
        let work_id = self.next_work_id("seed");
        let event = self
            .store
            .with_tasks(|tasks| tasks.start_seeding(task_id, &work_id));
        if event.is_some() {
            self.emit(TaskAction::SeedingStarted {
                task_id: task_id.to_string(),
                work_id,
                is_manual: true,
            });
        }
    }

    fn stop_seeding(&self, task_id: &str) {
        let work = self.store.with_tasks(|tasks| tasks.stop_seeding(task_id));
        self.emit_cancel(task_id, &work);
    }

    fn delete(&self, task_id: &str, delete_files: bool) {
        let max = self.max_task_num();
        let report = self
            .store
            .with_tasks(|tasks| tasks.delete(task_id, delete_files, max));
        let Some(report) = report else {
            return;
        };
        // `cancelChecksum(task)`：`checksumStopped.emit(task, None)`
        if let Some(work_id) = report.checksum_work {
            self.emit(TaskAction::ChecksumStopped {
                task_id: task_id.to_string(),
                work_id,
            });
        }
        // `_unwatchFile(task)`
        self.emit(TaskAction::UnwatchFile {
            task_id: task_id.to_string(),
        });
        // `fileDeleteDenied.emit()`（只提示一次）
        if report.delete_denied {
            self.emit(TaskAction::DeleteDenied);
        }
        // `_cancelWork(task, finished=onStopped)`
        self.emit_cancel(task_id, &report.cancel);
        // `onStopped`: `if canDelete: task.deleteFiles()`（占位符已就地删过）
        if report.delete_files {
            self.emit(TaskAction::DeleteFiles {
                task_id: task_id.to_string(),
            });
        }
        // `taskRemoved.emit(task.taskId)`
        self.emit(TaskAction::Removed {
            task_id: task_id.to_string(),
        });
        // 尾部 `_pump()`
        self.dispatch(&report.started);
    }

    fn redownload(&self, task_id: &str) {
        let max = self.max_task_num();
        let report = self
            .store
            .with_tasks(|tasks| tasks.redownload(task_id, max));
        let Some(report) = report else {
            return;
        };
        if let Some(work_id) = report.checksum_work {
            self.emit(TaskAction::ChecksumStopped {
                task_id: task_id.to_string(),
                work_id,
            });
        }
        self.emit(TaskAction::UnwatchFile {
            task_id: task_id.to_string(),
        });
        self.emit_cancel(task_id, &report.cancel);
        // `onStopped` 的第一句是 `task.deleteFiles()`——**不判 canDelete**。
        self.emit(TaskAction::DeleteFiles {
            task_id: task_id.to_string(),
        });
        // `reset()` + `_flushSoon()` + `_schedule()` 已在 `TaskService` 里做完。
        self.dispatch(&report.started);
    }

    fn move_to_front(&self, task_id: &str) {
        let ids = [task_id.to_string()];
        let moved = self.store.with_tasks(|tasks| tasks.move_to_front(&ids));
        if moved {
            self.emit(TaskAction::QueueChanged);
        }
    }

    fn set_category(&self, task_id: &str, category_id: &str) {
        // Python 只改字段 + `_flushSoon()`：没有信号、不重算 outputFolder。
        self.store
            .with_tasks(|tasks| tasks.set_category(task_id, category_id));
    }

    fn start_checksum(&self, task_id: &str, algorithm: &str) {
        let work_id = self.next_work_id("checksum");
        let event = self
            .store
            .with_tasks(|tasks| tasks.start_checksum(task_id, &work_id));
        if event.is_some() {
            self.emit(TaskAction::ChecksumStarted {
                task_id: task_id.to_string(),
                work_id,
                algorithm: algorithm.to_string(),
            });
        }
    }

    fn cancel_checksum(&self, task_id: &str) {
        let work_id = self
            .store
            .with_tasks(|tasks| tasks.cancel_checksum(task_id));
        if let Some(work_id) = work_id {
            self.emit(TaskAction::ChecksumStopped {
                task_id: task_id.to_string(),
                work_id,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::TaskService as _;
    use gd3_core::task::TaskRecord;

    /// 输出目录固定在 `temp_dir()` 下且**不创建**：`delete` 只删占位符，
    /// `delete_placeholder` 对不存在的路径返回 `Ok`。
    fn record(task_id: &str, status: TaskStatus) -> TaskRecord {
        TaskRecord {
            name: format!("{task_id}.bin"),
            url: format!("https://example.com/{task_id}"),
            pack_id: String::new(),
            task_id: task_id.to_string(),
            status,
            steps: Vec::new(),
            created_at: 0,
            completed_at: 0,
            output_folder: std::env::temp_dir()
                .join("gd3-ui-task-actions")
                .to_string_lossy()
                .into_owned(),
            file_size: 10,
            files: None,
            category: None,
            should_seed: false,
            checksums: serde_json::Map::new(),
            extra: serde_json::Map::new(),
            type_name: None,
        }
    }

    fn store(records: Vec<TaskRecord>) -> Rc<RefCell<TaskService>> {
        Rc::new(RefCell::new(TaskService::from_records(records)))
    }

    fn handle(
        store: &Rc<RefCell<TaskService>>,
        max_task_num: usize,
    ) -> TaskActions<Rc<RefCell<TaskService>>> {
        TaskActions::with_max(Rc::clone(store), max_task_num)
    }

    /// `TaskService::start` = `queue.wait`，**不 pump**（把队列布置好再手动派发）。
    fn enqueue(store: &Rc<RefCell<TaskService>>, ids: &[&str]) {
        store.with_tasks(|tasks| {
            for id in ids {
                assert!(tasks.start(id), "{id} 应入队");
            }
        });
    }

    fn status(store: &Rc<RefCell<TaskService>>, task_id: &str) -> TaskStatus {
        store
            .with_tasks(|tasks| tasks.task(task_id).map(|record| record.status))
            .expect("任务应在列表里")
    }

    fn started(task_id: &str, work_id: &str) -> TaskAction {
        TaskAction::Started {
            task_id: task_id.to_string(),
            work_id: work_id.to_string(),
        }
    }

    fn cancel(task_id: &str, work_id: &str) -> TaskAction {
        TaskAction::CancelWork {
            task_id: task_id.to_string(),
            work_id: work_id.to_string(),
        }
    }

    fn paused(task_id: &str) -> TaskAction {
        TaskAction::Paused {
            task_id: task_id.to_string(),
        }
    }

    #[test]
    fn pause_emits_cancel_then_paused_then_dispatches_the_freed_slot() {
        let store = store(vec![
            record("a", TaskStatus::Waiting),
            record("b", TaskStatus::Waiting),
        ]);
        let layer = handle(&store, 1);
        enqueue(&store, &["a", "b"]);
        assert_eq!(layer.pump(), vec!["a".to_string()]);
        assert_eq!(layer.take_effects(), vec![started("a", "run-1")]);

        layer.pause("a");

        assert_eq!(
            layer.take_effects(),
            vec![cancel("a", "run-1"), paused("a"), started("b", "run-2")]
        );
        assert_eq!(status(&store, "a"), TaskStatus::Paused);
        assert_eq!(status(&store, "b"), TaskStatus::Running);
        assert!(store.with_tasks(|tasks| tasks.waiting_order()).is_empty());
    }

    #[test]
    fn start_short_circuits_for_running_and_queued_tasks() {
        let store = store(vec![record("a", TaskStatus::Paused)]);
        let layer = handle(&store, 1);
        enqueue(&store, &["a"]);
        layer.pump();
        layer.take_effects();

        // 已在跑：Python 的 `start` 两条短路命中就 return，连 `_pump` 都不走。
        layer.start("a");
        assert!(layer.take_effects().is_empty());
        assert_eq!(status(&store, "a"), TaskStatus::Running);

        // 暂停后不在队列里：入队 + 派发。
        layer.pause("a");
        layer.take_effects();
        layer.start("a");
        assert_eq!(layer.take_effects(), vec![started("a", "run-2")]);
        assert_eq!(status(&store, "a"), TaskStatus::Running);
    }

    #[test]
    fn start_seeding_marks_the_record_and_emits_one_effect() {
        let store = store(vec![record("a", TaskStatus::Completed)]);
        let layer = handle(&store, 2);

        layer.start_seeding("a");
        assert_eq!(
            layer.take_effects(),
            vec![TaskAction::SeedingStarted {
                task_id: "a".to_string(),
                work_id: "seed-1".to_string(),
                is_manual: true,
            }]
        );
        assert!(
            store.with_tasks(|tasks| tasks.task("a").expect("记录").should_seed),
            "`startSeeding` 要把 `shouldSeed` 置真（持久化字段）"
        );
        assert!(store.with_tasks(|tasks| tasks.is_seeding("a")));

        // 任务不在列表里 → 不发效果（workId 已经消耗掉，无妨）。
        layer.start_seeding("ghost");
        assert!(layer.take_effects().is_empty());
    }

    #[test]
    fn stop_seeding_emits_seeding_stopped_before_the_cancel() {
        let store = store(vec![record("a", TaskStatus::Running)]);
        let layer = handle(&store, 2);
        layer.start_seeding("a");
        layer.take_effects();

        layer.stop_seeding("a");

        // `seedingStopped.emit(task)` 在 `cancel(workId)` 之前。
        assert_eq!(
            layer.take_effects(),
            vec![
                TaskAction::SeedingStopped {
                    task_id: "a".to_string(),
                },
                cancel("a", "seed-1"),
            ]
        );
        assert!(!store.with_tasks(|tasks| tasks.task("a").expect("记录").should_seed));
        assert!(!store.with_tasks(|tasks| tasks.is_seeding("a")));
    }

    #[test]
    fn delete_emits_unwatch_cancel_removed_then_dispatches() {
        let store = store(vec![
            record("a", TaskStatus::Running),
            record("b", TaskStatus::Waiting),
        ]);
        let layer = handle(&store, 1);
        enqueue(&store, &["a", "b"]);
        layer.pump();
        layer.take_effects();

        layer.delete("a", false);

        assert_eq!(
            layer.take_effects(),
            vec![
                TaskAction::UnwatchFile {
                    task_id: "a".to_string(),
                },
                cancel("a", "run-1"),
                TaskAction::Removed {
                    task_id: "a".to_string(),
                },
                started("b", "run-2"),
            ]
        );
        assert_eq!(store.with_tasks(|tasks| tasks.records().len()), 1);
        assert_eq!(status(&store, "b"), TaskStatus::Running);
    }

    #[test]
    fn delete_puts_delete_files_after_the_cancel_and_checksum_stopped_first() {
        let store = store(vec![record("a", TaskStatus::Running)]);
        let layer = handle(&store, 1);
        enqueue(&store, &["a"]);
        layer.pump();
        layer.start_checksum("a", "SHA-256");
        layer.take_effects();

        // `shouldDeleteFiles=true` 且 Linux 上 `_canDeleteIn` 恒真 → 带 `deleteFiles`。
        layer.delete("a", true);

        assert_eq!(
            layer.take_effects(),
            vec![
                TaskAction::ChecksumStopped {
                    task_id: "a".to_string(),
                    work_id: "checksum-2".to_string(),
                },
                TaskAction::UnwatchFile {
                    task_id: "a".to_string(),
                },
                cancel("a", "run-1"),
                TaskAction::DeleteFiles {
                    task_id: "a".to_string(),
                },
                TaskAction::Removed {
                    task_id: "a".to_string(),
                },
            ]
        );
        assert!(store.with_tasks(|tasks| tasks.records().is_empty()));
    }

    #[test]
    fn redownload_deletes_files_after_the_cancel_and_requeues() {
        let store = store(vec![record("a", TaskStatus::Completed)]);
        let layer = handle(&store, 1);
        enqueue(&store, &["a"]);
        layer.pump();
        layer.take_effects();
        store.with_tasks(|tasks| {
            tasks.task_mut("a").expect("记录").completed_at = 12345;
        });

        layer.redownload("a");

        assert_eq!(
            layer.take_effects(),
            vec![
                TaskAction::UnwatchFile {
                    task_id: "a".to_string(),
                },
                cancel("a", "run-1"),
                TaskAction::DeleteFiles {
                    task_id: "a".to_string(),
                },
                started("a", "run-2"),
            ]
        );
        let record = store.with_tasks(|tasks| tasks.task("a").expect("记录").clone());
        assert_eq!(record.completed_at, 0, "`reset()` 要清 `completedAt`");
        assert!(record.should_seed, "`reset()` 把 `shouldSeed` 置真");
        assert!(record.checksums.is_empty());
        assert_eq!(record.status, TaskStatus::Running);
    }

    #[test]
    fn move_to_front_emits_queue_changed_only_when_the_queue_takes_it() {
        let store = store(vec![
            record("a", TaskStatus::Waiting),
            record("b", TaskStatus::Waiting),
        ]);
        let layer = handle(&store, 2);
        enqueue(&store, &["a", "b"]);

        // 已经在队首也返回真（`TaskQueue::move_to_front` 只判「命中」）→ 照发信号。
        layer.move_to_front("a");
        assert_eq!(layer.take_effects(), vec![TaskAction::QueueChanged]);
        assert_eq!(
            store.with_tasks(|tasks| tasks.waiting_order()),
            vec!["a".to_string(), "b".to_string()]
        );

        // 不在等待队列里 → `false`，不发 `queueChanged`。
        layer.move_to_front("ghost");
        assert!(layer.take_effects().is_empty());
    }

    #[test]
    fn set_category_writes_the_field_without_any_effect() {
        let store = store(vec![record("a", TaskStatus::Completed)]);
        let layer = handle(&store, 1);
        assert!(!store.with_tasks(|tasks| tasks.pending_flush()));

        layer.set_category("a", "video");

        assert!(
            layer.take_effects().is_empty(),
            "Python 只改字段 + `_flushSoon`"
        );
        assert_eq!(
            store.with_tasks(|tasks| tasks.task("a").expect("记录").category.clone()),
            Some("video".to_string())
        );
        assert!(store.with_tasks(|tasks| tasks.pending_flush()));
    }

    #[test]
    fn checksum_start_and_cancel_round_trip() {
        let store = store(vec![record("a", TaskStatus::Completed)]);
        let layer = handle(&store, 1);

        layer.start_checksum("a", "SHA-256");
        assert_eq!(
            layer.take_effects(),
            vec![TaskAction::ChecksumStarted {
                task_id: "a".to_string(),
                work_id: "checksum-1".to_string(),
                algorithm: "SHA-256".to_string(),
            }]
        );

        layer.cancel_checksum("a");
        assert_eq!(
            layer.take_effects(),
            vec![TaskAction::ChecksumStopped {
                task_id: "a".to_string(),
                work_id: "checksum-1".to_string(),
            }]
        );

        // 登记表里已经没有了 → 第二次取消不发效果。
        layer.cancel_checksum("a");
        assert!(layer.take_effects().is_empty());
    }

    #[test]
    fn start_all_then_pause_all_walks_the_queue_in_order() {
        let store = store(vec![
            record("p", TaskStatus::Paused),
            record("w", TaskStatus::Waiting),
            record("c", TaskStatus::Completed),
        ]);
        let layer = handle(&store, 1);

        assert_eq!(layer.start_all(), vec!["p".to_string()]);
        assert_eq!(layer.take_effects(), vec![started("p", "run-1")]);
        assert_eq!(status(&store, "p"), TaskStatus::Running);
        assert_eq!(status(&store, "c"), TaskStatus::Completed);

        // `pauseAll`：`p` 在跑、`w` 在等；`pause(p)` 的 `_pump` 先把 `w` 派发出去，
        // 紧接着 `pause(w)` 又把它取消——Python 的中间态（第 99 步实测第 8 条）。
        assert_eq!(layer.pause_all(), vec!["w".to_string()]);
        assert_eq!(
            layer.take_effects(),
            vec![
                cancel("p", "run-1"),
                paused("p"),
                started("w", "run-2"),
                cancel("w", "run-2"),
                paused("w"),
            ]
        );
        assert_eq!(status(&store, "p"), TaskStatus::Paused);
        assert_eq!(status(&store, "w"), TaskStatus::Paused);
        assert_eq!(store.with_tasks(|tasks| tasks.running_count()), 0);
    }

    #[test]
    fn work_ids_are_unique_across_kinds() {
        let store = store(vec![record("a", TaskStatus::Paused)]);
        let layer = handle(&store, 1);

        layer.start_seeding("a");
        layer.start_checksum("a", "MD5");
        enqueue(&store, &["a"]);
        layer.pump();

        let mut seen: Vec<String> = Vec::new();
        for action in layer.take_effects() {
            let work_id = match action {
                TaskAction::Started { work_id, .. }
                | TaskAction::CancelWork { work_id, .. }
                | TaskAction::SeedingStarted { work_id, .. }
                | TaskAction::ChecksumStarted { work_id, .. }
                | TaskAction::ChecksumStopped { work_id, .. } => work_id,
                _ => continue,
            };
            assert!(!seen.contains(&work_id), "workId 重复：{work_id}");
            seen.push(work_id);
        }
        assert_eq!(seen.len(), 3, "做种 / 校验 / 下载各一个");
    }

    #[test]
    fn refcell_ref_mut_variant_borrows_the_engine_copy() {
        let mut service = TaskService::from_records(vec![record("a", TaskStatus::Waiting)]);
        assert!(service.start("a"));
        {
            let layer = TaskActions::with_max(RefCell::new(&mut service), 1);
            assert_eq!(layer.pump(), vec!["a".to_string()]);
            assert_eq!(layer.take_effects(), vec![started("a", "run-1")]);

            layer.pause("a");
            assert_eq!(
                layer.take_effects(),
                vec![cancel("a", "run-1"), paused("a")]
            );
        }
        // 句柄释放后，引擎那份能看到改动。
        assert_eq!(service.task("a").expect("记录").status, TaskStatus::Paused);
        assert_eq!(service.running_count(), 0);
    }

    #[test]
    fn on_run_finished_dispatches_the_next_waiting_task() {
        let store = store(vec![
            record("a", TaskStatus::Running),
            record("b", TaskStatus::Waiting),
        ]);
        let layer = handle(&store, 1);
        enqueue(&store, &["a", "b"]);
        layer.pump();
        layer.take_effects();

        assert!(!layer.on_run_finished("a"));
        assert_eq!(layer.take_effects(), vec![started("b", "run-2")]);
        assert!(!store.with_tasks(|tasks| tasks.is_running("a")));
        assert!(store.with_tasks(|tasks| tasks.is_running("b")));
    }

    #[test]
    fn rebalance_cancels_the_overflow_and_keeps_the_head() {
        let store = store(vec![
            record("a", TaskStatus::Running),
            record("b", TaskStatus::Running),
        ]);
        let wide = handle(&store, 2);
        enqueue(&store, &["a", "b"]);
        wide.pump();
        wide.take_effects();

        // 上限从 2 降到 1：`runningIds()[1:]` 里的 `b` 被放回等待队列。
        let narrow = handle(&store, 1);
        assert!(narrow.rebalance().is_empty(), "让位之后没有空位可派");
        assert_eq!(narrow.take_effects(), vec![cancel("b", "run-2")]);
        assert_eq!(status(&store, "b"), TaskStatus::Waiting);
        assert_eq!(status(&store, "a"), TaskStatus::Running);
    }
}
