//! 浏览器扩展卡的五条动作（第 122 步）：复制 / 重新生成配对令牌、打开商店页、
//! 本地解包安装、导出 crx。
//!
//! ## 为什么单独一层
//!
//! 第 117–121 步把「设置页 → `UserConfig.json`」这一半接完了，`SettingsActions` 里还剩
//! 五个动作没有落点：它们的共同点是**副作用不在配置文件里**（剪贴板、默认浏览器、
//! 文件系统、保存对话框）。塞进 [`SettingsWriter`] 会让「配置写回」这一层依赖进程与外设；
//! 写成 `defer("…")` 的待办清单只是把问题往后推（第 117 步的权宜之计，这一步清掉）。
//!
//! 所以：窗口回调装在 [`install`]，副作用经 [`Effects`] 注入 —— 生产实现是
//! [`SystemEffects`]，单测用假实现（不碰真剪贴板、不真起浏览器、不弹真对话框）。
//!
//! ## 与 Python 的对应
//!
//! | Slint 回调 | Python 源 | 副作用 |
//! |---|---|---|
//! | `copyPairToken` | `setting_page.py:437-441` | `QApplication.clipboard().setText(token)` + InfoBar |
//! | `regeneratePairToken` | `setting_page.py:443-448` | `regenerateToken()` + 同一个剪贴板 + InfoBar |
//! | `openStoreUrl` | `setting_cards.py` 的 `HyperlinkButton.setUrl` | `QDesktopServices.openUrl` |
//! | `chromiumInstall` | `setting_page.py:450-468` | `BrowserService.install()` → `ExtensionInstallDialog` |
//! | `exportCrx` | `setting_page.py:487-495` | `QFileDialog.getSaveFileName` → 写 crx |
//!
//! ## `regenerate` 为什么也在这里
//!
//! Python 的一次点击是「先 `cfg.set(cfg.browserExtensionPairToken, …)`，再
//! `QApplication.clipboard().setText(token)`」。cfg 写回仍由
//! [`SettingsWriter::regenerate_pair_token`] 做，但**入口必须和复制捆在同一个闭包里**，
//! 否则「生成了新令牌却忘了复制」。所以 `SettingsActions::pair_token_regenerate` 在
//! 第 122 步从动作集里搬到了这里（`SettingsActions` 40 → 35 字段）。
//!
//! ## 已知差异
//!
//! - **没有 Qt 就没有 `QFileDialog`**：Linux 用 `zenity` / `kdialog`（装了就用），
//!   macOS 用 `osascript`；Windows 与三者都没有的环境返回 [`SaveChoice::Unavailable`]
//!   —— 宿主能据此提示，而不是静默什么都不做。
//! - **剪贴板靠外部命令**：Linux 依次试 `wl-copy` / `xclip` / `xsel`，macOS `pbcopy`，
//!   Windows `clip.exe`。一条都不在就返回 `false`（Python 的 `QApplication.clipboard()`
//!   永远可用）。不引入 `arboard`：它要拖一串 X11 / Wayland 依赖，而这里只需要
//!   「把一段文本写进剪贴板」。
//! - **导出成功 / 失败都发通知**：Python 在 `:487-495` 静默写盘，一个 InfoBar 都没有；
//!   这里把结果摊成 [`Notice`]，宿主可以忽略。
//! - **解包失败文本取自 [`crate::extension_crx::CrxError`] 的 `Display`**：Python 走
//!   `toLocalizedError(toTaskError(error))`，措辞不同，语义相同。

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;

use gd3_core::desktop::{open_url_command, spawn_detached};
use gd3_core::filesystem::which;
use gd3_core::settings;

use slint::ComponentHandle;

use crate::AppWindow;
use crate::extension_crx;
use crate::settings_writer::SettingsWriter;

/// `setting_page.py:487`：`QFileDialog.getSaveFileName(..., "./Extension.crx", ...)` 的默认名。
pub const EXPORT_DEFAULT_NAME: &str = "Extension.crx";

/// 同一个调用里的过滤器文案（`"Chromium Extension(*.crx)"`）。
pub const EXPORT_FILTER: &str = "Chromium Extension(*.crx)";

/// `setting_page.py:439`：`InfoBar.success(self.tr("已复制配对令牌"), token, …)`。
pub const TOKEN_COPIED_TITLE: &str = "已复制配对令牌";

/// `setting_page.py:446`。
pub const TOKEN_REGENERATED_TITLE: &str = "已重新生成配对令牌";

/// `setting_page.py:447`。
pub const TOKEN_REGENERATED_BODY: &str = "新令牌已复制到剪贴板";

/// `setting_page.py:467`：`InfoBar.error(self.tr("解包失败"), …)`。
pub const UNPACK_FAILED_TITLE: &str = "解包失败";

/// `QFileDialog.getSaveFileName` 的三种结局。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveChoice {
    /// 用户选了路径（Python 的 `path` 非空）。
    Path(PathBuf),
    /// 用户取消（Python 的 `path == ""`）。
    Cancelled,
    /// 这个环境没有可用的保存对话框（没有 Qt，也没有 `zenity` / `kdialog` / `osascript`）。
    Unavailable,
}

/// 一次动作要向用户交代的事（`InfoBar` / `ExtensionInstallDialog` 的替身）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// 令牌已复制；正文就是令牌本身。
    TokenCopied(String),
    /// 新令牌已生成并复制；正文是固定文案（令牌在 `take_changes()` 里）。
    TokenRegenerated(String),
    /// 解包失败；正文是 [`crate::extension_crx::CrxError`] 的 `Display`。
    UnpackFailed(String),
    /// 解包成功；正文是落盘目录（Python 弹 `ExtensionInstallDialog(path)`）。
    ExtensionInstalled(PathBuf),
    /// crx 已写到这个路径。
    Exported(PathBuf),
    /// 用户在保存对话框里点了取消。
    ExportCancelled,
    /// 写盘失败（Python 侧 `f.open()` 失败时什么都不做）。
    ExportFailed(String),
    /// 没有可用的保存对话框。
    SaveDialogUnavailable,
}

impl Notice {
    /// `InfoBar` 的两行：标题 + 正文。
    pub fn text(&self) -> (String, String) {
        match self {
            Self::TokenCopied(token) => (TOKEN_COPIED_TITLE.to_string(), token.clone()),
            Self::TokenRegenerated(_) => (
                TOKEN_REGENERATED_TITLE.to_string(),
                TOKEN_REGENERATED_BODY.to_string(),
            ),
            Self::UnpackFailed(message) => (UNPACK_FAILED_TITLE.to_string(), message.clone()),
            Self::ExtensionInstalled(folder) => {
                ("扩展已解包".to_string(), folder.display().to_string())
            }
            Self::Exported(path) => ("扩展已导出".to_string(), path.display().to_string()),
            Self::ExportCancelled => ("已取消导出".to_string(), String::new()),
            Self::ExportFailed(message) => ("导出失败".to_string(), message.clone()),
            Self::SaveDialogUnavailable => (
                "无法选择导出路径".to_string(),
                "当前环境没有可用的保存对话框（zenity / kdialog / osascript）".to_string(),
            ),
        }
    }
}

/// 平台副作用的注入点。
///
/// 四个方法对应四类外设：剪贴板、默认浏览器、保存对话框、用户提示。单测实现它们
/// 就能把整条动作链路跑通，不需要 X11、Wayland 或真对话框。
pub trait Effects {
    /// `QApplication.clipboard().setText(text)`：成功返回 `true`。
    fn copy_text(&self, text: &str) -> bool;
    /// `QDesktopServices.openUrl(QUrl(url))`：进程起得来就返回 `true`。
    fn open_url(&self, url: &str) -> bool;
    /// `QFileDialog.getSaveFileName(...)`。
    fn choose_save_path(&self, suggested: &str) -> SaveChoice;
    /// `InfoBar` / `ExtensionInstallDialog`。
    fn notify(&self, notice: Notice);
}

/// 浏览器扩展卡的动作集。
///
/// 内部是 [`Rc`]：Slint 的五个回调各要一份，而 [`SettingsWriter`] 只有一份。
pub struct ExtensionActions {
    writer: Rc<SettingsWriter>,
    /// `Paths::extension_folder()`（`startup.py:49` 的 `APP_DATA_DIR / "browser_extension"`）。
    folder: PathBuf,
    effects: Rc<dyn Effects>,
}

impl ExtensionActions {
    /// 装一份动作集。`folder` 是解包目标（`BrowserService.install()` 里的
    /// `self._extensionFolder`）。
    pub fn new(writer: Rc<SettingsWriter>, folder: PathBuf, effects: Rc<dyn Effects>) -> Rc<Self> {
        Rc::new(Self {
            writer,
            folder,
            effects,
        })
    }

    /// 解包目标（`BrowserService._extensionFolder`）。引导框要把它显示给用户，
    /// 第 123 步的 `on_chromiumInstall` 解包成功后用它调
    /// [`crate::extension_install::show`]。
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// `cfg.browserExtensionPairToken.value`（缺键 / 类型不符 → 空串）。
    pub fn token(&self) -> String {
        settings::read_text(&self.writer.config(), "browserExtensionPairToken").unwrap_or_default()
    }

    /// `_onCopyTokenClicked`（`setting_page.py:437-441`）。
    pub fn copy_token(&self) -> bool {
        let token = self.token();
        let copied = self.effects.copy_text(&token);
        if copied {
            self.effects.notify(Notice::TokenCopied(token));
        }
        copied
    }

    /// `_onRegenerateTokenClicked`（`setting_page.py:443-448`）：先落盘新令牌，再复制。
    pub fn regenerate_token(&self) -> bool {
        let token = self.writer.regenerate_pair_token();
        let copied = self.effects.copy_text(&token);
        if copied {
            self.effects.notify(Notice::TokenRegenerated(token));
        }
        copied
    }

    /// `openStoreUrl`：`HyperlinkButton.setUrl` 的语义 = 默认浏览器打开。
    pub fn open_store(&self, url: &str) -> bool {
        self.effects.open_url(url)
    }

    /// `BrowserService.install()`（`browser_service.py:196-205`）+ `ExtensionInstallDialog`。
    pub fn install(&self) -> bool {
        self.install_from(extension_crx::crx_bytes())
    }

    /// 同上，但 crx 可注入 —— 坏包分支（`InfoBar.error(解包失败, …)`）要能测。
    pub fn install_from(&self, crx: &[u8]) -> bool {
        match extension_crx::unpack(crx, &self.folder) {
            Ok(_written) => {
                self.effects
                    .notify(Notice::ExtensionInstalled(self.folder.clone()));
                true
            }
            Err(error) => {
                self.effects.notify(Notice::UnpackFailed(error.to_string()));
                false
            }
        }
    }

    /// `_onExportExtensionClicked`（`setting_page.py:487-495`）。
    pub fn export(&self) -> bool {
        match self.effects.choose_save_path(EXPORT_DEFAULT_NAME) {
            SaveChoice::Path(path) => {
                match extension_crx::export(extension_crx::crx_bytes(), &path) {
                    Ok(()) => {
                        self.effects.notify(Notice::Exported(path));
                        true
                    }
                    Err(error) => {
                        self.effects
                            .notify(Notice::ExportFailed(format!("{}：{error}", path.display())));
                        false
                    }
                }
            }
            SaveChoice::Cancelled => {
                self.effects.notify(Notice::ExportCancelled);
                false
            }
            SaveChoice::Unavailable => {
                self.effects.notify(Notice::SaveDialogUnavailable);
                false
            }
        }
    }
}

/// 把五个回调装到窗口上。
///
/// 第 122 步起这五个回调**只由本模块安装**：[`crate::SettingsActions`] 里已经删掉了
/// 对应字段，`install_settings_actions` 不再碰它们。Slint 的 `on_<callback>` 二次安装是
/// 替换而非叠加，所以「谁装谁负责」在这里是硬约束。
pub fn install(window: &AppWindow, actions: Rc<ExtensionActions>) {
    let copy = Rc::clone(&actions);
    window.on_copyPairToken(move || {
        copy.copy_token();
    });
    let regenerate = Rc::clone(&actions);
    window.on_regeneratePairToken(move || {
        regenerate.regenerate_token();
    });
    let store = Rc::clone(&actions);
    window.on_openStoreUrl(move |url| {
        store.open_store(url.as_str());
    });
    // `_onChromiumInstallClicked`（`setting_page.py:450-468`）：解包成功才弹引导框，
    // 失败只留一条 `InfoBar.error`（`install()` 内部已经发过 `Notice::UnpackFailed`）。
    let unpack = Rc::clone(&actions);
    let win = window.as_weak();
    window.on_chromiumInstall(move || {
        if unpack.install()
            && let Some(window) = win.upgrade()
        {
            crate::extension_install::show(&window, unpack.folder());
        }
    });
    window.on_exportCrx(move || {
        actions.export();
    });
}

/// 生产实现：外部命令 + `eprintln!`。
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemEffects;

impl Effects for SystemEffects {
    fn copy_text(&self, text: &str) -> bool {
        copy_text(text)
    }

    fn open_url(&self, url: &str) -> bool {
        open_url(url)
    }

    fn choose_save_path(&self, suggested: &str) -> SaveChoice {
        choose_save_path(suggested)
    }

    fn notify(&self, notice: Notice) {
        let (title, body) = notice.text();
        eprintln!("{title}：{body}");
    }
}

/// 剪贴板写入的候选命令（按平台）。
///
/// Linux 上「Wayland 优先」：`wl-copy` 在纯 X11 会话里会立刻失败，[`copy_text_with`]
/// 会接着试 `xclip` / `xsel`。`which` 只用来过滤 Linux 的候选表。
pub fn clipboard_commands_on(os: &str, which: impl Fn(&str) -> bool) -> Vec<Vec<String>> {
    // Windows 的 `clip.exe` 与 macOS 的 `pbcopy` 是系统自带工具，路径不一定在
    // `PATH` 里（尤其是从桌面环境拉起、`PATH` 被精简过的时候），所以不走 `which`。
    let table: &[&[&str]] = match os {
        "windows" => return vec![vec!["clip.exe".to_string()]],
        "macos" => return vec![vec!["pbcopy".to_string()]],
        _ => &[
            &["wl-copy"],
            &["xclip", "-selection", "clipboard"],
            &["xsel", "--clipboard", "--input"],
        ],
    };
    table
        .iter()
        .filter(|argv| which(argv[0]))
        .map(|argv| argv.iter().map(|part| part.to_string()).collect())
        .collect()
}

/// 逐条试命令，`run` 负责「把文本喂给 stdin 并等退出码」；第一条成功就停。
pub fn copy_text_with(
    commands: &[Vec<String>],
    text: &str,
    run: impl Fn(&[String], &str) -> bool,
) -> bool {
    commands.iter().any(|command| run(command, text))
}

/// [`copy_text_with`] 的生产版本：查 `PATH`，用真子进程。
pub fn copy_text(text: &str) -> bool {
    let commands = clipboard_commands_on(std::env::consts::OS, |name| which(name).is_some());
    copy_text_with(&commands, text, run_with_stdin)
}

/// `QDesktopServices.openUrl`：起一个默认程序打开 URL，**不等它退出**。
pub fn open_url(url: &str) -> bool {
    spawn_detached(&open_url_command(url)).is_ok()
}

/// 保存对话框的候选命令：Linux `zenity` → `kdialog`，macOS `osascript`，Windows 无。
///
/// 命令把「用户选的绝对路径」打到 stdout；取消时非零退出。
pub fn save_dialog_command_on(
    os: &str,
    which: impl Fn(&str) -> bool,
    suggested: &str,
) -> Option<Vec<String>> {
    let argv: Vec<String> = match os {
        "windows" => return None,
        "macos" => vec![
            "osascript".to_string(),
            "-e".to_string(),
            format!(
                "POSIX path of (choose file name with prompt \"选择导出路径\" default name \"{suggested}\")"
            ),
        ],
        _ if which("zenity") => vec![
            "zenity".to_string(),
            "--file-selection".to_string(),
            "--save".to_string(),
            "--confirm-overwrite".to_string(),
            format!("--filename={suggested}"),
            format!("--file-filter={EXPORT_FILTER} | *.crx"),
        ],
        _ if which("kdialog") => vec![
            "kdialog".to_string(),
            "--getsavefilename".to_string(),
            suggested.to_string(),
            EXPORT_FILTER.to_string(),
        ],
        _ => return None,
    };
    Some(argv)
}

/// [`save_dialog_command_on`] 的生产版本。
pub fn choose_save_path(suggested: &str) -> SaveChoice {
    let command = save_dialog_command_on(
        std::env::consts::OS,
        |name| which(name).is_some(),
        suggested,
    );
    match command {
        None => SaveChoice::Unavailable,
        Some(command) => match run_capture(&command) {
            Ok(Some(path)) => SaveChoice::Path(path),
            // 非零退出（用户取消）与读不出路径都算取消 —— Python 的 `path == ""`。
            Ok(None) | Err(_) => SaveChoice::Cancelled,
        },
    }
}

/// 把 `text` 喂给子进程的 stdin，然后等它退出。
fn run_with_stdin(command: &[String], text: &str) -> bool {
    let Some((program, args)) = command.split_first() else {
        return false;
    };
    let mut child = match Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return false,
    };
    if let Some(mut stdin) = child.stdin.take()
        && stdin.write_all(text.as_bytes()).is_err()
    {
        let _ = child.kill();
        let _ = child.wait();
        return false;
    }
    child.wait().map(|status| status.success()).unwrap_or(false)
}

/// 跑一条命令并读 stdout 的第一行（`zenity` / `kdialog` / `osascript` 都是这个形状）。
fn run_capture(command: &[String]) -> std::io::Result<Option<PathBuf>> {
    let Some((program, args)) = command.split_first() else {
        return Ok(None);
    };
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let path = text.trim();
    if path.is_empty() {
        Ok(None)
    } else {
        Ok(Some(PathBuf::from(path)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};

    use serde_json::Value;

    use crate::config_store::ConfigStore;

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "gd3-ui-extension-actions-{}-{tag}-{seq}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    /// 带固定令牌的 writer（`BrowserService.__init__` 生成过令牌之后的状态）。
    fn writer_with_token(dir: &Path, token: &str) -> Rc<SettingsWriter> {
        let mut config = settings::default_config();
        settings::spec("browserExtensionPairToken")
            .expect("令牌 spec 存在")
            .write(&mut config, &Value::from(token));
        Rc::new(SettingsWriter::new(ConfigStore::from_config(
            dir.join("UserConfig.json"),
            &config,
        )))
    }

    fn strings(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| part.to_string()).collect()
    }

    /// 只认识给定名字的假 `which`（对应 `shutil.which` 命中）。
    fn only<'a>(known: &'a [&'a str]) -> impl Fn(&str) -> bool + 'a {
        move |name| known.contains(&name)
    }

    struct FakeEffects {
        copied: RefCell<Vec<String>>,
        opened: RefCell<Vec<String>>,
        notices: RefCell<Vec<Notice>>,
        save: RefCell<SaveChoice>,
        copy_ok: bool,
    }

    impl FakeEffects {
        fn new() -> Self {
            Self {
                copied: RefCell::new(Vec::new()),
                opened: RefCell::new(Vec::new()),
                notices: RefCell::new(Vec::new()),
                save: RefCell::new(SaveChoice::Cancelled),
                copy_ok: true,
            }
        }

        fn saving(path: &Path) -> Self {
            let fake = Self::new();
            *fake.save.borrow_mut() = SaveChoice::Path(path.to_path_buf());
            fake
        }
    }

    impl Effects for FakeEffects {
        fn copy_text(&self, text: &str) -> bool {
            if !self.copy_ok {
                return false;
            }
            self.copied.borrow_mut().push(text.to_string());
            true
        }

        fn open_url(&self, url: &str) -> bool {
            self.opened.borrow_mut().push(url.to_string());
            true
        }

        fn choose_save_path(&self, _suggested: &str) -> SaveChoice {
            self.save.borrow().clone()
        }

        fn notify(&self, notice: Notice) {
            self.notices.borrow_mut().push(notice);
        }
    }

    #[test]
    fn copy_token_writes_the_configured_token() {
        let dir = temp_dir("copy");
        let writer = writer_with_token(&dir, "fixed-token");
        let effects = Rc::new(FakeEffects::new());
        let actions = ExtensionActions::new(writer, dir.join("ext"), effects.clone());

        assert!(actions.copy_token());
        assert_eq!(effects.copied.borrow().as_slice(), ["fixed-token"]);
        assert_eq!(
            effects.notices.borrow().as_slice(),
            [Notice::TokenCopied("fixed-token".to_string())]
        );
    }

    #[test]
    fn a_failed_clipboard_write_is_not_announced() {
        let dir = temp_dir("copy-fail");
        let writer = writer_with_token(&dir, "fixed-token");
        let effects = Rc::new(FakeEffects {
            copy_ok: false,
            ..FakeEffects::new()
        });
        let actions = ExtensionActions::new(writer, dir.join("ext"), effects.clone());

        assert!(!actions.copy_token());
        assert!(effects.notices.borrow().is_empty());
    }

    #[test]
    fn regenerate_token_writes_cfg_and_copies_the_new_token() {
        let dir = temp_dir("regen");
        let writer = writer_with_token(&dir, "fixed-token");
        let effects = Rc::new(FakeEffects::new());
        let actions = ExtensionActions::new(Rc::clone(&writer), dir.join("ext"), effects.clone());

        assert!(actions.regenerate_token());
        let token =
            settings::read_text(&writer.config(), "browserExtensionPairToken").expect("令牌仍在");
        assert_ne!(token, "fixed-token");
        assert_eq!(token.len(), 22, "`token_urlsafe(16)` = 22 个字符");
        assert_eq!(
            effects.copied.borrow().as_slice(),
            std::slice::from_ref(&token)
        );
        assert_eq!(
            effects.notices.borrow().as_slice(),
            [Notice::TokenRegenerated(token)]
        );
    }

    #[test]
    fn open_store_forwards_the_url() {
        let dir = temp_dir("store");
        let effects = Rc::new(FakeEffects::new());
        let actions = ExtensionActions::new(
            writer_with_token(&dir, "t"),
            dir.join("ext"),
            effects.clone(),
        );

        let url = "https://chromewebstore.google.com/detail/ghost-downloader-for-brow/lagbjgkmaafnlinaeonbhjchnjinjpeh";
        assert!(actions.open_store(url));
        assert_eq!(effects.opened.borrow().as_slice(), [url]);
    }

    #[test]
    fn install_unpacks_the_bundled_crx_into_the_folder() {
        let dir = temp_dir("install");
        let folder = dir.join("browser_extension");
        let effects = Rc::new(FakeEffects::new());
        let actions = ExtensionActions::new(
            writer_with_token(&dir, "t"),
            folder.clone(),
            effects.clone(),
        );

        assert!(actions.install());
        assert_eq!(
            std::fs::metadata(folder.join("manifest.json"))
                .expect("manifest.json 已落盘")
                .len(),
            2084
        );
        assert_eq!(
            effects.notices.borrow().as_slice(),
            [Notice::ExtensionInstalled(folder)]
        );
    }

    #[test]
    fn install_reports_the_unpack_error() {
        let dir = temp_dir("install-bad");
        let effects = Rc::new(FakeEffects::new());
        let actions = ExtensionActions::new(
            writer_with_token(&dir, "t"),
            dir.join("browser_extension"),
            effects.clone(),
        );

        assert!(!actions.install_from(b"not a crx"));
        match effects.notices.borrow().as_slice() {
            [Notice::UnpackFailed(message)] => {
                assert!(message.contains("CRX"), "错误文本要能定位：{message}");
            }
            other => panic!("应当是解包失败：{other:?}"),
        }
    }

    #[test]
    fn export_writes_the_chosen_file() {
        let dir = temp_dir("export");
        let target = dir.join("Extension.crx");
        let effects = Rc::new(FakeEffects::saving(&target));
        let actions = ExtensionActions::new(
            writer_with_token(&dir, "t"),
            dir.join("ext"),
            effects.clone(),
        );

        assert!(actions.export());
        assert_eq!(
            std::fs::read(&target).expect("crx 已写出"),
            extension_crx::crx_bytes()
        );
        assert_eq!(
            effects.notices.borrow().as_slice(),
            [Notice::Exported(target)]
        );
    }

    #[test]
    fn export_reports_cancel_and_unavailable() {
        let dir = temp_dir("export-cancel");
        let cancelled = Rc::new(FakeEffects::new());
        let actions = ExtensionActions::new(
            writer_with_token(&dir, "t"),
            dir.join("ext"),
            cancelled.clone(),
        );
        assert!(!actions.export());
        assert_eq!(
            cancelled.notices.borrow().as_slice(),
            [Notice::ExportCancelled]
        );

        let unavailable = Rc::new(FakeEffects::new());
        *unavailable.save.borrow_mut() = SaveChoice::Unavailable;
        let actions = ExtensionActions::new(
            writer_with_token(&dir, "t"),
            dir.join("ext"),
            unavailable.clone(),
        );
        assert!(!actions.export());
        assert_eq!(
            unavailable.notices.borrow().as_slice(),
            [Notice::SaveDialogUnavailable]
        );
    }

    #[test]
    fn export_reports_write_failure() {
        let dir = temp_dir("export-fail");
        // 父目录不存在 → `fs::write` 失败（Python 侧 `f.open()` 失败，静默）。
        let target = dir.join("missing/sub/Extension.crx");
        let effects = Rc::new(FakeEffects::saving(&target));
        let actions = ExtensionActions::new(
            writer_with_token(&dir, "t"),
            dir.join("ext"),
            effects.clone(),
        );

        assert!(!actions.export());
        match effects.notices.borrow().as_slice() {
            [Notice::ExportFailed(message)] => {
                assert!(
                    message.contains("Extension.crx"),
                    "错误文本带路径：{message}"
                );
            }
            other => panic!("应当是导出失败：{other:?}"),
        }
    }

    #[test]
    fn clipboard_commands_follow_the_platform() {
        assert_eq!(
            clipboard_commands_on("linux", only(&[])),
            Vec::<Vec<String>>::new()
        );
        assert_eq!(
            clipboard_commands_on("linux", only(&["xsel"])),
            vec![strings(&["xsel", "--clipboard", "--input"])]
        );
        // Wayland 优先，X11 兜底。
        assert_eq!(
            clipboard_commands_on("linux", only(&["wl-copy", "xclip"])),
            vec![
                strings(&["wl-copy"]),
                strings(&["xclip", "-selection", "clipboard"]),
            ]
        );
        // macOS / Windows 只有一条固定命令，不看 `which`。
        assert_eq!(
            clipboard_commands_on("macos", only(&[])),
            vec![strings(&["pbcopy"])]
        );
        assert_eq!(
            clipboard_commands_on("windows", only(&[])),
            vec![strings(&["clip.exe"])]
        );
    }

    #[test]
    fn copy_text_tries_the_next_command_after_a_failure() {
        let commands = vec![strings(&["a"]), strings(&["b"]), strings(&["c"])];
        let tried = RefCell::new(Vec::new());
        let ok = copy_text_with(&commands, "文本", |command, text| {
            tried
                .borrow_mut()
                .push((command[0].clone(), text.to_string()));
            command[0] == "b"
        });
        assert!(ok, "第二条成功即停");
        assert_eq!(
            tried.borrow().as_slice(),
            [
                ("a".to_string(), "文本".to_string()),
                ("b".to_string(), "文本".to_string()),
            ]
        );

        assert!(!copy_text_with(&commands, "文本", |_, _| false));
        assert!(!copy_text_with(&[], "文本", |_, _| true));
    }

    #[test]
    fn save_dialog_commands_prefer_zenity_then_kdialog() {
        assert_eq!(
            save_dialog_command_on("linux", only(&["zenity", "kdialog"]), EXPORT_DEFAULT_NAME),
            Some(strings(&[
                "zenity",
                "--file-selection",
                "--save",
                "--confirm-overwrite",
                "--filename=Extension.crx",
                "--file-filter=Chromium Extension(*.crx) | *.crx",
            ]))
        );
        assert_eq!(
            save_dialog_command_on("linux", only(&["kdialog"]), EXPORT_DEFAULT_NAME),
            Some(strings(&[
                "kdialog",
                "--getsavefilename",
                "Extension.crx",
                "Chromium Extension(*.crx)",
            ]))
        );
        assert_eq!(
            save_dialog_command_on("linux", only(&[]), EXPORT_DEFAULT_NAME),
            None
        );
        // macOS 不查 `which`：`osascript` 是系统自带。
        assert_eq!(
            save_dialog_command_on("macos", only(&[]), EXPORT_DEFAULT_NAME),
            Some(strings(&[
                "osascript",
                "-e",
                "POSIX path of (choose file name with prompt \"选择导出路径\" default name \"Extension.crx\")",
            ]))
        );
        assert_eq!(
            save_dialog_command_on("windows", only(&["zenity"]), EXPORT_DEFAULT_NAME),
            None
        );
    }

    #[test]
    fn notices_carry_the_python_messages() {
        assert_eq!(
            Notice::TokenCopied("abc".to_string()).text(),
            ("已复制配对令牌".to_string(), "abc".to_string())
        );
        assert_eq!(
            Notice::TokenRegenerated("abc".to_string()).text(),
            (
                "已重新生成配对令牌".to_string(),
                "新令牌已复制到剪贴板".to_string()
            )
        );
        assert_eq!(
            Notice::UnpackFailed("坏包".to_string()).text(),
            ("解包失败".to_string(), "坏包".to_string())
        );
        assert_eq!(
            Notice::ExtensionInstalled(PathBuf::from("/tmp/ext")).text(),
            ("扩展已解包".to_string(), "/tmp/ext".to_string())
        );
    }
}
