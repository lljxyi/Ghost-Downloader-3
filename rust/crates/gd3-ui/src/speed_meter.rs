//! 速度表（第 104 步）——镜像 `app/services/speed_meter.py`（56 行）。
//!
//! ## 职责
//!
//! 「全局下载速度」这一条数字：任务每收一块数据就 `addSpeed(n)`，速度表每秒把
//! 累加到的字节数当成「瞬时速度」发布一次，然后清零重新累加。
//!
//! 它同时是**限速闸门**：`task_service.py:578` 把
//! `task.run(self._speedMeter.addSpeed, self._speedMeter.waitForSpeedLimit)` 交给任务，
//! 任务在每个分片前问一次「现在超限了吗」。
//!
//! ## 实证口径（`app/services/speed_meter.py` 实读）
//!
//! - `_tickLoop()`（:44-47）是 `while True: await sleep(1); post(_tick)`——**无限循环**，
//!   只有 `stop()` 取消 workId 才结束；`_tick`（:49-53）自己再查一次
//!   `if self._tickWorkId is None: return`，所以「取消后仍排到队里的 tick」不会再发信号；
//! - `_tick` 把 `_currentSpeed = _bytes` 后**立刻**把 `_bytes` 清零——两次 tick 之间
//!   累加的量就是这一秒的速度，不做滑动平均；
//! - `stop()`（:24-32）先 `cancel(workId)` 再把 `_bytes` / `_currentSpeed` 双双归零，
//!   最后 `speedChanged.emit(0)`——**即使本来就 0 也要发**，界面靠这一发把数字抹掉；
//! - `start()`（:19-22）用 `_tickWorkId is not None` 判重，`stop()` 用
//!   `is None` 判空：两个方向都是**幂等**的；
//! - `waitForSpeedLimit()`（:34-38）每轮**重新读** `cfg.isSpeedLimitEnabled.value` 与
//!   `cfg.speedLimitation.value`，判断是 `_bytes > limit`（严格大于）；
//! - `_onTickFailed`（:55-56）只把 workId 置空——失败后 `start()` 可以重来。
//!
//! ## 与 Python 的差异
//!
//! 1. **没有协程**：`_tickLoop` 的「每 1 秒」由宿主驱动——`start()` 只吐一条
//!    [`SpeedEffect::StartTicking`]，宿主拿到就起一个 1 秒周期、每周期调
//!    [`SpeedMeter::tick`]、并在失败时调 [`SpeedMeter::on_tick_failed`]；
//! 2. **`waitForSpeedLimit` 退化成谓词**：[`SpeedMeter::should_wait`] 只回答
//!    「这一轮要不要等」，`await sleep(0.1)` 的循环留给宿主（同 `task_draft` 的
//!    效果队列口径）；
//! 3. **配置从参数进来**：`cfg` 是模块级全局，Rust 侧由调用方每轮读
//!    `settings::read_bool(cfg, "isSpeedLimitEnabled")` /
//!    `settings::read_i64(cfg, "speedLimitation")` 再传进来——与 Python 的
//!    「每轮重读」等价，且模块不持有配置快照。

use std::cell::Cell;

/// `speedChanged.emit(...)` / `coroutineRunner.cancel(...)` 的摊平。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpeedEffect {
    /// `_tickLoop()` 开始跑：宿主起一个 1 秒周期，每周期调 [`SpeedMeter::tick`]。
    ///
    /// `work_id` 是不透明句柄，宿主在失败时要原样交回
    /// [`SpeedMeter::on_tick_failed`]（本仓的做法），或者自己忽略。
    StartTicking { work_id: String },
    /// `coroutineRunner.cancel(self._tickWorkId)`。
    Cancel { work_id: String },
    /// `speedChanged.emit(speed)`——`speed` 是**字节/秒**。
    Emit { speed: i64 },
}

/// `SpeedMeter`（`speed_meter.py:9-56`）。
///
/// `Default` 与 `new()` 等价（Python 的 `__init__` 只有 `coroutineRunner` 一个参数，
/// 而它在 Rust 侧已经退化成效果队列，没有可存的东西）。
#[derive(Clone, Debug, Default)]
pub struct SpeedMeter {
    /// `_bytes`：两次 tick 之间累加的字节数。
    bytes: Cell<i64>,
    /// `_currentSpeed`：上一次 tick 发布出去的数字。
    current_speed: Cell<i64>,
    /// `_tickWorkId`：`Some` = 循环在跑。
    tick_work_id: Cell<Option<u64>>,
    /// 发号器（`speed-1` / `speed-2` …），只增不减。
    issued: Cell<u64>,
}

impl SpeedMeter {
    pub fn new() -> Self {
        Self::default()
    }

    /// `currentSpeed`（:15-17）。
    pub fn current_speed(&self) -> i64 {
        self.current_speed.get()
    }

    /// `_bytes`（Python 没有公开，测试与宿主诊断用）。
    pub fn pending_bytes(&self) -> i64 {
        self.bytes.get()
    }

    /// `_tickWorkId is not None`。
    pub fn is_running(&self) -> bool {
        self.tick_work_id.get().is_some()
    }

    /// `start()`（:19-22）：已在跑 → 无效果。
    pub fn start(&mut self) -> Vec<SpeedEffect> {
        if self.tick_work_id.get().is_some() {
            return Vec::new();
        }
        let next = self.issued.get() + 1;
        self.issued.set(next);
        self.tick_work_id.set(Some(next));
        vec![SpeedEffect::StartTicking {
            work_id: format!("speed-{next}"),
        }]
    }

    /// `stop()`（:24-32）：没在跑 → 无效果；否则「取消 + 双清零 + 发 0」。
    ///
    /// **顺序照抄 Python**：先 `cancel` 再归零，`Emit(0)` 在最后。`Emit(0)` 无条件发
    /// ——即使 `current_speed` 本来就是 0。
    pub fn stop(&mut self) -> Vec<SpeedEffect> {
        let Some(work_id) = self.tick_work_id.get() else {
            return Vec::new();
        };
        self.tick_work_id.set(None);
        self.bytes.set(0);
        self.current_speed.set(0);
        vec![
            SpeedEffect::Cancel {
                work_id: format!("speed-{work_id}"),
            },
            SpeedEffect::Emit { speed: 0 },
        ]
    }

    /// `addSpeed(byteCount)`（:34）。
    ///
    /// 不检查「在不在跑」——Python 也不检查，没在跑时累加的量会一直留着，
    /// 直到下一次 `start()` 后的第一个 `tick` 把它当成速度发出去。
    pub fn add_speed(&mut self, byte_count: i64) {
        self.bytes.set(self.bytes.get() + byte_count);
    }

    /// `waitForSpeedLimit()`（:34-38）的单次判断：`enabled and _bytes > limit`。
    ///
    /// 宿主的口径是
    /// `while meter.should_wait(enabled, limit) { sleep(0.1) }`——
    /// 每轮都要**重新读**配置再传进来，才是 Python 的「每轮重读 `cfg`」。
    pub fn should_wait(&self, enabled: bool, limit_bytes: i64) -> bool {
        enabled && self.bytes.get() > limit_bytes
    }

    /// `_tick()`（:49-53）：发布累加量并清零；循环已停 → 无效果（迟到的 tick）。
    pub fn tick(&mut self) -> Vec<SpeedEffect> {
        if self.tick_work_id.get().is_none() {
            return Vec::new();
        }
        let speed = self.bytes.get();
        self.current_speed.set(speed);
        self.bytes.set(0);
        vec![SpeedEffect::Emit { speed }]
    }

    /// `_onTickFailed`（:55-56）：只把 workId 置空，不发信号。
    ///
    /// 置空之后 `start()` 能重新起一轮（新的 workId）；`_bytes` / `_currentSpeed`
    /// **不清**——Python 也没清，下一轮 `start()` 的第一次 tick 会把残留量发出去。
    pub fn on_tick_failed(&mut self) -> Vec<SpeedEffect> {
        self.tick_work_id.set(None);
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work_id(effect: &SpeedEffect) -> String {
        match effect {
            SpeedEffect::StartTicking { work_id } | SpeedEffect::Cancel { work_id } => {
                work_id.clone()
            }
            SpeedEffect::Emit { .. } => panic!("不是 workId 效果"),
        }
    }

    #[test]
    fn start_runs_the_tick_loop_once() {
        let mut meter = SpeedMeter::new();
        assert!(!meter.is_running());

        let effects = meter.start();

        assert_eq!(effects.len(), 1);
        assert_eq!(work_id(&effects[0]), "speed-1");
        assert!(meter.is_running());
    }

    #[test]
    fn start_twice_is_a_no_op() {
        let mut meter = SpeedMeter::new();
        meter.start();

        assert!(meter.start().is_empty());
        assert!(meter.is_running());
    }

    #[test]
    fn stop_cancels_then_resets_and_emits_zero() {
        let mut meter = SpeedMeter::new();
        meter.start();
        meter.add_speed(100);

        let effects = meter.stop();

        assert_eq!(
            effects,
            vec![
                SpeedEffect::Cancel {
                    work_id: "speed-1".into()
                },
                SpeedEffect::Emit { speed: 0 },
            ]
        );
        assert_eq!(meter.current_speed(), 0);
        assert_eq!(meter.pending_bytes(), 0);
        assert!(!meter.is_running());
    }

    #[test]
    fn stop_without_start_does_nothing() {
        let mut meter = SpeedMeter::new();
        assert!(meter.stop().is_empty());
    }

    #[test]
    fn stop_emits_zero_even_when_the_speed_is_already_zero() {
        let mut meter = SpeedMeter::new();
        meter.start();

        assert_eq!(
            meter.stop(),
            vec![
                SpeedEffect::Cancel {
                    work_id: "speed-1".into()
                },
                SpeedEffect::Emit { speed: 0 },
            ],
            "Python 无条件 emit(0)"
        );
    }

    #[test]
    fn tick_publishes_the_accumulated_bytes_and_clears_them() {
        let mut meter = SpeedMeter::new();
        meter.start();
        meter.add_speed(10);
        meter.add_speed(5);

        assert_eq!(meter.tick(), vec![SpeedEffect::Emit { speed: 15 }]);
        assert_eq!(meter.current_speed(), 15);
        assert_eq!(meter.pending_bytes(), 0);

        // 第二轮从 0 重新累加：没有滑动平均。
        meter.add_speed(3);
        assert_eq!(meter.tick(), vec![SpeedEffect::Emit { speed: 3 }]);
    }

    #[test]
    fn tick_without_a_running_loop_is_ignored() {
        let mut meter = SpeedMeter::new();
        meter.add_speed(42);

        assert!(meter.tick().is_empty(), "取消后迟到的 tick 不发布");
        assert_eq!(meter.current_speed(), 0);
        assert_eq!(meter.pending_bytes(), 42, "bytes 没被清掉");
    }

    #[test]
    fn a_failed_tick_loop_lets_start_run_again() {
        let mut meter = SpeedMeter::new();
        meter.start();

        assert!(meter.on_tick_failed().is_empty());
        assert!(!meter.is_running());

        let effects = meter.start();
        assert_eq!(work_id(&effects[0]), "speed-2");
    }

    #[test]
    fn add_speed_accumulates_while_stopped() {
        let mut meter = SpeedMeter::new();
        meter.add_speed(7);
        meter.start();

        assert_eq!(meter.tick(), vec![SpeedEffect::Emit { speed: 7 }]);
    }

    #[test]
    fn should_wait_compares_strictly_greater_than_the_limit() {
        let mut meter = SpeedMeter::new();
        meter.add_speed(100);

        assert!(!meter.should_wait(true, 100), "等于上限不算超");
        assert!(meter.should_wait(true, 99));
        assert!(!meter.should_wait(false, 1), "限速关掉就永不等待");
        assert!(!meter.should_wait(true, 100_000));
    }

    #[test]
    fn should_wait_reads_the_pending_bytes_not_the_published_speed() {
        let mut meter = SpeedMeter::new();
        meter.start();
        meter.add_speed(100);
        meter.tick();
        assert_eq!(meter.current_speed(), 100);

        // 新一轮只累加了 10 字节：闸门看 10，不看上一轮的 100。
        meter.add_speed(10);
        assert!(!meter.should_wait(true, 50));
    }

    #[test]
    fn current_speed_is_zero_until_the_first_tick() {
        let mut meter = SpeedMeter::new();
        assert_eq!(meter.current_speed(), 0);
        meter.start();
        meter.add_speed(999);
        assert_eq!(meter.current_speed(), 0, "addSpeed 不发布");
    }

    #[test]
    fn work_ids_are_never_reused() {
        let mut meter = SpeedMeter::new();
        let mut ids = Vec::new();
        for _ in 0..3 {
            ids.push(work_id(&meter.start()[0]));
            let effects = meter.stop();
            assert_eq!(work_id(&effects[0]), ids.last().unwrap().clone());
        }
        assert_eq!(ids, vec!["speed-1", "speed-2", "speed-3"]);
    }
}
