//! 展示层格式化助手：算法级镜像 `app/format.py` 的 `toReadableSize` /
//! `toReadableTime`（Phase 7 UI 伴生）。
//!
//! Python 先验字面量（2026-10-17（date 实取）实测 `toReadableSize(0/1/512/1024/
//! 1048576/2**40/1536)` 与 `toReadableTime(0/59/60/125/3900)`）：
//! ≡ "0.00 B"/"1.00 B"/"512.00 B"/"1.00 KB"/"1.00 MB"/"1.00 TB"/"1.50 KB"/
//!   "0s"/"59s"/"1m0s"/"2m5s"/"1h5m0s"。

/// 镜像 `app/format.py::toReadableSize`：B/KB/MB/GB 逐级；<1024 即归位；
/// 否则继续除以 1024，GB 后仍 ≥1024 则 "x.xx TB" 兜底。
pub fn to_readable_size(mut size: f64) -> String {
    for unit in ["B", "KB", "MB", "GB"] {
        if size < 1024.0 {
            return format!("{size:.2} {unit}");
        }
        size /= 1024.0;
    }
    format!("{size:.2} TB")
}

/// 镜像 `app/format.py::toReadableTime`：<60 → "Ns"；<3600 → "NmNs"；
/// 其余 → "HhMmSs"（Python `//`/`%` 整除语义，非负入参）。
pub fn to_readable_time(seconds: i64) -> String {
    if seconds < 60 {
        return format!("{seconds}s");
    } else if seconds < 3600 {
        return format!("{}m{}s", seconds / 60, seconds % 60);
    }
    let remaining = seconds % 3600;
    format!("{}h{}m{}s", seconds / 3600, remaining / 60, remaining % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Python 先验（见模块头）逐条锁定。
    #[test]
    fn readable_size_matches_python() {
        assert_eq!(to_readable_size(0.0), "0.00 B");
        assert_eq!(to_readable_size(1.0), "1.00 B");
        assert_eq!(to_readable_size(512.0), "512.00 B");
        assert_eq!(to_readable_size(1024.0), "1.00 KB");
        assert_eq!(to_readable_size(1048576.0), "1.00 MB");
        assert_eq!(to_readable_size(1_099_511_627_776.0), "1.00 TB");
        assert_eq!(to_readable_size(1536.0), "1.50 KB");
        assert_eq!(to_readable_size(1.5 * 1024.0 * 1024.0 * 1024.0), "1.50 GB");
    }

    #[test]
    fn readable_time_matches_python() {
        assert_eq!(to_readable_time(0), "0s");
        assert_eq!(to_readable_time(59), "59s");
        assert_eq!(to_readable_time(60), "1m0s");
        assert_eq!(to_readable_time(125), "2m5s");
        assert_eq!(to_readable_time(3900), "1h5m0s");
    }
}
