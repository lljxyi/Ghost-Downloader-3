//! Ghost Downloader 的 Rust 内核。
//!
//! 迁移期与 Python 实现共用同一份磁盘格式：两侧都能读写 `tasks.jsonl` 与 `.ghd`。

pub mod config;
pub mod container;
pub mod ed2k;
pub mod filename;
pub mod filesystem;
pub mod ghd;
pub mod github;
pub mod hls;
pub mod http_pack;
pub mod install;
pub mod loader;
pub mod m3u8;
pub mod pack;
pub mod pair_token;
pub mod path;
pub mod queue;
pub mod settings;
pub mod store;
pub mod task;
pub mod task_error;
pub mod version;

pub use container::{
    ContainerError, SegmentRange, build_mp4_segment_range, build_webm_segment_range,
    parse_mp4_boxes, parse_sidx,
};
pub use filename::{DEFAULT_MAX_FILENAME_LENGTH, split_stem_ext, to_safe_filename};
pub use filesystem::{
    delete_path, find_executable, is_existing, is_folder, local_file_path, to_posix_path,
};
pub use ghd::PartRecord;
pub use install::{delete_installed, install_file};
pub use loader::{
    DependencyError, DependencySkip, LoadReport, OrderReport, ScanReport, SeedReport, SeededPack,
    VersionRejection, load_packs, ordered_by_dependency, scan_packs, seed_packs,
};
pub use pack::{BinaryRuntime, FileType, ManifestError, PackManifest, UriScheme, VersionInfo};
pub use path::{posix_join, posix_normpath};
pub use queue::TaskQueue;
pub use store::{LoadOutcome, load_records, save_records, to_line};
pub use task::{Progress, TaskFile, TaskRecord, TaskStatus, TaskStep};
pub use task_error::{CANCELLED_MESSAGE, TaskError};
pub use version::{is_newer, parse_version};
