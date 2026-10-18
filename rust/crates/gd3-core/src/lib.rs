//! Ghost Downloader 的 Rust 内核。
//!
//! 迁移期与 Python 实现共用同一份磁盘格式：两侧都能读写 `tasks.jsonl` 与 `.ghd`。

pub mod checksum;
pub mod config;
pub mod container;
pub mod desktop;
pub mod ed2k;
pub mod file_association;
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
pub mod placeholder;
pub mod queue;
pub mod run_at_login;
pub mod settings;
pub mod store;
pub mod task;
pub mod task_error;
pub mod trash;
pub mod version;

pub use checksum::{
    CHUNK_SIZE, COMMON_ALGORITHMS, ChecksumError, SHAKE_LENGTHS, SUPPORTED_ALGORITHMS,
    match_checksum, parse_checksum, to_checksum,
};
pub use container::{
    ContainerError, SegmentRange, build_mp4_segment_range, build_webm_segment_range,
    parse_mp4_boxes, parse_sidx,
};
pub use desktop::{
    LINUX_BROWSERS, MACOS_BROWSERS, browser_url, open_chromium_url, open_chromium_url_on,
    open_file_command, open_file_command_on, open_folder_command, open_folder_command_on,
    restart_command, restart_command_on, reveal_in_folder, reveal_in_folder_on, shutdown_command,
    shutdown_command_on, sleep_command, sleep_command_on, spawn_detached,
};
pub use file_association::{
    MIME_PREFIX, desktop_entry_text, desktop_file_path, load_mime_types, register_file_types,
    register_url_scheme, save_mime_types, service_entry_text, service_file_path,
    unregister_url_scheme,
};
pub use filename::{DEFAULT_MAX_FILENAME_LENGTH, split_stem_ext, to_safe_filename};
pub use filesystem::{
    delete_path, find_executable, is_existing, is_folder, local_file_path, move_path,
    to_posix_path, which,
};
pub use ghd::PartRecord;
pub use install::{delete_installed, install_file};
pub use loader::{
    DependencyError, DependencySkip, LoadReport, OrderReport, ScanReport, SeedReport, SeededPack,
    VersionRejection, load_packs, ordered_by_dependency, scan_packs, seed_packs,
};
pub use pack::{BinaryRuntime, FileType, ManifestError, PackManifest, UriScheme, VersionInfo};
pub use path::{posix_join, posix_normpath};
pub use placeholder::{
    ConflictOutcome, CreateError, MAX_RENAME_ATTEMPTS, NameConflictChoice, PlaceholderRegistry,
    Placeholders, TakenPath, create_placeholders, delete_placeholder, placeholder_paths,
    placeholder_paths_of, resolve_conflict, sidecar_path,
};
pub use queue::TaskQueue;
pub use run_at_login::{
    LAUNCH_AGENT_LABEL, WINDOWS_RUN_KEY, WINDOWS_VALUE_NAME, autostart_entry_text,
    autostart_file_path, launch_agent_path, launch_agent_text, set_autostart, set_launch_agent,
    set_run_at_login, windows_run_value,
};
pub use store::{LoadOutcome, load_records, save_records, to_line};
pub use task::{Progress, TaskFile, TaskRecord, TaskStatus, TaskStep};
pub use task_error::{CANCELLED_MESSAGE, TaskError};
pub use trash::{
    MAX_ATTEMPTS, TrashLocation, choose_trash_dir, current_uid, delete_recoverably,
    deletion_date_at, deletion_date_now, encode_path, generic_data_dir, home_trash_dir,
    mount_point_of, move_to_trash, resolve_trash_dir, shared_trash_ok, topdir_trash_dir,
    trash_entry_name, trash_info_text,
};
pub use version::{is_newer, parse_version};
