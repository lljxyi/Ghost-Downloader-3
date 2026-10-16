//! 文件名净化与拆分：镜像 `app/platform/filesystem.py` 的 `toSafeFilename` / `splitStemExt`。
//!
//! 规格的唯一来源是 Python：`tests/fixtures/filename.jsonl` 由
//! `tests/fixtures/generate_filename_fixture.py` 生成，Rust 侧只读、逐字节对比。

/// 非法字符（Python `INVALID_FILENAME_PATTERN` 里 `<>:"/\|?*` 那一段）。
const INVALID_FILENAME_CHARS: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// Windows 保留名（Python `WINDOWS_RESERVED_FILENAMES`）。
const WINDOWS_RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// 默认长度上限（Python `toSafeFilename(maxLength=200)`）。
pub const DEFAULT_MAX_FILENAME_LENGTH: usize = 200;

/// 净化文件名（Python `toSafeFilename`）。
///
/// 顺序照抄 Python：非法字符整段折成一个下划线 → 去首尾空白与尾部点/空格 →
/// 空/`.`/`..` 用兜底名 → 保留名前缀下划线 → 超长时优先保住最后一段扩展名。
pub fn to_safe_filename(name: &str, fallback: &str, max_length: usize) -> String {
    let mut candidate = String::with_capacity(name.len());
    let mut in_invalid_run = false;
    for ch in name.chars() {
        // Python 的字符类是 `[\x00-\x1f\x7f<>:"/\|?*]`：注意只到 \x7f，
        // 不能用 Rust 的 `is_control()`（那还会吃掉 U+0080–U+009F）
        let invalid = ch <= '\u{1f}' || ch == '\u{7f}' || INVALID_FILENAME_CHARS.contains(&ch);
        if invalid {
            if !in_invalid_run {
                candidate.push('_');
                in_invalid_run = true;
            }
        } else {
            in_invalid_run = false;
            candidate.push(ch);
        }
    }

    let trimmed = candidate.trim().trim_end_matches(['.', ' ']);
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        return fallback.to_string();
    }

    let mut candidate = trimmed.to_string();
    let root = candidate.split('.').next().unwrap_or("");
    if WINDOWS_RESERVED
        .iter()
        .any(|reserved| root.eq_ignore_ascii_case(reserved))
    {
        candidate.insert(0, '_');
    }

    if max_length == 0 {
        return candidate;
    }

    // Python 的 len() 与切片都按码点算，所以这里也按 char 切
    let chars: Vec<char> = candidate.chars().collect();
    if chars.len() <= max_length {
        return candidate;
    }

    match chars.iter().rposition(|ch| *ch == '.') {
        // `rpartition(".")`：stem 非空且有点才算「有扩展名」
        Some(dot) if dot > 0 => {
            let suffix: String = chars[dot..].iter().collect();
            let keep = max_length as i64 - suffix.chars().count() as i64;
            if keep > 0 {
                let keep = (keep as usize).min(dot);
                let stem: String = chars[..keep].iter().collect();
                format!("{stem}{suffix}")
            } else {
                chars[..max_length].iter().collect()
            }
        }
        _ => chars[..max_length].iter().collect(),
    }
}

/// 拆出主干与扩展名（Python `splitStemExt`）。
///
/// 复合扩展名（`.tar.gz` 这类）整段算扩展名。
pub fn split_stem_ext(name: &str) -> (String, String) {
    if let Some(start) = compound_extension_start(name) {
        return (name[..start].to_string(), name[start..].to_string());
    }
    match name.rfind('.') {
        Some(dot) if dot > 0 => (name[..dot].to_string(), name[dot..].to_string()),
        _ => (name.to_string(), String::new()),
    }
}

/// 复合扩展名的起点：Python 正则 `\.[A-Za-z0-9]{1,4}\.(?:bz2?|gz|lzma|lzo|xz|z|zst)$` 的等价实现。
///
/// 归档扩展名固定取最后一个点之后的那段，所以只要再往前找一个「1–4 个字母数字」的段。
fn compound_extension_start(name: &str) -> Option<usize> {
    const ARCHIVE_EXTENSIONS: [&str; 8] = ["bz2", "bz", "gz", "lzma", "lzo", "xz", "z", "zst"];

    let archive_dot = name.rfind('.')?;
    let archive_extension = &name[archive_dot + 1..];
    if !ARCHIVE_EXTENSIONS
        .iter()
        .any(|ext| ext.eq_ignore_ascii_case(archive_extension))
    {
        return None;
    }

    let middle_dot = name[..archive_dot].rfind('.')?;
    let middle = &name[middle_dot + 1..archive_dot];
    let length = middle.chars().count();
    if !(1..=4).contains(&length) || !middle.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return None;
    }
    Some(middle_dot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_filename_keeps_ordinary_names() {
        assert_eq!(to_safe_filename("a.bin", "file", 200), "a.bin");
        assert_eq!(
            to_safe_filename("视频 名字.mp4", "file", 200),
            "视频 名字.mp4"
        );
        assert_eq!(to_safe_filename("a😀b.mp4", "file", 200), "a😀b.mp4");
    }

    #[test]
    fn safe_filename_collapses_invalid_runs() {
        assert_eq!(to_safe_filename("a/b\\c:d", "file", 200), "a_b_c_d");
        assert_eq!(to_safe_filename("<>:\"/\\|?*", "file", 200), "_");
    }

    #[test]
    fn safe_filename_uses_fallback_for_degenerate_names() {
        assert_eq!(to_safe_filename("", "兜底", 200), "兜底");
        assert_eq!(to_safe_filename("   ", "兜底", 200), "兜底");
        assert_eq!(to_safe_filename("..", "兜底", 200), "兜底");
    }

    #[test]
    fn safe_filename_prefixes_windows_reserved_names() {
        assert_eq!(to_safe_filename("con", "file", 200), "_con");
        assert_eq!(to_safe_filename("NUL.tar.gz", "file", 200), "_NUL.tar.gz");
        assert_eq!(to_safe_filename("COM10.log", "file", 200), "COM10.log");
    }

    #[test]
    fn safe_filename_truncates_by_code_points() {
        // 5 个字：stem 只剩 2 个字，扩展名保住
        assert_eq!(to_safe_filename("abcdef.gh", "file", 5), "ab.gh");
        assert_eq!(to_safe_filename("abcdef.gh", "file", 3), "abc");
        assert_eq!(to_safe_filename("abcdef.gh", "file", 0), "abcdef.gh");
        // 按码点而不是字节切：11 个字里留下 6 个（stem 2 + ".mkv" 4）
        assert_eq!(to_safe_filename("名字名字名字.mkv", "file", 6), "名字.mkv");
    }

    #[test]
    fn split_stem_ext_handles_compound_extensions() {
        assert_eq!(split_stem_ext("a.tar.gz"), ("a".into(), ".tar.gz".into()));
        assert_eq!(
            split_stem_ext("a.tar.gz.asc"),
            ("a.tar.gz".into(), ".asc".into())
        );
        assert_eq!(split_stem_ext("a.tar.7z"), ("a.tar".into(), ".7z".into()));
        assert_eq!(split_stem_ext(".hidden"), (".hidden".into(), "".into()));
        assert_eq!(split_stem_ext("a."), ("a".into(), ".".into()));
    }
}
