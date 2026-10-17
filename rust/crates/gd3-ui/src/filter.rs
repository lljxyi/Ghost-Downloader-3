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

/// 分类菜单条目（对齐 `CategoryService.categories()` 的最小子集）。
///
/// Python 侧 `Category` 还带 icon/summaryPrefix 等，UI 菜单只消费
/// categoryId + 展示名（toCategoryName），其余字段后续用到再接。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryRef {
    pub category_id: String,
    pub name: String,
}

/// 「全部分类」表头（task_page.py:_rebuildCategoryFilterMenu 的
/// allAction，对应空 categoryFilter ""）。
pub const ALL_CATEGORIES_LABEL: &str = "全部分类";

/// ComboBox 的 []string 模型：表头 + 各分类展示名（索引序即菜单序）。
pub fn menu_names(categories: &[CategoryRef]) -> Vec<String> {
    std::iter::once(ALL_CATEGORIES_LABEL.to_string())
        .chain(categories.iter().map(|category| category.name.clone()))
        .collect()
}

/// ComboBox 选中索引 → categoryFilter：0（表头）→ ""，其余取
/// `categories[index-1].categoryId`；越界容错回 ""（slint 回调只会
/// 带当前模型内的索引，防御性兜底）。
pub fn category_id_at(categories: &[CategoryRef], index: i32) -> &str {
    if index <= 0 {
        return "";
    }
    categories
        .get((index - 1) as usize)
        .map(|category| category.category_id.as_str())
        .unwrap_or("")
}

/// 镜像 `_rebuildCategoryFilterMenu` 尾部的失效容错：当前过滤值不
/// 「在合法集合（含 ""）」内 → 回落 ""（调用方需据此重刷列表）。
pub fn sanitize_category_filter<'a>(current: &'a str, categories: &[CategoryRef]) -> &'a str {
    if current.is_empty() || categories.iter().any(|c| c.category_id == current) {
        current
    } else {
        ""
    }
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

    /// 分类菜单：表头「全部分类」+ 各分类名；索引→id 映射（0=空，
    /// 越界容错）与失效过滤值清理（镜像 _rebuildCategoryFilterMenu）。
    #[test]
    fn category_menu_names_index_mapping_and_sanitize() {
        let categories = vec![
            CategoryRef {
                category_id: "cat_video".into(),
                name: "视频".into(),
            },
            CategoryRef {
                category_id: "cat_audio".into(),
                name: "音频".into(),
            },
        ];
        assert_eq!(menu_names(&categories), ["全部分类", "视频", "音频"]);
        assert_eq!(category_id_at(&categories, 0), "");
        assert_eq!(category_id_at(&categories, 1), "cat_video");
        assert_eq!(category_id_at(&categories, 2), "cat_audio");
        assert_eq!(category_id_at(&categories, 3), "");
        assert_eq!(category_id_at(&categories, -1), "");
        // 失效过滤值（分类被删）回落 ""；合法值与空值原样保留。
        assert_eq!(
            sanitize_category_filter("cat_audio", &categories),
            "cat_audio"
        );
        assert_eq!(sanitize_category_filter("cat_gone", &categories), "");
        assert_eq!(sanitize_category_filter("", &categories), "");
    }
}
