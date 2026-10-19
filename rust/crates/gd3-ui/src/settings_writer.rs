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
//! ## 列表写回（第 119 步）
//!
//! 九个动作不是「一个属性名 + 一个值」，而要**读列表 → 改一项 → 整表写回**：
//!
//! | 动作 | Python 源 | 写回 |
//! |---|---|---|
//! | [`SettingsWriter::headers_preset_pick`] | `setting_cards.py:715-717` | `currentHeadersPreset` |
//! | [`SettingsWriter::headers_preset_remove`] | `setting_cards.py:738-775` | `headersPresets`，必要时 `currentHeadersPreset` 回退一格 |
//! | [`SettingsWriter::headers_preset_submit`] | `setting_cards.py:719-736` | 追加（`index == -1`）/ 替换 |
//! | [`SettingsWriter::identity_preset_toggle`] | `setting_cards.py:588-593` | 复制该行、改 `isEnabled`、整表写回 |
//! | [`SettingsWriter::identity_preset_remove`] | `setting_cards.py:595-614` | 删一项、整表写回（**不动**当前索引） |
//! | [`SettingsWriter::identity_preset_submit`] | `setting_cards.py:566-586` | 追加（`index == -1`）/ 替换，替换时继承 `isEnabled` |
//! | [`SettingsWriter::category_reset`] | `category_settings.py:147-148` | 八预设整表写回 |
//! | [`SettingsWriter::category_remove`] | `category_settings.py:144-145` | 命中才写（Python 用列表推导，重复 id 一起删） |
//! | [`SettingsWriter::category_submit`] | `category_settings.py:129-142` | 按 `categoryId` 命中 → 替换；否则追加 |
//!
//! 三条照抄 Python 的细节：
//!
//! - **`currentHeadersPreset` 存的是原始索引，不是夹紧后的**：`_onRemoveClicked`
//!   读 `cfg.currentHeadersPreset.value`（`:756`），所以 `current = 5`、列表只剩 3 项时
//!   删第 0 项会把 5 写成 4；读侧 `currentHeadersPresetIndex()` 再夹成 `len - 1`。
//! - **列表读写走 `Value` 而不是强类型结构**：Python 是 `list(cfg.x.value)` 复制 dict、
//!   改一项、整表写回，未知键与键序都留着。`{**preset, "isEnabled": checked}`
//!   （`:592`）也不重排已有键 —— `Map::insert` 对已存在的键同语义。
//! - **`headers_preset_remove` 有 `len > 1` 前置条件**：Python 靠 `canRemove` 把删除按钮
//!   置灰（`:705` `canRemove=len(presets) > 1`），这里把同一条件写成函数级守卫 ——
//!   列表被删空时 `correct()` 会补回「默认」预设，等于删了又长出来。
//!
//! `category_*` 三个动作反过来走 [`crate::category`] 的强类型 `CategoryRule`：Python 的
//! `CategoryService._save()` 写的是 `[asdict(c) ...]`（`category_service.py:144-146`），
//! 未知键在 `_load`（`:136-143`）就丢了，所以这里与 Python 同样「过一遍 dataclass」。
//!
//! ## 不在动作集里的动作
//!
//! 两类动作不走 [`SettingsActions`]，因为它们**不写 cfg**：
//!
//! | 动作 | 归属 |
//! |---|---|
//! | 九个对话框开合（`*AddRequested` / `*EditRequested` / `*EditClosed`） | [`crate::settings_dialogs`]（第 120 步） |
//! | 五个扩展卡动作（复制/重生成令牌、商店链接、一键安装、导出 CRX） | [`crate::extension_actions`]（第 122 步） |
//!
//! 第 119–120 步用 `defer()` 记「还没接」的名字；第 122 步接完最后四个平台副作用后
//! `defer` 机制整体删除 —— 动作集里不再有「记一笔就算」的洞。
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
use crate::{category, headers, identity};

/// `headers::HeaderPreset` → `HeadersPresetEditDialog.preset()` 的形状
/// （`headers_preset_edit.py:58-62`）：`{"name": str, "headers": {name: value}}`。
///
/// 键序 `name` 在前：Python 的 dict 字面量就是这个顺序，落盘时由
/// `json.dumps` 原样写出。
fn header_preset_value(preset: &headers::HeaderPreset) -> Value {
    let mut headers_map = serde_json::Map::new();
    for (name, value) in &preset.headers {
        headers_map.insert(name.clone(), Value::from(value.clone()));
    }
    let mut row = serde_json::Map::new();
    row.insert("name".to_string(), Value::from(preset.name.clone()));
    row.insert("headers".to_string(), Value::Object(headers_map));
    Value::Object(row)
}

/// `identity::IdentityPreset` → `PresetEditDialog.preset()` 的五个键
/// （`preset_edit.py:113-123`，键序照抄）。
fn identity_preset_value(preset: &identity::IdentityPreset) -> Value {
    let mut row = serde_json::Map::new();
    row.insert("name".to_string(), Value::from(preset.name.clone()));
    row.insert(
        "clientProfile".to_string(),
        Value::from(preset.client_profile.clone()),
    );
    row.insert(
        "userAgent".to_string(),
        Value::from(preset.user_agent.clone()),
    );
    row.insert(
        "hosts".to_string(),
        Value::Array(preset.hosts.iter().cloned().map(Value::from).collect()),
    );
    row.insert("isEnabled".to_string(), Value::Bool(preset.is_enabled));
    Value::Object(row)
}

/// 设置页动作的落盘端。
///
/// 内部是 [`RefCell`]：Slint 的回调都是 `Fn`（不可变捕获），而 `ConfigStore::set` 要
/// `&mut self`。单线程 UI 上不存在跨线程争用，`RefCell` 足够。
pub struct SettingsWriter {
    store: RefCell<ConfigStore>,
    changes: RefCell<Vec<ConfigChange>>,
    failures: RefCell<Vec<ConfigStoreError>>,
}

impl SettingsWriter {
    /// 接管一份已加载的 [`ConfigStore`]。
    pub fn new(store: ConfigStore) -> Self {
        Self {
            store: RefCell::new(store),
            changes: RefCell::new(Vec::new()),
            failures: RefCell::new(Vec::new()),
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
    // 列表动作（第 119 步）
    // -----------------------------------------------------------------------

    /// 读一项列表的**逻辑值**（已过 `deserialize` + `correct`）。
    ///
    /// 缺键/类型不符 → 空表：42 项恒有值，这条分支只挡手改坏的配置。
    fn list(&self, attr: &str) -> Vec<Value> {
        match self.get(attr) {
            Some(Value::Array(items)) => items,
            _ => Vec::new(),
        }
    }

    /// 整表写回一项列表（`correct()` / `serialize()` / 整文件回写仍在 [`ConfigStore::set`]）。
    fn set_list(&self, attr: &'static str, items: Vec<Value>) {
        self.set(attr, Value::Array(items));
    }

    /// `Network/CurrentHeadersPreset`：`setting_cards.py:715-717` 的 `_onPick`。
    ///
    /// Python 原样写索引，由 `RangeValidator(0, 99)`（`cfg.py:332-334`）夹紧；读侧
    /// `currentHeadersPresetIndex()` 再夹到 `len - 1` —— 两段夹紧都不在这里。
    pub fn headers_preset_pick(&self, index: i64) {
        self.set("currentHeadersPreset", json!(index));
    }

    /// `Network/HeadersPresets`：`setting_cards.py:738-775` 的 `_onRemoveClicked`。
    ///
    /// 顺序照抄：先写列表，再读 `currentHeadersPreset`，`index <= current` 时回退一格
    /// （下限 0）。删除按钮在只剩一项时置灰（`:705`），同一条件在此守卫 —— 列表被删空
    /// 时 `correct()` 会补回「默认」预设，等于删了又长出来。
    pub fn headers_preset_remove(&self, index: i64) {
        let mut items = self.list("headersPresets");
        if index < 0 || index as usize >= items.len() || items.len() <= 1 {
            return;
        }
        items.remove(index as usize);
        self.set_list("headersPresets", items);
        let current = self
            .get("currentHeadersPreset")
            .and_then(|value| value.as_i64())
            .unwrap_or(0);
        if index <= current {
            self.set("currentHeadersPreset", json!((current - 1).max(0)));
        }
    }

    /// `Network/HeadersPresets`：`setting_cards.py:719-736` 的添加/编辑收尾。
    ///
    /// `index == -1` = 新建（Slint 的 `headersEditIndex` 用 `-1` 表示新建，见
    /// `ui/settings_page.slint:892`），否则替换该行。越界忽略：Python 在
    /// `_onEditClicked`（`:729`）里直接索引会抛 `IndexError`，Rust 侧容错。
    pub fn headers_preset_submit(&self, index: i64, preset: &headers::HeaderPreset) {
        let mut items = self.list("headersPresets");
        let value = header_preset_value(preset);
        if index < 0 {
            items.push(value);
        } else {
            let Some(slot) = items.get_mut(index as usize) else {
                return;
            };
            *slot = value;
        }
        self.set_list("headersPresets", items);
    }

    /// `Network/IdentityPresets`：`setting_cards.py:588-593` 的 `_onToggled`。
    ///
    /// `presets[index] = {**presets[index], "isEnabled": checked}`：复制该行、只改一个
    /// 键。`Map::insert` 对已存在的键保留原位置，与 Python 的 dict 展开一致。
    pub fn identity_preset_toggle(&self, index: i64, checked: bool) {
        if index < 0 {
            return;
        }
        let mut items = self.list("identityPresets");
        let Some(row) = items.get_mut(index as usize).and_then(Value::as_object_mut) else {
            return;
        };
        row.insert("isEnabled".to_string(), Value::Bool(checked));
        self.set_list("identityPresets", items);
    }

    /// `Network/IdentityPresets`：`setting_cards.py:595-614` 的 `_onRemoveClicked`。
    ///
    /// Python 只有越界守卫（`index < 0 or index >= len(presets)` → `return`），**没有**
    /// 「至少留一项」的限制：删空之后 `IdentityPresetListValidator.correct` 保留空表。
    pub fn identity_preset_remove(&self, index: i64) {
        let mut items = self.list("identityPresets");
        if index < 0 || index as usize >= items.len() {
            return;
        }
        items.remove(index as usize);
        self.set_list("identityPresets", items);
    }

    /// `Network/IdentityPresets`：`setting_cards.py:566-586` 的添加/编辑收尾。
    ///
    /// `index == -1` = 新建（Slint 的 `identityEditIndex` 缺省 `-1`）。**替换分支继承
    /// 既有 `isEnabled`**：Python 的 `PresetEditDialog.preset()` 在编辑态把原值抄回来
    /// （`preset_edit.py:121-122`），而 Slint 草稿恒带 `true` —— 不继承会悄悄启用停用预设。
    pub fn identity_preset_submit(&self, index: i64, preset: &identity::IdentityPreset) {
        let mut items = self.list("identityPresets");
        let mut value = identity_preset_value(preset);
        if index < 0 {
            items.push(value);
        } else {
            let Some(slot) = items.get_mut(index as usize) else {
                return;
            };
            let enabled = slot
                .get("isEnabled")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            if let Some(row) = value.as_object_mut() {
                row.insert("isEnabled".to_string(), Value::Bool(enabled));
            }
            *slot = value;
        }
        self.set_list("identityPresets", items);
    }

    /// `Category/CategoryRules`：`category_settings.py:147-148` 的「恢复默认」
    /// → `CategoryService.reset`（`category_service.py:212-214`）。
    pub fn category_reset(&self) {
        self.write_categories(category::reset_categories());
    }

    /// `Category/CategoryRules`：`category_settings.py:144-145` 的 `_onRemoveClicked`
    /// → `CategoryService.removeCategory`（`category_service.py:206-210`）。
    ///
    /// 没命中就不写盘（Python 比长度）。
    pub fn category_remove(&self, category_id: &str) {
        let mut rules = self.categories();
        if category::remove_category(&mut rules, category_id) {
            self.write_categories(rules);
        }
    }

    /// `Category/CategoryRules`：`category_settings.py:129-142` 的添加/编辑收尾。
    ///
    /// Python 是两条路：`_onAddClicked` → `addCategory`（无条件追加）；
    /// `_onEditClicked` 先 `categoryById`、查不到就 `return`，命中才 `updateCategory`。
    /// 这里按「`categoryId` 是否命中」合并成一条：命中 → 替换，否则追加。
    ///
    /// 新分类的 `categoryId` 来自 `Category.categoryId` 的 `default_factory`
    /// （`cat_{uuid4().hex}`，`category_service.py:103`）；对话框回发的草稿 id 是空串
    /// （`categoryEditId` 缺省 `""`，见 `ui/appwindow.slint:109`），所以这里补生成。
    pub fn category_submit(&self, rule: &category::CategoryRule) {
        let mut next = rule.clone();
        if next.category_id.is_empty() {
            next.category_id = category::new_category_id();
        }
        let mut rules = self.categories();
        if !category::update_category(&mut rules, next.clone()) {
            category::add_category(&mut rules, next);
        }
        self.write_categories(rules);
    }

    /// 当前分类表：`CategoryService._load`（`category_service.py:136-143`）口径 ——
    /// `categoryRules` 为空 → 八预设。
    pub fn categories(&self) -> Vec<category::CategoryRule> {
        let raw = self.get("categoryRules").map(|value| value.to_string());
        category::parse_rules(raw.as_deref())
    }

    /// `CategoryService._save`（`category_service.py:144-146`）：
    /// `cfg.set(cfg.categoryRules, [asdict(c) ...])`。
    ///
    /// 走 [`category::rules_to_json`] 再解回 `Value`：那份串已经被单测钉成 Python
    /// `json.dumps(ensure_ascii=False)` 口径，而 [`ConfigStore::set`] 会再序列化一次
    /// （`Kind::JsonList` → `py_json_dumps`，同口径），往返不改变字节。
    fn write_categories(&self, rules: Vec<category::CategoryRule>) {
        let text = category::rules_to_json(&rules);
        let value = serde_json::from_str::<Value>(&text).unwrap_or(Value::Array(Vec::new()));
        self.set("categoryRules", value);
    }

    // -----------------------------------------------------------------------
    // 动作集
    // -----------------------------------------------------------------------

    /// 把 27 个标量方法 + 9 个列表方法装成 [`SettingsActions`]（35 个字段全覆盖）。
    ///
    /// 每个闭包只做「一个属性名 + 一个值」；单位换算、索引→字面量、URL 拼装
    /// 都在 `install_settings_actions` 那一层（见 `lib.rs` 的同名注释）。
    ///
    /// 第 122 步起动作集里**没有平台副作用**：扩展卡那五个动作（含唯一的 cfg 写
    /// `regenerate_pair_token`）归 [`crate::extension_actions`]，它直接持 [`SettingsWriter`]
    /// 调方法，不经过 [`SettingsActions`]。
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
        let client_profile = Rc::clone(self);
        let url_scheme_enabled = Rc::clone(self);
        let check_update_at_startup = Rc::clone(self);
        let run_at_login = Rc::clone(self);
        let close_mode = Rc::clone(self);
        let clipboard_listener = Rc::clone(self);
        let dpi_scale = Rc::clone(self);

        let headers_preset_pick = Rc::clone(self);
        let headers_preset_remove = Rc::clone(self);
        let headers_preset_edit_submit = Rc::clone(self);
        let identity_preset_toggle = Rc::clone(self);
        let identity_preset_remove = Rc::clone(self);
        let identity_preset_edit_submit = Rc::clone(self);
        let category_reset = Rc::clone(self);
        let category_remove = Rc::clone(self);
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
            client_profile: Box::new(move |value| client_profile.client_profile(value)),
            url_scheme_enabled: Box::new(move |value| url_scheme_enabled.url_scheme_enabled(value)),
            check_update_at_startup: Box::new(move |value| {
                check_update_at_startup.check_update_at_startup(value)
            }),
            run_at_login: Box::new(move |value| run_at_login.run_at_login(value)),
            close_mode: Box::new(move |value| close_mode.close_mode(value)),
            clipboard_listener: Box::new(move |value| clipboard_listener.clipboard_listener(value)),
            dpi_scale: Box::new(move |value| dpi_scale.dpi_scale(value)),
            headers_preset_pick: Box::new(move |index| {
                headers_preset_pick.headers_preset_pick(index)
            }),
            headers_preset_remove: Box::new(move |index| {
                headers_preset_remove.headers_preset_remove(index)
            }),
            headers_preset_edit_submit: Box::new(move |(index, preset)| {
                headers_preset_edit_submit.headers_preset_submit(index, &preset)
            }),
            identity_preset_toggle: Box::new(move |index, checked| {
                identity_preset_toggle.identity_preset_toggle(index, checked)
            }),
            identity_preset_remove: Box::new(move |index| {
                identity_preset_remove.identity_preset_remove(index)
            }),
            identity_preset_edit_submit: Box::new(move |(index, preset)| {
                identity_preset_edit_submit.identity_preset_submit(index, &preset)
            }),
            category_reset: Box::new(move || category_reset.category_reset()),
            category_remove: Box::new(move |id| category_remove.category_remove(&id)),
            category_edit_submit: Box::new(move |rule| category_edit_submit.category_submit(&rule)),
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
        // 第 122 步起 `pair_token_regenerate` 不在动作集里：入口在
        // `extension_actions`（它要顺带复制剪贴板），这里直接调方法验落盘。
        writer.regenerate_pair_token();
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
    }

    fn header_preset(name: &str, pairs: &[(&str, &str)]) -> headers::HeaderPreset {
        headers::HeaderPreset {
            name: name.to_string(),
            headers: pairs
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        }
    }

    fn identity_preset(name: &str, enabled: bool) -> identity::IdentityPreset {
        identity::IdentityPreset {
            name: name.to_string(),
            client_profile: "chrome".to_string(),
            user_agent: "ua".to_string(),
            hosts: vec!["example.com".to_string()],
            is_enabled: enabled,
        }
    }

    fn rule(category_id: &str, name: &str) -> category::CategoryRule {
        category::CategoryRule {
            category_id: category_id.to_string(),
            name: name.to_string(),
            icon: "VIDEO".to_string(),
            extensions: vec!["mp4".to_string()],
            folder: None,
        }
    }

    /// 某项列表的逻辑值（写完立刻读回，等价于 Python 侧的 `cfg.x.value`）。
    fn list_of(writer: &Rc<SettingsWriter>, attr: &str) -> Vec<Value> {
        match writer.get(attr) {
            Some(Value::Array(items)) => items,
            other => panic!("{attr} 应当是数组：{other:?}"),
        }
    }

    #[test]
    fn picking_a_headers_preset_writes_the_raw_index() {
        let (_path, writer) = writer("pick");
        writer.headers_preset_pick(2);
        let changes = writer.take_changes();
        assert_eq!(attrs(&changes), ["currentHeadersPreset"]);
        assert_eq!(changes[0].value, json!(2));

        // `RangeValidator(0, 99)`：Python 侧超界会被夹到 99。
        writer.headers_preset_pick(120);
        assert_eq!(writer.take_changes()[0].value, json!(99));
    }

    #[test]
    fn removing_a_headers_preset_pulls_the_current_index_back_one() {
        let (_path, writer) = writer("headers-remove");
        writer.headers_preset_submit(-1, &header_preset("A", &[("X-Test", "1")]));
        writer.headers_preset_pick(1);
        assert_eq!(list_of(&writer, "headersPresets").len(), 2);

        writer.headers_preset_remove(0);
        let items = list_of(&writer, "headersPresets");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["name"], json!("A"));
        assert_eq!(items[0]["headers"], json!({"X-Test": "1"}));
        assert_eq!(writer.get("currentHeadersPreset"), Some(json!(0)));
    }

    #[test]
    fn removing_a_later_headers_preset_keeps_the_current_index() {
        let (_path, writer) = writer("headers-keep");
        writer.headers_preset_submit(-1, &header_preset("A", &[]));
        writer.headers_preset_submit(-1, &header_preset("B", &[]));
        writer.headers_preset_pick(0);

        writer.headers_preset_remove(2);
        assert_eq!(list_of(&writer, "headersPresets").len(), 2);
        assert_eq!(writer.get("currentHeadersPreset"), Some(json!(0)));
    }

    #[test]
    fn the_last_headers_preset_cannot_be_removed() {
        let (_path, writer) = writer("headers-last");
        assert_eq!(list_of(&writer, "headersPresets").len(), 1);
        writer.headers_preset_remove(0);
        // 删空会让 `correct()` 补回「默认」预设 —— 等于删了又长出来，所以直接不写。
        assert!(writer.take_changes().is_empty());
        assert_eq!(list_of(&writer, "headersPresets").len(), 1);
    }

    #[test]
    fn removing_a_headers_preset_ignores_out_of_range_indexes() {
        let (_path, writer) = writer("headers-range");
        writer.headers_preset_submit(-1, &header_preset("A", &[]));
        writer.take_changes();

        writer.headers_preset_remove(-1);
        writer.headers_preset_remove(9);
        assert!(writer.take_changes().is_empty());
        assert_eq!(list_of(&writer, "headersPresets").len(), 2);
    }

    #[test]
    fn submitting_a_headers_preset_appends_or_replaces() {
        let (_path, writer) = writer("headers-submit");
        writer.headers_preset_submit(-1, &header_preset("A", &[("X-Test", "1")]));
        let items = list_of(&writer, "headersPresets");
        assert_eq!(items.len(), 2);
        assert_eq!(items[1]["name"], json!("A"));
        // `headers_preset_edit.py:58-62`：只有 `name` 与 `headers` 两个键。
        assert_eq!(
            items[1].as_object().map(|row| row.len()),
            Some(2),
            "落盘形状只能有 name/headers"
        );

        writer.headers_preset_submit(0, &header_preset("B", &[("Y-Test", "2")]));
        let items = list_of(&writer, "headersPresets");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["name"], json!("B"));
        assert_eq!(items[0]["headers"], json!({"Y-Test": "2"}));
    }

    #[test]
    fn submitting_a_headers_preset_ignores_out_of_range_indexes() {
        let (_path, writer) = writer("headers-submit-range");
        writer.headers_preset_submit(5, &header_preset("A", &[]));
        assert!(writer.take_changes().is_empty());
        assert_eq!(list_of(&writer, "headersPresets").len(), 1);
    }

    #[test]
    fn toggling_an_identity_preset_keeps_the_other_keys() {
        let (_path, writer) = writer("identity-toggle");
        let before = list_of(&writer, "identityPresets");
        assert_eq!(before.len(), 1);

        writer.identity_preset_toggle(0, false);
        let after = list_of(&writer, "identityPresets");
        assert_eq!(after.len(), 1);
        assert_eq!(after[0]["isEnabled"], json!(false));
        // `{**preset, "isEnabled": checked}`：其余键原样，键数不变。
        assert_eq!(after[0]["name"], before[0]["name"]);
        assert_eq!(
            after[0].as_object().map(|row| row.len()),
            before[0].as_object().map(|row| row.len())
        );

        writer.identity_preset_toggle(3, true);
        assert_eq!(
            list_of(&writer, "identityPresets")[0]["isEnabled"],
            json!(false)
        );
    }

    #[test]
    fn removing_an_identity_preset_can_empty_the_list() {
        let (_path, writer) = writer("identity-remove");
        writer.identity_preset_remove(0);
        assert_eq!(list_of(&writer, "identityPresets"), Vec::<Value>::new());
        // `IdentityPresetListValidator` 没有回落：空表就是空表。
        writer.identity_preset_remove(0);
        assert_eq!(list_of(&writer, "identityPresets"), Vec::<Value>::new());
    }

    #[test]
    fn submitting_an_identity_preset_inherits_the_stored_enabled_flag() {
        let (_path, writer) = writer("identity-submit");
        writer.identity_preset_toggle(0, false);

        // 草稿恒带 `is_enabled: true`（Slint 侧没有这个输入），编辑态要继承原值。
        writer.identity_preset_submit(0, &identity_preset("改过的名字", true));
        let items = list_of(&writer, "identityPresets");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["name"], json!("改过的名字"));
        assert_eq!(items[0]["isEnabled"], json!(false));
        assert_eq!(items[0]["hosts"], json!(["example.com"]));

        // 新建走草稿自己的值。
        writer.identity_preset_submit(-1, &identity_preset("新的", false));
        let items = list_of(&writer, "identityPresets");
        assert_eq!(items.len(), 2);
        assert_eq!(items[1]["isEnabled"], json!(false));
        assert_eq!(items[1]["clientProfile"], json!("chrome"));
    }

    #[test]
    fn resetting_categories_writes_the_eight_presets() {
        let (_path, writer) = writer("category-reset");
        assert_eq!(list_of(&writer, "categoryRules"), Vec::<Value>::new());

        writer.category_reset();
        let items = list_of(&writer, "categoryRules");
        assert_eq!(items.len(), 8);
        assert_eq!(items[0]["categoryId"], json!("cat_video"));
        assert_eq!(items[0]["name"], json!("视频"));
        // `asdict()` 会带上 `folder`，八预设的取值是 `{default}/X`。
        assert_eq!(items[0]["folder"], json!("{default}/Video"));
        assert_eq!(
            items[0].as_object().map(|row| row.len()),
            Some(5),
            "asdict 只有 categoryId/name/icon/extensions/folder 五个键"
        );
    }

    #[test]
    fn removing_a_category_only_writes_on_a_hit() {
        let (_path, writer) = writer("category-remove");
        writer.category_reset();
        writer.take_changes();

        writer.category_remove("cat_nope");
        assert!(writer.take_changes().is_empty());
        assert_eq!(list_of(&writer, "categoryRules").len(), 8);

        writer.category_remove("cat_video");
        assert_eq!(attrs(&writer.take_changes()), ["categoryRules"]);
        let items = list_of(&writer, "categoryRules");
        assert_eq!(items.len(), 7);
        assert!(
            items
                .iter()
                .all(|item| item["categoryId"] != json!("cat_video"))
        );
    }

    #[test]
    fn submitting_a_category_replaces_a_known_id() {
        let (_path, writer) = writer("category-update");
        writer.category_reset();
        writer.take_changes();

        writer.category_submit(&rule("cat_video", "影片"));
        let items = list_of(&writer, "categoryRules");
        assert_eq!(items.len(), 8);
        assert_eq!(items[0]["categoryId"], json!("cat_video"));
        assert_eq!(items[0]["name"], json!("影片"));
        assert_eq!(items[0]["extensions"], json!(["mp4"]));
    }

    #[test]
    fn submitting_a_category_generates_an_id_for_new_rules() {
        let (_path, writer) = writer("category-add");
        writer.category_reset();
        writer.take_changes();

        writer.category_submit(&rule("", "自定义"));
        let items = list_of(&writer, "categoryRules");
        assert_eq!(items.len(), 9);
        let id = items[8]["categoryId"].as_str().expect("新分类要有 id");
        assert_eq!(id.len(), 36, "cat_ + uuid4().hex（32 位）");
        let hex = id.strip_prefix("cat_").expect("前缀");
        assert_eq!(hex.len(), 32);
        assert!(
            hex.chars()
                .all(|ch| ch.is_ascii_hexdigit() && !ch.is_uppercase())
        );
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
