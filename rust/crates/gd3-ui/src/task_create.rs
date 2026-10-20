//! 建任务链路：`URL → TaskRecord`（镜像 `features/http_pack/pack.py` 的
//! `buildTask` 与 `HttpParser`）。
//!
//! 到第 136 步为止，Rust 只跑**已经在** `tasks.jsonl` 里的任务：不解析 URL、不探测、
//! 不生成草稿。这一步补上「解析 + 探测 + 造记录」那一段，也就是把下面这条
//! Python 流水线搬过来：
//!
//! ```text
//! FeatureService.parse(options)                     ← 第 108 步已有（ParserChain）
//!   └─ HttpParser.parse(options)                     ← 本模块 [HttpParser]
//!        ├─ ResourceTaskOptions(name, size>0) ? → 直接当 RemoteFile
//!        └─ 否则 probe(options)                      → gd3_net::probe::probe
//!        └─ buildTask(options, file)                 ← 本模块 [build_task]
//! ```
//!
//! 三块映射：
//!
//! | Python | Rust |
//! |---|---|
//! | `RemoteFile`（`pack.py:36`） | `gd3_net::probe::RemoteFile`（字段一一对应，`fileSize` 是 `u64`） |
//! | `probe(options)` | [`Prober`] 的实现，缺省 [`NetProber`]（内建 current-thread runtime） |
//! | `buildTask(options, file)` | [`build_task`] |
//! | `HttpParser`（`priority = 100`） | [`HttpParser`] 实现 `TaskParser` |
//! | `TaskOptions` / `ResourceTaskOptions` | [`CreateOptions`] / [`ResourceOptions`] |
//!
//! 落盘的记录与 Python 逐键同名、同序（`serde_json` 开了 `preserve_order`，
//! `TaskStep.extra` 的插入顺序就是 JSON 里的顺序）：
//!
//! ```text
//! {"name","url","packId":"http","taskId":"tsk_…","status":"WAITING",
//!  "steps":[{"stepIndex":1,"fileIndex":null,"stepId":"stp_…","status":"WAITING",
//!            "progress":0,"receivedBytes":0,"speed":0,
//!            "url","fileSize","headers","clientProfile","userAgent",
//!            "subworkerCount","canUseRangeRequests","httpByteOffset","lastModified",
//!            "isAccelerated","type":"features.http_pack.task.HttpTaskStep"}],
//!  "createdAt","completedAt":0,"outputFolder","fileSize","files":null,
//!  "category":null,"shouldSeed":false,"checksums":{},
//!  "type":"features.http_pack.task.HttpTask"}
//! ```
//!
//! # 谁还没接上（**本步交付的是链路，不是入口**）
//!
//! 三条入口在 Python 侧都是 `featureService.parse` → `taskService.add`：
//! 位置参数 / 拖进来的 URL（`main_window.addUrls` → 草稿对话框）、浏览器扩展
//! （`BrowserService`）、aria2 RPC（`Aria2RpcService`）。Rust 侧这三处**仍然只有
//! 空的解析器链**（`Engine::new` 不装解析器，见 `engine.rs` 模块头），所以：
//!
//! - [`create_tasks`] 是给它们准备的**同一段胶水**（装 [`HttpParser`] + 逐条
//!   `parse` + `add`），单测与 e2e 都验过，但还没有生产调用方；
//! - 草稿对话框（`crate::draft` 的 `DraftParser`）要的是「提交 → 后台探测 →
//!   回填」，而 `DraftStore` 归 UI 线程所有，得配一条 worker 线程 + 通道；
//!   那一步（探针跑在后台、结果回灌 `on_parsed`）留给下一步。
//!
//! # 已知差异
//!
//! - **`ResourceTaskOptions` 靠键存在性判**：Python 有类型（`isinstance(options,
//!   ResourceTaskOptions)`），Rust 的 options 是 `Map<String, Value>`。这里按
//!   `name` / `size` / `canUseRangeRequests` 三个键**任一存在**判定，等价于
//!   `TaskOptions.fromOptions` 的 `filterFields` 只认自己那批键的做法；纯
//!   `TaskOptions` 不会带这三个键（`pack.py` 的 `buildTask` 只读它们）。
//! - **`headers` 只收字符串值**：Python `dict(options.headers)` 原样保留任意值，
//!   非字符串会在 httpx 里再炸；Rust 侧直接跳过非字符串项（值不是 str 的头本来就
//!   不可能正确发出去）。
//! - **`clientProfile` 的两处用法分开**：`probe` 用
//!   `options.clientProfile or cfg.clientProfile.value`（空则回落 cfg），而
//!   `buildTask` 写进 step 的是**原始值**（可能为空串）。Rust 侧同样是：
//!   [`HttpParser::parse`] 探测时回落 [`CreateDefaults::client_profile`]，
//!   [`build_task`] 落盘时写 `options.client_profile`。
//! - **`guess_extension` 不在本模块**：`gd3-net::probe` 的 `ExtensionResolver`
//!   由调用方注入（mimetypes 是平台相关的表）。不注入时兜底名就没有扩展名，
//!   与 `gd3_net::probe` 模块头记的是同一条差异。
//! - **探测是同步阻塞的**：`TaskParser::parse` 是同步签名，[`NetProber`] 自带一个
//!   current-thread runtime 并 `block_on`。**不要在 tokio 运行时线程里调用**
//!   （会 panic）；宿主若要接进 UI 线程，应在后台线程里跑（Python 侧是 `await`）。

use gd3_core::filename::{DEFAULT_MAX_FILENAME_LENGTH, to_safe_filename};
use gd3_core::task::{
    Progress, TaskRecord, TaskStatus, TaskStep, new_step_id, new_task_id, now_seconds,
};
use gd3_core::task_error::TaskError;
use gd3_net::probe::{ExtensionResolver, ProbeOptions, RemoteFile, probe};
use serde_json::{Map, Value};

use crate::feature_service::TaskParser;

/// `HttpPack.packId`（`features/http_pack/pack.py:210`）。
pub const HTTP_PACK_ID: &str = "http";
/// `type(HttpTask)` 的全名（`toDict` 的 `f"{cls.__module__}.{cls.__name__}"`）。
pub const HTTP_TASK_TYPE: &str = "features.http_pack.task.HttpTask";
/// `type(HttpTaskStep)` 的全名。
pub const HTTP_STEP_TYPE: &str = "features.http_pack.task.HttpTaskStep";
/// `HttpParser.priority = 100`（`pack.py:170`）。
pub const HTTP_PARSER_PRIORITY: i64 = 100;
/// `HttpTaskStep.subworkerCount` 的缺省值（`features/http_pack/task.py:69`）。
pub const DEFAULT_SUBWORKER_COUNT: i64 = 8;

const URL: &str = "url";
const OUTPUT_FOLDER: &str = "outputFolder";
const HEADERS: &str = "headers";
const CLIENT_PROFILE: &str = "clientProfile";
const USER_AGENT: &str = "userAgent";
const SUBWORKER_COUNT: &str = "subworkerCount";
const NAME: &str = "name";
const SIZE: &str = "size";
const CAN_USE_RANGE_REQUESTS: &str = "canUseRangeRequests";

/// 建任务时读 `cfg` 的三处缺省值（Python 的 `field(default_factory=...)`）。
///
/// 由宿主从 `UserConfig` 填：`outputFolder` ← `cfg.downloadFolder`、
/// `headers` ← `currentHeaders()`、`subworkerCount` ← `cfg.preBlockNum`、
/// `clientProfile` ← `cfg.clientProfile`（只用于探测时回落）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateDefaults {
    pub output_folder: String,
    pub headers: Vec<(String, String)>,
    pub client_profile: String,
    pub subworker_count: i64,
}

impl Default for CreateDefaults {
    fn default() -> Self {
        CreateDefaults {
            output_folder: String::new(),
            headers: Vec::new(),
            client_profile: String::new(),
            subworker_count: DEFAULT_SUBWORKER_COUNT,
        }
    }
}

impl CreateDefaults {
    /// 从 `UserConfig` 取四项缺省值（Python 的四个 `field(default_factory=...)`）。
    ///
    /// | 字段 | Python | 读法 |
    /// |---|---|---|
    /// | `output_folder` | `Path(cfg.downloadFolder.value)` | `settings::read_text("downloadFolder")` |
    /// | `headers` | `currentHeaders()` | `Network/HeadersPresets` + `CurrentHeadersPreset`（`crate::headers` 已有镜像） |
    /// | `client_profile` | `cfg.clientProfile.value` | `settings::read_text("clientProfile")` |
    /// | `subworker_count` | `cfg.preBlockNum.value` | `settings::read_i64("preBlockNum")` |
    pub fn from_config(config: &gd3_core::config::UserConfig) -> CreateDefaults {
        let presets = crate::headers::parse_presets(
            config.get_str("Network", "HeadersPresets").unwrap_or(""),
        );
        // `cfg.currentHeaders` 的读法：`currentHeadersPresetIndex()` 夹在
        // `[0, len-1]`，取那一份的 headers（`cfg.py:347-352`）。
        let index = crate::headers::current_index(
            config
                .get_py_str("Network", "CurrentHeadersPreset")
                .as_deref(),
            presets.len(),
        );
        CreateDefaults {
            output_folder: gd3_core::settings::read_text(config, "downloadFolder")
                .unwrap_or_default(),
            headers: presets
                .get(index)
                .map(|preset| preset.headers.clone())
                .unwrap_or_default(),
            client_profile: gd3_core::settings::read_text(config, "clientProfile")
                .unwrap_or_default(),
            subworker_count: gd3_core::settings::read_i64(config, "preBlockNum")
                .unwrap_or(DEFAULT_SUBWORKER_COUNT),
        }
    }
}

/// `ResourceTaskOptions` 独有的三个字段（`app/models/task.py:76-79`）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResourceOptions {
    pub name: String,
    pub size: i64,
    pub can_use_range_requests: bool,
}

/// `TaskOptions` 里 `buildTask` / `probe` 用到的字段（`app/models/task.py:60-67`）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CreateOptions {
    pub url: String,
    pub output_folder: String,
    pub headers: Vec<(String, String)>,
    pub client_profile: String,
    pub user_agent: String,
    pub source_user_agent: String,
    pub subworker_count: i64,
    /// `isinstance(options, ResourceTaskOptions)` 的 Rust 等价物（见模块头）。
    pub resource: Option<ResourceOptions>,
}

impl CreateOptions {
    /// `TaskOptions.fromOptions(options)`：缺的键取 `defaults`，多出来的键丢掉。
    pub fn from_options(options: &Map<String, Value>, defaults: &CreateDefaults) -> Self {
        let resource = if options.contains_key(NAME)
            || options.contains_key(SIZE)
            || options.contains_key(CAN_USE_RANGE_REQUESTS)
        {
            Some(ResourceOptions {
                name: text_of(options.get(NAME)),
                size: int_of(options.get(SIZE)).unwrap_or(0),
                can_use_range_requests: bool_of(options.get(CAN_USE_RANGE_REQUESTS))
                    .unwrap_or(false),
            })
        } else {
            None
        };

        CreateOptions {
            url: text_of(options.get(URL)),
            output_folder: options
                .get(OUTPUT_FOLDER)
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| defaults.output_folder.clone()),
            headers: headers_of(options.get(HEADERS)).unwrap_or_else(|| defaults.headers.clone()),
            client_profile: text_of(options.get(CLIENT_PROFILE)),
            user_agent: text_of(options.get(USER_AGENT)),
            source_user_agent: text_of(options.get("sourceUserAgent")),
            subworker_count: int_of(options.get(SUBWORKER_COUNT))
                .unwrap_or(defaults.subworker_count),
            resource,
        }
    }

    /// `ResourceTaskOptions` 且 `name` 非空、`size > 0` 时**不探测**，直接当远程文件。
    ///
    /// `pack.py:190-194`：`RemoteFile(name=options.name, fileSize=options.size,
    /// canUseRangeRequests=options.canUseRangeRequests)`，`lastModified` / `contentType`
    /// 走缺省空串。
    pub fn remote_file_from_options(&self) -> Option<RemoteFile> {
        let resource = self.resource.as_ref()?;
        if resource.name.is_empty() || resource.size <= 0 {
            return None;
        }
        Some(RemoteFile {
            name: resource.name.clone(),
            file_size: resource.size as u64,
            can_use_range_requests: resource.can_use_range_requests,
            last_modified: String::new(),
            content_type: String::new(),
        })
    }

    /// `options.clientProfile or cfg.clientProfile.value`。
    pub fn probe_client_profile(&self, defaults: &CreateDefaults) -> String {
        if self.client_profile.is_empty() {
            defaults.client_profile.clone()
        } else {
            self.client_profile.clone()
        }
    }

    /// 探测入参（`ProbeOptions`）。
    pub fn probe_options(&self, defaults: &CreateDefaults) -> ProbeOptions {
        ProbeOptions {
            url: self.url.clone(),
            headers: self.headers.clone(),
            user_agent: self.user_agent.clone(),
            client_profile: self.probe_client_profile(defaults),
            fallback_name: fallback_name(),
        }
    }
}

/// 镜像 `buildTask`：`options` + 探测结果 → 一条待下载的 `HttpTask`。
///
/// `name` 的取法照抄 `pack.py:158-160`：`ResourceTaskOptions` 且 `name` 非空时用
/// `toSafeFilename(options.name)`，否则用探测出来的 `file.name`（探针已经净化过）。
pub fn build_task(options: &CreateOptions, file: &RemoteFile) -> TaskRecord {
    let name = match options
        .resource
        .as_ref()
        .map(|resource| resource.name.as_str())
        .filter(|name| !name.is_empty())
    {
        Some(raw) => to_safe_filename(raw, &fallback_name(), DEFAULT_MAX_FILENAME_LENGTH),
        None => file.name.clone(),
    };

    // `RemoteFile.fileSize` 是 u64（`SpecialFileSize.UNKNOWN = 0` 也是 0），
    // 记录里是 i64；两侧都非负，钳一下防溢出。
    let file_size = file.file_size.min(i64::MAX as u64) as i64;

    let mut extra = Map::new();
    extra.insert(URL.to_string(), Value::String(options.url.clone()));
    extra.insert("fileSize".to_string(), Value::from(file_size));
    extra.insert(
        HEADERS.to_string(),
        Value::Object(headers_object(&options.headers)),
    );
    extra.insert(
        CLIENT_PROFILE.to_string(),
        Value::String(options.client_profile.clone()),
    );
    extra.insert(
        USER_AGENT.to_string(),
        Value::String(options.user_agent.clone()),
    );
    extra.insert(
        SUBWORKER_COUNT.to_string(),
        Value::from(options.subworker_count),
    );
    extra.insert(
        CAN_USE_RANGE_REQUESTS.to_string(),
        Value::Bool(file.can_use_range_requests),
    );
    extra.insert("httpByteOffset".to_string(), Value::from(0));
    extra.insert(
        "lastModified".to_string(),
        Value::String(file.last_modified.clone()),
    );
    extra.insert("isAccelerated".to_string(), Value::Bool(false));

    let step = TaskStep {
        step_index: 1,
        file_index: None,
        step_id: new_step_id(),
        status: TaskStatus::Waiting,
        progress: Progress::Int(0),
        received_bytes: 0,
        speed: 0,
        extra,
        type_name: Some(HTTP_STEP_TYPE.to_string()),
    };

    TaskRecord {
        name,
        url: options.url.clone(),
        pack_id: HTTP_PACK_ID.to_string(),
        task_id: new_task_id(),
        status: TaskStatus::Waiting,
        steps: vec![step],
        created_at: now_seconds(),
        completed_at: 0,
        output_folder: options.output_folder.clone(),
        file_size,
        files: None,
        category: None,
        // `Task.shouldSeed` 的 dataclass 缺省是 False；播种标记由 `TaskService.add` 置。
        should_seed: false,
        checksums: Map::new(),
        extra: Map::new(),
        type_name: Some(HTTP_TASK_TYPE.to_string()),
    }
}

/// 探测一个 URL（`pack.py::probe` 的可注入形态）。
///
/// 抽成 trait 是为了让 `parse` 在单测里不碰网络；生产实现见 [`NetProber`]。
///
/// `Send` 是给后台解析用的（`crate::draft_worker` 把 [`HttpParser`] 整个搬进
/// 工作线程）：`NetProber` 里装的是 `tokio::runtime::Runtime`，本来就是 `Send`。
pub trait Prober: Send {
    fn probe(&self, options: &ProbeOptions) -> Result<RemoteFile, TaskError>;
}

/// 真网络探测：内建一个 current-thread runtime，`block_on(gd3_net::probe::probe)`。
pub struct NetProber {
    /// `block_on` 的宿主。**不能**在别的 tokio 运行时线程上用（见模块头）。
    runtime: tokio::runtime::Runtime,
    /// `mimetypes.guess_extension` 的替身，由宿主注入（平台相关的表）。
    resolve_extension: Option<ExtensionResolver>,
}

impl NetProber {
    pub fn new(resolve_extension: Option<ExtensionResolver>) -> std::io::Result<NetProber> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        Ok(NetProber {
            runtime,
            resolve_extension,
        })
    }
}

impl Prober for NetProber {
    fn probe(&self, options: &ProbeOptions) -> Result<RemoteFile, TaskError> {
        self.runtime
            .block_on(probe(options, self.resolve_extension.clone()))
    }
}

/// `HttpParser`（`features/http_pack/pack.py:169`）：`priority = 100`，
/// `match` 只认 `http` / `https`，`matchPassive` 再要求可下载扩展名。
///
/// `match` / `matchPassive` 直接转调 `gd3_core::http_pack`（第 108 步已逐条对齐），
/// 本结构只把「探测 + 造记录」接上。
pub struct HttpParser {
    prober: Box<dyn Prober>,
    defaults: CreateDefaults,
}

impl HttpParser {
    pub fn new(prober: Box<dyn Prober>, defaults: CreateDefaults) -> HttpParser {
        HttpParser { prober, defaults }
    }

    /// 用真网络探测（[`NetProber`]，不注入扩展名解析表）。
    pub fn with_defaults(defaults: CreateDefaults) -> std::io::Result<HttpParser> {
        Ok(HttpParser::new(Box::new(NetProber::new(None)?), defaults))
    }

    pub fn defaults(&self) -> &CreateDefaults {
        &self.defaults
    }
}

impl TaskParser for HttpParser {
    fn priority(&self) -> i64 {
        HTTP_PARSER_PRIORITY
    }

    fn matches(&self, options: &Map<String, Value>) -> bool {
        gd3_core::http_pack::parser_match(&text_of(options.get(URL)))
    }

    fn matches_passive(&self, options: &Map<String, Value>) -> bool {
        gd3_core::http_pack::parser_match_passive(&text_of(options.get(URL)))
    }

    /// `HttpParser.parse`：`ResourceTaskOptions` 有名字和大小就直接用，否则探测。
    fn parse(&self, options: &Map<String, Value>) -> Result<TaskRecord, TaskError> {
        let options = CreateOptions::from_options(options, &self.defaults);
        let file = match options.remote_file_from_options() {
            Some(file) => file,
            None => self.prober.probe(&options.probe_options(&self.defaults))?,
        };
        Ok(build_task(&options, &file))
    }
}

/// 一批 URL 的建任务结果（[`create_tasks`] 的返回值）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreateReport {
    /// 真的进了任务列表的个数。
    pub created: usize,
    /// 解析/探测失败的 `(url, 原因)`；按输入顺序。
    pub failures: Vec<(String, String)>,
}

/// 把一批 URL 解析成任务并交给服务层（`auto_start = true`）。
///
/// 这是三条入口（草稿对话框 / 浏览器扩展 / aria2 RPC）共同的那一段
/// —— Python 侧 `featureService.parse` → `taskService.add` 的 Rust 版：解析器链
/// 为空时**顺手装上** [`HttpParser`]（用一个 [`NetProber`] 打头，除非调用方传
/// 自己的 [`Prober`] 进来），缺省值取自 `engine.config()`。
///
/// `prober` 传 `None` 就是真网络探测（[`NetProber`]，不注入扩展名表）；单测传
/// `Some` 免得碰网。
///
/// 失败的 URL 只记进 [`CreateReport::failures`]、**不中断**后续 URL（Python 的
/// 草稿界面也是逐条报错）。探测是同步的，所以这一整段是阻塞调用（见模块头）
/// ——宿主该在后台线程里调它。
pub fn create_tasks(
    engine: &mut crate::engine::Engine,
    urls: &[String],
    prober: Option<Box<dyn Prober>>,
) -> CreateReport {
    let mut report = CreateReport::default();
    if urls.is_empty() {
        return report;
    }

    let defaults = CreateDefaults::from_config(engine.config());
    let parser = match prober {
        Some(prober) => HttpParser::new(prober, defaults),
        None => match HttpParser::with_defaults(defaults) {
            Ok(parser) => parser,
            Err(error) => {
                report.failures = urls
                    .iter()
                    .map(|url| (url.clone(), format!("探测运行时建不起来：{error}")))
                    .collect();
                return report;
            }
        },
    };
    {
        let chain = engine.features_mut().parsers_mut();
        if chain.is_empty() {
            chain.register(Box::new(parser));
        }
    }

    let presets = identity_presets(engine.config());
    for url in urls {
        let mut options = Map::new();
        options.insert(URL.to_string(), Value::String(url.clone()));
        // `FeatureService.parse`：预设前置 → 第一个 match 的解析器出结果。
        let record = match engine.features().parsers().parse(&mut options, &presets) {
            Ok(record) => record,
            Err(error) => {
                report.failures.push((url.clone(), format!("{error}")));
                continue;
            }
        };
        match engine.tasks_mut().add(record, true) {
            crate::task_service::AddOutcome::Added { .. } => report.created += 1,
            crate::task_service::AddOutcome::DiskSpaceInsufficient { .. } => report.created += 1,
            crate::task_service::AddOutcome::Duplicate => {
                report
                    .failures
                    .push((url.clone(), "任务已在列表里".to_string()));
            }
        }
    }
    report
}

/// `identity.parsePresets(cfg.identityPresets.value)`（`engine.rs` 同名函数的转出口）。
fn identity_presets(config: &gd3_core::config::UserConfig) -> Vec<crate::identity::IdentityPreset> {
    let raw = gd3_core::settings::read(config, "identityPresets").map(|value| value.to_string());
    crate::identity::parse_presets(raw.as_deref())
}

/// `f"file_{time_ns()}"`（`pack.py` 的兜底名）。
fn fallback_name() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    format!("file_{nanos}")
}

fn text_of(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_default()
}

fn int_of(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(number) => number.as_i64().or_else(|| {
            number
                .as_f64()
                .filter(|value| value.is_finite())
                .map(|value| value as i64)
        }),
        // Python 的 `filterFields` 不做类型转换，但草稿界面的选项全是字符串。
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

fn bool_of(value: Option<&Value>) -> Option<bool> {
    match value? {
        Value::Bool(flag) => Some(*flag),
        Value::Number(number) => Some(number.as_f64().is_some_and(|value| value != 0.0)),
        Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" => Some(true),
            "0" | "false" | "no" | "" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// `dict(options.headers)`：只留字符串值（见模块头的差异说明）。
fn headers_of(value: Option<&Value>) -> Option<Vec<(String, String)>> {
    let map = value?.as_object()?;
    Some(
        map.iter()
            .filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_string())))
            .collect(),
    )
}

fn headers_object(headers: &[(String, String)]) -> Map<String, Value> {
    let mut object = Map::new();
    for (name, value) in headers {
        object.insert(name.clone(), Value::String(value.clone()));
    }
    object
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature_service::ParserChain;
    use std::sync::{Arc, Mutex};

    fn defaults() -> CreateDefaults {
        CreateDefaults {
            output_folder: "/tmp/out".to_string(),
            headers: vec![("user-agent".to_string(), "UA".to_string())],
            client_profile: "chrome".to_string(),
            subworker_count: 8,
        }
    }

    fn map_of(pairs: &[(&str, Value)]) -> Map<String, Value> {
        let mut map = Map::new();
        for (key, value) in pairs {
            map.insert(key.to_string(), value.clone());
        }
        map
    }

    fn file(name: &str) -> RemoteFile {
        RemoteFile {
            name: name.to_string(),
            file_size: 1234,
            can_use_range_requests: true,
            last_modified: "Wed, 21 Oct 2015 07:28:00 GMT".to_string(),
            content_type: "application/octet-stream".to_string(),
        }
    }

    /// 记下被问过的探测入参；`file` 为 `None` 时抛错。
    struct StubProber {
        file: Option<RemoteFile>,
        seen: Arc<Mutex<Vec<ProbeOptions>>>,
    }

    impl StubProber {
        fn new(file: Option<RemoteFile>) -> (StubProber, Arc<Mutex<Vec<ProbeOptions>>>) {
            let seen = Arc::new(Mutex::new(Vec::new()));
            (
                StubProber {
                    file,
                    seen: Arc::clone(&seen),
                },
                seen,
            )
        }
    }

    impl Prober for StubProber {
        fn probe(&self, options: &ProbeOptions) -> Result<RemoteFile, TaskError> {
            self.seen.lock().expect("seen").push(options.clone());
            match &self.file {
                Some(file) => Ok(file.clone()),
                None => Err(TaskError::io("探测失败")),
            }
        }
    }

    fn parser(file: Option<RemoteFile>) -> (HttpParser, Arc<Mutex<Vec<ProbeOptions>>>) {
        let (prober, seen) = StubProber::new(file);
        (HttpParser::new(Box::new(prober), defaults()), seen)
    }

    /// 记录去掉随机/时间部分之后的 JSON 文本（键序也要一致）。
    fn canonical(record: &TaskRecord) -> String {
        let mut value = serde_json::to_value(record).expect("序列化");
        let object = value.as_object_mut().expect("对象");
        object.insert("taskId".into(), Value::from("tsk_x"));
        object.insert("createdAt".into(), Value::from(0));
        if let Some(steps) = object.get_mut("steps").and_then(Value::as_array_mut) {
            for step in steps {
                step.as_object_mut()
                    .expect("步骤是对象")
                    .insert("stepId".into(), Value::from("stp_x"));
            }
        }
        serde_json::to_string(&value).expect("再序列化")
    }

    /// 逐字节对齐 Python `buildTask` 的产出（样本由真 Python 跑出来，见步骤笔记）。
    #[test]
    fn build_task_matches_the_python_shape_key_for_key() {
        let options = CreateOptions {
            url: "https://example.com/a.bin".to_string(),
            output_folder: "/tmp/out".to_string(),
            headers: vec![("user-agent".to_string(), "UA".to_string())],
            client_profile: "chrome".to_string(),
            user_agent: "UA2".to_string(),
            source_user_agent: String::new(),
            subworker_count: 8,
            resource: None,
        };
        let record = build_task(&options, &file("a.bin"));
        let expected = concat!(
            r#"{"name":"a.bin","url":"https://example.com/a.bin","packId":"http","taskId":"tsk_x","#,
            r#""status":"WAITING","steps":[{"stepIndex":1,"fileIndex":null,"stepId":"stp_x","#,
            r#""status":"WAITING","progress":0,"receivedBytes":0,"speed":0,"#,
            r#""url":"https://example.com/a.bin","fileSize":1234,"#,
            r#""headers":{"user-agent":"UA"},"clientProfile":"chrome","userAgent":"UA2","#,
            r#""subworkerCount":8,"canUseRangeRequests":true,"httpByteOffset":0,"#,
            r#""lastModified":"Wed, 21 Oct 2015 07:28:00 GMT","isAccelerated":false,"#,
            r#""type":"features.http_pack.task.HttpTaskStep"}],"createdAt":0,"completedAt":0,"#,
            r#""outputFolder":"/tmp/out","fileSize":1234,"files":null,"category":null,"#,
            r#""shouldSeed":false,"checksums":{},"type":"features.http_pack.task.HttpTask"}"#
        );
        assert_eq!(canonical(&record), expected);
    }

    /// `ResourceTaskOptions.name` 走 `toSafeFilename`（Python 同一份样本：
    /// `weird/name?.mp4` → `weird_name_.mp4`），且此时**不探测**。
    #[test]
    fn resource_name_override_is_sanitised_and_skips_the_probe() {
        let (parser, seen) = parser(None);
        let options = map_of(&[
            ("url", Value::from("https://example.com/b?x=1")),
            ("name", Value::from("weird/name?.mp4")),
            ("size", Value::from(7)),
        ]);
        let record = parser.parse(&options).expect("不该探测");
        assert!(
            seen.lock().expect("seen").is_empty(),
            "有 name+size 就不该发探测请求"
        );
        assert_eq!(record.name, "weird_name_.mp4");
        assert_eq!(record.file_size, 7);
        assert_eq!(record.url, "https://example.com/b?x=1");
        assert!(
            !record.steps[0].extra["canUseRangeRequests"]
                .as_bool()
                .unwrap()
        );
    }

    #[test]
    fn size_zero_still_probes() {
        let (parser, seen) = parser(Some(file("probed.bin")));
        let options = map_of(&[
            ("url", Value::from("https://example.com/b")),
            ("name", Value::from("given.bin")),
            ("size", Value::from(0)),
        ]);
        let record = parser.parse(&options).expect("应当探测");
        assert_eq!(seen.lock().expect("seen").len(), 1);
        // 探测成功后 name 仍取 options.name（`buildTask` 的覆盖分支）
        assert_eq!(record.name, "given.bin");
        assert_eq!(record.file_size, 1234);
    }

    #[test]
    fn parse_probes_with_the_config_defaults() {
        let (parser, seen) = parser(Some(file("a.bin")));
        // clientProfile 为空 → 探测时回落 cfg.clientProfile，但落盘保持空串
        let options = map_of(&[("url", Value::from("https://example.com/a.bin"))]);
        let record = parser.parse(&options).expect("应当探测");

        let seen = seen.lock().expect("seen");
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].client_profile, "chrome", "空则回落 cfg");
        assert_eq!(seen[0].url, "https://example.com/a.bin");
        assert_eq!(
            seen[0].headers,
            vec![("user-agent".to_string(), "UA".to_string())],
            "缺 headers 键时用 cfg 的 currentHeaders()"
        );
        assert!(
            seen[0].fallback_name.starts_with("file_"),
            "{}",
            seen[0].fallback_name
        );

        assert_eq!(record.steps[0].extra["clientProfile"], Value::from(""));
        assert_eq!(record.steps[0].extra["subworkerCount"], Value::from(8));
        assert_eq!(record.output_folder, "/tmp/out");
        assert_eq!(
            record.steps[0].extra["lastModified"],
            Value::from("Wed, 21 Oct 2015 07:28:00 GMT")
        );
        assert!(
            record.steps[0].extra["canUseRangeRequests"]
                .as_bool()
                .unwrap()
        );
        assert_eq!(record.short_type_name(), Some("HttpTask"));
        assert_eq!(record.steps[0].type_name.as_deref(), Some(HTTP_STEP_TYPE));
        assert!(!record.should_seed, "播种标记归 TaskService.add");
    }

    #[test]
    fn probe_errors_bubble_up_as_task_errors() {
        let (parser, _) = parser(None);
        let options = map_of(&[("url", Value::from("https://example.com/a.bin"))]);
        let error = parser.parse(&options).expect_err("探测失败要冒泡");
        assert!(error.format().contains("探测失败"), "{}", error.format());
    }

    #[test]
    fn match_and_match_passive_delegate_to_http_pack() {
        let (parser, _) = parser(None);
        assert_eq!(parser.priority(), 100);
        assert!(parser.matches(&map_of(&[("url", Value::from("https://a/b"))])));
        assert!(!parser.matches(&map_of(&[("url", Value::from("ftp://a/b.mp4"))])));
        assert!(parser.matches_passive(&map_of(&[("url", Value::from("https://a/b.MP4"))])));
        assert!(!parser.matches_passive(&map_of(&[("url", Value::from("https://a/b"))])));
    }

    /// 进 `ParserChain` 之后走的是同一条路（`FeatureService.parse` 的行为）。
    #[test]
    fn the_chain_parses_a_url_into_a_record() {
        let (parser, seen) = parser(Some(file("a.bin")));
        let mut chain = ParserChain::new();
        chain.register(Box::new(parser));
        assert_eq!(chain.priorities(), vec![100]);

        let mut options = map_of(&[("url", Value::from("https://example.com/a.bin"))]);
        let record = chain.parse(&mut options, &[]).expect("链条应当命中");
        assert_eq!(record.pack_id, "http");
        assert_eq!(record.name, "a.bin");
        assert_eq!(seen.lock().expect("seen").len(), 1);
        assert!(chain.matches("https://x/y.zip"));
    }

    /// `create_tasks` 走完「解析 → 入队 → 落盘」，解析失败的 URL 只计数不中断。
    #[test]
    fn create_tasks_adds_a_record_and_reports_failures() {
        let dir = std::env::temp_dir().join(format!(
            "gd3-ui-task-create-unit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let tasks_path = dir.join("tasks.jsonl");

        let config =
            gd3_core::settings::default_config_with_download_folder(&dir.to_string_lossy());
        let mut engine = crate::engine::Engine::new(&config, &tasks_path);
        engine.start().expect("装载任务列表");

        let (prober, seen) = StubProber::new(Some(file("a.bin")));
        let report = create_tasks(
            &mut engine,
            &[
                "https://example.com/a.bin".to_string(),
                "ftp://example.com/x".to_string(),
            ],
            Some(Box::new(prober)),
        );

        assert_eq!(report.created, 1, "只有 http URL 建得出来");
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures[0].0, "ftp://example.com/x");
        assert!(
            report.failures[0].1.contains("No parser matched"),
            "{}",
            report.failures[0].1
        );
        assert_eq!(seen.lock().expect("seen").len(), 1, "只有一条 URL 需要探测");
        assert_eq!(engine.tasks().len(), 1);
        assert_eq!(engine.features().parsers().priorities(), vec![100]);

        // 落盘之后 Python 侧按同一份口径读得回来。
        assert!(engine.tasks_mut().flush(&tasks_path).expect("落盘"));
        let text = std::fs::read_to_string(&tasks_path).expect("读回 tasks.jsonl");
        let record: TaskRecord = serde_json::from_str(text.trim()).expect("每行都是合法任务");
        assert_eq!(record.pack_id, "http");
        assert_eq!(record.name, "a.bin");
        assert_eq!(record.steps[0].step_index, 1);
        assert_eq!(record.short_type_name(), Some("HttpTask"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `CreateDefaults::from_config` 读的是宿主配置（缺省配置下 `headers` 是
    /// `默认` 预设的 8 条 BASE_HEADERS）。
    #[test]
    fn defaults_come_from_the_user_config() {
        let config = gd3_core::settings::default_config_with_download_folder("/tmp/dl");
        let defaults = CreateDefaults::from_config(&config);
        assert_eq!(defaults.output_folder, "/tmp/dl");
        assert_eq!(defaults.client_profile, "auto");
        assert_eq!(defaults.headers.len(), crate::headers::BASE_HEADERS.len());
        assert_eq!(defaults.headers[0].0, "accept-encoding");
        assert!(
            defaults.subworker_count >= 1,
            "{}",
            defaults.subworker_count
        );
    }

    #[test]
    fn headers_keep_order_and_drop_non_strings() {
        let headers = headers_of(Some(&serde_json::json!({
            "accept": "*/*",
            "x-bad": 3
        })))
        .expect("对象");
        assert_eq!(headers, vec![("accept".to_string(), "*/*".to_string())]);
        // 不是对象 → None（回落到 defaults）
        assert!(headers_of(Some(&Value::from("x"))).is_none());
        assert!(headers_of(None).is_none());
    }
}
