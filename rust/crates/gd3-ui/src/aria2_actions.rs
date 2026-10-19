//! aria2 RPC 的**应用侧半边**：`app/services/aria2_rpc.py:215-233` 的三个方法。
//!
//! ## 为什么这里只有三个方法
//!
//! `aria2_rpc.py` 是「协议 + 应用」两层混在一个类里：
//!
//! | Python | Rust |
//! |---|---|
//! | `parseHead` / `buildHttpResponse` / `buildPreflightHeaders` / `toHttpStatus` | `gd3-servers::http`（Phase 5） |
//! | `_run` 的 JSON-RPC 方法表、`_addUri` 的参数校验、token、WebSocket 帧 | `gd3-servers::aria2`（Phase 5/6，426 行 / `Aria2Action`） |
//! | `applyTaskOptions` / `_onTaskParsed` / `_onTaskParseFailed` | **本模块** |
//!
//! 也就是说：`aria2.addUri` 的协议判定、`gid` 生成、`dir` / `out` / `header` 的读法都在
//! `gd3-servers::aria2`（`Aria2TaskOptions { url, output_folder, headers, client_profile,
//! name }` 逐字段对应），它把「该解析并加入任务了」以 `Aria2Action::AddTask` 交出来。
//! 本模块是那个动作的消费者——它需要 `FeatureService.parse` 与「草稿还是直加」的策略，
//! 两者都在 `gd3-ui`。
//!
//! ## 逐条对照
//!
//! | Python | Rust |
//! |---|---|
//! | `coroutineRunner.submit(self._parse(options), done=…, failed=…)`（:217-221） | 同步调用 [`FeatureService::parse`]，结果摊成 [`Aria2Effect`] |
//! | `if name: task.setName(name)`（:224-225） | `task.name = to_safe_filename(name, fallback, 200)` |
//! | `cfg.shouldDraftTakenDownload` → `taskDraftRequested.emit([task])`（:226-228） | [`Aria2Effect::DraftRequested`] |
//! | `self._addTask(task)`（:229） | [`Aria2Effect::AddTask`] |
//! | `logger.warning(...)` + `parseFailed.emit(url, toTaskError(error))`（:232-233） | [`Aria2Effect::ParseFailed`]（日志归宿主） |
//!
//! ## 照抄的 Python 缺陷
//!
//! `Aria2RpcService.taskDraftRequested`（`aria2_rpc.py:108`）**在整个 Python 应用里没有
//! 任何 `.connect()`**——`grep -rn taskDraftRequested app/` 只命中 `browser_service.py`
//! 里那个同名的另一个 Signal。于是 `shouldDraftTakenDownload` 打开时，aria2 加进来的任务
//! 会 emit 一个没人听的信号，然后 `return`，**任务直接消失**。
//!
//! Rust 侧保留 [`Aria2Effect::DraftRequested`]（而不是丢任务）：效果队列的意义就是
//! 「谁来消费由宿主决定」。宿主若照抄 Python 不接这条，行为与 Python 一致；接了就是修复。
//! 这条差异写在这里，免得将来有人以为是漏了。

use gd3_core::filename::to_safe_filename;
use gd3_core::task::TaskRecord;
use gd3_core::task_error::TaskError;
use serde_json::{Map, Value};

use crate::feature_service::FeatureService;
use crate::identity::IdentityPreset;

/// `Task.setName` 的长度上限（`filesystem.py:39` 的 `maxLength: int = 200`）。
pub const NAME_MAX_LENGTH: usize = 200;

const URL: &str = "url";
const OUTPUT_FOLDER: &str = "outputFolder";
const CLIENT_PROFILE: &str = "clientProfile";
const HEADERS: &str = "headers";

/// `gd3-servers::aria2::Aria2TaskOptions` 在 `gd3-ui` 侧的同形物。
///
/// 不直接依赖 `gd3-servers`（`gd3-ui` 的依赖里没有它，也不该有：协议层不该把
/// tokio 拖进 UI crate），由宿主在两边的同名字段之间搬一次。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Aria2Request {
    /// `uris[0]`（`aria2_rpc.py:198` 已经滤掉非字符串项）。
    pub url: String,
    /// `options.dir`，缺省时 `gd3-servers` 已填成 `cfg.downloadFolder`（:205-209）。
    pub output_folder: String,
    /// `toHeaders(options)` 的结果：**保留原始大小写与顺序**（:90-104）。
    pub headers: Vec<(String, String)>,
    /// `"" if aria2RpcEmulateFingerprint else "raw"`（:210）。
    pub client_profile: String,
    /// `toText(options, "out")`（:212）；空串表示 aria2 没给名字。
    pub name: String,
}

/// 应用侧要做的后续动作。
///
/// 没有协程运行时，`coroutineRunner.submit(..., done=, failed=)` 的三条出路在这里
/// 摊成三个变体（同第 100 步 `TaskAction` / 第 102 步 `RuntimeEffect` 的口径）。
///
/// 只 `derive(Debug, Clone)`：`TaskRecord` 没有 `PartialEq`（含 `serde_json::Value`
/// 的 `extra` / `checksums`），断言用 `matches!`。
#[derive(Debug, Clone)]
pub enum Aria2Effect {
    /// `taskDraftRequested.emit([task])`：交给任务草稿流程，不直接入队。
    DraftRequested(TaskRecord),
    /// `self._addTask(task)`：`TaskService.add`。
    AddTask(TaskRecord),
    /// `parseFailed.emit(url, toTaskError(error))`。
    ParseFailed { url: String, error: TaskError },
}

/// `Aria2RpcService.applyTaskOptions(options, name)` + 两个回调。
///
/// 返回的 [`Aria2Effect`] 顺序与 Python 的调用顺序一致（至多一条）。
pub fn apply_task_options(
    features: &FeatureService,
    presets: &[IdentityPreset],
    request: Aria2Request,
    should_draft_taken_download: bool,
) -> Vec<Aria2Effect> {
    let mut options = request_options(&request);
    match features.parsers().parse(&mut options, presets) {
        Ok(mut task) => {
            if !request.name.is_empty() {
                task.name = safe_name(&request.name, &task.name);
            }
            if should_draft_taken_download {
                vec![Aria2Effect::DraftRequested(task)]
            } else {
                vec![Aria2Effect::AddTask(task)]
            }
        }
        Err(error) => vec![Aria2Effect::ParseFailed {
            url: request.url,
            error: error.to_task_error(),
        }],
    }
}

/// `TaskOptions(url=…, headers=…, outputFolder=…, clientProfile=…)`（`aria2_rpc.py:206-211`）。
///
/// `headers` 落成 JSON 对象：Python 传的是 `dict[str, str]`，解析器按
/// `options["headers"]["User-Agent"]` 之类的键读。
fn request_options(request: &Aria2Request) -> Map<String, Value> {
    let mut options = Map::new();
    options.insert(URL.to_string(), Value::String(request.url.clone()));
    options.insert(
        OUTPUT_FOLDER.to_string(),
        Value::String(request.output_folder.clone()),
    );
    options.insert(
        CLIENT_PROFILE.to_string(),
        Value::String(request.client_profile.clone()),
    );
    let mut headers = Map::new();
    for (name, value) in &request.headers {
        headers.insert(name.clone(), Value::String(value.clone()));
    }
    options.insert(HEADERS.to_string(), Value::Object(headers));
    options
}

/// `Task.setName(name)`：`toSafeFilename(name, fallback=self.name or "download")`
/// （`task.py:284-285`）。
///
/// `fallback` 取的是**当前**名字——名字为空才退到 `"download"`，这正是 Python
/// `self.name or "download"` 的求值时机（调用前）。
fn safe_name(name: &str, current: &str) -> String {
    let fallback = if current.is_empty() {
        "download"
    } else {
        current
    };
    to_safe_filename(name, fallback, NAME_MAX_LENGTH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gd3_core::task::TaskStatus;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// 解析器看到过的 options，按调用顺序。
    type SeenOptions = Rc<RefCell<Vec<Map<String, Value>>>>;

    fn record(url: &str, name: &str) -> TaskRecord {
        TaskRecord {
            name: name.to_string(),
            url: url.to_string(),
            pack_id: String::new(),
            task_id: format!("tsk_{url}"),
            status: TaskStatus::Waiting,
            steps: Vec::new(),
            created_at: 0,
            completed_at: 0,
            output_folder: String::new(),
            file_size: 0,
            files: None,
            category: None,
            should_seed: false,
            checksums: Map::new(),
            extra: Map::new(),
            type_name: None,
        }
    }

    /// 命中一切、返回固定记录的解析器；`seen` 留下它看到的 options。
    struct Echo {
        name: &'static str,
        seen: SeenOptions,
    }

    impl Echo {
        fn new(name: &'static str) -> Self {
            Echo {
                name,
                seen: SeenOptions::default(),
            }
        }
    }

    impl crate::feature_service::TaskParser for Echo {
        fn matches(&self, _options: &Map<String, Value>) -> bool {
            true
        }

        fn parse(&self, options: &Map<String, Value>) -> Result<TaskRecord, TaskError> {
            self.seen.borrow_mut().push(options.clone());
            let url = options
                .get(URL)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            Ok(record(&url, self.name))
        }
    }

    /// 一个只带 `Echo` 的服务。
    fn service_with(parser: Echo) -> (FeatureService, SeenOptions) {
        let seen = Rc::clone(&parser.seen);
        let mut features = FeatureService::new();
        features.parsers_mut().register(Box::new(parser));
        (features, seen)
    }

    fn request(url: &str) -> Aria2Request {
        Aria2Request {
            url: url.to_string(),
            output_folder: "/downloads".to_string(),
            headers: Vec::new(),
            client_profile: "raw".to_string(),
            name: String::new(),
        }
    }

    #[test]
    fn add_uri_reports_an_add_task() {
        let (features, _) = service_with(Echo::new("parsed.bin"));
        let effects = apply_task_options(&features, &[], request("https://a.example/f"), false);

        assert_eq!(effects.len(), 1);
        let Aria2Effect::AddTask(task) = &effects[0] else {
            panic!("应当直加：{effects:?}");
        };
        assert_eq!(task.url, "https://a.example/f");
        assert_eq!(task.name, "parsed.bin");
    }

    #[test]
    fn the_out_option_renames_the_task() {
        let (features, _) = service_with(Echo::new("parsed.bin"));
        let mut req = request("https://a.example/f");
        req.name = "报告.pdf".to_string();
        let effects = apply_task_options(&features, &[], req, false);

        let Aria2Effect::AddTask(task) = &effects[0] else {
            panic!("应当直加：{effects:?}");
        };
        assert_eq!(task.name, "报告.pdf");
    }

    #[test]
    fn a_sanitized_out_option_keeps_the_extension() {
        let (features, _) = service_with(Echo::new("parsed.bin"));
        let mut req = request("https://a.example/f");
        req.name = "a/b:c.txt".to_string();
        let effects = apply_task_options(&features, &[], req, false);

        let Aria2Effect::AddTask(task) = &effects[0] else {
            panic!("应当直加：{effects:?}");
        };
        assert_eq!(task.name, "a_b_c.txt");
    }

    #[test]
    fn an_empty_out_option_keeps_the_parsed_name() {
        let (features, _) = service_with(Echo::new("parsed.bin"));
        let effects = apply_task_options(&features, &[], request("https://a.example/f"), false);

        let Aria2Effect::AddTask(task) = &effects[0] else {
            panic!("应当直加：{effects:?}");
        };
        assert_eq!(task.name, "parsed.bin");
    }

    #[test]
    fn an_unusable_out_option_falls_back_to_the_parsed_name() {
        let (features, _) = service_with(Echo::new("parsed.bin"));
        let mut req = request("https://a.example/f");
        // `to_safe_filename` 会剥掉结尾的点，剥空了就退到 fallback
        req.name = "...".to_string();
        let effects = apply_task_options(&features, &[], req, false);

        let Aria2Effect::AddTask(task) = &effects[0] else {
            panic!("应当直加：{effects:?}");
        };
        assert_eq!(task.name, "parsed.bin");
    }

    #[test]
    fn an_empty_parsed_name_falls_back_to_download() {
        let (features, _) = service_with(Echo::new(""));
        let mut req = request("https://a.example/f");
        req.name = "...".to_string();
        let effects = apply_task_options(&features, &[], req, false);

        let Aria2Effect::AddTask(task) = &effects[0] else {
            panic!("应当直加：{effects:?}");
        };
        assert_eq!(task.name, "download");
    }

    #[test]
    fn the_draft_setting_routes_to_draft_requested() {
        let (features, _) = service_with(Echo::new("parsed.bin"));
        let effects = apply_task_options(&features, &[], request("https://a.example/f"), true);

        assert_eq!(effects.len(), 1);
        assert!(
            matches!(&effects[0], Aria2Effect::DraftRequested(task) if task.url == "https://a.example/f"),
            "应当进草稿：{effects:?}"
        );
    }

    #[test]
    fn no_parser_matched_becomes_a_parse_failed_effect() {
        let features = FeatureService::new();
        let effects = apply_task_options(&features, &[], request("https://a.example/f"), false);

        assert_eq!(effects.len(), 1);
        let Aria2Effect::ParseFailed { url, error } = &effects[0] else {
            panic!("应当报错：{effects:?}");
        };
        assert_eq!(url, "https://a.example/f");
        assert_eq!(
            error.format(),
            "发生了意外错误：No parser matched: https://a.example/f"
        );
    }

    #[test]
    fn the_request_options_carry_headers_folder_and_profile() {
        let (features, seen) = service_with(Echo::new("parsed.bin"));
        let mut req = request("https://a.example/f");
        req.output_folder = "/data".to_string();
        req.client_profile = String::new();
        req.headers = vec![
            ("User-Agent".to_string(), "aria2/1.37".to_string()),
            ("Referer".to_string(), "https://a.example/".to_string()),
        ];
        apply_task_options(&features, &[], req, false);

        let seen = seen.borrow();
        let options = seen.last().expect("解析器应当被调用");
        assert_eq!(
            options.get(OUTPUT_FOLDER).and_then(Value::as_str),
            Some("/data")
        );
        assert_eq!(
            options.get(CLIENT_PROFILE).and_then(Value::as_str),
            Some("")
        );
        let headers = options
            .get(HEADERS)
            .and_then(Value::as_object)
            .expect("headers 应当是对象");
        assert_eq!(
            headers.get("User-Agent").and_then(Value::as_str),
            Some("aria2/1.37")
        );
        assert_eq!(
            headers.get("Referer").and_then(Value::as_str),
            Some("https://a.example/")
        );
    }

    #[test]
    fn an_identity_preset_fills_the_profile_before_matching() {
        // 空的 clientProfile 会被预设补上，解析器看到的是补过的值。
        // 缺省预设只认 `*.pcs.baidu.com`（`identity.rs` 的 `default_presets`）。
        let (features, seen) = service_with(Echo::new("parsed.bin"));
        let presets = crate::identity::parse_presets(None);
        let mut req = request("https://pan.pcs.baidu.com/rest/2.0/x");
        req.client_profile = String::new();
        apply_task_options(&features, &presets, req, false);

        let seen = seen.borrow();
        let options = seen.last().expect("解析器应当被调用");
        assert_eq!(
            options.get(CLIENT_PROFILE).and_then(Value::as_str),
            Some("raw")
        );
    }
}
