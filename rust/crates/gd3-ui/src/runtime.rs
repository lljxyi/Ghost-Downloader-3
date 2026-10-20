//! Phase 8e 的**运行期宿主**：把 [`TaskRunner`] 接到窗口上。
//!
//! [`crate::task_runner`] 是纯状态机 + 工作线程；「谁按 tick 推它、谁把结果落盘、
//! 谁刷 UI」是这一层的事。Python 里这些是信号驱动的：
//!
//! | Python | 这里 |
//! |---|---|
//! | `cfg.maxTaskNum.valueChanged -> _rebalance` | 每 tick 现读 [`RunnerSettings::from_config`] |
//! | `TaskService.taskAdded/taskCompleted/taskFailed -> 卡片重画` | `report.changed / progressed` → [`TaskService::refresh`] |
//! | `TaskStore._flushSoon` 的 QTimer | `report.changed` → [`TaskService::flush`] |
//! | `speedMeter.speedChanged -> 顶栏` | 1 秒一次 `tick_speed_meter` + [`Engine::drain_speed_effects`] |
//!
//! # 为什么是轮询
//!
//! Slint 的 callback 只连到我们自己的闭包，没有「服务层属性变化信号」这一层
//! （与 [`crate::settings_bridge`] 同样的结论）。所以运行期用
//! [`slint::Timer`] 按 [`RUNNER_TICK_INTERVAL`]（100ms）收口一次：
//!
//! ```text
//! tick = 现读设置 → pump（派发等待队列）→ drain（收工作线程事件）
//!        → 每秒一次速度表 → 有改动才落盘 → 刷任务列表
//! ```
//!
//! 无头模式（`--headless`）没有窗口事件循环，`slint::Timer` 不会自己走，
//! 所以 [`Runtime::run_until_idle`] 手动按同样的间隔调 [`Runtime::tick`]。
//!
//! # 与 Python 的差异
//!
//! - **`.ghd` 由引擎自己维护**（`gd3-net`），这里只管「整任务完成后删」（在
//!   [`crate::task_runner`] 里）；
//! - **速度表按 tick 判 1 秒**，而不是每个任务一个协程定时器；`Emit` 的数字进
//!   [`RunnerTick::speed`]，宿主要显示时读它（Python 是信号直连顶栏）。
//! - **tick 之后可以挂一个钩子**（[`Runtime::set_tick_hook`]）：托盘提示气泡就是
//!   这样接上的（`tray.py:201-205` 的 `speedChanged -> setToolTip`）。Python 是信号
//!   直连，这里没有服务层信号，只能借 tick 这个既有的收口点。

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::engine::Engine;
use crate::task_runner::{RunnerSettings, TaskRunner};
use crate::task_service::{PauseReport, TaskService};
use slint::ComponentHandle;

use crate::AppWindow;

/// 运行期轮询间隔（Python 由信号驱动，这里最多晚这么久）。
pub const RUNNER_TICK_INTERVAL: Duration = Duration::from_millis(100);

/// 速度表的 tick 周期（`speed_meter.py` 的 1 秒）。
pub const SPEED_TICK_INTERVAL: Duration = Duration::from_secs(1);

/// 一次 [`Runtime::tick`] 的产物（无头冒烟与测试据此断言）。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RunnerTick {
    /// 这一轮派发出去的任务数（`taskStarted` 的次数）。
    pub started: usize,
    /// 这一轮收到进度的任务数。
    pub progressed: usize,
    /// 这一轮完成的任务 id。
    pub finished: Vec<String>,
    /// 这一轮失败的任务与**已经格式化**的错误文案。
    pub failed: Vec<(String, String)>,
    /// 真的写了 `tasks.jsonl`（有改动才写）。
    pub flushed: bool,
    /// 落盘失败（Python 会在这里抛 `OSError`）。
    pub flush_error: Option<String>,
    /// 这一轮速度表发布的数字（`speedChanged.emit` 的落点）。
    pub speed: Option<i64>,
}

/// 一次 tick 的全部动作（定时器闭包与无头循环共用同一个实现）。
fn tick_host(
    engine: &Rc<RefCell<Engine>>,
    runner: &Rc<RefCell<TaskRunner>>,
    window: &AppWindow,
    last_speed_tick: &Cell<Instant>,
    speed_tick_interval: Duration,
) -> RunnerTick {
    // 先把 Rc 复制出来：下面要同时借用 engine 与 runner，不能再借 `self`。
    let engine_rc = Rc::clone(engine);
    let runner_rc = Rc::clone(runner);
    let mut engine = engine_rc.borrow_mut();
    let mut runner = runner_rc.borrow_mut();

    // `cfg` 现读（Python 每个 `_pump` / 每个数据块都重读，设置页改完立刻生效）。
    runner.set_settings(RunnerSettings::from_config(engine.config()));
    let tasks_path = engine.tasks_path().to_path_buf();

    // 先 pump（派发等待队列）再 drain（收工作线程事件）—— 顺序与 Python 的
    // `_schedule` → `_dispatch` 一致。`started` 是两者之和：pump 报的是这一轮
    // 新派发出去的，drain 报的是上一轮派发后立刻回来「已开始」的。
    let dispatched = runner.pump(engine.tasks_mut());
    let report = runner.drain(engine.tasks_mut());
    let mut tick = RunnerTick {
        started: dispatched.len() + report.started.len(),
        progressed: report.progressed.len(),
        finished: report.finished.clone(),
        failed: report
            .failed
            .iter()
            .map(|(task_id, error)| (task_id.clone(), error.format()))
            .collect(),
        ..RunnerTick::default()
    };

    if last_speed_tick.get().elapsed() >= speed_tick_interval {
        last_speed_tick.set(Instant::now());
        engine.tasks_mut().tick_speed_meter();
        // 限速闸门的计数窗口与速度表对齐成同一个 1 秒节拍（`SpeedMeter._tick` 里
        // `self._bytes = 0`）。闸门自己也会滚窗口，这里对齐只为让「放行时刻」
        // 与顶栏速度表同一拍。
        runner.speed_limit().reset();
    }
    for effect in engine.drain_speed_effects() {
        if let crate::speed_meter::SpeedEffect::Emit { speed } = effect {
            tick.speed = Some(speed);
        }
    }

    // 只有「真的改了东西」才落盘（Python 的 `_flushSoon` 只出现在
    // `_onRunDone` / `_onRunFailed` / `pause` 之类的收尾处，进度不算）。
    if report.changed {
        match engine.tasks_mut().flush(&tasks_path) {
            Ok(written) => tick.flushed = written,
            Err(err) => tick.flush_error = Some(err.to_string()),
        }
    }

    // 进度 / 完成 / 失败都要重画列表（Python 靠信号逐个刷）。
    if report.changed || !report.progressed.is_empty() {
        engine.tasks().refresh(window);
    }
    tick
}

/// [`start_all`] / [`pause_all`] 的产物。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AllReport {
    /// 真的派发出去（起了工作线程）的任务 id。
    pub started: Vec<String>,
    /// 被置成 `PAUSED` 的任务 id（只 [`pause_all`] 会填）。
    pub paused: Vec<String>,
}

/// `startAll()`（`task_service.py:530-534`）：托盘「全部开始」的落点。
///
/// 两层配合，缺一不可：
///
/// 1. 服务层 [`TaskService::start_all`] 只把 `PAUSED` / `WAITING` / `FAILED` 挪进
///    等待队列（Python 的 `_schedule` + `_pump`），**不建 job、不起线程**；
/// 2. 真正的「建 job + 起工作线程」在运行器（[`TaskRunner::dispatch`]），与每个
///    tick 的 `pump` 同一份实现。
///
/// 所以这里不能只调服务层：那样队列里会多出一批「占着槽位、没人干活」的任务
/// （槽位由 `_pump` 的 `queue.run(id, "")` 占上，之后 `tasks.pump` 不会再挑它们）。
pub fn start_all(engine: &Rc<RefCell<Engine>>, runner: &Rc<RefCell<TaskRunner>>) -> AllReport {
    // 先克隆 Rc：下面要同时借 engine 与 runner，不能再借 `engine` / `runner` 形参。
    let engine_rc = Rc::clone(engine);
    let runner_rc = Rc::clone(runner);
    let mut engine = engine_rc.borrow_mut();
    let mut runner = runner_rc.borrow_mut();
    let max_task_num = engine.max_task_num();
    let mut report = AllReport::default();
    for task_id in engine.tasks_mut().start_all(max_task_num) {
        if runner.dispatch(engine.tasks_mut(), &task_id) {
            report.started.push(task_id);
        }
    }
    report
}

/// `pauseAll()`（`task_service.py:536-539`）：托盘「全部暂停」的落点。
///
/// 与 [`start_all`] 同样的两层：先快照「运行中或等待中」的 id（Python 的
/// `list(self._store.tasks.values())`，否则 `pause` 里的 `_pump` 会改动正在遍历的
/// 队列），再逐个走 [`TaskRunner::pause`]（**先取消工作线程**，再服务层记账）。
/// 服务层 `pause` 顺带 `_pump` 出来的新 id 同样要过运行器，否则占槽不干活。
pub fn pause_all(engine: &Rc<RefCell<Engine>>, runner: &Rc<RefCell<TaskRunner>>) -> AllReport {
    let engine_rc = Rc::clone(engine);
    let runner_rc = Rc::clone(runner);
    let mut engine = engine_rc.borrow_mut();
    let mut runner = runner_rc.borrow_mut();
    let max_task_num = engine.max_task_num();
    let ids: Vec<String> = {
        let tasks = engine.tasks();
        tasks
            .records()
            .iter()
            .filter(|record| tasks.is_running(&record.task_id) || tasks.is_waiting(&record.task_id))
            .map(|record| record.task_id.clone())
            .collect()
    };
    let mut report = AllReport::default();
    for task_id in ids {
        let Some(paused) = runner.pause(engine.tasks_mut(), &task_id, max_task_num) else {
            continue;
        };
        report.paused.push(task_id);
        for started in paused.started {
            if runner.dispatch(engine.tasks_mut(), &started) {
                report.started.push(started);
            }
        }
    }
    report
}

/// 一次 tick 结束后调用的钩子（[`Runtime::set_tick_hook`]）。
pub type TickHook = Rc<dyn Fn(&RunnerTick)>;

/// 运行期宿主：引擎 + 运行器 + 窗口 + 一个 tick 定时器。
pub struct Runtime {
    engine: Rc<RefCell<Engine>>,
    runner: Rc<RefCell<TaskRunner>>,
    window: AppWindow,
    tick_interval: Duration,
    speed_tick_interval: Duration,
    last_speed_tick: Rc<Cell<Instant>>,
    /// `Some` 表示定时器在跑；[`Runtime::shutdown`] 之后是 `None`。
    timer: Option<slint::Timer>,
    /// 每次 tick 之后的回调（托盘提示气泡）。`Rc<RefCell<_>>` 是因为定时器闭包里
    /// 只拿得到一份共享句柄，而 `set_tick_hook` 在闭包外面。
    tick_hook: Rc<RefCell<Option<TickHook>>>,
}

impl Runtime {
    pub fn new(
        engine: Rc<RefCell<Engine>>,
        runner: Rc<RefCell<TaskRunner>>,
        window: AppWindow,
    ) -> Runtime {
        Runtime {
            engine,
            runner,
            window,
            tick_interval: RUNNER_TICK_INTERVAL,
            speed_tick_interval: SPEED_TICK_INTERVAL,
            last_speed_tick: Rc::new(Cell::new(Instant::now())),
            timer: None,
            tick_hook: Rc::new(RefCell::new(None)),
        }
    }

    pub fn engine(&self) -> &Rc<RefCell<Engine>> {
        &self.engine
    }

    pub fn runner(&self) -> &Rc<RefCell<TaskRunner>> {
        &self.runner
    }

    pub fn window(&self) -> &AppWindow {
        &self.window
    }

    /// 自定 tick 间隔（测试用；生产走 [`RUNNER_TICK_INTERVAL`]）。
    pub fn set_tick_interval(&mut self, interval: Duration) {
        self.tick_interval = interval;
    }

    /// 自定速度表周期（测试用）。
    pub fn set_speed_tick_interval(&mut self, interval: Duration) {
        self.speed_tick_interval = interval;
    }

    /// 挂一个「每次 tick 之后」的回调（托盘提示气泡用）。
    ///
    /// 无头 `--until-idle` 走的是 [`Runtime::tick`]，有头走定时器闭包 —— 两条路径都
    /// 会调它，语义一致。回调在 tick **之后**跑，所以能读到这一轮的速度表数字。
    pub fn set_tick_hook(&mut self, hook: TickHook) {
        *self.tick_hook.borrow_mut() = Some(hook);
    }

    /// 摘掉 tick 钩子（托盘销毁时用；没挂过也没事）。
    pub fn clear_tick_hook(&mut self) {
        *self.tick_hook.borrow_mut() = None;
    }

    /// 装 tick 定时器（窗口事件循环驱动它）。
    pub fn install(&mut self) {
        let engine = Rc::clone(&self.engine);
        let runner = Rc::clone(&self.runner);
        let window = self.window.clone_strong();
        let last_speed_tick = Rc::clone(&self.last_speed_tick);
        let speed_tick_interval = self.speed_tick_interval;
        let tick_hook = Rc::clone(&self.tick_hook);
        let timer = slint::Timer::default();
        timer.start(slint::TimerMode::Repeated, self.tick_interval, move || {
            let tick = tick_host(
                &engine,
                &runner,
                &window,
                &last_speed_tick,
                speed_tick_interval,
            );
            run_tick_hook(&tick_hook, &tick);
        });
        self.timer = Some(timer);
    }

    pub fn is_ticking(&self) -> bool {
        self.timer.is_some()
    }

    /// 一次 tick（与定时器回调完全同一份实现）。
    pub fn tick(&self) -> RunnerTick {
        let tick = tick_host(
            &self.engine,
            &self.runner,
            &self.window,
            &self.last_speed_tick,
            self.speed_tick_interval,
        );
        run_tick_hook(&self.tick_hook, &tick);
        tick
    }

    /// 没有任务在跑（等价于 Python 的 `queue.runningCount() == 0`）。
    pub fn is_idle(&self) -> bool {
        self.runner.borrow().running_ids().is_empty()
    }

    /// `pause(task)`：先取消工作线程，再走服务层的记账（顺序见
    /// [`TaskRunner::pause`]）。
    pub fn pause(&self, task_id: &str) -> Option<PauseReport> {
        let engine = Rc::clone(&self.engine);
        let runner = Rc::clone(&self.runner);
        let mut engine = engine.borrow_mut();
        let mut runner = runner.borrow_mut();
        let max_task_num = runner.settings().max_task_num;
        runner.pause(engine.tasks_mut(), task_id, max_task_num)
    }

    /// `start(task)`（`task_service.py:392`）：入队，派发交给下一次 tick 的 `_pump`。
    pub fn start(&self, task_id: &str) -> bool {
        self.engine.borrow_mut().tasks_mut().start(task_id)
    }

    /// [`start_all`] 的方法形态（托盘「全部开始」）。
    pub fn start_all(&self) -> AllReport {
        start_all(&self.engine, &self.runner)
    }

    /// [`pause_all`] 的方法形态（托盘「全部暂停」）。
    pub fn pause_all(&self) -> AllReport {
        pause_all(&self.engine, &self.runner)
    }

    /// 一直 tick 到没有任务在跑，或到 `timeout`。返回 tick 次数。
    ///
    /// 无头模式没有窗口事件循环，`slint::Timer` 不会自己走，所以这里手动推进
    /// 定时器与重绘请求（`settings_bridge` 的无头补拍同理）。
    pub fn run_until_idle(&self, timeout: Duration) -> u32 {
        let deadline = Instant::now() + timeout;
        let mut ticks = 0u32;
        loop {
            self.tick();
            slint::platform::update_timers_and_animations();
            self.window.window().request_redraw();
            ticks += 1;
            if self.is_idle() || Instant::now() >= deadline {
                return ticks;
            }
            std::thread::sleep(self.tick_interval);
        }
    }

    /// 停机：停定时器 + 取消在跑的工作线程（`.ghd` 保住），返回没在 `wait`
    /// 内收工的任务 id。
    pub fn shutdown(&mut self, wait: Duration) -> Vec<String> {
        self.timer = None;
        self.runner.borrow_mut().shutdown(wait)
    }

    /// 引擎的共享句柄（宿主做停机收尾 / 无头摘要时用）。
    pub fn shared_engine(&self) -> Rc<RefCell<Engine>> {
        Rc::clone(&self.engine)
    }

    /// 把当前记录推给窗口（启动时一次；之后每个有改动的 tick 自己会重推）。
    ///
    /// 走 [`TaskService::refresh`] 而不是自己调 `push_task_records`：前者会填
    /// `isSeeding` / `canSeed` / `hasOutputFile` 这几个运行期事实。
    pub fn refresh(&self) {
        self.engine.borrow().tasks().refresh(&self.window);
    }
}

/// 调一次 tick 钩子。先把 `Rc` 克隆出来再调：回调里若想再挂/摘钩子也不会撞上
/// `RefCell` 的借用（`set_tick_hook` 要 `&mut Runtime`，正常路径上也进不来）。
fn run_tick_hook(hook: &RefCell<Option<TickHook>>, tick: &RunnerTick) {
    let hook = hook.borrow().clone();
    if let Some(hook) = hook {
        hook(tick);
    }
}

/// 让 `TaskService` 在新旧代码之间都能被 `Runtime` 拿出来（可读性用）。
#[allow(dead_code)]
fn as_service(engine: &Engine) -> &TaskService {
    engine.tasks()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::mpsc::Sender;

    use gd3_core::settings;
    use gd3_core::task::{Progress, TaskRecord, TaskStatus, TaskStep};
    use gd3_core::task_error::TaskError;
    use serde_json::{Map, Value, json};

    use slint::Model as _;

    use crate::task_runner::{DownloadJob, RunnerEvent, Spawner};
    use gd3_net::CancellationToken;

    #[derive(Default)]
    struct FakeState {
        jobs: RefCell<Vec<DownloadJob>>,
        cancels: RefCell<Vec<CancellationToken>>,
        sink: RefCell<Option<Sender<RunnerEvent>>>,
        gates: RefCell<Vec<Arc<crate::speed_limit::SpeedLimit>>>,
    }

    #[derive(Default, Clone)]
    struct FakeSpawner(Rc<FakeState>);

    impl FakeSpawner {
        fn last_work_id(&self) -> String {
            self.0
                .jobs
                .borrow()
                .last()
                .map(|job| job.work_id.clone())
                .expect("没有派发过 job")
        }

        fn jobs(&self) -> usize {
            self.0.jobs.borrow().len()
        }

        fn cancel_token(&self, index: usize) -> CancellationToken {
            self.0.cancels.borrow()[index].clone()
        }

        fn send(&self, event: RunnerEvent) {
            let sink = self.0.sink.borrow().clone().expect("还没有 sink");
            sink.send(event).expect("发事件");
        }
    }

    impl Spawner for FakeSpawner {
        fn spawn(
            &self,
            job: DownloadJob,
            cancel: CancellationToken,
            sink: Sender<RunnerEvent>,
            gate: Arc<crate::speed_limit::SpeedLimit>,
        ) {
            self.0.jobs.borrow_mut().push(job);
            self.0.cancels.borrow_mut().push(cancel);
            *self.0.sink.borrow_mut() = Some(sink);
            self.0.gates.borrow_mut().push(gate);
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("gd3-ui-runtime-{}-{tag}-{seq}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    fn record(
        task_id: &str,
        status: TaskStatus,
        url: &str,
        folder: &std::path::Path,
    ) -> TaskRecord {
        let mut extra = Map::new();
        extra.insert("url".to_string(), Value::String(url.to_string()));
        extra.insert("fileSize".to_string(), json!(0));
        let step = TaskStep {
            step_index: 0,
            file_index: None,
            step_id: "step-0".to_string(),
            status: TaskStatus::Waiting,
            progress: Progress::Int(0),
            received_bytes: 0,
            speed: 0,
            extra,
            type_name: None,
        };
        TaskRecord {
            name: format!("{task_id}.bin"),
            url: url.to_string(),
            pack_id: "http".to_string(),
            task_id: task_id.to_string(),
            status,
            steps: vec![step],
            created_at: 0,
            completed_at: 0,
            output_folder: folder.to_string_lossy().to_string(),
            file_size: 0,
            files: None,
            category: None,
            should_seed: false,
            checksums: Map::new(),
            extra: Map::new(),
            type_name: None,
        }
    }

    /// 造一套运行期：临时数据目录 + `tasks.jsonl` + 假 spawner。
    ///
    /// 缺省 `auto_start = true`：每条记录都会被塞进等待队列（`add` 的 `_schedule`），
    /// 与记录自己的 `status` 无关 —— 想复现「库里躺着一条完成的任务」得用
    /// [`harness_with`] 并关掉它。
    fn harness(tag: &str, tasks: &[(&str, TaskStatus)]) -> (Runtime, FakeSpawner, PathBuf) {
        harness_with(tag, tasks, true)
    }

    /// 同上，可选「入队时是否自动进等待队列」。
    fn harness_with(
        tag: &str,
        tasks: &[(&str, TaskStatus)],
        auto_start: bool,
    ) -> (Runtime, FakeSpawner, PathBuf) {
        let dir = temp_dir(tag);
        let tasks_path = dir.join("tasks.jsonl");
        let mut config = settings::default_config_with_download_folder(&dir.to_string_lossy());
        let mut engine = Engine::new(&config, &tasks_path);
        engine.start().expect("装载任务列表");
        for (task_id, status) in tasks {
            let url = format!("https://example.com/{task_id}.bin");
            let _ = engine
                .tasks_mut()
                .add(record(task_id, *status, &url, &dir), auto_start);
        }
        config = engine.config().clone();
        let spawner = FakeSpawner::default();
        let runner = TaskRunner::with_spawner(RunnerSettings::from_config(&config), {
            Box::new(spawner.clone())
        });
        let window = crate::create_headless();
        let mut runtime = Runtime::new(
            Rc::new(RefCell::new(engine)),
            Rc::new(RefCell::new(runner)),
            window,
        );
        runtime.set_tick_interval(Duration::from_millis(1));
        runtime.set_speed_tick_interval(Duration::from_secs(3600));
        (runtime, spawner, dir)
    }

    fn status_of(runtime: &Runtime, task_id: &str) -> TaskStatus {
        runtime
            .engine()
            .borrow()
            .tasks()
            .task(task_id)
            .map(|record| record.status)
            .expect("任务还在")
    }

    fn step_bytes(runtime: &Runtime, task_id: &str) -> i64 {
        runtime
            .engine()
            .borrow()
            .tasks()
            .task(task_id)
            .expect("任务还在")
            .steps[0]
            .received_bytes
    }

    #[test]
    fn a_tick_pumps_the_waiting_queue_and_spawns_the_worker() {
        let (runtime, spawner, dir) = harness("pump", &[("a", TaskStatus::Waiting)]);

        let tick = runtime.tick();

        assert_eq!(tick.started, 1);
        assert_eq!(spawner.jobs(), 1);
        assert_eq!(status_of(&runtime, "a"), TaskStatus::Running);
        assert!(!runtime.is_idle());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_progress_event_does_not_flush_the_task_list() {
        let (runtime, spawner, dir) = harness("progress", &[("a", TaskStatus::Waiting)]);
        runtime.tick();
        let work_id = spawner.last_work_id();

        spawner.send(RunnerEvent::Progress {
            work_id,
            task_id: "a".to_string(),
            position: 0,
            received_bytes: 1024,
            progress: 25.0,
            speed: 10,
            chunk_delta: 1024,
        });
        let tick = runtime.tick();

        assert_eq!(tick.progressed, 1);
        assert!(!tick.flushed, "进度不落盘");
        assert!(!tick.flush_error.is_some());
        assert_eq!(step_bytes(&runtime, "a"), 1024);
        assert_eq!(
            runtime
                .engine()
                .borrow()
                .tasks()
                .speed_meter()
                .pending_bytes(),
            1024
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_finished_task_is_flushed_to_disk() {
        let (runtime, spawner, dir) = harness("finish", &[("a", TaskStatus::Waiting)]);
        runtime.tick();
        let work_id = spawner.last_work_id();

        spawner.send(RunnerEvent::StepFinished {
            work_id: work_id.clone(),
            task_id: "a".to_string(),
            position: 0,
            can_use_range_requests: true,
            received_bytes: 3,
        });
        spawner.send(RunnerEvent::TaskFinished {
            work_id,
            task_id: "a".to_string(),
        });
        let tick = runtime.tick();

        assert_eq!(tick.finished, vec!["a".to_string()]);
        assert!(tick.flushed, "收尾要落盘");
        assert!(runtime.is_idle());
        let path = runtime.engine().borrow().tasks_path().to_path_buf();
        let text = std::fs::read_to_string(&path).expect("读回 tasks.jsonl");
        assert!(text.contains("\"COMPLETED\""), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_task_reports_the_formatted_error_and_flushes() {
        let (runtime, spawner, dir) = harness("fail", &[("a", TaskStatus::Waiting)]);
        runtime.tick();
        let work_id = spawner.last_work_id();

        spawner.send(RunnerEvent::TaskFailed {
            work_id,
            task_id: "a".to_string(),
            position: Some(0),
            error: TaskError::server_error(503),
        });
        let tick = runtime.tick();

        assert_eq!(tick.failed.len(), 1);
        assert_eq!(tick.failed[0].0, "a");
        assert!(tick.failed[0].1.contains("503"), "{:?}", tick.failed);
        assert!(tick.flushed);
        assert_eq!(status_of(&runtime, "a"), TaskStatus::Failed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pause_cancels_the_worker_and_marks_the_task_paused() {
        let (runtime, spawner, dir) = harness("pause", &[("a", TaskStatus::Waiting)]);
        runtime.tick();

        let report = runtime.pause("a").expect("任务在服务里");

        assert!(report.cancel.work_id.is_some());
        assert!(spawner.cancel_token(0).is_cancelled());
        assert_eq!(status_of(&runtime, "a"), TaskStatus::Paused);
        assert!(runtime.is_idle(), "取消之后运行集合里就没有它了");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_paused_task_can_be_started_again_and_the_next_tick_dispatches_it() {
        let (runtime, spawner, dir) = harness("resume", &[("a", TaskStatus::Waiting)]);
        runtime.tick();
        runtime.pause("a").expect("任务在服务里");

        assert!(runtime.start("a"), "暂停的任务可以重新入队");
        let tick = runtime.tick();

        assert_eq!(tick.started, 1);
        assert_eq!(spawner.jobs(), 2, "第二次派发是新的一条运行");
        assert_eq!(status_of(&runtime, "a"), TaskStatus::Running);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn start_all_dispatches_paused_waiting_and_failed_tasks() {
        let (runtime, spawner, dir) = harness_with(
            "start-all",
            &[
                ("a", TaskStatus::Paused),
                ("b", TaskStatus::Failed),
                ("c", TaskStatus::Completed),
            ],
            false,
        );

        let report = runtime.start_all();

        // 服务层的判据是 `PAUSED` / `WAITING` / `FAILED`（`startAll`），完成的那个不动。
        assert_eq!(report.started, vec!["a".to_string(), "b".to_string()]);
        assert!(report.paused.is_empty());
        assert_eq!(spawner.jobs(), 2, "两个任务都真的起了工作线程");
        assert_eq!(status_of(&runtime, "a"), TaskStatus::Running);
        assert_eq!(status_of(&runtime, "b"), TaskStatus::Running);
        assert_eq!(status_of(&runtime, "c"), TaskStatus::Completed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pause_all_cancels_every_running_worker() {
        let (runtime, spawner, dir) = harness(
            "pause-all",
            &[("a", TaskStatus::Waiting), ("b", TaskStatus::Waiting)],
        );
        let tick = runtime.tick();
        assert_eq!(tick.started, 2);
        assert_eq!(spawner.jobs(), 2);

        let report = runtime.pause_all();

        assert_eq!(report.paused, vec!["a".to_string(), "b".to_string()]);
        assert!(spawner.cancel_token(0).is_cancelled());
        assert!(spawner.cancel_token(1).is_cancelled());
        assert_eq!(status_of(&runtime, "a"), TaskStatus::Paused);
        assert_eq!(status_of(&runtime, "b"), TaskStatus::Paused);
        assert!(runtime.is_idle());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pause_all_leaves_idle_tasks_alone() {
        let (runtime, _spawner, dir) = harness_with(
            "pause-all-idle",
            &[("a", TaskStatus::Completed), ("b", TaskStatus::Paused)],
            false,
        );

        let report = runtime.pause_all();

        assert!(report.paused.is_empty(), "没有在跑/等待的任务就不动");
        assert_eq!(status_of(&runtime, "a"), TaskStatus::Completed);
        assert_eq!(status_of(&runtime, "b"), TaskStatus::Paused);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_tick_hook_runs_after_every_tick() {
        let (mut runtime, _spawner, dir) = harness("hook", &[("a", TaskStatus::Waiting)]);
        // 速度表每秒一次；这里把它压到 0，保证每个 tick 都有数字可读。
        runtime.set_speed_tick_interval(Duration::from_millis(0));
        let calls = Rc::new(Cell::new(0u32));
        let last_speed: Rc<Cell<Option<i64>>> = Rc::new(Cell::new(None));
        let (calls_in_hook, speed_in_hook) = (Rc::clone(&calls), Rc::clone(&last_speed));
        runtime.set_tick_hook(Rc::new(move |tick: &RunnerTick| {
            calls_in_hook.set(calls_in_hook.get() + 1);
            if tick.speed.is_some() {
                speed_in_hook.set(tick.speed);
            }
        }));

        runtime.tick();
        runtime.tick();

        assert_eq!(calls.get(), 2, "每个 tick 之后一次");
        assert_eq!(last_speed.get(), Some(0), "没有在跑的任务，速度是 0");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_pump_limit_follows_the_config() {
        let (runtime, _spawner, dir) = harness(
            "limit",
            &[("a", TaskStatus::Waiting), ("b", TaskStatus::Waiting)],
        );
        settings::write(
            runtime.engine().borrow_mut().config_mut(),
            "maxTaskNum",
            &json!(1),
        )
        .expect("已知 attr");

        let tick = runtime.tick();

        assert_eq!(tick.started, 1, "cfg.maxTaskNum 现读");
        assert_eq!(status_of(&runtime, "b"), TaskStatus::Waiting);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_until_idle_returns_immediately_when_there_is_nothing_to_do() {
        let (runtime, _spawner, dir) = harness("idle", &[]);

        let ticks = runtime.run_until_idle(Duration::from_millis(200));

        assert_eq!(ticks, 1);
        assert!(runtime.is_idle());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_until_idle_gives_up_at_the_deadline() {
        let (runtime, _spawner, dir) = harness("idle-timeout", &[("a", TaskStatus::Waiting)]);

        let ticks = runtime.run_until_idle(Duration::from_millis(30));

        assert!(ticks > 1, "会一直 tick 到超时：{ticks}");
        assert!(!runtime.is_idle(), "假 spawner 不会自己收工");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_speed_meter_ticks_on_its_own_schedule() {
        let (mut runtime, _spawner, dir) = harness("speed", &[("a", TaskStatus::Waiting)]);

        // 第一次 tick 派发任务 → `_refreshSpeedMeter()` 里 `start()` 起表。
        runtime.tick();
        assert!(runtime.engine().borrow().tasks().speed_meter().is_running());

        runtime.set_speed_tick_interval(Duration::from_millis(0));
        let tick = runtime.tick();

        assert_eq!(tick.speed, Some(0), "定时器到点 → 发布一次当前速度");
        assert!(runtime.engine().borrow().tasks().speed_meter().is_running());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_start_and_stop_around_the_timer() {
        let (mut runtime, _spawner, dir) = harness("timer", &[]);

        runtime.install();
        assert!(runtime.is_ticking());
        slint::platform::update_timers_and_animations();

        let stragglers = runtime.shutdown(Duration::from_millis(50));

        assert!(stragglers.is_empty());
        assert!(!runtime.is_ticking(), "停机要摘掉定时器");
        assert!(runtime.runner().borrow().is_shutdown());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refresh_pushes_the_loaded_records_to_the_window() {
        let (runtime, _spawner, dir) = harness("refresh", &[("a", TaskStatus::Completed)]);

        runtime.refresh();

        assert_eq!(
            runtime.window().get_tasks().row_count(),
            1,
            "记录推给窗口了"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_task_list_pushes_no_rows() {
        let (runtime, _spawner, dir) = harness("empty", &[]);
        runtime.refresh();
        assert_eq!(runtime.window().get_tasks().row_count(), 0);
        assert_eq!(as_service(&runtime.engine().borrow()).len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
