//! 任务列表排序语义：镜像 `app/view/pages/task_page.py::_refreshList`
//! 末段（614-639 行）的五个排序分支与缺省序。
//!
//! 口径：
//! - 缺省 = CREATED_AT + 倒序（task_page.py:143-144
//!   `_sortField = CREATED_AT`、`_sortAscending = False`）。
//! - 稳定性 = Python `list.sort` 语义——reverse 下相等元素仍保持原
//!   相对顺序（Rust `sort_by` 两侧比较器均为稳定排序，等价）。
//! - NAME 键 = `t.name.lower()`（size/completedAt 等为原值）。
//! - COMPLETED_AT：completedAt **真值即 ≠0**；completed 组按
//!   completedAt、pending 组按 createdAt 分别倒/升；拼接顺序由
//!   desc 决定（desc: pending+completed，asc: completed+pending）。
//! - QUEUE_POSITION：依赖 `taskService.waitingOrder()`（调用方传入
//!   taskId 列表）；running / waiting / rest 三组保序划分，waiting
//!   组恒按队列序升排、rest 组恒按 createdAt 倒排（不受 ascending
//!   影响，这与 Python 写死的 reverse=True 一致），仅组间拼接顺序
//!   随 ascending 翻转。
//!
//! 分层：本文件只做纯变换（不改输入记录、不碰筛选与文案）；菜单
//! 显示名归 UI 层（slint 侧硬编码镜像 Python .tr 字面量）。

use gd3_core::task::{TaskRecord, TaskStatus};

/// 排序字段（task_page.py:52-58 SortField，IntEnum 0-4）。
///
/// UI 侧不用本枚举的枚举序——ComboBox 显示序是菜单书写序
/// （按添加时间/按完成时间/按名称/按大小/按队列），经
/// [from_menu_index] 映射。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortField {
    #[default]
    CreatedAt,
    Name,
    Size,
    CompletedAt,
    QueuePosition,
}

impl SortField {
    /// ComboBox 显示序索引 → 字段；越界/未知容错回缺省 CreatedAt。
    pub fn from_menu_index(index: i32) -> SortField {
        match index {
            0 => SortField::CreatedAt,
            1 => SortField::CompletedAt,
            2 => SortField::Name,
            3 => SortField::Size,
            4 => SortField::QueuePosition,
            _ => SortField::CreatedAt,
        }
    }
}

/// 排序 `refs`（保序稳定性见模块注）。`waiting_order` 仅
/// QueuePosition 分支消费：队列内 taskId 顺序由调用方给定。
pub fn sort_refs(
    refs: &mut Vec<&TaskRecord>,
    field: SortField,
    ascending: bool,
    waiting_order: &[String],
) {
    match field {
        SortField::CreatedAt => {
            sort_by(refs, ascending, |r| r.created_at);
        }
        SortField::Name => {
            // Python 键 t.name.lower()。
            sort_by(refs, ascending, |r| r.name.to_lowercase());
        }
        SortField::Size => {
            sort_by(refs, ascending, |r| r.file_size);
        }
        SortField::CompletedAt => {
            // desc = !ascending（Python 632 行）。completed 真值 ≠0。
            let mut completed: Vec<&TaskRecord> = Vec::new();
            let mut pending: Vec<&TaskRecord> = Vec::new();
            for record in refs.drain(..) {
                if record.completed_at != 0 {
                    completed.push(record);
                } else {
                    pending.push(record);
                }
            }
            sort_by(&mut completed, ascending, |r| r.completed_at);
            sort_by(&mut pending, ascending, |r| r.created_at);
            // Python 636 行：desc（=ascending False）→ pending+completed。
            if ascending {
                completed.extend(pending);
                *refs = completed;
            } else {
                pending.extend(completed);
                *refs = pending;
            }
        }
        SortField::QueuePosition => {
            let waiting_index: std::collections::HashMap<&str, usize> = waiting_order
                .iter()
                .enumerate()
                .map(|(index, task_id)| (task_id.as_str(), index))
                .collect();
            let mut running: Vec<&TaskRecord> = Vec::new();
            let mut waiting: Vec<&TaskRecord> = Vec::new();
            let mut rest: Vec<&TaskRecord> = Vec::new();
            for record in refs.drain(..) {
                if record.status == TaskStatus::Running {
                    running.push(record);
                } else if waiting_index.contains_key(record.task_id.as_str()) {
                    waiting.push(record);
                } else {
                    rest.push(record);
                }
            }
            // Python 633 行：waiting 恒按队列序升排（reverse 未给参数）。
            sort_by(&mut waiting, true, |r| waiting_index[r.task_id.as_str()]);
            // Python 634 行：rest 恒按 createdAt 倒排（写死 reverse=True）。
            sort_by(&mut rest, false, |r| r.created_at);
            // Python 638 行：default 倒序 → running + waiting + rest，
            // 升序 → rest + waiting + running。
            if ascending {
                rest.extend(waiting);
                rest.extend(running);
                *refs = rest;
            } else {
                running.extend(waiting);
                running.extend(rest);
                *refs = running;
            }
        }
    }
}

/// 稳定排序：`ascending` 正常比较，否则反向比较——两种方向都满足
/// 「相等元素保持原相对顺序」（Python reverse=True 的稳定性等价）。
fn sort_by<T>(refs: &mut [&TaskRecord], ascending: bool, key: impl Fn(&TaskRecord) -> T)
where
    T: Ord,
{
    // sort_by_key 为稳定排序；Reverse 包装等价「反比较器」——两种方向
    // 都满足「相等元素保持原相对顺序」（Python reverse=True 等价）。
    if ascending {
        refs.sort_by_key(|record| key(record));
    } else {
        refs.sort_by_key(|record| std::cmp::Reverse(key(record)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(
        name: &str,
        status: &str,
        created_at: i64,
        file_size: i64,
        completed_at: i64,
    ) -> TaskRecord {
        serde_json::from_value(json!({
            "name": name,
            "url": format!("https://example/{name}"),
            "packId": "pack",
            "taskId": format!("tsk_{name}"),
            "status": status,
            "createdAt": created_at,
            "fileSize": file_size,
            "completedAt": completed_at,
        }))
        .expect("反序列化失败")
    }

    fn names<'a>(refs: &[&'a TaskRecord]) -> Vec<&'a str> {
        refs.iter().map(|r| r.name.as_str()).collect()
    }

    /// CreatedAt / Name / Size 三分支：倒序缺省、asc/desc 双向、
    /// name 键大小写不敏感、相等元素保序。
    #[test]
    fn basic_fields_sort_both_directions() {
        let a = record("aaa.bin", "WAITING", 100, 500, 0);
        let b = record("ABC.mkv", "RUNNING", 300, 300, 0);
        let c = record("bcd.mp4", "COMPLETED", 200, 400, 900);
        let mut view = vec![&a, &b, &c];

        // Name（键 = 小写 name）："aaa.bin" < "abc.mkv" < "bcd.mp4"。
        sort_refs(&mut view, SortField::Name, true, &[]);
        assert_eq!(names(&view), ["aaa.bin", "ABC.mkv", "bcd.mp4"]);
        sort_refs(&mut view, SortField::Name, false, &[]);
        assert_eq!(names(&view), ["bcd.mp4", "ABC.mkv", "aaa.bin"]);

        // Size：asc 300/400/500，desc 相反。
        sort_refs(&mut view, SortField::Size, true, &[]);
        assert_eq!(names(&view), ["ABC.mkv", "bcd.mp4", "aaa.bin"]);
        sort_refs(&mut view, SortField::Size, false, &[]);
        assert_eq!(names(&view), ["aaa.bin", "bcd.mp4", "ABC.mkv"]);

        // CreatedAt：asc 100/200/300；缺省倒序（降序 300/200/100）。
        sort_refs(&mut view, SortField::CreatedAt, true, &[]);
        assert_eq!(names(&view), ["aaa.bin", "bcd.mp4", "ABC.mkv"]);
        sort_refs(&mut view, SortField::CreatedAt, false, &[]);
        assert_eq!(names(&view), ["ABC.mkv", "bcd.mp4", "aaa.bin"]);

        // 稳定性：相等键保持原相对顺序（时间戳同值）。
        let t1 = record("x1.bin", "WAITING", 500, 0, 0);
        let t2 = record("x2.bin", "WAITING", 500, 0, 0);
        let t3 = record("x3.bin", "WAITING", 500, 0, 0);
        let mut view = vec![&t3, &t1, &t2];
        sort_refs(&mut view, SortField::CreatedAt, false, &[]);
        assert_eq!(names(&view), ["x3.bin", "x1.bin", "x2.bin"]);
        sort_refs(&mut view, SortField::CreatedAt, true, &[]);
        assert_eq!(names(&view), ["x3.bin", "x1.bin", "x2.bin"]);
    }

    /// COMPLETED_AT：desc = pending(createdAt 倒) + completed(completedAt
    /// 倒)；asc = completed(升) + pending(升)；completedAt 真值 ≠0。
    #[test]
    fn completed_at_partitions_and_directions() {
        let done_old = record("done_old.iso", "COMPLETED", 10, 0, 100);
        let done_new = record("done_new.iso", "FAILED", 20, 0, 400);
        let pend_late = record("pend_late.mkv", "RUNNING", 500, 0, 0);
        let pend_early = record("pend_early.mkv", "PAUSED", 50, 0, 0);
        let mut view = vec![&pend_late, &done_old, &pend_early, &done_new];

        // 缺省倒序：pending 组按 createdAt 倒 → pend_late, pend_early；
        // completed 组按 completedAt 倒 → done_new, done_old。
        sort_refs(&mut view, SortField::CompletedAt, false, &[]);
        assert_eq!(
            names(&view),
            [
                "pend_late.mkv",
                "pend_early.mkv",
                "done_new.iso",
                "done_old.iso"
            ]
        );
        // 升序：completed 组按 completedAt 升 → done_old, done_new；
        // pending 组按 createdAt 升 → pend_early, pend_late。
        sort_refs(&mut view, SortField::CompletedAt, true, &[]);
        assert_eq!(
            names(&view),
            [
                "done_old.iso",
                "done_new.iso",
                "pend_early.mkv",
                "pend_late.mkv"
            ]
        );
    }

    /// QUEUE_POSITION：running 组原著前、waiting 按传入队列序、rest 恒
    /// createdAt 倒；倒序（缺省）= running+waiting+rest，升序翻转组序。
    #[test]
    fn queue_position_partitions_by_waiting_order() {
        let r1 = record("run1.bin", "RUNNING", 900, 0, 0);
        let w2 = record("wait2.mp4", "WAITING", 800, 0, 0);
        let w1 = record("wait1.melody", "WAITING", 700, 0, 0);
        let rest_old = record("rest_old.iso", "PAUSED", 600, 0, 0);
        let rest_new = record("rest_new.vip", "WAITING", 950, 0, 0);
        let mut view = vec![&w1, &rest_new, &r1, &w2, &rest_old];

        // 倒序（缺省）：running=(run1)；waiting 按 [wait2, wait1] 序
        // → wait2, wait1；rest 恒 createdAt 倒 → rest_new, rest_old。
        // 注意：taskId = tsk_{name}（含扩展名），队列列表须带全名。
        let waiting = vec!["tsk_wait2.mp4".to_string(), "tsk_wait1.melody".to_string()];
        sort_refs(&mut view, SortField::QueuePosition, false, &waiting);
        assert_eq!(
            names(&view),
            [
                "run1.bin",
                "wait2.mp4",
                "wait1.melody",
                "rest_new.vip",
                "rest_old.iso"
            ]
        );
        // 升序：组间翻转 → rest 前置；rest 组内仍恒 createdAt 倒
        // （rest_new 950 > rest_old 600）；waiting 组内仍按队列序。
        sort_refs(&mut view, SortField::QueuePosition, true, &waiting);
        assert_eq!(
            names(&view),
            [
                "rest_new.vip",
                "rest_old.iso",
                "wait2.mp4",
                "wait1.melody",
                "run1.bin"
            ]
        );
    }

    /// from_menu_index：菜单显示序 → 字段；越界容错回 CreatedAt。
    #[test]
    fn from_menu_index_maps_display_order() {
        use SortField::{CompletedAt, CreatedAt, Name, QueuePosition, Size};
        assert_eq!(SortField::from_menu_index(0), CreatedAt);
        assert_eq!(SortField::from_menu_index(1), CompletedAt);
        assert_eq!(SortField::from_menu_index(2), Name);
        assert_eq!(SortField::from_menu_index(3), Size);
        assert_eq!(SortField::from_menu_index(4), QueuePosition);
        assert_eq!(SortField::from_menu_index(-1), CreatedAt);
        assert_eq!(SortField::from_menu_index(5), CreatedAt);
    }
}
