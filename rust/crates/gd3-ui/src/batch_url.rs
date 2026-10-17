//! 批量添加 URL（P2，第 64 步）：镜像 `app/view/dialogs/batch_url.py` 的模板
//! 展开算法 + 预览/计数/确认态语义。
//!
//! ## 实证口径（2026-10-17 实读源码）
//!
//! - 三个正则（`batch_url.py:17-19`）：
//!   `RANGE_PATTERN = \[([^\]]+)\]`、`LIST_PATTERN = \{([^}]+)\}`、
//!   `TOKEN_PATTERN = (\[[^\]]+\]|\{[^}]+\})`。Python 的 `re.split` 带捕获组，
//!   切出来是「普通文本 / token」交替的片段（含首尾空串）。Rust `regex` 的
//!   `split` **不返回捕获组**，所以这里手写 `split_tokens`（逐 match 取前后
//!   文本），语义与 Python 一致。
//! - `expandTemplate`：无 token（`len(tokens) == 1`）时，模板 **strip 后非空**
//!   才返回 `[template]`（返回的是**原始**模板，不是 strip 后的）；有 token 时
//!   每个片段变成一个候选列表，最后做**笛卡尔积**并直接拼接。
//!   - `{a,b}` → 逗号切分 + strip + 丢空项；全空 → 空列表 → 积为空 → 结果为空。
//!   - `[spec]` 能展开 → 用展开值；**不能展开（如 `[x-y]`）则当字面量**。
//!   - 片段列表里有空列表 → `itertools.product` 为空 → 结果为空列表。
//! - `_expandRange`：
//!   - `spec.split(":")` 多于 2 段 → `None`；步长解析失败或 `< 1` → `None`；
//!   - `rangePart.split("-", 1)` 必须恰好两段（负数不支持）；
//!   - 两端 strip 后都不能为空；
//!   - 数字：`start.isdigit() and end.isdigit()`，`s > e` → `None`；
//!     **补零宽度**只在 `start[0] == "0" and len(start) > 1` 时取 `len(start)`，
//!     其余为 0（即 `[1-100]` 不补零，`[001-100]` 补到 3 位）；
//!   - 字母：两端都**单字符**且 `isalpha()`，且同为小写或同为大写才处理，
//!     否则 `None`；`si > ei` → `None`；按 `alphabet[si:ei+1:step]` 取。
//! - 预览（`_onExpandNeeded`）：模板 `strip()` 为空 → 清空 URL、清预览、隐藏
//!   计数、禁用「确认」；否则展开后 `count == 0` 同样清预览/隐藏计数但
//!   `yesButton.setEnabled(count > 0)`；`count > 10` 时预览 = 前 10 条 +
//!   `"⋯"` + 最后一条；计数文案 `共 {} 个链接`（`CaptionLabel`，右对齐、隐藏
//!   初值）。
//! - 防抖：`QTimer(singleShot)`，间隔 **300ms**，`textChanged` 触发 `start()`。
//! - 布局：`widget.setFixedWidth(600)`；模板框可见行 2..3，预览框 3..12；
//!   预览只读；「确认」初始禁用。
//!
//! ## 刻意的偏离（保守，不影响正常输入）
//!
//! - Python 的 `str.isdigit()` 接受全角/阿拉伯-印度数字，`int()` 也能解析；
//!   本模块只认 **ASCII 数字**，非 ASCII 数字走 `None`（当字面量）。
//! - 单字符非 ASCII 字母（如 `é`）在 Python 里会 `ValueError`（
//!   `ascii_lowercase.index` 抛异常），本模块返回 `None`（当字面量），不 panic。

use std::sync::LazyLock;

use regex::Regex;

/// `\[([^\]]+)\]`
static RANGE_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]+)\]").expect("RANGE_PATTERN"));
/// `\{([^}]+)\}`
static LIST_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{([^}]+)\}").expect("LIST_PATTERN"));
/// `(\[[^\]]+\]|\{[^}]+\})`
static TOKEN_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\[[^\]]+\]|\{[^}]+\})").expect("TOKEN_PATTERN"));

/// 对话框静态文案与尺寸（`batch_url.py`）。
pub struct BatchUrlTexts;

impl BatchUrlTexts {
    pub const TITLE: &'static str = "批量添加";
    pub const TEMPLATE_PLACEHOLDER: &'static str =
        "http://example.com/img/[001-100].jpg\nhttp://example.com/{mp4,mkv}/video";
    pub const PREVIEW_PLACEHOLDER: &'static str = "输入模板后在此预览";
    pub const HELP_TITLE: &'static str = "语法说明";
    pub const HELP_TEXT: &'static str = "[1-100] → 1, 2, 3, …, 100\n\
[001-050] → 001, 002, …, 050 (自动补零)\n\
[1-10:2] → 1, 3, 5, 7, 9 (步长)\n\
[a-z] → a, b, …, z (字母范围)\n\
{mp4,mkv,avi} → mp4, mkv, avi (枚举)";
    /// `self.widget.setFixedWidth(600)`
    pub const WIDTH: i32 = 600;
    /// `self._expandTimer.setInterval(300)`
    pub const DEBOUNCE_MS: u64 = 300;
    /// `AutoSizingEdit(minimumVisibleLines=2, maximumVisibleLines=3)`
    pub const TEMPLATE_MIN_LINES: i32 = 2;
    pub const TEMPLATE_MAX_LINES: i32 = 3;
    /// `AutoSizingEdit(minimumVisibleLines=3, maximumVisibleLines=12)`
    pub const PREVIEW_MIN_LINES: i32 = 3;
    pub const PREVIEW_MAX_LINES: i32 = 12;
    /// 预览最多列出的条数（超过则「⋯ + 最后一条」）。
    pub const PREVIEW_LIMIT: usize = 10;
    pub const ELLIPSIS: &'static str = "⋯";
}

/// 计数文案：`self.tr("共 {} 个链接").format(count)`。
pub fn count_text(count: usize) -> String {
    format!("共 {count} 个链接")
}

/// `expandTemplate(template)`：模板 → URL 列表（顺序 = 笛卡尔积字典序）。
pub fn expand_template(template: &str) -> Vec<String> {
    let tokens = split_tokens(template);
    if tokens.len() == 1 {
        return if template.trim().is_empty() {
            Vec::new()
        } else {
            vec![template.to_string()]
        };
    }

    let mut segments: Vec<Vec<String>> = Vec::with_capacity(tokens.len());
    for token in tokens {
        if let Some(values) = list_values(token) {
            segments.push(values);
            continue;
        }
        if let Some(values) = range_values(token) {
            segments.push(values);
            continue;
        }
        segments.push(vec![token.to_string()]);
    }

    let mut out: Vec<String> = vec![String::new()];
    for segment in segments {
        let mut next: Vec<String> = Vec::with_capacity(out.len() * segment.len());
        for prefix in &out {
            for value in &segment {
                next.push(format!("{prefix}{value}"));
            }
        }
        out = next;
    }
    out
}

/// Python `TOKEN_PATTERN.split(template)` 的等价物：普通文本与 token 交替。
fn split_tokens(template: &str) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    let mut last = 0usize;
    for found in TOKEN_PATTERN.find_iter(template) {
        out.push(&template[last..found.start()]);
        out.push(found.as_str());
        last = found.end();
    }
    out.push(&template[last..]);
    out
}

/// `LIST_PATTERN.fullmatch(token)` → 逗号切分后的候选值。
fn list_values(token: &str) -> Option<Vec<String>> {
    let found = LIST_PATTERN.find(token)?;
    if found.start() != 0 || found.end() != token.len() {
        return None;
    }
    let inner = LIST_PATTERN.captures(token)?.get(1)?.as_str();
    Some(
        inner
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// `RANGE_PATTERN.fullmatch(token)` → 展开值；不能展开时返回 `None`（当字面量）。
fn range_values(token: &str) -> Option<Vec<String>> {
    let found = RANGE_PATTERN.find(token)?;
    if found.start() != 0 || found.end() != token.len() {
        return None;
    }
    let inner = RANGE_PATTERN.captures(token)?.get(1)?.as_str();
    expand_range(inner)
}

/// `_expandRange(spec)`：`[spec]` 的方括号内容 → 展开值。
pub fn expand_range(spec: &str) -> Option<Vec<String>> {
    let parts: Vec<&str> = spec.split(':').collect();
    if parts.len() > 2 {
        return None;
    }

    let mut step: i64 = 1;
    if parts.len() == 2 {
        // Python `int()` 容忍首尾空白与正负号。
        step = parts[1].trim().parse::<i64>().ok()?;
        if step < 1 {
            return None;
        }
    }

    let (start, end) = spec_range_halves(parts[0])?;

    if is_ascii_digits(start) && is_ascii_digits(end) {
        let width = if start.starts_with('0') && start.len() > 1 {
            start.len()
        } else {
            0
        };
        let s: i64 = start.parse().ok()?;
        let e: i64 = end.parse().ok()?;
        if s > e {
            return None;
        }
        let mut values = Vec::new();
        let mut current = s;
        while current <= e {
            values.push(format!("{current:0>width$}"));
            current += step;
        }
        return Some(values);
    }

    let start_char = single_alpha(start)?;
    let end_char = single_alpha(end)?;
    let alphabet: Vec<char> = if start_char.is_ascii_lowercase() && end_char.is_ascii_lowercase() {
        ('a'..='z').collect()
    } else if start_char.is_ascii_uppercase() && end_char.is_ascii_uppercase() {
        ('A'..='Z').collect()
    } else {
        return None;
    };

    let si = alphabet.iter().position(|c| *c == start_char)?;
    let ei = alphabet.iter().position(|c| *c == end_char)?;
    if si > ei {
        return None;
    }

    let step = usize::try_from(step).ok()?;
    Some(
        alphabet[si..=ei]
            .iter()
            .step_by(step)
            .map(char::to_string)
            .collect(),
    )
}

/// `rangePart.split("-", 1)` + 两端 strip + 非空校验。
fn spec_range_halves(range_part: &str) -> Option<(&str, &str)> {
    let (start, end) = range_part.split_once('-')?;
    let start = start.trim();
    let end = end.trim();
    if start.is_empty() || end.is_empty() {
        return None;
    }
    Some((start, end))
}

fn is_ascii_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// 单字符字母（`len(x) == 1 and x.isalpha()`）；非 ASCII 字母返回 `None`。
fn single_alpha(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let first = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    if !first.is_alphabetic() {
        return None;
    }
    if first.is_ascii_alphabetic() {
        Some(first)
    } else {
        None
    }
}

/// `_onExpandNeeded` 的输出：URL 列表 + 预览文本 + 计数标签 + 确认按钮可用性。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BatchPreview {
    pub urls: Vec<String>,
    /// 预览框内容（`setPlainText` 的口径：`"\n".join(lines)`）。
    pub lines: Vec<String>,
    /// `None` = 隐藏计数标签（Python 是 `countLabel.hide()`）。
    pub count_text: Option<String>,
    /// `yesButton.setEnabled(...)`
    pub ok_enabled: bool,
}

impl BatchPreview {
    /// 预览框的纯文本（`setPlainText` 入参）。
    pub fn preview_text(&self) -> String {
        self.lines.join("\n")
    }
}

/// 展开模板并算出对话框的完整状态（对应 `_onExpandNeeded`）。
pub fn preview(template: &str) -> BatchPreview {
    let template = template.trim();
    if template.is_empty() {
        return BatchPreview::default();
    }

    let urls = expand_template(template);
    let count = urls.len();
    let mut preview = BatchPreview {
        urls,
        lines: Vec::new(),
        count_text: None,
        ok_enabled: count > 0,
    };
    if count == 0 {
        return preview;
    }

    preview.lines = preview
        .urls
        .iter()
        .take(BatchUrlTexts::PREVIEW_LIMIT)
        .cloned()
        .collect();
    if count > BatchUrlTexts::PREVIEW_LIMIT {
        preview.lines.push(BatchUrlTexts::ELLIPSIS.to_string());
        preview.lines.push(preview.urls[count - 1].clone());
    }
    preview.count_text = Some(count_text(count));
    preview
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_template_returns_itself_once() {
        assert_eq!(
            expand_template("http://example.com/a.jpg"),
            vec!["http://example.com/a.jpg"]
        );
        // strip 只用于判空，返回的是原始模板。
        assert_eq!(
            expand_template("  http://example.com/a.jpg  "),
            vec!["  http://example.com/a.jpg  "]
        );
        assert!(expand_template("").is_empty());
        assert!(expand_template("   \n\t ").is_empty());
    }

    #[test]
    fn numeric_range_expands_inclusive() {
        assert_eq!(
            expand_template("http://e/img/[1-4].jpg"),
            vec![
                "http://e/img/1.jpg",
                "http://e/img/2.jpg",
                "http://e/img/3.jpg",
                "http://e/img/4.jpg",
            ]
        );
    }

    #[test]
    fn zero_padding_only_when_start_is_zero_prefixed() {
        assert_eq!(
            expand_range("001-003"),
            Some(vec!["001".into(), "002".into(), "003".into()])
        );
        // 起点 "1" 不补零 → 终点位数不影响。
        assert_eq!(
            expand_range("1-3"),
            Some(vec!["1".into(), "2".into(), "3".into()])
        );
        // 宽度取起点长度：起点 01、终点 100 → 补到 2 位（不截断）。
        let padded = expand_range("01-100").expect("01-100 应可展开");
        assert_eq!(padded.len(), 100);
        assert_eq!(padded[0], "01");
        assert_eq!(padded[9], "10");
        assert_eq!(padded[99], "100");
    }

    #[test]
    fn step_and_letter_ranges() {
        assert_eq!(
            expand_range("1-10:2"),
            Some(vec![
                "1".into(),
                "3".into(),
                "5".into(),
                "7".into(),
                "9".into()
            ])
        );
        assert_eq!(
            expand_range("a-e:2"),
            Some(vec!["a".into(), "c".into(), "e".into()])
        );
        assert_eq!(
            expand_range("A-C"),
            Some(vec!["A".into(), "B".into(), "C".into()])
        );
    }

    #[test]
    fn invalid_ranges_fall_back_to_literal() {
        // 步长非法
        assert_eq!(expand_range("1-5:0"), None);
        assert_eq!(expand_range("1-5:-1"), None);
        assert_eq!(expand_range("1-5:x"), None);
        // 段数过多
        assert_eq!(expand_range("1-5:2:3"), None);
        // 缺端点 / 空端点
        assert_eq!(expand_range("5"), None);
        assert_eq!(expand_range("-5"), None);
        assert_eq!(expand_range("5-"), None);
        // 倒序
        assert_eq!(expand_range("5-1"), None);
        assert_eq!(expand_range("e-a"), None);
        // 大小写混用
        assert_eq!(expand_range("a-Z"), None);
        // 数字与字母混用
        assert_eq!(expand_range("1-a"), None);
        // 非 ASCII 数字（保守偏离）
        assert_eq!(expand_range("١-٣"), None);

        // `[x-y]` 是合法的字母范围（→ x, y），要当字面量得用真正非法的组合。
        assert_eq!(
            expand_template("http://e/[x-y].jpg"),
            vec!["http://e/x.jpg", "http://e/y.jpg"]
        );
        assert_eq!(
            expand_template("http://e/[1-x].jpg"),
            vec!["http://e/[1-x].jpg"]
        );
    }

    #[test]
    fn list_expands_and_drops_empty_items() {
        assert_eq!(
            expand_template("http://e/{mp4,mkv}/v"),
            vec!["http://e/mp4/v", "http://e/mkv/v"]
        );
        // 空项被丢弃
        assert_eq!(
            expand_template("http://e/{a,,b}/v"),
            vec!["http://e/a/v", "http://e/b/v"]
        );
        // 全空 → 空列表 → 积为空
        assert!(expand_template("http://e/{,}/v").is_empty());
    }

    #[test]
    fn product_order_matches_itertools() {
        assert_eq!(expand_template("[1-2]{a,b}"), vec!["1a", "1b", "2a", "2b"]);
    }

    #[test]
    fn multiple_tokens_and_literals_combine() {
        assert_eq!(
            expand_template("http://e/[01-02]/{a,b}.bin"),
            vec![
                "http://e/01/a.bin",
                "http://e/01/b.bin",
                "http://e/02/a.bin",
                "http://e/02/b.bin",
            ]
        );
    }

    #[test]
    fn empty_brackets_stay_literal() {
        // `[]` 不匹配 `\[([^\]]+)\]` → 整段当字面量。
        assert_eq!(expand_template("a[]b"), vec!["a[]b"]);
        assert_eq!(expand_template("a{}b"), vec!["a{}b"]);
    }

    #[test]
    fn preview_empty_template_disables_ok() {
        let preview = preview("   ");
        assert_eq!(preview, BatchPreview::default());
        assert!(!preview.ok_enabled);
        assert_eq!(preview.preview_text(), "");
    }

    #[test]
    fn preview_lists_first_ten_plus_ellipsis_and_last() {
        let preview = preview("http://e/[1-12].jpg");
        assert_eq!(preview.urls.len(), 12);
        assert!(preview.ok_enabled);
        assert_eq!(preview.count_text.as_deref(), Some("共 12 个链接"));
        assert_eq!(preview.lines.len(), 12);
        assert_eq!(preview.lines[9], "http://e/10.jpg");
        assert_eq!(preview.lines[10], BatchUrlTexts::ELLIPSIS);
        assert_eq!(preview.lines[11], "http://e/12.jpg");
    }

    #[test]
    fn preview_without_overflow_has_no_ellipsis() {
        let preview = preview("http://e/[1-10].jpg");
        assert_eq!(preview.lines.len(), 10);
        assert!(!preview.lines.contains(&BatchUrlTexts::ELLIPSIS.to_string()));
        assert_eq!(preview.count_text.as_deref(), Some("共 10 个链接"));
    }

    #[test]
    fn preview_zero_results_hides_count_but_keeps_ok_disabled() {
        let preview = preview("http://e/{,}/v");
        assert!(preview.urls.is_empty());
        assert_eq!(preview.count_text, None);
        assert_eq!(preview.preview_text(), "");
        assert!(!preview.ok_enabled);
    }

    #[test]
    fn count_text_matches_python_format() {
        assert_eq!(count_text(0), "共 0 个链接");
        assert_eq!(count_text(100), "共 100 个链接");
    }

    #[test]
    fn single_value_list_is_not_collapsed() {
        // `{a}` → 一个候选，结果仍是模板本身（Python 同样不特殊处理）。
        assert_eq!(expand_template("x{a}y"), vec!["xay"]);
    }
}
