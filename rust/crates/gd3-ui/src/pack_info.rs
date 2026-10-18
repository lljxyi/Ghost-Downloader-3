//! 功能包信息对话框（P3，第 67 步）：镜像 `app/view/dialogs/pack_info.py`
//! （106 行）的语义层。
//!
//! ## 实证口径（2026-10-18 实读源码）
//!
//! - 状态色四组（:16-19）：
//!   `STATUS_SUCCESS_COLORS=("#0F7B0F","#6CCB5F")`、
//!   `STATUS_AVAILABLE_COLORS=("#9D5D00","#FFC83D")`、
//!   `STATUS_DOWNLOADING_COLORS=("#0067C0","#60CDFF")`、
//!   `STATUS_FAILED_COLORS=("#C42B1C","#FF99A4")`，都是 `(浅色, 深色)`。
//! - `PackRow._initWidget`（:30-42）：`nameLabel` 文本是 `type(pack).__name__`
//!   —— **Python 类名**，Rust 侧由 pack 自己给 `display_name`；
//!   `versionLabel` = `f"v{pack.manifest.version}"`；`statusLabel` 初值 `✓`
//!   且用 success 色；`statusLabel.setFixedWidth(18)`；`retryButton` 24×24 且
//!   **初始隐藏**（`hide()`）。
//! - `PackRow.update(info)`（:55-73）：只处理四个状态，**没有 else 分支**
//!   —— `IDLE` / `CHECKING` 不改任何东西：
//!   - `AVAILABLE` → `↑` + available 色 + 隐藏 retry；
//!   - `DOWNLOADING` → `↓` + downloading 色 + 隐藏 retry；
//!   - `READY` → `✓` + success 色 + 隐藏 retry；
//!   - `FAILED` → `✗` + failed 色 + **显示** retry。
//! - `PackInfoDialog._initWidget`（:84-98）：标题 `功能包`、
//!   `widget.setMinimumWidth(400)`、`yesButton` = `关闭`、`cancelButton.hide()`；
//!   遍历 packs，`manifest is None` 直接 `continue`，`_rows[manifest.name] = row`
//!   —— **键是 `manifest.name` 而不是类名**，重名时后者覆盖前者（只有最后一个
//!   行会收到更新，但所有行都加进布局）。
//! - `_onUpdateChanged(info)`（:103-106）：`_rows.get(info.targetId)`，
//!   拿不到就忽略（`None` → 什么都不做）。
//! - retry 按钮点击 → `self._updateService.download(manifestName)`
//!   （`lambda n=manifestName: ...`，绑定的是**声明时的**名字）。
//!
//! ## 与 Qt 的差异
//!
//! Qt 用 `updateService.changed` 信号；Rust 侧由外壳把 `(targetId, state)`
//! 喂给 [`PackInfoState::apply_update`]。`UpdateState` 在这里定义成 `i32` 枚举，
//! 值对齐 Python 的 `IntEnum`（`IDLE=1 … FAILED=6`），方便跨语言序列化。

use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// 文案与常量（`pack_info.py` 逐字）
// ---------------------------------------------------------------------------

/// `STATUS_SUCCESS_COLORS`（:16），`(浅色, 深色)`。
pub const STATUS_SUCCESS_COLORS: (&str, &str) = ("#0F7B0F", "#6CCB5F");
/// `STATUS_AVAILABLE_COLORS`（:17）
pub const STATUS_AVAILABLE_COLORS: (&str, &str) = ("#9D5D00", "#FFC83D");
/// `STATUS_DOWNLOADING_COLORS`（:18）
pub const STATUS_DOWNLOADING_COLORS: (&str, &str) = ("#0067C0", "#60CDFF");
/// `STATUS_FAILED_COLORS`（:19）
pub const STATUS_FAILED_COLORS: (&str, &str) = ("#C42B1C", "#FF99A4");

/// `SubtitleLabel(self.tr("功能包"))`（:85）
pub const TITLE: &str = "功能包";
/// `self.yesButton.setText(self.tr("关闭"))`（:89）
pub const BUTTON_CLOSE: &str = "关闭";
/// `self.widget.setMinimumWidth(400)`（:88）
pub const MIN_WIDTH: i32 = 400;
/// `self.statusLabel.setFixedWidth(18)`（:36）
pub const STATUS_LABEL_WIDTH: i32 = 18;
/// `self.retryButton.setFixedSize(24, 24)`（:40）
pub const RETRY_BUTTON_SIZE: (i32, i32) = (24, 24);
/// `CaptionLabel("✓")`（:35）—— `PackRow` 的初值。
pub const INITIAL_GLYPH: &str = "✓";

/// `app/services/update_service.py::UpdateState`（`IntEnum`，:95-101）。
///
/// `auto()` 从 1 起，所以数值就是 1..6；这里显式写出来，方便和 Python 侧互认。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum UpdateState {
    Idle = 1,
    Checking = 2,
    Available = 3,
    Downloading = 4,
    Ready = 5,
    Failed = 6,
}

impl UpdateState {
    /// `IntEnum` 的数值。
    pub fn value(self) -> i32 {
        self as i32
    }

    /// 从 `IntEnum` 数值还原；越界返回 `None`（Python 侧不可能出现）。
    pub fn from_value(value: i32) -> Option<Self> {
        Some(match value {
            1 => Self::Idle,
            2 => Self::Checking,
            3 => Self::Available,
            4 => Self::Downloading,
            5 => Self::Ready,
            6 => Self::Failed,
            _ => return None,
        })
    }

    /// `IntEnum` 的成员名（`str(UpdateState.AVAILABLE)` 之外的序列化口径）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "IDLE",
            Self::Checking => "CHECKING",
            Self::Available => "AVAILABLE",
            Self::Downloading => "DOWNLOADING",
            Self::Ready => "READY",
            Self::Failed => "FAILED",
        }
    }

    /// 名字 → 状态（大小写敏感，与 `UpdateState["IDLE"]` 一致）。
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "IDLE" => Self::Idle,
            "CHECKING" => Self::Checking,
            "AVAILABLE" => Self::Available,
            "DOWNLOADING" => Self::Downloading,
            "READY" => Self::Ready,
            "FAILED" => Self::Failed,
            _ => return None,
        })
    }
}

/// 一行功能包所需的输入：`display_name` = `type(pack).__name__`，
/// `manifest` = `(name, version)`，`None` 时该行被跳过。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackEntry {
    pub display_name: String,
    pub manifest_name: Option<String>,
    pub manifest_version: Option<String>,
}

impl PackEntry {
    /// 有 `manifest.toml` 的包。
    pub fn new(display_name: &str, manifest_name: &str, version: &str) -> Self {
        Self {
            display_name: display_name.to_string(),
            manifest_name: Some(manifest_name.to_string()),
            manifest_version: Some(version.to_string()),
        }
    }

    /// `pack.manifest is None` 的包 —— 构造对话框时被 `continue` 跳过。
    pub fn without_manifest(display_name: &str) -> Self {
        Self {
            display_name: display_name.to_string(),
            manifest_name: None,
            manifest_version: None,
        }
    }
}

/// `PackRow` 的可观察状态（Slint 侧一行三个属性 + retry 可见性）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackRowState {
    /// `nameLabel` 文本（`type(pack).__name__`）。
    pub display_name: String,
    /// `_rows` 的键（`manifest.name`），retry 回调要用它。
    pub manifest_name: String,
    /// `versionLabel` 文本（`f"v{version}"`）。
    pub version_text: String,
    /// `statusLabel` 文本。
    pub status_glyph: String,
    /// `statusLabel` 的 `(浅色, 深色)`。
    pub status_colors: (&'static str, &'static str),
    /// `retryButton` 是否可见。
    pub retry_visible: bool,
}

impl PackRowState {
    /// `PackRow._initWidget` + `_initLayout` 的终态：`✓` / success 色 / retry 隐藏。
    pub fn new(display_name: &str, manifest_name: &str, version: &str) -> Self {
        Self {
            display_name: display_name.to_string(),
            manifest_name: manifest_name.to_string(),
            version_text: format!("v{version}"),
            status_glyph: INITIAL_GLYPH.to_string(),
            status_colors: STATUS_SUCCESS_COLORS,
            retry_visible: false,
        }
    }

    /// `PackRow.update(info)`（:55-73）。
    ///
    /// 返回是否改动了状态：`IDLE` / `CHECKING` 走「没有 else 分支」的路径，
    /// 原样返回 `false`。
    pub fn apply(&mut self, state: UpdateState) -> bool {
        match state {
            UpdateState::Available => {
                self.status_glyph = "↑".to_string();
                self.status_colors = STATUS_AVAILABLE_COLORS;
                self.retry_visible = false;
            }
            UpdateState::Downloading => {
                self.status_glyph = "↓".to_string();
                self.status_colors = STATUS_DOWNLOADING_COLORS;
                self.retry_visible = false;
            }
            UpdateState::Ready => {
                self.status_glyph = INITIAL_GLYPH.to_string();
                self.status_colors = STATUS_SUCCESS_COLORS;
                self.retry_visible = false;
            }
            UpdateState::Failed => {
                self.status_glyph = "✗".to_string();
                self.status_colors = STATUS_FAILED_COLORS;
                self.retry_visible = true;
            }
            // Python 里 `IDLE` / `CHECKING` 落到 `if/elif` 链之外，什么都不做。
            UpdateState::Idle | UpdateState::Checking => return false,
        }
        true
    }
}

/// `PackInfoDialog` 的行集合 + `_rows` 索引。
#[derive(Clone, Debug, Default)]
pub struct PackInfoState {
    rows: Vec<PackRowState>,
    /// `_rows: dict[str, PackRow]` —— 键是 `manifest.name`，重名时后写覆盖。
    index: BTreeMap<String, usize>,
}

impl PackInfoState {
    /// `_initWidget` 的建行循环：`manifest is None` 跳过，其余按顺序建行。
    pub fn new(packs: &[PackEntry]) -> Self {
        let mut rows = Vec::new();
        let mut index = BTreeMap::new();
        for pack in packs {
            let (Some(manifest_name), Some(version)) =
                (pack.manifest_name.as_ref(), pack.manifest_version.as_ref())
            else {
                continue;
            };
            index.insert(manifest_name.clone(), rows.len());
            rows.push(PackRowState::new(
                &pack.display_name,
                manifest_name,
                version,
            ));
        }
        Self { rows, index }
    }

    /// 布局里的行（顺序即 `viewLayout.addWidget(row)` 的顺序）。
    pub fn rows(&self) -> &[PackRowState] {
        &self.rows
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// `_onUpdateChanged(info)`：`_rows.get(info.targetId)` 命中才更新。
    ///
    /// 返回命中的行下标（供外壳按需重绘），未命中返回 `None`。
    pub fn apply_update(&mut self, target_id: &str, state: UpdateState) -> Option<usize> {
        let index = *self.index.get(target_id)?;
        self.rows[index].apply(state);
        Some(index)
    }

    /// retry 按钮的回调参数：`updateService.download(manifestName)`。
    pub fn retry_target(&self, target_id: &str) -> Option<&str> {
        let index = *self.index.get(target_id)?;
        Some(self.rows[index].manifest_name.as_str())
    }

    /// 某个 `manifest.name` 当前对应的行（测试与外壳共用）。
    pub fn row(&self, manifest_name: &str) -> Option<&PackRowState> {
        self.index
            .get(manifest_name)
            .map(|index| &self.rows[*index])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packs() -> Vec<PackEntry> {
        vec![
            PackEntry::new("BiliPack", "bili", "1.2.3"),
            PackEntry::without_manifest("BrokenPack"),
            PackEntry::new("M3u8Pack", "m3u8", "0.4.0"),
        ]
    }

    #[test]
    fn rows_skip_packs_without_manifest() {
        let state = PackInfoState::new(&packs());
        assert_eq!(state.row_count(), 2);
        let names: Vec<&str> = state
            .rows()
            .iter()
            .map(|row| row.display_name.as_str())
            .collect();
        assert_eq!(names, vec!["BiliPack", "M3u8Pack"]);
        assert!(state.row("broken").is_none());
    }

    #[test]
    fn row_initial_state_is_a_success_check_mark() {
        let state = PackInfoState::new(&packs());
        let row = state.row("bili").expect("bili 行应存在");
        assert_eq!(row.display_name, "BiliPack");
        assert_eq!(row.manifest_name, "bili");
        assert_eq!(row.version_text, "v1.2.3");
        assert_eq!(row.status_glyph, "✓");
        assert_eq!(row.status_colors, STATUS_SUCCESS_COLORS);
        assert!(!row.retry_visible);
    }

    /// 四个状态各自的字形 / 颜色 / retry 可见性。
    #[test]
    fn update_branches_match_python_table() {
        let mut state = PackInfoState::new(&packs());
        let cases = [
            (UpdateState::Available, "↑", STATUS_AVAILABLE_COLORS, false),
            (
                UpdateState::Downloading,
                "↓",
                STATUS_DOWNLOADING_COLORS,
                false,
            ),
            (UpdateState::Ready, "✓", STATUS_SUCCESS_COLORS, false),
            (UpdateState::Failed, "✗", STATUS_FAILED_COLORS, true),
        ];
        for (update, glyph, colors, retry) in cases {
            assert_eq!(state.apply_update("bili", update), Some(0));
            let row = state.row("bili").expect("bili 行应存在");
            assert_eq!(row.status_glyph, glyph, "state={update:?}");
            assert_eq!(row.status_colors, colors, "state={update:?}");
            assert_eq!(row.retry_visible, retry, "state={update:?}");
        }
    }

    /// `IDLE` / `CHECKING` 没有 else 分支：`FAILED` 之后收到它们不会复原。
    #[test]
    fn idle_and_checking_leave_the_row_untouched() {
        let mut state = PackInfoState::new(&packs());
        state.apply_update("bili", UpdateState::Failed);
        for update in [UpdateState::Idle, UpdateState::Checking] {
            assert_eq!(state.apply_update("bili", update), Some(0));
            let row = state.row("bili").expect("bili 行应存在");
            assert_eq!(row.status_glyph, "✗", "state={update:?}");
            assert_eq!(row.status_colors, STATUS_FAILED_COLORS, "state={update:?}");
            assert!(row.retry_visible, "state={update:?}");
        }
        let mut row = PackRowState::new("BiliPack", "bili", "1.2.3");
        assert!(!row.apply(UpdateState::Idle));
        assert!(!row.apply(UpdateState::Checking));
        assert_eq!(row.status_glyph, "✓");
    }

    #[test]
    fn unknown_target_ids_are_ignored() {
        let mut state = PackInfoState::new(&packs());
        assert_eq!(state.apply_update("app", UpdateState::Failed), None);
        assert_eq!(state.apply_update("BiliPack", UpdateState::Failed), None);
        assert!(state.rows().iter().all(|row| row.status_glyph == "✓"));
        assert!(state.retry_target("app").is_none());
    }

    /// `_rows` 键是 `manifest.name`；重名时后写覆盖索引，但两行都在布局里。
    #[test]
    fn duplicate_manifest_names_last_row_wins() {
        let packs = vec![
            PackEntry::new("PackA", "dup", "1.0"),
            PackEntry::new("PackB", "dup", "2.0"),
        ];
        let mut state = PackInfoState::new(&packs);
        assert_eq!(state.row_count(), 2);
        assert_eq!(
            state.row("dup").map(|row| row.display_name.as_str()),
            Some("PackB")
        );
        assert_eq!(state.apply_update("dup", UpdateState::Failed), Some(1));
        assert_eq!(state.rows()[0].status_glyph, "✓");
        assert_eq!(state.rows()[1].status_glyph, "✗");
    }

    #[test]
    fn retry_target_returns_the_manifest_name() {
        let state = PackInfoState::new(&packs());
        assert_eq!(state.retry_target("bili"), Some("bili"));
        assert_eq!(state.retry_target("m3u8"), Some("m3u8"));
        assert_eq!(state.retry_target("M3u8Pack"), None);
    }

    #[test]
    fn update_state_round_trips_through_values_and_names() {
        for value in 1..=6 {
            let state = UpdateState::from_value(value).expect("1..=6 都应有状态");
            assert_eq!(state.value(), value);
            assert_eq!(UpdateState::from_name(state.as_str()), Some(state));
        }
        assert_eq!(UpdateState::from_value(0), None);
        assert_eq!(UpdateState::from_value(7), None);
        assert_eq!(UpdateState::from_name("idle"), None);
        assert_eq!(UpdateState::Available.value(), 3);
        assert_eq!(UpdateState::Failed.value(), 6);
    }

    #[test]
    fn dialog_literals_match_python() {
        assert_eq!(TITLE, "功能包");
        assert_eq!(BUTTON_CLOSE, "关闭");
        assert_eq!(MIN_WIDTH, 400);
        assert_eq!(STATUS_LABEL_WIDTH, 18);
        assert_eq!(RETRY_BUTTON_SIZE, (24, 24));
        assert_eq!(INITIAL_GLYPH, "✓");
        assert_eq!(STATUS_SUCCESS_COLORS, ("#0F7B0F", "#6CCB5F"));
        assert_eq!(STATUS_AVAILABLE_COLORS, ("#9D5D00", "#FFC83D"));
        assert_eq!(STATUS_DOWNLOADING_COLORS, ("#0067C0", "#60CDFF"));
        assert_eq!(STATUS_FAILED_COLORS, ("#C42B1C", "#FF99A4"));
    }
}
