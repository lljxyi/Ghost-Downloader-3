//! 占位符（placeholder）与重名冲突决策。
//!
//! 镜像 `app/models/task.py::deletePlaceholder` 与 `app/services/task_service.py`
//! 里 `TaskStore` 的 `add` / `_createPlaceholders` / `_setPlaceholders` /
//! `_removePlaceholders` / `_isUnfinishedPlaceholder`。
//!
//! 设计取向：冲突决策做成「纯循环 + 闭包注入」。`resolve_conflict` 不知道
//! `Task` 是什么，只通过三个闭包（建占位符 / 查占位符是否属于未完成任务 /
//! 可恢复删除）与外界交互 —— 于是全部决策分支都能在 `gd3-core` 里被单测钉住，
//! 而 `TaskService` 的接线（含 `tasks.jsonl` 落盘）留在 `gd3-ui`（第 93 步）。
//!
//! # 与 Python 的已知差异
//!
//! 1. **`placeholder_paths` 只返回 `[outputFolder/name]`**。Python 的
//!    `Task.placeholderPaths` 还会遍历 `self.steps`，把落在 `outputFolder` 下的
//!    `step.outputPath` 追加进去；但 `TaskStep.outputPath` 基类返回 `""` 且全仓
//!    没有任何子类覆盖它，所以那个循环永不追加。插件若自定义 step 覆盖
//!    `outputPath`，这里需要跟着扩。
//! 2. **改名重试有上限**（`MAX_RENAME_ATTEMPTS`）。Python 没有上限：当文件名长到
//!    触发 `toSafeFilename` 的 200 字符截断时，`name(1).ext` 与 `name(2).ext`
//!    截断后可能完全相同 → 死循环。这里到达上限后返回最后一次的 `Taken`。
//! 3. `CreateError::Io` 对应 Python 里非 `FileExistsError` 的 `OSError`：Python
//!    直接向上抛、`TaskStore.add` 中断，这里同样以 `Err` 冒泡而不是吞掉。

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::filename::{DEFAULT_MAX_FILENAME_LENGTH, split_stem_ext, to_safe_filename};
use crate::filesystem::delete_path;
use crate::task::TaskStatus;

/// 改名重试上限。见模块文档第 2 条差异。
pub const MAX_RENAME_ATTEMPTS: usize = 10_000;

/// 镜像 `NameConflictChoice`（`task_service.py:37`，`StrEnum`）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NameConflictChoice {
    /// `"keepBoth"`：自动改名，两个文件都留下。
    #[default]
    KeepBoth,
    /// `"overwrite"`：先可恢复删除已存在的那个，失败则退化成 `KeepBoth`。
    Overwrite,
    /// `"ask"`：交回调用方询问用户（`add` 返回 `None`）。
    Ask,
}

impl NameConflictChoice {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::KeepBoth => "keepBoth",
            Self::Overwrite => "overwrite",
            Self::Ask => "ask",
        }
    }

    /// 镜像 `NameConflictChoice(value)`；未知值在 Python 里抛 `ValueError`，
    /// 这里返回 `None`。
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "keepBoth" => Some(Self::KeepBoth),
            "overwrite" => Some(Self::Overwrite),
            "ask" => Some(Self::Ask),
            _ => None,
        }
    }
}

impl fmt::Display for NameConflictChoice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 占位符目标已被占用（Python 的 `FileExistsError` + `error.filename`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakenPath {
    pub path: PathBuf,
}

impl TakenPath {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl fmt::Display for TakenPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "placeholder already exists: {}",
            self.path.display()
        )
    }
}

impl std::error::Error for TakenPath {}

/// `create_placeholders` 的失败原因。
#[derive(Debug)]
pub enum CreateError {
    /// 目标已存在 —— 冲突决策循环要接着处理这个。
    Taken(TakenPath),
    /// 其它 IO 失败 —— Python 侧同样直接向上抛，`add` 中断。
    Io { path: PathBuf, error: io::Error },
}

impl CreateError {
    pub fn taken_path(&self) -> Option<&Path> {
        match self {
            Self::Taken(taken) => Some(&taken.path),
            Self::Io { .. } => None,
        }
    }
}

impl fmt::Display for CreateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Taken(taken) => write!(formatter, "{taken}"),
            Self::Io { path, error } => {
                write!(formatter, "cannot create {}: {error}", path.display())
            }
        }
    }
}

impl std::error::Error for CreateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Taken(taken) => Some(taken),
            Self::Io { error, .. } => Some(error),
        }
    }
}

impl From<TakenPath> for CreateError {
    fn from(taken: TakenPath) -> Self {
        Self::Taken(taken)
    }
}

/// 镜像 `Task.placeholderPaths` 的第一项，也就是唯一一项。
///
/// Python 是 `Path(self.outputFolder / self.name)`；`name` 已经过 `toSafeFilename`
/// 处理（不含分隔符），所以 `join` 与 pathlib 的 `/` 等价。
pub fn placeholder_paths(output_folder: &Path, name: &str) -> Vec<PathBuf> {
    vec![output_folder.join(name)]
}

/// 从**已经算好的** `Task.outputPath` 构造占位符列表。
///
/// 与 `placeholder_paths` 的区别：`BilibiliTask` 多文件时 `outputPath` 取的是
/// `outputFolder / Path(name).stem`（见 `TaskRecord::output_path_for`），不能由
/// `output_folder` + `name` 直接拼出来。改名重试时每个候选名字都要重算一次
/// `outputPath`，所以调用方用这个而不是 `placeholder_paths`。
pub fn placeholder_paths_of(output_path: &str) -> Vec<PathBuf> {
    vec![PathBuf::from(output_path)]
}

/// `<path>.ghd` —— Python 用 `Path(f"{path}.ghd")`，即整串路径直接拼后缀。
pub fn sidecar_path(path: &Path) -> PathBuf {
    let mut raw = OsString::from(path.as_os_str());
    raw.push(".ghd");
    PathBuf::from(raw)
}

/// 镜像 `deletePlaceholder`（`task.py:20`）：空目录 → `rmdir`，0 字节文件 →
/// `unlink`，其余**一律不动**。
///
/// 注意 `is_dir()` / `is_file()` 都跟随符号链接（Python 同样），所以「指向空目录的
/// 符号链接」会走到 `remove_dir` 并失败 —— 与 Python 抛 `NotADirectoryError` 一致。
pub fn delete_placeholder(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        if fs::read_dir(path)?.next().is_none() {
            return fs::remove_dir(path);
        }
        return Ok(());
    }
    if path.is_file() && fs::metadata(path)?.len() == 0 {
        return fs::remove_file(path);
    }
    Ok(())
}

/// 镜像 `_createPlaceholders`（`task_service.py:135`）。
///
/// - `existing` 里已有的路径直接跳过（`updatePlaceholders` 复用同一份占位符时用）；
/// - `is_output_folder && index == 0` → `mkdir`，否则 `create_new`；
/// - 任何一个失败就把**本次已建成的**逐个 `delete_placeholder` 回滚；
/// - 全部成功后清掉每个占位符旁边的 `<path>.ghd` 残留。
pub fn create_placeholders(
    paths: &[PathBuf],
    existing: &[PathBuf],
    is_output_folder: bool,
) -> Result<Vec<PathBuf>, CreateError> {
    let mut created: Vec<PathBuf> = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        if existing.contains(path) {
            continue;
        }
        let attempt = if is_output_folder && index == 0 {
            fs::create_dir(path)
        } else {
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .map(|_| ())
        };
        if let Err(error) = attempt {
            for done in &created {
                let _ = delete_placeholder(done);
            }
            if error.kind() == io::ErrorKind::AlreadyExists {
                return Err(CreateError::Taken(TakenPath::new(path.clone())));
            }
            return Err(CreateError::Io {
                path: path.clone(),
                error,
            });
        }
        created.push(path.clone());
    }
    for path in &created {
        delete_path(&sidecar_path(path));
    }
    Ok(created)
}

/// 冲突决策的产物。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictOutcome {
    /// 最终采用的名字（可能被改名，也可能因为覆盖成功而保持原样）。
    pub name: String,
    /// 是否发生过「选择覆盖但删除失败」。
    pub overwrite_failed: bool,
}

/// 镜像 `TaskStore.add` 的 `while True` 冲突循环（`task_service.py:65-80`）。
///
/// 返回值三层含义：
/// - `Ok(Some(outcome))` —— 占位符建成了；
/// - `Ok(None)` —— 用户选择 `Ask` 且占位符不属于未完成任务，调用方应放弃本次添加；
/// - `Err(error)` —— IO 失败，`add` 中断。
///
/// 闭包契约：
/// - `create(name)` 按给定名字建占位符，`Err(Taken(path))` 表示冲突；
/// - `is_unfinished(path)` 判断该占位符是否属于「尚未完成」的任务（属于就不许覆盖、
///   也不许因为 `Ask` 放弃，只能改名）；
/// - `delete_recoverably(path)` 把文件移进回收站，返回是否成功。
///
/// `stem` / `ext` 取自**原始**名字，改名序列是 `stem(1)ext`、`stem(2)ext`……
/// 每次改名都以「上一个名字」作 `toSafeFilename` 的 fallback（`setName` 的口径）。
pub fn resolve_conflict<C, U, D>(
    name: &str,
    choice: NameConflictChoice,
    mut create: C,
    mut is_unfinished: U,
    mut delete_recoverably: D,
) -> Result<Option<ConflictOutcome>, CreateError>
where
    C: FnMut(&str) -> Result<(), CreateError>,
    U: FnMut(&Path) -> bool,
    D: FnMut(&Path) -> bool,
{
    let (stem, ext) = split_stem_ext(name);
    let mut choice = choice;
    let mut current = name.to_string();
    let mut overwrite_failed = false;
    let mut index: usize = 0;
    let mut last_taken: Option<TakenPath> = None;

    for _ in 0..MAX_RENAME_ATTEMPTS {
        let taken = match create(&current) {
            Ok(()) => {
                return Ok(Some(ConflictOutcome {
                    name: current,
                    overwrite_failed,
                }));
            }
            Err(CreateError::Taken(taken)) => taken,
            Err(error) => return Err(error),
        };
        let unfinished = is_unfinished(&taken.path);
        if !unfinished && choice == NameConflictChoice::Ask {
            return Ok(None);
        }
        if !unfinished && choice == NameConflictChoice::Overwrite {
            if delete_recoverably(&taken.path) {
                last_taken = Some(taken);
                continue;
            }
            overwrite_failed = true;
            choice = NameConflictChoice::KeepBoth;
        }
        index += 1;
        current = to_safe_filename(
            &format!("{stem}({index}){ext}"),
            &current,
            DEFAULT_MAX_FILENAME_LENGTH,
        );
        last_taken = Some(taken);
    }

    Err(CreateError::Taken(
        last_taken.unwrap_or_else(|| TakenPath::new(PathBuf::new())),
    ))
}

/// 镜像 `Placeholders`（`task_service.py:44`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placeholders {
    pub output_folder: PathBuf,
    pub name: String,
    pub paths: Vec<PathBuf>,
}

/// 镜像 `TaskStore._placeholders` + `TaskStore._taskByPlaceholder`。
///
/// 两张表必须同步维护：`by_task` 记录「某个任务占了哪些路径」，`by_path` 记录
/// 「某个路径被哪个任务占着」。Python 的 `_taskByPlaceholder` 存的是 `Task` 对象，
/// 这里存 `taskId` —— 状态查询交给调用方（`is_unfinished` 的 `status_of`）。
#[derive(Debug, Default, Clone)]
pub struct PlaceholderRegistry {
    by_task: BTreeMap<String, Placeholders>,
    by_path: BTreeMap<PathBuf, String>,
}

impl PlaceholderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 镜像 `_setPlaceholders`：先撤掉旧登记，再写入新登记。
    pub fn set(&mut self, task_id: &str, output_folder: &Path, name: &str, paths: &[PathBuf]) {
        self.remove(task_id);
        self.by_task.insert(
            task_id.to_string(),
            Placeholders {
                output_folder: output_folder.to_path_buf(),
                name: name.to_string(),
                paths: paths.to_vec(),
            },
        );
        for path in paths {
            self.by_path.insert(path.clone(), task_id.to_string());
        }
    }

    /// 镜像 `_removePlaceholders`：只删「确实属于本任务」的 `by_path` 项，
    /// 以免误删后来者对该路径的登记。
    pub fn remove(&mut self, task_id: &str) -> Option<Placeholders> {
        let placeholders = self.by_task.remove(task_id)?;
        for path in &placeholders.paths {
            let owned = self.by_path.get(path).is_some_and(|owner| owner == task_id);
            if owned {
                self.by_path.remove(path);
            }
        }
        Some(placeholders)
    }

    pub fn get(&self, task_id: &str) -> Option<&Placeholders> {
        self.by_task.get(task_id)
    }

    /// 该路径当前被哪个任务占着（Python 的 `_taskByPlaceholder.get(path)`）。
    pub fn owner(&self, path: &Path) -> Option<&str> {
        self.by_path.get(path).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.by_task.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_task.is_empty()
    }

    /// 镜像 `_isUnfinishedPlaceholder`：占位符存在**且**所属任务不是 `COMPLETED`。
    pub fn is_unfinished(
        &self,
        path: &Path,
        status_of: impl Fn(&str) -> Option<TaskStatus>,
    ) -> bool {
        self.owner(path)
            .and_then(status_of)
            .is_some_and(|status| status != TaskStatus::Completed)
    }

    /// 镜像 `add` 里算 `replacedTasks` 的那一段：按 `paths` 顺序去重。
    ///
    /// Python 比的是 `Task` 对象身份（`replaced not in replacedTasks`），这里比
    /// `taskId` —— 同一个任务不会有两份登记，等价。
    pub fn replaced_tasks(&self, paths: &[PathBuf]) -> Vec<String> {
        let mut replaced: Vec<String> = Vec::new();
        for path in paths {
            if let Some(owner) = self.by_path.get(path)
                && !replaced.iter().any(|seen| seen == owner)
            {
                replaced.push(owner.clone());
            }
        }
        replaced
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("gd3ph-{}-{tag}-{id}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn child(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_file(path: &Path, body: &str) {
        fs::write(path, body).expect("write file");
    }

    #[test]
    fn choice_round_trips_through_strings() {
        for (choice, text) in [
            (NameConflictChoice::KeepBoth, "keepBoth"),
            (NameConflictChoice::Overwrite, "overwrite"),
            (NameConflictChoice::Ask, "ask"),
        ] {
            assert_eq!(choice.as_str(), text);
            assert_eq!(NameConflictChoice::parse(text), Some(choice));
            assert_eq!(choice.to_string(), text);
        }
        assert_eq!(NameConflictChoice::parse("nope"), None);
        assert_eq!(NameConflictChoice::default(), NameConflictChoice::KeepBoth);
    }

    #[test]
    fn placeholder_paths_joins_folder_and_name() {
        let paths = placeholder_paths(Path::new("/dl/out"), "a.txt");
        assert_eq!(paths, vec![PathBuf::from("/dl/out/a.txt")]);
    }

    #[test]
    fn sidecar_path_appends_suffix_to_whole_path() {
        assert_eq!(
            sidecar_path(Path::new("/dl/out/a.txt")),
            PathBuf::from("/dl/out/a.txt.ghd")
        );
    }

    #[test]
    fn delete_placeholder_removes_empty_dir() {
        let temp = TempDir::new("deldir");
        let dir = temp.child("out");
        fs::create_dir(&dir).expect("mkdir");
        delete_placeholder(&dir).expect("delete");
        assert!(!dir.exists());
    }

    #[test]
    fn delete_placeholder_keeps_non_empty_dir() {
        let temp = TempDir::new("keepdir");
        let dir = temp.child("out");
        fs::create_dir(&dir).expect("mkdir");
        write_file(&dir.join("inner.bin"), "x");
        delete_placeholder(&dir).expect("delete");
        assert!(dir.is_dir());
    }

    #[test]
    fn delete_placeholder_removes_zero_byte_file() {
        let temp = TempDir::new("delzero");
        let file = temp.child("a.txt");
        write_file(&file, "");
        delete_placeholder(&file).expect("delete");
        assert!(!file.exists());
    }

    #[test]
    fn delete_placeholder_keeps_non_zero_file() {
        let temp = TempDir::new("keepnon");
        let file = temp.child("a.txt");
        write_file(&file, "data");
        delete_placeholder(&file).expect("delete");
        assert!(file.exists());
    }

    #[test]
    fn delete_placeholder_ignores_missing_path() {
        let temp = TempDir::new("missing");
        delete_placeholder(&temp.child("nope.txt")).expect("no-op");
    }

    #[test]
    fn create_placeholders_makes_dir_for_output_folder() {
        let temp = TempDir::new("mkout");
        let target = temp.child("out");
        let created =
            create_placeholders(std::slice::from_ref(&target), &[], true).expect("create");
        assert_eq!(created, vec![target.clone()]);
        assert!(target.is_dir());
    }

    #[test]
    fn create_placeholders_makes_empty_file_otherwise() {
        let temp = TempDir::new("mkfile");
        let target = temp.child("a.txt");
        let created =
            create_placeholders(std::slice::from_ref(&target), &[], false).expect("create");
        assert_eq!(created, vec![target.clone()]);
        assert_eq!(fs::metadata(&target).expect("stat").len(), 0);
    }

    #[test]
    fn create_placeholders_skips_already_registered_paths() {
        let temp = TempDir::new("skip");
        let kept = temp.child("kept.txt");
        write_file(&kept, "data");
        let fresh = temp.child("fresh.txt");
        let created = create_placeholders(
            &[kept.clone(), fresh.clone()],
            std::slice::from_ref(&kept),
            false,
        )
        .expect("create");
        assert_eq!(created, vec![fresh.clone()]);
        // 已登记的那个一个字节都不动。
        assert_eq!(fs::read_to_string(&kept).expect("read"), "data");
    }

    #[test]
    fn create_placeholders_reports_taken_path() {
        let temp = TempDir::new("taken");
        let target = temp.child("a.txt");
        write_file(&target, "data");
        let error = create_placeholders(std::slice::from_ref(&target), &[], false)
            .expect_err("should conflict");
        assert_eq!(error.taken_path(), Some(target.as_path()));
        assert!(matches!(error, CreateError::Taken(_)));
    }

    #[test]
    fn create_placeholders_rolls_back_earlier_creations() {
        let temp = TempDir::new("rollback");
        let first = temp.child("first.txt");
        let second = temp.child("second.txt");
        write_file(&second, "occupied");
        let error = create_placeholders(&[first.clone(), second.clone()], &[], false)
            .expect_err("should conflict");
        assert_eq!(error.taken_path(), Some(second.as_path()));
        // 回滚只删「本次建成的」；被占用的那个原样保留。
        assert!(!first.exists());
        assert_eq!(fs::read_to_string(&second).expect("read"), "occupied");
    }

    #[test]
    fn create_placeholders_clears_ghd_sidecar() {
        let temp = TempDir::new("ghd");
        let target = temp.child("a.txt");
        let sidecar = temp.child("a.txt.ghd");
        write_file(&sidecar, "stale parts");
        create_placeholders(std::slice::from_ref(&target), &[], false).expect("create");
        assert!(target.exists());
        assert!(!sidecar.exists());
    }

    #[test]
    fn create_placeholders_reports_io_errors() {
        let temp = TempDir::new("ioerr");
        let target = temp.child("missing-parent").join("a.txt");
        let error = create_placeholders(std::slice::from_ref(&target), &[], false)
            .expect_err("should fail");
        assert!(matches!(error, CreateError::Io { .. }));
        assert_eq!(error.taken_path(), None);
    }

    #[test]
    fn resolve_conflict_passes_through_when_free() {
        let outcome = resolve_conflict(
            "a.txt",
            NameConflictChoice::KeepBoth,
            |_| Ok(()),
            |_| false,
            |_| false,
        )
        .expect("no io error")
        .expect("not asked");
        assert_eq!(
            outcome,
            ConflictOutcome {
                name: "a.txt".to_string(),
                overwrite_failed: false,
            }
        );
    }

    #[test]
    fn resolve_conflict_ask_gives_up_on_finished_placeholder() {
        let result = resolve_conflict(
            "a.txt",
            NameConflictChoice::Ask,
            |_| Err(CreateError::Taken(TakenPath::new("/dl/a.txt"))),
            |_| false,
            |_| false,
        )
        .expect("no io error");
        assert!(result.is_none());
    }

    #[test]
    fn resolve_conflict_keep_both_renames() {
        let mut seen: Vec<String> = Vec::new();
        let outcome = resolve_conflict(
            "a.txt",
            NameConflictChoice::KeepBoth,
            |name| {
                seen.push(name.to_string());
                if name == "a.txt" {
                    Err(CreateError::Taken(TakenPath::new("/dl/a.txt")))
                } else {
                    Ok(())
                }
            },
            |_| false,
            |_| false,
        )
        .expect("no io error")
        .expect("not asked");
        assert_eq!(seen, vec!["a.txt".to_string(), "a(1).txt".to_string()]);
        assert_eq!(outcome.name, "a(1).txt");
        assert!(!outcome.overwrite_failed);
    }

    #[test]
    fn resolve_conflict_overwrite_retries_same_name() {
        let mut seen: Vec<String> = Vec::new();
        let mut trashed: Vec<PathBuf> = Vec::new();
        let mut attempt = 0;
        let outcome = resolve_conflict(
            "a.txt",
            NameConflictChoice::Overwrite,
            |name| {
                seen.push(name.to_string());
                attempt += 1;
                if attempt == 1 {
                    Err(CreateError::Taken(TakenPath::new("/dl/a.txt")))
                } else {
                    Ok(())
                }
            },
            |_| false,
            |path| {
                trashed.push(path.to_path_buf());
                true
            },
        )
        .expect("no io error")
        .expect("not asked");
        assert_eq!(seen, vec!["a.txt".to_string(), "a.txt".to_string()]);
        assert_eq!(trashed, vec![PathBuf::from("/dl/a.txt")]);
        assert_eq!(outcome.name, "a.txt");
        assert!(!outcome.overwrite_failed);
    }

    #[test]
    fn resolve_conflict_overwrite_failure_degrades_to_keep_both() {
        let mut attempt = 0;
        let outcome = resolve_conflict(
            "a.txt",
            NameConflictChoice::Overwrite,
            |_| {
                attempt += 1;
                if attempt == 1 {
                    Err(CreateError::Taken(TakenPath::new("/dl/a.txt")))
                } else {
                    Ok(())
                }
            },
            |_| false,
            |_| false,
        )
        .expect("no io error")
        .expect("not asked");
        assert_eq!(outcome.name, "a(1).txt");
        assert!(outcome.overwrite_failed);
    }

    #[test]
    fn resolve_conflict_never_overwrites_unfinished_placeholder() {
        let mut trashed = 0usize;
        let mut attempt = 0;
        let outcome = resolve_conflict(
            "a.txt",
            NameConflictChoice::Overwrite,
            |_| {
                attempt += 1;
                if attempt == 1 {
                    Err(CreateError::Taken(TakenPath::new("/dl/a.txt")))
                } else {
                    Ok(())
                }
            },
            |_| true,
            |_| {
                trashed += 1;
                true
            },
        )
        .expect("no io error")
        .expect("not asked");
        assert_eq!(trashed, 0);
        assert_eq!(outcome.name, "a(1).txt");
        assert!(!outcome.overwrite_failed);
    }

    #[test]
    fn resolve_conflict_does_not_ask_away_unfinished_placeholder() {
        let mut attempt = 0;
        let result = resolve_conflict(
            "a.txt",
            NameConflictChoice::Ask,
            |_| {
                attempt += 1;
                if attempt == 1 {
                    Err(CreateError::Taken(TakenPath::new("/dl/a.txt")))
                } else {
                    Ok(())
                }
            },
            |_| true,
            |_| false,
        )
        .expect("no io error");
        let outcome = result.expect("must rename instead of giving up");
        assert_eq!(outcome.name, "a(1).txt");
    }

    #[test]
    fn resolve_conflict_renames_from_original_stem() {
        let mut seen: Vec<String> = Vec::new();
        let mut attempt = 0;
        let outcome = resolve_conflict(
            "movie.tar.gz",
            NameConflictChoice::KeepBoth,
            |name| {
                seen.push(name.to_string());
                attempt += 1;
                if attempt < 3 {
                    Err(CreateError::Taken(TakenPath::new("/dl/movie.tar.gz")))
                } else {
                    Ok(())
                }
            },
            |_| false,
            |_| false,
        )
        .expect("no io error")
        .expect("not asked");
        assert_eq!(
            seen,
            vec![
                "movie.tar.gz".to_string(),
                "movie(1).tar.gz".to_string(),
                "movie(2).tar.gz".to_string(),
            ]
        );
        assert_eq!(outcome.name, "movie(2).tar.gz");
    }

    #[test]
    fn resolve_conflict_gives_up_after_rename_cap() {
        let mut calls = 0usize;
        let result = resolve_conflict(
            "a.txt",
            NameConflictChoice::KeepBoth,
            |_| {
                calls += 1;
                Err(CreateError::Taken(TakenPath::new("/dl/a.txt")))
            },
            |_| false,
            |_| false,
        );
        assert!(matches!(result, Err(CreateError::Taken(_))));
        assert_eq!(calls, MAX_RENAME_ATTEMPTS);
    }

    #[test]
    fn resolve_conflict_propagates_io_errors() {
        let result = resolve_conflict(
            "a.txt",
            NameConflictChoice::KeepBoth,
            |_| {
                Err(CreateError::Io {
                    path: PathBuf::from("/dl/a.txt"),
                    error: io::Error::other("boom"),
                })
            },
            |_| false,
            |_| false,
        );
        assert!(matches!(result, Err(CreateError::Io { .. })));
    }

    #[test]
    fn registry_tracks_owner_and_paths() {
        let mut registry = PlaceholderRegistry::new();
        let path = PathBuf::from("/dl/a.txt");
        assert!(registry.is_empty());
        registry.set("t1", Path::new("/dl"), "a.txt", std::slice::from_ref(&path));
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.owner(&path), Some("t1"));
        let placeholders = registry.get("t1").expect("registered");
        assert_eq!(placeholders.name, "a.txt");
        assert_eq!(placeholders.output_folder, PathBuf::from("/dl"));
        assert_eq!(placeholders.paths, vec![path.clone()]);
        assert_eq!(registry.owner(Path::new("/dl/b.txt")), None);
    }

    #[test]
    fn registry_remove_keeps_later_owners_registration() {
        let mut registry = PlaceholderRegistry::new();
        let path = PathBuf::from("/dl/a.txt");
        registry.set("t1", Path::new("/dl"), "a.txt", std::slice::from_ref(&path));
        registry.set("t2", Path::new("/dl"), "a.txt", std::slice::from_ref(&path));
        assert_eq!(registry.owner(&path), Some("t2"));
        // 撤掉 t1 不能顺手清掉 t2 的登记（Python 的 `task.taskId == taskId` 守卫）。
        registry.remove("t1");
        assert_eq!(registry.owner(&path), Some("t2"));
        assert_eq!(registry.get("t1"), None);
        registry.remove("t2");
        assert_eq!(registry.owner(&path), None);
        assert!(registry.is_empty());
        assert!(registry.remove("t2").is_none());
    }

    #[test]
    fn registry_set_replaces_previous_paths() {
        let mut registry = PlaceholderRegistry::new();
        let old = PathBuf::from("/dl/a.txt");
        let new = PathBuf::from("/dl/b.txt");
        registry.set("t1", Path::new("/dl"), "a.txt", std::slice::from_ref(&old));
        registry.set("t1", Path::new("/dl"), "b.txt", std::slice::from_ref(&new));
        assert_eq!(registry.owner(&old), None);
        assert_eq!(registry.owner(&new), Some("t1"));
        assert_eq!(registry.get("t1").expect("registered").name, "b.txt");
    }

    #[test]
    fn registry_is_unfinished_follows_task_status() {
        let mut registry = PlaceholderRegistry::new();
        let done = PathBuf::from("/dl/done.txt");
        let running = PathBuf::from("/dl/running.txt");
        registry.set(
            "t1",
            Path::new("/dl"),
            "done.txt",
            std::slice::from_ref(&done),
        );
        registry.set(
            "t2",
            Path::new("/dl"),
            "running.txt",
            std::slice::from_ref(&running),
        );
        let status_of = |id: &str| match id {
            "t1" => Some(TaskStatus::Completed),
            "t2" => Some(TaskStatus::Running),
            _ => None,
        };
        assert!(!registry.is_unfinished(&done, status_of));
        assert!(registry.is_unfinished(&running, status_of));
        // 没登记的路径一律不算未完成。
        assert!(!registry.is_unfinished(Path::new("/dl/other.txt"), status_of));
    }

    #[test]
    fn registry_replaced_tasks_dedups_in_path_order() {
        let mut registry = PlaceholderRegistry::new();
        let a = PathBuf::from("/dl/a.txt");
        let b = PathBuf::from("/dl/b.txt");
        registry.set("t1", Path::new("/dl"), "a.txt", &[a.clone(), b.clone()]);
        assert_eq!(
            registry.replaced_tasks(&[b.clone(), a.clone(), b.clone()]),
            vec!["t1".to_string()]
        );
        assert!(
            registry
                .replaced_tasks(&[PathBuf::from("/dl/c.txt")])
                .is_empty()
        );
    }

    #[test]
    fn create_and_delete_round_trip_through_registry() {
        let temp = TempDir::new("roundtrip");
        let mut registry = PlaceholderRegistry::new();
        let paths = placeholder_paths(temp.path(), "a.txt");
        let created = create_placeholders(&paths, &[], false).expect("create");
        assert_eq!(created, paths);
        registry.set("t1", temp.path(), "a.txt", &paths);
        assert!(registry.is_unfinished(&paths[0], |_| Some(TaskStatus::Waiting)));
        registry.remove("t1");
        for path in &paths {
            delete_placeholder(path).expect("delete");
        }
        assert!(!paths[0].exists());
    }
}
