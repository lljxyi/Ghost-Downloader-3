//! 路径拼接：镜像 `pathlib.PurePosixPath` 的 `/` 运算符。
//!
//! `Task.outputPath` 在 Python 里是 `str(self.outputFolder / self.name)`，而
//! `pathlib` 的拼接**不是**简单的字符串相连：
//!
//! - 空组件与 `.` 组件被丢弃（`Path("") / "n"` → `n`、`Path("/a/./b") / "n"`
//!   → `/a/b/n`）；
//! - 重复斜杠被压成一个，但**恰好两个**前导斜杠会保留（`//Downloads/n`）——
//!   这是 POSIX「实现定义路径」的遗留语义，Python 原样保留；
//! - `..` **不做消解**（`/a/../b/n` 原样保留）。
//!
//! `std::path::Path::join` 只做「加分隔符再连」，上面四条全都不管，所以这里
//! 单独实现一份。**仅覆盖 POSIX 口径**：迁移目标是 Linux，`tasks.jsonl` 里的
//! `outputFolder` 也是 Python `PosixPath` 写出来的。Windows 上 Python 会给
//! `\`，届时需要另写一份 `WindowsPath` 镜像。

/// 镜像 `str(PurePosixPath(folder) / name)`。
///
/// `name` 是任务名（`toSafeFilename` 之后不含分隔符），原样接在末尾。
pub fn posix_join(folder: &str, name: &str) -> String {
    // 前导斜杠：1 个 = 绝对路径；恰好 2 个 = 保留双斜杠；3 个以上 = 压成 1 个。
    let leading = folder.bytes().take_while(|byte| *byte == b'/').count();
    let (prefix, rest) = match leading {
        0 => ("", folder),
        1 => ("/", &folder[1..]),
        2 => ("//", &folder[2..]),
        _ => ("/", folder.trim_start_matches('/')),
    };

    let mut parts: Vec<&str> = Vec::new();
    for part in rest.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        parts.push(part);
    }

    let mut joined = String::from(prefix);
    for part in &parts {
        joined.push_str(part);
        joined.push('/');
    }
    joined.push_str(name);
    joined
}

/// 镜像 `pathlib.PurePosixPath(name).stem`。
///
/// CPython 的实现是：
///
/// ```python
/// i = name.rfind(".")
/// return name[:i] if 0 < i < len(name) - 1 else name
/// ```
///
/// 注意它跟本项目的 `filename::split_stem_ext` **不是**一回事：后者把
/// `a.tar.gz` 拆成 `("a", ".tar.gz")`，`Path.stem` 只砍最后一段得 `a.tar`。
///
/// 入参是**最后一段路径组件**（`PurePosixPath.name`），不是整条路径：
/// `Path("a/b.txt").stem` 里 `name == "b.txt"`。`.` 与 `..` 在 `name` 阶段
/// 就已经被 `pathlib` 消掉了（`Path(".").name == ""`），所以这里对 `.` 返回
/// `.` 而不是 `""`——任务名走 `toSafeFilename` 之后不可能是这两个值。
pub fn posix_stem(name: &str) -> &str {
    match name.rfind('.') {
        Some(dot) if dot > 0 && dot < name.len() - 1 => &name[..dot],
        _ => name,
    }
}

/// 镜像 `str(PurePosixPath(p))`：去空组件与 `.`、压缩重复斜杠，**不消解 `..`**，
/// 恰好两个前导斜杠保留。
///
/// 期望值来自 CPython 3.13：
///
/// ```text
/// ""      -> "."        "a//b"  -> "a/b"      "///a" -> "/a"
/// "."     -> "."        "/"     -> "/"        "//a"  -> "//a"
/// "a/"    -> "a"        "//"    -> "//"       "a/.." -> "a/.."
/// ```
///
/// 注意它跟 [`posix_join`] 不是一回事：`posix_join("")` 得到 `/name`，而
/// `Path("")` 在 Python 里是 `.`。两者只在「拼接后归一」这一步口径相同。
pub fn posix_normpath(path: &str) -> String {
    // 前导斜杠：0 个 = 相对路径；1 个 = 绝对路径；恰好 2 个 = 保留双斜杠；
    // 3 个以上 = 压成 1 个（与 `posix_join` 同一套规则）。
    let leading = path.bytes().take_while(|byte| *byte == b'/').count();
    let (prefix, rest) = match leading {
        0 => ("", path),
        1 => ("/", &path[1..]),
        2 => ("//", &path[2..]),
        _ => ("/", path.trim_start_matches('/')),
    };

    let parts: Vec<&str> = rest
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();

    if parts.is_empty() {
        // `Path("")` / `Path(".")` → "."，但 `Path("/")` → "/"、`Path("//")` → "//"
        return if prefix.is_empty() {
            String::from(".")
        } else {
            String::from(prefix)
        };
    }

    let mut normalised = String::from(prefix);
    normalised.push_str(&parts.join("/"));
    normalised
}

/// 镜像 `PurePosixPath(p).relative_to(base)`：返回 `str(...)`，不可相对时 `None`
/// （Python 抛 `ValueError`）。
///
/// 三处非显然口径：
/// - 两边必须同为绝对或同为相对（`Path("/a/b").relative_to("")` 是 `ValueError`）；
/// - 「根」是路径的第一段：`//a` 的根是 `//`，与 `/a` 的 `/` **不同**，所以
///   `Path("//a/b").relative_to("/a")` 抛 `ValueError`；
/// - 余下部分为空时返回 `"."`（`Path("/a/b").relative_to("/a/b")`）。
///
/// 期望值来自 CPython 3.13：
///
/// ```text
/// "/a/b"  / "/a"   -> "b"      "/a/b"  / "/a/b" -> "."
/// "/a/b/c"/ "/a"   -> "b/c"    "a"     / "a"    -> "."
/// "//a/b" / "//a"  -> "b"      "//a/b" / "/a"   -> ValueError
/// "///a/b"/ "/a"   -> "b"      "/a/b"  / ""     -> ValueError
/// ```
pub fn posix_relative_to(path: &str, base: &str) -> Option<String> {
    let (path_root, path_parts) = split_root(path);
    let (base_root, base_parts) = split_root(base);
    if path_root != base_root || !path_parts.starts_with(&base_parts) {
        return None;
    }
    let rest = &path_parts[base_parts.len()..];
    if rest.is_empty() {
        return Some(String::from("."));
    }
    Some(rest.join("/"))
}

/// `(根, 组件)`：根是 `"/"` / `"//"` / `""`，组件已去掉空段与 `.`。
fn split_root(path: &str) -> (&str, Vec<&str>) {
    let leading = path.bytes().take_while(|byte| *byte == b'/').count();
    let (root, rest) = match leading {
        0 => ("", path),
        1 => ("/", &path[1..]),
        2 => ("//", &path[2..]),
        _ => ("/", path.trim_start_matches('/')),
    };
    let parts = rest
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    (root, parts)
}

#[cfg(test)]
mod tests {
    use super::{posix_join, posix_relative_to, posix_stem};

    #[test]
    fn matches_pure_posix_path_join() {
        // 期望值全部来自 CPython 3.13：
        // `str(Path(folder) / "name.txt")`
        let cases = [
            ("", "name.txt"),
            (".", "name.txt"),
            ("./", "name.txt"),
            ("a//b", "a/b/name.txt"),
            ("//Downloads", "//Downloads/name.txt"),
            ("/a/", "/a/name.txt"),
            ("/a/./b", "/a/b/name.txt"),
            ("/a/../b", "/a/../b/name.txt"),
            ("a", "a/name.txt"),
            ("/", "/name.txt"),
            ("///x", "/x/name.txt"),
            ("/a b", "/a b/name.txt"),
            ("rel/", "rel/name.txt"),
        ];
        for (folder, expected) in cases {
            assert_eq!(
                posix_join(folder, "name.txt"),
                expected,
                "folder={folder:?}"
            );
        }
    }

    #[test]
    fn keeps_double_slash_only_for_exactly_two() {
        assert_eq!(posix_join("//a", "n"), "//a/n");
        assert_eq!(posix_join("///a", "n"), "/a/n");
        assert_eq!(posix_join("////a//b", "n"), "/a/b/n");
    }

    #[test]
    fn posix_stem_matches_pathlib() {
        // 期望值来自 CPython：`PurePosixPath(name).stem`
        let cases = [
            ("a.tar.gz", "a.tar"),
            ("a.bin", "a"),
            (".bashrc", ".bashrc"),
            ("a.", "a."),
            ("noext", "noext"),
            ("..a", "."),
            ("a..b", "a."),
            ("...", "..."),
            (".a.b", ".a"),
            ("a.b.", "a.b."),
            ("..", ".."),
        ];
        for (name, expected) in cases {
            assert_eq!(posix_stem(name), expected, "name={name:?}");
        }
    }

    #[test]
    fn drops_dot_and_empty_components() {
        assert_eq!(posix_join("./a/./b/.", "n"), "a/b/n");
        assert_eq!(posix_join("..", "n"), "../n");
    }

    #[test]
    fn relative_to_matches_pathlib() {
        // 期望值全部来自 CPython 3.13：`str(Path(p).relative_to(base))`
        let cases = [
            ("/a/b", "/a", Some("b")),
            ("/a/b", "/a/b", Some(".")),
            ("/a/b/c", "/a", Some("b/c")),
            ("a/b", "a", Some("b")),
            ("a", "a", Some(".")),
            ("///a/b", "/a", Some("b")),
            ("/a/./b", "/a", Some("b")),
            ("/a/b/", "/a/", Some("b")),
            ("//a/b", "//a", Some("b")),
            // 下面全是 Python 的 `ValueError`：
            ("/a", "/a/b", None),
            ("//a/b", "/a", None),
            ("/a/b", "//a", None),
            ("a", "b", None),
            ("/a/b", "", None),
        ];
        for (path, base, expected) in cases {
            assert_eq!(
                posix_relative_to(path, base).as_deref(),
                expected,
                "path={path:?} base={base:?}"
            );
        }
    }
}

#[cfg(test)]
mod normpath_tests {
    use super::posix_normpath;

    #[test]
    fn matches_str_of_pure_posix_path() {
        // 期望值全部来自 CPython 3.13：`str(PurePosixPath(p))`
        let cases = [
            ("", "."),
            (".", "."),
            ("./", "."),
            ("a/", "a"),
            ("a//", "a"),
            ("a//b", "a/b"),
            ("/a//b/", "/a/b"),
            ("//", "//"),
            ("///a", "/a"),
            ("/", "/"),
            ("//a", "//a"),
            ("a/.", "a"),
            ("a/..", "a/.."),
            ("../a", "../a"),
            ("a\\b", "a\\b"),
            ("./a/./b", "a/b"),
            ("//a//b/./c", "//a/b/c"),
        ];
        for (raw, expected) in cases {
            assert_eq!(posix_normpath(raw), expected, "raw={raw:?}");
        }
    }
}
