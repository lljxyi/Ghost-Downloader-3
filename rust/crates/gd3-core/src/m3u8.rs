//! m3u8 pack 的输出解析层（镜像 `features/m3u8_pack/task.py` 的
//! `parseDiagnostic` / `parseError` / `M3U8TaskStep._parseOutputLine`）。
//!
//! 三块都是**纯文本解析**：输入是 N_m3u8DL-RE 的一行（或整段）stdout，输出是
//! 进度字段与错误分类。没有网络、没有状态、不碰 `cfg`，所以整块可以整体下沉。
//!
//! 正则用 `regex` crate 照抄 Python 的 `re` 模式：两者都是 leftmost-first
//! 语义，对这里用到的构造（`\d` / `\s` / `(?:)` / `|` / 可选组 / 贪婪量词）
//! 匹配结果逐条一致。取「最后一个匹配」照抄 Python 的
//! `for m in PATTERN.finditer(text): hit = m` —— 非重叠扫描取末条。
//!
//! 两处已知的、不影响实际输入的偏差：
//!
//! - Python `str.strip()` 会剥 `\x1c`–`\x1f`，Rust `str::trim` 不会（Unicode
//!   `White_Space` 之外的字符）；
//! - `toBytes` 的 `float(value)` 在 Python 里会抛 `ValueError`，这里退化成 0；
//!   正则已经把取值限制成 `\d+\.\d+`，实际取不到非法输入。

use regex::Regex;
use std::sync::LazyLock;

/// 镜像 `VOD_PROGRESS_PATTERN`（`task.py` 顶部）。
static VOD_PROGRESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(\d+)/(\d+)\s+(\d+\.\d+)%\s+(?:(?:(\d+\.\d+)(KB|MB|GB|B)/(\d+\.\d+)(KB|MB|GB|B))|-)?\s*(\d+\.\d+)(GBps|MBps|KBps|Bps)",
    )
    .expect("VOD_PROGRESS_PATTERN")
});

/// 镜像 `LIVE_PROGRESS_PATTERN`（`_DURATION` / `_SPEED` 展开后）。
static LIVE_PROGRESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"((?:\d{2}h)?\d{2}m\d{2}s)/((?:\d{2}h)?\d{2}m\d{2}s)\s+\d+/\d+\s+(Recording|Waiting)\s+(\d+)%\s+(-|(\d+\.\d+)(GBps|MBps|KBps|Bps))",
    )
    .expect("LIVE_PROGRESS_PATTERN")
});

/// 镜像 `LIVE_RECORD_PATTERN`。
static LIVE_RECORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"((?:\d{2}h)?\d{2}m\d{2}s)/((?:\d{2}h)?\d{2}m\d{2}s)\s+\d+\.\d+(?:KB|MB|GB|B)\s+\d+/\d+\s+(Recording|Waiting)\s+(-|(\d+\.\d+)(GBps|MBps|KBps|Bps))",
    )
    .expect("LIVE_RECORD_PATTERN")
});

/// 镜像 `LOG_LINE_PATTERN`。
static LOG_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\d{2}:\d{2}:\d{2}\.\d{3}\s+(WARN|ERROR)\s*:\s*(.*)").expect("LOG_LINE_PATTERN")
});

/// 镜像 `UNHANDLED_PATTERN`。
static UNHANDLED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Unhandled exception:\s*\S+:\s*(.*)").expect("UNHANDLED_PATTERN"));

/// 镜像 `ERROR_HINTS`：`(needle, message)`，顺序即优先级。
const ERROR_HINTS: [(&str, &str); 9] = [
    (
        "response status code does not indicate success: 403",
        "服务器拒绝了请求（403），链接可能已失效（{detail}）",
    ),
    (
        "response status code does not indicate success: 404",
        "资源不存在（404），链接可能已失效（{detail}）",
    ),
    (
        "response status code does not indicate success:",
        "服务器返回了错误（{detail}）",
    ),
    (
        "notsupportedexception",
        "获取到的内容不是有效的播放列表，链接可能已失效（{detail}）",
    ),
    ("filenotfoundexception", "缺少依赖程序（{detail}）"),
    (
        "no such host is known",
        "无法解析域名，请检查网络连接（{detail}）",
    ),
    (
        "connection refused",
        "连接被拒绝，请检查网络连接（{detail}）",
    ),
    (
        "the request was canceled due to the configured httpclient.timeout",
        "连接超时，请检查网络连接（{detail}）",
    ),
    (
        "the ssl connection could not be established",
        "SSL 连接失败，请检查网络连接（{detail}）",
    ),
];

/// 兜底文案（`parseError` 没命中任何 hint 时用）。
pub const UNKNOWN_EXIT_MESSAGE: &str = "进程异常退出（{code}）：{detail}";

/// 兜底文案里 `detail` 为空时的占位（`parseError` 里的 `detail or "N_m3u8DL-RE"`）。
pub const DEFAULT_DETAIL: &str = "N_m3u8DL-RE";

/// 镜像 `app/format.py::BYTE_SCALE`（只取进度行会用到的单位）。
fn scale_for(unit: &str) -> f64 {
    match unit {
        "B" | "Bps" => 1.0,
        "KB" | "KBps" => 1024.0,
        "MB" | "MBps" => 1024.0 * 1024.0,
        "GB" | "GBps" => 1024.0 * 1024.0 * 1024.0,
        _ => 0.0,
    }
}

/// 镜像 `app/format.py::toBytes`：`int(float(value) * BYTE_SCALE[unit])`。
///
/// `int()` 是向零截断，Rust 的 `as i64` 同样向零截断。
pub fn to_bytes(value: &str, unit: &str) -> i64 {
    let parsed: f64 = value.parse().unwrap_or(0.0);
    (parsed * scale_for(unit)) as i64
}

/// 镜像 `parseDiagnostic`：取最后一条 WARN/ERROR 行的正文，其次看
/// `Unhandled exception: <类型>: <正文>`。
pub fn parse_diagnostic(output: &str) -> String {
    let mut last = String::new();
    for captures in LOG_LINE.captures_iter(output) {
        last = captures[2].trim().to_string();
    }
    // Python 是 `cut = last.find(...)` + `if cut > 0`：命中在首位时**不**截断。
    if let Some(cut) = last.find("Unhandled exception:") {
        if cut > 0 {
            last = last[..cut].trim().to_string();
        }
    }
    if !last.is_empty() {
        return last;
    }
    UNHANDLED
        .captures(output)
        .map(|captures| captures[1].trim().to_string())
        .unwrap_or_default()
}

/// `parseError` 的返回值：文案模板 + 模板参数。
///
/// `code` 为 `None` 表示命中了 `ERROR_HINTS`（模板里只有 `{detail}`）；
/// 为 `Some` 表示走了兜底分支（模板里同时有 `{code}` 与 `{detail}`）。
#[derive(Debug, Clone, PartialEq)]
pub struct ErrorSpec {
    pub message: &'static str,
    pub code: Option<i64>,
    pub detail: String,
}

/// 镜像 `parseError`：按 `ERROR_HINTS` 顺序做 lowercase 子串匹配，首个命中即返回。
pub fn classify_error(output: &str, returncode: i64) -> ErrorSpec {
    let lowered = output.to_lowercase();
    for (needle, message) in ERROR_HINTS {
        if lowered.contains(needle) {
            return ErrorSpec {
                message,
                code: None,
                detail: parse_diagnostic(output),
            };
        }
    }
    let detail = parse_diagnostic(output);
    ErrorSpec {
        message: UNKNOWN_EXIT_MESSAGE,
        code: Some(returncode),
        detail: if detail.is_empty() {
            DEFAULT_DETAIL.to_string()
        } else {
            detail
        },
    }
}

/// `_parseOutputLine` 的结果：只有 `Some` 的字段才该被写回。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OutputUpdate {
    /// `line.strip()` 的末尾 1000 个字符。`None` 表示这行 strip 之后是空的，
    /// 调用方应当**直接返回**（Python 在赋值 `lastMessage` 之前就 return 了）。
    pub last_message: Option<String>,
    pub progress: Option<f64>,
    pub speed: Option<i64>,
    pub received_bytes: Option<i64>,
    /// 只在 `totalSize > 0` 时有值（Python 只在这时写回 `task.fileSize`）。
    pub file_size: Option<i64>,
    pub live_elapsed: Option<String>,
    pub live_total: Option<String>,
    pub live_status: Option<String>,
}

/// 取 `text` 的末尾 1000 个**字符**（Python 的 `text[-1000:]` 按字符切）。
fn tail_1000(text: &str) -> String {
    let count = text.chars().count();
    if count <= 1000 {
        return text.to_string();
    }
    text.chars().skip(count - 1000).collect()
}

/// 镜像 `M3U8TaskStep._parseOutputLine`：VOD → 直播进度 → 直播录制，首个命中即返回。
pub fn parse_output_line(line: &str) -> OutputUpdate {
    let text = line.trim();
    if text.is_empty() {
        return OutputUpdate::default();
    }

    let mut update = OutputUpdate {
        last_message: Some(tail_1000(text)),
        ..OutputUpdate::default()
    };

    if let Some(captures) = VOD_PROGRESS.captures_iter(text).last() {
        update.progress = captures[3].parse::<f64>().ok();
        if captures.get(4).is_some() {
            update.received_bytes = Some(to_bytes(&captures[4], &captures[5]));
            let total_size = to_bytes(&captures[6], &captures[7]);
            if total_size > 0 {
                update.file_size = Some(total_size);
            }
        }
        update.speed = Some(to_bytes(&captures[8], &captures[9]));
        return update;
    }

    if let Some(captures) = LIVE_PROGRESS.captures_iter(text).last() {
        update.live_elapsed = Some(captures[1].to_string());
        update.live_total = Some(captures[2].to_string());
        update.live_status = Some(captures[3].to_string());
        update.progress = captures[4].parse::<f64>().ok();
        update.speed = Some(if &captures[5] == "-" {
            0
        } else {
            to_bytes(&captures[6], &captures[7])
        });
        return update;
    }

    if let Some(captures) = LIVE_RECORD.captures_iter(text).last() {
        update.live_elapsed = Some(captures[1].to_string());
        update.live_total = Some(captures[2].to_string());
        update.live_status = Some(captures[3].to_string());
        update.speed = Some(if &captures[4] == "-" {
            0
        } else {
            to_bytes(&captures[5], &captures[6])
        });
    }

    update
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vod(text: &str) -> OutputUpdate {
        parse_output_line(text)
    }

    #[test]
    fn blank_line_has_no_last_message() {
        assert_eq!(parse_output_line("   "), OutputUpdate::default());
        assert_eq!(parse_output_line(""), OutputUpdate::default());
    }

    #[test]
    fn vod_progress_with_sizes() {
        let update =
            vod("Vid 1280x720 | 2000 Kbps ━━━━━━ 6/6857 0.09% 3.12MB/3.48GB1.55MBps01:27:07");
        assert_eq!(update.progress, Some(0.09));
        assert_eq!(update.received_bytes, Some(to_bytes("3.12", "MB")));
        assert_eq!(update.file_size, Some(to_bytes("3.48", "GB")));
        assert_eq!(update.speed, Some(to_bytes("1.55", "MBps")));
    }

    #[test]
    fn vod_progress_takes_the_last_match() {
        let blob = concat!(
            "Vid 1280x720 | 1650 Kbps | 30 ━━━━━━ 3/155 1.94% -0.00Bps00:00:37",
            "Vid 1280x720 | 1650 Kbps | 30 ━━━━━━ 35/155 22.58% 30.99MB/137.26MB28.45MBps00:00:06",
            "Vid 1280x720 | 1650 Kbps | 30 ━━━━━━ 100/155 64.52% 71.82MB/148.43MB40.83MBps00:00:02",
        );
        let update = vod(blob);
        assert_eq!(update.progress, Some(64.52));
        assert_eq!(update.received_bytes, Some(to_bytes("71.82", "MB")));
        assert_eq!(update.file_size, Some(to_bytes("148.43", "MB")));
        assert_eq!(update.speed, Some(to_bytes("40.83", "MBps")));
    }

    #[test]
    fn vod_dash_speed_is_zero() {
        let update = vod("Vid 1920x1080 | 6221 Kbps ━━━━━━ 0/64 0.00% -0.00Bps --:--:--");
        assert_eq!(update.progress, Some(0.0));
        assert_eq!(update.speed, Some(0));
        assert_eq!(update.received_bytes, None);
        assert_eq!(update.file_size, None);
    }

    #[test]
    fn live_progress_and_record_progress() {
        let live = vod("Vid ━━━━━━ 01h05m30s/01h30m00s 3000/4500 Recording 66% 2.10MBps");
        assert_eq!(live.live_elapsed.as_deref(), Some("01h05m30s"));
        assert_eq!(live.live_total.as_deref(), Some("01h30m00s"));
        assert_eq!(live.live_status.as_deref(), Some("Recording"));
        assert_eq!(live.progress, Some(66.0));
        assert_eq!(live.speed, Some(to_bytes("2.10", "MBps")));

        let record = vod("Vid ━━━━━━ 00m10s/00m30s 0.50MB 5/15 Recording -");
        assert_eq!(record.live_status.as_deref(), Some("Recording"));
        assert_eq!(record.speed, Some(0));
        assert_eq!(record.progress, None);
    }

    #[test]
    fn last_message_keeps_the_tail() {
        let filler = "x".repeat(2000);
        let update = vod(&filler);
        assert_eq!(update.last_message.as_deref(), Some(&filler[1000..]));
        assert_eq!(update.progress, None);
    }

    #[test]
    fn diagnostics_come_from_the_last_log_line() {
        let output = concat!(
            "22:17:47.595 INFO : N_m3u8DL-RE (Beta version) 20260628\n",
            "22:17:48.730 WARN : Response status code does not indicate success: 403 (Forbidden). (1/10)\n",
            "22:17:50.235 INFO : Loading URL: http://example.com\n",
            "22:17:51.306 WARN : Response status code does not indicate success: 403 (Forbidden). (2/10)\n",
        );
        assert_eq!(
            parse_diagnostic(output),
            "Response status code does not indicate success: 403 (Forbidden). (2/10)"
        );
        assert_eq!(parse_diagnostic("22:17:47.595 INFO : started\n"), "");
    }

    #[test]
    fn unhandled_exception_is_the_fallback_diagnostic() {
        let output = concat!(
            "17:07:01.414 INFO : N_m3u8DL-RE started\n",
            "Unhandled exception: System.Exception: something went wrong\n",
            "   at SomeNamespace.SomeMethod()\n",
        );
        let spec = classify_error(output, 1);
        assert_eq!(spec.detail, "something went wrong");
        assert_eq!(spec.code, Some(1));
        assert_eq!(spec.message, UNKNOWN_EXIT_MESSAGE);
    }

    #[test]
    fn hints_win_over_the_generic_http_message() {
        let forbidden = classify_error(
            "22:18:14.893 WARN : Response status code does not indicate success: 403 (Forbidden). (10/10)\n",
            1,
        );
        assert_eq!(forbidden.code, None);
        assert!(forbidden.message.contains("服务器拒绝了请求"));

        let empty = classify_error("", 7);
        assert_eq!(empty.detail, DEFAULT_DETAIL);
        assert_eq!(empty.code, Some(7));
    }
}
