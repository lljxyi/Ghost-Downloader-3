//! 任务草稿对话框的视图模型（P2，第 125 步）。
//!
//! 镜像 `app/view/dialogs/task_draft.py`（349 行）的 `TaskDraftDialog` 外壳，
//! 数据层是第 63/100 步的 [`crate::task_draft::TaskDraft`]、[`crate::draft`]、
//! [`crate::draft_card`]、[`crate::card_groups`]、[`crate::option_cards`]。
//! 这一步只补「对话框把状态机翻译成属性」的那一层，`ui/task_draft.slint` 只留形状。
//!
//! ## 实证口径（2026-10-19 实读源码，`translate` = identity）
//!
//! - **宽度**：`self.widget.setFixedWidth(700)`（:96）。
//! - **防抖**：`self._parseTimer.setInterval(1000)`（:132）+ `urlEdit.textChanged`
//!   → `_parseTimer.start`（:134）→ 超时 `_onParseNeeded`（:276-278）：
//!   先 `setBaseOptions(optionGroup.options())` 再 `setUrls(self._urls())`。
//! - **`_urls()`**（:321-326）：按行 strip、丢空行 → [`crate::draft::parse_url_lines`]。
//! - **`_onParseSucceeded`**（:280-286）：建卡 → 连 `categoryPicked` / `editRequested`
//!   → `draftGroup.addCard(url, card)` → `_refreshStats()`。
//! - **`_onParseFailed`**（:296-305）：`InfoBar.error("链接解析失败", f"{displayUrl}\n
//!   {toLocalizedError(error)}")`，`displayUrl` 超 48 字符截成 45 + `...`。
//! - **`_refreshStats`**（:315-319）：`successCount` = 有 task 的项数、
//!   `totalSize` = `Σ fileSize`（只算 `> 0`）、`fail` = `draft.failCount()`，
//!   交给 `DraftCardGroup.updateStats`（`card_groups.py:86-106`）。
//! - **`done(code, autoStart)`**（:212-227）：Accepted → `confirm(autoStart)`，
//!   否则 `clear()`；随后清空 `urlEdit`、`optionGroup.reset()`、`draftGroup.clear()`、
//!   `updateStats(0, 0, 0)`。
//! - **`validate()`**（:229-232）：先 `_onParseNeeded()` 再 `canConfirm()`。
//! - **`_accept(autoStart)`**（:272-274）：`validate()` 为真才 `done(Accepted)`。
//! - **`_onBatchClicked`**（:332-338）/ **`_onImportClicked`**（:340-349）：
//!   子对话框 + 文件过滤器（`import_name_filters` + `file_uri`）。
//! - **`addUrls`**（:193-201）：去重追加 → `_parseTimer.stop()` → `_onParseNeeded()`
//!   （**立即解析**，不等防抖）。
//! - **`addParsedTasks`**（:204-210）：`_draft.addParsedTasks(tasks)` 返回的新 URL
//!   追加进 `urlEdit`，再 `_parseTimer.stop()`。
//! - **`showStandalone` / `showMask`**（:147-191）：`urlEdit` 为空时用剪贴板里
//!   能 `featureService.match` 的行填充。
//!
//! ## 分工
//!
//! | 层 | 负责 |
//! |---|---|
//! | [`Texts`] | 全部文案（`self.tr(...)` 的中文原文） |
//! | [`TaskDraftUi`] | 状态机 + URL 文本 + 两个设置卡 + 防抖状态 → [`UiEffect`] |
//! | [`DraftView`] | 一次性快照：属性值 + 行模型 + 统计标签 |
//! | [`DraftHost`] | 宿主钩子：解析、冲突探测、`canEdit`、子对话框、任务落库 |
//! | [`apply`] / [`install`] | 灌属性、装回调、1 秒防抖定时器 |
//! | `ui/task_draft.slint` | 遮罩 + 面板 + 编辑器 + 列表 + 两个设置 + 三个按钮 |
//!
//! ## 与 Qt 侧的已知差异
//!
//! - **没有模态事件循环**：`exec()` 换成 `taskDraftVisible` 覆盖层；关框由
//!   [`UiEffect::Closed`] 通知宿主。
//! - **剪贴板读取未迁移**：[`TaskDraftUi::open`] 接受调用方给的剪贴板文本，
//!   本仓目前只有「写剪贴板」（`extension_actions::copy_text`），没有读；
//!   `boot.rs` 的打开入口传空串，行为等价于「剪贴板为空」。
//! - **`featureService.draftCard(task)` 的图标层不迁移**：`QFileIconProvider` 是
//!   平台位图，[`DraftRow`] 只给 `editable` / `conflict` / `category` 三个语义位。
//! - **`PrimarySplitPushButton` 拆成两个按钮**：「下载」/「稍后下载」分别对应
//!   `clicked` 与 `dropDownClicked`（:143-144）。
//! - **子对话框（批量 URL / 导入文件 / 编辑任务 / 选目录）只发效果**：这几层
//!   （`batch_url.py` / `file_select.py` / `edit_task.py`）的 UI 还没迁，
//!   [`DraftHost`] 的默认实现是空操作。
//! - **解析是同步的**：`coroutineRunner.submit` 换成 [`DraftHost::parse`] 的直接调用，
//!   宿主在自己的线程/事件循环里跑（`ParserChain::parse` 本身是同步函数）。

use crate::card_groups::{self, DraftStats};
use crate::category::CategoryRule;
use crate::draft::{
    append_unique_urls, clipboard_urls, error_url_display, import_name_filters, parse_url_lines,
};
use crate::draft_card::{self, size_label_text};
use crate::option_cards::{OptionCard, OptionValue, OutputFolderCard, SubworkerCountCard};
use crate::task_draft::{DraftEffect, TaskDraft};
use gd3_core::task::TaskRecord;
use gd3_core::task_error::TaskError;
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use slint::{ComponentHandle as _, ModelRc, VecModel};

use crate::{AppWindow, DraftRowData};

/// `self.widget.setFixedWidth(700)`（`task_draft.py:96`）。
pub const DIALOG_WIDTH: i32 = 700;
/// `self._parseTimer.setInterval(1000)`（:132）。
pub const PARSE_DELAY_MS: u64 = 1000;
/// `DraftCardGroup.MIN_VISIBLE = 5`（`card_groups.py:24`）。
pub const MIN_VISIBLE: usize = 5;
/// `DraftCard` 固定高 35（`draft_cards.py:53`）。
pub const CARD_HEIGHT: i32 = 35;
/// `_onParseFailed` 的截断阈值（:299）：`len(url) <= 48` 原样，否则前 45 + `...`。
pub const ERROR_URL_MAX_CHARS: usize = 48;

/// 全部文案（`self.tr(...)` 的中文原文，无译文层）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Texts {
    /// `setWindowTitle("添加任务")`（`task_draft.py:88`）。
    pub title: String,
    /// `添加多个下载链接时，请确保每行只有一个下载链接`（:97）。
    pub placeholder: String,
    /// `批量添加`（:113 附近的 `batchButton`）。
    pub batch: String,
    /// `导入文件`（:113 附近的 `importButton`）。
    pub import: String,
    /// `解析结果`（`card_groups.py:80`）。
    pub draft_title: String,
    /// `下载设置`（`card_groups.py` 的 `OptionCardGroup` 标题）。
    pub options_title: String,
    /// `选择下载路径`（`option_cards.py:99`）。
    pub folder_title: String,
    /// 浏览按钮文案 —— Qt 侧 `FolderPicker` 是图标按钮，这里给个词。
    pub folder_browse: String,
    /// `预分配线程数`（`option_cards.py:141`）。
    pub subworker_title: String,
    /// `下载`（:105）。
    pub download: String,
    /// `稍后下载`（:107，dropButton 的 tooltip）。
    pub later: String,
    /// `取消`（:117）。
    pub cancel: String,
    /// `QFileDialog.getOpenFileNames(..., self.tr("导入文件"), ...)`（:346）。
    pub import_dialog_title: String,
    /// `所有可导入文件 ({0})` 里的前半句（:342）。
    pub import_all_label: String,
    /// `InfoBar.error(self.tr("链接解析失败"), ...)`（:301）。
    pub parse_failed_title: String,
}

impl Default for Texts {
    fn default() -> Self {
        Self {
            title: "添加任务".to_string(),
            placeholder: "添加多个下载链接时，请确保每行只有一个下载链接".to_string(),
            batch: "批量添加".to_string(),
            import: "导入文件".to_string(),
            draft_title: "解析结果".to_string(),
            options_title: "下载设置".to_string(),
            folder_title: "选择下载路径".to_string(),
            folder_browse: "浏览".to_string(),
            subworker_title: "预分配线程数".to_string(),
            download: "下载".to_string(),
            later: "稍后下载".to_string(),
            cancel: "取消".to_string(),
            import_dialog_title: "导入文件".to_string(),
            import_all_label: "所有可导入文件".to_string(),
            parse_failed_title: "链接解析失败".to_string(),
        }
    }
}

/// 一行解析结果（`DraftCard` 的可见字段）。`url` 是行级回调的键。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DraftRow {
    /// `_cardByUrl` 的键，也是 `editRequested` / `categoryPicked` 的参数。
    pub url: String,
    /// `nameLabel` 的文本（`task.name`）。
    pub name: String,
    /// `sizeLabel`：`toReadableSize(fileSize) if fileSize > 1 else "--"`。
    pub size: String,
    /// 分类按钮的 tooltip（`toCategoryName(category)` 或 `未分类`）。
    pub category: String,
    /// `cfg.isCategoryEnabled` 为假 → 按钮 `hide()`（`draft_cards.py:117`）。
    pub category_visible: bool,
    /// `_probeConflict(task) is not None`。
    pub conflict: bool,
    /// `task.canEdit`（Python 是 `ClassVar`，由宿主注入）。
    pub editable: bool,
}

/// 一次属性快照：`apply` 把它整块灌进 `AppWindow`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftView {
    pub open: bool,
    pub title: String,
    pub placeholder: String,
    pub batch_text: String,
    pub import_text: String,
    /// `importButton.setVisible(bool(self._fileTypes))`（:101）。
    pub import_visible: bool,
    pub url_text: String,
    /// `parsingBusyChanged` → `progressBar.setVisible`（:136）。
    pub parsing: bool,
    pub draft_title: String,
    pub rows: Vec<DraftRow>,
    pub ratio_text: String,
    pub ratio_visible: bool,
    pub size_text: String,
    pub size_visible: bool,
    pub options_title: String,
    pub folder_title: String,
    pub folder_browse_text: String,
    pub folder_text: String,
    pub subworker_title: String,
    pub subworker_value: i32,
    pub subworker_min: i32,
    pub subworker_max: i32,
    pub download_text: String,
    pub later_text: String,
    pub cancel_text: String,
    /// `_draft.canConfirm()`：主按钮与「稍后下载」是否可点。
    pub can_confirm: bool,
}

/// 对话框对外的效果（状态机效果 + 宿主侧动作）。
///
/// 不实现 `PartialEq`：[`DraftEffect`] 本身没有（`TaskRecord` 不带 `PartialEq`），
/// 测试用 `matches!` 断言语义。
#[derive(Clone, Debug)]
pub enum UiEffect {
    /// 状态机自己的信号（`Cancel` / `Busy` / `Succeeded` / `Changed` / `Cleared` /
    /// `Confirmed`）。`Parse` 已被 [`TaskDraftUi`] 就地执行，不会出现在这里。
    ///
    /// 装箱：`DraftEffect` 含 `TaskRecord`，直接内联会让整个枚举 416 字节。
    Draft(Box<DraftEffect>),
    /// `InfoBar.error("链接解析失败", ...)`（:301-305）。
    ParseError { title: String, body: String },
    /// `_onEditRequested(url)`（:287-294）。
    EditRequested { url: String },
    /// 分类按钮的右键菜单（`draft_cards.py:125-137`）：`entries` 已含
    /// 「未分类」+ 分隔线 + 各分类项。
    CategoryMenuRequested {
        url: String,
        entries: Vec<crate::draft_card::DraftMenuEntry>,
    },
    /// `_onBatchClicked`（:332）。
    BatchRequested,
    /// `_onImportClicked`（:340）：已算好的过滤器（合并项在前）。
    ImportRequested { title: String, filters: Vec<String> },
    /// `FolderPicker` 的浏览按钮。
    FolderPickRequested,
    /// 对话框关闭（Accepted 或 Rejected）。
    Closed,
}

/// 宿主钩子。全部有默认实现，宿主只覆盖它能做的部分。
pub trait DraftHost {
    /// `task.canEdit`（Python `ClassVar`，`task.py:187-190`）。
    fn can_edit(&self, _task: &TaskRecord) -> bool {
        false
    }

    /// `featureService.probeConflict(task)`。
    fn has_conflict(&self, _task: &TaskRecord) -> bool {
        false
    }

    /// 执行一次解析（`featureService.parse(options)`）。`options` 含 `"url"` 键。
    fn parse(&self, _options: &Map<String, Value>) -> Result<TaskRecord, TaskError> {
        Err(TaskError::new("解析器未接入", Vec::new()))
    }

    /// 解析失败的提示（Qt 侧是 `InfoBar.error`）。
    fn notify_parse_failure(&self, title: &str, body: &str) {
        eprintln!("{title}：{body}");
    }

    /// `DraftEditDialog(task, ...).exec()`（:292）。
    fn edit_task(&self, _url: &str) {}

    /// `BatchUrlDialog(parent).exec()`（:335）。
    fn batch_url(&self) {}

    /// 弹分类菜单（`draft_cards.py:136` 的 `menu.exec(...)`）。
    fn category_menu(&self, _url: &str, _entries: &[crate::draft_card::DraftMenuEntry]) {}

    /// `QFileDialog.getOpenFileNames(...)`（:346）。
    fn import_files(&self, _title: &str, _filters: &[String]) {}

    /// `FolderPicker` 的目录选择框。
    fn pick_folder(&self) {}

    /// `taskConfirmed`：把任务交给 `TaskService`。
    fn confirmed(&self, _task: &TaskRecord, _auto_start: bool) {}

    /// 对话框已关闭。
    fn closed(&self) {}
}

/// 什么都不做的宿主（测试与「还没接线」的默认形态）。
#[derive(Clone, Copy, Debug, Default)]
pub struct PlainHost;

impl DraftHost for PlainHost {}

/// `OptionValue` → 任务草稿的 options 字典值。
///
/// 口径同 Python `optionGroup.options()`：`outputFolder` 是路径原串、
/// `subworkerCount` 是整数、`headers` 是 lower-case 键的对象。
pub fn option_value_to_json(value: &OptionValue) -> Value {
    match value {
        OptionValue::Text(text) | OptionValue::Path(text) => Value::String(text.clone()),
        OptionValue::Number(number) => Value::from(*number),
        OptionValue::Headers(pairs) => Value::Object(
            pairs
                .iter()
                .map(|(name, value)| (name.clone(), Value::String(value.clone())))
                .collect(),
        ),
    }
}

/// 任务草稿对话框的运行时状态（`TaskDraftDialog` 的成员变量）。
pub struct TaskDraftUi {
    texts: Texts,
    draft: TaskDraft,
    /// `urlEdit.toPlainText()`。
    url_text: String,
    /// `OutputFolderCard`。
    folder: OutputFolderCard,
    /// `SubworkerCountCard`。
    subworker: SubworkerCountCard,
    /// `categoryService.categories()`。
    categories: Vec<CategoryRule>,
    /// `cfg.isCategoryEnabled`。
    category_enabled: bool,
    /// `featureService.fileTypes()`：`(displayName, [extensions])`。
    file_types: Vec<(String, Vec<String>)>,
    open: bool,
}

impl TaskDraftUi {
    /// `TaskDraftDialog(draft, featureService, categoryService)`。
    ///
    /// `default_folder` = `cfg.downloadFolder.value`、`folder_history` =
    /// `cfg.get(cfg.downloadFolder).history`、`subworker_default` =
    /// `cfg.preBlockNum.value`（缺省 8）。
    pub fn new(
        texts: Texts,
        default_folder: &str,
        folder_history: &[String],
        subworker_default: i32,
    ) -> Self {
        Self {
            texts,
            draft: TaskDraft::new(),
            url_text: String::new(),
            folder: OutputFolderCard::new(None, default_folder, folder_history),
            subworker: SubworkerCountCard::new(0, subworker_default),
            categories: Vec::new(),
            category_enabled: false,
            file_types: Vec::new(),
            open: false,
        }
    }

    /// `categoryService.categories()` + `cfg.isCategoryEnabled`。
    pub fn set_categories(&mut self, categories: Vec<CategoryRule>, enabled: bool) {
        self.categories = categories;
        self.category_enabled = enabled;
    }

    /// `featureService.fileTypes()`；为空时「导入文件」按钮不显示（:101）。
    pub fn set_file_types(&mut self, file_types: Vec<(String, Vec<String>)>) {
        self.file_types = file_types;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn texts(&self) -> &Texts {
        &self.texts
    }

    pub fn draft(&self) -> &TaskDraft {
        &self.draft
    }

    pub fn draft_mut(&mut self) -> &mut TaskDraft {
        &mut self.draft
    }

    pub fn url_text(&self) -> &str {
        &self.url_text
    }

    pub fn folder_text(&self) -> &str {
        self.folder.text()
    }

    pub fn subworker_value(&self) -> i32 {
        self.subworker.value()
    }

    /// `self._urls()`（:321-326）。
    pub fn urls(&self) -> Vec<String> {
        parse_url_lines(&self.url_text)
    }

    /// `optionGroup.options()`：两张卡按加入顺序拼成 dict。
    pub fn options(&self) -> Map<String, Value> {
        let cards: [&dyn OptionCard; 2] = [&self.folder, &self.subworker];
        let mut options = Map::new();
        for card in cards {
            for (key, value) in card.options() {
                options.insert(key.to_string(), option_value_to_json(&value));
            }
        }
        options
    }

    /// `_refreshStats()`（:315-319）。
    pub fn stats(&self) -> DraftStats {
        let urls = self.draft.urls();
        let tasks: Vec<Option<&TaskRecord>> =
            urls.iter().map(|url| self.draft.task_by_url(url)).collect();
        let success = tasks.iter().filter(|task| task.is_some()).count();
        let total_size: i64 = tasks
            .iter()
            .filter_map(|task| *task)
            .filter(|task| task.file_size > 0)
            .map(|task| task.file_size)
            .sum();
        card_groups::update_stats(success, self.draft.fail_count(), total_size)
    }

    /// `showStandalone()` / `showMask()` 的前半段（:147-191）：`urlEdit` 为空时用
    /// 剪贴板里 `matcher` 认的行填充。返回「URL 文本变了，需要重新起防抖」。
    pub fn open(&mut self, clipboard: &str, matcher: impl Fn(&str) -> bool) -> bool {
        let mut armed = false;
        if self.url_text.trim().is_empty() {
            let urls = clipboard_urls(clipboard, matcher);
            if !urls.is_empty() {
                self.url_text = urls.join("\n");
                armed = true;
            }
        }
        self.open = true;
        armed
    }

    /// 覆盖层可见性（`close` 只用于宿主强制收起，不算一次 `done`）。
    pub fn set_open(&mut self, open: bool) {
        self.open = open;
    }

    /// `urlEdit.textChanged`：只记文本，防抖由宿主定时器负责（:134）。
    pub fn text_changed(&mut self, text: &str) {
        self.url_text = text.to_string();
    }

    /// `FolderPicker` 的文本编辑（`path()` 在 `parse_needed` 时提交）。
    pub fn folder_edited(&mut self, text: &str) {
        self.folder.set_text(text);
    }

    /// `slider.valueChanged`。
    pub fn subworker_edited(&mut self, value: i32) {
        self.subworker.set_value(value);
    }

    /// `_onParseNeeded()`（:276-278）。
    pub fn parse_needed(&mut self, host: &dyn DraftHost) -> Vec<UiEffect> {
        // `FolderPicker` 的 `editingFinished` 在这里落地：读 options 之前提交一次。
        self.folder.commit();
        let options = self.options();
        let mut effects = self.draft.set_base_options(options);
        let urls = self.urls();
        effects.extend(self.draft.set_urls(&urls));
        self.run(effects, host)
    }

    /// `addUrls(urls)`（:193-201）：去重追加 → 立即解析（不等防抖）。
    pub fn add_urls(&mut self, urls: &[String], host: &dyn DraftHost) -> Vec<UiEffect> {
        if urls.is_empty() {
            return Vec::new();
        }
        let to_add = append_unique_urls(&self.urls(), urls);
        if to_add.is_empty() {
            return Vec::new();
        }
        if !self.url_text.is_empty() {
            self.url_text.push('\n');
        }
        self.url_text.push_str(&to_add.join("\n"));
        self.parse_needed(host)
    }

    /// `addParsedTasks(tasks)`（:204-210）：新 URL 追加进 `urlEdit`，**不**重新解析。
    pub fn add_parsed_tasks(
        &mut self,
        tasks: Vec<TaskRecord>,
        host: &dyn DraftHost,
    ) -> Vec<UiEffect> {
        if tasks.is_empty() {
            return Vec::new();
        }
        let report = self.draft.add_parsed_tasks(tasks);
        if !report.new_urls.is_empty() {
            if !self.url_text.is_empty() {
                self.url_text.push('\n');
            }
            self.url_text.push_str(&report.new_urls.join("\n"));
        }
        self.run(report.effects, host)
    }

    /// `validate()`（:229-232）：先解析再问 `canConfirm()`。
    pub fn validate(&mut self, host: &dyn DraftHost) -> (bool, Vec<UiEffect>) {
        let effects = self.parse_needed(host);
        (self.draft.can_confirm(), effects)
    }

    /// `_accept(autoStart)`（:272-274）。
    pub fn accept(&mut self, auto_start: bool, host: &dyn DraftHost) -> Vec<UiEffect> {
        let (ok, mut effects) = self.validate(host);
        if !ok {
            return effects;
        }
        effects.extend(self.done(true, auto_start, host));
        effects
    }

    /// `done(Rejected)`（:212-227 的 else 分支）：取消按钮。
    pub fn cancel(&mut self, host: &dyn DraftHost) -> Vec<UiEffect> {
        self.done(false, true, host)
    }

    /// `_onEditRequested(url)`（:287-294）：没有 task 时直接返回。
    pub fn edit_requested(&mut self, url: &str) -> Vec<UiEffect> {
        if self.draft.task_by_url(url).is_none() {
            return Vec::new();
        }
        vec![UiEffect::EditRequested {
            url: url.to_string(),
        }]
    }

    /// `categoryButton.clicked` → 弹分类菜单（`draft_cards.py:125-137`）。
    pub fn category_menu_requested(&self, url: &str) -> Vec<UiEffect> {
        if !self.category_enabled {
            return Vec::new();
        }
        vec![UiEffect::CategoryMenuRequested {
            url: url.to_string(),
            entries: draft_card::category_menu(&self.categories),
        }]
    }

    /// 菜单选中后的回写（`_pickCategory`）：`""` = 未分类。
    pub fn pick_category(
        &mut self,
        url: &str,
        category_id: &str,
        host: &dyn DraftHost,
    ) -> Vec<UiEffect> {
        let category = (!category_id.is_empty()).then_some(category_id);
        let effects = self.draft.set_url_category(url, category);
        self.run(effects, host)
    }

    /// `_onBatchClicked`（:332-338）。
    pub fn batch_requested(&mut self) -> Vec<UiEffect> {
        vec![UiEffect::BatchRequested]
    }

    /// `_onImportClicked`（:340-349）：`bool(self._fileTypes)` 为假时按钮不显示。
    pub fn import_requested(&mut self) -> Vec<UiEffect> {
        if self.file_types.is_empty() {
            return Vec::new();
        }
        let (merged, per_type) =
            import_name_filters(&self.file_types, &self.texts.import_all_label);
        let mut filters = vec![merged];
        filters.extend(per_type);
        vec![UiEffect::ImportRequested {
            title: self.texts.import_dialog_title.clone(),
            filters,
        }]
    }

    /// `FolderPicker` 的浏览按钮。
    pub fn folder_pick_requested(&mut self) -> Vec<UiEffect> {
        vec![UiEffect::FolderPickRequested]
    }

    /// 属性快照。
    pub fn view(&self, host: &dyn DraftHost) -> DraftView {
        let stats = self.stats();
        let rows: Vec<DraftRow> = self
            .draft
            .items()
            .iter()
            .filter_map(|item| {
                let task = item.task.as_ref()?;
                let category = draft_card::category_button(
                    self.category_enabled,
                    &self.categories,
                    task.category.as_deref().unwrap_or_default(),
                );
                Some(DraftRow {
                    url: item.url.clone(),
                    name: task.name.clone(),
                    size: size_label_text(task.file_size),
                    category: category
                        .as_ref()
                        .map(|button| button.tooltip.clone())
                        .unwrap_or_default(),
                    category_visible: category.is_some(),
                    conflict: host.has_conflict(task),
                    editable: host.can_edit(task),
                })
            })
            .collect();
        DraftView {
            open: self.open,
            title: self.texts.title.clone(),
            placeholder: self.texts.placeholder.clone(),
            batch_text: self.texts.batch.clone(),
            import_text: self.texts.import.clone(),
            import_visible: !self.file_types.is_empty(),
            url_text: self.url_text.clone(),
            parsing: self.draft.is_parsing(),
            draft_title: self.texts.draft_title.clone(),
            rows,
            ratio_text: stats.ratio_text,
            ratio_visible: stats.ratio_visible,
            size_text: stats.size_text,
            size_visible: stats.size_visible,
            options_title: self.texts.options_title.clone(),
            folder_title: self.texts.folder_title.clone(),
            folder_browse_text: self.texts.folder_browse.clone(),
            folder_text: self.folder.text().to_string(),
            subworker_title: self.texts.subworker_title.clone(),
            subworker_value: self.subworker.value(),
            subworker_min: crate::option_cards::PRE_BLOCK_MIN,
            subworker_max: crate::option_cards::PRE_BLOCK_MAX,
            download_text: self.texts.download.clone(),
            later_text: self.texts.later.clone(),
            cancel_text: self.texts.cancel.clone(),
            can_confirm: self.draft.can_confirm(),
        }
    }

    /// `done(code, autoStart)`（:212-227）。
    fn done(&mut self, accepted: bool, auto_start: bool, host: &dyn DraftHost) -> Vec<UiEffect> {
        let draft_effects = if accepted {
            self.draft.confirm(auto_start)
        } else {
            self.draft.clear()
        };
        let mut effects = self.run(draft_effects, host);
        self.url_text.clear();
        self.folder.reset();
        self.subworker.reset();
        self.open = false;
        effects.push(UiEffect::Closed);
        effects
    }

    /// 把状态机效果翻译成 [`UiEffect`]：`Parse` 就地执行并递归消费回调效果，
    /// `Failed` 变成一条 `InfoBar` 文案。
    fn run(&mut self, effects: Vec<DraftEffect>, host: &dyn DraftHost) -> Vec<UiEffect> {
        let mut out = Vec::new();
        for effect in effects {
            match effect {
                DraftEffect::Parse { work_id, options } => {
                    let more = match host.parse(&options) {
                        Ok(task) => self.draft.on_parsed(&work_id, task),
                        Err(error) => self.draft.on_parse_failed(&work_id, error),
                    };
                    out.extend(self.run(more, host));
                }
                DraftEffect::Failed { url, error } => out.push(UiEffect::ParseError {
                    title: self.texts.parse_failed_title.clone(),
                    body: format!("{}\n{}", error_url_display(&url), error.format()),
                }),
                other => out.push(UiEffect::Draft(Box::new(other))),
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// 窗口接线
// ---------------------------------------------------------------------------

/// 把 [`DraftView`] 整块灌进 `AppWindow`。
pub fn apply(window: &AppWindow, view: &DraftView) {
    window.set_taskDraftVisible(view.open);
    window.set_taskDraftTitle(view.title.as_str().into());
    window.set_taskDraftPlaceholder(view.placeholder.as_str().into());
    window.set_taskDraftBatchText(view.batch_text.as_str().into());
    window.set_taskDraftImportText(view.import_text.as_str().into());
    window.set_taskDraftImportVisible(view.import_visible);
    window.set_taskDraftUrlText(view.url_text.as_str().into());
    window.set_taskDraftParsing(view.parsing);
    window.set_taskDraftDraftTitle(view.draft_title.as_str().into());
    let rows: Vec<DraftRowData> = view.rows.iter().map(to_slint_row).collect();
    window.set_taskDraftRows(ModelRc::new(VecModel::from(rows)));
    window.set_taskDraftRatioText(view.ratio_text.as_str().into());
    window.set_taskDraftRatioVisible(view.ratio_visible);
    window.set_taskDraftSizeText(view.size_text.as_str().into());
    window.set_taskDraftSizeVisible(view.size_visible);
    window.set_taskDraftOptionsTitle(view.options_title.as_str().into());
    window.set_taskDraftFolderTitle(view.folder_title.as_str().into());
    window.set_taskDraftFolderBrowseText(view.folder_browse_text.as_str().into());
    window.set_taskDraftFolderText(view.folder_text.as_str().into());
    window.set_taskDraftSubworkerTitle(view.subworker_title.as_str().into());
    window.set_taskDraftSubworkerValue(view.subworker_value as f32);
    window.set_taskDraftSubworkerMin(view.subworker_min);
    window.set_taskDraftSubworkerMax(view.subworker_max);
    window.set_taskDraftDownloadText(view.download_text.as_str().into());
    window.set_taskDraftLaterText(view.later_text.as_str().into());
    window.set_taskDraftCancelText(view.cancel_text.as_str().into());
    window.set_taskDraftCanConfirm(view.can_confirm);
}

fn to_slint_row(row: &DraftRow) -> DraftRowData {
    DraftRowData {
        url: row.url.as_str().into(),
        name: row.name.as_str().into(),
        size: row.size.as_str().into(),
        category: row.category.as_str().into(),
        category_visible: row.category_visible,
        conflict: row.conflict,
        editable: row.editable,
    }
}

/// 重新算一次快照并灌进窗口。
pub fn refresh(window: &AppWindow, ui: &Rc<RefCell<TaskDraftUi>>, host: &dyn DraftHost) {
    let view = ui.borrow().view(host);
    apply(window, &view);
}

/// 把效果分派给宿主，然后刷新快照。
fn handle(
    window: &AppWindow,
    ui: &Rc<RefCell<TaskDraftUi>>,
    host: &dyn DraftHost,
    effects: Vec<UiEffect>,
) {
    for effect in effects {
        match effect {
            UiEffect::ParseError { title, body } => host.notify_parse_failure(&title, &body),
            UiEffect::EditRequested { url } => host.edit_task(&url),
            UiEffect::CategoryMenuRequested { url, entries } => {
                host.category_menu(&url, &entries);
            }
            UiEffect::BatchRequested => host.batch_url(),
            UiEffect::ImportRequested { title, filters } => host.import_files(&title, &filters),
            UiEffect::FolderPickRequested => host.pick_folder(),
            UiEffect::Draft(draft) => {
                if let DraftEffect::Confirmed { task, auto_start } = *draft {
                    host.confirmed(&task, auto_start);
                }
            }
            UiEffect::Closed => host.closed(),
        }
    }
    refresh(window, ui, host);
}

/// 装全部回调，返回「打开对话框」的入口（`showStandalone` / `showMask` 的可见性部分）。
///
/// 防抖定时器在这里：`urlEdit.textChanged` → `_parseTimer.start`（1000ms 单次）→
/// `_onParseNeeded`。Slint 的 `TextEdit.edited` 是逐键触发，所以每次编辑都重启定时器。
pub fn install(
    window: &AppWindow,
    ui: Rc<RefCell<TaskDraftUi>>,
    host: Rc<dyn DraftHost>,
) -> Rc<dyn Fn()> {
    let timer = Rc::new(slint::Timer::default());

    let fire: Rc<dyn Fn()> = {
        let ui = Rc::clone(&ui);
        let host = Rc::clone(&host);
        let weak = window.as_weak();
        Rc::new(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let effects = ui.borrow_mut().parse_needed(host.as_ref());
            handle(&window, &ui, host.as_ref(), effects);
        })
    };

    {
        let ui = Rc::clone(&ui);
        let timer = Rc::clone(&timer);
        let fire = Rc::clone(&fire);
        window.on_taskDraftUrlEdited(move |text| {
            ui.borrow_mut().text_changed(text.as_str());
            let fire = Rc::clone(&fire);
            timer.start(
                slint::TimerMode::SingleShot,
                Duration::from_millis(PARSE_DELAY_MS),
                move || fire(),
            );
        });
    }

    {
        let ui = Rc::clone(&ui);
        let host = Rc::clone(&host);
        let weak = window.as_weak();
        window.on_taskDraftBatchClicked(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let effects = ui.borrow_mut().batch_requested();
            handle(&window, &ui, host.as_ref(), effects);
        });
    }

    {
        let ui = Rc::clone(&ui);
        let host = Rc::clone(&host);
        let weak = window.as_weak();
        window.on_taskDraftImportClicked(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let effects = ui.borrow_mut().import_requested();
            handle(&window, &ui, host.as_ref(), effects);
        });
    }

    {
        let ui = Rc::clone(&ui);
        window.on_taskDraftFolderEdited(move |text| {
            // 不刷新：`folderText` 是 `in-out`，回灌会把光标弹回开头。
            ui.borrow_mut().folder_edited(text.as_str());
        });
    }

    {
        let ui = Rc::clone(&ui);
        let host = Rc::clone(&host);
        let weak = window.as_weak();
        window.on_taskDraftFolderPickRequested(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let effects = ui.borrow_mut().folder_pick_requested();
            handle(&window, &ui, host.as_ref(), effects);
        });
    }

    {
        let ui = Rc::clone(&ui);
        let host = Rc::clone(&host);
        let weak = window.as_weak();
        window.on_taskDraftSubworkerEdited(move |value| {
            let Some(window) = weak.upgrade() else {
                return;
            };
            ui.borrow_mut().subworker_edited(value);
            refresh(&window, &ui, host.as_ref());
        });
    }

    {
        let ui = Rc::clone(&ui);
        let host = Rc::clone(&host);
        let weak = window.as_weak();
        window.on_taskDraftDownloadClicked(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let effects = ui.borrow_mut().accept(true, host.as_ref());
            handle(&window, &ui, host.as_ref(), effects);
        });
    }

    {
        let ui = Rc::clone(&ui);
        let host = Rc::clone(&host);
        let weak = window.as_weak();
        window.on_taskDraftLaterClicked(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let effects = ui.borrow_mut().accept(false, host.as_ref());
            handle(&window, &ui, host.as_ref(), effects);
        });
    }

    {
        let ui = Rc::clone(&ui);
        let host = Rc::clone(&host);
        let weak = window.as_weak();
        window.on_taskDraftCancelClicked(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let effects = ui.borrow_mut().cancel(host.as_ref());
            handle(&window, &ui, host.as_ref(), effects);
        });
    }

    {
        let ui = Rc::clone(&ui);
        let host = Rc::clone(&host);
        let weak = window.as_weak();
        window.on_taskDraftRowEditRequested(move |url| {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let effects = ui.borrow_mut().edit_requested(url.as_str());
            handle(&window, &ui, host.as_ref(), effects);
        });
    }

    {
        let ui = Rc::clone(&ui);
        let host = Rc::clone(&host);
        let weak = window.as_weak();
        window.on_taskDraftRowCategoryRequested(move |url| {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let effects = ui.borrow().category_menu_requested(url.as_str());
            handle(&window, &ui, host.as_ref(), effects);
        });
    }

    let opener: Rc<dyn Fn()> = {
        let ui = Rc::clone(&ui);
        let host = Rc::clone(&host);
        let weak = window.as_weak();
        Rc::new(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            // 剪贴板读取未迁移（见模块头「已知差异」），打开时不预填。
            ui.borrow_mut().set_open(true);
            refresh(&window, &ui, &*host);
        })
    };

    refresh(window, &ui, &*host);
    opener
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use slint::Model as _;

    use super::*;
    use gd3_core::task::TaskStatus;

    /// 记录宿主侧动作的假实现。
    struct FakeHost {
        tasks: RefCell<HashMap<String, TaskRecord>>,
        error: RefCell<Option<TaskError>>,
        log: RefCell<Vec<String>>,
        editable: bool,
        conflict: bool,
    }

    impl FakeHost {
        fn new() -> Self {
            Self {
                tasks: RefCell::new(HashMap::new()),
                error: RefCell::new(None),
                log: RefCell::new(Vec::new()),
                editable: false,
                conflict: false,
            }
        }

        fn with_task(url: &str, size: i64) -> Self {
            let host = Self::new();
            host.tasks
                .borrow_mut()
                .insert(url.to_string(), record(url, size));
            host
        }

        fn failing(error: TaskError) -> Self {
            let host = Self::new();
            *host.error.borrow_mut() = Some(error);
            host
        }

        fn log(&self) -> Vec<String> {
            self.log.borrow().clone()
        }
    }

    impl DraftHost for FakeHost {
        fn can_edit(&self, _task: &TaskRecord) -> bool {
            self.editable
        }

        fn has_conflict(&self, _task: &TaskRecord) -> bool {
            self.conflict
        }

        fn parse(&self, options: &Map<String, Value>) -> Result<TaskRecord, TaskError> {
            if let Some(error) = self.error.borrow().clone() {
                return Err(error);
            }
            let url = options
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            self.tasks
                .borrow()
                .get(&url)
                .cloned()
                .ok_or_else(|| TaskError::new("未知链接", Vec::new()))
        }

        fn notify_parse_failure(&self, title: &str, body: &str) {
            self.log.borrow_mut().push(format!("error:{title}:{body}"));
        }

        fn edit_task(&self, url: &str) {
            self.log.borrow_mut().push(format!("edit:{url}"));
        }

        fn category_menu(&self, url: &str, entries: &[crate::draft_card::DraftMenuEntry]) {
            self.log
                .borrow_mut()
                .push(format!("menu:{url}:{}", entries.len()));
        }

        fn batch_url(&self) {
            self.log.borrow_mut().push("batch".to_string());
        }

        fn import_files(&self, title: &str, filters: &[String]) {
            self.log
                .borrow_mut()
                .push(format!("import:{title}:{}", filters.join("|")));
        }

        fn pick_folder(&self) {
            self.log.borrow_mut().push("pick".to_string());
        }

        fn confirmed(&self, task: &TaskRecord, auto_start: bool) {
            self.log
                .borrow_mut()
                .push(format!("confirm:{}:{auto_start}", task.name));
        }

        fn closed(&self) {
            self.log.borrow_mut().push("closed".to_string());
        }
    }

    /// 把效果里所有 `Confirmed` 的 `auto_start` 标记按顺序抽出来。
    fn confirmed_flags(effects: &[UiEffect]) -> Vec<bool> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                UiEffect::Draft(draft) => match **draft {
                    DraftEffect::Confirmed { auto_start, .. } => Some(auto_start),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    fn record(url: &str, size: i64) -> TaskRecord {
        TaskRecord {
            name: format!("{}.bin", url.rsplit('/').next().unwrap_or("x")),
            url: url.to_string(),
            pack_id: String::new(),
            task_id: format!("tsk_{url}"),
            status: TaskStatus::Waiting,
            steps: Vec::new(),
            created_at: 0,
            completed_at: 0,
            output_folder: String::new(),
            file_size: size,
            files: None,
            category: None,
            should_seed: false,
            checksums: Map::new(),
            extra: Map::new(),
            type_name: None,
        }
    }

    fn ui() -> TaskDraftUi {
        TaskDraftUi::new(Texts::default(), "/dl", &[], 8)
    }

    fn category(id: &str, name: &str) -> CategoryRule {
        CategoryRule {
            category_id: id.to_string(),
            name: name.to_string(),
            icon: "TAG".to_string(),
            extensions: Vec::new(),
            folder: None,
        }
    }

    #[test]
    fn a_fresh_dialog_shows_no_rows_and_no_stats() {
        let view = ui().view(&PlainHost);
        assert!(!view.open);
        assert!(view.rows.is_empty());
        assert!(!view.ratio_visible);
        assert!(!view.size_visible);
        assert!(!view.import_visible);
        assert!(!view.can_confirm);
        assert!(!view.parsing);
        assert_eq!(view.title, "添加任务");
        assert_eq!(
            view.placeholder,
            "添加多个下载链接时，请确保每行只有一个下载链接"
        );
        assert_eq!(view.batch_text, "批量添加");
        assert_eq!(view.import_text, "导入文件");
        assert_eq!(view.draft_title, "解析结果");
        assert_eq!(view.options_title, "下载设置");
        assert_eq!(view.folder_title, "选择下载路径");
        assert_eq!(view.folder_text, "/dl");
        assert_eq!(view.subworker_title, "预分配线程数");
        assert_eq!(view.subworker_value, 8);
        assert_eq!(view.subworker_min, 1);
        assert_eq!(view.subworker_max, 256);
        assert_eq!(view.download_text, "下载");
        assert_eq!(view.later_text, "稍后下载");
        assert_eq!(view.cancel_text, "取消");
    }

    #[test]
    fn a_successful_parse_turns_into_one_row() {
        let host = FakeHost::with_task("https://a/1", 1536);
        let mut ui = ui();
        ui.text_changed("https://a/1");
        let effects = ui.parse_needed(&host);
        // 只剩状态机信号，没有错误。
        assert!(
            effects
                .iter()
                .all(|effect| !matches!(effect, UiEffect::ParseError { .. }))
        );
        let view = ui.view(&host);
        assert_eq!(view.rows.len(), 1);
        assert_eq!(view.rows[0].url, "https://a/1");
        assert_eq!(view.rows[0].name, "1.bin");
        assert_eq!(view.rows[0].size, "1.50 KB");
        assert!(!view.rows[0].conflict);
        assert!(!view.rows[0].editable);
        assert!(view.can_confirm);
        assert!(!view.parsing);
        assert!(view.size_visible);
        assert_eq!(view.size_text, "1.50 KB");
        assert!(!view.ratio_visible);
    }

    #[test]
    fn the_size_label_keeps_the_card_threshold() {
        // `fileSize > 1` 才显示尺寸，`1` 也是 `"--"`（`draft_cards.py:40`）。
        let host = FakeHost::with_task("https://a/1", 1);
        let mut ui = ui();
        ui.text_changed("https://a/1");
        ui.parse_needed(&host);
        assert_eq!(ui.view(&host).rows[0].size, "--");
    }

    #[test]
    fn a_failed_parse_reports_the_truncated_url_and_the_ratio() {
        let url = "https://example.com/".to_string() + &"a".repeat(60);
        let host = FakeHost::failing(TaskError::new("解析失败", Vec::new()));
        let mut ui = ui();
        ui.text_changed(&url);
        let effects = ui.parse_needed(&host);
        let (title, body) = effects
            .iter()
            .find_map(|effect| match effect {
                UiEffect::ParseError { title, body } => Some((title.clone(), body.clone())),
                _ => None,
            })
            .expect("应当有解析失败提示");
        assert_eq!(title, "链接解析失败");
        // 前 45 字符 + "..."，再换行接本地化错误。
        let display = format!("{}...", &url[..45]);
        assert_eq!(body, format!("{display}\n解析失败"));
        let view = ui.view(&host);
        assert!(view.rows.is_empty());
        assert!(view.ratio_visible);
        assert_eq!(view.ratio_text, "0/1");
        assert!(!view.size_visible);
        assert!(!view.can_confirm);
    }

    #[test]
    fn stats_add_up_the_sizes_of_the_parsed_tasks() {
        let host = FakeHost::new();
        host.tasks
            .borrow_mut()
            .insert("https://a/1".to_string(), record("https://a/1", 1000));
        host.tasks
            .borrow_mut()
            .insert("https://a/2".to_string(), record("https://a/2", 2000));
        let mut ui = ui();
        ui.text_changed("https://a/1\nhttps://a/2");
        ui.parse_needed(&host);
        let view = ui.view(&host);
        assert_eq!(view.rows.len(), 2);
        assert!(view.size_visible);
        assert_eq!(view.size_text, crate::format::to_readable_size(3000.0));
    }

    #[test]
    fn a_parsed_task_without_a_size_falls_back_to_the_task_count() {
        let host = FakeHost::with_task("https://a/1", 0);
        let mut ui = ui();
        ui.text_changed("https://a/1");
        ui.parse_needed(&host);
        let view = ui.view(&host);
        assert!(view.size_visible);
        assert_eq!(view.size_text, "1 个任务");
    }

    #[test]
    fn accepting_parses_then_confirms_then_closes() {
        let host = FakeHost::with_task("https://a/1", 0);
        let mut ui = ui();
        ui.text_changed("https://a/1");
        let effects = ui.accept(true, &host);
        // `accept` 先 `validate()`（解析，效果排在前面）再 `done()`（确认 + 关闭），
        // 所以「确认」不是第一条，但「关闭」必须是最后一条。
        assert_eq!(confirmed_flags(&effects), vec![true]);
        assert!(matches!(effects.last(), Some(UiEffect::Closed)));
        assert!(ui.draft().items().is_empty());
        assert_eq!(ui.url_text(), "");
        assert!(!ui.is_open());
    }

    #[test]
    fn later_download_confirms_without_auto_start() {
        let host = FakeHost::with_task("https://a/1", 0);
        let mut ui = ui();
        ui.text_changed("https://a/1");
        let effects = ui.accept(false, &host);
        assert_eq!(confirmed_flags(&effects), vec![false]);
    }

    #[test]
    fn accepting_without_a_parsed_task_keeps_the_dialog_open() {
        let host = FakeHost::failing(TaskError::new("解析失败", Vec::new()));
        let mut ui = ui();
        ui.set_open(true);
        ui.text_changed("https://a/1");
        let effects = ui.accept(true, &host);
        assert!(confirmed_flags(&effects).is_empty());
        assert!(!matches!(effects.last(), Some(UiEffect::Closed)));
        assert!(ui.is_open());
    }

    #[test]
    fn cancelling_clears_without_confirming() {
        let host = FakeHost::with_task("https://a/1", 0);
        let mut ui = ui();
        ui.text_changed("https://a/1");
        ui.parse_needed(&host);
        let effects = ui.cancel(&host);
        assert!(confirmed_flags(&effects).is_empty());
        assert!(matches!(effects.last(), Some(UiEffect::Closed)));
        assert!(ui.draft().items().is_empty());
        assert_eq!(ui.url_text(), "");
    }

    #[test]
    fn done_resets_both_option_cards() {
        let host = FakeHost::with_task("https://a/1", 0);
        let mut ui = ui();
        ui.folder_edited("/tmp/elsewhere");
        ui.subworker_edited(16);
        ui.text_changed("https://a/1");
        ui.accept(true, &host);
        assert_eq!(ui.folder_text(), "/dl");
        assert_eq!(ui.subworker_value(), 8);
        let view = ui.view(&host);
        assert_eq!(view.folder_text, "/dl");
        assert_eq!(view.subworker_value, 8);
    }

    #[test]
    fn the_options_carry_the_folder_and_the_subworker_count() {
        let host = FakeHost::with_task("https://a/1", 0);
        let mut ui = ui();
        ui.folder_edited("/tmp/x");
        ui.subworker_edited(32);
        ui.text_changed("https://a/1");
        ui.parse_needed(&host);
        let options = ui.draft().base_options();
        assert_eq!(
            options.get("outputFolder").and_then(Value::as_str),
            Some("/tmp/x")
        );
        assert_eq!(
            options.get("subworkerCount").and_then(Value::as_i64),
            Some(32)
        );
        // 提交过的目录进了历史。
        assert_eq!(ui.folder_text(), "/tmp/x");
    }

    #[test]
    fn a_blank_folder_keeps_the_previous_path() {
        let host = FakeHost::with_task("https://a/1", 0);
        let mut ui = ui();
        ui.folder_edited("   ");
        ui.text_changed("https://a/1");
        ui.parse_needed(&host);
        assert_eq!(
            ui.draft()
                .base_options()
                .get("outputFolder")
                .and_then(Value::as_str),
            Some("/dl")
        );
    }

    #[test]
    fn add_urls_dedups_and_parses_immediately() {
        let host = FakeHost::with_task("https://b/2", 0);
        host.tasks
            .borrow_mut()
            .insert("https://a/1".to_string(), record("https://a/1", 0));
        let mut ui = ui();
        ui.text_changed("https://a/1");
        let effects = ui.add_urls(
            &[
                "https://b/2".to_string(),
                "  https://a/1  ".to_string(),
                String::new(),
                "https://b/2".to_string(),
            ],
            &host,
        );
        assert_eq!(ui.url_text(), "https://a/1\nhttps://b/2");
        assert!(
            ui.view(&host)
                .rows
                .iter()
                .any(|row| row.url == "https://b/2")
        );
        assert!(
            effects
                .iter()
                .all(|effect| !matches!(effect, UiEffect::ParseError { .. }))
        );
    }

    #[test]
    fn add_urls_with_nothing_new_does_not_touch_the_editor() {
        let host = FakeHost::new();
        let mut ui = ui();
        ui.text_changed("https://a/1");
        let effects = ui.add_urls(&["https://a/1".to_string()], &host);
        assert!(effects.is_empty());
        assert_eq!(ui.url_text(), "https://a/1");
    }

    #[test]
    fn the_clipboard_fills_an_empty_editor_only_once() {
        let mut ui = ui();
        let armed = ui.open("https://x/1\nhttps://y/2", |url| {
            url.starts_with("https://")
        });
        assert!(armed);
        assert_eq!(ui.url_text(), "https://x/1\nhttps://y/2");
        assert!(ui.is_open());
        let armed = ui.open("https://z/3", |url| url.starts_with("https://"));
        assert!(!armed);
        assert_eq!(ui.url_text(), "https://x/1\nhttps://y/2");
    }

    #[test]
    fn the_clipboard_filter_drops_unmatched_lines() {
        let mut ui = ui();
        ui.open("不是链接\nhttps://x/1\n", |url| {
            url.starts_with("https://")
        });
        assert_eq!(ui.url_text(), "https://x/1");
    }

    #[test]
    fn the_category_button_follows_the_config_switch() {
        let host = FakeHost::with_task("https://a/1", 0);
        let mut disabled = ui();
        disabled.set_categories(vec![category("cat_video", "视频")], false);
        disabled.text_changed("https://a/1");
        disabled.parse_needed(&host);
        assert!(!disabled.view(&host).rows[0].category_visible);

        let mut enabled = ui();
        enabled.set_categories(vec![category("cat_video", "视频")], true);
        enabled.draft_mut().update("https://a/1", |task| {
            task.category = Some("cat_video".into())
        });
        enabled.text_changed("https://a/1");
        enabled.parse_needed(&host);
        // 解析出来的 task 没有分类 → 回落到「未分类」。
        let row = &enabled.view(&host).rows[0];
        assert!(row.category_visible);
        assert_eq!(row.category, "未分类");
    }

    #[test]
    fn picking_a_category_writes_the_override() {
        let host = FakeHost::with_task("https://a/1", 0);
        let mut ui = ui();
        ui.set_categories(vec![category("cat_video", "视频")], true);
        ui.text_changed("https://a/1");
        ui.parse_needed(&host);
        ui.pick_category("https://a/1", "cat_video", &host);
        assert_eq!(
            ui.draft()
                .item_by_url("https://a/1")
                .unwrap()
                .category_override,
            Some("cat_video".to_string())
        );
        assert_eq!(ui.view(&host).rows[0].category, "视频");
        // 空串 = 未分类（清掉覆盖）。
        ui.pick_category("https://a/1", "", &host);
        assert_eq!(
            ui.draft()
                .item_by_url("https://a/1")
                .unwrap()
                .category_override,
            None
        );
    }

    #[test]
    fn the_category_menu_is_only_offered_when_enabled() {
        let mut ui = ui();
        ui.set_categories(vec![category("cat_video", "视频")], false);
        assert!(ui.category_menu_requested("https://a/1").is_empty());
        ui.set_categories(vec![category("cat_video", "视频")], true);
        let effects = ui.category_menu_requested("https://a/1");
        match &effects[0] {
            UiEffect::CategoryMenuRequested { url, entries } => {
                assert_eq!(url, "https://a/1");
                // 未分类 + 分隔线 + 1 个分类。
                assert_eq!(entries.len(), 3);
            }
            other => panic!("期望分类菜单，实际 {other:?}"),
        }
    }

    #[test]
    fn editing_is_ignored_without_a_parsed_task() {
        let mut ui = ui();
        assert!(ui.edit_requested("https://a/1").is_empty());
        let host = FakeHost::with_task("https://a/1", 0);
        ui.text_changed("https://a/1");
        ui.parse_needed(&host);
        assert!(matches!(
            ui.edit_requested("https://a/1").as_slice(),
            [UiEffect::EditRequested { .. }]
        ));
    }

    #[test]
    fn the_import_button_needs_file_types() {
        let mut ui = ui();
        assert!(!ui.view(&PlainHost).import_visible);
        assert!(ui.import_requested().is_empty());

        ui.set_file_types(vec![
            (
                "Torrent".to_string(),
                vec![".torrent".to_string(), ".bt".to_string()],
            ),
            ("Metalink".to_string(), vec![".meta4".to_string()]),
        ]);
        assert!(ui.view(&PlainHost).import_visible);
        let effects = ui.import_requested();
        match &effects[0] {
            UiEffect::ImportRequested { title, filters } => {
                assert_eq!(title, "导入文件");
                assert_eq!(
                    filters,
                    &vec![
                        "所有可导入文件 (*.torrent *.bt *.meta4)".to_string(),
                        "Torrent (*.torrent *.bt)".to_string(),
                        "Metalink (*.meta4)".to_string(),
                    ]
                );
            }
            other => panic!("期望导入请求，实际 {other:?}"),
        }
    }

    #[test]
    fn batch_and_folder_requests_are_plain_effects() {
        let mut ui = ui();
        assert!(matches!(
            ui.batch_requested().as_slice(),
            [UiEffect::BatchRequested]
        ));
        assert!(matches!(
            ui.folder_pick_requested().as_slice(),
            [UiEffect::FolderPickRequested]
        ));
    }

    #[test]
    fn option_values_become_json() {
        assert_eq!(
            option_value_to_json(&OptionValue::Text("auto".into())),
            Value::String("auto".into())
        );
        assert_eq!(
            option_value_to_json(&OptionValue::Path("/tmp/x".into())),
            Value::String("/tmp/x".into())
        );
        assert_eq!(
            option_value_to_json(&OptionValue::Number(8)),
            Value::from(8)
        );
        let headers = option_value_to_json(&OptionValue::Headers(vec![(
            "user-agent".to_string(),
            "curl".to_string(),
        )]));
        assert_eq!(headers["user-agent"], Value::String("curl".into()));
    }

    #[test]
    fn add_parsed_tasks_appends_the_new_urls_without_reparsing() {
        let host = FakeHost::new();
        let mut ui = ui();
        ui.add_parsed_tasks(vec![record("https://a/1", 0)], &host);
        assert_eq!(ui.url_text(), "https://a/1");
        assert_eq!(ui.view(&host).rows.len(), 1);
        // 已经在草稿里 → 不重复追加。
        ui.add_parsed_tasks(vec![record("https://a/1", 0)], &host);
        assert_eq!(ui.url_text(), "https://a/1");
    }

    #[test]
    fn apply_pushes_the_whole_snapshot_into_the_window() {
        let window = crate::create_headless();
        let host = FakeHost::with_task("https://a/1", 1536);
        let mut ui = ui();
        ui.set_open(true);
        ui.text_changed("https://a/1");
        ui.parse_needed(&host);
        let view = ui.view(&host);
        apply(&window, &view);

        assert!(window.get_taskDraftVisible());
        assert_eq!(window.get_taskDraftTitle().as_str(), "添加任务");
        assert_eq!(window.get_taskDraftUrlText().as_str(), "https://a/1");
        assert!(window.get_taskDraftCanConfirm());
        assert!(!window.get_taskDraftImportVisible());
        assert_eq!(window.get_taskDraftRows().row_count(), 1);
        let row = window.get_taskDraftRows().row_data(0).unwrap();
        assert_eq!(row.name.as_str(), "1.bin");
        assert_eq!(row.size.as_str(), "1.50 KB");
        assert_eq!(window.get_taskDraftSizeText().as_str(), "1.50 KB");
        assert!(window.get_taskDraftSizeVisible());
    }

    #[test]
    fn installing_wires_the_download_button() {
        let window = crate::create_headless();
        let host = Rc::new(FakeHost::with_task("https://a/1", 0));
        let ui = Rc::new(RefCell::new(ui()));
        let opener = install(&window, Rc::clone(&ui), host.clone());

        opener();
        assert!(window.get_taskDraftVisible());

        ui.borrow_mut().text_changed("https://a/1");
        window.invoke_taskDraftDownloadClicked();

        assert_eq!(
            host.log(),
            vec!["confirm:1.bin:true".to_string(), "closed".to_string()]
        );
        assert!(!window.get_taskDraftVisible(), "关框后覆盖层应当收起");
    }

    #[test]
    fn installing_routes_the_secondary_callbacks() {
        let window = crate::create_headless();
        let host = Rc::new(FakeHost::with_task("https://a/1", 0));
        let ui = Rc::new(RefCell::new(ui()));
        install(&window, Rc::clone(&ui), host.clone());

        window.invoke_taskDraftBatchClicked();
        window.invoke_taskDraftFolderPickRequested();
        window.invoke_taskDraftCancelClicked();
        assert_eq!(
            host.log(),
            vec![
                "batch".to_string(),
                "pick".to_string(),
                "closed".to_string()
            ]
        );
    }
}
