//! `app/platform/desktop.py:112-114` 的 `deleteRecoverably` → freedesktop 回收站。
//!
//! Python 侧只有两行：
//!
//! ```python
//! def deleteRecoverably(path: str) -> bool:
//!     return QFile.moveToTrash(path)
//! ```
//!
//! 所以本模块要复刻的不是 Python 代码，而是 **Qt 的 `QFile::moveToTrash`**。
//! 下面每一条口径都是第 91 步用 PySide6 + `QT_QPA_PLATFORM=offscreen` 真跑实测出来的
//! （探针目录 `/root/gd3probe2..10`），不是照 freedesktop 规范推测的：
//!
//! | 实测项 | 口径 |
//! |---|---|
//! | 回收站根 | `QStandardPaths::GenericDataLocation` + `/Trash`，即 `$XDG_DATA_HOME/Trash`（未设 → `$HOME/.local/share/Trash`） |
//! | `$XDG_DATA_HOME` 不存在 | 直接返回 `False`，**什么都不建**（不自动补父目录） |
//! | 目录结构 | `Trash/{files,info}`，三个目录都是 `0700` |
//! | `.trashinfo` | `0644`（umask 口径），正文见 [`trash_info_text`] |
//! | 被移入的文件 | 原 mode 保留（`rename`，不是 copy） |
//! | `Path=`（home） | **原样**：传 `rel.txt` 就写 `rel.txt`，传 `sub/../rel2.txt` 就写原样 |
//! | `Path=`（顶层） | **相对 `$topdir`**：`/tmp/a/b.txt` → `a/b.txt`（不带前导 `/`） |
//! | 冲突命名 | 第 0 次 `<name>`；第 i 次（i≥1）`<name>-<inode + i - 1>` |
//! | 源文件不存在 | 返回 `False`，但**回收站目录仍会被创建**（先建目录再 stat 源） |
//! | 符号链接 | 按链接本身移入（不跟随）；目录整棵移入 |
//!
//! ## 顶层回收站（跨设备）三分支
//!
//! 文件与 home 回收站不同设备时，回收站落在文件所在挂载点：
//!
//! 1. `$topdir/.Trash` 存在、是目录、**带 sticky 位**、**不是符号链接** →
//!    `$topdir/.Trash/<uid>/{files,info}`（`0700`）；
//! 2. `$topdir/.Trash` 存在但**无 sticky** → Qt 打
//!    `Warning: '<dir>' doesn't have sticky bit set!` 后退化成 `$topdir/.Trash-<uid>`；
//! 3. `.Trash` 不存在 → 建 `$topdir/.Trash-<uid>`（`0700`）。
//!
//! Rust 侧不打日志（库里没有 logger），分支选择做成纯函数 [`choose_trash_dir`]，
//! 由调用方按返回值判断要不要提示。
//!
//! ## 参数化
//!
//! Python 里 `QStandardPaths` / `getuid()` / `QDateTime` 都是隐式全局，Rust 侧全部
//! 作参数传入（`xdg_data_home` / `uid` / `deletion_date`），理由同第 87 步的
//! `app_version`：可测、不依赖进程环境。真正的入口 [`delete_recoverably`] 才去取
//! [`current_uid`] 与 [`deletion_date_now`]。
//!
//! ## 与 Python 的已知差异
//!
//! 1. 返回值：Qt 给 `bool`，Rust 的 [`move_to_trash`] 给 `io::Result<PathBuf>`
//!    （`Err` = Python 的 `False`，`Ok` 里带回收站内的最终路径，便于调用方记日志）；
//!    [`delete_recoverably`] 保留 `bool` 口径；
//! 2. 冲突重试上限：Qt 用 `for (int i = 0; i < 100; ++i)` 之后放弃，Rust 同样
//!    [`MAX_ATTEMPTS`] 次后返回 `AlreadyExists`；
//! 3. `Path=` 里的非 UTF-8 字节：Python 走 `surrogateescape` 原样保留，Rust 走
//!    `to_string_lossy()`（会替换成 U+FFFD）；
//! 4. 跨设备 `rename`（`EXDEV`）：Qt 也是失败即回滚，且正常情况下不会发生——
//!    回收站目录本来就按「文件所在设备」选，Rust 侧同样不做 copy 兜底。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// 冲突重试次数上限（Qt 同样是 100）。
pub const MAX_ATTEMPTS: usize = 100;

/// 百分号编码用的十六进制表（**大写**，与 Qt 一致）。
const HEX: &[u8; 16] = b"0123456789ABCDEF";

/// home 回收站根：`<xdg_data_home>/Trash`。
///
/// `xdg_data_home` 由调用方解析（`$XDG_DATA_HOME` 或 `$HOME/.local/share`），
/// 本函数不做环境变量读取。
pub fn home_trash_dir(xdg_data_home: &Path) -> PathBuf {
    xdg_data_home.join("Trash")
}

/// `QStandardPaths::GenericDataLocation` 的解析：`xdg` 非空则用它，否则
/// `home/.local/share`。
pub fn generic_data_dir(home: &Path, xdg: Option<&std::ffi::OsStr>) -> PathBuf {
    match xdg {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => home.join(".local").join("share"),
    }
}

/// `$topdir` 上的回收站目录（纯路径计算，不碰文件系统）。
///
/// `shared` 为 `true` 表示 `.Trash` 可用（sticky + 非符号链接），
/// 落到 `$topdir/.Trash/<uid>`；否则退化 `$topdir/.Trash-<uid>`。
pub fn topdir_trash_dir(topdir: &Path, shared: bool, uid: u32) -> PathBuf {
    if shared {
        topdir.join(".Trash").join(uid.to_string())
    } else {
        topdir.join(format!(".Trash-{uid}"))
    }
}

/// 回收站目录的选择逻辑（纯函数，设备/sticky 的判定由调用方做完再传进来）。
///
/// - `same_device`：文件与 home 回收站同设备 → 直接用 `home_trash`；
/// - 否则看 `$topdir` 上的 `.Trash` 能不能用（`shared_ok`）。
pub fn choose_trash_dir(
    topdir: &Path,
    home_trash: &Path,
    same_device: bool,
    shared_ok: bool,
    uid: u32,
) -> PathBuf {
    if same_device {
        return home_trash.to_path_buf();
    }
    topdir_trash_dir(topdir, shared_ok, uid)
}

/// 回收站位置：目录 + （顶层回收站时的）挂载点。
///
/// 拆出 `topdir` 是因为 `.trashinfo` 里的 `Path=` 口径**两种回收站不一样**：
/// home 回收站写调用方传进来的原样，顶层回收站写相对 `$topdir` 的路径。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrashLocation {
    /// home 回收站是 `<xdg>/Trash`；顶层回收站是 `$topdir/.Trash/<uid>` 或 `$topdir/.Trash-<uid>`
    pub dir: PathBuf,
    /// 顶层回收站时是 `$topdir`；home 回收站时为 `None`
    pub topdir: Option<PathBuf>,
}

impl TrashLocation {
    /// home 回收站。
    pub fn home(dir: PathBuf) -> Self {
        Self { dir, topdir: None }
    }

    /// 顶层回收站。
    pub fn in_topdir(dir: PathBuf, topdir: PathBuf) -> Self {
        Self {
            dir,
            topdir: Some(topdir),
        }
    }

    /// `.trashinfo` 里 `Path=` 的值（第 91 步 PySide6 实测）。
    ///
    /// - home 回收站：**原样**（`/root/a/b.txt` 就写 `/root/a/b.txt`，`rel.txt` 就写 `rel.txt`）；
    /// - 顶层回收站：**相对 `$topdir`**（`/tmp/a/b.txt` → `a/b.txt`）；
    /// - 顶层回收站但路径不在 `$topdir` 下：退回原样（Qt 这里不会出现，属防御分支）。
    pub fn original_path(&self, path: &Path) -> String {
        match &self.topdir {
            Some(topdir) => match path.strip_prefix(topdir) {
                Ok(relative) => relative.to_string_lossy().into_owned(),
                Err(_) => path.to_string_lossy().into_owned(),
            },
            None => path.to_string_lossy().into_owned(),
        }
    }
}

/// `.trashinfo` 里的 `Path=` 的百分号编码。
///
/// 实测口径：保留 `A-Za-z0-9` `-` `.` `_` `~` 和 `/`（**斜杠不编码**），
/// 其余按 UTF-8 逐字节编码成大写 `%XX`。
pub fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for &byte in path.as_bytes() {
        let keep = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/');
        if keep {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0xf) as usize] as char);
        }
    }
    out
}

/// `.trashinfo` 的完整正文（**字节级**）。
///
/// 实测：`[Trash Info]\nPath=<编码后>\nDeletionDate=<YYYY-MM-DDTHH:MM:SS>\n`，
/// 本地时间、无时区后缀、结尾有换行。
pub fn trash_info_text(original_path: &str, deletion_date: &str) -> String {
    format!(
        "[Trash Info]\nPath={}\nDeletionDate={deletion_date}\n",
        encode_path(original_path)
    )
}

/// 冲突时的条目名。
///
/// 实测（5 次硬链接同 inode `271242`）：`dup.txt`、`dup.txt-271242`、
/// `dup.txt-271243`、`dup.txt-271244`、`dup.txt-271245` —— 即第 `attempt` 次
/// （从 0 起）用 `inode + attempt - 1`。
pub fn trash_entry_name(name: &str, inode: u64, attempt: usize) -> String {
    if attempt == 0 {
        return name.to_string();
    }
    format!("{name}-{}", inode + attempt as u64 - 1)
}

/// 当前用户的 uid。
#[cfg(unix)]
pub fn current_uid() -> u32 {
    // SAFETY: getuid(2) 无参数、不会失败。
    unsafe { libc::getuid() }
}

/// 当前用户的 uid（非 Unix 恒 0）。
#[cfg(not(unix))]
pub fn current_uid() -> u32 {
    0
}

/// 把 `epoch_secs` 按**本地时区**格式化成 `YYYY-MM-DDTHH:MM:SS`。
#[cfg(unix)]
pub fn deletion_date_at(epoch_secs: i64) -> String {
    let time = epoch_secs as libc::time_t;
    // SAFETY: `tm` 是 POD，先清零再交给 localtime_r 填满；指针非空。
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let filled = unsafe { !libc::localtime_r(&time, &mut tm).is_null() };
    if !filled {
        return String::new();
    }
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

/// `deletion_date_at` 的非 Unix 版本：恒返回空串（没有可复刻的本地时间口径）。
#[cfg(not(unix))]
pub fn deletion_date_at(_epoch_secs: i64) -> String {
    String::new()
}

/// 当前本地时间，格式见 [`deletion_date_at`]。
#[cfg(unix)]
pub fn deletion_date_now() -> String {
    // SAFETY: time(NULL) 只是读系统时钟。
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    deletion_date_at(now as i64)
}

/// `deletion_date_now` 的非 Unix 版本：恒返回空串。
#[cfg(not(unix))]
pub fn deletion_date_now() -> String {
    String::new()
}

/// `path` 自身（或最近一个存在的祖先）所在的挂载点。
///
/// 做法与 Qt 一致：从 `path` 往上走，直到父目录的 `st_dev` 变化为止。
#[cfg(unix)]
pub fn mount_point_of(path: &Path) -> io::Result<PathBuf> {
    use std::os::unix::fs::MetadataExt as _;

    let start = nearest_existing(path).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("{}: 没有存在的祖先目录", path.display()),
        )
    })?;
    let dev = fs::symlink_metadata(&start)?.dev();
    let mut current = start;
    loop {
        let parent = match current.parent() {
            Some(parent) if parent != current => parent.to_path_buf(),
            _ => return Ok(current),
        };
        match fs::metadata(&parent) {
            Ok(meta) if meta.dev() == dev => current = parent,
            Ok(_) => return Ok(current),
            Err(_) => return Ok(current),
        }
    }
}

/// `mount_point_of` 的非 Unix 版本：恒 `Unsupported`。
#[cfg(not(unix))]
pub fn mount_point_of(_path: &Path) -> io::Result<PathBuf> {
    Err(unsupported())
}

/// `$topdir/.Trash` 能不能当共享回收站：存在、是目录、带 sticky 位、不是符号链接。
#[cfg(unix)]
pub fn shared_trash_ok(topdir: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    match fs::symlink_metadata(topdir.join(".Trash")) {
        Ok(meta) => !meta.file_type().is_symlink() && meta.is_dir() && meta.mode() & 0o1000 != 0,
        Err(_) => false,
    }
}

/// `shared_trash_ok` 的非 Unix 版本：恒 `false`。
#[cfg(not(unix))]
pub fn shared_trash_ok(_topdir: &Path) -> bool {
    false
}

/// 决定 `path` 该进哪个回收站（会 stat 文件系统）。
///
/// 判定不出设备时退化到 home 回收站——与 Qt 一致（源不存在时它也只是失败返回）。
#[cfg(unix)]
pub fn resolve_trash_dir(path: &Path, xdg_data_home: &Path, uid: u32) -> TrashLocation {
    let home_trash = home_trash_dir(xdg_data_home);
    let same_device = match (device_of(path), device_of(&home_trash)) {
        (Some(file_dev), Some(home_dev)) => file_dev == home_dev,
        _ => true,
    };
    if same_device {
        return TrashLocation::home(home_trash);
    }
    match mount_point_of(path) {
        Ok(topdir) => {
            let shared_ok = shared_trash_ok(&topdir);
            let dir = choose_trash_dir(&topdir, &home_trash, false, shared_ok, uid);
            TrashLocation::in_topdir(dir, topdir)
        }
        Err(_) => TrashLocation::home(home_trash),
    }
}

/// `resolve_trash_dir` 的非 Unix 版本：恒 home 回收站。
#[cfg(not(unix))]
pub fn resolve_trash_dir(_path: &Path, xdg_data_home: &Path, _uid: u32) -> TrashLocation {
    TrashLocation::home(home_trash_dir(xdg_data_home))
}

/// 把 `path` 移进回收站，返回回收站内的最终路径。
///
/// - `xdg_data_home` 不存在 → `NotFound`，**什么都不建**（Qt 同口径）；
/// - 源文件不存在 → `NotFound`，但回收站目录**已经被建好**（Qt 同口径）；
/// - 冲突时按 [`trash_entry_name`] 递增重试，超过 [`MAX_ATTEMPTS`] 给 `AlreadyExists`。
#[cfg(unix)]
pub fn move_to_trash(
    path: &Path,
    xdg_data_home: &Path,
    uid: u32,
    deletion_date: &str,
) -> io::Result<PathBuf> {
    use std::os::unix::fs::MetadataExt as _;

    if !xdg_data_home.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{}: 数据目录不存在", xdg_data_home.display()),
        ));
    }
    let location = resolve_trash_dir(path, xdg_data_home, uid);
    let files_dir = location.dir.join("files");
    let info_dir = location.dir.join("info");
    create_private_dir(&files_dir)?;
    create_private_dir(&info_dir)?;

    let meta = fs::symlink_metadata(path)?;
    let name = path
        .file_name()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{}: 没有文件名", path.display()),
            )
        })?
        .to_string_lossy()
        .into_owned();
    let inode = meta.ino();
    let original = location.original_path(path);

    for attempt in 0..MAX_ATTEMPTS {
        let entry = trash_entry_name(&name, inode, attempt);
        let target = files_dir.join(&entry);
        let info_path = info_dir.join(format!("{entry}.trashinfo"));
        if fs::symlink_metadata(&target).is_ok() || fs::symlink_metadata(&info_path).is_ok() {
            continue;
        }
        fs::write(&info_path, trash_info_text(&original, deletion_date))?;
        match fs::rename(path, &target) {
            Ok(()) => return Ok(target),
            Err(err) => {
                // Qt 同样回滚：rename 失败就把刚写的 .trashinfo 删掉。
                let _ = fs::remove_file(&info_path);
                return Err(err);
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("{original}: 回收站里 {MAX_ATTEMPTS} 个候选名都被占用"),
    ))
}

/// `move_to_trash` 的非 Unix 版本：恒 `Unsupported`。
#[cfg(not(unix))]
pub fn move_to_trash(
    _path: &Path,
    _xdg_data_home: &Path,
    _uid: u32,
    _deletion_date: &str,
) -> io::Result<PathBuf> {
    Err(unsupported())
}

/// `app/platform/desktop.py:112-114` 的 `deleteRecoverably`：`bool` 口径。
///
/// `true` = 已移入回收站，`false` = Qt 的 `QFile.moveToTrash` 返回 `False`。
pub fn delete_recoverably(path: &Path, xdg_data_home: &Path, uid: u32) -> bool {
    move_to_trash(path, xdg_data_home, uid, &deletion_date_now()).is_ok()
}

/// `path` 自身（优先不跟随符号链接）或最近一个存在祖先的 `st_dev`。
#[cfg(unix)]
fn device_of(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt as _;

    if let Ok(meta) = fs::symlink_metadata(path) {
        return Some(meta.dev());
    }
    let mut current = path.parent();
    while let Some(candidate) = current {
        if let Ok(meta) = fs::metadata(candidate) {
            return Some(meta.dev());
        }
        current = candidate.parent();
    }
    None
}

/// 最近的、`symlink_metadata` 能成功的祖先（含 `path` 自身）。
#[cfg(unix)]
fn nearest_existing(path: &Path) -> Option<PathBuf> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        if fs::symlink_metadata(candidate).is_ok() {
            return Some(candidate.to_path_buf());
        }
        current = candidate.parent();
    }
    None
}

/// 建目录并尽量给 `0700`（已存在则不动）。
#[cfg(unix)]
fn create_private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;

    if path.is_dir() {
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(path)
}

#[cfg(not(unix))]
fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "freedesktop 回收站在非 Unix 平台未实现",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::ffi::OsStr;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = env::temp_dir().join(format!("gd3tr-{}-{tag}-{id}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn child(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_file(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, body).expect("write file");
    }

    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::MetadataExt as _;
        fs::metadata(path).expect("stat").mode() & 0o777
    }

    fn inode_of(path: &Path) -> u64 {
        use std::os::unix::fs::MetadataExt as _;
        fs::symlink_metadata(path).expect("stat").ino()
    }

    #[test]
    fn home_trash_dir_joins_trash() {
        assert_eq!(
            home_trash_dir(Path::new("/home/u/.local/share")),
            PathBuf::from("/home/u/.local/share/Trash")
        );
    }

    #[test]
    fn generic_data_dir_prefers_non_empty_xdg() {
        let home = Path::new("/home/u");
        assert_eq!(
            generic_data_dir(home, Some(OsStr::new("/data"))),
            PathBuf::from("/data")
        );
        assert_eq!(
            generic_data_dir(home, Some(OsStr::new(""))),
            PathBuf::from("/home/u/.local/share")
        );
        assert_eq!(
            generic_data_dir(home, None),
            PathBuf::from("/home/u/.local/share")
        );
    }

    /// 第 91 步用 PySide6 实测出来的完整字符表。
    #[test]
    fn encode_path_keeps_unreserved_and_slash() {
        assert_eq!(encode_path("AZaz09-._~/"), "AZaz09-._~/");
        let cases = [
            ("!", "%21"),
            ("*", "%2A"),
            ("'", "%27"),
            ("(", "%28"),
            (")", "%29"),
            (";", "%3B"),
            (":", "%3A"),
            ("@", "%40"),
            ("&", "%26"),
            ("=", "%3D"),
            ("+", "%2B"),
            ("$", "%24"),
            (",", "%2C"),
            ("?", "%3F"),
            ("[", "%5B"),
            ("]", "%5D"),
            ("{", "%7B"),
            ("}", "%7D"),
            ("|", "%7C"),
            ("\\", "%5C"),
            ("^", "%5E"),
            ("`", "%60"),
            ("<", "%3C"),
            (">", "%3E"),
            ("#", "%23"),
            ("%", "%25"),
            (" ", "%20"),
            ("\t", "%09"),
        ];
        for (raw, encoded) in cases {
            assert_eq!(encode_path(raw), encoded, "raw = {raw:?}");
        }
    }

    #[test]
    fn encode_path_encodes_non_ascii_bytes() {
        assert_eq!(encode_path("中"), "%E4%B8%AD");
        assert_eq!(
            encode_path("/tmp/a b/中文.txt"),
            "/tmp/a%20b/%E4%B8%AD%E6%96%87.txt"
        );
    }

    #[test]
    fn trash_info_text_is_byte_exact() {
        let text = trash_info_text("/tmp/a b/中.txt", "2026-10-18T11:34:05");
        assert_eq!(
            text,
            "[Trash Info]\nPath=/tmp/a%20b/%E4%B8%AD.txt\nDeletionDate=2026-10-18T11:34:05\n"
        );
        // 13（头）+ 5（Path=）+ 24（编码后的路径）+ 1 + 13（DeletionDate=）+ 19 + 1
        assert_eq!(text.len(), 13 + 5 + 24 + 1 + 13 + 19 + 1);
    }

    /// `Path=` 不绝对化：传什么写什么。
    #[test]
    fn trash_info_text_keeps_relative_path_verbatim() {
        assert!(
            trash_info_text("sub/../rel2.txt", "2026-10-18T11:34:05")
                .contains("\nPath=sub/../rel2.txt\n")
        );
        assert!(trash_info_text("rel.txt", "2026-10-18T11:34:05").contains("\nPath=rel.txt\n"));
    }

    /// 实测：`dup.txt`、`dup.txt-271242`、`dup.txt-271243`……
    #[test]
    fn trash_entry_name_first_then_inode_offsets() {
        assert_eq!(trash_entry_name("dup.txt", 271_242, 0), "dup.txt");
        assert_eq!(trash_entry_name("dup.txt", 271_242, 1), "dup.txt-271242");
        assert_eq!(trash_entry_name("dup.txt", 271_242, 2), "dup.txt-271243");
        assert_eq!(trash_entry_name("dup.txt", 271_242, 5), "dup.txt-271246");
    }

    /// 实测（第 91 步）：home 回收站写原样，顶层回收站写相对 `$topdir`。
    #[test]
    fn trash_location_original_path_follows_trash_kind() {
        let home = TrashLocation::home(PathBuf::from("/home/u/.local/share/Trash"));
        assert_eq!(
            home.original_path(Path::new("/home/u/a.txt")),
            "/home/u/a.txt"
        );
        assert_eq!(home.original_path(Path::new("rel.txt")), "rel.txt");

        let top = TrashLocation::in_topdir(PathBuf::from("/tmp/.Trash-0"), PathBuf::from("/tmp"));
        assert_eq!(top.original_path(Path::new("/tmp/a/b.txt")), "a/b.txt");
        assert_eq!(
            top.original_path(Path::new("/tmp/gd3probe-py2/sub/deep/x y 中.txt")),
            "gd3probe-py2/sub/deep/x y 中.txt"
        );
        // 不在 $topdir 下：防御性退回原样
        assert_eq!(
            top.original_path(Path::new("/elsewhere/c.txt")),
            "/elsewhere/c.txt"
        );
    }

    #[test]
    fn topdir_trash_dir_shared_and_fallback() {
        let topdir = Path::new("/mnt/disk");
        assert_eq!(
            topdir_trash_dir(topdir, true, 1000),
            PathBuf::from("/mnt/disk/.Trash/1000")
        );
        assert_eq!(
            topdir_trash_dir(topdir, false, 1000),
            PathBuf::from("/mnt/disk/.Trash-1000")
        );
    }

    #[test]
    fn choose_trash_dir_same_device_uses_home() {
        assert_eq!(
            choose_trash_dir(
                Path::new("/mnt/disk"),
                Path::new("/home/u/.local/share/Trash"),
                true,
                true,
                1000
            ),
            PathBuf::from("/home/u/.local/share/Trash")
        );
    }

    #[test]
    fn choose_trash_dir_other_device_follows_sticky() {
        let topdir = Path::new("/mnt/disk");
        let home = Path::new("/home/u/.local/share/Trash");
        assert_eq!(
            choose_trash_dir(topdir, home, false, true, 1000),
            PathBuf::from("/mnt/disk/.Trash/1000")
        );
        assert_eq!(
            choose_trash_dir(topdir, home, false, false, 1000),
            PathBuf::from("/mnt/disk/.Trash-1000")
        );
    }

    #[test]
    fn shared_trash_ok_requires_sticky_dir() {
        use std::os::unix::fs::PermissionsExt as _;

        let tmp = TempDir::new("sticky");
        // 没有 .Trash
        assert!(!shared_trash_ok(tmp.path()));

        // 普通目录（无 sticky）
        let shared = tmp.child(".Trash");
        fs::create_dir(&shared).expect("mkdir .Trash");
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o755)).expect("chmod");
        assert!(!shared_trash_ok(tmp.path()));

        // 带 sticky
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o1777)).expect("chmod");
        assert!(shared_trash_ok(tmp.path()));

        // 符号链接（即使目标带 sticky）也不算
        let other = TempDir::new("sticky-link");
        let target = other.child("shared");
        fs::create_dir(&target).expect("mkdir target");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o1777)).expect("chmod");
        let link = tmp.child(".Trash2");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        let with_link = TempDir::new("sticky-link2");
        std::os::unix::fs::symlink(&target, with_link.child(".Trash")).expect("symlink");
        assert!(!shared_trash_ok(with_link.path()));
    }

    #[test]
    fn mount_point_of_walks_up_to_device_change() {
        use std::os::unix::fs::MetadataExt as _;

        let tmp = TempDir::new("mount");
        let file = tmp.child("a.txt");
        write_file(&file, "x");
        let mount = mount_point_of(&file).expect("mount point");
        assert!(file.starts_with(&mount));
        let dev = fs::metadata(&file).expect("stat file").dev();
        assert_eq!(fs::metadata(&mount).expect("stat mount").dev(), dev);
        if let Some(parent) = mount.parent() {
            if let Ok(meta) = fs::metadata(parent) {
                assert_ne!(meta.dev(), dev, "挂载点的父目录必须换设备");
            }
        }
    }

    #[test]
    fn resolve_trash_dir_prefers_home_when_same_device() {
        let tmp = TempDir::new("resolve-same");
        let xdg = tmp.child("xdg");
        fs::create_dir_all(&xdg).expect("mkdir xdg");
        let file = tmp.child("keep.txt");
        write_file(&file, "x");
        assert_eq!(
            resolve_trash_dir(&file, &xdg, 1000),
            TrashLocation::home(home_trash_dir(&xdg)),
            "同一设备应该进 home 回收站"
        );
    }

    #[test]
    fn resolve_trash_dir_falls_back_to_home_for_missing_source() {
        let tmp = TempDir::new("resolve-missing");
        let xdg = tmp.child("xdg");
        fs::create_dir_all(&xdg).expect("mkdir xdg");
        let missing = tmp.child("nope.txt");
        assert_eq!(
            resolve_trash_dir(&missing, &xdg, 1000),
            TrashLocation::home(home_trash_dir(&xdg))
        );
    }

    #[test]
    fn move_to_trash_moves_file_and_writes_info() {
        let tmp = TempDir::new("move");
        let xdg = tmp.child("xdg");
        fs::create_dir_all(&xdg).expect("mkdir xdg");
        let file = tmp.child("report.txt");
        write_file(&file, "hello");

        let target = move_to_trash(&file, &xdg, 1000, "2026-10-18T11:34:05").expect("trash");
        assert!(!file.exists(), "源文件必须消失");
        assert_eq!(
            target,
            home_trash_dir(&xdg).join("files").join("report.txt")
        );
        assert_eq!(fs::read_to_string(&target).expect("read target"), "hello");
        assert_eq!(
            fs::read_to_string(tmp.child("xdg/Trash/info/report.txt.trashinfo")).expect("info"),
            trash_info_text(&file.to_string_lossy(), "2026-10-18T11:34:05")
        );
        assert_eq!(mode_of(&tmp.child("xdg/Trash")), 0o700);
        assert_eq!(mode_of(&tmp.child("xdg/Trash/files")), 0o700);
        assert_eq!(mode_of(&tmp.child("xdg/Trash/info")), 0o700);
    }

    #[test]
    fn move_to_trash_uses_inode_suffix_on_conflict() {
        let tmp = TempDir::new("conflict");
        let xdg = tmp.child("xdg");
        fs::create_dir_all(&xdg).expect("mkdir xdg");
        let first = tmp.child("d1/same.txt");
        let second = tmp.child("d2/same.txt");
        write_file(&first, "one");
        write_file(&second, "two");

        let first_target = move_to_trash(&first, &xdg, 1000, "2026-10-18T11:34:05").expect("first");
        assert_eq!(first_target.file_name().unwrap(), "same.txt");

        let second_inode = inode_of(&second);
        let second_target =
            move_to_trash(&second, &xdg, 1000, "2026-10-18T11:34:05").expect("second");
        assert_eq!(
            second_target.file_name().unwrap().to_string_lossy(),
            format!("same.txt-{second_inode}")
        );
        assert_eq!(fs::read_to_string(&second_target).expect("read"), "two");
    }

    #[test]
    fn move_to_trash_creates_dirs_before_failing_on_missing_source() {
        let tmp = TempDir::new("missing");
        let xdg = tmp.child("xdg");
        fs::create_dir_all(&xdg).expect("mkdir xdg");
        let missing = tmp.child("nope.txt");
        let err =
            move_to_trash(&missing, &xdg, 1000, "2026-10-18T11:34:05").expect_err("must fail");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(tmp.child("xdg/Trash/files").is_dir(), "目录仍要建好");
        assert!(tmp.child("xdg/Trash/info").is_dir());
        assert!(!tmp.child("xdg/Trash/info/nope.txt.trashinfo").exists());
    }

    #[test]
    fn move_to_trash_needs_existing_data_home() {
        let tmp = TempDir::new("nodata");
        let xdg = tmp.child("xdg");
        let file = tmp.child("a.txt");
        write_file(&file, "x");
        let err = move_to_trash(&file, &xdg, 1000, "2026-10-18T11:34:05").expect_err("must fail");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(!xdg.exists(), "$XDG_DATA_HOME 不该被创建");
        assert!(file.exists(), "源文件不能被移走");
    }

    #[test]
    fn move_to_trash_moves_symlink_as_link() {
        let tmp = TempDir::new("symlink");
        let xdg = tmp.child("xdg");
        fs::create_dir_all(&xdg).expect("mkdir xdg");
        let real = tmp.child("real.txt");
        write_file(&real, "payload");
        let link = tmp.child("link.txt");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");

        let target = move_to_trash(&link, &xdg, 1000, "2026-10-18T11:34:05").expect("trash");
        assert!(real.exists(), "链接目标不能被删");
        let meta = fs::symlink_metadata(&target).expect("stat target");
        assert!(meta.file_type().is_symlink(), "回收站里应还是符号链接");
        assert_eq!(fs::read_link(&target).expect("readlink"), real);
    }

    #[test]
    fn move_to_trash_moves_directory_tree() {
        let tmp = TempDir::new("dir");
        let xdg = tmp.child("xdg");
        fs::create_dir_all(&xdg).expect("mkdir xdg");
        let dir = tmp.child("folder");
        write_file(&dir.join("inner.txt"), "inner");
        write_file(&dir.join("sub/deep.txt"), "deep");

        let target = move_to_trash(&dir, &xdg, 1000, "2026-10-18T11:34:05").expect("trash");
        assert!(!dir.exists());
        assert_eq!(
            fs::read_to_string(target.join("sub/deep.txt")).expect("read"),
            "deep"
        );
    }

    #[test]
    fn delete_recoverably_reports_bool() {
        let tmp = TempDir::new("bool");
        let xdg = tmp.child("xdg");
        fs::create_dir_all(&xdg).expect("mkdir xdg");
        let file = tmp.child("gone.txt");
        write_file(&file, "x");
        assert!(delete_recoverably(&file, &xdg, current_uid()));
        assert!(!file.exists());
        assert!(
            !delete_recoverably(&file, &xdg, current_uid()),
            "第二次必须 false"
        );
    }

    #[test]
    fn deletion_date_now_has_expected_shape() {
        let stamp = deletion_date_now();
        assert_eq!(stamp.len(), 19, "stamp = {stamp:?}");
        let bytes = stamp.as_bytes();
        assert_eq!(bytes[4], b'-');
        assert_eq!(bytes[7], b'-');
        assert_eq!(bytes[10], b'T');
        assert_eq!(bytes[13], b':');
        assert_eq!(bytes[16], b':');
        for (index, byte) in bytes.iter().enumerate() {
            if matches!(index, 4 | 7 | 10 | 13 | 16) {
                continue;
            }
            assert!(byte.is_ascii_digit(), "stamp = {stamp:?}");
        }
        assert_eq!(deletion_date_at(0).len(), 19);
    }
}
