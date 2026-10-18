//! 任务草稿卡片（第 77 步）：`app/view/cards/draft_cards.py`(195) 的语义镜像。
//!
//! 为什么单独一层：`card_groups.rs` 只管「分组容器里放几张卡片、卡片多高」，
//! 这张卡片自己的**交互契约**（改名、切分类、多文件摘要）此前没有落点。
//!
//! ## 实证口径（2026-10-18 实读源码）
//!
//! - 卡片固定高 **35**（:53）、布局 margins `(10, 2, 10, 2)`（:71）、间距 **12**
//!   （:72）、文件图标 **16×16**（:37）、两个按钮 **28×28**（:57/:59）、
//!   冲突图标 **16×16**（:63）；`paintEvent` 在顶部铺 1px 分隔线
//!   `QColor(0, 0, 0, 96 if dark else 24)`（:151）。
//! - 尺寸标签（:40）：`toReadableSize(fileSize) if fileSize > 1 else "--"`——
//!   **注意阈值是 `> 1`**，`fileSize == 1` 也显示 `"--"`。
//! - 改名（:93-110）：进入编辑时选中**词干**（`splitStemExt` 的 stem，
//!   `setSelection(0, len(stem))`，按字符数）；提交时 `text().strip()`，
//!   非空且与原值不同才改名，改名走 `task.setName()` →
//!   **`toSafeFilename(newName, fallback=self.name or "download")`**，
//!   随后标签与编辑框都显示**净化后**的名字，并刷新文件图标与冲突图标；
//!   无论改没改都 `nameEdit.hide()` + `nameLabel.show()`。
//! - 分类按钮（:112-123）：`cfg.isCategoryEnabled` 为假时 **`hide()`**；
//!   找到分类 → `getattr(FluentIcon, category.icon, TAG)` + `toCategoryName`；
//!   找不到 → `TAG` + `"未分类"`。
//! - 分类菜单（:125-137）：第一项恒为 `未分类`（回传 `""`）+ `addSeparator()`
//!   + 逐分类项；`menu.exec(button.mapToGlobal(button.rect().bottomLeft()))`
//!     ——锚点是按钮的**左下角**。
//! - 选中分类（:139-143）：先写 `task.category`，再刷按钮、刷冲突图标，
//!   最后 `categoryPicked.emit(categoryId)`。
//! - 多文件卡（:154-195）：只有 `len(files) > 1` 才建「选择文件」按钮
//!   （`FluentIcon.LIBRARY`，28×28，tooltip `"选择文件"`）；摘要文案
//!   `"{selected}/{total} 个文件 · {size}"`，而 `files` 为空或只有 1 个时
//!   退回 `toReadableSize(fileSize)`——**这条分支不带 `"--"` 兜底**，
//!   与基类的尺寸标签口径不同。
//! - `task.setSelection(indexes)`（`models/task.py:411-418`）：按 `file.index`
//!   命中集合置 `selected`，然后 **`fileSize = Σ selected.size`**。
//!
//! ## 刻意的偏离
//!
//! - **不建模 `QFileIconProvider`**：图标是平台相关位图，本层只给尺寸常量；
//!   图标怎么取由外壳层决定。
//! - **`probeConflict` / `categoryService` 一律当入参**：冲突探测要读文件系统、
//!   分类服务要读配置，都不是本层的职责。`conflict_visible(Option<&T>)` 只
//!   复刻 `is not None` 这一句。
//! - **`finish_name_edit` 返回 `Option<String>`**：`Some(name)` = 发生了改名，
//!   调用方据此重刷文件图标与冲突图标（Python 在 `_onNameEdited` 里直接调）。
//! - **`getattr(FluentIcon, ...)` 走白名单**（[`icon_or_tag`]），与
//!   `category_settings` 同一条偏差。
//! - 无译文层，文案按中文原文硬编码。

use crate::card_groups::Rgba;
use crate::category::CategoryRule;
use crate::category_settings::{FALLBACK_ICON, icon_or_tag, to_category_name};
use crate::format::to_readable_size;
use crate::pystr::py_strip;
use gd3_core::filename::{split_stem_ext, to_safe_filename};

/// `setFixedHeight(35)`（:53），与分组容器的卡片高一致。
pub use crate::card_groups::DRAFT_CARD_HEIGHT;

/// `layout.setContentsMargins(10, 2, 10, 2)`（:71）。
pub const CARD_MARGINS: (i32, i32, i32, i32) = (10, 2, 10, 2);
/// `layout.setSpacing(12)`（:72）。
pub const CARD_SPACING: i32 = 12;
/// `iconLabel.setFixedSize(16, 16)` / `conflictIcon.setFixedSize(16, 16)`（:37/:63）。
pub const ICON_SIZE: i32 = 16;
/// `categoryButton` / `editButton` / `_selectFilesButton` 的 `setFixedSize(28, 28)`。
pub const BUTTON_SIZE: i32 = 28;
/// `editButton.setToolTip(self.tr("编辑任务参数"))`（:60）。
pub const EDIT_TOOLTIP: &str = "编辑任务参数";
/// `conflictIcon.setToolTip(self.tr("下载目录中已有同名文件，下载时按设置处理"))`（:64）。
pub const CONFLICT_TOOLTIP: &str = "下载目录中已有同名文件，下载时按设置处理";
/// `_selectFilesButton.setToolTip(self.tr("选择文件"))`（:163）。
pub const SELECT_FILES_TOOLTIP: &str = "选择文件";
/// 无分类时分类按钮的提示文案（:122）。
pub const UNCATEGORIZED_LABEL: &str = "未分类";
/// 无分类项回传的 categoryId（:128 的 `""`）。
pub const UNCATEGORIZED_ID: &str = "";
/// `toSafeFilename` 的兜底名（`models/task.py:285` 的 `self.name or "download"`）。
pub const FALLBACK_NAME: &str = "download";
/// `toSafeFilename` 的默认长度上限（`filesystem.py:39`）。
pub const MAX_NAME_LENGTH: usize = 200;

/// `painter.fillRect(0, 0, w, 1, QColor(0, 0, 0, 96 if dark else 24))`（:151）。
pub fn separator_color(dark: bool) -> Rgba {
    (0, 0, 0, if dark { 96 } else { 24 })
}

/// 基类的尺寸标签（:40）：`> 1` 才格式化，否则 `"--"`。
pub fn size_label_text(file_size: i64) -> String {
    if file_size > 1 {
        to_readable_size(file_size as f64)
    } else {
        "--".to_string()
    }
}

/// `setSelection(0, len(stem))`（:98-99）：返回 `(起点, 长度)`，长度按**字符**算。
pub fn name_edit_selection(name: &str) -> (i32, i32) {
    let (stem, _) = split_stem_ext(name);
    (0, stem.chars().count() as i32)
}

/// `menu.exec(button.mapToGlobal(button.rect().bottomLeft()))`（:137）。
pub fn menu_anchor(button_left: i32, button_top: i32, button_height: i32) -> (i32, i32) {
    (button_left, button_top + button_height)
}

/// `self._probeConflict(self._task) is not None`（:87）。
pub fn conflict_visible<T>(probe: Option<&T>) -> bool {
    probe.is_some()
}

/// 分类按钮的显示态（:117-122）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CategoryButton {
    /// 已过 [`icon_or_tag`] 的图标名。
    pub icon: String,
    /// `toCategoryName(category)` 或 `"未分类"`。
    pub tooltip: String,
}

/// `_refreshCategoryButton`（:112-123）：`None` = 按钮 `hide()`。
pub fn category_button(
    enabled: bool,
    categories: &[CategoryRule],
    category_id: &str,
) -> Option<CategoryButton> {
    if !enabled {
        return None;
    }
    match categories
        .iter()
        .find(|rule| rule.category_id == category_id)
    {
        Some(rule) => Some(CategoryButton {
            icon: icon_or_tag(&rule.icon).to_string(),
            tooltip: to_category_name(&rule.name).to_string(),
        }),
        None => Some(CategoryButton {
            icon: FALLBACK_ICON.to_string(),
            tooltip: UNCATEGORIZED_LABEL.to_string(),
        }),
    }
}

/// 分类菜单的一项（:127-136）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftMenuEntry {
    /// `Action(icon, text)` + 点击回传的 categoryId。
    Action {
        icon: String,
        text: String,
        category_id: String,
    },
    /// `menu.addSeparator()`。
    Separator,
}

/// `_showCategoryMenu`（:125-136）：未分类 + 分隔线 + 逐个分类。
pub fn category_menu(categories: &[CategoryRule]) -> Vec<DraftMenuEntry> {
    let mut entries = vec![
        DraftMenuEntry::Action {
            icon: FALLBACK_ICON.to_string(),
            text: UNCATEGORIZED_LABEL.to_string(),
            category_id: UNCATEGORIZED_ID.to_string(),
        },
        DraftMenuEntry::Separator,
    ];
    entries.extend(categories.iter().map(|rule| DraftMenuEntry::Action {
        icon: icon_or_tag(&rule.icon).to_string(),
        text: to_category_name(&rule.name).to_string(),
        category_id: rule.category_id.clone(),
    }));
    entries
}

/// 卡片对外发的两个信号（:25-26）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftCardSignal {
    /// `categoryPicked = Signal(str)`。
    CategoryPicked(String),
    /// `editRequested = Signal()`。
    EditRequested,
}

/// `editButton.clicked.connect(self.editRequested.emit)`（:82）。
pub fn edit_requested() -> DraftCardSignal {
    DraftCardSignal::EditRequested
}

/// `DraftCard` 的数据面：任务名 / 尺寸 / 可编辑 / 分类 + 改名编辑态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftCardState {
    name: String,
    file_size: i64,
    can_edit: bool,
    category: String,
    /// `nameEdit.isVisible()`。
    editing: bool,
    /// `nameEdit.text()`。
    edit_text: String,
}

impl DraftCardState {
    pub fn new(
        name: impl Into<String>,
        file_size: i64,
        can_edit: bool,
        category: impl Into<String>,
    ) -> Self {
        let name = name.into();
        Self {
            edit_text: name.clone(),
            name,
            file_size,
            can_edit,
            category: category.into(),
            editing: false,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn file_size(&self) -> i64 {
        self.file_size
    }

    pub fn can_edit(&self) -> bool {
        self.can_edit
    }

    pub fn category(&self) -> &str {
        &self.category
    }

    pub fn editing(&self) -> bool {
        self.editing
    }

    pub fn edit_text(&self) -> &str {
        &self.edit_text
    }

    /// `sizeLabel` 的文本（:40）。
    pub fn size_label(&self) -> String {
        size_label_text(self.file_size)
    }

    /// `editButton.setVisible(self._task.canEdit)`（:62）。
    pub fn edit_button_visible(&self) -> bool {
        self.can_edit
    }

    /// `_refreshCategoryButton` 的结果（:112-123）。
    pub fn category_button(
        &self,
        enabled: bool,
        categories: &[CategoryRule],
    ) -> Option<CategoryButton> {
        category_button(enabled, categories, &self.category)
    }

    /// `_onNameEditRequested`（:93-99）：返回新的选中区间。
    pub fn start_name_edit(&mut self) -> (i32, i32) {
        self.editing = true;
        self.edit_text = self.name.clone();
        name_edit_selection(&self.name)
    }

    /// `_onNameEdited`（:101-110）。
    ///
    /// `Some(name)` = 发生了改名（名字已净化），调用方需要重刷文件图标与冲突图标；
    /// `None` = 没改名。两种情况都会退出编辑态。
    ///
    /// 未处于编辑态时直接返回 `None`：Python 的 `editingFinished` 只会在
    /// `nameEdit` 拿到过焦点后触发，而它初始是隐藏的。
    pub fn finish_name_edit(&mut self) -> Option<String> {
        if !self.editing {
            return None;
        }
        self.editing = false;

        let new_name = py_strip(&self.edit_text).to_string();
        if new_name.is_empty() || new_name == self.name {
            // Python 保留编辑框里的原文，但下一次 start_name_edit 会覆盖它。
            return None;
        }

        let fallback = if self.name.is_empty() {
            FALLBACK_NAME.to_string()
        } else {
            self.name.clone()
        };
        let safe = to_safe_filename(&new_name, &fallback, MAX_NAME_LENGTH);
        self.name = safe.clone();
        self.edit_text = safe.clone();
        Some(safe)
    }

    /// `_onCategoryPicked`（:139-143）：先写分类，再刷按钮/冲突图标，最后发信号。
    pub fn pick_category(&mut self, category_id: &str) -> DraftCardSignal {
        self.category = category_id.to_string();
        DraftCardSignal::CategoryPicked(category_id.to_string())
    }
}

impl DraftCardState {
    /// `task.setSelection` 之后回写 `fileSize`（`models/task.py:417`）。
    pub fn set_file_size(&mut self, file_size: i64) {
        self.file_size = file_size;
    }
}

/// `task.files` 的一项（`models/task.py` 的 `TaskFile`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftFile {
    pub index: i64,
    pub size: i64,
    pub selected: bool,
}

impl DraftFile {
    pub fn new(index: i64, size: i64, selected: bool) -> Self {
        Self {
            index,
            size,
            selected,
        }
    }
}

/// `MultiFileDraftCard` 的数据面：基类状态 + 文件清单。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultiFileDraftCardState {
    card: DraftCardState,
    files: Vec<DraftFile>,
}

impl MultiFileDraftCardState {
    pub fn new(card: DraftCardState, files: Vec<DraftFile>) -> Self {
        Self { card, files }
    }

    pub fn card(&self) -> &DraftCardState {
        &self.card
    }

    pub fn card_mut(&mut self) -> &mut DraftCardState {
        &mut self.card
    }

    pub fn files(&self) -> &[DraftFile] {
        &self.files
    }

    /// `if self._task.files and len(self._task.files) > 1`（:160）。
    pub fn has_select_button(&self) -> bool {
        self.files.len() > 1
    }

    /// `_refreshSummary`（:186-195）。
    ///
    /// 注意 <=1 的分支直接用 `toReadableSize(fileSize)`，**不带基类那条
    /// `"--"` 兜底**。
    pub fn summary_text(&self) -> String {
        if self.files.len() <= 1 {
            return to_readable_size(self.card.file_size as f64);
        }
        let selected = self.files.iter().filter(|file| file.selected).count();
        format!(
            "{}/{} 个文件 · {}",
            selected,
            self.files.len(),
            to_readable_size(self.card.file_size as f64)
        )
    }

    /// `task.setSelection(indexes)`（`models/task.py:411-418`）：按 `index` 命中
    /// 集合置 `selected`，再令 `fileSize = Σ selected.size`。
    ///
    /// `files` 为空时 Python 直接 `return`（:412-413），这里同理。
    pub fn apply_selection(&mut self, indexes: &[i64]) {
        if self.files.is_empty() {
            return;
        }
        for file in &mut self.files {
            file.selected = indexes.contains(&file.index);
        }
        let total = self
            .files
            .iter()
            .filter(|file| file.selected)
            .map(|file| file.size)
            .sum();
        self.card.set_file_size(total);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(id: &str, name: &str, icon: &str) -> CategoryRule {
        CategoryRule {
            category_id: id.to_string(),
            name: name.to_string(),
            icon: icon.to_string(),
            extensions: Vec::new(),
            folder: None,
        }
    }

    fn categories() -> Vec<CategoryRule> {
        vec![
            rule("cat_video", "视频", "VIDEO"),
            rule("cat_audio", "音频", "MUSIC"),
            rule("cat_bad", "怪图标", "NO_SUCH_ICON"),
        ]
    }

    #[test]
    fn layout_constants_match_the_qt_fixed_sizes() {
        assert_eq!(DRAFT_CARD_HEIGHT, 35);
        assert_eq!(CARD_MARGINS, (10, 2, 10, 2));
        assert_eq!(CARD_SPACING, 12);
        assert_eq!(ICON_SIZE, 16);
        assert_eq!(BUTTON_SIZE, 28);
        assert_eq!(separator_color(true), (0, 0, 0, 96));
        assert_eq!(separator_color(false), (0, 0, 0, 24));
    }

    #[test]
    fn size_label_uses_strictly_greater_than_one() {
        assert_eq!(size_label_text(0), "--");
        assert_eq!(size_label_text(1), "--", "阈值是 > 1，等于 1 也兜底");
        assert_eq!(size_label_text(2), "2.00 B");
        assert_eq!(size_label_text(1024), "1.00 KB");
        assert_eq!(DraftCardState::new("a", 1, true, "").size_label(), "--");
    }

    #[test]
    fn name_edit_selection_covers_the_stem_only() {
        assert_eq!(name_edit_selection("a.tar.gz"), (0, 1));
        assert_eq!(
            name_edit_selection("视频.mp4"),
            (0, 2),
            "按字符数不是字节数"
        );
        assert_eq!(name_edit_selection(".bashrc"), (0, 7));
        assert_eq!(name_edit_selection("a"), (0, 1));
    }

    #[test]
    fn start_name_edit_exposes_the_full_name() {
        let mut card = DraftCardState::new("视频.mp4", 1024, true, "cat_video");
        assert_eq!(card.start_name_edit(), (0, 2));
        assert!(card.editing());
        assert_eq!(card.edit_text(), "视频.mp4");
    }

    #[test]
    fn finish_name_edit_renames_and_sanitizes() {
        let mut card = DraftCardState::new("old.mp4", 1024, true, "");
        card.start_name_edit();
        card.edit_text = "  a/b  ".to_string();
        assert_eq!(card.finish_name_edit(), Some("a_b".to_string()));
        assert_eq!(card.name(), "a_b");
        assert_eq!(card.edit_text(), "a_b", "编辑框显示净化后的名字");
        assert!(!card.editing(), "提交后退出编辑态");
    }

    #[test]
    fn finish_name_edit_ignores_blank_and_unchanged() {
        let mut card = DraftCardState::new("keep.mp4", 1024, true, "");
        card.start_name_edit();
        card.edit_text = "   ".to_string();
        assert_eq!(card.finish_name_edit(), None);
        assert_eq!(card.name(), "keep.mp4");
        assert!(!card.editing());

        card.start_name_edit();
        card.edit_text = "  keep.mp4 ".to_string();
        assert_eq!(card.finish_name_edit(), None, "strip 后与原名相同不算改名");
        assert_eq!(card.name(), "keep.mp4");
    }

    #[test]
    fn finish_name_edit_is_a_noop_outside_editing() {
        let mut card = DraftCardState::new("keep.mp4", 1024, true, "");
        card.edit_text = "other.mp4".to_string();
        assert_eq!(card.finish_name_edit(), None);
        assert_eq!(card.name(), "keep.mp4");
    }

    #[test]
    fn finish_name_edit_falls_back_to_the_old_name() {
        let mut card = DraftCardState::new("old.mp4", 1024, true, "");
        card.start_name_edit();
        card.edit_text = ".".to_string();
        // toSafeFilename(".") 净化后为空 → fallback = 旧名字。
        assert_eq!(card.finish_name_edit(), Some("old.mp4".to_string()));
    }

    #[test]
    fn category_button_hides_when_categories_are_disabled() {
        let card = DraftCardState::new("a", 1024, true, "cat_video");
        assert_eq!(card.category_button(false, &categories()), None);
        assert!(card.category_button(true, &categories()).is_some());
    }

    #[test]
    fn category_button_falls_back_to_tag() {
        let card = DraftCardState::new("a", 1024, true, "");
        let button = card.category_button(true, &categories()).unwrap();
        assert_eq!(button.icon, "TAG");
        assert_eq!(button.tooltip, "未分类");

        let card = DraftCardState::new("a", 1024, true, "cat_bad");
        let button = card.category_button(true, &categories()).unwrap();
        assert_eq!(button.icon, "TAG", "不认识的图标名退化成 TAG");
        assert_eq!(button.tooltip, "怪图标", "名字仍然照原样显示");
    }

    #[test]
    fn category_button_shows_the_matched_category() {
        let card = DraftCardState::new("a", 1024, true, "cat_audio");
        let button = card.category_button(true, &categories()).unwrap();
        assert_eq!(button.icon, "MUSIC");
        assert_eq!(button.tooltip, "音频");
    }

    #[test]
    fn category_menu_is_uncategorized_then_separator_then_rules() {
        let menu = category_menu(&categories());
        assert_eq!(menu.len(), 2 + 3);
        assert_eq!(
            menu[0],
            DraftMenuEntry::Action {
                icon: "TAG".to_string(),
                text: "未分类".to_string(),
                category_id: String::new(),
            }
        );
        assert_eq!(menu[1], DraftMenuEntry::Separator);
        assert_eq!(
            menu[2],
            DraftMenuEntry::Action {
                icon: "VIDEO".to_string(),
                text: "视频".to_string(),
                category_id: "cat_video".to_string(),
            }
        );
        assert_eq!(
            menu[4],
            DraftMenuEntry::Action {
                icon: "TAG".to_string(),
                text: "怪图标".to_string(),
                category_id: "cat_bad".to_string(),
            }
        );
        // 空分类列表也至少有「未分类 + 分隔线」。
        assert_eq!(category_menu(&[]).len(), 2);
    }

    #[test]
    fn pick_category_updates_state_and_emits() {
        let mut card = DraftCardState::new("a", 1024, true, "cat_video");
        assert_eq!(
            card.pick_category("cat_audio"),
            DraftCardSignal::CategoryPicked("cat_audio".to_string())
        );
        assert_eq!(card.category(), "cat_audio");
        let button = card.category_button(true, &categories()).unwrap();
        assert_eq!(button.icon, "MUSIC");
        assert_eq!(edit_requested(), DraftCardSignal::EditRequested);
    }

    #[test]
    fn conflict_visible_mirrors_is_not_none() {
        assert!(!conflict_visible::<()>(None));
        let hit = "a.mp4";
        assert!(conflict_visible(Some(&hit)));
    }

    #[test]
    fn menu_anchor_is_the_button_bottom_left() {
        assert_eq!(menu_anchor(100, 40, 28), (100, 68));
    }

    #[test]
    fn multi_file_select_button_needs_more_than_one_file() {
        let card = DraftCardState::new("a.mkv", 0, true, "");
        let single = MultiFileDraftCardState::new(card.clone(), vec![DraftFile::new(0, 10, true)]);
        assert!(!single.has_select_button());
        let empty = MultiFileDraftCardState::new(card.clone(), Vec::new());
        assert!(!empty.has_select_button());
        let multi = MultiFileDraftCardState::new(
            card,
            vec![DraftFile::new(0, 10, true), DraftFile::new(1, 20, false)],
        );
        assert!(multi.has_select_button());
    }

    #[test]
    fn multi_file_summary_uses_the_count_format() {
        let card = DraftCardState::new("a.mkv", 2048, true, "");
        let multi = MultiFileDraftCardState::new(
            card.clone(),
            vec![
                DraftFile::new(0, 1024, true),
                DraftFile::new(1, 1024, false),
                DraftFile::new(2, 0, true),
            ],
        );
        assert_eq!(multi.summary_text(), "2/3 个文件 · 2.00 KB");

        let single = MultiFileDraftCardState::new(card, vec![DraftFile::new(0, 2048, true)]);
        assert_eq!(single.summary_text(), "2.00 KB");

        let none =
            MultiFileDraftCardState::new(DraftCardState::new("a.mkv", 0, true, ""), Vec::new());
        assert_eq!(none.summary_text(), "0.00 B", "<=1 的分支没有 \"--\" 兜底");
    }

    #[test]
    fn apply_selection_recomputes_file_size() {
        let card = DraftCardState::new("a.mkv", 400, true, "");
        let mut multi = MultiFileDraftCardState::new(
            card,
            vec![
                DraftFile::new(0, 100, true),
                DraftFile::new(1, 200, false),
                DraftFile::new(2, 300, true),
            ],
        );
        multi.apply_selection(&[1, 2]);
        assert_eq!(
            multi.files().iter().map(|f| f.selected).collect::<Vec<_>>(),
            [false, true, true]
        );
        assert_eq!(multi.card().file_size(), 500);
        assert_eq!(multi.summary_text(), "2/3 个文件 · 500.00 B");
        // 空清单直接返回，不动 fileSize。
        let mut empty =
            MultiFileDraftCardState::new(DraftCardState::new("a", 7, true, ""), Vec::new());
        empty.apply_selection(&[0]);
        assert_eq!(empty.card().file_size(), 7);
    }
}
