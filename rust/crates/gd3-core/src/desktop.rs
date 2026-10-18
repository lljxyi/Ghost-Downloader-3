//! 桌面动作（`app/platform/desktop.py`，405 行）里**不依赖 Qt / Win32 ctypes** 的部分。
//!
//! Python 这份文件里大半是 `QDesktopServices` / `QProcess` / `ctypes` + COM 的东西，
//! Rust 侧没有等价物；能搬的是「选哪条命令、参数是什么」这一层：
//!
//! | Python | 这里 |
//! | --- | --- |
//! | `shutdown`（:145-153） | [`shutdown_command_on`] / [`shutdown_command`] |
//! | `restart`（:156-164） | [`restart_command_on`] / [`restart_command`] |
//! | `sleep`（:167-175） | [`sleep_command_on`] / [`sleep_command`] |
//! | `openFile`（:121-123） | [`open_file_command_on`] / [`open_file_command`] |
//! | `openFolder`（:126-127） | [`open_folder_command_on`] / [`open_folder_command`] |
//! | `revealInFolder`（:130-142） | [`reveal_in_folder_on`] / [`reveal_in_folder`] |
//! | `openChromiumUrl`（:178-234） | [`open_chromium_url_on`] / [`open_chromium_url`] |
//! | `Popen` / `QProcess.startDetached` | [`spawn_detached`] |
//! | `buildFontFamilies`（:15-45） | **未迁移**：入参是 `QLocale` 的 `script()` / `territory()` |
//! | `deleteRecoverably`（:112-114） | **未迁移**：`QFile.moveToTrash` 要 freedesktop 回收站实现 |
//! | `_sendForegroundInput` / `_revealInFolderShell` / `requestForeground` / `raiseWindow` / `startFileDrag` | **N/A**：纯 Win32 `ctypes` + Qt 窗口 |
//!
//! # 设计
//!
//! 每个动作拆成两层：
//!
//! 1. `*_on(os, ...)`：**纯函数**，`os` 就是 `"linux"` / `"macos"` / `"windows"`
//!    （= `std::env::consts::OS` 的取值，也正好是 `sys.platform` 的 `linux` / `darwin`
//!    / `win32` 的另一种写法），返回命令 argv；
//! 2. 不带 `_on` 的包装：按 `std::env::consts::OS` 分发。
//!
//! 这样三条平台的命令表都能在本机（Linux）被单测钉住，而不是靠 `cfg!` 分支。
//!
//! **`os` 的判定照抄 Python 的 `match sys.platform`**：只认 `win32` / `darwin` 两个具名
//! 分支，**其余一律落进 `case _`**（即 POSIX 分支）。所以 `"linux"` 与 `"freebsd"`、
//! `"openbsd7"`、甚至空串走的是同一条命令——这一点有单测钉住，别按「未知平台就返回
//! `None`」去改，那是另一种行为。
//!
//! `openChromiumUrl` 的「浏览器装没装」通过 `which` 闭包注入：
//! [`open_chromium_url`] 用 [`crate::filesystem::which`] 查 `PATH`，
//! 单测则传一个只认识几个名字的假 `which`，不依赖本机装了什么浏览器。
//!
//! # 与 Python 的已知差异
//!
//! 1. **Windows 的 `openFile` / `openFolder` / `revealInFolder` / `openChromiumUrl`
//!    未迁移**：Python 走 `QDesktopServices`（内部 `ShellExecute`）、
//!    `SHOpenFolderAndSelectItems`、`winreg` 查 `App Paths`，都需要 Win32 API。
//!    `*_on("windows", ...)` 对这些返回 `None`；`shutdown` / `restart` / `sleep`
//!    是纯命令表，三平台都在。
//! 2. Python 的 `openFile` / `openFolder` 用 `QDesktopServices.openUrl`，
//!    Linux 上 Qt 内部也是走 `xdg-open`（`xdg-open` 缺失时退回 `gio open` / 桌面环境自己的
//!    handler）；这里直接写 `xdg-open`，缺它时 [`spawn_detached`] 返回 `NotFound`。
//! 3. `QProcess.startDetached` 会**脱离**父进程的会话；[`spawn_detached`] 用
//!    `setsid(2)`（`pre_exec`）复刻，并且 stdin/stdout/stderr 全部接 `/dev/null`。
//!    与 Python 的 `Popen` 一样**不等子进程**，所以子进程退出后会短暂留一个僵尸，
//!    直到父进程退出（Python 侧同样如此，未做 `SIGCHLD` 处理）。
//! 4. `openChromiumUrl` 在 Linux 上是「`which` 命中第一个就 `Popen` 并返回 `True`」；
//!    [`open_chromium_url_on`] 只负责**挑**命令，`Popen` 失败时 Python 会 `continue`
//!    试下一个，这里把重试留给调用方（它拿到 `spawn_detached` 的 `Err`）。
//! 5. macOS 分支 Python 是 `subprocess.run(["open", "-a", app, url])` 看返回码，
//!    即「同步试装」；这里统一抽象成 `which(app)`，不真去 `open -a` 试。

use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::filesystem::{is_existing, which};

/// Linux 上按顺序尝试的浏览器命令与对应的 URL scheme。
///
/// 顺序即 Python `desktop.py:219-227` 的字面顺序（命中第一个就返回）。
pub const LINUX_BROWSERS: &[(&str, &str)] = &[
    ("google-chrome", "chrome"),
    ("google-chrome-stable", "chrome"),
    ("microsoft-edge-stable", "edge"),
    ("microsoft-edge", "edge"),
    ("brave-browser", "brave"),
    ("chromium-browser", "chrome"),
    ("chromium", "chrome"),
];

/// macOS 上按顺序尝试的浏览器 App 名与对应的 URL scheme（`desktop.py:189-194`）。
pub const MACOS_BROWSERS: &[(&str, &str)] = &[
    ("Google Chrome", "chrome"),
    ("Microsoft Edge", "edge"),
    ("Brave Browser", "brave"),
    ("Chromium", "chrome"),
];

/// `toBrowserUrl`：`chrome://` 前缀换成该浏览器自己的 scheme，其它原样。
///
/// `url[9:]` 就是 `strip_prefix("chrome://")`；`"chrome://"` 本身（长度 9）换出来是
/// `"{scheme}://"`。大小写敏感（`startswith` 不做大小写折叠）。
pub fn browser_url(url: &str, scheme: &str) -> String {
    match url.strip_prefix("chrome://") {
        Some(rest) => format!("{scheme}://{rest}"),
        None => url.to_string(),
    }
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| part.to_string()).collect()
}

/// `shutdown` 的命令表。
pub fn shutdown_command_on(os: &str) -> Vec<String> {
    match os {
        "windows" => argv(&["shutdown", "/s", "/t", "0"]),
        "macos" => argv(&["osascript", "-e", "tell app \"System Events\" to shut down"]),
        _ => argv(&["shutdown", "-h", "now"]),
    }
}

/// `restart` 的命令表。
pub fn restart_command_on(os: &str) -> Vec<String> {
    match os {
        "windows" => argv(&["shutdown", "/r", "/t", "0"]),
        "macos" => argv(&["osascript", "-e", "tell app \"System Events\" to restart"]),
        _ => argv(&["shutdown", "-r", "now"]),
    }
}

/// `sleep` 的命令表。
pub fn sleep_command_on(os: &str) -> Vec<String> {
    match os {
        "windows" => argv(&[
            "rundll32.exe",
            "powrprof.dll,SetSuspendState",
            "0",
            "1",
            "0",
        ]),
        "macos" => argv(&["pmset", "sleepnow"]),
        _ => argv(&["systemctl", "suspend"]),
    }
}

/// `openFile`：Windows 未迁移（`ShellExecute`）。
pub fn open_file_command_on(os: &str, path: &Path) -> Option<Vec<String>> {
    open_target_command_on(os, path)
}

/// `openFolder`：Windows 未迁移（`ShellExecute`）。
pub fn open_folder_command_on(os: &str, path: &Path) -> Option<Vec<String>> {
    open_target_command_on(os, path)
}

/// `openFile` / `openFolder` 的共同形态：Linux `xdg-open`、macOS `open`。
fn open_target_command_on(os: &str, path: &Path) -> Option<Vec<String>> {
    let opener = match os {
        "windows" => return None,
        "macos" => "open",
        _ => "xdg-open",
    };
    Some(vec![opener.to_string(), path.display().to_string()])
}

/// 镜像 `str(Path(path).parent)`。
///
/// Rust 的 `Path::new("foo").parent()` 是**空路径**（`display()` 出来是空串），
/// Python 的 `Path("foo").parent` 是 `Path(".")`；`Path::new("/").parent()` 是 `None`，
/// Python 的 `Path("/").parent` 还是 `Path("/")`。`revealInFolder` 的 Linux 分支
/// 直接把这个值塞进 `xdg-open`，所以两种边界都得对齐。
fn parent_display(path: &Path) -> String {
    match path.parent() {
        Some(parent) if parent.as_os_str().is_empty() => ".".to_string(),
        Some(parent) => parent.display().to_string(),
        // 只有根路径（和空路径）没有 parent
        None if path.as_os_str().is_empty() => ".".to_string(),
        None => path.display().to_string(),
    }
}

/// `revealInFolder` 的命令表。
///
/// 镜像 `desktop.py:130-142` 的两段判定：
///
/// - 路径**存在** → 各平台「在文件夹里显示」的那条命令；
/// - 路径不存在但**父目录存在** → 退化成 `openFolder(parent)`；
/// - 都不存在 → `None`（Python 什么都不做）。
pub fn reveal_in_folder_on(os: &str, path: &Path) -> Option<Vec<String>> {
    if is_existing(path) {
        return match os {
            "macos" => Some(vec![
                "open".to_string(),
                "-R".to_string(),
                path.display().to_string(),
            ]),
            // Windows：`SHOpenFolderAndSelectItems`，未迁移。
            "windows" => None,
            // Linux：`QProcess.startDetached("xdg-open", [str(path.parent)])`。
            _ => Some(vec!["xdg-open".to_string(), parent_display(path)]),
        };
    }
    let parent = path.parent()?;
    if is_existing(parent) {
        return open_folder_command_on(os, parent);
    }
    None
}

/// `openChromiumUrl` 在 Linux / macOS 上挑一条命令；Windows 未迁移（`winreg`）。
///
/// `which` 决定「这个命令/App 在不在」：Linux 用 `shutil.which`，macOS 用 `open -a`
/// 的返回码（这里统一抽象成「有没有」）。命中第一个就返回。
pub fn open_chromium_url_on(
    os: &str,
    url: &str,
    which: impl Fn(&str) -> bool,
) -> Option<Vec<String>> {
    let table: &[(&str, &str)] = match os {
        "windows" => return None,
        "macos" => MACOS_BROWSERS,
        _ => LINUX_BROWSERS,
    };
    for (command, scheme) in table {
        if !which(command) {
            continue;
        }
        let target = browser_url(url, scheme);
        return Some(match os {
            "macos" => vec![
                "open".to_string(),
                "-a".to_string(),
                command.to_string(),
                target,
            ],
            _ => vec![command.to_string(), target],
        });
    }
    None
}

/// [`open_chromium_url_on`] 的当前平台版本，`which` 走 `PATH`。
pub fn open_chromium_url(url: &str) -> Option<Vec<String>> {
    open_chromium_url_on(std::env::consts::OS, url, |command| {
        which(command).is_some()
    })
}

/// `shutdown`（当前平台）。
pub fn shutdown_command() -> Vec<String> {
    shutdown_command_on(std::env::consts::OS)
}

/// `restart`（当前平台）。
pub fn restart_command() -> Vec<String> {
    restart_command_on(std::env::consts::OS)
}

/// `sleep`（当前平台）。
pub fn sleep_command() -> Vec<String> {
    sleep_command_on(std::env::consts::OS)
}

/// `openFile`（当前平台）。
pub fn open_file_command(path: &Path) -> Option<Vec<String>> {
    open_file_command_on(std::env::consts::OS, path)
}

/// `openFolder`（当前平台）。
pub fn open_folder_command(path: &Path) -> Option<Vec<String>> {
    open_folder_command_on(std::env::consts::OS, path)
}

/// `revealInFolder`（当前平台）。
pub fn reveal_in_folder(path: &Path) -> Option<Vec<String>> {
    reveal_in_folder_on(std::env::consts::OS, path)
}

/// `Popen` / `QProcess.startDetached`：起一个**不等**的进程。
///
/// Unix 上 `setsid(2)` 让子进程脱离当前会话（父进程退出不会被带走，也不会收到
/// 终端的 `SIGHUP`）；三路标准流全部接 `/dev/null`，避免子进程往父进程的管道里写。
///
/// 空命令返回 `InvalidInput`（`Command::new("")` 在 Linux 上也会是 `NotFound`，
/// 但显式一点更好定位）。子进程是否起来只看 `spawn` 是否成功——**不等它退出**，
/// 所以拿不到退出码（Python 的 `Popen` 也一样，`QProcess.startDetached` 更彻底）。
pub fn spawn_detached(command: &[String]) -> io::Result<()> {
    let (program, args) = match command.split_first() {
        Some(split) => split,
        None => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "spawn_detached 需要至少一个参数（程序名）",
            ));
        }
    };
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt as _;
        cmd.pre_exec(|| {
            // `setsid` 只在调用者不是进程组组长时成功；失败也不影响后续 `exec`。
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn().map(|_child| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::fs;
    use std::io;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = env::temp_dir().join(format!("gd3dsk-{}-{tag}-{id}", std::process::id()));
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

    /// 只认识给定名字的假 `which`（对应 `shutil.which` 命中）。
    fn which_only<'a>(known: &'a [&'a str]) -> impl Fn(&str) -> bool + 'a {
        move |name| known.contains(&name)
    }

    fn strings(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| part.to_string()).collect()
    }

    #[test]
    fn linux_browser_table_matches_python() {
        // 顺序即 `desktop.py:219-227` 的字面顺序：命中第一个就返回，顺序不能动。
        assert_eq!(
            LINUX_BROWSERS,
            &[
                ("google-chrome", "chrome"),
                ("google-chrome-stable", "chrome"),
                ("microsoft-edge-stable", "edge"),
                ("microsoft-edge", "edge"),
                ("brave-browser", "brave"),
                ("chromium-browser", "chrome"),
                ("chromium", "chrome"),
            ]
        );
    }

    #[test]
    fn macos_browser_table_matches_python() {
        // `desktop.py:189-194`
        assert_eq!(
            MACOS_BROWSERS,
            &[
                ("Google Chrome", "chrome"),
                ("Microsoft Edge", "edge"),
                ("Brave Browser", "brave"),
                ("Chromium", "chrome"),
            ]
        );
    }

    #[test]
    fn browser_url_only_rewrites_chrome_prefix() {
        assert_eq!(
            browser_url("chrome://extensions", "chrome"),
            "chrome://extensions"
        );
        assert_eq!(
            browser_url("chrome://extensions", "edge"),
            "edge://extensions"
        );
        // 长度正好 9 → 后半段是空串
        assert_eq!(browser_url("chrome://", "brave"), "brave://");
        // 只剥一次前缀
        assert_eq!(
            browser_url("chrome://chrome://x", "edge"),
            "edge://chrome://x"
        );
        // 大小写敏感、非 chrome:// 一律原样
        assert_eq!(
            browser_url("Chrome://extensions", "edge"),
            "Chrome://extensions"
        );
        assert_eq!(
            browser_url("https://example.com", "edge"),
            "https://example.com"
        );
        assert_eq!(browser_url("", "chrome"), "");
    }

    #[test]
    fn power_commands_match_python() {
        assert_eq!(
            shutdown_command_on("windows"),
            ["shutdown", "/s", "/t", "0"]
        );
        assert_eq!(
            shutdown_command_on("macos"),
            ["osascript", "-e", "tell app \"System Events\" to shut down"]
        );
        assert_eq!(shutdown_command_on("linux"), ["shutdown", "-h", "now"]);

        assert_eq!(restart_command_on("windows"), ["shutdown", "/r", "/t", "0"]);
        assert_eq!(
            restart_command_on("macos"),
            ["osascript", "-e", "tell app \"System Events\" to restart"]
        );
        assert_eq!(restart_command_on("linux"), ["shutdown", "-r", "now"]);

        assert_eq!(
            sleep_command_on("windows"),
            [
                "rundll32.exe",
                "powrprof.dll,SetSuspendState",
                "0",
                "1",
                "0"
            ]
        );
        assert_eq!(sleep_command_on("macos"), ["pmset", "sleepnow"]);
        assert_eq!(sleep_command_on("linux"), ["systemctl", "suspend"]);
    }

    #[test]
    fn unknown_platform_falls_into_posix_branch() {
        // Python 的 `match sys.platform` 只有 `win32` / `darwin` 两个具名分支，
        // 其余（含 freebsd / 空串）全部落进 `case _` = POSIX 分支。
        for os in ["linux", "freebsd13", "openbsd7", "android", ""] {
            assert_eq!(
                shutdown_command_on(os),
                ["shutdown", "-h", "now"],
                "os = {os}"
            );
            assert_eq!(
                restart_command_on(os),
                ["shutdown", "-r", "now"],
                "os = {os}"
            );
            assert_eq!(sleep_command_on(os), ["systemctl", "suspend"], "os = {os}");
            assert_eq!(
                open_file_command_on(os, Path::new("/tmp/x")),
                Some(strings(&["xdg-open", "/tmp/x"])),
                "os = {os}"
            );
            assert_eq!(
                open_chromium_url_on(os, "https://x", which_only(&["chromium"])),
                Some(strings(&["chromium", "https://x"])),
                "os = {os}"
            );
        }
    }

    #[test]
    fn open_target_commands_match_python() {
        let path = Path::new("/tmp/a.txt");
        assert_eq!(
            open_file_command_on("linux", path),
            Some(strings(&["xdg-open", "/tmp/a.txt"]))
        );
        assert_eq!(
            open_folder_command_on("linux", path),
            Some(strings(&["xdg-open", "/tmp/a.txt"]))
        );
        assert_eq!(
            open_file_command_on("macos", path),
            Some(strings(&["open", "/tmp/a.txt"]))
        );
        assert_eq!(
            open_folder_command_on("macos", path),
            Some(strings(&["open", "/tmp/a.txt"]))
        );
        // Windows：`QDesktopServices.openUrl` 要 `ShellExecute`，未迁移
        assert_eq!(open_file_command_on("windows", path), None);
        assert_eq!(open_folder_command_on("windows", path), None);
    }

    #[test]
    fn parent_display_mirrors_python_path_parent() {
        // Python: str(Path("foo").parent) == "."
        assert_eq!(parent_display(Path::new("foo")), ".");
        assert_eq!(parent_display(Path::new("a/b")), "a");
        assert_eq!(parent_display(Path::new("/a/b")), "/a");
        // Path::new("/").parent() 是 None，Python 里 Path("/").parent 还是 Path("/")
        assert_eq!(parent_display(Path::new("/")), "/");
        assert_eq!(parent_display(Path::new("")), ".");
        assert_eq!(parent_display(Path::new("/a/b/")), "/a");
    }

    #[test]
    fn reveal_in_folder_three_branches() {
        let dir = TempDir::new("reveal");
        let folder = dir.path().display().to_string();
        let file = dir.path().join("a.txt");
        fs::write(&file, b"x").expect("write");
        let file_str = file.display().to_string();

        // 分支 1：路径存在
        assert_eq!(
            reveal_in_folder_on("linux", &file),
            Some(strings(&["xdg-open", &folder]))
        );
        assert_eq!(
            reveal_in_folder_on("macos", &file),
            Some(strings(&["open", "-R", &file_str]))
        );
        assert_eq!(reveal_in_folder_on("windows", &file), None);

        // 分支 2：路径不存在但父目录存在 → openFolder(parent)
        let ghost = dir.path().join("ghost.log");
        assert_eq!(
            reveal_in_folder_on("linux", &ghost),
            Some(strings(&["xdg-open", &folder]))
        );
        assert_eq!(
            reveal_in_folder_on("macos", &ghost),
            Some(strings(&["open", &folder]))
        );
        assert_eq!(reveal_in_folder_on("windows", &ghost), None);

        // 分支 3：路径与父目录都不存在 → Python 什么都不做
        let missing = dir.path().join("nope").join("ghost.log");
        assert_eq!(reveal_in_folder_on("linux", &missing), None);
        assert_eq!(reveal_in_folder_on("macos", &missing), None);
        assert_eq!(reveal_in_folder_on("windows", &missing), None);
    }

    #[test]
    fn reveal_in_folder_opens_directory_itself_when_it_exists() {
        // 目录存在时 Linux 走 `xdg-open str(parent)`，不是打开它自己
        let dir = TempDir::new("reveal-dir");
        let inner = dir.path().join("inner");
        fs::create_dir(&inner).expect("mkdir");
        assert_eq!(
            reveal_in_folder_on("linux", &inner),
            Some(strings(&["xdg-open", &dir.path().display().to_string()]))
        );
        // macOS 是 `open -R <目录>`（在 Finder 里选中它）
        assert_eq!(
            reveal_in_folder_on("macos", &inner),
            Some(strings(&["open", "-R", &inner.display().to_string()]))
        );
    }

    #[test]
    fn open_chromium_url_picks_first_installed() {
        let url = "chrome://extensions";
        // 只装了 brave-browser → 用 brave scheme
        assert_eq!(
            open_chromium_url_on("linux", url, which_only(&["brave-browser"])),
            Some(strings(&["brave-browser", "brave://extensions"]))
        );
        // 同时装了 chromium 与 microsoft-edge → 按表序命中 microsoft-edge
        assert_eq!(
            open_chromium_url_on("linux", url, which_only(&["chromium", "microsoft-edge"])),
            Some(strings(&["microsoft-edge", "edge://extensions"]))
        );
        // 一个都没装 → None（Python 返回 False）
        assert_eq!(open_chromium_url_on("linux", url, |_| false), None);
        // Windows：`winreg` 查 `App Paths`，未迁移
        assert_eq!(open_chromium_url_on("windows", url, |_| true), None);
    }

    #[test]
    fn open_chromium_url_macos_uses_open_dash_a() {
        assert_eq!(
            open_chromium_url_on(
                "macos",
                "chrome://extensions",
                which_only(&["Google Chrome"])
            ),
            Some(strings(&[
                "open",
                "-a",
                "Google Chrome",
                "chrome://extensions"
            ]))
        );
        // 非 chrome:// 的 URL 原样传
        assert_eq!(
            open_chromium_url_on(
                "macos",
                "https://example.com",
                which_only(&["Brave Browser"])
            ),
            Some(strings(&[
                "open",
                "-a",
                "Brave Browser",
                "https://example.com"
            ]))
        );
        // macOS 表里没有的名字不算命中
        assert_eq!(
            open_chromium_url_on("macos", "https://x", which_only(&["chromium"])),
            None
        );
    }

    #[test]
    fn platform_wrappers_match_current_os() {
        let os = env::consts::OS;
        assert_eq!(shutdown_command(), shutdown_command_on(os));
        assert_eq!(restart_command(), restart_command_on(os));
        assert_eq!(sleep_command(), sleep_command_on(os));
        let path = Path::new("/tmp/x");
        assert_eq!(open_file_command(path), open_file_command_on(os, path));
        assert_eq!(open_folder_command(path), open_folder_command_on(os, path));
    }

    #[test]
    fn spawn_detached_rejects_empty_command() {
        let error = spawn_detached(&[]).expect_err("空命令要报错");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn spawn_detached_does_not_wait_for_child() {
        // `sh -c "exit 7"` 立刻退出；我们不等它，只看 spawn 成功
        spawn_detached(&strings(&["sh", "-c", "exit 7"])).expect("spawn sh");
        spawn_detached(&strings(&["true"])).expect("spawn true");
        let error = spawn_detached(&strings(&["gd3-no-such-program-xyz"])).expect_err("要报错");
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }
}
