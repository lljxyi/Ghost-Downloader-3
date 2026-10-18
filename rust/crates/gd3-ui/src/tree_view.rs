//! 组件库（P4，第 69 步）：镜像 `app/view/components/tree_view.py`（62 行）
//! 的 [`AutoSizingTreeView`] 尺寸口径。
//!
//! ## 实证口径（2026-10-18 实读源码）
//!
//! - 构造：`setSizePolicy(Expanding, Maximum)`；`expanded`/`collapsed`、
//!   `model.rowsInserted/rowsRemoved/modelReset` 全部接 `updateGeometry`
//!   （**尺寸随展开状态与模型变化重算**，不是一次算完）。
//! - `_visibleRowCount` = 从根索引递归计数：每行算 1，**仅当该行展开**才把
//!   子行加进来。
//! - `_rowHeight`：模型有行时 `max(sizeHintForRow(0), 1)`，否则**默认 30**。
//! - `_contentWidth`：表头 `count() == 0` 时退化成 `super().sizeHint().width()`；
//!   否则 `header.length() + frameWidth()*2 + verticalScrollBar().sizeHint().width()`。
//! - `_sizeForRows(count)` = `QSize(contentWidth, headerHeight + count*rowHeight + 4)`，
//!   表头不可见时 `headerHeight = 0`。
//! - `minimumSizeHint` 用 `min(_minimumVisibleRows, 可见行数)`；
//!   `sizeHint` 先按 `_maximumVisibleRows` 截断，再 `expandedTo(minimumSizeHint())`
//!   ——即**高度下限永远不低于最小可见行数对应的高度**。
//!
//! ## 与 Qt 的差异
//!
//! 表头长度、行高提示、滚动条宽度、`super().sizeHint()` 都是 Qt 布局实测值，
//! 本模块用 [`TreeMetrics`] 当入参，尺寸公式逐行照抄。

/// `return 30`（tree_view.py:38）
pub const DEFAULT_ROW_HEIGHT: i32 = 30;
/// `headerHeight + count * self._rowHeight() + 4`（:44）
pub const SIZE_PADDING: i32 = 4;

/// 一棵「行是否展开」的树。根层就是 `QModelIndex()` 的 `rowCount`。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowNode {
    pub expanded: bool,
    pub children: Vec<RowNode>,
}

impl RowNode {
    pub fn leaf() -> Self {
        Self {
            expanded: false,
            children: Vec::new(),
        }
    }

    pub fn expanded(children: Vec<RowNode>) -> Self {
        Self {
            expanded: true,
            children,
        }
    }

    /// 折叠但带子行：`isExpanded(index)` 为假 → 子行不计入。
    pub fn collapsed(children: Vec<RowNode>) -> Self {
        Self {
            expanded: false,
            children,
        }
    }
}

/// `_countVisible(QModelIndex())`：递归数可见行。
pub fn count_visible(rows: &[RowNode]) -> i32 {
    let mut count = 0;
    for row in rows {
        count += 1;
        if row.expanded {
            count += count_visible(&row.children);
        }
    }
    count
}

/// Qt 布局实测值（本模块无法自己算的那些）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TreeMetrics {
    /// `header().count()`
    pub header_count: usize,
    /// `header().length()`
    pub header_length: i32,
    /// `frameWidth()`
    pub frame_width: i32,
    /// `verticalScrollBar().sizeHint().width()`
    pub scrollbar_width: i32,
    /// `super().sizeHint().width()`（无表头时的兜底）
    pub fallback_width: i32,
    /// `header().isVisible()`
    pub header_visible: bool,
    /// `header().height()`
    pub header_height: i32,
    /// `sizeHintForRow(0)`
    pub size_hint_row0: i32,
    /// `model().rowCount() > 0`
    pub model_has_rows: bool,
}

impl Default for TreeMetrics {
    fn default() -> Self {
        Self {
            header_count: 0,
            header_length: 0,
            frame_width: 0,
            scrollbar_width: 0,
            fallback_width: 0,
            header_visible: true,
            header_height: 0,
            size_hint_row0: 0,
            model_has_rows: false,
        }
    }
}

/// `(minimumVisibleRows, maximumVisibleRows)` 的构造参数。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutoSizingTreeView {
    pub minimum_visible_rows: i32,
    pub maximum_visible_rows: Option<i32>,
}

impl AutoSizingTreeView {
    /// `__init__(..., minimumVisibleRows: int = 1, maximumVisibleRows: int | None = None)`
    pub fn new(minimum_visible_rows: i32, maximum_visible_rows: Option<i32>) -> Self {
        Self {
            minimum_visible_rows,
            maximum_visible_rows,
        }
    }

    /// `_rowHeight()`。
    pub fn row_height(&self, metrics: &TreeMetrics) -> i32 {
        if metrics.model_has_rows {
            metrics.size_hint_row0.max(1)
        } else {
            DEFAULT_ROW_HEIGHT
        }
    }

    /// `_contentWidth()`。
    pub fn content_width(&self, metrics: &TreeMetrics) -> i32 {
        if metrics.header_count == 0 {
            return metrics.fallback_width;
        }
        metrics.header_length + metrics.frame_width * 2 + metrics.scrollbar_width
    }

    /// `_sizeForRows(count)` → `(width, height)`。
    pub fn size_for_rows(&self, count: i32, metrics: &TreeMetrics) -> (i32, i32) {
        let header_height = if metrics.header_visible {
            metrics.header_height
        } else {
            0
        };
        (
            self.content_width(metrics),
            header_height + count * self.row_height(metrics) + SIZE_PADDING,
        )
    }

    /// `minimumSizeHint()`。
    pub fn minimum_size_hint(&self, visible_rows: i32, metrics: &TreeMetrics) -> (i32, i32) {
        self.size_for_rows(self.minimum_visible_rows.min(visible_rows), metrics)
    }

    /// `sizeHint()`：先按 `maximumVisibleRows` 截断，再与最小尺寸取逐分量最大。
    pub fn size_hint(&self, visible_rows: i32, metrics: &TreeMetrics) -> (i32, i32) {
        let count = match self.maximum_visible_rows {
            Some(max_rows) => visible_rows.min(max_rows),
            None => visible_rows,
        };
        let (width, height) = self.size_for_rows(count, metrics);
        let (min_width, min_height) = self.minimum_size_hint(visible_rows, metrics);
        (width.max(min_width), height.max(min_height))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics() -> TreeMetrics {
        TreeMetrics {
            header_count: 3,
            header_length: 300,
            frame_width: 1,
            scrollbar_width: 12,
            fallback_width: 999,
            header_visible: true,
            header_height: 36,
            size_hint_row0: 28,
            model_has_rows: true,
        }
    }

    #[test]
    fn constants_match_python() {
        assert_eq!(DEFAULT_ROW_HEIGHT, 30);
        assert_eq!(SIZE_PADDING, 4);
    }

    /// 折叠行的子行不计数；展开才递归。
    #[test]
    fn visible_rows_respect_expansion() {
        let rows = vec![
            RowNode::collapsed(vec![RowNode::leaf(), RowNode::leaf()]),
            RowNode::expanded(vec![
                RowNode::leaf(),
                RowNode::expanded(vec![RowNode::leaf(), RowNode::leaf()]),
            ]),
        ];
        // 根 2 行；第一行折叠不计子行；第二行展开 +2，其中第二个子行再展开 +2。
        assert_eq!(count_visible(&rows), 2 + 2 + 2);
        assert_eq!(count_visible(&[]), 0);
        assert_eq!(count_visible(&[RowNode::leaf()]), 1);
    }

    /// 行高：有行取 `max(hint,1)`，无行固定 30。
    #[test]
    fn row_height_falls_back_to_thirty() {
        let view = AutoSizingTreeView::new(1, None);
        assert_eq!(view.row_height(&metrics()), 28);
        let mut empty = metrics();
        empty.model_has_rows = false;
        assert_eq!(view.row_height(&empty), DEFAULT_ROW_HEIGHT);
        let mut zero_hint = metrics();
        zero_hint.size_hint_row0 = 0;
        assert_eq!(view.row_height(&zero_hint), 1);
    }

    /// 内容宽：无表头退化到 `super().sizeHint().width()`。
    #[test]
    fn content_width_branches_on_header_count() {
        let view = AutoSizingTreeView::new(1, None);
        assert_eq!(view.content_width(&metrics()), 300 + 2 + 12);
        let mut no_header = metrics();
        no_header.header_count = 0;
        assert_eq!(view.content_width(&no_header), 999);
    }

    /// `sizeForRows`：表头可见加表头高，不可见按 0；末尾 +4。
    #[test]
    fn size_for_rows_includes_header_and_padding() {
        let view = AutoSizingTreeView::new(1, None);
        assert_eq!(view.size_for_rows(3, &metrics()), (314, 36 + 3 * 28 + 4));
        let mut hidden = metrics();
        hidden.header_visible = false;
        assert_eq!(view.size_for_rows(3, &hidden), (314, 3 * 28 + 4));
        assert_eq!(view.size_for_rows(0, &hidden), (314, 4));
    }

    /// `minimumSizeHint` 用 `min(minimumVisibleRows, 可见行数)`。
    #[test]
    fn minimum_size_uses_the_smaller_row_count() {
        let view = AutoSizingTreeView::new(1, Some(6));
        assert_eq!(view.minimum_size_hint(5, &metrics()), (314, 36 + 28 + 4));
        let taller = AutoSizingTreeView::new(3, Some(6));
        assert_eq!(
            taller.minimum_size_hint(5, &metrics()),
            (314, 36 + 3 * 28 + 4)
        );
        assert_eq!(taller.minimum_size_hint(0, &metrics()), (314, 36 + 4));
    }

    /// `sizeHint`：上限截断后再 `expandedTo(minimumSizeHint())`。
    #[test]
    fn size_hint_clamps_to_maximum_then_expands_to_minimum() {
        let view = AutoSizingTreeView::new(1, Some(6));
        assert_eq!(view.size_hint(20, &metrics()), (314, 36 + 6 * 28 + 4));

        // 上限比最小行数还小：高度被最小尺寸抬回去。
        let odd = AutoSizingTreeView::new(4, Some(2));
        let (width, height) = odd.size_hint(10, &metrics());
        assert_eq!(width, 314);
        assert_eq!(height, 36 + 4 * 28 + 4);

        // 无上限：按可见行数全展开。
        let unbounded = AutoSizingTreeView::new(1, None);
        assert_eq!(unbounded.size_hint(3, &metrics()), (314, 36 + 3 * 28 + 4));
    }
}
