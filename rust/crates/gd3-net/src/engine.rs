//! HTTP 分片传输引擎：镜像 `features/http_pack/task.py`。
//!
//! 保持一致的行为：分片划分与 `.ghd` 续传、永久/临时错误区分、失败后 5 秒重试、
//! 服务器不支持范围请求时降级单流、每块回调速度与限流钩子。
//!
//! 与 Python 的结构差异：实时位置放在原子量里（Python 直接改 `receivedBytes`）；
//! 文件用 `std::fs` + pwrite 语义（Windows `seek_write` / Unix `write_at`）打开，
//! 与 Python 的 `os.open` + `pwrite` 一样是阻塞调用，不移动文件游标。

use crate::NetError;
use crate::plan::{Plan, Segment, SplitScheduler};
use crate::ranges::{
    END_NOT_SUPPORTED, END_UNKNOWN, Subworker, load_subworkers, save_subworkers, split_ranges,
    top_up_subworkers,
};
use futures_util::StreamExt;
use gd3_core::ghd::PartRecord;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio_util::sync::CancellationToken;
use wreq::header::{HeaderMap, HeaderName, HeaderValue};
use wreq_util::{Emulation, Platform, Profile};

/// 流读取超时（Python `STREAM_READ_TIMEOUT`）。
pub const STREAM_READ_TIMEOUT: Duration = Duration::from_secs(30);
/// 临时失败后的重试间隔（Python 里是 5 秒）。
pub const RETRY_DELAY: Duration = Duration::from_secs(5);
/// 探活请求超时（Python `buildClient(timeout=30)`）。
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
/// 重新切分的门槛默认值（Python `cfg.maxReassignSize` 默认 512，单位 KB）。
pub const DEFAULT_MAX_REASSIGN_SIZE: u64 = 512 * 1024;
/// 进度采样周期（Python `_supervise` 里的 `asyncio.sleep(1)`）。
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(1);

/// 这些状态码直接判永久失败，不重试（Python `PERMANENT_STATUS`）。
const PERMANENT_STATUS: [u16; 7] = [400, 401, 403, 404, 405, 410, 451];

/// 致命 IO 错误：立刻失败，不重试（Python `FATAL_IO_ERRNO` =
/// ENOSPC 28 / EDQUOT 122 / EROFS 30 / EIO 5，外加 39、112 两个 Windows 磁盘码）。
const FATAL_IO_ERRNO: [i32; 6] = [28, 122, 30, 5, 39, 112];

pub struct TransferProgress {
    pub received_bytes: u64,
    pub file_size: u64,
    pub progress: f64,
    pub speed: i64,
    /// 本次回调新增的字节数；每秒采样时为 0（Python 只在数据块上调用 `reportSpeed`）。
    pub chunk_delta: u64,
}

/// 全局限流钩子：每个数据块后调用一次，由调用方决定是否等待。
/// 全局限流钩子：每个数据块后调用一次，由调用方决定是否等待。
pub trait SpeedGate: Send + Sync {
    fn wait(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}
pub type ProgressCallback = Arc<dyn Fn(TransferProgress) + Send + Sync>;

/// 构建 HTTP 客户端。与 Python `buildClient` 同位置：TLS/HTTP2 指纹也在这里决定。
///
/// `total_timeout` 覆盖整请求（开始连接 → 响应体读完），`read_timeout` 只管读阶段。
/// 流式下载只能给后者：Python 侧同样只传 `read_timeout`，给总超时会掐死长下载。
///
/// `profile` 是应用的 client profile 名（`raw` / `auto` / `chrome` / `Chrome149` …）。
pub fn build_client(
    total_timeout: Option<Duration>,
    read_timeout: Option<Duration>,
    profile: &str,
) -> wreq::Client {
    let mut builder = wreq::Client::builder();
    if let Some(timeout) = total_timeout {
        builder = builder.timeout(timeout);
    }
    if let Some(timeout) = read_timeout {
        builder = builder.read_timeout(timeout);
    }
    if let Some(emulation) = resolve_emulation(profile) {
        builder = builder.emulation(emulation);
    }
    builder.build().expect("构建 HTTP 客户端失败")
}

/// 应用 profile 名 → wreq-util 身份。
///
/// `raw` 表示不做模拟；家族名取该家族最新版本；版本名精确匹配。
/// wreq-util 3.0.0-rc.14 没有 Edge 与 Android 系列身份，这类名字退回 Chrome 最新版（与 Python 侧不同，见 README）。
pub fn resolve_emulation(profile: &str) -> Option<Emulation> {
    let host = if cfg!(windows) {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::MacOS
    } else {
        Platform::Linux
    };

    let pinned = match profile {
        "raw" => return None,
        "" | "auto" | "chrome" => Profile::Chrome149,
        "firefox" => Profile::Firefox151,
        "safari" => Profile::Safari26_4,
        "opera" => Profile::Opera131,
        "okhttp" => Profile::OkHttp5,
        name => {
            let mut resolved = None;
            for candidate in PROFILE_NAMES {
                if candidate.0.eq_ignore_ascii_case(name) {
                    resolved = Some(candidate.1);
                    break;
                }
            }
            resolved?
        }
    };

    Some(Emulation::builder().profile(pinned).platform(host).build())
}

/// wreq-util 里可用的身份（家族名与版本名都列上，供 profile 字符串匹配）。
const PROFILE_NAMES: &[(&str, Profile)] = &[
    ("Chrome149", Profile::Chrome149),
    ("Chrome148", Profile::Chrome148),
    ("Chrome147", Profile::Chrome147),
    ("Firefox151", Profile::Firefox151),
    ("Firefox150", Profile::Firefox150),
    ("Safari26_4", Profile::Safari26_4),
    ("Safari26_3", Profile::Safari26_3),
    ("Opera131", Profile::Opera131),
    ("OkHttp5", Profile::OkHttp5),
];

pub struct HttpEngine {
    pub url: String,
    pub output_path: PathBuf,
    /// 额外请求头，键名大小写不敏感。
    pub headers: Vec<(String, String)>,
    pub user_agent: String,
    /// 应用的 client profile 名（`raw` / `auto` / `chrome` / `Chrome149` …），决定 TLS/HTTP2 指纹。
    pub client_profile: String,
    /// 0 表示大小未知（Python `SpecialFileSize.UNKNOWN`）。
    pub file_size: u64,
    pub can_use_range_requests: bool,
    pub subworker_count: usize,
    /// 容器片段下载时成品文件在远端文件里的偏移。
    pub http_byte_offset: u64,
    pub speed_gate: Option<Arc<dyn SpeedGate>>,
    pub on_progress: Option<ProgressCallback>,
    pub retry_delay: Duration,
    /// 读阶段的空闲超时：多久没有新数据就算停滞并重试（Python `readTimeout=STREAM_READ_TIMEOUT`）。
    /// 注意别把它设成 `ClientBuilder::timeout`——那是整请求超时，会掐死长下载。
    pub read_timeout: Duration,
    /// 重新切分的门槛：最慢那段剩余超过它才会被劈开（Python `cfg.maxReassignSize` × 1024）。
    pub max_reassign_size: u64,
    /// 自动加速开关（Python `cfg.autoSpeedUp`）。
    pub auto_speed_up: bool,
    /// 进度采样周期；生产是 1 秒，测试调小才跑得动。
    pub tick_interval: Duration,
    /// 上一次 `run` 结束后各分片的最终位置 `(start, position, end)`。
    pub finished_subworkers: Vec<(u64, u64, u64)>,
}

impl HttpEngine {
    pub fn new(url: impl Into<String>, output_path: impl Into<PathBuf>, file_size: u64) -> Self {
        HttpEngine {
            url: url.into(),
            output_path: output_path.into(),
            headers: Vec::new(),
            user_agent: String::new(),
            file_size,
            can_use_range_requests: true,
            subworker_count: 8,
            http_byte_offset: 0,
            speed_gate: None,
            on_progress: None,
            retry_delay: RETRY_DELAY,
            read_timeout: STREAM_READ_TIMEOUT,
            max_reassign_size: DEFAULT_MAX_REASSIGN_SIZE,
            auto_speed_up: true,
            tick_interval: DEFAULT_TICK_INTERVAL,
            finished_subworkers: Vec::new(),
            client_profile: String::new(),
        }
    }

    /// `{成品路径}.ghd`。
    pub fn record_path(&self) -> PathBuf {
        PathBuf::from(format!("{}.ghd", self.output_path.display()))
    }

    /// 带上 user-agent 的最终请求头（已有 user-agent 时不覆盖，大小写不敏感）。
    pub fn effective_headers(&self) -> Vec<(String, String)> {
        let mut headers = self.headers.clone();
        let has_user_agent = headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("user-agent"));
        if !self.user_agent.is_empty() && !has_user_agent {
            headers.push(("user-agent".to_string(), self.user_agent.clone()));
        }
        headers
    }

    /// 探活并取回重定向后的最终 URL（Python 的 `range: bytes=0-0` 探测）。
    pub async fn probe_url(&self) -> Result<String, NetError> {
        let mut headers = self.effective_headers();
        headers.push(("range".to_string(), "bytes=0-0".to_string()));
        headers.push(("accept-encoding".to_string(), "identity".to_string()));

        let client = build_client(Some(PROBE_TIMEOUT), None, &self.client_profile);
        let response = client
            .get(&self.url)
            .headers(to_header_map(&headers)?)
            .send()
            .await
            .map_err(NetError::io)?;

        let final_url = response.uri().to_string();
        let status = response.status().as_u16();
        if PERMANENT_STATUS.contains(&status) || has_cf_mitigated(response.headers()) {
            return Err(NetError::server_error(status));
        }
        Ok(final_url)
    }

    /// 跑完所有分片。成功返回 `Ok(())`；被取消时保留 `.ghd` 以便续传。
    pub async fn run(&mut self, cancel: CancellationToken) -> Result<(), NetError> {
        let effective_url = self.probe_url().await?;
        let headers = self.effective_headers();
        let record_path = self.record_path();

        let mut subworkers = if self.can_use_range_requests {
            load_subworkers(&record_path)
        } else {
            Vec::new()
        };
        let restored = !subworkers.is_empty();
        if restored {
            top_up_subworkers(&mut subworkers, self.file_size, self.subworker_count);
        } else {
            if !self.can_use_range_requests {
                let _ = std::fs::remove_file(&record_path);
            }
            subworkers = split_ranges(
                self.file_size,
                self.subworker_count,
                self.can_use_range_requests,
            );
        }

        let file = Arc::new(open_output(&self.output_path, self.can_use_range_requests)?);
        if !restored && self.file_size > 0 {
            // 预分配：等价于 Python 的 ftruncate(fd, fileSize)——Windows 上它先标稀疏
            mark_sparse(&file);
            let _ = file.set_len(self.file_size);
        }

        let mut can_use_range_requests = self.can_use_range_requests;
        let mut shared = Arc::new(Shared::new(&subworkers));

        loop {
            let context = Arc::new(RunContext {
                client: build_client(None, Some(self.read_timeout), &self.client_profile),
                url: effective_url.clone(),
                headers: headers.clone(),
                http_byte_offset: self.http_byte_offset,
                file: Arc::clone(&file),
                shared: Arc::clone(&shared),
                max_reassign_size: self.max_reassign_size as i64,
                auto_speed_up: self.auto_speed_up,
                tick_interval: self.tick_interval,
                cancel: cancel.clone(),
                gate: self.speed_gate.clone(),
                on_progress: self.on_progress.clone(),
                file_size: self.file_size,
                record_path: record_path.clone(),
                retry_delay: self.retry_delay,
                can_use_range_requests,
            });

            let outcomes = run_all_subworkers(Arc::clone(&context)).await;

            // 供调用方回写状态：各分片的最终位置（start, position, end）——
            // 重新切分过的计划在这里才定型，所以从计划里取而不是从开跑前的划分里取
            self.finished_subworkers = shared.plan.snapshot();

            if cancel.is_cancelled() {
                return Err(NetError::cancelled());
            }

            if let Some(status) = outcomes.iter().find_map(|outcome| match outcome {
                Err(FlowError::Permanent(status)) => Some(*status),
                _ => None,
            }) {
                return Err(NetError::server_error(status));
            }

            if let Some(error) = outcomes.iter().find_map(|outcome| match outcome {
                Err(FlowError::FatalIo(error)) => Some(error.clone()),
                _ => None,
            }) {
                return Err(error);
            }

            let range_unsupported = outcomes
                .iter()
                .any(|outcome| matches!(outcome, Err(FlowError::RangeNotSupported)));

            if range_unsupported && can_use_range_requests {
                // 降级单流：清空重来，丢掉分片记录
                can_use_range_requests = false;
                self.can_use_range_requests = false;
                subworkers = split_ranges(self.file_size, self.subworker_count, false);
                shared = Arc::new(Shared::new(&subworkers));
                let _ = file.set_len(0);
                let _ = std::fs::remove_file(&record_path);
                continue;
            }

            if let Some(error) = outcomes.iter().find_map(|outcome| match outcome {
                Err(FlowError::Transient(error)) => Some(error.clone()),
                _ => None,
            }) {
                return Err(error);
            }

            if can_use_range_requests {
                let _ = std::fs::remove_file(&record_path);
            }
            return Ok(());
        }
    }
}

struct Shared {
    /// 运行中的分片计划：可以边跑边切。
    plan: Plan,
    /// 新分片的启动错开（Python `_nextSplitStartAt`）。
    scheduler: SplitScheduler,
    received: AtomicU64,
    reported: AtomicU64,
    speed: AtomicI64,
}

impl Shared {
    fn new(subworkers: &[Subworker]) -> Self {
        Shared {
            plan: Plan::new(subworkers),
            scheduler: SplitScheduler::new(),
            received: AtomicU64::new(0),
            reported: AtomicU64::new(0),
            speed: AtomicI64::new(0),
        }
    }
}

struct RunContext {
    client: wreq::Client,
    url: String,
    headers: Vec<(String, String)>,
    http_byte_offset: u64,
    file: Arc<std::fs::File>,
    shared: Arc<Shared>,
    /// 重新切分的门槛（Python `cfg.maxReassignSize` × 1024，单位字节）。
    max_reassign_size: i64,
    /// 自动加速开关（Python `cfg.autoSpeedUp`）。
    auto_speed_up: bool,
    /// 进度采样周期（Python 里是 `asyncio.sleep(1)`）。
    tick_interval: Duration,
    cancel: CancellationToken,
    gate: Option<Arc<dyn SpeedGate>>,
    on_progress: Option<ProgressCallback>,
    file_size: u64,
    record_path: PathBuf,
    retry_delay: Duration,
    can_use_range_requests: bool,
}

enum FlowError {
    /// 永久失败：立刻返回，不重试。
    Permanent(u16),
    /// 服务器用 200 回应范围请求：降级单流。
    RangeNotSupported,
    /// 致命 IO（磁盘满等）：立刻返回。
    FatalIo(NetError),
    /// 被取消。
    Cancelled,
    /// 其它错误：按 Python 语义重试。
    Transient(NetError),
}

async fn run_all_subworkers(context: Arc<RunContext>) -> Vec<Result<(), FlowError>> {
    let (spawner, mut spawn_rx) = unbounded_channel::<(Arc<Segment>, Duration)>();
    let mut set = tokio::task::JoinSet::new();
    for segment in context.shared.plan.segments() {
        spawn_segment(&mut set, &context, &spawner, segment, Duration::ZERO);
    }

    let supervisor = {
        let task_context = Arc::clone(&context);
        let spawner = spawner.clone();
        tokio::spawn(async move { supervise(task_context, spawner).await })
    };

    let mut outcomes = Vec::new();
    loop {
        let joined = tokio::select! {
            joined = set.join_next() => joined,
            spawned = spawn_rx.recv() => {
                if let Some((segment, delay)) = spawned {
                    spawn_segment(&mut set, &context, &spawner, segment, delay);
                }
                continue;
            }
        };
        match joined {
            Some(joined) => outcomes.push(match joined {
                Ok(outcome) => outcome,
                Err(join_error) => Err(FlowError::Transient(NetError::io(join_error))),
            }),
            None => break,
        }
    }
    supervisor.abort();
    outcomes
}

async fn run_subworker(
    context: Arc<RunContext>,
    spawner: UnboundedSender<(Arc<Segment>, Duration)>,
    segment: Arc<Segment>,
    delay: Duration,
) -> Result<(), FlowError> {
    if !delay.is_zero() && sleep_or_cancel(delay, &context.cancel).await {
        return Err(FlowError::Cancelled);
    }

    if segment.end() == END_NOT_SUPPORTED {
        return run_single_stream(context, segment).await;
    }

    // 大小未知：开放区间收一轮就算完，不重新切分（Python 的 UNKNOWN 分支提前 return）
    if segment.end() == END_UNKNOWN {
        let mut position = segment.position();
        loop {
            let range = format!("bytes={}-", context.http_byte_offset + position);
            match stream_once(&context, &segment, &mut position, &range).await {
                Ok(()) => return Ok(()),
                Err(FlowError::Cancelled) => return Err(FlowError::Cancelled),
                Err(FlowError::Transient(_)) => {
                    if context.cancel.is_cancelled() {
                        return Err(FlowError::Cancelled);
                    }
                    // Python 在这里记日志后等 5 秒重试
                    if sleep_or_cancel(context.retry_delay, &context.cancel).await {
                        return Err(FlowError::Cancelled);
                    }
                }
                Err(other) => return Err(other),
            }
        }
    }

    // 固定区间：位置越过末尾就收工；末尾可能被重新切分改小，所以每轮都重新读
    let mut position = segment.position();
    while position <= segment.end() as u64 {
        let end = segment.end();
        let range = format!(
            "bytes={}-{}",
            context.http_byte_offset + position,
            context.http_byte_offset + end as u64
        );

        match stream_once(&context, &segment, &mut position, &range).await {
            Ok(()) => {}
            Err(FlowError::Cancelled) => return Err(FlowError::Cancelled),
            Err(FlowError::Transient(_)) => {
                if context.cancel.is_cancelled() {
                    return Err(FlowError::Cancelled);
                }
                if sleep_or_cancel(context.retry_delay, &context.cancel).await {
                    return Err(FlowError::Cancelled);
                }
            }
            Err(other) => return Err(other),
        }
    }

    // Python：收工时把 receivedBytes 夹到 end - start + 1，再偷最慢那段的后半
    let end = segment.end();
    if end > 0 {
        segment.position.store(end as u64 + 1, Ordering::Relaxed);
    }
    reassign(&context, &spawner);
    Ok(())
}

/// 单流模式：不支持范围请求时从零覆盖下载（Python 的 `NOT_SUPPORTED` 分支）。
async fn run_single_stream(
    context: Arc<RunContext>,
    segment: Arc<Segment>,
) -> Result<(), FlowError> {
    loop {
        let _ = context.file.set_len(0);
        let mut position = 0u64;
        segment.position.store(0, Ordering::Relaxed);

        let outcome = stream_body(
            &context,
            &segment,
            &mut position,
            context.headers.clone(),
            false,
        )
        .await;
        match outcome {
            Ok(()) => {
                let _ = context.file.set_len(position);
                return Ok(());
            }
            Err(FlowError::Cancelled) => return Err(FlowError::Cancelled),
            Err(FlowError::Permanent(status)) => return Err(FlowError::Permanent(status)),
            Err(FlowError::FatalIo(error)) => return Err(FlowError::FatalIo(error)),
            Err(_) => {
                if sleep_or_cancel(context.retry_delay, &context.cancel).await {
                    return Err(FlowError::Cancelled);
                }
            }
        }
    }
}

/// 固定区间：一次请求 + 收流；流提前结束时由调用方决定是否重来。
async fn stream_once(
    context: &Arc<RunContext>,
    segment: &Arc<Segment>,
    position: &mut u64,
    range: &str,
) -> Result<(), FlowError> {
    let mut headers = context.headers.clone();
    headers.push(("range".to_string(), range.to_string()));
    headers.push(("accept-encoding".to_string(), "identity".to_string()));
    stream_body(context, segment, position, headers, true).await
}

async fn stream_body(
    context: &Arc<RunContext>,
    segment: &Arc<Segment>,
    position: &mut u64,
    headers: Vec<(String, String)>,
    asks_range: bool,
) -> Result<(), FlowError> {
    if context.cancel.is_cancelled() {
        return Err(FlowError::Cancelled);
    }

    let request = context
        .client
        .get(&context.url)
        .headers(to_header_map(&headers).map_err(FlowError::Transient)?);

    let response = tokio::select! {
        response = request.send() => response,
        _ = context.cancel.cancelled() => return Err(FlowError::Cancelled),
    }
    .map_err(|error| FlowError::Transient(NetError::io(error)))?;

    let status = response.status().as_u16();
    if PERMANENT_STATUS.contains(&status) || has_cf_mitigated(response.headers()) {
        return Err(FlowError::Permanent(status));
    }
    if asks_range && status == 200 {
        return Err(FlowError::RangeNotSupported);
    }
    if asks_range && status != 206 {
        return Err(FlowError::Transient(NetError::range_rejected(status)));
    }
    if !asks_range && status != 200 {
        return Err(FlowError::Transient(NetError::unexpected_status(status)));
    }

    let mut stream = response.bytes_stream();
    loop {
        let next = tokio::select! {
            next = stream.next() => next,
            _ = context.cancel.cancelled() => return Err(FlowError::Cancelled),
        };

        let chunk = match next {
            Some(Ok(chunk)) => chunk,
            Some(Err(error)) => return Err(FlowError::Transient(NetError::io(error))),
            None => break,
        };
        if chunk.is_empty() {
            continue;
        }

        // 末尾每次重新读：重新切分会把它改小，正在收的流必须立刻照做
        let end = segment.end();
        let data = if end > 0 {
            let end = end as u64;
            if *position > end {
                break;
            }
            let allowed = end - *position + 1;
            if chunk.len() as u64 > allowed {
                &chunk[..allowed as usize]
            } else {
                &chunk
            }
        } else {
            &chunk[..]
        };

        write_all_at(&context.file, data, *position).map_err(classify_io_error)?;
        *position += data.len() as u64;
        segment.position.store(*position, Ordering::Relaxed);
        context
            .shared
            .received
            .fetch_add(data.len() as u64, Ordering::Relaxed);

        report_progress(context, data.len() as u64);
        if let Some(gate) = &context.gate {
            gate.wait().await;
        }
    }

    Ok(())
}

/// 每个采样周期：写 `.ghd`、算速度、回调进度，并驱动自动加速（Python 的 `_supervise`）。
async fn supervise(context: Arc<RunContext>, spawner: UnboundedSender<(Arc<Segment>, Duration)>) {
    let mut accelerator = Accelerator::default();
    loop {
        if context.can_use_range_requests {
            let records: Vec<PartRecord> = context
                .shared
                .plan
                .segments()
                .iter()
                .filter(|segment| segment.end() > 0)
                .map(|segment| PartRecord {
                    start: segment.start,
                    position: segment.position(),
                    end: segment.end() as u64,
                })
                .collect();
            let _ = save_subworkers(&context.record_path, &records);
        }

        report_progress(&context, 0);
        accelerator.tick(&context, &spawner);
        if sleep_or_cancel(context.tick_interval, &context.cancel).await {
            return;
        }
    }
}

fn report_progress(context: &Arc<RunContext>, chunk_delta: u64) {
    let received = context.shared.received.load(Ordering::Relaxed);
    let reported = context.shared.reported.swap(received, Ordering::Relaxed);
    context
        .shared
        .speed
        .store(received as i64 - reported as i64, Ordering::Relaxed);

    if let Some(callback) = &context.on_progress {
        let progress = if context.file_size > 0 {
            (received as f64 / context.file_size as f64) * 100.0
        } else {
            0.0
        };
        callback(TransferProgress {
            received_bytes: received,
            file_size: context.file_size,
            progress,
            speed: received as i64 - reported as i64,
            chunk_delta,
        });
    }
}

/// 等待指定时长；被取消时返回 true。
async fn sleep_or_cancel(duration: Duration, cancel: &CancellationToken) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(duration) => false,
        _ = cancel.cancelled() => true,
    }
}

/// 预分配前把文件标成稀疏（Python `ftruncate` 里的 `FSCTL_SET_SPARSE`）。
///
/// 不标的话 NTFS 会把新空间真的写零：下一个 10GB 的文件就先写 10GB 零，
/// 又慢又占盘。文件系统不支持时只是少了这个优化，所以错误一律忽略（Python 也只记一条 debug 日志）。
#[cfg(windows)]
fn mark_sparse(file: &std::fs::File) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::FSCTL_SET_SPARSE;

    let mut returned = 0u32;
    // SAFETY：句柄来自仍然活着的 `file`；in/out 缓冲都传空（FSCTL_SET_SPARSE 允许）
    unsafe {
        DeviceIoControl(
            file.as_raw_handle() as _,
            FSCTL_SET_SPARSE,
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        );
    }
}

#[cfg(not(windows))]
fn mark_sparse(_file: &std::fs::File) {}

fn open_output(path: &Path, can_use_range_requests: bool) -> Result<std::fs::File, NetError> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(NetError::io)?;
    }
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(!can_use_range_requests)
        .open(path)
        .map_err(NetError::io)
}

/// 致命 IO 错误码判定（Python `FATAL_IO_ERRNO`）。单独抽出来是为了能对着 Python 的集合测。
pub fn is_fatal_io_error(code: i32) -> bool {
    FATAL_IO_ERRNO.contains(&code)
}

fn classify_io_error(error: io::Error) -> FlowError {
    let is_fatal = error.raw_os_error().is_some_and(is_fatal_io_error);
    if is_fatal {
        FlowError::FatalIo(NetError::disk_full())
    } else {
        FlowError::Transient(NetError::io(error))
    }
}

/// pwrite 语义：不改文件游标，可被多个分片共享。
fn write_all_at(file: &std::fs::File, data: &[u8], offset: u64) -> io::Result<()> {
    let mut written = 0usize;
    while written < data.len() {
        let count = write_at(file, &data[written..], offset + written as u64)?;
        if count == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "写入返回 0 字节"));
        }
        written += count;
    }
    Ok(())
}

#[cfg(windows)]
fn write_at(file: &std::fs::File, data: &[u8], offset: u64) -> io::Result<usize> {
    use std::os::windows::fs::FileExt;
    file.seek_write(data, offset)
}

#[cfg(unix)]
fn write_at(file: &std::fs::File, data: &[u8], offset: u64) -> io::Result<usize> {
    use std::os::unix::fs::FileExt;
    file.write_at(data, offset)
}

pub(crate) fn to_header_map(headers: &[(String, String)]) -> Result<HeaderMap, NetError> {
    let mut map = HeaderMap::new();
    for (name, value) in headers {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(NetError::io)?;
        let value = HeaderValue::from_str(value).map_err(NetError::io)?;
        map.insert(name, value);
    }
    Ok(map)
}

fn has_cf_mitigated(headers: &HeaderMap) -> bool {
    headers.contains_key("cf-mitigated")
}

/// 把一个分段的下载任务排进 JoinSet（Python `_taskGroup.create_task(self._runSubworker(...))`）。
fn spawn_segment(
    set: &mut tokio::task::JoinSet<Result<(), FlowError>>,
    context: &Arc<RunContext>,
    spawner: &UnboundedSender<(Arc<Segment>, Duration)>,
    segment: Arc<Segment>,
    delay: Duration,
) {
    let task_context = Arc::clone(context);
    let task_spawner = spawner.clone();
    set.spawn(async move { run_subworker(task_context, task_spawner, segment, delay).await });
}

/// 一个分片收工后去偷最慢那段的后半（Python `_reassignSubworker`）。
///
/// 新段不会立刻开跑：`SplitScheduler` 用 `SPLIT_START_INTERVAL` 把启动时刻错开，
/// 免得几十个任务同时扑上去反而把速度压下来。
fn reassign(context: &Arc<RunContext>, spawner: &UnboundedSender<(Arc<Segment>, Duration)>) {
    if context.file_size == 0 {
        return;
    }
    let Some((segment, delay)) = context
        .shared
        .scheduler
        .split(&context.shared.plan, context.max_reassign_size)
    else {
        return;
    };
    let _ = spawner.send((segment, delay));
}

/// 自动加速的判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccelDecision {
    /// 样本不够、波动太大或还在 5 秒窗口里：这一轮不动。
    Wait,
    /// 第一次判定：先切 4 刀试探。
    Split,
    /// 提速划算，下一轮从头再试探。
    KeepGoing,
    /// 切了也不快：停手，不再切。
    Stop,
}

/// 自动加速（Python `_autoSpeedUp`）：速度稳定时多切几刀，切了不提速就停手。
#[derive(Default)]
struct Accelerator {
    /// 最近 5 个采样周期的速度。
    history: Vec<i64>,
    /// 第一轮试探的开始时刻；`None` 表示还没开始试探。
    check_started: Option<Instant>,
    initial_workers: usize,
    initial_speed: f64,
    /// 判定「切了也不快」之后就再也不切了。
    accelerated: bool,
}

impl Accelerator {
    fn tick(
        &mut self,
        context: &Arc<RunContext>,
        spawner: &UnboundedSender<(Arc<Segment>, Duration)>,
    ) {
        if !context.auto_speed_up {
            return;
        }
        let speed = context.shared.speed.load(Ordering::Relaxed);
        let workers = context.shared.plan.segment_count();
        if self.decide(speed, workers, Instant::now()) == AccelDecision::Split {
            for _ in 0..4 {
                reassign(context, spawner);
            }
        }
    }

    /// 判定（Python `_autoSpeedUp` 的全部分支）。与 `RunContext` 无关，方便逐分支测。
    fn decide(&mut self, speed: i64, workers: usize, now: Instant) -> AccelDecision {
        if self.accelerated {
            return AccelDecision::Wait;
        }

        self.history.push(speed);
        if self.history.len() > 5 {
            self.history.remove(0);
        }
        if self.history.len() < 5 {
            return AccelDecision::Wait;
        }

        let average = self.history.iter().sum::<i64>() as f64 / self.history.len() as f64;
        if average == 0.0 {
            return AccelDecision::Wait;
        }

        // 波动超过 15% 就说明样本不可比，这一轮不作判断
        let max_deviation = self
            .history
            .iter()
            .map(|speed| (*speed as f64 - average).abs() / average)
            .fold(0.0f64, f64::max);
        if max_deviation > 0.15 {
            return AccelDecision::Wait;
        }

        match self.check_started {
            None => {
                self.initial_workers = workers;
                self.initial_speed = average;
                self.check_started = Some(now);
                AccelDecision::Split
            }
            Some(started) => {
                // 给新分片 5 秒时间证明自己
                if now.saturating_duration_since(started) <= Duration::from_secs(5) {
                    return AccelDecision::Wait;
                }
                let worker_ratio =
                    (workers as f64 - self.initial_workers as f64) / self.initial_workers as f64;
                let speed_ratio = (average - self.initial_speed) / self.initial_speed;
                if speed_ratio < 0.8 * worker_ratio {
                    self.accelerated = true;
                    AccelDecision::Stop
                } else {
                    // 提速划算，下一轮从头再试探
                    self.check_started = None;
                    AccelDecision::KeepGoing
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 5 个样本 + 平稳 + 5 秒窗口：Python `_autoSpeedUp` 的每条分支。
    #[test]
    fn auto_speed_up_follows_pythons_branches() {
        let start = Instant::now();
        let mut accelerator = Accelerator::default();

        // 样本不够：前 4 次一律不动
        for _ in 0..4 {
            assert_eq!(
                accelerator.decide(1000, 2, start),
                AccelDecision::Wait,
                "样本不足 5 个不该下结论"
            );
        }
        // 第 5 个样本、速度平稳 → 先切 4 刀试探
        assert_eq!(accelerator.decide(1000, 2, start), AccelDecision::Split);
        // 5 秒窗口内不下结论
        assert_eq!(
            accelerator.decide(1000, 2, start + Duration::from_secs(4)),
            AccelDecision::Wait
        );
        // 段数翻倍（2→4）但速度只涨了 10% → 停手
        assert_eq!(
            accelerator.decide(1100, 4, start + Duration::from_secs(6)),
            AccelDecision::Stop
        );
        assert!(accelerator.accelerated);
        // 停手之后再也不切
        assert_eq!(
            accelerator.decide(1100, 4, start + Duration::from_secs(60)),
            AccelDecision::Wait
        );
    }
    /// 段数没变（切不动，例如剩余都不够门槛）时速度也没变：继续试探，不是停手。
    #[test]
    fn auto_speed_up_keeps_going_when_workers_do_not_grow() {
        let start = Instant::now();
        let mut accelerator = Accelerator::default();

        for _ in 0..5 {
            accelerator.decide(1000, 2, start);
        }
        assert_eq!(
            accelerator.decide(1000, 2, start + Duration::from_secs(6)),
            AccelDecision::KeepGoing
        );
        assert!(!accelerator.accelerated);
        assert!(accelerator.check_started.is_none(), "应当重新开始试探");
    }

    /// 速度样本波动超过 15% 时不下结论。
    #[test]
    fn auto_speed_up_waits_on_unstable_speed() {
        let start = Instant::now();
        let mut accelerator = Accelerator::default();

        for speed in [1000, 1000, 1000, 1000, 1400] {
            assert_eq!(accelerator.decide(speed, 2, start), AccelDecision::Wait);
        }
    }

    /// 速度全为 0（例如还没开始收数据）时不下结论，也避免除零。
    #[test]
    fn auto_speed_up_waits_when_speed_is_zero() {
        let start = Instant::now();
        let mut accelerator = Accelerator::default();

        for _ in 0..5 {
            assert_eq!(accelerator.decide(0, 2, start), AccelDecision::Wait);
        }
    }
}
