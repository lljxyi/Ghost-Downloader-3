//! 字幕语言选择对话框（P3，第 67 步）：镜像
//! `app/view/dialogs/subtitle_select.py`（88 行）的语义层。
//!
//! ## 实证口径（2026-10-18 实读源码）
//!
//! - 构造参数 `choices: list[tuple[str, str]]`（`(langCode, label)`）与
//!   `selected: list[str]`（**语言代码**列表，`_initWidget(set(selected))` 先转集合）。
//! - `_initWidget`（:33-49）：`widget.setMinimumWidth(400)`、`yesButton` = `确定`、
//!   `cancelButton` = `取消`；`AutoSizingTreeView(minimumVisibleRows=3)`；
//!   每行一个 `QStandardItem(label)`，可勾选，`Checked` 当且仅当
//!   `langCode in selected`，`UserRole` 存 `langCode`。
//! - `_setAll(checked)`（:71-74）：逐行 `setCheckState`；`全选` / `全不选` 各接一次。
//! - `_refreshSummary`（:76-81）：`"{0}/{1} 种语言".format(count, rowCount)`，
//!   `count` 只数 `Checked` 的行。`itemChanged` 每变一次都刷新。
//! - `selectedLanguages`（:83-88）：按**模型顺序**（不是点击顺序）收集被勾选行的
//!   `UserRole`，所以返回的是 `langCode` 而非 `label`。
//!
//! ## 与 Qt 的差异
//!
//! Qt 侧靠 `itemChanged` 信号驱动摘要刷新；Rust 侧把「改状态」与「取摘要」拆开，
//! `set_checked` / `set_all` 只改状态，外壳在调用后重新读 [`SubtitleSelectState::summary_text`]。
//! `checked` 用与 `choices` 等长的 `Vec<bool>` 而不是 `HashSet`，因为同一 `langCode`
//! 出现两次时 Python 会**两行都勾上**（集合成员测试），用位向量才能照抄这个行为。

// ---------------------------------------------------------------------------
// 文案与常量（`subtitle_select.py` 逐字）
// ---------------------------------------------------------------------------

/// `SubtitleLabel(self.tr("选择字幕语言"))`（:19）
pub const TITLE: &str = "选择字幕语言";
/// `PrimaryPushButton(self.tr("全选"))`（:22）
pub const BUTTON_SELECT_ALL: &str = "全选";
/// `PushButton(self.tr("全不选"))`（:23）
pub const BUTTON_CLEAR: &str = "全不选";
/// `yesButton.setText(self.tr("确定"))`（:35）
pub const BUTTON_OK: &str = "确定";
/// `cancelButton.setText(self.tr("取消"))`（:36）
pub const BUTTON_CANCEL: &str = "取消";
/// `self.widget.setMinimumWidth(400)`（:34）
pub const MIN_WIDTH: i32 = 400;
/// `AutoSizingTreeView(self, minimumVisibleRows=3)`（:25）
pub const MIN_VISIBLE_ROWS: i32 = 3;
/// `"{0}/{1} 种语言"`（:81）
pub const SUMMARY_TEMPLATE: &str = "{0}/{1} 种语言";

/// 一行字幕语言选项（`choices` 里的一项）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubtitleChoice {
    /// `(langCode, label)` 的 `langCode`，勾选态与返回值都按它走。
    pub lang_code: String,
    /// 显示文本。
    pub label: String,
}

impl SubtitleChoice {
    pub fn new(lang_code: &str, label: &str) -> Self {
        Self {
            lang_code: lang_code.to_string(),
            label: label.to_string(),
        }
    }
}

/// 摊平给 Slint `ListView` 的一行（`treeView` + `treeModel` 的 `UserRole`/`CheckState`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubtitleRow {
    pub lang_code: String,
    pub label: String,
    pub checked: bool,
}

/// 对话框状态：`treeModel` 的行序 + 每行的勾选态。
#[derive(Clone, Debug, Default)]
pub struct SubtitleSelectState {
    choices: Vec<SubtitleChoice>,
    checked: Vec<bool>,
}

impl SubtitleSelectState {
    /// 镜像 `_initWidget(set(selected))`：`selected` 里出现的 `langCode` 全勾上。
    ///
    /// 集合成员测试意味着**重复的 `langCode` 会被一起勾上**，未知代码被忽略。
    pub fn new(choices: Vec<SubtitleChoice>, selected: &[&str]) -> Self {
        let checked = choices
            .iter()
            .map(|choice| selected.contains(&choice.lang_code.as_str()))
            .collect();
        Self { choices, checked }
    }

    pub fn choices(&self) -> &[SubtitleChoice] {
        &self.choices
    }

    pub fn row_count(&self) -> usize {
        self.choices.len()
    }

    /// 先序摊平给 Slint。
    pub fn rows(&self) -> Vec<SubtitleRow> {
        self.choices
            .iter()
            .zip(self.checked.iter())
            .map(|(choice, checked)| SubtitleRow {
                lang_code: choice.lang_code.clone(),
                label: choice.label.clone(),
                checked: *checked,
            })
            .collect()
    }

    pub fn is_checked(&self, index: usize) -> bool {
        self.checked.get(index).copied().unwrap_or(false)
    }

    /// 单行勾选（`item.setCheckState(...)`）。越界返回 `false`，不改状态。
    pub fn set_checked(&mut self, index: usize, checked: bool) -> bool {
        match self.checked.get_mut(index) {
            Some(slot) => {
                *slot = checked;
                true
            }
            None => false,
        }
    }

    /// `_setAll(checked)`：逐行设置，与逐行 `itemChanged` 的终态一致。
    pub fn set_all(&mut self, checked: bool) {
        self.checked.fill(checked);
    }

    pub fn checked_count(&self) -> usize {
        self.checked.iter().filter(|checked| **checked).count()
    }

    /// `"{0}/{1} 种语言"`（`_refreshSummary`，:81）。
    pub fn summary_text(&self) -> String {
        SUMMARY_TEMPLATE
            .replace("{0}", &self.checked_count().to_string())
            .replace("{1}", &self.row_count().to_string())
    }

    /// `selectedLanguages()`：按模型顺序返回被勾选行的 `langCode`（:83-88）。
    pub fn selected_languages(&self) -> Vec<String> {
        self.choices
            .iter()
            .zip(self.checked.iter())
            .filter(|(_, checked)| **checked)
            .map(|(choice, _)| choice.lang_code.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choices() -> Vec<SubtitleChoice> {
        vec![
            SubtitleChoice::new("zh", "中文"),
            SubtitleChoice::new("en", "English"),
            SubtitleChoice::new("ja", "日本語"),
        ]
    }

    #[test]
    fn selected_codes_check_their_rows() {
        let state = SubtitleSelectState::new(choices(), &["en"]);
        assert_eq!(state.row_count(), 3);
        assert_eq!(
            state.rows(),
            vec![
                SubtitleRow {
                    lang_code: "zh".into(),
                    label: "中文".into(),
                    checked: false,
                },
                SubtitleRow {
                    lang_code: "en".into(),
                    label: "English".into(),
                    checked: true,
                },
                SubtitleRow {
                    lang_code: "ja".into(),
                    label: "日本語".into(),
                    checked: false,
                },
            ]
        );
        assert_eq!(state.summary_text(), "1/3 种语言");
        assert_eq!(state.selected_languages(), vec!["en".to_string()]);
    }

    #[test]
    fn empty_selection_summarises_zero() {
        let state = SubtitleSelectState::new(choices(), &[]);
        assert_eq!(state.checked_count(), 0);
        assert_eq!(state.summary_text(), "0/3 种语言");
        assert!(state.selected_languages().is_empty());
    }

    #[test]
    fn unknown_selected_codes_are_ignored() {
        let state = SubtitleSelectState::new(choices(), &["fr", "de"]);
        assert_eq!(state.checked_count(), 0);
        assert_eq!(state.summary_text(), "0/3 种语言");
    }

    /// `_setAll` 两个按钮：全选 → 3/3，全不选 → 0/3。
    #[test]
    fn set_all_toggles_every_row() {
        let mut state = SubtitleSelectState::new(choices(), &[]);
        state.set_all(true);
        assert_eq!(state.summary_text(), "3/3 种语言");
        assert_eq!(
            state.selected_languages(),
            vec!["zh".to_string(), "en".to_string(), "ja".to_string()]
        );
        state.set_all(false);
        assert_eq!(state.summary_text(), "0/3 种语言");
        assert!(state.selected_languages().is_empty());
    }

    /// 单行勾选（`itemChanged` 的来源）；越界不改状态。
    #[test]
    fn set_checked_handles_single_rows_and_out_of_range() {
        let mut state = SubtitleSelectState::new(choices(), &[]);
        assert!(state.set_checked(2, true));
        assert!(state.is_checked(2));
        assert_eq!(state.summary_text(), "1/3 种语言");
        assert!(!state.set_checked(3, true));
        assert_eq!(state.row_count(), 3);
        assert!(!state.is_checked(3));
        assert!(state.set_checked(2, false));
        assert_eq!(state.summary_text(), "0/3 种语言");
    }

    /// `selectedLanguages` 按模型顺序，不按 `selected` 传入顺序。
    #[test]
    fn selected_languages_follow_model_order() {
        let state = SubtitleSelectState::new(choices(), &["ja", "zh"]);
        assert_eq!(
            state.selected_languages(),
            vec!["zh".to_string(), "ja".to_string()]
        );
    }

    /// 重复的 `langCode` 一起被勾上（`set(selected)` 的成员测试口径）。
    #[test]
    fn duplicate_lang_codes_are_all_checked() {
        let dup = vec![
            SubtitleChoice::new("zh", "中文（简体）"),
            SubtitleChoice::new("zh", "中文（繁体）"),
            SubtitleChoice::new("en", "English"),
        ];
        let state = SubtitleSelectState::new(dup, &["zh"]);
        assert!(state.is_checked(0));
        assert!(state.is_checked(1));
        assert!(!state.is_checked(2));
        assert_eq!(state.summary_text(), "2/3 种语言");
        assert_eq!(
            state.selected_languages(),
            vec!["zh".to_string(), "zh".to_string()]
        );
    }

    /// 空选项列表：摘要落在 `0/0`，不 panic。
    #[test]
    fn empty_choices_summarise_zero_of_zero() {
        let state = SubtitleSelectState::new(Vec::new(), &["zh"]);
        assert_eq!(state.rows(), Vec::new());
        assert_eq!(state.summary_text(), "0/0 种语言");
    }

    #[test]
    fn summary_template_matches_python_literal() {
        assert_eq!(SUMMARY_TEMPLATE, "{0}/{1} 种语言");
        assert_eq!(MIN_WIDTH, 400);
        assert_eq!(MIN_VISIBLE_ROWS, 3);
    }
}
