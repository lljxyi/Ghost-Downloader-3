//! `app/platform/file_association.py` 的 **Linux 分支**。
//!
//! | Python | Rust |
//! |---|---|
//! | `loadMimeTypes`（:118-126） | [`load_mime_types`] |
//! | `saveMimeTypes`（:129-178） | [`save_mime_types`] |
//! | `_registerLinux`（:107-109） | [`register_file_types`] |
//! | `registerUrlScheme`（:32-39）的 Linux 分支 | [`register_url_scheme`] |
//! | `unregisterUrlScheme`（:42-49）的 Linux 分支 | [`unregister_url_scheme`] |
//! | `register`（:22-29）/ `_registerWindows` / `_registerUrlSchemeWindows` / `_unregisterUrlSchemeWindows` | **未迁移** |
//!
//! ## 参数化
//!
//! Python 直接读三个全局：`Path.home()`、`EXECUTABLE_PATH`、`DESKTOP_ID`。
//! Rust 侧全部作参数传入（`home` / `executable_path` / `desktop_id`），
//! 理由同第 87 步的 `app_version`：`gd3-core` 不该硬编码 `gd3-ui` 的常量。
//!
//! ## `saveMimeTypes` 的副作用被拆成返回值
//!
//! Python 在写完两份文件后**就地**跑两个外部命令（`update-desktop-database` 与
//! 每个 mime 一次 `xdg-mime default`），失败只打日志。Rust 侧把命令**按顺序返回**
//! （`Vec<Vec<String>>`），由调用方决定怎么跑：
//!
//! - `update-desktop-database` **总是**排在第一个（即使 `mimes` 为空、文件被删）；
//! - 之后是每个 mime 一条 `xdg-mime default <desktop_id>.desktop <mime>`；
//! - Python 在 `xdg-mime` 不存在时 `break`（后面的 mime 也不再试），
//!   调用方按「`NotFound` 就停」复刻即可。
//!
//! ## 与 Python 的已知差异
//!
//! 1. 异常处理：`register` / `registerUrlScheme` / `unregisterUrlScheme` 在 Python 里
//!    是 `try/except Exception` + 日志，Rust 侧返回 `io::Result`，由调用方决定是否吞掉；
//! 2. `for mime in mimes` 遍历的是 `set`，Python 的顺序取决于哈希；Rust 用
//!    `BTreeSet`（= 排序后），命令顺序**确定**但可能与某次 Python 运行不同
//!    （落盘内容不受影响，`';'.join(sorted(mimes))` 本来就排过序）；
//! 3. 路径里的非 UTF-8 字节：Python 走 `surrogateescape` 原样保留，Rust 走
//!    `Path::display()`（`to_string_lossy`）；
//! 4. `register` 的 Windows 分支（`winreg` + `ctypes.windll.shell32.SHChangeNotify`）
//!    判 **N/A**，没有可复刻的纯逻辑。
//!
//! ## 文件内容（逐字节钉住）
//!
//! `~/.local/share/applications/<desktop_id>.desktop`：
//!
//! ```text
//! [Desktop Entry]
//! Type=Application
//! Name=Ghost Downloader
//! Exec="<executable_path>" %U
//! Icon=ghost-downloader
//! Terminal=false
//! Categories=Network;Utility;
//! DBusActivatable=true
//! MimeType=<sorted(mimes) 用 ';' 连接>;
//! ```
//!
//! `~/.local/share/dbus-1/services/<desktop_id>.service`：
//!
//! ```text
//! [D-BUS Service]
//! Name=<desktop_id>
//! Exec="<executable_path>"
//! ```

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::pack::{FileType, split_lines};

/// `x-scheme-handler/` 前缀：URL scheme 在 MIME 数据库里的写法。
pub const MIME_PREFIX: &str = "x-scheme-handler/";

/// `Path.home() / ".local/share/applications" / f"{DESKTOP_ID}.desktop"`。
pub fn desktop_file_path(home: &Path, desktop_id: &str) -> PathBuf {
    home.join(".local/share/applications")
        .join(format!("{desktop_id}.desktop"))
}

/// `Path.home() / ".local/share/dbus-1/services" / f"{DESKTOP_ID}.service"`。
pub fn service_file_path(home: &Path, desktop_id: &str) -> PathBuf {
    home.join(".local/share/dbus-1/services")
        .join(format!("{desktop_id}.service"))
}

/// 镜像 `loadMimeTypes`：读第一行 `MimeType=`，按 `;` 切开、丢掉空段。
///
/// - 文件不存在 → 空集合（Python 先 `exists()` 再读）；
/// - 一行 `MimeType=` 都没有 → 空集合；
/// - **只看第一行** `MimeType=`（Python 在循环里直接 `return`）。
pub fn load_mime_types(home: &Path, desktop_id: &str) -> io::Result<BTreeSet<String>> {
    let path = desktop_file_path(home, desktop_id);
    if !path.exists() {
        return Ok(BTreeSet::new());
    }
    // `read_text(encoding="utf-8")`：非法 UTF-8 抛 `UnicodeDecodeError`。
    let text = fs::read_to_string(&path)?;
    for line in split_lines(&text) {
        if let Some(rest) = line.strip_prefix("MimeType=") {
            return Ok(rest
                .split(';')
                .filter(|mime| !mime.is_empty())
                .map(str::to_string)
                .collect());
        }
    }
    Ok(BTreeSet::new())
}

/// `.desktop` 的正文（`saveMimeTypes` 里那段 `write_text`）。
pub fn desktop_entry_text(executable_path: &Path, mime_types: &BTreeSet<String>) -> String {
    let joined = mime_types
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(";");
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Ghost Downloader\n\
         Exec=\"{}\" %U\n\
         Icon=ghost-downloader\n\
         Terminal=false\n\
         Categories=Network;Utility;\n\
         DBusActivatable=true\n\
         MimeType={joined};\n",
        executable_path.display()
    )
}

/// `.service` 的正文（D-Bus 激活用）。
pub fn service_entry_text(executable_path: &Path, desktop_id: &str) -> String {
    format!(
        "[D-BUS Service]\n\
         Name={desktop_id}\n\
         Exec=\"{}\"\n",
        executable_path.display()
    )
}

/// 镜像 `saveMimeTypes`：写两份文件（或删掉它们），返回**待执行**的命令列表。
///
/// 返回的命令按 Python 的执行顺序排列，见模块头。
pub fn save_mime_types(
    mime_types: &BTreeSet<String>,
    home: &Path,
    desktop_id: &str,
    executable_path: &Path,
) -> io::Result<Vec<Vec<String>>> {
    let applications_dir = home.join(".local/share/applications");
    let services_dir = home.join(".local/share/dbus-1/services");
    let desktop_file = desktop_file_path(home, desktop_id);
    let service_file = service_file_path(home, desktop_id);

    if mime_types.is_empty() {
        // `unlink(missing_ok=True)`：不存在不算错，目录也不删。
        remove_if_exists(&desktop_file)?;
        remove_if_exists(&service_file)?;
    } else {
        fs::create_dir_all(&applications_dir)?;
        fs::create_dir_all(&services_dir)?;
        fs::write(
            &desktop_file,
            desktop_entry_text(executable_path, mime_types),
        )?;
        fs::write(
            &service_file,
            service_entry_text(executable_path, desktop_id),
        )?;
    }

    let mut commands = vec![vec![
        "update-desktop-database".to_string(),
        applications_dir.display().to_string(),
    ]];
    for mime in mime_types {
        commands.push(vec![
            "xdg-mime".to_string(),
            "default".to_string(),
            format!("{desktop_id}.desktop"),
            mime.clone(),
        ]);
    }
    Ok(commands)
}

/// 镜像 `_registerLinux`：**保留**已有 MIME 里所有 `x-scheme-handler/*`，
/// 再加上每个 `FileType` 的 `mimeType`。
///
/// 注意：已有 MIME 里**不是** scheme 的那些会被丢掉（Python 就是这么写的，
/// `saveMimeTypes(schemes | {ft.mimeType for ft in fileTypes})`）。
pub fn register_file_types(
    file_types: &[FileType],
    home: &Path,
    desktop_id: &str,
    executable_path: &Path,
) -> io::Result<Vec<Vec<String>>> {
    let mut mime_types: BTreeSet<String> = load_mime_types(home, desktop_id)?
        .into_iter()
        .filter(|mime| mime.starts_with(MIME_PREFIX))
        .collect();
    for file_type in file_types {
        mime_types.insert(file_type.mime_type.clone());
    }
    save_mime_types(&mime_types, home, desktop_id, executable_path)
}

/// 镜像 `registerUrlScheme` 的 Linux 分支：在已有 MIME 上并上
/// `x-scheme-handler/<scheme>`（**不动**其它已有 MIME）。
pub fn register_url_scheme(
    scheme: &str,
    home: &Path,
    desktop_id: &str,
    executable_path: &Path,
) -> io::Result<Vec<Vec<String>>> {
    let mut mime_types = load_mime_types(home, desktop_id)?;
    mime_types.insert(format!("{MIME_PREFIX}{scheme}"));
    save_mime_types(&mime_types, home, desktop_id, executable_path)
}

/// 镜像 `unregisterUrlScheme` 的 Linux 分支：从已有 MIME 里去掉
/// `x-scheme-handler/<scheme>`（**保留**其它）。
pub fn unregister_url_scheme(
    scheme: &str,
    home: &Path,
    desktop_id: &str,
    executable_path: &Path,
) -> io::Result<Vec<Vec<String>>> {
    let mut mime_types = load_mime_types(home, desktop_id)?;
    mime_types.remove(&format!("{MIME_PREFIX}{scheme}"));
    save_mime_types(&mime_types, home, desktop_id, executable_path)
}

/// `Path.unlink(missing_ok=True)`：只有 `NotFound` 被吞掉，其它错误照抛。
///
/// `crate` 内共用（`run_at_login` 的禁用分支也要这一份语义）。
pub(crate) fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
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
            let dir = env::temp_dir().join(format!("gd3fa-{}-{tag}-{id}", std::process::id()));
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

    fn executable() -> PathBuf {
        PathBuf::from("/opt/ghost/GhostDownloader")
    }

    fn mimes(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    fn file_type(mime: &str) -> FileType {
        FileType {
            extensions: vec![".ghd".to_string()],
            display_name: "Ghost Downloader Task".to_string(),
            mime_type: mime.to_string(),
            icon: "ghd".to_string(),
        }
    }

    #[test]
    fn load_mime_types_returns_empty_when_the_desktop_file_is_absent() {
        let home = TempDir::new("absent");
        assert_eq!(
            load_mime_types(home.path(), DESKTOP_ID).expect("load"),
            BTreeSet::new()
        );
    }

    #[test]
    fn load_mime_types_reads_only_the_first_mime_type_line() {
        let home = TempDir::new("first");
        let path = desktop_file_path(home.path(), DESKTOP_ID);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(
            &path,
            "[Desktop Entry]\nName=x\nMimeType=a;b;;c;\nMimeType=z;\n",
        )
        .expect("write");

        // 空段被丢掉；第二行 `MimeType=` 被忽略。
        assert_eq!(
            load_mime_types(home.path(), DESKTOP_ID).expect("load"),
            mimes(&["a", "b", "c"])
        );
    }

    #[test]
    fn load_mime_types_returns_empty_without_a_mime_type_line() {
        let home = TempDir::new("nomime");
        let path = desktop_file_path(home.path(), DESKTOP_ID);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, "[Desktop Entry]\nName=x\n").expect("write");
        assert_eq!(
            load_mime_types(home.path(), DESKTOP_ID).expect("load"),
            BTreeSet::new()
        );
    }

    #[test]
    fn desktop_entry_text_is_byte_identical_to_python() {
        let text = desktop_entry_text(&executable(), &mimes(&["image/png", "text/plain"]));
        assert_eq!(
            text,
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Ghost Downloader\n\
             Exec=\"/opt/ghost/GhostDownloader\" %U\n\
             Icon=ghost-downloader\n\
             Terminal=false\n\
             Categories=Network;Utility;\n\
             DBusActivatable=true\n\
             MimeType=image/png;text/plain;\n"
        );
    }

    #[test]
    fn service_entry_text_is_byte_identical_to_python() {
        assert_eq!(
            service_entry_text(&executable(), DESKTOP_ID),
            format!("[D-BUS Service]\nName={DESKTOP_ID}\nExec=\"/opt/ghost/GhostDownloader\"\n")
        );
    }

    #[test]
    fn save_mime_types_writes_both_files_and_returns_commands() {
        let home = TempDir::new("save");
        let commands = save_mime_types(
            &mimes(&["text/plain", "image/png"]),
            home.path(),
            DESKTOP_ID,
            &executable(),
        )
        .expect("save");

        let desktop_file = desktop_file_path(home.path(), DESKTOP_ID);
        let service_file = service_file_path(home.path(), DESKTOP_ID);
        assert_eq!(
            fs::read_to_string(&desktop_file).expect("read"),
            desktop_entry_text(&executable(), &mimes(&["image/png", "text/plain"]))
        );
        assert_eq!(
            fs::read_to_string(&service_file).expect("read"),
            service_entry_text(&executable(), DESKTOP_ID)
        );

        // 命令：`update-desktop-database` 打头，之后每个 mime 一条 `xdg-mime`（排序后）。
        let applications = home.path().join(".local/share/applications");
        assert_eq!(
            commands,
            vec![
                vec![
                    "update-desktop-database".to_string(),
                    applications.display().to_string(),
                ],
                vec![
                    "xdg-mime".to_string(),
                    "default".to_string(),
                    format!("{DESKTOP_ID}.desktop"),
                    "image/png".to_string(),
                ],
                vec![
                    "xdg-mime".to_string(),
                    "default".to_string(),
                    format!("{DESKTOP_ID}.desktop"),
                    "text/plain".to_string(),
                ],
            ]
        );
    }

    #[test]
    fn save_mime_types_with_no_mimes_removes_both_files() {
        let home = TempDir::new("clear");
        save_mime_types(
            &mimes(&["text/plain"]),
            home.path(),
            DESKTOP_ID,
            &executable(),
        )
        .expect("save");
        let desktop_file = desktop_file_path(home.path(), DESKTOP_ID);
        let service_file = service_file_path(home.path(), DESKTOP_ID);
        assert!(desktop_file.exists());
        assert!(service_file.exists());

        let commands = save_mime_types(&BTreeSet::new(), home.path(), DESKTOP_ID, &executable())
            .expect("clear");
        assert!(!desktop_file.exists());
        assert!(!service_file.exists());
        // 目录留着（`unlink` 不删目录），`update-desktop-database` 照样跑。
        assert!(home.path().join(".local/share/applications").is_dir());
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0][0], "update-desktop-database");
    }

    #[test]
    fn save_mime_types_tolerates_missing_files() {
        let home = TempDir::new("missing");
        let commands = save_mime_types(&BTreeSet::new(), home.path(), DESKTOP_ID, &executable())
            .expect("no-op");
        assert_eq!(commands.len(), 1);
        // 目录都不该被创建（Python 在空集合时先 `unlink` 后跑命令，不 `mkdir`）。
        assert!(!home.path().join(".local/share").exists());
    }

    #[test]
    fn register_url_scheme_unions_with_existing_mimes() {
        let home = TempDir::new("scheme");
        save_mime_types(
            &mimes(&["x-scheme-handler/other", "text/plain"]),
            home.path(),
            DESKTOP_ID,
            &executable(),
        )
        .expect("seed");

        register_url_scheme("ghostdownloader", home.path(), DESKTOP_ID, &executable())
            .expect("register");
        assert_eq!(
            load_mime_types(home.path(), DESKTOP_ID).expect("load"),
            mimes(&[
                "text/plain",
                "x-scheme-handler/other",
                "x-scheme-handler/ghostdownloader"
            ])
        );
    }

    #[test]
    fn unregister_url_scheme_keeps_the_others() {
        let home = TempDir::new("unscheme");
        save_mime_types(
            &mimes(&["x-scheme-handler/ghostdownloader", "text/plain"]),
            home.path(),
            DESKTOP_ID,
            &executable(),
        )
        .expect("seed");

        unregister_url_scheme("ghostdownloader", home.path(), DESKTOP_ID, &executable())
            .expect("unregister");
        assert_eq!(
            load_mime_types(home.path(), DESKTOP_ID).expect("load"),
            mimes(&["text/plain"])
        );
    }

    #[test]
    fn unregister_url_scheme_with_nothing_left_removes_the_files() {
        let home = TempDir::new("unlast");
        register_url_scheme("ghostdownloader", home.path(), DESKTOP_ID, &executable())
            .expect("register");
        unregister_url_scheme("ghostdownloader", home.path(), DESKTOP_ID, &executable())
            .expect("unregister");
        assert!(!desktop_file_path(home.path(), DESKTOP_ID).exists());
        assert!(!service_file_path(home.path(), DESKTOP_ID).exists());
    }

    #[test]
    fn register_file_types_keeps_only_scheme_mimes_plus_the_file_types() {
        let home = TempDir::new("ftypes");
        save_mime_types(
            &mimes(&[
                "x-scheme-handler/ghostdownloader",
                "text/plain",
                "image/png",
            ]),
            home.path(),
            DESKTOP_ID,
            &executable(),
        )
        .expect("seed");

        register_file_types(
            &[file_type("application/x-ghd")],
            home.path(),
            DESKTOP_ID,
            &executable(),
        )
        .expect("register");

        // `text/plain` / `image/png` 被丢掉（`_registerLinux` 只保留 scheme）。
        assert_eq!(
            load_mime_types(home.path(), DESKTOP_ID).expect("load"),
            mimes(&["application/x-ghd", "x-scheme-handler/ghostdownloader"])
        );
    }

    #[test]
    fn register_file_types_without_any_keeps_nothing_and_deletes_the_files() {
        let home = TempDir::new("ftypesempty");
        save_mime_types(
            &mimes(&["text/plain"]),
            home.path(),
            DESKTOP_ID,
            &executable(),
        )
        .expect("seed");
        register_file_types(&[], home.path(), DESKTOP_ID, &executable()).expect("register");
        assert!(!desktop_file_path(home.path(), DESKTOP_ID).exists());
    }

    #[test]
    fn save_mime_types_is_idempotent() {
        let home = TempDir::new("idem");
        let mimes = mimes(&["x-scheme-handler/ghostdownloader", "application/x-ghd"]);
        save_mime_types(&mimes, home.path(), DESKTOP_ID, &executable()).expect("first");
        let first = fs::read_to_string(desktop_file_path(home.path(), DESKTOP_ID)).expect("read");
        save_mime_types(&mimes, home.path(), DESKTOP_ID, &executable()).expect("second");
        let second = fs::read_to_string(desktop_file_path(home.path(), DESKTOP_ID)).expect("read");
        assert_eq!(first, second);
    }
}
