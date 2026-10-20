//! 限速闸门：`app/services/speed_meter.py:39-41` 的 `waitForSpeedLimit` 的落地版。
//!
//! ```python
//! def addSpeed(self, byteCount: int) -> None:
//!     self._bytes += byteCount
//!
//! async def waitForSpeedLimit(self) -> None:
//!     while cfg.isSpeedLimitEnabled.value and self._bytes > cfg.speedLimitation.value:
//!         await asyncio.sleep(0.1)
//! ```
//!
//! Python 的版本活在**协程里**：`_bytes` 是普通整数，`SpeedMeter._tickLoop`
//! （`speed_meter.py:44-47`）每秒把它清零，任务的每个数据块后问一次闸门。Rust 的下载
//! 跑在 tokio 工作线程上，于是这里用一组原子量做跨线程共享：
//!
//! | 字段 | Python | 谁写 |
//! |---|---|---|
//! | `bytes` | `_bytes` | 工作线程每个数据块 `add` |
//! | `enabled` / `limit` | `cfg.isSpeedLimitEnabled` / `cfg.speedLimitation` | 宿主每轮 tick `configure` |
//! | `window_start` | `_tickLoop` 的 1 秒节拍 | **闸门自己**（见下） |
//! | `closed` | （无） | 停机时 `close` |
//!
//! ## 与 Python 的差异
//!
//! 1. **判断在 `>`，不是 `>=`**：`_bytes > limit` 是严格大于，`pending_bytes == limit`
//!    时**不**等（照抄，别「修正」）。
//! 2. **`cfg` 从参数进来**：`waitForSpeedLimit` 每轮直接读全局 `cfg`，Rust 侧由宿主每轮
//!    tick 用 [`SpeedLimit::configure`] 把现值推进来（对应 `isSpeedLimitEnabled` /
//!    `speedLimitation` 两个 attr）。
//! 3. **计数窗口自带时钟**：Python 靠 `SpeedMeter._tickLoop` 每秒清零，而那个节拍在
//!    Rust 里是 UI 线程的 `slint::Timer` —— 工作线程不能假设宿主一定在 tick
//!    （无头驱动的测试、以及停机之后都没有）。所以闸门自己记窗口起点，
//!    [`SpeedLimit::exceeded`] 里先把过期的窗口滚掉（[`WINDOW`]）。这与
//!    「每秒清零」在**外部行为**上完全一致：一个窗口内最多放行 `limit` 字节。
//!    宿主的 `reset` 仍然可用（照 Python 的 1 秒节拍对齐），但不再是正确性的前提。
//! 4. **多一个 `closed`**：Python 没有，因为它只需要协程被取消。Rust 里工作线程可能正
//!    卡在 [`SpeedGate::wait`] 的睡眠里，`TaskRunner::shutdown` 调
//!    [`SpeedLimit::close`] 让它立刻返回（否则最多要等满一个窗口）。
//! 5. 轮询间隔仍是 100 ms（`await asyncio.sleep(0.1)`），见 [`POLL_INTERVAL`]。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gd3_net::SpeedGate;

/// `await asyncio.sleep(0.1)`。
pub const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// `SpeedMeter._tickLoop` 的 1 秒节拍（计数窗口长度）。
pub const WINDOW: Duration = Duration::from_secs(1);

/// 全局限速闸门（一个 `TaskRunner` 一份，`Arc` 共享给所有工作线程）。
#[derive(Debug)]
pub struct SpeedLimit {
    /// `_bytes`：当前窗口里收到的字节数。
    bytes: AtomicI64,
    /// `cfg.isSpeedLimitEnabled`。
    enabled: AtomicBool,
    /// `cfg.speedLimitation`（字节/秒）。
    limit: AtomicI64,
    /// 停机标志（见模块头差异 4）。
    closed: AtomicBool,
    /// 建实例的时刻；`window_start` 以它为原点记毫秒。
    epoch: Instant,
    /// 当前计数窗口的起点（`epoch` 之后多少毫秒）。
    window_start: AtomicU64,
}

impl Default for SpeedLimit {
    fn default() -> Self {
        SpeedLimit {
            bytes: AtomicI64::new(0),
            enabled: AtomicBool::new(false),
            limit: AtomicI64::new(0),
            closed: AtomicBool::new(false),
            epoch: Instant::now(),
            window_start: AtomicU64::new(0),
        }
    }
}

impl SpeedLimit {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// 宿主每轮 tick 推进来的现值（`cfg.isSpeedLimitEnabled` / `cfg.speedLimitation`）。
    pub fn configure(&self, enabled: bool, limit_bytes: i64) {
        self.enabled.store(enabled, Ordering::Relaxed);
        self.limit.store(limit_bytes, Ordering::Relaxed);
    }

    /// `addSpeed(byteCount)`：每个数据块加一次。
    pub fn add(&self, bytes: u64) {
        if bytes == 0 {
            return;
        }
        self.bytes.fetch_add(bytes as i64, Ordering::Relaxed);
    }

    /// 当前窗口的累加量（诊断 / 测试用）。
    pub fn pending_bytes(&self) -> i64 {
        self.bytes.load(Ordering::Relaxed)
    }

    /// 手动开新窗口（`SpeedMeter._tick` 里 `self._bytes = 0`）。
    ///
    /// 宿主可按 1 秒节拍调它（让清零时刻与顶栏速度表对齐）；不调也不会错 ——
    /// [`SpeedLimit::exceeded`] 会自己滚窗口。
    pub fn reset(&self) {
        self.roll_window(self.now_millis());
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn limit_bytes(&self) -> i64 {
        self.limit.load(Ordering::Relaxed)
    }

    /// `while cfg.isSpeedLimitEnabled.value and self._bytes > cfg.speedLimitation.value`。
    pub fn exceeded(&self) -> bool {
        if self.closed.load(Ordering::Relaxed) || !self.enabled.load(Ordering::Relaxed) {
            return false;
        }
        // 窗口过期就先滚（等价于 Python 的 `_tick` 每秒清零）。
        self.roll_window_if_stale();
        self.bytes.load(Ordering::Relaxed) > self.limit.load(Ordering::Relaxed)
    }

    /// 停机：所有正在等闸门的任务立刻放行。
    pub fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    fn now_millis(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    fn roll_window_if_stale(&self) {
        let now = self.now_millis();
        if now.saturating_sub(self.window_start.load(Ordering::Relaxed))
            >= WINDOW.as_millis() as u64
        {
            self.roll_window(now);
        }
    }

    /// 把窗口起点推到 `now` 并清零。多个线程同时滚时只有一个赢（`compare_exchange`），
    /// 免得两次清零把同一批字节算丢两遍。
    ///
    /// 目标取 `now.max(start + 1)`：**保证严格前进**。否则同一毫秒里两次滚动的目标相同，
    /// `compare_exchange` 在「值没变」时也会成功，两个线程会把窗口清两遍
    /// （刚加进来的字节就被抹掉了）。
    fn roll_window(&self, now: u64) {
        let mut start = self.window_start.load(Ordering::Relaxed);
        loop {
            let target = now.max(start + 1);
            match self.window_start.compare_exchange_weak(
                start,
                target,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    self.bytes.store(0, Ordering::Relaxed);
                    return;
                }
                Err(actual) => start = actual,
            }
        }
    }
}

impl SpeedGate for SpeedLimit {
    fn wait(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            // `while` 每轮重新判断（窗口会滚、`cfg` 也可能改、停机时 `closed` 变真）。
            while self.exceeded() {
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("建测试运行时")
    }

    #[test]
    fn a_closed_or_disabled_gate_never_waits() {
        let gate = SpeedLimit::new();
        gate.add(10 * 1024 * 1024);
        assert!(!gate.exceeded(), "缺省关着（`isSpeedLimitEnabled=False`）");

        gate.configure(true, 1024);
        assert!(gate.exceeded(), "开起来且超了就该等");

        gate.configure(false, 1024);
        assert!(!gate.exceeded(), "关掉立刻放行");

        gate.configure(true, 1024);
        gate.close();
        assert!(!gate.exceeded(), "停机后必须放行，否则 shutdown 会卡住");
        assert!(gate.is_closed());
    }

    #[test]
    fn the_comparison_is_strictly_greater() {
        let gate = SpeedLimit::new();
        gate.configure(true, 1024);
        gate.add(1024);
        assert_eq!(gate.pending_bytes(), 1024);
        assert!(!gate.exceeded(), "等于上限不算超（`_bytes > limit`）");
        gate.add(1);
        assert!(gate.exceeded());
    }

    #[test]
    fn reset_starts_a_new_window() {
        let gate = SpeedLimit::new();
        gate.configure(true, 1024);
        gate.add(4096);
        assert!(gate.exceeded());
        gate.reset();
        assert_eq!(gate.pending_bytes(), 0);
        assert!(!gate.exceeded());
        assert_eq!(gate.limit_bytes(), 1024, "reset 不动配置");
        assert!(gate.is_enabled());
    }

    #[test]
    fn the_window_rolls_by_itself() {
        // 不靠宿主 tick：等过 WINDOW 之后自己清零（模块头差异 3）。
        let gate = SpeedLimit::new();
        gate.configure(true, 1024);
        gate.add(4096);
        assert!(gate.exceeded());
        thread::sleep(WINDOW + Duration::from_millis(50));
        assert!(!gate.exceeded(), "窗口过期就该放行");
        assert_eq!(gate.pending_bytes(), 0);
    }

    #[test]
    fn concurrent_rolls_zero_the_window_once() {
        let gate = SpeedLimit::new();
        gate.configure(true, 1);
        gate.add(1024);
        thread::sleep(WINDOW + Duration::from_millis(50));
        let gate = Arc::clone(&gate);
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let gate = Arc::clone(&gate);
                thread::spawn(move || {
                    // 每个线程都试着滚窗口；只有一个能赢，其它看到已经滚过了。
                    gate.roll_window_if_stale();
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("线程");
        }
        assert_eq!(gate.pending_bytes(), 0);
    }

    #[test]
    fn wait_returns_at_once_when_under_the_limit() {
        let gate = SpeedLimit::new();
        gate.configure(true, 1024);
        gate.add(10);
        runtime().block_on(async {
            tokio::time::timeout(POLL_INTERVAL * 10, SpeedGate::wait(&*gate))
                .await
                .expect("不超限不该等");
        });
    }

    #[test]
    fn wait_releases_when_the_window_resets() {
        let gate = SpeedLimit::new();
        gate.configure(true, 1024);
        gate.add(8192);
        let ticker = Arc::clone(&gate);
        let handle = thread::spawn(move || {
            thread::sleep(POLL_INTERVAL);
            ticker.reset();
        });
        let started = Instant::now();
        runtime().block_on(async {
            tokio::time::timeout(Duration::from_secs(5), SpeedGate::wait(&*gate))
                .await
                .expect("清零之后必须放行");
        });
        handle.join().expect("ticker");
        assert!(
            started.elapsed() >= POLL_INTERVAL / 2,
            "确实等过（否则说明闸门没起作用）：{:?}",
            started.elapsed()
        );
        assert!(!gate.exceeded());
    }

    #[test]
    fn close_releases_a_waiting_gate() {
        let gate = SpeedLimit::new();
        gate.configure(true, 1);
        gate.add(1024);
        let closer = Arc::clone(&gate);
        let handle = thread::spawn(move || {
            thread::sleep(POLL_INTERVAL);
            closer.close();
        });
        runtime().block_on(async {
            tokio::time::timeout(Duration::from_secs(5), SpeedGate::wait(&*gate))
                .await
                .expect("停机要能解救卡住的等待");
        });
        handle.join().expect("closer");
    }
}
