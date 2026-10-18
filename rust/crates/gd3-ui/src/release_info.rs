//! 发行说明对话框（P3，第 68 步）：镜像
//! `app/view/dialogs/release_info.py`（123 行）+ `app/update.py::assetScore`
//! /`bestAsset`（:50-120）的语义层。
//!
//! ## 实证口径（2026-10-18 实读源码）
//!
//! - `yesButton.setText(self.tr("下载"))`（:44）；**`cancelButton` 不隐藏**，
//!   保持 `MessageBoxBase` 默认的 `取消`（源码里没有 `setVisible(False)`，
//!   第 62 步实测确认）。
//! - `widget.setMinimumWidth(min(580, self.width() - 48))`（:43）——先按
//!   窗口宽减 48 取小；资产表非空时再 `max(旧值, needed + 48 + 20)`（:98），
//!   其中 `needed` 是 Qt `sizeHintForColumn` 三列之和，本模块把它当入参。
//! - `dateLabel = CaptionLabel(release.publishedAt[:10] if release.publishedAt
//!   else "")`（:35）——空串就空串，**不补占位符**；`[:10]` 是码点切片。
//! - `prereleaseLabel.setVisible(self._release.prerelease)`（:46）；
//!   `descriptionEdit.setMarkdown(self._release.body or self.tr("暂无更新说明"))`
//!   （:52）——`or` 语义：空串才回退。
//! - `assetView.setVisible(bool(self._release.assets))`（:85）；表头
//!   `["文件名", "大小", "下载次数"]`（:67）；第二列 `toReadableSize(asset.size)`，
//!   第三列 `str(asset.downloadCount)`（:70-74）。
//! - 初始选中：`bestAsset(release)` 非空时按 **对象同一性**（`item.data(UserRole)
//!   is best`，:89-95）选中对应行 → 行序与 `assets` 序一致，故等价于
//!   [`best_asset_index`]。`validate()` = 当前有选中项（:122）。
//! - `detailButton` → `QDesktopServices.openUrl(release.pageUrl)`（:117）；
//!   `sponsorButton` → `DONATE_URL`（`app/config/constants.py:9`）。
//!
//! ## `assetScore` 的精确口径（`app/update.py:60-120`）
//!
//! 1. `lower = name.lower()`；
//! 2. 平台关键词按序加权 `40 - i*10`，取**最大值**（win32 且系统 < Win10 时
//!    前缀 `windows7`，于是 `windows7` 得 40、`windows` 得 30；否则 `windows`
//!    得 40）；
//! 3. 架构关键词是**硬门槛**（任一子串命中即可），按 `platform.machine().lower()`
//!    归一成三组别名，未知机器退化成 `[machine]` 自身；
//! 4. 平台分 + 20 后按平台加分：win32 `setup`+`.exe` +100 / `.msi` +90 /
//!    `.zip` +20；darwin `.dmg` +100 / `.pkg` +90 / `.zip` +30；android
//!    `.apk` +100；其余 `.appimage` +100 / `.deb`|`.rpm` +90 / `.tar.xz` +80 /
//!    `.tar.gz`|`.zip` +50；
//! 5. 平台分 0 或架构不命中 → `-1`。
//!
//! `bestAsset` 用 `score > bestScore`（`bestScore` 初值 `-1`）取最大、
//! **同分取先出现者**；若最终 `bestScore < 0` 返回 `None`（:54-60）。
//!
//! ## 与 Qt 的差异
//!
//! `QDesktopServices.openUrl`、`QTextCursor.MoveOperation.Start`（把说明滚到
//! 顶部）都是外壳副作用，本模块只产出 URL 与 [`ReleaseInfoState::description_scrolls_to_start`]
//! 标记；`sizeHintForColumn` 的实测宽度由 Slint 侧测量后作为 `needed` 传入。

use crate::format::to_readable_size;

// ---------------------------------------------------------------------------
// 文案与常量（逐字）
// ---------------------------------------------------------------------------

/// `app/config/constants.py:9`
pub const DONATE_URL: &str = "https://gd.xychr.com/docs/donate/";
/// `yesButton.setText(self.tr("下载"))`（release_info.py:44）
pub const BUTTON_DOWNLOAD: &str = "下载";
/// `cancelButton` 保持 `MessageBoxBase` 默认文案（源码未覆盖）
pub const BUTTON_CANCEL: &str = "取消";
/// `CaptionLabel(self.tr("⚠️ 预发布版本"))`（:36）
pub const PRERELEASE_LABEL: &str = "⚠️ 预发布版本";
/// `detailButton.setToolTip(self.tr("打开发布页"))`（:47）
pub const TOOLTIP_DETAIL: &str = "打开发布页";
/// `sponsorButton.setToolTip(self.tr("赞助作者"))`（:50）
pub const TOOLTIP_SPONSOR: &str = "赞助作者";
/// `setMarkdown(body or self.tr("暂无更新说明"))`（:52）
pub const EMPTY_BODY: &str = "暂无更新说明";
/// `setHorizontalHeaderLabels([...])`（:67）
pub const ASSET_HEADERS: [&str; 3] = ["文件名", "大小", "下载次数"];
/// `Release.fromResponse` 的版本兜底（sources.py:94）
pub const UNKNOWN_VERSION: &str = "Unknown";

/// `min(580, self.width() - 48)`（:43）
pub const BASE_MIN_WIDTH: i32 = 580;
/// 同上：窗口宽度的减数
pub const MIN_WIDTH_MARGIN: i32 = 48;
/// `needed + 48 + 20`（:98）
pub const ASSET_WIDTH_EXTRA: i32 = 48 + 20;
/// `titleLayout.setSpacing(6)`（:104）
pub const TITLE_SPACING: i32 = 6;
/// `viewLayout.addSpacing(12)`（:112）
pub const SPACING_AFTER_TITLE: i32 = 12;
/// `MarkdownViewer(minimumVisibleLines=5, maximumVisibleLines=16)`（:39）
pub const DESCRIPTION_MIN_LINES: u32 = 5;
/// 同上
pub const DESCRIPTION_MAX_LINES: u32 = 16;
/// `AutoSizingTreeView(minimumVisibleRows=1, maximumVisibleRows=6)`（:40）
pub const ASSET_MIN_ROWS: u32 = 1;
/// 同上
pub const ASSET_MAX_ROWS: u32 = 6;

// ---------------------------------------------------------------------------
// `app/sources.py` 的两个 dataclass
// ---------------------------------------------------------------------------

/// `app/sources.py:58` `ReleaseAsset`
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReleaseAsset {
    pub name: String,
    pub size: i64,
    pub download_count: i64,
    pub download_url: String,
}

/// `app/sources.py:66` `Release`
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub published_at: String,
    pub body: String,
    pub page_url: String,
    pub prerelease: bool,
    pub assets: Vec<ReleaseAsset>,
}

impl Release {
    /// `Release.fromResponse`（sources.py:74-100）：
    /// `tag_name` → `name` 取首个 strip 后非空者为版本；`published_at` 空则
    /// 退 `created_at`；`body` 空串兜底；`version` 空兜 `"Unknown"`。
    pub fn from_response(data: &serde_json::Value) -> Self {
        let mut version = String::new();
        for key in ["tag_name", "name"] {
            let value = data
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if !value.is_empty() {
                version = value;
                break;
            }
        }

        let assets = data
            .get("assets")
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|a| ReleaseAsset {
                        name: a
                            .get("name")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        size: a
                            .get("size")
                            .and_then(serde_json::Value::as_i64)
                            .unwrap_or(0),
                        download_count: a
                            .get("download_count")
                            .and_then(serde_json::Value::as_i64)
                            .unwrap_or(0),
                        download_url: a
                            .get("browser_download_url")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let published_at = {
            let primary = data
                .get("published_at")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if primary.is_empty() {
                data.get("created_at")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string()
            } else {
                primary.to_string()
            }
        };

        Self {
            version: if version.is_empty() {
                UNKNOWN_VERSION.to_string()
            } else {
                version
            },
            published_at,
            body: data
                .get("body")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string(),
            page_url: data
                .get("html_url")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string(),
            prerelease: data
                .get("prerelease")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            assets,
        }
    }

    /// `dateLabel` 的正文：`publishedAt[:10] if publishedAt else ""`（:35）。
    pub fn published_date(&self) -> String {
        self.published_at.chars().take(10).collect()
    }

    /// `descriptionEdit.setMarkdown(body or "暂无更新说明")`（:52）。
    pub fn description_markdown(&self) -> &str {
        if self.body.is_empty() {
            EMPTY_BODY
        } else {
            &self.body
        }
    }
}

// ---------------------------------------------------------------------------
// `app/update.py::assetScore`
// ---------------------------------------------------------------------------

/// `sys.platform` / `IS_ANDROID` 的三态（`isLessThanWin10()` 决定 `windows7`
/// 关键词是否参与加权）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    /// `sys.platform == "win32"`；`legacy` = `isLessThanWin10()`
    Windows { legacy: bool },
    /// `sys.platform == "darwin"`
    MacOs,
    /// `app.platform.android.IS_ANDROID`
    Android,
    /// 其余（`sys.platform` 非 win32/darwin 且非 android）
    Linux,
}

impl Platform {
    /// `platformKw`（update.py:65-73），按声明顺序参与 `40 - i*10` 加权。
    pub fn keywords(self) -> Vec<&'static str> {
        match self {
            Self::Windows { legacy: true } => vec!["windows7", "windows"],
            Self::Windows { legacy: false } => vec!["windows"],
            Self::MacOs => vec!["macos", "darwin", "mac"],
            Self::Android => vec!["android"],
            Self::Linux => vec!["linux"],
        }
    }
}

/// `archKw`（update.py:76-84）：`machine` 已是 `platform.machine().lower()`。
pub fn arch_keywords(machine: &str) -> Vec<String> {
    let group: Option<&[&str]> = match machine {
        "amd64" | "x86_64" => Some(&["x86_64", "amd64", "x64"]),
        "arm64" | "aarch64" => Some(&["arm64", "aarch64"]),
        "x86" | "i386" | "i686" => Some(&["x86", "i386", "i686"]),
        _ => None,
    };
    match group {
        Some(words) => words.iter().map(|w| (*w).to_string()).collect(),
        None if machine.is_empty() => Vec::new(),
        // 未知机器退化成 `[machine]` 自身（Python 同一分支）。
        None => vec![machine.to_string()],
    }
}

/// `assetScore(name)`（update.py:60-120）。`machine` 需已小写。
pub fn asset_score(name: &str, platform: Platform, machine: &str) -> i32 {
    let lower = name.to_lowercase();

    let mut platform_score = 0;
    for (i, kw) in platform.keywords().iter().enumerate() {
        if lower.contains(kw) {
            platform_score = platform_score.max(40 - i as i32 * 10);
        }
    }

    let arch = arch_keywords(machine);
    if platform_score == 0 || !arch.iter().any(|kw| lower.contains(kw.as_str())) {
        return -1;
    }

    let mut score = platform_score + 20;
    match platform {
        Platform::Windows { .. } => {
            if lower.contains("setup") && lower.ends_with(".exe") {
                score += 100;
            } else if lower.ends_with(".msi") {
                score += 90;
            } else if lower.ends_with(".zip") {
                score += 20;
            }
        }
        Platform::MacOs => {
            if lower.ends_with(".dmg") {
                score += 100;
            } else if lower.ends_with(".pkg") {
                score += 90;
            } else if lower.ends_with(".zip") {
                score += 30;
            }
        }
        Platform::Android => {
            if lower.ends_with(".apk") {
                score += 100;
            }
        }
        Platform::Linux => {
            if lower.ends_with(".appimage") {
                score += 100;
            } else if lower.ends_with(".deb") || lower.ends_with(".rpm") {
                score += 90;
            } else if lower.ends_with(".tar.xz") {
                score += 80;
            } else if lower.ends_with(".tar.gz") || lower.ends_with(".zip") {
                score += 50;
            }
        }
    }
    score
}

/// `bestAsset(release)` 的行号（`best if bestScore >= 0 else None`，:54-60）。
/// 同分取先出现者。
pub fn best_asset_index(release: &Release, platform: Platform, machine: &str) -> Option<usize> {
    let mut best: Option<usize> = None;
    let mut best_score = -1;
    for (i, asset) in release.assets.iter().enumerate() {
        let score = asset_score(&asset.name, platform, machine);
        if score > best_score {
            best = Some(i);
            best_score = score;
        }
    }
    if best_score >= 0 { best } else { None }
}

/// `bestAsset(release)` 的对象本身。
pub fn best_asset<'a>(
    release: &'a Release,
    platform: Platform,
    machine: &str,
) -> Option<&'a ReleaseAsset> {
    best_asset_index(release, platform, machine).map(|i| &release.assets[i])
}

// ---------------------------------------------------------------------------
// 对话框状态
// ---------------------------------------------------------------------------

/// `assetModel` 的一行（三列文本，`QStandardItem(asset)` 走 UserRole 由索引承担）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetRow {
    pub name: String,
    pub size_text: String,
    pub download_count_text: String,
}

/// `ReleaseInfoDialog` 的可断言状态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseInfoState {
    release: Release,
    rows: Vec<AssetRow>,
    selected: Option<usize>,
}

impl ReleaseInfoState {
    /// `__init__` + `_initWidget`：建行、按 `bestAsset` 预选、算可见性。
    pub fn new(release: Release, platform: Platform, machine: &str) -> Self {
        let rows = release
            .assets
            .iter()
            .map(|asset| AssetRow {
                name: asset.name.clone(),
                size_text: to_readable_size(asset.size as f64),
                download_count_text: asset.download_count.to_string(),
            })
            .collect();
        let selected = best_asset_index(&release, platform, machine);
        Self {
            release,
            rows,
            selected,
        }
    }

    pub fn release(&self) -> &Release {
        &self.release
    }

    pub fn rows(&self) -> &[AssetRow] {
        &self.rows
    }

    /// `assetView.setVisible(bool(self._release.assets))`（:85）。
    pub fn assets_visible(&self) -> bool {
        !self.release.assets.is_empty()
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.selected
    }

    /// `assetView.setCurrentIndex(...)`：越界索引在 Qt 里等于「无选中」。
    pub fn set_selected(&mut self, index: Option<usize>) {
        self.selected = index.filter(|i| *i < self.release.assets.len());
    }

    /// `selectedAsset()`（:113-120）。
    pub fn selected_asset(&self) -> Option<&ReleaseAsset> {
        self.selected.map(|i| &self.release.assets[i])
    }

    /// `validate()`（:122-123）。
    pub fn validate(&self) -> bool {
        self.selected_asset().is_some()
    }

    /// `descriptionEdit.setMarkdown(...)` 的正文。
    pub fn description_markdown(&self) -> &str {
        self.release.description_markdown()
    }

    /// `descriptionEdit.moveCursor(QTextCursor.MoveOperation.Start)`（:53）——
    /// 外壳据此把滚动位置归零。
    pub fn description_scrolls_to_start(&self) -> bool {
        true
    }

    pub fn published_date(&self) -> String {
        self.release.published_date()
    }

    /// `prereleaseLabel.setVisible(self._release.prerelease)`（:46）。
    pub fn prerelease_visible(&self) -> bool {
        self.release.prerelease
    }

    /// `detailButton` 的目标（:117）。
    pub fn detail_url(&self) -> &str {
        &self.release.page_url
    }

    /// `sponsorButton` 的目标（:118）。
    pub fn sponsor_url(&self) -> &'static str {
        DONATE_URL
    }

    /// `widget.setMinimumWidth(...)` 的两段式（:43 + :98）。
    /// `available_width` = `self.width()`，`needed` = 三列
    /// `sizeHintForColumn` 之和（Qt 侧测量值）。
    pub fn min_width(&self, available_width: i32, needed: i32) -> i32 {
        let base = BASE_MIN_WIDTH.min(available_width - MIN_WIDTH_MARGIN);
        if self.release.assets.is_empty() {
            base
        } else {
            base.max(needed + ASSET_WIDTH_EXTRA)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn asset(name: &str, size: i64, count: i64) -> ReleaseAsset {
        ReleaseAsset {
            name: name.to_string(),
            size,
            download_count: count,
            download_url: format!("https://example.invalid/{name}"),
        }
    }

    fn release(assets: Vec<ReleaseAsset>) -> Release {
        Release {
            version: "v3.6.0".into(),
            published_at: "2026-10-01T12:34:56Z".into(),
            body: "## 更新内容\n- 修了个 bug".into(),
            page_url: "https://github.com/XiaoYouChR/Ghost-Downloader-3/releases/tag/v3.6.0".into(),
            prerelease: false,
            assets,
        }
    }

    #[test]
    fn literals_match_python() {
        assert_eq!(DONATE_URL, "https://gd.xychr.com/docs/donate/");
        assert_eq!(BUTTON_DOWNLOAD, "下载");
        assert_eq!(BUTTON_CANCEL, "取消");
        assert_eq!(PRERELEASE_LABEL, "⚠️ 预发布版本");
        assert_eq!(TOOLTIP_DETAIL, "打开发布页");
        assert_eq!(TOOLTIP_SPONSOR, "赞助作者");
        assert_eq!(EMPTY_BODY, "暂无更新说明");
        assert_eq!(ASSET_HEADERS, ["文件名", "大小", "下载次数"]);
        assert_eq!(
            (
                BASE_MIN_WIDTH,
                MIN_WIDTH_MARGIN,
                ASSET_WIDTH_EXTRA,
                TITLE_SPACING,
                SPACING_AFTER_TITLE
            ),
            (580, 48, 68, 6, 12)
        );
        assert_eq!(
            (
                DESCRIPTION_MIN_LINES,
                DESCRIPTION_MAX_LINES,
                ASSET_MIN_ROWS,
                ASSET_MAX_ROWS
            ),
            (5, 16, 1, 6)
        );
    }

    /// `fromResponse`：`tag_name` 优先，空则 `name`，再空兜 `"Unknown"`。
    #[test]
    fn from_response_prefers_tag_name_then_name_then_unknown() {
        let tagged = Release::from_response(&json!({"tag_name": "  v3.6.0 ", "name": "ignored"}));
        assert_eq!(tagged.version, "v3.6.0");
        let named = Release::from_response(&json!({"tag_name": "   ", "name": "v3.5.1"}));
        assert_eq!(named.version, "v3.5.1");
        let empty = Release::from_response(&json!({}));
        assert_eq!(empty.version, "Unknown");
    }

    /// `published_at` 空则退 `created_at`；body/html_url/prerelease 缺省值。
    #[test]
    fn from_response_falls_back_to_created_at_and_defaults() {
        let data = json!({
            "tag_name": "v3.6.0",
            "published_at": "",
            "created_at": "2026-09-30T00:00:00Z",
            "prerelease": true,
            "html_url": "https://example.invalid/r",
        });
        let release = Release::from_response(&data);
        assert_eq!(release.published_at, "2026-09-30T00:00:00Z");
        assert_eq!(release.body, "");
        assert!(release.prerelease);
        assert_eq!(release.page_url, "https://example.invalid/r");
        assert!(release.assets.is_empty());
    }

    /// assets 逐项映射，缺键取 `name=""` / `size=0` / `download_count=0`。
    #[test]
    fn from_response_maps_assets() {
        let data = json!({
            "tag_name": "v3.6.0",
            "assets": [
                {"name": "app.AppImage", "size": 1048576, "download_count": 12,
                 "browser_download_url": "https://example.invalid/a"},
                {"name": "partial"}
            ]
        });
        let release = Release::from_response(&data);
        assert_eq!(release.assets.len(), 2);
        assert_eq!(release.assets[0].name, "app.AppImage");
        assert_eq!(release.assets[0].size, 1_048_576);
        assert_eq!(release.assets[0].download_count, 12);
        assert_eq!(release.assets[0].download_url, "https://example.invalid/a");
        assert_eq!(release.assets[1].size, 0);
        assert_eq!(release.assets[1].download_count, 0);
        assert_eq!(release.assets[1].download_url, "");
    }

    /// `publishedAt[:10]`：长串截 10 个码点，短串原样，空串空串。
    #[test]
    fn published_date_slices_ten_code_points() {
        let mut r = release(vec![]);
        assert_eq!(r.published_date(), "2026-10-01");
        r.published_at = "2026".into();
        assert_eq!(r.published_date(), "2026");
        r.published_at = String::new();
        assert_eq!(r.published_date(), "");
        // 码点而非字节：10 个汉字不该被截断在半个字节上。
        r.published_at = "一二三四五六七八九十十一".into();
        assert_eq!(r.published_date(), "一二三四五六七八九十");
    }

    /// `body or "暂无更新说明"`。
    #[test]
    fn empty_body_falls_back_to_placeholder() {
        let mut r = release(vec![]);
        assert_eq!(r.description_markdown(), "## 更新内容\n- 修了个 bug");
        r.body = String::new();
        assert_eq!(r.description_markdown(), "暂无更新说明");
    }

    /// Linux 平台分：关键词 40 + 20，再按扩展名加分。
    #[test]
    fn linux_asset_scores() {
        let p = Platform::Linux;
        assert_eq!(asset_score("app-linux-x86_64.AppImage", p, "x86_64"), 160);
        assert_eq!(asset_score("app_linux_amd64.deb", p, "amd64"), 150);
        assert_eq!(asset_score("app-linux-x64.rpm", p, "x86_64"), 150);
        assert_eq!(asset_score("app-linux-x64.tar.xz", p, "x86_64"), 140);
        assert_eq!(asset_score("app-linux-x64.tar.gz", p, "x86_64"), 110);
        assert_eq!(asset_score("app-linux-x64.zip", p, "x86_64"), 110);
        // 命中平台但扩展名不认 → 只有基础分 60。
        assert_eq!(asset_score("app-linux-x64.bin", p, "x86_64"), 60);
        // 架构门槛：aarch64 机器不认 x64 资产。
        assert_eq!(asset_score("app-linux-x64.AppImage", p, "aarch64"), -1);
        assert_eq!(asset_score("app-linux-arm64.AppImage", p, "aarch64"), 160);
        // 平台不命中 → -1。
        assert_eq!(asset_score("app-windows-x64.exe", p, "x86_64"), -1);
    }

    /// Windows：`windows7` 关键词在 legacy 下才加权；`setup*.exe` 优先于 msi。
    #[test]
    fn windows_asset_scores_respect_legacy_flag() {
        let modern = Platform::Windows { legacy: false };
        assert_eq!(
            asset_score("App-windows-x64-setup.exe", modern, "amd64"),
            160
        );
        assert_eq!(asset_score("App-windows-x64.msi", modern, "amd64"), 150);
        assert_eq!(asset_score("App-windows-x64.zip", modern, "amd64"), 80);
        assert_eq!(asset_score("App-windows-x64.7z", modern, "amd64"), 60);
        // 非 legacy 时 `windows7` 不再给 40，但 `windows` 子串仍命中 → 40。
        assert_eq!(
            asset_score("App-windows7-x64-setup.exe", modern, "amd64"),
            160
        );

        let legacy = Platform::Windows { legacy: true };
        // `windows7` 得 40 + 20 + 100；`windows` 得 30 + 20 + 100。
        assert_eq!(
            asset_score("App-windows7-x64-setup.exe", legacy, "amd64"),
            160
        );
        assert_eq!(
            asset_score("App-windows-x64-setup.exe", legacy, "amd64"),
            150
        );
        // 无 `setup` 的裸 .exe 不加分（`"setup" in lower` 是硬条件）。
        assert_eq!(asset_score("App-windows-x64.exe", modern, "amd64"), 60);
    }

    /// macOS / Android 的扩展名加分。
    #[test]
    fn macos_and_android_asset_scores() {
        let mac = Platform::MacOs;
        assert_eq!(asset_score("App-macos-arm64.dmg", mac, "arm64"), 160);
        assert_eq!(asset_score("App-darwin-x64.pkg", mac, "x86_64"), 140);
        assert_eq!(asset_score("App-mac-x64.zip", mac, "x86_64"), 70);
        // `mac` 是第三个关键词 → 40 - 2*10 = 20；20 + 20 + 100。
        assert_eq!(asset_score("App-mac-arm64.dmg", mac, "arm64"), 140);

        let android = Platform::Android;
        assert_eq!(asset_score("App-android-arm64.apk", android, "arm64"), 160);
        assert_eq!(asset_score("App-android-arm64.zip", android, "arm64"), 60);
        assert_eq!(asset_score("App-linux-arm64.apk", android, "arm64"), -1);
    }

    /// 未知机器退化成 `[machine]` 自身；空机器则架构门槛永远不命中。
    #[test]
    fn arch_keywords_fall_back_to_machine_itself() {
        assert_eq!(arch_keywords("riscv64"), vec!["riscv64".to_string()]);
        assert!(arch_keywords("").is_empty());
        assert_eq!(
            arch_keywords("x86_64"),
            vec!["x86_64".to_string(), "amd64".into(), "x64".into()]
        );
        assert_eq!(
            asset_score("app-linux-riscv64.AppImage", Platform::Linux, "riscv64"),
            160
        );
        assert_eq!(
            asset_score("app-linux-riscv64.AppImage", Platform::Linux, ""),
            -1
        );
    }

    /// `bestAsset`：同分取先出现者；全为 -1 时返回 None。
    #[test]
    fn best_asset_picks_first_highest_score() {
        let r = release(vec![
            asset("app-linux-x64.zip", 1, 0),
            asset("app-linux-x64.AppImage", 2, 0),
            asset("app-linux-x64.AppImage.asc", 3, 0),
        ]);
        assert_eq!(best_asset_index(&r, Platform::Linux, "x86_64"), Some(1));
        assert_eq!(
            best_asset(&r, Platform::Linux, "x86_64").map(|a| a.name.as_str()),
            Some("app-linux-x64.AppImage")
        );

        let tie = release(vec![
            asset("app-linux-x64.AppImage", 1, 0),
            asset("app-linux-amd64.AppImage", 2, 0),
        ]);
        assert_eq!(best_asset_index(&tie, Platform::Linux, "x86_64"), Some(0));

        let none = release(vec![asset("app-windows-x64.zip", 1, 0)]);
        assert_eq!(best_asset_index(&none, Platform::Linux, "x86_64"), None);
        assert!(best_asset(&none, Platform::Linux, "x86_64").is_none());

        let empty = release(vec![]);
        assert_eq!(best_asset_index(&empty, Platform::Linux, "x86_64"), None);
    }

    /// 行文本：`toReadableSize(size)` + `str(download_count)`，顺序同 assets。
    #[test]
    fn rows_mirror_model_text() {
        let state = ReleaseInfoState::new(
            release(vec![
                asset("app-linux-x64.AppImage", 1_048_576, 12),
                asset("app-linux-x64.deb", 0, 0),
            ]),
            Platform::Linux,
            "x86_64",
        );
        assert_eq!(state.rows().len(), 2);
        assert_eq!(state.rows()[0].name, "app-linux-x64.AppImage");
        assert_eq!(state.rows()[0].size_text, "1.00 MB");
        assert_eq!(state.rows()[0].download_count_text, "12");
        assert_eq!(state.rows()[1].size_text, "0.00 B");
        assert_eq!(state.rows()[1].download_count_text, "0");
    }

    /// 初始选中 = `bestAsset` 行；无资产则不可确认。
    #[test]
    fn initial_selection_is_best_asset_and_validate_follows() {
        let state = ReleaseInfoState::new(
            release(vec![
                asset("app-linux-x64.zip", 1, 0),
                asset("app-linux-x64.AppImage", 2, 0),
            ]),
            Platform::Linux,
            "x86_64",
        );
        assert!(state.assets_visible());
        assert_eq!(state.selected_index(), Some(1));
        assert_eq!(
            state.selected_asset().map(|a| a.name.as_str()),
            Some("app-linux-x64.AppImage")
        );
        assert!(state.validate());

        let empty = ReleaseInfoState::new(release(vec![]), Platform::Linux, "x86_64");
        assert!(!empty.assets_visible());
        assert_eq!(empty.selected_index(), None);
        assert!(!empty.validate());
    }

    /// 手工改选：越界索引等价于「无选中」（Qt 的 invalid index）。
    #[test]
    fn set_selected_rejects_out_of_range() {
        let mut state = ReleaseInfoState::new(
            release(vec![asset("app-linux-x64.zip", 1, 0)]),
            Platform::Linux,
            "x86_64",
        );
        state.set_selected(Some(0));
        assert!(state.validate());
        state.set_selected(Some(1));
        assert_eq!(state.selected_index(), None);
        assert!(!state.validate());
        state.set_selected(None);
        assert!(!state.validate());
    }

    /// 最小宽度两段式：无资产只吃 `min(580, w-48)`；有资产再吃 `needed+68`。
    #[test]
    fn min_width_matches_two_stage_rule() {
        let with_assets = ReleaseInfoState::new(
            release(vec![asset("app-linux-x64.AppImage", 1, 0)]),
            Platform::Linux,
            "x86_64",
        );
        // 窗口很窄：580 vs 400-48=352 → 352；needed 小则保持 352。
        assert_eq!(with_assets.min_width(400, 10), 352);
        // 资产列宽撑开：352 vs 400+68=468 → 468。
        assert_eq!(with_assets.min_width(400, 400), 468);
        // 宽窗口：min(580, 1000-48=952) = 580。
        assert_eq!(with_assets.min_width(1000, 10), 580);

        let no_assets = ReleaseInfoState::new(release(vec![]), Platform::Linux, "x86_64");
        assert_eq!(no_assets.min_width(1000, 9999), 580);
        assert_eq!(no_assets.min_width(400, 9999), 352);
    }

    /// 标题区/按钮的可断言字段。
    #[test]
    fn header_fields_mirror_widgets() {
        let mut r = release(vec![]);
        r.prerelease = true;
        let state = ReleaseInfoState::new(r, Platform::Linux, "x86_64");
        assert_eq!(state.published_date(), "2026-10-01");
        assert!(state.prerelease_visible());
        assert_eq!(state.detail_url(), state.release().page_url.as_str());
        assert_eq!(state.sponsor_url(), DONATE_URL);
        assert!(state.description_scrolls_to_start());
        assert_eq!(state.description_markdown(), "## 更新内容\n- 修了个 bug");
    }
}
