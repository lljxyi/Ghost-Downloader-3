// Ghost-Downloader-3 Slint UI 的 Rust 侧入口。
//
// Phase 7a：只装最小窗口骨架 + 无头实例化冒烟（cargo test 可在
// 无显示器的构建机上跑，走 software renderer + 自制 Platform）。
// 逐页从 Qt 迁过来时在 ui/ 增加组件、在本 crate 挂回调。
//
// slint 1.18.1 API 实证锚点（registry i-slint-core-1.18.1）：
// - platform.rs: `set_platform(platform: Box<dyn Platform + 'static>) -> Result<(), SetPlatformError>`
//   （1.18 没有 `is_using_platform_set`；`SetPlatformError::AlreadySet` 即「已装好」信号）
// - Platform 唯一必需方法：`create_window_adapter -> Result<Rc<dyn WindowAdapter>, PlatformError>`
// - MinimalSoftwareWindow::new(repaint_buffer_type: RepaintBufferType) -> Rc<Self>（自身已包 Rc）

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PlatformError, WindowAdapter};
use std::rc::Rc;

mod format;
pub mod row;

slint::include_modules!();

/// 无头平台：不依赖任何显示服务（测试/裸构建机都跑得起来）。
struct HeadlessPlatform;

impl Platform for HeadlessPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
    }
}

/// 无头实例化 AppWindow（进程内 set_platform 只成功一次，重复=AlreadySet）。
pub fn create_headless() -> AppWindow {
    // AlreadySet 说明平台已装好（比如同进程第二个实例），直接复用即可。
    let _ = slint::platform::set_platform(Box::new(HeadlessPlatform));
    let window = AppWindow::new().expect("AppWindow 实例化失败");
    window.show().expect("show 失败");
    window
}

/// Rust 侧推送任务行到 UI（Phase 7b 契约）。
///
/// 行文案（statusText/progressText）由调用方负责；这里只做 model 装配。
/// （slint 1.18 生成的模型属性 setter 是无返回值的，见 target build 产物实证。）
pub fn set_task_rows(window: &AppWindow, rows: Vec<TaskRow>) {
    let model = slint::ModelRc::new(slint::VecModel::from(rows));
    window.set_tasks(model);
}

/// 推流路径：`Vec<TaskRecord>` → TaskRow 文案 → model 装配。
///
/// `is_seeding` 由调用方给定（isSeeding 是运行时属性、不在记录里），
/// 这里只做「快照 → 文案 → 行」的桥。
pub fn push_task_records(
    window: &AppWindow,
    records: &[gd3_core::task::TaskRecord],
    is_seeding: impl Fn(&gd3_core::task::TaskRecord) -> bool,
) {
    let rows: Vec<TaskRow> = records
        .iter()
        .map(|record| row::to_task_row_with(record, is_seeding(record)))
        .collect();
    set_task_rows(window, rows);
}

#[cfg(test)]
mod smoke {
    use slint::Model;
    /// 无头实例化：组件树的属性可读写、状态文本可镜像给 Rust 侧。
    #[test]
    fn appwindow_instantiates_headless_and_rust_side_sets_status() {
        let window = crate::create_headless();
        assert_eq!(window.get_statusText(), "Ghost-Downloader-3 (Slint)");
        window.set_statusText("headless smoke".into());
        assert_eq!(window.get_statusText(), "headless smoke");
    }

    /// 任务列表行模型：Rust 侧推送 → Slint model 逐行可读。
    #[test]
    fn task_page_rows_round_trip_headless() {
        let window = crate::create_headless();
        crate::set_task_rows(
            &window,
            vec![
                crate::TaskRow {
                    taskId: "tsk_1".into(),
                    name: "a.bin".into(),
                    statusText: "等待中".into(),
                    progressText: "0.00 B / --".into(),
                },
                crate::TaskRow {
                    taskId: "tsk_2".into(),
                    name: "b.iso".into(),
                    statusText: "正在下载".into(),
                    progressText: "1.00 MB / 4.00 MB".into(),
                },
            ],
        );
        let model = window.get_tasks();
        assert_eq!(model.row_count(), 2);
        let first = model.row_data(0).expect("首行缺失");
        assert_eq!(first.taskId, "tsk_1");
        assert_eq!(first.name, "a.bin");
        assert_eq!(first.statusText, "等待中");
        let second = model.row_data(1).expect("次行缺失");
        assert_eq!(second.taskId, "tsk_2");
        assert_eq!(second.statusText, "正在下载");
    }

    /// 端到端推流：TaskRecord（磁盘同态 JSON 构造）→ push_task_records
    /// → Slint model 逐行核对文案。
    #[test]
    fn push_task_records_shapes_records_into_rows_headless() {
        use gd3_core::task::TaskRecord;
        use serde_json::json;

        let records: Vec<TaskRecord> = serde_json::from_value(json!([
            {
                "name": "a.bin", "url": "https://example/1", "packId": "pack",
                "taskId": "tsk_a", "status": "RUNNING", "fileSize": 4096,
                "steps": [
                    {"stepIndex": 0, "progress": 30, "speed": 512, "receivedBytes": 384},
                    {"stepIndex": 1, "progress": 70, "speed": 512, "receivedBytes": 128},
                ],
            },
            {
                "name": "b.iso", "url": "https://example/2", "packId": "pack",
                "taskId": "tsk_b", "status": "COMPLETED", "fileSize": 1048576,
            },
        ]))
        .expect("TaskRecord 数组反序列化失败");
        let window = crate::create_headless();
        crate::push_task_records(&window, &records, |record| record.task_id == "tsk_b");
        let model = window.get_tasks();
        assert_eq!(model.row_count(), 2);
        let first = model.row_data(0).expect("首行缺失");
        assert_eq!(first.taskId, "tsk_a");
        assert_eq!(first.statusText, "1.00 KB/s · 512.00 B / 4.00 KB · 3s");
        assert_eq!(first.progressText, "512.00 B/4.00 KB");
        let second = model.row_data(1).expect("次行缺失");
        assert_eq!(second.statusText, "做种中");
        assert_eq!(second.progressText, "1.00 MB");
    }
}
