//! HTTP 传输引擎：镜像 `features/http_pack/task.py` 的分片下载语义。

pub mod bili;
pub mod engine;
pub mod ftp;
pub mod github;
pub mod hf;
pub mod plan;
pub mod probe;
pub mod ranges;
pub mod sources;

pub use engine::{
    HttpEngine, ProgressCallback, SpeedGate, TransferProgress, build_client, is_fatal_io_error,
};
pub use plan::{Plan, SPLIT_START_INTERVAL, Segment, SplitScheduler};
pub use ranges::{
    END_NOT_SUPPORTED, END_UNKNOWN, Subworker, load_subworkers, save_subworkers, split_ranges,
    split_slowest, top_up_subworkers,
};
pub use tokio_util::sync::CancellationToken;

/// 任务错误：定义在 `gd3-core`（CONTEXT.md 的 Task Error），这里沿用迁移期的名字。
pub use gd3_core::{CANCELLED_MESSAGE, TaskError as NetError};
