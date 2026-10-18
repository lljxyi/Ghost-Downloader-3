//! 任务草稿：镜像 `app/services/task_draft.py`（241 行）。
//!
//! 这是「批量 URL 面板」的状态机核心：一串 URL 进来，每个 URL 对应一个
//! [`DraftItem`]（正在解析 / 解析成功 / 解析失败 / 已确认）。`set_urls` 用
//! [`crate::sequence_matcher`] 对「旧 URL 列表 → 新 URL 列表」做增量 diff，只对
//! **变化的那几段**取消旧解析、提交新解析；`equal` 段里的 item 原样保留（连同已
//! 解析出来的 Task）。
//!
//! ```text
//! setUrls(urls)      去重保序 → SequenceMatcher(旧, 新) → 逐段：
//!                      equal   → 原样搬 item（不重新解析）
//!                      replace/delete → 取消旧段里还在解析的 parseId
//!                      replace/insert → 新段每个 url 各提交一次解析
//!                    → parsingBusyChanged → [submitErrors] → itemsChanged
//! refresh(url)       只对「有 error」的 item 重新提交 → busy → [failed] → changed
//! addParsedTasks(ts) 已存在则（有 task 就跳过 / 还在解析就先取消）否则追加 → busy → changed
//! confirm(autoStart) 第一遍：有 task 的直接 taskConfirmed；在解析的记下 confirmedOptions
//!                    第二遍：在解析且没被确认的取消掉
//!                    然后清空 items（还在解析但已确认的移进 detached）→ busy → cleared
//! clear()            取消所有在解析的 → 清空 → cleared → busy
//! ```
//!
//! # 三个缺口与它们的补法
//!
//! 1. **没有协程运行时**：Python 的 `coroutineRunner.submit(coro, done=…, failed=…,
//!    item=…)` 在 Rust 里没有对应物，退化成 [`DraftEffect::Parse`] / [`DraftEffect::Cancel`]
//!    效果队列（同第 100/101/102 步的形态）。宿主拿到 `Parse` 去跑
//!    `featureService.parse(options)`，跑完按 `work_id` 回调 [`TaskDraft::on_parsed`] /
//!    [`TaskDraft::on_parse_failed`]。
//! 2. **回调参数是「闭包捕获的 item」**：Python 把 `item` 塞进闭包，所以回调能直接改
//!    那个对象（哪怕它已经从 `_items` 里摘掉了）。Rust 没有闭包捕获，只能按
//!    `parseId` 反查——而 `parseId` 在取消时被清空，于是「取消后迟到的回调」自然
//!    查不到，与 Python 的 `if not item.parseId: return` 等价。
//! 3. **`confirm()` 之后还在解析的 item 无处安放**：Python 靠闭包继续持有它，Rust 只能
//!    显式存进 [`TaskDraft::detached_items`]。这些 item 必然已经
//!    `confirmed_options.is_some()`（第二遍循环把没确认的取消掉了），所以
//!    `on_parsed` 命中 `detached` 时直接走 `taskConfirmed`，`on_parse_failed` 命中时
//!    直接丢弃（Python 那边只记一条 warning）。
//!
//! # 与 Python 的差异
//!
//! - `submitErrors`（`submit` 抛异常时的兜底）判 N/A：`Parse` 只是往效果队列推一条，
//!   不会失败。所以 `setUrls` 的「busy → 各 submitError → changed」在 Rust 里退化成
//!   「busy → changed」；
//! - `setUrls` 的 `parseId` 由本模块发号（`draft-1` / `draft-2` …，一个全局计数器），
//!   Python 拿的是 `submit` 的返回值。宿主把它当**不透明句柄**原样交回；
//! - `Parse` 效果携带的是**原始 options 字典**（含 `"url"` 键），没有在这里过一层
//!   `TaskOptions::from_options`——Python 会在 `submit` 里转一次，但本仓的
//!   [`TaskOptions`] 只建模 `outputFolder` / `category` 两键，提前转换会把
//!   `headers` / `subworkerCount` 这些尚未建模的键丢掉。

use crate::sequence_matcher::{OpTag, opcodes};
use gd3_core::task::{TaskOptions, TaskRecord};
use gd3_core::task_error::TaskError;
use serde_json::{Map, Value};
use std::collections::HashSet;

/// 一个 URL 的草稿项（`task_draft.DraftItem`）。
#[derive(Debug, Clone, Default)]
pub struct DraftItem {
    pub url: String,
    /// 正在解析时的 workId；空串表示「没在解析」。宿主把它当不透明句柄。
    pub parse_id: String,
    /// 解析成功后的任务对象。
    pub task: Option<TaskRecord>,
    /// 最近一次解析的错误（成功时清空）。
    pub error: Option<TaskError>,
    /// 单 URL 的分类覆盖；`None` = 用 base options 里的分类。
    ///
    /// Python 是 `categoryOverride: str | None`，`setUrlCategory(url, None)` 只是
    /// 清掉覆盖，无法表达「把分类置成 None」——同口径。
    pub category_override: Option<String>,
    /// `confirm()` 时该 item 还在解析 → 记下待确认的 options，解析回来直接确认。
    pub confirmed_options: Option<Map<String, Value>>,
    /// 与 `confirmed_options` 一起记下的 `autoStart`（缺省 `true`，对齐 Python）。
    pub confirmed_auto_start: bool,
}

impl DraftItem {
    fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            confirmed_auto_start: true,
            ..Self::default()
        }
    }

    fn is_parsing(&self) -> bool {
        !self.parse_id.is_empty()
    }
}

/// 草稿的状态变化（Python 的六个信号）。
#[derive(Debug, Clone)]
pub enum DraftEffect {
    /// 派发一次解析：宿主跑 `featureService.parse(options)`，完成后回调 `on_parsed`。
    ///
    /// `options` 是 base options + `url`（+ 分类覆盖），**原样**转交。
    Parse {
        work_id: String,
        options: Map<String, Value>,
    },
    /// 取消一次解析（Python 的 `coroutineRunner.cancel(parseId)`）。
    Cancel { work_id: String },
    /// `parsingBusyChanged`：还有没有 item 在解析。
    Busy { busy: bool },
    /// `parseSucceeded`：解析成功（`addParsedTasks` 与 `onParsed` 共用）。
    Succeeded { url: String, task: TaskRecord },
    /// `parseFailed`。
    Failed { url: String, error: TaskError },
    /// `itemsChanged`。
    Changed,
    /// `itemsCleared`。
    Cleared,
    /// `taskConfirmed`：草稿里这个任务可以交给 `TaskService` 了。
    Confirmed { task: TaskRecord, auto_start: bool },
}

/// `addParsedTasks` 的返回值：新增的 URL（顺序同入参）与效果队列。
#[derive(Debug, Clone, Default)]
pub struct AddParsedReport {
    pub new_urls: Vec<String>,
    pub effects: Vec<DraftEffect>,
}

/// 任务草稿。
#[derive(Debug, Clone, Default)]
pub struct TaskDraft {
    items: Vec<DraftItem>,
    /// 提交解析时与 `"url"` 一起塞进去的公共选项（`outputFolder` / `category` 等）。
    base_options: Map<String, Value>,
    /// `confirm()` 之后仍在解析、但**已经确认**的 item（Python 靠闭包持有）。
    detached: Vec<DraftItem>,
    /// 效果队列里 `work_id` 的发号器。
    issued: u64,
}

impl TaskDraft {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn urls(&self) -> Vec<String> {
        self.items.iter().map(|item| item.url.clone()).collect()
    }

    pub fn items(&self) -> &[DraftItem] {
        &self.items
    }

    pub fn detached_items(&self) -> &[DraftItem] {
        &self.detached
    }

    pub fn base_options(&self) -> &Map<String, Value> {
        &self.base_options
    }

    pub fn item_by_url(&self, url: &str) -> Option<&DraftItem> {
        self.items.iter().find(|item| item.url == url)
    }

    pub fn task_by_url(&self, url: &str) -> Option<&TaskRecord> {
        self.item_by_url(url).and_then(|item| item.task.as_ref())
    }

    /// 还有没有 item 在解析（`_isParsing`）。
    ///
    /// 只看 `items`，**不看 `detached`**——照抄 Python（`confirm` 清空 `_items` 之后
    /// `parsingBusyChanged(False)`，哪怕还有游离的解析在跑）。
    pub fn is_parsing(&self) -> bool {
        self.items.iter().any(DraftItem::is_parsing)
    }

    pub fn fail_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.error.is_some())
            .count()
    }

    pub fn can_confirm(&self) -> bool {
        self.items
            .iter()
            .any(|item| item.is_parsing() || item.task.is_some())
    }

    /// 就地改一个已解析的 item（`update`）。没有 task 的 item 直接忽略。
    pub fn update(&mut self, url: &str, mutate: impl FnOnce(&mut TaskRecord)) -> Vec<DraftEffect> {
        let Some(index) = self.items.iter().position(|item| item.url == url) else {
            return Vec::new();
        };
        let Some(task) = self.items[index].task.as_mut() else {
            return Vec::new();
        };
        mutate(task);
        vec![DraftEffect::Changed]
    }

    /// 换掉公共选项，并把已解析出来的 Task 全部重刷一遍（`setBaseOptions`）。
    pub fn set_base_options(&mut self, options: Map<String, Value>) -> Vec<DraftEffect> {
        self.base_options = options;
        let Self {
            items,
            base_options,
            ..
        } = self;
        for item in items.iter_mut() {
            if item.task.is_some() {
                let options = build_options(base_options, item);
                if let Some(task) = item.task.as_mut() {
                    task.set_options(&TaskOptions::from_options(&options));
                }
            }
        }
        vec![DraftEffect::Changed]
    }

    /// 给单个 URL 设分类覆盖（`setUrlCategory`）。`None` = 取消覆盖。
    pub fn set_url_category(&mut self, url: &str, category: Option<&str>) -> Vec<DraftEffect> {
        let Some(index) = self.items.iter().position(|item| item.url == url) else {
            return Vec::new();
        };
        let item = &mut self.items[index];
        item.category_override = category.map(str::to_string);
        if let Some(task) = item.task.as_mut() {
            task.category = category.map(str::to_string);
        }
        vec![DraftEffect::Changed]
    }

    /// 整体换 URL 列表（`setUrls`）：增量 diff，只重解析变化的段。
    pub fn set_urls(&mut self, urls: &[String]) -> Vec<DraftEffect> {
        let next_urls = dedupe(urls);
        let previous = std::mem::take(&mut self.items);
        let previous_urls: Vec<String> = previous.iter().map(|item| item.url.clone()).collect();
        // 用 `Option` 占位，好把 item 从「旧列表」搬进「新列表」而不复制
        let mut slots: Vec<Option<DraftItem>> = previous.into_iter().map(Some).collect();
        let mut next_items: Vec<DraftItem> = Vec::with_capacity(next_urls.len());
        let mut effects: Vec<DraftEffect> = Vec::new();

        for opcode in opcodes(&previous_urls, &next_urls) {
            if opcode.tag == OpTag::Equal {
                for slot in &mut slots[opcode.i1..opcode.i2] {
                    if let Some(item) = slot.take() {
                        next_items.push(item);
                    }
                }
                continue;
            }
            // 旧段里还在解析的，全部取消；已解析出来的 item 直接丢弃
            for slot in &mut slots[opcode.i1..opcode.i2] {
                if let Some(mut item) = slot.take()
                    && item.is_parsing()
                {
                    effects.push(DraftEffect::Cancel {
                        work_id: std::mem::take(&mut item.parse_id),
                    });
                }
            }
            // 新段每个 url 各提交一次解析
            for url in &next_urls[opcode.j1..opcode.j2] {
                let mut item = DraftItem::new(url.clone());
                submit_new(
                    &self.base_options,
                    &mut self.issued,
                    &mut item,
                    &mut effects,
                );
                next_items.push(item);
            }
        }

        self.items = next_items;
        effects.push(DraftEffect::Busy {
            busy: self.is_parsing(),
        });
        effects.push(DraftEffect::Changed);
        effects
    }

    /// 重试一个失败的 item（`refresh`）。没有 error 的 item 直接忽略。
    pub fn refresh(&mut self, url: &str) -> Vec<DraftEffect> {
        let Some(index) = self.items.iter().position(|item| item.url == url) else {
            return Vec::new();
        };
        if self.items[index].error.is_none() {
            return Vec::new();
        }
        let mut effects: Vec<DraftEffect> = Vec::new();
        self.submit_at(index, &mut effects);
        effects.push(DraftEffect::Busy {
            busy: self.is_parsing(),
        });
        effects.push(DraftEffect::Changed);
        effects
    }

    /// 把外部已经解析好的任务塞进草稿（`addParsedTasks`）。
    ///
    /// 三种分支：已有 task → 跳过；还在解析 → 先取消再接管；没有这个 url → 追加。
    pub fn add_parsed_tasks(&mut self, tasks: Vec<TaskRecord>) -> AddParsedReport {
        let mut report = AddParsedReport::default();
        if tasks.is_empty() {
            return report;
        }

        for mut task in tasks {
            let url = task.url.clone();
            let index = match self.items.iter().position(|item| item.url == url) {
                Some(index) => {
                    if self.items[index].task.is_some() {
                        continue;
                    }
                    if self.items[index].is_parsing() {
                        let work_id = std::mem::take(&mut self.items[index].parse_id);
                        report.effects.push(DraftEffect::Cancel { work_id });
                    }
                    index
                }
                None => {
                    report.new_urls.push(url.clone());
                    self.items.push(DraftItem::new(url.clone()));
                    self.items.len() - 1
                }
            };

            let options = build_options(&self.base_options, &self.items[index]);
            task.set_options(&TaskOptions::from_options(&options));
            let item = &mut self.items[index];
            item.error = None;
            item.task = Some(task.clone());
            report.effects.push(DraftEffect::Succeeded { url, task });
        }

        report.effects.push(DraftEffect::Busy {
            busy: self.is_parsing(),
        });
        report.effects.push(DraftEffect::Changed);
        report
    }

    /// 确认整份草稿（`confirm`）：已解析的直接交出去，在解析的记下待确认，
    /// 剩下没被确认的取消掉。
    pub fn confirm(&mut self, auto_start: bool) -> Vec<DraftEffect> {
        let mut effects: Vec<DraftEffect> = Vec::new();
        let Self {
            items,
            detached,
            base_options,
            ..
        } = self;

        // 第一遍：能交的立刻交，在解析的记下 options
        for item in items.iter_mut() {
            if item.task.is_some() {
                let options = build_options(base_options, item);
                if let Some(task) = item.task.as_mut() {
                    task.set_options(&TaskOptions::from_options(&options));
                    effects.push(DraftEffect::Confirmed {
                        task: task.clone(),
                        auto_start,
                    });
                }
            } else if item.is_parsing() {
                item.confirmed_options = Some(build_options(base_options, item));
                item.confirmed_auto_start = auto_start;
            }
        }

        // 第二遍：在解析但没被确认的（也就是既没 task 又没走上面那条分支的）取消掉
        for item in items.iter_mut() {
            if item.is_parsing() && item.confirmed_options.is_none() {
                effects.push(DraftEffect::Cancel {
                    work_id: std::mem::take(&mut item.parse_id),
                });
            }
        }

        // 已确认但还在解析的 item 得留下来，等回调
        detached.extend(items.drain(..).filter(DraftItem::is_parsing));
        effects.push(DraftEffect::Busy {
            busy: self.is_parsing(),
        });
        effects.push(DraftEffect::Cleared);
        effects
    }

    /// 清空草稿（`clear`）：取消所有在解析的 item。
    ///
    /// `detached` 里的 item 不动——它们是**已确认**的任务，清草稿不该把它们的解析
    /// 掐掉（Python 那边也没有这个概念）。
    pub fn clear(&mut self) -> Vec<DraftEffect> {
        let mut effects: Vec<DraftEffect> = Vec::new();
        for item in self.items.iter_mut() {
            if item.is_parsing() {
                effects.push(DraftEffect::Cancel {
                    work_id: std::mem::take(&mut item.parse_id),
                });
            }
        }
        self.items.clear();
        effects.push(DraftEffect::Cleared);
        effects.push(DraftEffect::Busy {
            busy: self.is_parsing(),
        });
        effects
    }

    /// 解析成功的回调（`_onParsed`）。`work_id` 是 [`DraftEffect::Parse`] 里那个。
    pub fn on_parsed(&mut self, work_id: &str, task: TaskRecord) -> Vec<DraftEffect> {
        // 游离项：`confirm()` 之后还在解析的，直接确认
        if let Some(index) = self
            .detached
            .iter()
            .position(|item| item.parse_id == work_id)
        {
            let mut item = self.detached.remove(index);
            let Some(options) = item.confirmed_options.take() else {
                return Vec::new();
            };
            let mut task = task;
            task.set_options(&TaskOptions::from_options(&options));
            return vec![DraftEffect::Confirmed {
                task,
                auto_start: item.confirmed_auto_start,
            }];
        }

        // 普通项：按 parseId 反查；取消过的 item 已经被清空 parseId，查不到 → 忽略
        let Some(index) = self.items.iter().position(|item| item.parse_id == work_id) else {
            return Vec::new();
        };
        let options = build_options(&self.base_options, &self.items[index]);
        let mut task = task;
        task.set_options(&TaskOptions::from_options(&options));
        let item = &mut self.items[index];
        item.parse_id.clear();
        item.error = None;
        item.task = Some(task.clone());
        let mut effects = vec![DraftEffect::Succeeded {
            url: item.url.clone(),
            task,
        }];
        effects.push(DraftEffect::Busy {
            busy: self.is_parsing(),
        });
        effects.push(DraftEffect::Changed);
        effects
    }

    /// 解析失败的回调（`_onParseFailed`）。
    pub fn on_parse_failed(&mut self, work_id: &str, error: TaskError) -> Vec<DraftEffect> {
        // 游离项：Python 只记一条 warning，然后丢掉
        if let Some(index) = self
            .detached
            .iter()
            .position(|item| item.parse_id == work_id)
        {
            self.detached.remove(index);
            return Vec::new();
        }

        let Some(index) = self.items.iter().position(|item| item.parse_id == work_id) else {
            return Vec::new();
        };
        let item = &mut self.items[index];
        item.parse_id.clear();
        item.error = Some(error.clone());
        let mut effects = vec![DraftEffect::Failed {
            url: item.url.clone(),
            error,
        }];
        effects.push(DraftEffect::Busy {
            busy: self.is_parsing(),
        });
        effects.push(DraftEffect::Changed);
        effects
    }

    fn submit_at(&mut self, index: usize, effects: &mut Vec<DraftEffect>) {
        let Self {
            items,
            base_options,
            issued,
            ..
        } = self;
        submit_new(base_options, issued, &mut items[index], effects);
    }
}

/// `_buildOptions`：base options 的副本 + 分类覆盖。
///
/// 写成自由函数是为了避开「`&mut self.items` 与 `&self.base_options` 同时借」。
/// `Map` 在 `preserve_order` 下是 `IndexMap`：覆盖已有键不移动位置，与 Python
/// `dict.copy()` + `options["category"] = …` 一致。
fn build_options(base: &Map<String, Value>, item: &DraftItem) -> Map<String, Value> {
    let mut options = base.clone();
    if let Some(category) = &item.category_override {
        options.insert("category".to_string(), Value::String(category.clone()));
    }
    options
}

/// `_submit` 的 Rust 形态：清 error、发号、把 `Parse` 推进效果队列。
///
/// Python 在这里还会 `try/except` 兜 `submit` 抛的异常；Rust 的「提交」只是入队，
/// 不会失败，所以没有对应的 `submitErrors`。
fn submit_new(
    base: &Map<String, Value>,
    issued: &mut u64,
    item: &mut DraftItem,
    effects: &mut Vec<DraftEffect>,
) {
    item.error = None;
    let next = *issued + 1;
    *issued = next;
    let work_id = format!("draft-{next}");
    let mut options = build_options(base, item);
    options.insert("url".to_string(), Value::String(item.url.clone()));
    item.parse_id = work_id.clone();
    effects.push(DraftEffect::Parse { work_id, options });
}

/// `setUrls` 开头那个 `[u for u in urls if u not in seen and not seen.add(u)]`。
fn dedupe(urls: &[String]) -> Vec<String> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out: Vec<String> = Vec::with_capacity(urls.len());
    for url in urls {
        if seen.insert(url.as_str()) {
            out.push(url.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use gd3_core::task::TaskStatus;

    fn urls(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    fn options(pairs: &[(&str, &str)]) -> Map<String, Value> {
        let mut map = Map::new();
        for (key, value) in pairs {
            map.insert((*key).to_string(), Value::String((*value).to_string()));
        }
        map
    }

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

    /// `TaskRecord` 没有 `PartialEq`，效果只能摊成字符串再比。
    fn trace(effects: &[DraftEffect]) -> Vec<String> {
        effects
            .iter()
            .map(|effect| match effect {
                DraftEffect::Parse { work_id, options } => format!(
                    "parse {work_id} {}",
                    options
                        .get("url")
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                ),
                DraftEffect::Cancel { work_id } => format!("cancel {work_id}"),
                DraftEffect::Busy { busy } => format!("busy {busy}"),
                DraftEffect::Succeeded { url, .. } => format!("succeeded {url}"),
                DraftEffect::Failed { url, .. } => format!("failed {url}"),
                DraftEffect::Changed => "changed".to_string(),
                DraftEffect::Cleared => "cleared".to_string(),
                DraftEffect::Confirmed { task, auto_start } => {
                    format!("confirmed {} {auto_start}", task.url)
                }
            })
            .collect()
    }

    fn draft_with(items: &[&str]) -> TaskDraft {
        let mut draft = TaskDraft::new();
        draft.set_urls(&urls(items));
        draft
    }

    #[test]
    fn set_urls_submits_each_url_once_and_reports_busy() {
        let mut draft = TaskDraft::new();
        let effects = draft.set_urls(&urls(&["a", "b"]));

        assert_eq!(
            trace(&effects),
            vec!["parse draft-1 a", "parse draft-2 b", "busy true", "changed"]
        );
        assert_eq!(draft.urls(), vec!["a", "b"]);
        assert!(draft.is_parsing());
        assert_eq!(draft.items()[0].parse_id, "draft-1");
    }

    #[test]
    fn set_urls_dedupes_while_keeping_the_first_occurrence() {
        let mut draft = TaskDraft::new();
        let effects = draft.set_urls(&urls(&["a", "b", "a", "c"]));

        assert_eq!(draft.urls(), vec!["a", "b", "c"]);
        assert_eq!(
            trace(&effects),
            vec![
                "parse draft-1 a",
                "parse draft-2 b",
                "parse draft-3 c",
                "busy true",
                "changed"
            ]
        );
    }

    #[test]
    fn set_urls_keeps_equal_items_without_resubmitting() {
        let mut draft = draft_with(&["a", "b"]);
        draft.on_parsed("draft-1", record("a"));

        let effects = draft.set_urls(&urls(&["a", "b", "c"]));

        // `a` 是 equal 段 → 原样搬；`b` 也在 equal 段；只有 `c` 是新提交的
        assert_eq!(
            trace(&effects),
            vec!["parse draft-3 c", "busy true", "changed"]
        );
        assert!(draft.task_by_url("a").is_some());
        assert_eq!(draft.item_by_url("b").unwrap().parse_id, "draft-2");
    }

    #[test]
    fn set_urls_with_an_identical_list_only_emits() {
        let mut draft = draft_with(&["a", "b"]);

        let effects = draft.set_urls(&urls(&["a", "b"]));

        assert_eq!(trace(&effects), vec!["busy true", "changed"]);
        assert_eq!(draft.item_by_url("a").unwrap().parse_id, "draft-1");
        assert_eq!(draft.item_by_url("b").unwrap().parse_id, "draft-2");
    }

    #[test]
    fn set_urls_replaces_the_middle_span() {
        let mut draft = draft_with(&["a", "b", "c"]);

        let effects = draft.set_urls(&urls(&["a", "x", "c"]));

        assert_eq!(
            trace(&effects),
            vec!["cancel draft-2", "parse draft-4 x", "busy true", "changed"]
        );
        assert_eq!(draft.urls(), vec!["a", "x", "c"]);
        assert_eq!(draft.item_by_url("a").unwrap().parse_id, "draft-1");
        assert_eq!(draft.item_by_url("c").unwrap().parse_id, "draft-3");
    }

    #[test]
    fn set_urls_cancels_the_tail() {
        let mut draft = draft_with(&["a", "b", "c"]);

        let effects = draft.set_urls(&urls(&["a"]));

        assert_eq!(
            trace(&effects),
            vec!["cancel draft-2", "cancel draft-3", "busy true", "changed"]
        );
        assert_eq!(draft.urls(), vec!["a"]);
    }

    #[test]
    fn set_urls_does_not_cancel_items_that_already_finished() {
        let mut draft = draft_with(&["a", "b"]);
        draft.on_parsed("draft-2", record("b"));

        let effects = draft.set_urls(&urls(&["a"]));

        // `b` 已经解析完了，`parseId` 为空 → 没有 Cancel
        assert_eq!(trace(&effects), vec!["busy true", "changed"]);
    }

    #[test]
    fn set_urls_to_an_empty_list_cancels_everything() {
        let mut draft = draft_with(&["a", "b"]);

        let effects = draft.set_urls(&[]);

        assert_eq!(
            trace(&effects),
            vec!["cancel draft-1", "cancel draft-2", "busy false", "changed"]
        );
        assert!(draft.items().is_empty());
        assert!(!draft.is_parsing());
    }

    #[test]
    fn refresh_resubmits_only_the_failed_item() {
        let mut draft = draft_with(&["a", "b"]);
        draft.on_parse_failed("draft-1", TaskError::server_error(500));
        assert_eq!(draft.fail_count(), 1);

        let effects = draft.refresh("a");

        assert_eq!(
            trace(&effects),
            vec!["parse draft-3 a", "busy true", "changed"]
        );
        assert_eq!(draft.fail_count(), 0);
        assert_eq!(draft.item_by_url("b").unwrap().parse_id, "draft-2");
    }

    #[test]
    fn refresh_ignores_healthy_and_unknown_items() {
        let mut draft = draft_with(&["a"]);

        assert!(draft.refresh("a").is_empty());
        assert!(draft.refresh("zzz").is_empty());
    }

    #[test]
    fn add_parsed_tasks_appends_unknown_urls() {
        let mut draft = draft_with(&["a"]);

        let report = draft.add_parsed_tasks(vec![record("b")]);

        assert_eq!(report.new_urls, vec!["b"]);
        assert_eq!(
            trace(&report.effects),
            vec!["succeeded b", "busy true", "changed"]
        );
        assert_eq!(draft.urls(), vec!["a", "b"]);
        assert!(draft.task_by_url("b").is_some());
    }

    #[test]
    fn add_parsed_tasks_skips_urls_that_already_have_a_task() {
        let mut draft = draft_with(&["a"]);
        draft.on_parsed("draft-1", record("a"));

        let report = draft.add_parsed_tasks(vec![record("a")]);

        assert!(report.new_urls.is_empty());
        // 没有新增、也没有 parseSucceeded，但 `addParsedTasks` 照样发 busy + changed；
        // `a` 早已解析完（parseId 为空）→ busy 是 false
        assert_eq!(trace(&report.effects), vec!["busy false", "changed"]);
    }

    #[test]
    fn add_parsed_tasks_cancels_a_pending_parse_and_takes_over() {
        let mut draft = draft_with(&["a"]);

        let report = draft.add_parsed_tasks(vec![record("a")]);

        assert!(report.new_urls.is_empty());
        assert_eq!(
            trace(&report.effects),
            vec!["cancel draft-1", "succeeded a", "busy false", "changed"]
        );
        assert!(draft.task_by_url("a").is_some());
        assert!(!draft.is_parsing());
    }

    #[test]
    fn add_parsed_tasks_with_no_tasks_is_a_no_op() {
        let mut draft = draft_with(&["a"]);

        let report = draft.add_parsed_tasks(Vec::new());

        assert!(report.new_urls.is_empty());
        assert!(report.effects.is_empty());
        assert_eq!(draft.item_by_url("a").unwrap().parse_id, "draft-1");
    }

    #[test]
    fn on_parsed_stores_the_task_and_reports_success() {
        let mut draft = draft_with(&["a"]);

        let effects = draft.on_parsed("draft-1", record("a"));

        assert_eq!(
            trace(&effects),
            vec!["succeeded a", "busy false", "changed"]
        );
        assert!(!draft.is_parsing());
        assert_eq!(draft.task_by_url("a").unwrap().name, "a.bin");
    }

    #[test]
    fn on_parsed_applies_the_base_options() {
        let mut draft = TaskDraft::new();
        draft.set_base_options(options(&[("outputFolder", "/dl"), ("category", "video")]));
        draft.set_urls(&urls(&["a"]));

        let effects = draft.on_parsed("draft-1", record("a"));

        // Python 的 `_onParsed` 会先 `task.setOptions(self._buildOptions(item))`
        let DraftEffect::Succeeded { task, .. } = &effects[0] else {
            panic!("期望 Succeeded，实际 {:?}", trace(&effects));
        };
        assert_eq!(task.output_folder, "/dl");
        assert_eq!(task.category.as_deref(), Some("video"));
        assert_eq!(draft.task_by_url("a").unwrap().output_folder, "/dl");
    }

    #[test]
    fn on_parsed_of_a_cancelled_parse_is_ignored() {
        let mut draft = draft_with(&["a"]);
        draft.set_urls(&[]);

        // 取消时 parseId 已被清空 → 按 workId 反查不到，迟到的回调被丢掉
        assert!(draft.on_parsed("draft-1", record("a")).is_empty());
        assert!(draft.items().is_empty());
    }

    #[test]
    fn on_parse_failed_records_the_error() {
        let mut draft = draft_with(&["a"]);

        let effects = draft.on_parse_failed("draft-1", TaskError::server_error(500));

        assert_eq!(trace(&effects), vec!["failed a", "busy false", "changed"]);
        assert_eq!(draft.fail_count(), 1);
        assert_eq!(
            draft.item_by_url("a").unwrap().error,
            Some(TaskError::server_error(500))
        );
        assert!(
            draft
                .on_parse_failed("draft-9", TaskError::disk_full())
                .is_empty()
        );
    }

    #[test]
    fn confirm_hands_over_parsed_items_and_detaches_the_pending_ones() {
        let mut draft = draft_with(&["a", "b"]);
        draft.on_parsed("draft-1", record("a"));

        let effects = draft.confirm(true);

        assert_eq!(
            trace(&effects),
            vec!["confirmed a true", "busy false", "cleared"]
        );
        assert!(draft.items().is_empty());
        assert_eq!(draft.detached_items().len(), 1);
        assert_eq!(draft.detached_items()[0].url, "b");

        // 游离项的解析回来了 → 直接确认
        let effects = draft.on_parsed("draft-2", record("b"));

        assert_eq!(trace(&effects), vec!["confirmed b true"]);
        assert!(draft.detached_items().is_empty());
    }

    #[test]
    fn confirm_carries_the_auto_start_flag() {
        let mut draft = draft_with(&["a"]);
        draft.on_parsed("draft-1", record("a"));

        let effects = draft.confirm(false);

        assert_eq!(
            trace(&effects),
            vec!["confirmed a false", "busy false", "cleared"]
        );
    }

    #[test]
    fn confirm_freezes_the_options_of_a_pending_item() {
        let mut draft = TaskDraft::new();
        draft.set_base_options(options(&[("outputFolder", "/dl")]));
        draft.set_urls(&urls(&["a"]));

        draft.confirm(false);
        let detached = &draft.detached_items()[0];
        assert_eq!(
            detached.confirmed_options.as_ref().unwrap()["outputFolder"],
            Value::String("/dl".to_string())
        );
        assert!(!detached.confirmed_auto_start);

        let effects = draft.on_parsed("draft-1", record("a"));
        let DraftEffect::Confirmed { task, auto_start } = &effects[0] else {
            panic!("期望 Confirmed，实际 {:?}", trace(&effects));
        };
        assert_eq!(task.output_folder, "/dl");
        assert!(!auto_start);
    }

    #[test]
    fn on_parse_failed_of_a_detached_item_drops_it_silently() {
        let mut draft = draft_with(&["a"]);
        draft.confirm(true);
        assert_eq!(draft.detached_items().len(), 1);

        assert!(
            draft
                .on_parse_failed("draft-1", TaskError::server_error(500))
                .is_empty()
        );
        assert!(draft.detached_items().is_empty());
    }

    #[test]
    fn clear_cancels_every_pending_parse() {
        let mut draft = draft_with(&["a", "b"]);

        let effects = draft.clear();

        // `clear` 先 `itemsCleared` 再 `parsingBusyChanged`（与 `confirm` 相反）
        assert_eq!(
            trace(&effects),
            vec!["cancel draft-1", "cancel draft-2", "cleared", "busy false"]
        );
        assert!(draft.items().is_empty());
    }

    #[test]
    fn clear_leaves_detached_items_alone() {
        let mut draft = draft_with(&["a"]);
        draft.confirm(true);

        let effects = draft.clear();

        assert_eq!(trace(&effects), vec!["cleared", "busy false"]);
        assert_eq!(draft.detached_items().len(), 1);
    }

    #[test]
    fn set_base_options_rewrites_parsed_tasks() {
        let mut draft = draft_with(&["a"]);
        draft.on_parsed("draft-1", record("a"));

        let effects =
            draft.set_base_options(options(&[("outputFolder", "/dl"), ("category", "video")]));

        assert_eq!(trace(&effects), vec!["changed"]);
        let task = draft.task_by_url("a").unwrap();
        assert_eq!(task.output_folder, "/dl");
        assert_eq!(task.category.as_deref(), Some("video"));
    }

    #[test]
    fn parse_carries_the_base_options_and_the_url() {
        let mut draft = TaskDraft::new();
        draft.set_base_options(options(&[("outputFolder", "/dl")]));

        let effects = draft.set_urls(&urls(&["a"]));

        let DraftEffect::Parse { work_id, options } = &effects[0] else {
            panic!("期望 Parse，实际 {:?}", trace(&effects));
        };
        assert_eq!(work_id, "draft-1");
        assert_eq!(options["url"], Value::String("a".to_string()));
        assert_eq!(options["outputFolder"], Value::String("/dl".to_string()));
    }

    #[test]
    fn a_category_override_wins_over_the_base_options() {
        let mut draft = TaskDraft::new();
        draft.set_base_options(options(&[("category", "video")]));
        draft.set_urls(&urls(&["a"]));
        draft.set_url_category("a", Some("music"));

        draft.confirm(true);
        let effects = draft.on_parsed("draft-1", record("a"));

        let DraftEffect::Confirmed { task, .. } = &effects[0] else {
            panic!("期望 Confirmed，实际 {:?}", trace(&effects));
        };
        assert_eq!(task.category.as_deref(), Some("music"));
    }

    #[test]
    fn set_url_category_updates_the_parsed_task_too() {
        let mut draft = draft_with(&["a"]);
        draft.on_parsed("draft-1", record("a"));

        assert_eq!(
            trace(&draft.set_url_category("a", Some("music"))),
            vec!["changed"]
        );
        assert_eq!(
            draft.task_by_url("a").unwrap().category.as_deref(),
            Some("music")
        );

        draft.set_url_category("a", None);
        assert_eq!(draft.item_by_url("a").unwrap().category_override, None);
        assert_eq!(draft.task_by_url("a").unwrap().category, None);

        assert!(draft.set_url_category("zzz", Some("x")).is_empty());
    }

    #[test]
    fn update_mutates_only_parsed_items() {
        let mut draft = draft_with(&["a", "b"]);
        draft.on_parsed("draft-1", record("a"));

        let effects = draft.update("a", |task| task.name = "renamed".to_string());

        assert_eq!(trace(&effects), vec!["changed"]);
        assert_eq!(draft.task_by_url("a").unwrap().name, "renamed");
        // 还在解析的 `b` 与不存在的 url 都没有 task → 无效果
        assert!(
            draft
                .update("b", |task| task.name = "x".to_string())
                .is_empty()
        );
        assert!(
            draft
                .update("zzz", |task| task.name = "x".to_string())
                .is_empty()
        );
    }

    #[test]
    fn can_confirm_is_true_while_anything_is_parsing_or_parsed() {
        let mut draft = TaskDraft::new();
        assert!(!draft.can_confirm());

        draft.set_urls(&urls(&["a", "b"]));
        assert!(draft.can_confirm());
        assert_eq!(draft.fail_count(), 0);

        draft.on_parse_failed("draft-1", TaskError::server_error(500));
        draft.on_parsed("draft-2", record("b"));
        assert_eq!(draft.fail_count(), 1);
        assert!(draft.can_confirm());
    }

    #[test]
    fn can_confirm_is_false_when_every_item_failed() {
        let mut draft = draft_with(&["a"]);

        draft.on_parse_failed("draft-1", TaskError::server_error(500));

        assert!(!draft.can_confirm());
        assert_eq!(draft.fail_count(), 1);
    }

    #[test]
    fn work_ids_are_never_reused() {
        let mut draft = TaskDraft::new();
        draft.set_urls(&urls(&["a"]));
        draft.set_urls(&[]);

        draft.set_urls(&urls(&["b"]));

        assert_eq!(draft.item_by_url("b").unwrap().parse_id, "draft-2");
    }
}
