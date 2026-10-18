//! 文件系统探测与路径换算：镜像 `app/platform/filesystem.py` 里除
//! `toSafeFilename` / `splitStemExt`（已在 [`crate::filename`]）之外的六个函数。
//!
//! | Python | Rust |
//! |---|---|
//! | `isExisting` | [`is_existing`] |
//! | `isFolder` | [`is_folder`] |
//! | `toPosixPath` | [`to_posix_path`] |
//! | `localFilePath` | [`local_file_path`] |
//! | `findExecutable` | [`find_executable`] |
//! | `deletePath` | [`delete_path`] |
//! | `shutil.move` | [`move_path`] |
//!
//! ## 与 Python 的已知差异
//!
//! 1. `isExisting` / `isFolder` 捕获 `ValueError`（路径含 NUL 字节时 `stat` 会抛）。
//!    Rust 的 `Path` 允许 NUL，`stat` 只会失败 —— 两侧都返回 false，结果一致。
//! 2. `localFilePath` 的 `path.is_file()` 对含 NUL 的路径抛 `ValueError`，而 Python 只
//!    在构造 `Path` 时捕获了 `OSError` —— 即 Python 会崩，Rust 返回 `None`。
//! 3. 百分号解码失败时的替换字符个数以 `String::from_utf8_lossy` 为准，与 CPython 的
//!    `errors="replace"` 在极端截断序列上可能差一个字符（都按「最大子部分」替换）。
//! 4. `findExecutable` 的 Windows 分支没实现：Python 那边要按 `PATHEXT` 逐个后缀试，
//!    这里只做「直接命中」——Windows 后端整体未做（同 `single_instance`）。
//! 5. `Path.suffix.lower()` 用 Rust 的 `to_lowercase()`（Unicode 全量小写），CPython 的
//!    `str.lower()` 在个别字符（如 `İ`）上口径不同。
//!
//! 规格来源：`app/platform/filesystem.py:24-124`。

use std::collections::HashSet;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::path::posix_normpath;

/// 镜像 `isExisting`：`stat` 成功即存在（跟随符号链接）。
pub fn is_existing(path: impl AsRef<Path>) -> bool {
    fs::metadata(path.as_ref()).is_ok()
}

/// 镜像 `isFolder`：`stat` 成功且是目录（`S_ISDIR`，跟随符号链接）。
pub fn is_folder(path: impl AsRef<Path>) -> bool {
    fs::metadata(path.as_ref())
        .map(|meta| meta.is_dir())
        .unwrap_or(false)
}

/// 镜像 `toPosixPath`：`str(Path(path)).replace("\\", "/")`。
///
/// 顺序不能反：POSIX 上反斜杠是**普通字符**，`Path("a\\b")` 只有一段，所以先按
/// `PurePosixPath` 归一、再把反斜杠换成 `/`。
pub fn to_posix_path(path: impl AsRef<Path>) -> String {
    let raw = path.as_ref().to_string_lossy();
    posix_normpath(&raw).replace('\\', "/")
}

/// 镜像 `localFilePath`：把 `file://` URL 换成磁盘路径，不是普通文件就返回 `None`。
///
/// `valid_suffixes` 的条目按 Python 的约定是**小写、带点**（`".torrent"`），比较对象是
/// `Path.suffix.lower()`；传 `None` 表示不按后缀过滤（Python 的 `validSuffixes=None`）。
pub fn local_file_path(url: &str, valid_suffixes: Option<&[&str]>) -> Option<PathBuf> {
    let (scheme, netloc, raw_path) = split_url(url);
    if scheme != "file" {
        return None;
    }

    let raw = resolve_netloc(&netloc, &raw_path)?;
    let path = PathBuf::from(posix_normpath(&unquote(&raw)));
    if !path.is_file() {
        return None;
    }
    if let Some(suffixes) = valid_suffixes {
        let suffix = path_suffix(&path);
        // Python 只把**路径的**后缀转小写再跟集合比，集合里的项按原样用
        if !suffixes.contains(&suffix.as_str()) {
            return None;
        }
    }
    Some(path)
}

/// 镜像 `findExecutable`：`installFolder/<name>[.exe]` 优先，否则查 `PATH`。
///
/// 找不到返回空串（Python 返回 `""`，不是 `None`）；返回值一律过 [`to_posix_path`]。
pub fn find_executable(install_folder: &Path, name: &str) -> String {
    let exe = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    let candidate = install_folder.join(&exe);
    if candidate.is_file() {
        return to_posix_path(&candidate);
    }
    match which_in(name, &env::var("PATH").unwrap_or_default()) {
        Some(found) => to_posix_path(&found),
        None => String::new(),
    }
}

/// 镜像 `shutil.which(name)`：只在 `PATH` 里找（不看当前目录、不认安装目录）。
///
/// [`find_executable`] 会先试 `install_folder/<name>`，`install_folder` 传空路径时
/// 那条分支就退化成「相对当前目录」的 `is_file()` 检查；`shutil.which` 没有这一步。
/// 需要纯粹按 `PATH` 判定「命令在不在」的调用方（比如 `openChromiumUrl`）用这个。
///
/// 与 Python 的已知差异：`shutil.which` 在 `PATH` 未设置时退回 `os.defpath`
/// （`/bin:/usr/bin`），这里退回空串（= 什么都不查）—— 与 [`find_executable`] 一致。
pub fn which(name: &str) -> Option<PathBuf> {
    which_in(name, &env::var("PATH").unwrap_or_default())
}

/// 镜像 `deletePath`：目录整棵删、其余 `unlink`；不存在与失败都静默。
///
/// `is_dir()` 跟随符号链接，`is_symlink()` 不跟 —— 所以「指向目录的符号链接」走
/// `unlink` 分支，只删链接不删目标（Python 同一个判断）。
pub fn delete_path(path: &Path) {
    let is_link = fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false);
    if path.is_dir() && !is_link {
        let _ = fs::remove_dir_all(path);
    } else {
        let _ = fs::remove_file(path);
    }
}

/// 镜像 `shutil.move` 的「目标不存在」分支：先 `rename`，跨设备退化成「复制 + 删除」。
///
/// 调用方（`TaskService::update_placeholders`）总是先 `delete_path(to)`，所以不用处理
/// `shutil.move` 的「`dst` 是已存在目录 → 移进去」那条分支。
///
/// 与 Python 的差异：
///
/// 1. 只在 `EXDEV`（`ErrorKind::CrossesDevices`）时回退；CPython 是 `except OSError`
///    全量回退，但其余错误（`EACCES`、`ENOENT`）回退后同样失败，只是错误信息不同。
/// 2. 回退用 `fs::copy`（保留权限位、不保留 mtime），`shutil.move` 用 `copy2`
///    （连 atime/mtime 一起保留）。
/// 3. 符号链接跨设备时不复刻 `os.readlink` + `os.symlink`，而是按 `fs::copy` 跟随链接
///    复制内容再删链接 —— 占位符场景不会出现符号链接。
pub fn move_path(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            if fs::symlink_metadata(from)?.is_dir() {
                copy_dir_all(from, to)?;
                fs::remove_dir_all(from)
            } else {
                fs::copy(from, to)?;
                fs::remove_file(from)
            }
        }
        Err(error) => Err(error),
    }
}

/// `copytree` 的最小版：建目录 + 逐项递归复制（不保留元数据，见 [`move_path`]）。
fn copy_dir_all(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// 镜像 `localFilePath` 里 netloc 的三分支：空 / `localhost` / 其余。
fn resolve_netloc(netloc: &str, path: &str) -> Option<String> {
    if netloc.is_empty() || netloc.to_lowercase() == "localhost" {
        return Some(path.to_string());
    }
    #[cfg(windows)]
    {
        Some(windows_netloc_path(netloc, path))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Windows 上 `file://C:/x` 是盘符、`file://host/share` 是 UNC。
#[cfg(windows)]
fn windows_netloc_path(netloc: &str, path: &str) -> String {
    let bytes = netloc.as_bytes();
    let is_drive = bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if is_drive {
        format!("{netloc}{path}")
    } else {
        format!("//{netloc}{path}")
    }
}

/// 镜像 `PurePath.suffix`：最后一段组件里最后一个 `.` 之后，且要满足 `0 < i < len-1`。
fn path_suffix(path: &Path) -> String {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    match name.rfind('.') {
        Some(dot) if dot > 0 && dot < name.len() - 1 => name[dot..].to_lowercase(),
        _ => String::new(),
    }
}

/// 镜像 `urllib.parse.urlsplit` 的 scheme / netloc / path 三段。
///
/// 只搬 `localFilePath` 用得到的部分：query 与 fragment 直接丢掉（Python 的
/// `parsed.path` 本来就不含它们），params 对 `file` scheme 不拆（`file` 不在
/// `uses_params` 里）。
fn split_url(url: &str) -> (String, String, String) {
    // Python 只 `lstrip`（尾部空格保留在 path 里），另外删掉 URL 中的 \t \r \n
    let cleaned: String = url
        .trim_start_matches(|ch: char| (ch as u32) <= 0x20)
        .chars()
        .filter(|ch| !matches!(ch, '\t' | '\r' | '\n'))
        .collect();

    let mut rest: &str = &cleaned;
    let mut scheme = String::new();
    if let Some(colon) = cleaned.find(':') {
        let head = &cleaned[..colon];
        let starts_with_letter = head
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic());
        let all_scheme_chars = head
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'));
        if starts_with_letter && all_scheme_chars {
            scheme = head.to_ascii_lowercase();
            rest = &cleaned[colon + 1..];
        }
    }

    let mut netloc = String::new();
    if let Some(after) = rest.strip_prefix("//") {
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        netloc = after[..end].to_string();
        rest = &after[end..];
    }

    let end = rest.find(['?', '#']).unwrap_or(rest.len());
    (scheme, netloc, rest[..end].to_string())
}

/// 镜像 `urllib.request.url2pathname` 的 POSIX 实现 —— 就是 `unquote`。
///
/// CPython 的 `unquote_to_bytes` 先按 `%` 切段，每段只解前两个十六进制字符、剩下的
/// **原样**拼回去（所以 `%4142` → `A42` 而不是 `AB`）；解不出十六进制的 `%` 连同本段
/// 一起保留。最后按 UTF-8 解码，坏字节换成 U+FFFD。
fn unquote(text: &str) -> String {
    let raw = text.as_bytes();
    let mut decoded: Vec<u8> = Vec::with_capacity(raw.len());
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'%' {
            if let (Some(high), Some(low)) = (
                raw.get(index + 1).copied().and_then(hex_value),
                raw.get(index + 2).copied().and_then(hex_value),
            ) {
                decoded.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        decoded.push(raw[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// 镜像 `shutil.which`（POSIX 分支）。
///
/// `path_var` 单独传是为了能测：Python 的 `which(cmd, path=None)` 也有这个口子。
fn which_in(cmd: &str, path_var: &str) -> Option<PathBuf> {
    // 有目录部分时只查这一条，不再搜 PATH（`./script` 也走这里）
    if let Some(slash) = cmd.rfind('/') {
        let (dirname, base) = (&cmd[..slash], &cmd[slash + 1..]);
        if !dirname.is_empty() {
            let candidate = Path::new(dirname).join(base);
            return access_check(&candidate).then_some(candidate);
        }
    }
    // PATH="" 什么都不查；PATH=":" 会在当前目录里查（空段 → join 出相对名）
    if path_var.is_empty() {
        return None;
    }
    let mut seen: HashSet<&str> = HashSet::new();
    for dir in path_var.split(':') {
        // Python 用 `os.path.normcase(dir)` 去重，POSIX 上是恒等
        if !seen.insert(dir) {
            continue;
        }
        let candidate = Path::new(dir).join(cmd);
        if access_check(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// 镜像 `shutil._access_check`：存在、可执行、且不是目录。
#[cfg(unix)]
fn access_check(path: &Path) -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    // `os.path.exists` + `not os.path.isdir`（`stat` 跟随符号链接）
    match fs::metadata(path) {
        Ok(meta) if !meta.is_dir() => {}
        _ => return false,
    }
    let Ok(raw) = CString::new(path.as_os_str().as_bytes()) else {
        // 含 NUL 字节的路径：Python 那边 `os.access` 会抛 ValueError
        return false;
    };
    // SAFETY：`raw` 是以 NUL 结尾的合法 C 字符串，`access(2)` 只读它，不保留指针
    unsafe { libc::access(raw.as_ptr(), libc::X_OK) == 0 }
}

/// Windows 上 `_access_check` 还要按 `PATHEXT` 试后缀，这里只做直接命中。
#[cfg(not(unix))]
fn access_check(path: &Path) -> bool {
    path.is_file()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// `gd3-core` 没有 `tempfile` 依赖，跟 `gd3-ui` 的测试一个套路：临时目录 + 计数器。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = env::temp_dir().join(format!("gd3fs-{}-{tag}-{id}", std::process::id()));
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

    fn touch(path: &Path, body: &[u8]) {
        let mut file = fs::File::create(path).expect("create file");
        file.write_all(body).expect("write file");
    }

    fn chmod(path: &Path, mode: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("chmod");
    }

    #[test]
    fn is_existing_and_is_folder_match_python() {
        let dir = TempDir::new("exists");
        let file = dir.path().join("a.txt");
        touch(&file, b"x");

        assert!(is_existing(&file));
        assert!(is_existing(dir.path()));
        assert!(!is_existing(dir.path().join("missing")));

        assert!(!is_folder(&file));
        assert!(is_folder(dir.path()));
        assert!(!is_folder(dir.path().join("missing")));

        // Python 的 `Path("").stat()` → FileNotFoundError → False
        assert!(!is_existing(""));
        assert!(!is_folder(""));
    }

    #[test]
    fn is_folder_follows_symlinks() {
        let dir = TempDir::new("symlink-folder");
        let target = dir.path().join("real");
        fs::create_dir(&target).expect("mkdir");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");

        // `stat` 跟随链接 → S_ISDIR 为真
        assert!(is_folder(&link));
        assert!(is_existing(&link));
    }

    #[test]
    fn to_posix_path_normalises_like_pathlib() {
        let cases = [
            ("", "."),
            ("a//b/", "a/b"),
            ("//x/./y", "//x/y"),
            ("///x", "/x"),
            ("a\\b", "a/b"),
            ("/tmp//a/./b.txt", "/tmp/a/b.txt"),
        ];
        for (raw, expected) in cases {
            assert_eq!(to_posix_path(raw), expected, "raw={raw:?}");
        }
        // 反斜杠是普通字符，先归一后替换：`a\b/c` 里 `a\b` 是一段
        assert_eq!(to_posix_path("a\\b//c"), "a/b/c");
    }

    #[test]
    fn split_url_matches_urlsplit() {
        // 期望值来自 CPython 3.13：`urllib.parse.urlparse(u)` 的 scheme/netloc/path
        let cases = [
            ("file:///tmp/x.txt", "file", "", "/tmp/x.txt"),
            ("file://localhost/tmp/x", "file", "localhost", "/tmp/x"),
            ("file://host/share/x", "file", "host", "/share/x"),
            ("file:/tmp/x", "file", "", "/tmp/x"),
            ("file:tmp/x", "file", "", "tmp/x"),
            ("file://", "file", "", ""),
            ("FILE:///tmp/x", "file", "", "/tmp/x"),
            ("file://LOCALHOST/tmp/x", "file", "LOCALHOST", "/tmp/x"),
            ("  file:///tmp/x  ", "file", "", "/tmp/x  "),
            ("file:///tmp/x?q=1", "file", "", "/tmp/x"),
            ("file:///tmp/x#frag", "file", "", "/tmp/x"),
            ("file:///C:/x", "file", "", "/C:/x"),
            ("http:///tmp/x", "http", "", "/tmp/x"),
            ("file:///tmp/a+b.txt", "file", "", "/tmp/a+b.txt"),
            (":file:///tmp/x", "", "", ":file:///tmp/x"),
            ("f1le:///tmp/x", "f1le", "", "/tmp/x"),
            ("fi le:///tmp/x", "", "", "fi le:///tmp/x"),
        ];
        for (url, scheme, netloc, path) in cases {
            let got = split_url(url);
            assert_eq!(
                (got.0.as_str(), got.1.as_str(), got.2.as_str()),
                (scheme, netloc, path),
                "url={url:?}"
            );
        }
    }

    #[test]
    fn unquote_matches_python() {
        // 期望值来自 CPython 3.13：`urllib.parse.unquote(s)`
        let cases = [
            ("/tmp/a%20b.txt", "/tmp/a b.txt"),
            ("%4142", "A42"),
            ("%%41", "%A"),
            ("%2", "%2"),
            ("%zz", "%zz"),
            ("%", "%"),
            ("abc%", "abc%"),
            ("%E4%B8%AD", "中"),
            ("a+b", "a+b"),
            ("%FF", "\u{fffd}"),
            ("%C3%28", "\u{fffd}("),
            ("/tmp/a%2Fb", "/tmp/a/b"),
        ];
        for (raw, expected) in cases {
            assert_eq!(unquote(raw), expected, "raw={raw:?}");
        }
    }

    fn file_url(path: &Path) -> String {
        format!("file://{}", path.to_string_lossy())
    }

    #[test]
    fn local_file_path_accepts_plain_file_url() {
        let dir = TempDir::new("local-url");
        let file = dir.path().join("x.txt");
        touch(&file, b"x");

        assert_eq!(local_file_path(&file_url(&file), None), Some(file.clone()));
        // `file://localhost/...` 与 `file:/...` 都落到同一个路径
        let localhost_url = format!("file://localhost{}", file.to_string_lossy());
        assert_eq!(local_file_path(&localhost_url, None), Some(file.clone()));
        let short_url = format!("file:{}", file.to_string_lossy());
        assert_eq!(local_file_path(&short_url, None), Some(file.clone()));
        // query / fragment 不参与路径
        assert_eq!(
            local_file_path(&format!("{}?a=1#b", file_url(&file)), None),
            Some(file.clone())
        );
    }

    #[test]
    fn local_file_path_rejects_foreign_host() {
        let dir = TempDir::new("foreign-host");
        let file = dir.path().join("x.txt");
        touch(&file, b"x");
        let url = format!("file://server{}", file.to_string_lossy());
        // POSIX 上非空 netloc 且不是 localhost → None（Windows 才是 UNC）
        assert_eq!(local_file_path(&url, None), None);
    }

    #[test]
    fn local_file_path_rejects_non_file_scheme_and_missing() {
        let dir = TempDir::new("local-missing");
        let file = dir.path().join("x.txt");
        touch(&file, b"x");

        assert_eq!(local_file_path("http://example.com/x.txt", None), None);
        assert_eq!(local_file_path("https:///tmp/x", None), None);
        assert_eq!(
            local_file_path(&file_url(&dir.path().join("nope")), None),
            None
        );
        // 目录不算「文件」
        assert_eq!(local_file_path(&file_url(dir.path()), None), None);
        // 空路径：`Path("")` → "."，是个目录
        assert_eq!(local_file_path("file://", None), None);
    }

    #[test]
    fn local_file_path_honours_valid_suffixes() {
        let dir = TempDir::new("local-suffix");
        let torrent = dir.path().join("a.TORRENT");
        touch(&torrent, b"x");
        let plain = dir.path().join("a.txt");
        touch(&plain, b"x");

        let suffixes: &[&str] = &[".torrent"];
        // `Path.suffix.lower()` 与列表里的项比较：大写后缀也算命中
        assert_eq!(
            local_file_path(&file_url(&torrent), Some(suffixes)),
            Some(torrent.clone())
        );
        assert_eq!(local_file_path(&file_url(&plain), Some(suffixes)), None);
        // 无后缀的路径 suffix 是 ""，同样不命中
        let bare = dir.path().join("bare");
        touch(&bare, b"x");
        assert_eq!(local_file_path(&file_url(&bare), Some(suffixes)), None);
        assert_eq!(local_file_path(&file_url(&bare), None), Some(bare));
    }

    #[test]
    fn local_file_path_decodes_percent_and_unicode() {
        let dir = TempDir::new("local-unicode");
        let file = dir.path().join("中文 名字.txt");
        touch(&file, b"x");

        // 直接放非 ASCII 字符（Python 的 unquote 会先 UTF-8 编码再解回来）
        assert_eq!(local_file_path(&file_url(&file), None), Some(file.clone()));

        // 百分号编码形式
        let encoded = format!(
            "file://{}/%E4%B8%AD%E6%96%87%20%E5%90%8D%E5%AD%97.txt",
            dir.path().to_string_lossy()
        );
        assert_eq!(local_file_path(&encoded, None), Some(file.clone()));
    }

    #[test]
    fn find_executable_prefers_install_folder() {
        let dir = TempDir::new("find-exe");
        let tool = dir.path().join("aria2c");
        touch(&tool, b"#!/bin/sh\n");
        chmod(&tool, 0o755);

        assert_eq!(find_executable(dir.path(), "aria2c"), to_posix_path(&tool));
        // 目录里没有、PATH 里也没有 → 空串（Python 返回 ""）
        assert_eq!(find_executable(dir.path(), "gd3-no-such-binary-xyz"), "");
    }

    #[test]
    fn find_executable_ignores_directory_with_same_name() {
        let dir = TempDir::new("find-exe-dir");
        fs::create_dir(dir.path().join("ffmpeg")).expect("mkdir");
        // `candidate.is_file()` 为假 → 继续找 PATH → 找不到 → 空串
        assert_eq!(find_executable(dir.path(), "ffmpeg"), "");
    }

    #[test]
    fn which_requires_execute_bit() {
        let dir = TempDir::new("which-mode");
        let path_var = dir.path().to_string_lossy().into_owned();

        let tool = dir.path().join("tool");
        touch(&tool, b"x");
        chmod(&tool, 0o644);
        assert_eq!(which_in("tool", &path_var), None);

        chmod(&tool, 0o755);
        assert_eq!(which_in("tool", &path_var), Some(tool));
    }

    #[test]
    fn which_skips_directories_and_missing() {
        let dir = TempDir::new("which-dir");
        let path_var = dir.path().to_string_lossy().into_owned();
        fs::create_dir(dir.path().join("sub")).expect("mkdir");
        chmod(&dir.path().join("sub"), 0o755);

        assert_eq!(which_in("sub", &path_var), None);
        assert_eq!(which_in("nope", &path_var), None);
        assert_eq!(which_in("nope", ""), None);
    }

    #[test]
    fn which_uses_dirname_when_present() {
        let dir = TempDir::new("which-dirname");
        let tool = dir.path().join("tool");
        touch(&tool, b"x");
        chmod(&tool, 0o755);

        let absolute = tool.to_string_lossy().into_owned();
        // 带目录部分时只查这一条，跟 PATH 无关
        assert_eq!(which_in(&absolute, ""), Some(tool.clone()));

        let relative = format!("{}/tool", dir.path().to_string_lossy());
        assert_eq!(which_in(&relative, ""), Some(PathBuf::from(relative)));

        let missing = format!("{}/nope", dir.path().to_string_lossy());
        assert_eq!(which_in(&missing, ""), None);
    }

    #[test]
    fn which_walks_path_in_order() {
        let first = TempDir::new("which-first");
        let second = TempDir::new("which-second");
        let a = first.path().join("dup");
        touch(&a, b"x");
        chmod(&a, 0o755);
        let b = second.path().join("dup");
        touch(&b, b"x");
        chmod(&b, 0o755);

        let path_var = format!(
            "{}:{}",
            first.path().to_string_lossy(),
            second.path().to_string_lossy()
        );
        assert_eq!(which_in("dup", &path_var), Some(a));
    }

    #[test]
    fn delete_path_removes_files_and_trees() {
        let dir = TempDir::new("delete");

        let file = dir.path().join("a.txt");
        touch(&file, b"x");
        delete_path(&file);
        assert!(!is_existing(&file));

        let tree = dir.path().join("tree");
        fs::create_dir_all(tree.join("nested")).expect("mkdir");
        touch(&tree.join("nested").join("b.txt"), b"x");
        delete_path(&tree);
        assert!(!is_existing(&tree));

        // 不存在 → 静默
        delete_path(&dir.path().join("missing"));
    }

    #[test]
    fn delete_path_unlinks_symlink_to_directory() {
        let dir = TempDir::new("delete-link");
        let target = dir.path().join("real");
        fs::create_dir(&target).expect("mkdir");
        touch(&target.join("keep.txt"), b"x");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");

        delete_path(&link);
        // 只删链接，目标还在
        assert!(!is_existing(&link));
        assert!(is_folder(&target));
        assert!(is_existing(target.join("keep.txt")));
    }

    #[test]
    fn move_path_renames_within_one_device() {
        let dir = TempDir::new("move-file");
        let from = dir.path().join("a.txt");
        touch(&from, b"data");
        let to = dir.path().join("b.txt");
        move_path(&from, &to).expect("同设备 rename");
        assert!(!is_existing(&from));
        assert_eq!(fs::read(&to).expect("读回"), b"data");
    }

    #[test]
    fn move_path_moves_a_whole_directory() {
        let dir = TempDir::new("move-dir");
        let from = dir.path().join("src");
        fs::create_dir_all(from.join("nested")).expect("mkdir");
        touch(&from.join("nested").join("b.txt"), b"x");
        let to = dir.path().join("dst");
        move_path(&from, &to).expect("目录 rename");
        assert!(!is_existing(&from));
        assert_eq!(
            fs::read(to.join("nested").join("b.txt")).expect("读回"),
            b"x"
        );
    }

    #[test]
    fn move_path_reports_missing_source() {
        let dir = TempDir::new("move-missing");
        let error = move_path(&dir.path().join("nope"), &dir.path().join("dst"))
            .expect_err("源不存在应报错");
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn which_only_searches_path() {
        // `sh` 一定在 PATH 里
        assert!(which("sh").is_some());
        assert_eq!(which("gd3-no-such-command-xyz"), None);
        // `shutil.which` 认 `/` 时不搜 PATH：给绝对路径就按那条路径判
        assert!(which("/bin/sh").is_some());
    }
}
