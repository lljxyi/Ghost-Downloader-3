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
//! - 不做 `_rebalance`（依赖 `Task.canPause` 这个运行时事实）、不做做种/校验/
//!   文件监听/速度表（`_seed`/`_onSeeding*`/`startChecksum`/`_watchFile`/
//!   `_refreshSpeedMeter` 全部留给引擎层）。

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

use crate::AppWindow;
use crate::card::CardRuntime;
use crate::category::CategoryService;
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
        self.cancel_work(task_id);
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
}
