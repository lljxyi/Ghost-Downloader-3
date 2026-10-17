//! `gd3_core::task::TaskRecord` → Slint `TaskRow` 映射（Phase 7b）。
//!
//! 文案口径逐分支镜像：
//! - 大小行：`app/view/cards/task_cards.py::toSizeText`（SIZE_FIELD 的
//!   `{None: toSizeText}`——所有状态都走同一函数）。
//! - 状态行：`app/view/dialogs/task_drawer.py::DetailCard._toStatusText`
//!   （RUNNING 拼「速度 · 已收 / 总量 · 剩余时间」，其余按状态出文案）。
//!
//! Python 先验字面量（2026-10-17（date 实取）实测）：
//! - `toSizeText`：completed+fileSize=1048576 → "1.00 MB"；
//!   fileSize=4096/received=512 → "512.00 B/4.00 KB"；fileSize=0 → "512.00 B/--"。
//! - `_toStatusText` running(1024, 512, 4096) →
//!   "1.00 KB/s · 512.00 B / 4.00 KB · 3s"（eta=int((4096-512)/1024)=3）。
//! - 状态字段文案：COMPLETED=已完成 / FAILED=失败 / PAUSED=已暂停 /
//!   WAITING=等待中（`self.tr` 原文）。
//!
//! 分层注意：`Task.isSeeding` 是运行时属性、`tasks.jsonl` 记录里没有，
//! 所以 COMPLETED 的「做种中」由调用方经 `is_seeding` 传入（与
//! 「兜底不在核心层」同一分层原则）。

use crate::TaskRow;
use crate::format::{to_readable_size, to_readable_time};
use gd3_core::task::{TaskRecord, TaskStatus};

/// 镜像 `toSizeText`：completed 且 fileSize>0 → 纯大小；否则「已收/总量」，
/// fileSize<=0 时总量显示 "--"。
///
/// 抽成「status + fileSize + received」三元组是为了让 `card::info_fields`
/// 复用同一份口径——Python 侧卡片信息行（`SIZE_FIELD` 的 `{None: toSizeText}`）
/// 与任务抽屉的大小行本来就是同一个函数。
pub fn size_text(status: TaskStatus, file_size: i64, received: i64) -> Option<String> {
    if status == TaskStatus::Completed {
        return if file_size > 0 {
            Some(to_readable_size(file_size as f64))
        } else {
            None
        };
    }
    if file_size > 0 {
        return Some(format!(
            "{}/{}",
            to_readable_size(received as f64),
            to_readable_size(file_size as f64)
        ));
    }
    Some(format!("{}/--", to_readable_size(received as f64)))
}

/// `TaskRecord` 版包装（既有调用点与测试沿用）。
pub fn to_size_text(record: &TaskRecord, received: i64) -> Option<String> {
    size_text(record.status, record.file_size, received)
}

/// 镜像 `_toStatusText`：running → 「速度/s · 已收 / 总量 · 时间」拼接；
/// completed → 做种中/已完成；failed → 失败；paused → 已暂停；缺省 → 等待中。
pub fn to_status_text(record: &TaskRecord, speed: i64, received: i64, is_seeding: bool) -> String {
    if record.status == TaskStatus::Running {
        let mut parts = vec![format!("{}/s", to_readable_size(speed as f64))];
        if record.file_size > 0 {
            parts.push(format!(
                "{} / {}",
                to_readable_size(received as f64),
                to_readable_size(record.file_size as f64)
            ));
            if speed > 0 {
                // Python int() 截断（true division 后取整向零）。
                let eta = ((record.file_size - received) as f64 / speed as f64) as i64;
                parts.push(to_readable_time(eta));
            }
        }
        return parts.join(" · ");
    }
    if record.status == TaskStatus::Completed {
        return if is_seeding {
            "做种中".into()
        } else {
            "已完成".into()
        };
    }
    if record.status == TaskStatus::Failed {
        return "失败".into();
    }
    if record.status == TaskStatus::Paused {
        return "已暂停".into();
    }
    "等待中".into()
}

/// 用记录自身的 current_snapshot 装配一行（非做种）。
pub fn to_task_row(record: &TaskRecord) -> TaskRow {
    to_task_row_with(record, false)
}

/// 同上，但由调用方声明做种态（COMPLETED 时影响文案）。
pub fn to_task_row_with(record: &TaskRecord, is_seeding: bool) -> TaskRow {
    // 快照 (progress 0-100, speed, received)；progress 供进度条直通。
    let (progress, speed, received) = record.current_snapshot();
    TaskRow {
        taskId: record.task_id.clone().into(),
        name: record.name.clone().into(),
        statusText: to_status_text(record, speed, received, is_seeding).into(),
        // 渲染进度走 progressText 文案；数值 progress 另供进度条绑定。
        progressText: to_size_text(record, received).unwrap_or_default().into(),
        // 结构化字段：磁盘同名枚举字面量 + 快照百分比（0-100）。
        status: status_literal(record.status).into(),
        // slint float 是 f32（E0308 实证），进度直通时收窄；0-100 量级
        // 无精度损失顾虑。
        progress: progress as f32,
    }
}

/// 磁盘持久化口径的枚举名（Python TaskStatus.name，serde UPPERCASE 同款）。
pub fn status_literal(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Waiting => "WAITING",
        TaskStatus::Running => "RUNNING",
        TaskStatus::Paused => "PAUSED",
        TaskStatus::Completed => "COMPLETED",
        TaskStatus::Failed => "FAILED",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 以磁盘同名 JSON 形态构造（serde 全默认值路径），锁 serde 字段名。
    fn record(json: serde_json::Value) -> TaskRecord {
        serde_json::from_value(json).expect("TaskRecord 反序列化失败")
    }

    #[test]
    fn size_text_matches_to_size_text_prior() {
        let completed = record(json! ({
            "name": "a.bin", "url": "https://example/1", "packId": "pack",
            "taskId": "tsk_1", "status": "COMPLETED", "fileSize": 1048576,
        }));
        assert_eq!(to_size_text(&completed, 0), Some("1.00 MB".into()));

        let running = record(json! ({
            "name": "a.bin", "url": "https://example/1", "packId": "pack",
            "taskId": "tsk_1", "status": "RUNNING", "fileSize": 4096,
        }));
        assert_eq!(to_size_text(&running, 512), Some("512.00 B/4.00 KB".into()));

        let nosize = record(json! ({
            "name": "a.bin", "url": "https://example/1", "packId": "pack",
            "taskId": "tsk_1", "fileSize": 0,
        }));
        assert_eq!(to_size_text(&nosize, 512), Some("512.00 B/--".into()));
    }

    #[test]
    fn status_text_matches_status_prior() {
        let completed = record(json! ({
            "name": "a.bin", "url": "https://example/1", "packId": "pack",
            "taskId": "tsk_1", "status": "COMPLETED", "fileSize": 0,
        }));
        assert_eq!(to_status_text(&completed, 0, 0, false), "已完成");
        assert_eq!(to_status_text(&completed, 0, 0, true), "做种中");

        for (status, want) in [
            ("FAILED", "失败"),
            ("PAUSED", "已暂停"),
            ("WAITING", "等待中"),
        ] {
            let r = record(json! ({
                "name": "a.bin", "url": "https://example/1", "packId": "pack",
                "taskId": "tsk_1", "status": status, "fileSize": 0,
            }));
            assert_eq!(to_status_text(&r, 0, 0, false), want);
        }
    }

    /// Python 先验：running(1024, 512, 4096) →
    /// "1.00 KB/s · 512.00 B / 4.00 KB · 3s"；无 size 且 0 速 → 纯速度段。
    #[test]
    fn running_status_text_matches_python_prior() {
        let running = record(json! ({
            "name": "a.bin", "url": "https://example/1", "packId": "pack",
            "taskId": "tsk_1", "status": "RUNNING", "fileSize": 4096,
        }));
        assert_eq!(
            to_status_text(&running, 1024, 512, false),
            "1.00 KB/s · 512.00 B / 4.00 KB · 3s"
        );

        let running_nosize = record(json! ({
            "name": "a.bin", "url": "https://example/1", "packId": "pack",
            "taskId": "tsk_1", "status": "RUNNING", "fileSize": 0,
        }));
        assert_eq!(to_status_text(&running_nosize, 0, 512, false), "0.00 B/s");
    }

    /// 整行装配：快照取选中 step 之和（gd3-core current_snapshot），文案
    /// 逐位对 Python 先验。
    #[test]
    fn task_row_assembles_from_record_snapshot() {
        let r = record(json! ({
            "name": "视频.mp4", "url": "https://example/1", "packId": "m3u8_pack",
            "taskId": "tsk_assemble", "status": "RUNNING", "fileSize": 4096,
            "steps": [
                {"stepIndex": 0, "progress": 30, "speed": 512, "receivedBytes": 384},
                {"stepIndex": 1, "progress": 70, "speed": 512, "receivedBytes": 128},
            ],
        }));
        let row = to_task_row(&r);
        assert_eq!(row.taskId, "tsk_assemble");
        assert_eq!(row.name, "视频.mp4");
        // 快照 received=384+128=512、speed=512+512=1024 → 与手工先验一致。
        assert_eq!(row.statusText, "1.00 KB/s · 512.00 B / 4.00 KB · 3s");
        assert_eq!(row.progressText, "512.00 B/4.00 KB");
        // progress = (30+70)/2 = 50（0-100）；status 为枚举字面量。
        assert_eq!(row.progress, 50.0);
        assert_eq!(row.status, "RUNNING");
    }

    /// status_literal：全枚举覆盖（磁盘 TaskStatus.name 同款）。
    #[test]
    fn status_literal_covers_all_statuses() {
        for (literal, want) in [
            ("WAITING", "WAITING"),
            ("RUNNING", "RUNNING"),
            ("PAUSED", "PAUSED"),
            ("COMPLETED", "COMPLETED"),
            ("FAILED", "FAILED"),
        ] {
            let r = record(json! ({
                "name": "a.bin", "url": "https://example/1", "packId": "pack",
                "taskId": "tsk_lit", "status": literal, "fileSize": 0,
            }));
            assert_eq!(status_literal(r.status), want);
        }
    }
}
