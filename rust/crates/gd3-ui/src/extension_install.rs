//! 扩展安装引导框（P3，第 123 步）：镜像 `app/view/dialogs/extension_install.py`（133 行）。
//!
//! 第 122 步的 `extension_actions::install()` 解包成功后只发一条
//! [`crate::extension_actions::Notice::ExtensionInstalled`]（`SystemEffects` 里
//! `eprintln!` 一行）。这一步把 Python 真正弹的那个五步引导框补上：本模块给**文案与
//! 副作用**，`ui/extension_install.slint` 给形状，`boot.rs` 把两边接起来。
//!
//! ## 实证口径（2026-10-19 实读源码，`translate` = identity）
//!
//! - **标题**：`扩展已解包，按以下步骤安装`（`extension_install.py:24`，`SubtitleLabel`）。
//! - **预览**：`PREVIEW_SIZE = QSize(720, 405)`（:14）、
//!   `PREVIEW_GIF = ":/res/install_chrome_extension_guidance.webp"`（:15）；
//!   `QMovie.isValid()` 为假时正文退成 `演示动画即将上线`（:54）。
//! - **五步**（:70-76，`enumerate(steps, 1)` 给序号）：
//!   1. `在浏览器中打开 {chip("chrome://extensions", mono=True)}`
//!   2. `开启 {chip("开发者模式")}`
//!   3. `将 {chip("扩展文件夹")} 拖入浏览器窗口`
//!   4. `启用扩展`
//!   5. `打开 {chip("扩展弹窗")}，点击 {chip("自动配对")}`
//!
//!   `chip()`（:66-68）产出
//!   `<span style="background-color: {chipBg};{family}">&nbsp;{text}&nbsp;</span>`，
//!   其中 `family` 只在 `mono=True` 时是 ` font-family: 'Cascadia Code', Consolas, monospace;`，
//!   `chipBg` 暗色 `rgba(255, 255, 255, 0.1)`、亮色 `rgba(0, 0, 0, 0.06)`。
//! - **两个按钮**：yes = `打开扩展页面并定位目录`（:38）、cancel = `关闭`（:39）。
//!   顺序照 `MessageBoxBase.__initLayout`（`buttonLayout.addWidget(yesButton)` 在前），
//!   即 **左 yes 右 cancel**。
//! - **尺寸**：`setMinimumWidth(PREVIEW_SIZE.width() + 48)` = **768**（:33，+48 是
//!   `scrollLayout` 左右各 24 的边距）；`setMaximumHeight(PREVIEW_SIZE.height() + 385)`
//!   = **790**（:35，385 = 48 边距 + 60 空隙 + 28 标题 + 168 步骤 + 81 按钮组）。
//! - **yes 不关框**：`_bind`（:120-121）先 `yesButton.clicked.disconnect()` 再连
//!   `_onInstall` —— 点 yes 只做副作用，框还开着；关框只有 cancel
//!   （`MessageBoxBase.__onCancelButtonClicked` → `reject()`）。
//! - **`_onInstall`**（:123-133）：`revealInFolder(str(self._path))` →
//!   `openChromiumUrl("chrome://extensions")`；**返回假**才
//!   `clipboard.setText("chrome://extensions")` +
//!   `InfoBar.info(请手动打开浏览器, chrome://extensions 已复制到剪贴板，请粘贴到地址栏,
//!   duration=5000, position=TOP)`。
//!
//! ## 分工
//!
//! | 层 | 负责 |
//! |---|---|
//! | [`Guide`] / [`Step`] / [`Segment`] | 全部文案、步骤拆分、尺寸常量、富文本渲染 |
//! | [`plan_confirm`] / [`confirm`] | 「点 yes 之后发生什么」，副作用经 [`GuideEffects`] 注入 |
//! | [`show`] / [`hide`] / [`install`] | 往 `AppWindow` 的属性上灌值、装两个回调 |
//! | `ui/extension_install.slint` | 遮罩 + 面板 + 预览 + 五步 + 两个按钮的形状 |
//!
//! ## 与 Qt 侧的已知差异
//!
//! - **预览是静态 PNG 而不是 webp 动画**：`.slint` 用 `@image-url` 编译期嵌入
//!   `app/assets/install_chrome_extension_guidance.png`（1415×670 的静帧）。webp 动画
//!   要解码器 + 逐帧推进，Slint 侧没有等价物；`image-fit: contain` 保住 16:9 的观感。
//!   Python 的 `演示动画即将上线` 兜底文案仍保留为 `previewText` 属性（图缺失时显示）。
//! - **没有模态事件循环**：`MessageBoxBase.exec()` 换成 `extensionInstallVisible`
//!   覆盖层 + `confirmed` / `dismissed` 两个回调。
//! - **`revealInFolder` 在 Windows 上返回假**：`gd3_core::desktop::reveal_in_folder`
//!   走 `open_target_command_on`，那条对 Windows 还没迁移（`ShellExecute` 未接），
//!   与第 122 步 `open_url` 用 `rundll32` 是两码事。Linux/macOS 正常。
//! - **`InfoBar` 换成 `eprintln!`**：与 `extension_actions::SystemEffects::notify`
//!   同一口径，等通知条接线那一步再换。

use std::path::{Path, PathBuf};
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{AppWindow, InstallStep};

// ---------------------------------------------------------------------------
// 文案与尺寸（`extension_install.py` 逐字）
// ---------------------------------------------------------------------------

/// `PREVIEW_SIZE.width()`（:14）。
pub const PREVIEW_WIDTH: i32 = 720;

/// `PREVIEW_SIZE.height()`（:14）。
pub const PREVIEW_HEIGHT: i32 = 405;

/// `setMinimumWidth(PREVIEW_SIZE.width() + 48)`（:33）。
pub const MIN_WIDTH: i32 = PREVIEW_WIDTH + 48;

/// `setMaximumHeight(PREVIEW_SIZE.height() + 385)`（:35）。
pub const MAX_HEIGHT: i32 = PREVIEW_HEIGHT + 385;

/// `PREVIEW_GIF`（:15）—— Qt 资源路径，这里只作为**事实记录**保留。
///
/// Slint 侧不用它：`.slint` 直接 `@image-url` 嵌入同名 PNG（见模块头「已知差异」）。
pub const PREVIEW_RESOURCE: &str = ":/res/install_chrome_extension_guidance.webp";

/// `movie.isValid()` 为假时的占位文案（:54）。
pub const PREVIEW_FALLBACK: &str = "演示动画即将上线";

/// 标题（:24）。
pub const TITLE: &str = "扩展已解包，按以下步骤安装";

/// yes 按钮（:38）。
pub const YES_LABEL: &str = "打开扩展页面并定位目录";

/// cancel 按钮（:39）。
pub const CANCEL_LABEL: &str = "关闭";

/// `_onInstall` 里 `openChromiumUrl` 的入参（:127）。
pub const CHROMIUM_EXTENSIONS_URL: &str = "chrome://extensions";

/// `InfoBar.info(..., duration=5000, ...)`（:132）。
pub const INFO_DURATION_MS: i32 = 5000;

/// `InfoBar.info` 的标题（:130）。
pub const MANUAL_OPEN_TITLE: &str = "请手动打开浏览器";

/// `InfoBar.info` 的正文（:131）。
pub const MANUAL_OPEN_BODY: &str = "chrome://extensions 已复制到剪贴板，请粘贴到地址栏";

/// 暗色主题下的 chip 底色（:64）。
pub const CHIP_BACKGROUND_DARK: &str = "rgba(255, 255, 255, 0.1)";

/// 亮色主题下的 chip 底色（:64）。
pub const CHIP_BACKGROUND_LIGHT: &str = "rgba(0, 0, 0, 0.06)";

/// `mono=True` 时拼进 style 的字族段（:67，注意开头那个空格是 Python 原文）。
pub const CHIP_MONO_FAMILY: &str = " font-family: 'Cascadia Code', Consolas, monospace;";

/// 预览占位框的暗色底（:56）。
pub const PREVIEW_BACKGROUND_DARK: &str = "rgba(255, 255, 255, 0.06)";

/// 预览占位框的暗色字（:56）。
pub const PREVIEW_COLOR_DARK: &str = "rgba(255, 255, 255, 0.5)";

/// 预览占位框的亮色底（:58）。
pub const PREVIEW_BACKGROUND_LIGHT: &str = "rgba(0, 0, 0, 0.04)";

/// 预览占位框的亮色字（:58）。
pub const PREVIEW_COLOR_LIGHT: &str = "rgba(0, 0, 0, 0.4)";

/// 预览占位框的样式表（:59-61，`QLabel { background: …; color: …; border-radius: 8px; }`）。
pub fn preview_style(dark: bool) -> String {
    let (background, color) = if dark {
        (PREVIEW_BACKGROUND_DARK, PREVIEW_COLOR_DARK)
    } else {
        (PREVIEW_BACKGROUND_LIGHT, PREVIEW_COLOR_LIGHT)
    };
    format!("QLabel {{ background: {background}; color: {color}; border-radius: 8px; }}")
}

// ---------------------------------------------------------------------------
// 步骤：片段 → 纯文本 / 富文本
// ---------------------------------------------------------------------------

/// 一步引导里的一个片段。
///
/// Python 的 `chip(text, mono=False)` 把文字包进带底色的 `<span>`；模板里的普通文字
/// 是片段之外的字面量。这里把两者都摊成片段，[`Step::plain_text`] 与
/// [`Step::rich_text`] 才能共用同一份数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// 片段文字（chip 内 / 外的原文，不含 `&nbsp;`）。
    pub text: String,
    /// 对应 `chip(..., mono=True)`：加等宽字族。
    pub mono: bool,
    /// 是否包在 `<span style="background-color: …">` 里。
    pub chip: bool,
}

impl Segment {
    /// 模板里的普通文字。
    pub fn plain(text: &str) -> Self {
        Self {
            text: text.to_string(),
            mono: false,
            chip: false,
        }
    }

    /// `chip(text)`。
    pub fn chip(text: &str) -> Self {
        Self {
            text: text.to_string(),
            mono: false,
            chip: true,
        }
    }

    /// `chip(text, mono=True)`。
    pub fn mono_chip(text: &str) -> Self {
        Self {
            text: text.to_string(),
            mono: true,
            chip: true,
        }
    }

    /// 渲染成 Python 的富文本片段（普通片段原样返回）。
    pub fn render(&self, dark: bool) -> String {
        if !self.chip {
            return self.text.clone();
        }
        let background = if dark {
            CHIP_BACKGROUND_DARK
        } else {
            CHIP_BACKGROUND_LIGHT
        };
        let family = if self.mono { CHIP_MONO_FAMILY } else { "" };
        let text = &self.text;
        format!("<span style=\"background-color: {background};{family}\">&nbsp;{text}&nbsp;</span>")
    }
}

/// 一步引导：序号（`enumerate(..., 1)`）+ 片段表。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// 显示在圆形徽标里的序号，从 1 开始。
    pub index: usize,
    /// 片段表（普通文字与 chip 交替）。
    pub segments: Vec<Segment>,
}

impl Step {
    /// 纯文本（chip 两侧不留 `&nbsp;`）—— 给 Slint 的 `InstallStep.text`。
    pub fn plain_text(&self) -> String {
        let mut text = String::new();
        for segment in &self.segments {
            text.push_str(&segment.text);
        }
        text
    }

    /// Python 的 `stepLabel.setText(rich_text)` 原样（`Qt.TextFormat.RichText`）。
    pub fn rich_text(&self, dark: bool) -> String {
        let mut text = String::new();
        for segment in &self.segments {
            text.push_str(&segment.render(dark));
        }
        text
    }
}

/// 五步引导（`_initSteps` 的 `steps` 列表，:70-76）。
pub fn steps() -> Vec<Step> {
    vec![
        Step {
            index: 1,
            segments: vec![
                Segment::plain("在浏览器中打开 "),
                Segment::mono_chip(CHROMIUM_EXTENSIONS_URL),
            ],
        },
        Step {
            index: 2,
            segments: vec![Segment::plain("开启 "), Segment::chip("开发者模式")],
        },
        Step {
            index: 3,
            segments: vec![
                Segment::plain("将 "),
                Segment::chip("扩展文件夹"),
                Segment::plain(" 拖入浏览器窗口"),
            ],
        },
        Step {
            index: 4,
            segments: vec![Segment::plain("启用扩展")],
        },
        Step {
            index: 5,
            segments: vec![
                Segment::plain("打开 "),
                Segment::chip("扩展弹窗"),
                Segment::plain("，点击 "),
                Segment::chip("自动配对"),
            ],
        },
    ]
}

// ---------------------------------------------------------------------------
// Guide：一份完整的对话框内容
// ---------------------------------------------------------------------------

/// 对话框的全部静态内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guide {
    /// 标题（[`TITLE`]）。
    pub title: &'static str,
    /// yes 按钮文案（[`YES_LABEL`]）。
    pub yes_label: &'static str,
    /// cancel 按钮文案（[`CANCEL_LABEL`]）。
    pub cancel_label: &'static str,
    /// Qt 资源路径（[`PREVIEW_RESOURCE`]，Slint 侧不用）。
    pub preview_resource: &'static str,
    /// 预览缺失时的占位文案（[`PREVIEW_FALLBACK`]）。
    pub preview_fallback: &'static str,
    /// 最小宽度（[`MIN_WIDTH`]）。
    pub min_width: i32,
    /// 最大高度（[`MAX_HEIGHT`]）。
    pub max_height: i32,
    /// 五步。
    pub steps: Vec<Step>,
}

impl Default for Guide {
    fn default() -> Self {
        Self::new()
    }
}

impl Guide {
    /// 一份与 Python 逐字一致的引导内容。
    pub fn new() -> Self {
        Self {
            title: TITLE,
            yes_label: YES_LABEL,
            cancel_label: CANCEL_LABEL,
            preview_resource: PREVIEW_RESOURCE,
            preview_fallback: PREVIEW_FALLBACK,
            min_width: MIN_WIDTH,
            max_height: MAX_HEIGHT,
            steps: steps(),
        }
    }

    /// 摊成 Slint 的 `InstallStep` 模型数据（序号 + 纯文本）。
    pub fn to_slint_steps(&self) -> Vec<InstallStep> {
        self.steps
            .iter()
            .map(|step| InstallStep {
                index: step.index as i32,
                text: step.plain_text().into(),
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// 点 yes 之后：副作用计划
// ---------------------------------------------------------------------------

/// 一次 `_onInstall` 产生的副作用（顺序即发生顺序）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// `revealInFolder(str(self._path))`。
    RevealInFolder(PathBuf),
    /// `openChromiumUrl("chrome://extensions")`。
    OpenChromiumUrl(String),
    /// `QApplication.clipboard().setText("chrome://extensions")`。
    CopyText(String),
    /// `InfoBar.info(请手动打开浏览器, …)`。
    Notify {
        /// 标题。
        title: &'static str,
        /// 正文。
        body: &'static str,
    },
}

/// `_onInstall` 的计划：先定位目录、再开扩展页；**开不起来**才复制链接 + 提示。
///
/// `opened` 是 `openChromiumUrl` 的返回值 —— 计划要能表达两个分支，所以它是入参
/// 而不是在这里调用。
pub fn plan_confirm(folder: &Path, opened: bool) -> Vec<Effect> {
    let mut plan = vec![
        Effect::RevealInFolder(folder.to_path_buf()),
        Effect::OpenChromiumUrl(CHROMIUM_EXTENSIONS_URL.to_string()),
    ];
    if !opened {
        plan.push(Effect::CopyText(CHROMIUM_EXTENSIONS_URL.to_string()));
        plan.push(Effect::Notify {
            title: MANUAL_OPEN_TITLE,
            body: MANUAL_OPEN_BODY,
        });
    }
    plan
}

/// 执行 [`plan_confirm`] 的外设注入点。
///
/// 与 `extension_actions::Effects` 分开：这里多一个 `reveal_in_folder`、少一个保存
/// 对话框，且 `notify` 只要两行文字（`InfoBar.info` 没有按钮、没有复选框）。
pub trait GuideEffects {
    /// `revealInFolder(path)`：成功返回 `true`。
    fn reveal_in_folder(&self, folder: &Path) -> bool;
    /// `QDesktopServices.openUrl` / `openChromiumUrl`：起得来返回 `true`。
    fn open_url(&self, url: &str) -> bool;
    /// `QApplication.clipboard().setText`。
    fn copy_text(&self, text: &str) -> bool;
    /// `InfoBar.info`。
    fn notify(&self, title: &str, body: &str);
}

/// 跑一遍 `_onInstall`：定位目录 → 开扩展页 → 失败则复制链接 + 提示。
///
/// 返回值是「浏览器有没有打开」，与 Python 的 `openChromiumUrl` 返回值同义。
pub fn confirm(effects: &dyn GuideEffects, folder: &Path) -> bool {
    effects.reveal_in_folder(folder);
    let opened = effects.open_url(CHROMIUM_EXTENSIONS_URL);
    if !opened {
        effects.copy_text(CHROMIUM_EXTENSIONS_URL);
        effects.notify(MANUAL_OPEN_TITLE, MANUAL_OPEN_BODY);
    }
    opened
}

/// 生产实现：`gd3_core::desktop` + 第 122 步的剪贴板命令 + `eprintln!`。
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemGuide;

impl GuideEffects for SystemGuide {
    fn reveal_in_folder(&self, folder: &Path) -> bool {
        gd3_core::desktop::reveal_in_folder(folder)
            .is_some_and(|command| gd3_core::desktop::spawn_detached(&command).is_ok())
    }

    fn open_url(&self, url: &str) -> bool {
        gd3_core::desktop::open_chromium_url(url)
            .is_some_and(|command| gd3_core::desktop::spawn_detached(&command).is_ok())
    }

    fn copy_text(&self, text: &str) -> bool {
        crate::extension_actions::copy_text(text)
    }

    fn notify(&self, title: &str, body: &str) {
        eprintln!("{title}：{body}");
    }
}

// ---------------------------------------------------------------------------
// 窗口接线
// ---------------------------------------------------------------------------

/// 把引导框的六个属性灌满并显示（Python 的 `ExtensionInstallDialog(path).exec()`）。
pub fn show(window: &AppWindow, folder: &Path) {
    let guide = Guide::new();
    window.set_extensionInstallFolder(folder.display().to_string().into());
    window.set_extensionInstallTitle(guide.title.into());
    window.set_extensionInstallSteps(ModelRc::new(VecModel::from(guide.to_slint_steps())));
    window.set_extensionInstallPreviewText(guide.preview_fallback.into());
    window.set_extensionInstallYesText(guide.yes_label.into());
    window.set_extensionInstallCancelText(guide.cancel_label.into());
    window.set_extensionInstallVisible(true);
}

/// 关掉引导框（cancel 的语义：`reject()`）。
pub fn hide(window: &AppWindow) {
    window.set_extensionInstallVisible(false);
}

/// 装两个回调。
///
/// `confirmed` **不改可见性** —— Python 的 yes 按钮被 `disconnect()` 过，点它只做
/// 副作用；`dismissed` 才关框。
pub fn install(window: &AppWindow, effects: Rc<dyn GuideEffects>) {
    let win = window.as_weak();
    let confirm_effects = Rc::clone(&effects);
    window.on_extensionInstallConfirmed(move || {
        if let Some(window) = win.upgrade() {
            let folder = PathBuf::from(window.get_extensionInstallFolder().as_str());
            confirm(confirm_effects.as_ref(), &folder);
        }
    });

    let win = window.as_weak();
    window.on_extensionInstallDismissed(move || {
        if let Some(window) = win.upgrade() {
            hide(&window);
        }
    });
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::Path;

    use slint::Model as _;

    use super::*;

    /// 记录每次外设调用的假实现（不碰真剪贴板、不真起浏览器）。
    struct FakeGuide {
        log: Rc<RefCell<Vec<String>>>,
        opened: bool,
        revealed: bool,
    }

    impl FakeGuide {
        fn new(opened: bool) -> (Rc<Self>, Rc<RefCell<Vec<String>>>) {
            let log = Rc::new(RefCell::new(Vec::new()));
            let guide = Rc::new(Self {
                log: Rc::clone(&log),
                opened,
                revealed: true,
            });
            (guide, log)
        }
    }

    impl GuideEffects for FakeGuide {
        fn reveal_in_folder(&self, folder: &Path) -> bool {
            self.log
                .borrow_mut()
                .push(format!("reveal:{}", folder.display()));
            self.revealed
        }

        fn open_url(&self, url: &str) -> bool {
            self.log.borrow_mut().push(format!("open:{url}"));
            self.opened
        }

        fn copy_text(&self, text: &str) -> bool {
            self.log.borrow_mut().push(format!("copy:{text}"));
            true
        }

        fn notify(&self, title: &str, body: &str) {
            self.log.borrow_mut().push(format!("notify:{title}|{body}"));
        }
    }

    #[test]
    fn the_guide_matches_the_python_constants() {
        let guide = Guide::new();
        assert_eq!(guide.title, "扩展已解包，按以下步骤安装");
        assert_eq!(guide.yes_label, "打开扩展页面并定位目录");
        assert_eq!(guide.cancel_label, "关闭");
        assert_eq!(guide.preview_fallback, "演示动画即将上线");
        assert_eq!(
            guide.preview_resource,
            ":/res/install_chrome_extension_guidance.webp"
        );
        assert_eq!(guide.min_width, 768, "720 + 48");
        assert_eq!(guide.max_height, 790, "405 + 385");
        assert_eq!(Guide::default(), guide);
    }

    #[test]
    fn the_five_steps_are_the_python_lines() {
        let texts: Vec<String> = steps().iter().map(Step::plain_text).collect();
        assert_eq!(
            texts,
            [
                "在浏览器中打开 chrome://extensions",
                "开启 开发者模式",
                "将 扩展文件夹 拖入浏览器窗口",
                "启用扩展",
                "打开 扩展弹窗，点击 自动配对",
            ]
        );
        let indices: Vec<usize> = steps().iter().map(|step| step.index).collect();
        assert_eq!(indices, [1, 2, 3, 4, 5], "enumerate(steps, 1)");
    }

    #[test]
    fn the_first_step_wraps_the_url_in_a_mono_chip() {
        let step = &steps()[0];
        assert_eq!(
            step.rich_text(true),
            "在浏览器中打开 <span style=\"background-color: rgba(255, 255, 255, 0.1); \
             font-family: 'Cascadia Code', Consolas, monospace;\">&nbsp;chrome://extensions&nbsp;</span>"
        );
        assert_eq!(
            step.segments[1],
            Segment::mono_chip("chrome://extensions"),
            "chip(text, mono=True)"
        );
    }

    #[test]
    fn the_chip_background_follows_the_theme() {
        let step = &steps()[1];
        assert!(
            step.rich_text(true)
                .contains("background-color: rgba(255, 255, 255, 0.1);"),
            "暗色：{}",
            step.rich_text(true)
        );
        assert!(
            step.rich_text(false)
                .contains("background-color: rgba(0, 0, 0, 0.06);"),
            "亮色：{}",
            step.rich_text(false)
        );
        assert!(
            !step.rich_text(true).contains("font-family"),
            "非 mono 的 chip 不带字族段"
        );
    }

    #[test]
    fn a_step_without_chips_renders_as_plain_text() {
        let step = &steps()[3];
        assert_eq!(step.segments, [Segment::plain("启用扩展")]);
        assert_eq!(step.rich_text(true), "启用扩展");
        assert_eq!(step.rich_text(false), "启用扩展");
    }

    #[test]
    fn a_plain_segment_ignores_the_theme() {
        assert_eq!(Segment::plain("拖入").render(true), "拖入");
        assert_eq!(Segment::plain("拖入").render(false), "拖入");
        assert_eq!(
            Segment::chip("x").render(false),
            "<span style=\"background-color: rgba(0, 0, 0, 0.06);\">&nbsp;x&nbsp;</span>"
        );
    }

    #[test]
    fn the_preview_placeholder_style_follows_the_theme() {
        assert_eq!(
            preview_style(true),
            "QLabel { background: rgba(255, 255, 255, 0.06); color: rgba(255, 255, 255, 0.5); \
             border-radius: 8px; }"
        );
        assert_eq!(
            preview_style(false),
            "QLabel { background: rgba(0, 0, 0, 0.04); color: rgba(0, 0, 0, 0.4); \
             border-radius: 8px; }"
        );
    }

    #[test]
    fn slint_steps_carry_the_index_and_the_plain_text() {
        let model: Vec<InstallStep> = Guide::new().to_slint_steps();
        assert_eq!(model.len(), 5);
        assert_eq!(model[0].index, 1);
        assert_eq!(model[0].text, "在浏览器中打开 chrome://extensions");
        assert_eq!(model[4].index, 5);
        assert_eq!(model[4].text, "打开 扩展弹窗，点击 自动配对");
    }

    #[test]
    fn a_failed_browser_open_falls_back_to_the_clipboard() {
        let plan = plan_confirm(Path::new("/tmp/ext"), false);
        assert_eq!(
            plan,
            [
                Effect::RevealInFolder(PathBuf::from("/tmp/ext")),
                Effect::OpenChromiumUrl("chrome://extensions".to_string()),
                Effect::CopyText("chrome://extensions".to_string()),
                Effect::Notify {
                    title: "请手动打开浏览器",
                    body: "chrome://extensions 已复制到剪贴板，请粘贴到地址栏",
                },
            ]
        );
    }

    #[test]
    fn a_successful_browser_open_stops_after_the_url() {
        let plan = plan_confirm(Path::new("/tmp/ext"), true);
        assert_eq!(plan.len(), 2, "成功时不复制、不提示");
        assert_eq!(plan[0], Effect::RevealInFolder(PathBuf::from("/tmp/ext")));
        assert_eq!(
            plan[1],
            Effect::OpenChromiumUrl("chrome://extensions".to_string())
        );
    }

    #[test]
    fn confirm_runs_the_fallback_in_order() {
        let (guide, log) = FakeGuide::new(false);
        let opened = confirm(guide.as_ref(), Path::new("/tmp/ext"));
        assert!(!opened);
        assert_eq!(
            *log.borrow(),
            [
                "reveal:/tmp/ext",
                "open:chrome://extensions",
                "copy:chrome://extensions",
                "notify:请手动打开浏览器|chrome://extensions 已复制到剪贴板，请粘贴到地址栏",
            ]
        );
    }

    #[test]
    fn confirm_reports_the_browser_result() {
        let (guide, log) = FakeGuide::new(true);
        assert!(confirm(guide.as_ref(), Path::new("/tmp/ext")));
        assert_eq!(
            *log.borrow(),
            ["reveal:/tmp/ext", "open:chrome://extensions"]
        );
    }

    #[test]
    fn confirm_ignores_the_reveal_result() {
        // Python 不检查 `revealInFolder` 的返回值：定位失败也照样开扩展页。
        let log = Rc::new(RefCell::new(Vec::new()));
        let guide = Rc::new(FakeGuide {
            log: Rc::clone(&log),
            opened: true,
            revealed: false,
        });
        assert!(confirm(guide.as_ref(), Path::new("/tmp/ext")));
        assert_eq!(
            *log.borrow(),
            ["reveal:/tmp/ext", "open:chrome://extensions"]
        );
    }

    #[test]
    fn show_fills_every_property_and_opens_the_overlay() {
        let window = crate::create_headless();
        assert!(!window.get_extensionInstallVisible());
        show(&window, Path::new("/tmp/ext"));
        assert!(window.get_extensionInstallVisible());
        assert_eq!(window.get_extensionInstallFolder(), "/tmp/ext");
        assert_eq!(
            window.get_extensionInstallTitle(),
            "扩展已解包，按以下步骤安装"
        );
        assert_eq!(window.get_extensionInstallPreviewText(), "演示动画即将上线");
        assert_eq!(
            window.get_extensionInstallYesText(),
            "打开扩展页面并定位目录"
        );
        assert_eq!(window.get_extensionInstallCancelText(), "关闭");
        let model = window.get_extensionInstallSteps();
        assert_eq!(model.row_count(), 5);
        assert_eq!(model.row_data(0).unwrap().index, 1);
        assert_eq!(
            model.row_data(0).unwrap().text,
            "在浏览器中打开 chrome://extensions"
        );
    }

    #[test]
    fn hide_only_flips_the_visibility() {
        let window = crate::create_headless();
        show(&window, Path::new("/tmp/ext"));
        hide(&window);
        assert!(!window.get_extensionInstallVisible());
        assert_eq!(
            window.get_extensionInstallFolder(),
            "/tmp/ext",
            "属性不清空"
        );
    }

    #[test]
    fn confirmed_runs_the_effects_but_keeps_the_dialog_open() {
        let window = crate::create_headless();
        let (guide, log) = FakeGuide::new(true);
        install(&window, guide);
        show(&window, Path::new("/tmp/ext"));

        window.invoke_extensionInstallConfirmed();
        assert_eq!(
            *log.borrow(),
            ["reveal:/tmp/ext", "open:chrome://extensions"]
        );
        assert!(
            window.get_extensionInstallVisible(),
            "yes 按钮被 disconnect 过，点它不关框"
        );
    }

    #[test]
    fn dismissed_closes_the_dialog() {
        let window = crate::create_headless();
        let (guide, log) = FakeGuide::new(true);
        install(&window, guide);
        show(&window, Path::new("/tmp/ext"));

        window.invoke_extensionInstallDismissed();
        assert!(!window.get_extensionInstallVisible());
        assert!(log.borrow().is_empty(), "cancel 不产生副作用");
    }
}
