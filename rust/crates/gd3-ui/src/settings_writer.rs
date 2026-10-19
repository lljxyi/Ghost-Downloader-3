//! 设置页回写：`SettingsActions` 的 cfg 写半边。
//!
//! 第 32–39 步把设置页的每个控件都接成了 [`SettingsActions`] 的一个闭包，第 113 步把
//! `QConfig.set` 的语义落成了 [`ConfigStore`]。这一步把两者接起来：**动作 → 属性名 →
//! `ConfigStore::set`**，于是设置页的编辑终于能落盘。
//!
//! | Python 调用点 | 这里 |
//! |---|---|
//! | `setting_cards.py:85` `cfg.set(item, int(spinBox.value() / division))` | [`SettingsWriter::max_task_num`] 等整数动作 |
//! | `setting_cards.py:115` `cfg.set(item, spinBox.value() / 100)` | [`SettingsWriter::dpi_scale`] |
//! | `setting_cards.py:138` `cfg.set(item, lineEdit.text())` | [`SettingsWriter::aria2_rpc_token`] |
//! | `setting_cards.py:372` `cfg.set(item, path)` | [`SettingsWriter::download_folder`] |
//! | `setting_cards.py:542` `cfg.set(cfg.clientProfile, value)` | [`SettingsWriter::client_profile`] |
//! | `setting_page.py:427` `cfg.set(cfg.downloadFolder, path)` | 同上 |
//! | `browser_service.py:190` `cfg.set(cfg.browserExtensionPairToken, token_urlsafe(16))` | [`SettingsWriter::regenerate_pair_token`] |
//! | `oobe_window.py:718` / `:720` / `:737` | [`SettingsWriter::run_at_login`] / [`SettingsWriter::clipboard_listener`] / [`SettingsWriter::url_scheme_enabled`] |
//!
//! `SwitchSettingCard` / `ComboBoxSettingCard` 是 qfluentwidgets 的类，它们的处理器就是
//! 一行 `qconfig.set(self.configItem, value)` —— 所以「动作收配置单位」这条惯例（换算在
//! install 层，见 `lib.rs::SettingsActions` 的注释）成立之后，这里每个方法都是
//! 「一个属性名 + 一个值」。
//!
//! ## 三条结构决策
//!
//! - **写盘复用 [`ConfigStore::set`]**，不在这里重写 `correct()` / `serialize()` /
//!   「相等早退」/「整文件回写」四件事。第 113 步已经把它们逐条对齐过 `QConfig.set`。
//! - **效果靠 [`SettingsWriter::take_changes`] 显式取走**，不在这里发信号。Python 的
//!   `valueChanged` 是同步回调，宿主（`boot::run`）拿到 `ConfigChange` 后要决定
//!   「重推 UI / 重启服务 / 提示重启」，这些都不属于配置层。
//! - **动作收配置单位**：`speed_limitation` 收字节、`dpi_scale` 收 0–5 浮点、
//!   `name_conflict` / `close_mode` / `client_profile` 收字面量 —— 索引→字面量与显示
//!   单位→配置单位的换算都在 `install_settings_actions` 那一层做完了。
//!
//! ## 本步的边界（没做的部分）
//!
//! 只做「**单个标量 → 一个 cfg 键**」。其余动作一律走 [`SettingsWriter::defer`] 记一笔，
//! 由各自的接线步接管：
//!
//! | 类别 | 动作 | 为什么留后 |
//! |---|---|---|
//! | 列表写 | `headers_preset_*` / `identity_preset_*` / `category_*` | 要读列表、改一项、再整表写回，且删除前有确认框；等对应的对话框/卡片镜像（P1/P3）落地 |
//! | 平台副作用 | `pair_token_copy` / `store_url_open` / `chromium_install` / `chromium_crx_export` | 剪贴板、浏览器、扩展解包归平台层 |
//!
//! `take_deferred()` 能列出被记下的动作名，测试里钉住了这份名单 —— 它是一张**待办清单**，
//! 不是「悄悄丢掉用户的编辑」。
//!
//! ## 与 Python 的已知差异
//!
//! - **写盘失败的收集方式**：Python 在 `cfg.set` 里直接抛 `OSError`（调用栈中断）；
//!   这里塞进 [`SettingsWriter::take_failures`]，宿主下一帧统一处理。内存里的值**已经改了**
//!   （与 Python 的「先改值再 `save()`」一致）。
//! - **重推 UI 不在写盘路径上**：Python 的 `valueChanged` 会让卡片自己回读
//!   `correct()` 之后的值（例如端口被夹到 1024），这里要宿主调 `push_settings` 重推。

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gd3_core::config::UserConfig;
use gd3_core::pair_token;
use gd3_core::settings;
use serde_json::{Value, json};

use crate::SettingsActions;
use crate::config_store::{ConfigChange, ConfigStore, ConfigStoreError};

/// 设置页动作的落盘端。
///
/// 内部是 [`RefCell`]：Slint 的回调都是 `Fn`（不可变捕获），而 `ConfigStore::set` 要
/// `&mut self`。单线程 UI 上不存在跨线程争用，`RefCell` 足够。
pub struct SettingsWriter {
    store: RefCell<ConfigStore>,
    changes: RefCell<Vec<ConfigChange>>,
    failures: RefCell<Vec<ConfigStoreError>>,
    deferred: RefCell<Vec<&'static str>>,
}

impl SettingsWriter {
    /// 接管一份已加载的 [`ConfigStore`]。
    pub fn new(store: ConfigStore) -> Self {
        Self {
            store: RefCell::new(store),
            changes: RefCell::new(Vec::new()),
            failures: RefCell::new(Vec::new()),
            deferred: RefCell::new(Vec::new()),
        }
    }

    /// `ConfigStore::load` + 接管：文件不存在时保留缺省值且**不写盘**
    /// （Python 的 `qconfig.load` 也不写）。
    pub fn open(path: impl Into<PathBuf>) -> std::io::Result<Self> {
        Ok(Self::new(ConfigStore::load(path)?))
    }

    /// 落盘路径（`UserConfig.json`）。
    pub fn path(&self) -> PathBuf {
        self.store.borrow().path().to_path_buf()
    }

    /// 当前配置的全量快照（[`gd3_core::settings::SPECS`] 的 42 项，逻辑值）。
    ///
    /// 宿主拿它重推 UI（`push_settings`）——`correct()` 可能夹紧过值，界面要跟着改。
    pub fn config(&self) -> UserConfig {
        self.store.borrow().config().clone()
    }

    /// 按属性名读逻辑值（缺键 → 缺省值）。
    pub fn get(&self, attr: &str) -> Option<Value> {
        self.store.borrow().get(attr)
    }

    /// 取走并清空「已落盘的改动」。
    pub fn take_changes(&self) -> Vec<ConfigChange> {
        std::mem::take(&mut *self.changes.borrow_mut())
    }

    /// 取走并清空「写盘失败」。
    pub fn take_failures(&self) -> Vec<ConfigStoreError> {
        std::mem::take(&mut *self.failures.borrow_mut())
    }

    /// 取走并清空「留给后续接线步的动作名」。
    pub fn take_deferred(&self) -> Vec<&'static str> {
        std::mem::take(&mut *self.deferred.borrow_mut())
    }

    /// 把外部写回的 `browserExtensionPairToken` 吞回文件。
    ///
    /// Python 的 `cfg` 是全局单例：`browser_service.py:190` 生成新令牌时
    /// `cfg.set(cfg.browserExtensionPairToken, token_urlsafe(16))` 立刻落盘。这里
    /// [`crate::engine::Engine`] 持有自己的一份克隆，[`crate::engine::Engine::refresh_services`]
    /// 可能在里面换掉令牌（首次启动令牌为空时会重新生成），所以要同步回来。
    ///
    /// 返回 `Some(change)` 表示确实写了一次盘（同一次改动也会进 [`Self::take_changes`]）。
    pub fn adopt(&self, config: &UserConfig) -> Option<ConfigChange> {
        let token = settings::read(config, "browserExtensionPairToken")?;
        if self.get("browserExtensionPairToken").as_ref() == Some(&token) {
            return None;
        }
        self.set("browserExtensionPairToken", token);
        self.changes.borrow().last().cloned()
    }

    /// `qconfig.set(item, value)`：相等早退 → `correct()` → 整文件回写。
    fn set(&self, attr: &'static str, value: Value) {
        match self.store.borrow_mut().set(attr, &value) {
            Ok(Some(change)) => self.changes.borrow_mut().push(change),
            // Python 的 `if item.value == value: return`：没写盘、没信号。
            Ok(None) => {}
            Err(error) => self.failures.borrow_mut().push(error),
        }
    }

    /// 记一笔「这一步不管」的动作（见模块头的边界表）。
    fn defer(&self, action: &'static str) {
        self.deferred.borrow_mut().push(action);
    }

    // -----------------------------------------------------------------------
    // 标量动作
    // -----------------------------------------------------------------------

    /// `GeneralDownload/DownloadFolder`：`cfg.set(cfg.downloadFolder, path)`。
    pub fn download_folder(&self, path: &str) {
        self.set("downloadFolder", Value::from(path));
    }

    /// `GeneralDownload/MaxTaskNum`：`SpinBoxSettingCard`，`division=1`。
    pub fn max_task_num(&self, value: i32) {
        self.set("maxTaskNum", json!(value));
    }

    /// `GeneralDownload/PreBlockNum`：`division=1`。
    pub fn pre_block_num(&self, value: i32) {
        self.set("preBlockNum", json!(value));
    }

    /// `Category/EnableCategory`：`SwitchSettingCard`。
    pub fn category_enabled(&self, value: bool) {
        self.set("isCategoryEnabled", Value::Bool(value));
    }

    /// `GeneralDownload/SpeedLimitation`：收**字节**（显示 KB/s → 字节的换算在 install 层）。
    pub fn speed_limitation(&self, bytes: i64) {
        self.set("speedLimitation", json!(bytes));
    }

    /// `GeneralDownload/AutoSpeedUp`。
    pub fn auto_speed_up(&self, value: bool) {
        self.set("autoSpeedUp", Value::Bool(value));
    }

    /// `GeneralDownload/MaxReassignSize`。
    pub fn max_reassign_size(&self, value: i64) {
        self.set("maxReassignSize", json!(value));
    }

    /// `GeneralDownload/PreserveLastModified`。
    pub fn preserve_last_modified(&self, value: bool) {
        self.set("shouldPreserveLastModified", Value::Bool(value));
    }

    /// `GeneralDownload/shouldVerifySsl`（`restart=True`）。
    pub fn verify_ssl(&self, value: bool) {
        self.set("shouldVerifySsl", Value::Bool(value));
    }

    /// `GeneralDownload/NameConflict`：收字面量（`"ask"` / `"rename"` …）。
    pub fn name_conflict(&self, value: &str) {
        self.set("nameConflict", Value::from(value));
    }

    /// `Network/ShouldUseSystemDns`。
    pub fn use_system_dns(&self, value: bool) {
        self.set("shouldUseSystemDns", Value::Bool(value));
    }

    /// `GeneralDownload/ProxyServer`：收配置字面量（`"Off"` / `"Auto"` / 拼好的 URL）。
    pub fn proxy_server(&self, value: &str) {
        self.set("proxyServer", Value::from(value));
    }

    /// `Aria2Rpc/Enabled`。
    pub fn aria2_enabled(&self, value: bool) {
        self.set("isAria2RpcEnabled", Value::Bool(value));
    }

    /// `Aria2Rpc/Port`：`division=1`，`correct()` 会夹到 1024–65535。
    pub fn aria2_rpc_port(&self, value: i64) {
        self.set("aria2RpcPort", json!(value));
    }

    /// `Aria2Rpc/Token`：`LineEditSettingCard` 逐字符写盘，`Kind::Text` 无 validator。
    pub fn aria2_rpc_token(&self, value: &str) {
        self.set("aria2RpcToken", Value::from(value));
    }

    /// `Aria2Rpc/EmulateFingerprint`。
    pub fn aria2_emulate_fingerprint(&self, value: bool) {
        self.set("aria2RpcEmulateFingerprint", Value::Bool(value));
    }

    /// `Browser/EnableBrowserExtension`。
    pub fn browser_enabled(&self, value: bool) {
        self.set("isBrowserExtensionEnabled", Value::Bool(value));
    }

    /// `Browser/EnableRaiseWindowWhenReceiveMsg`（`cfg.shouldDraftTakenDownload`）。
    pub fn browser_draft_taken(&self, value: bool) {
        self.set("shouldDraftTakenDownload", Value::Bool(value));
    }

    /// `Browser/Port`：`division=1`。
    pub fn browser_port(&self, value: i64) {
        self.set("browserExtensionPort", json!(value));
    }

    /// `Browser/BrowserExtensionPairToken`：重新生成并落盘，返回新令牌。
    ///
    /// 等价于 `browser_service.py:190` 的
    /// `cfg.set(cfg.browserExtensionPairToken, token_urlsafe(16))`；生成口径见
    /// [`gd3_core::pair_token::generate`]（22 字符）。
    pub fn regenerate_pair_token(&self) -> String {
        let token = pair_token::generate();
        self.set("browserExtensionPairToken", Value::from(token.clone()));
        token
    }

    /// `Network/ClientProfile`：收 profile 字面量（`"auto"` / `"raw"` / 家族名）。
    pub fn client_profile(&self, value: &str) {
        self.set("clientProfile", Value::from(value));
    }

    /// `Browser/UrlSchemeRegistered`。
    pub fn url_scheme_enabled(&self, value: bool) {
        self.set("isUrlSchemeRegistered", Value::Bool(value));
    }

    /// `Software/CheckUpdateAtStartUp`。
    pub fn check_update_at_startup(&self, value: bool) {
        self.set("shouldCheckUpdateAtStartup", Value::Bool(value));
    }

    /// `Software/AutoRun`。
    pub fn run_at_login(&self, value: bool) {
        self.set("shouldRunAtLogin", Value::Bool(value));
    }

    /// `Software/CloseMode`：收字面量（`"Ask"` / `"Background"` / `"Quit"`）。
    pub fn close_mode(&self, value: &str) {
        self.set("closeMode", Value::from(value));
    }

    /// `Software/ClipboardListener`。
    pub fn clipboard_listener(&self, value: bool) {
        self.set("isClipboardListenerEnabled", Value::Bool(value));
    }

    /// `Personalization/DpiScale`：收 **0–5 浮点**（显示 % → `/100` 在 install 层）。
    /// `restart=True`：Python 会弹「重启后生效」。
    pub fn dpi_scale(&self, value: f64) {
        self.set("dpiScale", json!(value));
    }

    // -----------------------------------------------------------------------
    // 动作集
    // -----------------------------------------------------------------------

    /// 把 27 个标量方法装成 [`SettingsActions`]（49 个字段全覆盖）。
    ///
    /// 每个闭包只做「一个属性名 + 一个值」；单位换算、索引→字面量、URL 拼装
    /// 都在 `install_settings_actions` 那一层（见 `lib.rs` 的同名注释）。
    ///
    /// 其余 22 个动作（列表写 18 个 + 平台副作用 4 个）调 [`SettingsWriter::defer`]：
    /// 它们要么得先读列表改一项再整表写回（删除前还有确认框），要么归平台层。
    /// 这不是静默丢弃 —— [`SettingsWriter::take_deferred`] 会把名字交出去。
    pub fn actions(self: &Rc<Self>) -> SettingsActions {
        let folder = Rc::clone(self);
        let max_task_num = Rc::clone(self);
        let pre_block_num = Rc::clone(self);
        let category_enabled = Rc::clone(self);
        let speed_limitation = Rc::clone(self);
        let auto_speed_up = Rc::clone(self);
        let max_reassign_size = Rc::clone(self);
        let preserve_last_modified = Rc::clone(self);
        let verify_ssl = Rc::clone(self);
        let name_conflict = Rc::clone(self);
        let use_system_dns = Rc::clone(self);
        let proxy_changed = Rc::clone(self);
        let aria2_enabled = Rc::clone(self);
        let aria2_rpc_port = Rc::clone(self);
        let aria2_rpc_token = Rc::clone(self);
        let aria2_emulate_fingerprint = Rc::clone(self);
        let browser_enabled = Rc::clone(self);
        let browser_draft_taken = Rc::clone(self);
        let browser_port = Rc::clone(self);
        let pair_token_regenerate = Rc::clone(self);
        let client_profile = Rc::clone(self);
        let url_scheme_enabled = Rc::clone(self);
        let check_update_at_startup = Rc::clone(self);
        let run_at_login = Rc::clone(self);
        let close_mode = Rc::clone(self);
        let clipboard_listener = Rc::clone(self);
        let dpi_scale = Rc::clone(self);

        let pair_token_copy = Rc::clone(self);
        let store_url_open = Rc::clone(self);
        let chromium_install = Rc::clone(self);
        let chromium_crx_export = Rc::clone(self);
        let headers_preset_pick = Rc::clone(self);
        let headers_preset_remove = Rc::clone(self);
        let headers_preset_add_requested = Rc::clone(self);
        let headers_preset_edit_requested = Rc::clone(self);
        let headers_preset_edit_closed = Rc::clone(self);
        let headers_preset_edit_submit = Rc::clone(self);
        let identity_preset_toggle = Rc::clone(self);
        let identity_preset_remove = Rc::clone(self);
        let identity_preset_add_requested = Rc::clone(self);
        let identity_preset_edit_requested = Rc::clone(self);
        let identity_preset_edit_closed = Rc::clone(self);
        let identity_preset_edit_submit = Rc::clone(self);
        let category_reset = Rc::clone(self);
        let category_remove = Rc::clone(self);
        let category_add_requested = Rc::clone(self);
        let category_edit_requested = Rc::clone(self);
        let category_edit_closed = Rc::clone(self);
        let category_edit_submit = Rc::clone(self);

        SettingsActions {
            folder: Box::new(move |text| folder.download_folder(text)),
            max_task_num: Box::new(move |value| max_task_num.max_task_num(value)),
            pre_block_num: Box::new(move |value| pre_block_num.pre_block_num(value)),
            category_enabled: Box::new(move |value| category_enabled.category_enabled(value)),
            speed_limitation: Box::new(move |bytes| speed_limitation.speed_limitation(bytes)),
            auto_speed_up: Box::new(move |value| auto_speed_up.auto_speed_up(value)),
            max_reassign_size: Box::new(move |value| max_reassign_size.max_reassign_size(value)),
            preserve_last_modified: Box::new(move |value| {
                preserve_last_modified.preserve_last_modified(value)
            }),
            verify_ssl: Box::new(move |value| verify_ssl.verify_ssl(value)),
            name_conflict: Box::new(move |value| name_conflict.name_conflict(value)),
            use_system_dns: Box::new(move |value| use_system_dns.use_system_dns(value)),
            proxy_changed: Box::new(move |value| proxy_changed.proxy_server(value)),
            aria2_enabled: Box::new(move |value| aria2_enabled.aria2_enabled(value)),
            aria2_rpc_port: Box::new(move |value| aria2_rpc_port.aria2_rpc_port(value)),
            aria2_rpc_token: Box::new(move |value| aria2_rpc_token.aria2_rpc_token(value)),
            aria2_emulate_fingerprint: Box::new(move |value| {
                aria2_emulate_fingerprint.aria2_emulate_fingerprint(value)
            }),
            browser_enabled: Box::new(move |value| browser_enabled.browser_enabled(value)),
            browser_draft_taken: Box::new(move |value| {
                browser_draft_taken.browser_draft_taken(value)
            }),
            browser_port: Box::new(move |value| browser_port.browser_port(value)),
            // 新令牌通过 `take_changes()` 交给宿主（`ConfigChange::value`），
            // 由它重推 `browserExtensionPairToken` 属性。
            pair_token_regenerate: Box::new(move || {
                let _ = pair_token_regenerate.regenerate_pair_token();
            }),
            client_profile: Box::new(move |value| client_profile.client_profile(value)),
            url_scheme_enabled: Box::new(move |value| url_scheme_enabled.url_scheme_enabled(value)),
            check_update_at_startup: Box::new(move |value| {
                check_update_at_startup.check_update_at_startup(value)
            }),
            run_at_login: Box::new(move |value| run_at_login.run_at_login(value)),
            close_mode: Box::new(move |value| close_mode.close_mode(value)),
            clipboard_listener: Box::new(move |value| clipboard_listener.clipboard_listener(value)),
            dpi_scale: Box::new(move |value| dpi_scale.dpi_scale(value)),

            pair_token_copy: Box::new(move || pair_token_copy.defer("copyPairToken")),
            store_url_open: Box::new(move |_| store_url_open.defer("openStoreUrl")),
            chromium_install: Box::new(move || chromium_install.defer("chromiumInstall")),
            chromium_crx_export: Box::new(move || chromium_crx_export.defer("exportCrx")),
            headers_preset_pick: Box::new(move |_| {
                headers_preset_pick.defer("headersPresetPicked")
            }),
            headers_preset_remove: Box::new(move |_| {
                headers_preset_remove.defer("headersPresetRemoveRequested")
            }),
            headers_preset_add_requested: Box::new(move || {
                headers_preset_add_requested.defer("headersPresetAddRequested")
            }),
            headers_preset_edit_requested: Box::new(move |_| {
                headers_preset_edit_requested.defer("headersPresetEditRequested")
            }),
            headers_preset_edit_closed: Box::new(move || {
                headers_preset_edit_closed.defer("headersPresetEditClosed")
            }),
            headers_preset_edit_submit: Box::new(move |_| {
                headers_preset_edit_submit.defer("headersPresetEditAccepted")
            }),
            identity_preset_toggle: Box::new(move |_, _| {
                identity_preset_toggle.defer("identityPresetToggled")
            }),
            identity_preset_remove: Box::new(move |_| {
                identity_preset_remove.defer("identityPresetRemoveRequested")
            }),
            identity_preset_add_requested: Box::new(move || {
                identity_preset_add_requested.defer("identityPresetAddRequested")
            }),
            identity_preset_edit_requested: Box::new(move |_| {
                identity_preset_edit_requested.defer("identityPresetEditRequested")
            }),
            identity_preset_edit_closed: Box::new(move || {
                identity_preset_edit_closed.defer("identityPresetEditClosed")
            }),
            identity_preset_edit_submit: Box::new(move |_| {
                identity_preset_edit_submit.defer("identityPresetEditAccepted")
            }),
            category_reset: Box::new(move || category_reset.defer("categoryResetRequested")),
            category_remove: Box::new(move |_| category_remove.defer("categoryRemoveRequested")),
            category_add_requested: Box::new(move || {
                category_add_requested.defer("categoryAddRequested")
            }),
            category_edit_requested: Box::new(move |_| {
                category_edit_requested.defer("categoryEditRequested")
            }),
            category_edit_closed: Box::new(move || {
                category_edit_closed.defer("categoryEditClosed")
            }),
            category_edit_submit: Box::new(move |_| {
                category_edit_submit.defer("categoryEditAccepted")
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    use gd3_core::settings;

    use crate::{category, headers, identity};

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "gd3-ui-settings-writer-{}-{tag}-{seq}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    /// 一个「路径在临时目录里、内存是缺省值」的 writer。
    fn writer(tag: &str) -> (PathBuf, Rc<SettingsWriter>) {
        let dir = temp_dir(tag);
        let path = dir.join("UserConfig.json");
        let store = ConfigStore::new(&path);
        (path, Rc::new(SettingsWriter::new(store)))
    }

    fn read(path: &std::path::Path) -> String {
        std::fs::read_to_string(path).expect("读回写出的配置")
    }

    fn attrs(changes: &[ConfigChange]) -> Vec<&'static str> {
        changes.iter().map(|change| change.attr).collect()
    }

    #[test]
    fn a_scalar_action_writes_the_whole_file() {
        let (path, writer) = writer("scalar");
        let actions = writer.actions();
        (actions.max_task_num)(7);

        let changes = writer.take_changes();
        assert_eq!(attrs(&changes), ["maxTaskNum"]);
        assert_eq!(changes[0].group, "GeneralDownload");
        assert_eq!(changes[0].key, "MaxTaskNum");
        assert_eq!(changes[0].previous, json!(3));
        assert_eq!(changes[0].value, json!(7));
        assert_eq!(changes[0].stored, json!(7));
        assert!(changes[0].emitted);
        assert!(!changes[0].restart);

        // 期望值由 `settings::write` 独立算出，不复用被测实现的 snapshot()。
        let mut expected = settings::default_config();
        settings::write(&mut expected, "maxTaskNum", &json!(7)).expect("已知 attr");
        assert_eq!(read(&path), expected.to_json_text());
    }

    #[test]
    fn an_unchanged_value_leaves_the_file_alone() {
        let (path, writer) = writer("unchanged");
        let actions = writer.actions();
        // 缺省就是 3：`if item.value == value: return`。
        (actions.max_task_num)(3);
        assert!(writer.take_changes().is_empty());
        assert!(writer.take_failures().is_empty());
        assert!(!path.exists(), "相等早退不落盘，文件不该被创建");
    }

    #[test]
    fn an_out_of_range_value_is_clamped_before_it_lands() {
        let (_path, writer) = writer("clamp");
        let actions = writer.actions();
        // Python: `RangeValidator(1, 10).correct(99)` → 10。
        (actions.max_task_num)(99);
        // Python: `RangeValidator(1024, 65535).correct(80)` → 1024。
        (actions.aria2_rpc_port)(80);

        let changes = writer.take_changes();
        assert_eq!(attrs(&changes), ["maxTaskNum", "aria2RpcPort"]);
        assert_eq!(changes[0].value, json!(10));
        assert_eq!(changes[1].value, json!(1024));
        assert_eq!(writer.get("aria2RpcPort"), Some(json!(1024)));
    }

    #[test]
    fn the_speed_action_takes_bytes() {
        let (_path, writer) = writer("speed");
        let actions = writer.actions();
        (actions.speed_limitation)(2 * 1024 * 1024);
        assert_eq!(writer.get("speedLimitation"), Some(json!(2_097_152)));
        // 低于下限也夹到 1024（`RangeValidator(1024, 104857600)`）。
        (actions.speed_limitation)(1);
        assert_eq!(writer.get("speedLimitation"), Some(json!(1024)));
    }

    #[test]
    fn text_actions_store_the_literal() {
        let (path, writer) = writer("text");
        let actions = writer.actions();
        (actions.folder)("/tmp/gd3-writer-downloads");
        (actions.name_conflict)("overwrite");
        (actions.close_mode)("Quit");
        (actions.client_profile)("chrome");
        (actions.proxy_changed)("Off");
        (actions.aria2_rpc_token)("s3cret");

        let changes = writer.take_changes();
        assert_eq!(
            attrs(&changes),
            [
                "downloadFolder",
                "nameConflict",
                "closeMode",
                "clientProfile",
                "proxyServer",
                "aria2RpcToken",
            ]
        );
        assert_eq!(
            changes.iter().map(|c| c.value.clone()).collect::<Vec<_>>(),
            [
                json!("/tmp/gd3-writer-downloads"),
                json!("overwrite"),
                json!("Quit"),
                json!("chrome"),
                json!("Off"),
                json!("s3cret"),
            ]
        );
        // 令牌是 `Kind::Text`，无 validator：原样落盘，不 trim。
        assert!(read(&path).contains("\"Token\": \"s3cret\""));
    }

    #[test]
    fn the_pair_token_action_reports_a_fresh_token() {
        let (_path, writer) = writer("token");
        let token = writer.regenerate_pair_token();
        // `token_urlsafe(16)` → 22 个字符。
        assert_eq!(token.len(), 22);
        assert_eq!(writer.get("browserExtensionPairToken"), Some(json!(token)));

        let changes = writer.take_changes();
        assert_eq!(attrs(&changes), ["browserExtensionPairToken"]);
        assert_eq!(changes[0].previous, json!(""));
        assert_eq!(changes[0].value, json!(token));
        assert!(changes[0].emitted);
    }

    #[test]
    fn the_zero_dpi_scale_is_already_the_default() {
        let (path, writer) = writer("dpi-zero");
        let actions = writer.actions();
        // 缺省落盘是 int `0`，而 `0 == 0.0` 为真 → 相等早退。
        (actions.dpi_scale)(0.0);
        assert!(writer.take_changes().is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn the_restart_actions_are_flagged() {
        let (_path, writer) = writer("restart");
        let actions = writer.actions();
        (actions.verify_ssl)(true);
        (actions.dpi_scale)(1.5);
        (actions.max_task_num)(5);

        let changes = writer.take_changes();
        assert_eq!(
            attrs(&changes),
            ["shouldVerifySsl", "dpiScale", "maxTaskNum"]
        );
        assert!(changes[0].restart, "shouldVerifySsl 的 restart=True");
        assert!(changes[1].restart, "dpiScale 的 restart=True");
        assert!(!changes[2].restart);
        // `max(0, 1.5)` 保留浮点形态 → 落盘 `1.5`。
        assert_eq!(changes[1].value, json!(1.5));
    }

    #[test]
    fn a_save_failure_is_collected_and_the_value_stays_in_memory() {
        let dir = temp_dir("failure");
        // 把父目录占成一个普通文件：`mkdir(parents=True)` 必然失败。
        let blocked = dir.join("blocked");
        std::fs::write(&blocked, b"not a directory").expect("占位文件");
        let store = ConfigStore::new(blocked.join("UserConfig.json"));
        let writer = Rc::new(SettingsWriter::new(store));
        let actions = writer.actions();
        (actions.max_task_num)(9);

        // Python 会在这里抛 `OSError`；这里改成收集。
        assert!(writer.take_changes().is_empty());
        let failures = writer.take_failures();
        assert_eq!(failures.len(), 1);
        assert!(matches!(failures[0], ConfigStoreError::Save(_)));
        // 「先改值再 save()」：内存里已经是新值。
        assert_eq!(writer.get("maxTaskNum"), Some(json!(9)));
    }

    #[test]
    fn every_scalar_action_reaches_the_store() {
        let (_path, writer) = writer("wired");
        let actions = writer.actions();
        (actions.folder)("/tmp/gd3-wired");
        (actions.max_task_num)(4);
        (actions.pre_block_num)(9);
        (actions.category_enabled)(true);
        (actions.speed_limitation)(1024 * 1024);
        (actions.auto_speed_up)(false);
        (actions.max_reassign_size)(1024);
        (actions.preserve_last_modified)(true);
        (actions.verify_ssl)(true);
        (actions.name_conflict)("overwrite");
        (actions.use_system_dns)(false);
        (actions.proxy_changed)("Off");
        (actions.aria2_enabled)(true);
        (actions.aria2_rpc_port)(16801);
        (actions.aria2_rpc_token)("abc");
        (actions.aria2_emulate_fingerprint)(true);
        (actions.browser_enabled)(false);
        (actions.browser_draft_taken)(true);
        (actions.browser_port)(14371);
        (actions.pair_token_regenerate)();
        (actions.client_profile)("chrome");
        (actions.url_scheme_enabled)(false);
        (actions.check_update_at_startup)(false);
        (actions.run_at_login)(true);
        (actions.close_mode)("Quit");
        (actions.clipboard_listener)(false);
        (actions.dpi_scale)(1.5);

        assert_eq!(
            attrs(&writer.take_changes()),
            [
                "downloadFolder",
                "maxTaskNum",
                "preBlockNum",
                "isCategoryEnabled",
                "speedLimitation",
                "autoSpeedUp",
                "maxReassignSize",
                "shouldPreserveLastModified",
                "shouldVerifySsl",
                "nameConflict",
                "shouldUseSystemDns",
                "proxyServer",
                "isAria2RpcEnabled",
                "aria2RpcPort",
                "aria2RpcToken",
                "aria2RpcEmulateFingerprint",
                "isBrowserExtensionEnabled",
                "shouldDraftTakenDownload",
                "browserExtensionPort",
                "browserExtensionPairToken",
                "clientProfile",
                "isUrlSchemeRegistered",
                "shouldCheckUpdateAtStartup",
                "shouldRunAtLogin",
                "closeMode",
                "isClipboardListenerEnabled",
                "dpiScale",
            ]
        );
        assert!(
            writer.take_failures().is_empty(),
            "27 个属性名都要能查到 spec"
        );
        assert!(writer.take_deferred().is_empty(), "标量动作不进待办清单");
    }

    #[test]
    fn the_deferred_actions_name_what_is_still_missing() {
        let (path, writer) = writer("deferred");
        let actions = writer.actions();
        (actions.pair_token_copy)();
        (actions.store_url_open)("https://microsoftedge.microsoft.com/addons".into());
        (actions.chromium_install)();
        (actions.chromium_crx_export)();
        (actions.headers_preset_pick)(0);
        (actions.headers_preset_remove)(1);
        (actions.headers_preset_add_requested)();
        (actions.headers_preset_edit_requested)(0);
        (actions.headers_preset_edit_closed)();
        (actions.headers_preset_edit_submit)((
            0,
            headers::HeaderPreset {
                name: "x".into(),
                headers: Vec::new(),
            },
        ));
        (actions.identity_preset_toggle)(0, false);
        (actions.identity_preset_remove)(1);
        (actions.identity_preset_add_requested)();
        (actions.identity_preset_edit_requested)(0);
        (actions.identity_preset_edit_closed)();
        (actions.identity_preset_edit_submit)((
            0,
            identity::IdentityPreset {
                name: "n".into(),
                client_profile: "raw".into(),
                user_agent: "ua".into(),
                hosts: Vec::new(),
                is_enabled: true,
            },
        ));
        (actions.category_reset)();
        (actions.category_remove)("cat_video".into());
        (actions.category_add_requested)();
        (actions.category_edit_requested)("cat_video".into());
        (actions.category_edit_closed)();
        (actions.category_edit_submit)(category::CategoryRule {
            category_id: "cat_video".into(),
            name: "视频".into(),
            icon: "VIDEO".into(),
            extensions: Vec::new(),
            folder: None,
        });

        assert_eq!(
            writer.take_deferred(),
            [
                "copyPairToken",
                "openStoreUrl",
                "chromiumInstall",
                "exportCrx",
                "headersPresetPicked",
                "headersPresetRemoveRequested",
                "headersPresetAddRequested",
                "headersPresetEditRequested",
                "headersPresetEditClosed",
                "headersPresetEditAccepted",
                "identityPresetToggled",
                "identityPresetRemoveRequested",
                "identityPresetAddRequested",
                "identityPresetEditRequested",
                "identityPresetEditClosed",
                "identityPresetEditAccepted",
                "categoryResetRequested",
                "categoryRemoveRequested",
                "categoryAddRequested",
                "categoryEditRequested",
                "categoryEditClosed",
                "categoryEditAccepted",
            ]
        );
        // 待办清单里的动作一个都不碰文件，也不进改动列表。
        assert!(writer.take_changes().is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn the_config_snapshot_follows_the_writes() {
        let (_path, writer) = writer("snapshot");
        let actions = writer.actions();
        (actions.max_task_num)(6);
        (actions.close_mode)("Background");
        assert_eq!(
            settings::read(&writer.config(), "maxTaskNum"),
            Some(json!(6))
        );
        assert_eq!(
            settings::read(&writer.config(), "closeMode"),
            Some(json!("Background"))
        );
    }

    #[test]
    fn adopt_takes_a_token_that_the_engine_generated() {
        let (_path, writer) = writer("adopt");
        // `Engine::refresh_services` → `BrowserService::apply_config` 在令牌为空时
        // 会重新生成一个（`browser_service.py:190` 的 `cfg.set` 自己落盘）。
        let mut config = writer.config();
        config.set(
            "Browser",
            "BrowserExtensionPairToken",
            json!("token-from-the-engine"),
        );

        let change = writer.adopt(&config).expect("应当吞回文件");
        assert_eq!(change.attr, "browserExtensionPairToken");
        assert_eq!(
            writer.get("browserExtensionPairToken"),
            Some(json!("token-from-the-engine"))
        );
        assert!(writer.adopt(&config).is_none(), "第二次没有变化");
        assert_eq!(attrs(&writer.take_changes()), ["browserExtensionPairToken"]);
    }

    #[test]
    fn changes_are_taken_once() {
        let (_path, writer) = writer("take");
        let actions = writer.actions();
        (actions.max_task_num)(5);
        assert_eq!(writer.take_changes().len(), 1);
        assert!(writer.take_changes().is_empty());
        assert!(writer.take_failures().is_empty());
        assert!(writer.take_deferred().is_empty());
    }

    #[test]
    fn open_leaves_a_missing_file_alone() {
        let dir = temp_dir("open");
        let path = dir.join("UserConfig.json");
        let writer = SettingsWriter::open(&path).expect("缺文件也要成功");
        assert_eq!(writer.path(), path);
        assert_eq!(writer.get("maxTaskNum"), Some(json!(3)));
        assert!(!path.exists(), "`qconfig.load` 不写盘");
    }

    #[test]
    fn an_unknown_attribute_never_comes_out_of_the_actions() {
        // 27 个方法全走常量属性名，唯一能造出 UnknownAttribute 的路径是
        // `ConfigStore::set` 的手写调用；这里钉住「写错名字会进 failures 而不是 panic」。
        let (_path, writer) = writer("unknown");
        writer.set("noSuchAttribute", json!(1));
        let failures = writer.take_failures();
        assert_eq!(failures.len(), 1);
        assert!(matches!(
            &failures[0],
            ConfigStoreError::UnknownAttribute(attr) if attr == "noSuchAttribute"
        ));
        assert!(writer.take_changes().is_empty());
    }
}
