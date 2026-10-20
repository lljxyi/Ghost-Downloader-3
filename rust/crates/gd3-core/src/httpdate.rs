//! HTTP-date 解析与「保留文件修改时间」。
//!
//! 镜像 `features/http_pack/rust_engine.py:103-110` 的 `_restoreMtime`
//! （以及 `features/http_pack/task.py:472-480` 里那份重复实现）：
//!
//! ```python
//! if not cfg.shouldPreserveLastModified.value or not step.lastModified:
//!     return
//! try:
//!     mtime = parsedate_to_datetime(step.lastModified).timestamp()
//!     os.utime(Path(step.outputPath), (mtime, mtime))
//! except Exception as e:
//!     logger.opt(exception=e).warning("设置文件修改时间失败 {}", step.outputPath)
//! ```
//!
//! `step.lastModified` 是创建任务时（`pack.py:177` 取响应头 `last-modified`、
//! `rust_probe.py:48`）存进 `tasks.jsonl` 的原始字符串，所以这里要自己做解析：
//! `email.utils.parsedate_to_datetime` 认 HTTP-date 的三种历史格式
//! （IMF-fixdate / RFC 850 / asctime），std 没有对应物。
//!
//! | Python | Rust |
//! |---|---|
//! | `parsedate_to_datetime(s).timestamp()` | [`parse`] / [`parse_unix_seconds`] |
//! | `os.utime(path, (mtime, mtime))` | [`apply_last_modified`] |
//!
//! ## 与 Python 的已知差异
//!
//! 1. **只改 mtime，不改 atime**。`os.utime(path, (mtime, mtime))` 两个都设；
//!    std 只有 [`std::fs::File::set_modified`]。设 atime 需要 `filetime` 这类外部
//!    依赖，为了一个「看得见的属性」不值得，这里只落 mtime（atime 由内核随手更新）。
//! 2. **不认识的时区名按 UTC 处理**。Python 走 `email.utils._timezones` 表，表外的
//!    缩写让 `parsedate_to_datetime` 得到 naive datetime，`.timestamp()` 再按**本地**
//!    时区解释 —— 结果取决于运行机器的 `TZ`。我们固定 UTC（可复现）。表内的缩写
//!    （EST/EDT/CST/CDT/MST/MDT/PST/PDT/AST/ADT/UT/UTC/GMT/Z）两侧一致。
//! 3. 数值时区除 `+0800` 之外也接受 `+08:00`（Python 的 `parsedate` 只吃前者）。
//!    收窄会得到 `None`，放宽只会让更多输入可用，属于安全方向。
//! 4. 星期名只跳过、不校验（Python 的 `parsedate_tz` 同样不校验）。
//! 5. 解析失败一律 `None`，不区分「缺字段」和「越界」。Python 侧是
//!    `parsedate_to_datetime` 返回 `None` 后 `.timestamp()` 抛 `AttributeError`
//!    —— 最终都是那条 `设置文件修改时间失败` 告警，调用方看到的形态一致。

use std::fmt;
use std::fs::File;
use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// HTTP-date 里的月份缩写（小写，比对时只取前三个字母）。
const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

/// `email.utils._timezones` 的同一张表（值时区偏移，单位小时）。
const ZONES: [(&str, i64); 14] = [
    ("ut", 0),
    ("utc", 0),
    ("gmt", 0),
    ("z", 0),
    ("ast", -4),
    ("adt", -3),
    ("est", -5),
    ("edt", -4),
    ("cst", -6),
    ("cdt", -5),
    ("mst", -7),
    ("mdt", -6),
    ("pst", -8),
    ("pdt", -7),
];

/// [`apply_last_modified`] 的失败原因（只用来打日志，不参与控制流）。
#[derive(Debug)]
pub enum LastModifiedError {
    /// 时间字符串解析不出来。
    Unparsable(String),
    /// 改时间戳本身失败（文件不存在 / 没有权限 / 文件系统不认）。
    Io(io::Error),
}

impl fmt::Display for LastModifiedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LastModifiedError::Unparsable(raw) => {
                write!(f, "无法解析 Last-Modified：{raw:?}")
            }
            LastModifiedError::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for LastModifiedError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LastModifiedError::Unparsable(_) => None,
            LastModifiedError::Io(error) => Some(error),
        }
    }
}

/// 解析 HTTP-date，返回 Unix 秒（`parsedate_to_datetime(s).timestamp()` 的整数版）。
///
/// 三种格式都认：`Sun, 06 Nov 1994 08:49:37 GMT`（IMF-fixdate）、
/// `Sunday, 06-Nov-94 08:49:37 GMT`（RFC 850）、`Sun Nov  6 08:49:37 1994`（asctime）。
pub fn parse_unix_seconds(raw: &str) -> Option<i64> {
    let text = raw.trim();
    if text.is_empty() {
        return None;
    }

    let mut day: Option<i64> = None;
    let mut month: Option<i64> = None;
    let mut year: Option<i64> = None;
    let mut time: Option<(i64, i64, i64)> = None;
    // `None` = 没见过时区（含不认识的缩写），一律按 UTC 算。
    let mut offset: Option<i64> = None;

    for token in text.split(|c: char| c.is_whitespace() || c == ',') {
        if token.is_empty() {
            continue;
        }
        // `+0800` / `-0500` / `+08:00`
        if token.starts_with('+')
            || (token.starts_with('-') && token[1..].starts_with(|c: char| c.is_ascii_digit()))
        {
            if let Some(value) = parse_offset(token) {
                offset.get_or_insert(value);
            }
            continue;
        }
        // `06-Nov-94` / `06-Nov-1994`
        if token.contains('-') {
            let parts: Vec<&str> = token.split('-').collect();
            if parts.len() == 3
                && let (Some(d), Some(m), Some(y)) = (
                    parse_digits(parts[0]),
                    month_of(parts[1]),
                    parse_digits(parts[2]),
                )
            {
                day.get_or_insert(d);
                month.get_or_insert(m);
                year.get_or_insert(y);
            }
            continue;
        }
        // `08:49:37`
        if token.contains(':') {
            if let Some(parsed) = parse_time(token) {
                time.get_or_insert(parsed);
            }
            continue;
        }
        if token.chars().all(|c| c.is_ascii_digit()) {
            let value = parse_digits(token)?;
            // 三位以上一定是年（`1994`）；一位/两位按「日」优先（`06 Nov 1994`）。
            if token.len() >= 3 || day.is_some() {
                year.get_or_insert(value);
            } else if (1..=31).contains(&value) {
                day.get_or_insert(value);
            } else {
                year.get_or_insert(value);
            }
            continue;
        }
        // 字母：月份名 / 时区缩写 / 星期名（星期名直接跳过）
        if let Some(found) = month_of(token) {
            month.get_or_insert(found);
        } else if let Some(found) = zone_of(token) {
            offset.get_or_insert(found);
        }
    }

    let (day, month, year) = (day?, month?, year?);
    let (hour, minute, second) = time?;
    if year < 1 || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    // 两位年份的世纪归属照 `email._parseaddr._parsedate_tz`：
    // `yy += 1900; if yy < 1969: yy += 100` —— 0..=68 落到 2000 年代，69..=99 到 1900 年代。
    let year = if year < 100 {
        if year < 69 { year + 2000 } else { year + 1900 }
    } else {
        year
    };

    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second - offset.unwrap_or(0);
    Some(seconds)
}

/// [`parse_unix_seconds`] 的 [`SystemTime`] 版（1970 年之前也照样能表示）。
pub fn parse(raw: &str) -> Option<SystemTime> {
    let seconds = parse_unix_seconds(raw)?;
    if seconds >= 0 {
        UNIX_EPOCH.checked_add(Duration::from_secs(seconds as u64))
    } else {
        UNIX_EPOCH.checked_sub(Duration::from_secs(seconds.unsigned_abs()))
    }
}

/// 把文件的 mtime 设成 `raw` 里的时间（`os.utime(path, (mtime, mtime))` 的 mtime 部分）。
///
/// 返回落下去的时间，方便调用方打日志 / 断言。
pub fn apply_last_modified(path: &Path, raw: &str) -> Result<SystemTime, LastModifiedError> {
    let time = parse(raw).ok_or_else(|| LastModifiedError::Unparsable(raw.to_string()))?;
    let file = open_for_times(path).map_err(LastModifiedError::Io)?;
    file.set_modified(time).map_err(LastModifiedError::Io)?;
    Ok(time)
}

/// `os.utime` 只要「是属主 / 有写权限」就够，不需要写句柄；先试写开，失败退只读。
fn open_for_times(path: &Path) -> io::Result<File> {
    match File::options().write(true).open(path) {
        Ok(file) => Ok(file),
        Err(_) => File::open(path),
    }
}

fn parse_digits(token: &str) -> Option<i64> {
    if token.is_empty() || !token.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    token.parse().ok()
}

/// `Nov` / `November` → 11（只比前三个字母，大小写无关）。
fn month_of(token: &str) -> Option<i64> {
    let lower = token.to_ascii_lowercase();
    let head = lower.get(..3)?;
    if !head.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    MONTHS
        .iter()
        .position(|name| *name == head)
        .map(|index| index as i64 + 1)
}

/// `GMT` / `Z` / `EST` … → 偏移秒。
fn zone_of(token: &str) -> Option<i64> {
    let lower = token.to_ascii_lowercase();
    ZONES
        .iter()
        .find(|(name, _)| *name == lower)
        .map(|(_, hours)| hours * 3_600)
}

/// `+0800` / `-0500` / `+08:00` → 偏移秒。
fn parse_offset(token: &str) -> Option<i64> {
    let (sign, rest) = match token.as_bytes().first()? {
        b'-' => (-1_i64, &token[1..]),
        b'+' => (1_i64, &token[1..]),
        _ => return None,
    };
    let digits: String = rest.chars().filter(|c| *c != ':').collect();
    if digits.len() != 4 {
        return None;
    }
    let hours = parse_digits(&digits[..2])?;
    let minutes = parse_digits(&digits[2..])?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some(sign * (hours * 3_600 + minutes * 60))
}

/// `08:49:37`（秒可以带小数，Python 也会截掉）。
fn parse_time(token: &str) -> Option<(i64, i64, i64)> {
    let parts: Vec<&str> = token.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let hour = parse_digits(parts[0])?;
    let minute = parse_digits(parts[1])?;
    let second: i64 = parts[2].split('.').next().and_then(parse_digits)?;
    Some((hour, minute, second))
}

/// 公历年月日 → 距 1970-01-01 的天数（Howard Hinnant 的 `days_from_civil`）。
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// `Sun, 06 Nov 1994 08:49:37 GMT` 的 Unix 秒（HTTP-date 的经典样本）。
    const EPOCH: i64 = 784_111_777;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = env::temp_dir().join(format!("gd3hd-{}-{tag}-{id}", std::process::id()));
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

    #[test]
    fn imf_fixdate() {
        assert_eq!(
            parse_unix_seconds("Sun, 06 Nov 1994 08:49:37 GMT"),
            Some(EPOCH)
        );
    }

    #[test]
    fn rfc850_and_asctime() {
        assert_eq!(
            parse_unix_seconds("Sunday, 06-Nov-94 08:49:37 GMT"),
            Some(EPOCH)
        );
        assert_eq!(parse_unix_seconds("Sun Nov  6 08:49:37 1994"), Some(EPOCH));
    }

    #[test]
    fn numeric_and_named_offsets() {
        // `+0200` = 当地时间，比 UTC 早两小时 → 少 7200 秒
        assert_eq!(
            parse_unix_seconds("Sun, 06 Nov 1994 10:49:37 +0200"),
            Some(EPOCH)
        );
        assert_eq!(
            parse_unix_seconds("Sun, 06 Nov 1994 10:49:37 +02:00"),
            Some(EPOCH)
        );
        // `EST` = UTC-5 → 本地 03:49:37
        assert_eq!(
            parse_unix_seconds("Sun, 06 Nov 1994 03:49:37 EST"),
            Some(EPOCH)
        );
        // 不认识的缩写按 UTC（文档里的差异 2）
        assert_eq!(
            parse_unix_seconds("Sun, 06 Nov 1994 08:49:37 XYZ"),
            Some(EPOCH)
        );
    }

    #[test]
    fn two_digit_year_pivots_like_python() {
        let old = parse_unix_seconds("Mon, 01 Jan 68 00:00:00 GMT").expect("2068");
        let new = parse_unix_seconds("Mon, 01 Jan 2068 00:00:00 GMT").expect("2068");
        assert_eq!(old, new);
        let old = parse_unix_seconds("Thu, 01 Jan 69 00:00:00 GMT").expect("1969");
        let new = parse_unix_seconds("Wed, 01 Jan 1969 00:00:00 GMT").expect("1969");
        assert_eq!(old, new);
    }

    #[test]
    fn matches_python_parsedate() {
        // 这张表是 `email.utils.parsedate_to_datetime(s).timestamp()` 的实跑结果
        // （Python 3.13，`TZ=UTC`），逐条对齐，避免「自洽但错」。
        let cases: [(&str, i64); 12] = [
            ("Sun, 06 Nov 1994 08:49:37 GMT", 784_111_777),
            ("Sunday, 06-Nov-94 08:49:37 GMT", 784_111_777),
            ("Sun Nov  6 08:49:37 1994", 784_111_777),
            ("Sun, 06 Nov 1994 10:49:37 +0200", 784_111_777),
            ("Sun, 06 Nov 1994 03:49:37 EST", 784_111_777),
            ("Mon, 01 Jan 68 00:00:00 GMT", 3_092_601_600),
            ("Thu, 01 Jan 69 00:00:00 GMT", -31_536_000),
            ("Wed, 21 Oct 2015 07:28:00 GMT", 1_445_412_480),
            ("Fri, 13 Feb 2009 23:31:30 -0800", 1_234_596_690),
            ("Tue, 29 Feb 2028 12:00:00 GMT", 1_835_438_400),
            ("Thu, 01 Jan 1970 00:00:00 GMT", 0),
            ("Wed, 01 Jan 1969 00:00:00 GMT", -31_536_000),
        ];
        for (raw, expected) in cases {
            assert_eq!(parse_unix_seconds(raw), Some(expected), "{raw:?}");
        }
    }

    #[test]
    fn month_names_accept_full_and_abbreviated() {
        assert_eq!(month_of("November"), Some(11));
        assert_eq!(month_of("nov"), Some(11));
        assert_eq!(month_of("NOV"), Some(11));
        assert_eq!(month_of("Nope"), None);
        // 非 ASCII 不能让 `get(..3)` 切在字符中间
        assert_eq!(month_of("十一月"), None);
    }

    #[test]
    fn rejects_malformed_input() {
        for raw in [
            "",
            "   ",
            "garbage",
            "06 Nov 1994",
            "Sun, 06 Nov 1994",
            "Sun, 06 Foo 1994 08:49:37 GMT",
            "Sun, 32 Nov 1994 08:49:37 GMT",
            "Sun, 06 Nov 1994 24:49:37 GMT",
            "Sun, 06 Nov 1994 08:61:37 GMT",
            "Sun, 06 Nov -1994 08:49:37 GMT",
        ] {
            assert_eq!(parse_unix_seconds(raw), None, "应拒绝 {raw:?}");
        }
    }

    #[test]
    fn parse_maps_to_system_time() {
        let time = parse("Sun, 06 Nov 1994 08:49:37 GMT").expect("可解析");
        assert_eq!(
            time.duration_since(UNIX_EPOCH)
                .expect("1970 之后")
                .as_secs(),
            EPOCH as u64
        );
        // 1970 之前也能表示
        let before = parse("Wed, 01 Jan 1969 00:00:00 GMT").expect("1969");
        assert!(before < UNIX_EPOCH);
        assert_eq!(
            UNIX_EPOCH.duration_since(before).expect("差值").as_secs(),
            31_536_000
        );
    }

    #[test]
    fn apply_sets_mtime_on_existing_file() {
        let dir = TempDir::new("mtime");
        let file = dir.path().join("out.bin");
        fs::write(&file, b"payload").expect("写文件");
        let applied =
            apply_last_modified(&file, "Sun, 06 Nov 1994 08:49:37 GMT").expect("落 mtime");
        assert_eq!(
            applied
                .duration_since(UNIX_EPOCH)
                .expect("1970 之后")
                .as_secs(),
            EPOCH as u64
        );
        let meta = fs::metadata(&file).expect("stat");
        let modified = meta.modified().expect("mtime");
        assert_eq!(
            modified
                .duration_since(UNIX_EPOCH)
                .expect("1970 之后")
                .as_secs(),
            EPOCH as u64
        );
        // 内容不能被动过
        assert_eq!(fs::read(&file).expect("读回"), b"payload");
    }

    #[test]
    fn apply_handles_read_only_file() {
        let dir = TempDir::new("mtime-ro");
        let file = dir.path().join("ro.bin");
        fs::write(&file, b"x").expect("写文件");
        let mut perms = fs::metadata(&file).expect("stat").permissions();
        perms.set_readonly(true);
        fs::set_permissions(&file, perms).expect("禁用写");
        // 只读文件照样能改时间（`os.utime` 只要属主）
        apply_last_modified(&file, "Sun, 06 Nov 1994 08:49:37 GMT").expect("只读也要能改 mtime");
        let mut perms = fs::metadata(&file).expect("stat").permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        fs::set_permissions(&file, perms).expect("恢复写");
    }

    #[test]
    fn apply_reports_unparsable_and_missing() {
        let dir = TempDir::new("mtime-bad");
        let file = dir.path().join("out.bin");
        fs::write(&file, b"x").expect("写文件");
        match apply_last_modified(&file, "not a date") {
            Err(LastModifiedError::Unparsable(raw)) => assert_eq!(raw, "not a date"),
            other => panic!("应报解析失败，实际 {other:?}"),
        }
        match apply_last_modified(
            &dir.path().join("nope.bin"),
            "Sun, 06 Nov 1994 08:49:37 GMT",
        ) {
            Err(LastModifiedError::Io(error)) => {
                assert_eq!(error.kind(), io::ErrorKind::NotFound);
            }
            other => panic!("应报 IO 失败，实际 {other:?}"),
        }
    }
}
