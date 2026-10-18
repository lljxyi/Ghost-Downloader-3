//! `app/view/components/markdown_viewer.py`（215 行）的语义镜像。
//!
//! 覆盖三块纯逻辑：
//!
//! 1. [`match_alert`] / [`insert_alert_separators`]——GitHub 风格提示块
//!    （`> [!NOTE]`）的识别与预处理（`_matchAlert` / `setMarkdown`）；
//! 2. [`alert_color`] / [`alert_icon_svg`] / [`alert_heading_html`] /
//!    [`alert_body_html`]——`_insertAlert` 的 HTML/SVG 拼装；
//! 3. [`MarkdownViewerState`]——`sizeHint` / `minimumSizeHint` 的自适应高度
//!    （`_sizeForLines` + `sizeHint`）。
//!
//! 不做的事：
//!
//! - `QTextDocument` 的 Markdown 解析本身（`document().setMarkdown`）——
//!   归 Slint 侧的渲染管线，这里只产出「给渲染器的输入」；
//! - `_refreshAlertStyle` 的块遍历（`document().begin()` 搭配
//!   `leftMargin() > 0`、连续块合并、`QTextCursor` 选区）是纯 Qt 文档模型操作，
//!   没有配置口径，迁到 Slint 时改在 `setMarkdown` 之前的文本层做（见下条）；
//! - `QTextTableFormat` 的实际排版（`insertTable(1, 1)`）：只保留
//!   [`ALERT_BORDER`] 等常量，供 Slint 侧建等价布局；
//! - `QSvgRenderer` / `QPixmap` 的位图光栅化：只产出 SVG 文本，光栅化归
//!   Slint `Image` 源（`data:` URL 或内置图标）。
//!
//! ## 实证结论（Python 源码口径）
//!
//! - `ALERT_MARKER` 是 `^\s*\[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\]\s*`，
//!   **大写 only**（`[!note]` 不匹配）；`^` 没有 `MULTILINE`，所以只锚在
//!   字符串起点；
//! - `ALERT_BLOCK` 带 `re.MULTILINE`，块首是 `^[ \t]*>[ \t]*\[!KIND\]`，
//!   续行 `(?:\n[ \t]*>.*)*` **必须**以 `>` 开头（空行即截断）；
//! - `setMarkdown` 对每个块插入 `"\n\n<!-- -->"`（CommonMark 的空 HTML 注释
//!   用来切断引用块，否则两个相邻提示块会被并成一段）；
//! - `_matchAlert(block.text())` 拿的是 **Qt 已去掉 `>` 前缀**的块文本，
//!   所以 `> [!NOTE]` 在 `_matchAlert` 眼里是 `"[!NOTE]"`；本模块的
//!   [`match_alert`] 也按「无 `>` 前缀」的口径实现（见
//!   [`match_alert_ignores_blockquote_prefix`] 测试）；
//! - `ALERT_SPECS` 五色的 light/dark 取值：note `#0969da`/`#2f81f7`、
//!   tip `#1a7f37`/`#3fb950`、important `#8250df`/`#a371f7`、
//!   warning `#9a6700`/`#d29922`、caution `#cf222e`/`#f85149`；
//! - `_insertAlert` 的表格：`border=0`、`cellPadding=8`、
//!   `width=PercentageLength(100)`、单元格左边框 4px 实线着色、
//!   左内边距 12、图标 16×16；
//! - `_sizeForLines` = `padding + lineSpacing * count`，`padding` 汇总了
//!   `contentsMargins` + `viewportMargins` + `frameWidth*2` + `2*round(documentMargin)`；
//! - `sizeHint` 先 `max(minHeight, padding + round(docHeight))`，
//!   再（若有 `_maximumVisibleLines`）`min(..., maxHeight)`——**顺序不能反**，
//!   否则 `min > max` 时结果不同；
//! - `alert_body_html` 的 `ALERT_MARKER.sub("", body, 1)` 同样锚在 body 起点，
//!   而 Qt 的 `toHtml()` 片段以 `<p style=...>` 开头，所以**实际运行中
//!   marker 通常并不会被去掉**；本模块忠实保留这个锚定语义（见
//!   [`body_html_only_strips_marker_at_body_start`]）。
//!
//! ## 已知偏差
//!
//! - `alert_body_html` 在缺 `<!--StartFragment-->` / `<!--EndFragment-->`
//!   标记时返回 `None`；Python 会 `IndexError`。调用方（`_refreshAlertStyle`）
//!   的输入必然带标记，故这条路径只在手工构造时出现；
//! - `ALERT_SPECS` 的 SVG path 是 `octicons` 快照，升级图标集后需重新核对
//!   （与 `profile.rs::FAMILY_VERSIONS` 同性质）。

use std::sync::LazyLock;

use regex::Regex;

use crate::editors::py_round;

/// `ALERT_MARKER = re.compile(r"^\s*\[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\]\s*")`
/// ——注意没有 `re.MULTILINE`。
static ALERT_MARKER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*\[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\]\s*").expect("ALERT_MARKER")
});

/// `ALERT_BLOCK = re.compile(r"^[ \t]*>[ \t]*\[!(?:NOTE|TIP|IMPORTANT|WARNING|CAUTION)\].*(?:\n[ \t]*>.*)*", re.MULTILINE)`
/// ——`(?m)` 是 Rust 侧的等价物。
static ALERT_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*>[ \t]*\[!(?:NOTE|TIP|IMPORTANT|WARNING|CAUTION)\].*(?:\n[ \t]*>.*)*")
        .expect("ALERT_BLOCK")
});

/// `re.sub(r'margin-left:\d+px;', '', body)`。
static MARGIN_LEFT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"margin-left:\d+px;").expect("MARGIN_LEFT"));

/// `setMarkdown` 插入的分隔串。
pub const ALERT_SEPARATOR: &str = "\n\n<!-- -->";
/// Qt `toHtml()` 的片段起始标记。
pub const FRAGMENT_START: &str = "<!--StartFragment-->";
/// Qt `toHtml()` 的片段结束标记。
pub const FRAGMENT_END: &str = "<!--EndFragment-->";

/// `__init__(minimumVisibleLines: int = 5)`。
pub const DEFAULT_MINIMUM_VISIBLE_LINES: i32 = 5;

/// `tableFormat.setBorder(0)`。
pub const ALERT_BORDER: i32 = 0;
/// `tableFormat.setCellPadding(8)`。
pub const ALERT_CELL_PADDING: i32 = 8;
/// `QTextLength(PercentageLength, 100)`。
pub const ALERT_WIDTH_PERCENT: i32 = 100;
/// `cellFormat.setLeftBorder(4)`。
pub const ALERT_LEFT_BORDER_WIDTH: i32 = 4;
/// `BorderStyle_Solid`。
pub const ALERT_LEFT_BORDER_STYLE: &str = "solid";
/// `cellFormat.setLeftPadding(12)`。
pub const ALERT_LEFT_PADDING: i32 = 12;
/// `<img ... width="16" height="16" />`。
pub const ALERT_ICON_SIZE: i32 = 16;
/// `_toAlertIcon(icon, color, size=16)` 的 viewBox。
pub const ALERT_ICON_VIEWBOX: &str = "0 0 16 16";

/// `ALERT_SPECS` 的五个 kind。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AlertKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl AlertKind {
    /// 字典键 / `alert:` URL scheme 的后半段（`_matchAlert` 的 `group(1).lower()`）。
    pub fn key(self) -> &'static str {
        match self {
            AlertKind::Note => "note",
            AlertKind::Tip => "tip",
            AlertKind::Important => "important",
            AlertKind::Warning => "warning",
            AlertKind::Caution => "caution",
        }
    }

    /// `titles` 字典的值（`self.tr("Note")` 等，无译文层即恒等）。
    pub fn title(self) -> &'static str {
        match self {
            AlertKind::Note => "Note",
            AlertKind::Tip => "Tip",
            AlertKind::Important => "Important",
            AlertKind::Warning => "Warning",
            AlertKind::Caution => "Caution",
        }
    }

    /// 五值全表（按 `ALERT_SPECS` 的字面顺序）。
    pub const ALL: [AlertKind; 5] = [
        AlertKind::Note,
        AlertKind::Tip,
        AlertKind::Important,
        AlertKind::Warning,
        AlertKind::Caution,
    ];
}

/// `ALERT_SPECS` 的 SVG path（octicons 快照，见模块头「已知偏差」）。
fn alert_icon_path(kind: AlertKind) -> &'static str {
    match kind {
        AlertKind::Note => {
            "M0 8a8 8 0 1 1 16 0A8 8 0 0 1 0 8Zm8-6.5a6.5 6.5 0 1 0 0 13 6.5 6.5 0 0 0 0-13ZM6.5 7.75A.75.75 0 0 1 7.25 7h1a.75.75 0 0 1 .75.75v2.75h.25a.75.75 0 0 1 0 1.5h-2a.75.75 0 0 1 0-1.5h.25v-2h-.25a.75.75 0 0 1-.75-.75ZM8 6a1 1 0 1 1 0-2 1 1 0 0 1 0 2Z"
        }
        AlertKind::Tip => {
            "M8 1.5c-2.363 0-4 1.69-4 3.75 0 .984.424 1.625.984 2.304l.214.253c.223.264.47.556.673.848.284.411.537.896.621 1.49a.75.75 0 0 1-1.484.211c-.04-.282-.163-.547-.37-.847a8.456 8.456 0 0 0-.542-.68c-.084-.1-.173-.205-.268-.32C3.201 7.75 2.5 6.766 2.5 5.25 2.5 2.31 4.863 0 8 0c3.137 0 5.5 2.31 5.5 5.25 0 1.516-.701 2.5-1.358 3.29-.095.115-.184.22-.268.319-.207.245-.383.453-.541.681-.208.3-.33.565-.37.847a.751.751 0 0 1-1.485-.212c.084-.593.337-1.078.621-1.489.203-.292.45-.584.673-.848.075-.088.147-.173.213-.253.561-.679.985-1.32.985-2.304 0-2.06-1.637-3.75-4-3.75ZM5.75 12h4.5a.75.75 0 0 1 0 1.5h-4.5a.75.75 0 0 1 0-1.5ZM6 15.25a.75.75 0 0 1 .75-.75h2.5a.75.75 0 0 1 0 1.5h-2.5a.75.75 0 0 1-.75-.75Z"
        }
        AlertKind::Important => {
            "M0 1.75C0 .784.784 0 1.75 0h12.5C15.216 0 16 .784 16 1.75v9.5A1.75 1.75 0 0 1 14.25 13H8.06l-2.573 2.573A1.458 1.458 0 0 1 3 14.543V13H1.75A1.75 1.75 0 0 1 0 11.25Zm1.75-.25a.25.25 0 0 0-.25.25v9.5c0 .138.112.25.25.25h2a.75.75 0 0 1 .75.75v2.19l2.72-2.72a.749.749 0 0 1 .53-.22h6.5a.25.25 0 0 0 .25-.25v-9.5a.25.25 0 0 0-.25-.25Zm7 2.25v2.5a.75.75 0 0 1-1.5 0v-2.5a.75.75 0 0 1 1.5 0ZM8 9a1 1 0 1 1 0-2 1 1 0 0 1 0 2Z"
        }
        AlertKind::Warning => {
            "M6.457 1.047c.659-1.234 2.427-1.234 3.086 0l6.082 11.378A1.75 1.75 0 0 1 14.082 15H1.918a1.75 1.75 0 0 1-1.543-2.575Zm1.763.707a.25.25 0 0 0-.44 0L1.698 13.132a.25.25 0 0 0 .22.368h12.164a.25.25 0 0 0 .22-.368Zm.53 3.996v2.5a.75.75 0 0 1-1.5 0v-2.5a.75.75 0 0 1 1.5 0ZM9 11a1 1 0 1 1-2 0 1 1 0 0 1 2 0Z"
        }
        AlertKind::Caution => {
            "M4.47.22A.749.749 0 0 1 5 0h6c.199 0 .389.079.53.22l4.25 4.25c.141.14.22.331.22.53v6a.749.749 0 0 1-.22.53l-4.25 4.25A.749.749 0 0 1 11 16H5a.749.749 0 0 1-.53-.22L.22 11.53A.749.749 0 0 1 0 11V5c0-.199.079-.389.22-.53Zm.84 1.28L1.5 5.31v5.38l3.81 3.81h5.38l3.81-3.81V5.31L10.69 1.5ZM8 4a.75.75 0 0 1 .75.75v3.5a.75.75 0 0 1-1.5 0v-3.5A.75.75 0 0 1 8 4Zm0 8a1 1 0 1 1 0-2 1 1 0 0 1 0 2Z"
        }
    }
}

/// `ALERT_SPECS[kind]` 的 light 色。
fn alert_light(kind: AlertKind) -> &'static str {
    match kind {
        AlertKind::Note => "#0969da",
        AlertKind::Tip => "#1a7f37",
        AlertKind::Important => "#8250df",
        AlertKind::Warning => "#9a6700",
        AlertKind::Caution => "#cf222e",
    }
}

/// `ALERT_SPECS[kind]` 的 dark 色。
fn alert_dark(kind: AlertKind) -> &'static str {
    match kind {
        AlertKind::Note => "#2f81f7",
        AlertKind::Tip => "#3fb950",
        AlertKind::Important => "#a371f7",
        AlertKind::Warning => "#d29922",
        AlertKind::Caution => "#f85149",
    }
}

/// `color = dark if isDark else light`。
pub fn alert_color(kind: AlertKind, dark: bool) -> &'static str {
    if dark {
        alert_dark(kind)
    } else {
        alert_light(kind)
    }
}

/// `_toAlertIcon` 产出的 SVG 文本（光栅化留给渲染层）。
pub fn alert_icon_svg(kind: AlertKind, dark: bool) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{}\" fill=\"{}\">\
         <path fill-rule=\"evenodd\" d=\"{}\"/></svg>",
        ALERT_ICON_VIEWBOX,
        alert_color(kind, dark),
        alert_icon_path(kind)
    )
}

/// `_insertAlert` 里 `heading` 的字面拼装。
pub fn alert_heading_html(kind: AlertKind, dark: bool) -> String {
    format!(
        "<p style=\"margin-top:0; margin-bottom:4px; color:{};\">\
         <img src=\"alert:{}\" width=\"{}\" height=\"{}\" />&nbsp;<b>{}</b></p>",
        alert_color(kind, dark),
        kind.key(),
        ALERT_ICON_SIZE,
        ALERT_ICON_SIZE,
        kind.title()
    )
}

/// `_matchAlert(text)`：`text` 是 Qt 已去掉 `>` 前缀的块文本。
pub fn match_alert(text: &str) -> Option<AlertKind> {
    let caps = ALERT_MARKER.captures(text)?;
    // 正则锚在起点，但 `captures` 在无 `^` 的方言里可能扫到别处；这里显式确认。
    if caps.get(0)?.start() != 0 {
        return None;
    }
    match &caps[1] {
        "NOTE" => Some(AlertKind::Note),
        "TIP" => Some(AlertKind::Tip),
        "IMPORTANT" => Some(AlertKind::Important),
        "WARNING" => Some(AlertKind::Warning),
        "CAUTION" => Some(AlertKind::Caution),
        _ => None,
    }
}

/// `setMarkdown` 的预处理：每个提示块后插 `ALERT_SEPARATOR`。
pub fn insert_alert_separators(text: &str) -> String {
    ALERT_BLOCK
        .replace_all(text, |caps: &regex::Captures<'_>| {
            format!("{}{}", &caps[0], ALERT_SEPARATOR)
        })
        .into_owned()
}

/// `_refreshAlertStyle` 里 `body` 的拼装：剥片段标记 → 去 `margin-left` →
/// 包一层 `<p>`。
///
/// `html` 是 `QTextCursor.selection().toHtml()` 的输出，必须同时含
/// [`FRAGMENT_START`] 与 [`FRAGMENT_END`]（见模块头「已知偏差」）。
pub fn alert_body_html(html: &str) -> Option<String> {
    let start = html.find(FRAGMENT_START)? + FRAGMENT_START.len();
    let rest = &html[start..];
    let end = rest.find(FRAGMENT_END)?;
    let body = ALERT_MARKER.replacen(&rest[..end], 1, "");
    let body = MARGIN_LEFT.replace_all(&body, "");
    Some(format!("<p>{body}</p>"))
}

/// `_sizeForLines(count)` 的高度：`padding + lineSpacing * count`。
pub fn size_for_lines(count: i32, padding: i32, line_spacing: i32) -> i32 {
    padding + line_spacing * count
}

/// `padding + round(document().size().height())`。
pub fn content_height(padding: i32, doc_height: f64) -> i32 {
    padding + py_round(doc_height)
}

/// `sizeHint` 的夹取：先 `max(minimum)`，再（可选）`min(maximum)`。
pub fn resolve_height(content: i32, minimum: i32, maximum: Option<i32>) -> i32 {
    let height = content.max(minimum);
    match maximum {
        Some(maximum) => height.min(maximum),
        None => height,
    }
}

/// `MarkdownViewer` 的几何状态：可见行数 + 从 Qt 侧实测传入的 padding /
/// 行距（`fontMetrics().lineSpacing()`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkdownViewerState {
    minimum_visible_lines: i32,
    maximum_visible_lines: Option<i32>,
    padding: i32,
    line_spacing: i32,
}

impl MarkdownViewerState {
    /// `__init__(minimumVisibleLines=5, maximumVisibleLines=None)`。
    pub fn new(minimum_visible_lines: i32, maximum_visible_lines: Option<i32>) -> Self {
        Self {
            minimum_visible_lines,
            maximum_visible_lines,
            padding: 0,
            line_spacing: 0,
        }
    }

    /// `padding` / `lineSpacing` 由 Qt 侧实测值注入（`contentsMargins` +
    /// `viewportMargins` + `frameWidth*2` + `2*round(documentMargin)`）。
    pub fn with_metrics(mut self, padding: i32, line_spacing: i32) -> Self {
        self.padding = padding;
        self.line_spacing = line_spacing;
        self
    }

    pub fn minimum_visible_lines(&self) -> i32 {
        self.minimum_visible_lines
    }

    pub fn maximum_visible_lines(&self) -> Option<i32> {
        self.maximum_visible_lines
    }

    pub fn padding(&self) -> i32 {
        self.padding
    }

    pub fn line_spacing(&self) -> i32 {
        self.line_spacing
    }

    /// `minimumSizeHint()`。
    pub fn minimum_size_hint(&self) -> i32 {
        size_for_lines(self.minimum_visible_lines, self.padding, self.line_spacing)
    }

    /// `sizeHint()` 的高度部分（宽度归布局）。
    pub fn size_hint(&self, doc_height: f64) -> i32 {
        let content = content_height(self.padding, doc_height);
        let maximum = self
            .maximum_visible_lines
            .map(|lines| size_for_lines(lines, self.padding, self.line_spacing));
        resolve_height(content, self.minimum_size_hint(), maximum)
    }

    /// `setMarkdown(text)` 交给渲染层的那份文本。
    pub fn set_markdown(&self, text: &str) -> String {
        insert_alert_separators(text)
    }
}

impl Default for MarkdownViewerState {
    fn default() -> Self {
        Self::new(DEFAULT_MINIMUM_VISIBLE_LINES, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ALERT_MARKER` 只认大写，且 `\s*` 吃前后空白。
    #[test]
    fn marker_is_uppercase_only() {
        assert_eq!(match_alert("[!NOTE]"), Some(AlertKind::Note));
        assert_eq!(match_alert("  \t[!TIP]foo"), Some(AlertKind::Tip));
        assert_eq!(
            match_alert("[!IMPORTANT]\nbody"),
            Some(AlertKind::Important)
        );
        assert_eq!(match_alert("[!WARNING]  "), Some(AlertKind::Warning));
        assert_eq!(match_alert("[!CAUTION]x"), Some(AlertKind::Caution));
        // 小写不匹配（Python 的 pattern 没有 re.IGNORECASE）。
        assert_eq!(match_alert("[!note] x"), None);
        assert_eq!(match_alert("[!Note] x"), None);
        // 其它 kind 不匹配。
        assert_eq!(match_alert("[!DANGER] x"), None);
        // 空串 / 无标记。
        assert_eq!(match_alert(""), None);
        assert_eq!(match_alert("正文"), None);
    }

    /// `_matchAlert` 收的是 Qt 已去掉 `>` 的块文本；带 `>` 不匹配。
    #[test]
    fn match_alert_ignores_blockquote_prefix() {
        assert_eq!(match_alert("> [!NOTE]"), None);
        assert_eq!(match_alert("> [!NOTE]\n> body"), None);
    }

    /// `setMarkdown`：每个提示块后插 `\n\n<!-- -->`，普通引用不动。
    #[test]
    fn separators_inserted_after_each_alert_block() {
        let text = "> [!NOTE]\n> body line\n\nplain\n\n> [!TIP]\n> tip body\n";
        assert_eq!(
            insert_alert_separators(text),
            "> [!NOTE]\n> body line\n\n<!-- -->\n\nplain\n\n> [!TIP]\n> tip body\n\n<!-- -->\n"
        );
    }

    /// 续行必须紧接 `>`；空行即截断（后面的行不再并入同一块）。
    #[test]
    fn separator_block_stops_at_blank_line() {
        let text = "> [!NOTE]\n> one\n\n> two\n";
        assert_eq!(
            insert_alert_separators(text),
            "> [!NOTE]\n> one\n\n<!-- -->\n\n> two\n"
        );
        // 缩进的 `>` 也算续行；缩进 4 空格仍是 `[ \t]*`。
        let text = "  > [!NOTE]\n  > one\n\t> two";
        assert_eq!(
            insert_alert_separators(text),
            "  > [!NOTE]\n  > one\n\t> two\n\n<!-- -->"
        );
    }

    /// 普通引用 / 块中间出现的 marker 都不是提示块。
    #[test]
    fn separator_skips_plain_quotes() {
        let text = "> hello\n> world\n";
        assert_eq!(insert_alert_separators(text), text);
        let text = "> see [!NOTE] here\n";
        assert_eq!(insert_alert_separators(text), text);
        // 不带 `>` 的裸 marker 不匹配 ALERT_BLOCK（只匹配 ALERT_MARKER）。
        let text = "[!NOTE] bare\n";
        assert_eq!(insert_alert_separators(text), text);
    }

    /// 五色的 light/dark 取值（`ALERT_SPECS` 快照）。
    #[test]
    fn alert_colors_match_specs() {
        let expected = [
            (AlertKind::Note, "#0969da", "#2f81f7"),
            (AlertKind::Tip, "#1a7f37", "#3fb950"),
            (AlertKind::Important, "#8250df", "#a371f7"),
            (AlertKind::Warning, "#9a6700", "#d29922"),
            (AlertKind::Caution, "#cf222e", "#f85149"),
        ];
        for (kind, light, dark) in expected {
            assert_eq!(alert_color(kind, false), light);
            assert_eq!(alert_color(kind, true), dark);
        }
        // 字典键即 `alert:` URL 的后半段。
        assert_eq!(
            AlertKind::ALL.map(|k| k.key()),
            ["note", "tip", "important", "warning", "caution"]
        );
        assert_eq!(
            AlertKind::ALL.map(|k| k.title()),
            ["Note", "Tip", "Important", "Warning", "Caution"]
        );
    }

    /// `_toAlertIcon` 的 SVG 文本：viewBox / fill / evenodd 三件套。
    #[test]
    fn icon_svg_wraps_path_with_color() {
        let svg = alert_icon_svg(AlertKind::Tip, true);
        assert!(svg.starts_with(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" fill=\"#3fb950\">"
        ));
        assert!(svg.contains("<path fill-rule=\"evenodd\" d=\"M8 1.5c-2.363 0-4 1.69-4 3.75"));
        assert!(svg.ends_with("/></svg>"));
        // 浅色换色，path 不变。
        let light = alert_icon_svg(AlertKind::Tip, false);
        assert!(light.contains("fill=\"#1a7f37\""));
        assert_eq!(light.replace("#1a7f37", "#3fb950"), svg);
    }

    /// heading 的字面拼装（含 `&nbsp;` 与 `alert:` scheme）。
    #[test]
    fn heading_html_is_exact() {
        assert_eq!(
            alert_heading_html(AlertKind::Note, false),
            "<p style=\"margin-top:0; margin-bottom:4px; color:#0969da;\">\
             <img src=\"alert:note\" width=\"16\" height=\"16\" />&nbsp;<b>Note</b></p>"
        );
        assert_eq!(
            alert_heading_html(AlertKind::Caution, true),
            "<p style=\"margin-top:0; margin-bottom:4px; color:#f85149;\">\
             <img src=\"alert:caution\" width=\"16\" height=\"16\" />&nbsp;<b>Caution</b></p>"
        );
    }

    /// body：剥片段标记 + 去 `margin-left:Npx;` + 包 `<p>`。
    #[test]
    fn body_html_strips_margins_and_wraps() {
        let html = "<!--StartFragment--><p style=\"margin-left:0px;\">one</p>\
                    <p style=\"margin-left:12px;\">two</p><!--EndFragment-->";
        assert_eq!(
            alert_body_html(html).as_deref(),
            Some("<p><p style=\"\">one</p><p style=\"\">two</p></p>")
        );
        // 只切第一对标记；后面的同名标记留在 body 里。
        let html = "<!--StartFragment-->a<!--EndFragment-->b<!--EndFragment-->";
        assert_eq!(alert_body_html(html).as_deref(), Some("<p>a</p>"));
    }

    /// `ALERT_MARKER.sub("", body, 1)` 锚在 body 起点：Qt 的 `<p>` 前缀
    /// 让 marker 实际不会被去掉。
    #[test]
    fn body_html_only_strips_marker_at_body_start() {
        let html = "<!--StartFragment-->[!NOTE] rest<!--EndFragment-->";
        assert_eq!(alert_body_html(html).as_deref(), Some("<p>rest</p>"));
        let html = "<!--StartFragment--><p>[!NOTE] rest</p><!--EndFragment-->";
        assert_eq!(
            alert_body_html(html).as_deref(),
            Some("<p><p>[!NOTE] rest</p></p>")
        );
    }

    /// 缺片段标记 → `None`（Python 会 `IndexError`，见模块头「已知偏差」）。
    #[test]
    fn body_html_requires_fragment_markers() {
        assert_eq!(alert_body_html("plain"), None);
        assert_eq!(alert_body_html("<!--StartFragment-->x"), None);
        assert_eq!(
            alert_body_html("<!--EndFragment-->x<!--StartFragment-->"),
            None
        );
    }

    /// `_sizeForLines` / `content_height` / `resolve_height` 的口径。
    #[test]
    fn size_helpers_mirror_python() {
        assert_eq!(size_for_lines(5, 10, 16), 90);
        assert_eq!(size_for_lines(0, 10, 16), 10);
        // `round()` 半值取偶：10.5 → 10，11.5 → 12。
        assert_eq!(content_height(4, 10.5), 14);
        assert_eq!(content_height(4, 11.5), 16);
        // 先 max 再 min（顺序不能反）。
        assert_eq!(resolve_height(20, 90, None), 90);
        assert_eq!(resolve_height(200, 90, Some(138)), 138);
        assert_eq!(resolve_height(200, 90, None), 200);
        // min > max 时结果取 max（Python 同样先 max 后 min）。
        assert_eq!(resolve_height(0, 90, Some(40)), 40);
    }

    /// `sizeHint` 全链路：minimumSizeHint + 上下夹取。
    #[test]
    fn viewer_state_clamps_size_hint() {
        let state = MarkdownViewerState::default().with_metrics(10, 16);
        assert_eq!(state.minimum_visible_lines(), DEFAULT_MINIMUM_VISIBLE_LINES);
        assert_eq!(state.minimum_size_hint(), 90);
        assert_eq!(state.size_hint(20.0), 90);
        assert_eq!(state.size_hint(500.0), 510);

        let state = MarkdownViewerState::new(3, Some(8)).with_metrics(10, 16);
        assert_eq!(state.minimum_size_hint(), 58);
        assert_eq!(state.size_hint(20.0), 58);
        assert_eq!(state.size_hint(500.0), 138);

        // 无上限时不再夹：内容 95 已高于最小值 10。
        let state = MarkdownViewerState::new(1, None).with_metrics(0, 10);
        assert_eq!(state.minimum_size_hint(), 10);
        assert_eq!(state.size_hint(95.0), 95);
        assert_eq!(state.size_hint(5.0), 10);
    }

    /// `setMarkdown` 只是 `insert_alert_separators` 的转发。
    #[test]
    fn state_set_markdown_forwards() {
        let state = MarkdownViewerState::default();
        assert_eq!(
            state.set_markdown("> [!NOTE]\n> x"),
            "> [!NOTE]\n> x\n\n<!-- -->"
        );
    }
}
