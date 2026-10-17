//! 任务列表筛选语义：镜像 `app/view/pages/task_page.py::_refreshList`
//! （599-620 行）的筛选项与路由字面量（45-47 行 ROUTE_TO_FILTER）。
//!
//! 口径（顺序：先过滤、不动排序——Python 侧排序在这之后另行处理）：
//! - FilterMode 三态经段路由字面量转换：all/active/completed，
//!   未知字面量容错回 All（合法字面量由桥接层保证只传三种）。
//! - ACTIVE = RUNNING ∪ WAITING ∪ PAUSED；COMPLETED = COMPLETED ∪ FAILED
//!   （FILTER_TO_STATUSES，40-41 行；五态枚举 app/models/task.py:27 已实测）。
//! - `category` 非空 → `t.category == self._categoryFilter` 精确相等
//!   （task_page.py:605-607）。
//! - `search` 非空 → `lower in t.name.lower() or lower in t.url.lower()`
//!   （task_page.py:608-611）。
//!
//! 分层：本文件只做纯变换（不改输入、不碰排序与文案），文案仍由
//! row.rs 的镜像层给出。

use gd3_core::task::{TaskRecord, TaskStatus};

/// 段路由（task_page.py:45-47 ROUTE_TO_FILTER）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilterMode {
    #[default]
    All,
    Active,
    Completed,
}

impl FilterMode {
    /// 镜像 `ROUTE_TO_FILTER`；Python 调用处只传三个合法键，
    /// 未知键容错回缺省 All。
    pub fn from_route(route: &str) -> FilterMode {
        match route {
            "active" => FilterMode::Active,
            "completed" => FilterMode::Completed,
            _ => FilterMode::All,
        }
    }

    /// 状态是否入选（FILTER_TO_STATUSES，40-41 行）。
    pub fn matches(self, status: TaskStatus) -> bool {
        match self {
            FilterMode::All => true,
            FilterMode::Active => matches!(
                status,
                TaskStatus::Running | TaskStatus::Waiting | TaskStatus::Paused
            ),
            FilterMode::Completed => {
                matches!(status, TaskStatus::Completed | TaskStatus::Failed)
            }
        }
    }
}

/// 筛选条件集合（Python 侧任一为空即跳过该项）。
#[derive(Debug, Clone, Copy, Default)]
pub struct TaskFilter<'a> {
    pub mode: FilterMode,
    /// 空字符串 = 不过滤（Python `if self._categoryFilter`）。
    pub category: &'a str,
    /// 空字符串 = 不过滤（Python `if self._searchText`）。
    pub search: &'a str,
}

/// 筛选 `records`，返回引用并保持原顺序（Python 过滤是列表推导、
/// 不改变相对顺序）。
pub fn filtered<'a>(records: &'a [TaskRecord], filter: TaskFilter<'a>) -> Vec<&'a TaskRecord> {
    let lower_search = filter.search.to_lowercase();
    let category_empty = filter.category.is_empty();
    let search_empty = lower_search.is_empty();
    let mode = filter.mode;
    records
        .iter()
        .filter(|record| {
            mode.matches(record.status)
                && (category_empty || record.category.as_deref() == Some(filter.category))
                && (search_empty
                    || record.name.to_lowercase().contains(&lower_search)
                    || record.url.to_lowercase().contains(&lower_search))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(name: &str, status: &str, category: serde_json::Value) -> TaskRecord {
        serde_json::from_value(json! ({
            "name": name,
            "url": format!("https://example/{name}"),
            "packId": "pack",
            "taskId": format!("tsk_{name}"),
            "status": status,
            "category": category,
        }))
        .expect("反序列化失败")
    }

    /// 状态集合语义：ACTIVE = RUNNING∪WAITING∪PAUSED、
    /// COMPLETED = COMPLETED∪FAILED、ALL 全过。
    #[test]
    fn mode_matches_status_sets() {
        for (status, all, active, completed) in [
            (TaskStatus::Waiting, true, true, false),
            (TaskStatus::Running, true, true, false),
            (TaskStatus::Paused, true, true, false),
            (TaskStatus::Completed, true, false, true),
            (TaskStatus::Failed, true, false, true),
        ] {
            assert_eq!(FilterMode::All.matches(status), all, "{status:?}");
            assert_eq!(FilterMode::Active.matches(status), active, "{status:?}");
            assert_eq!(
                FilterMode::Completed.matches(status),
                completed,
                "{status:?}"
            );
        }
    }

    /// 路由字面量（ROUTE_TO_FILTER 三键）。
    #[test]
    fn from_route_matches_python_literals() {
        assert_eq!(FilterMode::from_route("active"), FilterMode::Active);
        assert_eq!(FilterMode::from_route("completed"), FilterMode::Completed);
        assert_eq!(FilterMode::from_route("all"), FilterMode::All);
    }

    #[test]
    fn filtered_keeps_order_and_combines_conditions() {
        let records = vec![
            record("Ball.mkv", "RUNNING", json!("vip")),
            record("d2.mp4", "WAITING", json!(null)),
            record("AbC.iso", "COMPLETED", json!("vip")),
            record("ball_backup.mkv", "FAILED", json!(null)),
        ];
        let view = |mode: FilterMode, category: &'static str, search: &'static str| {
            filtered(
                &records,
                TaskFilter {
                    mode,
                    category,
                    search,
                },
            )
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>()
        };
        // 保持原顺序（Python 列表推导不改变相对顺序）。
        assert_eq!(
            view(FilterMode::All, "", ""),
            ["Ball.mkv", "d2.mp4", "AbC.iso", "ball_backup.mkv"]
        );
        assert_eq!(view(FilterMode::Active, "", ""), ["Ball.mkv", "d2.mp4"]);
        assert_eq!(
            view(FilterMode::Completed, "", ""),
            ["AbC.iso", "ball_backup.mkv"]
        );
        // 分类精确相等：Python `t.category == self._categoryFilter`。
        assert_eq!(view(FilterMode::All, "vip", ""), ["Ball.mkv", "AbC.iso"]);
        assert_eq!(view(FilterMode::Active, "vip", ""), ["Ball.mkv"]);
        // 搜索大小写不敏感子串（name，url 同理经 lower）。
        assert_eq!(
            view(FilterMode::All, "", "BALL"),
            ["Ball.mkv", "ball_backup.mkv"]
        );
        assert_eq!(view(FilterMode::All, "", "abc"), ["AbC.iso"]);
        // 条件叠加：mode ∩ category ∩ search。
        assert_eq!(view(FilterMode::Completed, "vip", "abc"), ["AbC.iso"]);
        assert_eq!(view(FilterMode::Active, "vip", "abc"), Vec::<&str>::new());
    }
}
