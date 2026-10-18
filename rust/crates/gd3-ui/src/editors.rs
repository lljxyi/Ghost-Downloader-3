//! 组件库：自适应编辑器（镜像 `app/view/components/editors.py`，124 行）。
//!
//! 三个控件的「可断言事实」：
//!
//! - `AutoSizingEdit`：高度 = `padding + lineSpacing * 行数`，宽度取
//!   `super().sizeHint().width()`；`padding` 由五段拼成
//!   （contentsMargins 上下 + viewportMargins 上下 + `frameWidth*2` +
//!   `round(documentMargin*2)`）；`minimumSizeHint` 用
//!   `min(minimumVisibleLines, blockCount)`，`sizeHint` 先用
//!   `maximumVisibleLines` 截断再 `expandedTo(minimumSizeHint())`；
//! - `AutoSizingComboBox`：`sizeHint().width() = horizontalAdvance(currentText)
//!   + 60`（`COMBO_PADDING`），高度取基类；
//! - `FolderPicker`：`comboBox` 最小宽 200、浏览按钮 28×28、布局边距 0 /
//!   间距 8、`path()` 是 `currentText().strip()`、`_commit` 空路径不发信号、
//!   历史「去重后插到最前、只留前 20 条」、下拉只装非空文件夹。
//!
//! 不镜像：`QSizePolicy`（Slint 布局里由 `preferred-height` 表达）、
//! `QFileDialog`（选目录是平台对话框，归调用方）。

/// `COMBO_PADDING: Final[int] = 60`。
pub const COMBO_PADDING: i32 = 60;
/// `self.comboBox.setMinimumWidth(200)`。
pub const FOLDER_COMBO_MIN_WIDTH: i32 = 200;
/// `self.browseButton.setFixedSize(28, 28)`。
pub const BROWSE_BUTTON_SIZE: i32 = 28;
/// `layout.setSpacing(8)`。
pub const FOLDER_PICKER_SPACING: i32 = 8;
/// `cfg.set(cfg.memoryDownloadFolders, history[:20])`。
pub const FOLDER_HISTORY_LIMIT: usize = 20;
/// `self.browseButton.setToolTip(self.tr("浏览文件夹"))`。
pub const BROWSE_BUTTON_TOOLTIP: &str = "浏览文件夹";
/// `QFileDialog.getExistingDirectory(self.window(), self.tr("选择文件夹"), ...)`。
pub const BROWSE_DIALOG_TITLE: &str = "选择文件夹";

/// Python 的 `round()`：**半值取偶**（banker's rounding）。
///
/// `documentMargin` 是浮点（qfluentwidgets 默认 4.0），`round(2.5) == 2`
/// 而 Rust 的 `f64::round` 给 3——差 1 像素就在这条路径上。
pub fn py_round(value: f64) -> i32 {
    let floor = value.floor();
    let diff = value - floor;
    let rounded = if (diff - 0.5).abs() < f64::EPSILON {
        // 恰在半格上：取偶。
        let candidate = floor as i64;
        if candidate % 2 == 0 {
            candidate
        } else {
            candidate + 1
        }
    } else {
        value.round() as i64
    };
    rounded as i32
}

/// `_sizeForLines` 需要的四段尺寸（都是 Qt 侧实测值，本 crate 不查控件）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EditMetrics {
    /// `super().sizeHint().width()`
    pub width: i32,
    /// `margins.top+bottom + viewportMargins.top+bottom + frameWidth*2
    /// + round(documentMargin*2)`
    pub padding: i32,
    /// `fontMetrics().lineSpacing()`
    pub line_spacing: i32,
}

impl EditMetrics {
    /// 按 `_sizeForLines` 的五段口径拼 `padding`。
    ///
    /// `contents_margins` / `viewport_margins` 都是 `(top, bottom)`。
    pub fn new(
        contents_margins: (i32, i32),
        viewport_margins: (i32, i32),
        frame_width: i32,
        document_margin: f64,
        width: i32,
        line_spacing: i32,
    ) -> Self {
        let padding = contents_margins.0
            + contents_margins.1
            + viewport_margins.0
            + viewport_margins.1
            + frame_width * 2
            + py_round(document_margin * 2.0);
        Self {
            width,
            padding,
            line_spacing,
        }
    }

    /// `padding + lineSpacing * count`。
    pub fn height_for_lines(&self, count: i32) -> i32 {
        self.padding + self.line_spacing * count
    }
}

/// `AutoSizingEdit`：随文档行数长高，`minimumVisibleLines` / 可选的
/// `maximumVisibleLines` 夹住两端。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutoSizingEdit {
    pub minimum_visible_lines: i32,
    pub maximum_visible_lines: Option<i32>,
}

impl AutoSizingEdit {
    /// Python 默认 `minimumVisibleLines=5, maximumVisibleLines=None`。
    pub fn new(minimum_visible_lines: i32, maximum_visible_lines: Option<i32>) -> Self {
        Self {
            minimum_visible_lines,
            maximum_visible_lines,
        }
    }

    pub fn size_for_lines(&self, count: i32, metrics: &EditMetrics) -> (i32, i32) {
        (metrics.width, metrics.height_for_lines(count))
    }

    /// `minimumSizeHint`：`min(minimumVisibleLines, blockCount)` 行。
    pub fn minimum_size_hint(&self, block_count: i32, metrics: &EditMetrics) -> (i32, i32) {
        self.size_for_lines(self.minimum_visible_lines.min(block_count), metrics)
    }

    /// `sizeHint`：先按 `maximumVisibleLines` 截断，再 `expandedTo(最小)`。
    ///
    /// `expandedTo` 是**逐维取大**——最小高度比截断后的高度还大时以最小为准。
    pub fn size_hint(&self, block_count: i32, metrics: &EditMetrics) -> (i32, i32) {
        let mut count = block_count;
        if let Some(maximum) = self.maximum_visible_lines {
            count = count.min(maximum);
        }
        let (width, height) = self.size_for_lines(count, metrics);
        let (min_width, min_height) = self.minimum_size_hint(block_count, metrics);
        (width.max(min_width), height.max(min_height))
    }
}

/// `AutoSizingComboBox.sizeHint()`：宽随当前文本，高随基类。
///
/// 注意宽度是 `advance(currentText) + 60`，**不夹**最小宽度——
/// `FolderPicker` 的 `setMinimumWidth(200)` 是布局层的事，不在这条公式里。
pub fn combo_size_hint(text_advance: i32, base_height: i32) -> (i32, i32) {
    (text_advance + COMBO_PADDING, base_height)
}

/// `FolderPicker.path()`：`comboBox.currentText().strip()`。
pub fn folder_path(current_text: &str) -> String {
    current_text.trim().to_string()
}

/// `_commit`：空路径（strip 后）**不发** `pathChanged`。
pub fn folder_commit(current_text: &str) -> Option<String> {
    let path = folder_path(current_text);
    if path.is_empty() { None } else { Some(path) }
}

/// `_onBrowseClicked` 的返回值：选中目录才 `setCurrentText` + 发信号；
/// 取消（空串）什么都不做。
pub fn browse_selected(folder: Option<&str>) -> Option<String> {
    folder
        .map(str::trim)
        .filter(|folder| !folder.is_empty())
        .map(str::to_string)
}

/// `saveHistory(path)`：去重后插到最前，只留前 20 条。
///
/// Python 侧是 `if path in history: history.remove(path)`——只删**第一个**
/// 命中项（历史里本不该有重复，这里照抄「删首个」语义）。
pub fn save_history(history: &[String], path: &str) -> Vec<String> {
    let mut history = history.to_vec();
    if let Some(index) = history.iter().position(|entry| entry == path) {
        history.remove(index);
    }
    history.insert(0, path.to_string());
    history.truncate(FOLDER_HISTORY_LIMIT);
    history
}

/// `refreshHistory()` 装进下拉的条目：跳过空串（`if folder:`）。
pub fn refresh_history(folders: &[String]) -> Vec<String> {
    folders
        .iter()
        .filter(|folder| !folder.is_empty())
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 默认 qfluentwidgets 风格：内容边距 0、viewport 边距 0、frame 1、
    /// documentMargin 4.0、行距 16。
    fn metrics() -> EditMetrics {
        EditMetrics::new((0, 0), (0, 0), 1, 4.0, 300, 16)
    }

    #[test]
    fn padding_uses_all_five_segments() {
        // 0+0 + 0+0 + 1*2 + round(4.0*2)=8 → 10。
        assert_eq!(metrics().padding, 10);
        assert_eq!(metrics().height_for_lines(3), 10 + 16 * 3);
        // 有内容边距与 viewport 边距时逐段相加。
        let padded = EditMetrics::new((2, 3), (4, 5), 2, 0.0, 200, 10);
        assert_eq!(padded.padding, 18);
        assert_eq!(padded.width, 200);
    }

    #[test]
    fn py_round_is_half_to_even() {
        assert_eq!(py_round(2.5), 2);
        assert_eq!(py_round(3.5), 4);
        assert_eq!(py_round(0.5), 0);
        assert_eq!(py_round(1.5), 2);
        assert_eq!(py_round(2.4), 2);
        assert_eq!(py_round(2.6), 3);
        assert_eq!(py_round(-2.5), -2);
        assert_eq!(py_round(8.0), 8);
    }

    #[test]
    fn minimum_size_hint_uses_the_smaller_line_count() {
        let edit = AutoSizingEdit::new(5, None);
        let metrics = metrics();
        // 只有 2 块 → 2 行（不是 5 行）。
        assert_eq!(edit.minimum_size_hint(2, &metrics), (300, 10 + 32));
        // 10 块 → 5 行（夹到 minimumVisibleLines）。
        assert_eq!(edit.minimum_size_hint(10, &metrics), (300, 10 + 80));
    }

    #[test]
    fn size_hint_clamps_to_maximum_then_expands_to_minimum() {
        let metrics = metrics();
        let edit = AutoSizingEdit::new(5, Some(8));
        // 3 块：3 行 > 最小 3 行 → 3 行。
        assert_eq!(edit.size_hint(3, &metrics), (300, 10 + 48));
        // 20 块：截到 8 行。
        assert_eq!(edit.size_hint(20, &metrics), (300, 10 + 128));
        // 无上限时按块数直算。
        let unbounded = AutoSizingEdit::new(5, None);
        assert_eq!(unbounded.size_hint(20, &metrics), (300, 10 + 320));
        // 上限小于最小可见行数：expandedTo 把高度抬回最小。
        let squeezed = AutoSizingEdit::new(5, Some(2));
        assert_eq!(squeezed.size_hint(20, &metrics), (300, 10 + 80));
    }

    #[test]
    fn combo_width_adds_sixty_pixels_to_the_text_advance() {
        assert_eq!(combo_size_hint(120, 33), (180, 33));
        assert_eq!(combo_size_hint(0, 33), (60, 33));
        assert_eq!(COMBO_PADDING, 60);
    }

    #[test]
    fn folder_path_strips_and_commit_skips_empty() {
        assert_eq!(folder_path("  /tmp/a  "), "/tmp/a");
        assert_eq!(folder_commit("  /tmp/a  "), Some("/tmp/a".to_string()));
        // 全空白 → None（不发信号）。
        assert_eq!(folder_commit("   "), None);
        assert_eq!(folder_commit(""), None);
    }

    #[test]
    fn save_history_moves_to_front_and_truncates_to_twenty() {
        let history = vec!["/b".to_string(), "/a".to_string()];
        assert_eq!(save_history(&history, "/a"), vec!["/a", "/b"]);
        // 新路径插到最前。
        assert_eq!(save_history(&history, "/c"), vec!["/c", "/b", "/a"]);
        // 25 条 → 只留前 20 条（新的在首位）。
        let long: Vec<String> = (0..25).map(|i| format!("/p{i}")).collect();
        let saved = save_history(&long, "/new");
        assert_eq!(saved.len(), 20);
        assert_eq!(saved[0], "/new");
        assert_eq!(saved[19], "/p18");
        // 重复项只删首个（历史里不该有重复，语义照抄）。
        let dup = vec!["/a".to_string(), "/a".to_string()];
        assert_eq!(save_history(&dup, "/a"), vec!["/a", "/a"]);
    }

    #[test]
    fn refresh_history_drops_empty_entries_only() {
        let folders = vec!["".to_string(), "/a".to_string(), " ".to_string()];
        // 只跳过**空串**——纯空白在 Python 里是真值，保留原样。
        assert_eq!(refresh_history(&folders), vec!["/a", " "]);
    }

    #[test]
    fn browse_selection_ignores_cancel_and_blank() {
        assert_eq!(browse_selected(Some("/tmp/x")), Some("/tmp/x".to_string()));
        assert_eq!(browse_selected(Some("   ")), None);
        assert_eq!(browse_selected(None), None);
    }
}
