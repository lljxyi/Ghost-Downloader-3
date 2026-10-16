//! 分片划分与恢复：镜像 `features/http_pack/task.py` 的 `_buildSubworkers` / `_loadRecord` / `_splitSlowest`。
//!
//! `Subworker` 是「计划」：`start`/`end` 定死，`position` 是恢复起点。
//! 运行中的实时位置由引擎里的原子量持有，`position` 只用于开跑前的划分与补齐。

use gd3_core::ghd::{PartRecord, load_records};
use std::io;
use std::path::Path;

/// 不支持范围请求（Python `SpecialFileSize.NOT_SUPPORTED`）。
pub const END_NOT_SUPPORTED: i64 = -1;
/// 文件大小未知（Python `SpecialFileSize.UNKNOWN`）。
pub const END_UNKNOWN: i64 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Subworker {
    pub index: usize,
    pub start: u64,
    pub position: u64,
    /// 闭区间末尾；`END_NOT_SUPPORTED` / `END_UNKNOWN` 为哨兵值。
    pub end: i64,
}

impl Subworker {
    pub fn new(index: usize, start: u64, end: i64) -> Self {
        Subworker {
            index,
            start,
            position: start,
            end,
        }
    }

    /// 还差多少字节写完这一段（含两端）。
    pub fn remaining(&self) -> i64 {
        self.end - self.position as i64 + 1
    }

    pub fn is_fixed_range(&self) -> bool {
        self.end > 0
    }
}

/// 按 Python 的算法切分：前 n-1 段等长，最后一段吃掉余数。
pub fn split_ranges(
    file_size: u64,
    subworker_count: usize,
    can_use_range_requests: bool,
) -> Vec<Subworker> {
    if !can_use_range_requests {
        return vec![Subworker::new(0, 0, END_NOT_SUPPORTED)];
    }
    if file_size == 0 {
        return vec![Subworker::new(0, 0, END_UNKNOWN)];
    }

    let count = subworker_count.min(file_size as usize).max(1);
    let chunk_size = file_size / count as u64;

    let mut subworkers = Vec::with_capacity(count);
    let mut start = 0u64;
    for index in 0..count - 1 {
        let end = start + chunk_size - 1;
        subworkers.push(Subworker::new(index, start, end as i64));
        start = end + 1;
    }
    subworkers.push(Subworker::new(count - 1, start, file_size as i64 - 1));
    subworkers
}

/// 把一段对半劈开：返回 `(慢段的新末尾, 新段起点, 新段末尾)`。
///
/// 抽出来是为了让「运行前的补齐」（本模块）与「运行中的重新切分」（`plan` 模块）
/// 用同一份算术。剩余不足 2 字节返回 `None`。
pub fn split_remaining(position: u64, end: i64) -> Option<(i64, u64, i64)> {
    let remaining_bytes = end - position as i64 + 1;
    if remaining_bytes < 2 {
        return None;
    }
    let base = remaining_bytes / 2;
    let remainder = remaining_bytes % 2;
    let new_end = position as i64 + base + remainder - 1;
    Some((new_end, new_end as u64 + 1, end))
}

/// 把最慢（剩余最多）的一段对半劈开，新段插在它后面。返回新段。
///
/// 与 Python 一致：剩余不足 2 字节时不切；并列时取靠前的那一段（Python `max` 取首个最大值）。
pub fn split_slowest(subworkers: &mut Vec<Subworker>) -> Option<Subworker> {
    // Python 的 max() 取第一个最大值，Rust 的 max_by_key 取最后一个，所以这里手写比较
    let mut slowest_index = 0;
    for (index, subworker) in subworkers.iter().enumerate() {
        if subworker.remaining() > subworkers[slowest_index].remaining() {
            slowest_index = index;
        }
    }

    let slowest = &subworkers[slowest_index];
    let (new_end, new_start, old_end) = split_remaining(slowest.position, slowest.end)?;
    subworkers[slowest_index].end = new_end;

    let new_subworker = Subworker::new(subworkers.len(), new_start, old_end);
    subworkers.insert(slowest_index + 1, new_subworker);
    Some(new_subworker)
}

/// 恢复后补齐线程数：补到 `min(subworker_count, file_size)`，切不动就停。
pub fn top_up_subworkers(subworkers: &mut Vec<Subworker>, file_size: u64, subworker_count: usize) {
    if file_size == 0 {
        return;
    }
    let target = subworker_count.min(file_size as usize);
    while subworkers.len() < target {
        if split_slowest(subworkers).is_none() {
            break;
        }
    }
}

/// 读 `.ghd` 恢复分片；文件缺失或损坏都返回空（Python 侧损坏时从零开始）。
pub fn load_subworkers(record_path: &Path) -> Vec<Subworker> {
    let records: Vec<PartRecord> = match load_records(record_path) {
        Ok(records) => records,
        Err(_) => return Vec::new(),
    };

    records
        .into_iter()
        .enumerate()
        .map(|(index, record)| Subworker {
            index,
            start: record.start,
            position: record.position,
            end: record.end as i64,
        })
        .collect()
}

/// 把当前进度写成 `.ghd`（只在范围请求模式下调用，`end` 必须是有效闭区间末尾）。
pub fn save_subworkers(record_path: &Path, subworkers: &[PartRecord]) -> io::Result<()> {
    gd3_core::ghd::save_records(record_path, subworkers)
}
