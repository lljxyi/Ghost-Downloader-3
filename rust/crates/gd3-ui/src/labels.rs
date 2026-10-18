//! 组件库：标签族（镜像 `app/view/components/labels.py`，176 行）。
//!
//! 语义镜像范围（Qt 绘制/几何里能落成「可断言事实」的那部分）：
//!
//! - `ElidedLabel.paintEvent`：`elidedText(text, ElideRight, width())`；
//! - `IconLabelBase._initIcon` / `setIcon`：`setMinimumHeight(max(size,
//!   fontMetrics().height()))`、`setIndent(iconSize + 4 if icon else 0)`；
//! - `IconLabelBase.paintEvent`：图标 `yOffset = (height - iconSize) // 2`，
//!   类级 `_iconCache` 以 `id(icon)` 为键（同一图标对象只 `pixmap()` 一次）；
//! - `IconStrongBodyLabel._elide`：`width = contentsRect().width() - indent()`，
//!   `width > 0` 才省略，否则原文；`toolTip = 全文（省略时才设，否则空串）`；
//! - `EditableLabel`：150ms `OutCubic` 下划线动画、`hideEvent` 归零、
//!   左键双击发 `editRequested`、下划线几何与笔色（深色主题白、浅色黑）。
//!
//! 不镜像：`QPainter` 实际绘制、`QPropertyAnimation` 的时钟（这里只给
//! 「从哪到哪、多久、什么曲线」，帧推进归 Slint 的 `animate`）。
//!
//! 省略算法：Qt 的 `QFontMetrics::elidedText` 需要字形宽度表，本 crate
//! 不碰字体，故把「测宽」做成入参闭包 `advance(&str) -> i32`；算法本身
//! 照抄 Qt：放得下就原文、`width <= 0` 就原文、否则在 `width - 省略号宽`
//! 内二分找最长前缀再接 `…`（U+2026，单字符）。

use std::collections::HashMap;

/// `setIndent(self.iconSize + 4 if icon is not None else 0)` 里的 4。
pub const ICON_INDENT_GAP: i32 = 4;
/// `self._underlineAnim.setDuration(150)`。
pub const UNDERLINE_DURATION_MS: u64 = 150;
/// `self._underlineProgress = 0.0`（构造与 `hideEvent` 的落点）。
pub const UNDERLINE_IDLE_PROGRESS: f64 = 0.0;
/// `Qt.TextElideMode.ElideRight` 的省略号——单码点，不是三个句点。
pub const ELLIPSIS: char = '…';

/// 测宽函数签名（Qt 的 `fontMetrics().horizontalAdvance(text)`）。
///
/// 传 `&str` 而非 `char` 是为了让调用方能按整段文字缓存宽度。
pub type Advance<'a> = &'a dyn Fn(&str) -> i32;

/// `QFontMetrics::elidedText(text, ElideRight, width)` 的等价实现。
///
/// 与 Qt 逐条对齐的三条短路：
/// 1. 空串 → 空串；
/// 2. `width <= 0` → **原文**（Qt 在 `width <= 0` 时不省略，不是截成空串）；
/// 3. 整段放得下（`advance(text) <= width`）→ 原文。
///
/// 否则在 `width - advance("…")` 的预算里二分出最长可容纳前缀。预算为负
/// （连省略号都放不下）时只返回省略号本身——Qt 在同样场景下也只留省略号
/// （见测试 `ellipsis_alone_when_budget_is_negative`）。
pub fn elide_right(text: &str, width: i32, advance: Advance<'_>) -> String {
    if text.is_empty() || width <= 0 {
        return text.to_string();
    }
    if advance(text) <= width {
        return text.to_string();
    }
    let ellipsis = ELLIPSIS.to_string();
    let budget = width - advance(&ellipsis);
    if budget < 0 {
        return ellipsis;
    }
    // 二分：最大的 k 使 advance(前 k 个码点) <= budget。
    let chars: Vec<char> = text.chars().collect();
    let (mut low, mut high) = (0usize, chars.len());
    while low < high {
        let mid = (low + high).div_ceil(2);
        let prefix: String = chars[..mid].iter().collect();
        if advance(&prefix) <= budget {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    let prefix: String = chars[..low].iter().collect();
    format!("{prefix}{ELLIPSIS}")
}

/// `IconLabelBase._initIcon` 的 `setMinimumHeight(max(size, fm.height()))`。
pub fn icon_minimum_height(size: i32, font_metrics_height: i32) -> i32 {
    size.max(font_metrics_height)
}

/// `IconLabelBase.setIcon` 的 `setIndent(...)`：**有图标才有缩进**，
/// 且缩进是 `iconSize + 4`（图标占位 + 间隙），不是 `iconSize`。
pub fn icon_indent(icon_size: i32, has_icon: bool) -> i32 {
    if has_icon {
        icon_size + ICON_INDENT_GAP
    } else {
        0
    }
}

/// `IconLabelBase.paintEvent` 的 `yOffset = (self.height() - self.iconSize) // 2`。
///
/// Python 的 `//` 对负数向下取整（`-3 // 2 == -2`），Rust 的 `/` 向零截断，
/// 故显式 `div_euclid`——图标比标签高时两者结果不同。
pub fn icon_y_offset(height: i32, icon_size: i32) -> i32 {
    (height - icon_size).div_euclid(2)
}

/// 类级 `_iconCache`（键是 `id(self.icon)`）的等价物。
///
/// Qt 侧缓存的是「同一图标对象只渲染一次」；Rust 侧没有对象身份，用调用方
/// 给的键（通常是 `FluentIcon` 枚举值转 `usize`）。`clear_cache` 镜像
/// `IconLabelBase.clearCache`（主题切换时由 `qconfig` 信号触发）。
#[derive(Debug, Default)]
pub struct IconCache {
    entries: HashMap<usize, usize>,
}

impl IconCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// 命中就返回旧 id；否则调 `render` 生成并登记（返回其 id）。
    ///
    /// `render` 只在首次调用——测试用「计数闭包」锁住这一点。
    pub fn pixmap_id(&mut self, icon_id: usize, render: impl FnOnce() -> usize) -> usize {
        if let Some(cached) = self.entries.get(&icon_id) {
            return *cached;
        }
        let pixmap = render();
        self.entries.insert(icon_id, pixmap);
        pixmap
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

/// `IconStrongBodyLabel`（可省略 + 全文 tooltip）。
///
/// 状态只有「全文 + 图标尺寸/有无」；省略结果是 `(宽度, 测宽函数)` 的纯函数，
/// 不缓存——Qt 侧在 `setText` / `resizeEvent` 时重算，Rust 侧由 Slint 的
/// 绑定在宽度变化时重调。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IconStrongBodyLabel {
    pub full_text: String,
    pub icon_size: i32,
    pub has_icon: bool,
}

impl IconStrongBodyLabel {
    pub fn new(full_text: impl Into<String>, size: i32) -> Self {
        Self {
            full_text: full_text.into(),
            icon_size: size,
            has_icon: false,
        }
    }

    /// `setIcon`：缩进随「有无图标」翻转，`iconSize` 只在构造时定。
    pub fn set_icon(&mut self, has_icon: bool) {
        self.has_icon = has_icon;
    }

    pub fn indent(&self) -> i32 {
        icon_indent(self.icon_size, self.has_icon)
    }

    /// `_elide` 的显示文本：`width = contentsRect().width() - indent()`，
    /// `width > 0` 才省略，否则**原文**（Qt 侧此时也不设 tooltip）。
    pub fn display_text(&self, contents_width: i32, advance: Advance<'_>) -> String {
        let width = contents_width - self.indent();
        if width > 0 {
            elide_right(&self.full_text, width, advance)
        } else {
            self.full_text.clone()
        }
    }

    /// `setToolTip(self._fullText if elided != self._fullText else "")`。
    pub fn tooltip(&self, contents_width: i32, advance: Advance<'_>) -> String {
        let display = self.display_text(contents_width, advance);
        if display != self.full_text {
            self.full_text.clone()
        } else {
            String::new()
        }
    }
}

/// `ElidedLabel`：`paintEvent` 每次按当前宽度重算省略文本（无状态）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ElidedLabel {
    pub text: String,
}

impl ElidedLabel {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    pub fn display_text(&self, width: i32, advance: Advance<'_>) -> String {
        elide_right(&self.text, width, advance)
    }
}

/// `QEasingCurve.Type.OutCubic`：`1 - (1 - t)^3`。
pub fn ease_out_cubic(t: f64) -> f64 {
    let inv = 1.0 - t.clamp(0.0, 1.0);
    1.0 - inv * inv * inv
}

/// `_animateTo(end)` 下发的一次动画（`stop()` + `setStartValue(当前)` +
/// `setEndValue(end)` + `start()`）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnderlineAnimation {
    pub start_value: f64,
    pub end_value: f64,
    pub duration_ms: u64,
}

impl UnderlineAnimation {
    /// 按进度求当前值（`OutCubic`）——Slint 侧动画推进时的取值口径。
    pub fn value_at(&self, progress: f64) -> f64 {
        let eased = ease_out_cubic(progress);
        self.start_value + (self.end_value - self.start_value) * eased
    }
}

/// `EditableLabel`（悬停下划线 + 双击进编辑）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditableLabel {
    pub text: String,
    pub underline_progress: f64,
}

impl EditableLabel {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            underline_progress: UNDERLINE_IDLE_PROGRESS,
        }
    }

    /// `_elide`：`width = contentsRect().width()`——**不减缩进**（本类
    /// 没有图标），与 `IconStrongBodyLabel` 差一个 `indent()`。
    pub fn display_text(&self, contents_width: i32, advance: Advance<'_>) -> String {
        if contents_width > 0 {
            elide_right(&self.text, contents_width, advance)
        } else {
            self.text.clone()
        }
    }

    pub fn tooltip(&self, contents_width: i32, advance: Advance<'_>) -> String {
        let display = self.display_text(contents_width, advance);
        if display != self.text {
            self.text.clone()
        } else {
            String::new()
        }
    }

    /// `enterEvent` / `leaveEvent` 的落点（`_animateTo(1.0)` / `_animateTo(0.0)`）。
    pub fn animate_to(&self, end_value: f64) -> UnderlineAnimation {
        UnderlineAnimation {
            start_value: self.underline_progress,
            end_value,
            duration_ms: UNDERLINE_DURATION_MS,
        }
    }

    pub fn enter(&self) -> UnderlineAnimation {
        self.animate_to(1.0)
    }

    pub fn leave(&self) -> UnderlineAnimation {
        self.animate_to(0.0)
    }

    /// `hideEvent`：`stop()` + 进度归零（**不等动画播完**）。
    pub fn hide(&mut self) {
        self.underline_progress = UNDERLINE_IDLE_PROGRESS;
    }

    /// `mouseDoubleClickEvent`：只认左键，其余按键交给基类（不发信号）。
    pub fn double_click_requests_edit(left_button: bool) -> bool {
        left_button
    }

    /// `paintEvent` 的下划线纵坐标：
    /// `rect.top() + (rect.height() - fm.height()) // 2 + fm.ascent() + 2`。
    ///
    /// 三处 `//` 都按 Python 语义（向下取整）；`+ 2` 是基线下的留白。
    pub fn underline_y(rect_top: i32, rect_height: i32, fm_height: i32, fm_ascent: i32) -> i32 {
        rect_top + (rect_height - fm_height).div_euclid(2) + fm_ascent + 2
    }

    /// 下划线长度：`int(min(advance(全文), rect.width()) * progress)`。
    ///
    /// 注意入参是 `super().text()`——即**已省略后的显示文本**，不是全文。
    pub fn underline_length(display_advance: i32, rect_width: i32, progress: f64) -> i32 {
        let text_width = display_advance.min(rect_width);
        (text_width as f64 * progress) as i32
    }

    /// `paintEvent` 的早退：`if self._underlineProgress <= 0: return`。
    pub fn draws_underline(progress: f64) -> bool {
        progress > 0.0
    }

    /// 笔色：`QColor(255,255,255) if isDarkTheme() else QColor(0,0,0)`。
    pub fn underline_color(dark: bool) -> (u8, u8, u8) {
        if dark { (255, 255, 255) } else { (0, 0, 0) }
    }

    /// `QPen(color, 1)` 的宽度。
    pub const PEN_WIDTH: i32 = 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 等宽测宽器：每个码点 10 像素——二分结果可手算。
    fn per_char(px: i32) -> impl Fn(&str) -> i32 {
        move |text: &str| text.chars().count() as i32 * px
    }

    #[test]
    fn elide_short_circuits_empty_zero_width_and_fitting_text() {
        let advance = per_char(10);
        assert_eq!(elide_right("", 100, &advance), "");
        // width <= 0 → 原文（Qt 语义，不是空串）。
        assert_eq!(elide_right("abcdef", 0, &advance), "abcdef");
        assert_eq!(elide_right("abcdef", -5, &advance), "abcdef");
        // 恰好放得下 → 原文（`<=`，边界含等号）。
        assert_eq!(elide_right("abcdef", 60, &advance), "abcdef");
    }

    #[test]
    fn elide_keeps_the_longest_prefix_that_fits_with_the_ellipsis() {
        let advance = per_char(10);
        // 6 字符 60px 超过 50px；省略号 10px → 预算 40px → 前 4 字符 + …。
        assert_eq!(elide_right("abcdef", 50, &advance), "abcd…");
        assert_eq!(elide_right("abcdef", 59, &advance), "abcd…");
        assert_eq!(elide_right("abcdef", 61, &advance), "abcdef");
    }

    #[test]
    fn ellipsis_alone_when_budget_is_negative() {
        let advance = per_char(10);
        // 宽度 5px 连省略号（10px）都放不下 → 只剩省略号。
        assert_eq!(elide_right("abcdef", 5, &advance), "…");
    }

    #[test]
    fn elide_counts_codepoints_not_bytes() {
        // 每个码点 10px：3 个汉字 = 30px，宽度 25px → 预算 15px → 1 个汉字 + …。
        let advance = per_char(10);
        assert_eq!(elide_right("中文测试", 25, &advance), "中…");
    }

    #[test]
    fn icon_geometry_matches_python_floor_division() {
        assert_eq!(icon_minimum_height(16, 20), 20);
        assert_eq!(icon_minimum_height(24, 20), 24);
        // 有图标才缩进，且是 iconSize + 4。
        assert_eq!(icon_indent(16, true), 20);
        assert_eq!(icon_indent(16, false), 0);
        // 正数两者一致，负数按 Python 向下取整（-3 // 2 == -2）。
        assert_eq!(icon_y_offset(40, 16), 12);
        assert_eq!(icon_y_offset(14, 16), -1);
        assert_eq!(icon_y_offset(13, 16), -2);
    }

    #[test]
    fn icon_cache_renders_once_per_key_and_clears() {
        let mut cache = IconCache::new();
        let mut renders = 0;
        let first = cache.pixmap_id(7, || {
            renders += 1;
            100
        });
        let again = cache.pixmap_id(7, || {
            renders += 1;
            200
        });
        assert_eq!((first, again), (100, 100));
        assert_eq!(renders, 1);
        assert_eq!(cache.len(), 1);
        // 另一个键独立登记。
        assert_eq!(cache.pixmap_id(8, || 300), 300);
        assert_eq!(cache.len(), 2);
        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn icon_strong_body_label_elides_minus_indent_and_sets_tooltip() {
        let advance = per_char(10);
        let mut label = IconStrongBodyLabel::new("abcdef", 16);
        // 无图标：indent 0 → 宽度 50 可用 → 省略到 4 字符 + …。
        assert_eq!(label.display_text(50, &advance), "abcd…");
        assert_eq!(label.tooltip(50, &advance), "abcdef");
        // 有图标：indent = 20 → 可用 30 → 预算 20 → 2 字符 + …。
        label.set_icon(true);
        assert_eq!(label.indent(), 20);
        assert_eq!(label.display_text(50, &advance), "ab…");
        // 可用宽度 <= 0（contents 20 - indent 20 = 0）→ 原文，tooltip 也空。
        assert_eq!(label.display_text(20, &advance), "abcdef");
        assert_eq!(label.tooltip(20, &advance), "");
    }

    #[test]
    fn elided_label_has_no_indent_and_never_sets_tooltip() {
        let advance = per_char(10);
        let label = ElidedLabel::new("abcdef");
        assert_eq!(label.display_text(50, &advance), "abcd…");
        assert_eq!(label.display_text(0, &advance), "abcdef");
        // 结构体本身没有 tooltip 字段——`ElidedLabel` 不装 ToolTipFilter。
    }

    #[test]
    fn editable_label_animation_uses_out_cubic_and_resets_on_hide() {
        let mut label = EditableLabel::new("name");
        let enter = label.enter();
        assert_eq!(enter.start_value, 0.0);
        assert_eq!(enter.end_value, 1.0);
        assert_eq!(enter.duration_ms, 150);
        // OutCubic 端点：t=0 → 起点、t=1 → 终点、t=0.5 → 0.875。
        assert_eq!(enter.value_at(0.0), 0.0);
        assert_eq!(enter.value_at(1.0), 1.0);
        assert!((enter.value_at(0.5) - 0.875).abs() < 1e-9);
        // 中途离开：起点是当前值（不是 1.0）。
        label.underline_progress = 0.4;
        let leave = label.leave();
        assert_eq!(leave.start_value, 0.4);
        assert_eq!(leave.end_value, 0.0);
        // hideEvent 归零，不等动画播完。
        label.hide();
        assert_eq!(label.underline_progress, 0.0);
    }

    #[test]
    fn editable_label_geometry_and_paint_gate() {
        // rect(0,100)、fm 高 16 上升 12 → 0 + (100-16)//2 + 12 + 2 = 56。
        assert_eq!(EditableLabel::underline_y(0, 100, 16, 12), 56);
        // 负偏移按 Python 向下取整：(15-16)//2 = -1。
        assert_eq!(EditableLabel::underline_y(0, 15, 16, 12), 13);
        // 只认左键。
        assert!(EditableLabel::double_click_requests_edit(true));
        assert!(!EditableLabel::double_click_requests_edit(false));
        // 下划线长度 = int(min(advance, rect.width) * progress)。
        assert_eq!(EditableLabel::underline_length(40, 100, 0.5), 20);
        assert_eq!(EditableLabel::underline_length(200, 100, 0.5), 50);
        assert_eq!(EditableLabel::underline_length(40, 100, 0.0), 0);
        // `<= 0` 不画。
        assert!(!EditableLabel::draws_underline(0.0));
        assert!(!EditableLabel::draws_underline(-0.1));
        assert!(EditableLabel::draws_underline(0.01));
        assert_eq!(EditableLabel::underline_color(true), (255, 255, 255));
        assert_eq!(EditableLabel::underline_color(false), (0, 0, 0));
        assert_eq!(EditableLabel::PEN_WIDTH, 1);
    }

    #[test]
    fn editable_label_elides_on_full_contents_width() {
        let advance = per_char(10);
        let label = EditableLabel::new("abcdef");
        assert_eq!(label.display_text(50, &advance), "abcd…");
        assert_eq!(label.tooltip(50, &advance), "abcdef");
        assert_eq!(label.display_text(60, &advance), "abcdef");
        assert_eq!(label.tooltip(60, &advance), "");
        assert_eq!(label.display_text(0, &advance), "abcdef");
    }
}
