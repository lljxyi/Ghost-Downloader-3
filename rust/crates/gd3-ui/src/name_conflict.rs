//! 重名冲突提示（P2，第 65 步 b）：镜像 `app/view/dialogs/name_conflict.py`
//! （82 行）与 `app/services/name_conflict_queue.py`（81 行）的语义层。
//!
//! ## 实证口径（2026-10-17 实读源码 + venv 实跑，TZ=UTC、`translate`=identity）
//!
//! - **标题**：`下载目录中已有 {takenPath.name}`（`Path.name` 口径：去掉尾部
//!   `/` 后取最后一段；`""`/`"/"` → `""`）。
//! - **正文**（`toContentText`，:16-22）三行，`\n` 连接：
//!   1. 文件夹 → `现有文件夹：{modified} 修改`；
//!      文件 → `现有文件：{toReadableSize(existingSize)}，{modified} 修改`；
//!   2. `新文件：{size}`，其中 `size = toReadableSize(task.fileSize)`，
//!      **只有 `task.fileSize > 0` 才算**，否则 `未知大小`；
//!   3. `覆盖时，旧文件会移到回收站。`
//!      `modified = datetime.fromtimestamp(modifiedAt).strftime("%Y-%m-%d %H:%M")`
//!      —— 本地时区、无秒，故复用 [`drawer::to_time_text`]（带秒）取前 16 字节。
//! - **venv 实跑的四组正文**（`utc_offset_seconds = 0`）：
//!   - `(isFolder=True, existing=0, modified=1700000000, taskSize=0)` →
//!     `现有文件夹：2023-11-14 22:13 修改\n新文件：未知大小\n覆盖时，旧文件会移到回收站。`
//!   - `(False, 4096, 1700000000, 1048576)` →
//!     `现有文件：4.00 KB，2023-11-14 22:13 修改\n新文件：1.00 MB\n覆盖时，旧文件会移到回收站。`
//!   - `(False, 0, 0, 0)` →
//!     `现有文件：0.00 B，1970-01-01 00:00 修改\n新文件：未知大小\n覆盖时，旧文件会移到回收站。`
//!   - `(False, 1536, 1700000123, 512)` →
//!     `现有文件：1.50 KB，2023-11-14 22:15 修改\n新文件：512.00 B\n覆盖时，旧文件会移到回收站。`
//! - **按钮**：`yesButton` = `保留两者`、`cancelButton` = `取消`、
//!   `overwriteButton` = `覆盖`（`buttonLayout.insertWidget(1, ..., 1)`）；
//!   标题栏隐藏（`setTitleBarVisible(False)`）。
//! - **「对其余」复选框**（:40-41）：文案
//!   `对其余 {restCount} 个冲突执行相同操作`（**构造时**就用当时的 restCount
//!   格式化，restCount=0 也照样格式化），可见性 `restCount > 0`；缺省未勾选。
//! - **默认选择**：`_choice = KEEP_BOTH`；点「覆盖」→ `OVERWRITE` + `accept()`。
//!   `_onFinished`（:63-69）：Accepted → `queue.setChoice(taskId, choice,
//!   restCheckBox.isChecked())`；否则 `queue.cancel(taskId)`。
//! - **队列**（`name_conflict_queue.py`）：
//!   - `add(task)`：入队；**仅当** `conflict is None && !isRefreshPosted` 时置位
//!     并 `post(self._refresh)`（本层用 [`ConflictEvent::RefreshPosted`] 表达
//!     「调用方须自己调 `refresh()`」）。
//!   - `setChoice(taskId, choice, isAppliedToRest)`：`conflict is None` 或
//!     `taskId` 不匹配 → **直接 return（不 refresh、不 add）**；命中则先
//!     `add(conflict.task, choice)`，若 `isAppliedToRest` 再按 `restCount`
//!     切前段逐条 `add(task, choice)`（`rest` 是切出来的前段、队列只留后段），
//!     末尾 `_refresh()`。
//!   - `cancel(taskId)`：同样先校验再 `_refresh()`。
//!   - `_refresh()`：清 `isRefreshPosted`、清 `conflict`；循环 `pop(0)`：
//!     `probeConflict(task)` 为 `None` → `add(task, KEEP_BOTH)` 继续；
//!     命中 → 记 `conflict`（`restCount = 剩余队列长度`）并 break；
//!     末尾**无条件** `conflictChanged.emit(self._conflict)`（可能为 None）。
//!   - `modifiedAt = int(info.st_mtime)`（向零截断），`existingSize = st_size`、
//!     `isFolder = stat.S_ISDIR(st_mode)` —— 本层让 `probe` 直接给这些值，
//!     但把 `st_mtime` 保留成 `f64`，截断在队列里做，保证 `int()` 口径。
//! - **`NameConflictPrompt`**（:72-82）：`conflictChanged(None)` 直接忽略；
//!   非 None 就建对话框、加入集合、`show()` + `raiseWindow()`；对话框
//!   `destroyed` 时移出集合。本层返回 [`DialogRequest`]（`raise_window` 由外壳
//!   执行，UI 语义层不做副作用）。
//!
//! 与 Qt 的差异：`QDialog` 的生命周期换成「返回 [`DialogOutcome`] 让外壳去调
//! 队列」；`Signal` 换成返回事件 `Vec`。

use crate::drawer::to_time_text;
use crate::format::to_readable_size;
use std::collections::{BTreeSet, VecDeque};

// ---------------------------------------------------------------------------
// 文案（`name_conflict.py` 逐字，translate 上下文一律 `NameConflictDialog`）
// ---------------------------------------------------------------------------

/// `QCoreApplication.translate("NameConflictDialog", ...)` 的上下文名。
pub const TRANSLATE_CONTEXT: &str = "NameConflictDialog";
/// `"下载目录中已有 {0}"`（:28）
pub const TITLE_TEMPLATE: &str = "下载目录中已有 {0}";
/// `"现有文件夹：{0} 修改"`（:18）
pub const EXISTING_FOLDER_TEMPLATE: &str = "现有文件夹：{0} 修改";
/// `"现有文件：{0}，{1} 修改"`（:20）
pub const EXISTING_FILE_TEMPLATE: &str = "现有文件：{0}，{1} 修改";
/// `"未知大小"`（:21）
pub const UNKNOWN_SIZE: &str = "未知大小";
/// `"新文件：{0}"`（:22）
pub const NEW_FILE_TEMPLATE: &str = "新文件：{0}";
/// `"覆盖时，旧文件会移到回收站。"`（:22）
pub const OVERWRITE_NOTE: &str = "覆盖时，旧文件会移到回收站。";
/// `yesButton.setText(...)`（:36）
pub const BUTTON_KEEP_BOTH: &str = "保留两者";
/// `cancelButton.setText(...)`（:37）
pub const BUTTON_CANCEL: &str = "取消";
/// `PushButton(...)`（:38）
pub const BUTTON_OVERWRITE: &str = "覆盖";
/// `"对其余 {0} 个冲突执行相同操作"`（:40）
pub const REST_CHECKBOX_TEMPLATE: &str = "对其余 {0} 个冲突执行相同操作";

// ---------------------------------------------------------------------------
// 选择
// ---------------------------------------------------------------------------

/// `task_service.py:32-35` 的 `NameConflictChoice(StrEnum)`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameConflictChoice {
    KeepBoth,
    Overwrite,
    Ask,
}

impl NameConflictChoice {
    /// StrEnum 的值（写进 `tasks.jsonl` / 配置里的口径）。
    pub fn as_str(self) -> &'static str {
        match self {
            NameConflictChoice::KeepBoth => "keepBoth",
            NameConflictChoice::Overwrite => "overwrite",
            NameConflictChoice::Ask => "ask",
        }
    }

    /// `NameConflictChoice(value)`；未知值 → None（Python 抛 `ValueError`）。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "keepBoth" => Some(NameConflictChoice::KeepBoth),
            "overwrite" => Some(NameConflictChoice::Overwrite),
            "ask" => Some(NameConflictChoice::Ask),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// 冲突数据
// ---------------------------------------------------------------------------

/// 队列里排队的任务在视图层用到的两列（`task.taskId` / `task.fileSize`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictTask {
    pub task_id: String,
    pub file_size: i64,
}

impl ConflictTask {
    pub fn new(task_id: impl Into<String>, file_size: i64) -> Self {
        ConflictTask {
            task_id: task_id.into(),
            file_size,
        }
    }
}

/// `probeConflict` 的产物 + `takenPath.stat()` 的四列。
///
/// `modified_at` 保留 `st_mtime` 的浮点形态，截断由队列按 `int()` 口径做。
#[derive(Clone, Debug, PartialEq)]
pub struct ProbeTarget {
    /// `takenPath`（对话框只用它的 `name`）。
    pub path: String,
    /// `stat.S_ISDIR(info.st_mode)`
    pub is_folder: bool,
    /// `info.st_size`
    pub existing_size: i64,
    /// `info.st_mtime`（秒，浮点）
    pub modified_at: f64,
}

/// `NameConflict`（frozen dataclass，:19-25）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameConflict {
    pub task: ConflictTask,
    pub taken_path: String,
    pub is_folder: bool,
    pub existing_size: i64,
    pub modified_at: i64,
    /// `len(self._tasks)`（**探测时**剩余队列长度）。
    pub rest_count: usize,
}

/// 镜像 `Path(path).name`：去掉尾部 `/` 后取最后一段。
pub fn path_name(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rsplit_once('/') {
        Some((_, tail)) => tail.to_string(),
        None => trimmed.to_string(),
    }
}

impl NameConflict {
    /// `"下载目录中已有 {0}".format(conflict.takenPath.name)`
    pub fn title_text(&self) -> String {
        title_text(&self.taken_path)
    }

    /// `toContentText(conflict)`，`utc_offset_seconds` 是本地时区偏移。
    pub fn content_text(&self, utc_offset_seconds: i64) -> String {
        content_text(self, utc_offset_seconds)
    }
}

/// 标题（独立函数便于视图层直接调用）。
pub fn title_text(taken_path: &str) -> String {
    TITLE_TEMPLATE.replace("{0}", &path_name(taken_path))
}

/// `toContentText(conflict)`（:16-22）。
pub fn content_text(conflict: &NameConflict, utc_offset_seconds: i64) -> String {
    let full = to_time_text(conflict.modified_at, utc_offset_seconds);
    // `strftime("%Y-%m-%d %H:%M")`：砍掉秒。`to_time_text` 全 ASCII，前 16 字节安全。
    let modified = full.get(..16).unwrap_or(&full);
    let existing = if conflict.is_folder {
        EXISTING_FOLDER_TEMPLATE.replace("{0}", modified)
    } else {
        EXISTING_FILE_TEMPLATE
            .replace("{0}", &to_readable_size(conflict.existing_size as f64))
            .replace("{1}", modified)
    };
    let new_size = if conflict.task.file_size > 0 {
        to_readable_size(conflict.task.file_size as f64)
    } else {
        UNKNOWN_SIZE.to_string()
    };
    [
        existing,
        NEW_FILE_TEMPLATE.replace("{0}", &new_size),
        OVERWRITE_NOTE.to_string(),
    ]
    .join("\n")
}

/// `"对其余 {0} 个冲突执行相同操作".format(restCount)`
pub fn rest_checkbox_text(rest_count: usize) -> String {
    REST_CHECKBOX_TEMPLATE.replace("{0}", &rest_count.to_string())
}

/// `restCheckBox.setVisible(restCount > 0)`（:41）
pub fn rest_checkbox_visible(rest_count: usize) -> bool {
    rest_count > 0
}

// ---------------------------------------------------------------------------
// 对话框状态
// ---------------------------------------------------------------------------

/// `_onFinished` 的结果：外壳拿它去调 [`NameConflictQueue`]。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogOutcome {
    /// `queue.setChoice(taskId, choice, isAppliedToRest)`
    SetChoice {
        task_id: String,
        choice: NameConflictChoice,
        is_applied_to_rest: bool,
    },
    /// `queue.cancel(taskId)`
    Cancel { task_id: String },
}

/// `NameConflictDialog` 的状态（控件文本 + 选择 + 复选框）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameConflictDialogState {
    pub conflict: NameConflict,
    /// `self._choice`，缺省 `KEEP_BOTH`（:31）。
    pub choice: NameConflictChoice,
    /// `restCheckBox.isChecked()`，Qt 缺省未勾选。
    pub rest_checked: bool,
}

impl NameConflictDialogState {
    /// `__init__` 的控件初始态。
    pub fn new(conflict: NameConflict) -> Self {
        NameConflictDialogState {
            conflict,
            choice: NameConflictChoice::KeepBoth,
            rest_checked: false,
        }
    }

    /// `self.setTitleBarVisible(False)`（:35）——外壳据此隐藏标题栏。
    pub fn title_bar_visible(&self) -> bool {
        false
    }

    /// `Dialog(title, content)` 的标题。
    pub fn title_text(&self) -> String {
        self.conflict.title_text()
    }

    /// `Dialog(title, content)` 的正文。
    pub fn content_text(&self, utc_offset_seconds: i64) -> String {
        self.conflict.content_text(utc_offset_seconds)
    }

    /// `restCheckBox` 文案（构造时定死，见模块头）。
    pub fn rest_checkbox_text(&self) -> String {
        rest_checkbox_text(self.conflict.rest_count)
    }

    /// `restCheckBox.setVisible(...)`。
    pub fn rest_checkbox_visible(&self) -> bool {
        rest_checkbox_visible(self.conflict.rest_count)
    }

    /// `_onOverwriteClicked`：置 OVERWRITE（随后 `accept()` 由外壳调 `finish`）。
    pub fn choose_overwrite(&mut self) {
        self.choice = NameConflictChoice::Overwrite;
    }

    /// 用户切复选框（Qt 侧由控件自己维护，这里显式建模）。
    pub fn set_rest_checked(&mut self, checked: bool) {
        self.rest_checked = checked;
    }

    /// `_onFinished`（:63-69）：`accepted` = `result() == DialogCode.Accepted`。
    pub fn finish(&self, accepted: bool) -> DialogOutcome {
        let task_id = self.conflict.task.task_id.clone();
        if accepted {
            DialogOutcome::SetChoice {
                task_id,
                choice: self.choice,
                is_applied_to_rest: self.rest_checked,
            }
        } else {
            DialogOutcome::Cancel { task_id }
        }
    }
}

// ---------------------------------------------------------------------------
// 队列
// ---------------------------------------------------------------------------

/// 队列对引擎侧的两处注入（Python 的 `probeConflict` / `add`）。
pub trait ConflictActions {
    /// `probeConflict(task) -> Path | None` + `takenPath.stat()`。
    fn probe(&mut self, task: &ConflictTask) -> Option<ProbeTarget>;
    /// `add(task, choice=...) -> bool`（返回值 Python 侧被忽略）。
    fn add(&mut self, task: &ConflictTask, choice: NameConflictChoice) -> bool;
}

/// `NameConflictQueue` 对外产生的事件（`Signal` 的替代）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConflictEvent {
    /// `self._post(self._refresh)`：**调用方须自己调 [`NameConflictQueue::refresh`]**。
    RefreshPosted,
    /// `self.conflictChanged.emit(self._conflict)`（`None` 也发）。
    ConflictChanged(Option<NameConflict>),
}

/// `NameConflictQueue`（`name_conflict_queue.py:29-79`）。
#[derive(Debug, Default)]
pub struct NameConflictQueue {
    tasks: VecDeque<ConflictTask>,
    conflict: Option<NameConflict>,
    is_refresh_posted: bool,
}

impl NameConflictQueue {
    pub fn new() -> Self {
        NameConflictQueue::default()
    }

    /// 当前待用户决策的冲突（`self._conflict`）。
    pub fn conflict(&self) -> Option<&NameConflict> {
        self.conflict.as_ref()
    }

    /// 队列长度（不含已取出的当前冲突任务）。
    pub fn pending(&self) -> usize {
        self.tasks.len()
    }

    /// `add(task)`（:37-41）。
    pub fn add<A: ConflictActions>(
        &mut self,
        _actions: &mut A,
        task: ConflictTask,
    ) -> Vec<ConflictEvent> {
        self.tasks.push_back(task);
        if self.conflict.is_none() && !self.is_refresh_posted {
            self.is_refresh_posted = true;
            return vec![ConflictEvent::RefreshPosted];
        }
        Vec::new()
    }

    /// `setChoice(taskId, choice, isAppliedToRest)`（:43-52）。
    pub fn set_choice<A: ConflictActions>(
        &mut self,
        actions: &mut A,
        task_id: &str,
        choice: NameConflictChoice,
        is_applied_to_rest: bool,
    ) -> Vec<ConflictEvent> {
        let Some(conflict) = self.conflict.clone() else {
            return Vec::new();
        };
        if conflict.task.task_id != task_id {
            return Vec::new();
        }
        actions.add(&conflict.task, choice);
        if is_applied_to_rest {
            // `rest, self._tasks = self._tasks[:restCount], self._tasks[restCount:]`
            let keep_from = conflict.rest_count.min(self.tasks.len());
            let rest: Vec<ConflictTask> = self.tasks.drain(..keep_from).collect();
            for task in rest {
                actions.add(&task, choice);
            }
        }
        self.refresh(actions)
    }

    /// `cancel(taskId)`（:54-57）。
    pub fn cancel<A: ConflictActions>(
        &mut self,
        actions: &mut A,
        task_id: &str,
    ) -> Vec<ConflictEvent> {
        let Some(conflict) = self.conflict.as_ref() else {
            return Vec::new();
        };
        if conflict.task.task_id != task_id {
            return Vec::new();
        }
        self.refresh(actions)
    }

    /// `_refresh()`（:59-71）：末尾无条件 emit（可能为 None）。
    pub fn refresh<A: ConflictActions>(&mut self, actions: &mut A) -> Vec<ConflictEvent> {
        self.is_refresh_posted = false;
        self.conflict = None;
        while let Some(task) = self.tasks.pop_front() {
            let Some(target) = actions.probe(&task) else {
                actions.add(&task, NameConflictChoice::KeepBoth);
                continue;
            };
            self.conflict = Some(NameConflict {
                task,
                taken_path: target.path,
                is_folder: target.is_folder,
                existing_size: target.existing_size,
                // `int(info.st_mtime)`：向零截断（`as` 对 f64→i64 同样向零）。
                modified_at: target.modified_at as i64,
                rest_count: self.tasks.len(),
            });
            break;
        }
        vec![ConflictEvent::ConflictChanged(self.conflict.clone())]
    }
}

// ---------------------------------------------------------------------------
// 提示器
// ---------------------------------------------------------------------------

/// `NameConflictPrompt` 要外壳做的事（建窗 + `show()` + `raiseWindow()`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DialogRequest {
    /// 本层分配的对话框 id（Qt 侧是对象身份）。
    pub dialog_id: u64,
    pub state: NameConflictDialogState,
    /// `raiseWindow(dialog)`：外壳层执行（本层不做副作用）。
    pub raise_window: bool,
}

/// `NameConflictPrompt`（:72-82）。
#[derive(Debug, Default)]
pub struct NameConflictPrompt {
    next_dialog_id: u64,
    open: BTreeSet<u64>,
}

impl NameConflictPrompt {
    pub fn new() -> Self {
        NameConflictPrompt::default()
    }

    /// 当前打开的对话框数（`len(self._dialogs)`）。
    pub fn open_count(&self) -> usize {
        self.open.len()
    }

    /// `_onConflictChanged(conflict)`：`None` 直接忽略。
    pub fn on_conflict_changed(
        &mut self,
        conflict: Option<&NameConflict>,
    ) -> Option<DialogRequest> {
        let conflict = conflict?;
        let dialog_id = self.next_dialog_id;
        self.next_dialog_id += 1;
        self.open.insert(dialog_id);
        Some(DialogRequest {
            dialog_id,
            state: NameConflictDialogState::new(conflict.clone()),
            raise_window: true,
        })
    }

    /// `dialog.destroyed` → `self._dialogs.discard(dialog)`。
    pub fn close(&mut self, dialog_id: u64) {
        self.open.remove(&dialog_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conflict(
        task_id: &str,
        task_size: i64,
        is_folder: bool,
        existing_size: i64,
        modified_at: i64,
        rest_count: usize,
    ) -> NameConflict {
        NameConflict {
            task: ConflictTask::new(task_id, task_size),
            taken_path: format!("/dl/{task_id}.bin"),
            is_folder,
            existing_size,
            modified_at,
            rest_count,
        }
    }

    /// 四组正文与 venv 实跑逐字节一致（`utc_offset_seconds = 0`）。
    #[test]
    fn content_text_matches_python() {
        let cases: [(NameConflict, &str); 4] = [
            (
                conflict("t1", 0, true, 0, 1_700_000_000, 0),
                "现有文件夹：2023-11-14 22:13 修改\n新文件：未知大小\n覆盖时，旧文件会移到回收站。",
            ),
            (
                conflict("t2", 1_048_576, false, 4096, 1_700_000_000, 0),
                "现有文件：4.00 KB，2023-11-14 22:13 修改\n新文件：1.00 MB\n覆盖时，旧文件会移到回收站。",
            ),
            (
                conflict("t3", 0, false, 0, 0, 0),
                "现有文件：0.00 B，1970-01-01 00:00 修改\n新文件：未知大小\n覆盖时，旧文件会移到回收站。",
            ),
            (
                conflict("t4", 512, false, 1536, 1_700_000_123, 0),
                "现有文件：1.50 KB，2023-11-14 22:15 修改\n新文件：512.00 B\n覆盖时，旧文件会移到回收站。",
            ),
        ];
        for (item, expected) in cases {
            assert_eq!(
                item.content_text(0),
                expected,
                "content({})",
                item.task.task_id
            );
        }
    }

    /// 标题取 `Path(takenPath).name`；时区偏移只影响正文。
    #[test]
    fn title_uses_taken_path_name() {
        assert_eq!(title_text("/dl/video.mp4"), "下载目录中已有 video.mp4");
        assert_eq!(title_text("/dl/sub/"), "下载目录中已有 sub");
        assert_eq!(title_text("plain.bin"), "下载目录中已有 plain.bin");
        assert_eq!(path_name(""), "");
        assert_eq!(path_name("/"), "");
        let item = conflict("t1", 1, false, 1, 1_700_000_000, 0);
        assert_eq!(item.title_text(), "下载目录中已有 t1.bin");
        // +8 小时：22:13 UTC → 次日 06:13。
        assert!(item.content_text(8 * 3600).contains("2023-11-15 06:13"));
    }

    /// 复选框文案/可见性：0 个也照样格式化，但不可见。
    #[test]
    fn rest_checkbox_text_and_visibility() {
        assert_eq!(rest_checkbox_text(0), "对其余 0 个冲突执行相同操作");
        assert_eq!(rest_checkbox_text(3), "对其余 3 个冲突执行相同操作");
        assert!(!rest_checkbox_visible(0));
        assert!(rest_checkbox_visible(1));
        let state = NameConflictDialogState::new(conflict("t1", 1, false, 1, 0, 2));
        assert_eq!(state.rest_checkbox_text(), "对其余 2 个冲突执行相同操作");
        assert!(state.rest_checkbox_visible());
        assert!(!state.title_bar_visible());
    }

    /// `_onFinished` 两条路径 + 「覆盖」按钮的缺省选择。
    #[test]
    fn dialog_outcomes_mirror_on_finished() {
        let mut state = NameConflictDialogState::new(conflict("t1", 1, false, 1, 0, 2));
        assert_eq!(state.choice, NameConflictChoice::KeepBoth);
        assert_eq!(
            state.finish(true),
            DialogOutcome::SetChoice {
                task_id: "t1".into(),
                choice: NameConflictChoice::KeepBoth,
                is_applied_to_rest: false,
            }
        );
        state.set_rest_checked(true);
        assert_eq!(
            state.finish(true),
            DialogOutcome::SetChoice {
                task_id: "t1".into(),
                choice: NameConflictChoice::KeepBoth,
                is_applied_to_rest: true,
            }
        );
        state.choose_overwrite();
        assert_eq!(state.choice, NameConflictChoice::Overwrite);
        assert_eq!(
            state.finish(true),
            DialogOutcome::SetChoice {
                task_id: "t1".into(),
                choice: NameConflictChoice::Overwrite,
                is_applied_to_rest: true,
            }
        );
        assert_eq!(
            state.finish(false),
            DialogOutcome::Cancel {
                task_id: "t1".into()
            }
        );
    }

    #[test]
    fn choice_round_trips_through_strenum_values() {
        for choice in [
            NameConflictChoice::KeepBoth,
            NameConflictChoice::Overwrite,
            NameConflictChoice::Ask,
        ] {
            assert_eq!(NameConflictChoice::parse(choice.as_str()), Some(choice));
        }
        assert_eq!(NameConflictChoice::parse("keepboth"), None);
        assert_eq!(NameConflictChoice::KeepBoth.as_str(), "keepBoth");
        assert_eq!(NameConflictChoice::Overwrite.as_str(), "overwrite");
        assert_eq!(NameConflictChoice::Ask.as_str(), "ask");
    }

    // -- 队列 ---------------------------------------------------------------

    /// 记录 `probe`/`add` 调用的假引擎侧。
    struct Fake {
        conflicts: Vec<(String, ProbeTarget)>,
        adds: Vec<(String, NameConflictChoice)>,
        probes: Vec<String>,
    }

    impl Fake {
        fn new(conflicts: Vec<(&str, ProbeTarget)>) -> Self {
            Fake {
                conflicts: conflicts
                    .into_iter()
                    .map(|(id, target)| (id.to_string(), target))
                    .collect(),
                adds: Vec::new(),
                probes: Vec::new(),
            }
        }

        fn adds(&self) -> Vec<(String, &'static str)> {
            self.adds
                .iter()
                .map(|(id, choice)| (id.clone(), choice.as_str()))
                .collect()
        }
    }

    impl ConflictActions for Fake {
        fn probe(&mut self, task: &ConflictTask) -> Option<ProbeTarget> {
            self.probes.push(task.task_id.clone());
            self.conflicts
                .iter()
                .find(|(id, _)| *id == task.task_id)
                .map(|(_, target)| target.clone())
        }

        fn add(&mut self, task: &ConflictTask, choice: NameConflictChoice) -> bool {
            self.adds.push((task.task_id.clone(), choice));
            true
        }
    }

    fn target(path: &str, is_folder: bool, size: i64, mtime: f64) -> ProbeTarget {
        ProbeTarget {
            path: path.into(),
            is_folder,
            existing_size: size,
            modified_at: mtime,
        }
    }

    /// `add` 只在「无冲突且未 post」时 post 一次；`refresh` 末尾无条件 emit。
    #[test]
    fn add_posts_refresh_once_and_refresh_emits() {
        let mut queue = NameConflictQueue::new();
        let mut fake = Fake::new(vec![("t2", target("/dl/t2.bin", false, 10, 1.9))]);
        assert_eq!(
            queue.add(&mut fake, ConflictTask::new("t1", 1)),
            vec![ConflictEvent::RefreshPosted]
        );
        assert_eq!(queue.add(&mut fake, ConflictTask::new("t2", 2)), Vec::new());
        assert_eq!(queue.pending(), 2);
        let events = queue.refresh(&mut fake);
        // t1 无冲突 → KEEP_BOTH 直接放行；t2 命中 → restCount = 0。
        assert_eq!(fake.adds(), vec![("t1".to_string(), "keepBoth")]);
        assert_eq!(fake.probes, vec!["t1", "t2"]);
        let Some(ConflictEvent::ConflictChanged(Some(item))) = events.first() else {
            panic!("事件应为 ConflictChanged(Some)：{events:?}");
        };
        assert_eq!(events.len(), 1);
        assert_eq!(item.task.task_id, "t2");
        assert_eq!(item.taken_path, "/dl/t2.bin");
        assert_eq!(item.existing_size, 10);
        // `int(1.9)` = 1（向零截断）。
        assert_eq!(item.modified_at, 1);
        assert_eq!(item.rest_count, 0);
        assert_eq!(queue.conflict(), Some(item));
        // 已有 conflict 时 add 只入队、不 post（`_isRefreshPosted` 已清，但冲突还在）。
        assert_eq!(
            queue.add(&mut fake, ConflictTask::new("t3", 3)),
            Vec::new(),
            "已有 conflict 时不 post"
        );
    }

    /// `refresh` 全部无冲突 → `ConflictChanged(None)`，且全部按 KEEP_BOTH 放行。
    #[test]
    fn refresh_without_conflicts_emits_none() {
        let mut queue = NameConflictQueue::new();
        let mut fake = Fake::new(vec![]);
        queue.add(&mut fake, ConflictTask::new("t1", 1));
        queue.add(&mut fake, ConflictTask::new("t2", 2));
        let events = queue.refresh(&mut fake);
        assert_eq!(events, vec![ConflictEvent::ConflictChanged(None)]);
        assert_eq!(
            fake.adds(),
            vec![
                ("t1".to_string(), "keepBoth"),
                ("t2".to_string(), "keepBoth"),
            ]
        );
        assert_eq!(queue.pending(), 0);
        assert_eq!(queue.conflict(), None);
        // 冲突清空后 add 会重新 post。
        assert_eq!(
            queue.add(&mut fake, ConflictTask::new("t3", 3)),
            vec![ConflictEvent::RefreshPosted]
        );
    }

    /// `setChoice` 的 taskId 不匹配/无冲突 → 直接 return（不 add、不 refresh）。
    #[test]
    fn set_choice_ignores_unknown_task() {
        let mut queue = NameConflictQueue::new();
        let mut fake = Fake::new(vec![("t1", target("/dl/t1.bin", false, 10, 1.0))]);
        queue.add(&mut fake, ConflictTask::new("t1", 1));
        queue.add(&mut fake, ConflictTask::new("t2", 2));
        queue.refresh(&mut fake);
        fake.adds.clear();
        fake.probes.clear();
        assert_eq!(
            queue.set_choice(&mut fake, "t9", NameConflictChoice::Overwrite, true),
            Vec::new()
        );
        assert!(fake.adds.is_empty());
        assert!(fake.probes.is_empty());
        assert_eq!(queue.pending(), 1);
        assert!(queue.conflict().is_some());
        // 无冲突时同样直接 return。
        let mut empty = NameConflictQueue::new();
        assert_eq!(
            empty.set_choice(&mut fake, "t1", NameConflictChoice::KeepBoth, true),
            Vec::new()
        );
        assert!(fake.adds.is_empty());
    }

    /// `setChoice` 命中且不套用到其余 → 只 add 当前任务，然后 refresh 出下一个冲突。
    #[test]
    fn set_choice_advances_to_next_conflict() {
        let mut queue = NameConflictQueue::new();
        let mut fake = Fake::new(vec![
            ("t1", target("/dl/t1.bin", false, 10, 1.0)),
            ("t3", target("/dl/t3.bin", true, 4096, 2.0)),
        ]);
        for id in ["t1", "t2", "t3"] {
            queue.add(&mut fake, ConflictTask::new(id, 1));
        }
        queue.refresh(&mut fake);
        assert_eq!(queue.conflict().unwrap().task.task_id, "t1");
        assert_eq!(queue.conflict().unwrap().rest_count, 2);
        let events = queue.set_choice(&mut fake, "t1", NameConflictChoice::Overwrite, false);
        // t2 无冲突 → KEEP_BOTH；t3 命中 → 成为新冲突（restCount = 0）。
        assert_eq!(
            fake.adds(),
            vec![
                ("t1".to_string(), "overwrite"),
                ("t2".to_string(), "keepBoth"),
            ]
        );
        assert_eq!(events.len(), 1);
        let Some(ConflictEvent::ConflictChanged(Some(item))) = events.first() else {
            panic!("事件应为 ConflictChanged(Some)：{events:?}");
        };
        assert_eq!(item.task.task_id, "t3");
        assert!(item.is_folder);
        assert_eq!(item.rest_count, 0);
    }

    /// `isAppliedToRest` 时按 `restCount` 切前段逐条 add（顺序 = 队列顺序）。
    #[test]
    fn set_choice_applies_to_rest_in_queue_order() {
        let mut queue = NameConflictQueue::new();
        let mut fake = Fake::new(vec![("t1", target("/dl/t1.bin", false, 10, 1.0))]);
        for id in ["t1", "t2", "t3", "t4"] {
            queue.add(&mut fake, ConflictTask::new(id, 1));
        }
        queue.refresh(&mut fake);
        assert_eq!(queue.conflict().unwrap().rest_count, 3);
        let events = queue.set_choice(&mut fake, "t1", NameConflictChoice::KeepBoth, true);
        assert_eq!(
            fake.adds(),
            vec![
                ("t1".to_string(), "keepBoth"),
                ("t2".to_string(), "keepBoth"),
                ("t3".to_string(), "keepBoth"),
                ("t4".to_string(), "keepBoth"),
            ]
        );
        // 队列已被切空 → emit None。
        assert_eq!(events, vec![ConflictEvent::ConflictChanged(None)]);
        assert_eq!(queue.pending(), 0);
        assert_eq!(queue.conflict(), None);
    }

    /// `cancel` 命中 → 只 refresh（不 add 当前冲突任务）；不匹配 → 什么都不做。
    #[test]
    fn cancel_only_refreshes() {
        let mut queue = NameConflictQueue::new();
        let mut fake = Fake::new(vec![
            ("t1", target("/dl/t1.bin", false, 10, 1.0)),
            ("t2", target("/dl/t2.bin", false, 20, 2.0)),
        ]);
        queue.add(&mut fake, ConflictTask::new("t1", 1));
        queue.add(&mut fake, ConflictTask::new("t2", 2));
        queue.refresh(&mut fake);
        assert_eq!(
            queue.cancel(&mut fake, "nope"),
            Vec::new(),
            "taskId 不匹配 → 不 refresh"
        );
        let events = queue.cancel(&mut fake, "t1");
        // t1 被丢弃（未 add），t2 命中成为新冲突。
        assert!(fake.adds().is_empty());
        let Some(ConflictEvent::ConflictChanged(Some(item))) = events.first() else {
            panic!("事件应为 ConflictChanged(Some)：{events:?}");
        };
        assert_eq!(item.task.task_id, "t2");
        assert_eq!(item.modified_at, 2);
    }

    /// `NameConflictPrompt`：None 忽略、非 None 建窗并 raise；close 减一。
    #[test]
    fn prompt_opens_dialog_per_conflict() {
        let mut prompt = NameConflictPrompt::new();
        assert_eq!(prompt.on_conflict_changed(None), None);
        assert_eq!(prompt.open_count(), 0);
        let item = conflict("t1", 1, false, 1, 0, 3);
        let request = prompt.on_conflict_changed(Some(&item)).expect("应建窗");
        assert_eq!(request.dialog_id, 0);
        assert!(request.raise_window);
        assert_eq!(request.state.conflict, item);
        assert_eq!(request.state.choice, NameConflictChoice::KeepBoth);
        assert_eq!(prompt.open_count(), 1);
        let second = prompt.on_conflict_changed(Some(&item)).expect("应再建窗");
        assert_eq!(second.dialog_id, 1);
        assert_eq!(prompt.open_count(), 2);
        prompt.close(0);
        assert_eq!(prompt.open_count(), 1);
        prompt.close(0);
        assert_eq!(prompt.open_count(), 1, "重复 close 幂等");
    }
}
