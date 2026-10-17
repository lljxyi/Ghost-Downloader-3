//! 计划任务（P2，第 64 步）：镜像 `app/services/plan.py::Plan` /
//! `PlanAction` 与 `app/view/dialogs/plan_task.py` 的对话框语义。
//!
//! ## 实证口径（2026-10-17 实读源码）
//!
//! - `PlanAction` 是 `IntEnum`，取值 **SHUTDOWN=0 / RESTART=1 / SLEEP=2 /
//!   OPEN_FILE=3**，与对话框里单选按钮的添加顺序（关机、重启、睡眠、打开
//!   文件）一一对应；缺省勾选「关机」（`plan_task.py:38`）。
//! - `Plan.set(action, filePath, onCleared)` 只记状态；`clear()` 先清状态
//!   **再**调 `onCleared`（`plan.py:26-32`）；`trigger()` 先检查
//!   `action is None` → 返回、再检查 `allCompleted()` → 返回，然后**先
//!   `clear()` 再执行动作**（`plan.py:34-52`）——顺序很关键：关机前必须把
//!   状态清掉，否则重入会重复触发。
//! - `allCompleted` 在 `Ghost-Downloader-3.py:94` 注入为
//!   `lambda: taskService.runningCount() == 0`，由
//!   `taskService.tasksAllCompleted` 信号驱动 `plan.trigger`。
//! - 动作分发：`OPEN_FILE` 且 `filePath` 非空才 `openFile`，否则**什么都不做**
//!   （空路径不报错）；`RESTART`/`SLEEP`/其余 → `restart`/`sleep`/`shutdown`。
//! - 校验（`validate`）：只有「打开文件」且 `pathEdit.text().strip()` 为空时
//!   不允许确认；`selectedFilePath()` 返回的就是 **strip 后**的文本。
//! - 任务页按钮（`task_page.py:481-494`）：勾选态点击 → 打开对话框；对话框
//!   被接受 → `plan.set(action, path, onCleared=取消勾选)`；被拒绝 → 直接取消
//!   勾选；取消勾选态点击 → `plan.clear()`。
//!
//! 平台动作（关机/重启/睡眠/开文件）由 `gd3-core`/外壳层执行，本模块只返回
//! `PlanCommand`，不直接调系统 API（UI 层不做副作用）。

/// `app/services/plan.py::PlanAction`（数值必须与 Python 一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanAction {
    Shutdown = 0,
    Restart = 1,
    Sleep = 2,
    OpenFile = 3,
}

impl PlanAction {
    /// 单选按钮顺序（= 数值顺序）。
    pub const ALL: [PlanAction; 4] = [
        PlanAction::Shutdown,
        PlanAction::Restart,
        PlanAction::Sleep,
        PlanAction::OpenFile,
    ];

    pub fn as_i32(self) -> i32 {
        self as i32
    }

    pub fn from_i32(value: i32) -> Option<Self> {
        match value {
            0 => Some(PlanAction::Shutdown),
            1 => Some(PlanAction::Restart),
            2 => Some(PlanAction::Sleep),
            3 => Some(PlanAction::OpenFile),
            _ => None,
        }
    }

    /// 单选按钮文案（`plan_task.py:19-22`）。
    pub fn label(self) -> &'static str {
        match self {
            PlanAction::Shutdown => "关机",
            PlanAction::Restart => "重启",
            PlanAction::Sleep => "睡眠",
            PlanAction::OpenFile => "打开文件",
        }
    }
}

/// `Plan.clear()` 的回调（Python 是 `onCleared`，用来取消按钮勾选）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanEvent {
    /// `onCleared` 被调用（Python 只在注册过回调时才调）。
    Cleared,
}

/// 需要外壳层执行的平台动作（`plan.py:44-52` 的分发结果）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanCommand {
    pub action: PlanAction,
    /// 仅 `OpenFile` 有意义；`openFile` 只在非空时执行。
    pub file_path: String,
}

impl PlanCommand {
    /// Python 侧 `if filePath:` 才执行，所以空路径 = 无事可做。
    pub fn has_effect(&self) -> bool {
        match self.action {
            PlanAction::OpenFile => !self.file_path.is_empty(),
            _ => true,
        }
    }
}

/// `app/services/plan.py::Plan`。
#[derive(Debug, Clone, Default)]
pub struct Plan {
    action: Option<PlanAction>,
    file_path: String,
    /// Python 的 `self._onCleared is not None`。
    has_on_cleared: bool,
}

impl Plan {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn action(&self) -> Option<PlanAction> {
        self.action
    }

    pub fn file_path(&self) -> &str {
        &self.file_path
    }

    pub fn is_set(&self) -> bool {
        self.action.is_some()
    }

    /// `set(action, filePath, onCleared)`。
    pub fn set(&mut self, action: PlanAction, file_path: &str, on_cleared: bool) {
        self.action = Some(action);
        self.file_path = file_path.to_string();
        self.has_on_cleared = on_cleared;
    }

    /// `clear()`：先清状态，再（若有）发 `onCleared`。
    pub fn clear(&mut self) -> Vec<PlanEvent> {
        let had_callback = self.has_on_cleared;
        self.action = None;
        self.file_path.clear();
        self.has_on_cleared = false;
        if had_callback {
            vec![PlanEvent::Cleared]
        } else {
            Vec::new()
        }
    }

    /// `trigger()`：未设动作或还有任务在跑 → `None`；否则**先清空**再返回动作。
    pub fn trigger(&mut self, all_completed: bool) -> Option<PlanCommand> {
        let action = self.action?;
        if !all_completed {
            return None;
        }
        let file_path = std::mem::take(&mut self.file_path);
        let _ = self.clear();
        Some(PlanCommand { action, file_path })
    }
}

/// 对话框静态文案（`plan_task.py`）。
pub struct PlanDialogTexts;

impl PlanDialogTexts {
    pub const TITLE: &'static str = "设置计划任务";
    pub const CONTENT: &'static str = "所有任务完成后执行以下操作：";
    pub const ACCEPT: &'static str = "确认";
    pub const CANCEL: &'static str = "取消";
    pub const PATH_PLACEHOLDER: &'static str = "请选择要打开的文件";
    pub const BROWSE_TOOLTIP: &'static str = "选择文件";
    /// `self.widget.setMinimumWidth(420)`
    pub const MIN_WIDTH: i32 = 420;
}

/// `PlanTaskDialog.__init__` 的初始状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanDialogState {
    pub checked: PlanAction,
    pub file_path: String,
    /// 只有勾选「打开文件」时路径行才可见（`_onActionChanged`）。
    pub path_visible: bool,
}

impl Default for PlanDialogState {
    fn default() -> Self {
        Self {
            checked: PlanAction::Shutdown,
            file_path: String::new(),
            path_visible: false,
        }
    }
}

impl PlanDialogState {
    /// 切换单选（互斥组）：同步路径行可见性。
    pub fn set_checked(&mut self, action: PlanAction) {
        self.checked = action;
        self.path_visible = action == PlanAction::OpenFile;
    }

    /// `selectedAction()`：单选按钮 → 枚举（默认关机）。
    pub fn selected_action(&self) -> PlanAction {
        self.checked
    }

    /// `selectedFilePath()`：**strip 后**的文本。
    pub fn selected_file_path(&self) -> &str {
        self.file_path.trim()
    }

    /// `validate()`：只有「打开文件 + 空路径」会被拦。
    pub fn validate(&self) -> bool {
        if self.checked == PlanAction::OpenFile && self.selected_file_path().is_empty() {
            return false;
        }
        true
    }
}

/// 任务页「计划任务」按钮的点击结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanButtonOutcome {
    /// 需要弹出对话框（勾选态点击且当前没有计划 → 打开对话框）。
    OpenDialog,
    /// 对话框被接受：`plan.set(...)` 已生效。
    Accepted(PlanCommand),
    /// 对话框被拒绝 / 需要直接取消按钮勾选。
    Uncheck,
    /// 取消勾选态点击：`plan.clear()` 已生效（可能带 `Cleared` 事件）。
    Cleared(Vec<PlanEvent>),
}

/// 对话框的执行结果（喂给 `plan_button_clicked`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanDialogOutcome<'a> {
    /// 还没弹过：本次点击就是「弹对话框」。
    NotShown,
    /// 用户取消。
    Rejected,
    /// 用户确认。
    Accepted(&'a PlanDialogState),
}

/// `task_page.py:481-494` 的按钮逻辑（把对话框结果当参数传进来）。
pub fn plan_button_clicked(
    plan: &mut Plan,
    checked: bool,
    dialog_result: PlanDialogOutcome<'_>,
) -> PlanButtonOutcome {
    if !checked {
        // 取消勾选 → plan.clear()（按钮状态由 onCleared 回调复位）。
        return PlanButtonOutcome::Cleared(plan.clear());
    }

    match dialog_result {
        PlanDialogOutcome::NotShown => PlanButtonOutcome::OpenDialog,
        PlanDialogOutcome::Rejected => PlanButtonOutcome::Uncheck,
        PlanDialogOutcome::Accepted(state) => {
            if !state.validate() {
                return PlanButtonOutcome::Uncheck;
            }
            plan.set(state.selected_action(), state.selected_file_path(), true);
            PlanButtonOutcome::Accepted(PlanCommand {
                action: state.selected_action(),
                file_path: state.selected_file_path().to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_values_match_python_int_enum() {
        assert_eq!(PlanAction::Shutdown.as_i32(), 0);
        assert_eq!(PlanAction::Restart.as_i32(), 1);
        assert_eq!(PlanAction::Sleep.as_i32(), 2);
        assert_eq!(PlanAction::OpenFile.as_i32(), 3);
        for action in PlanAction::ALL {
            assert_eq!(PlanAction::from_i32(action.as_i32()), Some(action));
        }
        assert_eq!(PlanAction::from_i32(4), None);
        assert_eq!(PlanAction::from_i32(-1), None);
    }

    #[test]
    fn action_labels_follow_radio_order() {
        let labels: Vec<&str> = PlanAction::ALL.iter().map(|a| a.label()).collect();
        assert_eq!(labels, vec!["关机", "重启", "睡眠", "打开文件"]);
    }

    #[test]
    fn default_dialog_checks_shutdown_and_hides_path() {
        let state = PlanDialogState::default();
        assert_eq!(state.checked, PlanAction::Shutdown);
        assert!(!state.path_visible);
        assert!(state.validate());
    }

    #[test]
    fn selecting_open_file_shows_path_row_only() {
        let mut state = PlanDialogState::default();
        state.set_checked(PlanAction::OpenFile);
        assert!(state.path_visible);
        state.set_checked(PlanAction::Sleep);
        assert!(!state.path_visible);
    }

    #[test]
    fn validate_requires_path_only_for_open_file() {
        let mut state = PlanDialogState::default();
        state.set_checked(PlanAction::OpenFile);
        state.file_path = "   ".into();
        assert!(!state.validate(), "打开文件 + 空白路径必须拦住");
        state.file_path = "  /tmp/a.mp4 ".into();
        assert!(state.validate());
        assert_eq!(state.selected_file_path(), "/tmp/a.mp4");
    }

    #[test]
    fn clear_emits_event_only_when_callback_registered() {
        let mut plan = Plan::new();
        plan.set(PlanAction::Sleep, "", true);
        assert_eq!(plan.clear(), vec![PlanEvent::Cleared]);
        assert!(!plan.is_set());
        // 第二次没有回调可调。
        assert!(plan.clear().is_empty());
    }

    #[test]
    fn clear_without_callback_returns_nothing() {
        let mut plan = Plan::new();
        plan.set(PlanAction::Restart, "", false);
        assert!(plan.clear().is_empty());
        assert!(!plan.is_set());
    }

    #[test]
    fn trigger_noop_without_action_or_when_tasks_running() {
        let mut plan = Plan::new();
        assert_eq!(plan.trigger(true), None);

        plan.set(PlanAction::Shutdown, "", true);
        assert_eq!(plan.trigger(false), None, "还有任务在跑 → 不触发");
        assert!(plan.is_set(), "未触发时状态必须保留");

        let command = plan.trigger(true).expect("应触发");
        assert_eq!(command.action, PlanAction::Shutdown);
        assert!(command.has_effect());
        // trigger 内部先 clear：状态清空、回调事件被吞掉（Python 同样丢弃返回值）。
        assert!(!plan.is_set());
        assert!(plan.file_path().is_empty());
    }

    #[test]
    fn trigger_open_file_without_path_has_no_effect() {
        let mut plan = Plan::new();
        plan.set(PlanAction::OpenFile, "", true);
        let command = plan.trigger(true).expect("应触发");
        assert!(!command.has_effect(), "空路径不执行 openFile");
        assert_eq!(command.file_path, "");
    }

    #[test]
    fn trigger_open_file_with_path_keeps_path() {
        let mut plan = Plan::new();
        plan.set(PlanAction::OpenFile, "/tmp/a.mp4", true);
        let command = plan.trigger(true).unwrap();
        assert_eq!(command.action, PlanAction::OpenFile);
        assert_eq!(command.file_path, "/tmp/a.mp4");
        assert!(command.has_effect());
    }

    #[test]
    fn button_flow_matches_task_page() {
        let mut plan = Plan::new();

        // 勾选态第一次点击 → 弹对话框。
        assert_eq!(
            plan_button_clicked(&mut plan, true, PlanDialogOutcome::NotShown),
            PlanButtonOutcome::OpenDialog
        );

        // 对话框被拒绝 → 取消勾选。
        assert_eq!(
            plan_button_clicked(&mut plan, true, PlanDialogOutcome::Rejected),
            PlanButtonOutcome::Uncheck
        );

        // 被接受 → plan.set + 返回命令。
        let mut state = PlanDialogState::default();
        state.set_checked(PlanAction::Restart);
        assert_eq!(
            plan_button_clicked(&mut plan, true, PlanDialogOutcome::Accepted(&state)),
            PlanButtonOutcome::Accepted(PlanCommand {
                action: PlanAction::Restart,
                file_path: String::new(),
            })
        );
        assert_eq!(plan.action(), Some(PlanAction::Restart));

        // 取消勾选 → clear，且因为 set 时注册了 onCleared → 有 Cleared 事件。
        assert_eq!(
            plan_button_clicked(&mut plan, false, PlanDialogOutcome::NotShown),
            PlanButtonOutcome::Cleared(vec![PlanEvent::Cleared])
        );
        assert!(!plan.is_set());
    }

    #[test]
    fn accepted_open_file_without_path_is_rejected() {
        let mut plan = Plan::new();
        let mut state = PlanDialogState::default();
        state.set_checked(PlanAction::OpenFile);
        state.file_path = "   ".into();
        assert_eq!(
            plan_button_clicked(&mut plan, true, PlanDialogOutcome::Accepted(&state)),
            PlanButtonOutcome::Uncheck
        );
        assert!(!plan.is_set());
    }
}
