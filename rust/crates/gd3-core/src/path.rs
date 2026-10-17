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

#[cfg(test)]
mod tests {
    use super::{posix_join, posix_stem};

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
}
