//! 浏览器扩展服务的**载荷层**：`app/services/browser_service.py:47-168`。
//!
//! `browser_service.py`（597 行）混了三层，本模块只负责中间那一层：
//!
//! | Python | Rust |
//! |---|---|
//! | WebSocket 握手 + 帧（`_send` / `_sendError` 的封包部分） | `gd3-servers::websocket`（Phase 5/6） |
//! | **五个 `StrEnum` + `toStr` / `toInt` + 三个 `to*` 载荷转换** | **本模块** |
//! | `BrowserClientSession` / 配对 / hello / create_task 状态机 | 第 111 步（`browser_session`） |
//! | 托盘图标、配对对话框、`extensionUpdated` 的 UI 提示 | Phase 7 主窗口那条线 |
//!
//! ## 逐条对照
//!
//! | Python | Rust |
//! |---|---|
//! | `MessageType` / `ErrorCode` / `TaskAction` / `TaskSource` / `CreateTaskStatus`（:47-90） | [`MessageType`] / [`ErrorCode`] / [`BrowserAction`] / [`TaskSource`] / [`CreateTaskStatus`] |
//! | `PROTOCOL_VERSION = 2`（:91） | [`PROTOCOL_VERSION`] |
//! | `toStr(data, key, default="")`（:93-96） | `to_str` |
//! | `toInt(data, key, default)`（:98-101） | `to_int` |
//! | `toResourceTaskOptions(resource)`（:103-114） | [`to_resource_task_options`] |
//! | `toTaskOptions(source, payload)`（:116-149） | [`to_task_options`] |
//! | `toTaskSummary(task)`（:151-168） | [`to_task_summary`] |
//!
//! ## 与 Python 的差异（都不是「还没做」）
//!
//! - **`TaskOptions` 的 `headers` 缺省值**：Python 的 dataclass 缺省是
//!   `currentHeaders()`（当前请求头预设），所以 `MergeTaskOptions` 不传 `headers` 时拿到的是
//!   预设里的那套。Rust 侧没有响应式配置层，预设由调用方放进
//!   [`PayloadDefaults::current_headers`]，语义相同。
//! - **`toInt` 认 bool**：Python 的 `isinstance(True, int)` 为真且 `True > 0`，所以 `true`
//!   会当成 `1`；这里照抄（`false` 不满足 `> 0`，落回缺省值）。
//! - **`headers` 不是对象时按空处理**：Python 的 `hdrs.get(...)` 会抛 `AttributeError`
//!   （扩展发坏包时整个会话崩掉）。Rust 退化成空 map，差异记在这里。
//! - **`resources` 里的非对象元素按「没有」处理**：Python 的 `toResourceTaskOptions`
//!   会对它调 `resource.get(...)` 抛 `AttributeError`，同样是整条会话崩掉。
//! - **`path` 不是字符串时按缺省目录处理**：Python 的 `Path(1)` 抛 `TypeError`。
//! - **`round(progress, 2)` 走 `format!("{value:.2}")` 再 parse 回 `f64`**：Python 是对
//!   二进制值的精确十进制展开做五成双，`(x * 100).round_ties_even() / 100` 在
//!   `round(42.675, 2)` 上会给出 `42.68`（乘积恰好舍入到 `4267.5`），必须用定精度格式化。

use gd3_core::filesystem::is_existing;
use gd3_core::path::{posix_parent, posix_suffix};
use gd3_core::task::{TaskRecord, TaskStatus};
use serde_json::{Map, Value};

use crate::facts;
use crate::feature_service::is_truthy;

/// `PROTOCOL_VERSION = 2`（`browser_service.py:91`）。
pub const PROTOCOL_VERSION: i64 = 2;

/// 五个 `StrEnum` 共用的样板：`as_str()` + `parse()`。
macro_rules! str_enum {
    ($name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            /// 线上表示（`StrEnum` 的 `str()`）。
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }

            /// 认不出时 `None`（Python 侧 `StrEnum(...)` 解析失败会抛 `ValueError`）。
            pub fn parse(text: &str) -> Option<Self> {
                match text {
                    $($text => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

str_enum!(MessageType {
    Error => "error",
    Hello => "hello",
    HelloAck => "hello_ack",
    PairRequest => "pair_request",
    PairResult => "pair_result",
    SubscribeTasks => "subscribe_tasks",
    TaskSnapshot => "task_snapshot",
    CreateTask => "create_task",
    CreateTaskResult => "create_task_result",
    TaskAction => "task_action",
    TaskActionResult => "task_action_result",
    Reload => "reload",
});

str_enum!(ErrorCode {
    BadRequest => "bad_request",
    ProtocolMismatch => "protocol_mismatch",
    Unauthorized => "unauthorized",
});

// `browser_service.py` 的 `TaskAction`。名字带 `Browser` 前缀是因为 `gd3-ui` 里已经有一个
// `actions::TaskAction`（第 100 步的任务动作效果），两者语义无关，同名会让 `use` 撞车。
// （宏调用上不能挂文档注释——rustdoc 不为宏展开生成文档。）
str_enum!(BrowserAction {
    TogglePause => "toggle_pause",
    Cancel => "cancel",
    Remove => "remove",
    Redownload => "redownload",
    OpenFile => "open_file",
    OpenFolder => "open_folder",
});

str_enum!(TaskSource {
    Download => "download",
    Resource => "resource",
    ResourceMerge => "resource_merge",
    PageMedia => "page_media",
});

str_enum!(CreateTaskStatus {
    Created => "created",
    Drafted => "drafted",
    Rejected => "rejected",
});

/// `toStr(data, key, default="")`：只认字符串，其余一律缺省。
pub(crate) fn to_str(data: &Map<String, Value>, key: &str) -> String {
    data.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `toInt(data, key, default)`：只认「正」整数，其余落回缺省。
///
/// Python 的 `isinstance(value, int) and value > 0` 里 `True` 也算 `int`，所以 `true` → `1`；
/// 浮点（`3.0`）不算 `int`，落回缺省。
pub(crate) fn to_int(data: &Map<String, Value>, key: &str, default: i64) -> i64 {
    match data.get(key) {
        Some(Value::Bool(true)) => 1,
        Some(value) => match value.as_i64() {
            Some(number) if number > 0 => number,
            _ => default,
        },
        None => default,
    }
}

/// `resource.get("headers") or {}`：非空对象才认，其余（含非对象）都当空。
fn header_map(data: &Map<String, Value>) -> Map<String, Value> {
    match data.get("headers") {
        Some(Value::Object(headers)) if !headers.is_empty() => headers.clone(),
        _ => Map::new(),
    }
}

/// `headers.get("user-agent", "")`：**键名是小写的** `user-agent`，不是 `User-Agent`。
fn header_text(headers: &Map<String, Value>, key: &str) -> String {
    headers
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `toTaskOptions` 要读的三项配置：`cfg.downloadFolder` / `cfg.preBlockNum` /
/// `currentHeaders()`（`cfg.py:351`）。
///
/// Rust 侧没有响应式配置层，由调用方每轮现读再传入（同 `identity::parse_presets` 的口径）。
#[derive(Debug, Clone, PartialEq)]
pub struct PayloadDefaults {
    pub download_folder: String,
    pub pre_block_num: i64,
    pub current_headers: Map<String, Value>,
}

/// `app/models/task.py:60-67` 的 `TaskOptions` 基类字段。
#[derive(Debug, Clone, PartialEq)]
pub struct BaseOptions {
    pub url: String,
    pub output_folder: String,
    pub headers: Map<String, Value>,
    pub client_profile: String,
    pub user_agent: String,
    pub source_user_agent: String,
    pub subworker_count: i64,
}

/// `ResourceTaskOptions`（`task.py:76-79`）。
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceOptions {
    pub base: BaseOptions,
    pub name: String,
    pub size: i64,
    pub can_use_range_requests: bool,
}

/// `PageTaskOptions`（`task.py:82-85`）。
#[derive(Debug, Clone, PartialEq)]
pub struct PageOptions {
    pub base: BaseOptions,
    pub page_url: String,
    pub page_title: String,
}

/// `MergeTaskOptions`（`task.py:88-91`）。
#[derive(Debug, Clone, PartialEq)]
pub struct MergeOptions {
    pub base: BaseOptions,
    pub video: Option<ResourceOptions>,
    pub audio: Option<ResourceOptions>,
}

/// `toTaskOptions` 的三条出路（Python 靠子类区分，Rust 用枚举）。
///
/// `Merge` 变体装箱：`MergeOptions` 比另外两个变体大 400 字节以上
/// （`clippy::large_enum_variant`），载荷本身寿命很短，装箱不影响语义。
#[derive(Debug, Clone, PartialEq)]
pub enum TaskOptionsPayload {
    Resource(ResourceOptions),
    Page(PageOptions),
    Merge(Box<MergeOptions>),
}

impl TaskOptionsPayload {
    /// 三条出路共有的字段。
    pub fn base(&self) -> &BaseOptions {
        match self {
            TaskOptionsPayload::Resource(options) => &options.base,
            TaskOptionsPayload::Page(options) => &options.base,
            TaskOptionsPayload::Merge(options) => &options.base,
        }
    }

    /// `toTaskOptions` 的结果摊成 [`ParserChain::parse`](crate::feature_service::ParserChain)
    /// 吃的那张 map。
    ///
    /// 键名 = Python dataclass 的**字段名**（`url` / `outputFolder` / `subworkerCount` / …），
    /// 与 `feature_service::apply_identity_preset` 读的 `clientProfile` / `userAgent` 同一套口径
    /// ——Python 的解析器就是按属性名读的（`options.outputFolder`），属性名即字段名。
    ///
    /// `outputFolder` 在这里是字符串：Python 那份是 `Path`，但它只在解析器内部被
    /// `/` 拼接或 `str()`，Rust 侧的解析器拿字符串自己拼，语义相同。
    pub fn to_map(&self) -> Map<String, Value> {
        match self {
            TaskOptionsPayload::Resource(options) => {
                let mut map = base_map(&options.base);
                insert_resource_fields(&mut map, options);
                map
            }
            TaskOptionsPayload::Page(options) => {
                let mut map = base_map(&options.base);
                map.insert("pageUrl".to_string(), Value::from(options.page_url.clone()));
                map.insert(
                    "pageTitle".to_string(),
                    Value::from(options.page_title.clone()),
                );
                map
            }
            TaskOptionsPayload::Merge(options) => {
                let mut map = base_map(&options.base);
                map.insert("video".to_string(), resource_value(options.video.as_ref()));
                map.insert("audio".to_string(), resource_value(options.audio.as_ref()));
                map
            }
        }
    }
}

/// `TaskOptions` 基类的七个字段（`task.py:60-67` 的声明序）。
fn base_map(base: &BaseOptions) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("url".to_string(), Value::from(base.url.clone()));
    map.insert(
        "outputFolder".to_string(),
        Value::from(base.output_folder.clone()),
    );
    map.insert("headers".to_string(), Value::Object(base.headers.clone()));
    map.insert(
        "clientProfile".to_string(),
        Value::from(base.client_profile.clone()),
    );
    map.insert(
        "userAgent".to_string(),
        Value::from(base.user_agent.clone()),
    );
    map.insert(
        "sourceUserAgent".to_string(),
        Value::from(base.source_user_agent.clone()),
    );
    map.insert(
        "subworkerCount".to_string(),
        Value::from(base.subworker_count),
    );
    map
}

/// `ResourceTaskOptions` 多出来的三个字段（`task.py:76-79`）。
fn insert_resource_fields(map: &mut Map<String, Value>, options: &ResourceOptions) {
    map.insert("name".to_string(), Value::from(options.name.clone()));
    map.insert("size".to_string(), Value::from(options.size));
    map.insert(
        "canUseRangeRequests".to_string(),
        Value::Bool(options.can_use_range_requests),
    );
}

/// `MergeTaskOptions.video` / `.audio`：`None` 落 `null`（Python 侧就是 `None`）。
fn resource_value(options: Option<&ResourceOptions>) -> Value {
    match options {
        None => Value::Null,
        Some(options) => {
            let mut map = base_map(&options.base);
            insert_resource_fields(&mut map, options);
            Value::Object(map)
        }
    }
}

// ---------------------------------------------------------------------------
// 载荷转换
// ---------------------------------------------------------------------------

/// `round(progress, 2)`。
///
/// Python 的 `round(float, 2)` 是对**二进制值的精确十进制展开**做「四舍六入五成双」：
/// `round(42.675, 2) == 42.67`（`42.675` 的双精度值略小于十进制 42.675）。
/// 先乘 100 再 `round_ties_even` 会算错——`42.675 * 100.0` 恰好舍入成 `4267.5`，
/// 五成双给出 `4268`。所以这里走 `format!("{value:.2}")`：Rust 的定精度格式化同样是对
/// 精确十进制展开做五成双，再 parse 回 `f64` 就与 CPython 逐位一致。
fn round2(value: f64) -> f64 {
    format!("{value:.2}").parse().unwrap_or(value)
}

/// `task.status.name.lower()`。
fn status_name(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Waiting => "waiting",
        TaskStatus::Running => "running",
        TaskStatus::Paused => "paused",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
    }
}

/// `outputPath.suffix.lstrip(".").lower()`。
fn file_ext(output_path: &str) -> String {
    posix_suffix(output_path)
        .trim_start_matches('.')
        .to_lowercase()
}

/// 三个子类共有的 `TaskOptions` 字段。
///
/// `subworker_count` 一律先填 [`PayloadDefaults::pre_block_num`]：这正是 dataclass 的
/// `field(default_factory=lambda: cfg.preBlockNum.value)`，只有 `RESOURCE` / `DOWNLOAD`
/// 分支会用 `toInt(payload, "preBlockNum", …)` 覆写。
fn base_options(
    url: String,
    output_folder: String,
    headers: Map<String, Value>,
    defaults: &PayloadDefaults,
) -> BaseOptions {
    let source_user_agent = header_text(&headers, "user-agent");
    BaseOptions {
        url,
        output_folder,
        headers,
        client_profile: String::new(),
        user_agent: String::new(),
        source_user_agent,
        subworker_count: defaults.pre_block_num,
    }
}

/// `toResourceTaskOptions(resource)`（`browser_service.py:103-114`）。
///
/// 注意这个函数**不传** `outputFolder` / `headers` / `subworkerCount`，所以它们取
/// dataclass 缺省值（`cfg.downloadFolder` / `currentHeaders()` / `cfg.preBlockNum`）。
/// `RESOURCE` 分支的 `replace(...)` 只覆写前两个里的后两个之一，`outputFolder` 由调用方再改。
pub fn to_resource_task_options(
    resource: &Map<String, Value>,
    defaults: &PayloadDefaults,
) -> ResourceOptions {
    let headers = header_map(resource);
    ResourceOptions {
        base: base_options(
            to_str(resource, "url"),
            defaults.download_folder.clone(),
            headers,
            defaults,
        ),
        name: to_str(resource, "filename"),
        size: to_int(resource, "size", 0),
        can_use_range_requests: is_truthy(resource.get("supportsRange")),
    }
}

/// `toTaskOptions(source, payload)`（`browser_service.py:116-149`）。
///
/// Python 靠三个 dataclass 子类区分三条出路，Rust 用 [`TaskOptionsPayload`] 枚举。
/// `raise ValueError(f"unsupported task source: {source}")` 那条分支在 Rust 里不存在：
/// [`TaskSource`] 是枚举，构造不出第四种。
pub fn to_task_options(
    source: TaskSource,
    payload: &Map<String, Value>,
    defaults: &PayloadDefaults,
) -> TaskOptionsPayload {
    // `Path(rawPath) if rawPath else Path(cfg.downloadFolder.value)`：空字符串与缺键同义。
    let raw_path = to_str(payload, "path");
    let output_folder = if raw_path.is_empty() {
        defaults.download_folder.clone()
    } else {
        raw_path
    };

    match source {
        TaskSource::ResourceMerge => {
            let resources = payload
                .get("resources")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let video = resources
                .first()
                .and_then(Value::as_object)
                .map(|resource| to_resource_task_options(resource, defaults));
            let audio = resources
                .get(1)
                .and_then(Value::as_object)
                .map(|resource| to_resource_task_options(resource, defaults));
            let url = video
                .as_ref()
                .map(|options| options.base.url.clone())
                .unwrap_or_default();
            TaskOptionsPayload::Merge(Box::new(MergeOptions {
                base: base_options(
                    url,
                    output_folder,
                    defaults.current_headers.clone(),
                    defaults,
                ),
                video,
                audio,
            }))
        }
        TaskSource::PageMedia => {
            let headers = header_map(payload);
            TaskOptionsPayload::Page(PageOptions {
                base: base_options(to_str(payload, "url"), output_folder, headers, defaults),
                page_url: to_str(payload, "pageUrl"),
                page_title: to_str(payload, "pageTitle"),
            })
        }
        TaskSource::Resource | TaskSource::Download => {
            let mut options = to_resource_task_options(payload, defaults);
            options.base.output_folder = output_folder;
            options.base.subworker_count = to_int(payload, "preBlockNum", defaults.pre_block_num);
            TaskOptionsPayload::Resource(options)
        }
    }
}

/// `toTaskSummary(task)`（`browser_service.py:151-168`）。
///
/// 键顺序与 Python 字面量一致（`preserve_order` 下 `Map` 是 `IndexMap`）。
pub fn to_task_summary(record: &TaskRecord) -> Map<String, Value> {
    let (progress, speed, received_bytes) = record.current_snapshot();
    let output_path = record.output_path();

    let mut summary = Map::new();
    summary.insert("taskId".to_string(), Value::from(record.task_id.clone()));
    summary.insert("name".to_string(), Value::from(record.name.clone()));
    summary.insert(
        "status".to_string(),
        Value::from(status_name(record.status)),
    );
    summary.insert("progress".to_string(), Value::from(round2(progress)));
    summary.insert("receivedBytes".to_string(), Value::from(received_bytes));
    summary.insert("fileSize".to_string(), Value::from(record.file_size));
    summary.insert("speed".to_string(), Value::from(speed));
    summary.insert("createdAt".to_string(), Value::from(record.created_at));
    summary.insert(
        "canPause".to_string(),
        Value::from(facts::can_pause(record)),
    );
    summary.insert(
        "canOpenFile".to_string(),
        Value::from(is_existing(&output_path)),
    );
    summary.insert(
        "canOpenFolder".to_string(),
        Value::from(is_existing(posix_parent(&output_path))),
    );
    summary.insert("fileExt".to_string(), Value::from(file_ext(&output_path)));
    summary.insert("packName".to_string(), Value::from(record.pack_id.clone()));
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use gd3_core::task::{Progress, TaskStep};
    use serde_json::json;

    fn defaults() -> PayloadDefaults {
        let mut headers = Map::new();
        headers.insert("User-Agent".to_string(), Value::from("preset-agent"));
        PayloadDefaults {
            download_folder: "/downloads".to_string(),
            pre_block_num: 4,
            current_headers: headers,
        }
    }

    fn payload(value: Value) -> Map<String, Value> {
        match value {
            Value::Object(map) => map,
            _ => panic!("payload must be an object"),
        }
    }

    fn record(name: &str, status: TaskStatus) -> TaskRecord {
        TaskRecord {
            name: name.to_string(),
            url: "https://example.com/a".to_string(),
            pack_id: "generic".to_string(),
            task_id: "tsk_1".to_string(),
            status,
            steps: Vec::new(),
            created_at: 1_700_000_000,
            completed_at: 0,
            output_folder: "/downloads".to_string(),
            file_size: 0,
            files: None,
            category: None,
            should_seed: false,
            checksums: Map::new(),
            extra: Map::new(),
            type_name: None,
        }
    }

    fn step(index: i64, progress: f64, status: TaskStatus) -> TaskStep {
        TaskStep {
            step_index: index,
            file_index: None,
            step_id: format!("step_{index}"),
            status,
            progress: Progress::Float(progress),
            received_bytes: 0,
            speed: 0,
            extra: Map::new(),
            type_name: None,
        }
    }

    fn resource(options: TaskOptionsPayload) -> ResourceOptions {
        match options {
            TaskOptionsPayload::Resource(options) => options,
            other => panic!("expected a resource payload, got {other:?}"),
        }
    }

    #[test]
    fn every_str_enum_round_trips() {
        for (text, value) in [
            ("error", MessageType::Error),
            ("hello", MessageType::Hello),
            ("hello_ack", MessageType::HelloAck),
            ("pair_request", MessageType::PairRequest),
            ("pair_result", MessageType::PairResult),
            ("subscribe_tasks", MessageType::SubscribeTasks),
            ("task_snapshot", MessageType::TaskSnapshot),
            ("create_task", MessageType::CreateTask),
            ("create_task_result", MessageType::CreateTaskResult),
            ("task_action", MessageType::TaskAction),
            ("task_action_result", MessageType::TaskActionResult),
            ("reload", MessageType::Reload),
        ] {
            assert_eq!(MessageType::parse(text), Some(value), "MessageType::{text}");
            assert_eq!(value.as_str(), text);
        }
        assert_eq!(
            ErrorCode::parse("protocol_mismatch"),
            Some(ErrorCode::ProtocolMismatch)
        );
        assert_eq!(
            BrowserAction::parse("open_folder"),
            Some(BrowserAction::OpenFolder)
        );
        assert_eq!(
            TaskSource::parse("resource_merge"),
            Some(TaskSource::ResourceMerge)
        );
        assert_eq!(
            CreateTaskStatus::parse("drafted"),
            Some(CreateTaskStatus::Drafted)
        );
        assert_eq!(PROTOCOL_VERSION, 2);
    }

    #[test]
    fn unknown_enum_text_is_rejected() {
        assert_eq!(MessageType::parse(""), None);
        assert_eq!(MessageType::parse("HELLO"), None);
        assert_eq!(MessageType::parse("hello "), None);
        assert_eq!(TaskSource::parse("downloads"), None);
        assert_eq!(BrowserAction::parse("toggle-pause"), None);
    }

    #[test]
    fn to_str_only_accepts_strings() {
        let data = payload(json!({"text": "x", "number": 1, "null": null, "list": ["x"]}));
        assert_eq!(to_str(&data, "text"), "x");
        assert_eq!(to_str(&data, "number"), "");
        assert_eq!(to_str(&data, "null"), "");
        assert_eq!(to_str(&data, "list"), "");
        assert_eq!(to_str(&data, "missing"), "");
    }

    #[test]
    fn to_int_treats_true_as_one_and_rejects_floats() {
        let data = payload(json!({
            "positive": 8,
            "zero": 0,
            "negative": -3,
            "float": 3.0,
            "yes": true,
            "no": false,
            "text": "8",
        }));
        assert_eq!(to_int(&data, "positive", 7), 8);
        assert_eq!(to_int(&data, "zero", 7), 7);
        assert_eq!(to_int(&data, "negative", 7), 7);
        assert_eq!(to_int(&data, "float", 7), 7);
        assert_eq!(to_int(&data, "yes", 7), 1);
        assert_eq!(to_int(&data, "no", 7), 7);
        assert_eq!(to_int(&data, "text", 7), 7);
        assert_eq!(to_int(&data, "missing", 7), 7);
    }

    #[test]
    fn round2_matches_python_round() {
        // 期望值来自 CPython 3.13：`round(42.675, 2) == 42.67` 等。
        assert_eq!(round2(42.675), 42.67);
        assert_eq!(round2(0.125), 0.12);
        assert_eq!(round2(1.005), 1.0);
        assert_eq!(round2(33.333), 33.33);
        assert_eq!(round2(100.0), 100.0);
        assert_eq!(round2(0.0), 0.0);
    }

    #[test]
    fn a_resource_payload_carries_name_size_and_range_support() {
        let data = payload(json!({
            "url": "https://cdn.example.com/a.bin",
            "filename": "a.bin",
            "size": 1234,
            "supportsRange": true,
            "headers": {"user-agent": "ua-1", "Referer": "https://example.com"},
        }));
        let options = to_resource_task_options(&data, &defaults());
        assert_eq!(options.base.url, "https://cdn.example.com/a.bin");
        assert_eq!(options.base.output_folder, "/downloads");
        assert_eq!(options.base.headers.len(), 2);
        assert_eq!(options.base.source_user_agent, "ua-1");
        assert_eq!(options.base.subworker_count, 4);
        assert_eq!(options.base.client_profile, "");
        assert_eq!(options.base.user_agent, "");
        assert_eq!(options.name, "a.bin");
        assert_eq!(options.size, 1234);
        assert!(options.can_use_range_requests);
    }

    #[test]
    fn the_user_agent_header_lookup_is_case_sensitive() {
        // Python 的 `hdrs.get("user-agent", "")` 是精确匹配：`User-Agent` 取不到。
        let data = payload(json!({"headers": {"User-Agent": "ua-1"}}));
        let options = to_resource_task_options(&data, &defaults());
        assert_eq!(options.base.headers.len(), 1);
        assert_eq!(options.base.source_user_agent, "");
    }

    #[test]
    fn a_broken_headers_field_degrades_to_an_empty_map() {
        // Python 会 `AttributeError`；Rust 退化成空 map（差异写进模块头）。
        let data = payload(json!({"headers": "nope"}));
        let options = to_resource_task_options(&data, &defaults());
        assert!(options.base.headers.is_empty());
        assert_eq!(options.base.source_user_agent, "");

        let data = payload(json!({"headers": {}}));
        assert!(
            to_resource_task_options(&data, &defaults())
                .base
                .headers
                .is_empty()
        );
    }

    #[test]
    fn a_download_payload_overrides_the_output_folder_and_subworkers() {
        let data = payload(json!({
            "url": "https://x.example.com/a.bin",
            "path": "/media",
            "preBlockNum": 12,
            "size": 5,
        }));
        let options = resource(to_task_options(TaskSource::Download, &data, &defaults()));
        assert_eq!(options.base.output_folder, "/media");
        assert_eq!(options.base.subworker_count, 12);
        assert_eq!(options.size, 5);
        assert!(options.base.headers.is_empty());
        assert!(!options.can_use_range_requests);
    }

    #[test]
    fn an_empty_path_falls_back_to_the_download_folder() {
        let data = payload(json!({"url": "https://x.example.com/a.bin", "path": ""}));
        let options = resource(to_task_options(TaskSource::Resource, &data, &defaults()));
        assert_eq!(options.base.output_folder, "/downloads");
        // `preBlockNum` 缺键 → `cfg.preBlockNum`
        assert_eq!(options.base.subworker_count, 4);
    }

    #[test]
    fn a_page_payload_carries_page_url_and_title() {
        let data = payload(json!({
            "url": "https://x.example.com/v.m3u8",
            "pageUrl": "https://x.example.com/watch",
            "pageTitle": "第 1 集",
            "headers": {"user-agent": "ua-2"},
        }));
        let options = match to_task_options(TaskSource::PageMedia, &data, &defaults()) {
            TaskOptionsPayload::Page(options) => options,
            other => panic!("expected a page payload, got {other:?}"),
        };
        assert_eq!(options.base.url, "https://x.example.com/v.m3u8");
        assert_eq!(options.base.output_folder, "/downloads");
        assert_eq!(options.base.source_user_agent, "ua-2");
        assert_eq!(options.base.subworker_count, 4);
        assert_eq!(options.page_url, "https://x.example.com/watch");
        assert_eq!(options.page_title, "第 1 集");
    }

    #[test]
    fn a_merge_payload_maps_two_resources_to_video_and_audio() {
        let data = payload(json!({
            "path": "/media",
            "resources": [
                {"url": "https://x.example.com/v.mp4", "filename": "v.mp4", "size": 10,
                 "supportsRange": true},
                {"url": "https://x.example.com/a.m4a", "filename": "a.m4a", "size": 2},
            ],
        }));
        let options = match to_task_options(TaskSource::ResourceMerge, &data, &defaults()) {
            TaskOptionsPayload::Merge(options) => options,
            other => panic!("expected a merge payload, got {other:?}"),
        };
        assert_eq!(options.base.url, "https://x.example.com/v.mp4");
        assert_eq!(options.base.output_folder, "/media");
        // `MergeTaskOptions` 不传 `headers` → dataclass 缺省 `currentHeaders()`
        assert_eq!(
            options.base.headers.get("User-Agent"),
            Some(&Value::from("preset-agent"))
        );
        let video = options.video.as_ref().expect("video");
        let audio = options.audio.as_ref().expect("audio");
        assert_eq!(video.base.url, "https://x.example.com/v.mp4");
        assert_eq!(video.name, "v.mp4");
        assert_eq!(video.size, 10);
        assert!(video.can_use_range_requests);
        assert_eq!(audio.name, "a.m4a");
        assert!(!audio.can_use_range_requests);
        // 嵌套 `ResourceTaskOptions` 也走 dataclass 缺省：`downloadFolder` / `preBlockNum`
        assert_eq!(video.base.output_folder, "/downloads");
        assert_eq!(audio.base.subworker_count, 4);
    }

    #[test]
    fn a_merge_payload_without_resources_has_an_empty_url() {
        let data = payload(json!({"resources": []}));
        let options = match to_task_options(TaskSource::ResourceMerge, &data, &defaults()) {
            TaskOptionsPayload::Merge(options) => options,
            other => panic!("expected a merge payload, got {other:?}"),
        };
        assert!(options.video.is_none());
        assert!(options.audio.is_none());
        assert_eq!(options.base.url, "");
        assert_eq!(options.base.output_folder, "/downloads");

        // 只有一个 resource：video 有、audio 没有。
        let data = payload(json!({"resources": [{"url": "https://x.example.com/v.mp4"}]}));
        let options = match to_task_options(TaskSource::ResourceMerge, &data, &defaults()) {
            TaskOptionsPayload::Merge(options) => options,
            other => panic!("expected a merge payload, got {other:?}"),
        };
        assert_eq!(options.base.url, "https://x.example.com/v.mp4");
        assert!(options.audio.is_none());
    }

    #[test]
    fn a_summary_rounds_the_progress_and_names_the_status() {
        let mut task = record("movie.MKV", TaskStatus::Running);
        task.file_size = 4096;
        task.steps = vec![step(1, 42.675, TaskStatus::Running)];

        let summary = to_task_summary(&task);
        let keys: Vec<&str> = summary.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "taskId",
                "name",
                "status",
                "progress",
                "receivedBytes",
                "fileSize",
                "speed",
                "createdAt",
                "canPause",
                "canOpenFile",
                "canOpenFolder",
                "fileExt",
                "packName",
            ]
        );
        assert_eq!(summary["taskId"], Value::from("tsk_1"));
        assert_eq!(summary["name"], Value::from("movie.MKV"));
        assert_eq!(summary["status"], Value::from("running"));
        assert_eq!(summary["progress"], Value::from(42.67));
        assert_eq!(summary["receivedBytes"], Value::from(0));
        assert_eq!(summary["fileSize"], Value::from(4096));
        assert_eq!(summary["speed"], Value::from(0));
        assert_eq!(summary["createdAt"], Value::from(1_700_000_000));
        assert_eq!(summary["canPause"], Value::from(true));
        assert_eq!(summary["fileExt"], Value::from("mkv"));
        assert_eq!(summary["packName"], Value::from("generic"));
    }

    #[test]
    fn every_task_status_lowers_to_its_enum_name() {
        for (status, text) in [
            (TaskStatus::Waiting, "waiting"),
            (TaskStatus::Running, "running"),
            (TaskStatus::Paused, "paused"),
            (TaskStatus::Completed, "completed"),
            (TaskStatus::Failed, "failed"),
        ] {
            assert_eq!(status_name(status), text);
            let summary = to_task_summary(&record("a.bin", status));
            assert_eq!(summary["status"], Value::from(text));
        }
    }

    #[test]
    fn a_summary_reports_existing_paths_as_openable() {
        let folder =
            std::env::temp_dir().join(format!("gd3-ui-browser-payload-{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("create folder");
        let file = folder.join("Movie.MKV");
        std::fs::write(&file, b"x").expect("write file");

        let mut task = record("Movie.MKV", TaskStatus::Completed);
        task.output_folder = folder.to_string_lossy().into_owned();
        let summary = to_task_summary(&task);
        assert_eq!(summary["canOpenFile"], Value::from(true));
        assert_eq!(summary["canOpenFolder"], Value::from(true));
        assert_eq!(summary["fileExt"], Value::from("mkv"));

        std::fs::remove_dir_all(&folder).expect("cleanup");
    }

    #[test]
    fn a_summary_reports_missing_paths_as_not_openable() {
        let folder = std::env::temp_dir().join(format!(
            "gd3-ui-browser-payload-missing-{}",
            std::process::id()
        ));
        let mut task = record("Movie.MKV", TaskStatus::Completed);
        task.output_folder = folder.to_string_lossy().into_owned();
        let summary = to_task_summary(&task);
        assert_eq!(summary["canOpenFile"], Value::from(false));
        assert_eq!(summary["canOpenFolder"], Value::from(false));
    }

    #[test]
    fn a_summary_without_an_extension_has_an_empty_file_ext() {
        let task = record(".bashrc", TaskStatus::Waiting);
        assert_eq!(to_task_summary(&task)["fileExt"], Value::from(""));
        let task = record("b.", TaskStatus::Waiting);
        assert_eq!(to_task_summary(&task)["fileExt"], Value::from(""));
        let task = record("a.tar.GZ", TaskStatus::Waiting);
        assert_eq!(to_task_summary(&task)["fileExt"], Value::from("gz"));
    }
}
