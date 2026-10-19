//! 设置页接线：把 [`SettingsWriter`] 装到窗口上，并把写盘后的效果落地。
//!
//! 第 117 步让「动作 → cfg 键」这条路能落盘，但生产路径上 `install_settings_actions`
//! 仍然没人调，写盘之后的「重推 UI / 提示重启 / 提示失败」也没有归属。这一步补上：
//!
//! ```text
//! Slint 回调 → SettingsActions → SettingsWriter::set → 落盘
//!                                       ↓ take_changes()
//!                              SettingsBridge::drain → push_settings（重推）
//!                                                   → restart_attrs（提示重启）
//!                                                   → failures（提示失败）
//!                                                   → deferred（还没接的动作）
//! ```
//!
//! ## 为什么是轮询
//!
//! Python 的 `ConfigItem.valueChanged` 是同步信号：`cfg.set` 里改完值立刻回调，
//! 卡片自己回读 `correct()` 之后的值。Slint 的 callback 只连到我们自己的闭包，
//! 没有「属性变化信号」这一层，所以用 [`slint::Timer`] 按 [`POLL_INTERVAL`]（50ms）
//! 轮询一次。代价是最多一帧的延迟，好处是**一处**收口 —— 不必把 49 个闭包各包一层。
//!
//! ## 宿主的钩子
//!
//! 轮询只负责「取走 + 重推」，**刷新服务 / 提示重启 / 提示失败**由宿主决定
//! （[`SettingsBridge::set_drain_handler`]）。`boot::run` 的处理器做三件事：
//!
//! 1. 把 writer 的配置交给 [`crate::engine::Engine`] 并 `refresh_services()`
//!    （Python 每处现读全局 `cfg`，这里引擎持克隆，必须显式同步）；
//! 2. 把 `refresh_services()` 可能换掉的 `browserExtensionPairToken` 用
//!    [`SettingsWriter::adopt`] 吞回文件（Python 的 `cfg.set` 自己落盘）；
//! 3. 失败 / 待办 / 需重启的属性名打到 stderr。
//!
//! 处理器在**重推之前**调用，所以它改出来的值也能进同一帧的 `push_settings`。
//!
//! ## 定时器不持 `Rc<Self>`
//!
//! 回调里只拿 `Weak<SettingsBridge>` 与 `Weak<AppWindow>`，所以
//! 「bridge → timer → 回调 → bridge」不构成引用环；[`SettingsBridge::stop`] 把 timer
//! drop 掉即可彻底停掉。宿主（`boot::run`）只要保证 `Rc<SettingsBridge>` 活到事件循环
//! 结束。
//!
//! ## 与 Python 的已知差异
//!
//! - **重推是整页**：Python 的 `setting_cards.py:71` 只让**那一张卡**回读自己的值；
//!   这里统一调 [`crate::push_settings`] 推全量 42 项。同一个值再写一次不会触发 Slint
//!   的属性通知，所以正在编辑的输入框不会被重置；真正会变的是 `correct()` 夹紧过的项。
//! - **`deferred` 只记录不执行**：用户点了还没接的按钮（列表写 / 平台副作用）时，
//!   这里把名字交出去，宿主决定 `eprintln!` 还是弹提示。见
//!   [`crate::settings_writer`] 模块头的边界表。

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use crate::config_store::{ConfigChange, ConfigStoreError};
use crate::settings_writer::SettingsWriter;
use slint::ComponentHandle;

use crate::{AppWindow, install_settings_actions, push_settings};

/// 轮询间隔（Python 的 `valueChanged` 是同步的，这里最多晚这么久）。
pub const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// 一次 [`SettingsBridge::drain`] 的结果。
///
/// 字段都是「从 writer 里取走的东西」，所以 drain 之后 writer 那边已经清空。
#[derive(Debug, Default)]
pub struct SettingsDrain {
    /// 已落盘的改动（宿主据此刷新服务）。
    pub changes: Vec<ConfigChange>,
    /// 写盘失败（Python 会在这里抛 `OSError`）。
    pub failures: Vec<ConfigStoreError>,
    /// 还没接的动作名（待办清单）。
    pub deferred: Vec<&'static str>,
    /// 是否重推过 UI。
    pub repushed: bool,
}

impl SettingsDrain {
    /// 什么都没发生。
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
            && self.failures.is_empty()
            && self.deferred.is_empty()
            && !self.repushed
    }

    /// 需要提示「重启后生效」的属性名（`restart=True` 的三项）。
    pub fn restart_attrs(&self) -> Vec<&'static str> {
        self.changes
            .iter()
            .filter(|change| change.restart)
            .map(|change| change.attr)
            .collect()
    }
}

/// 每次 [`SettingsBridge::drain`] 取到东西之后交给宿主的回调。
type DrainHandler = Box<dyn FnMut(&SettingsDrain)>;

/// 「设置页 ↔ cfg」的接线器。
pub struct SettingsBridge {
    writer: Rc<SettingsWriter>,
    /// `push_settings` 的第二个参数（`DOWNLOAD_DIR` 的兜底）。
    default_download_folder: String,
    /// `Some` 表示轮询中；[`SettingsBridge::stop`] 之后是 `None`。
    timer: RefCell<Option<slint::Timer>>,
    /// 宿主的 drain 钩子（见模块头的「宿主的钩子」）。
    on_drain: RefCell<Option<DrainHandler>>,
}

impl SettingsBridge {
    pub fn new(writer: Rc<SettingsWriter>, default_download_folder: impl Into<String>) -> Rc<Self> {
        Rc::new(Self {
            writer,
            default_download_folder: default_download_folder.into(),
            timer: RefCell::new(None),
            on_drain: RefCell::new(None),
        })
    }

    pub fn writer(&self) -> &Rc<SettingsWriter> {
        &self.writer
    }

    /// 装动作 + 起轮询（[`POLL_INTERVAL`]）。
    pub fn install(self: &Rc<Self>, window: &AppWindow) {
        self.install_with_interval(window, POLL_INTERVAL);
    }

    /// 同 [`SettingsBridge::install`]，但自定轮询间隔（测试用）。
    pub fn install_with_interval(self: &Rc<Self>, window: &AppWindow, interval: Duration) {
        install_settings_actions(window, Rc::new(self.writer.actions()));
        let bridge = Rc::downgrade(self);
        let target = window.as_weak();
        let timer = slint::Timer::default();
        timer.start(slint::TimerMode::Repeated, interval, move || {
            if let (Some(bridge), Some(window)) = (bridge.upgrade(), target.upgrade()) {
                let _ = bridge.drain(&window);
            }
        });
        *self.timer.borrow_mut() = Some(timer);
    }

    /// 装宿主的 drain 钩子（见模块头的「宿主的钩子」）。
    ///
    /// 钩子在**重推之前**调用，所以它改出来的值（`adopt` 换令牌）也能进同一帧。
    /// 钩子内**不要**再调 [`Self::drain`]（`RefCell` 已经可变借用）。
    pub fn set_drain_handler(&self, handler: impl FnMut(&SettingsDrain) + 'static) {
        *self.on_drain.borrow_mut() = Some(Box::new(handler));
    }

    /// 取走改动/失败/待办，交给宿主钩子，再把当前配置重推到窗口。
    ///
    /// 幂等：没有待处理的东西时什么都不做（`repushed == false`、不调钩子）。
    pub fn drain(&self, window: &AppWindow) -> SettingsDrain {
        let failures = self.writer.take_failures();
        let changes = self.writer.take_changes();
        let deferred = self.writer.take_deferred();
        // 失败也要重推：`ConfigStore::set` 是先改内存再 `save()`，值已经变了。
        let repushed = !changes.is_empty() || !failures.is_empty();
        let drained = SettingsDrain {
            changes,
            failures,
            deferred,
            repushed,
        };
        if !drained.is_empty()
            && let Some(handler) = self.on_drain.borrow_mut().as_mut()
        {
            handler(&drained);
        }
        if repushed {
            push_settings(window, &self.writer.config(), &self.default_download_folder);
        }
        drained
    }

    /// 停掉轮询（drop timer）。动作仍然可用，只是不再自动 drain。
    pub fn stop(&self) {
        self.timer.borrow_mut().take();
    }

    /// 轮询是否在跑。
    pub fn is_polling(&self) -> bool {
        self.timer.borrow().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use serde_json::json;

    use crate::config_store::ConfigStore;

    /// 钩子里记下的「改动条数 + 当前 `maxTaskNum`」。
    type Seen = Vec<(usize, Option<serde_json::Value>)>;

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "gd3-ui-settings-bridge-{}-{tag}-{seq}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    /// 一个「路径在临时目录里、内存是缺省值」的 bridge。
    fn bridge(tag: &str) -> (PathBuf, Rc<SettingsBridge>) {
        let dir = temp_dir(tag);
        let path = dir.join("UserConfig.json");
        let writer = Rc::new(SettingsWriter::new(ConfigStore::new(&path)));
        (path, SettingsBridge::new(writer, "//Downloads"))
    }

    /// 推进 Slint 的定时器，直到 `done` 为真或超时（最多 ~500ms）。
    fn pump_until(mut done: impl FnMut() -> bool) -> bool {
        for _ in 0..250 {
            slint::platform::update_timers_and_animations();
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        false
    }

    #[test]
    fn install_wires_the_actions_to_the_writer() {
        let (path, bridge) = bridge("install");
        let window = crate::create_headless();
        bridge.install(&window);

        window.invoke_maxTaskNumChanged(7);
        assert_eq!(bridge.writer().get("maxTaskNum"), Some(json!(7)));
        assert!(path.exists(), "动作已经落盘");

        let drained = bridge.drain(&window);
        assert_eq!(drained.changes.len(), 1);
        assert_eq!(drained.changes[0].attr, "maxTaskNum");
        assert!(drained.repushed);
        assert_eq!(window.get_maxTaskNum(), 7);
        bridge.stop();
    }

    #[test]
    fn drain_pushes_the_value_that_correction_produced() {
        let (_path, bridge) = bridge("corrected");
        let window = crate::create_headless();
        bridge.install(&window);

        // Python: `RangeValidator(1024, 65535).correct(80)` → 1024。
        window.invoke_aria2RpcPortChanged(80);
        assert_eq!(bridge.writer().get("aria2RpcPort"), Some(json!(1024)));

        let drained = bridge.drain(&window);
        assert_eq!(drained.changes[0].value, json!(1024));
        // 卡片自己回读 correct() 之后的值 —— 这里靠重推。
        assert_eq!(window.get_aria2RpcPort(), 1024);
        bridge.stop();
    }

    #[test]
    fn a_second_drain_does_nothing() {
        let (_path, bridge) = bridge("idempotent");
        let window = crate::create_headless();
        bridge.install(&window);
        window.invoke_maxTaskNumChanged(5);
        assert!(bridge.drain(&window).repushed);

        let again = bridge.drain(&window);
        assert!(again.is_empty());
        assert!(!again.repushed);
        assert!(again.restart_attrs().is_empty());
        bridge.stop();
    }

    #[test]
    fn an_unchanged_value_is_not_pushed_again() {
        let (path, bridge) = bridge("unchanged");
        let window = crate::create_headless();
        bridge.install(&window);
        // 缺省就是 3：相等早退，既不落盘也不重推。
        window.invoke_maxTaskNumChanged(3);

        let drained = bridge.drain(&window);
        assert!(drained.changes.is_empty());
        assert!(!drained.repushed);
        assert!(!path.exists());
        bridge.stop();
    }

    #[test]
    fn a_failed_save_is_reported_and_still_repushes() {
        let dir = temp_dir("failure");
        // 父目录占成普通文件 → `mkdir(parents=True)` 失败。
        let blocked = dir.join("blocked");
        std::fs::write(&blocked, b"not a directory").expect("占位文件");
        let writer = Rc::new(SettingsWriter::new(ConfigStore::new(
            blocked.join("UserConfig.json"),
        )));
        let bridge = SettingsBridge::new(writer, "//Downloads");
        let window = crate::create_headless();
        bridge.install(&window);

        window.invoke_maxTaskNumChanged(9);
        let drained = bridge.drain(&window);
        assert_eq!(drained.failures.len(), 1);
        assert!(drained.changes.is_empty());
        assert!(drained.repushed, "内存里的值已经改了，界面要跟上");
        assert_eq!(window.get_maxTaskNum(), 9);
        bridge.stop();
    }

    #[test]
    fn deferred_actions_are_reported_not_executed() {
        let (path, bridge) = bridge("deferred");
        let window = crate::create_headless();
        bridge.install(&window);

        window.invoke_copyPairToken();
        // 第 119 步之后 `categoryResetRequested` 会真写盘，这里换一个仍待办的
        // 对话框动作（开合类）来钉「待办不进改动列表」。
        window.invoke_categoryAddRequested();
        let drained = bridge.drain(&window);
        assert_eq!(drained.deferred, ["copyPairToken", "categoryAddRequested"]);
        assert!(drained.changes.is_empty());
        assert!(!drained.repushed);
        assert!(!path.exists());
        bridge.stop();
    }

    #[test]
    fn restart_items_are_listed_for_the_host() {
        let (_path, bridge) = bridge("restart");
        let window = crate::create_headless();
        bridge.install(&window);

        window.invoke_verifySslChanged(true);
        window.invoke_maxTaskNumChanged(4);
        let drained = bridge.drain(&window);
        assert_eq!(drained.restart_attrs(), ["shouldVerifySsl"]);
        bridge.stop();
    }

    #[test]
    fn the_drain_handler_sees_the_change_before_the_repush() {
        let (_path, bridge) = bridge("handler");
        let window = crate::create_headless();
        bridge.install(&window);

        let seen: Rc<RefCell<Seen>> = Rc::new(RefCell::new(Vec::new()));
        {
            let seen = Rc::clone(&seen);
            let weak = Rc::downgrade(&bridge);
            bridge.set_drain_handler(move |drained| {
                let value = weak
                    .upgrade()
                    .and_then(|bridge| bridge.writer().get("maxTaskNum"));
                seen.borrow_mut().push((drained.changes.len(), value));
            });
        }

        window.invoke_maxTaskNumChanged(9);
        let drained = bridge.drain(&window);
        assert!(drained.repushed);
        assert_eq!(seen.borrow().len(), 1);
        assert_eq!(seen.borrow()[0].0, 1);
        assert_eq!(seen.borrow()[0].1, Some(json!(9)));
        bridge.stop();
    }

    #[test]
    fn the_drain_handler_is_not_called_for_an_empty_drain() {
        let (_path, bridge) = bridge("handler-empty");
        let window = crate::create_headless();
        bridge.install(&window);
        let calls = Rc::new(RefCell::new(0u32));
        {
            let calls = Rc::clone(&calls);
            bridge.set_drain_handler(move |_| *calls.borrow_mut() += 1);
        }
        assert!(bridge.drain(&window).is_empty());
        assert_eq!(*calls.borrow(), 0);
        bridge.stop();
    }

    #[test]
    fn the_poll_timer_drains_on_its_own() {
        let (_path, bridge) = bridge("timer");
        let window = crate::create_headless();
        bridge.install_with_interval(&window, Duration::from_millis(1));
        assert!(bridge.is_polling());

        window.invoke_maxTaskNumChanged(8);
        assert_eq!(bridge.writer().get("maxTaskNum"), Some(json!(8)));
        // 定时器把改动取走了 → 窗口属性跟着变。
        assert!(
            pump_until(|| window.get_maxTaskNum() == 8),
            "轮询定时器没把改动推到 UI"
        );
        assert!(
            bridge.writer().take_changes().is_empty(),
            "已经被定时器取走"
        );
        bridge.stop();
    }

    #[test]
    fn stop_stops_the_polling() {
        let (_path, bridge) = bridge("stop");
        let window = crate::create_headless();
        bridge.install_with_interval(&window, Duration::from_millis(1));
        bridge.stop();
        assert!(!bridge.is_polling());

        window.invoke_maxTaskNumChanged(6);
        // 定时器没了 → 改动留在 writer 里等宿主取。
        for _ in 0..20 {
            slint::platform::update_timers_and_animations();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(bridge.writer().take_changes().len(), 1);
        assert_eq!(window.get_maxTaskNum(), 3, "没人重推，UI 还是缺省");
    }
}
