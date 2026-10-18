//! `app/view/components/bootstrap_list.py`（266 行）的语义镜像。
//!
//! 覆盖三块纯逻辑：
//!
//! 1. [`to_ago_text`] / [`to_status_text`] / [`card_content`]——卡片副标题的
//!    格式化口径（`_refreshContent` / `toStatusText` / `toAgoText`）；
//! 2. [`is_subscription_url`]——镜像 CPython `urlsplit` 的**判定**部分
//!    （`validate()` 的逐行校验）；
//! 3. [`BootstrapListDialogState`]——「管理订阅」对话框的行模型
//!    （`BootstrapListDialog` + `SubscriptionRow`）。
//!
//! 不做的事（Qt 侧只有视图语义，无配置口径）：
//!
//! - `SettingCard` / `MessageBoxBase` / `InfoBar` / `AutoSizingEdit` 的控件与
//!   布局（`hBoxLayout.addSpacing(8)`、`viewLayout.addSpacing(8)` …）；
//! - `self._submit(coro, done=..., failed=..., owner=self)` 的协程调度与
//!   `refreshButton.setEnabled(False)` 的按钮态（归引擎桥接步）；
//! - `BootstrapList.refresh()` 的 `asyncio.shield` 复用、`_cachePath` 的
//!   `sha1(url)[:16]`、`statuses()` 的读盘（归 `gd3-core` / 引擎桥接步）。
//!
//! ## 实证结论（Python 源码口径）
//!
//! - `toAgoText` 的四个门槛是 **60 / 3600 / 86400**，都取 `int(seconds // n)`
//!   （Python 浮点地板除），负数在第一个分支就返回「刚刚」；
//! - `toStatusText` 的 `count is None` 分支里，`status is not None and
//!   status.error` 是**真值**判断——空串 `""` 不算失败（`SubscriptionStatus`
//!   的 `error` 缺省就是 `""`）；
//! - `_refreshContent` 的「更新失败」在两处口径不同：有缓存时只在
//!   `time.time() - newest > FAILURE_NOTICE_AGE`（**严格大于** 7 天）才追加，
//!   无缓存时 `hasError` 立即追加；
//! - `hasError` 看的是**全部** statuses（含无缓存的），而 `newest` 只取
//!   `count is not None` 的那批——两者集合不同，故「某个订阅从没拉过、
//!   另一个有缓存」时用后者的时间戳判新鲜度；
//! - `validate()` 先逐行校验**全部**通过才 `cfg.set`（任何一行失败都不写盘），
//!   去重靠 `dict.fromkeys`（保序、按 `row.url` 即 strip 后的值）；
//! - 自定义条目用 `line.strip()` 逐行清洗后 `"\n".join(... if line)`，
//!   空行与纯空白行都被丢掉（`splitlines()` 的口径见
//!   [`crate::pystr::split_lines`]）。
//!
//! ## 已知偏差
//!
//! - `urlsplit` 对「`[` 与 `]` 不配对」的 netloc 抛 `ValueError`；这里退化为
//!   `false`（校验失败而不是崩对话框）。同一输入下 Python 会抛异常到
//!   `validate()` 外面，行为本来就不是「校验失败」，故不算语义冲突；
//! - `SubscriptionStatus.updatedAt` 在 Python 是 `float | None`（未缓存时
//!   `None`），这里保留 `Option<f64>`，但 `statuses()` 保证「`count` 与
//!   `updatedAt` 同生同灭」，所以 `None` 分支只在手工构造时出现，取 0.0 兜底。

use crate::pystr::py_strip;

/// `FAILURE_NOTICE_AGE = 7 * 86400`：有缓存时「更新失败」的滞后门槛（秒）。
pub const FAILURE_NOTICE_AGE: f64 = 7.0 * 86400.0;
/// `CARD_HEIGHT = 70`：卡片定高。
pub const CARD_HEIGHT: i32 = 70;
/// `app/bootstrap_list.py::MAX_AGE = 86400`：缓存新鲜度门槛（秒）。
///
/// 卡片的格式化不用它（那是 `BootstrapList.isStale()` 的事），列在这里是为了
/// 让「7 天」与「1 天」两个时间门槛在同一处可见，避免被后人搞混。
pub const MAX_AGE: f64 = 86400.0;
/// `self.widget.setMinimumWidth(720)`：对话框最小宽度。
pub const DIALOG_MIN_WIDTH: i32 = 720;

/// `self.tr("管理")`：卡片右侧主按钮。
pub const MANAGE_BUTTON_TEXT: &str = "管理";
/// `self.refreshButton.setToolTip(self.tr("立即更新"))`。
pub const REFRESH_TOOLTIP: &str = "立即更新";
/// `self.tr("订阅地址")`：对话框第一段标题。
pub const SUBSCRIPTION_HEADER: &str = "订阅地址";
/// `self.tr("恢复默认")`。
pub const RESTORE_TEXT: &str = "恢复默认";
/// `self.tr("添加")`。
pub const ADD_TEXT: &str = "添加";
/// `self.tr("自定义条目")`。
pub const CUSTOM_HEADER: &str = "自定义条目";
/// `self.yesButton.setText(self.tr("保存并更新"))`。
pub const SAVE_BUTTON_TEXT: &str = "保存并更新";
/// `self.cancelButton.setText(self.tr("取消"))`。
pub const CANCEL_BUTTON_TEXT: &str = "取消";
/// `self.urlEdit.setPlaceholderText("https://")`（`SubscriptionRow`）。
pub const URL_PLACEHOLDER: &str = "https://";
/// `self.customEdit.setPlaceholderText(self.tr("每行一个，不会被订阅更新覆盖"))`。
pub const CUSTOM_PLACEHOLDER: &str = "每行一个，不会被订阅更新覆盖";
/// `InfoBar.error(self.tr("订阅地址无效"), ...)` 的标题。
pub const INVALID_URL_TITLE: &str = "订阅地址无效";
/// `InfoBar.error(..., self.tr("请输入有效的 HTTP/HTTPS 地址"))` 的正文。
pub const INVALID_URL_TEXT: &str = "请输入有效的 HTTP/HTTPS 地址";

/// `toAgoText(seconds)`：相对时间的人话。
///
/// 四个分支的边界是 `60` / `3600` / `86400`，除数是 `60` / `3600` / `86400`。
/// 负数落第一个分支（「刚刚」），所以取整不会碰到负值。
pub fn to_ago_text(seconds: f64) -> String {
    if seconds < 60.0 {
        return "刚刚".to_string();
    }
    if seconds < 3600.0 {
        return format!("{} 分钟前", (seconds / 60.0).floor() as i64);
    }
    if seconds < 86400.0 {
        return format!("{} 小时前", (seconds / 3600.0).floor() as i64);
    }
    format!("{} 天前", (seconds / 86400.0).floor() as i64)
}

/// `app/bootstrap_list.py::SubscriptionStatus` 的镜像。
#[derive(Clone, Debug, PartialEq)]
pub struct SubscriptionStatus {
    /// 订阅地址（`statuses()` 按 `self._sources()` 的顺序产出）。
    pub url: String,
    /// `len(set(parse(bytes)))`；未缓存时 `None`。
    pub count: Option<i64>,
    /// 缓存文件的 `st_mtime`；未缓存时 `None`。
    pub updated_at: Option<f64>,
    /// `self._errors.get(url, "")`——缺省空串。
    pub error: Option<String>,
}

impl SubscriptionStatus {
    /// 便捷构造：`count` 与 `updated_at` 同生同灭（`statuses()` 的不变式）。
    pub fn cached(url: impl Into<String>, count: i64, updated_at: f64) -> Self {
        Self {
            url: url.into(),
            count: Some(count),
            updated_at: Some(updated_at),
            error: None,
        }
    }

    /// 便捷构造：未缓存（`count is None`）。
    pub fn uncached(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            count: None,
            updated_at: None,
            error: None,
        }
    }

    /// 带错误串的链式构造。
    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }

    /// `bool(status.error)`——**空串为假**（`error` 的缺省值就是 `""`）。
    pub fn has_error(&self) -> bool {
        self.error.as_deref().is_some_and(|text| !text.is_empty())
    }

    /// `status.updatedAt`，`None` 时取 0.0（见模块头「已知偏差」）。
    fn updated_at_or_zero(&self) -> f64 {
        self.updated_at.unwrap_or(0.0)
    }
}

/// `toStatusText(status)`：行尾状态文字。
///
/// `status is None` 与 `count is None` 走同一支：有 `error` 报「更新失败」，
/// 否则「未拉取」。有缓存时是 `"N 条，<ago>更新"`，再带 error 才追加
/// 「更新失败」——注意这里**不看** `FAILURE_NOTICE_AGE`，只要 `error` 真值
/// 就追加（与 `_refreshContent` 的口径不同）。
pub fn to_status_text(status: Option<&SubscriptionStatus>, now: f64) -> String {
    let Some(status) = status else {
        return "未拉取".to_string();
    };
    let Some(count) = status.count else {
        return if status.has_error() {
            "更新失败".to_string()
        } else {
            "未拉取".to_string()
        };
    };
    let mut parts = vec![
        format!("{count} 条"),
        format!("{}更新", to_ago_text(now - status.updated_at_or_zero())),
    ];
    if status.has_error() {
        parts.push("更新失败".to_string());
    }
    parts.join("，")
}

/// `isSubscriptionUrl(url)`：scheme ∈ {http, https} 且 netloc 非空。
///
/// 镜像 CPython `urlsplit` 的判定链路（`Lib/urllib/parse.py`）：
///
/// 1. 去掉前导 C0 控制符与空格（`_WHATWG_C0_CONTROL_OR_SPACE`，即 `0x00..=0x20`；
///    **只 lstrip**，尾部空格保留——CPython 的注释就是这么写的）；
/// 2. 删除 `\t` / `\r` / `\n`（`_UNSAFE_URL_BYTES_TO_REMOVE`）；
/// 3. 找第一个 `:`：`i > 0` 且 `url[0]` 是 ASCII 字母，且 `url[:i]` 全在
///    `ascii_letters + digits + "+-."` 里，才算 scheme（小写化后切走）；
/// 4. 剩余部分以 `//` 开头才解析 netloc：netloc 是 `//` 之后到最早的
///    `/`、`?`、`#` 之间的子串；
/// 5. `[`/`]` 不配对 → CPython 抛 `ValueError`，这里返回 `false`。
pub fn is_subscription_url(url: &str) -> bool {
    // 1. lstrip C0 控制符与空格。
    let trimmed = url.trim_start_matches(|c: char| (c as u32) <= 0x20);
    // 2. 删除三个不安全字节。
    let cleaned: String = trimmed
        .chars()
        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
        .collect();

    // 3. scheme 识别。
    let mut scheme = "";
    let mut rest = cleaned.as_str();
    if let Some(index) = cleaned.find(':') {
        let is_scheme = index > 0
            && cleaned.as_bytes()[0].is_ascii_alphabetic()
            && cleaned[..index]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
        if is_scheme {
            scheme = &cleaned[..index];
            rest = &cleaned[index + 1..];
        }
    }
    // scheme 已被 urlsplit 小写化（`url[:i].lower()`）。
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return false;
    }

    // 4. netloc。
    let Some(after_slashes) = rest.strip_prefix("//") else {
        return false;
    };
    let end = after_slashes
        .find(['/', '?', '#'])
        .unwrap_or(after_slashes.len());
    let netloc = &after_slashes[..end];

    // 5. 括号配对（CPython 的 ValueError → 这里 false）。
    let brackets_balanced = netloc.contains('[') == netloc.contains(']');
    if !brackets_balanced {
        return false;
    }
    !netloc.is_empty()
}

/// `BootstrapListCard._refreshContent()`：卡片副标题。
///
/// `entry_count` 是 `bootstrapList.entryCount()`（Python 侧用 `set` 去重后的
/// 条目数），由调用方给——本 crate 不读盘。
///
/// 分支口径见模块头「实证结论」：`cached` 只看 `count is not None`，
/// `hasError` 看全部 statuses，「更新失败」的追加条件两处不同。
pub fn card_content(
    is_refreshing: bool,
    statuses: &[SubscriptionStatus],
    entry_count: usize,
    network_text: &str,
    now: f64,
) -> String {
    if is_refreshing {
        return "正在更新…".to_string();
    }
    let cached: Vec<&SubscriptionStatus> = statuses
        .iter()
        .filter(|status| status.count.is_some())
        .collect();
    let has_error = statuses.iter().any(SubscriptionStatus::has_error);
    let mut parts = vec![format!("{} 个订阅", statuses.len())];
    if cached.is_empty() {
        parts.push("使用内置列表".to_string());
        if has_error {
            parts.push("更新失败".to_string());
        }
    } else {
        // `max(s.updatedAt for s in cached)`；statuses() 保证有值，None 兜 0.0。
        let newest = cached
            .iter()
            .map(|status| status.updated_at_or_zero())
            .fold(f64::NEG_INFINITY, f64::max);
        parts.push(format!("共 {entry_count} 条"));
        parts.push(format!("{}更新", to_ago_text(now - newest)));
        if has_error && now - newest > FAILURE_NOTICE_AGE {
            parts.push("更新失败".to_string());
        }
    }
    if !network_text.is_empty() {
        parts.push(network_text.to_string());
    }
    parts.join("，")
}

/// `SubscriptionRow` 的行状态（去掉控件，只留数据）。
#[derive(Clone, Debug, PartialEq)]
pub struct SubscriptionRow {
    /// `urlEdit.text()` 的原始内容——`url` 属性才 strip。
    pub text: String,
    /// `statusLabel` 的文字（`toStatusText` 的结果，构造时算一次）。
    pub status_text: String,
    /// `statusLabel.setToolTip(status.error if status is not None else "")`。
    pub tooltip: String,
}

impl SubscriptionRow {
    /// `@property def url(self) -> str: return self.urlEdit.text().strip()`。
    pub fn url(&self) -> &str {
        py_strip(&self.text)
    }
}

/// `validate()` 通过后的落盘产物。
///
/// `sources` 已按 `dict.fromkeys` 去重保序；`custom` 为 `None` 表示
/// `customItem is None`（Python 侧整段 `cfg.set` 都不执行）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub sources: Vec<String>,
    pub custom: Option<String>,
}

/// `validate()` 的失败点：第 `row` 行的地址非法（Python 侧对那一行
/// `setFocus()` 并弹 `InfoBar.error`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidUrl {
    pub row: usize,
}

/// `BootstrapListDialog` 的行模型。
///
/// 与 Python 的对应关系：
///
/// | Python | 这里 |
/// |---|---|
/// | `self._sourcesItem.value`（初始行） | `new(sources, ...)` |
/// | `self._sourcesItem.defaultValue`（恢复默认） | `default_sources` |
/// | `self._customItem`（`None` = 不显示自定义段） | `has_custom_item` |
/// | `self.customEdit.toPlainText()` | `custom_text` / `set_custom_text` |
/// | `self._statuses`（`{s.url: s}`，后者覆盖前者） | 内部 `HashMap` |
/// | `self._rows` | `rows` |
#[derive(Debug)]
pub struct BootstrapListDialogState {
    statuses: std::collections::HashMap<String, SubscriptionStatus>,
    default_sources: Vec<String>,
    has_custom_item: bool,
    custom_text: String,
    rows: Vec<SubscriptionRow>,
}

impl BootstrapListDialogState {
    /// `BootstrapListDialog.__init__` + `_initWidget`：按 `sources` 逐行建行。
    ///
    /// `now` 由调用方给（Python 的 `toStatusText` 内部调 `time.time()`）。
    pub fn new(
        sources: &[String],
        default_sources: &[String],
        custom: Option<&str>,
        statuses: Vec<SubscriptionStatus>,
        now: f64,
    ) -> Self {
        // `{s.url: s for s in bootstrapList.statuses()}`——同 url 后者覆盖。
        let statuses = statuses
            .into_iter()
            .map(|status| (status.url.clone(), status))
            .collect();
        let mut state = Self {
            statuses,
            default_sources: default_sources.to_vec(),
            has_custom_item: custom.is_some(),
            // `customEdit.setPlainText(self._customItem.value)`
            custom_text: custom.unwrap_or_default().to_string(),
            rows: Vec::new(),
        };
        for url in sources {
            state.add_row(url, now);
        }
        state
    }

    pub fn rows(&self) -> &[SubscriptionRow] {
        &self.rows
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// `self.customLabel.setVisible(self._customItem is not None)`。
    pub fn has_custom_item(&self) -> bool {
        self.has_custom_item
    }

    pub fn custom_text(&self) -> &str {
        &self.custom_text
    }

    /// `self.customEdit.setPlainText(...)` 的对应物。
    pub fn set_custom_text(&mut self, text: &str) {
        self.custom_text = text.to_string();
    }

    /// `_addRow(url)`：状态文字在**建行时**算一次（Python 也是构造时传进
    /// `SubscriptionRow`，之后不再刷新）。
    pub fn add_row(&mut self, url: &str, now: f64) {
        let status = self.statuses.get(url);
        self.rows.push(SubscriptionRow {
            text: url.to_string(),
            status_text: to_status_text(status, now),
            tooltip: status
                .and_then(|status| status.error.clone())
                .unwrap_or_default(),
        });
    }

    /// `_onRowRemoved(row)`：按位置移除（Python 用对象身份 `list.remove`，
    /// 行对象在 Rust 侧没有身份，故收索引）。
    ///
    /// 越界时什么都不做（Python 的 `remove` 会抛 `ValueError`，属调用方 bug）。
    pub fn remove_row(&mut self, index: usize) {
        if index < self.rows.len() {
            self.rows.remove(index);
        }
    }

    /// `_onRestoreClicked()`：清空后按 `defaultValue` 重建。
    pub fn restore_defaults(&mut self, now: f64) {
        self.rows.clear();
        let defaults = self.default_sources.clone();
        for url in &defaults {
            self.add_row(url, now);
        }
    }

    /// `validate()`：逐行校验 → 去重 → 清洗自定义条目。
    ///
    /// 校验失败时**不产生** [`Commit`]（Python 侧 `cfg.set` 在循环之后，
    /// 任何一行失败都直接 `return False`，前面已通过的行也不会写盘）。
    pub fn validate(&self) -> Result<Commit, InvalidUrl> {
        for (index, row) in self.rows.iter().enumerate() {
            if !is_subscription_url(row.url()) {
                return Err(InvalidUrl { row: index });
            }
        }
        // `list(dict.fromkeys(row.url for row in self._rows))`
        let mut sources: Vec<String> = Vec::new();
        for row in &self.rows {
            let url = row.url();
            if !sources.iter().any(|seen| seen == url) {
                sources.push(url.to_string());
            }
        }
        let custom = if self.has_custom_item {
            let lines = crate::pystr::split_lines(&self.custom_text);
            let stripped: Vec<&str> = lines
                .iter()
                .map(|line| py_strip(line))
                .filter(|line| !line.is_empty())
                .collect();
            Some(stripped.join("\n"))
        } else {
            None
        };
        Ok(Commit { sources, custom })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `toAgoText` 的四个门槛与取整口径（`int(seconds // n)`）。
    #[test]
    fn ago_text_uses_python_floor_division() {
        assert_eq!(to_ago_text(-5.0), "刚刚");
        assert_eq!(to_ago_text(0.0), "刚刚");
        assert_eq!(to_ago_text(59.9), "刚刚");
        assert_eq!(to_ago_text(60.0), "1 分钟前");
        assert_eq!(to_ago_text(119.0), "1 分钟前");
        assert_eq!(to_ago_text(3599.0), "59 分钟前");
        assert_eq!(to_ago_text(3600.0), "1 小时前");
        assert_eq!(to_ago_text(86399.0), "23 小时前");
        assert_eq!(to_ago_text(86400.0), "1 天前");
        assert_eq!(to_ago_text(604800.0), "7 天前");
    }

    /// `toStatusText`：三支 + 空串 error 不算失败。
    #[test]
    fn status_text_mirrors_python_branches() {
        // status is None
        assert_eq!(to_status_text(None, 1000.0), "未拉取");
        // count is None，无 error
        let status = SubscriptionStatus::uncached("u");
        assert_eq!(to_status_text(Some(&status), 1000.0), "未拉取");
        // count is None，error 是空串 → 仍是「未拉取」（真值判断）
        let status = SubscriptionStatus::uncached("u").with_error("");
        assert_eq!(to_status_text(Some(&status), 1000.0), "未拉取");
        // count is None，error 非空
        let status = SubscriptionStatus::uncached("u").with_error("boom");
        assert_eq!(to_status_text(Some(&status), 1000.0), "更新失败");
        // 有缓存：`"N 条，<ago>更新"`
        let status = SubscriptionStatus::cached("u", 12, 1000.0);
        assert_eq!(to_status_text(Some(&status), 1090.0), "12 条，1 分钟前更新");
        // 有缓存 + error：追加「更新失败」，不看 7 天门槛。
        let status = SubscriptionStatus::cached("u", 12, 1000.0).with_error("boom");
        assert_eq!(
            to_status_text(Some(&status), 1090.0),
            "12 条，1 分钟前更新，更新失败"
        );
    }

    /// `isSubscriptionUrl`：镜像 urlsplit 的五步判定。
    #[test]
    fn subscription_url_mirrors_urlsplit() {
        assert!(is_subscription_url("https://a.b/c"));
        assert!(is_subscription_url("http://x"));
        assert!(is_subscription_url("HTTPS://x"));
        assert!(is_subscription_url("https://a?b"));
        assert!(is_subscription_url("https://a#b"));
        // lstrip C0/空格 + 删除 \t\r\n。
        assert!(is_subscription_url(" https://x"));
        assert!(is_subscription_url("\u{1}https://x"));
        assert!(is_subscription_url("ht\ttp://x"));
        assert!(is_subscription_url("http://x\r\n"));
        // 尾部空格保留（CPython 只 lstrip）——不影响 netloc 非空。
        assert!(is_subscription_url("https://x "));
        // scheme 不在白名单。
        assert!(!is_subscription_url("ftp://x"));
        assert!(!is_subscription_url("//x"));
        assert!(!is_subscription_url("x"));
        assert!(!is_subscription_url(""));
        // netloc 为空。
        assert!(!is_subscription_url("https://"));
        assert!(!is_subscription_url("https:///path"));
        // scheme 字符集：`_` 不在 `ascii_letters + digits + "+-."` 里。
        assert!(!is_subscription_url("h_ttp://x"));
        // 非法 IPv6 括号：Python 抛 ValueError，这里 false。
        assert!(!is_subscription_url("https://[::1/x"));
        assert!(!is_subscription_url("https://::1]/x"));
        assert!(is_subscription_url("https://[::1]/x"));
    }

    /// `_refreshContent`：正在更新 / 无缓存 / 有缓存三支。
    #[test]
    fn card_content_covers_refreshing_and_uncached() {
        let now = 1_000_000.0;
        assert_eq!(card_content(true, &[], 0, "", now), "正在更新…");
        assert_eq!(
            card_content(false, &[], 0, "", now),
            "0 个订阅，使用内置列表"
        );
        let statuses = vec![SubscriptionStatus::uncached("u")];
        assert_eq!(
            card_content(false, &statuses, 0, "", now),
            "1 个订阅，使用内置列表"
        );
        let statuses = vec![SubscriptionStatus::uncached("u").with_error("boom")];
        assert_eq!(
            card_content(false, &statuses, 0, "", now),
            "1 个订阅，使用内置列表，更新失败"
        );
        // 网络描述非空才追加（`if self._networkText`）。
        assert_eq!(
            card_content(false, &[], 0, "网络正常", now),
            "0 个订阅，使用内置列表，网络正常"
        );
    }

    /// `_refreshContent`：有缓存时「更新失败」只在超过 7 天才追加。
    #[test]
    fn card_content_reports_failure_only_after_seven_days() {
        let now = 1_000_000.0;
        // 90 秒前刚更新，带 error → 不报「更新失败」。
        let fresh = vec![SubscriptionStatus::cached("u", 12, now - 90.0).with_error("boom")];
        assert_eq!(
            card_content(false, &fresh, 12, "", now),
            "1 个订阅，共 12 条，1 分钟前更新"
        );
        // 恰好 7 天：`>` 是严格大于，不追加。
        let exact =
            vec![SubscriptionStatus::cached("u", 12, now - FAILURE_NOTICE_AGE).with_error("boom")];
        assert_eq!(
            card_content(false, &exact, 12, "", now),
            "1 个订阅，共 12 条，7 天前更新"
        );
        // 超过 7 天：追加。
        let stale = vec![
            SubscriptionStatus::cached("u", 12, now - FAILURE_NOTICE_AGE - 1.0).with_error("boom"),
        ];
        assert_eq!(
            card_content(false, &stale, 12, "", now),
            "1 个订阅，共 12 条，7 天前更新，更新失败"
        );
    }

    /// `newest` 只取有缓存的那批，`hasError` 看全部 statuses。
    #[test]
    fn card_content_splits_cached_and_error_sets() {
        let now = 1_000_000.0;
        // 一个从没拉过（带 error）+ 一个很旧但没 error：
        // hasError 为真、newest 取旧的缓存时间 → 超过 7 天 → 报失败。
        let statuses = vec![
            SubscriptionStatus::uncached("a").with_error("boom"),
            SubscriptionStatus::cached("b", 7, now - FAILURE_NOTICE_AGE - 10.0),
        ];
        assert_eq!(
            card_content(false, &statuses, 7, "", now),
            "2 个订阅，共 7 条，7 天前更新，更新失败"
        );
        // 反过来：缓存很新、另一条带 error → 新鲜度用新的，不报失败。
        let statuses = vec![
            SubscriptionStatus::uncached("a").with_error("boom"),
            SubscriptionStatus::cached("b", 7, now - 30.0),
        ];
        assert_eq!(
            card_content(false, &statuses, 7, "", now),
            "2 个订阅，共 7 条，刚刚更新"
        );
    }

    /// 对话框初始行：逐行状态文字 + tooltip。
    #[test]
    fn dialog_builds_rows_with_status_text_and_tooltip() {
        let now = 1_000_000.0;
        let statuses = vec![
            SubscriptionStatus::cached("https://a", 12, now - 90.0),
            SubscriptionStatus::uncached("https://b").with_error("boom"),
        ];
        let sources = vec![
            "https://a".to_string(),
            "https://b".to_string(),
            "https://c".to_string(),
        ];
        let state = BootstrapListDialogState::new(&sources, &[], None, statuses, now);
        assert_eq!(state.row_count(), 3);
        let rows = state.rows();
        assert_eq!(rows[0].status_text, "12 条，1 分钟前更新");
        assert_eq!(rows[0].tooltip, "");
        assert_eq!(rows[1].status_text, "更新失败");
        assert_eq!(rows[1].tooltip, "boom");
        assert_eq!(rows[2].status_text, "未拉取");
        assert_eq!(rows[2].tooltip, "");
        // customItem is None → 自定义段整体不显示。
        assert!(!state.has_custom_item());
        assert_eq!(state.custom_text(), "");
    }

    /// `_addRow("")` + `validate()`：空行即非法，且不写盘。
    #[test]
    fn validate_rejects_empty_row() {
        let mut state =
            BootstrapListDialogState::new(&["https://a".to_string()], &[], None, vec![], 0.0);
        state.add_row("", 0.0);
        assert_eq!(state.validate(), Err(InvalidUrl { row: 1 }));
        // 删掉空行后通过。
        state.remove_row(1);
        assert_eq!(
            state.validate(),
            Ok(Commit {
                sources: vec!["https://a".to_string()],
                custom: None,
            })
        );
    }

    /// `validate()` 的去重口径：按 strip 后的 url 保序去重。
    #[test]
    fn validate_dedups_stripped_urls_preserving_order() {
        let mut state = BootstrapListDialogState::new(&[], &[], None, vec![], 0.0);
        state.add_row("  https://b  ", 0.0);
        state.add_row("https://a", 0.0);
        state.add_row("https://b", 0.0);
        assert_eq!(
            state.validate(),
            Ok(Commit {
                sources: vec!["https://b".to_string(), "https://a".to_string()],
                custom: None,
            })
        );
        // `url` 属性 strip：首行 `"  https://b  "` 参与校验的是 strip 后的值。
        assert_eq!(state.rows()[0].url(), "https://b");
    }

    /// 自定义条目：逐行 strip、丢空行、`\n` 连接；无 customItem 时为 None。
    #[test]
    fn validate_cleans_custom_lines() {
        let mut state = BootstrapListDialogState::new(
            &["https://a".to_string()],
            &[],
            Some("old"),
            vec![],
            0.0,
        );
        assert!(state.has_custom_item());
        assert_eq!(state.custom_text(), "old");
        state.set_custom_text("  x  \n\n\t y\n");
        assert_eq!(
            state.validate(),
            Ok(Commit {
                sources: vec!["https://a".to_string()],
                custom: Some("x\ny".to_string()),
            })
        );
        // 全空 → 空串（Python `"\n".join([])`）。
        state.set_custom_text("   \n\t\n");
        assert_eq!(
            state.validate(),
            Ok(Commit {
                sources: vec!["https://a".to_string()],
                custom: Some(String::new()),
            })
        );
    }

    /// `_onRestoreClicked`：清空后按 defaultValue 重建（状态文字同样重算）。
    #[test]
    fn restore_defaults_rebuilds_rows() {
        let now = 1_000_000.0;
        let statuses = vec![SubscriptionStatus::cached("https://d", 3, now - 7200.0)];
        let defaults = vec!["https://d".to_string()];
        let mut state = BootstrapListDialogState::new(
            &["https://a".to_string()],
            &defaults,
            None,
            statuses,
            now,
        );
        state.restore_defaults(now);
        assert_eq!(state.row_count(), 1);
        assert_eq!(state.rows()[0].text, "https://d");
        assert_eq!(state.rows()[0].status_text, "3 条，2 小时前更新");
        // 恢复默认后校验通过。
        assert!(state.validate().is_ok());
    }

    /// `remove_row` 越界是静默 no-op（Python 的 `list.remove` 会抛，属调用方 bug）。
    #[test]
    fn remove_row_out_of_range_is_noop() {
        let mut state =
            BootstrapListDialogState::new(&["https://a".to_string()], &[], None, vec![], 0.0);
        state.remove_row(5);
        assert_eq!(state.row_count(), 1);
        state.remove_row(0);
        assert_eq!(state.row_count(), 0);
    }
}
