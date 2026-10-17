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

pub mod filter;
mod format;
pub mod row;
pub mod sort;

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

/// 筛选后的推流：按 `filter::TaskFilter` 过滤 records 再成行推送
/// （顺路接通 `set_searchText`/`set_filterMode` 的 UI 态）。
pub fn refresh_task_rows(
    window: &AppWindow,
    records: &[gd3_core::task::TaskRecord],
    filter: filter::TaskFilter<'_>,
    sort_field: sort::SortField,
    sort_ascending: bool,
    waiting_order: &[String],
    is_seeding: impl Fn(&gd3_core::task::TaskRecord) -> bool,
) {
    // 筛选（保序）→ 排序（镜像 task_page.py::_refreshList 末段）→ 成行。
    // waitingOrder 由调用方给（taskService.waitingOrder()）。
    let mut refs = filter::filtered(records, filter);
    sort::sort_refs(&mut refs, sort_field, sort_ascending, waiting_order);
    let rows: Vec<TaskRow> = refs
        .iter()
        .map(|record| row::to_task_row_with(record, is_seeding(record)))
        .collect();
    set_task_rows(window, rows);
}

/// 分类筛选下拉（ComboBox）的 [string] 模型装配（filter::menu_names
/// 已含「全部分类」表头；current-index 由调用方回写）。
///
/// slint 生成名保留 camelCase：set_categoryNames / get_categoryIndex。
pub fn set_category_names(window: &AppWindow, names: Vec<String>) {
    let names: Vec<slint::SharedString> =
        names.into_iter().map(slint::SharedString::from).collect();
    let model = slint::ModelRc::new(slint::VecModel::from(names));
    window.set_categoryNames(model);
}

/// Rust 侧动作集：Slint 回调 → 调用方桥接（task_service/queue 等）。
///
/// 分层：Slint 只发 taskId/路由字面量；真正的暂停/删除/筛选语义都在
/// 调用方，由 install_task_actions 注册到 AppWindow。
pub struct TaskActions {
    pub search: Box<dyn Fn(&str)>,
    pub filter_mode: Box<dyn Fn(&str)>,
    pub category_selected: Box<dyn Fn(i32)>,
    pub sort_field: Box<dyn Fn(i32)>,
    pub sort_order: Box<dyn Fn(i32)>,
    pub pause: Box<dyn Fn(&str)>,
    pub delete: Box<dyn Fn(&str)>,
}

/// 把动作集注册到 AppWindow 的五个回调上（on_searchChanged 等生成名
/// 保留 slint 原命名，见 build 产物 appwindow.rs:9572-9608）。
pub fn install_task_actions(window: &AppWindow, actions: std::rc::Rc<TaskActions>) {
    let search = actions.clone();
    window.on_searchChanged(move |text| (search.search)(&text));
    let filter_mode = actions.clone();
    window.on_filterModeChanged(move |route| (filter_mode.filter_mode)(&route));
    let category_selected = actions.clone();
    window.on_categoryFilterChanged(move |index| (category_selected.category_selected)(index));
    let sort_field = actions.clone();
    window.on_sortFieldChanged(move |index| (sort_field.sort_field)(index));
    let sort_order = actions.clone();
    window.on_sortOrderChanged(move |index| (sort_order.sort_order)(index));
    let pause = actions.clone();
    window.on_taskPause(move |task_id| (pause.pause)(&task_id));
    let delete = actions.clone();
    window.on_taskDelete(move |task_id| (delete.delete)(&task_id));
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

    /// 筛选推流：refresh_task_rows 过滤后仅剩选中状态的行（语义由
    /// filter.rs 纯变换测试锁定，这里锁「过滤 → 成行 → 装配」通路）。
    #[test]
    fn refresh_task_rows_applies_filter_headless() {
        use gd3_core::task::TaskRecord;

        use serde_json::json;
        let records: Vec<TaskRecord> = serde_json::from_value(json!([
            {
                "name": "run.bin", "url": "https://e/1", "packId": "p",
                "taskId": "t1", "status": "RUNNING", "fileSize": 2048,
                "steps": [{"stepIndex": 0, "progress": 10, "speed": 0, "receivedBytes": 8}],
            },
            {
                "name": "done.iso", "url": "https://e/2", "packId": "p",
                "taskId": "t2", "status": "COMPLETED", "fileSize": 1024,
            },
        ]))
        .expect("反序列化失败");
        let window = crate::create_headless();

        // ACTIVE：只剩 RUNNING 行。
        crate::refresh_task_rows(
            &window,
            &records,
            crate::filter::TaskFilter {
                mode: crate::filter::FilterMode::Active,
                category: "",
                search: "",
            },
            crate::sort::SortField::CreatedAt,
            false,
            &[],
            |_| false,
        );
        let model = window.get_tasks();
        assert_eq!(model.row_count(), 1);
        assert_eq!(model.row_data(0).expect("缺失").name, "run.bin");

        // 搜索不命中 → 空列表（UI 侧空态文案在后续步接）。
        crate::refresh_task_rows(
            &window,
            &records,
            crate::filter::TaskFilter {
                mode: crate::filter::FilterMode::All,
                category: "",
                search: "zzz",
            },
            crate::sort::SortField::CreatedAt,
            false,
            &[],
            |_| false,
        );
        assert_eq!(window.get_tasks().row_count(), 0);
    }

    /// 回调注册冒烟：install_task_actions 把四个动作挂上不 panic
    /// （生成名 on_searchChanged 等保留 slint 命名）。
    #[test]
    fn install_task_actions_registers_handlers_headless() {
        use std::cell::RefCell;
        use std::rc::Rc;

        let window = crate::create_headless();
        let hits: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let actions = Rc::new(crate::TaskActions {
            search: Box::new({
                let h = hits.clone();
                move |s| h.borrow_mut().push(s.into())
            }),
            filter_mode: Box::new(|_| {}),
            category_selected: Box::new(|_| {}),
            sort_field: Box::new(|_| {}),
            sort_order: Box::new(|_| {}),
            pause: Box::new(|_| {}),
            delete: Box::new(|_| {}),
        });
        crate::install_task_actions(&window, actions);
        // 无头环境无法模拟点按；注册本身即通路（真实触发屏上联调）。
        assert!(hits.borrow().is_empty());
    }

    /// 分类筛选 UI：menu_names 装配 [string] 模型 + categoryIndex
    /// 属性可回写（选中态由调用方管理，ComboBox 绑定印证连通）。
    #[test]
    fn set_category_names_and_index_round_trip_headless() {
        use crate::filter::{CategoryRef, menu_names};

        let categories = vec![
            CategoryRef {
                category_id: "cat_video".into(),
                name: "视频".into(),
            },
            CategoryRef {
                category_id: "cat_audio".into(),
                name: "音频".into(),
            },
        ];
        let names = menu_names(&categories);
        assert_eq!(names.first().map(String::as_str), Some("全部分类"));
        let window = crate::create_headless();
        crate::set_category_names(&window, names);
        let model = window.get_categoryNames();
        assert_eq!(model.row_count(), 3);
        assert_eq!(model.row_data(1).as_deref(), Some("视频"));
        window.set_categoryIndex(2);
        assert_eq!(window.get_categoryIndex(), 2);
    }

    /// 排序 ComboBox 索引：缺省字段 0（按添加时间）、顺序 1（倒序）
    /// round-trip；字段索引 → SortField 的映射在 sort::from_menu_index。
    #[test]
    fn sort_combo_indexes_round_trip_headless() {
        let window = crate::create_headless();
        assert_eq!(window.get_sortFieldIndex(), 0);
        assert_eq!(window.get_sortOrderIndex(), 1);
        window.set_sortFieldIndex(3);
        window.set_sortOrderIndex(0);
        assert_eq!(window.get_sortFieldIndex(), 3);
        assert_eq!(window.get_sortOrderIndex(), 0);
    }

    /// refresh 串接排序：筛选后按 Name 倒序成行（五个排序分支的纯变换
    /// 语义由 sort.rs 测试锁定，这里锁「筛选 → 排序 → 装配」链路）。
    #[test]
    fn refresh_task_rows_applies_sort_headless() {
        use gd3_core::task::TaskRecord;
        use serde_json::json;

        let records: Vec<TaskRecord> = serde_json::from_value(json!([
            {
                "name": "b.iso", "url": "https://e/1", "packId": "p",
                "taskId": "t1", "status": "WAITING", "fileSize": 1024,
                "createdAt": 300,
            },
            {
                "name": "a.bin", "url": "https://e/2", "packId": "p",
                "taskId": "t2", "status": "RUNNING", "fileSize": 2048,
                "createdAt": 100,
                "steps": [
                    {"stepIndex": 0, "progress": 10, "speed": 0, "receivedBytes": 8},
                ],
            },
            {
                "name": "c.m3u8", "url": "https://e/3", "packId": "p",
                "taskId": "t3", "status": "PAUSED", "fileSize": 4096,
                "createdAt": 200,
            },
        ]))
        .expect("反序列化失败");
        let window = crate::create_headless();
        crate::refresh_task_rows(
            &window,
            &records,
            crate::filter::TaskFilter {
                mode: crate::filter::FilterMode::All,
                category: "",
                search: "",
            },
            crate::sort::SortField::Name,
            false,
            &[],
            |_| false,
        );
        let model = window.get_tasks();
        assert_eq!(model.row_count(), 3);
        let names: Vec<String> = (0..model.row_count())
            .filter_map(|index| model.row_data(index).map(|row| row.name.to_string()))
            .collect();
        assert_eq!(names, ["c.m3u8", "b.iso", "a.bin"]);
    }
}
