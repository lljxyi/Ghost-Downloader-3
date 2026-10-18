//! 浏览器扩展安装引导对话框（P3，第 68 步）：镜像
//! `app/view/dialogs/extension_install.py`（133 行）的语义层。
//!
//! ## 实证口径（2026-10-18 实读源码）
//!
//! - `PREVIEW_SIZE = QSize(720, 405)`、`PREVIEW_GIF = ":/res/
//!   install_chrome_extension_guidance.webp"`（:12-13）。
//! - `_initWidget`（:34-47）：`widget.setMinimumWidth(PREVIEW_SIZE.width() + 48)`
//!   （注释写明「+ scrollLayout 左右各 24 边距」）、
//!   `widget.setMaximumHeight(PREVIEW_SIZE.height() + 385)`（注释拆解
//!   48+60+28+168+81）；`yesButton` = `打开扩展页面并定位目录`、
//!   `cancelButton` = `关闭`。
//! - `_initPreview`（:50-66）：`QMovie` 有效时铺满预览区并 `start()`；
//!   **无效时**才退化成文字 `演示动画即将上线` + 主题相关的灰底灰字
//!   （深色 `rgba(255, 255, 255, 0.06)` / `rgba(255, 255, 255, 0.5)`，
//!   浅色 `rgba(0, 0, 0, 0.04)` / `rgba(0, 0, 0, 0.4)`），圆角 8px。
//! - `_initSteps`（:68-100）：五步富文本，每步的「命令/名词」用
//!   [`chip`] 包成灰底 `<span>`（`&nbsp;` 两侧留白）；命令类走等宽字体
//!   `'Cascadia Code', Consolas, monospace`；步骤徽章是 24×24 圆形、
//!   底色 `themeColor()`、白字加粗。
//! - `_onInstall`（:130-133）：先 `revealInFolder(path)`，再
//!   `openChromiumUrl("chrome://extensions")`；**打开失败**才写剪贴板 +
//!   弹 5 秒 InfoBar（标题 `请手动打开浏览器`、正文
//!   `chrome://extensions 已复制到剪贴板，请粘贴到地址栏`）。
//!
//! ## 与 Qt 的差异
//!
//! `QMovie` 的有效性、`themeColor()`、剪贴板与 `openChromiumUrl` 都在外壳侧，
//! 本模块只把它们当**入参/出参**：`gif_available` 决定预览是动图还是文字，
//! `accent` 由调用方给（主题色），`install_action(path, opened)` 把
//! 「打开失败 → 复制 + InfoBar」摊成可断言的 [`InstallOutcome`]。

// ---------------------------------------------------------------------------
// 文案与常量（`extension_install.py` 逐字）
// ---------------------------------------------------------------------------

/// `PREVIEW_SIZE.width()`（:12）
pub const PREVIEW_WIDTH: i32 = 720;
/// `PREVIEW_SIZE.height()`（:12）
pub const PREVIEW_HEIGHT: i32 = 405;
/// `PREVIEW_GIF`（:13）
pub const PREVIEW_GIF: &str = ":/res/install_chrome_extension_guidance.webp";
/// `widget.setMinimumWidth(PREVIEW_SIZE.width() + 48)`（:36）
pub const MIN_WIDTH: i32 = PREVIEW_WIDTH + 48;
/// `widget.setMaximumHeight(PREVIEW_SIZE.height() + 385)`（:38）
pub const MAX_HEIGHT: i32 = PREVIEW_HEIGHT + 385;
/// `yesButton.setText(self.tr("打开扩展页面并定位目录"))`（:41）
pub const BUTTON_OPEN: &str = "打开扩展页面并定位目录";
/// `cancelButton.setText(self.tr("关闭"))`（:42）
pub const BUTTON_CLOSE: &str = "关闭";
/// `self.previewLabel.setText(self.tr("演示动画即将上线"))`（:60）
pub const FALLBACK_PREVIEW_TEXT: &str = "演示动画即将上线";
/// `openChromiumUrl("chrome://extensions")`（:132）
pub const CHROMIUM_EXTENSIONS_URL: &str = "chrome://extensions";
/// `InfoBar.info(self.tr("请手动打开浏览器"), ...)` 的标题（:138）
pub const MANUAL_OPEN_TITLE: &str = "请手动打开浏览器";
/// InfoBar 正文（:139）
pub const MANUAL_OPEN_CONTENT: &str = "chrome://extensions 已复制到剪贴板，请粘贴到地址栏";
/// `duration=5000`（:140）
pub const MANUAL_OPEN_DURATION_MS: u64 = 5000;
/// `chip()` 里命令类文本追加的字体声明（:72）
pub const MONO_FAMILY: &str = " font-family: 'Cascadia Code', Consolas, monospace;";
/// 徽章直径（`badge.setFixedSize(24, 24)`，:92）
pub const BADGE_SIZE: i32 = 24;

/// `chipBg = "rgba(255, 255, 255, 0.1)" if isDarkTheme() else "rgba(0, 0, 0, 0.06)"`（:69）
pub fn chip_background(dark: bool) -> &'static str {
    if dark {
        "rgba(255, 255, 255, 0.1)"
    } else {
        "rgba(0, 0, 0, 0.06)"
    }
}

/// 退化预览的 `(background, color)`（:62-65）。
pub fn fallback_colors(dark: bool) -> (&'static str, &'static str) {
    if dark {
        ("rgba(255, 255, 255, 0.06)", "rgba(255, 255, 255, 0.5)")
    } else {
        ("rgba(0, 0, 0, 0.04)", "rgba(0, 0, 0, 0.4)")
    }
}

/// 退化预览的样式表（:66-68）：`QLabel { background: {background}; color: {color};
/// border-radius: 8px; }`。
pub fn fallback_style(dark: bool) -> String {
    let (background, color) = fallback_colors(dark);
    format!("QLabel {{ background: {background}; color: {color}; border-radius: 8px; }}")
}

/// 步骤徽章样式（:95-97）：`background: {accent}; color: white; border-radius: 12px;
/// font-weight: bold;`。
pub fn badge_style(accent: &str) -> String {
    format!("background: {accent}; color: white; border-radius: 12px; font-weight: bold;")
}

/// `chip(text, mono)`（:71-74）：`<span style="background-color: {chipBg};{family}">
/// &nbsp;{text}&nbsp;</span>`，`family` 仅等宽时非空。
pub fn chip(text: &str, mono: bool, dark: bool) -> String {
    let family = if mono { MONO_FAMILY } else { "" };
    format!(
        "<span style=\"background-color: {};{family}\">&nbsp;{text}&nbsp;</span>",
        chip_background(dark)
    )
}

/// `_initSteps` 的五步富文本（:76-82），按声明顺序。
pub fn steps(dark: bool) -> Vec<String> {
    vec![
        format!("在浏览器中打开 {}", chip("chrome://extensions", true, dark)),
        format!("开启 {}", chip("开发者模式", false, dark)),
        format!("将 {} 拖入浏览器窗口", chip("扩展文件夹", false, dark)),
        "启用扩展".to_string(),
        format!(
            "打开 {}，点击 {}",
            chip("扩展弹窗", false, dark),
            chip("自动配对", false, dark)
        ),
    ]
}

/// 安装动作的产物：外壳据此做副作用（资源管理器定位 / 打开浏览器 / 剪贴板 + InfoBar）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallOutcome {
    /// `revealInFolder(str(self._path))`（:136）
    pub reveal_path: String,
    /// `openChromiumUrl(...)` 的目标（:132）
    pub chromium_url: String,
    /// `openChromiumUrl` 是否成功；`false` 时外壳要写剪贴板并弹 InfoBar。
    pub opened: bool,
    /// 失败时的 InfoBar 文案（成功时为 `None`）。
    pub fallback: Option<ManualOpenHint>,
}

/// `InfoBar.info(...)` 的三个可断言字段（:138-141）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManualOpenHint {
    pub title: String,
    pub content: String,
    pub duration_ms: u64,
}

impl ManualOpenHint {
    /// `_onInstall` 的失败分支文案。
    pub fn new() -> Self {
        Self {
            title: MANUAL_OPEN_TITLE.to_string(),
            content: MANUAL_OPEN_CONTENT.to_string(),
            duration_ms: MANUAL_OPEN_DURATION_MS,
        }
    }
}

impl Default for ManualOpenHint {
    fn default() -> Self {
        Self::new()
    }
}

/// 对话框状态：预览是动图还是退化文字 + 步骤富文本。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensionInstallState {
    dark: bool,
    gif_available: bool,
    path: String,
}

impl ExtensionInstallState {
    /// `__init__(path)` + `_initPreview()` 的 `movie.isValid()` 分支。
    pub fn new(path: &str, dark: bool, gif_available: bool) -> Self {
        Self {
            dark,
            gif_available,
            path: path.to_string(),
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn is_dark(&self) -> bool {
        self.dark
    }

    /// 动图可用时为 `None`（`previewLabel.setMovie(...)` 分支，:55-58）。
    pub fn preview_text(&self) -> Option<&'static str> {
        if self.gif_available {
            None
        } else {
            Some(FALLBACK_PREVIEW_TEXT)
        }
    }

    /// 退化分支的样式；动图可用时为 `None`（Qt 侧根本不会设样式表）。
    pub fn preview_style(&self) -> Option<String> {
        if self.gif_available {
            None
        } else {
            Some(fallback_style(self.dark))
        }
    }

    pub fn steps(&self) -> Vec<String> {
        steps(self.dark)
    }

    /// `_onInstall`（:130-141）：`opened` 由外壳的 `openChromiumUrl` 结果给。
    pub fn install_action(&self, opened: bool) -> InstallOutcome {
        InstallOutcome {
            reveal_path: self.path.clone(),
            chromium_url: CHROMIUM_EXTENSIONS_URL.to_string(),
            opened,
            fallback: if opened {
                None
            } else {
                Some(ManualOpenHint::new())
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_literals_match_python() {
        assert_eq!((PREVIEW_WIDTH, PREVIEW_HEIGHT), (720, 405));
        assert_eq!(MIN_WIDTH, 768);
        assert_eq!(MAX_HEIGHT, 790);
        assert_eq!(PREVIEW_GIF, ":/res/install_chrome_extension_guidance.webp");
        assert_eq!(BADGE_SIZE, 24);
    }

    /// `chip()` 的 HTML 逐字节（深色 + 等宽 / 浅色 + 普通各一组）。
    #[test]
    fn chip_html_is_byte_exact() {
        assert_eq!(
            chip("chrome://extensions", true, true),
            "<span style=\"background-color: rgba(255, 255, 255, 0.1); \
             font-family: 'Cascadia Code', Consolas, monospace;\">&nbsp;chrome://extensions&nbsp;</span>"
        );
        assert_eq!(
            chip("开发者模式", false, false),
            "<span style=\"background-color: rgba(0, 0, 0, 0.06);\">&nbsp;开发者模式&nbsp;</span>"
        );
    }

    /// 五步顺序与文案（浅色主题）。
    #[test]
    fn steps_match_python_order() {
        let steps = steps(false);
        assert_eq!(steps.len(), 5);
        assert!(steps[0].starts_with("在浏览器中打开 <span"));
        assert!(steps[0].contains("&nbsp;chrome://extensions&nbsp;"));
        assert_eq!(
            steps[1],
            "开启 <span style=\"background-color: rgba(0, 0, 0, 0.06);\">&nbsp;开发者模式&nbsp;</span>"
        );
        assert!(steps[2].starts_with("将 ") && steps[2].ends_with(" 拖入浏览器窗口"));
        assert_eq!(steps[3], "启用扩展");
        assert!(steps[4].starts_with("打开 ") && steps[4].contains("，点击 "));
        assert!(steps[4].contains("&nbsp;扩展弹窗&nbsp;"));
        assert!(steps[4].contains("&nbsp;自动配对&nbsp;"));
    }

    /// 动图可用时不给文字也不给样式（`setMovie` 分支提前 return）。
    #[test]
    fn valid_gif_suppresses_fallback_text_and_style() {
        let state = ExtensionInstallState::new("/tmp/ext", false, true);
        assert_eq!(state.preview_text(), None);
        assert_eq!(state.preview_style(), None);
        assert_eq!(state.path(), "/tmp/ext");
    }

    /// 动图不可用时文字 + 主题色样式。
    #[test]
    fn missing_gif_falls_back_to_themed_label() {
        let dark = ExtensionInstallState::new("/tmp/ext", true, false);
        assert_eq!(dark.preview_text(), Some("演示动画即将上线"));
        assert_eq!(
            dark.preview_style().as_deref(),
            Some(
                "QLabel { background: rgba(255, 255, 255, 0.06); color: rgba(255, 255, 255, 0.5); border-radius: 8px; }"
            )
        );
        let light = ExtensionInstallState::new("/tmp/ext", false, false);
        assert_eq!(
            light.preview_style().as_deref(),
            Some(
                "QLabel { background: rgba(0, 0, 0, 0.04); color: rgba(0, 0, 0, 0.4); border-radius: 8px; }"
            )
        );
    }

    /// 打开成功：只定位目录 + 打开浏览器，不碰剪贴板。
    #[test]
    fn successful_open_has_no_fallback_hint() {
        let state = ExtensionInstallState::new("/opt/ext", true, true);
        let outcome = state.install_action(true);
        assert_eq!(outcome.reveal_path, "/opt/ext");
        assert_eq!(outcome.chromium_url, "chrome://extensions");
        assert!(outcome.opened);
        assert_eq!(outcome.fallback, None);
    }

    /// 打开失败：剪贴板 + InfoBar 三字段。
    #[test]
    fn failed_open_returns_the_manual_hint() {
        let state = ExtensionInstallState::new("/opt/ext", false, false);
        let outcome = state.install_action(false);
        assert!(!outcome.opened);
        let hint = outcome.fallback.expect("失败必须给 InfoBar 文案");
        assert_eq!(hint.title, "请手动打开浏览器");
        assert_eq!(
            hint.content,
            "chrome://extensions 已复制到剪贴板，请粘贴到地址栏"
        );
        assert_eq!(hint.duration_ms, 5000);
        // 正文里嵌的 URL 与 `openChromiumUrl` 的目标同源。
        assert!(hint.content.contains(CHROMIUM_EXTENSIONS_URL));
    }

    #[test]
    fn badge_style_embeds_accent() {
        assert_eq!(
            badge_style("#0067C0"),
            "background: #0067C0; color: white; border-radius: 12px; font-weight: bold;"
        );
    }
}
