//! 引擎集成测试：在本地 HTTP 服务器上验证分片下载、`.ghd` 续传、重试、降级与错误分类。
//!
//! 规格对应 `tests/test_http_subworker.py`。服务器实现见 `tests/support/test_server.rs`。

#[path = "support/test_server.rs"]
mod test_server;

use gd3_core::ghd::PartRecord;
use gd3_net::{CancellationToken, HttpEngine, NetError, TransferProgress};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_server::{Mode, TestServer};

/// 确定性伪随机内容，断言逐字节相等时才有意义。
fn content(size: usize) -> Vec<u8> {
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    (0..size)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

fn temp_path(name: &str) -> PathBuf {
    let folder = std::env::temp_dir().join("gd3-net-tests");
    std::fs::create_dir_all(&folder).expect("建临时目录失败");
    folder.join(name)
}

fn record_path(target: &Path) -> PathBuf {
    PathBuf::from(format!("{}.ghd", target.display()))
}

async fn download(
    server: &TestServer,
    target: &Path,
    subworker_count: usize,
) -> Result<(), NetError> {
    let mut engine = HttpEngine::new(server.url("/file"), target, server.content().len() as u64);
    engine.subworker_count = subworker_count;
    engine.run(CancellationToken::new()).await
}

fn requested_ranges(server: &TestServer) -> Vec<String> {
    server
        .requests()
        .iter()
        .filter_map(|request| request.range.clone())
        .collect()
}

#[tokio::test]
async fn multi_subworker_download_matches_content() {
    let data = content(100_000);
    let server = TestServer::start(data.clone()).await;
    let target = temp_path("multi.bin");
    let _ = std::fs::remove_file(&target);

    download(&server, &target, 8).await.expect("下载失败");

    assert_eq!(std::fs::read(&target).expect("读回失败"), data);
    let ranges = requested_ranges(&server);
    assert!(ranges.len() >= 2, "应该发出多个范围请求：{ranges:?}");
    assert!(
        ranges.iter().any(|range| range == "bytes=0-0"),
        "探活请求应带上 bytes=0-0：{ranges:?}"
    );
    assert!(!record_path(&target).exists(), "成功后必须删掉 .ghd");
}

#[tokio::test]
async fn single_subworker_download_matches_content() {
    let data = content(70_000);
    let server = TestServer::start(data.clone()).await;
    let target = temp_path("single.bin");
    let _ = std::fs::remove_file(&target);

    download(&server, &target, 1).await.expect("下载失败");
    assert_eq!(std::fs::read(&target).expect("读回失败"), data);
}

#[tokio::test]
async fn many_subworkers_download_matches_content() {
    let data = content(200_000);
    let server = TestServer::start(data.clone()).await;
    let target = temp_path("many.bin");
    let _ = std::fs::remove_file(&target);

    download(&server, &target, 16).await.expect("下载失败");
    assert_eq!(std::fs::read(&target).expect("读回失败"), data);
    assert!(
        requested_ranges(&server).len() >= 8,
        "16 个分片应有多个范围请求"
    );
}

#[tokio::test]
async fn resume_from_record_only_fetches_remaining_range() {
    let data = content(100_000);
    let server = TestServer::start(data.clone()).await;
    let target = temp_path("resume.bin");
    let record = record_path(&target);

    let half = 50_000u64;
    std::fs::write(&target, &data[..half as usize]).expect("写半成品失败");
    gd3_core::ghd::save_records(
        &record,
        &[
            PartRecord {
                start: 0,
                position: half,
                end: half - 1,
            },
            PartRecord {
                start: half,
                position: half,
                end: data.len() as u64 - 1,
            },
        ],
    )
    .expect("写记录失败");

    download(&server, &target, 2).await.expect("续传失败");

    assert_eq!(std::fs::read(&target).expect("读回失败"), data);
    let ranges = requested_ranges(&server);
    assert!(
        ranges
            .iter()
            .any(|range| range == &format!("bytes={half}-{}", data.len() - 1)),
        "只该请求剩下的后半段：{ranges:?}"
    );
    assert!(!record.exists(), "完成后必须删掉 .ghd");
}

#[tokio::test]
async fn resume_with_increased_thread_count() {
    let data = content(100_000);
    let server = TestServer::start(data.clone()).await;
    let target = temp_path("resume-more.bin");
    let record = record_path(&target);

    let quarter = 25_000u64;
    std::fs::write(&target, &data[..quarter as usize]).expect("写半成品失败");
    gd3_core::ghd::save_records(
        &record,
        &[
            PartRecord {
                start: 0,
                position: quarter,
                end: 49_999,
            },
            PartRecord {
                start: 50_000,
                position: 50_000,
                end: data.len() as u64 - 1,
            },
        ],
    )
    .expect("写记录失败");

    // 恢复后补齐到 4 个分片：把最慢的两段继续劈开
    download(&server, &target, 4).await.expect("续传失败");
    assert_eq!(std::fs::read(&target).expect("读回失败"), data);
}

#[tokio::test]
async fn corrupt_record_starts_fresh() {
    let data = content(60_000);
    let server = TestServer::start(data.clone()).await;
    let target = temp_path("corrupt.bin");
    let record = record_path(&target);

    std::fs::write(&record, [0u8; 25]).expect("写坏记录失败");
    download(&server, &target, 2).await.expect("重下失败");

    assert_eq!(std::fs::read(&target).expect("读回失败"), data);
    assert!(!record.exists());
}

#[tokio::test]
async fn transient_500_retries_then_succeeds() {
    let data = content(50_000);
    let server = TestServer::start_with(data.clone(), Mode::FailTimes(2)).await;
    let target = temp_path("retry.bin");
    let _ = std::fs::remove_file(&target);

    let mut engine = HttpEngine::new(server.url("/file"), &target, data.len() as u64);
    engine.subworker_count = 1;
    engine.retry_delay = Duration::from_millis(50);

    engine
        .run(CancellationToken::new())
        .await
        .expect("重试后应当成功");
    assert_eq!(std::fs::read(&target).expect("读回失败"), data);
    assert!(server.requests().len() >= 3, "应看到 500 之后的重试请求");
}

#[tokio::test]
async fn range_unsupported_falls_back_to_single_stream() {
    let data = content(80_000);
    let server = TestServer::start_with(data.clone(), Mode::IgnoreRange).await;
    let target = temp_path("fallback.bin");
    let _ = std::fs::remove_file(&target);

    download(&server, &target, 4).await.expect("降级后应当成功");

    assert_eq!(std::fs::read(&target).expect("读回失败"), data);
    assert!(!record_path(&target).exists(), "降级单流不写 .ghd");
}

#[tokio::test]
async fn forbidden_is_permanent_and_not_retried() {
    let server = TestServer::start_with(content(1_000), Mode::Forbidden).await;
    let target = temp_path("forbidden.bin");

    let error = download(&server, &target, 4)
        .await
        .expect_err("403 应当失败");

    assert_eq!(error.message, "服务器返回了错误（{status}）");
    assert_eq!(error.params, vec![("status", "403".to_string())]);
    assert_eq!(error.format(), "服务器返回了错误（403）");
    assert_eq!(
        server.requests().len(),
        1,
        "探活就被拒，不该有重试：{:?}",
        server.requests()
    );
}

#[tokio::test]
async fn progress_callback_reports_totals() {
    let data = content(90_000);
    let server = TestServer::start(data.clone()).await;
    let target = temp_path("progress.bin");
    let _ = std::fs::remove_file(&target);

    let seen: Arc<Mutex<Vec<TransferProgress>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);

    let mut engine = HttpEngine::new(server.url("/file"), &target, data.len() as u64);
    engine.subworker_count = 4;
    engine.on_progress = Some(Arc::new(move |progress| {
        sink.lock().expect("锁失败").push(progress);
    }));

    engine
        .run(CancellationToken::new())
        .await
        .expect("下载失败");

    let seen = seen.lock().expect("锁失败");
    assert!(!seen.is_empty(), "应当收到进度回调");
    let last = seen.last().expect("至少一次回调");
    assert_eq!(last.received_bytes, data.len() as u64);
    assert_eq!(last.file_size, data.len() as u64);
    assert!(
        (last.progress - 100.0).abs() < 0.001,
        "进度应到 100：{}",
        last.progress
    );
}

#[tokio::test]
async fn cancelled_run_keeps_resume_record() {
    let data = content(40_000);
    let server = TestServer::start(data.clone()).await;
    let target = temp_path("cancel.bin");
    let record = record_path(&target);

    gd3_core::ghd::save_records(
        &record,
        &[PartRecord {
            start: 0,
            position: 10_000,
            end: data.len() as u64 - 1,
        }],
    )
    .expect("写记录失败");

    let cancel = CancellationToken::new();
    cancel.cancel();

    let mut engine = HttpEngine::new(server.url("/file"), &target, data.len() as u64);
    engine.subworker_count = 1;
    let error = engine.run(cancel).await.expect_err("取消应当报错");

    assert_eq!(error.message, "下载已取消");
    assert!(record.exists(), "暂停必须保留 .ghd 以便续传");
}

/// 读超时是**空闲**超时：正文总时长超过读超时，但只要每片之间不断流就该成功。
///
/// 这条测试盯的是一个真 bug：把 `read_timeout` 设成 `ClientBuilder::timeout`（整请求超时）
/// 会让所有超过该时长的下载失败，Python 侧用的是 `read_timeout`。
#[tokio::test]
async fn slow_but_progressing_body_survives_the_read_timeout() {
    let data = content(20_000);
    // 10 片 × 60ms ≈ 600ms 总时长，每片间隔远小于读超时
    let server = TestServer::start_with(
        data.clone(),
        Mode::Slow {
            piece: 2_000,
            gap: Duration::from_millis(60),
        },
    )
    .await;
    let target = temp_path("slow.bin");
    let _ = std::fs::remove_file(&target);

    let mut engine = HttpEngine::new(server.url("/file"), &target, data.len() as u64);
    engine.subworker_count = 1;
    engine.read_timeout = Duration::from_millis(200);
    engine
        .run(CancellationToken::new())
        .await
        .expect("慢速下载不该失败");

    assert_eq!(std::fs::read(&target).expect("读回失败"), data);
    assert_eq!(
        requested_ranges(&server).len(),
        2,
        "探活 1 次 + 正文 1 次，不该因读超时重来"
    );
}

/// 真正停滞（发一半就不动）时读超时必须生效：分片放弃当前流并按 `retry_delay` 重试。
#[tokio::test]
async fn stalled_stream_is_abandoned_and_retried() {
    let data = content(20_000);
    let server = TestServer::start_with(data.clone(), Mode::Stall).await;
    let target = temp_path("stall.bin");
    let _ = std::fs::remove_file(&target);

    let cancel = CancellationToken::new();
    let mut engine = HttpEngine::new(server.url("/file"), &target, data.len() as u64);
    engine.subworker_count = 1;
    engine.read_timeout = Duration::from_millis(150);
    engine.retry_delay = Duration::from_millis(50);

    let watcher = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(700)).await;
            cancel.cancel();
        })
    };
    let error = engine
        .run(cancel)
        .await
        .expect_err("停滞的流不该被当成下载成功");
    watcher.abort();

    assert_eq!(error.message, "下载已取消");
    let attempts = requested_ranges(&server)
        .into_iter()
        .filter(|range| range != "bytes=0-0")
        .count();
    assert!(
        attempts >= 2,
        "读超时后应当重试同一条范围，实际只发了 {attempts} 次"
    );
}

/// 致命 IO 错误码必须与 Python 的 `FATAL_IO_ERRNO` 一字不差。
///
/// Python 侧（`features/http_pack/task.py`）是
/// `{errno.ENOSPC, errno.EDQUOT, errno.EROFS, errno.EIO, 39, 112}`：
/// 这几个码一出就立刻失败，不重试——重试磁盘满只会白等。
#[test]
fn fatal_io_errno_matches_python() {
    // ENOSPC / EDQUOT / EROFS / EIO + 39、112（Windows 磁盘相关）
    for code in [28, 122, 30, 5, 39, 112] {
        assert!(
            gd3_net::is_fatal_io_error(code),
            "{code} 应当是致命 IO 错误"
        );
    }
    // 别的 IO 错误仍然按临时失败重试（32 = Windows 共享冲突）
    for code in [32, 2, 13, 0] {
        assert!(
            !gd3_net::is_fatal_io_error(code),
            "{code} 不该被当成致命错误"
        );
    }
}

/// 一个分片收工后会去偷最慢那段的后半（Python `test_reassignment_during_download`）。
///
/// 造一份「第一段只剩 100 字节、第二段还有 300KB」的 `.ghd`：第一段几乎立刻收工，
/// 收工时第二段还剩一大截，于是它被劈成两半、后半段晚 0.2 秒开跑。
#[tokio::test]
async fn finished_subworker_splits_the_slowest_one() {
    let data = content(400_000);
    // 慢速发正文：第二段要跑一秒多，第一段收工时它肯定还没跑完
    let server = TestServer::start_with(
        data.clone(),
        Mode::Slow {
            piece: 4_096,
            gap: Duration::from_millis(10),
        },
    )
    .await;
    let target = temp_path("reassign.bin");
    let record = record_path(&target);
    let _ = std::fs::remove_file(&target);

    std::fs::write(&target, &data).expect("写半成品失败");
    gd3_core::ghd::save_records(
        &record,
        &[
            PartRecord {
                start: 0,
                position: 99_900,
                end: 99_999,
            },
            PartRecord {
                start: 100_000,
                position: 100_000,
                end: 399_999,
            },
        ],
    )
    .expect("写记录失败");

    let mut engine = HttpEngine::new(server.url("/file"), &target, data.len() as u64);
    engine.subworker_count = 2;
    engine.max_reassign_size = 32 * 1024;
    engine.auto_speed_up = false;
    engine
        .run(CancellationToken::new())
        .await
        .expect("下载失败");

    assert_eq!(
        std::fs::read(&target).expect("读回失败"),
        data,
        "切分不能漏字节"
    );
    let segments = engine.finished_subworkers.len();
    assert!(
        segments >= 3,
        "第一段收工后应当把最慢那段劈开，实际只有 {segments} 段"
    );
    let ranges = requested_ranges(&server);
    assert!(
        ranges
            .iter()
            .any(|range| range.starts_with("bytes=250000-")),
        "被劈出来的后半段应当从 250000 开始请求：{ranges:?}"
    );
}

/// 门槛比任何一段的剩余都大时，谁收工都偷不到东西：段数保持不变。
#[tokio::test]
async fn no_split_when_every_segment_is_below_the_threshold() {
    let data = content(400_000);
    let server = TestServer::start_with(
        data.clone(),
        Mode::Slow {
            piece: 2_000,
            gap: Duration::from_millis(5),
        },
    )
    .await;
    let target = temp_path("no-split.bin");
    let _ = std::fs::remove_file(&target);

    let mut engine = HttpEngine::new(server.url("/file"), &target, data.len() as u64);
    engine.subworker_count = 4;
    // 4MB 门槛：任何一段的剩余都远小于它
    engine.max_reassign_size = 4 * 1024 * 1024;
    engine.tick_interval = Duration::from_millis(10);
    engine
        .run(CancellationToken::new())
        .await
        .expect("下载失败");

    assert_eq!(std::fs::read(&target).expect("读回失败"), data);
    assert_eq!(
        engine.finished_subworkers.len(),
        4,
        "剩余都不够门槛时不该多切段"
    );
}

/// 预分配必须标稀疏：NTFS 上不标就会把新空间真的写零。
///
/// 用「取消掉的下载」把流程停在预分配之后：文件已经拉到 `file_size`，
/// 但一字节都没写，所以占用大小应当远小于逻辑大小。
#[cfg(windows)]
#[tokio::test]
async fn preallocation_marks_the_file_sparse() {
    let size = 8 * 1024 * 1024u64;
    let server = TestServer::start(content(size as usize)).await;
    let target = temp_path("sparse.bin");
    let _ = std::fs::remove_file(&target);
    // 上一次跑留下的 .ghd 会让引擎走「续传」分支而跳过预分配
    let _ = std::fs::remove_file(record_path(&target));
    let cancel = CancellationToken::new();
    cancel.cancel();
    let mut engine = HttpEngine::new(server.url("/file"), &target, size);
    let error = engine.run(cancel).await.expect_err("取消应当报错");
    assert_eq!(error.message, "下载已取消");

    let file = std::fs::File::open(&target).expect("打开失败");
    assert_eq!(file.metadata().expect("元数据失败").len(), size);
    let allocated = allocated_size(&file);
    assert!(
        allocated < size / 4,
        "预分配应当是稀疏的：逻辑 {size} 字节，实际占用 {allocated} 字节"
    );
}

/// 文件的**实际占用**大小（`FILE_STANDARD_INFO.AllocationSize`）。
#[cfg(windows)]
fn allocated_size(file: &std::fs::File) -> u64 {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_STANDARD_INFO, FileStandardInfo, GetFileInformationByHandleEx,
    };

    let mut info: FILE_STANDARD_INFO = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle() as _,
            FileStandardInfo,
            std::ptr::from_mut(&mut info).cast(),
            std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
        )
    };
    assert_ne!(ok, 0, "取分配大小失败");
    info.AllocationSize as u64
}
