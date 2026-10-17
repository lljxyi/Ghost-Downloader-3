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
//! 第 51 步起：`to_task_row` 不再自己拼文案，改为**直接消费 `card.rs` 派生层**
//! （`info_fields` / `status_line` / `progress_state` / 四个 `*_button`）。
//! 本模块只剩「大小/状态文案」两个共用原语（`size_text` 被 `card::info_fields`
//! 复用，`to_status_text` 留给任务抽屉的 `DetailCard` 口径）。
//!
//! 分层注意：`Task.isSeeding` 是运行时属性、`tasks.jsonl` 记录里没有，
//! 连同 `canPause`/`canSeed`/`hasOutputFile`/`outputPath` 存在性/完成时间/
//! `lastError` 一起由调用方经 `card::CardRuntime` 传入（与「兜底不在核心层」
//! 同一分层原则）。

use crate::TaskRow;
use crate::card::{
    CardRuntime, CardTask, checksum_button, info_fields, open_file_button, progress_state,
    select_files_button, status_line, toggle_button,
};
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

/// 记录 + 运行时事实 → Slint `TaskRow`（第 51 步：文案全部取自 `card.rs`）。
///
/// 逐段镜像 Qt 卡片：
/// - 信息行三段（speed / eta / size）取 `card::info_fields`，`None` 即隐藏；
/// - 状态行取 `card::status_line`，`None` 即 `statusLabel.hide()`；
/// - 进度条取 `card::progress_state`，可见性按 `ProgressVisibility::visible()`
///   兜底（`Keep` 视为可见）；
/// - 四个按钮取 `toggle_button` / `select_files_button` / `checksum_button`
///   / `open_file_button`，`icon` 已是「生效图标」（`spec.icon` 回落后的值）。
///
/// `TaskRow` 里没有的字段（`openFolder`/`delete` 恒可见）由 Slint 侧写死——
/// Python 侧它们的 `states is None`，`_refreshButtons` 直接 `continue`。
pub fn to_task_row(record: &TaskRecord, runtime: &CardRuntime) -> TaskRow {
    let task = CardTask::from_record(record, runtime);
    let fields = info_fields(&task);
    let field = |name: &str| {
        fields
            .iter()
            .find(|f| f.name == name)
            .and_then(|f| f.text.clone())
    };
    let (speed_text, speed_visible) = split_text(field("speed"));
    let (eta_text, eta_visible) = split_text(field("eta"));
    let (size_text, size_visible) = split_text(field("size"));
    let (line_text, line_visible, line_warn) = match status_line(&task) {
        Some(line) => (line.text.into(), true, line.warn),
        None => (slint::SharedString::default(), false, false),
    };
    let progress = progress_state(&task);
    let toggle = toggle_button(&task);
    let select_files = select_files_button(&task);
    let checksum = checksum_button(&task);
    let open_file = open_file_button(&task);
    TaskRow {
        taskId: record.task_id.clone().into(),
        name: record.name.clone().into(),
        status: status_literal(record.status).into(),
        speedText: speed_text,
        speedVisible: speed_visible,
        etaText: eta_text,
        etaVisible: eta_visible,
        sizeText: size_text,
        sizeVisible: size_visible,
        statusLine: line_text,
        statusLineVisible: line_visible,
        statusWarn: line_warn,
        // Python `setValue(int(progress))`：f64 → i32 向零截断。
        progressValue: progress.value as i32,
        progressIndeterminate: progress.indeterminate,
        progressVisible: progress.visibility.visible(),
        progressError: progress.error,
        toggleIcon: toggle.icon.into(),
        toggleEnabled: toggle.enabled,
        selectFilesVisible: select_files.visible,
        checksumVisible: checksum.visible,
        checksumEnabled: checksum.enabled,
        openFileEnabled: open_file.enabled,
    }
}

/// 信息行一段的「文本 + 可见性」拆分：`None` → 空串 + 隐藏。
fn split_text(field: Option<String>) -> (slint::SharedString, bool) {
    match field {
        Some(text) => (text.into(), true),
        None => (slint::SharedString::default(), false),
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

    /// 整行装配：快照取选中 step 之和（gd3-core current_snapshot），各段文案
    /// 逐位对 Python 先验（第 51 步起全部经 `card.rs` 派生层）。
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
        let row = to_task_row(&r, &CardRuntime::default());
        assert_eq!(row.taskId, "tsk_assemble");
        assert_eq!(row.name, "视频.mp4");
        assert_eq!(row.status, "RUNNING");
        // 信息行三段：快照 received=384+128=512、speed=512+512=1024。
        assert_eq!(row.speedText, "1.00 KB/s");
        assert!(row.speedVisible);
        assert_eq!(row.etaText, "3s");
        assert!(row.etaVisible);
        assert_eq!(row.sizeText, "512.00 B/4.00 KB");
        assert!(row.sizeVisible);
        // RUNNING 状态行整段隐藏（`statusLabel.hide()`）。
        assert_eq!(row.statusLine, "");
        assert!(!row.statusLineVisible);
        assert!(!row.statusWarn);
        // progress = (30+70)/2 = 50；RUNNING 才 show()。
        assert_eq!(row.progressValue, 50);
        assert!(!row.progressIndeterminate);
        assert!(row.progressVisible);
        assert!(!row.progressError);
        // 按钮：RUNNING → PAUSE；多文件才显示「选择文件」；校验/打开文件未就绪。
        assert_eq!(row.toggleIcon, "PAUSE");
        assert!(row.toggleEnabled);
        assert!(!row.selectFilesVisible);
        assert!(row.checksumVisible);
        assert!(!row.checksumEnabled);
        assert!(!row.openFileEnabled);
    }

    /// COMPLETED 分支：状态行文案 / 告警色 / 进度条隐藏 / 按钮可用性。
    #[test]
    fn completed_row_uses_card_status_line_and_hides_progress() {
        let r = record(json! ({
            "name": "b.iso", "url": "https://example/2", "packId": "pack",
            "taskId": "tsk_done", "status": "COMPLETED", "fileSize": 1048576,
        }));
        let runtime = CardRuntime {
            completed_at_text: Some("2026-10-17 12:00:00".into()),
            ..CardRuntime::default()
        };
        let row = to_task_row(&r, &runtime);
        // 信息行：COMPLETED 只剩大小段（speed/eta 的 formats 只有 RUNNING 键）。
        assert!(!row.speedVisible);
        assert!(!row.etaVisible);
        assert_eq!(row.sizeText, "1.00 MB");
        assert!(row.sizeVisible);
        // 状态行「完成于 …」，非告警色。
        assert_eq!(row.statusLine, "完成于 2026-10-17 12:00:00");
        assert!(row.statusLineVisible);
        assert!(!row.statusWarn);
        // COMPLETED 才 hide() 进度条。
        assert!(!row.progressVisible);
        // 按钮：canSeed=False → PLAY 但禁用；文件在 → 可打开/可校验。
        assert_eq!(row.toggleIcon, "PLAY");
        assert!(!row.toggleEnabled);
        assert!(row.checksumVisible);
        assert!(row.checksumEnabled);
        assert!(row.openFileEnabled);

        // 输出文件不存在 → 「文件不存在」+ 告警色（QColor(200,160,80)）。
        let missing = CardRuntime {
            output_exists: false,
            ..CardRuntime::default()
        };
        let row = to_task_row(&r, &missing);
        assert_eq!(row.statusLine, "文件不存在");
        assert!(row.statusWarn);
        assert!(!row.checksumEnabled);
        assert!(!row.openFileEnabled);
    }

    /// 多文件任务：WAITING/COMPLETED 的状态行改成「已选/总数 个文件」，
    /// 且「选择文件」按钮出现、校验按钮隐藏（`MultiFileTaskCard` 覆盖）。
    #[test]
    fn multi_file_row_overrides_status_line_with_file_count() {
        let r = record(json! ({
            "name": "合集", "url": "https://example/3", "packId": "pack",
            "taskId": "tsk_multi", "status": "COMPLETED", "fileSize": 2048,
            "files": [
                {"index": 0, "relativePath": "a.bin", "size": 1024, "selected": true},
                {"index": 1, "relativePath": "b.bin", "size": 1024, "selected": false},
                {"index": 2, "relativePath": "c.bin", "size": 0, "selected": true},
            ],
        }));
        let row = to_task_row(&r, &CardRuntime::default());
        assert_eq!(row.statusLine, "2/3 个文件");
        assert!(row.statusLineVisible);
        assert!(row.selectFilesVisible);
        assert!(!row.checksumVisible);
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
