//! 卡片/抽屉动作到服务层的桥（第 60 步，P0 收口）。
//!
//! 前面几步把「点什么 → 该调哪个服务方法」算清楚了，但算出来的只是意图
//! （`card::ToggleAction` / `card_menu::CardClick`）。这一层把意图**真的落到
//! 服务调用**上，判定标准是 Python 侧的调用序列：
//!
//! - `_onToggleClicked`（`task_cards.py:363-372`）：按 `isSeeding` → `COMPLETED`
//!   → `RUNNING` → 其余 的短路链调 `stopSeeding` / `startSeeding` / `pause` /
//!   `start`，**然后 `self.refresh()`**（刷新归调用方，本层只回传分派结果）；
//! - `_onDeleteClicked`（`task_cards.py:377-385`）：`MessageBox("删除任务",
//!   "确定要删除这个下载任务吗？")` + `CheckBox("同时删除已下载的文件")`，
//!   勾选初值取 `cfg.shouldDeleteFilesOnRemove.value`（缺省 `False`）；确定后
//!   **先 `cfg.set(shouldDeleteFilesOnRemove, 勾选值)` 再 `delete(task, 勾选值)`**
//!   ——顺序有语义：用户的选择要落盘，即使删除本身失败；
//! - `_onChecksumClicked`（`task_cards.py:387-405`）：三条短路的结果
//!   （`card_menu::ChecksumOutcome`）分别落到 `cancelChecksum` / 置状态行 /
//!   弹菜单；
//! - `createContextMenu` 的七个动作（`task_cards.py:434-476`）→ `CardClick`。
//!
//! 为什么用 trait 而不是闭包集合：服务方法有 10 个、UI 钩子 3 个，逐个塞进
//! 结构体字段会让装配层写十几行 `Box::new(move |..|)`，且没有「少实现一个
//! 方法」的编译期保护。trait 让调用方（PyO3 适配层 / 未来的纯 Rust 外壳）
//! 只写一个 `impl`，缺方法直接编译失败。
//!
//! 分层边界：**本层不持任务对象**。Python 的服务方法收 `Task` 实例，Rust 侧
//! 传 `taskId`——记录本身归调用方（它才知道从哪个 store 里取），本层只保证
//! 「按 id 调哪个方法、参数是什么」。

use crate::card::{ToggleAction, toggle_action};
use crate::card_menu::{CardClick, ChecksumOutcome};
use crate::window::{BarButton, DIALOG_CANCEL, DIALOG_OK, DialogSpec};
use gd3_core::task::TaskStatus;

/// 卡片动作触达的任务服务面（镜像 `taskService` 的相关方法）。
///
/// 参数一律是 `taskId` 而不是 `Task`：Python 侧的 `task` 参数是「就地改状态 +
/// 打事件」，Rust 侧由调用方按 id 找回记录。
pub trait TaskService {
    /// `taskService.pause(task)`
    fn pause(&self, task_id: &str);
    /// `taskService.start(task)`
    fn start(&self, task_id: &str);
    /// `taskService.startSeeding(task)`
    fn start_seeding(&self, task_id: &str);
    /// `taskService.stopSeeding(task)`
    fn stop_seeding(&self, task_id: &str);
    /// `taskService.delete(task, shouldDeleteFiles)`
    fn delete(&self, task_id: &str, delete_files: bool);
    /// `taskService.redownload(task)`
    fn redownload(&self, task_id: &str);
    /// `taskService.moveToFront([taskId])`——Python 收列表，卡片永远只传自己那一条。
    fn move_to_front(&self, task_id: &str);
    /// `taskService.setCategory(task, categoryId)`；空串 = 未分类。
    fn set_category(&self, task_id: &str, category_id: &str);
    /// `taskService.startChecksum(task, algorithm)`
    fn start_checksum(&self, task_id: &str, algorithm: &str);
    /// `taskService.cancelChecksum(task)`
    fn cancel_checksum(&self, task_id: &str);
}

/// 卡片动作触达的 UI 钩子（剪贴板 / 抽屉 / 编辑对话框 / 打开文件与文件夹）。
///
/// 这些都不是 `taskService` 的方法，Python 侧分别落在
/// `QApplication.instance().clipboardListener`、`TaskDrawer(...).show()`、
/// `featureService.createEditDialog(...)`、`app.platform.desktop.openFile` /
/// `revealInFolder`。
pub trait UiHooks {
    /// `clipboardListener.setUrls([task.url])`
    fn copy_url(&self, url: &str);
    /// `TaskDrawer(task, …).show()`
    fn open_drawer(&self, task_id: &str);
    /// `featureService.createEditDialog(task, window).exec()` + `refresh()`
    fn edit_task(&self, task_id: &str);
    /// `app.platform.desktop.openFile(task.outputPath)`（`task_cards.py:249`）。
    ///
    /// 参数是**路径**而不是 taskId：Python 侧 lambda 直接闭包了
    /// `self._task.outputPath`，这一层不需要再回头查记录。
    fn open_file(&self, path: &str);
    /// `app.platform.desktop.revealInFolder(task.outputPath)`（`task_cards.py:250`）。
    fn reveal_in_folder(&self, path: &str);
}

/// 镜像 `_onToggleClicked` 的分派并**执行**（返回分派结果供调用方 `refresh()`）。
///
/// 顺序语义见 `card::toggle_action`：`isSeeding` 优先于 `COMPLETED`。
pub fn apply_toggle(
    service: &dyn TaskService,
    task_id: &str,
    status: TaskStatus,
    is_seeding: bool,
) -> ToggleAction {
    let action = toggle_action(status, is_seeding);
    match action {
        ToggleAction::StopSeeding => service.stop_seeding(task_id),
        ToggleAction::StartSeeding => service.start_seeding(task_id),
        ToggleAction::Pause => service.pause(task_id),
        ToggleAction::Start => service.start(task_id),
    }
    action
}

/// `MessageBox(self.tr("删除任务"), self.tr("确定要删除这个下载任务吗？"), …)` 的标题。
pub const DELETE_DIALOG_TITLE: &str = "删除任务";
/// 同上：正文。
pub const DELETE_DIALOG_TEXT: &str = "确定要删除这个下载任务吗？";
/// `CheckBox(self.tr("同时删除已下载的文件"))`。
pub const DELETE_DIALOG_CHECKBOX: &str = "同时删除已下载的文件";

/// 镜像 `_onDeleteClicked` 的确认框形状。
///
/// `delete_files_default` 是 `cfg.shouldDeleteFilesOnRemove.value`（`cfg.py:255`
/// 缺省 `False`）——本层不读配置，由调用方给。按钮是 `MessageBox` 默认的
/// 「确定」/「取消」，`yesButton` 是主按钮。
pub fn delete_confirm_dialog(delete_files_default: bool) -> DialogSpec {
    DialogSpec {
        title: DELETE_DIALOG_TITLE.to_string(),
        text: DELETE_DIALOG_TEXT.to_string(),
        checkbox: Some((DELETE_DIALOG_CHECKBOX.to_string(), delete_files_default)),
        buttons: vec![
            BarButton {
                text: DIALOG_OK.to_string(),
                primary: true,
            },
            BarButton {
                text: DIALOG_CANCEL.to_string(),
                primary: false,
            },
        ],
    }
}

/// 确认框「确定」后的两步：**先落盘配置、再删任务**。
///
/// 返回 `delete_files` 本身，供调用方写回
/// `cfg.set(cfg.shouldDeleteFilesOnRemove, deleteFiles.isChecked())`——顺序与
/// Python 一致（先 `cfg.set` 再 `delete`）。
pub fn apply_delete(service: &dyn TaskService, task_id: &str, delete_files: bool) -> bool {
    // Python: `cfg.set(...)` 在这里发生（调用方负责），然后 `delete`。
    service.delete(task_id, delete_files);
    delete_files
}

/// 镜像 `_onChecksumClicked` 的三条短路落点。
///
/// - `CancelChecksum` → `service.cancel_checksum(taskId)`，返回 `None`
///   （Python 那条分支直接 `return`，不改状态行）；
/// - `SetStatus(text)` → 返回 `Some(text)`，由调用方 `_setStatus(text)`
///   （状态行是视图，本层只回传文案）；
/// - `ShowMenu(entries)` → 返回 `None`，菜单由装配层（`menu_view`）弹。
///
/// 之所以「弹菜单」也回 `None`：菜单的可见性是视图状态，服务层不该知道。
pub fn apply_checksum(
    service: &dyn TaskService,
    task_id: &str,
    outcome: &ChecksumOutcome,
) -> Option<&'static str> {
    match outcome {
        ChecksumOutcome::CancelChecksum => {
            service.cancel_checksum(task_id);
            None
        }
        ChecksumOutcome::SetStatus(text) => Some(text),
        ChecksumOutcome::ShowMenu(_) => None,
    }
}

/// 镜像 `createContextMenu()` 里每个 `triggered` 的落点。
///
/// `url` 只在 `CopyUrl` 用到（Python 取 `task.url`）；其余分支忽略它，
/// 但函数签名统一带 `url`，免得调用方为一次复制去分支。
pub fn dispatch_click(
    click: &CardClick,
    task_id: &str,
    url: &str,
    service: &dyn TaskService,
    hooks: &dyn UiHooks,
) {
    match click {
        CardClick::OpenDrawer => hooks.open_drawer(task_id),
        CardClick::CopyUrl => hooks.copy_url(url),
        CardClick::EditTask => hooks.edit_task(task_id),
        CardClick::MoveToFront => service.move_to_front(task_id),
        CardClick::Redownload => service.redownload(task_id),
        CardClick::SetCategory(category_id) => service.set_category(task_id, category_id),
        CardClick::StartChecksum(algorithm) => service.start_checksum(task_id, algorithm),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// 记录型服务：把每次调用摊成 `"method(args)"` 字符串，便于按序断言。
    #[derive(Default)]
    struct Recorder {
        calls: RefCell<Vec<String>>,
    }

    impl Recorder {
        fn log(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
    }

    impl TaskService for Recorder {
        fn pause(&self, task_id: &str) {
            self.calls.borrow_mut().push(format!("pause({task_id})"));
        }
        fn start(&self, task_id: &str) {
            self.calls.borrow_mut().push(format!("start({task_id})"));
        }
        fn start_seeding(&self, task_id: &str) {
            self.calls
                .borrow_mut()
                .push(format!("startSeeding({task_id})"));
        }
        fn stop_seeding(&self, task_id: &str) {
            self.calls
                .borrow_mut()
                .push(format!("stopSeeding({task_id})"));
        }
        fn delete(&self, task_id: &str, delete_files: bool) {
            self.calls
                .borrow_mut()
                .push(format!("delete({task_id}, {delete_files})"));
        }
        fn redownload(&self, task_id: &str) {
            self.calls
                .borrow_mut()
                .push(format!("redownload({task_id})"));
        }
        fn move_to_front(&self, task_id: &str) {
            self.calls
                .borrow_mut()
                .push(format!("moveToFront({task_id})"));
        }
        fn set_category(&self, task_id: &str, category_id: &str) {
            self.calls
                .borrow_mut()
                .push(format!("setCategory({task_id}, {category_id})"));
        }
        fn start_checksum(&self, task_id: &str, algorithm: &str) {
            self.calls
                .borrow_mut()
                .push(format!("startChecksum({task_id}, {algorithm})"));
        }
        fn cancel_checksum(&self, task_id: &str) {
            self.calls
                .borrow_mut()
                .push(format!("cancelChecksum({task_id})"));
        }
    }

    #[derive(Default)]
    struct Hooks {
        calls: RefCell<Vec<String>>,
    }

    impl UiHooks for Hooks {
        fn copy_url(&self, url: &str) {
            self.calls.borrow_mut().push(format!("setUrls({url})"));
        }
        fn open_drawer(&self, task_id: &str) {
            self.calls.borrow_mut().push(format!("drawer({task_id})"));
        }
        fn edit_task(&self, task_id: &str) {
            self.calls
                .borrow_mut()
                .push(format!("editDialog({task_id})"));
        }
        fn open_file(&self, path: &str) {
            self.calls.borrow_mut().push(format!("openFile({path})"));
        }
        fn reveal_in_folder(&self, path: &str) {
            self.calls
                .borrow_mut()
                .push(format!("revealInFolder({path})"));
        }
    }

    /// `_onToggleClicked` 的四路分派：一次点按**恰好一次**服务调用。
    #[test]
    fn apply_toggle_matches_python_dispatch() {
        let cases = [
            (TaskStatus::Completed, true, "stopSeeding(t1)"),
            (TaskStatus::Completed, false, "startSeeding(t1)"),
            (TaskStatus::Running, false, "pause(t1)"),
            (TaskStatus::Running, true, "stopSeeding(t1)"),
            (TaskStatus::Paused, false, "start(t1)"),
            (TaskStatus::Waiting, false, "start(t1)"),
            (TaskStatus::Failed, false, "start(t1)"),
        ];
        for (status, is_seeding, expected) in cases {
            let service = Recorder::default();
            let action = apply_toggle(&service, "t1", status, is_seeding);
            assert_eq!(service.log(), vec![expected.to_string()], "{status:?}");
            // 返回值必须与 `card::toggle_action` 同义（调用方据此刷新）。
            assert_eq!(action, toggle_action(status, is_seeding));
        }
    }

    /// 删除确认框形状：标题 / 正文 / 复选框文案 + 勾选初值 / 按钮。
    #[test]
    fn delete_confirm_dialog_shape() {
        let spec = delete_confirm_dialog(false);
        assert_eq!(spec.title, "删除任务");
        assert_eq!(spec.text, "确定要删除这个下载任务吗？");
        assert_eq!(
            spec.checkbox,
            Some(("同时删除已下载的文件".to_string(), false))
        );
        assert_eq!(spec.buttons.len(), 2);
        assert_eq!(spec.buttons[0].text, DIALOG_OK);
        assert!(spec.buttons[0].primary);
        assert_eq!(spec.buttons[1].text, DIALOG_CANCEL);
        assert!(!spec.buttons[1].primary);

        // 勾选初值跟着 cfg 走（`cfg.shouldDeleteFilesOnRemove`）。
        let spec = delete_confirm_dialog(true);
        assert_eq!(
            spec.checkbox,
            Some(("同时删除已下载的文件".to_string(), true))
        );
    }

    /// 确定后：先 `cfg.set`（返回值）再 `delete(task, 勾选值)`。
    #[test]
    fn apply_delete_persists_flag_and_deletes() {
        let service = Recorder::default();
        let persisted = apply_delete(&service, "t1", true);
        assert!(persisted, "调用方要把它写回 cfg.shouldDeleteFilesOnRemove");
        assert_eq!(service.log(), vec!["delete(t1, true)".to_string()]);

        let service = Recorder::default();
        assert!(!apply_delete(&service, "t2", false));
        assert_eq!(service.log(), vec!["delete(t2, false)".to_string()]);
    }

    /// 校验按钮三条短路：只有「进行中」会打到服务层。
    #[test]
    fn apply_checksum_short_circuits() {
        let service = Recorder::default();
        let outcome = ChecksumOutcome::CancelChecksum;
        assert_eq!(apply_checksum(&service, "t1", &outcome), None);
        assert_eq!(service.log(), vec!["cancelChecksum(t1)".to_string()]);

        let service = Recorder::default();
        let outcome = ChecksumOutcome::SetStatus("文件不存在，无法校验");
        assert_eq!(
            apply_checksum(&service, "t1", &outcome),
            Some("文件不存在，无法校验")
        );
        assert!(service.log().is_empty(), "置状态行不该碰服务层");

        let service = Recorder::default();
        let outcome = ChecksumOutcome::ShowMenu(Vec::new());
        assert_eq!(apply_checksum(&service, "t1", &outcome), None);
        assert!(service.log().is_empty(), "弹菜单不该碰服务层");
    }

    /// 右键菜单七个动作的落点（`createContextMenu` 的 `triggered`）。
    #[test]
    fn dispatch_click_routes_each_variant() {
        let service = Recorder::default();
        let hooks = Hooks::default();

        dispatch_click(&CardClick::OpenDrawer, "t1", "u", &service, &hooks);
        dispatch_click(&CardClick::CopyUrl, "t1", "https://e/1", &service, &hooks);
        dispatch_click(&CardClick::EditTask, "t1", "u", &service, &hooks);
        dispatch_click(&CardClick::MoveToFront, "t1", "u", &service, &hooks);
        dispatch_click(&CardClick::Redownload, "t1", "u", &service, &hooks);
        dispatch_click(
            &CardClick::SetCategory("cat_video".into()),
            "t1",
            "u",
            &service,
            &hooks,
        );
        dispatch_click(
            &CardClick::SetCategory(String::new()),
            "t1",
            "u",
            &service,
            &hooks,
        );
        dispatch_click(
            &CardClick::StartChecksum("sha256".into()),
            "t1",
            "u",
            &service,
            &hooks,
        );

        assert_eq!(
            hooks.calls.borrow().clone(),
            vec![
                "drawer(t1)".to_string(),
                "setUrls(https://e/1)".to_string(),
                "editDialog(t1)".to_string(),
            ]
        );
        assert_eq!(
            service.log(),
            vec![
                "moveToFront(t1)".to_string(),
                "redownload(t1)".to_string(),
                "setCategory(t1, cat_video)".to_string(),
                // 「未分类」就是空串（`setCategory(task, "")`）。
                "setCategory(t1, )".to_string(),
                "startChecksum(t1, sha256)".to_string(),
            ]
        );
    }
}
