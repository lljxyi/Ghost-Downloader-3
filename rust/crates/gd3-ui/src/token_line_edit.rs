//! 镜像 `app/view/components/token_line_edit.py`（273 行）：令牌输入框
//! （`TokenWidget` 圆角标签 + `TokenInput` 单行输入 + `TokenLineEdit` 流式布局）。
//!
//! Qt 侧是三个自绘控件，本模块把「布局尺寸 + 输入状态机」算成纯数据：
//!
//! - [`TokenWidgetState`]：单个令牌的文本与 `elide()` 后的最大宽度；
//! - [`TokenLineEdit`]：令牌列表、输入框文本、焦点状态、上限与可见性；
//! - [`TokenLineEdit::key_press`]：回车/小键盘回车/空格提交、空文本退格删末尾；
//! - [`TokenLineEdit::focus_out`]：失焦时把残留文本提交成令牌；
//! - [`TokenLineEdit::underline`]：聚焦时底部 2px 主题色带（圆角矩形减矩形）。
//!
//! ## 实证结论（逐条对着源码抄的）
//!
//! - `TokenWidget.elide(maxWidth)`：`available = maxWidth - 8 - 4 - 2 -
//!   closeButton.width()` = `maxWidth - 34`（左内边距 8、右内边距 4、
//!   间距 2、关闭按钮 20）；**同时** `setMaximumWidth(maxWidth)`，所以
//!   即使 `available <= 0`（`elidedText` 会原样返回全文）标签也不会被截断，
//!   而是被外层最大宽度裁掉。
//! - `_refreshTokenWidths`：`maxWidth = self.width() - 12`，**只有
//!   `maxWidth > 0` 才遍历 elide**；宽度不足 13 时令牌**完全不动**
//!   （不会退化成 0 或负数）。
//! - `setTokens` **不去重、不查上限**：直接 `_clearAll()` 后逐个
//!   `_insertToken`，最后只发一次 `tokensChanged`。
//! - `_addToken` 才是「查上限 → 查重」的入口：`maxTokenCount > 0 and
//!   len(tokens) >= maxTokenCount` 先返回（**上限优先于查重**，两者都
//!   不插入、不发信号）。
//! - `TokenInput.keyPressEvent`：回车/小键盘回车/空格都走提交分支，
//!   文本 `strip()` 后非空才 `submitted.emit` + `clear()`；**无论是否
//!   提交都 `return`**（空格因此永远打不进输入框）。
//! - `eventFilter` 的退格分支：`event.key() == Key_Backspace and not
//!   lineEdit.text()` 时删末尾令牌并 **`return True`**（即使没有令牌可删，
//!   也把事件吃掉，不交给输入框）。
//! - `FocusOut`：先置 `_hasFocus = False` 再提交残留文本；`_addToken`
//!   可能因上限/查重不插入，但 `lineEdit.clear()` **无条件执行**。
//! - `_refreshInputVisibility`：`maxTokenCount <= 0 or len(tokens) <
//!   maxTokenCount`；`setMaxTokenCount` 会立刻重算可见性。
//! - `paintEvent`：未聚焦直接 `return`；聚焦时画
//!   `roundedRect(0, h-10, w, 10, 5)` 减去 `rect(0, h-10, w, 8)`，
//!   即底部 `2px` 的主题色带（左右下角按半径裁圆，实际半径被 Qt 夹到
//!   `height/2 = 1`）。
//! - `resizeEvent` 只做 `_refreshTokenWidths()`（不重排，重排靠
//!   `FlowLayout` 自己的 `invalidate()`）。
//! - `_onThemeChanged` 只对每个令牌 `update()`，不改状态。

use crate::labels::{Advance, elide_right};
use crate::pystr::py_strip;

/// `TokenWidget.setFixedHeight(24)`。
pub const TOKEN_HEIGHT: i32 = 24;
/// `closeButton.setFixedSize(20, 20)`。
pub const CLOSE_SIZE: i32 = 20;
/// `closeButton.setIconSize(QSize(8, 8))`。
pub const CLOSE_ICON_SIZE: i32 = 8;
/// 令牌布局 `setContentsMargins(8, 0, 4, 0)` 的左值。
pub const TOKEN_MARGIN_LEFT: i32 = 8;
/// 令牌布局的右值。
pub const TOKEN_MARGIN_RIGHT: i32 = 4;
/// 令牌布局 `setSpacing(2)`。
pub const TOKEN_SPACING: i32 = 2;
/// `drawRoundedRect(self.rect(), 4, 4)`。
pub const TOKEN_RADIUS: i32 = 4;
/// `elide()` 里被固定占用的宽度合计 `8 + 4 + 2 + 20`。
pub const TOKEN_ELIDE_SLACK: i32 =
    TOKEN_MARGIN_LEFT + TOKEN_MARGIN_RIGHT + TOKEN_SPACING + CLOSE_SIZE;
/// `TokenInput.setMinimumWidth(60)`。
pub const INPUT_MIN_WIDTH: i32 = 60;
/// `TokenInput.setFixedHeight(24)`。
pub const INPUT_HEIGHT: i32 = 24;
/// `TokenLineEdit.setMinimumHeight(33)`。
pub const LINE_MIN_HEIGHT: i32 = 33;
/// `flowLayout.setContentsMargins(4, 4, 4, 4)`。
pub const FLOW_MARGIN: i32 = 4;
/// `flowLayout.setHorizontalSpacing(4)` / `setVerticalSpacing(4)`。
pub const FLOW_SPACING: i32 = 4;
/// `_refreshTokenWidths` 的 `self.width() - 12`。
pub const TOKEN_WIDTH_SLACK: i32 = 12;
/// `barPath.addRoundedRect(QRectF(0, h - 10, w, 10), 5, 5)` 的高度。
pub const UNDERLINE_HEIGHT: i32 = 10;
/// 下划线圆角半径 `5`。
pub const UNDERLINE_RADIUS: i32 = 5;
/// `rectPath.addRect(0, h - 10, w, 8)` 的高度（被减掉的部分）。
pub const UNDERLINE_CUT: i32 = 8;
/// `self.lineEdit.setPlaceholderText(self.tr("添加..."))`。
pub const DEFAULT_PLACEHOLDER: &str = "添加...";

/// Qt `QColor` 的 `(r, g, b, a)`。
pub type Rgba = (u8, u8, u8, u8);

/// `TokenWidget.paintEvent` 的底色：深色 `(255,255,255,20)`，浅色 `(0,0,0,12)`。
pub fn token_background(dark: bool) -> Rgba {
    if dark {
        (255, 255, 255, 20)
    } else {
        (0, 0, 0, 12)
    }
}

/// `TokenWidget.elide` 传给 `elidedText` 的可用宽度。
pub fn token_elide_width(max_width: i32) -> i32 {
    max_width - TOKEN_ELIDE_SLACK
}

/// 令牌标签在给定最大宽度下的显示文本（`elidedText(ElideRight, available)`）。
pub fn token_label_text(text: &str, max_width: Option<i32>, advance: Advance<'_>) -> String {
    match max_width {
        Some(max_width) => elide_right(text, token_elide_width(max_width), advance),
        None => text.to_string(),
    }
}

/// `TokenWidget` 的状态：原文 + `setMaximumWidth` 的值。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenWidgetState {
    text: String,
    max_width: Option<i32>,
}

impl TokenWidgetState {
    /// `TokenWidget(text)`：初始没有最大宽度（未 elide 过）。
    pub fn new(text: &str) -> Self {
        Self {
            text: text.to_string(),
            max_width: None,
        }
    }

    /// `text()` 返回**原文**（不受 elide 影响）。
    pub fn text(&self) -> &str {
        &self.text
    }

    /// `setMaximumWidth` 的值；`None` = 从未 elide。
    pub fn max_width(&self) -> Option<i32> {
        self.max_width
    }

    /// `elide(maxWidth)`：先算标签文本，再 `setMaximumWidth(maxWidth)`。
    pub fn elide(&mut self, max_width: i32) {
        self.max_width = Some(max_width);
    }

    /// 当前显示文本（`label.text()`）。
    pub fn label_text(&self, advance: Advance<'_>) -> String {
        token_label_text(&self.text, self.max_width, advance)
    }
}

/// 输入框里能触发行为的按键（其余按键不改变令牌状态）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKey {
    /// `Qt.Key_Return`。
    Return,
    /// `Qt.Key_Enter`（小键盘）。
    Enter,
    /// `Qt.Key_Space`。
    Space,
    /// `Qt.Key_Backspace`。
    Backspace,
    /// 其它按键：交给 `QLineEdit` 自己处理。
    Other,
}

impl TokenKey {
    /// 是否走 `TokenInput.keyPressEvent` 的提交分支。
    pub fn submits(self) -> bool {
        matches!(self, Self::Return | Self::Enter | Self::Space)
    }
}

/// `key_press` 的结果。
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct KeyOutcome {
    /// 是否 `return True`（吃掉事件，不再交给输入框）。
    pub consumed: bool,
    /// 本次调用后 `tokensChanged` 的载荷；`None` = 没发信号。
    pub tokens_changed: Option<Vec<String>>,
}

/// 聚焦下划线的几何：`roundedRect` 减 `rect` 之后剩下的底部色带。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnderlineGeometry {
    /// 左边界（恒为 0）。
    pub x: i32,
    /// 上边界 = `height - UNDERLINE_HEIGHT + UNDERLINE_CUT`。
    pub y: i32,
    /// 宽度 = 控件宽。
    pub width: i32,
    /// 高度 = `UNDERLINE_HEIGHT - UNDERLINE_CUT` = 2。
    pub height: i32,
    /// 有效圆角半径（被夹到 `height / 2`）。
    pub radius: i32,
}

/// `TokenLineEdit` 的状态机。
#[derive(Clone, Debug)]
pub struct TokenLineEdit {
    tokens: Vec<String>,
    widgets: Vec<TokenWidgetState>,
    max_token_count: i32,
    has_focus: bool,
    input_text: String,
    placeholder: String,
    width: i32,
}

impl TokenLineEdit {
    /// `TokenLineEdit(parent, *, maxTokenCount=0)`；`0` 表示不限。
    pub fn new(max_token_count: i32) -> Self {
        Self {
            tokens: Vec::new(),
            widgets: Vec::new(),
            max_token_count,
            has_focus: false,
            input_text: String::new(),
            placeholder: DEFAULT_PLACEHOLDER.to_string(),
            width: 0,
        }
    }

    /// `tokens()`：返回副本，顺序 = 插入顺序。
    pub fn tokens(&self) -> Vec<String> {
        self.tokens.clone()
    }

    /// 令牌控件列表（与 [`Self::tokens`] 一一对应）。
    pub fn widgets(&self) -> &[TokenWidgetState] {
        &self.widgets
    }

    /// `_hasFocus`。
    pub fn has_focus(&self) -> bool {
        self.has_focus
    }

    /// `lineEdit.text()`。
    pub fn input_text(&self) -> &str {
        &self.input_text
    }

    /// 占位文本（默认「添加...」）。
    pub fn placeholder(&self) -> &str {
        &self.placeholder
    }

    /// `_maxTokenCount`。
    pub fn max_token_count(&self) -> i32 {
        self.max_token_count
    }

    /// `lineEdit.isVisible()`：还能不能再加令牌。
    pub fn input_visible(&self) -> bool {
        self.max_token_count <= 0 || (self.tokens.len() as i32) < self.max_token_count
    }

    /// `setPlaceholderText(text)`。
    pub fn set_placeholder_text(&mut self, text: &str) {
        self.placeholder = text.to_string();
    }

    /// `setMaxTokenCount(count)`：立刻重算输入框可见性。
    pub fn set_max_token_count(&mut self, count: i32) {
        self.max_token_count = count;
    }

    /// 模拟用户在输入框里打字（Qt 负责文本编辑，本侧只保留结果）。
    pub fn set_input_text(&mut self, text: &str) {
        self.input_text = text.to_string();
    }

    /// `resizeEvent` → `_refreshTokenWidths()`：`maxWidth = width - 12`，
    /// 只有 `maxWidth > 0` 才逐个 elide。
    pub fn set_width(&mut self, width: i32) {
        self.width = width;
        let max_width = width - TOKEN_WIDTH_SLACK;
        if max_width > 0 {
            for widget in &mut self.widgets {
                widget.elide(max_width);
            }
        }
    }

    /// 当前控件宽度（`resizeEvent` 记录值）。
    pub fn width(&self) -> i32 {
        self.width
    }

    /// `setTokens(tokens)`：清空后逐个插入（**不去重、不查上限**），
    /// 最后发一次 `tokensChanged`。
    pub fn set_tokens(&mut self, tokens: &[String]) -> Vec<String> {
        self.tokens.clear();
        self.widgets.clear();
        for token in tokens {
            self.insert_token(token);
        }
        self.tokens()
    }

    /// `_addToken(text)`：先查上限、再查重；插入成功才返回 `Some`。
    pub fn add_token(&mut self, text: &str) -> Option<Vec<String>> {
        if self.max_token_count > 0 && (self.tokens.len() as i32) >= self.max_token_count {
            return None;
        }
        if self.tokens.iter().any(|token| token == text) {
            return None;
        }
        self.insert_token(text);
        Some(self.tokens())
    }

    /// `_insertToken(text)`：追加令牌 + 新控件（插在输入框之前），
    /// 然后重算宽度与可见性。**不发信号**。
    fn insert_token(&mut self, text: &str) {
        self.tokens.push(text.to_string());
        self.widgets.push(TokenWidgetState::new(text));
        let max_width = self.width - TOKEN_WIDTH_SLACK;
        if max_width > 0
            && let Some(widget) = self.widgets.last_mut()
        {
            widget.elide(max_width);
        }
    }

    /// `_removeToken(widget)` 按索引版本：删令牌 + 删控件，返回新列表。
    pub fn remove_index(&mut self, index: usize) -> Option<Vec<String>> {
        if index >= self.tokens.len() {
            return None;
        }
        self.tokens.remove(index);
        self.widgets.remove(index);
        Some(self.tokens())
    }

    /// 删末尾令牌（`_removeToken(self._tokenWidgets[-1])`）。
    pub fn remove_last(&mut self) -> Option<Vec<String>> {
        match self.tokens.len() {
            0 => None,
            len => self.remove_index(len - 1),
        }
    }

    /// `TokenInput.keyPressEvent` + `TokenLineEdit.eventFilter` 的合并语义。
    ///
    /// 提交分支（回车/小键盘回车/空格）恒 `consumed = true`；退格仅在
    /// **输入框为空**时被吃掉并删末尾令牌，否则交还输入框（`consumed = false`）。
    pub fn key_press(&mut self, key: TokenKey) -> KeyOutcome {
        if key.submits() {
            let text = py_strip(&self.input_text).to_string();
            let mut outcome = KeyOutcome {
                consumed: true,
                tokens_changed: None,
            };
            if !text.is_empty() {
                outcome.tokens_changed = self.add_token(&text);
                self.input_text.clear();
            }
            return outcome;
        }
        if key == TokenKey::Backspace && self.input_text.is_empty() {
            return KeyOutcome {
                consumed: true,
                tokens_changed: self.remove_last(),
            };
        }
        KeyOutcome::default()
    }

    /// `FocusIn`：置焦点标志（`update()` 由调用方负责）。
    pub fn focus_in(&mut self) {
        self.has_focus = true;
    }

    /// `FocusOut`：清焦点 + 提交残留文本（`_addToken` 的结果决定信号）。
    pub fn focus_out(&mut self) -> Option<Vec<String>> {
        self.has_focus = false;
        let text = py_strip(&self.input_text).to_string();
        if text.is_empty() {
            return None;
        }
        let changed = self.add_token(&text);
        self.input_text.clear();
        changed
    }

    /// 聚焦时的底部色带；未聚焦返回 `None`（`paintEvent` 直接 return）。
    ///
    /// 高度小于 [`UNDERLINE_CUT`] 时 `y` 会为负（Qt 同样不夹取）。
    pub fn underline(&self, width: i32, height: i32) -> Option<UnderlineGeometry> {
        if !self.has_focus {
            return None;
        }
        let band_height = UNDERLINE_HEIGHT - UNDERLINE_CUT;
        Some(UnderlineGeometry {
            x: 0,
            y: height - UNDERLINE_HEIGHT + UNDERLINE_CUT,
            width,
            height: band_height,
            radius: UNDERLINE_RADIUS.min(band_height / 2),
        })
    }
}

impl Default for TokenLineEdit {
    fn default() -> Self {
        Self::new(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widths(text: &str) -> i32 {
        text.chars().count() as i32 * 10
    }

    fn advance() -> impl Fn(&str) -> i32 {
        |text: &str| widths(text)
    }

    #[test]
    fn elide_slack_matches_python() {
        assert_eq!(TOKEN_ELIDE_SLACK, 34);
        assert_eq!(token_elide_width(100), 66);
        assert_eq!(token_elide_width(30), -4);
    }

    #[test]
    fn widget_without_elide_keeps_full_text() {
        let widget = TokenWidgetState::new("abcdefgh");
        assert_eq!(widget.max_width(), None);
        assert_eq!(widget.label_text(&advance()), "abcdefgh");
        // elide 只记宽度，不裁原文。
        let mut widget = widget;
        widget.elide(40);
        assert_eq!(widget.max_width(), Some(40));
        assert_eq!(widget.text(), "abcdefgh");
        // available = 40 - 34 = 6 → 只能放 0 个字符 + 省略号。
        assert_eq!(widget.label_text(&advance()), "…");
    }

    #[test]
    fn refresh_widths_requires_positive_budget() {
        let mut line = TokenLineEdit::new(0);
        let tokens = vec!["a".to_string()];
        line.set_tokens(&tokens);
        assert_eq!(line.widgets()[0].max_width(), None);

        // width 12 → maxWidth 0 → 不动。
        line.set_width(12);
        assert_eq!(line.widgets()[0].max_width(), None);
        // width 13 → maxWidth 1 → 记录。
        line.set_width(13);
        assert_eq!(line.widgets()[0].max_width(), Some(1));
        assert_eq!(line.width(), 13);
    }

    #[test]
    fn set_tokens_skips_dedupe_and_limit() {
        let mut line = TokenLineEdit::new(2);
        let tokens = vec![
            "x".to_string(),
            "x".to_string(),
            "y".to_string(),
            "z".to_string(),
        ];
        assert_eq!(line.set_tokens(&tokens), tokens);
        assert_eq!(line.widgets().len(), 4);
        // 已超上限 → 输入框隐藏。
        assert!(!line.input_visible());
    }

    #[test]
    fn add_token_rejects_limit_before_duplicate() {
        let mut line = TokenLineEdit::new(1);
        assert_eq!(line.add_token("a"), Some(vec!["a".to_string()]));
        // 已达上限：即使重复也走上限分支，返回 None。
        assert_eq!(line.add_token("a"), None);
        assert_eq!(line.tokens(), vec!["a".to_string()]);
    }

    #[test]
    fn add_token_rejects_duplicates() {
        let mut line = TokenLineEdit::new(0);
        assert!(line.add_token("a").is_some());
        assert_eq!(line.add_token("a"), None);
        assert!(line.add_token("b").is_some());
        assert_eq!(line.tokens(), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn submit_keys_strip_and_clear() {
        for key in [TokenKey::Return, TokenKey::Enter, TokenKey::Space] {
            let mut line = TokenLineEdit::new(0);
            line.set_input_text("  hello \t");
            let outcome = line.key_press(key);
            assert!(outcome.consumed, "{key:?}");
            assert_eq!(outcome.tokens_changed, Some(vec!["hello".to_string()]));
            assert_eq!(line.input_text(), "");
        }
    }

    #[test]
    fn empty_submit_is_consumed_without_token() {
        let mut line = TokenLineEdit::new(0);
        line.set_input_text("   ");
        let outcome = line.key_press(TokenKey::Space);
        assert!(outcome.consumed);
        assert_eq!(outcome.tokens_changed, None);
        // 空白文本不提交，也不清空（Python 在 `if text:` 内才 clear）。
        assert_eq!(line.input_text(), "   ");
        assert!(line.tokens().is_empty());
    }

    #[test]
    fn backspace_only_consumed_when_input_empty() {
        let mut line = TokenLineEdit::new(0);
        line.set_tokens(&["a".to_string(), "b".to_string()]);

        line.set_input_text("ab");
        let outcome = line.key_press(TokenKey::Backspace);
        assert!(!outcome.consumed);
        assert_eq!(outcome.tokens_changed, None);
        assert_eq!(line.tokens(), vec!["a".to_string(), "b".to_string()]);

        line.set_input_text("");
        let outcome = line.key_press(TokenKey::Backspace);
        assert!(outcome.consumed);
        assert_eq!(outcome.tokens_changed, Some(vec!["a".to_string()]));

        // 没有令牌可删时仍然吃掉事件。
        let mut empty = TokenLineEdit::new(0);
        let outcome = empty.key_press(TokenKey::Backspace);
        assert!(outcome.consumed);
        assert_eq!(outcome.tokens_changed, None);
    }

    #[test]
    fn focus_out_commits_pending_text() {
        let mut line = TokenLineEdit::new(0);
        line.focus_in();
        assert!(line.has_focus());
        line.set_input_text("  pending ");
        assert_eq!(line.focus_out(), Some(vec!["pending".to_string()]));
        assert!(!line.has_focus());
        assert_eq!(line.input_text(), "");

        // 已存在的重复项：不插入也不发信号，但输入框照样清空。
        line.set_input_text("pending");
        assert_eq!(line.focus_out(), None);
        assert_eq!(line.input_text(), "");
        assert_eq!(line.tokens(), vec!["pending".to_string()]);
    }

    #[test]
    fn input_visibility_follows_max_count() {
        let mut line = TokenLineEdit::new(0);
        assert!(line.input_visible());
        line.set_tokens(&["a".to_string(), "b".to_string(), "c".to_string()]);
        assert!(line.input_visible());

        line.set_max_token_count(3);
        assert!(!line.input_visible());
        line.set_max_token_count(4);
        assert!(line.input_visible());
        line.set_max_token_count(0);
        assert!(line.input_visible());
        assert_eq!(line.max_token_count(), 0);
    }

    #[test]
    fn underline_only_when_focused() {
        let mut line = TokenLineEdit::new(0);
        assert_eq!(line.underline(200, 33), None);
        line.focus_in();
        let geometry = line.underline(200, 33).expect("focused");
        assert_eq!(
            geometry,
            UnderlineGeometry {
                x: 0,
                y: 31, // h - 10 + 8（barPath 减掉 rectPath 后剩下的 2px 带）
                width: 200,
                height: 2,
                radius: 1, // min(5, 2 / 2)
            }
        );
        // 高度不足时 y 为负，不夹取。
        assert_eq!(line.underline(10, 4).expect("focused").y, 2);
        assert_eq!(line.underline(10, 0).expect("focused").y, -2);
    }

    #[test]
    fn token_background_follows_theme() {
        assert_eq!(token_background(true), (255, 255, 255, 20));
        assert_eq!(token_background(false), (0, 0, 0, 12));
        assert_eq!(TOKEN_HEIGHT, 24);
        assert_eq!(CLOSE_SIZE, 20);
        assert_eq!(CLOSE_ICON_SIZE, 8);
        assert_eq!(TOKEN_RADIUS, 4);
        assert_eq!(INPUT_MIN_WIDTH, 60);
        assert_eq!(INPUT_HEIGHT, 24);
        assert_eq!(LINE_MIN_HEIGHT, 33);
        assert_eq!(FLOW_MARGIN, 4);
        assert_eq!(FLOW_SPACING, 4);
        assert_eq!(DEFAULT_PLACEHOLDER, "添加...");
    }
}
