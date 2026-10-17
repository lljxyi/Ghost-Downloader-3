//! 任务卡片的**交互层**（第 58 步）：右键菜单、校验菜单、拖拽资格、松开手势。
//!
//! 第 50 步的 `card.rs` 摊平了 `refresh()` 那条「按状态分发」的表；本模块摊平
//! `task_cards.py` 里剩下的、**只在鼠标与菜单里发生**的判定：
//!
//! - `createContextMenu()`：`任务详情` → 分隔符 → `复制下载链接` →
//!   [`can_edit and status != COMPLETED`] `编辑任务参数...` → `移到最前` →
//!   `重新下载` → [`cfg.isCategoryEnabled.value`] 子菜单 `移动到分类`
//!   （`未分类` → 分隔符 → 每个分类）；
//! - `_onChecksumClicked()`：三条短路（校验进行中 → 取消；目录或文件不存在 →
//!   状态行「文件不存在，无法校验」；否则弹菜单）；
//! - `_canDrag()`：`COMPLETED and hasOutputFile and isExisting(outputPath)`；
//! - `mouseReleaseEvent()`：名字区 → 打开文件、信息区 → 抽屉、其余 → 选择；
//! - `mouseDoubleClickEvent()`：左键 → 打开文件。
//!
//! ## 分层边界
//!
//! - **图标名不硬编码**：`getattr(FluentIcon, category.icon, FluentIcon.TAG)`
//!   需要一个「成员名表」，`FluentIcon` 是 `qfluentwidgets` 的枚举，表由调用方
//!   （`fluent_icons.json`）给，本模块只做查表与 `TAG` 兜底；
//! - **算法名表不硬编码**：`ALGORITHMS = sorted(hashlib.algorithms_available)`
//!   是平台相关的（OpenSSL 版本决定），由调用方给；只有 `COMMON_ALGORITHMS`
//!   是仓库里的常量（`app/checksum.py`）；
//! - **不持有行为**：菜单项只描述「点了要做什么」（[`CardClick`]），
//!   真调用留给装配层——与第 57 步 `BarButtonData.index` 同一原则；
//! - **文件系统事实由调用方给**：`isExisting` / `isFolder` 是 IO，
//!   `output_exists` / `is_folder` 都是入参。
//!
//! ## 两处实证（写进金文件生成脚本的坑）
//!
//! 1. `qfluentwidgets.Action` 是 `singledispatchmethod`，按**第一个位置参数**
//!    的类型分派：`Action(text: str, parent)` 与 `Action(icon, text, parent)`
//!    两种。右键菜单走三参（带图标），校验菜单走两参
//!    （`Action(algorithm.upper(), self)`）——**没有图标**，文本是算法名大写。
//! 2. `_onEditClicked()` 先 `featureService.createEditDialog(...).exec()`，
//!    再 `refresh()`（后者会打 `taskService.checksumProgress`）。所以「点一项
//!    菜单会造成什么」是**调用序列**，不是单个调用。

use gd3_core::task::TaskStatus;

/// 菜单文案（`self.tr(...)` 的原文；本仓库装了中文 translator）。
pub const MENU_DETAIL: &str = "任务详情";
pub const MENU_COPY_URL: &str = "复制下载链接";
pub const MENU_EDIT: &str = "编辑任务参数...";
pub const MENU_MOVE_TO_FRONT: &str = "移到最前";
pub const MENU_REDOWNLOAD: &str = "重新下载";
pub const MENU_MOVE_TO_CATEGORY: &str = "移动到分类";
pub const MENU_UNCATEGORIZED: &str = "未分类";
pub const MENU_MORE: &str = "更多";

/// `_onChecksumClicked` 的第二条短路文案。
pub const STATUS_FILE_MISSING: &str = "文件不存在，无法校验";

/// `app/checksum.py` 的 `COMMON_ALGORITHMS`（直接列在菜单顶层的算法）。
pub const COMMON_ALGORITHMS: [&str; 4] = ["md5", "sha1", "sha256", "sha512"];

/// 右键菜单里的图标（`FluentIcon` 成员名）。
pub const ICON_DETAIL: &str = "INFO";
pub const ICON_COPY_URL: &str = "COPY";
pub const ICON_EDIT: &str = "EDIT";
pub const ICON_MOVE_TO_FRONT: &str = "UP";
pub const ICON_REDOWNLOAD: &str = "UPDATE";
pub const ICON_TAG: &str = "TAG";
pub const ICON_UNCATEGORIZED: &str = "MORE";

/// 菜单项被点击后要执行的动作。
///
/// 只描述意图：参数里的 id / 算法名由派生层填，真正的服务调用在装配层。
/// 每个变体都对应 Python 侧一条 `triggered.connect(...)` 的 lambda 体。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardClick {
    /// `self._openDrawer()` → `TaskDrawer(task, …).show()`。
    OpenDrawer,
    /// `QApplication.instance().clipboardListener.setUrls([task.url])`。
    CopyUrl,
    /// `featureService.createEditDialog(task, window).exec()` + `refresh()`。
    EditTask,
    /// `taskService.moveToFront([task.taskId])`。
    MoveToFront,
    /// `taskService.redownload(task)`。
    Redownload,
    /// `taskService.setCategory(task, categoryId)`；空串 = 未分类。
    SetCategory(String),
    /// `taskService.startChecksum(task, algorithm)`。
    StartChecksum(String),
}

impl CardClick {
    /// 触发后第一个打到的服务方法（`(sink, call)`）。
    ///
    /// `sink` 与金文件里的 `calls[0].sink` 同义：`task` / `feature` /
    /// `clipboard` / `drawer`。只用于对齐测试与装配层分派。
    pub fn dispatch(&self) -> (&'static str, &'static str) {
        match self {
            CardClick::OpenDrawer => ("drawer", "TaskDrawer"),
            CardClick::CopyUrl => ("clipboard", "setUrls"),
            CardClick::EditTask => ("feature", "createEditDialog"),
            CardClick::MoveToFront => ("task", "moveToFront"),
            CardClick::Redownload => ("task", "redownload"),
            CardClick::SetCategory(_) => ("task", "setCategory"),
            CardClick::StartChecksum(_) => ("task", "startChecksum"),
        }
    }
}

/// 菜单里的一条动作（`QAction`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardMenuAction {
    /// `QAction.text()`。校验菜单里是 `algorithm.upper()`。
    pub text: String,
    /// `QAction` 的图标名；`None` = 两参重载 `Action(text, parent)`，无图标。
    pub icon: Option<String>,
    /// `setChecked(...)` 的取值；`None` = 从没调过 `setCheckable`/`setChecked`。
    pub checked: Option<bool>,
    pub click: CardClick,
}

impl CardMenuAction {
    fn new(text: impl Into<String>, icon: impl Into<String>, click: CardClick) -> Self {
        CardMenuAction {
            text: text.into(),
            icon: Some(icon.into()),
            checked: None,
            click,
        }
    }
}

/// 子菜单（`RoundMenu(title, parent)` + `setIcon(...)`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardMenuSubmenu {
    pub text: String,
    pub icon: Option<String>,
    pub items: Vec<CardMenuEntry>,
}

/// 菜单里的一项：动作 / 分隔符 / 子菜单。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardMenuEntry {
    Separator,
    Action(CardMenuAction),
    Submenu(CardMenuSubmenu),
}

impl CardMenuEntry {
    /// 动作项（测试与装配层用；`Separator`/`Submenu` 返回 `None`）。
    pub fn action(&self) -> Option<&CardMenuAction> {
        match self {
            CardMenuEntry::Action(action) => Some(action),
            _ => None,
        }
    }
}

/// 右键菜单里「移动到分类」子菜单需要的一个分类。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuCategory {
    /// `Category.categoryId`（`setCategory` 的第二个参数）。
    pub id: String,
    /// 已翻译的显示名（`toCategoryName` = `translate("Categories", name)`）。
    pub name: String,
    /// `Category.icon`（`FluentIcon` 成员名，可能是用户自填的非法名）。
    pub icon: String,
}

/// `createContextMenu()` 的输入。
#[derive(Clone, Debug)]
pub struct ContextMenuInput<'a> {
    /// `Task.canEdit`（类属性）。
    pub can_edit: bool,
    /// `Task.status`。
    pub status: TaskStatus,
    /// `cfg.isCategoryEnabled.value`。
    pub category_enabled: bool,
    pub categories: &'a [MenuCategory],
    /// `FluentIcon` 的成员名表，用于 `getattr(FluentIcon, icon, FluentIcon.TAG)`。
    pub known_icons: &'a [String],
}

/// `getattr(FluentIcon, name, FluentIcon.TAG)`：不在表里就退回 `TAG`。
pub fn category_menu_icon<'a>(known_icons: &'a [String], icon: &'a str) -> &'a str {
    if known_icons.iter().any(|known| known == icon) {
        icon
    } else {
        ICON_TAG
    }
}

/// 镜像 `TaskCard.createContextMenu()`（顺序即 Python 的 `addAction` 顺序）。
pub fn context_menu(input: &ContextMenuInput<'_>) -> Vec<CardMenuEntry> {
    let mut menu = Vec::with_capacity(8);
    menu.push(CardMenuEntry::Action(CardMenuAction::new(
        MENU_DETAIL,
        ICON_DETAIL,
        CardClick::OpenDrawer,
    )));
    menu.push(CardMenuEntry::Separator);
    menu.push(CardMenuEntry::Action(CardMenuAction::new(
        MENU_COPY_URL,
        ICON_COPY_URL,
        CardClick::CopyUrl,
    )));
    // `if self._task.canEdit and self._task.status != TaskStatus.COMPLETED`
    if input.can_edit && input.status != TaskStatus::Completed {
        menu.push(CardMenuEntry::Action(CardMenuAction::new(
            MENU_EDIT,
            ICON_EDIT,
            CardClick::EditTask,
        )));
    }
    menu.push(CardMenuEntry::Action(CardMenuAction::new(
        MENU_MOVE_TO_FRONT,
        ICON_MOVE_TO_FRONT,
        CardClick::MoveToFront,
    )));
    menu.push(CardMenuEntry::Action(CardMenuAction::new(
        MENU_REDOWNLOAD,
        ICON_REDOWNLOAD,
        CardClick::Redownload,
    )));

    if input.category_enabled {
        let mut items = Vec::with_capacity(input.categories.len() + 2);
        items.push(CardMenuEntry::Action(CardMenuAction::new(
            MENU_UNCATEGORIZED,
            ICON_UNCATEGORIZED,
            CardClick::SetCategory(String::new()),
        )));
        items.push(CardMenuEntry::Separator);
        for category in input.categories {
            let icon = category_menu_icon(input.known_icons, &category.icon);
            items.push(CardMenuEntry::Action(CardMenuAction::new(
                category.name.clone(),
                icon,
                CardClick::SetCategory(category.id.clone()),
            )));
        }
        menu.push(CardMenuEntry::Submenu(CardMenuSubmenu {
            text: MENU_MOVE_TO_CATEGORY.to_string(),
            icon: Some(ICON_TAG.to_string()),
            items,
        }));
    }
    menu
}

/// `_onChecksumClicked()` 弹的菜单的输入。
#[derive(Clone, Debug)]
pub struct ChecksumMenuInput<'a> {
    /// `ALGORITHMS = sorted(hashlib.algorithms_available)`（平台相关，调用方给）。
    pub algorithms: &'a [String],
    /// `task.checksums`（`{algorithm: value}` 的键集）。
    pub checksums: &'a [String],
}

/// 镜像 `_onChecksumClicked()` 里那个菜单的构造。
///
/// 顶层是 `COMMON_ALGORITHMS` 里的算法，其余全进 `更多` 子菜单；`更多`
/// **无条件** `addMenu`（Python 没有判空）。
pub fn checksum_menu(input: &ChecksumMenuInput<'_>) -> Vec<CardMenuEntry> {
    let mut common = Vec::new();
    let mut more = Vec::new();
    for algorithm in input.algorithms {
        let action = CardMenuAction {
            // `Action(algorithm.upper(), self)`：两参重载，**没有图标**。
            text: algorithm.to_uppercase(),
            icon: None,
            checked: Some(input.checksums.iter().any(|known| known == algorithm)),
            click: CardClick::StartChecksum(algorithm.clone()),
        };
        let entry = CardMenuEntry::Action(action);
        if COMMON_ALGORITHMS.contains(&algorithm.as_str()) {
            common.push(entry);
        } else {
            more.push(entry);
        }
    }
    common.push(CardMenuEntry::Submenu(CardMenuSubmenu {
        text: MENU_MORE.to_string(),
        icon: None,
        items: more,
    }));
    common
}

/// `_onChecksumClicked()` 的三种结局。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChecksumOutcome {
    /// 校验进行中 → `taskService.cancelChecksum(task)`。
    CancelChecksum,
    /// 目录或文件不存在 → `_setStatus(...)`。
    SetStatus(&'static str),
    /// 否则弹菜单。
    ShowMenu(Vec<CardMenuEntry>),
}

/// 镜像 `_onChecksumClicked()` 的三条短路。
pub fn checksum_click(
    in_progress: bool,
    is_folder: bool,
    output_exists: bool,
    menu: impl FnOnce() -> Vec<CardMenuEntry>,
) -> ChecksumOutcome {
    if in_progress {
        return ChecksumOutcome::CancelChecksum;
    }
    // `if isFolder(outputPath) or not isExisting(outputPath)`
    if is_folder || !output_exists {
        return ChecksumOutcome::SetStatus(STATUS_FILE_MISSING);
    }
    ChecksumOutcome::ShowMenu(menu())
}

/// 镜像 `_canDrag()`：只有「已完成 + 有输出文件 + 文件还在」能拖。
pub fn can_drag(status: TaskStatus, has_output_file: bool, output_exists: bool) -> bool {
    status == TaskStatus::Completed && has_output_file && output_exists
}

/// `mouseReleaseEvent()` 的分派结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseGesture {
    /// `button != Left` 或 `_hasDragged`：什么都不做。
    Ignore,
    /// 名字区（且没按 Shift）→ `openFile(task.outputPath)`。
    OpenFile,
    /// 信息区（且没按 Shift）→ `_openDrawer()`。
    OpenDrawer,
    /// 其余 → `selectionChanged.emit(checked, extend)`。
    Select { checked: bool, extend: bool },
}

/// `mouseReleaseEvent()` 的输入。
#[derive(Clone, Copy, Debug)]
pub struct ReleaseInput {
    /// `e.button() == Qt.MouseButton.LeftButton`。
    pub left_button: bool,
    /// `self._hasDragged`。
    pub has_dragged: bool,
    /// `self._matchNameText(pos)`。
    pub match_name: bool,
    /// `self._matchInfoText(pos)`。
    pub match_info: bool,
    /// `e.modifiers() & Qt.KeyboardModifier.ShiftModifier`。
    pub extend: bool,
    /// `self._isSelectionMode`。
    pub selection_mode: bool,
    /// `self.isChecked()`。
    pub is_checked: bool,
}

/// 镜像 `TaskCard.mouseReleaseEvent()` 的分派链。
///
/// `checked = True if extend or not self._isSelectionMode else not self.isChecked()`
/// ——`extend` 时恒为真、非选择模式恒为真，只有「选择模式 + 没按 Shift」才是取反。
pub fn release_gesture(input: &ReleaseInput) -> ReleaseGesture {
    if !input.left_button || input.has_dragged {
        return ReleaseGesture::Ignore;
    }
    if !input.extend && input.match_name {
        return ReleaseGesture::OpenFile;
    }
    if !input.extend && input.match_info {
        return ReleaseGesture::OpenDrawer;
    }
    ReleaseGesture::Select {
        checked: input.extend || !input.selection_mode || !input.is_checked,
        extend: input.extend,
    }
}

/// 镜像 `mouseDoubleClickEvent()`：左键双击 = 打开文件。
pub fn double_click_opens_file(left_button: bool) -> bool {
    left_button
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const FIXTURE: &str = include_str!("../tests/fixtures/card_menu.jsonl");
    const ICONS: &str = include_str!("../tests/fixtures/fluent_icons.json");

    /// 生成脚本 `_phantom_task` 里固定的任务 id / URL / 输出路径（全篇一致）。
    ///
    /// 这三条是**输入**而不是被测逻辑：金文件每条 case 只描述「什么状态 +
    /// 点了什么」，卡片身上的 id 与路径由脚本固定。若生成脚本改了它们，
    /// `assert_menu` 的 `args` 比对会立刻红，不会静默漂移。
    const TASK_ID: &str = "tsk_1";
    const TASK_URL: &str = "https://example/a.bin";
    /// `outputFolder=Path("/tmp")` + `name="a.bin"` → `Task.outputPath`。
    const OUTPUT_PATH: &str = "/tmp/a.bin";

    fn cases() -> Vec<Value> {
        FIXTURE
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("金文件行不是 JSON"))
            .collect()
    }

    fn known_icons() -> Vec<String> {
        serde_json::from_str(ICONS).expect("图标表不是 JSON 数组")
    }

    fn status_of(case: &Value) -> TaskStatus {
        match case["status"].as_str().expect("status 缺失") {
            "WAITING" => TaskStatus::Waiting,
            "RUNNING" => TaskStatus::Running,
            "PAUSED" => TaskStatus::Paused,
            "COMPLETED" => TaskStatus::Completed,
            "FAILED" => TaskStatus::Failed,
            other => panic!("未知状态：{other}"),
        }
    }

    fn strings(value: &Value) -> Vec<String> {
        value
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| item.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn text(value: &str) -> Value {
        Value::from(value.to_string())
    }

    /// 一项菜单动作被点击后，`taskService` / `featureService` / 剪贴板 / 抽屉
    /// 收到的第一个调用应该长什么样——由 [`CardClick`] 反推。
    fn expected_args(click: &CardClick) -> Vec<Value> {
        match click {
            CardClick::OpenDrawer => vec![text(TASK_ID)],
            // `setUrls([task.url])`：一个 URL 的列表。
            CardClick::CopyUrl => vec![Value::Array(vec![text(TASK_URL)])],
            // `createEditDialog(task, self.window())`：第二个参数是父窗口。
            CardClick::EditTask => vec![text(TASK_ID), text("window")],
            // `moveToFront([task.taskId])`：id 的列表。
            CardClick::MoveToFront => vec![Value::Array(vec![text(TASK_ID)])],
            CardClick::Redownload => vec![text(TASK_ID)],
            CardClick::SetCategory(category_id) => vec![text(TASK_ID), text(category_id)],
            CardClick::StartChecksum(algorithm) => vec![text(TASK_ID), text(algorithm)],
        }
    }

    /// 逐项比对菜单树：项数、类型、文案、图标、`checked`，以及动作的调用。
    fn assert_menu(entries: &[CardMenuEntry], expected: &Value, path: &str) {
        let expected = expected
            .as_array()
            .unwrap_or_else(|| panic!("{path}: 金文件不是数组"));
        assert_eq!(entries.len(), expected.len(), "{path}: 项数不同");
        for (index, (entry, want)) in entries.iter().zip(expected).enumerate() {
            let at = format!("{path}[{index}]");
            match want["type"].as_str().expect("type 缺失") {
                "separator" => assert!(
                    matches!(entry, CardMenuEntry::Separator),
                    "{at}: 应为分隔符，实为 {entry:?}"
                ),
                "action" => {
                    let action = entry
                        .action()
                        .unwrap_or_else(|| panic!("{at}: 应为动作，实为 {entry:?}"));
                    assert_eq!(
                        action.text,
                        want["text"].as_str().expect("text 缺失"),
                        "{at}: 文本"
                    );
                    // `icon` 为 null = 两参重载 `Action(text, parent)`。
                    assert_eq!(action.icon.as_deref(), want["icon"].as_str(), "{at}: 图标");
                    assert_eq!(
                        action.checked,
                        want.get("checked").and_then(Value::as_bool),
                        "{at}: checked"
                    );
                    let calls = want["invoke"]["calls"]
                        .as_array()
                        .unwrap_or_else(|| panic!("{at}: invoke.calls 缺失"));
                    let first = calls
                        .first()
                        .unwrap_or_else(|| panic!("{at}: 回调没有产生任何调用"));
                    let (sink, call) = action.click.dispatch();
                    assert_eq!(first["sink"].as_str(), Some(sink), "{at}: 调用落点");
                    assert_eq!(first["call"].as_str(), Some(call), "{at}: 方法名");
                    assert_eq!(
                        first["args"],
                        Value::Array(expected_args(&action.click)),
                        "{at}: 参数"
                    );
                }
                "submenu" => {
                    let CardMenuEntry::Submenu(submenu) = entry else {
                        panic!("{at}: 应为子菜单，实为 {entry:?}")
                    };
                    assert_eq!(
                        submenu.text,
                        want["text"].as_str().expect("子菜单 text 缺失"),
                        "{at}: 子菜单标题"
                    );
                    assert_eq!(
                        submenu.icon.as_deref(),
                        want["icon"].as_str(),
                        "{at}: 子菜单图标"
                    );
                    assert_menu(&submenu.items, &want["items"], &at);
                }
                other => panic!("{at}: 未知 type {other}"),
            }
        }
    }

    /// 第 58 步的主钉子：`card_menu.jsonl` 全量重算。
    #[test]
    fn matches_python_card_menu_fixture() {
        let cases = cases();
        let icons = known_icons();
        let mut kinds: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for case in &cases {
            let kind = case["kind"].as_str().expect("kind 缺失");
            *kinds.entry(kind.to_string()).or_insert(0) += 1;
            match kind {
                "context-menu" => {
                    let categories: Vec<MenuCategory> = case["categories"]
                        .as_array()
                        .expect("categories 缺失")
                        .iter()
                        .map(|category| MenuCategory {
                            id: category["categoryId"]
                                .as_str()
                                .expect("categoryId 缺失")
                                .to_string(),
                            name: category["name"].as_str().expect("name 缺失").to_string(),
                            icon: category["icon"].as_str().expect("icon 缺失").to_string(),
                        })
                        .collect();
                    let input = ContextMenuInput {
                        can_edit: case["canEdit"].as_bool().expect("canEdit 缺失"),
                        status: status_of(case),
                        category_enabled: case["categoryEnabled"]
                            .as_bool()
                            .expect("categoryEnabled 缺失"),
                        categories: &categories,
                        known_icons: &icons,
                    };
                    assert_menu(&context_menu(&input), &case["menu"], "menu");
                }
                "checksum-click" => {
                    let algorithms = strings(&case["algorithms"]);
                    let checksums = strings(&case["checksums"]);
                    let menu_input = ChecksumMenuInput {
                        algorithms: &algorithms,
                        checksums: &checksums,
                    };
                    let outcome = checksum_click(
                        case["inProgress"].as_bool().expect("inProgress 缺失"),
                        case["folder"].as_bool().expect("folder 缺失"),
                        case["exists"].as_bool().expect("exists 缺失"),
                        || checksum_menu(&menu_input),
                    );
                    let want = &case["outcome"];
                    let kind_want = want["kind"].as_str().expect("outcome.kind 缺失");
                    match (&outcome, kind_want) {
                        (ChecksumOutcome::CancelChecksum, "cancel") => {}
                        (ChecksumOutcome::SetStatus(status), "status") => assert_eq!(
                            *status,
                            want["text"].as_str().expect("outcome.text 缺失"),
                            "状态行文案"
                        ),
                        (ChecksumOutcome::ShowMenu(items), "menu") => {
                            assert_menu(items, &want["menu"], "checksum")
                        }
                        (other, expected) => {
                            panic!("结局不符：Rust {other:?} / Python {expected}")
                        }
                    }
                    // 调用序列：第一条恒是 `checksumProgress`；取消分支多一条。
                    let calls = case["calls"].as_array().expect("calls 缺失");
                    assert_eq!(calls[0]["sink"].as_str(), Some("task"), "第一条调用落点");
                    assert_eq!(
                        calls[0]["call"].as_str(),
                        Some("checksumProgress"),
                        "第一条调用"
                    );
                    assert_eq!(calls[0]["args"], Value::Array(vec![text(TASK_ID)]));
                    if kind_want == "cancel" {
                        assert_eq!(calls.len(), 2, "取消分支应只有两条调用");
                        assert_eq!(calls[1]["sink"].as_str(), Some("task"));
                        assert_eq!(calls[1]["call"].as_str(), Some("cancelChecksum"));
                        assert_eq!(calls[1]["args"], Value::Array(vec![text(TASK_ID)]));
                    } else {
                        assert_eq!(calls.len(), 1, "非取消分支不该再打服务");
                    }
                }
                "can-drag" => assert_eq!(
                    can_drag(
                        status_of(case),
                        case["hasOutputFile"].as_bool().expect("hasOutputFile 缺失"),
                        case["exists"].as_bool().expect("exists 缺失"),
                    ),
                    case["result"].as_bool().expect("result 缺失"),
                    "canDrag"
                ),
                "release" => {
                    let input = ReleaseInput {
                        left_button: case["button"].as_str().expect("button 缺失") == "left",
                        has_dragged: case["hasDragged"].as_bool().expect("hasDragged 缺失"),
                        match_name: case["matchName"].as_bool().expect("matchName 缺失"),
                        match_info: case["matchInfo"].as_bool().expect("matchInfo 缺失"),
                        extend: case["extend"].as_bool().expect("extend 缺失"),
                        selection_mode: case["selectionMode"]
                            .as_bool()
                            .expect("selectionMode 缺失"),
                        is_checked: case["checked"].as_bool().expect("checked 缺失"),
                    };
                    let want = &case["outcome"];
                    let gesture = want["gesture"].as_str().expect("gesture 缺失");
                    match release_gesture(&input) {
                        ReleaseGesture::Ignore => assert_eq!(gesture, "none", "应提前返回"),
                        ReleaseGesture::OpenFile => {
                            assert_eq!(gesture, "openFile");
                            assert_eq!(want["path"].as_str(), Some(OUTPUT_PATH), "打开的文件");
                        }
                        ReleaseGesture::OpenDrawer => {
                            assert_eq!(gesture, "openDrawer");
                            let calls = want["calls"].as_array().expect("calls 缺失");
                            assert_eq!(calls.len(), 2, "开抽屉 = 构造 + show");
                            assert_eq!(calls[0]["sink"].as_str(), Some("drawer"));
                            assert_eq!(calls[0]["call"].as_str(), Some("TaskDrawer"));
                            assert_eq!(calls[0]["args"], Value::Array(vec![text(TASK_ID)]));
                            assert_eq!(calls[1]["call"].as_str(), Some("show"));
                        }
                        ReleaseGesture::Select { checked, extend } => {
                            assert_eq!(gesture, "select");
                            assert_eq!(checked, want["checked"].as_bool().expect("checked 缺失"));
                            assert_eq!(extend, want["extend"].as_bool().expect("extend 缺失"));
                        }
                    }
                }
                other => panic!("未知 kind：{other}"),
            }
        }
        assert_eq!(cases.len(), 133, "金文件条数");
        assert_eq!(kinds.get("context-menu"), Some(&16), "右键菜单用例数");
        assert_eq!(kinds.get("checksum-click"), Some(&5), "校验用例数");
        assert_eq!(kinds.get("can-drag"), Some(&16), "拖拽资格用例数");
        assert_eq!(kinds.get("release"), Some(&96), "松开手势用例数");
    }

    /// 图标兜底：`getattr(FluentIcon, name, FluentIcon.TAG)`。
    #[test]
    fn category_icon_falls_back_to_tag() {
        let icons = known_icons();
        assert_eq!(
            category_menu_icon(&icons, "VIDEO"),
            "VIDEO",
            "表里的图标原样用"
        );
        assert_eq!(
            category_menu_icon(&icons, "NOT_A_REAL_ICON"),
            ICON_TAG,
            "表外的图标退回 TAG"
        );
        // 右键菜单里的固定图标也必须在表里，否则 Rust 侧会显示成 TAG。
        for icon in [
            ICON_DETAIL,
            ICON_COPY_URL,
            ICON_EDIT,
            ICON_MOVE_TO_FRONT,
            ICON_REDOWNLOAD,
            ICON_TAG,
            ICON_UNCATEGORIZED,
        ] {
            assert!(
                icons.iter().any(|known| known == icon),
                "{icon} 不在 FluentIcon 表里"
            );
        }
    }

    /// `更多` 子菜单**无条件**存在（Python 没有判空），且算法按输入顺序分配。
    #[test]
    fn checksum_menu_keeps_more_submenu_even_when_empty() {
        let common: Vec<String> = COMMON_ALGORITHMS
            .iter()
            .map(|name| name.to_string())
            .collect();
        let menu = checksum_menu(&ChecksumMenuInput {
            algorithms: &common,
            checksums: &[],
        });
        assert_eq!(
            menu.len(),
            COMMON_ALGORITHMS.len() + 1,
            "4 个常用算法 + 更多"
        );
        let Some(CardMenuEntry::Submenu(more)) = menu.last() else {
            panic!("最后一项应为子菜单");
        };
        assert_eq!(more.text, MENU_MORE);
        assert!(more.icon.is_none(), "`更多` 没有 setIcon");
        assert!(more.items.is_empty(), "没有剩余算法时子菜单为空");
        for (entry, algorithm) in menu.iter().zip(COMMON_ALGORITHMS) {
            let action = entry.action().expect("应为动作");
            assert_eq!(action.text, algorithm.to_uppercase(), "文本是算法名大写");
            assert!(action.icon.is_none(), "两参重载没有图标");
            assert_eq!(action.checked, Some(false));
        }
    }

    /// 双击：只有左键打开文件。
    #[test]
    fn double_click_opens_file_only_for_left_button() {
        assert!(double_click_opens_file(true));
        assert!(!double_click_opens_file(false));
    }
}
