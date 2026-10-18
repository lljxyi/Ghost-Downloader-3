//! 编辑任务参数对话框（P3，第 66 步）：镜像 `app/view/dialogs/edit_task.py`
//! （127 行）的语义层。
//!
//! ## 实证口径（2026-10-17 实读源码）
//!
//! - **公共骨架**（`EditTaskDialog.__init__`，:21-41）：标题 `编辑任务参数`、
//!   `widget.setMinimumWidth(680)`、进度条**初始隐藏**、`yesButton` = `应用`、
//!   `cancelButton` = `取消`；卡片按传入顺序 `addCard`。
//! - **`DraftEditDialog.accept()`**（:44-47）：`task.setOptions(cardGroup.options())`
//!   后 `accept()` —— 建任务流程的草稿编辑，不重新解析。
//! - **`LiveEditDialog.accept()`**（:53-70）：
//!   `options = cardGroup.options()`；`newUrl = options.pop("url", "").strip()`
//!   —— 注意 **`pop` 会从 options 里删掉 url**，所以「url 没变」分支传给
//!   `taskService.edit` 的 options 里**没有 url 键**。
//!   - `newUrl` 为空或等于 `task.url` → 直接 `edit(task, options)` + `accept()`；
//!   - 否则 → `_setInteractive(False)`（进度条显示、`yesButton` 禁用）、
//!     `TaskOptions.fromOptions({**options, "url": newUrl})` 提交解析，
//!     记 `_pendingParseId`，并把 options 交给回调（Python 靠闭包捕获，
//!     Rust 侧存 `pending_options`）。
//! - **`reject()`**：先 `_cancelPendingParse()`（有 pending 才 `cancel` 并清空）
//!   再 `reject()`。
//! - **`_onReparsed(newTask, options)`**（:100-125）：清 pending；
//!   `task.canReuseProgress(newTask)` → `edit(task, options, newTask)` + accept；
//!   否则 `receivedBytes = task.currentSnapshot()[2]`，`> 0` 时弹
//!   `MessageBox("确认更换链接", "新链接与原链接的内容不一致，将清除已下载的
//!   {toReadableSize(receivedBytes)} 数据，是否继续？")`，用户不确认 →
//!   `_setInteractive(True)` 且**不关闭**；确认或 `receivedBytes == 0` →
//!   `edit(task, options, newTask)` + accept。
//! - **`_onReparseFailed`**（:126-136）：清 pending、`_setInteractive(True)`、
//!   弹 `InfoBar.error(标题="链接解析失败", 正文=toLocalizedError(toTaskError(error)),
//!   duration=4000, position=TOP)`。`toLocalizedError` 在 identity 口径下
//!   等于 `error.message.format_map(error.params)`（`app/i18n.py:5-8`），
//!   本层直接收已本地化好的文本（外壳负责翻译与参数替换）。
//!
//! 与 Qt 的差异：`accept()`/`reject()` 换成返回枚举，外壳据此调
//! `taskService.edit` / `coroutineRunner.submit|cancel` / 弹窗。

use crate::draft::Options;
use crate::format::to_readable_size;

// ---------------------------------------------------------------------------
// 文案与常量（`edit_task.py` 逐字）
// ---------------------------------------------------------------------------

/// `SubtitleLabel(self.tr("编辑任务参数"))`（:24）
pub const TITLE: &str = "编辑任务参数";
/// `yesButton.setText(self.tr("应用"))`（:31）
pub const BUTTON_APPLY: &str = "应用";
/// `cancelButton.setText(self.tr("取消"))`（:32）
pub const BUTTON_CANCEL: &str = "取消";
/// `self.widget.setMinimumWidth(680)`（:30）
pub const MIN_WIDTH: i32 = 680;
/// `MessageBox(self.tr("确认更换链接"), ...)`（:105）
pub const CONFIRM_TITLE: &str = "确认更换链接";
/// `"新链接与原链接的内容不一致，将清除已下载的 {0} 数据，是否继续？"`（:106-108）
pub const CONFIRM_TEMPLATE: &str =
    "新链接与原链接的内容不一致，将清除已下载的 {0} 数据，是否继续？";
/// `InfoBar.error(title=self.tr("链接解析失败"), ...)`（:130）
pub const PARSE_FAILED_TITLE: &str = "链接解析失败";
/// `duration=4000`（:133）
pub const INFO_BAR_DURATION_MS: u32 = 4000;

/// `options.pop("url", "").strip()`（:56）——**会从 options 里删掉 `url` 键**。
pub fn take_url(options: &mut Options) -> String {
    options.remove("url").unwrap_or_default().trim().to_string()
}

// ---------------------------------------------------------------------------
// 草稿编辑（`DraftEditDialog`）
// ---------------------------------------------------------------------------

/// `DraftEditDialog.accept()` 的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftAccept {
    /// `self._task.setOptions(...)` 的参数。
    pub set_options: Options,
    /// `super().accept()`。
    pub accepted: bool,
}

/// `DraftEditDialog` 的状态。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DraftEditState {
    pub accepted: bool,
}

impl DraftEditState {
    pub fn new() -> Self {
        DraftEditState::default()
    }

    /// `accept()`：写回卡片组选项并关闭。
    pub fn accept(&mut self, options: Options) -> DraftAccept {
        self.accepted = true;
        DraftAccept {
            set_options: options,
            accepted: true,
        }
    }
}

// ---------------------------------------------------------------------------
// 实时编辑（`LiveEditDialog`）
// ---------------------------------------------------------------------------

/// `LiveEditDialog.accept()` 的三条路径。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveAccept {
    /// `edit(task, options)` + `accept()`（url 为空或未变；options 已无 url 键）
    Edit { options: Options },
    /// 已提交解析（`_setInteractive(False)` 已生效）；外壳去等回调
    Parsing { parse_id: String, options: Options },
}

/// `_onReparsed` / `_onReparseFailed` 的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReparseOutcome {
    /// `edit(task, options, newTask)` + `accept()`
    EditAndClose { options: Options },
    /// 弹确认框；用户答复后调 [`LiveEditState::confirm_replacement`]
    Confirm { title: String, message: String },
    /// 用户放弃确认：`_setInteractive(True)`，对话框保持打开
    Restored,
    /// 解析失败：`_setInteractive(True)` + `InfoBar.error`
    ParseFailed {
        title: String,
        content: String,
        duration_ms: u32,
        at_top: bool,
    },
}

/// `LiveEditDialog` 的状态机。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveEditState {
    /// `self._task.url`
    pub task_url: String,
    /// `self._pendingParseId`（空串 = 无 pending）
    pub pending_parse_id: String,
    /// `_setInteractive` 的当前值（`true` = 可交互）
    pub interactive: bool,
    /// Python 用闭包捕获的 `options`（`_onReparsed(newTask, options)` 的第二参）
    pub pending_options: Option<Options>,
}

impl LiveEditState {
    pub fn new(task_url: impl Into<String>) -> Self {
        LiveEditState {
            task_url: task_url.into(),
            pending_parse_id: String::new(),
            interactive: true,
            pending_options: None,
        }
    }

    /// `_setInteractive(enabled)`（:94-96）：进度条可见性取反、`yesButton` 使能。
    pub fn set_interactive(&mut self, enabled: bool) {
        self.interactive = enabled;
    }

    /// 进度条是否可见（`self.progressBar.setVisible(not enabled)`）。
    pub fn progress_visible(&self) -> bool {
        !self.interactive
    }

    /// `yesButton` 是否可用（`self.yesButton.setEnabled(enabled)`）。
    pub fn yes_enabled(&self) -> bool {
        self.interactive
    }

    /// `accept()`（:53-70）。
    ///
    /// `submit` 对应 `coroutineRunner.submit(featureService.parse(taskOptions), ...)`，
    /// 返回 parseId。
    pub fn accept(
        &mut self,
        options: &mut Options,
        submit: impl FnOnce(&Options) -> String,
    ) -> LiveAccept {
        let new_url = take_url(options);
        if new_url.is_empty() || new_url == self.task_url {
            return LiveAccept::Edit {
                options: options.clone(),
            };
        }
        self.set_interactive(false);
        // `TaskOptions.fromOptions({**options, "url": newUrl})`
        let mut task_options = options.clone();
        task_options.insert("url".to_string(), new_url);
        let parse_id = submit(&task_options);
        self.pending_parse_id = parse_id.clone();
        self.pending_options = Some(options.clone());
        LiveAccept::Parsing {
            parse_id,
            options: options.clone(),
        }
    }

    /// `reject()` 里的 `_cancelPendingParse()`：返回是否真的取消了（供外壳调
    /// `coroutineRunner.cancel`）。
    pub fn cancel_pending_parse(&mut self) -> Option<String> {
        if self.pending_parse_id.is_empty() {
            return None;
        }
        let parse_id = std::mem::take(&mut self.pending_parse_id);
        self.pending_options = None;
        Some(parse_id)
    }

    /// `_onReparsed(newTask, options)`（:100-125）。
    ///
    /// `can_reuse_progress` = `self._task.canReuseProgress(newTask)`，
    /// `received_bytes` = `self._task.currentSnapshot()[2]`。
    pub fn on_reparsed(&mut self, can_reuse_progress: bool, received_bytes: i64) -> ReparseOutcome {
        self.pending_parse_id.clear();
        if can_reuse_progress {
            return self.take_edit_outcome();
        }
        if received_bytes > 0 {
            return ReparseOutcome::Confirm {
                title: CONFIRM_TITLE.to_string(),
                message: CONFIRM_TEMPLATE.replace("{0}", &to_readable_size(received_bytes as f64)),
            };
        }
        self.take_edit_outcome()
    }

    /// 确认框的答复（`if not confirm.exec(): self._setInteractive(True); return`）。
    pub fn confirm_replacement(&mut self, confirmed: bool) -> ReparseOutcome {
        if confirmed {
            return self.take_edit_outcome();
        }
        self.pending_options = None;
        self.set_interactive(true);
        ReparseOutcome::Restored
    }

    /// `_onReparseFailed(error, **_)`（:126-136）。
    pub fn on_parse_failed(&mut self, localized_error: &str) -> ReparseOutcome {
        self.pending_parse_id.clear();
        self.pending_options = None;
        self.set_interactive(true);
        ReparseOutcome::ParseFailed {
            title: PARSE_FAILED_TITLE.to_string(),
            content: localized_error.to_string(),
            duration_ms: INFO_BAR_DURATION_MS,
            at_top: true,
        }
    }

    fn take_edit_outcome(&mut self) -> ReparseOutcome {
        ReparseOutcome::EditAndClose {
            options: self.pending_options.take().unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(pairs: &[(&str, &str)]) -> Options {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn draft_accept_writes_options_back() {
        let mut state = DraftEditState::new();
        assert!(!state.accepted);
        let out = state.accept(mk(&[("url", "https://e/1")]));
        assert!(out.accepted);
        assert!(state.accepted);
        assert_eq!(
            out.set_options.get("url").map(String::as_str),
            Some("https://e/1")
        );
    }

    #[test]
    fn take_url_pops_and_strips() {
        let mut opts = mk(&[("url", "  https://e/1  "), ("name", "x")]);
        assert_eq!(take_url(&mut opts), "https://e/1");
        // `pop` 之后 options 里没有 url 键。
        assert!(!opts.contains_key("url"));
        assert_eq!(opts.len(), 1);
        // 缺失键 → 空串。
        assert_eq!(take_url(&mut opts), "");
    }

    /// 空 url / 同 url → 直接 edit（options 已无 url 键，交互态不变）。
    #[test]
    fn unchanged_url_edits_directly() {
        let mut state = LiveEditState::new("https://e/1");
        let mut opts = mk(&[("url", "https://e/1"), ("name", "x")]);
        let out = state.accept(&mut opts, |_| panic!("不应提交解析"));
        let LiveAccept::Edit { options } = out else {
            panic!("应为 Edit：{out:?}");
        };
        assert!(!options.contains_key("url"));
        assert_eq!(options.get("name").map(String::as_str), Some("x"));
        assert!(state.interactive);
        assert!(!state.progress_visible());
        assert!(state.yes_enabled());
        assert!(state.pending_parse_id.is_empty());

        // 空 url 同样走 Edit。
        let mut opts = mk(&[("url", "   ")]);
        let out = state.accept(&mut opts, |_| panic!("不应提交解析"));
        assert!(matches!(out, LiveAccept::Edit { .. }));
    }

    /// 新 url → 提交解析、禁用交互、记 pending（parse 收到的 options 带新 url）。
    #[test]
    fn changed_url_submits_parse() {
        let mut state = LiveEditState::new("https://e/1");
        let mut opts = mk(&[("url", "  https://e/2 "), ("name", "x")]);
        let mut submitted: Option<Options> = None;
        let out = state.accept(&mut opts, |task_options| {
            submitted = Some(task_options.clone());
            "parse-1".to_string()
        });
        let LiveAccept::Parsing { parse_id, options } = out else {
            panic!("应为 Parsing：{out:?}");
        };
        assert_eq!(parse_id, "parse-1");
        assert_eq!(state.pending_parse_id, "parse-1");
        assert!(!options.contains_key("url"));
        assert!(!state.interactive);
        assert!(state.progress_visible());
        assert!(!state.yes_enabled());
        // 提交给解析器的 options = 原 options + strip 后的 url。
        let submitted = submitted.expect("应提交解析");
        assert_eq!(
            submitted.get("url").map(String::as_str),
            Some("https://e/2")
        );
        assert_eq!(submitted.get("name").map(String::as_str), Some("x"));
        assert_eq!(state.pending_options.as_ref(), Some(&options));
    }

    /// `reject()` 只在有 pending 时取消一次。
    #[test]
    fn reject_cancels_pending_once() {
        let mut state = LiveEditState::new("https://e/1");
        assert_eq!(state.cancel_pending_parse(), None);
        let mut opts = mk(&[("url", "https://e/2")]);
        state.accept(&mut opts, |_| "parse-1".to_string());
        assert_eq!(state.cancel_pending_parse(), Some("parse-1".to_string()));
        assert_eq!(state.cancel_pending_parse(), None);
        assert!(state.pending_options.is_none());
        // 取消后交互态**不会**被恢复（Python `reject()` 直接关窗）。
        assert!(!state.interactive);
    }

    fn parsing_state(received_bytes: i64) -> LiveEditState {
        let mut state = LiveEditState::new("https://e/1");
        let mut opts = mk(&[("url", "https://e/2"), ("name", "x")]);
        state.accept(&mut opts, |_| "parse-1".to_string());
        let _ = received_bytes;
        state
    }

    /// 能复用进度 → 直接换任务关闭。
    #[test]
    fn reparsed_with_reusable_progress_closes() {
        let mut state = parsing_state(0);
        let out = state.on_reparsed(true, 4096);
        let ReparseOutcome::EditAndClose { options } = out else {
            panic!("应为 EditAndClose：{out:?}");
        };
        assert_eq!(options.get("name").map(String::as_str), Some("x"));
        assert!(!options.contains_key("url"));
        assert!(state.pending_parse_id.is_empty());
        assert!(state.pending_options.is_none());
    }

    /// 不能复用但已下载 0 字节 → 直接换任务关闭。
    #[test]
    fn reparsed_without_progress_closes() {
        let mut state = parsing_state(0);
        assert!(matches!(
            state.on_reparsed(false, 0),
            ReparseOutcome::EditAndClose { .. }
        ));
    }

    /// 不能复用且已下载 > 0 → 弹确认；拒绝则恢复交互、保留对话框。
    #[test]
    fn reparsed_with_downloaded_bytes_asks_confirmation() {
        let mut state = parsing_state(0);
        let out = state.on_reparsed(false, 1536);
        let ReparseOutcome::Confirm { title, message } = out else {
            panic!("应为 Confirm：{out:?}");
        };
        assert_eq!(title, "确认更换链接");
        assert_eq!(
            message,
            "新链接与原链接的内容不一致，将清除已下载的 1.50 KB 数据，是否继续？"
        );
        // 拒绝 → 恢复交互、选项丢弃、不关闭。
        assert_eq!(state.confirm_replacement(false), ReparseOutcome::Restored);
        assert!(state.interactive);
        assert!(state.yes_enabled());
        assert!(state.pending_options.is_none());
    }

    /// 确认 → 用同一份 options 换任务关闭。
    #[test]
    fn confirmed_replacement_closes_with_same_options() {
        let mut state = parsing_state(0);
        assert!(matches!(
            state.on_reparsed(false, 1),
            ReparseOutcome::Confirm { .. }
        ));
        let out = state.confirm_replacement(true);
        let ReparseOutcome::EditAndClose { options } = out else {
            panic!("应为 EditAndClose：{out:?}");
        };
        assert_eq!(options.get("name").map(String::as_str), Some("x"));
        assert!(state.pending_options.is_none());
    }

    /// 解析失败 → InfoBar.error 四要素 + 恢复交互 + 清 pending。
    #[test]
    fn parse_failure_restores_interaction() {
        let mut state = parsing_state(0);
        let out = state.on_parse_failed("无法下载 x/y/z");
        let ReparseOutcome::ParseFailed {
            title,
            content,
            duration_ms,
            at_top,
        } = out
        else {
            panic!("应为 ParseFailed：{out:?}");
        };
        assert_eq!(title, "链接解析失败");
        assert_eq!(content, "无法下载 x/y/z");
        assert_eq!(duration_ms, 4000);
        assert!(at_top);
        assert!(state.interactive);
        assert!(state.pending_parse_id.is_empty());
        assert!(state.pending_options.is_none());
    }

    #[test]
    fn skeleton_constants_match_python() {
        assert_eq!(TITLE, "编辑任务参数");
        assert_eq!(BUTTON_APPLY, "应用");
        assert_eq!(BUTTON_CANCEL, "取消");
        assert_eq!(MIN_WIDTH, 680);
        assert_eq!(INFO_BAR_DURATION_MS, 4000);
    }
}
