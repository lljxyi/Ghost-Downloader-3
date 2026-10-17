//! 菜单的**摊平视图**（第 59 步）：把 [`card_menu`] 的菜单树变成 Slint 能渲染的行列表。
//!
//! `card_menu.rs` 给的是树（`CardMenuEntry` / 子菜单嵌套），Qt 侧靠
//! `RoundMenu.addMenu(...)` 弹**真弹出窗口**表达层级；Slint 侧没有弹出子窗口的
//! 概念，本模块改成「缩进 + 就地展开」：
//!
//! ```text
//! 任务详情
//! ─────────────
//! 复制下载链接
//! 移到最前
//! 重新下载
//! 移动到分类  ›          ← 子菜单折叠时只有这一行
//!    未分类              ← 点开之后，子项以 depth+1 缩进跟在下面
//!    ─────────────
//!    视频
//! ```
//!
//! **这是刻意偏离**（记录在案）：Qt 里子菜单是独立弹出窗口，鼠标悬停即展开；
//! Slint 里点击展开/收起，展开态是本模块的状态（[`CardMenuView::toggle`]）。
//! 语义上「能点到的动作集合」与 Qt 完全一致——折叠只影响可见性，不影响
//! [`CardMenuView::click`] 能解析出的动作。
//!
//! ## 分层边界
//!
//! - 不持有行为：[`MenuRow`] 只描述「这一行长什么样」，点击解析成
//!   [`CardClick`]（与第 58 步同一原则，真服务调用在装配层）；
//! - 不做文案/图标兜底：树的构造在 `card_menu.rs`，本模块只摊平；
//! - 面板尺寸（[`panel_height`]）按行高常量算，Slint 侧用同样的像素值，
//!   两边靠 [`PANEL_ROW_ACTION_HEIGHT`] 等常量对齐（见 `card_menu.slint`
//!   的注释与 `panel_height_matches_row_kinds` 测试）。

use crate::card_menu::{CardClick, CardMenuEntry};

/// 一行的高度（`QAction` 在 `RoundMenu` 里的实测高度，像素）。
pub const PANEL_ROW_ACTION_HEIGHT: i32 = 30;
/// 分隔符高度（`addSeparator()` 的 1px 线 + 上下留白）。
pub const PANEL_ROW_SEPARATOR_HEIGHT: i32 = 9;
/// 面板上下内边距（单侧）。
pub const PANEL_PADDING: i32 = 4;

/// 一行的类型，数值即 Slint 侧的 `CardMenuRowData.kind`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuRowKind {
    /// `QAction`。
    Action,
    /// `addSeparator()`。
    Separator,
    /// `RoundMenu`（`addMenu`）——折叠/展开的容器行。
    Submenu,
}

impl MenuRowKind {
    /// Slint 侧的整数编码（0 动作 / 1 分隔符 / 2 子菜单）。
    pub fn code(self) -> i32 {
        match self {
            MenuRowKind::Action => 0,
            MenuRowKind::Separator => 1,
            MenuRowKind::Submenu => 2,
        }
    }
}

/// 摊平后的一行。字段与 `ui/card_menu.slint` 的 `CardMenuRowData` 一一对应。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuRow {
    pub kind: MenuRowKind,
    /// 缩进层级：顶层 0，子菜单项 1。
    pub depth: u8,
    /// 动作文案 / 子菜单标题；分隔符为空串。
    pub text: String,
    /// 图标名（`FluentIcon` 成员名）；`None` = 没有图标（`Action(text, parent)`）。
    pub icon: Option<String>,
    /// `setChecked(...)` 的取值；`false` 既表示「未勾选」也表示「不可勾选」，
    /// 区分靠 [`MenuRow::checkable`]。
    pub checked: bool,
    /// `checked.is_some()`：这一行是否有勾选框（只有校验算法项有）。
    pub checkable: bool,
    /// 仅 [`MenuRowKind::Submenu`] 有意义：当前是否展开。
    pub expanded: bool,
    /// 逐级下标：`[4, 2]` = 第 5 项的第二个子项。分隔符/动作/子菜单都用它定位。
    pub path: Vec<usize>,
}

impl MenuRow {
    /// 这一行在 Slint 侧占的高度（像素）。
    pub fn height(&self) -> i32 {
        match self.kind {
            MenuRowKind::Separator => PANEL_ROW_SEPARATOR_HEIGHT,
            _ => PANEL_ROW_ACTION_HEIGHT,
        }
    }
}

/// 面板总高度：上下内边距 + 各行高度之和。
///
/// Slint 里布局无法把「子项高度之和」回传给父元素，所以面板高度由这里算好
/// 推给 `CardMenuPopup.panelHeight`。
pub fn panel_height(rows: &[MenuRow]) -> i32 {
    rows.iter().map(MenuRow::height).sum::<i32>() + 2 * PANEL_PADDING
}

/// 菜单视图：菜单树 + 已展开的子菜单路径集合。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardMenuView {
    entries: Vec<CardMenuEntry>,
    /// 展开的子菜单 `path`（与 [`MenuRow::path`] 同义）。
    expanded: Vec<Vec<usize>>,
}

impl CardMenuView {
    /// 从 `card_menu::context_menu` / `checksum_menu` 的产物建视图（全部折叠）。
    pub fn new(entries: Vec<CardMenuEntry>) -> Self {
        Self {
            entries,
            expanded: Vec::new(),
        }
    }

    /// 原始菜单树（测试与装配层用）。
    pub fn entries(&self) -> &[CardMenuEntry] {
        &self.entries
    }

    /// 摊平后的行列表（顺序即渲染顺序）。
    pub fn rows(&self) -> Vec<MenuRow> {
        let mut out = Vec::with_capacity(self.entries.len() + 4);
        let mut path = Vec::with_capacity(2);
        self.flatten(&self.entries, &mut path, 0, &mut out);
        out
    }

    fn flatten(
        &self,
        entries: &[CardMenuEntry],
        path: &mut Vec<usize>,
        depth: u8,
        out: &mut Vec<MenuRow>,
    ) {
        for (index, entry) in entries.iter().enumerate() {
            path.push(index);
            match entry {
                CardMenuEntry::Separator => out.push(MenuRow {
                    kind: MenuRowKind::Separator,
                    depth,
                    text: String::new(),
                    icon: None,
                    checked: false,
                    checkable: false,
                    expanded: false,
                    path: path.clone(),
                }),
                CardMenuEntry::Action(action) => out.push(MenuRow {
                    kind: MenuRowKind::Action,
                    depth,
                    text: action.text.clone(),
                    icon: action.icon.clone(),
                    checked: action.checked.unwrap_or(false),
                    checkable: action.checked.is_some(),
                    expanded: false,
                    path: path.clone(),
                }),
                CardMenuEntry::Submenu(submenu) => {
                    let expanded = self.expanded.iter().any(|open| open == path);
                    out.push(MenuRow {
                        kind: MenuRowKind::Submenu,
                        depth,
                        text: submenu.text.clone(),
                        icon: submenu.icon.clone(),
                        checked: false,
                        checkable: false,
                        expanded,
                        path: path.clone(),
                    });
                    if expanded {
                        self.flatten(&submenu.items, path, depth + 1, out);
                    }
                }
            }
            path.pop();
        }
    }

    /// 折叠/展开第 `row` 行（仅子菜单行有效）。返回是否真的切换了。
    ///
    /// 摊平后的下标会变，调用方切换后必须重新取 [`CardMenuView::rows`]。
    pub fn toggle(&mut self, row: usize) -> bool {
        let Some(target) = self.rows().into_iter().nth(row) else {
            return false;
        };
        if target.kind != MenuRowKind::Submenu {
            return false;
        }
        match self.expanded.iter().position(|open| *open == target.path) {
            Some(at) => {
                self.expanded.remove(at);
            }
            None => self.expanded.push(target.path),
        }
        true
    }

    /// 第 `row` 行被点击后要执行的动作；分隔符与子菜单行返回 `None`。
    ///
    /// 子菜单行在 Qt 侧「点击 = 展开」（悬停即展开），Slint 侧走
    /// [`CardMenuView::toggle`]，故这里不返回动作。
    pub fn click(&self, row: usize) -> Option<CardClick> {
        let target = self.rows().into_iter().nth(row)?;
        match self.entry_at(&target.path)? {
            CardMenuEntry::Action(action) => Some(action.click.clone()),
            _ => None,
        }
    }

    /// 按 `path` 逐级取菜单项。
    fn entry_at(&self, path: &[usize]) -> Option<&CardMenuEntry> {
        let (first, rest) = path.split_first()?;
        let mut current = self.entries.get(*first)?;
        for index in rest {
            match current {
                CardMenuEntry::Submenu(submenu) => current = submenu.items.get(*index)?,
                _ => return None,
            }
        }
        Some(current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card_menu::{
        ChecksumMenuInput, ContextMenuInput, MENU_COPY_URL, MENU_DETAIL, MENU_MORE,
        MENU_MOVE_TO_CATEGORY, MENU_REDOWNLOAD, MENU_UNCATEGORIZED, MenuCategory, checksum_menu,
        context_menu,
    };
    use gd3_core::task::TaskStatus;

    fn categories() -> Vec<MenuCategory> {
        vec![
            MenuCategory {
                id: "cat_video".into(),
                name: "视频".into(),
                icon: "VIDEO".into(),
            },
            MenuCategory {
                id: "cat_other".into(),
                name: "杂项".into(),
                icon: "NOT_A_REAL_ICON".into(),
            },
        ]
    }

    fn icons() -> Vec<String> {
        vec!["VIDEO".into(), "TAG".into()]
    }

    fn context_view() -> CardMenuView {
        let categories = categories();
        let icons = icons();
        CardMenuView::new(context_menu(&ContextMenuInput {
            can_edit: true,
            status: TaskStatus::Running,
            category_enabled: true,
            categories: &categories,
            known_icons: &icons,
        }))
    }

    fn texts(rows: &[MenuRow]) -> Vec<&str> {
        rows.iter().map(|row| row.text.as_str()).collect()
    }

    #[test]
    fn rows_flatten_collapsed_submenu_into_one_line() {
        let view = context_view();
        let rows = view.rows();
        // 任务详情 / 分隔 / 复制链接 / 编辑 / 移到最前 / 重新下载 / 移动到分类
        assert_eq!(rows.len(), 7);
        assert_eq!(
            texts(&rows),
            vec![
                MENU_DETAIL,
                "",
                MENU_COPY_URL,
                "编辑任务参数...",
                "移到最前",
                MENU_REDOWNLOAD,
                MENU_MOVE_TO_CATEGORY,
            ]
        );
        assert_eq!(rows[1].kind, MenuRowKind::Separator);
        assert_eq!(rows[6].kind, MenuRowKind::Submenu);
        assert_eq!(rows[6].path, vec![6]);
        assert!(!rows[6].expanded);
        assert!(rows.iter().all(|row| row.depth == 0));
        // 右键菜单的动作项从来没有勾选框。
        assert!(rows.iter().all(|row| !row.checkable && !row.checked));
        // 图标名透传（含 `TAG` 兜底已在 card_menu 侧完成）。
        assert_eq!(rows[0].icon.as_deref(), Some("INFO"));
    }

    #[test]
    fn toggle_expands_submenu_with_indented_rows() {
        let mut view = context_view();
        let before = view.rows();
        assert!(view.toggle(6));
        let after = view.rows();
        // 未分类 + 分隔符 + 两个分类 = 4 行新展开。
        assert_eq!(after.len(), before.len() + 4);
        assert!(after[6].expanded);
        assert_eq!(
            texts(&after[7..]),
            vec![MENU_UNCATEGORIZED, "", "视频", "杂项"]
        );
        assert!(after[7..].iter().all(|row| row.depth == 1));
        assert_eq!(after[7].path, vec![6, 0]);
        assert_eq!(after[10].path, vec![6, 3]);
        // 非法分类图标在 card_menu 侧已兜底成 TAG。
        assert_eq!(after[10].icon.as_deref(), Some("TAG"));
        // 再点一次收起。
        assert!(view.toggle(6));
        assert_eq!(view.rows().len(), before.len());
        assert!(!view.rows()[6].expanded);
    }

    #[test]
    fn toggle_ignores_action_and_separator_rows() {
        let mut view = context_view();
        assert!(!view.toggle(0), "动作行不切换");
        assert!(!view.toggle(1), "分隔符不切换");
        assert!(!view.toggle(99), "越界不 panic");
        assert_eq!(view.rows().len(), 7);
    }

    #[test]
    fn click_resolves_actions_including_nested_category() {
        let mut view = context_view();
        assert_eq!(view.click(0), Some(CardClick::OpenDrawer));
        assert_eq!(view.click(1), None, "分隔符没有动作");
        assert_eq!(view.click(2), Some(CardClick::CopyUrl));
        assert_eq!(view.click(3), Some(CardClick::EditTask));
        assert_eq!(view.click(4), Some(CardClick::MoveToFront));
        assert_eq!(view.click(5), Some(CardClick::Redownload));
        assert_eq!(view.click(6), None, "子菜单行只负责展开");
        assert_eq!(view.click(99), None);

        // 折叠状态下子项不可点（不在行列表里）；展开后能解析到分类 id。
        assert!(view.toggle(6));
        assert_eq!(view.click(7), Some(CardClick::SetCategory(String::new())));
        assert_eq!(view.click(8), None);
        assert_eq!(
            view.click(9),
            Some(CardClick::SetCategory("cat_video".into()))
        );
        assert_eq!(
            view.click(10),
            Some(CardClick::SetCategory("cat_other".into()))
        );
    }

    #[test]
    fn click_skips_edit_row_when_not_editable() {
        let categories = categories();
        let icons = icons();
        let view = CardMenuView::new(context_menu(&ContextMenuInput {
            can_edit: false,
            status: TaskStatus::Completed,
            category_enabled: false,
            categories: &categories,
            known_icons: &icons,
        }));
        let rows = view.rows();
        assert_eq!(
            texts(&rows),
            vec![MENU_DETAIL, "", MENU_COPY_URL, "移到最前", MENU_REDOWNLOAD]
        );
        assert_eq!(view.click(3), Some(CardClick::MoveToFront));
    }

    #[test]
    fn checksum_menu_rows_are_checkable_and_nest_into_more() {
        let algorithms: Vec<String> = ["blake2b", "md5", "sha1", "sha256", "sha512"]
            .iter()
            .map(|name| name.to_string())
            .collect();
        let checksums = vec!["sha256".to_string()];
        let view = CardMenuView::new(checksum_menu(&ChecksumMenuInput {
            algorithms: &algorithms,
            checksums: &checksums,
        }));
        let rows = view.rows();
        // md5 / sha1 / sha256 / sha512 / 更多
        assert_eq!(
            texts(&rows),
            vec!["MD5", "SHA1", "SHA256", "SHA512", MENU_MORE]
        );
        // 四个算法行可勾选；`更多` 是子菜单行，没有勾选框。
        assert!(rows[..4].iter().all(|row| row.checkable));
        assert!(!rows[4].checkable);
        assert_eq!(
            rows.iter().filter(|row| row.checked).count(),
            1,
            "只有 sha256 打勾"
        );
        assert!(rows[2].checked);
        assert_eq!(rows[4].kind, MenuRowKind::Submenu);
        // 校验菜单的两参 `Action(text, parent)` 没有图标。
        assert!(rows.iter().all(|row| row.icon.is_none()));

        let mut view = view;
        assert!(view.toggle(4));
        let rows = view.rows();
        assert_eq!(rows.len(), 6);
        assert_eq!(rows[5].text, "BLAKE2B");
        assert_eq!(rows[5].depth, 1);
        assert!(!rows[5].checked);
        assert_eq!(
            view.click(5),
            Some(CardClick::StartChecksum("blake2b".into()))
        );
    }

    #[test]
    fn empty_more_submenu_still_renders_one_line() {
        // `更多` 在 Python 侧无条件 `addMenu`，即使里面一个算法都没有。
        let algorithms: Vec<String> = ["md5"].iter().map(|name| name.to_string()).collect();
        let view = CardMenuView::new(checksum_menu(&ChecksumMenuInput {
            algorithms: &algorithms,
            checksums: &[],
        }));
        let rows = view.rows();
        assert_eq!(texts(&rows), vec!["MD5", MENU_MORE]);
        let mut view = view;
        assert!(view.toggle(1));
        assert_eq!(view.rows().len(), 2, "展开空子菜单不新增行");
    }

    #[test]
    fn panel_height_matches_row_kinds() {
        let view = context_view();
        let collapsed = view.rows();
        assert_eq!(
            panel_height(&collapsed),
            2 * PANEL_PADDING + 6 * PANEL_ROW_ACTION_HEIGHT + PANEL_ROW_SEPARATOR_HEIGHT
        );
        let mut view = view;
        view.toggle(6);
        let expanded = view.rows();
        // 展开后多 3 个动作行 + 1 个分隔符行。
        assert_eq!(
            panel_height(&expanded),
            panel_height(&collapsed) + 3 * PANEL_ROW_ACTION_HEIGHT + PANEL_ROW_SEPARATOR_HEIGHT
        );
    }
}
