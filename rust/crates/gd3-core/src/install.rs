//! `app/install.py` 的 Rust 对应物（只搬纯文件系统部分）。
//!
//! | Python | Rust |
//! |---|---|
//! | `deleteInstalled` | [`delete_installed`] |
//! | `installFile` | [`install_file`] |
//! | `installArchive` | **未迁移** |
//!
//! `installArchive`（`app/install.py:27-79`）要 `zipfile` / `tarfile("r:gz")` 解码，
//! 属于「装 FeaturePack 运行时」的范畴，跟 `update_service`（还要 `r:xz`、`hdiutil`）
//! 一起排期；本模块先把「换文件」这一步钉住。
//!
//! ## 语义（照抄 Python，不要「改进」）
//!
//! - `deleteInstalled` 是「先改名再加后缀，然后按 `<name>.*.old` 扫一遍全删」：
//!   改名让**正在运行的旧二进制**（Windows 上删不掉）先让路，随后的扫描顺带清掉
//!   之前几次留下的同名残留；
//! - 后缀是 `uuid4().hex[:8]`（8 个小写十六进制字符），不做任何查重；
//! - `installFile` = `deleteInstalled(target)` + `source.replace(target)`；
//!   `os.replace` 在**同一文件系统**上是原子的，跨设备失败（`EXDEV`）——
//!   Rust 的 `fs::rename` 完全同构；
//! - 这两个函数都不吞错误（Python 侧没有 `try/except`），只有内部的 `deletePath`
//!   自己吞 —— 与 [`crate::filesystem::delete_path`] 一致。
//!
//! ## 与 Python 的已知差异
//!
//! 1. `Path.glob` 的匹配在 Python 里是 `fnmatch` 语义；这里按**前缀 + 后缀**判定。
//!    因为模式恒为 `glob.escape(name) + ".*.old"`，而 `glob.escape` 只把 `[` / `]` /
//!    `*` / `?` 包成字符类（`a[1]` → `a[[]1]`），剩下真正起通配作用的只有那一个
//!    `*` —— 「以 `name.` 开头、以 `.old` 结尾、长度容得下中间那段」与 `fnmatch`
//!    完全等价（`*` 不跨 `/`，而目录项本来就不含 `/`）。
//! 2. `path` 为空、`/`、`.` 时 Python 的 `with_name` 抛 `ValueError`（整个函数中断）；
//!    Rust 的 `file_name()` 是 `None` → 直接返回（一个字节都不动）。`..` 这类
//!    Python 认作文件名的路径，Rust 同样跳过。
//! 3. 非 UTF-8 文件名：Python 走 `surrogateescape` 原样保留，Rust 用
//!    `to_string_lossy()`（坏字节变 `U+FFFD`）。改名后的名字可能与 Python 不同，
//!    但都会被随后的扫描删掉。
//!
//! 规格来源：`app/install.py:15-25`。

use std::fs;
use std::io;
use std::path::Path;

use uuid::Uuid;

use crate::filesystem::delete_path;

/// `fnmatch` 模式 `<name>.*.old` 的等价判定（见模块头第 1 条）。
///
/// `prefix` 是 `"<name>."`；中间那段可以为空，所以长度下限是
/// `prefix.len() + ".old".len()`。
fn is_leftover(entry: &str, prefix: &str) -> bool {
    entry.len() >= prefix.len() + 4 && entry.starts_with(prefix) && entry.ends_with(".old")
}

/// `Path("a").parent` 在 Python 是 `Path(".")`，Rust 是 `Path("")` ——
/// 空的时候补 `.`，否则 `read_dir("")` 会 ENOENT。
fn parent_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// 镜像 `deleteInstalled`：存在就改名成 `<name>.<8hex>.old`，再清掉所有同名残留。
///
/// 改名失败会原样返回错误（Python 让 `OSError` 冒出去）；扫描阶段走
/// [`delete_path`]，错误一律吞掉。
pub fn delete_installed(path: &Path) -> io::Result<()> {
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return Ok(());
    };
    // `path.exists() or path.is_symlink()` —— 断链符号链接 `exists()` 是假、
    // `is_symlink()` 是真；`symlink_metadata` 对两者都成功，正好等价。
    if fs::symlink_metadata(path).is_ok() {
        let suffix = Uuid::new_v4().simple().to_string();
        let renamed = path.with_file_name(format!("{name}.{}.old", &suffix[..8]));
        fs::rename(path, &renamed)?;
    }
    let prefix = format!("{name}.");
    if let Ok(entries) = fs::read_dir(parent_dir(path)) {
        for entry in entries.flatten() {
            let entry_name = entry.file_name().to_string_lossy().into_owned();
            if is_leftover(&entry_name, &prefix) {
                delete_path(&entry.path());
            }
        }
    }
    Ok(())
}

/// 镜像 `installFile`：先让目标让路，再把 `source` 顶上（`os.replace` 语义）。
pub fn install_file(source: &Path, target: &Path) -> io::Result<()> {
    delete_installed(target)?;
    fs::rename(source, target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = env::temp_dir().join(format!("gd3in-{}-{tag}-{id}", std::process::id()));
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
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        let mut file = fs::File::create(path).expect("create file");
        file.write_all(body).expect("write file");
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut out: Vec<String> = fs::read_dir(dir)
            .expect("read dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        out.sort();
        out
    }

    #[test]
    fn is_leftover_matches_fnmatch_shape() {
        assert!(is_leftover("ffmpeg.1a2b3c4d.old", "ffmpeg."));
        // 中间段可以为空：`ffmpeg.*.old` 匹配 `ffmpeg..old`。
        assert!(is_leftover("ffmpeg..old", "ffmpeg."));
        // 少一个点 → 不匹配。
        assert!(!is_leftover("ffmpeg.old", "ffmpeg."));
        // 前缀不同 → 不匹配（`ffmpeg2.1a2b3c4d.old` 不是 `ffmpeg.*.old`）。
        assert!(!is_leftover("ffmpeg2.1a2b3c4d.old", "ffmpeg."));
        // 结尾不同 → 不匹配。
        assert!(!is_leftover("ffmpeg.1a2b3c4d.old.bak", "ffmpeg."));
    }

    #[test]
    fn parent_dir_falls_back_to_cwd() {
        assert_eq!(parent_dir(Path::new("ffmpeg")), Path::new("."));
        assert_eq!(parent_dir(Path::new("a/ffmpeg")), Path::new("a"));
        assert_eq!(parent_dir(Path::new("/a/ffmpeg")), Path::new("/a"));
    }

    #[test]
    fn delete_installed_removes_file_and_leaves_siblings() {
        let dir = TempDir::new("file");
        let target = dir.path().join("ffmpeg");
        touch(&target, b"bin");
        touch(&dir.path().join("ffmpeg2"), b"other");
        touch(&dir.path().join("other.txt"), b"other");

        delete_installed(&target).expect("delete");

        assert_eq!(names(dir.path()), vec!["ffmpeg2", "other.txt"]);
    }

    #[test]
    fn delete_installed_removes_directory_recursively() {
        let dir = TempDir::new("dir");
        let target = dir.path().join("runtime");
        touch(&target.join("bin/ffmpeg"), b"bin");
        touch(&target.join("share/doc.txt"), b"doc");

        delete_installed(&target).expect("delete");

        assert!(names(dir.path()).is_empty());
    }

    #[test]
    fn delete_installed_cleans_leftovers_from_previous_runs() {
        let dir = TempDir::new("leftover");
        touch(&dir.path().join("ffmpeg.00000000.old"), b"old");
        touch(&dir.path().join("ffmpeg.ffffffff.old"), b"old");
        touch(&dir.path().join("ffmpeg.old"), b"keep"); // 少一个点，不匹配
        touch(&dir.path().join("ffmpeg"), b"new");

        delete_installed(&dir.path().join("ffmpeg")).expect("delete");

        assert_eq!(names(dir.path()), vec!["ffmpeg.old"]);
    }

    #[test]
    fn delete_installed_is_noop_when_missing() {
        let dir = TempDir::new("missing");
        delete_installed(&dir.path().join("nope")).expect("noop");
        assert!(names(dir.path()).is_empty());
    }

    #[test]
    fn delete_installed_removes_broken_symlink() {
        let dir = TempDir::new("link");
        let link = dir.path().join("ffmpeg");
        std::os::unix::fs::symlink(dir.path().join("gone"), &link).expect("symlink");

        delete_installed(&link).expect("delete");

        assert!(names(dir.path()).is_empty());
    }

    #[test]
    fn delete_installed_keeps_symlink_target() {
        let dir = TempDir::new("target");
        let real = dir.path().join("real/ffmpeg");
        touch(&real, b"bin");
        let link = dir.path().join("ffmpeg");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");

        delete_installed(&link).expect("delete");

        assert!(!link.exists());
        assert!(real.exists(), "符号链接只删自己");
    }

    #[test]
    fn delete_installed_ignores_empty_path() {
        // Python 的 `with_name` 在这里抛 ValueError；Rust 直接返回。
        delete_installed(Path::new("")).expect("noop");
    }

    #[test]
    fn install_file_replaces_target_and_cleans_leftovers() {
        let dir = TempDir::new("install");
        let source = dir.path().join("download/ffmpeg");
        let target = dir.path().join("bin/ffmpeg");
        touch(&source, b"new");
        touch(&target, b"old");
        touch(&dir.path().join("bin/ffmpeg.12345678.old"), b"stale");

        install_file(&source, &target).expect("install");

        assert_eq!(fs::read(&target).expect("read"), b"new");
        assert!(!source.exists());
        assert_eq!(names(&dir.path().join("bin")), vec!["ffmpeg"]);
    }

    #[test]
    fn install_file_creates_target_when_absent() {
        let dir = TempDir::new("install-new");
        let source = dir.path().join("ffmpeg");
        let target = dir.path().join("bin/ffmpeg");
        touch(&source, b"bin");
        fs::create_dir_all(dir.path().join("bin")).expect("create bin");

        install_file(&source, &target).expect("install");

        assert_eq!(fs::read(&target).expect("read"), b"bin");
        assert!(!source.exists());
    }

    #[test]
    fn install_file_does_not_create_the_target_parent() {
        // `os.replace` 不建目录：父目录缺失就是 `FileNotFoundError`。
        let dir = TempDir::new("install-noparent");
        let source = dir.path().join("ffmpeg");
        let target = dir.path().join("missing/ffmpeg");
        touch(&source, b"bin");

        let err = install_file(&source, &target).expect_err("parent missing");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(source.exists(), "source 不该被搬走");
    }
}
