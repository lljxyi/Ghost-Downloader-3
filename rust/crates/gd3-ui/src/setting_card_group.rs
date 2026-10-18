//! 可折叠设置卡片与分组容器（P1）：镜像
//! `app/view/components/setting_card_group.py`（290 行）里**尚未被
//! `settings_group.rs` 覆盖**的那一半。
//!
//! 分工（`settings_group.rs` 是第 61 步的产物）：
//!
//! - 已覆盖：`isSearchMatch` / `setSearchText` 的可见性与折叠判定 /
//!   `_onExpandClicked` 对 `cfg.expandedSettingGroups` 的增删 / `_saveOrder` /
//!   `_restoreOrder` / `updateArrows`。
//! - 本模块补：`QWIDGETSIZE_MAX`、`LabelElideFilter` 的逐行省略、两个折叠
//!   动画的起止值、`CollapsibleSettingCard` 的 `addGroupWidget` 分隔线规则、
//!   `CollapsibleSettingCardGroup` 的头部几何、背景色、分隔线矩形、
//!   头部命中判定与 `ExpandButton` 角度。
//!
//! Python 先验（2026-10-18 实读源码，逐条对应）：
//!
//! - **`QWIDGETSIZE_MAX = (1 << 24) - 1`**（:26）：Qt 的「不限高」哨兵值，
//!   两处 `finished` 回调都拿它解除动画留下的 `maximumHeight`。
//! - **`LabelElideFilter`**（:30-43）：`"\n".join(elidedText(line, ElideRight,
//!   rect.width()) for line in obj.text().splitlines() or [""])`——逐行省略
//!   后**用 `\n` 重新拼**，所以原文里的 `\r\n` / `\u{2028}` 等行界会被
//!   归一成 `\n`；`or [""]` 只影响空串（结果仍是空串）。
//! - **`CardPaintFilter`**（:46-49）：吞掉所有 `Paint` 事件——卡片本体由
//!   `SettingCardGroup` 统一绘制（配合 `_normalBackgroundColor`），故卡片
//!   自己的 `paintEvent` 不会跑。
//! - **`CollapsibleSettingCard`**（:52-105）：`view.setMaximumHeight(0)`
//!   起手；`_expandAnim` 时长 **200ms**、曲线 **OutCubic**；`setExpand`
//!   幂等（同值直接 return），否则 `start = view.height()`、
//!   `end = 0 if not isExpand else view.sizeHint().height()`；
//!   `_onExpandFinished` **只在展开时**把 `maximumHeight` 放成
//!   `QWIDGETSIZE_MAX`（折叠时不改，留 0）。
//!   `addGroupWidget`：`viewLayout.count() >= 1` 时**先插一条
//!   `GroupSeparator`** 再加控件（即第一张卡前没有分隔线）。
//! - **`CollapsibleSettingCardGroup`**（:107-290）：头部
//!   `setContentsMargins(16, 10, 8, 10)` + `setSpacing(4)`；标题固定高
//!   **26**；上下移动按钮 `setFixedSize(30, 30)` + `setIconSize(10, 10)`；
//!   `_normalBackgroundColor` = `QColor(255, 255, 255, 13 if dark else 170)`
//!   ——注意与 `card_groups.py` 的 200 不同；`paintEvent` 在
//!   `cardContainer.height() > 0` 时于 `(1, cardContainer.y() - 1,
//!   width() - 2, 1)` 画一条 `QColor(0, 0, 0, 96 if dark else 24)` 分隔线；
//!   `mousePressEvent` 只在**左键且 `y < cardContainer.geometry().top()`**
//!   时切换折叠；`_collapseAnim` 同样 200ms OutCubic，
//!   `start = cardContainer.height()`、`end = 0 if collapsed else
//!   cardContainer.sizeHint().height()`；`_initWidget` 里
//!   `_collapsed = objectName() not in cfg.expandedSettingGroups`，
//!   `cardContainer.setMaximumHeight(0 if collapsed else QWIDGETSIZE_MAX)`，
//!   `expandButton.setAngle(0 if collapsed else 180)`；
//!   `addSettingCard` 对每张卡的标题/内容标签做
//!   `setWordWrap(False)` + `QSizePolicy(Ignored, Preferred)` +
//!   `AlignVCenter` + 装 `LabelElideFilter`，并给内容布局
//!   `setStretchFactor(1 << 16)`。
//!
//! 不镜像：`QPainter` 实际绘制、`QPropertyAnimation` 的时钟推进（这里只给
//! 「从哪到哪、多久、什么曲线」）、`FluentStyleSheet.apply` 的样式表内容。

use crate::labels::{Advance, elide_right};
use crate::pystr::split_lines;

/// `QWIDGETSIZE_MAX = (1 << 24) - 1`（:26）。
pub const QWIDGETSIZE_MAX: i32 = (1 << 24) - 1;
/// `_expandAnim.setDuration(200)` / `_collapseAnim.setDuration(200)`。
pub const COLLAPSE_DURATION_MS: u32 = 200;
/// 两条动画共用的缓动曲线名（`QEasingCurve.Type.OutCubic`）。
pub const COLLAPSE_EASING: &str = "OutCubic";
/// `headerLayout.setContentsMargins(16, 10, 8, 10)`（:158）。
pub const HEADER_MARGINS: (i32, i32, i32, i32) = (16, 10, 8, 10);
/// `headerLayout.setSpacing(4)`（:159）。
pub const HEADER_SPACING: i32 = 4;
/// `titleLabel.setFixedHeight(26)`（:144）。
pub const TITLE_HEIGHT: i32 = 26;
/// `btn.setFixedSize(30, 30)`（:139）。
pub const MOVE_BUTTON_SIZE: i32 = 30;
/// `btn.setIconSize(QSize(10, 10))`（:140）。
pub const MOVE_ICON_SIZE: i32 = 10;
/// `expandButton.setAngle(0 if collapsed else 180)`（:155）。
pub const EXPAND_ANGLE_COLLAPSED: i32 = 0;
pub const EXPAND_ANGLE_EXPANDED: i32 = 180;
/// `w.hBoxLayout.setStretchFactor(w.vBoxLayout, 1 << 16)`（:186）。
pub const CARD_STRETCH_FACTOR: i32 = 1 << 16;
/// `label.setWordWrap(False)`（:181）。
pub const LABEL_WORD_WRAP: bool = false;

/// Qt `QColor` 的 RGBA 口径（本 crate 统一用元组表达）。
pub type Rgba = (u8, u8, u8, u8);

/// `_normalBackgroundColor`：`QColor(255, 255, 255, 13 if dark else 170)`
/// （:222）——与 `card_groups.py` 的浅色 200 不同，别混用。
pub fn group_background(dark: bool) -> Rgba {
    (255, 255, 255, if dark { 13 } else { 170 })
}

/// `paintEvent` 的分隔线色：`QColor(0, 0, 0, 96 if dark else 24)`（:203）。
pub fn separator_color(dark: bool) -> Rgba {
    (0, 0, 0, if dark { 96 } else { 24 })
}

/// 分隔线矩形 `fillRect(1, cardContainer.y() - 1, width() - 2, 1)`（:203）。
pub fn separator_rect(width: i32, container_y: i32) -> (i32, i32, i32, i32) {
    (1, container_y - 1, width - 2, 1)
}

/// `expandButton.setAngle(0 if collapsed else 180)`。
pub fn expand_angle(collapsed: bool) -> i32 {
    if collapsed {
        EXPAND_ANGLE_COLLAPSED
    } else {
        EXPAND_ANGLE_EXPANDED
    }
}

/// `LabelElideFilter.eventFilter` 的文本结果：逐行 `elide_right` 后用
/// `\n` 拼接。
///
/// Python 的 `obj.text().splitlines() or [""]`：空串走 `[""]`，拼接后仍是
/// 空串，故直接短路返回。行界集合用 `pystr::split_lines`（与 CPython
/// `str.splitlines` 一致）。
pub fn elide_lines(text: &str, width: i32, advance: Advance<'_>) -> String {
    let lines = split_lines(text);
    if lines.is_empty() {
        return String::new();
    }
    lines
        .iter()
        .map(|line| elide_right(line, width, advance))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `cardContainer.setMaximumHeight(0 if collapsed else QWIDGETSIZE_MAX)`
/// （:154）以及 `_onCollapseFinished` 的落点。
pub fn container_max_height(collapsed: bool) -> i32 {
    if collapsed { 0 } else { QWIDGETSIZE_MAX }
}

/// 一次折叠/展开动画的起止值。
///
/// 两条动画的 `start` 都取**当前实测高度**（不是 `maximumHeight`），
/// 这样动画中途再次点击不会跳变。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CollapseAnim {
    pub start: i32,
    pub end: i32,
    pub duration_ms: u32,
    pub easing: &'static str,
}

/// `CollapsibleSettingCard.setExpand` 的动画参数（:91-96）。
///
/// `view_height` = `self.view.height()`，`view_size_hint` =
/// `self.view.sizeHint().height()`——两者都是 Qt 布局实测值，由调用方给。
pub fn expand_anim(view_height: i32, view_size_hint: i32, is_expand: bool) -> CollapseAnim {
    CollapseAnim {
        start: view_height,
        end: if is_expand { view_size_hint } else { 0 },
        duration_ms: COLLAPSE_DURATION_MS,
        easing: COLLAPSE_EASING,
    }
}

/// `CollapsibleSettingCardGroup._setCollapsed` 的动画参数（:243-247）。
pub fn collapse_anim(
    container_height: i32,
    container_size_hint: i32,
    collapsed: bool,
) -> CollapseAnim {
    CollapseAnim {
        start: container_height,
        end: if collapsed { 0 } else { container_size_hint },
        duration_ms: COLLAPSE_DURATION_MS,
        easing: COLLAPSE_EASING,
    }
}

/// `CollapsibleSettingCard` 的数据层：`isExpand` + `view.maximumHeight()`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollapsibleSettingCardState {
    is_expand: bool,
    view_max_height: i32,
}

impl Default for CollapsibleSettingCardState {
    fn default() -> Self {
        Self::new()
    }
}

impl CollapsibleSettingCardState {
    /// 构造：`self.isExpand = False` + `view.setMaximumHeight(0)`（:56 / :71）。
    pub fn new() -> Self {
        Self {
            is_expand: false,
            view_max_height: 0,
        }
    }

    pub fn is_expand(&self) -> bool {
        self.is_expand
    }

    pub fn view_max_height(&self) -> i32 {
        self.view_max_height
    }

    /// `setExpand(isExpand)`：同值直接 return（**不发动画**），否则返回动画。
    pub fn set_expand(
        &mut self,
        is_expand: bool,
        view_height: i32,
        view_size_hint: i32,
    ) -> Option<CollapseAnim> {
        if self.is_expand == is_expand {
            return None;
        }
        self.is_expand = is_expand;
        Some(expand_anim(view_height, view_size_hint, is_expand))
    }

    /// `_onExpandClicked`：`self.setExpand(not self.isExpand)`。
    pub fn toggle(&mut self, view_height: i32, view_size_hint: i32) -> Option<CollapseAnim> {
        self.set_expand(!self.is_expand, view_height, view_size_hint)
    }

    /// `_onExpandFinished`：**只在展开时**解除限高（:102-105）。
    pub fn finish_anim(&mut self) {
        if self.is_expand {
            self.view_max_height = QWIDGETSIZE_MAX;
        }
    }

    /// `addGroupWidget` 是否要先插一条 `GroupSeparator`
    /// （`if self.viewLayout.count() >= 1`，:86）。
    pub fn needs_group_separator(view_count: usize) -> bool {
        view_count >= 1
    }
}

/// `CollapsibleSettingCardGroup` 的数据层：`_collapsed` +
/// `cardContainer.maximumHeight()`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupCollapseState {
    collapsed: bool,
    container_max_height: i32,
}

impl GroupCollapseState {
    /// `_initWidget` 的初值：`collapsed` 由 `cfg.expandedSettingGroups` 推出
    /// （见 `settings_group::is_collapsed`），`maximumHeight` 同步。
    pub fn new(collapsed: bool) -> Self {
        Self {
            collapsed,
            container_max_height: container_max_height(collapsed),
        }
    }

    pub fn collapsed(&self) -> bool {
        self.collapsed
    }

    pub fn container_max_height(&self) -> i32 {
        self.container_max_height
    }

    /// `expandButton.setAngle(...)`：折叠 0°、展开 180°。
    pub fn expand_angle(&self) -> i32 {
        expand_angle(self.collapsed)
    }

    /// `_setCollapsed(collapsed)`：同值直接 return，否则返回动画参数。
    pub fn set_collapsed(
        &mut self,
        collapsed: bool,
        container_height: i32,
        container_size_hint: i32,
    ) -> Option<CollapseAnim> {
        if collapsed == self.collapsed {
            return None;
        }
        self.collapsed = collapsed;
        Some(collapse_anim(
            container_height,
            container_size_hint,
            collapsed,
        ))
    }

    /// `_onCollapseFinished`：**只在展开完成时**解除限高（:251-253）。
    pub fn finish_anim(&mut self) {
        if !self.collapsed {
            self.container_max_height = QWIDGETSIZE_MAX;
        }
    }

    /// `mousePressEvent` 的命中判定：`y < cardContainer.geometry().top()`
    /// （:197）。调用方还须自行判左键。
    pub fn header_hit(y: i32, container_top: i32) -> bool {
        y < container_top
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_advance(text: &str) -> i32 {
        text.chars().count() as i32 * 8
    }

    #[test]
    fn constants_match_python_source() {
        assert_eq!(QWIDGETSIZE_MAX, 16_777_215);
        assert_eq!(COLLAPSE_DURATION_MS, 200);
        assert_eq!(COLLAPSE_EASING, "OutCubic");
        assert_eq!(HEADER_MARGINS, (16, 10, 8, 10));
        assert_eq!(HEADER_SPACING, 4);
        assert_eq!(TITLE_HEIGHT, 26);
        assert_eq!(MOVE_BUTTON_SIZE, 30);
        assert_eq!(MOVE_ICON_SIZE, 10);
        assert_eq!(CARD_STRETCH_FACTOR, 65_536);
        const { assert!(!LABEL_WORD_WRAP) };
    }

    #[test]
    fn theme_colors_differ_from_card_groups_module() {
        assert_eq!(group_background(true), (255, 255, 255, 13));
        // setting_card_group.py 的浅色是 170，card_groups.py 是 200。
        assert_eq!(group_background(false), (255, 255, 255, 170));
        assert_eq!(separator_color(true), (0, 0, 0, 96));
        assert_eq!(separator_color(false), (0, 0, 0, 24));
    }

    #[test]
    fn separator_rect_sits_one_pixel_above_container() {
        // fillRect(1, y - 1, width - 2, 1)
        assert_eq!(separator_rect(300, 40), (1, 39, 298, 1));
        // y = 0 时矩形落在 -1（Qt 会裁掉，Python 照画），照抄。
        assert_eq!(separator_rect(120, 0), (1, -1, 118, 1));
    }

    #[test]
    fn expand_angle_follows_collapsed_state() {
        assert_eq!(expand_angle(true), 0);
        assert_eq!(expand_angle(false), 180);
    }

    #[test]
    fn elide_lines_elides_each_line_independently() {
        // 每行都超出 24px（3 个字符 = 24）：预算 24 - 8 = 16 → 2 字符 + "…"。
        let text = "abcdef\nghijkl";
        assert_eq!(
            elide_lines(text, 24, &fake_advance),
            "ab\u{2026}\ngh\u{2026}"
        );
    }

    #[test]
    fn elide_lines_keeps_short_lines_and_normalizes_line_breaks() {
        // "a\r\nb" 的 splitlines → ["a", "b"]，拼接恒用 "\n"。
        assert_eq!(elide_lines("a\r\nb", 100, &fake_advance), "a\nb");
        assert_eq!(elide_lines("abc", 100, &fake_advance), "abc");
    }

    #[test]
    fn elide_lines_empty_text_stays_empty() {
        assert_eq!(elide_lines("", 100, &fake_advance), "");
        // 只有换行：splitlines → [""] → 拼接回空串。
        assert_eq!(elide_lines("\n", 100, &fake_advance), "");
    }

    #[test]
    fn expand_anim_starts_at_current_height() {
        let open = expand_anim(0, 240, true);
        assert_eq!((open.start, open.end), (0, 240));
        assert_eq!(open.duration_ms, 200);
        assert_eq!(open.easing, "OutCubic");

        // 折叠的 end 恒为 0，与 sizeHint 无关。
        let close = expand_anim(240, 240, false);
        assert_eq!((close.start, close.end), (240, 0));
    }

    #[test]
    fn card_set_expand_is_idempotent() {
        let mut state = CollapsibleSettingCardState::new();
        assert!(!state.is_expand());
        assert_eq!(state.view_max_height(), 0);
        assert_eq!(state.set_expand(false, 0, 240), None);

        let anim = state.set_expand(true, 0, 240).expect("should animate");
        assert_eq!((anim.start, anim.end), (0, 240));
        assert!(state.is_expand());
        assert_eq!(state.set_expand(true, 240, 240), None);
    }

    #[test]
    fn card_finish_anim_only_relaxes_when_expanded() {
        let mut state = CollapsibleSettingCardState::new();
        state.set_expand(true, 0, 240);
        state.finish_anim();
        assert_eq!(state.view_max_height(), QWIDGETSIZE_MAX);

        // 折叠完成时不改 maximumHeight（动画已把它收到 0）。
        state.set_expand(false, 240, 240);
        state.finish_anim();
        assert_eq!(state.view_max_height(), QWIDGETSIZE_MAX);
        assert!(!state.is_expand());
    }

    #[test]
    fn toggle_mirrors_on_expand_clicked() {
        let mut state = CollapsibleSettingCardState::new();
        assert_eq!(state.toggle(0, 90).map(|anim| anim.end), Some(90));
        assert_eq!(state.toggle(90, 90).map(|anim| anim.end), Some(0));
    }

    #[test]
    fn group_separator_only_from_second_widget() {
        assert!(!CollapsibleSettingCardState::needs_group_separator(0));
        assert!(CollapsibleSettingCardState::needs_group_separator(1));
        assert!(CollapsibleSettingCardState::needs_group_separator(2));
    }

    #[test]
    fn group_state_starts_from_expanded_groups_membership() {
        let collapsed = GroupCollapseState::new(true);
        assert!(collapsed.collapsed());
        assert_eq!(collapsed.container_max_height(), 0);
        assert_eq!(collapsed.expand_angle(), 0);

        let expanded = GroupCollapseState::new(false);
        assert!(!expanded.collapsed());
        assert_eq!(expanded.container_max_height(), QWIDGETSIZE_MAX);
        assert_eq!(expanded.expand_angle(), 180);
    }

    #[test]
    fn group_set_collapsed_is_idempotent_and_animates_from_height() {
        let mut state = GroupCollapseState::new(true);
        assert_eq!(state.set_collapsed(true, 0, 400), None);

        let anim = state.set_collapsed(false, 0, 400).expect("should animate");
        assert_eq!((anim.start, anim.end), (0, 400));
        assert_eq!(anim.easing, "OutCubic");
        state.finish_anim();
        assert_eq!(state.container_max_height(), QWIDGETSIZE_MAX);

        let anim = state.set_collapsed(true, 400, 400).expect("should animate");
        assert_eq!((anim.start, anim.end), (400, 0));
        state.finish_anim();
        assert_eq!(state.container_max_height(), QWIDGETSIZE_MAX);
        assert!(state.collapsed());
    }

    #[test]
    fn header_hit_only_above_container() {
        assert!(GroupCollapseState::header_hit(10, 40));
        assert!(!GroupCollapseState::header_hit(40, 40));
        assert!(!GroupCollapseState::header_hit(60, 40));
        // 容器贴顶（折叠态）时任何 y >= 0 都不算头部。
        assert!(!GroupCollapseState::header_hit(0, 0));
    }
}
