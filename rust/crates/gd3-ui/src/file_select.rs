//! 文件选择对话框（P2，第 65 步）：镜像 `app/view/dialogs/file_select.py`
//! 的语义层——勾选树、按类型选择菜单、摘要文案、校验。
//!
//! ## 实证口径（2026-10-17 实读源码 + venv 实跑）
//!
//! - **显示路径**：`FileSelectDialog._fileDisplayPath` = `task.toDisplayPath(file)`
//!   = `file.relativePath`（`models/task.py:227`，HTTP 包里就是相对路径），
//!   建树时按 `"/"` 切分（`file_select.py:92`）。
//! - **文件夹行**：路径前缀去重（`folderItems` 以 tuple 前缀为键），
//!   勾选态初值 **Unchecked**、可勾选、大小列为空串；叶子行勾选态取
//!   `file.selected`、大小列 `toReadableSize(file.size)`。
//! - **建树后统一自底向上重算**（`_updateBranchCheckState`，:180-190）：
//!   全 Checked → Checked；全 Unchecked → Unchecked；否则 PartiallyChecked。
//! - **用户点勾选框**（`_onItemChanged`，:164-172）：只对第 0 列生效；节点
//!   有子节点且新态 **不是** PartiallyChecked 时向下整棵覆盖；随后沿父链
//!   重算（`_updateAncestorCheckStates`，:192-201，注意它**只**看直接子节点
//!   的当前态，不递归）。父链重算里「空子节点列表」按 Python `all([])==True`
//!   落 Checked——该分支实际不可达（无子节点的文件夹不存在），但照抄语义。
//! - **`selectedIndexes()`**（:241-242）只统计 `_fileItems`（= 叶子/文件行）
//!   里 Checked 的 `file.index`，文件夹行不计。
//! - **批量选择**（`_setSelectedIndexes`，:203-212）先逐叶子置态，再对每个
//!   根节点自底向上重算；「全选/全不选/反选/按类型」四个入口都走它。
//!   `_invertSelection` 取的是「当前已选」的补集（:221-224）。
//! - **按类型菜单**（`_buildTypeMenu`，:129-162）：`matchByName(file.relativePath)`
//!   命中才计数（空串=未分类不计数），**按首次出现顺序**出项；每项文案
//!   `仅选{分类名} ({n})`；未分类追加一项 `仅选其他 ({n})`（`categoryId=""`）；
//!   一项都没有时按钮禁用（`setEnabled(bool(actions()))`）。
//! - **摘要文案**（`_updateSummary`，:233-239）：
//!   `已选择 {n}/{total} 个文件，共 {toReadableSize(选中字节和)}`，
//!   `total = len(_fileItems)`、`n = len(selectedIndexes())`。
//! - **校验**（:244-252）：有选中即通过；否则弹 InfoBar
//!   （标题 `至少选择一个文件` / 正文 `当前没有任何文件被勾选`）返回 false。
//! - `matchByName` 的 `Path.suffixes` 口径按 **CPython 3.13**
//!   （`/usr/lib/python3.13/pathlib/_abc.py:182-192`）：
//!   `name` 以 `.` 结尾 → 空列表；否则 `name.lstrip('.')` 后按 `.` 切、
//!   丢掉首段、每段前缀补 `.`。venv 实跑（3.13.5）：
//!   `a.tar.gz→[.tar,.gz]`、`.bashrc→[]`、`a.→[]`、`x.y.z.→[]`、
//!   `a..b→[.,.b]`、`.tar.gz→[.gz]`、`A.B.C→[.B,.C]`。
//! - 交叉验证：`matchByName` 用 venv 跑 12 个文件名，Rust 侧输出逐条一致
//!   （含 `A.B.C`/`a.b-c.d` 落空串）。
//!
//! 与 Qt 的差异：Slint 侧没有 `QStandardItemModel`，本模块用 arena 建同一棵
//! 树并自己算勾选态；`QFileIconProvider` 的图标由视图层给（本层只标
//! `is_folder`）。

use crate::format::to_readable_size;
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// 文案与常量（`file_select.py` 逐字）
// ---------------------------------------------------------------------------

/// `SubtitleLabel(self.tr("选择下载文件"))`（:31）
pub const TITLE: &str = "选择下载文件";
/// `PrimaryPushButton(self.tr("全选"))`（:36）
pub const BUTTON_SELECT_ALL: &str = "全选";
/// `PushButton(self.tr("全不选"))`（:37）
pub const BUTTON_CLEAR: &str = "全不选";
/// `PushButton(self.tr("反选"))`（:38）
pub const BUTTON_INVERT: &str = "反选";
/// `DropDownPushButton(self.tr("按类型选择"))`（:39）
pub const BUTTON_BY_TYPE: &str = "按类型选择";
/// `yesButton.setText(self.tr("应用"))`（:54）
pub const BUTTON_APPLY: &str = "应用";
/// `cancelButton.setText(self.tr("取消"))`（:55）
pub const BUTTON_CANCEL: &str = "取消";
/// `setHorizontalHeaderLabels([tr("文件"), tr("大小")])`（:57）
pub const HEADER_NAME: &str = "文件";
pub const HEADER_SIZE: &str = "大小";
/// `self.widget.setMinimumWidth(720)`（:53）
pub const MIN_WIDTH: i32 = 720;
/// `AutoSizingTreeView(self, minimumVisibleRows=3)`（:33）
pub const MIN_VISIBLE_ROWS: i32 = 3;
/// 未分类项的文案（`self.tr("其他")`，:155）
pub const OTHER_CATEGORY: &str = "其他";
/// `InfoBar.warning(self.tr("至少选择一个文件"), ...)`（:247-251）
pub const VALIDATE_TITLE: &str = "至少选择一个文件";
pub const VALIDATE_TEXT: &str = "当前没有任何文件被勾选";

// ---------------------------------------------------------------------------
// 勾选态
// ---------------------------------------------------------------------------

/// `Qt.CheckState` 的数值口径（Unchecked=0 / PartiallyChecked=1 / Checked=2）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckState {
    Unchecked = 0,
    PartiallyChecked = 1,
    Checked = 2,
}

impl CheckState {
    /// Qt 数值（Slint 侧用 int 传勾选态时按这个口径）。
    pub fn code(self) -> i32 {
        self as i32
    }

    /// `Qt.CheckState.Checked if checked else Qt.CheckState.Unchecked`。
    pub fn from_checked(checked: bool) -> Self {
        if checked {
            CheckState::Checked
        } else {
            CheckState::Unchecked
        }
    }
}

// ---------------------------------------------------------------------------
// 输入：任务文件项
// ---------------------------------------------------------------------------

/// `Task.files` 里视图真正用到的那几列（`file.index` / `file.relativePath`
/// / `file.size` / `file.selected`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectableFile {
    pub index: i64,
    pub relative_path: String,
    pub size: i64,
    pub selected: bool,
}

// ---------------------------------------------------------------------------
// 树
// ---------------------------------------------------------------------------

/// 摊平后的一行（Slint `ListView` 直接吃）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRow {
    /// 行在树里的完整路径（文件夹=前缀，文件=完整相对路径切分）——
    /// 回写勾选态时用它定位（Qt 侧对应 item 指针）。
    pub path: Vec<String>,
    /// 第 0 列文本（`QStandardItem(part)` / `QStandardItem(name)`）。
    pub name: String,
    /// 缩进层级（根 = 0）。
    pub depth: usize,
    /// 文件夹行（图标取 Folder）。
    pub is_folder: bool,
    /// 第 1 列文本：文件夹空串、文件 `toReadableSize(size)`。
    pub size_text: String,
    pub check: CheckState,
    /// 文件行的 `file.index`（文件夹行 None）——`selectedIndexes()` 的来源。
    pub file_index: Option<i64>,
}

struct Node {
    name: String,
    path: Vec<String>,
    is_folder: bool,
    file_index: Option<i64>,
    size: i64,
    check: CheckState,
    parent: Option<usize>,
    children: Vec<usize>,
}

/// 文件选择对话框的勾选树状态机（Qt `QStandardItemModel` 的镜像）。
pub struct FileSelect {
    files: Vec<SelectableFile>,
    nodes: Vec<Node>,
    roots: Vec<usize>,
}

impl FileSelect {
    /// 建树 + 自底向上算勾选态（镜像 `_buildTree` 的 `for i in range(root.rowCount())` 收尾）。
    pub fn new(files: Vec<SelectableFile>) -> Self {
        let mut state = FileSelect {
            files,
            nodes: Vec::new(),
            roots: Vec::new(),
        };
        state.rebuild();
        state
    }

    /// 重建（文件列表变化后调；Qt 侧对应重开对话框）。
    pub fn rebuild(&mut self) {
        self.nodes.clear();
        self.roots.clear();
        // Python 用 dict 以 tuple 前缀为键，插入序即建行序；这里用 BTreeMap
        // 只做查重，行序仍由 `self.nodes` 的 push 顺序决定。
        let mut folders: BTreeMap<Vec<String>, usize> = BTreeMap::new();
        let files = std::mem::take(&mut self.files);
        for file in &files {
            let parts: Vec<String> = file.relative_path.split('/').map(str::to_string).collect();
            let mut parent: Option<usize> = None;
            let mut prefix: Vec<String> = Vec::new();
            let folder_parts = parts.len().saturating_sub(1);
            for part in &parts[..folder_parts] {
                prefix.push(part.clone());
                let key = prefix.clone();
                let node_index = match folders.get(&key) {
                    Some(existing) => *existing,
                    None => {
                        let index = self.nodes.len();
                        self.nodes.push(Node {
                            name: part.clone(),
                            path: key.clone(),
                            is_folder: true,
                            file_index: None,
                            size: 0,
                            check: CheckState::Unchecked,
                            parent,
                            children: Vec::new(),
                        });
                        folders.insert(key, index);
                        match parent {
                            Some(parent) => self.nodes[parent].children.push(index),
                            None => self.roots.push(index),
                        }
                        index
                    }
                };
                parent = Some(node_index);
            }
            // `parts[-1] if parts else displayPath`：`str.split("/")` 永远至少
            // 一段，故等价于取最后一段（空 relativePath → 空名）。
            let name = parts.last().cloned().unwrap_or_default();
            let index = self.nodes.len();
            self.nodes.push(Node {
                name,
                path: parts.clone(),
                is_folder: false,
                file_index: Some(file.index),
                size: file.size,
                check: CheckState::from_checked(file.selected),
                parent,
                children: Vec::new(),
            });
            match parent {
                Some(parent) => self.nodes[parent].children.push(index),
                None => self.roots.push(index),
            }
        }
        self.files = files;
        let roots = self.roots.clone();
        for root in roots {
            self.update_branch_check_state(root);
        }
    }

    /// 输入文件项（保持调用方给的顺序）。
    pub fn files(&self) -> &[SelectableFile] {
        &self.files
    }

    /// 摊平成行（先序遍历，深度递增）——镜像 `treeView.expandAll()` 后的可见序。
    pub fn rows(&self) -> Vec<TreeRow> {
        let mut rows = Vec::with_capacity(self.nodes.len());
        let mut stack: Vec<(usize, usize)> = self.roots.iter().rev().map(|i| (*i, 0)).collect();
        while let Some((index, depth)) = stack.pop() {
            let node = &self.nodes[index];
            rows.push(TreeRow {
                path: node.path.clone(),
                name: node.name.clone(),
                depth,
                is_folder: node.is_folder,
                size_text: if node.is_folder {
                    String::new()
                } else {
                    to_readable_size(node.size as f64)
                },
                check: node.check,
                file_index: node.file_index,
            });
            for child in node.children.iter().rev() {
                stack.push((*child, depth + 1));
            }
        }
        rows
    }

    /// 某一行（按路径定位）的勾选态；找不到 = None。
    pub fn check_of(&self, path: &[String]) -> Option<CheckState> {
        self.find(path).map(|index| self.nodes[index].check)
    }

    fn find(&self, path: &[String]) -> Option<usize> {
        self.nodes
            .iter()
            .position(|node| node.path.as_slice() == path)
    }

    /// `selectedIndexes()`：只有文件行的 Checked 计入（文件夹行不算）。
    pub fn selected_indexes(&self) -> Vec<i64> {
        let mut indexes: Vec<i64> = self
            .nodes
            .iter()
            .filter(|node| node.check == CheckState::Checked)
            .filter_map(|node| node.file_index)
            .collect();
        indexes.sort_unstable();
        indexes.dedup();
        indexes
    }

    /// `_setSelectedIndexes`：逐叶子置态 → 每个根自底向上重算。
    pub fn set_selected_indexes(&mut self, selected: &[i64]) {
        for node in &mut self.nodes {
            if let Some(index) = node.file_index {
                node.check = CheckState::from_checked(selected.contains(&index));
            }
        }
        let roots = self.roots.clone();
        for root in roots {
            self.update_branch_check_state(root);
        }
    }

    /// `_setAll(checked)`。
    pub fn set_all(&mut self, checked: bool) {
        if checked {
            let all: Vec<i64> = self.files.iter().map(|file| file.index).collect();
            self.set_selected_indexes(&all);
        } else {
            self.set_selected_indexes(&[]);
        }
    }

    /// `_invertSelection`：当前已选的补集（顺序按 `task.files`）。
    pub fn invert_selection(&mut self) {
        let current = self.selected_indexes();
        let next: Vec<i64> = self
            .files
            .iter()
            .map(|file| file.index)
            .filter(|index| !current.contains(index))
            .collect();
        self.set_selected_indexes(&next);
    }

    /// `_selectCategory(categoryId)`：`matchByName(relativePath) == categoryId`
    /// 的文件置选中，其余取消（`categoryId == ""` 即「未分类」）。
    pub fn select_category(&mut self, category_id: &str, matcher: impl Fn(&str) -> String) {
        let next: Vec<i64> = self
            .files
            .iter()
            .filter(|file| matcher(&file.relative_path) == category_id)
            .map(|file| file.index)
            .collect();
        self.set_selected_indexes(&next);
    }

    /// `_onItemChanged`：用户点了某行（第 0 列）的勾选框。
    ///
    /// `path` 是行路径（文件夹=前缀、文件=完整路径），`check` 是点击后的新态。
    /// 返回是否命中一行（未命中 = Qt 侧不可能发生，调用方兜底忽略）。
    pub fn set_check(&mut self, path: &[String], check: CheckState) -> bool {
        let Some(index) = self.find(path) else {
            return false;
        };
        self.nodes[index].check = check;
        if !self.nodes[index].children.is_empty() && check != CheckState::PartiallyChecked {
            self.set_children_check_state(index, check);
        }
        self.update_ancestor_check_states(self.nodes[index].parent);
        true
    }

    /// `_setChildrenCheckState`：整棵子树覆盖（递归，含叶子）。
    fn set_children_check_state(&mut self, index: usize, check: CheckState) {
        let children = self.nodes[index].children.clone();
        for child in children {
            self.nodes[child].check = check;
            self.set_children_check_state(child, check);
        }
    }

    /// `_updateBranchCheckState`：自底向上重算，返回该节点算完后的态。
    fn update_branch_check_state(&mut self, index: usize) -> CheckState {
        let children = self.nodes[index].children.clone();
        if children.is_empty() {
            return self.nodes[index].check;
        }
        let mut all_checked = true;
        let mut all_unchecked = true;
        for child in children {
            match self.update_branch_check_state(child) {
                CheckState::Checked => all_unchecked = false,
                CheckState::Unchecked => all_checked = false,
                CheckState::PartiallyChecked => {
                    all_checked = false;
                    all_unchecked = false;
                }
            }
        }
        self.nodes[index].check = if all_checked {
            CheckState::Checked
        } else if all_unchecked {
            CheckState::Unchecked
        } else {
            CheckState::PartiallyChecked
        };
        self.nodes[index].check
    }

    /// `_updateAncestorCheckStates`：沿父链往上重算（不递归子节点）。
    fn update_ancestor_check_states(&mut self, mut parent: Option<usize>) {
        while let Some(index) = parent {
            let children = self.nodes[index].children.clone();
            let mut all_checked = true;
            let mut all_unchecked = true;
            for child in children {
                match self.nodes[child].check {
                    CheckState::Checked => all_unchecked = false,
                    CheckState::Unchecked => all_checked = false,
                    CheckState::PartiallyChecked => {
                        all_checked = false;
                        all_unchecked = false;
                    }
                }
            }
            // Python `all([])` 为 True → 空子节点列表落 Checked（不可达，照抄）。
            self.nodes[index].check = if all_checked {
                CheckState::Checked
            } else if all_unchecked {
                CheckState::Unchecked
            } else {
                CheckState::PartiallyChecked
            };
            parent = self.nodes[index].parent;
        }
    }

    /// 选中字节和（`sum(f.size for f in task.files if f.index in selected)`）。
    pub fn selected_size(&self) -> i64 {
        let selected = self.selected_indexes();
        self.files
            .iter()
            .filter(|file| selected.contains(&file.index))
            .map(|file| file.size)
            .sum()
    }

    /// `_updateSummary` 的文案。
    pub fn summary_text(&self) -> String {
        let selected = self.selected_indexes();
        format!(
            "已选择 {}/{} 个文件，共 {}",
            selected.len(),
            self.files.len(),
            to_readable_size(self.selected_size() as f64)
        )
    }

    /// `validate()`：有选中即通过（否则调用方弹 [`VALIDATE_TITLE`]/[`VALIDATE_TEXT`]）。
    pub fn validate(&self) -> bool {
        !self.selected_indexes().is_empty()
    }
}

// ---------------------------------------------------------------------------
// 按类型选择菜单
// ---------------------------------------------------------------------------

/// `CategoryService.categoryById` 的投影（`extensions` 须是归一化后的
/// 「无点小写」口径，与 `category::parse_rules` 的产物一致）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CategoryInfo {
    pub category_id: String,
    pub name: String,
    pub extensions: Vec<String>,
}

/// 菜单项（`Action(icon, text)`；`category_id == ""` 即「其他」）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeMenuItem {
    pub category_id: String,
    pub label: String,
    pub count: usize,
}

/// 镜像 `Path(name).suffixes`（CPython 3.13 口径，见模块头）。
pub fn path_suffixes(name: &str) -> Vec<String> {
    // `Path.name`：去掉尾部分隔符后取最后一段。
    let trimmed = name.trim_end_matches('/');
    let file_name = match trimmed.rsplit_once('/') {
        Some((_, tail)) => tail,
        None => trimmed,
    };
    if file_name.ends_with('.') {
        return Vec::new();
    }
    let stripped = file_name.trim_start_matches('.');
    let mut segments = stripped.split('.');
    segments.next();
    segments.map(|segment| format!(".{segment}")).collect()
}

/// `matchByName` 的中间量：`[s.lstrip(".").lower() for s in Path(filename).suffixes]`。
pub fn normalized_suffixes(name: &str) -> Vec<String> {
    path_suffixes(name)
        .iter()
        .map(|suffix| suffix.trim_start_matches('.').to_lowercase())
        .collect()
}

/// 镜像 `CategoryService.matchByName`（`services/category_service.py:157-171`）：
/// 候选顺序「末两段拼接」→「末段」，逐个候选遍历**全部分类**（分类内按
/// `extensions` 顺序），命中即返回 categoryId；无后缀或无命中返回空串。
pub fn match_by_name(filename: &str, categories: &[CategoryInfo]) -> String {
    let suffixes = normalized_suffixes(filename);
    if suffixes.is_empty() {
        return String::new();
    }
    let mut candidates: Vec<String> = Vec::new();
    if suffixes.len() >= 2 {
        candidates.push(suffixes[suffixes.len() - 2..].join("."));
    }
    if let Some(last) = suffixes.last() {
        candidates.push(last.clone());
    }
    for candidate in candidates {
        for category in categories {
            if category.extensions.contains(&candidate) {
                return category.category_id.clone();
            }
        }
    }
    String::new()
}

/// 镜像 `_buildTypeMenu`：按首次出现顺序出项，末尾补「其他」。
pub fn type_menu(files: &[SelectableFile], categories: &[CategoryInfo]) -> Vec<TypeMenuItem> {
    let mut order: Vec<String> = Vec::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for file in files {
        let category_id = match_by_name(&file.relative_path, categories);
        if category_id.is_empty() {
            continue;
        }
        if !counts.contains_key(&category_id) {
            order.push(category_id.clone());
        }
        *counts.entry(category_id).or_insert(0) += 1;
    }
    let mut items: Vec<TypeMenuItem> = Vec::new();
    for category_id in order {
        let Some(category) = categories
            .iter()
            .find(|category| category.category_id == category_id)
        else {
            // `categoryById` 返回 None → `continue`（:137-139）
            continue;
        };
        let count = counts.get(&category_id).copied().unwrap_or(0);
        items.push(TypeMenuItem {
            label: format!("仅选{} ({})", category.name, count),
            category_id,
            count,
        });
    }
    let uncategorized = files
        .iter()
        .filter(|file| match_by_name(&file.relative_path, categories).is_empty())
        .count();
    if uncategorized > 0 {
        items.push(TypeMenuItem {
            category_id: String::new(),
            label: format!("仅选{OTHER_CATEGORY} ({uncategorized})"),
            count: uncategorized,
        });
    }
    items
}

/// `setEnabled(bool(menu.actions()))`：一项都没有时按钮禁用。
pub fn type_menu_enabled(items: &[TypeMenuItem]) -> bool {
    !items.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(index: i64, path: &str, size: i64, selected: bool) -> SelectableFile {
        SelectableFile {
            index,
            relative_path: path.into(),
            size,
            selected,
        }
    }

    fn categories() -> Vec<CategoryInfo> {
        vec![
            CategoryInfo {
                category_id: "cat_video".into(),
                name: "视频".into(),
                extensions: vec!["mp4".into(), "mkv".into(), "tar.gz".into()],
            },
            CategoryInfo {
                category_id: "cat_audio".into(),
                name: "音频".into(),
                extensions: vec!["mp3".into()],
            },
        ]
    }

    /// venv 实跑（CPython 3.13.5）的 `Path.suffixes` 逐条锁定。
    #[test]
    fn path_suffixes_matches_cpython_313() {
        let cases: [(&str, Vec<&str>); 12] = [
            ("a.tar.gz", vec![".tar", ".gz"]),
            ("a.MP4", vec![".MP4"]),
            ("noext", vec![]),
            (".bashrc", vec![]),
            ("a.", vec![]),
            ("x.y.z.", vec![]),
            ("a..b", vec![".", ".b"]),
            (".tar.gz", vec![".gz"]),
            ("文件夹/视频.MKV", vec![".MKV"]),
            ("dir/a.b/c.MKV", vec![".MKV"]),
            ("A.B.C", vec![".B", ".C"]),
            ("a.b-c.d", vec![".b-c", ".d"]),
        ];
        for (name, expected) in cases {
            assert_eq!(path_suffixes(name), expected, "suffixes({name})");
        }
    }

    /// `matchByName` 与 venv 实跑逐条一致（含空串分支）。
    #[test]
    fn match_by_name_matches_python() {
        let cats = categories();
        let cases: [(&str, &str); 12] = [
            ("a.tar.gz", "cat_video"),
            ("a.MP4", "cat_video"),
            ("noext", ""),
            (".bashrc", ""),
            ("a.", ""),
            ("x.y.z.", ""),
            ("a..b", ""),
            (".tar.gz", ""),
            ("文件夹/视频.MKV", "cat_video"),
            ("dir/a.b/c.MKV", "cat_video"),
            ("A.B.C", ""),
            ("a.b-c.d", ""),
        ];
        for (name, expected) in cases {
            assert_eq!(match_by_name(name, &cats), expected, "match({name})");
        }
        // 无分类表 → 一律空串。
        assert_eq!(match_by_name("a.mp4", &[]), "");
        // 「末两段拼接」优先于「末段」：a.mp3.mp4 命中 video(mp4) 而非 audio(mp3)。
        assert_eq!(match_by_name("a.mp3.mp4", &cats), "cat_video");
        // 只有两段时拼接优先：a.tar.gz 的 "tar.gz" 命中 video。
        assert_eq!(match_by_name("a.mp3.gz", &cats), "");
    }

    /// 建树：文件夹去重 + 自底向上算态 + 大小列口径。
    #[test]
    fn build_tree_dedupes_folders_and_computes_branch_states() {
        let select = FileSelect::new(vec![
            file(0, "video/a.mp4", 1024, true),
            file(1, "video/b.mp4", 512, true),
            file(2, "doc/c.txt", 2048, false),
            file(3, "root.bin", 0, false),
        ]);
        let rows = select.rows();
        // 先序：video(checked) → a.mp4 → b.mp4 → doc(unchecked) → c.txt → root.bin
        let names: Vec<(&str, usize, bool)> = rows
            .iter()
            .map(|row| (row.name.as_str(), row.depth, row.is_folder))
            .collect();
        assert_eq!(
            names,
            vec![
                ("video", 0, true),
                ("a.mp4", 1, false),
                ("b.mp4", 1, false),
                ("doc", 0, true),
                ("c.txt", 1, false),
                ("root.bin", 0, false),
            ]
        );
        // video 下两个文件都是 selected=True → 自底向上全 Checked。
        assert_eq!(rows[0].check, CheckState::Checked);
        assert_eq!(rows[1].check, CheckState::Checked);
        assert_eq!(rows[3].check, CheckState::Unchecked);
        // 文件夹行大小列空串；文件行 toReadableSize。
        assert_eq!(rows[0].size_text, "");
        assert_eq!(rows[1].size_text, "1.00 KB");
        assert_eq!(rows[4].size_text, "2.00 KB");
        assert_eq!(rows[5].size_text, "0.00 B");
        // 全选的文件夹 → Checked（自底向上）。
        let select = FileSelect::new(vec![
            file(0, "v/a.mp4", 1, true),
            file(1, "v/b.mp4", 1, true),
        ]);
        assert_eq!(select.rows()[0].check, CheckState::Checked);
        // 全不选 → Unchecked。
        let select = FileSelect::new(vec![file(0, "v/a.mp4", 1, false)]);
        assert_eq!(select.rows()[0].check, CheckState::Unchecked);
    }

    /// 无子目录的相对路径落根行；空路径也能建行（`split("/")` 至少一段）。
    #[test]
    fn flat_and_empty_paths_become_root_rows() {
        let select = FileSelect::new(vec![file(0, "", 0, false), file(1, "a.bin", 7, true)]);
        let rows = select.rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "");
        assert_eq!(rows[0].depth, 0);
        assert!(!rows[0].is_folder);
        assert_eq!(rows[1].name, "a.bin");
        assert_eq!(rows[1].check, CheckState::Checked);
        assert_eq!(select.selected_indexes(), vec![1]);
    }

    /// `selectedIndexes()` 只看文件行；摘要口径与 `_updateSummary` 一致。
    #[test]
    fn selected_indexes_and_summary_follow_python() {
        let mut select = FileSelect::new(vec![
            file(0, "v/a.mp4", 1024, true),
            file(1, "v/b.mp4", 1024, false),
            file(2, "c.bin", 1024, true),
        ]);
        assert_eq!(select.selected_indexes(), vec![0, 2]);
        assert_eq!(select.selected_size(), 2048);
        assert_eq!(select.summary_text(), "已选择 2/3 个文件，共 2.00 KB");
        assert!(select.validate());
        select.set_all(false);
        assert_eq!(select.summary_text(), "已选择 0/3 个文件，共 0.00 B");
        assert!(!select.validate());
        select.set_all(true);
        assert_eq!(select.summary_text(), "已选择 3/3 个文件，共 3.00 KB");
    }

    /// 点文件夹勾选框：向下整棵覆盖 + 沿父链重算。
    #[test]
    fn folder_click_cascades_down_and_up() {
        let mut select = FileSelect::new(vec![
            file(0, "top/v/a.mp4", 1, false),
            file(1, "top/v/b.mp4", 1, false),
            file(2, "top/c.bin", 1, false),
        ]);
        // 勾 top → 全选。
        assert!(select.set_check(&["top".into()], CheckState::Checked));
        assert_eq!(select.selected_indexes(), vec![0, 1, 2]);
        assert_eq!(select.check_of(&["top".into()]), Some(CheckState::Checked));
        // 取消 v → top 变部分选中，c.bin 仍选中。
        assert!(select.set_check(&["top".into(), "v".into()], CheckState::Unchecked));
        assert_eq!(select.selected_indexes(), vec![2]);
        assert_eq!(
            select.check_of(&["top".into()]),
            Some(CheckState::PartiallyChecked)
        );
        // 再勾 v → top 回到 Checked。
        assert!(select.set_check(&["top".into(), "v".into()], CheckState::Checked));
        assert_eq!(select.check_of(&["top".into()]), Some(CheckState::Checked));
        // 取消叶子 a.mp4 → v 与 top 都变部分选中。
        assert!(select.set_check(
            &["top".into(), "v".into(), "a.mp4".into()],
            CheckState::Unchecked
        ));
        assert_eq!(
            select.check_of(&["top".into(), "v".into()]),
            Some(CheckState::PartiallyChecked)
        );
        assert_eq!(
            select.check_of(&["top".into()]),
            Some(CheckState::PartiallyChecked)
        );
        // 越界路径：不命中，不改任何状态。
        assert!(!select.set_check(&["nope".into()], CheckState::Checked));
    }

    /// PartiallyChecked 点击不下传（`_onItemChanged` 的守卫）。
    #[test]
    fn partial_check_does_not_cascade() {
        let mut select = FileSelect::new(vec![file(0, "d/a", 1, true), file(1, "d/b", 1, false)]);
        assert_eq!(
            select.check_of(&["d".into()]),
            Some(CheckState::PartiallyChecked)
        );
        assert!(select.set_check(&["d".into()], CheckState::PartiallyChecked));
        assert_eq!(select.selected_indexes(), vec![0]);
    }

    /// `_setSelectedIndexes` 的批量入口：反选/按类型都走它。
    #[test]
    fn invert_and_select_by_category() {
        let mut select = FileSelect::new(vec![
            file(0, "v/a.mp4", 1, true),
            file(1, "v/b.mkv", 1, false),
            file(2, "m/c.mp3", 1, false),
            file(3, "x/d.bin", 1, false),
        ]);
        select.invert_selection();
        assert_eq!(select.selected_indexes(), vec![1, 2, 3]);
        let cats = categories();
        select.select_category("cat_video", |name| match_by_name(name, &cats));
        // mp4 与 mkv 都属 cat_video（分类 extensions 里两者都有）。
        assert_eq!(select.selected_indexes(), vec![0, 1]);
        assert_eq!(select.check_of(&["v".into()]), Some(CheckState::Checked));
        assert_eq!(select.check_of(&["m".into()]), Some(CheckState::Unchecked));
        // 未分类（categoryId=""）。
        select.select_category("", |name| match_by_name(name, &cats));
        assert_eq!(select.selected_indexes(), vec![3]);
    }

    /// 按类型菜单：首次出现顺序 + 计数 + 未分类兜底 + 空表禁用。
    #[test]
    fn type_menu_mirrors_build_type_menu() {
        let cats = categories();
        let files = vec![
            file(0, "m/c.mp3", 1, false),
            file(1, "v/a.mp4", 1, false),
            file(2, "v/b.mkv", 1, false),
            file(3, "x/d.bin", 1, false),
            file(4, "x/e.bin", 1, false),
        ];
        let items = type_menu(&files, &cats);
        let labels: Vec<(&str, usize)> = items
            .iter()
            .map(|item| (item.label.as_str(), item.count))
            .collect();
        // 音频先出现（m/c.mp3），再视频，末尾其他。
        assert_eq!(
            labels,
            vec![
                ("仅选音频 (1)", 1),
                ("仅选视频 (2)", 2),
                ("仅选其他 (2)", 2)
            ]
        );
        assert_eq!(items[0].category_id, "cat_audio");
        assert_eq!(items[2].category_id, "");
        assert!(type_menu_enabled(&items));
        // 全部有分类 → 无「其他」项。
        let items = type_menu(&files[..3], &cats);
        assert_eq!(items.len(), 2);
        // 无文件 → 空表（按钮禁用）。
        let empty: Vec<SelectableFile> = Vec::new();
        assert!(type_menu(&empty, &cats).is_empty());
        assert!(!type_menu_enabled(&type_menu(&empty, &cats)));
        // 分类表里查不到的 categoryId 不会出项（`categoryById` 为 None → continue）。
        let items = type_menu(&files[..1], &[]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "仅选其他 (1)");
    }

    /// 文案常量与 Python 逐字一致。
    #[test]
    fn texts_match_python_literals() {
        assert_eq!(TITLE, "选择下载文件");
        assert_eq!(BUTTON_SELECT_ALL, "全选");
        assert_eq!(BUTTON_CLEAR, "全不选");
        assert_eq!(BUTTON_INVERT, "反选");
        assert_eq!(BUTTON_BY_TYPE, "按类型选择");
        assert_eq!(BUTTON_APPLY, "应用");
        assert_eq!(BUTTON_CANCEL, "取消");
        assert_eq!((HEADER_NAME, HEADER_SIZE), ("文件", "大小"));
        assert_eq!((MIN_WIDTH, MIN_VISIBLE_ROWS), (720, 3));
        assert_eq!(
            (VALIDATE_TITLE, VALIDATE_TEXT),
            ("至少选择一个文件", "当前没有任何文件被勾选")
        );
        assert_eq!(CheckState::Unchecked.code(), 0);
        assert_eq!(CheckState::PartiallyChecked.code(), 1);
        assert_eq!(CheckState::Checked.code(), 2);
    }
}
