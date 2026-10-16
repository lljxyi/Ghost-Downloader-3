//! Ghost Downloader 的 Rust 内核。
//!
//! 迁移期与 Python 实现共用同一份磁盘格式：两侧都能读写 `tasks.jsonl` 与 `.ghd`。

pub mod container;
pub mod filename;
pub mod ghd;
pub mod store;
pub mod task;
pub mod task_error;

pub use container::{
    ContainerError, SegmentRange, build_mp4_segment_range, build_webm_segment_range,
    parse_mp4_boxes, parse_sidx,
};
pub use filename::{DEFAULT_MAX_FILENAME_LENGTH, split_stem_ext, to_safe_filename};
pub use ghd::PartRecord;
pub use store::{LoadOutcome, load_records, save_records, to_line};
pub use task::{Progress, TaskFile, TaskRecord, TaskStatus, TaskStep};
pub use task_error::{CANCELLED_MESSAGE, TaskError};
