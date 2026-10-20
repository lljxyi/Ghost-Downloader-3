//! 外壳动作的**宿主落点**（第 140 步）：把 `lib.rs::ShellActions`（第 57 步只给了
//! 空壳：`nav_item` / `search` / `info_bar_*` / `dialog_*` / 拖放）真正接到引擎、
//! 草稿对话框、关窗链和页面上。
//!
//! ## 接线（对照 `app/view/windows/main_window.py`）
//!
//! | 动作 | Python | 这里 |
//! |---|---|---|
//! | `nav_item("addTaskButton")` | `onClick=lambda: self.addUrls([])` → `showMask()` | [`DraftUi::open`] |
//! | `nav_item(PAGE_*)` | `_showPage` → `switchTo` + `_updateSearchTarget` | [`crate::switch_page`] + 重推任务行 |
//! | `search(text)` | `_onSearchTextChanged` → `page.setSearchText` | 任务页：`filter::TaskFilter` → `refresh_task_rows`（见差异 1） |
//! | `info_bar_button(i)` | 每个按钮自己的 `clicked.connect(lambda: …)` | 宿主登记的动作表（[`ShellHost::show_info_bar`]） |
//! | `info_bar_closed()` | `InfoBar.close()` | [`crate::hide_info_bar`] + 清表 |
//! | `dialog_button(i)` | `dialog.exec()` 的返回值（按钮身份） | 先问 [`crate::close_request`] 的关窗链，再问宿主登记的动作表 |
//! | `dialog_checkbox(on)` | `checkbox.isChecked()` | [`CloseHooks::set_remember`]（当前只有关窗框有复选框） |
//! | `can_drop` / `drop` | `dragEnterEvent` / `dropEvent` | [`crate::window::drop_decision`] / `drop_urls` → `DraftUi::open_with_urls` |
//!
//! ## 与 Python 的已知差异
//!
//! 1. **设置页检索未接线**：Python 的 `SettingPage.setSearchText` 会折叠 / 隐藏不匹配
//!    的设置卡片，而 `ui/settings_page.slint` 的卡片是一整块静态布局、没有逐卡可见性
//!    属性。这里只打**一次** stderr 说明（`ShellHost::search`），搜索框本身照常可输入。
//!    任务页检索是**真接**的：镜像 `task_page._refreshList` 的「先过滤、再排序、后成行」，
//!    过滤条件读窗口上的 `filterMode` / `categoryIndex` / `sortFieldIndex` /
//!    `sortOrderIndex`（Python 读同名页内状态）。
//! 2. **未知路由只记一行**：`setupPacks` 追加的功能包页面在 Rust 侧没有页面类，
//!    `nav_item` 收到未知 `routeKey` 时不会切页（`currentRoute` 不动）。
//! 3. **拖放只认本地文件**：`DataTransfer` 不暴露 URL 列表（`lib.rs::DropPayload` 已记），
//!    所以「拖链接」拿不到，只能拖本地文件；本地路径按 Qt 的 `QUrl::fromLocalFile`
//!    口径拼成 `file://…`（**不做百分号编码**，含空格 / 非 ASCII 的路径按原样拼）。
//! 4. **「有模态框盖着」是粗判**：`window::drop_decision` 需要 `mask_dialog_visible`，
//!    这里按「草稿框 / 消息框 / 扩展安装引导框任一可见」合成（Python 是
//!    `findChildren(MaskDialogBase)` 逐个 `isVisible()`）。
//! 5. **通知条 / 消息框各只有一份**：Slint 侧 `infoBar*` / `dialog*` 是单组属性，
//!    新的会覆盖旧的（Qt 侧可以叠多条 `InfoBar`）。按钮动作表同理，单槽。
//! 6. **通知条没有自动消失 / 没有「进度 toast」**：Python 的 `InfoBar` 有计时器，
//!    `ProgressToast` 另有生命周期；这里由用户点关闭按钮收起。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::{ComponentHandle, Model};

use crate::AppWindow;
use crate::close_request::CloseHooks;
use crate::draft_host::DraftUi;
use crate::engine::Engine;
use crate::window;

/// 通知条按钮的动作：`(窗口, 宿主)`。带宿主的目的是让动作能收起通知条 / 清表，
/// 而不用在宿主里存一份自己的 `Rc`（那会绕成环）。
pub type InfoAction = Rc<dyn Fn(&AppWindow, &ShellHost)>;

/// 消息框按钮的动作：`(窗口, 宿主, 按钮序号)`。返回 `true` = 这个框已处理完。
pub type DialogAction = Rc<dyn Fn(&AppWindow, &ShellHost, usize) -> bool>;

/// 外壳动作的宿主。只持引擎 / 草稿框 / 关窗链的 `Rc`，**不**持窗口强引用
/// （窗口回调里存着宿主的 `Rc`，强引用会绕成环）。
pub struct ShellHost {
    engine: Rc<RefCell<Engine>>,
    draft: Option<Rc<DraftUi>>,
    close: Rc<CloseHooks>,
    /// 当前通知条的按钮动作（与 `infoBarButtons` 的 `index` 对齐）。
    info_actions: RefCell<Vec<InfoAction>>,
    /// 当前消息框的按钮动作（关窗链处理不了时按序号问这里）。
    dialog_actions: RefCell<Vec<DialogAction>>,
    /// 「设置页检索没接」的提示只打一次。
    warned_settings_search: Cell<bool>,
}

impl ShellHost {
    /// 草稿框可以为 `None`：后台解析线程起不来时窗口照常（`boot` 的软失败口径）。
    pub fn new(
        engine: Rc<RefCell<Engine>>,
        draft: Option<Rc<DraftUi>>,
        close: Rc<CloseHooks>,
    ) -> Rc<Self> {
        Rc::new(Self {
            engine,
            draft,
            close,
            info_actions: RefCell::new(Vec::new()),
            dialog_actions: RefCell::new(Vec::new()),
            warned_settings_search: Cell::new(false),
        })
    }

    /// 导航项点击：`addTaskButton` 开草稿框，页面路由切页。
    pub fn nav_item(&self, window: &AppWindow, route_key: &str) {
        if route_key == window::ROUTE_ADD_TASK {
            self.open_draft(window);
            return;
        }
        if route_key == window::PAGE_TASK || route_key == window::PAGE_SETTINGS {
            crate::switch_page(window, route_key);
            // `switch_page` 会清空 `searchText`（`_updateSearchTarget` 里的
            // `searchEdit.clear()`），任务页要按「空检索」重推一遍行，否则旧过滤残留。
            if route_key == window::PAGE_TASK {
                self.apply_task_search(window, "");
            }
            return;
        }
        eprintln!("gd3-ui：未知导航项 {route_key}（功能包页面尚未接线），不切页");
    }

    /// 标题栏搜索框的键入：按当前页面分派。
    pub fn search(&self, window: &AppWindow, text: &str) {
        match window.get_currentRoute().as_str() {
            window::PAGE_TASK => self.apply_task_search(window, text),
            window::PAGE_SETTINGS => {
                if !self.warned_settings_search.replace(true) {
                    eprintln!(
                        "gd3-ui：设置页检索尚未接线（settings_page.slint 的卡片没有逐卡可见性），\
                         搜索框可以输入但不会过滤设置项。"
                    );
                }
            }
            route => eprintln!("gd3-ui：{route} 页没有 setSearchText，忽略检索 {text:?}"),
        }
    }

    /// 任务页检索：`filter::TaskFilter` → `sort::sort_refs` → 行
    /// （镜像 `task_page.py::_refreshList` 末段；条件全部读窗口上的现态）。
    pub fn apply_task_search(&self, window: &AppWindow, text: &str) {
        let mode = crate::filter::FilterMode::from_route(&window.get_filterMode());
        let category = {
            let index = window.get_categoryIndex();
            if index <= 0 {
                String::new()
            } else {
                window
                    .get_categoryNames()
                    .row_data(index as usize)
                    .map(|name| name.to_string())
                    .unwrap_or_default()
            }
        };
        let field = crate::sort::SortField::from_menu_index(window.get_sortFieldIndex());
        // `sortOrderIndex`：0 = 顺序、1 = 倒序（`task_page.slint` 的 ComboBox）。
        let ascending = window.get_sortOrderIndex() == 0;

        let engine = self.engine.borrow();
        let tasks = engine.tasks();
        let waiting_order = tasks.waiting_order();
        crate::refresh_task_rows(
            window,
            tasks.records(),
            crate::filter::TaskFilter {
                mode,
                category: &category,
                search: text,
            },
            field,
            ascending,
            &waiting_order,
            |record| {
                let kind = crate::facts::task_kind(record.type_name.as_deref());
                crate::card::CardRuntime {
                    can_seed: kind.can_seed,
                    has_output_file: kind.has_output_file,
                    is_seeding: tasks.is_seeding(&record.task_id),
                    ..crate::card::CardRuntime::default()
                }
            },
        );
    }

    /// 弹一条通知条并把按钮动作登记上（`InfoBar(...)` + `addWidget` + `show()`）。
    pub fn show_info_bar(
        &self,
        window: &AppWindow,
        spec: &window::InfoBarSpec,
        actions: Vec<InfoAction>,
    ) {
        crate::show_info_bar(window, spec);
        *self.info_actions.borrow_mut() = actions;
    }

    /// 镜像 `_onListenFailed`：监听器起不来 → 通知条 +「前往设置」。
    pub fn show_listen_failed(&self, window: &AppWindow, title: &str, error_text: &str) {
        let spec = window::listen_failed_info_bar(title, error_text);
        self.show_info_bar(
            window,
            &spec,
            vec![Rc::new(|target: &AppWindow, host: &ShellHost| {
                // Python：`lambda: (infoBar.close(), self._showPage(SettingPage))`
                host.hide_info_bar(target);
                crate::switch_page(target, window::PAGE_SETTINGS);
            })],
        );
    }

    /// 通知条第 `index` 个按钮被点。
    pub fn info_bar_button(&self, window: &AppWindow, index: i32) {
        let index = index.max(0) as usize;
        let action = self.info_actions.borrow().get(index).cloned();
        match action {
            Some(action) => action(window, self),
            None => eprintln!("gd3-ui：通知条第 {index} 个按钮没有登记处理函数"),
        }
    }

    /// 通知条被收起：清动作表 + 隐通知条（幂等）。
    pub fn hide_info_bar(&self, window: &AppWindow) {
        self.info_actions.borrow_mut().clear();
        crate::hide_info_bar(window);
    }

    /// 弹消息框并把按钮动作登记上。动作表按**序号**对齐 `spec.buttons`。
    pub fn show_dialog(
        &self,
        window: &AppWindow,
        spec: &window::DialogSpec,
        actions: Vec<DialogAction>,
    ) {
        crate::show_dialog(window, spec);
        *self.dialog_actions.borrow_mut() = actions;
    }

    /// 消息框第 `index` 个按钮被点：先问关窗链，再问登记的动作表。
    pub fn dialog_button(&self, window: &AppWindow, index: i32) {
        let index = index.max(0) as usize;
        if self.close.handle_button(window, index) {
            return;
        }
        let action = self.dialog_actions.borrow().get(index).cloned();
        match action {
            Some(action) => {
                if action(window, self, index) {
                    // 处理完就清表：Qt 侧每个 `exec()` 都是新对话框对象。
                    self.dialog_actions.borrow_mut().clear();
                }
            }
            None => eprintln!("gd3-ui：消息框第 {index} 个按钮没有登记处理函数"),
        }
    }

    /// 消息框复选框切换。当前只有关窗框有复选框，语义就是「记住我的选择」。
    pub fn dialog_checkbox(&self, _window: &AppWindow, checked: bool) {
        self.close.set_remember(checked);
    }

    /// `dragEnterEvent`：这个载荷能不能接。
    pub fn can_drop(&self, window: &AppWindow, payload: &crate::DropPayload) -> bool {
        let input = payload.drop_input(false, self.mask_visible(window));
        window::drop_decision(&input, |line| self.matches(line)) == window::DropDecision::Accept
    }

    /// `dropEvent`：取址后交给 `addUrls`（开草稿框 + 立刻解析）。
    pub fn drop_payload(&self, _window: &AppWindow, payload: &crate::DropPayload) {
        let urls: Vec<String> = if payload.file_paths.is_empty() {
            window::drop_urls(false, &[], payload.text.as_deref().unwrap_or(""), |line| {
                self.matches(line)
            })
        } else {
            payload
                .file_paths
                .iter()
                .map(|path| file_url(path))
                .collect()
        };
        if urls.is_empty() {
            return;
        }
        match &self.draft {
            Some(draft) => draft.open_with_urls(&urls),
            None => eprintln!("gd3-ui：草稿对话框不可用（后台解析线程未启动），丢弃 {urls:?}"),
        }
    }

    /// `featureService.match(line)`。
    fn matches(&self, line: &str) -> bool {
        self.engine.borrow().features().parsers().matches(line)
    }

    /// 「有模态框盖着」：草稿框 / 消息框 / 扩展安装引导框任一可见（见差异 4）。
    fn mask_visible(&self, window: &AppWindow) -> bool {
        window.get_dialogVisible()
            || window.get_taskDraftVisible()
            || window.get_extensionInstallVisible()
    }

    fn open_draft(&self, _window: &AppWindow) {
        match &self.draft {
            Some(draft) => draft.open(),
            None => eprintln!("gd3-ui：草稿对话框不可用（后台解析线程未启动）"),
        }
    }
}

/// 本地路径 → `file://` URL（Qt `QUrl::fromLocalFile` 的口径，不做百分号编码）。
pub fn file_url(path: &str) -> String {
    if path.starts_with("file://") {
        return path.to_string();
    }
    let normalized = path.replace('\\', "/");
    if normalized.starts_with('/') {
        format!("file://{normalized}")
    } else {
        // Windows 的 `C:/…` → `file:///C:/…`。
        format!("file:///{normalized}")
    }
}

/// 把外壳动作装到窗口上（`boot` 在有头 / 无头两条路径上都调一次）。
pub fn install(window: &AppWindow, host: Rc<ShellHost>) {
    let nav = Rc::clone(&host);
    let nav_window = window.as_weak();
    let search = Rc::clone(&host);
    let search_window = window.as_weak();
    let bar_button = Rc::clone(&host);
    let bar_button_window = window.as_weak();
    let bar_closed = Rc::clone(&host);
    let bar_closed_window = window.as_weak();
    let dialog_button = Rc::clone(&host);
    let dialog_button_window = window.as_weak();
    let dialog_checkbox = Rc::clone(&host);
    let dialog_checkbox_window = window.as_weak();
    let can_drop = Rc::clone(&host);
    let can_drop_window = window.as_weak();
    let drop = Rc::clone(&host);
    let drop_window = window.as_weak();

    crate::install_shell_actions(
        window,
        Rc::new(crate::ShellActions {
            nav_item: Box::new(move |route_key| {
                if let Some(window) = nav_window.upgrade() {
                    nav.nav_item(&window, route_key);
                }
            }),
            search: Box::new(move |text| {
                if let Some(window) = search_window.upgrade() {
                    search.search(&window, text);
                }
            }),
            info_bar_button: Box::new(move |index| {
                if let Some(window) = bar_button_window.upgrade() {
                    bar_button.info_bar_button(&window, index);
                }
            }),
            info_bar_closed: Box::new(move || {
                if let Some(window) = bar_closed_window.upgrade() {
                    bar_closed.hide_info_bar(&window);
                }
            }),
            dialog_button: Box::new(move |index| {
                if let Some(window) = dialog_button_window.upgrade() {
                    dialog_button.dialog_button(&window, index);
                }
            }),
            dialog_checkbox: Box::new(move |checked| {
                if let Some(window) = dialog_checkbox_window.upgrade() {
                    dialog_checkbox.dialog_checkbox(&window, checked);
                }
            }),
            can_drop: Box::new(move |payload: &crate::DropPayload| {
                match can_drop_window.upgrade() {
                    Some(window) => can_drop.can_drop(&window, payload),
                    None => false,
                }
            }),
            drop: Box::new(move |payload: &crate::DropPayload| {
                if let Some(window) = drop_window.upgrade() {
                    drop.drop_payload(&window, payload);
                }
            }),
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use gd3_core::task::{TaskRecord, TaskStatus};
    use gd3_core::task_error::TaskError;
    use gd3_net::probe::{ProbeOptions, RemoteFile};
    use serde_json::{Map, Value};

    use crate::config_store::ConfigStore;
    use crate::draft_worker::ParseWorker;
    use crate::settings_writer::SettingsWriter;
    use crate::task_create::{CreateDefaults, HttpParser, Prober};

    /// 桩探测器：一条网络都不发。
    struct StubProber;

    impl Prober for StubProber {
        fn probe(&self, options: &ProbeOptions) -> Result<RemoteFile, TaskError> {
            Ok(RemoteFile {
                name: options.url.rsplit('/').next().unwrap_or("x").to_string(),
                file_size: 64,
                can_use_range_requests: true,
                last_modified: String::new(),
                content_type: "application/octet-stream".to_string(),
            })
        }
    }

    /// 桩解析器：只负责「认领 http(s) URL」——`Engine::new` 故意不装解析器
    /// （第 137 步），而外壳的拖放判定要问 `featureService.match(line)`。
    struct AnyHttpParser;

    impl crate::feature_service::TaskParser for AnyHttpParser {
        fn matches(&self, options: &Map<String, Value>) -> bool {
            options
                .get("url")
                .and_then(Value::as_str)
                .is_some_and(|url| url.starts_with("http://") || url.starts_with("https://"))
        }

        fn parse(&self, options: &Map<String, Value>) -> Result<TaskRecord, TaskError> {
            let url = options
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let name = url.rsplit('/').next().unwrap_or("x").to_string();
            Ok(record(&name, "", TaskStatus::Waiting))
        }
    }

    fn record(name: &str, folder: &str, status: TaskStatus) -> TaskRecord {
        TaskRecord {
            name: name.to_string(),
            url: format!("http://example.com/{name}"),
            pack_id: "http".to_string(),
            task_id: format!("tsk_{name}"),
            status,
            steps: Vec::new(),
            created_at: 0,
            completed_at: 0,
            output_folder: folder.to_string(),
            file_size: 64,
            files: None,
            category: None,
            should_seed: false,
            checksums: Map::new(),
            extra: Map::new(),
            type_name: None,
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "gd3-ui-shell-host-{}-{tag}-{seq}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    /// 无头窗口 + 真引擎 + 关窗链 + 真草稿框（桩探测器）。
    struct Harness {
        window: AppWindow,
        engine: Rc<RefCell<Engine>>,
        draft: Rc<DraftUi>,
        host: Rc<ShellHost>,
        dir: PathBuf,
    }

    impl Harness {
        fn new(tag: &str) -> Harness {
            let dir = temp_dir(tag);
            let config =
                gd3_core::settings::default_config_with_download_folder(&dir.to_string_lossy());
            let mut engine = Engine::new(&config, dir.join("tasks.jsonl"));
            engine.start().expect("装载任务列表");
            engine
                .features_mut()
                .parsers_mut()
                .register(Box::new(AnyHttpParser));
            let engine = Rc::new(RefCell::new(engine));

            let window = crate::create_headless();
            let writer = Rc::new(SettingsWriter::new(ConfigStore::from_config(
                dir.join("UserConfig.json"),
                &config,
            )));
            let close = crate::close_request::install_shared(&window, writer);

            let parser = HttpParser::new(Box::new(StubProber), CreateDefaults::default());
            let worker = Arc::new(ParseWorker::with_parser(parser).expect("worker"));
            let draft = Rc::new(DraftUi::build(
                &window,
                Rc::clone(&engine),
                worker,
                &dir.to_string_lossy(),
                &[],
                8,
            ));

            let host = ShellHost::new(Rc::clone(&engine), Some(Rc::clone(&draft)), close);
            install(&window, Rc::clone(&host));
            Harness {
                window,
                engine,
                draft,
                host,
                dir,
            }
        }

        fn add_record(&self, name: &str, status: TaskStatus) {
            let mut engine = self.engine.borrow_mut();
            engine
                .tasks_mut()
                .add(record(name, &self.dir.to_string_lossy(), status), false);
        }
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn payload(files: &[&str], text: Option<&str>) -> crate::DropPayload {
        crate::DropPayload {
            file_paths: files.iter().map(|path| path.to_string()).collect(),
            text: text.map(str::to_string),
        }
    }

    #[test]
    fn nav_item_switches_pages_and_opens_the_draft() {
        let harness = Harness::new("nav");
        harness.window.invoke_navItemClicked("SettingPage".into());
        assert_eq!(harness.window.get_currentRoute(), "SettingPage");
        assert_eq!(
            harness.window.get_searchPlaceholder(),
            "搜索设置",
            "搜索框换口径"
        );

        harness.window.invoke_navItemClicked("TaskPage".into());
        assert_eq!(harness.window.get_currentRoute(), "TaskPage");

        assert!(!harness.window.get_taskDraftVisible());
        harness.window.invoke_navItemClicked("addTaskButton".into());
        assert!(
            harness.window.get_taskDraftVisible(),
            "「新建任务」开草稿框（addUrls([])）"
        );
        assert_eq!(harness.window.get_currentRoute(), "TaskPage", "开框不切页");
        assert!(harness.draft.ui().borrow().is_open());
    }

    #[test]
    fn an_unknown_route_leaves_the_page_alone() {
        let harness = Harness::new("unknown-route");
        harness.window.invoke_navItemClicked("PackPage".into());
        assert_eq!(
            harness.window.get_currentRoute(),
            "TaskPage",
            "不认的就不动"
        );
    }

    #[test]
    fn the_task_page_search_filters_the_rows() {
        let harness = Harness::new("search");
        harness.add_record("alpha.bin", TaskStatus::Running);
        harness.add_record("beta.bin", TaskStatus::Running);
        harness.host.apply_task_search(&harness.window, "");
        assert_eq!(harness.window.get_tasks().row_count(), 2);

        harness.window.invoke_searchEdited("alpha".into());
        assert_eq!(harness.window.get_tasks().row_count(), 1, "只留 alpha");
        assert_eq!(
            harness.window.get_tasks().row_data(0).unwrap().name,
            "alpha.bin"
        );

        // 检索 + 段筛选叠加：Completed 段里没有 running 任务。
        harness.window.set_filterMode("completed".into());
        harness.host.apply_task_search(&harness.window, "alpha");
        assert_eq!(harness.window.get_tasks().row_count(), 0);
    }

    #[test]
    fn switching_to_the_task_page_clears_the_old_filter() {
        let harness = Harness::new("clear-filter");
        harness.add_record("alpha.bin", TaskStatus::Running);
        harness.host.apply_task_search(&harness.window, "nope");
        assert_eq!(harness.window.get_tasks().row_count(), 0);

        harness.window.invoke_navItemClicked("TaskPage".into());
        assert_eq!(
            harness.window.get_tasks().row_count(),
            1,
            "切页要把检索清掉并重推行"
        );
    }

    #[test]
    fn the_settings_page_search_does_not_touch_the_task_rows() {
        let harness = Harness::new("settings-search");
        harness.add_record("alpha.bin", TaskStatus::Running);
        harness.host.apply_task_search(&harness.window, "");
        harness.window.invoke_navItemClicked("SettingPage".into());
        harness.window.invoke_searchEdited("folder".into());
        assert_eq!(
            harness.window.get_tasks().row_count(),
            1,
            "设置页检索不动任务列表"
        );
        assert_eq!(harness.window.get_currentRoute(), "SettingPage");
    }

    #[test]
    fn a_local_file_drop_becomes_a_file_url() {
        assert_eq!(file_url("/tmp/a.bin"), "file:///tmp/a.bin");
        assert_eq!(file_url("C:\\tmp\\a.bin"), "file:///C:/tmp/a.bin");
        assert_eq!(file_url("file:///tmp/a.bin"), "file:///tmp/a.bin");
    }

    #[test]
    fn a_visible_mask_blocks_the_drop() {
        let harness = Harness::new("mask");
        let files = payload(&["/tmp/a.bin"], None);
        assert!(harness.host.can_drop(&harness.window, &files));
        crate::show_dialog(&harness.window, &window::close_ask_dialog());
        assert!(
            !harness.host.can_drop(&harness.window, &files),
            "有模态框盖着就不接"
        );
    }

    #[test]
    fn a_drop_of_url_text_starts_parsing() {
        let harness = Harness::new("drop-text");
        let text = payload(&[], Some("https://example.com/one.bin"));
        assert!(harness.host.can_drop(&harness.window, &text), "链接可接");
        harness.host.drop_payload(&harness.window, &text);
        assert!(harness.window.get_taskDraftVisible(), "开草稿框");
        assert_eq!(
            harness.draft.ui().borrow().urls(),
            vec!["https://example.com/one.bin".to_string()]
        );

        // 不带链接的文本（`featureService.match` 不认）→ 什么都不做。
        let junk = payload(&[], Some("hello world"));
        assert!(!harness.host.can_drop(&harness.window, &junk));
        harness.host.drop_payload(&harness.window, &junk);
        assert_eq!(harness.draft.ui().borrow().urls().len(), 1, "没加进去");
    }

    #[test]
    fn a_local_file_drop_opens_the_draft_with_a_file_url() {
        let harness = Harness::new("drop-file");
        let files = payload(&["/tmp/a.bin"], None);
        harness.host.drop_payload(&harness.window, &files);
        assert!(harness.window.get_taskDraftVisible());
        assert_eq!(
            harness.draft.ui().borrow().urls(),
            vec!["file:///tmp/a.bin".to_string()]
        );
    }

    #[test]
    fn the_listen_failure_info_bar_goes_to_the_settings_page() {
        let harness = Harness::new("listen-failed");
        harness
            .host
            .show_listen_failed(&harness.window, "浏览器扩展服务无法启动", "端口被占用");
        assert!(harness.window.get_infoBarVisible());
        assert_eq!(harness.window.get_infoBarTitle(), "浏览器扩展服务无法启动");
        assert_eq!(
            harness.window.get_infoBarButtons().row_count(),
            1,
            "只有一个「前往设置」"
        );

        harness.window.invoke_infoBarButtonClicked(0);
        assert!(!harness.window.get_infoBarVisible(), "点完收起通知条");
        assert_eq!(harness.window.get_currentRoute(), "SettingPage", "跳设置页");
    }

    #[test]
    fn closing_the_info_bar_hides_it_and_clears_the_actions() {
        let harness = Harness::new("info-bar-close");
        harness
            .host
            .show_listen_failed(&harness.window, "标题", "正文");
        harness.window.invoke_infoBarClosed();
        assert!(!harness.window.get_infoBarVisible());
        // 表已清：再点不会有动作（只打一行）也不 panic。
        harness.window.invoke_infoBarButtonClicked(0);
        assert_eq!(harness.window.get_currentRoute(), "TaskPage");
    }

    #[test]
    fn the_close_dialog_still_works_through_the_shell_host() {
        let harness = Harness::new("close-dialog");
        crate::show_dialog(&harness.window, &window::close_ask_dialog());
        // 序号 1 = 「继续在后台运行」。
        harness.window.invoke_dialogButtonClicked(1);
        assert!(
            !harness.window.get_dialogVisible(),
            "关窗链认得这个框，点完就收起"
        );
    }

    #[test]
    fn a_dialog_registered_by_the_host_resolves_its_own_button() {
        let harness = Harness::new("own-dialog");
        let spec = window::exception_dialog("炸了");
        let touched = Rc::new(Cell::new(false));
        let flag = Rc::clone(&touched);
        harness.host.show_dialog(
            &harness.window,
            &spec,
            vec![Rc::new(move |target: &AppWindow, _host, _index| {
                flag.set(true);
                crate::hide_dialog(target);
                true
            })],
        );
        harness.window.invoke_dialogButtonClicked(0);
        assert!(touched.get(), "登记的动作跑到了");
        assert!(!harness.window.get_dialogVisible());
    }

    #[test]
    fn the_dialog_checkbox_feeds_the_close_chain() {
        let harness = Harness::new("checkbox");
        crate::show_dialog(&harness.window, &window::close_ask_dialog());
        harness.window.invoke_dialogCheckboxToggled(true);
        assert!(harness.host.close.remember(), "「记住我的选择」传进关窗链");
    }
}
