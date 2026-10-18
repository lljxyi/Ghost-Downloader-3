//! `app/update.py` 的版本解析与比较（`:11-31`）。
//!
//! 实现原本在 `gd3-ui::setting_cards`（第 62 步），但 `gd3-core::loader` 的
//! `gdMinVersion` 门禁也要用它，而 `gd3-core` 不能反过来依赖 `gd3-ui` —— 所以搬到
//! 这里，`gd3-ui::setting_cards` 改成 `pub use`。
//!
//! ## `parseVersion` 的精确口径
//!
//! 1. `lstrip("vV")`：只去**前导**的 `v` / `V`，不是 `strip`；
//! 2. 逐字符累积十进制数字；
//! 3. 遇到 `.` **且**当前已累积到数字时切一段，否则走第 4 条；
//! 4. 遇到其它任何字符立即 `break`（`1.2b3` → `(1, 2)`、`" 4.4"` → `()`）；
//! 5. 循环结束后若还有未切段的数字，补一段（`1.2.3.` → `(1, 2, 3)`）；
//! 6. 去掉**尾部**的 0 段（`1.2.0.0` → `(1, 2)`），中间的 0 保留（`1.0.2` → `(1, 0, 2)`）；
//! 7. 返回元组，比较用 Python 元组语义 —— 短前缀更小，所以 `(1, 2) < (1, 2, 0, 1)`
//!    即使后者去尾零后是 `(1, 2, 0, 1)`。
//!
//! ## 与 Python 的已知差异
//!
//! Python 的 `str.isdigit()` 对 Unicode 十进制数字为真，`int(ch)` 也接受它们 ——
//! CPython 3.13 实测 `parseVersion("１.２")` 得 `(1, 2)`。Rust 的 `char::to_digit(10)`
//! 只认 ASCII，`parse_version("１.２")` 得 `()`。版本串实际只用 ASCII 数字，
//! `manifest.toml` 与 `UserConfig.json` 里不会出现全角数字。

/// 镜像 `app/update.py::parseVersion`（:11-27）。
pub fn parse_version(text: &str) -> Vec<u64> {
    let mut segments: Vec<u64> = Vec::new();
    let mut acc: u64 = 0;
    let mut has_digit = false;
    for ch in text.trim_start_matches(['v', 'V']).chars() {
        if let Some(digit) = ch.to_digit(10) {
            acc = acc * 10 + u64::from(digit);
            has_digit = true;
        } else if ch == '.' && has_digit {
            segments.push(acc);
            acc = 0;
            has_digit = false;
        } else {
            break;
        }
    }
    if has_digit {
        segments.push(acc);
    }
    while segments.last() == Some(&0) {
        segments.pop();
    }
    segments
}

/// `isNewer(current, latest) = parseVersion(latest) > parseVersion(current)`。
pub fn is_newer(current: &str, latest: &str) -> bool {
    parse_version(latest) > parse_version(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_version_mirrors_update_py() {
        assert_eq!(parse_version("v1.2.3"), vec![1, 2, 3]);
        assert_eq!(parse_version("VV2.0.0"), vec![2]);
        assert_eq!(parse_version("1.0.0.0"), vec![1]);
        assert_eq!(parse_version(""), Vec::<u64>::new());
        // 非数字开头 → 空（"latest" 完全解析不出）。
        assert_eq!(parse_version("latest"), Vec::<u64>::new());
        // 遇到非法字符即停：`1.2b3` → [1, 2]。
        assert_eq!(parse_version("1.2b3"), vec![1, 2]);
        // 连续点：第一个点后 hasDigit 为假，第二个点走 else → break。
        assert_eq!(parse_version("1..2"), vec![1]);
    }

    #[test]
    fn parse_version_keeps_inner_zeroes_and_drops_trailing_ones() {
        // CPython 3.13 实测：`1.0.2` 中间的 0 保留。
        assert_eq!(parse_version("1.0.2"), vec![1, 0, 2]);
        // 尾部 0 全去：`1.2.3.4.5.0.0` → (1,2,3,4,5)。
        assert_eq!(parse_version("1.2.3.4.5.0.0"), vec![1, 2, 3, 4, 5]);
        // 前导 0 只是数值 0：`0001` → (1,)。
        assert_eq!(parse_version("0001"), vec![1]);
        // `0.0` 全被去掉 → 空（等价于「没有版本」）。
        assert_eq!(parse_version("0.0"), Vec::<u64>::new());
    }

    #[test]
    fn parse_version_stops_at_the_first_non_version_char() {
        // 实测：`1.2.3-rc1` → (1,2,3)；`4.4.0\n` → (4,4)；` 4.4` → ()。
        assert_eq!(parse_version("1.2.3-rc1"), vec![1, 2, 3]);
        assert_eq!(parse_version("4.4.0\n"), vec![4, 4]);
        assert_eq!(parse_version(" 4.4"), Vec::<u64>::new());
        assert_eq!(parse_version(".1.2"), Vec::<u64>::new());
        assert_eq!(parse_version("1.2.3."), vec![1, 2, 3]);
        assert_eq!(parse_version("v"), Vec::<u64>::new());
    }

    #[test]
    fn is_newer_compares_parsed_tuples() {
        assert!(is_newer("1.2.3", "1.2.4"));
        assert!(!is_newer("1.2.4", "1.2.3"));
        // 去尾零后相等。
        assert!(!is_newer("1.2.0", "1.2"));
        // 短前缀更小（Python 元组语义）。
        assert!(is_newer("1.2", "1.2.0.1"));
        assert!(!is_newer("", ""));
        // 解析不出的一方是空元组：`()` < `(4, 4)`。
        assert!(is_newer("latest", "4.4.0"));
        assert!(!is_newer("4.4.0", "latest"));
    }
}
