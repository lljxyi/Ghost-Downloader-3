//! 运行中的分片计划：镜像 `features/http_pack/task.py` 的 `_splitSlowest` / `_reassignSubworker`。
//!
//! 与 `ranges.rs` 的区别：那里的 `Subworker` 是「开跑前的计划」，定死不变；
//! 这里的分段在下载过程中会被重新切分——最慢的那段 `end` 会被改小，
//! 而它对应的下载任务必须立刻看到新的末尾，所以 `end` 是原子的。

use crate::ranges::{Subworker, split_remaining};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 新分片之间的启动间隔（Python `SPLIT_START_INTERVAL`）。
pub const SPLIT_START_INTERVAL: Duration = Duration::from_millis(200);

/// 运行中的一段。
#[derive(Debug)]
pub struct Segment {
    pub start: u64,
    /// 闭区间末尾；`END_NOT_SUPPORTED` / `END_UNKNOWN` 为哨兵值。运行中可能被改小。
    pub end: AtomicI64,
    pub position: AtomicU64,
}

impl Segment {
    fn new(start: u64, position: u64, end: i64) -> Segment {
        Segment {
            start,
            end: AtomicI64::new(end),
            position: AtomicU64::new(position),
        }
    }

    pub fn end(&self) -> i64 {
        self.end.load(Ordering::Relaxed)
    }

    pub fn position(&self) -> u64 {
        self.position.load(Ordering::Relaxed)
    }

    /// 还差多少字节写完这一段（含两端）——Python `sw.end - sw.position + 1`。
    pub fn remaining(&self) -> i64 {
        self.end() - self.position() as i64 + 1
    }
}

/// 分片计划：`Mutex` 只保护「增删段」与「改末尾」，下载任务各自读原子量。
#[derive(Debug)]
pub struct Plan {
    segments: Mutex<Vec<Arc<Segment>>>,
}

impl Plan {
    pub fn new(subworkers: &[Subworker]) -> Plan {
        Plan {
            segments: Mutex::new(
                subworkers
                    .iter()
                    .map(|subworker| {
                        Arc::new(Segment::new(
                            subworker.start,
                            subworker.position,
                            subworker.end,
                        ))
                    })
                    .collect(),
            ),
        }
    }

    /// 当前所有分段（按顺序）。
    pub fn segments(&self) -> Vec<Arc<Segment>> {
        self.segments.lock().expect("分片锁").clone()
    }

    pub fn segment_count(&self) -> usize {
        self.segments.lock().expect("分片锁").len()
    }

    /// 按顺序取 `(start, position, end)`：写 `.ghd` 与回写 `step.subworkers` 都用它。
    ///
    /// `end` 为哨兵值时按 0 报（与 `.ghd` 只记正数末尾的既有行为一致）。
    pub fn snapshot(&self) -> Vec<(u64, u64, u64)> {
        self.segments
            .lock()
            .expect("分片锁")
            .iter()
            .map(|segment| {
                (
                    segment.start,
                    segment.position(),
                    segment.end().max(0) as u64,
                )
            })
            .collect()
    }

    /// 把最慢（剩余最多）的一段对半劈开，新段插在它后面（Python `_splitSlowest`）。
    ///
    /// 剩余不足 `min_remaining` 字节时不切，返回 `None`。
    pub fn split_slowest(&self, min_remaining: i64) -> Option<Arc<Segment>> {
        let mut segments = self.segments.lock().expect("分片锁");

        // Python 的 max() 取第一个最大值，Rust 的 max_by_key 取最后一个，所以这里手写比较
        let mut slowest_index = 0;
        for (index, segment) in segments.iter().enumerate() {
            if segment.remaining() > segments[slowest_index].remaining() {
                slowest_index = index;
            }
        }

        let slowest = Arc::clone(&segments[slowest_index]);
        if slowest.remaining() < min_remaining {
            return None;
        }
        let (new_end, new_start, old_end) = split_remaining(slowest.position(), slowest.end())?;
        slowest.end.store(new_end, Ordering::Relaxed);

        let segment = Arc::new(Segment::new(new_start, new_start, old_end));
        segments.insert(slowest_index + 1, Arc::clone(&segment));
        Some(segment)
    }
}

/// 重新切分的调度：把新分片的启动时刻按 `SPLIT_START_INTERVAL` 错开。
///
/// Python 里就是 `_nextSplitStartAt` 那三行：`startAt = max(now, next)`，
/// `next = startAt + 0.2`，然后让新任务等 `startAt - now` 再跑。
#[derive(Debug, Default)]
pub struct SplitScheduler {
    next_split_at: Mutex<Option<Instant>>,
}

impl SplitScheduler {
    pub fn new() -> SplitScheduler {
        SplitScheduler::default()
    }

    /// 切最慢的一段，返回 `(新段, 等多久再开跑)`。
    ///
    /// 剩余不足 `min_remaining` 字节（或切不动）时返回 `None`，此时计划不变。
    pub fn split(&self, plan: &Plan, min_remaining: i64) -> Option<(Arc<Segment>, Duration)> {
        let segment = plan.split_slowest(min_remaining)?;

        let mut next = self.next_split_at.lock().expect("切分锁");
        let now = Instant::now();
        let start_at = match *next {
            Some(at) if at > now => at,
            _ => now,
        };
        *next = Some(start_at + SPLIT_START_INTERVAL);
        Some((segment, start_at.saturating_duration_since(now)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ranges::{Subworker, split_ranges};

    /// Python `TestReassignment.test_split_subworkers_start_one_interval_apart`。
    #[test]
    fn splits_start_one_interval_apart() {
        let plan = Plan::new(&split_ranges(8_000_000, 1, true));
        let scheduler = SplitScheduler::new();

        let delays: Vec<Duration> = (0..3)
            .map(|_| {
                let (_, delay) = scheduler.split(&plan, 1).expect("应该能切");
                delay
            })
            .collect();

        assert_eq!(plan.segment_count(), 4, "切了 3 次就该有 4 段");
        let seconds: Vec<f64> = delays.iter().map(|delay| delay.as_secs_f64()).collect();
        assert!(
            seconds[0] < 0.01 && (seconds[1] - 0.2).abs() < 0.01 && (seconds[2] - 0.4).abs() < 0.01,
            "启动间隔应当是 0 / 0.2 / 0.4：{seconds:?}"
        );
    }

    /// Python `TestReassignment.test_splits_slowest_subworker`。
    #[test]
    fn splits_the_slowest_segment() {
        let mut subworkers = split_ranges(4_000_000, 2, true);
        // sw0 快跑完了，sw1 还没开始
        subworkers[0].position = subworkers[0].end as u64 - 9;
        let plan = Plan::new(&subworkers);

        let segment = plan.split_slowest(1).expect("应该切最慢的那段");

        let snapshot = plan.snapshot();
        assert_eq!(snapshot.len(), 3);
        let (sw1_start, _, sw1_end) = snapshot[1];
        let (sw2_start, _, sw2_end) = snapshot[2];
        assert_eq!(sw1_start, 2_000_000);
        assert_eq!(segment.start, sw1_end + 1, "新段应当紧接慢段之后");
        assert_eq!(sw2_start, sw1_end + 1);
        assert_eq!(sw2_end, 3_999_999);
        assert!(sw1_end < 3_999_999, "慢段的末尾必须被改小");
    }

    /// Python `TestReassignment.test_new_subworker_covers_second_half`。
    #[test]
    fn new_segment_covers_exactly_the_other_half() {
        let mut subworkers = split_ranges(4_000_000, 2, true);
        subworkers[0].position = subworkers[0].end as u64 - 9;
        let plan = Plan::new(&subworkers);

        let old_position = plan.snapshot()[1].1;
        let remaining = 4_000_000u64 - old_position;

        let segment = plan.split_slowest(1).expect("应该能切");

        let snapshot = plan.snapshot();
        let slow_end = snapshot[1].2;
        let new_range = slow_end - old_position + 1;
        let new_segment_range = segment.end() as u64 - segment.start + 1;
        assert_eq!(
            new_range + new_segment_range,
            remaining,
            "两段加起来要覆盖原来那一段"
        );
    }

    /// Python `TestReassignment.test_skips_when_below_threshold`（默认阈值 512KB）。
    #[test]
    fn skips_when_below_the_threshold() {
        let plan = Plan::new(&split_ranges(1000, 2, true));

        assert!(plan.split_slowest(512 * 1024).is_none());
        assert_eq!(plan.segment_count(), 2, "低于阈值不该切");
    }

    /// Python `TestReassignment.test_skips_when_filesize_zero`。
    #[test]
    fn skips_when_file_size_is_unknown() {
        let plan = Plan::new(&[Subworker::new(0, 0, crate::ranges::END_UNKNOWN)]);

        assert!(plan.split_slowest(512 * 1024).is_none());
        assert_eq!(plan.segment_count(), 1);
    }

    /// 并列时取靠前的那一段（Python `max` 取首个最大值）。
    #[test]
    fn ties_go_to_the_earlier_segment() {
        let plan = Plan::new(&[Subworker::new(0, 0, 99), Subworker::new(1, 100, 199)]);

        let segment = plan.split_slowest(1).expect("应该能切");

        assert_eq!(segment.start, 50, "应当切第一段");
        assert_eq!(plan.snapshot()[0].2, 49);
    }
}
