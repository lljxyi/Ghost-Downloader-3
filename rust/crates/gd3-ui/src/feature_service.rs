//! 功能包服务（`app/services/feature_service.py`，234 行）的**可移植内核**。
//!
//! ## 搬了什么
//!
//! - `TaskParser` 的三段契约（`app/models/pack.py:38-52`）：`priority`（缺省 100）、
//!   `match` / `matchPassive`（后者缺省转前者）、`parse`；
//! - [`ParserChain`]：`FeatureService.register` 的「追加 + 按 `priority` **稳定**排序」
//!   （`feature_service.py:61`）与 `parse` / `match` / `matchPassive` 的判定顺序
//!   ——先到先得，`parse` 一个都不中时抛 `ValueError`（:88）；
//! - [`apply_identity_preset`]：`parse` 的前置（:77-89）——`options.clientProfile` 为空
//!   时按 host 找预设（`app/client.py:83` 的 `matchIdentityPreset`），把
//!   `clientProfile` / `userAgent` 补进 options。
//!
//! ## 没搬（结构性不可移植，不是「还没做」）
//!
//! `FeatureService` 的其余成员全长在 Python 插件上：
//!
//! | Python | 为什么不行 |
//! |---|---|
//! | `load()` / `register()` 往 pack 上回填 `parse` / `addTask` / `submit` | 需要 `FeaturePack` 实例（`features/` 下的 Python 类），Rust 侧加载不了 |
//! | `taskCard` / `draftCard` / `optionCards` / `editCards` / `detailCards` | 返回 PySide6 `QWidget`；Slint 侧按 `packId` 自己分发卡片 |
//! | `pages()` / `settingGroups()` / `createPage` | `PackPage` / `CollapsibleSettingCardGroup` 都是 Qt 类 |
//! | `runtimes()` / `fileTypes()` / `uriSchemes()` 的聚合 | 数据源是 pack 方法；本仓没有 pack 实例可聚合 |
//! | `_registerAssociations()` / `isFileAssociationEnabled()` / `setFileAssociation()` | 依赖上面三项（`gd3-core::file_association` 只有纯数据层） |
//! | `activate()` / `deactivate()` | 协程 + `threading.Event` 等待；没有 pack 生命周期 |
//! | `PackConfig.__init_subclass__` 往 `cfg` 上挂 `pack_<类名>_<属性>` | 动态改配置类；Rust 的 `SPECS` 是静态表 |
//!
//! 于是 [`FeatureService`] 在这里只剩「持有 parser 链」与一个立即返回的 `deactivate()`
//! ——Python 在没有 pack 时同样空转（提交一个空协程再等它置位）。

use gd3_core::task::TaskRecord;
use gd3_core::task_error::TaskError;
use serde_json::{Map, Value};

use crate::identity::{IdentityPreset, match_identity_preset};

/// `TaskParser.priority` 的缺省值（`app/models/pack.py:42`）。
pub const DEFAULT_PRIORITY: i64 = 100;

/// `app/models/task.py:64-65` 的两个键名。
const CLIENT_PROFILE: &str = "clientProfile";
const USER_AGENT: &str = "userAgent";
const URL: &str = "url";

/// `app/models/pack.py::TaskParser` 的可移植契约。
pub trait TaskParser {
    /// 小的先跑（`self._parsers.sort(key=lambda p: p.priority)`）。
    fn priority(&self) -> i64 {
        DEFAULT_PRIORITY
    }

    /// `match(options)`。
    fn matches(&self, options: &Map<String, Value>) -> bool;

    /// `matchPassive(options)`：缺省转 `match`（`pack.py:49-50`）。
    fn matches_passive(&self, options: &Map<String, Value>) -> bool {
        self.matches(options)
    }

    /// `parse(options)`。
    ///
    /// Python 是 `async def` 且返回 `Task`；这里同步返回 `TaskRecord`，需要异步的实现
    /// 自己在内部阻塞（同 `gd3-core::checksum` 的口径）。
    fn parse(&self, options: &Map<String, Value>) -> Result<TaskRecord, TaskError>;
}

/// `parse` 的两种失败：链上没人认领，或认领的解析器自己出错。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// `raise ValueError(f"No parser matched: {options.url}")`（`feature_service.py:88`）。
    NoParserMatched { url: String },
    /// 解析器抛出的 `TaskError`（Python 侧原样冒泡）。
    Task(TaskError),
}

impl ParseError {
    /// 落到 `TaskError`（`toTaskError` 口径：`ValueError` → `发生了意外错误：{detail}`）。
    pub fn to_task_error(&self) -> TaskError {
        match self {
            ParseError::NoParserMatched { url } => {
                TaskError::io(format!("No parser matched: {url}"))
            }
            ParseError::Task(error) => error.clone(),
        }
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::NoParserMatched { url } => write!(formatter, "No parser matched: {url}"),
            ParseError::Task(error) => formatter.write_str(&error.format()),
        }
    }
}

/// `FeatureService._parsers` 的容器。
#[derive(Default)]
pub struct ParserChain {
    parsers: Vec<Box<dyn TaskParser>>,
}

impl ParserChain {
    pub fn new() -> Self {
        ParserChain::default()
    }

    /// `register` 里的 `append` + `sort(key=priority)`（`feature_service.py:60-61`）。
    ///
    /// Python 的 `list.sort` 是**稳定**的，同优先级保持注册顺序；这里「插到第一个
    /// `priority` 更大的元素之前」，得到的序与稳定排序一致。
    pub fn register(&mut self, parser: Box<dyn TaskParser>) {
        let priority = parser.priority();
        match self.parsers.iter().position(|p| p.priority() > priority) {
            Some(index) => self.parsers.insert(index, parser),
            None => self.parsers.push(parser),
        }
    }

    pub fn len(&self) -> usize {
        self.parsers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.parsers.is_empty()
    }

    /// 已注册解析器按运行顺序的 `priority`（测试与宿主用来核对排序）。
    pub fn priorities(&self) -> Vec<i64> {
        self.parsers
            .iter()
            .map(|parser| parser.priority())
            .collect()
    }

    /// `FeatureService.parse`：预设前置 → 按序第一个 `match` 的解析器出结果。
    ///
    /// `options` 就地补 `clientProfile` / `userAgent`（Python 的 `replace` 造新对象，
    /// 调用方持有的那份不变；本仓 `DraftEffect::Parse` 交出去的就是可变副本，
    /// 语义等价）。
    pub fn parse(
        &self,
        options: &mut Map<String, Value>,
        presets: &[IdentityPreset],
    ) -> Result<TaskRecord, ParseError> {
        apply_identity_preset(options, presets);
        for parser in &self.parsers {
            if parser.matches(options) {
                return parser.parse(options).map_err(ParseError::Task);
            }
        }
        Err(ParseError::NoParserMatched {
            url: url_of(options),
        })
    }

    /// `FeatureService.match(url)`。
    pub fn matches(&self, url: &str) -> bool {
        let options = url_options(url);
        self.parsers.iter().any(|parser| parser.matches(&options))
    }

    /// `FeatureService.matchPassive(url)`。
    pub fn matches_passive(&self, url: &str) -> bool {
        let options = url_options(url);
        self.parsers
            .iter()
            .any(|parser| parser.matches_passive(&options))
    }
}

/// `FeatureService.parse` 的前置：`options.clientProfile` 为空时按 host 补预设。
///
/// 返回是否改动了 `options`。语义照 `feature_service.py:77-89`：
/// - 已有的 `clientProfile` 不覆盖（`if not options.clientProfile`）；
/// - 只补**非空**字段（`if preset["clientProfile"]`）；
/// - 两个字段都空就什么也不做（`if kwargs`）。
pub fn apply_identity_preset(options: &mut Map<String, Value>, presets: &[IdentityPreset]) -> bool {
    if is_truthy(options.get(CLIENT_PROFILE)) {
        return false;
    }
    let host = gd3_core::url::host_of(&url_of(options));
    let Some(preset) = match_identity_preset(&host, presets) else {
        return false;
    };
    let mut changed = false;
    if !preset.client_profile.is_empty() {
        options.insert(
            CLIENT_PROFILE.to_string(),
            Value::String(preset.client_profile.clone()),
        );
        changed = true;
    }
    if !preset.user_agent.is_empty() {
        options.insert(
            USER_AGENT.to_string(),
            Value::String(preset.user_agent.clone()),
        );
        changed = true;
    }
    changed
}

/// `app/services/feature_service.py::FeatureService` 的可移植切片。
#[derive(Default)]
pub struct FeatureService {
    parsers: ParserChain,
}

impl FeatureService {
    pub fn new() -> Self {
        FeatureService::default()
    }

    pub fn parsers(&self) -> &ParserChain {
        &self.parsers
    }

    pub fn parsers_mut(&mut self) -> &mut ParserChain {
        &mut self.parsers
    }

    /// `activate()`：逐个 `await pack.activate()`（`feature_service.py:43-48`）。
    ///
    /// 没有 pack → `activateAll()` 的循环体不执行，`submit` 一个空协程。这里连
    /// 协程都不用造，直接返回（Python 在没有 pack 时同样不产生任何副作用）。
    pub fn activate(&self) {}

    /// `deactivate()`：提交 `deactivateAll()` 再 `event.wait()`（`feature_service.py:225-234`）。
    ///
    /// 没有 pack 实现 → 没有可等的协程 → 立即返回（Python 在没有 pack 时同样空转）。
    pub fn deactivate(&self) {}
}

/// `TaskOptions(url=url)` 的最小等价物。
///
/// Python 还会填 `outputFolder` / `headers` / `subworkerCount`（都要读 `cfg`）；
/// 现有八个解析器的 `match` / `matchPassive` 只读 `options.url`
/// （`features/*/pack.py` 实测），所以这里只放 `url`，把读配置留给调用方。
fn url_options(url: &str) -> Map<String, Value> {
    let mut options = Map::new();
    options.insert(URL.to_string(), Value::String(url.to_string()));
    options
}

fn url_of(options: &Map<String, Value>) -> String {
    options
        .get(URL)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Python 的真值判定（`not options.clientProfile`）。
fn is_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|n| n != 0.0),
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Object(map)) => !map.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::parse_presets;
    use gd3_core::task::TaskStatus;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn record(url: &str) -> TaskRecord {
        TaskRecord {
            name: format!("{url}.bin"),
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

    /// 记录被问过什么的解析器：`log` 里按顺序落 `match@<priority>` / `parse@<priority>`。
    struct Stub {
        priority: i64,
        matched: bool,
        task_url: &'static str,
        log: Rc<RefCell<Vec<String>>>,
    }

    impl Stub {
        fn new(priority: i64, matched: bool, task_url: &'static str) -> Self {
            Stub {
                priority,
                matched,
                task_url,
                log: Rc::new(RefCell::new(Vec::new())),
            }
        }
    }

    impl TaskParser for Stub {
        fn priority(&self) -> i64 {
            self.priority
        }

        fn matches(&self, _options: &Map<String, Value>) -> bool {
            self.log
                .borrow_mut()
                .push(format!("match@{}", self.priority));
            self.matched
        }

        fn parse(&self, _options: &Map<String, Value>) -> Result<TaskRecord, TaskError> {
            self.log
                .borrow_mut()
                .push(format!("parse@{}", self.priority));
            Ok(record(self.task_url))
        }
    }

    /// 只有 `matchPassive` 与 `match` 不一致时才需要它。
    struct PassiveStub {
        matched: bool,
        passive: bool,
    }

    impl TaskParser for PassiveStub {
        fn matches(&self, _options: &Map<String, Value>) -> bool {
            self.matched
        }

        fn matches_passive(&self, _options: &Map<String, Value>) -> bool {
            self.passive
        }

        fn parse(&self, _options: &Map<String, Value>) -> Result<TaskRecord, TaskError> {
            unreachable!("matchPassive 的判定不该走到 parse")
        }
    }

    /// 只在 `clientProfile == "raw"` 时命中：钉住「预设前置先于 match」。
    struct RawProfileParser;

    impl TaskParser for RawProfileParser {
        fn matches(&self, options: &Map<String, Value>) -> bool {
            options.get(CLIENT_PROFILE).and_then(Value::as_str) == Some("raw")
        }

        fn parse(&self, options: &Map<String, Value>) -> Result<TaskRecord, TaskError> {
            Ok(record(&url_of(options)))
        }
    }

    fn options_for(url: &str) -> Map<String, Value> {
        url_options(url)
    }

    #[test]
    fn register_sorts_by_priority_and_keeps_registration_order() {
        let mut chain = ParserChain::new();
        chain.register(Box::new(Stub::new(100, true, "high")));
        chain.register(Box::new(Stub::new(50, true, "first")));
        chain.register(Box::new(Stub::new(50, true, "second")));

        assert_eq!(chain.priorities(), vec![50, 50, 100]);

        // 稳定排序：同优先级里先注册的先跑
        let mut options = options_for("https://example.com/a.zip");
        let task = chain.parse(&mut options, &[]).expect("应当命中");
        assert_eq!(task.url, "first");
    }

    #[test]
    fn parse_picks_the_lowest_priority_match() {
        let mut chain = ParserChain::new();
        chain.register(Box::new(Stub::new(100, true, "high")));
        chain.register(Box::new(Stub::new(50, true, "low")));

        let mut options = options_for("https://example.com/a.zip");
        let task = chain.parse(&mut options, &[]).expect("应当命中");
        assert_eq!(task.url, "low");
    }

    #[test]
    fn parse_skips_parsers_that_do_not_match() {
        let first = Stub::new(50, false, "never");
        let log = Rc::clone(&first.log);
        let mut chain = ParserChain::new();
        chain.register(Box::new(first));
        chain.register(Box::new(Stub::new(100, true, "second")));

        let mut options = options_for("https://example.com/a.zip");
        let task = chain.parse(&mut options, &[]).expect("应当命中");
        assert_eq!(task.url, "second");
        // 第一个只被问过 match，没有 parse
        assert_eq!(*log.borrow(), vec!["match@50".to_string()]);
    }

    #[test]
    fn parse_without_a_match_reports_no_parser_matched() {
        let mut chain = ParserChain::new();
        chain.register(Box::new(Stub::new(50, false, "never")));

        let mut options = options_for("https://example.com/a.zip");
        let error = chain.parse(&mut options, &[]).expect_err("不该命中");
        assert_eq!(
            error,
            ParseError::NoParserMatched {
                url: "https://example.com/a.zip".to_string()
            }
        );
        assert_eq!(
            error.to_task_error().format(),
            "发生了意外错误：No parser matched: https://example.com/a.zip"
        );
    }

    #[test]
    fn parser_errors_pass_through_untouched() {
        struct Failing;

        impl TaskParser for Failing {
            fn matches(&self, _options: &Map<String, Value>) -> bool {
                true
            }

            fn parse(&self, _options: &Map<String, Value>) -> Result<TaskRecord, TaskError> {
                Err(TaskError::disk_full())
            }
        }

        let mut chain = ParserChain::new();
        chain.register(Box::new(Failing));

        let mut options = options_for("https://example.com/a.zip");
        let error = chain.parse(&mut options, &[]).expect_err("不该命中");
        assert_eq!(error, ParseError::Task(TaskError::disk_full()));
        assert_eq!(error.to_task_error(), TaskError::disk_full());
    }

    #[test]
    fn matches_passive_falls_back_to_matches() {
        let mut chain = ParserChain::new();
        chain.register(Box::new(Stub::new(100, true, "task")));

        assert!(chain.matches("https://example.com/a.zip"));
        assert!(chain.matches_passive("https://example.com/a.zip"));
    }

    #[test]
    fn matches_and_matches_passive_are_independent() {
        let mut chain = ParserChain::new();
        chain.register(Box::new(PassiveStub {
            matched: false,
            passive: true,
        }));

        assert!(!chain.matches("https://example.com/a.zip"));
        assert!(chain.matches_passive("https://example.com/a.zip"));
    }

    #[test]
    fn match_only_gets_the_url_key() {
        struct UrlOnly;

        impl TaskParser for UrlOnly {
            fn matches(&self, options: &Map<String, Value>) -> bool {
                assert_eq!(options.len(), 1);
                options.get(URL).and_then(Value::as_str) == Some("https://example.com/a.zip")
            }

            fn parse(&self, _options: &Map<String, Value>) -> Result<TaskRecord, TaskError> {
                unreachable!("只测 match")
            }
        }

        let mut chain = ParserChain::new();
        chain.register(Box::new(UrlOnly));
        assert!(chain.matches("https://example.com/a.zip"));
    }

    #[test]
    fn identity_preset_fills_an_empty_client_profile() {
        let presets = parse_presets(None);
        let mut options = options_for("https://pcs.baidu.com/rest/2.0/x");

        assert!(apply_identity_preset(&mut options, &presets));
        assert_eq!(
            options.get(CLIENT_PROFILE).and_then(Value::as_str),
            Some("raw")
        );
        assert_eq!(
            options.get(USER_AGENT).and_then(Value::as_str),
            Some("pan.baidu.com")
        );
    }

    #[test]
    fn identity_preset_leaves_an_explicit_profile_alone() {
        let presets = parse_presets(None);
        let mut options = options_for("https://pcs.baidu.com/rest/2.0/x");
        options.insert(CLIENT_PROFILE.to_string(), Value::String("auto".into()));

        assert!(!apply_identity_preset(&mut options, &presets));
        assert_eq!(
            options.get(CLIENT_PROFILE).and_then(Value::as_str),
            Some("auto")
        );
        assert!(!options.contains_key(USER_AGENT));
    }

    #[test]
    fn identity_preset_treats_null_as_empty() {
        let presets = parse_presets(None);
        let mut options = options_for("https://pcs.baidu.com/rest/2.0/x");
        options.insert(CLIENT_PROFILE.to_string(), Value::Null);

        assert!(apply_identity_preset(&mut options, &presets));
        assert_eq!(
            options.get(CLIENT_PROFILE).and_then(Value::as_str),
            Some("raw")
        );
    }

    #[test]
    fn identity_preset_skips_unknown_hosts() {
        let presets = parse_presets(None);
        let mut options = options_for("https://example.com/a.zip");

        assert!(!apply_identity_preset(&mut options, &presets));
        assert!(!options.contains_key(CLIENT_PROFILE));
    }

    #[test]
    fn identity_preset_fills_nothing_when_the_preset_is_empty() {
        let presets = vec![IdentityPreset {
            name: "空预设".into(),
            client_profile: String::new(),
            user_agent: String::new(),
            hosts: vec!["*.example.com".into()],
            is_enabled: true,
        }];
        let mut options = options_for("https://a.example.com/x");

        // 命中了预设，但两个字段都空 → `if kwargs` 不成立，什么也不补
        assert!(!apply_identity_preset(&mut options, &presets));
        assert!(!options.contains_key(CLIENT_PROFILE));
        assert!(!options.contains_key(USER_AGENT));
    }

    #[test]
    fn parse_applies_the_preset_before_matching() {
        let mut chain = ParserChain::new();
        chain.register(Box::new(RawProfileParser));

        let mut options = options_for("https://pcs.baidu.com/rest/2.0/x");
        let task = chain
            .parse(&mut options, &parse_presets(None))
            .expect("预设前置应当让解析器命中");
        assert_eq!(task.url, "https://pcs.baidu.com/rest/2.0/x");
        assert_eq!(
            options.get(CLIENT_PROFILE).and_then(Value::as_str),
            Some("raw")
        );
    }

    #[test]
    fn a_feature_service_without_packs_has_an_empty_chain() {
        let mut service = FeatureService::new();
        assert!(service.parsers().is_empty());
        service
            .parsers_mut()
            .register(Box::new(Stub::new(100, true, "task")));
        assert_eq!(service.parsers().len(), 1);
        // 没有 pack → 没有可等的协程，立即返回
        service.deactivate();
    }
}
