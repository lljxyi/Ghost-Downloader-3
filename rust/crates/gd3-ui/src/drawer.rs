//! 任务抽屉派生层（`app/view/dialogs/task_drawer.py` 镜像；第 52 步）。
//!
//! Python 侧抽屉是六张卡片的组合：`HeaderCard` / `ErrorBar` / `InfoCard` /
//! `ChecksumCard` / `FileListCard` + `featureService.detailCards()`。本模块把
//! 前五张的 `refresh()` 摊平成纯函数（每张一个 `*_state` 函数 + 一个结果
//! struct），逐条对齐：
//!
//! - `HeaderCard.refresh`：名称、进度条 `setVisible(status != COMPLETED and
//!   fileSize > 0)` / `setValue(int(progress))` / `setError(status == FAILED)`、
//!   状态行 `_toStatusText`（与 `DetailCard._toStatusText` **同一份实现**，
//!   直接复用 `row::to_status_text`）；
//! - `ErrorBar.refresh`：`status == FAILED` 才取 `lastError`，
//!   `toLocalizedError` 后为空则整条隐藏（复用 `card::error_text` 的无译文层）；
//! - `InfoCard.refresh`：六行（下载链接/保存位置/大小/分类/创建时间/完成时间），
//!   大小行 `fileSize > 0` 才可见，分类行按 `cfg.isCategoryEnabled`，
//!   完成时间行按 `bool(completedAt)`；
//! - `ChecksumCard.refresh`：`visible = bool(checksums) and not (files and
//!   len(files) > 1)`；行按**原始键**排序后 `algorithm.upper()`；
//!   `_refreshMatch` 走 `matchChecksum`（`parseChecksum` = 首个空白分隔词小写）；
//! - `FileListCard.refresh`：`visible = len(files) > 1`，
//!   行 = (relativePath, toReadableSize(size), `_toFileStatusText`)。
//!
//! 分层边界（与 `card.rs` 一致：IO / 译文 / 时区不在本层）：
//! - `output_path`：Python `str(Path(task.outputFolder) / task.name)` 是路径
//!   拼接，跨平台分隔符由调用方定（与 `output_exists` 同源，调用方本就要算）；
//! - 分类显示名：`toCategoryName(category)` 需要译文，调用方给成品字符串；
//!   未分类时 Python 走 `self.tr("未分类")`，本层直接出源串；
//! - `utc_offset_seconds`：`datetime.fromtimestamp` 用**本地时区**，`gd3-ui`
//!   没有时区库，故由调用方给偏移，本层只做「时间戳 + 偏移 → 格式化」；
//! - `expected_checksum`：`expectedEdit.text()`，控件状态归调用方。
//!
//! 未纳入本步的：`TaskDrawer` 自身的滑入/滑出动画与 `featureService`
//! 扩展卡片（前者是平台外壳，后者是插件位）。

use crate::card::{CardRuntime, CardTask, error_text};
use crate::format::to_readable_size;
use crate::pystr::{py_isspace, py_strip};
use crate::row::to_status_text;
use gd3_core::task::{TaskRecord, TaskStatus};
use serde_json::Value;

/// `DRAWER_WIDTH`：抽屉宽度上限（`min(DRAWER_WIDTH, self.width())`）。
pub const DRAWER_WIDTH: i32 = 592;
/// `SLIDE_DURATION`：滑入/滑出动画时长（毫秒）。
pub const SLIDE_DURATION: i32 = 300;
/// `CAPTION_WIDTH`：信息行标题固定宽度。
pub const CAPTION_WIDTH: i32 = 88;
/// `ROW_BUTTON_SIZE`：信息行内小按钮边长。
pub const ROW_BUTTON_SIZE: i32 = 28;
/// `MATCH_COLORS`：校验值命中时输入框的聚焦边框色（深色 / 浅色主题）。
pub const MATCH_COLORS: (&str, &str) = ("#0F7B0F", "#6CCB5F");

/// 记录里**没有**、需要调用方补的抽屉输入。
///
/// 复用 `card::CardRuntime`（`isSeeding` / `canPause` / `hasOutputFile` /
/// `output_exists` / `lastError` 与卡片同源），另加抽屉专属四项。
#[derive(Clone, Debug, Default)]
pub struct DrawerRuntime {
    /// 卡片层已经需要的运行时事实（`is_seeding` / `output_exists` / `last_error`…）。
    pub card: CardRuntime,
    /// `task.outputPath`（`str(Path(outputFolder) / name)`，分隔符由调用方定）。
    pub output_path: String,
    /// 本地时区偏移（秒）；`datetime.fromtimestamp` 的等价输入。
    pub utc_offset_seconds: i64,
    /// `cfg.isCategoryEnabled.value`。
    pub category_enabled: bool,
    /// `toCategoryName(category)`；`None` = 任务无分类 → 显示「未分类」。
    pub category_name: Option<String>,
    /// `expectedEdit.text()`（校验值比对输入框当前内容）。
    pub expected_checksum: String,
}

// ---------------------------------------------------------------------------
// 时间与校验值原语
// ---------------------------------------------------------------------------

/// 镜像 `toTimeText(timestamp)`：`datetime.fromtimestamp(t).strftime("%Y-%m-%d %H:%M:%S")`。
///
/// `utc_offset_seconds` 就是「本地时区相对 UTC 的偏移」，故
/// `to_time_text(t, off)` 等价于把 `t + off` 当 UTC 格式化。跨 DST 的时间点
/// 由调用方按当时生效的偏移给（Python 的 `fromtimestamp` 也按当时偏移算）。
pub fn to_time_text(timestamp: i64, utc_offset_seconds: i64) -> String {
    let local = timestamp.saturating_add(utc_offset_seconds);
    let days = local.div_euclid(86_400);
    let seconds = local.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (seconds / 3600, (seconds / 60) % 60, seconds % 60);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
}

/// 天数（1970-01-01 起）→ (年, 月, 日)。Howard Hinnant 的 `civil_from_days`。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { y + 1 } else { y }, month, day)
}

/// 镜像 CPython 无参 `str.split()`：按 `str.isspace()` 的字符切，丢弃空段。
fn py_split_whitespace(text: &str) -> Vec<&str> {
    text.split(py_isspace)
        .filter(|part| !part.is_empty())
        .collect()
}

/// 镜像 `parseChecksum(text)`：`(text.split() or [""])[0].lower()`。
pub fn parse_checksum(text: &str) -> String {
    py_split_whitespace(text)
        .first()
        .map(|first| first.to_lowercase())
        .unwrap_or_default()
}

/// 镜像 `matchChecksum(checksums, expected)`：按 **dict 插入顺序**找第一个
/// 值等于 `parseChecksum(expected)` 的算法，返回其原始键（`None` = 都不一致）。
pub fn match_checksum<'a>(
    checksums: &'a serde_json::Map<String, Value>,
    expected: &str,
) -> Option<&'a str> {
    let expected = parse_checksum(expected);
    checksums
        .iter()
        .find(|(_, value)| value.as_str() == Some(expected.as_str()))
        .map(|(algorithm, _)| algorithm.as_str())
}

// ---------------------------------------------------------------------------
// HeaderCard
// ---------------------------------------------------------------------------

/// `HeaderCard.refresh()` 的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderCardState {
    pub name: String,
    /// `setValue(int(progress))`。
    pub progress_value: i64,
    /// `setVisible(task.status != COMPLETED and task.fileSize > 0)`——注意与
    /// 列表卡片的「三态可见性」不同，抽屉头部是**直接**设可见性。
    pub progress_visible: bool,
    /// `setError(task.status == FAILED)`。
    pub progress_error: bool,
    /// `statusLabel.setText(self._toStatusText(speed, received))`（恒设置）。
    pub status_text: String,
}

/// 镜像 `HeaderCard.refresh`（图标 pixmap 需要真实文件系统，不在派生层）。
pub fn header_card(record: &TaskRecord, runtime: &DrawerRuntime) -> HeaderCardState {
    let task = CardTask::from_record(record, &runtime.card);
    HeaderCardState {
        name: record.name.clone(),
        progress_value: task.progress as i64,
        progress_visible: record.status != TaskStatus::Completed && record.file_size > 0,
        progress_error: record.status == TaskStatus::Failed,
        status_text: to_status_text(record, task.speed, task.received, task.is_seeding),
    }
}

// ---------------------------------------------------------------------------
// ErrorBar
// ---------------------------------------------------------------------------

/// `ErrorBar.refresh()` 的结果。`text` 为空即整条隐藏（Python `setVisible`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorBarState {
    pub visible: bool,
    /// `InfoBar` 的标题：`self.tr("下载失败")`。
    pub title: &'static str,
    pub text: String,
}

/// 镜像 `ErrorBar.refresh`：只有 FAILED 才看 `lastError`。
pub fn error_bar(record: &TaskRecord, runtime: &DrawerRuntime) -> ErrorBarState {
    let text = if record.status == TaskStatus::Failed {
        runtime
            .card
            .last_error
            .as_ref()
            .map(error_text)
            .unwrap_or_default()
    } else {
        String::new()
    };
    ErrorBarState {
        visible: !text.is_empty(),
        title: "下载失败",
        text,
    }
}

// ---------------------------------------------------------------------------
// InfoCard
// ---------------------------------------------------------------------------

/// 信息行（`InfoRow`）：标题固定、值可选中复制。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InfoRowState {
    /// 行标识（`url` / `path` / `size` / `category` / `created` / `completed`）。
    pub name: &'static str,
    /// `CaptionLabel(caption)` 的源串。
    pub caption: &'static str,
    pub text: String,
    pub visible: bool,
}

/// `InfoCard.refresh()` 的结果：六行，顺序即 `_initLayout` 的 `addWidget` 顺序。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InfoCardState {
    pub rows: Vec<InfoRowState>,
}

/// 镜像 `InfoCard.refresh`。
pub fn info_card(record: &TaskRecord, runtime: &DrawerRuntime) -> InfoCardState {
    let offset = runtime.utc_offset_seconds;
    let has_completed = record.completed_at != 0;
    InfoCardState {
        rows: vec![
            InfoRowState {
                name: "url",
                caption: "下载链接",
                text: record.url.clone(),
                visible: true,
            },
            InfoRowState {
                name: "path",
                caption: "保存位置",
                text: runtime.output_path.clone(),
                visible: true,
            },
            InfoRowState {
                name: "size",
                caption: "大小",
                // Python 无条件 setValue，隐藏时也留着文本。
                text: to_readable_size(record.file_size as f64),
                visible: record.file_size > 0,
            },
            InfoRowState {
                name: "category",
                caption: "分类",
                text: runtime
                    .category_name
                    .clone()
                    .unwrap_or_else(|| "未分类".to_string()),
                visible: runtime.category_enabled,
            },
            InfoRowState {
                name: "created",
                caption: "创建时间",
                text: to_time_text(record.created_at, offset),
                visible: true,
            },
            InfoRowState {
                name: "completed",
                caption: "完成时间",
                text: if has_completed {
                    to_time_text(record.completed_at, offset)
                } else {
                    String::new()
                },
                visible: has_completed,
            },
        ],
    }
}

// ---------------------------------------------------------------------------
// ChecksumCard
// ---------------------------------------------------------------------------

/// 校验值一行：`InfoRow(algorithm.upper(), …)`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChecksumRow {
    /// 已 `upper()` 的算法名（Python `algorithm.upper()`）。
    pub algorithm: String,
    pub value: String,
}

/// `_refreshMatch` 的结果（期望值输入框的三处状态）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChecksumMatch {
    /// `matchChecksum` 命中的算法键（**原始**大小写，`None` = 都不一致）。
    pub matched: Option<String>,
    /// `setError(bool(expected.strip()) and algorithm is None)`。
    pub error: bool,
    /// `setToolTip(...)` 的文本。
    pub tooltip: String,
}

/// `ChecksumCard.refresh()` 的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChecksumCardState {
    /// `setVisible(bool(checksums) and not (files and len(files) > 1))`。
    pub visible: bool,
    pub rows: Vec<ChecksumRow>,
    pub matched: ChecksumMatch,
}

/// 镜像 `ChecksumCard.refresh` + `_refreshMatch`。
///
/// Python 侧还有一层 `task.checksums == self._shownChecksums` 的**缓存短路**
/// （相等就不重建行、也不调 `_refreshMatch`）——那是控件状态，归调用方。
pub fn checksum_card(record: &TaskRecord, expected: &str) -> ChecksumCardState {
    let files = record.files.as_deref().unwrap_or(&[]);
    let visible = !record.checksums.is_empty() && files.len() <= 1;

    // `sorted(task.checksums.items())` 按**原始键**排序，之后才 upper()。
    let mut entries: Vec<(&str, &str)> = record
        .checksums
        .iter()
        .map(|(algorithm, value)| (algorithm.as_str(), value.as_str().unwrap_or_default()))
        .collect();
    entries.sort_by(|left, right| left.0.cmp(right.0));
    let rows: Vec<ChecksumRow> = entries
        .iter()
        .map(|(algorithm, value)| ChecksumRow {
            algorithm: algorithm.to_uppercase(),
            value: (*value).to_string(),
        })
        .collect();

    let matched = match_checksum(&record.checksums, expected).map(str::to_string);
    let has_expected = !py_strip(expected).is_empty();
    let tooltip = match (&matched, has_expected) {
        (Some(algorithm), _) => format!("与 {} 一致", algorithm.to_uppercase()),
        (None, true) => "与已计算的校验值都不一致".to_string(),
        (None, false) => String::new(),
    };
    ChecksumCardState {
        visible,
        rows,
        matched: ChecksumMatch {
            error: has_expected && matched.is_none(),
            tooltip,
            matched,
        },
    }
}

// ---------------------------------------------------------------------------
// FileListCard
// ---------------------------------------------------------------------------

/// 文件列表一行：`(toDisplayPath(file), toReadableSize(size), _toFileStatusText)`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileListRow {
    /// `task.toDisplayPath(file)`——当前实现就是 `file.relativePath`。
    pub path: String,
    pub size: String,
    pub status: String,
}

/// `FileListCard.refresh()` 的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileListState {
    /// `setVisible(len(files) > 1)`。
    pub visible: bool,
    pub rows: Vec<FileListRow>,
}

/// 镜像 `_toFileStatusText`：未选择 → 未选择；已完成 → 已完成；
/// 否则 `downloadedBytes * 100 // size`（size<=0 时 "--"）。
pub fn file_status_text(selected: bool, completed: bool, size: i64, downloaded: i64) -> String {
    if !selected {
        return "未选择".to_string();
    }
    if completed {
        return "已完成".to_string();
    }
    if size > 0 {
        return format!("{}%", downloaded.saturating_mul(100) / size);
    }
    "--".to_string()
}

/// 镜像 `FileListCard.refresh` 的重算条件：
/// `len(files) > 1 and not (status == _lastStatus and status != RUNNING)`
/// ——RUNNING 时每次刷新都要重算，其余状态只在状态变化时重算。
pub fn file_list_needs_refresh(last_status: Option<TaskStatus>, current: TaskStatus) -> bool {
    !(last_status == Some(current) && current != TaskStatus::Running)
}

/// 镜像 `FileListCard.refresh` 的行内容。
///
/// 两条 Python 守卫要分开看：
/// - `len(files) <= 1` 时 `refresh` 直接 `return`，`setRows` **从不调用**，
///   表格里也就没有行 → 本函数此时返回空 `rows`；
/// - `status == _lastStatus and status != RUNNING` 的短路是**控件缓存**，
///   行内容本身不受影响，故单独暴露为 `file_list_needs_refresh`。
pub fn file_list_card(record: &TaskRecord) -> FileListState {
    let files = record.files.as_deref().unwrap_or(&[]);
    if files.len() <= 1 {
        return FileListState {
            visible: false,
            rows: Vec::new(),
        };
    }
    FileListState {
        visible: true,
        rows: files
            .iter()
            .map(|file| FileListRow {
                path: file.relative_path.clone(),
                size: to_readable_size(file.size as f64),
                status: file_status_text(
                    file.selected,
                    file.completed,
                    file.size,
                    file.downloaded_bytes,
                ),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gd3_core::task_error::TaskError;
    use serde_json::json;

    fn record(json: Value) -> TaskRecord {
        serde_json::from_value(json).expect("TaskRecord 反序列化失败")
    }

    fn base(status: &str) -> serde_json::Value {
        json!({
            "name": "a.bin", "url": "https://example.invalid/1", "packId": "pack",
            "taskId": "tsk_1", "status": status, "fileSize": 4096,
        })
    }

    #[test]
    fn to_time_text_matches_strftime_prior() {
        // 1970-01-01T00:00:00Z
        assert_eq!(to_time_text(0, 0), "1970-01-01 00:00:00");
        // 2026-01-01T00:04:05Z（金文件里的 COMPLETED_AT）
        assert_eq!(to_time_text(1_767_225_845, 0), "2026-01-01 00:04:05");
        // 东八区：+08:00 → 08:04:05。
        assert_eq!(to_time_text(1_767_225_845, 8 * 3600), "2026-01-01 08:04:05");
        // 负时间戳：1969-12-31 00:00:00Z。
        assert_eq!(to_time_text(-86_400, 0), "1969-12-31 00:00:00");
        // 闰日 + 世纪闰年规则。
        assert_eq!(to_time_text(951_782_400, 0), "2000-02-29 00:00:00");
        assert_eq!(to_time_text(1_709_164_800, 0), "2024-02-29 00:00:00");
    }

    #[test]
    fn parse_checksum_takes_first_token_lowercased() {
        assert_eq!(parse_checksum("  ABCDEF  a.bin\n"), "abcdef");
        assert_eq!(parse_checksum(""), "");
        // 纯空白 → `split()` 返回 []，`or [""]` 兜底。
        assert_eq!(parse_checksum(" \t\u{1c}\u{a0}"), "");
        // Python 的空白集含 \x1c-\x1f（Rust is_whitespace 不含）。
        assert_eq!(parse_checksum("\u{1f}DeAdBeEf\u{1e}"), "deadbeef");
    }

    #[test]
    fn match_checksum_walks_dict_order() {
        let checksums: serde_json::Map<String, Value> = serde_json::from_value(json!({
            "sha256": "aa", "md5": "bb",
        }))
        .unwrap();
        assert_eq!(match_checksum(&checksums, "BB"), Some("md5"));
        assert_eq!(match_checksum(&checksums, "bb extra"), Some("md5"));
        assert_eq!(match_checksum(&checksums, "cc"), None);
        assert_eq!(match_checksum(&checksums, ""), None);
    }

    #[test]
    fn header_card_mirrors_refresh() {
        let mut running = base("RUNNING");
        running["steps"] = json!([
            {"stepIndex": 0, "progress": 25.5, "speed": 1024, "receivedBytes": 512},
        ]);
        let state = header_card(&record(running), &DrawerRuntime::default());
        assert_eq!(state.name, "a.bin");
        assert_eq!(state.progress_value, 25, "int() 向零截断");
        assert!(state.progress_visible);
        assert!(!state.progress_error);
        assert_eq!(state.status_text, "1.00 KB/s · 512.00 B / 4.00 KB · 3s");

        // COMPLETED：进度条直接隐藏（与列表卡片的三态不同）。
        let mut completed = base("COMPLETED");
        completed["steps"] = json!([{"stepIndex": 0, "progress": 100.0, "receivedBytes": 4096}]);
        let state = header_card(&record(completed), &DrawerRuntime::default());
        assert!(!state.progress_visible);
        assert_eq!(state.status_text, "已完成");

        // fileSize<=0 时即便 RUNNING 也隐藏。
        let mut nosize = base("RUNNING");
        nosize["fileSize"] = json!(0);
        assert!(!header_card(&record(nosize), &DrawerRuntime::default()).progress_visible);

        // FAILED：setError(True)。
        let failed = base("FAILED");
        assert!(header_card(&record(failed), &DrawerRuntime::default()).progress_error);
    }

    #[test]
    fn error_bar_only_reads_last_error_when_failed() {
        let failed = record(base("FAILED"));
        let runtime = DrawerRuntime {
            card: CardRuntime {
                last_error: Some(TaskError::server_error(500)),
                ..CardRuntime::default()
            },
            ..DrawerRuntime::default()
        };
        let state = error_bar(&failed, &runtime);
        assert!(state.visible);
        assert_eq!(state.title, "下载失败");
        assert_eq!(state.text, "服务器返回了错误（500）");

        // 非 FAILED 一律隐藏（即便带 lastError）。
        let running = record(base("RUNNING"));
        assert!(!error_bar(&running, &runtime).visible);

        // FAILED 但无错误 → 空串隐藏。
        assert!(!error_bar(&failed, &DrawerRuntime::default()).visible);
    }

    #[test]
    fn info_card_lists_six_rows() {
        let mut source = base("COMPLETED");
        source["createdAt"] = json!(1_767_225_845);
        source["completedAt"] = json!(1_767_225_845);
        let runtime = DrawerRuntime {
            output_path: "/downloads/a.bin".into(),
            utc_offset_seconds: 8 * 3600,
            category_enabled: true,
            category_name: Some("视频".into()),
            ..DrawerRuntime::default()
        };
        let state = info_card(&record(source), &runtime);
        let names: Vec<&str> = state.rows.iter().map(|row| row.name).collect();
        assert_eq!(
            names,
            ["url", "path", "size", "category", "created", "completed"]
        );
        let row = |name: &str| state.rows.iter().find(|r| r.name == name).unwrap();
        assert_eq!(row("url").text, "https://example.invalid/1");
        assert_eq!(row("path").text, "/downloads/a.bin");
        assert_eq!(row("size").text, "4.00 KB");
        assert!(row("size").visible);
        assert_eq!(row("category").text, "视频");
        assert!(row("category").visible);
        assert_eq!(row("created").text, "2026-01-01 08:04:05");
        assert_eq!(row("completed").text, "2026-01-01 08:04:05");
        assert!(row("completed").visible);

        // 无分类 / 分类关闭 / 无完成时间 / 总量未知。
        let mut plain = base("WAITING");
        plain["fileSize"] = json!(0);
        let state = info_card(&record(plain), &DrawerRuntime::default());
        let row = |name: &str| state.rows.iter().find(|r| r.name == name).unwrap();
        assert_eq!(row("category").text, "未分类");
        assert!(!row("category").visible, "cfg.isCategoryEnabled 默认 False");
        assert_eq!(row("completed").text, "");
        assert!(!row("completed").visible);
        assert_eq!(row("size").text, "0.00 B");
        assert!(!row("size").visible);
    }

    #[test]
    fn checksum_card_sorts_by_raw_key_and_matches() {
        let mut source = base("COMPLETED");
        // 插入顺序 sha256 → md5；排序后 md5 → sha256。
        source["checksums"] = json!({"sha256": "aa", "md5": "bb"});
        let state = checksum_card(&record(source.clone()), "");
        assert!(state.visible);
        assert_eq!(
            state
                .rows
                .iter()
                .map(|row| (row.algorithm.as_str(), row.value.as_str()))
                .collect::<Vec<_>>(),
            [("MD5", "bb"), ("SHA256", "aa")]
        );
        // 空期望值：不报错、无提示。
        assert!(!state.matched.error);
        assert_eq!(state.matched.tooltip, "");
        assert_eq!(state.matched.matched, None);

        // 命中：tooltip「与 MD5 一致」，不报错。
        let state = checksum_card(&record(source.clone()), " bb\n");
        assert_eq!(state.matched.matched.as_deref(), Some("md5"));
        assert_eq!(state.matched.tooltip, "与 MD5 一致");
        assert!(!state.matched.error);

        // 不命中：报错 + 提示。
        let state = checksum_card(&record(source.clone()), "cc");
        assert!(state.matched.error);
        assert_eq!(state.matched.tooltip, "与已计算的校验值都不一致");

        // 纯空白期望值：不报错。
        let state = checksum_card(&record(source.clone()), "  ");
        assert!(!state.matched.error);
        assert_eq!(state.matched.tooltip, "");

        // 多文件：整卡隐藏。
        source["files"] = json!([
            {"index": 0, "relativePath": "a.bin", "size": 1, "selected": true},
            {"index": 1, "relativePath": "b.bin", "size": 1, "selected": true},
        ]);
        assert!(!checksum_card(&record(source.clone()), "").visible);

        // 无校验值：隐藏。
        let mut empty = base("COMPLETED");
        empty["files"] = json!([
            {"index": 0, "relativePath": "a.bin", "size": 1, "selected": true},
        ]);
        assert!(!checksum_card(&record(empty), "").visible);
    }

    #[test]
    fn checksum_rows_sort_by_raw_key_not_uppercased() {
        // "Sha256" < "sha1"（'S'=0x53 < 's'=0x73），但 upper 后 SHA1 < SHA256。
        let mut source = base("COMPLETED");
        source["checksums"] = json!({"sha1": "aa", "Sha256": "bb"});
        let state = checksum_card(&record(source), "");
        assert_eq!(
            state
                .rows
                .iter()
                .map(|row| row.algorithm.as_str())
                .collect::<Vec<_>>(),
            ["SHA256", "SHA1"],
            "Python sorted() 按原始键排"
        );
    }

    #[test]
    fn file_list_card_mirrors_rows_and_status_text() {
        assert_eq!(file_status_text(false, true, 10, 10), "未选择");
        assert_eq!(file_status_text(true, true, 10, 10), "已完成");
        assert_eq!(file_status_text(true, false, 200, 1), "0%");
        assert_eq!(file_status_text(true, false, 200, 199), "99%");
        assert_eq!(file_status_text(true, false, 0, 5), "--");

        let mut source = base("RUNNING");
        source["files"] = json!([
            {"index": 0, "relativePath": "a.bin", "size": 200, "selected": true,
             "downloadedBytes": 100},
            {"index": 1, "relativePath": "b/c.bin", "size": 0, "selected": false},
        ]);
        let state = file_list_card(&record(source));
        assert!(state.visible);
        assert_eq!(
            state.rows,
            vec![
                FileListRow {
                    path: "a.bin".into(),
                    size: "200.00 B".into(),
                    status: "50%".into(),
                },
                FileListRow {
                    path: "b/c.bin".into(),
                    size: "0.00 B".into(),
                    status: "未选择".into(),
                },
            ]
        );

        // 单文件 / 空列表：隐藏，且 `setRows` 从不调用（Python 提前 return）。
        let single = record(base("WAITING"));
        assert_eq!(
            file_list_card(&single),
            FileListState {
                visible: false,
                rows: Vec::new(),
            }
        );
    }

    #[test]
    fn file_list_refresh_condition_mirrors_python_guard() {
        // RUNNING 每次都重算。
        assert!(file_list_needs_refresh(
            Some(TaskStatus::Running),
            TaskStatus::Running
        ));
        // 状态变了就重算。
        assert!(file_list_needs_refresh(
            Some(TaskStatus::Waiting),
            TaskStatus::Completed
        ));
        assert!(file_list_needs_refresh(None, TaskStatus::Completed));
        // 状态没变且非 RUNNING：跳过。
        assert!(!file_list_needs_refresh(
            Some(TaskStatus::Completed),
            TaskStatus::Completed
        ));
    }

    /// 状态字面量（`TaskStatus` 的 serde 表示与 Python 的 `.name` 一致）。
    fn status_from_name(name: &str) -> TaskStatus {
        match name {
            "WAITING" => TaskStatus::Waiting,
            "RUNNING" => TaskStatus::Running,
            "PAUSED" => TaskStatus::Paused,
            "COMPLETED" => TaskStatus::Completed,
            "FAILED" => TaskStatus::Failed,
            other => panic!("未知状态：{other}"),
        }
    }

    /// 金文件一条 case → (`DrawerRuntime`, `FileListCard._lastStatus`)。
    ///
    /// `lastStatus` 是控件缓存状态（Python `_lastStatus`），只用来判
    /// `file_list_needs_refresh`，不进 `DrawerRuntime`。
    fn runtime_from_json(case: &Value) -> (DrawerRuntime, Option<TaskStatus>) {
        let runtime = &case["runtime"];
        let last_error = runtime
            .get("lastError")
            .filter(|value| !value.is_null())
            .map(|value| {
                let message = value["message"].as_str().expect("message 缺失");
                // `TaskError.message` 是 &'static str：测试进程内泄漏可忽略。
                let leaked: &'static str = Box::leak(message.to_string().into_boxed_str());
                let params = value["params"]
                    .as_object()
                    .map(|map| {
                        map.iter()
                            .map(|(key, param)| {
                                let leaked_key: &'static str =
                                    Box::leak(key.clone().into_boxed_str());
                                (leaked_key, param.as_str().unwrap_or("").to_string())
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                TaskError::new(leaked, params)
            });
        let card = CardRuntime {
            is_seeding: runtime["isSeeding"].as_bool().unwrap_or(false),
            has_output_file: runtime["hasOutputFile"].as_bool().unwrap_or(true),
            output_exists: runtime["outputExists"].as_bool().unwrap_or(true),
            last_error,
            ..CardRuntime::default()
        };
        let last_status = runtime
            .get("lastStatus")
            .and_then(Value::as_str)
            .map(status_from_name);
        (
            DrawerRuntime {
                card,
                output_path: runtime["outputPath"].as_str().unwrap_or("").to_string(),
                utc_offset_seconds: runtime["utcOffsetSeconds"].as_i64().unwrap_or(0),
                category_enabled: runtime["categoryEnabled"].as_bool().unwrap_or(false),
                category_name: runtime["categoryName"].as_str().map(str::to_string),
                expected_checksum: runtime["expectedChecksum"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
            },
            last_status,
        )
    }

    /// 抽屉派生层与 CPython 逐条对比（`drawer.jsonl`）：金文件由
    /// `generate_drawer_fixture.py` 跑真 `HeaderCard.refresh()` /
    /// `ErrorBar.refresh()` / `InfoCard.refresh()` / `ChecksumCard.refresh()` +
    /// `_refreshMatch()` / `FileListCard.refresh()`（widget 全换记录型桩）生成。
    #[test]
    fn drawer_layer_matches_python_fixture() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/drawer.jsonl");
        let text = std::fs::read_to_string(&path).expect("drawer.jsonl 缺失");
        let mut cases = 0usize;
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let case: Value = serde_json::from_str(line).expect("金文件行不是合法 JSON");
            let label = case["label"].as_str().unwrap_or("");
            let record: TaskRecord =
                serde_json::from_value(case["record"].clone()).expect("record 反序列化失败");
            let (runtime, last_status) = runtime_from_json(&case);
            let want = &case["want"];

            match case["card"].as_str().expect("card 缺失") {
                "header" => {
                    let got = header_card(&record, &runtime);
                    assert_eq!(
                        got.name,
                        want["name"].as_str().unwrap_or(""),
                        "名称：{label}"
                    );
                    assert_eq!(
                        got.progress_value,
                        want["progressValue"].as_i64().unwrap_or(0),
                        "进度值：{label}"
                    );
                    assert_eq!(
                        got.progress_visible,
                        want["progressVisible"].as_bool().unwrap_or(false),
                        "进度条可见：{label}"
                    );
                    assert_eq!(
                        got.progress_error,
                        want["progressError"].as_bool().unwrap_or(false),
                        "进度条错误态：{label}"
                    );
                    assert_eq!(
                        got.status_text,
                        want["statusText"].as_str().unwrap_or(""),
                        "状态行：{label}"
                    );
                }
                "errorBar" => {
                    let got = error_bar(&record, &runtime);
                    assert_eq!(
                        got.visible,
                        want["visible"].as_bool().unwrap_or(false),
                        "错误条可见：{label}"
                    );
                    assert_eq!(
                        got.text,
                        want["text"].as_str().unwrap_or(""),
                        "错误条正文：{label}"
                    );
                    // 标题只在有条时才有意义（Python 无正文就不建 InfoBar）。
                    if got.visible {
                        assert_eq!(
                            got.title,
                            want["title"].as_str().unwrap_or(""),
                            "标题：{label}"
                        );
                    }
                }
                "info" => {
                    let got = info_card(&record, &runtime);
                    let rows = want["rows"].as_array().expect("rows 必须是数组");
                    assert_eq!(got.rows.len(), rows.len(), "信息行数：{label}");
                    for (row, expected) in got.rows.iter().zip(rows) {
                        assert_eq!(
                            row.name,
                            expected["name"].as_str().unwrap_or(""),
                            "行名：{label}"
                        );
                        assert_eq!(
                            row.caption,
                            expected["caption"].as_str().unwrap_or(""),
                            "行标题（{}）：{label}",
                            row.name
                        );
                        assert_eq!(
                            row.text,
                            expected["text"].as_str().unwrap_or(""),
                            "行值（{}）：{label}",
                            row.name
                        );
                        assert_eq!(
                            row.visible,
                            expected["visible"].as_bool().unwrap_or(false),
                            "行可见性（{}）：{label}",
                            row.name
                        );
                    }
                }
                "checksum" => {
                    let got = checksum_card(&record, &runtime.expected_checksum);
                    assert_eq!(
                        got.visible,
                        want["visible"].as_bool().unwrap_or(false),
                        "校验卡可见：{label}"
                    );
                    let rows = want["rows"].as_array().expect("rows 必须是数组");
                    assert_eq!(got.rows.len(), rows.len(), "校验行数：{label}");
                    for (row, expected) in got.rows.iter().zip(rows) {
                        assert_eq!(
                            row.algorithm,
                            expected["algorithm"].as_str().unwrap_or(""),
                            "算法名：{label}"
                        );
                        assert_eq!(
                            row.value,
                            expected["value"].as_str().unwrap_or(""),
                            "校验值（{}）：{label}",
                            row.algorithm
                        );
                    }
                    let matched = &want["matched"];
                    assert_eq!(
                        got.matched.matched.as_deref(),
                        matched["matched"].as_str(),
                        "命中算法：{label}"
                    );
                    assert_eq!(
                        got.matched.error,
                        matched["error"].as_bool().unwrap_or(false),
                        "输入框报错：{label}"
                    );
                    assert_eq!(
                        got.matched.tooltip,
                        matched["tooltip"].as_str().unwrap_or(""),
                        "输入框提示：{label}"
                    );
                }
                "fileList" => {
                    let got = file_list_card(&record);
                    assert_eq!(
                        got.visible,
                        want["visible"].as_bool().unwrap_or(false),
                        "文件列表可见：{label}"
                    );
                    let rows = want["rows"].as_array().expect("rows 必须是数组");
                    assert_eq!(got.rows.len(), rows.len(), "文件行数：{label}");
                    for (row, expected) in got.rows.iter().zip(rows) {
                        assert_eq!(
                            row.path,
                            expected["path"].as_str().unwrap_or(""),
                            "文件路径：{label}"
                        );
                        assert_eq!(
                            row.size,
                            expected["size"].as_str().unwrap_or(""),
                            "文件大小（{}）：{label}",
                            row.path
                        );
                        assert_eq!(
                            row.status,
                            expected["status"].as_str().unwrap_or(""),
                            "文件状态（{}）：{label}",
                            row.path
                        );
                    }
                    assert_eq!(
                        file_list_needs_refresh(last_status, record.status),
                        want["needsRefresh"].as_bool().unwrap_or(false),
                        "重算条件：{label}"
                    );
                }
                other => panic!("未知卡片：{other}（{label}）"),
            }
            cases += 1;
        }
        // 防「金文件被清空/被裁剪仍全绿」。
        assert_eq!(cases, 48, "金文件条数变化需同步更新断言");
    }
}
