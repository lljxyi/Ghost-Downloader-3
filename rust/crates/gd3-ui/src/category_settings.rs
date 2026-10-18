//! 镜像 `app/view/components/category_settings.py`（148 行）。
//!
//! 两块东西：
//!
//! 1. `CategoryRowWidget`——一行分类（图标 / 名称 / 摘要 / 编辑 / 删除），
//!    以及它发出的 `editRequested` / `removeRequested`；
//! 2. `CategoryRulesCard`——可折叠的「下载分类规则」卡，行列表 + 底部
//!    「恢复默认 / 添加分类」按钮容器，`categoriesChanged` 时整体重建。
//!
//! ## 实证结论（逐条对着源码抄的，不是推断）
//!
//! - `toCategoryName` 是 `QCoreApplication.translate("Categories", name)`：
//!   译文表里没有条目就**原样返回**。Rust 侧没有译文层，故恒等；文件头
//!   那 9 个 `N("Categories", ...)` 只做翻译工具提取标记，运行期不参与。
//! - `IconWidget(getattr(FluentIcon, category.icon, FluentIcon.TAG), self)`：
//!   未知图标名兜 `TAG`。注意 `getattr` 接受**任意** FluentIcon 成员名
//!   （`"EDIT"` 也解析得到），不止 `ICON_CHOICES` 那 25 个——本侧只认
//!   `ICON_CHOICES ∪ {"TAG"}`，手改配置写入别的成员名时会退化成 `TAG`
//!   （唯一的已知偏差，见 [`icon_or_tag`]）。
//! - `_toSummary` 与 `category.rs::summary_of` 逐字同口径：空 → 「无扩展名」，
//!   前 4 项 `", "` 连接，超过 4 项 → `"{head} 等 {count} 项"`（`count` 是
//!   **总项数**，不是剩余项数）。
//! - 行布局 `setContentsMargins(48, 8, 24, 8)` + `setSpacing(12)`，图标
//!   `setFixedSize(16, 16)`；按钮容器同一组 margins，先 `addStretch(1)`
//!   再「恢复默认」「添加分类」。
//! - `_reload()` 先删旧行、再按 `categories()` 顺序重建、最后重新
//!   `addWidget(self.buttonContainer)`（按钮容器被**移出再移回**，始终排在
//!   行列表之后），收尾 `card.setContent("已配置 {n} 个分类")`。
//! - `_onEditClicked` 先 `categoryById(id)`，查不到直接 `return`（不发信号、
//!   不开框）；`_onRemoveClicked` 不做任何前置检查，直接 `removeCategory`。

use crate::category::{CategoryRule, ICON_CHOICES};

/// 行内图标 `setFixedSize(16, 16)`。
pub const ROW_ICON_SIZE: i32 = 16;
/// 行布局 `setContentsMargins(48, 8, 24, 8)`（左 48 与卡片的图标列对齐）。
pub const ROW_MARGINS: (i32, i32, i32, i32) = (48, 8, 24, 8);
/// 行布局 `setSpacing(12)`。
pub const ROW_SPACING: i32 = 12;
/// 按钮容器 `setContentsMargins(48, 8, 24, 8)`。
pub const BUTTON_MARGINS: (i32, i32, i32, i32) = (48, 8, 24, 8);
/// `self.tr("编辑")`。
pub const EDIT_TOOLTIP: &str = "编辑";
/// `self.tr("删除")`。
pub const REMOVE_TOOLTIP: &str = "删除";

/// `CollapsibleSettingCard(FluentIcon.TAG, ...)` 的标题。
pub const CARD_TITLE: &str = "下载分类规则";
/// 卡片副标题。
pub const CARD_CONTENT: &str = "根据扩展名自动归类，可为分类指定下载文件夹";
/// `PushButton(FluentIcon.SYNC, ...)`。
pub const RESET_LABEL: &str = "恢复默认";
/// `PushButton(FluentIcon.ADD, ...)`。
pub const ADD_LABEL: &str = "添加分类";

/// `getattr(FluentIcon, icon, FluentIcon.TAG)` 的兜底图标名。
pub const FALLBACK_ICON: &str = "TAG";

/// `toCategoryName(category)`——`translate("Categories", name)` 的无译文层
/// 等价物：原样返回。
///
/// 保留成函数是为了让调用点与 Python 一一对应（`task_cards.py` /
/// `task_page.py` / `draft_cards.py` 都从 `category_settings` 导它）。
pub fn to_category_name(name: &str) -> &str {
    name
}

/// `getattr(FluentIcon, icon, FluentIcon.TAG)`：认识的名字原样给，其余兜
/// `TAG`。
///
/// 认识集合 = [`ICON_CHOICES`]（`CategoryEditDialog` 下拉的全部可选项）
/// ∪ `{"TAG"}`。Python 的 `getattr` 还认 `EDIT`/`DELETE` 之类的非选项成员，
/// 那些只有手改 `UserConfig.json` 才可能出现，本侧统一退化成 `TAG`。
pub fn icon_or_tag(icon: &str) -> &str {
    if icon == FALLBACK_ICON || ICON_CHOICES.contains(&icon) {
        icon
    } else {
        FALLBACK_ICON
    }
}

/// `CategoryRowWidget` 的数据面（一行分类）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CategoryRow {
    /// `category.categoryId`（编辑/删除信号里回传的原值）。
    pub category_id: String,
    /// 已过 [`icon_or_tag`] 的图标名。
    pub icon: String,
    /// 已过 [`to_category_name`] 的显示名。
    pub name: String,
    /// [`crate::category::summary_of`] 的扩展名摘要。
    pub summary: String,
}

impl CategoryRow {
    /// `CategoryRowWidget.__init__`：`_categoryId` / 图标 / `toCategoryName` /
    /// `_toSummary` 四件套。
    pub fn from_rule(rule: &CategoryRule) -> Self {
        Self {
            category_id: rule.category_id.clone(),
            icon: icon_or_tag(&rule.icon).to_string(),
            name: to_category_name(&rule.name).to_string(),
            summary: crate::category::summary_of(&rule.extensions),
        }
    }
}

/// `CategoryRulesCard` 的数据面：行列表 + 卡片副标题。
///
/// Qt 侧的行 widget / 按钮容器都是子控件；Slint 侧用 model 表达，
/// 故这里只留「重建后应该长什么样」。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CategoryRulesCard {
    rows: Vec<CategoryRow>,
}

impl CategoryRulesCard {
    /// `__init__` 收尾的 `_reload()`。
    pub fn new(rules: &[CategoryRule]) -> Self {
        Self {
            rows: rules.iter().map(CategoryRow::from_rule).collect(),
        }
    }

    /// `_reload()`：整表重建（顺序跟 `categories()` 一致，不排序、不去重）。
    pub fn reload(&mut self, rules: &[CategoryRule]) {
        self.rows = rules.iter().map(CategoryRow::from_rule).collect();
    }

    /// 当前行（顺序 = `categories()` 顺序）。
    pub fn rows(&self) -> &[CategoryRow] {
        &self.rows
    }

    /// `card.setContent(self.tr("已配置 {0} 个分类").format(len(rows)))`。
    pub fn content(&self) -> String {
        format!("已配置 {} 个分类", self.rows.len())
    }
}

/// 四个按钮的语义落点（`_onAddClicked` / `_onEditClicked` /
/// `_onRemoveClicked` / `_onResetClicked`）。
///
/// Qt 侧四个槽直接调 `categoryService`；Rust 侧把「用户点了哪个按钮」
/// 变成值返回，服务调用归调用方（与 `SettingsActions` 同一分工）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CategoryRequest {
    /// `_onAddClicked`：`CategoryEditDialog(window())` 通过后 `addCategory`。
    Add,
    /// `_onEditClicked`：带 `categoryId` 回 `categoryById` 再开框。
    Edit(String),
    /// `_onRemoveClicked`：`removeCategory(categoryId)`。
    Remove(String),
    /// `_onResetClicked`：`reset()`。
    Reset,
}

/// `_onEditClicked` 的前半段：`categoryById(categoryId)` 查不到就
/// `return`（返回 `None`，调用方据此不开框）。
///
/// 注意 `_onRemoveClicked` **没有**这层守卫——删不存在的 id 直接交给
/// 服务（服务自身幂等），故这里不给 `remove_request` 做同样的事。
pub fn edit_request(rules: &[CategoryRule], category_id: &str) -> Option<CategoryRequest> {
    rules
        .iter()
        .any(|rule| rule.category_id == category_id)
        .then(|| CategoryRequest::Edit(category_id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::category::parse_rules;

    fn rule(id: &str, icon: &str, name: &str, extensions: &[&str]) -> CategoryRule {
        CategoryRule {
            category_id: id.into(),
            name: name.into(),
            icon: icon.into(),
            extensions: extensions.iter().map(|ext| ext.to_string()).collect(),
            folder: None,
        }
    }

    #[test]
    fn constants_mirror_python_literals() {
        assert_eq!(ROW_ICON_SIZE, 16);
        assert_eq!(ROW_MARGINS, (48, 8, 24, 8));
        assert_eq!(ROW_SPACING, 12);
        assert_eq!(BUTTON_MARGINS, (48, 8, 24, 8));
        assert_eq!(EDIT_TOOLTIP, "编辑");
        assert_eq!(REMOVE_TOOLTIP, "删除");
        assert_eq!(CARD_TITLE, "下载分类规则");
        assert_eq!(CARD_CONTENT, "根据扩展名自动归类，可为分类指定下载文件夹");
        assert_eq!(RESET_LABEL, "恢复默认");
        assert_eq!(ADD_LABEL, "添加分类");
        assert_eq!(FALLBACK_ICON, "TAG");
    }

    /// `translate("Categories", name)` 无译文层 → 恒等（含九个 N() 标记词）。
    #[test]
    fn to_category_name_is_identity_without_translation_layer() {
        for name in [
            "视频",
            "音频",
            "图片",
            "字幕",
            "文档",
            "压缩包",
            "程序",
            "其他",
            "未命名分类",
            "自定义",
            "",
        ] {
            assert_eq!(to_category_name(name), name);
        }
    }

    #[test]
    fn icon_or_tag_keeps_known_names_and_falls_back_to_tag() {
        // 下拉里的 25 个全部认识。
        for choice in ICON_CHOICES {
            assert_eq!(icon_or_tag(choice), choice, "已知图标 {choice} 不该被替换");
        }
        // TAG 本身不在 ICON_CHOICES 里，但必须原样保留。
        assert_eq!(icon_or_tag("TAG"), "TAG");
        // 未知 / 空串 / 大小写不符 → TAG（`getattr` 也是大小写敏感）。
        assert_eq!(icon_or_tag("NOT_AN_ICON"), "TAG");
        assert_eq!(icon_or_tag(""), "TAG");
        assert_eq!(icon_or_tag("video"), "TAG");
        // 已知偏差：Python `getattr` 能解析非选项成员名，本侧退化。
        assert_eq!(icon_or_tag("EDIT"), "TAG");
    }

    #[test]
    fn from_rule_maps_icon_name_and_summary() {
        let row = CategoryRow::from_rule(&rule("cat_x", "VIDEO", "视频", &["mp4", "mkv"]));
        assert_eq!(row.category_id, "cat_x");
        assert_eq!(row.icon, "VIDEO");
        assert_eq!(row.name, "视频");
        assert_eq!(row.summary, "mp4, mkv");
    }

    /// `_toSummary` 三档：0 项 / ≤4 项 / >4 项（`count` 是总项数）。
    #[test]
    fn summary_three_branches() {
        assert_eq!(
            CategoryRow::from_rule(&rule("c", "TAG", "n", &[])).summary,
            "无扩展名"
        );
        assert_eq!(
            CategoryRow::from_rule(&rule("c", "TAG", "n", &["a", "b", "c", "d"])).summary,
            "a, b, c, d"
        );
        assert_eq!(
            CategoryRow::from_rule(&rule("c", "TAG", "n", &["a", "b", "c", "d", "e"])).summary,
            "a, b, c, d 等 5 项"
        );
        // 未知图标在行里也兜 TAG。
        assert_eq!(
            CategoryRow::from_rule(&rule("c", "NOPE", "n", &[])).icon,
            "TAG"
        );
    }

    #[test]
    fn card_content_counts_rows() {
        assert_eq!(CategoryRulesCard::new(&[]).content(), "已配置 0 个分类");
        let rules = parse_rules(None);
        assert_eq!(rules.len(), 8, "空配置 → 八预设");
        assert_eq!(CategoryRulesCard::new(&rules).content(), "已配置 8 个分类");
    }

    #[test]
    fn new_preserves_category_order_and_ids() {
        let rules = parse_rules(None);
        let card = CategoryRulesCard::new(&rules);
        assert_eq!(card.rows().len(), rules.len());
        for (row, rule) in card.rows().iter().zip(rules.iter()) {
            assert_eq!(row.category_id, rule.category_id);
            assert_eq!(row.name, rule.name);
            assert_eq!(row.icon, rule.icon);
        }
        assert_eq!(card.rows()[0].name, "视频");
        assert_eq!(card.rows()[0].summary, "mp4, mkv, avi, mov 等 20 项");
    }

    /// `_reload()` 整表替换，不追加、不保留旧行。
    #[test]
    fn reload_replaces_rows_wholesale() {
        let mut card = CategoryRulesCard::new(&parse_rules(None));
        assert_eq!(card.rows().len(), 8);
        card.reload(&[rule("only", "PHOTO", "图片", &["png"])]);
        assert_eq!(card.rows().len(), 1);
        assert_eq!(card.rows()[0].category_id, "only");
        assert_eq!(card.content(), "已配置 1 个分类");
        card.reload(&[]);
        assert!(card.rows().is_empty());
        assert_eq!(card.content(), "已配置 0 个分类");
    }

    /// `_onEditClicked` 的 `categoryById` 守卫：查不到就什么都不做。
    #[test]
    fn edit_request_requires_existing_id() {
        let rules = parse_rules(None);
        assert_eq!(
            edit_request(&rules, "cat_video"),
            Some(CategoryRequest::Edit("cat_video".into()))
        );
        assert_eq!(edit_request(&rules, "nope"), None);
        assert_eq!(edit_request(&[], "cat_video"), None);
    }

    /// 四个请求值的语义（`_onRemoveClicked` 无守卫，直接构造）。
    #[test]
    fn request_values_carry_payload_verbatim() {
        assert_eq!(CategoryRequest::Add, CategoryRequest::Add);
        assert_eq!(CategoryRequest::Reset, CategoryRequest::Reset);
        assert_eq!(
            CategoryRequest::Remove("cat_x".into()),
            CategoryRequest::Remove("cat_x".to_string())
        );
        // 空 id 也原样携带（服务侧决定怎么处理）。
        assert_eq!(
            CategoryRequest::Edit(String::new()),
            CategoryRequest::Edit("".into())
        );
    }
}
