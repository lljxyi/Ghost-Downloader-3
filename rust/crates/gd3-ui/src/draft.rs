//! 任务草稿（P2，第 63 步）：镜像 `app/services/task_draft.py::TaskDraft` 的
//! 状态机 + `app/view/dialogs/task_draft.py` 的纯函数部分（URL 拆行/去重、
//! 剪贴板过滤、导入文件过滤器、解析结果统计、错误提示的 URL 截断）。
//!
//! 为什么先做这一层：草稿对话框（`task_draft.py` 349 行）与「稍后下载」、
//! 浏览器接管草稿、批量导入都只是这层状态机的 Qt 外壳；把状态机与信号
//! 顺序钉死，Slint 侧才有可断言的装配点。
//!
//! ## 实证口径（2026-10-17 实读 `app/services/task_draft.py`）
//!
//! - **信号顺序是契约**，逐条照抄：
//!   - `setUrls`：`parsingBusyChanged` → 每个提交失败的 `parseFailed` →
//!     `itemsChanged`（:108-121）；
//!   - `clear`：`itemsCleared` **然后** `parsingBusyChanged`（:181-186，与
//!     `setUrls` 相反）；
//!   - `confirm`：先逐项 `taskConfirmed`，再对「有 parseId 且没记下
//!     confirmedOptions」的项取消，最后 `parsingBusyChanged` + `itemsCleared`
//!     （:163-178，顺序与 `clear` 相反）；
//!   - `refresh`：`parsingBusyChanged` → 失败时 `parseFailed` → `itemsChanged`。
//! - `setUrls` 用 `difflib.SequenceMatcher(a=旧,b=新)` 复用「没变」的草稿项。
//!   输入已按首次出现去重（:100-101），所以两侧元素都唯一 → 匹配块恰好是
//!   **交集**，复用项保持原状态、新增项按新顺序创建、删除项取消解析。
//!   本模块直接用「按新顺序取旧项、缺失则新建」，观测结果与 difflib 等价
//!   （差异只在 `parseId` 取消的调用次序，Python 侧取消顺序对结果无影响）。
//! - `canConfirm` = 任一项 `parseId` 非空**或** `task` 非空（:49-50）。
//! - `confirm` 的两段式：已解析项立刻确认；仍在解析的项记下
//!   `confirmedOptions` / `confirmedAutoStart`，等解析回来再确认（:163-178）。
//! - `_onParsed` 先看 `confirmedOptions`（后台确认分支），再看 `parseId`
//!   是否还在（:203-216）。
//! - `updateStats`（`card_groups.py:79-94`）：`total = success + fail`；为 0
//!   时两个标签都隐藏；`fail > 0` 才显示 `"{success}/{total}"`；尺寸标签
//!   `totalSize > 0` → `toReadableSize`，否则 `success > 0` → `"{n} 个任务"`，
//!   否则隐藏。
//! - `_onParseFailed` 的 URL 展示：`len <= 48` 原样，否则 `url[:45] + "..."`
//!   （`task_draft.py:300-301`）。
//! - `_onImportClicked` 的过滤器：先一条
//!   `所有可导入文件 (*ext1 *ext2 ...)`（全部文件类型的扩展名摊平），再每个
//!   类型一条 `{displayName} (*ext ...)`，用 `";;"` 连接（:341-349）。
//!
//! ## 刻意的偏离（都写在这里，避免以后误判为 bug）
//!
//! 1. **回调按 parseId 认领，而不是按 Python 的对象引用**。Python 的
//!    `_onParsed(task, item)` 拿的是提交时捕获的 item，只检查 `item.parseId`
//!    是否非空、**不比对 id**；同一 URL 被取消后又重新提交时，旧请求的迟到
//!    回调会被当成新请求的结果。本模块按 id 匹配，迟到回调直接丢弃——正常
//!    路径（不重提交）观测一致，异常路径更保守。
//!
//!    由此带来一个必须显式处理的差异：`confirm` 会清空 `items`，Python 却靠
//!    闭包捕获的 item 对象继续持有 `confirmedOptions`。所以本模块额外维护
//!    `pending_confirm: parseId -> (options, autoStart)`，`on_parsed` /
//!    `on_parse_failed` 先查这张表——否则「确认后解析回来再建任务」这条路
//!    会静默丢任务。
//! 2. **`Task` 只保留草稿对话框用得到的字段**（url/name/fileSize/category +
//!    options 快照）。任务本体在 `gd3-core`，这里不复制它的状态机。

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::format::to_readable_size;

/// 草稿项的基础选项（Python 侧是 `OutputFolderCard.options()` /
/// `SubworkerCountCard.options()` 合并出的 dict）。
///
/// 用 `BTreeMap` 而不是 `HashMap`：`_buildOptions` 的键集合要能稳定断言。
pub type Options = BTreeMap<String, String>;

/// 草稿项的解析结果（草稿对话框用到的字段子集）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftTask {
    pub url: String,
    pub name: String,
    /// 字节数；未知/未解析为 0（`_refreshStats` 只累加 `> 0` 的项）。
    pub file_size: i64,
    pub category: Option<String>,
    /// `task.setOptions(...)` 的最后一次快照。
    pub options: Options,
}

impl DraftTask {
    pub fn new(url: &str, name: &str, file_size: i64) -> Self {
        Self {
            url: url.to_string(),
            name: name.to_string(),
            file_size,
            category: None,
            options: Options::new(),
        }
    }
}

/// 一个 URL 的草稿项（`task_draft.py:19-26` 的 `DraftItem`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DraftItem {
    pub url: String,
    /// 非空 = 正在解析；`""` = 没有在跑的解析请求。
    pub parse_id: String,
    pub task: Option<DraftTask>,
    pub error: Option<String>,
    pub category_override: Option<String>,
    /// 「稍后下载/后台确认」时记下的选项，等解析回来再套用。
    pub confirmed_options: Option<Options>,
    pub confirmed_auto_start: bool,
}

impl DraftItem {
    pub fn new(url: &str) -> Self {
        Self {
            url: url.to_string(),
            confirmed_auto_start: true,
            ..Default::default()
        }
    }
}

/// 信号（Python 侧是 `Signal`；这里改成返回值，调用方按顺序转发）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DraftEvent {
    /// `parsingBusyChanged(bool)`
    ParsingBusy(bool),
    /// `parseSucceeded(url, task)`
    ParseSucceeded(String, DraftTask),
    /// `parseFailed(url, error)`
    ParseFailed(String, String),
    /// `itemsChanged()`
    ItemsChanged,
    /// `itemsCleared()`
    ItemsCleared,
    /// `taskConfirmed(task, autoStart)`
    TaskConfirmed(DraftTask, bool),
}

/// 解析请求的提交方（Python 侧是 `coroutineRunner.submit`）。
///
/// `submit` 返回 `parseId`；抛错等价于 `Err(错误串)`，此时草稿项记下
/// `error` 且 `setUrls` 会补发 `ParseFailed`（`task_draft.py:123-135`）。
pub trait DraftParser {
    fn submit(&mut self, url: &str, options: &Options) -> Result<String, String>;
    fn cancel(&mut self, parse_id: &str);
}

/// 什么都不做的解析器（默认实现：给每个 URL 一个序号 id）。
#[derive(Debug, Default)]
pub struct IdParser {
    next: u64,
    pub cancelled: Vec<String>,
}

impl DraftParser for IdParser {
    fn submit(&mut self, _url: &str, _options: &Options) -> Result<String, String> {
        self.next += 1;
        Ok(format!("parse_{}", self.next))
    }

    fn cancel(&mut self, parse_id: &str) {
        self.cancelled.push(parse_id.to_string());
    }
}

/// 草稿状态机（`TaskDraft`）。
pub struct DraftStore<P: DraftParser> {
    parser: P,
    items: Vec<DraftItem>,
    base_options: Options,
    /// 已确认但还在解析的项：`parseId -> (confirmedOptions, autoStart)`。
    ///
    /// Python 靠回调闭包捕获的 item 对象引用继续持有这两个字段（item 已从
    /// `_items` 里移除但对象还活着）；Rust 侧没有对象引用，只能按 `parseId`
    /// 记住，否则「确认后解析回来再建任务」这条路会断（`confirm` 清空
    /// `items` 后 `on_parsed` 找不到项，等于静默丢任务）。
    pending_confirm: BTreeMap<String, (Options, bool)>,
}

impl<P: DraftParser> DraftStore<P> {
    pub fn new(parser: P) -> Self {
        Self {
            parser,
            items: Vec::new(),
            base_options: Options::new(),
            pending_confirm: BTreeMap::new(),
        }
    }

    pub fn parser(&self) -> &P {
        &self.parser
    }

    pub fn urls(&self) -> Vec<String> {
        self.items.iter().map(|item| item.url.clone()).collect()
    }

    pub fn items(&self) -> &[DraftItem] {
        &self.items
    }

    pub fn item_by_url(&self, url: &str) -> Option<&DraftItem> {
        self.items.iter().find(|item| item.url == url)
    }

    pub fn task_by_url(&self, url: &str) -> Option<&DraftTask> {
        self.item_by_url(url).and_then(|item| item.task.as_ref())
    }

    /// `sum(1 for item if item.error is not None)`
    pub fn fail_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.error.is_some())
            .count()
    }

    pub fn can_confirm(&self) -> bool {
        self.items
            .iter()
            .any(|item| !item.parse_id.is_empty() || item.task.is_some())
    }

    pub fn is_parsing(&self) -> bool {
        self.items.iter().any(|item| !item.parse_id.is_empty())
    }

    pub fn base_options(&self) -> &Options {
        &self.base_options
    }

    /// `setBaseOptions`：覆盖基础选项，已解析项立刻重算选项。
    pub fn set_base_options(&mut self, options: Options) -> Vec<DraftEvent> {
        self.base_options = options;
        for item in &mut self.items {
            if item.task.is_some() {
                let built = Self::build_options(&self.base_options, item);
                if let Some(task) = item.task.as_mut() {
                    task.options = built;
                }
            }
        }
        vec![DraftEvent::ItemsChanged]
    }

    /// `setUrlCategory`：记下分类覆盖（`None` = 不覆盖）。
    pub fn set_url_category(&mut self, url: &str, category: Option<&str>) -> Vec<DraftEvent> {
        let Some(item) = self.items.iter_mut().find(|item| item.url == url) else {
            return Vec::new();
        };
        item.category_override = category.map(str::to_string);
        if let Some(task) = item.task.as_mut() {
            task.category = category.map(str::to_string);
        }
        vec![DraftEvent::ItemsChanged]
    }

    /// `update(url, mutate)`：原地改已解析的任务，然后 `itemsChanged`。
    pub fn update(&mut self, url: &str, mutate: impl FnOnce(&mut DraftTask)) -> Vec<DraftEvent> {
        let Some(item) = self.items.iter_mut().find(|item| item.url == url) else {
            return Vec::new();
        };
        let Some(task) = item.task.as_mut() else {
            return Vec::new();
        };
        mutate(task);
        vec![DraftEvent::ItemsChanged]
    }

    /// `setUrls`：按新 URL 列表重建（复用未变项、取消删除项、提交新增项）。
    pub fn set_urls(&mut self, urls: Vec<String>) -> Vec<DraftEvent> {
        let mut seen = HashSet::new();
        let urls: Vec<String> = urls
            .into_iter()
            .filter(|url| seen.insert(url.clone()))
            .collect();

        let previous = std::mem::take(&mut self.items);
        let mut by_url: HashMap<String, DraftItem> = previous
            .into_iter()
            .map(|item| (item.url.clone(), item))
            .collect();

        let mut next: Vec<DraftItem> = Vec::with_capacity(urls.len());
        let mut submit_errors: Vec<(String, String)> = Vec::new();

        for url in urls {
            if let Some(item) = by_url.remove(&url) {
                next.push(item);
                continue;
            }
            let mut item = DraftItem::new(&url);
            if let Err(error) = self.submit(&mut item) {
                submit_errors.push((url.clone(), error));
            }
            next.push(item);
        }

        // 剩下的都是被删掉的 URL：取消还在跑的解析（`task_draft.py:109-112`）。
        for (_, mut item) in by_url {
            if !item.parse_id.is_empty() {
                self.parser.cancel(&item.parse_id);
                item.parse_id.clear();
            }
        }

        self.items = next;
        let mut events = vec![DraftEvent::ParsingBusy(self.is_parsing())];
        for (url, error) in submit_errors {
            events.push(DraftEvent::ParseFailed(url, error));
        }
        events.push(DraftEvent::ItemsChanged);
        events
    }

    /// `refresh`：只对「上次失败」的项重试。
    pub fn refresh(&mut self, url: &str) -> Vec<DraftEvent> {
        let Some(index) = self.items.iter().position(|item| item.url == url) else {
            return Vec::new();
        };
        if self.items[index].error.is_none() {
            return Vec::new();
        }
        let error = self.submit_at(index);
        let mut events = vec![DraftEvent::ParsingBusy(self.is_parsing())];
        if let Some(error) = error {
            events.push(DraftEvent::ParseFailed(url.to_string(), error));
        }
        events.push(DraftEvent::ItemsChanged);
        events
    }

    /// `addParsedTasks`：把外部已解析好的任务塞进草稿，返回新增的 URL。
    pub fn add_parsed_tasks(&mut self, tasks: Vec<DraftTask>) -> (Vec<String>, Vec<DraftEvent>) {
        if tasks.is_empty() {
            return (Vec::new(), Vec::new());
        }
        let mut new_urls: Vec<String> = Vec::new();
        let mut events: Vec<DraftEvent> = Vec::new();

        for mut task in tasks {
            let url = task.url.clone();
            let index = match self.items.iter().position(|item| item.url == url) {
                Some(index) => {
                    let item = &self.items[index];
                    if item.task.is_some() {
                        // 已经解析过了：忽略重复投喂（:141-143）。
                        continue;
                    }
                    if !item.parse_id.is_empty() {
                        let parse_id = item.parse_id.clone();
                        self.parser.cancel(&parse_id);
                        self.items[index].parse_id.clear();
                    }
                    index
                }
                None => {
                    new_urls.push(url.clone());
                    self.items.push(DraftItem::new(&url));
                    self.items.len() - 1
                }
            };
            let built = Self::build_options(&self.base_options, &self.items[index]);
            task.options = built;
            let item = &mut self.items[index];
            item.error = None;
            item.task = Some(task.clone());
            events.push(DraftEvent::ParseSucceeded(url, task));
        }

        events.push(DraftEvent::ParsingBusy(self.is_parsing()));
        events.push(DraftEvent::ItemsChanged);
        (new_urls, events)
    }

    /// `confirm(autoStart)`：两段式确认（已解析立刻确认、在跑的等回来再确认）。
    pub fn confirm(&mut self, auto_start: bool) -> Vec<DraftEvent> {
        let mut events: Vec<DraftEvent> = Vec::new();
        for index in 0..self.items.len() {
            if self.items[index].task.is_some() {
                let built = Self::build_options(&self.base_options, &self.items[index]);
                let task = self.items[index].task.as_mut().expect("task is some");
                task.options = built;
                events.push(DraftEvent::TaskConfirmed(task.clone(), auto_start));
            } else if !self.items[index].parse_id.is_empty() {
                let built = Self::build_options(&self.base_options, &self.items[index]);
                let parse_id = self.items[index].parse_id.clone();
                self.pending_confirm
                    .insert(parse_id, (built.clone(), auto_start));
                self.items[index].confirmed_options = Some(built);
                self.items[index].confirmed_auto_start = auto_start;
            }
        }

        for item in &mut self.items {
            if !item.parse_id.is_empty() && item.confirmed_options.is_none() {
                let parse_id = item.parse_id.clone();
                self.parser.cancel(&parse_id);
                item.parse_id.clear();
            }
        }

        self.items.clear();
        events.push(DraftEvent::ParsingBusy(self.is_parsing()));
        events.push(DraftEvent::ItemsCleared);
        events
    }

    /// `clear()`：全部取消，`itemsCleared` 先于 `parsingBusyChanged`。
    pub fn clear(&mut self) -> Vec<DraftEvent> {
        for item in &mut self.items {
            if !item.parse_id.is_empty() {
                let parse_id = item.parse_id.clone();
                self.parser.cancel(&parse_id);
                item.parse_id.clear();
            }
        }
        self.items.clear();
        vec![
            DraftEvent::ItemsCleared,
            DraftEvent::ParsingBusy(self.is_parsing()),
        ]
    }

    /// 解析成功回调（`_onParsed`）。
    pub fn on_parsed(&mut self, parse_id: &str, mut task: DraftTask) -> Vec<DraftEvent> {
        // 后台确认分支（`_onParsed` :203-209）：项已从列表移除，靠 parseId 认领。
        if let Some((options, auto_start)) = self.pending_confirm.remove(parse_id) {
            task.category = options
                .get("category")
                .cloned()
                .or_else(|| task.category.clone());
            task.options = options;
            return vec![DraftEvent::TaskConfirmed(task, auto_start)];
        }

        let Some(index) = self.items.iter().position(|item| item.parse_id == parse_id) else {
            return Vec::new();
        };

        // 后台确认分支：套用记下的选项并立刻确认（:204-209）。
        if let Some(options) = self.items[index].confirmed_options.take() {
            task.category = options
                .get("category")
                .cloned()
                .or_else(|| task.category.clone());
            task.options = options;
            let auto_start = self.items[index].confirmed_auto_start;
            return vec![DraftEvent::TaskConfirmed(task, auto_start)];
        }

        self.items[index].parse_id.clear();
        let built = Self::build_options(&self.base_options, &self.items[index]);
        task.options = built;
        let url = self.items[index].url.clone();
        self.items[index].task = Some(task.clone());
        vec![
            DraftEvent::ParseSucceeded(url, task),
            DraftEvent::ParsingBusy(self.is_parsing()),
            DraftEvent::ItemsChanged,
        ]
    }

    /// 解析失败回调（`_onParseFailed`）。
    pub fn on_parse_failed(&mut self, parse_id: &str, error: String) -> Vec<DraftEvent> {
        // 后台确认的项解析失败：只丢选项，不报错、不清 parseId（:218-222）。
        if self.pending_confirm.remove(parse_id).is_some() {
            return Vec::new();
        }

        let Some(index) = self.items.iter().position(|item| item.parse_id == parse_id) else {
            return Vec::new();
        };

        if self.items[index].confirmed_options.take().is_some() {
            // 后台确认的项解析失败：只丢选项，不报错、不清 parseId（:218-222）。
            return Vec::new();
        }

        self.items[index].parse_id.clear();
        self.items[index].error = Some(error.clone());
        let url = self.items[index].url.clone();
        vec![
            DraftEvent::ParseFailed(url, error),
            DraftEvent::ParsingBusy(self.is_parsing()),
            DraftEvent::ItemsChanged,
        ]
    }

    fn submit(&mut self, item: &mut DraftItem) -> Result<(), String> {
        item.error = None;
        match self.parser.submit(&item.url, &self.base_options) {
            Ok(parse_id) => {
                item.parse_id = parse_id;
                Ok(())
            }
            Err(error) => {
                item.error = Some(error.clone());
                Err(error)
            }
        }
    }

    fn submit_at(&mut self, index: usize) -> Option<String> {
        let url = self.items[index].url.clone();
        let result = self.parser.submit(&url, &self.base_options);
        match result {
            Ok(parse_id) => {
                self.items[index].parse_id = parse_id;
                self.items[index].error = None;
                None
            }
            Err(error) => {
                self.items[index].error = Some(error.clone());
                Some(error)
            }
        }
    }

    fn build_options(base: &Options, item: &DraftItem) -> Options {
        let mut options = base.clone();
        if let Some(category) = &item.category_override {
            options.insert("category".to_string(), category.clone());
        }
        options
    }
}

/// `task_draft.py::_urls`：按行拆分、去空白、丢空行。
pub fn parse_url_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// `task_draft.py::addUrls` 的增量部分：去掉空串与已存在的 URL（含本次重复）。
pub fn append_unique_urls(existing: &[String], incoming: &[String]) -> Vec<String> {
    let mut seen: HashSet<&str> = existing.iter().map(String::as_str).collect();
    let mut to_add: Vec<String> = Vec::new();
    for url in incoming {
        let stripped = url.trim();
        if stripped.is_empty() {
            continue;
        }
        if seen.insert(stripped) {
            to_add.push(stripped.to_string());
        }
    }
    to_add
}

/// `task_draft.py::_clipboardUrls`：逐行去空白，只留 `matcher` 认的链接。
pub fn clipboard_urls(text: &str, matcher: impl Fn(&str) -> bool) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && matcher(line))
        .map(str::to_string)
        .collect()
}

/// `card_groups.py::DraftCardGroup.updateStats` 的两个标签。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DraftStats {
    /// `Some("{success}/{total}")` 仅在 `fail > 0` 时出现。
    pub ratio: Option<String>,
    /// `Some(toReadableSize(total))` / `Some("{n} 个任务")` / `None`。
    pub size: Option<String>,
}

/// 镜像 `updateStats`：`total == 0` 时两个标签都隐藏。
pub fn draft_stats(success: usize, fail: usize, total_size: i64) -> DraftStats {
    let total = success + fail;
    if total == 0 {
        return DraftStats::default();
    }
    let ratio = (fail > 0).then(|| format!("{success}/{total}"));
    let size = if total_size > 0 {
        Some(to_readable_size(total_size as f64))
    } else if success > 0 {
        Some(format!("{success} 个任务"))
    } else {
        None
    };
    DraftStats { ratio, size }
}

/// `task_draft.py::_onParseFailed` 的 URL 展示：超过 48 字符截断成 45 + "..."。
pub fn error_url_display(url: &str) -> String {
    let chars: Vec<char> = url.chars().collect();
    if chars.len() <= 48 {
        return url.to_string();
    }
    format!("{}...", chars[..45].iter().collect::<String>())
}

/// 导入文件过滤器（`_onImportClicked`）：`(合并过滤器, [每类型过滤器])`。
pub fn import_name_filters(
    file_types: &[(String, Vec<String>)],
    all_label: &str,
) -> (String, Vec<String>) {
    let globs: Vec<String> = file_types
        .iter()
        .flat_map(|(_, extensions)| extensions.iter())
        .map(|ext| format!("*{ext}"))
        .collect();
    let merged = format!("{all_label} ({})", globs.join(" "));
    let per_type: Vec<String> = file_types
        .iter()
        .map(|(display_name, extensions)| {
            let globs: Vec<String> = extensions.iter().map(|ext| format!("*{ext}")).collect();
            format!("{display_name} ({})", globs.join(" "))
        })
        .collect();
    (merged, per_type)
}

/// `Path(p).as_uri()`：`file://` + 百分号编码（`urllib.parse.quote(safe='/')`
/// 的字符集：未保留字符 `A-Za-z0-9-._~` 与 `/` 原样，其余按 UTF-8 编码）。
pub fn file_uri(path: &str) -> String {
    let mut out = String::from("file://");
    for byte in path.as_bytes() {
        let keep =
            byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'.' | b'_' | b'~' | b'/');
        if keep {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// `DraftCardGroup` 的高度口径（`CARD_HEIGHT=35`、`MIN_VISIBLE=5`、
/// 标题行 30px、无内边距）。
pub const DRAFT_CARD_HEIGHT: i32 = 35;
pub const DRAFT_MIN_VISIBLE: usize = 5;
pub const DRAFT_HEADER_HEIGHT: i32 = 30;

/// `sizeHint` / `minimumSizeHint` 的高度：卡片总高（最小可见数）封顶。
pub fn draft_group_height(card_heights: &[i32]) -> (i32, i32) {
    let padding = DRAFT_HEADER_HEIGHT;
    let total: i32 = card_heights.iter().sum();
    let minimum: i32 = card_heights.iter().take(DRAFT_MIN_VISIBLE).sum();
    (padding + minimum.max(0), padding + total.max(minimum))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 记录提交/取消的假解析器：`fail` 里的 URL 提交即失败。
    #[derive(Default)]
    struct FakeParser {
        fail: Vec<String>,
        submitted: Vec<String>,
        cancelled: Vec<String>,
        next: u64,
    }

    impl FakeParser {
        fn with_fail(urls: &[&str]) -> Self {
            Self {
                fail: urls.iter().map(|u| u.to_string()).collect(),
                ..Default::default()
            }
        }
    }

    impl DraftParser for FakeParser {
        fn submit(&mut self, url: &str, _options: &Options) -> Result<String, String> {
            self.submitted.push(url.to_string());
            if self.fail.contains(&url.to_string()) {
                return Err("解析失败".to_string());
            }
            self.next += 1;
            Ok(format!("p{}", self.next))
        }

        fn cancel(&mut self, parse_id: &str) {
            self.cancelled.push(parse_id.to_string());
        }
    }

    fn store() -> DraftStore<FakeParser> {
        DraftStore::new(FakeParser::default())
    }

    #[test]
    fn parse_url_lines_trims_and_drops_blank_lines() {
        assert_eq!(
            parse_url_lines("  http://a/1 \n\n\thttp://b/2\r\n   \n"),
            vec!["http://a/1", "http://b/2"]
        );
        assert!(parse_url_lines("").is_empty());
        assert!(parse_url_lines("\n \n").is_empty());
    }

    #[test]
    fn append_unique_urls_skips_existing_and_duplicates() {
        let existing = vec!["http://a/1".to_string()];
        let incoming = vec![
            " http://a/1 ".to_string(),
            "http://b/2".to_string(),
            "  ".to_string(),
            "http://b/2".to_string(),
            "http://c/3".to_string(),
        ];
        assert_eq!(
            append_unique_urls(&existing, &incoming),
            vec!["http://b/2", "http://c/3"]
        );
    }

    #[test]
    fn clipboard_urls_filters_by_matcher() {
        let text = " http://a/1 \nnot-a-url\n\nmagnet:?xt=1\n";
        let picked = clipboard_urls(text, |line| {
            line.starts_with("http") || line.starts_with("magnet:")
        });
        assert_eq!(picked, vec!["http://a/1", "magnet:?xt=1"]);
    }

    #[test]
    fn set_urls_emits_python_signal_order_and_reuses_unchanged_items() {
        let mut store = store();
        let events = store.set_urls(vec!["u1".into(), "u2".into()]);
        assert_eq!(
            events,
            vec![DraftEvent::ParsingBusy(true), DraftEvent::ItemsChanged]
        );
        assert_eq!(store.urls(), vec!["u1", "u2"]);
        // 解析完成一项。
        let events = store.on_parsed("p1", DraftTask::new("u1", "a.bin", 10));
        assert_eq!(events.len(), 3);
        assert_eq!(
            events[0],
            DraftEvent::ParseSucceeded("u1".into(), DraftTask::new("u1", "a.bin", 10))
        );
        assert_eq!(events[1], DraftEvent::ParsingBusy(true));
        assert_eq!(events[2], DraftEvent::ItemsChanged);

        // 再设成「u1 + u3」：u1 复用（task 保留），u2 被取消，u3 新提交。
        let events = store.set_urls(vec!["u1".into(), "u3".into()]);
        assert_eq!(
            events,
            vec![DraftEvent::ParsingBusy(true), DraftEvent::ItemsChanged]
        );
        assert!(store.task_by_url("u1").is_some());
        assert_eq!(store.parser().cancelled, vec!["p2"]);
        assert_eq!(store.urls(), vec!["u1", "u3"]);
    }

    #[test]
    fn set_urls_reports_submit_errors_after_busy_signal() {
        let mut store = DraftStore::new(FakeParser::with_fail(&["bad"]));
        let events = store.set_urls(vec!["ok".into(), "bad".into()]);
        assert_eq!(
            events,
            vec![
                DraftEvent::ParsingBusy(true),
                DraftEvent::ParseFailed("bad".into(), "解析失败".into()),
                DraftEvent::ItemsChanged,
            ]
        );
        assert_eq!(store.fail_count(), 1);
        assert!(store.can_confirm(), "仍有解析中的项 → 可确认");
    }

    #[test]
    fn can_confirm_requires_parse_id_or_task() {
        let mut store = DraftStore::new(FakeParser::with_fail(&["bad"]));
        store.set_urls(vec!["bad".into()]);
        assert!(!store.can_confirm(), "只有失败项时不可确认");
        store.set_urls(vec!["bad".into(), "ok".into()]);
        assert!(store.can_confirm());
    }

    #[test]
    fn confirm_confirms_parsed_items_and_defers_running_ones() {
        let mut store = store();
        store.set_urls(vec!["u1".into(), "u2".into()]);
        store.on_parsed("p1", DraftTask::new("u1", "a.bin", 7));
        store.set_base_options(Options::from([(
            "subworkerCount".to_string(),
            "8".to_string(),
        )]));

        let events = store.confirm(true);
        // 已解析的 u1 立刻确认；u2 仍在解析 → 记下 confirmedOptions。
        assert_eq!(events.len(), 3);
        match &events[0] {
            DraftEvent::TaskConfirmed(task, auto_start) => {
                assert_eq!(task.url, "u1");
                assert!(*auto_start);
                assert_eq!(
                    task.options.get("subworkerCount").map(String::as_str),
                    Some("8")
                );
            }
            other => panic!("期望 TaskConfirmed，得到 {other:?}"),
        }
        assert_eq!(events[1], DraftEvent::ParsingBusy(false));
        assert_eq!(events[2], DraftEvent::ItemsCleared);
        assert!(store.items().is_empty());

        // 迟到的解析结果走「后台确认」分支：不报错、直接确认。
        let events = store.on_parsed("p2", DraftTask::new("u2", "b.bin", 9));
        assert_eq!(events.len(), 1);
        match &events[0] {
            DraftEvent::TaskConfirmed(task, auto_start) => {
                assert_eq!(task.url, "u2");
                assert!(*auto_start);
            }
            other => panic!("期望后台确认，得到 {other:?}"),
        }
    }

    #[test]
    fn confirm_without_auto_start_keeps_flag_false() {
        let mut store = store();
        store.set_urls(vec!["u1".into()]);
        let events = store.confirm(false);
        assert_eq!(events[0], DraftEvent::ParsingBusy(false));
        assert_eq!(events[1], DraftEvent::ItemsCleared);
        match &store.on_parsed("p1", DraftTask::new("u1", "a", 1))[0] {
            DraftEvent::TaskConfirmed(_, auto_start) => assert!(!*auto_start),
            other => panic!("期望 TaskConfirmed，得到 {other:?}"),
        }
    }

    #[test]
    fn clear_cancels_and_emits_cleared_before_busy() {
        let mut store = store();
        store.set_urls(vec!["u1".into(), "u2".into()]);
        let events = store.clear();
        assert_eq!(
            events,
            vec![DraftEvent::ItemsCleared, DraftEvent::ParsingBusy(false)]
        );
        assert_eq!(store.parser().cancelled, vec!["p1", "p2"]);
        assert!(store.urls().is_empty());
    }

    #[test]
    fn parse_failure_and_refresh_round_trip() {
        let mut store = DraftStore::new(FakeParser::with_fail(&["u1"]));
        store.set_urls(vec!["u1".into()]);
        assert_eq!(store.fail_count(), 1);
        // refresh 只对失败项生效。
        let events = store.refresh("u1");
        assert_eq!(
            events,
            vec![
                DraftEvent::ParsingBusy(false),
                DraftEvent::ParseFailed("u1".into(), "解析失败".into()),
                DraftEvent::ItemsChanged,
            ]
        );
        assert!(store.refresh("nope").is_empty());
    }

    #[test]
    fn parse_failed_callback_mirrors_python_branching() {
        let mut store = store();
        store.set_urls(vec!["u1".into()]);
        let events = store.on_parse_failed("p1", "boom".into());
        assert_eq!(
            events,
            vec![
                DraftEvent::ParseFailed("u1".into(), "boom".into()),
                DraftEvent::ParsingBusy(false),
                DraftEvent::ItemsChanged,
            ]
        );
        // 迟到/未知 parseId → 无事件（Python 侧同样是提前 return）。
        assert!(store.on_parse_failed("p9", "boom".into()).is_empty());
        assert!(
            store
                .on_parsed("p9", DraftTask::new("u9", "x", 1))
                .is_empty()
        );
    }

    #[test]
    fn add_parsed_tasks_appends_new_urls_and_ignores_resolved_duplicates() {
        let mut store = store();
        store.set_urls(vec!["u1".into()]);
        let (new_urls, events) = store.add_parsed_tasks(vec![
            DraftTask::new("u1", "a.bin", 1),
            DraftTask::new("u2", "b.bin", 2),
        ]);
        assert_eq!(new_urls, vec!["u2"]);
        // u1 覆盖解析中的项（取消 p1）+ u2 新建 → 两次 ParseSucceeded。
        assert_eq!(store.parser().cancelled, vec!["p1"]);
        assert_eq!(events.len(), 4);
        assert_eq!(events[2], DraftEvent::ParsingBusy(false));
        assert_eq!(events[3], DraftEvent::ItemsChanged);
        // 再来一次：两项都已解析 → 什么都不做。
        let (new_urls, events) = store.add_parsed_tasks(vec![DraftTask::new("u1", "a.bin", 1)]);
        assert!(new_urls.is_empty());
        // Python 侧即使一项都没处理，末尾仍无条件发 parsingBusyChanged + itemsChanged
        // （task_draft.py:154-156），所以这里不是空事件。
        assert_eq!(
            events,
            vec![DraftEvent::ParsingBusy(false), DraftEvent::ItemsChanged]
        );
    }

    #[test]
    fn set_url_category_overrides_build_options() {
        let mut store = store();
        store.set_urls(vec!["u1".into()]);
        store.on_parsed("p1", DraftTask::new("u1", "a.bin", 1));
        store.set_base_options(Options::from([(
            "outputFolder".to_string(),
            "/tmp".to_string(),
        )]));
        let events = store.set_url_category("u1", Some("cat_video"));
        assert_eq!(events, vec![DraftEvent::ItemsChanged]);
        assert_eq!(
            store.task_by_url("u1").unwrap().category.as_deref(),
            Some("cat_video")
        );
        // 基础选项重算时保留分类覆盖。
        store.set_base_options(Options::from([(
            "outputFolder".to_string(),
            "/data".to_string(),
        )]));
        let task = store.task_by_url("u1").unwrap();
        assert_eq!(
            task.options.get("category").map(String::as_str),
            Some("cat_video")
        );
        assert_eq!(
            task.options.get("outputFolder").map(String::as_str),
            Some("/data")
        );
    }

    #[test]
    fn draft_stats_mirror_update_stats() {
        assert_eq!(draft_stats(0, 0, 0), DraftStats::default());
        // 全成功且有体积：只有尺寸标签。
        assert_eq!(
            draft_stats(3, 0, 1536),
            DraftStats {
                ratio: None,
                size: Some("1.50 KB".to_string()),
            }
        );
        // 全成功但体积未知："{n} 个任务"。
        assert_eq!(
            draft_stats(2, 0, 0),
            DraftStats {
                ratio: None,
                size: Some("2 个任务".to_string()),
            }
        );
        // 有失败：比例标签 + 尺寸标签。
        assert_eq!(
            draft_stats(1, 2, 1024),
            DraftStats {
                ratio: Some("1/3".to_string()),
                size: Some("1.00 KB".to_string()),
            }
        );
        // 有失败、无成功、无体积：只有比例标签。
        assert_eq!(
            draft_stats(0, 1, 0),
            DraftStats {
                ratio: Some("0/1".to_string()),
                size: None,
            }
        );
    }

    #[test]
    fn error_url_display_truncates_past_48_chars() {
        let short = "http://a/1";
        assert_eq!(error_url_display(short), short);
        let exactly_48 = "a".repeat(48);
        assert_eq!(error_url_display(&exactly_48), exactly_48);
        let long = "b".repeat(49);
        assert_eq!(error_url_display(&long), format!("{}...", "b".repeat(45)));
    }

    #[test]
    fn import_name_filters_join_like_python() {
        let types = vec![
            (
                "Torrent".to_string(),
                vec![".torrent".to_string(), ".bt".to_string()],
            ),
            ("Metalink".to_string(), vec![".meta4".to_string()]),
        ];
        let (merged, per_type) = import_name_filters(&types, "所有可导入文件");
        assert_eq!(merged, "所有可导入文件 (*.torrent *.bt *.meta4)");
        assert_eq!(
            per_type,
            vec!["Torrent (*.torrent *.bt)", "Metalink (*.meta4)"]
        );
    }

    #[test]
    fn file_uri_percent_encodes_like_path_as_uri() {
        assert_eq!(file_uri("/tmp/a.bin"), "file:///tmp/a.bin");
        assert_eq!(file_uri("/tmp/a b.bin"), "file:///tmp/a%20b.bin");
        assert_eq!(
            file_uri("/tmp/中文.torrent"),
            "file:///tmp/%E4%B8%AD%E6%96%87.torrent"
        );
        assert_eq!(file_uri("/tmp/a+b#c"), "file:///tmp/a%2Bb%23c");
    }

    #[test]
    fn draft_group_height_caps_at_min_visible() {
        assert_eq!(draft_group_height(&[]), (30, 30));
        assert_eq!(draft_group_height(&[35, 35]), (30 + 70, 30 + 70));
        let heights = vec![35; 8];
        let (minimum, content) = draft_group_height(&heights);
        assert_eq!(minimum, 30 + 5 * 35);
        assert_eq!(content, 30 + 8 * 35);
    }
}
