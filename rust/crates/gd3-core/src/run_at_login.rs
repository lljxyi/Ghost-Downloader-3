//! 开机自启（`app/platform/run_at_login.py`，79 行）的 Linux / macOS 分支。
//!
//! # 与 Python 的对照
//!
//! | Python | 这里 |
//! | --- | --- |
//! | `setRunAtLogin(enabled)`（按 `sys.platform` 分发） | [`set_run_at_login`]（按 `std::env::consts::OS` 分发） |
//! | `_setLinux` | [`set_autostart`] + [`autostart_file_path`] + [`autostart_entry_text`] |
//! | `_setMacOS` | [`set_launch_agent`] + [`launch_agent_path`] + [`launch_agent_text`] |
//! | `_setWindows`（`winreg`） | [`windows_run_value`]（只算注册表数据，**写注册表判 N/A**） |
//!
//! # 参数化说明
//!
//! `home` / `desktop_id` / `executable_path` / `version` 全部由调用方传入：
//! `gd3-core` 不硬编码 `gd3-ui` 里的 `DESKTOP_ID` / `VERSION` /
//! `EXECUTABLE_PATH`（后者在 Python 里是 `app/config/paths.py` 的运行时探测结果，
//! 依赖 `$APPIMAGE` / `sys.argv[0]` / `sys.executable`，本来就不该由 core 猜）。
//!
//! macOS 的 plist 路径在 Python 里写死成 `/Users/{getpwuid(os.getuid()).pw_name}/...`，
//! 这里换成 `home`（POSIX 上两者相同，且可测）。
//!
//! # 已知差异
//!
//! - **Windows 注册表未迁移**：Python 用 `winreg` 写
//!   `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`，值名 `GhostDownloader`、
//!   数据 `"<EXECUTABLE_PATH>" --silence`。Rust 侧没有等价的标准库 API
//!   （需要 `windows-sys` / `winreg` crate，本机离线不可用），
//!   [`set_run_at_login`] 在 Windows 上返回 `io::ErrorKind::Unsupported` 而不是
//!   **静默成功**——让用户看得见失败，而不是以为设置生效了。
//!   [`windows_run_value`] 保留数据格式（纯函数 + 单测），等注册表 API 可用时接上。
//! - Python 的 `sys.platform` 只有 `win32` / `darwin` / `linux` 三个分支，
//!   其它平台**静默不做任何事**；[`set_run_at_login`] 对未知平台同样返回 `Ok(())`。
//! - Python 全程没有 `try/except`（`run_at_login.py` 里一个都没有），文件系统错误
//!   原样冒到调用方；这里同样返回 `io::Result`，日志由调用方打。
//! - 禁用时只 `unlink`，**目录不删**（`~/.config/autostart` / `LaunchAgents` 留着）。
//!
//! # 落盘正文
//!
//! Linux（`~/.config/autostart/{desktop_id}.desktop`）：
//!
//! ```text
//! [Desktop Entry]
//! Type=Application
//! Version=<version>
//! Name=Ghost Downloader 3
//! Comment=A multi-threading downloader with QThread based on PySide6
//! Exec="<executable_path>" --silence
//! StartupNotify=false
//! Terminal=false
//! ```
//!
//! macOS（`<home>/Library/LaunchAgents/com.xiaoyouchr.ghostdownloader.plist`）：
//!
//! ```text
//! <?xml version="1.0" encoding="UTF-8"?>
//! <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
//! <plist version="1.0">
//! <dict>
//! <key>Label</key>
//! <string>com.xiaoyouchr.ghostdownloader</string>
//! <key>ProgramArguments</key>
//! <array>
//! <string><executable_path></string>
//! <string>--silence</string>
//! </array>
//! <key>RunAtLoad</key>
//! <true/>
//! </dict>
//! </plist>
//! ```

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::file_association::remove_if_exists;

/// macOS `LaunchAgent` 的 `Label`，也是 plist 的文件名（`{LABEL}.plist`）。
pub const LAUNCH_AGENT_LABEL: &str = "com.xiaoyouchr.ghostdownloader";

/// Windows 自启键（`HKEY_CURRENT_USER` 下的相对路径）：**只做文档与断言用**。
pub const WINDOWS_RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// Windows 自启值名。
pub const WINDOWS_VALUE_NAME: &str = "GhostDownloader";

/// `Path.home() / ".config/autostart" / f"{DESKTOP_ID}.desktop"`。
pub fn autostart_file_path(home: &Path, desktop_id: &str) -> PathBuf {
    home.join(".config/autostart")
        .join(format!("{desktop_id}.desktop"))
}

/// Linux 自启项的正文（`_setLinux` 里那段 `write_text`）。
pub fn autostart_entry_text(executable_path: &Path, version: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version={version}\n\
         Name=Ghost Downloader 3\n\
         Comment=A multi-threading downloader with QThread based on PySide6\n\
         Exec=\"{}\" --silence\n\
         StartupNotify=false\n\
         Terminal=false\n",
        executable_path.display()
    )
}

/// 镜像 `_setLinux`：启用时 `mkdir -p` + 写 `.desktop`，禁用时 `unlink(missing_ok=True)`。
pub fn set_autostart(
    enabled: bool,
    home: &Path,
    desktop_id: &str,
    executable_path: &Path,
    version: &str,
) -> io::Result<()> {
    let path = autostart_file_path(home, desktop_id);
    if enabled {
        fs::create_dir_all(home.join(".config/autostart"))?;
        fs::write(&path, autostart_entry_text(executable_path, version))?;
        Ok(())
    } else {
        remove_if_exists(&path)
    }
}

/// `<home>/Library/LaunchAgents/{LAUNCH_AGENT_LABEL}.plist`。
///
/// Python 写的是 `/Users/{用户名}/Library/LaunchAgents/...`，
/// 在 macOS 上 `home` 就是 `/Users/{用户名}`。
pub fn launch_agent_path(home: &Path) -> PathBuf {
    home.join("Library/LaunchAgents")
        .join(format!("{LAUNCH_AGENT_LABEL}.plist"))
}

/// macOS `LaunchAgent` 的 plist 正文（`_setMacOS` 里那段 `write_text`）。
pub fn launch_agent_text(executable_path: &Path) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         <key>Label</key>\n\
         <string>{LAUNCH_AGENT_LABEL}</string>\n\
         <key>ProgramArguments</key>\n\
         <array>\n\
         <string>{}</string>\n\
         <string>--silence</string>\n\
         </array>\n\
         <key>RunAtLoad</key>\n\
         <true/>\n\
         </dict>\n\
         </plist>\n",
        executable_path.display()
    )
}

/// 镜像 `_setMacOS`：启用时 `mkdir -p` + 写 plist，禁用时 `unlink(missing_ok=True)`。
pub fn set_launch_agent(enabled: bool, home: &Path, executable_path: &Path) -> io::Result<()> {
    let path = launch_agent_path(home);
    if enabled {
        fs::create_dir_all(home.join("Library/LaunchAgents"))?;
        fs::write(&path, launch_agent_text(executable_path))?;
        Ok(())
    } else {
        remove_if_exists(&path)
    }
}

/// Windows 注册表值的数据部分：`"<EXECUTABLE_PATH>" --silence`。
///
/// **只算数据**：`winreg` 的写入未迁移，见模块头。
pub fn windows_run_value(executable_path: &Path) -> String {
    format!("\"{}\" --silence", executable_path.display())
}

/// 镜像 `setRunAtLogin`：按平台分发。
///
/// - `linux` → [`set_autostart`]；
/// - `macos` → [`set_launch_agent`]；
/// - `windows` → `Err(Unsupported)`（注册表未迁移，见模块头）；
/// - 其它 → `Ok(())`（Python 的 `if/elif` 链没有 `else`，静默不做）。
pub fn set_run_at_login(
    enabled: bool,
    home: &Path,
    desktop_id: &str,
    executable_path: &Path,
    version: &str,
) -> io::Result<()> {
    match std::env::consts::OS {
        "linux" => set_autostart(enabled, home, desktop_id, executable_path, version),
        "macos" => set_launch_agent(enabled, home, executable_path),
        "windows" => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Windows 注册表自启尚未迁移（winreg 不可用）",
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = env::temp_dir().join(format!("gd3ral-{}-{tag}-{id}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const DESKTOP_ID: &str = "io.github.xiaoyouchr.GhostDownloader";
    const VERSION: &str = "4.4.0";

    fn executable() -> PathBuf {
        PathBuf::from("/opt/ghost/GhostDownloader")
    }

    #[test]
    fn autostart_entry_text_is_byte_identical_to_python() {
        // 期望值由 CPython 3.13 调 `run_at_login.setRunAtLogin(True)` 生成（217 字节）。
        let text = autostart_entry_text(&executable(), VERSION);
        assert_eq!(text.len(), 217);
        assert_eq!(
            text,
            "[Desktop Entry]\n\
             Type=Application\n\
             Version=4.4.0\n\
             Name=Ghost Downloader 3\n\
             Comment=A multi-threading downloader with QThread based on PySide6\n\
             Exec=\"/opt/ghost/GhostDownloader\" --silence\n\
             StartupNotify=false\n\
             Terminal=false\n"
        );
    }

    #[test]
    fn launch_agent_text_is_byte_identical_to_python() {
        // 期望值由 CPython 3.13 按 `_setMacOS` 的 f-string 拼接（398 字节）。
        let text = launch_agent_text(&executable());
        assert_eq!(text.len(), 398);
        assert_eq!(
            text,
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
             \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n\
             <dict>\n\
             <key>Label</key>\n\
             <string>com.xiaoyouchr.ghostdownloader</string>\n\
             <key>ProgramArguments</key>\n\
             <array>\n\
             <string>/opt/ghost/GhostDownloader</string>\n\
             <string>--silence</string>\n\
             </array>\n\
             <key>RunAtLoad</key>\n\
             <true/>\n\
             </dict>\n\
             </plist>\n"
        );
    }

    #[test]
    fn autostart_file_path_matches_python() {
        let home = PathBuf::from("/home/alice");
        assert_eq!(
            autostart_file_path(&home, DESKTOP_ID),
            PathBuf::from(
                "/home/alice/.config/autostart/io.github.xiaoyouchr.GhostDownloader.desktop"
            )
        );
    }

    #[test]
    fn launch_agent_path_matches_python() {
        assert_eq!(
            launch_agent_path(Path::new("/Users/alice")),
            PathBuf::from("/Users/alice/Library/LaunchAgents/com.xiaoyouchr.ghostdownloader.plist")
        );
    }

    #[test]
    fn set_autostart_creates_the_desktop_file() {
        let home = TempDir::new("enable");
        set_autostart(true, home.path(), DESKTOP_ID, &executable(), VERSION).expect("enable");

        let path = autostart_file_path(home.path(), DESKTOP_ID);
        assert!(path.is_file());
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            autostart_entry_text(&executable(), VERSION)
        );
    }

    #[test]
    fn set_autostart_creates_missing_parent_directories() {
        let home = TempDir::new("parents");
        // `~/.config` 都不存在。
        assert!(!home.path().join(".config").exists());
        set_autostart(true, home.path(), DESKTOP_ID, &executable(), VERSION).expect("enable");
        assert!(home.path().join(".config/autostart").is_dir());
    }

    #[test]
    fn set_autostart_disable_removes_the_file_but_keeps_the_directory() {
        let home = TempDir::new("disable");
        set_autostart(true, home.path(), DESKTOP_ID, &executable(), VERSION).expect("enable");
        let path = autostart_file_path(home.path(), DESKTOP_ID);
        assert!(path.is_file());

        set_autostart(false, home.path(), DESKTOP_ID, &executable(), VERSION).expect("disable");
        assert!(!path.exists());
        assert!(home.path().join(".config/autostart").is_dir());
    }

    #[test]
    fn set_autostart_disable_tolerates_missing_file_and_directory() {
        let home = TempDir::new("nodisable");
        set_autostart(false, home.path(), DESKTOP_ID, &executable(), VERSION).expect("no-op");
        // 目录不该被创建（禁用分支不 `mkdir`）。
        assert!(!home.path().join(".config").exists());
    }

    #[test]
    fn set_autostart_overwrites_an_existing_file() {
        let home = TempDir::new("overwrite");
        set_autostart(true, home.path(), DESKTOP_ID, &executable(), "1.0.0").expect("first");
        set_autostart(true, home.path(), DESKTOP_ID, &executable(), "4.4.0").expect("second");
        let text = fs::read_to_string(autostart_file_path(home.path(), DESKTOP_ID)).expect("read");
        assert!(text.contains("Version=4.4.0\n"));
    }

    #[test]
    fn set_launch_agent_creates_the_plist() {
        let home = TempDir::new("plist");
        set_launch_agent(true, home.path(), &executable()).expect("enable");
        let path = launch_agent_path(home.path());
        assert!(path.is_file());
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            launch_agent_text(&executable())
        );
    }

    #[test]
    fn set_launch_agent_disable_removes_the_plist_but_keeps_the_directory() {
        let home = TempDir::new("plistoff");
        set_launch_agent(true, home.path(), &executable()).expect("enable");
        set_launch_agent(false, home.path(), &executable()).expect("disable");
        assert!(!launch_agent_path(home.path()).exists());
        assert!(home.path().join("Library/LaunchAgents").is_dir());
    }

    #[test]
    fn set_launch_agent_disable_tolerates_missing_plist() {
        let home = TempDir::new("plistnone");
        set_launch_agent(false, home.path(), &executable()).expect("no-op");
        assert!(!home.path().join("Library").exists());
    }

    #[test]
    fn windows_run_value_matches_python() {
        assert_eq!(
            windows_run_value(&executable()),
            "\"/opt/ghost/GhostDownloader\" --silence"
        );
        assert_eq!(WINDOWS_VALUE_NAME, "GhostDownloader");
        assert_eq!(
            WINDOWS_RUN_KEY,
            "Software\\Microsoft\\Windows\\CurrentVersion\\Run"
        );
    }

    #[test]
    fn set_run_at_login_dispatches_to_the_current_platform() {
        let home = TempDir::new("dispatch");
        let result = set_run_at_login(true, home.path(), DESKTOP_ID, &executable(), VERSION);
        match std::env::consts::OS {
            "linux" => {
                result.expect("linux enable");
                assert!(autostart_file_path(home.path(), DESKTOP_ID).is_file());
                assert!(!launch_agent_path(home.path()).exists());
            }
            "macos" => {
                result.expect("macos enable");
                assert!(launch_agent_path(home.path()).is_file());
                assert!(!autostart_file_path(home.path(), DESKTOP_ID).exists());
            }
            "windows" => {
                let error = result.expect_err("windows is unsupported");
                assert_eq!(error.kind(), io::ErrorKind::Unsupported);
            }
            _ => result.expect("no-op platform"),
        }
    }
}
