//! 引擎装配门面：镜像 `app/engine.py`（74 行）的 `Engine`。
//!
//! `app/engine.py` 里没有一行业务逻辑，它只做两件事：**按固定顺序把服务拼起来**
//! （外加唯一一条信号连线），以及**按固定顺序启动/停止它们**。这一步把这份
//! 「装配契约」搬过来：
//!
//! ```text
//! __init__:
//!   CategoryService()
//!   SpeedMeter(coroutineRunner)
//!   TaskService(coroutineRunner, categoryService, speedMeter, fileWatcher,
//!               deleteRecoverably, lambda: NameConflictChoice(cfg.nameConflict.value))
//!   NameConflictQueue(taskService.probeConflict, taskService.add, coroutineRunner.post)
//!   taskService.nameConflicted.connect(nameConflictQueue.add)      ← 唯一的信号连线
//!   RuntimeStatusService(coroutineRunner)
//!   FeatureService(taskService, categoryService, coroutineRunner, runtimeStatusService)
//!   BrowserService(coroutineRunner, taskService, speedMeter.speedChanged,
//!                  featureService.parse, loadCrx, extensionFolder)
//!   browserListener = createBrowserListener(coroutineRunner, browserService)
//!   Aria2RpcService(coroutineRunner, featureService.parse, taskService.add)
//!   aria2Listener  = createAria2Listener(coroutineRunner, aria2RpcService)
//!
//! start: resumeSaved → featureService.activate → browserListener.start
//!        → aria2Listener.start
//! stop : taskService.stop → taskService.flush → speedMeter.stop
//!        → browserListener.stop → aria2Listener.stop → featureService.deactivate
//!        → taskService.flush                                    ← 收尾再 flush 一次
//! ```
//!
//! 顺序不是随便排的：`NameConflictQueue` 要拿 `taskService` 的两个方法，所以必须在
//! `TaskService` 之后；`FeatureService` 要拿 `taskService`/`categoryService`/
//! `runtimeStatusService`，三者都得先有。这些依赖关系逐条记进 [`Engine::trace`]，
//! 测试钉死——服务内部还是桩，但「谁在谁之后、谁连谁」不会被后续改动悄悄改掉。
//!
//! # 哪些是真的
//!
//! - [`TaskService`]：第 81/82 步的记账半边（含 `tasks.jsonl` 装载与落盘）；
//! - [`SpeedMeter`]：整类都在 [`crate::speed_meter`]（第 104 步），由
//!   [`TaskService`] 持有（第 105 步）；本模块只负责把效果记进轨迹
//!   （[`Engine::drain_speed_effects`]）；
//! - [`NameConflictQueue`]：复用 [`crate::name_conflict::NameConflictQueue`]
//!   （第 65 步 b 已逐条对齐 `name_conflict_queue.py`）；
//! - [`CategoryService`]：规则装载 + 匹配 + 输出目录推导（第 95 步起整类都在
//!   [`crate::category`]，这里 `pub use` 转出口）；`TaskService` 持有的是一份
//!   **克隆**，改规则后要 [`Engine::push_categories`] 推过去（见「已知差异」）；
//! - [`RuntimeStatusService`]：第 102 步起整类都在 [`crate::runtime_status`]，这里
//!   `pub use` 转出口（第 107 步删掉了第 83 步那份空壳桩）；
//! - 监听器：`PortListener` / `RustBrowserListener` / `RustAria2Listener` 三个实现
//!   都还没迁（套接字层在 `gd3-servers`），这里只保留**后端开关点**与配置快照。
//!
//! # 已知差异
//!
//! - **配置是快照**：Python 的 `cfg.*` 是活对象——`PortListener` 连 `valueChanged`、
//!   `_pump` 每次都读 `cfg.maxTaskNum.value`、`nameConflict` 走 `lambda` 现读。
//!   Rust 侧还没有响应式配置层：[`Engine`] 持有一份 `UserConfig` 克隆，
//!   `nameConflict` / `maxTaskNum` **每次用时现读**（与 Python 同构），
//!   监听器则在构造时取快照 [`ListenerSettings`]——改配置不会自动重开端口；
//! - `TaskService::stop()` 只改记录状态、**不动队列**（Python 也不动），所以停机后
//!   `runningCount()` 仍可能 >0；真正的协程取消归引擎层；
//! - `TaskStore.flush()` 没有脏标记（每次都整文件重写），Rust 的 `flush()` 有
//!   `pending_flush` 短路：停机时若一个 `RUNNING`/`WAITING` 任务都没有，
//!   `tasks.jsonl` 不会被重新写一遍（字节内容等价，只是少了这次写盘）；
//! - `featureService` 的**解析器链**已搬（第 108 步，`crate::feature_service`）：
//!   `register` 的稳定排序、`parse` 的身份预设前置、`match` / `matchPassive`、
//!   `No parser matched: <url>` 都在；**pack 本身搬不了**——`loadPacks` 要
//!   `importlib` 加载 `features/` 下的 Python 子类，`pack.parsers` /
//!   `pack.pages()` / `pack.config` / `taskCardClass` 全是 Python 对象。
//!   `gd3-core::loader`（第 87 步）只搬了「按依赖排序 manifest 列表」。
//!   所以 `activate()` / `deactivate()` 在无 pack 时是空转（Python 同）；
//! - `browserService` / `aria2RpcService` 仍是桩：装配点保留，行为没有。
//!   两者共用 `featureService.parse` 这条端口（`browser_service.py:496` 与
//!   `aria2_rpc.py:218` 各调一次），所以它们能接上的前提是先有 pack；
//!   `loadCrx` / `deleteRecoverably` / `fileWatcher` 同理；
//! - `probeConflict` 已接线（第 96 步）：`TaskConflictActions::probe` 转调
//!   [`TaskService::probe_conflict_by_id`]，即 `TaskStore.probeConflict` +
//!   `categoryService.outputFolderOf`；Python 里队列拿到的是 `Task` 对象，Rust 侧
//!   队列只存 `ConflictTask { task_id, file_size }`，所以按 id 回查；
//! - **分类服务是克隆**：Python 的 `Engine` / `TaskService` / `FeatureService` 拿的是
//!   同一个 `CategoryService` 实例，改规则立刻全处可见；Rust 侧
//!   [`Engine::new`] 把 `category.clone()` 交给 `TaskService`，所以改完规则必须
//!   调 [`Engine::push_categories`]（或 `TaskService::set_categories`）重新同步；
//! - Python 的 `_pump()` 在 `_schedule` 里被顺带调用；这里由调用方显式
//!   [`Engine::pump`]（`resume_saved` 只入队，第 82 步的结论）。

use std::cell::RefCell;
use std::io;
use std::path::{Path, PathBuf};

use gd3_core::config::UserConfig;
use gd3_core::settings;

pub use crate::category::CategoryService;
pub use crate::feature_service::FeatureService;
use crate::name_conflict::{
    ConflictActions, ConflictEvent, ConflictTask, DialogOutcome, NameConflictChoice,
    NameConflictQueue, ProbeTarget,
};
pub use crate::runtime_status::RuntimeStatusService;
use crate::speed_meter::{SpeedEffect, SpeedMeter};
use crate::task_service::{AddOutcome, TaskService};

// ---------------------------------------------------------------------------
// 配置
// ---------------------------------------------------------------------------

/// `cfg.py` 里引擎用到的六项（`engine.py` 直接读到的四项 + 两处「用时现读」的）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EngineConfig {
    /// `cfg.nameConflict`（`lambda: NameConflictChoice(cfg.nameConflict.value)`）。
    pub name_conflict: NameConflictChoice,
    /// `cfg.isBrowserExtensionEnabled`。
    pub browser_enabled: bool,
    /// `cfg.browserExtensionPort`。
    pub browser_port: u16,
    /// `cfg.isAria2RpcEnabled`。
    pub aria2_enabled: bool,
    /// `cfg.aria2RpcPort`。
    pub aria2_port: u16,
    /// `cfg.maxTaskNum`（`_pump` 的并发上限）。
    pub max_task_num: usize,
}

impl EngineConfig {
    /// 从配置读这六项：每项都过 `validator.correct()`（`Spec::read` 的口径），
    /// 与 Python 的 `cfg.*.value` 同源。
    pub fn from_config(config: &UserConfig) -> Self {
        EngineConfig {
            name_conflict: name_conflict_choice(config),
            browser_enabled: settings::read_bool(config, "isBrowserExtensionEnabled")
                .unwrap_or(false),
            browser_port: port_attr(config, "browserExtensionPort"),
            aria2_enabled: settings::read_bool(config, "isAria2RpcEnabled").unwrap_or(false),
            aria2_port: port_attr(config, "aria2RpcPort"),
            max_task_num: max_task_num(config),
        }
    }
}

/// `lambda: NameConflictChoice(cfg.nameConflict.value)`：未知值回落 `keepBoth`
/// （`OptionsValidator.correct()` 取 `options[0]`）。
fn name_conflict_choice(config: &UserConfig) -> NameConflictChoice {
    settings::read_text(config, "nameConflict")
        .and_then(|value| NameConflictChoice::parse(&value))
        .unwrap_or(NameConflictChoice::KeepBoth)
}

/// 整数项：`Spec::read` 已过 `correct()`；缺项时用源码缺省值（不另抄常量）。
fn int_attr(config: &UserConfig, attr: &str) -> i64 {
    settings::read_i64(config, attr).unwrap_or_else(|| {
        match settings::spec(attr).map(|spec| spec.kind) {
            Some(settings::Kind::Int { default, .. }) => default,
            _ => 0,
        }
    })
}

/// 端口项：`RangeValidator(1024, 65535)` 已保证落在 `u16` 内。
fn port_attr(config: &UserConfig, attr: &str) -> u16 {
    int_attr(config, attr) as u16
}

/// `cfg.maxTaskNum`（`RangeValidator(1, 10)`）。
fn max_task_num(config: &UserConfig) -> usize {
    int_attr(config, "maxTaskNum").max(1) as usize
}

// ---------------------------------------------------------------------------
// 桩服务
// ---------------------------------------------------------------------------

// `FeatureService` 不再是桩：真实现见 `crate::feature_service`（上面 `pub use`）。
// `Engine::new` 的装配点、`Engine::start` / `Engine::stop` 的调用点都保留。

/// `app/services/browser_service.py`（约 500 行）的桩：配对、WebSocket 帧、
/// 建任务动作都未迁移。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BrowserService;

impl BrowserService {
    pub fn new() -> Self {
        BrowserService
    }
}

/// `app/services/aria2_rpc.py`（约 400 行）的桩：JSON-RPC 方法表与任务草稿
/// 请求都未迁移。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Aria2RpcService;

impl Aria2RpcService {
    pub fn new() -> Self {
        Aria2RpcService
    }
}

// ---------------------------------------------------------------------------
// 监听器
// ---------------------------------------------------------------------------

/// `GD3_RUST_BROWSER`（`app/services/rust_browser.py` 的开关）。
pub const RUST_BROWSER_ENV: &str = "GD3_RUST_BROWSER";
/// `GD3_RUST_ARIA2`（`app/services/rust_aria2.py` 的开关）。
pub const RUST_ARIA2_ENV: &str = "GD3_RUST_ARIA2";

/// 监听器后端：`engine.py` 里 `createBrowserListener` / `createAria2Listener`
/// 的两个分支。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListenerBackend {
    /// `RustBrowserListener` / `RustAria2Listener`（`GD3_RUST_*` == `"1"`）。
    Rust,
    /// 默认的 `PortListener`。
    PortListener,
}

/// `PortListener.__init__` 的两个配置项。
///
/// Python 存的是 `ConfigItem` 引用，`start()` 时读值并连 `valueChanged`；
/// Rust 侧配置还没有响应式层，所以这里是**构造时快照**（见模块头「已知差异」）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListenerSettings {
    pub enabled: bool,
    pub port: u16,
}

/// `PortListener` / `Rust*Listener` 的共同公开面（`start`/`stop`/`state`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Listener {
    backend: ListenerBackend,
    settings: ListenerSettings,
    running: bool,
}

impl Listener {
    /// 后端类型（测试与「状态行」用）。
    pub fn backend(&self) -> ListenerBackend {
        self.backend
    }

    /// 构造时快照的配置。
    pub fn settings(&self) -> ListenerSettings {
        self.settings
    }

    /// `start()` 之后、`stop()` 之前为真。
    ///
    /// 注意 `isEnabled == false` 时 Python 也会 `start()`（提交一个「关掉监听」的
    /// 协程），所以这里同样置真——「在跑」= 生命周期已启动，不等于端口已绑定。
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// `PortListener.start()`。
    pub fn start(&mut self) {
        self.running = true;
    }

    /// `PortListener.stop()`。
    pub fn stop(&mut self) {
        self.running = false;
    }
}

/// `os.environ.get(var) == "1"`（`rust_browser.isEnabled()` 的精确口径）。
fn flag_is_on(value: Option<&str>) -> bool {
    value == Some("1")
}

/// 读环境变量开关（`GD3_RUST_BROWSER` / `GD3_RUST_ARIA2`）。
pub fn rust_backend_enabled(var: &str) -> bool {
    flag_is_on(std::env::var(var).ok().as_deref())
}

/// `createBrowserListener`（`engine.py:52-60`）。
pub fn create_browser_listener(settings: ListenerSettings, rust_backend: bool) -> Listener {
    Listener {
        backend: if rust_backend {
            ListenerBackend::Rust
        } else {
            ListenerBackend::PortListener
        },
        settings,
        running: false,
    }
}

/// `createAria2Listener`（`engine.py:63-74`）。
pub fn create_aria2_listener(settings: ListenerSettings, rust_backend: bool) -> Listener {
    Listener {
        backend: if rust_backend {
            ListenerBackend::Rust
        } else {
            ListenerBackend::PortListener
        },
        settings,
        running: false,
    }
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// `Engine` 的装配轨迹（测试逐条比对，见模块头）。
///
/// 标签约定：`construct:<名字>` / `connect:<谁> -> <谁>` /
/// `start:<方法>` / `stop:<方法>` / `nameConflictQueue.<方法>(<参数>)`。
pub type Trace = Vec<String>;

/// `app/engine.py::Engine`：装配 + 生命周期，不含业务逻辑。
pub struct Engine {
    config: UserConfig,
    tasks_path: PathBuf,
    category: CategoryService,
    tasks: TaskService,
    name_conflicts: NameConflictQueue,
    runtime_status: RuntimeStatusService,
    features: FeatureService,
    browser: BrowserService,
    aria2: Aria2RpcService,
    browser_listener: Listener,
    aria2_listener: Listener,
    trace: Trace,
}

/// `Engine.start()` 的产物（Python 里这些是信号与副作用，没有返回值）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StartOutcome {
    /// `TaskStore.loadSaved()` 跳过的坏行（Python 只记日志）。
    pub load_failures: Vec<String>,
    /// `resumeSaved()` 重新入队的任务 id（`WAITING`/`RUNNING`）。
    pub resumed: Vec<String>,
}

impl Engine {
    /// 按 `Engine.__init__` 的顺序装配。
    ///
    /// `config` 是 `UserConfig` 的克隆（Python 里是全局 `cfg`）；
    /// `tasks_path` 是 `APP_DATA_DIR / "tasks.jsonl"`（`TaskStore._path`）。
    pub fn new(config: &UserConfig, tasks_path: impl Into<PathBuf>) -> Engine {
        let config = config.clone();
        let tasks_path = tasks_path.into();
        let settings = EngineConfig::from_config(&config);
        let mut trace: Trace = Vec::new();

        trace.push("construct:CategoryService".to_string());
        let category = CategoryService::from_config(&config);

        // Python 在这里构造 `SpeedMeter` 并注入 `TaskService`；Rust 侧速度表归
        // `TaskService` 自己持有（第 105 步），这里只留构造轨迹以保住装配顺序。
        trace.push("construct:SpeedMeter".to_string());

        trace.push(
            "connect:taskService <- categoryService, speedMeter, fileWatcher, \
             deleteRecoverably, nameConflictChoice"
                .to_string(),
        );
        trace.push("construct:TaskService".to_string());
        let mut tasks = TaskService::new();
        // `TaskService.add` 的第一步就是 `categoryService.outputFolderOf(task)`；
        // Python 传的是同一个实例，Rust 侧传克隆（见「已知差异」）。
        tasks.set_categories(category.clone());

        trace.push(
            "connect:nameConflictQueue <- taskService.probeConflict, taskService.add, post"
                .to_string(),
        );
        trace.push("construct:NameConflictQueue".to_string());
        let name_conflicts = NameConflictQueue::new();

        // `engine.py:25`：整个引擎唯一一条信号连线。
        trace.push("connect:taskService.nameConflicted -> nameConflictQueue.add".to_string());

        trace.push("construct:RuntimeStatusService".to_string());
        let runtime_status = RuntimeStatusService::new();

        trace.push(
            "connect:featureService <- taskService, categoryService, runtimeStatusService"
                .to_string(),
        );
        trace.push("construct:FeatureService".to_string());
        let features = FeatureService::new();

        trace.push(
            "connect:browserService <- taskService, speedMeter.speedChanged, \
             featureService.parse, loadCrx, extensionFolder"
                .to_string(),
        );
        trace.push("construct:BrowserService".to_string());
        let browser = BrowserService::new();

        trace.push("construct:browserListener".to_string());
        let browser_listener = create_browser_listener(
            ListenerSettings {
                enabled: settings.browser_enabled,
                port: settings.browser_port,
            },
            rust_backend_enabled(RUST_BROWSER_ENV),
        );

        trace.push("connect:aria2RpcService <- featureService.parse, taskService.add".to_string());
        trace.push("construct:Aria2RpcService".to_string());
        let aria2 = Aria2RpcService::new();

        trace.push("construct:aria2Listener".to_string());
        let aria2_listener = create_aria2_listener(
            ListenerSettings {
                enabled: settings.aria2_enabled,
                port: settings.aria2_port,
            },
            rust_backend_enabled(RUST_ARIA2_ENV),
        );

        Engine {
            config,
            tasks_path,
            category,
            tasks,
            name_conflicts,
            runtime_status,
            features,
            browser,
            aria2,
            browser_listener,
            aria2_listener,
            trace,
        }
    }

    /// 装配轨迹（构造 + 生命周期），见 [`Trace`]。
    pub fn trace(&self) -> &[String] {
        &self.trace
    }

    /// Python 里的全局 `cfg`（引擎持有一份克隆）。
    pub fn config(&self) -> &UserConfig {
        &self.config
    }

    /// 可变配置：Python 改 `cfg.xxx.value` 的地方（设置页）走这里。
    pub fn config_mut(&mut self) -> &mut UserConfig {
        &mut self.config
    }

    /// `APP_DATA_DIR / "tasks.jsonl"`（或 `--tasks-file` 的覆盖值）。
    pub fn tasks_path(&self) -> &Path {
        &self.tasks_path
    }

    pub fn category(&self) -> &CategoryService {
        &self.category
    }

    /// 改完规则/输出设置后把分类服务推给 [`TaskService`]。
    ///
    /// Python 没有这一步（同一个对象），Rust 侧是克隆；见「已知差异」。
    pub fn push_categories(&mut self) {
        self.tasks.set_categories(self.category.clone());
    }

    /// `engine.speedMeter`：Python 里 `TaskService` / `BrowserService` 拿的是
    /// **同一个实例**，Rust 侧归 [`TaskService`] 所有，这里只转出口。
    ///
    /// 写入口（`addSpeed` / `tick` / `stop`）都在 [`TaskService`] 上
    /// （`add_speed` / `tick_speed_meter` / `stop_speed_meter`），因为它们要顺带把
    /// 效果攒进缓冲——绕过去就等于把 `speedChanged` 丢了。
    pub fn speed_meter(&self) -> &SpeedMeter {
        self.tasks.speed_meter()
    }

    pub fn tasks(&self) -> &TaskService {
        &self.tasks
    }

    /// 任务列表的写入口（UI 动作层用，等价于 Python 的 `engine.taskService`）。
    pub fn tasks_mut(&mut self) -> &mut TaskService {
        &mut self.tasks
    }

    /// 动作层句柄（第 100 步）：把 [`crate::actions::TaskService`] 的 11 个方法
    /// 落到引擎自己这份 [`TaskService`] 上，副作用摊成
    /// [`crate::task_actions::TaskAction`]。
    ///
    /// 借用形态是 `RefCell<&mut TaskService>`：`boot::run` 里引擎是局部变量
    /// （没有 `Rc`），这个句柄只能**临时**持有——借出期间不能再碰 `engine.tasks`，
    /// 拿到效果队列就 `drop` 掉。UI 要长期持有就让调用方自己拿
    /// `Rc<RefCell<TaskService>>`（同一个 trait 的第三个 `impl`）。
    ///
    /// `maxTaskNum` 在构造时读一次：一次动作内配置不会变，等价于 Python 每次
    /// `_pump` 现读 `cfg.maxTaskNum.value`。
    pub fn task_actions(&mut self) -> crate::task_actions::TaskActions<RefCell<&mut TaskService>> {
        let max = self.max_task_num();
        crate::task_actions::TaskActions::with_max(RefCell::new(&mut self.tasks), max)
    }

    pub fn name_conflicts(&self) -> &NameConflictQueue {
        &self.name_conflicts
    }

    pub fn runtime_status(&self) -> &RuntimeStatusService {
        &self.runtime_status
    }

    /// 运行时状态的写入口（`install` / `cancelInstall` / `refreshStatus`）。
    ///
    /// Python 里 `RuntimeCard`（`setting_cards.py:804`）与 OOBE 向导直接调
    /// `runtimeStatusService`；Rust 侧那些卡片还没接线，这里先留出口。
    pub fn runtime_status_mut(&mut self) -> &mut RuntimeStatusService {
        &mut self.runtime_status
    }

    pub fn features(&self) -> &FeatureService {
        &self.features
    }

    /// 注册解析器的写入口（`FeatureService.register` 的落点）。
    ///
    /// Python 在 `load()` 里遍历 `loadPacks()` 的结果逐个 `register`；Rust 没有
    /// pack 可加载，宿主（扩展安装流程 / 测试）直接往链里塞 [`TaskParser`]。
    pub fn features_mut(&mut self) -> &mut FeatureService {
        &mut self.features
    }

    pub fn browser(&self) -> &BrowserService {
        &self.browser
    }

    pub fn aria2(&self) -> &Aria2RpcService {
        &self.aria2
    }

    pub fn browser_listener(&self) -> &Listener {
        &self.browser_listener
    }

    pub fn aria2_listener(&self) -> &Listener {
        &self.aria2_listener
    }

    /// `EngineConfig` 的现读形态（Python 的 `cfg.*.value` 每次用时现取）。
    pub fn settings(&self) -> EngineConfig {
        EngineConfig::from_config(&self.config)
    }

    /// `lambda: NameConflictChoice(cfg.nameConflict.value)`——**每次调用现读**，
    /// 所以设置页改了 `nameConflict` 之后下一次冲突就用新值。
    pub fn name_conflict_choice(&self) -> NameConflictChoice {
        name_conflict_choice(&self.config)
    }

    /// `cfg.maxTaskNum.value`。
    pub fn max_task_num(&self) -> usize {
        max_task_num(&self.config)
    }

    /// `Engine.start()`：`resumeSaved()` → `featureService.activate()` →
    /// 两个监听器 `start()`。
    ///
    /// Python 的 `resumeSaved()` 内部先 `TaskStore.loadSaved()`（读 `tasks.jsonl`），
    /// 所以装载也在这里发生；坏行只计数（Python 记日志后跳过）。
    pub fn start(&mut self) -> io::Result<StartOutcome> {
        self.trace.push("start:loadSaved".to_string());
        let load_failures = self.tasks.load_into(&self.tasks_path)?;

        self.trace.push("start:resumeSaved".to_string());
        let resumed = self.tasks.resume_saved();

        self.trace.push("start:featureService.activate".to_string());
        self.features.activate();

        self.trace.push("start:browserListener.start".to_string());
        self.browser_listener.start();
        self.trace.push("start:aria2Listener.start".to_string());
        self.aria2_listener.start();

        Ok(StartOutcome {
            load_failures,
            resumed,
        })
    }

    /// `Engine.stop()`：先把在跑/等待的任务标成 `PAUSED` 并落盘，再收尾。
    ///
    /// **`flush()` 出现两次**（`engine.py:44` 与 `:49`），第二次是给
    /// `featureService.deactivate()` 里可能产生的改动兜底。Python 的 `flush()`
    /// 每次都写；Rust 的 `flush()` 有脏标记短路，所以第二次通常是 `Ok(false)`，
    /// 但顺序与「两次都在」这个事实保持不变。
    pub fn stop(&mut self) -> io::Result<()> {
        self.trace.push("stop:taskService.stop".to_string());
        self.tasks.stop();

        self.trace.push("stop:flush#1".to_string());
        self.tasks.flush(&self.tasks_path)?;

        self.trace.push("stop:speedMeter.stop".to_string());
        self.tasks.stop_speed_meter();
        self.drain_speed_effects();

        self.trace.push("stop:browserListener.stop".to_string());
        self.browser_listener.stop();

        self.trace.push("stop:aria2Listener.stop".to_string());
        self.aria2_listener.stop();

        self.trace
            .push("stop:featureService.deactivate".to_string());
        self.features.deactivate();

        self.trace.push("stop:flush#2".to_string());
        self.tasks.flush(&self.tasks_path)?;
        Ok(())
    }

    /// `_pump()`：按 `cfg.maxTaskNum` 派发等待队列。
    ///
    /// Python 在 `_schedule` 里顺带调用；这里由调用方显式触发（没有引擎时
    /// 「派发」只是记账，见 `task_service.rs` 的模块头）。
    pub fn pump(&mut self) -> Vec<String> {
        let dispatched = self.tasks.pump(self.max_task_num());
        self.drain_speed_effects();
        dispatched
    }

    /// 取走 [`TaskService::take_speed_effects`] 攒下的速度表效果，并把它们记进轨迹。
    ///
    /// 标签：`speed:start:<workId>` / `speed:cancel:<workId>` / `speed:emit:<值>`。
    /// 宿主（`boot::run` / 主窗口）拿返回值去真的起 1 秒周期、取消周期、刷数字；
    /// 引擎测试靠轨迹钉住「谁在什么时候起表」。
    pub fn drain_speed_effects(&mut self) -> Vec<SpeedEffect> {
        let effects = self.tasks.take_speed_effects();
        for effect in &effects {
            self.trace.push(match effect {
                SpeedEffect::StartTicking { work_id } => format!("speed:start:{work_id}"),
                SpeedEffect::Cancel { work_id } => format!("speed:cancel:{work_id}"),
                SpeedEffect::Emit { speed } => format!("speed:emit:{speed}"),
            });
        }
        effects
    }

    /// `taskService.nameConflicted.connect(nameConflictQueue.add)` 的落点。
    ///
    /// 返回队列事件：`RefreshPosted` 表示「调用方须自己调
    /// [`Engine::refresh_conflicts`]」（Python 是 `coroutineRunner.post(self._refresh)`）。
    ///
    /// `probe` 走 [`TaskService::probe_conflict_by_id`]（`TaskStore.probeConflict`）：
    /// 占位符**已存在且不属于未完成任务**才算冲突，此时队列不再放行，等
    /// [`Engine::apply_dialog_outcome`] 收用户的选择。
    pub fn on_name_conflicted(&mut self, task: ConflictTask) -> Vec<ConflictEvent> {
        let mut actions = TaskConflictActions {
            tasks: &mut self.tasks,
            calls: Vec::new(),
        };
        let events = self.name_conflicts.add(&mut actions, task);
        absorb_calls(&mut self.trace, &mut actions);
        events
    }

    /// `_post(self._refresh)` 的落点：探测队列里的下一个冲突并 emit `conflictChanged`。
    pub fn refresh_conflicts(&mut self) -> Vec<ConflictEvent> {
        let mut actions = TaskConflictActions {
            tasks: &mut self.tasks,
            calls: Vec::new(),
        };
        let events = self.name_conflicts.refresh(&mut actions);
        absorb_calls(&mut self.trace, &mut actions);
        events
    }

    /// `NameConflictPrompt` 的结论 → `queue.setChoice` / `queue.cancel`。
    ///
    /// 队列里没有当前冲突（用户没被问过，或已被 `cancel` 清掉）时两个分支都是
    /// 「直接 return」——语义在第 65 步 b 已对齐。
    pub fn apply_dialog_outcome(&mut self, outcome: &DialogOutcome) -> Vec<ConflictEvent> {
        let mut actions = TaskConflictActions {
            tasks: &mut self.tasks,
            calls: Vec::new(),
        };
        let events = match outcome {
            DialogOutcome::SetChoice {
                task_id,
                choice,
                is_applied_to_rest,
            } => {
                self.name_conflicts
                    .set_choice(&mut actions, task_id, *choice, *is_applied_to_rest)
            }
            DialogOutcome::Cancel { task_id } => self.name_conflicts.cancel(&mut actions, task_id),
        };
        absorb_calls(&mut self.trace, &mut actions);
        events
    }
}

/// 队列对引擎的两处注入（`ConflictActions`）：
/// `probe` = `taskService.probeConflict`，`add` = `taskService.add`。
struct TaskConflictActions<'a> {
    tasks: &'a mut TaskService,
    calls: Vec<String>,
}

/// 把 `ConflictActions` 记下的调用并进轨迹。
///
/// 单独成函数而不是 `Engine` 的方法：`actions` 借着 `self.tasks`，方法签名
/// `&mut self` 会与它冲突；这里只借 `self.trace`（不同字段）。
fn absorb_calls(trace: &mut Trace, actions: &mut TaskConflictActions<'_>) {
    trace.extend(std::mem::take(&mut actions.calls));
}

impl ConflictActions for TaskConflictActions<'_> {
    /// `TaskService.probeConflict(task)`：`outputFolderOf` 之后再
    /// `TaskStore.probeConflict`，四列 `stat()` 一起带回来。
    fn probe(&mut self, task: &ConflictTask) -> Option<ProbeTarget> {
        self.tasks.probe_conflict_by_id(&task.task_id)
    }

    /// `TaskService.add(task, choice=choice)`：任务已经在列表里，所以 Python 在
    /// 第一行就 `return True`（Rust 对应 [`AddOutcome::Duplicate`]）。
    fn add(&mut self, task: &ConflictTask, choice: NameConflictChoice) -> bool {
        self.calls.push(format!(
            "nameConflictQueue.add({}, {})",
            task.task_id,
            choice.as_str()
        ));
        let Some(record) = self.tasks.task(&task.task_id).cloned() else {
            return false;
        };
        matches!(
            self.tasks.add(record, true),
            AddOutcome::Added { .. } | AddOutcome::Duplicate
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    use gd3_core::task::{TaskRecord, TaskStatus};
    use serde_json::json;

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("gd3-ui-engine-{}-{tag}-{seq}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    /// 造一条记录：只写 6 个必填字段，其余走 serde 缺省。
    fn record(task_id: &str, status: TaskStatus, file_size: i64) -> TaskRecord {
        serde_json::from_value(json!({
            "name": format!("{task_id}.bin"),
            "url": format!("https://example.com/{task_id}"),
            "packId": task_id,
            "taskId": task_id,
            "status": status,
            "fileSize": file_size,
        }))
        .expect("造记录")
    }

    fn line(record: &TaskRecord) -> String {
        serde_json::to_string(record).expect("序列化记录")
    }

    fn config_with(pairs: &[(&str, serde_json::Value)]) -> UserConfig {
        let mut config = settings::default_config();
        for (attr, value) in pairs {
            settings::write(&mut config, attr, value).expect("已知 attr");
        }
        config
    }

    #[test]
    fn construction_order_mirrors_python_engine() {
        let engine = Engine::new(&settings::default_config(), "/nonexistent/tasks.jsonl");
        let expected = strings(&[
            "construct:CategoryService",
            "construct:SpeedMeter",
            "connect:taskService <- categoryService, speedMeter, fileWatcher, \
             deleteRecoverably, nameConflictChoice",
            "construct:TaskService",
            "connect:nameConflictQueue <- taskService.probeConflict, taskService.add, post",
            "construct:NameConflictQueue",
            "connect:taskService.nameConflicted -> nameConflictQueue.add",
            "construct:RuntimeStatusService",
            "connect:featureService <- taskService, categoryService, runtimeStatusService",
            "construct:FeatureService",
            "connect:browserService <- taskService, speedMeter.speedChanged, \
             featureService.parse, loadCrx, extensionFolder",
            "construct:BrowserService",
            "construct:browserListener",
            "connect:aria2RpcService <- featureService.parse, taskService.add",
            "construct:Aria2RpcService",
            "construct:aria2Listener",
        ]);
        assert_eq!(engine.trace(), expected.as_slice());
        assert_eq!(engine.tasks_path(), Path::new("/nonexistent/tasks.jsonl"));
    }

    #[test]
    fn start_order_mirrors_python() {
        let dir = temp_dir("start-order");
        let mut engine = Engine::new(&settings::default_config(), dir.join("tasks.jsonl"));
        let before = engine.trace().len();

        let outcome = engine.start().expect("start");

        let expected = strings(&[
            "start:loadSaved",
            "start:resumeSaved",
            "start:featureService.activate",
            "start:browserListener.start",
            "start:aria2Listener.start",
        ]);
        assert_eq!(&engine.trace()[before..], expected.as_slice());
        assert_eq!(outcome, StartOutcome::default());
        assert!(engine.browser_listener().is_running());
        assert!(engine.aria2_listener().is_running());
    }

    #[test]
    fn start_loads_tasks_and_requeues() {
        let dir = temp_dir("start-load");
        let path = dir.join("tasks.jsonl");
        let text = [
            line(&record("a", TaskStatus::Waiting, 10)),
            line(&record("b", TaskStatus::Running, 20)),
            line(&record("c", TaskStatus::Paused, 30)),
            "not json".to_string(),
        ]
        .join("\n")
            + "\n";
        std::fs::write(&path, text).expect("写 tasks.jsonl");

        let mut engine = Engine::new(&settings::default_config(), &path);
        let outcome = engine.start().expect("start");

        assert_eq!(outcome.load_failures.len(), 1);
        assert_eq!(outcome.resumed, strings(&["a", "b"]));
        assert_eq!(engine.tasks().len(), 3);
        // RUNNING 也会被 resumeSaved 改回 WAITING（Python 的 setStatus）。
        assert_eq!(
            engine.tasks().task("a").expect("a").status,
            TaskStatus::Waiting
        );
        assert_eq!(
            engine.tasks().task("b").expect("b").status,
            TaskStatus::Waiting
        );
        assert_eq!(
            engine.tasks().task("c").expect("c").status,
            TaskStatus::Paused
        );
        assert_eq!(engine.tasks().waiting_order(), strings(&["a", "b"]));
        // 启动不 pump（第 82 步结论）：没有任何任务被派发。
        assert_eq!(engine.tasks().running_count(), 0);
    }

    #[test]
    fn start_missing_tasks_file_is_empty_not_error() {
        let dir = temp_dir("start-missing");
        let path = dir.join("tasks.jsonl");
        let mut engine = Engine::new(&settings::default_config(), &path);

        let outcome = engine.start().expect("缺失文件不算错");

        assert_eq!(outcome, StartOutcome::default());
        assert!(engine.tasks().is_loaded());
        assert!(engine.tasks().is_empty());
        assert!(!path.exists(), "启动不写盘");
    }

    #[test]
    fn stop_order_mirrors_python_and_flushes_twice() {
        let dir = temp_dir("stop-order");
        let path = dir.join("tasks.jsonl");
        std::fs::write(&path, line(&record("a", TaskStatus::Running, 10)) + "\n").expect("写盘");
        let mut engine = Engine::new(&settings::default_config(), &path);
        engine.start().expect("start");
        // 让速度表真的跑起来（`pump` 派发后运行队列非空），这样 `stop()` 的
        // cancel / emit 两个效果才有东西可发。
        assert_eq!(engine.pump(), vec!["a".to_string()]);
        let before = engine.trace().len();

        engine.stop().expect("stop");

        let expected = strings(&[
            "stop:taskService.stop",
            "stop:flush#1",
            "stop:speedMeter.stop",
            "speed:cancel:speed-1",
            "speed:emit:0",
            "stop:browserListener.stop",
            "stop:aria2Listener.stop",
            "stop:featureService.deactivate",
            "stop:flush#2",
        ]);
        assert_eq!(&engine.trace()[before..], expected.as_slice());
        assert!(!engine.speed_meter().is_running());
        assert!(!engine.browser_listener().is_running());
        assert!(!engine.aria2_listener().is_running());
        // 第一次 flush 真写了；第二次因脏标记已清而短路（Python 两次都写）。
        assert!(!engine.tasks().pending_flush());
        let text = std::fs::read_to_string(&path).expect("读回");
        assert!(text.contains("\"PAUSED\""), "{text}");
    }

    #[test]
    fn pump_records_the_speed_meter_start_in_the_trace() {
        let dir = temp_dir("pump-speed");
        let path = dir.join("tasks.jsonl");
        std::fs::write(&path, line(&record("a", TaskStatus::Waiting, 10)) + "\n").expect("写盘");
        let mut engine = Engine::new(&settings::default_config(), &path);
        engine.start().expect("start");
        let before = engine.trace().len();

        let dispatched = engine.pump();

        assert_eq!(dispatched, vec!["a".to_string()]);
        assert_eq!(
            &engine.trace()[before..],
            strings(&["speed:start:speed-1"]).as_slice()
        );
        assert!(engine.speed_meter().is_running());
        // 效果已经被 drain 取走了，缓冲是空的。
        assert!(engine.drain_speed_effects().is_empty());
    }

    #[test]
    fn draining_an_idle_speed_meter_yields_nothing() {
        let dir = temp_dir("drain-idle");
        let mut engine = Engine::new(&settings::default_config(), dir.join("tasks.jsonl"));
        engine.start().expect("start");
        let before = engine.trace().len();

        assert!(engine.drain_speed_effects().is_empty());

        assert_eq!(engine.trace().len(), before, "空闲时不记标签");
    }

    #[test]
    fn stop_pauses_running_and_waiting_only() {
        let dir = temp_dir("stop-status");
        let path = dir.join("tasks.jsonl");
        let config = config_with(&[("maxTaskNum", json!(1))]);
        let mut engine = Engine::new(&config, &path);
        engine
            .tasks_mut()
            .add(record("done", TaskStatus::Completed, 5), false);
        engine
            .tasks_mut()
            .add(record("run", TaskStatus::Waiting, 5), true);
        engine
            .tasks_mut()
            .add(record("wait", TaskStatus::Waiting, 5), true);
        // 走 pump 派发：`run` 从等待队列弹出并进入 running。
        assert_eq!(engine.pump(), strings(&["run"]));
        assert_eq!(engine.tasks().running_count(), 1);

        engine.stop().expect("服务未 load，flush 短路");

        assert_eq!(
            engine.tasks().task("done").expect("done").status,
            TaskStatus::Completed
        );
        assert_eq!(
            engine.tasks().task("run").expect("run").status,
            TaskStatus::Paused
        );
        assert_eq!(
            engine.tasks().task("wait").expect("wait").status,
            TaskStatus::Paused
        );
        // 队列不动是保留的缺陷：running_count 仍是 1，等待队列里还有 wait。
        assert_eq!(engine.tasks().running_count(), 1);
        assert_eq!(engine.tasks().waiting_order(), strings(&["wait"]));
        assert!(!path.exists(), "未 load 的服务不该落盘");
    }

    #[test]
    fn stop_flush_persists_paused_status() {
        let dir = temp_dir("stop-flush");
        let path = dir.join("tasks.jsonl");
        std::fs::write(&path, line(&record("a", TaskStatus::Waiting, 10)) + "\n").expect("写盘");
        let mut engine = Engine::new(&settings::default_config(), &path);
        engine.start().expect("start");
        assert_eq!(
            engine.tasks().task("a").expect("a").status,
            TaskStatus::Waiting
        );

        engine.stop().expect("stop");

        let text = std::fs::read_to_string(&path).expect("读回");
        // 落盘格式是 `json.dumps` 口径（`", "` / `": "`），见 `config::write_indented` 同源实现。
        assert!(text.contains("\"taskId\": \"a\""), "{text}");
        assert!(text.contains("\"PAUSED\""), "{text}");
        assert!(!text.contains("\"WAITING\""), "{text}");
    }

    #[test]
    fn pump_uses_max_task_num_from_config() {
        let dir = temp_dir("pump");
        let config = config_with(&[("maxTaskNum", json!(1))]);
        let mut engine = Engine::new(&config, dir.join("tasks.jsonl"));
        for id in ["a", "b", "c"] {
            engine
                .tasks_mut()
                .add(record(id, TaskStatus::Waiting, 10), true);
        }

        assert_eq!(engine.max_task_num(), 1);
        assert_eq!(engine.pump(), strings(&["a"]));
        assert_eq!(engine.tasks().running_count(), 1);
        assert_eq!(engine.tasks().waiting_order(), strings(&["b", "c"]));
    }

    #[test]
    fn name_conflict_choice_reads_config_live() {
        let dir = temp_dir("choice");
        let mut engine = Engine::new(&settings::default_config(), dir.join("tasks.jsonl"));
        // `cfg.py` 缺省 `keepBoth`。
        assert_eq!(engine.name_conflict_choice(), NameConflictChoice::KeepBoth);

        settings::write(engine.config_mut(), "nameConflict", &json!("ask")).expect("已知 attr");
        assert_eq!(engine.name_conflict_choice(), NameConflictChoice::Ask);

        settings::write(engine.config_mut(), "nameConflict", &json!("overwrite"))
            .expect("已知 attr");
        assert_eq!(
            engine.settings().name_conflict,
            NameConflictChoice::Overwrite
        );
    }

    #[test]
    fn engine_reads_listener_settings_from_config() {
        let dir = temp_dir("listeners");
        let config = config_with(&[
            ("isBrowserExtensionEnabled", json!(true)),
            ("browserExtensionPort", json!(12345)),
            ("isAria2RpcEnabled", json!(false)),
            ("aria2RpcPort", json!(17000)),
        ]);
        let engine = Engine::new(&config, dir.join("tasks.jsonl"));

        assert_eq!(
            engine.browser_listener().settings(),
            ListenerSettings {
                enabled: true,
                port: 12345,
            }
        );
        assert_eq!(
            engine.aria2_listener().settings(),
            ListenerSettings {
                enabled: false,
                port: 17000,
            }
        );
    }

    #[test]
    fn listener_factories_switch_on_backend_flag() {
        let settings = ListenerSettings {
            enabled: true,
            port: 1080,
        };
        assert_eq!(
            create_browser_listener(settings, true).backend(),
            ListenerBackend::Rust
        );
        assert_eq!(
            create_browser_listener(settings, false).backend(),
            ListenerBackend::PortListener
        );
        assert_eq!(
            create_aria2_listener(settings, true).backend(),
            ListenerBackend::Rust
        );
        assert_eq!(
            create_aria2_listener(settings, false).backend(),
            ListenerBackend::PortListener
        );
    }

    #[test]
    fn flag_is_on_matches_python_environ_get() {
        assert!(flag_is_on(Some("1")));
        for value in [
            Some("true"),
            Some("0"),
            Some("01"),
            Some(""),
            Some("1 "),
            None,
        ] {
            assert!(!flag_is_on(value), "{value:?} 不该算开启");
        }
    }

    #[test]
    fn name_conflicted_enqueues_and_reevaluates() {
        let dir = temp_dir("conflict");
        let mut engine = Engine::new(&settings::default_config(), dir.join("tasks.jsonl"));
        engine
            .tasks_mut()
            .add(record("a", TaskStatus::Waiting, 10), false);

        let events = engine.on_name_conflicted(ConflictTask::new("a", 10));
        assert_eq!(events, vec![ConflictEvent::RefreshPosted]);

        // `_post(self._refresh)` 的落点：probe 恒 None，直接按 keepBoth 放行。
        let events = engine.refresh_conflicts();
        assert_eq!(events, vec![ConflictEvent::ConflictChanged(None)]);
        assert!(engine.name_conflicts().conflict().is_none());
        assert!(
            engine
                .trace()
                .contains(&"nameConflictQueue.add(a, keepBoth)".to_string()),
            "{:?}",
            engine.trace()
        );
    }

    #[test]
    fn apply_dialog_outcome_without_conflict_is_noop() {
        let dir = temp_dir("dialog-noop");
        let mut engine = Engine::new(&settings::default_config(), dir.join("tasks.jsonl"));
        let before = engine.trace().len();

        let events = engine.apply_dialog_outcome(&DialogOutcome::Cancel {
            task_id: "a".to_string(),
        });
        assert!(events.is_empty());
        let events = engine.apply_dialog_outcome(&DialogOutcome::SetChoice {
            task_id: "a".to_string(),
            choice: NameConflictChoice::Overwrite,
            is_applied_to_rest: false,
        });
        assert!(events.is_empty());
        // 队列里没有 conflict → 两个分支都直接 return，没有 `nameConflictQueue.*` 调用。
        assert_eq!(engine.trace().len(), before);
    }

    #[test]
    fn category_service_loads_rules_from_config() {
        let engine = Engine::new(&settings::default_config(), "/nonexistent/tasks.jsonl");
        // `categoryRules` 缺省 `"[]"` → `parse_rules` 回落八条预设。
        assert_eq!(engine.category().rules().len(), 8);

        let config = config_with(&[(
            "categoryRules",
            json!([{
                "categoryId": "cat_x",
                "name": "X",
                "icon": "TAG",
                "folder": null,
                "extensions": ["x"],
            }]),
        )]);
        let engine = Engine::new(&config, "/nonexistent/tasks.jsonl");
        assert_eq!(engine.category().rules().len(), 1);
        assert_eq!(engine.category().rules()[0].name, "X");
    }
}
