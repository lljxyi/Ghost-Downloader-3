//! 更新源的网络半边：镜像 `app/sources.py` 的七个取数函数 + `app/client.py::fetchFile`。
//!
//! 第 114 步的 `gd3_core::sources` 只搬了纯逻辑（URL 拼接、失败文案、挑选口径）；
//! 这一步补网络：竞速、`gather` 合并、流式落盘。
//!
//! | Python（`app/sources.py`） | 这里 |
//! |---|---|
//! | `fetchJson(repo, branch, path)` :135 | [`fetch_json`] |
//! | `fetchLatestRelease(repo)` :101 | [`fetch_latest_release`] |
//! | `probeDownloadUrl(repo, tag, asset)` :191 | [`probe_download_url`] |
//! | `fetchReleaseAsset(repo, tag, asset, path, onProgress)` :204 | [`fetch_release_asset`] |
//! | `fetchPypiReleases(package)` :241 | [`fetch_pypi_releases`] |
//! | `fetchPypiFile(url, path, onProgress)` :265 | [`fetch_pypi_file`] |
//! | `app/client.py::fetchFile(url, path, onProgress)` :162 | [`write_stream`]（流式半边）+ [`Transport::download`] |
//!
//! `fetchRawFile`（:162）没有对应函数：`gd3-core` 里的 raw 下载走 `HttpEngine`，
//! 那条路径已经有自己的重试与分片语义，不该被这里的「Range 探测 + 单流」覆盖。
//!
//! ## 结构决策
//!
//! - **HTTP 走 [`Transport`] 抽象**：真实现是 [`WreqTransport`]，测试注入桩。没有这层，
//!   「竞速先完成者胜」「`gather` 后按 `SOURCE_ORDER` 合并」这些口径只能靠真网络验证。
//! - **`Release` 不搬过来**：`app/sources.py::fetchLatestRelease` 返回 `Release`，而
//!   `Release` 定义在 `gd3-ui::release_info`（第 68 步）。`gd3-net` 不能反向依赖 `gd3-ui`，
//!   所以 [`fetch_latest_release`] 返回**原始 JSON + 源名**，`Release::from_response`
//!   留给 UI 层调用。
//! - **`staggered_race` 不 spawn**：用 `FuturesUnordered` + `select!` 手写，所以 attempt
//!   可以借用 `&dyn Transport`，不必先 `Arc` 一遍。胜出即 `return`，其余 future 随
//!   `FuturesUnordered` 一起 drop —— 与 Python 取消输家等价。
//! - **`fetchFile` 的流循环抽成 [`write_stream`]**：真正的 `wreq` 调用只做「建 client →
//!   发请求 → 拿 `bytes_stream()`」，剩下的空块跳过、进度百分比、父目录创建都能离线测。
//!
//! ## 与 Python 的已知差异
//!
//! - **`content-length` 解析失败当 0**：Python 的 `int(headers.get("content-length", b"0"))`
//!   遇到非数字会抛 `ValueError` 把整个下载打断；这里退回 0（= 不回调进度）。
//! - **非 2xx 一律 `Status`**：Python 的 `raise_for_status()` 只对 4xx/5xx 抛，3xx 会
//!   继续 `resp.json()`。这里 `>= 400` 才判错，与 `gd3-net::hf` 的口径一致。
//! - **未知源的 URL 拼接被跳过**：`build_download_url` / `build_raw_url` 对未知源返回
//!   `None`（第 114 步的刻意设计，Python 是 `KeyError`），所以那种源不会进竞速。
//! - **`fetchLatestRelease` 的版本提取只认字符串**：Python 是 `str(data.get(key) or "")`，
//!   数字会被 `str()` 化；这里非字符串当空串（`{"tag_name": 5}` 这种输入不存在）。

use crate::NetError;
use crate::engine::build_client;
use futures_util::stream::FuturesUnordered;
use futures_util::{Stream, StreamExt};
use gd3_core::sources::{
    Repo, SOURCE_ORDER, STAGGER_DELAY, build_download_url, build_pypi_url, build_raw_url,
    download_failure, endpoints, json_failure, latest_release_failure, merge_pypi_releases,
    newest_by_version, pypi_file_candidates, pypi_file_failure, pypi_releases_failure,
};
use serde_json::Value;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::time::Duration;

/// `buildClient(timeout=15)`：JSON / release 接口的整体超时。
pub const JSON_TIMEOUT: Duration = Duration::from_secs(15);
/// `buildClient(timeout=10)`：`Range: bytes=0-0` 探测的整体超时。
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// `fetchFile` 的 `buildClient(readTimeout=30)`：只限读阶段，不掐总时长。
pub const FETCH_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// `Range: bytes=0-0` —— 所有「探测」请求共用的头。
const RANGE_HEADERS: [(&str, &str); 1] = [("Range", "bytes=0-0")];

/// `staggered_race` 的起跑间隔（秒 → `Duration`）。
fn stagger_delay() -> Duration {
    Duration::from_secs_f64(STAGGER_DELAY)
}

/// 探测请求的头（`{"Range": "bytes=0-0"}`）。
fn range_headers() -> Vec<(String, String)> {
    RANGE_HEADERS
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

// ---------------------------------------------------------------------------
// 传输层
// ---------------------------------------------------------------------------

/// 单次请求的失败原因。
///
/// Python 里这些全是异常（`raise_for_status` 的 `HTTPStatusError`、各种传输异常），
/// `sources.py` 逐个 `except Exception` 后 `raise`，最终由竞速/gather 统一丢弃。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// `raise_for_status()`：`>= 400` 的状态码。
    Status(u16),
    /// 网络、解码或落盘失败。
    Transport(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Status(status) => write!(formatter, "HTTP {status}"),
            FetchError::Transport(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for FetchError {}

/// 装箱后的请求 future（trait 方法要返回它，才能对 `dyn Transport` 用）。
pub type BoxFetch<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// 「按 URL 取数据」的三件事，`app/client.py` 的调用点各取所需。
///
/// 真实现是 [`WreqTransport`]；测试注入桩即可覆盖竞速、合并与落盘口径。
pub trait Transport: Send + Sync {
    /// `resp = await client.get(url); resp.raise_for_status(); await resp.json()`。
    fn get_json<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(String, String)],
        timeout: Duration,
    ) -> BoxFetch<'a, Result<Value, FetchError>>;

    /// `resp = await client.get(url); resp.raise_for_status()`，响应体立刻丢弃。
    fn probe<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(String, String)],
        timeout: Duration,
    ) -> BoxFetch<'a, Result<(), FetchError>>;

    /// `fetchFile`：流式落盘，`progress` 收 0–100 的百分比。
    fn download<'a>(
        &'a self,
        url: &'a str,
        output: &'a Path,
        progress: Option<&'a (dyn Fn(f64) + Send + Sync)>,
    ) -> BoxFetch<'a, Result<(), FetchError>>;
}

/// `wreq` 实现：client 的指纹/profile 由 `gd3_net::build_client` 决定。
pub struct WreqTransport {
    profile: String,
}

impl WreqTransport {
    /// `profile` 是应用的 client profile 名（`raw` / `auto` / `chrome` …）。
    pub fn new(profile: impl Into<String>) -> Self {
        Self {
            profile: profile.into(),
        }
    }

    /// 构造时用的 profile 名。
    pub fn profile(&self) -> &str {
        &self.profile
    }
}

impl Transport for WreqTransport {
    fn get_json<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(String, String)],
        timeout: Duration,
    ) -> BoxFetch<'a, Result<Value, FetchError>> {
        Box::pin(async move {
            let client = build_client(Some(timeout), None, &self.profile);
            let request = client
                .get(url)
                .headers(crate::engine::to_header_map(headers).map_err(describe)?);
            let response = request.send().await.map_err(describe)?;
            let status = response.status().as_u16();
            if status >= 400 {
                return Err(FetchError::Status(status));
            }
            let bytes = response.bytes().await.map_err(describe)?;
            serde_json::from_slice(&bytes).map_err(describe)
        })
    }

    fn probe<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(String, String)],
        timeout: Duration,
    ) -> BoxFetch<'a, Result<(), FetchError>> {
        Box::pin(async move {
            let client = build_client(Some(timeout), None, &self.profile);
            let request = client
                .get(url)
                .headers(crate::engine::to_header_map(headers).map_err(describe)?);
            let response = request.send().await.map_err(describe)?;
            let status = response.status().as_u16();
            if status >= 400 {
                return Err(FetchError::Status(status));
            }
            // Python 的 `resp.close()`：这里 drop 掉即可
            Ok(())
        })
    }

    fn download<'a>(
        &'a self,
        url: &'a str,
        output: &'a Path,
        progress: Option<&'a (dyn Fn(f64) + Send + Sync)>,
    ) -> BoxFetch<'a, Result<(), FetchError>> {
        Box::pin(async move {
            // 只给读超时：`fetchFile` 传的是 `readTimeout=30`
            let client = build_client(None, Some(FETCH_READ_TIMEOUT), &self.profile);
            let response = client.get(url).send().await.map_err(describe)?;
            let status = response.status().as_u16();
            if status >= 400 {
                return Err(FetchError::Status(status));
            }
            let total = content_length(response.headers());
            write_stream(output, total, response.bytes_stream(), progress).await
        })
    }
}

/// 任何 `Display` 错误 → [`FetchError::Transport`]。
fn describe(error: impl std::fmt::Display) -> FetchError {
    FetchError::Transport(error.to_string())
}

/// `int(response.headers.get("content-length", b"0"))`。
///
/// 差异：非数字当 0（Python 会抛 `ValueError`），见模块头。
fn content_length(headers: &wreq::header::HeaderMap) -> u64 {
    headers
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

/// `app/client.py::fetchFile` 的流循环：建父目录 → 逐块落盘 → 回调百分比。
///
/// 空块跳过（`if not chunk: continue`）；`total == 0` 时不回调
/// （Python 的 `if onProgress and total`）。
pub async fn write_stream<S, B, E>(
    output: &Path,
    total: u64,
    stream: S,
    progress: Option<&(dyn Fn(f64) + Send + Sync)>,
) -> Result<(), FetchError>
where
    S: Stream<Item = Result<B, E>>,
    B: AsRef<[u8]>,
    E: std::fmt::Display,
{
    if let Some(parent) = output.parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent).await.map_err(describe)?;
    }

    let mut file = tokio::fs::File::create(output).await.map_err(describe)?;
    let mut received: u64 = 0;
    let mut failure: Option<FetchError> = None;
    let mut stream = std::pin::pin!(stream);
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                failure = Some(describe(error));
                break;
            }
        };
        let data = chunk.as_ref();
        if data.is_empty() {
            continue;
        }
        tokio::io::AsyncWriteExt::write_all(&mut file, data)
            .await
            .map_err(describe)?;
        received += data.len() as u64;
        if let Some(callback) = progress
            && total != 0
        {
            callback(received as f64 / total as f64 * 100.0);
        }
    }
    // 出错也要 flush：Python 的 `with open(...)` 在异常路径上同样 `close()`，
    // 而 `tokio::fs::File` 自带缓冲，不 flush 会丢掉最后一段。
    tokio::io::AsyncWriteExt::flush(&mut file)
        .await
        .map_err(describe)?;
    match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// 竞速
// ---------------------------------------------------------------------------

/// 一次竞速里单个 attempt 的结果：`(下标, 结果)`。
type Attempt<'a, T> = Pin<Box<dyn Future<Output = (usize, Result<T, FetchError>)> + Send + 'a>>;

/// `asyncio.staggered_race` 的等价物：按 `delay` 逐个起跑，第一个 `Ok` 胜出。
///
/// 失败立刻起下一个（Python 的 `await wait(futures, timeout=delay)` 会在任何一个
/// 完成时返回，不必等满 `delay`）；胜出后其余 future 随 `FuturesUnordered` drop 掉，
/// 等价于 Python 取消输家。全部失败 → `None`。
async fn race_first_ok<'a, T>(
    count: usize,
    delay: Duration,
    mut make: impl FnMut(usize) -> Attempt<'a, T>,
) -> Option<(usize, T)>
where
    T: 'a,
{
    if count == 0 {
        return None;
    }

    let mut inflight: FuturesUnordered<Attempt<'a, T>> = FuturesUnordered::new();
    // 第一个立即起跑。**不能**靠 `sleep(Duration::ZERO)` 当「立即」：tokio 的
    // 定时器有 1ms 粒度，零时长 sleep 首次 poll 仍是 Pending，而空队列的
    // `inflight.next()` 会立刻给出 `None` —— `biased` 会选中后者直接退出。
    inflight.push(make(0));
    let mut next = 1usize;
    // `Sleep` 不是 `Unpin`：装进 `Box::pin` 才能反复 `reset` 后借给 `select!`
    let mut wait = Box::pin(tokio::time::sleep(delay));

    loop {
        tokio::select! {
            biased;
            // 起跑：之后每个间隔 `delay`
            _ = &mut wait, if next < count => {
                inflight.push(make(next));
                next += 1;
                wait.as_mut().reset(tokio::time::Instant::now() + delay);
            }
            completed = inflight.next() => {
                match completed {
                    None => break,
                    Some((index, Ok(value))) => return Some((index, value)),
                    Some((_, Err(_))) => {
                        // 失败：立刻补上下一个起跑位（不等满 delay）
                        if next < count {
                            inflight.push(make(next));
                            next += 1;
                            wait.as_mut().reset(tokio::time::Instant::now() + delay);
                        }
                    }
                }
            }
        }
    }
    None
}

/// `build_sources()` + 按源拼 URL，拼不出来的源直接跳过（未知源，见模块头）。
fn raw_attempts(repo: &Repo, branch: &str, path: &str) -> Vec<(&'static str, String)> {
    repo.build_sources()
        .into_iter()
        .filter_map(|source| build_raw_url(repo, branch, path, source).map(|url| (source, url)))
        .collect()
}

/// `buildSources()` + `buildDownloadUrl`。
fn download_attempts(repo: &Repo, tag: &str, asset: &str) -> Vec<String> {
    repo.build_sources()
        .into_iter()
        .filter_map(|source| build_download_url(repo, tag, asset, source))
        .collect()
}

// ---------------------------------------------------------------------------
// 七个取数函数
// ---------------------------------------------------------------------------

/// 把 attempt 的 future 装箱成 [`Attempt`]（`as` 不能做 unsizing，只能靠这个）。
fn boxed<'a, T>(
    future: impl Future<Output = (usize, Result<T, FetchError>)> + Send + 'a,
) -> Attempt<'a, T> {
    Box::pin(future)
}

/// `fetchJson(repo, branch, path)`（`sources.py:135`）：竞速两个源的 raw 地址，返回
/// `(JSON, 获胜源)`。全部失败 → [`json_failure`]。
pub async fn fetch_json(
    transport: &dyn Transport,
    repo: &Repo,
    branch: &str,
    path: &str,
) -> Result<(Value, &'static str), NetError> {
    let attempts = raw_attempts(repo, branch, path);

    let winner = race_first_ok(attempts.len(), stagger_delay(), |index| {
        let (source, url) = &attempts[index];
        boxed(async move {
            let result = transport.get_json(url, &[], JSON_TIMEOUT).await;
            (index, result.map(|value| (value, *source)))
        })
    })
    .await;

    match winner {
        Some((_, (data, source))) => Ok((data, source)),
        None => Err(json_failure(&repo.name, branch, path)),
    }
}

/// 一个候选 release（`newest_by_version` 的入参）。
struct Candidate {
    data: Value,
    source: &'static str,
    version: String,
}

/// `fetchLatestRelease(repo)`（`sources.py:101`）：`gather` 两个源，取
/// `parseVersion` 最大者，同分取**先出现**者。
///
/// 返回原始 JSON + 源名 —— `Release` 的构造在 `gd3-ui`（见模块头）。
/// `version_of` 负责从 JSON 里抠出版本串（`Release.fromResponse` 的
/// `tag_name` → `name` → 空串 三步）。
pub async fn fetch_latest_release(
    transport: &dyn Transport,
    repo: &Repo,
    version_of: impl Fn(&Value) -> String,
) -> Result<(Value, &'static str), NetError> {
    let attempts: Vec<(&'static str, String)> = repo
        .build_sources()
        .into_iter()
        .filter_map(|source| {
            let name = repo.name_on(source)?;
            let api = &endpoints(source)?.api;
            Some((source, format!("{api}/{name}/releases/latest")))
        })
        .collect();

    // `asyncio.gather(*[...])`：同时起跑，结果按 `buildSources()` 的顺序回来
    let mut futures = Vec::with_capacity(attempts.len());
    for (source, url) in &attempts {
        let source = *source;
        let version_of = &version_of;
        futures.push(async move {
            // `headers = {"accept": "application/vnd.github+json"} if source == "github" else {}`
            let headers = if source == "github" {
                vec![(
                    "accept".to_string(),
                    "application/vnd.github+json".to_string(),
                )]
            } else {
                Vec::new()
            };
            let result = transport.get_json(url, &headers, JSON_TIMEOUT).await;
            result.map(|data| Candidate {
                version: version_of(&data),
                source,
                data,
            })
        });
    }
    let results = futures_util::future::join_all(futures).await;

    match newest_by_version(results, |candidate| candidate.version.as_str()) {
        Some(candidate) => Ok((candidate.data, candidate.source)),
        None => Err(latest_release_failure(&repo.name)),
    }
}

/// `probeDownloadUrl(repo, tag, asset)`（`sources.py:191`）：竞速 `Range: bytes=0-0`
/// 探测，返回能拿到的那个 URL。全部失败 → [`download_failure`]。
pub async fn probe_download_url(
    transport: &dyn Transport,
    repo: &Repo,
    tag: &str,
    asset: &str,
) -> Result<String, NetError> {
    let attempts = download_attempts(repo, tag, asset);
    let headers = range_headers();

    let winner = race_first_ok(attempts.len(), stagger_delay(), |index| {
        let url = &attempts[index];
        let headers = &headers;
        boxed(async move {
            let result = transport.probe(url, headers, PROBE_TIMEOUT).await;
            (index, result.map(|()| url.clone()))
        })
    })
    .await;

    match winner {
        Some((_, url)) => Ok(url),
        None => Err(download_failure(&repo.name, tag, asset)),
    }
}

/// `fetchReleaseAsset(repo, tag, asset, outputPath, onProgress)`（`sources.py:204`）：
/// 先探测出可用 URL，再流式下载。返回实际用的 URL。
pub async fn fetch_release_asset(
    transport: &dyn Transport,
    repo: &Repo,
    tag: &str,
    asset: &str,
    output: &Path,
    progress: Option<&(dyn Fn(f64) + Send + Sync)>,
) -> Result<String, NetError> {
    let url = probe_download_url(transport, repo, tag, asset).await?;
    transport
        .download(&url, output, progress)
        .await
        .map_err(NetError::io)?;
    Ok(url)
}

/// `fetchPypiReleases(package)`（`sources.py:241`）：`gather` 两个源（顺序 = `SOURCE_ORDER`），
/// 丢掉失败的，然后 `for result in reversed(succeeded): releases |= result`。
///
/// 全部失败 → [`pypi_releases_failure`]。
pub async fn fetch_pypi_releases(
    transport: &dyn Transport,
    package: &str,
) -> Result<serde_json::Map<String, Value>, NetError> {
    let mut futures = Vec::with_capacity(SOURCE_ORDER.len());
    for source in SOURCE_ORDER {
        let url = build_pypi_url(package, source);
        futures.push(async move {
            let value = transport.get_json(&url, &[], JSON_TIMEOUT).await?;
            // `(await resp.json())["releases"]`：缺键 / 非对象都是失败项
            match value.get("releases") {
                Some(Value::Object(releases)) => Ok(releases.clone()),
                _ => Err(FetchError::Transport("响应里没有 releases".to_string())),
            }
        });
    }
    let results = futures_util::future::join_all(futures).await;

    match merge_pypi_releases(results) {
        Some(releases) => Ok(releases),
        None => Err(pypi_releases_failure(package)),
    }
}

/// `fetchPypiFile(url, outputPath, onProgress)`（`sources.py:265`）：`pythonhosted`
/// 的地址会多一个 BFSU 镜像候选，两个候选竞速探测，胜者下载。
pub async fn fetch_pypi_file(
    transport: &dyn Transport,
    url: &str,
    output: &Path,
    progress: Option<&(dyn Fn(f64) + Send + Sync)>,
) -> Result<(), NetError> {
    let candidates = pypi_file_candidates(url);
    let headers = range_headers();

    let winner = race_first_ok(candidates.len(), stagger_delay(), |index| {
        let candidate = &candidates[index];
        let headers = &headers;
        boxed(async move {
            let result = transport.probe(candidate, headers, PROBE_TIMEOUT).await;
            (index, result.map(|()| candidate.clone()))
        })
    })
    .await;

    let Some((_, chosen)) = winner else {
        return Err(pypi_file_failure(url));
    };
    transport
        .download(&chosen, output, progress)
        .await
        .map_err(NetError::io)
}

#[cfg(test)]
mod tests {
    use super::*;

    use gd3_core::sources::app_repo;
    use serde_json::json;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Python 侧实测的地址（第 114 步的金值 + `buildPypiUrl` 的两个分支）
    const GH_RAW: &str =
        "https://raw.githubusercontent.com/XiaoYouChR/Ghost-Downloader-3/main/versions.json";
    const GC_RAW: &str =
        "https://cnb.cool/XiaoYouChR/Ghost-Downloader-3/-/git/raw/main/versions.json";
    const GH_LATEST: &str =
        "https://api.github.com/repos/XiaoYouChR/Ghost-Downloader-3/releases/latest";
    const GC_LATEST: &str =
        "https://api.gitcode.com/api/v5/repos/XiaoYouChR/Ghost-Downloader-3/releases/latest";
    const GH_DL: &str =
        "https://github.com/XiaoYouChR/Ghost-Downloader-3/releases/download/v4.4.0/app.exe";
    const GC_DL: &str =
        "https://gitcode.com/XiaoYouChR/Ghost-Downloader-3/releases/download/v4.4.0/app.exe";
    const PYPI_GH: &str = "https://pypi.org/pypi/foo/json";
    const PYPI_GC: &str = "https://mirrors.bfsu.edu.cn/pypi/web/json/foo";
    const WHEEL: &str = "https://files.pythonhosted.org/packages/ab/cd/foo-1.0-py3-none-any.whl";
    const WHEEL_MIRROR: &str =
        "https://mirrors.bfsu.edu.cn/pypi/web/packages/ab/cd/foo-1.0-py3-none-any.whl";

    /// `(URL, [(名字, 值)])` —— 每次请求带上的头，按调用顺序记录。
    type RecordedHeaders = Vec<(String, Vec<(String, String)>)>;

    /// 可注入的 `Transport`：按 URL 查响应，记录调用顺序与请求头。
    #[derive(Default)]
    struct Stub {
        json_responses: Mutex<HashMap<String, Result<Value, FetchError>>>,
        probe_responses: Mutex<HashMap<String, Result<(), FetchError>>>,
        download_responses: Mutex<HashMap<String, Result<Vec<u8>, FetchError>>>,
        delays: Mutex<HashMap<String, u64>>,
        calls: Mutex<Vec<String>>,
        headers: Mutex<RecordedHeaders>,
    }

    impl Stub {
        fn new() -> Self {
            Self::default()
        }

        fn json(self, url: &str, value: Value) -> Self {
            self.json_responses
                .lock()
                .unwrap()
                .insert(url.to_string(), Ok(value));
            self
        }

        fn json_error(self, url: &str, error: FetchError) -> Self {
            self.json_responses
                .lock()
                .unwrap()
                .insert(url.to_string(), Err(error));
            self
        }

        fn probe(self, url: &str) -> Self {
            self.probe_responses
                .lock()
                .unwrap()
                .insert(url.to_string(), Ok(()));
            self
        }

        fn probe_error(self, url: &str, error: FetchError) -> Self {
            self.probe_responses
                .lock()
                .unwrap()
                .insert(url.to_string(), Err(error));
            self
        }

        fn download_body(self, url: &str, body: &[u8]) -> Self {
            self.download_responses
                .lock()
                .unwrap()
                .insert(url.to_string(), Ok(body.to_vec()));
            self
        }

        fn download_error(self, url: &str, error: FetchError) -> Self {
            self.download_responses
                .lock()
                .unwrap()
                .insert(url.to_string(), Err(error));
            self
        }

        fn delay(self, url: &str, millis: u64) -> Self {
            self.delays.lock().unwrap().insert(url.to_string(), millis);
            self
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        fn headers_of(&self, url: &str) -> Vec<(String, String)> {
            self.headers
                .lock()
                .unwrap()
                .iter()
                .rev()
                .find(|(seen, _)| seen == url)
                .map(|(_, headers)| headers.clone())
                .unwrap_or_default()
        }

        fn record(&self, call: String, headers: &[(String, String)]) {
            let url = headers_url(&call);
            self.calls.lock().unwrap().push(call);
            self.headers.lock().unwrap().push((url, headers.to_vec()));
        }

        async fn pause(&self, url: &str) {
            let delay = self.delays.lock().unwrap().get(url).copied().unwrap_or(0);
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
        }
    }

    /// 从 `"json <url>"` 这种记录里取回 URL（只为给 `headers_of` 找键）。
    fn headers_url(call: &str) -> String {
        call.split_once(' ')
            .map(|(_, url)| url)
            .unwrap_or("")
            .to_string()
    }

    impl Transport for Stub {
        fn get_json<'a>(
            &'a self,
            url: &'a str,
            headers: &'a [(String, String)],
            _timeout: Duration,
        ) -> BoxFetch<'a, Result<Value, FetchError>> {
            Box::pin(async move {
                self.record(format!("json {url}"), headers);
                self.pause(url).await;
                self.json_responses
                    .lock()
                    .unwrap()
                    .get(url)
                    .cloned()
                    .unwrap_or(Err(FetchError::Status(404)))
            })
        }

        fn probe<'a>(
            &'a self,
            url: &'a str,
            headers: &'a [(String, String)],
            _timeout: Duration,
        ) -> BoxFetch<'a, Result<(), FetchError>> {
            Box::pin(async move {
                self.record(format!("probe {url}"), headers);
                self.pause(url).await;
                self.probe_responses
                    .lock()
                    .unwrap()
                    .get(url)
                    .cloned()
                    .unwrap_or(Err(FetchError::Status(404)))
            })
        }

        fn download<'a>(
            &'a self,
            url: &'a str,
            output: &'a Path,
            progress: Option<&'a (dyn Fn(f64) + Send + Sync)>,
        ) -> BoxFetch<'a, Result<(), FetchError>> {
            Box::pin(async move {
                self.record(format!("download {url}"), &[]);
                let payload = self
                    .download_responses
                    .lock()
                    .unwrap()
                    .get(url)
                    .cloned()
                    .unwrap_or_else(|| Ok(b"payload".to_vec()));
                match payload {
                    Ok(bytes) => {
                        tokio::fs::write(output, &bytes).await.map_err(describe)?;
                        if let Some(callback) = progress {
                            callback(100.0);
                        }
                        Ok(())
                    }
                    Err(error) => Err(error),
                }
            })
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "gd3-net-sources-{}-{tag}-{seq}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    /// `Release.fromResponse` 的版本提取（`tag_name` → `name` → 空串）。
    fn version_of(data: &Value) -> String {
        for key in ["tag_name", "name"] {
            let value = data.get(key).and_then(Value::as_str).unwrap_or("").trim();
            if !value.is_empty() {
                return value.to_string();
            }
        }
        String::new()
    }

    fn header_value(headers: &[(String, String)], name: &str) -> Option<String> {
        headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    }

    // ── fetchJson ──

    #[tokio::test]
    async fn fetch_json_takes_the_first_source_that_answers() {
        let stub = Stub::new()
            .json_error(GH_RAW, FetchError::Status(404))
            .json(GC_RAW, json!({"app": {"version": "4.5.0"}}));

        let (data, source) = fetch_json(&stub, &app_repo(), "main", "versions.json")
            .await
            .unwrap();

        assert_eq!(source, "gitcode");
        assert_eq!(data["app"]["version"], json!("4.5.0"));
        assert_eq!(
            stub.calls(),
            vec![format!("json {GH_RAW}"), format!("json {GC_RAW}")]
        );
    }

    #[tokio::test]
    async fn fetch_json_prefers_the_faster_source() {
        // github 慢 80ms，gitcode 立刻回；STAGGER_DELAY 只有 50ms → gitcode 先完成
        let stub = Stub::new()
            .delay(GH_RAW, 80)
            .json(GH_RAW, json!({"from": "github"}))
            .json(GC_RAW, json!({"from": "gitcode"}));

        let (data, source) = fetch_json(&stub, &app_repo(), "main", "versions.json")
            .await
            .unwrap();

        assert_eq!(source, "gitcode");
        assert_eq!(data["from"], json!("gitcode"));
    }

    #[tokio::test]
    async fn fetch_json_reports_the_python_failure() {
        let stub = Stub::new();
        let error = fetch_json(&stub, &app_repo(), "main", "versions.json")
            .await
            .unwrap_err();
        assert_eq!(
            error.format(),
            "无法获取 XiaoYouChR/Ghost-Downloader-3/main/versions.json"
        );
    }

    // ── probeDownloadUrl ──

    #[tokio::test]
    async fn probe_download_url_asks_for_a_single_byte() {
        let stub = Stub::new()
            .probe_error(GC_DL, FetchError::Status(404))
            .probe(GH_DL);

        let url = probe_download_url(&stub, &app_repo(), "v4.4.0", "app.exe")
            .await
            .unwrap();

        assert_eq!(url, GH_DL);
        assert_eq!(
            header_value(&stub.headers_of(GH_DL), "Range").as_deref(),
            Some("bytes=0-0")
        );
    }

    #[tokio::test]
    async fn probe_download_url_reports_the_download_failure() {
        let stub = Stub::new();
        let error = probe_download_url(&stub, &app_repo(), "v4.4.0", "app.exe")
            .await
            .unwrap_err();
        assert_eq!(
            error.format(),
            "无法下载 XiaoYouChR/Ghost-Downloader-3/v4.4.0/app.exe"
        );
    }

    // ── fetchReleaseAsset ──

    #[tokio::test]
    async fn fetch_release_asset_downloads_the_probed_url() {
        let dir = temp_dir("asset");
        let output = dir.join("app.exe");
        let stub = Stub::new()
            .probe_error(GC_DL, FetchError::Status(404))
            .probe(GH_DL)
            .download_body(GH_DL, b"binary");

        let url = fetch_release_asset(&stub, &app_repo(), "v4.4.0", "app.exe", &output, None)
            .await
            .unwrap();

        assert_eq!(url, GH_DL);
        assert_eq!(std::fs::read(&output).unwrap(), b"binary");
        assert!(stub.calls().contains(&format!("download {GH_DL}")));
    }

    #[tokio::test]
    async fn fetch_release_asset_does_not_download_when_the_probe_fails() {
        let dir = temp_dir("asset-fail");
        let output = dir.join("app.exe");
        let stub = Stub::new();

        let error = fetch_release_asset(&stub, &app_repo(), "v4.4.0", "app.exe", &output, None)
            .await
            .unwrap_err();

        assert_eq!(
            error.format(),
            "无法下载 XiaoYouChR/Ghost-Downloader-3/v4.4.0/app.exe"
        );
        assert!(!stub.calls().iter().any(|call| call.starts_with("download")));
        assert!(!output.exists());
    }

    #[tokio::test]
    async fn fetch_release_asset_reports_a_download_failure() {
        let dir = temp_dir("asset-dl-fail");
        let output = dir.join("app.exe");
        let stub = Stub::new()
            .probe_error(GC_DL, FetchError::Status(404))
            .probe(GH_DL)
            .download_error(GH_DL, FetchError::Status(500));

        let error = fetch_release_asset(&stub, &app_repo(), "v4.4.0", "app.exe", &output, None)
            .await
            .unwrap_err();

        // Python 的 `fetchFile` 抛异常 → `toTaskError` 兜底成 `发生了意外错误：…`
        assert_eq!(error.format(), "发生了意外错误：HTTP 500");
        assert!(!output.exists());
    }

    // ── fetchPypiReleases ──

    #[tokio::test]
    async fn fetch_pypi_releases_lets_the_earlier_source_win() {
        let stub = Stub::new()
            .json(PYPI_GH, json!({"releases": {"1.0": [1], "2.0": []}}))
            .json(PYPI_GC, json!({"releases": {"1.0": [2], "3.0": []}}));

        let releases = fetch_pypi_releases(&stub, "foo").await.unwrap();

        // reversed(succeeded) → gitcode 先合并，github 后合并覆盖同键
        assert_eq!(
            releases.keys().cloned().collect::<Vec<_>>(),
            vec!["1.0".to_string(), "3.0".to_string(), "2.0".to_string()]
        );
        assert_eq!(releases["1.0"], json!([1]));
    }

    #[tokio::test]
    async fn fetch_pypi_releases_keeps_going_with_one_source_down() {
        let stub = Stub::new()
            .json_error(PYPI_GC, FetchError::Status(500))
            .json(PYPI_GH, json!({"releases": {"1.0": []}}));

        let releases = fetch_pypi_releases(&stub, "foo").await.unwrap();
        assert_eq!(
            releases.keys().cloned().collect::<Vec<_>>(),
            vec!["1.0".to_string()]
        );
    }

    #[tokio::test]
    async fn fetch_pypi_releases_reports_the_failure() {
        let stub = Stub::new();
        let error = fetch_pypi_releases(&stub, "foo").await.unwrap_err();
        assert_eq!(error.format(), "无法获取 PyPI 包信息: foo");
    }

    #[tokio::test]
    async fn fetch_pypi_releases_rejects_a_payload_without_releases() {
        let stub = Stub::new()
            .json(PYPI_GH, json!({"info": {}}))
            .json(PYPI_GC, json!({"releases": {"1.0": []}}));

        // github 那份没有 releases → 当成失败项，只剩 gitcode
        let releases = fetch_pypi_releases(&stub, "foo").await.unwrap();
        assert_eq!(
            releases.keys().cloned().collect::<Vec<_>>(),
            vec!["1.0".to_string()]
        );
    }

    // ── fetchPypiFile ──

    #[tokio::test]
    async fn fetch_pypi_file_falls_back_to_the_mirror() {
        let dir = temp_dir("pypi");
        let output = dir.join("foo.whl");
        let stub = Stub::new()
            .probe_error(WHEEL, FetchError::Status(404))
            .probe(WHEEL_MIRROR)
            .download_body(WHEEL_MIRROR, b"wheel");

        fetch_pypi_file(&stub, WHEEL, &output, None).await.unwrap();

        assert_eq!(std::fs::read(&output).unwrap(), b"wheel");
        assert_eq!(
            stub.calls(),
            vec![
                format!("probe {WHEEL}"),
                format!("probe {WHEEL_MIRROR}"),
                format!("download {WHEEL_MIRROR}"),
            ]
        );
    }

    #[tokio::test]
    async fn fetch_pypi_file_does_not_mirror_other_urls() {
        let dir = temp_dir("pypi-plain");
        let output = dir.join("foo.whl");
        let stub = Stub::new().probe_error("https://example.com/foo.whl", FetchError::Status(404));

        let error = fetch_pypi_file(&stub, "https://example.com/foo.whl", &output, None)
            .await
            .unwrap_err();

        assert_eq!(error.format(), "无法下载 https://example.com/foo.whl");
        assert_eq!(
            stub.calls(),
            vec!["probe https://example.com/foo.whl".to_string()]
        );
    }

    #[tokio::test]
    async fn fetch_pypi_file_reports_the_failure() {
        let dir = temp_dir("pypi-fail");
        let output = dir.join("foo.whl");
        let stub = Stub::new();

        let error = fetch_pypi_file(&stub, WHEEL, &output, None)
            .await
            .unwrap_err();

        assert_eq!(
            error.format(),
            "无法下载 https://files.pythonhosted.org/packages/ab/cd/foo-1.0-py3-none-any.whl"
        );
        assert!(!stub.calls().iter().any(|call| call.starts_with("download")));
    }

    // ── fetchLatestRelease ──

    #[tokio::test]
    async fn fetch_latest_release_picks_the_newest_version() {
        let stub = Stub::new()
            .json(GH_LATEST, json!({"tag_name": "v4.4.0"}))
            .json(GC_LATEST, json!({"tag_name": "v4.5.0"}));

        let (data, source) = fetch_latest_release(&stub, &app_repo(), version_of)
            .await
            .unwrap();

        assert_eq!(source, "gitcode");
        assert_eq!(data["tag_name"], json!("v4.5.0"));
    }

    #[tokio::test]
    async fn fetch_latest_release_skips_a_failing_source() {
        let stub = Stub::new()
            .json_error(GC_LATEST, FetchError::Status(500))
            .json(GH_LATEST, json!({"name": "v4.3.1"}));

        let (data, source) = fetch_latest_release(&stub, &app_repo(), version_of)
            .await
            .unwrap();

        assert_eq!(source, "github");
        assert_eq!(data["name"], json!("v4.3.1"));
    }

    #[tokio::test]
    async fn fetch_latest_release_reports_the_failure() {
        let stub = Stub::new();
        let error = fetch_latest_release(&stub, &app_repo(), version_of)
            .await
            .unwrap_err();
        assert_eq!(
            error.format(),
            "无法获取 XiaoYouChR/Ghost-Downloader-3 的最新 release"
        );
    }

    #[tokio::test]
    async fn fetch_latest_release_sends_the_github_accept_header() {
        let stub = Stub::new()
            .json(GH_LATEST, json!({"tag_name": "v4.4.0"}))
            .json(GC_LATEST, json!({"tag_name": "v4.4.0"}));

        let (_, source) = fetch_latest_release(&stub, &app_repo(), version_of)
            .await
            .unwrap();

        // 同版本取先出现者 → github
        assert_eq!(source, "github");
        assert_eq!(
            header_value(&stub.headers_of(GH_LATEST), "accept").as_deref(),
            Some("application/vnd.github+json")
        );
        assert_eq!(header_value(&stub.headers_of(GC_LATEST), "accept"), None);
    }

    // ── write_stream（fetchFile 的流循环） ──

    #[tokio::test]
    async fn write_stream_reports_progress_against_content_length() {
        let dir = temp_dir("stream");
        let output = dir.join("out.bin");
        let seen: Mutex<Vec<f64>> = Mutex::new(Vec::new());
        let callback = |value: f64| seen.lock().unwrap().push(value);
        let stream = futures_util::stream::iter(vec![
            Ok::<Vec<u8>, String>(b"abcde".to_vec()),
            Ok(Vec::new()),
            Ok(b"fghij".to_vec()),
        ]);

        write_stream(&output, 10, stream, Some(&callback))
            .await
            .unwrap();

        assert_eq!(*seen.lock().unwrap(), vec![50.0, 100.0]);
        assert_eq!(std::fs::read(&output).unwrap(), b"abcdefghij");
    }

    #[tokio::test]
    async fn write_stream_skips_the_callback_without_a_content_length() {
        let dir = temp_dir("stream-no-length");
        let output = dir.join("out.bin");
        let seen: Mutex<Vec<f64>> = Mutex::new(Vec::new());
        let callback = |value: f64| seen.lock().unwrap().push(value);
        let stream = futures_util::stream::iter(vec![Ok::<Vec<u8>, String>(b"abc".to_vec())]);

        write_stream(&output, 0, stream, Some(&callback))
            .await
            .unwrap();

        assert!(seen.lock().unwrap().is_empty());
        assert_eq!(std::fs::read(&output).unwrap(), b"abc");
    }

    #[tokio::test]
    async fn write_stream_creates_the_parent_directory() {
        let dir = temp_dir("stream-nested");
        let output = dir.join("a/b/out.bin");
        let stream = futures_util::stream::iter(vec![Ok::<Vec<u8>, String>(b"x".to_vec())]);

        write_stream(&output, 1, stream, None).await.unwrap();

        assert_eq!(std::fs::read(&output).unwrap(), b"x");
    }

    #[tokio::test]
    async fn write_stream_reports_a_transport_error() {
        let dir = temp_dir("stream-error");
        let output = dir.join("out.bin");
        let stream = futures_util::stream::iter(vec![
            Ok::<Vec<u8>, String>(b"abc".to_vec()),
            Err("boom".to_string()),
        ]);

        let error = write_stream(&output, 10, stream, None).await.unwrap_err();

        assert_eq!(error, FetchError::Transport("boom".to_string()));
        // 已写入的部分留在文件里（Python 的 `open(..., "wb")` 也不会回滚）
        assert_eq!(std::fs::read(&output).unwrap(), b"abc");
    }

    // ── 辅助口径 ──

    #[test]
    fn content_length_falls_back_to_zero() {
        let mut headers = wreq::header::HeaderMap::new();
        headers.insert(
            "content-length",
            wreq::header::HeaderValue::from_static("12"),
        );
        assert_eq!(content_length(&headers), 12);

        headers.insert(
            "content-length",
            wreq::header::HeaderValue::from_static("oops"),
        );
        assert_eq!(content_length(&headers), 0);

        headers.remove("content-length");
        assert_eq!(content_length(&headers), 0);
    }

    #[test]
    fn fetch_error_renders_both_shapes() {
        assert_eq!(FetchError::Status(404).to_string(), "HTTP 404");
        assert_eq!(
            FetchError::Transport("boom".to_string()).to_string(),
            "boom"
        );
    }

    #[test]
    fn wreq_transport_keeps_its_profile() {
        let transport = WreqTransport::new("chrome");
        assert_eq!(transport.profile(), "chrome");
    }
}
