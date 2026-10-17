// Ghost-Downloader-3 Slint UI 的 Rust 侧入口。
//
// Phase 7a：只装最小窗口骨架 + 无头实例化冒烟（cargo test 可在
// 无显示器的构建机上跑，走 software renderer + 自制 Platform）。
// 逐页从 Qt 迁过来时在 ui/ 增加组件、在本 crate 挂回调。
//
// slint 1.18.1 API 实证锚点（registry i-slint-core-1.18.1）：
// - platform.rs: `set_platform(platform: Box<dyn Platform + 'static>) -> Result<(), SetPlatformError>`
//   （1.18 没有 `is_using_platform_set`；`SetPlatformError::AlreadySet` 即「已装好」信号）
// - Platform 唯一必需方法：`create_window_adapter -> Result<Rc<dyn WindowAdapter>, PlatformError>`
// - MinimalSoftwareWindow::new(repaint_buffer_type: RepaintBufferType) -> Rc<Self>（自身已包 Rc）

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PlatformError, WindowAdapter};
use std::rc::Rc;

pub mod close_mode;
pub mod conflict;
pub mod filter;
mod format;
pub mod profile;
pub mod proxy;
pub mod row;
pub mod sort;
pub mod speed;

slint::include_modules!();

/// 无头平台：不依赖任何显示服务（测试/裸构建机都跑得起来）。
struct HeadlessPlatform;

impl Platform for HeadlessPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
    }
}

/// 无头实例化 AppWindow（进程内 set_platform 只成功一次，重复=AlreadySet）。
pub fn create_headless() -> AppWindow {
    // AlreadySet 说明平台已装好（比如同进程第二个实例），直接复用即可。
    let _ = slint::platform::set_platform(Box::new(HeadlessPlatform));
    let window = AppWindow::new().expect("AppWindow 实例化失败");
    window.show().expect("show 失败");
    window
}

/// Rust 侧推送任务行到 UI（Phase 7b 契约）。
///
/// 行文案（statusText/progressText）由调用方负责；这里只做 model 装配。
/// （slint 1.18 生成的模型属性 setter 是无返回值的，见 target build 产物实证。）
pub fn set_task_rows(window: &AppWindow, rows: Vec<TaskRow>) {
    let model = slint::ModelRc::new(slint::VecModel::from(rows));
    window.set_tasks(model);
}

/// 推流路径：`Vec<TaskRecord>` → TaskRow 文案 → model 装配。
///
/// `is_seeding` 由调用方给定（isSeeding 是运行时属性、不在记录里），
/// 这里只做「快照 → 文案 → 行」的桥。
pub fn push_task_records(
    window: &AppWindow,
    records: &[gd3_core::task::TaskRecord],
    is_seeding: impl Fn(&gd3_core::task::TaskRecord) -> bool,
) {
    let rows: Vec<TaskRow> = records
        .iter()
        .map(|record| row::to_task_row_with(record, is_seeding(record)))
        .collect();
    set_task_rows(window, rows);
}

/// 筛选后的推流：按 `filter::TaskFilter` 过滤 records 再成行推送
/// （顺路接通 `set_searchText`/`set_filterMode` 的 UI 态）。
pub fn refresh_task_rows(
    window: &AppWindow,
    records: &[gd3_core::task::TaskRecord],
    filter: filter::TaskFilter<'_>,
    sort_field: sort::SortField,
    sort_ascending: bool,
    waiting_order: &[String],
    is_seeding: impl Fn(&gd3_core::task::TaskRecord) -> bool,
) {
    // 筛选（保序）→ 排序（镜像 task_page.py::_refreshList 末段）→ 成行。
    // waitingOrder 由调用方给（taskService.waitingOrder()）。
    let mut refs = filter::filtered(records, filter);
    sort::sort_refs(&mut refs, sort_field, sort_ascending, waiting_order);
    let rows: Vec<TaskRow> = refs
        .iter()
        .map(|record| row::to_task_row_with(record, is_seeding(record)))
        .collect();
    set_task_rows(window, rows);
}

/// 分类筛选下拉（ComboBox）的 [string] 模型装配（filter::menu_names
/// 已含「全部分类」表头；current-index 由调用方回写）。
///
/// slint 生成名保留 camelCase：set_categoryNames / get_categoryIndex。
pub fn set_category_names(window: &AppWindow, names: Vec<String>) {
    let names: Vec<slint::SharedString> =
        names.into_iter().map(slint::SharedString::from).collect();
    let model = slint::ModelRc::new(slint::VecModel::from(names));
    window.set_categoryNames(model);
}

/// 设置页（第 32 步先验四键）装配：UserConfig typed 读 → 窗口属性。
/// 键缺省/非数字取 Python 缺省口径（cfg.py），不猜。
pub fn push_settings(
    window: &AppWindow,
    config: &gd3_core::config::UserConfig,
    default_download_folder: &str,
) {
    window.set_downloadFolder(
        config
            .get_str("GeneralDownload", "DownloadFolder")
            .unwrap_or(default_download_folder)
            .to_string()
            .into(),
    );
    window.set_maxTaskNum(config.get_i64("GeneralDownload", "MaxTaskNum").unwrap_or(3) as i32);
    window.set_preBlockNum(
        config
            .get_i64("GeneralDownload", "PreBlockNum")
            .unwrap_or(8) as i32,
    );
    window.set_categoryEnabled(
        config
            .get_bool("Category", "EnableCategory")
            .unwrap_or(false),
    );
    // 第 33 步：限速值。配置存字节（GeneralDownload/SpeedLimitation，
    // 缺省 4194304=cfg.py），窗口属性用显示口径 KB/s（1/1024 换算
    // 在 speed.rs 镜像层）。
    window.set_speedLimitationKb(speed::to_display_kb(
        config
            .get_i64("GeneralDownload", "SpeedLimitation")
            .unwrap_or(speed::DEFAULT_BYTES),
    ) as i32);
    // 第 34 步：GeneralDownload 组剩余简版键（键名/缺省 cfg.py 实证）；
    // NameConflict 镜像 ComboBoxSettingCard——存字面量、UI 发索引
    // （映射归 conflict.rs）。
    window.set_autoSpeedUp(
        config
            .get_bool("GeneralDownload", "AutoSpeedUp")
            .unwrap_or(true),
    );
    window.set_maxReassignSize(
        config
            .get_i64("GeneralDownload", "MaxReassignSize")
            .unwrap_or(512) as i32,
    );
    window.set_preserveLastModified(
        config
            .get_bool("GeneralDownload", "PreserveLastModified")
            .unwrap_or(false),
    );
    window.set_verifySsl(
        config
            .get_bool("GeneralDownload", "shouldVerifySsl")
            .unwrap_or(false),
    );
    window.set_nameConflictIndex(conflict::index_of_option(
        config
            .get_str("GeneralDownload", "NameConflict")
            .unwrap_or(conflict::DEFAULT_OPTION),
    ) as i32);
    window.set_useSystemDns(
        config
            .get_bool("Network", "ShouldUseSystemDns")
            .unwrap_or(true),
    );
    // 第 35 步：ProxyServer（cfg.py GeneralDownload 缺省 "Auto"）。
    // choice 由 proxy.rs 镜像派生；自定义四字段仅自定义模式时解析
    // 推送（Auto/Off 模式下 Python 端卡内自定义字段保持控件旧值，
    // 进程启动即缺省空）。
    let proxy_value = config
        .get_str("GeneralDownload", "ProxyServer")
        .unwrap_or(proxy::VALUE_DEFAULT);
    window.set_proxyChoice(proxy::choice_of_value(proxy_value));
    if proxy::choice_of_value(proxy_value) == proxy::CHOICE_CUSTOM {
        let fields = proxy::parse_fields(proxy_value);
        window.set_proxyProtocolIndex(fields.protocol_index);
        window.set_proxyHost(fields.host.as_str().into());
        window.set_proxyPort(fields.port.as_str().into());
        window.set_proxyUser(fields.user.as_str().into());
        window.set_proxyPass(fields.pass.as_str().into());
    }
    // 第 36 步：Aria2 RPC 组（cfg.py:277-280 实证：Enabled 缺省 F /
    // Port 缺省 16800 范围 1024-65535 / Token 缺省 "" /
    // EmulateFingerprint 缺省 F）。
    window.set_aria2Enabled(config.get_bool("Aria2Rpc", "Enabled").unwrap_or(false));
    window.set_aria2RpcPort(config.get_i64("Aria2Rpc", "Port").unwrap_or(16_800) as i32);
    window.set_aria2RpcToken(
        config
            .get_str("Aria2Rpc", "Token")
            .unwrap_or("")
            .to_string()
            .into(),
    );
    window.set_aria2EmulateFingerprint(
        config
            .get_bool("Aria2Rpc", "EmulateFingerprint")
            .unwrap_or(false),
    );
    // 第 37 步：浏览器扩展组简版三键（cfg.py:268-272 实证：
    // EnableBrowserExtension 缺省 T / EnableRaiseWindowWhenReceiveMsg
    // 缺省 F——UI 文案「接管下载时进入草稿模式」同一键 / Port 缺省
    // 14370 范围 1024-65535）。
    window.set_browserEnabled(
        config
            .get_bool("Browser", "EnableBrowserExtension")
            .unwrap_or(true),
    );
    window.set_browserDraftTaken(
        config
            .get_bool("Browser", "EnableRaiseWindowWhenReceiveMsg")
            .unwrap_or(false),
    );
    window.set_browserPort(config.get_i64("Browser", "Port").unwrap_or(14_370) as i32);
    // 第 40 步：配对令牌（cfg.py:269 BrowserExtensionPairToken 缺省
    // ""。镜像 PrimaryPushSettingCard setContent——启动时空令牌自动
    // 补生成留引擎桥接步，先推当前值）。
    window.set_pairToken(
        config
            .get_str("Browser", "BrowserExtensionPairToken")
            .unwrap_or("")
            .into(),
    );
    // 第 41 步：模拟身份（cfg.py Network/ClientProfile 缺省 "auto"；
    // 镜像 DropDownPushButton setText(toProfileLabel(value)) 的索引
    // 口径，映射在 profile.rs 镜像层）。
    window.set_clientProfileIndex(profile::index_of(
        config
            .get_str("Network", "ClientProfile")
            .unwrap_or(profile::DEFAULT_PROFILE),
    ) as i32);
    // 第 38 步：关联 urlScheme 静态卡 + 软件组简版（cfg.py 实证：
    // Browser/UrlSchemeRegistered 缺省 T（sys!=darwin 显示）；
    // Software/CheckUpdateAtStartUp 缺省 T / AutoRun 缺省 F（应用
    // 生效归桥接步）/ CloseMode 缺省 "Ask" / ClipboardListener 缺省
    // T——CloseMode 镜像 ComboBoxSettingCard 存枚举值字面量）。
    window.set_urlSchemeEnabled(
        config
            .get_bool("Browser", "UrlSchemeRegistered")
            .unwrap_or(true),
    );
    window.set_checkUpdateAtStartup(
        config
            .get_bool("Software", "CheckUpdateAtStartUp")
            .unwrap_or(true),
    );
    window.set_runAtLogin(config.get_bool("Software", "AutoRun").unwrap_or(false));
    window.set_closeModeIndex(close_mode::index_of_option(
        config
            .get_str("Software", "CloseMode")
            .unwrap_or(close_mode::DEFAULT_OPTION),
    ) as i32);
    window.set_clipboardListener(
        config
            .get_bool("Software", "ClipboardListener")
            .unwrap_or(true),
    );
    // 第 39 步：DpiScale 界面缩放（cfg.py:296 缺省 0「自动」；配置
    // 0-5 浮点 → 显示 % *100 round 防浮点尾差；磁盘串 python
    // str(float) 口径镜像在 gd3-core get_f64/set_f64）。
    window.set_uiScalePercent(
        (config.get_f64("Personalization", "DpiScale").unwrap_or(0.0) * 100.0).round() as i32,
    );
}

/// 设置页动作集：控件变化 → 调用方 typed set + 落盘。
pub struct SettingsActions {
    pub folder: Box<dyn Fn(&str)>,
    pub max_task_num: Box<dyn Fn(i32)>,
    pub pre_block_num: Box<dyn Fn(i32)>,
    pub category_enabled: Box<dyn Fn(bool)>,
    /// 限速值动作收**配置口径字节**（显示 KB/s → 字节换算在 install
    /// 层完成，与其它动作统一「动作收配置单位」）。
    pub speed_limitation: Box<dyn Fn(i64)>,
    // 第 34 步：GeneralDownload 组剩余简版动作（统一收配置单位；
    // NameConflict 收选项字面量，索引→字面量换算在 install 层）。
    pub auto_speed_up: Box<dyn Fn(bool)>,
    pub max_reassign_size: Box<dyn Fn(i64)>,
    pub preserve_last_modified: Box<dyn Fn(bool)>,
    pub verify_ssl: Box<dyn Fn(bool)>,
    pub name_conflict: Box<dyn Fn(&str)>,
    pub use_system_dns: Box<dyn Fn(bool)>,
    // 第 35 步：代理动作收**配置口径字面量**——Off/Auto 原样，自定义
    // 模式收拼装并校验后的 URL（拼装/校验在 install 层 proxy.rs 镜像
    // ProxyValidator；无效 URL 不落盘，镜像 leaveEvent 语义）。
    pub proxy_changed: Box<dyn Fn(&str)>,
    // 第 36 步：Aria2 RPC 组动作（端口收 i64 配置单位 division=1、
    // 令牌收原样串——Python 端 cfg.set 不 trim）。
    pub aria2_enabled: Box<dyn Fn(bool)>,
    pub aria2_rpc_port: Box<dyn Fn(i64)>,
    pub aria2_rpc_token: Box<dyn Fn(&str)>,
    pub aria2_emulate_fingerprint: Box<dyn Fn(bool)>,
    // 第 37 步：浏览器扩展组动作（端口收 i64 配置单位 division=1）。
    pub browser_enabled: Box<dyn Fn(bool)>,
    pub browser_draft_taken: Box<dyn Fn(bool)>,
    pub browser_port: Box<dyn Fn(i64)>,
    // 第 40 步：浏览器扩展动作卡（复制令牌/重新生成/一键安装/导出
    // CRX 收 ()，商店链接收 URL 原样串）。剪贴板写入、扩展解包、
    // 拉起浏览器归平台/引擎桥接步；重新生成的 cfg 写口径由
    // gd3-core pair_token::generate 兜（token_urlsafe(16) 等价）。
    pub pair_token_copy: Box<dyn Fn()>,
    pub pair_token_regenerate: Box<dyn Fn()>,
    pub store_url_open: Box<dyn Fn(String)>,
    pub chromium_install: Box<dyn Fn()>,
    pub chromium_crx_export: Box<dyn Fn()>,
    // 第 41 步：模拟身份动作收 profile 字面量（auto/raw/家族），索引
    // →字面量换算在 install 层 profile.rs；引擎 resolve 在 gd3-net。
    pub client_profile: Box<dyn Fn(&str)>,
    // 第 38 步：关联 + 软件组动作（CloseMode 收字面量，索引→字面量
    // 换算在 install 层 close_mode.rs）。
    pub url_scheme_enabled: Box<dyn Fn(bool)>,
    pub check_update_at_startup: Box<dyn Fn(bool)>,
    pub run_at_login: Box<dyn Fn(bool)>,
    pub close_mode: Box<dyn Fn(&str)>,
    pub clipboard_listener: Box<dyn Fn(bool)>,
    // 第 39 步：界面缩放动作收**配置口径 0-5 浮点**（显示 % → 配置
    // /100 换算在 install 层，对齐「动作收配置单位」惯例；restart=True
    // 生效重启归引擎桥接步）。
    pub dpi_scale: Box<dyn Fn(f64)>,
}

pub fn install_settings_actions(window: &AppWindow, actions: std::rc::Rc<SettingsActions>) {
    let folder = actions.clone();
    window.on_downloadFolderChanged(move |text| (folder.folder)(&text));
    let max_task_num = actions.clone();
    window.on_maxTaskNumChanged(move |value| (max_task_num.max_task_num)(value));
    let pre_block_num = actions.clone();
    window.on_preBlockNumChanged(move |value| (pre_block_num.pre_block_num)(value));
    let category = actions.clone();
    window.on_categoryEnabledChanged(move |value| (category.category_enabled)(value));
    // 第 33 步：SpinBox 发显示口径 KB/s；动作收配置口径字节
    // （Python 端 cfg.set(int(value/division)) 的镜像，换算在
    // speed.rs from_display_kb）。
    let speed = actions.clone();
    window.on_speedLimitationChanged(move |kb| {
        (speed.speed_limitation)(speed::from_display_kb(kb as i64))
    });
    // 第 34 步：GeneralDownload 简版动作注册（NameConflict 由 install
    // 层把索引换成字面量，动作统一收配置单位）。
    let auto_speed_up = actions.clone();
    window.on_autoSpeedUpChanged(move |v| (auto_speed_up.auto_speed_up)(v));
    let max_reassign_size = actions.clone();
    window.on_maxReassignSizeChanged(move |v| (max_reassign_size.max_reassign_size)(v as i64));
    let preserve_last_modified = actions.clone();
    window.on_preserveLastModifiedChanged(move |v| {
        (preserve_last_modified.preserve_last_modified)(v)
    });
    let verify_ssl = actions.clone();
    window.on_verifySslChanged(move |v| (verify_ssl.verify_ssl)(v));
    let name_conflict = actions.clone();
    window.on_nameConflictChanged(move |i| {
        (name_conflict.name_conflict)(conflict::option_of_index(i as i64))
    });
    let use_system_dns = actions.clone();
    window.on_useSystemDnsChanged(move |v| (use_system_dns.use_system_dns)(v));
    // 第 35 步：代理。Slint 发 choice+字段；install 层换算成配置
    // 字面量——Off/Auto 直取，自定义模式拼 URL 并以 proxy::validate
    // 校验，无效 URL 不调用动作（Python 端 leaveEvent 仅在有效时
    // cfg.set 的镜像）。
    let proxy = actions.clone();
    let win = window.clone_strong();
    let win_fields = window.clone_strong();
    let proxy_fields = proxy.clone();
    window.on_proxyChoiceChanged(move |choice| {
        let value = match choice {
            proxy::CHOICE_OFF => proxy::VALUE_OFF.to_string(),
            proxy::CHOICE_AUTO => proxy::VALUE_AUTO.to_string(),
            _ => {
                let protocol = proxy::PROTOCOLS
                    .get(win.get_proxyProtocolIndex() as usize)
                    .copied()
                    .unwrap_or(proxy::PROTOCOLS[0]);
                let url = proxy::build_url(
                    protocol,
                    &win.get_proxyHost(),
                    &win.get_proxyPort(),
                    &win.get_proxyUser(),
                    &win.get_proxyPass(),
                );
                if !proxy::validate(&url) {
                    return;
                }
                url
            }
        };
        (proxy.proxy_changed)(&value);
    });
    window.on_proxyFieldsChanged(move |protocol_index, host, port, user, pass| {
        let protocol = proxy::PROTOCOLS
            .get(protocol_index as usize)
            .copied()
            .unwrap_or(proxy::PROTOCOLS[0]);
        let url = proxy::build_url(protocol, &host, &port, &user, &pass);
        if win_fields.get_proxyChoice() == proxy::CHOICE_CUSTOM && proxy::validate(&url) {
            (proxy_fields.proxy_changed)(&url);
        }
    });
    // 第 36 步：Aria2 RPC 组注册（对齐留引擎桥接步）。
    let aria2_enabled = actions.clone();
    window.on_aria2EnabledChanged(move |v| (aria2_enabled.aria2_enabled)(v));
    let aria2_rpc_port = actions.clone();
    window.on_aria2RpcPortChanged(move |v| (aria2_rpc_port.aria2_rpc_port)(v as i64));
    let aria2_rpc_token = actions.clone();
    window.on_aria2RpcTokenChanged(move |t| (aria2_rpc_token.aria2_rpc_token)(&t));
    let aria2_emulate = actions.clone();
    window.on_aria2EmulateFingerprintChanged(move |v| (aria2_emulate.aria2_emulate_fingerprint)(v));
    // 第 37 步：浏览器扩展组注册（WS 监听真启停等对齐留引擎桥接步）。
    let browser_enabled = actions.clone();
    window.on_browserEnabledChanged(move |v| (browser_enabled.browser_enabled)(v));
    let browser_draft = actions.clone();
    window.on_browserDraftTakenChanged(move |v| (browser_draft.browser_draft_taken)(v));
    let browser_port = actions.clone();
    window.on_browserPortChanged(move |v| (browser_port.browser_port)(v as i64));
    // 第 40 步：浏览器扩展动作卡注册（对齐留调用方/引擎桥接步）。
    let pair_token_copy = actions.clone();
    window.on_copyPairToken(move || (pair_token_copy.pair_token_copy)());
    let pair_token_regen = actions.clone();
    window.on_regeneratePairToken(move || (pair_token_regen.pair_token_regenerate)());
    let store_url = actions.clone();
    window.on_openStoreUrl(move |url| (store_url.store_url_open)(url.to_string()));
    let chromium_install = actions.clone();
    window.on_chromiumInstall(move || (chromium_install.chromium_install)());
    let chromium_crx = actions.clone();
    window.on_exportCrx(move || (chromium_crx.chromium_crx_export)());
    // 第 41 步：模拟身份注册（索引 → 字面量，镜像 _onPick）。
    let client_profile = actions.clone();
    window.on_clientProfileChanged(move |i| {
        (client_profile.client_profile)(profile::value_of(i as i64))
    });
    // 第 38 步：关联 + 软件组注册（urlScheme 协议注册真生效/
    // 开机启动注册归引擎桥接步）。
    let url_scheme = actions.clone();
    window.on_urlSchemeEnabledChanged(move |v| (url_scheme.url_scheme_enabled)(v));
    let check_update = actions.clone();
    window.on_checkUpdateAtStartupChanged(move |v| (check_update.check_update_at_startup)(v));
    let run_at_login = actions.clone();
    window.on_runAtLoginChanged(move |v| (run_at_login.run_at_login)(v));
    let close_mode = actions.clone();
    window.on_closeModeChanged(move |i| {
        (close_mode.close_mode)(close_mode::option_of_index(i as i64))
    });
    let clipboard = actions.clone();
    window.on_clipboardListenerChanged(move |v| (clipboard.clipboard_listener)(v));
    // 第 39 步：界面缩放（镜像 Python leaveEvent cfg.set(int(spinBox
    // .value()/division))，division=1/100 显示% → 配置值）。
    let dpi_scale = actions.clone();
    window.on_uiScaleChanged(move |percent| (dpi_scale.dpi_scale)(percent as f64 / 100.0));
}
/// Rust 侧动作集：Slint 回调 → 调用方桥接（task_service/queue 等）。
///
/// 分层：Slint 只发 taskId/路由字面量；真正的暂停/删除/筛选语义都在
/// 调用方，由 install_task_actions 注册到 AppWindow。
pub struct TaskActions {
    pub search: Box<dyn Fn(&str)>,
    pub filter_mode: Box<dyn Fn(&str)>,
    pub category_selected: Box<dyn Fn(i32)>,
    pub sort_field: Box<dyn Fn(i32)>,
    pub sort_order: Box<dyn Fn(i32)>,
    pub pause: Box<dyn Fn(&str)>,
    pub delete: Box<dyn Fn(&str)>,
}

/// 把动作集注册到 AppWindow 的五个回调上（on_searchChanged 等生成名
/// 保留 slint 原命名，见 build 产物 appwindow.rs:9572-9608）。
pub fn install_task_actions(window: &AppWindow, actions: std::rc::Rc<TaskActions>) {
    let search = actions.clone();
    window.on_searchChanged(move |text| (search.search)(&text));
    let filter_mode = actions.clone();
    window.on_filterModeChanged(move |route| (filter_mode.filter_mode)(&route));
    let category_selected = actions.clone();
    window.on_categoryFilterChanged(move |index| (category_selected.category_selected)(index));
    let sort_field = actions.clone();
    window.on_sortFieldChanged(move |index| (sort_field.sort_field)(index));
    let sort_order = actions.clone();
    window.on_sortOrderChanged(move |index| (sort_order.sort_order)(index));
    let pause = actions.clone();
    window.on_taskPause(move |task_id| (pause.pause)(&task_id));
    let delete = actions.clone();
    window.on_taskDelete(move |task_id| (delete.delete)(&task_id));
}

#[cfg(test)]
mod smoke {
    use slint::Model;
    /// 无头实例化：组件树的属性可读写、状态文本可镜像给 Rust 侧。
    #[test]
    fn appwindow_instantiates_headless_and_rust_side_sets_status() {
        let window = crate::create_headless();
        assert_eq!(window.get_statusText(), "Ghost-Downloader-3 (Slint)");
        window.set_statusText("headless smoke".into());
        assert_eq!(window.get_statusText(), "headless smoke");
    }

    /// 任务列表行模型：Rust 侧推送 → Slint model 逐行可读。
    #[test]
    fn task_page_rows_round_trip_headless() {
        let window = crate::create_headless();
        crate::set_task_rows(
            &window,
            vec![
                crate::TaskRow {
                    taskId: "tsk_1".into(),
                    name: "a.bin".into(),
                    statusText: "等待中".into(),
                    progressText: "0.00 B / --".into(),
                    status: "WAITING".into(),
                    progress: 0.0,
                },
                crate::TaskRow {
                    taskId: "tsk_2".into(),
                    name: "b.iso".into(),
                    statusText: "正在下载".into(),
                    progressText: "1.00 MB / 4.00 MB".into(),
                    status: "RUNNING".into(),
                    progress: 25.0,
                },
            ],
        );
        let model = window.get_tasks();
        assert_eq!(model.row_count(), 2);
        let first = model.row_data(0).expect("首行缺失");
        assert_eq!(first.taskId, "tsk_1");
        assert_eq!(first.name, "a.bin");
        assert_eq!(first.statusText, "等待中");
        assert_eq!(first.status, "WAITING");
        assert_eq!(first.progress, 0.0);
        let second = model.row_data(1).expect("次行缺失");
        assert_eq!(second.taskId, "tsk_2");
        assert_eq!(second.statusText, "正在下载");
    }

    /// 端到端推流：TaskRecord（磁盘同态 JSON 构造）→ push_task_records
    /// → Slint model 逐行核对文案。
    #[test]
    fn push_task_records_shapes_records_into_rows_headless() {
        use gd3_core::task::TaskRecord;
        use serde_json::json;

        let records: Vec<TaskRecord> = serde_json::from_value(json!([
            {
                "name": "a.bin", "url": "https://example/1", "packId": "pack",
                "taskId": "tsk_a", "status": "RUNNING", "fileSize": 4096,
                "steps": [
                    {"stepIndex": 0, "progress": 30, "speed": 512, "receivedBytes": 384},
                    {"stepIndex": 1, "progress": 70, "speed": 512, "receivedBytes": 128},
                ],
            },
            {
                "name": "b.iso", "url": "https://example/2", "packId": "pack",
                "taskId": "tsk_b", "status": "COMPLETED", "fileSize": 1048576,
            },
        ]))
        .expect("TaskRecord 数组反序列化失败");
        let window = crate::create_headless();
        crate::push_task_records(&window, &records, |record| record.task_id == "tsk_b");
        let model = window.get_tasks();
        assert_eq!(model.row_count(), 2);
        let first = model.row_data(0).expect("首行缺失");
        assert_eq!(first.taskId, "tsk_a");
        assert_eq!(first.statusText, "1.00 KB/s · 512.00 B / 4.00 KB · 3s");
        assert_eq!(first.progressText, "512.00 B/4.00 KB");
        assert_eq!(first.status, "RUNNING");
        assert_eq!(first.progress, 50.0);
        let second = model.row_data(1).expect("次行缺失");
        assert_eq!(second.statusText, "做种中");
        assert_eq!(second.progressText, "1.00 MB");
    }

    /// 筛选推流：refresh_task_rows 过滤后仅剩选中状态的行（语义由
    /// filter.rs 纯变换测试锁定，这里锁「过滤 → 成行 → 装配」通路）。
    #[test]
    fn refresh_task_rows_applies_filter_headless() {
        use gd3_core::task::TaskRecord;

        use serde_json::json;
        let records: Vec<TaskRecord> = serde_json::from_value(json!([
            {
                "name": "run.bin", "url": "https://e/1", "packId": "p",
                "taskId": "t1", "status": "RUNNING", "fileSize": 2048,
                "steps": [{"stepIndex": 0, "progress": 10, "speed": 0, "receivedBytes": 8}],
            },
            {
                "name": "done.iso", "url": "https://e/2", "packId": "p",
                "taskId": "t2", "status": "COMPLETED", "fileSize": 1024,
            },
        ]))
        .expect("反序列化失败");
        let window = crate::create_headless();

        // ACTIVE：只剩 RUNNING 行。
        crate::refresh_task_rows(
            &window,
            &records,
            crate::filter::TaskFilter {
                mode: crate::filter::FilterMode::Active,
                category: "",
                search: "",
            },
            crate::sort::SortField::CreatedAt,
            false,
            &[],
            |_| false,
        );
        let model = window.get_tasks();
        assert_eq!(model.row_count(), 1);
        assert_eq!(model.row_data(0).expect("缺失").name, "run.bin");

        // 搜索不命中 → 空列表（UI 侧空态文案在后续步接）。
        crate::refresh_task_rows(
            &window,
            &records,
            crate::filter::TaskFilter {
                mode: crate::filter::FilterMode::All,
                category: "",
                search: "zzz",
            },
            crate::sort::SortField::CreatedAt,
            false,
            &[],
            |_| false,
        );
        assert_eq!(window.get_tasks().row_count(), 0);
    }

    /// 回调注册冒烟：install_task_actions 把四个动作挂上不 panic
    /// （生成名 on_searchChanged 等保留 slint 命名）。
    #[test]
    fn install_task_actions_registers_handlers_headless() {
        use std::cell::RefCell;
        use std::rc::Rc;

        let window = crate::create_headless();
        let hits: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let actions = Rc::new(crate::TaskActions {
            search: Box::new({
                let h = hits.clone();
                move |s| h.borrow_mut().push(s.into())
            }),
            filter_mode: Box::new(|_| {}),
            category_selected: Box::new(|_| {}),
            sort_field: Box::new(|_| {}),
            sort_order: Box::new(|_| {}),
            pause: Box::new(|_| {}),
            delete: Box::new(|_| {}),
        });
        crate::install_task_actions(&window, actions);
        // 无头环境无法模拟点按；注册本身即通路（真实触发屏上联调）。
        assert!(hits.borrow().is_empty());
    }

    /// 分类筛选 UI：menu_names 装配 [string] 模型 + categoryIndex
    /// 属性可回写（选中态由调用方管理，ComboBox 绑定印证连通）。
    #[test]
    fn set_category_names_and_index_round_trip_headless() {
        use crate::filter::{CategoryRef, menu_names};

        let categories = vec![
            CategoryRef {
                category_id: "cat_video".into(),
                name: "视频".into(),
            },
            CategoryRef {
                category_id: "cat_audio".into(),
                name: "音频".into(),
            },
        ];
        let names = menu_names(&categories);
        assert_eq!(names.first().map(String::as_str), Some("全部分类"));
        let window = crate::create_headless();
        crate::set_category_names(&window, names);
        let model = window.get_categoryNames();
        assert_eq!(model.row_count(), 3);
        assert_eq!(model.row_data(1).as_deref(), Some("视频"));
        window.set_categoryIndex(2);
        assert_eq!(window.get_categoryIndex(), 2);
    }

    /// 设置页装配（第 32 步四键 + 第 33 步限速）：UserConfig → 窗口
    /// 属性缺省容错与 install 注册冒烟（含 KB/s→字节回调换算验证）。
    #[test]
    fn settings_page_push_and_install_headless() {
        use gd3_core::config::UserConfig;
        use std::cell::RefCell;
        use std::rc::Rc;

        let mut config = UserConfig::new();
        config.set_str("GeneralDownload", "DownloadFolder", "/data/ghost");
        config.set_i64("GeneralDownload", "MaxTaskNum", 5);
        config.set_bool("Category", "EnableCategory", true);
        config.set_i64("GeneralDownload", "SpeedLimitation", 8_388_608);
        config.set_bool("GeneralDownload", "AutoSpeedUp", false);
        config.set_i64("GeneralDownload", "MaxReassignSize", 1024);
        config.set_bool("GeneralDownload", "PreserveLastModified", true);
        config.set_bool("GeneralDownload", "shouldVerifySsl", true);
        config.set_str("GeneralDownload", "NameConflict", "ask");
        config.set_bool("Network", "ShouldUseSystemDns", false);
        config.set_str(
            "GeneralDownload",
            "ProxyServer",
            "socks5://u:p@1.2.3.4:1080",
        );
        config.set_bool("Aria2Rpc", "Enabled", true);
        config.set_i64("Aria2Rpc", "Port", 10800);
        config.set_str("Aria2Rpc", "Token", "secret");
        config.set_bool("Aria2Rpc", "EmulateFingerprint", true);
        config.set_bool("Browser", "EnableBrowserExtension", false);
        config.set_bool("Browser", "EnableRaiseWindowWhenReceiveMsg", true);
        config.set_i64("Browser", "Port", 15370);
        config.set_bool("Browser", "UrlSchemeRegistered", false);
        config.set_bool("Software", "CheckUpdateAtStartUp", false);
        config.set_bool("Software", "AutoRun", true);
        config.set_str("Software", "CloseMode", "Quit");
        config.set_bool("Software", "ClipboardListener", false);
        config.set_f64("Personalization", "DpiScale", 1.25);
        config.set_str("Browser", "BrowserExtensionPairToken", "p41rt0kenal");
        config.set_str("Network", "ClientProfile", "firefox-android");
        let window = crate::create_headless();
        crate::push_settings(&window, &config, "/default/download");
        assert_eq!(window.get_downloadFolder(), "/data/ghost");
        assert_eq!(window.get_maxTaskNum(), 5);
        // PreBlockNum 缺键 → Python 缺省 8。
        assert_eq!(window.get_preBlockNum(), 8);
        assert!(window.get_categoryEnabled());
        // 第 33 步：限速值字节 → 显示 KB/s（8388608/1024=8192）。
        assert_eq!(window.get_speedLimitationKb(), 8192);
        // 第 34 步：有值推送（AutoSpeedUp 取 False / MaxReassignSize
        // 直读 / NameConflict="ask"→索引 2 / DNS 走 Network 组）。
        assert!(!window.get_autoSpeedUp());
        assert_eq!(window.get_maxReassignSize(), 1024);
        assert!(window.get_preserveLastModified());
        assert!(window.get_verifySsl());
        assert_eq!(window.get_nameConflictIndex(), 2);
        assert!(!window.get_useSystemDns());
        // 第 35 步：有值推送——自定义代理 URL 解析（choice=2、协议
        // socks5=索引 1、凭据拆回四字段）。
        assert_eq!(window.get_proxyChoice(), 2);
        assert_eq!(window.get_proxyProtocolIndex(), 1);
        assert_eq!(window.get_proxyHost(), "1.2.3.4");
        assert_eq!(window.get_proxyPort(), "1080");
        assert_eq!(window.get_proxyUser(), "u");
        assert_eq!(window.get_proxyPass(), "p");
        // 第 36 步：有值推送（Aria2 组四键同态）。
        assert!(window.get_aria2Enabled());
        assert_eq!(window.get_aria2RpcPort(), 10800);
        assert_eq!(window.get_aria2RpcToken(), "secret");
        assert!(window.get_aria2EmulateFingerprint());
        // 第 37 步：有值推送（浏览器组三键同态）。
        assert!(!window.get_browserEnabled());
        assert!(window.get_browserDraftTaken());
        assert_eq!(window.get_browserPort(), 15370);
        // 第 40 步：有值推送（配对令牌原样串）。
        assert_eq!(window.get_pairToken(), "p41rt0kenal");
        // 第 41 步：有值推送（ClientProfile="firefox-android" → 索引 5）。
        assert_eq!(window.get_clientProfileIndex(), 5);
        // 第 38 步：有值推送（urlScheme=F / 更新=F / 自启=T /
        // CloseMode="Quit"→索引 2 / 剪贴板=F）。
        assert!(!window.get_urlSchemeEnabled());
        assert!(!window.get_checkUpdateAtStartup());
        assert!(window.get_runAtLogin());
        assert_eq!(window.get_closeModeIndex(), 2);
        assert!(!window.get_clipboardListener());
        // 第 39 步：有值推送（DpiScale=1.25 → 显示 125%）。
        assert_eq!(window.get_uiScalePercent(), 125);

        // 缺省容错：空配置 → folder 用调用方缺省，数字走 Python 缺省。
        let window = crate::create_headless();
        crate::push_settings(&window, &UserConfig::new(), "/default/download");
        assert_eq!(window.get_downloadFolder(), "/default/download");
        assert_eq!(window.get_maxTaskNum(), 3);
        assert_eq!(window.get_preBlockNum(), 8);
        assert!(!window.get_categoryEnabled());
        // 限速值缺省 4194304 → 显示 4096 KB/s。
        assert_eq!(window.get_speedLimitationKb(), 4096);
        // 第 34 步：缺省容错（AutoSpeedUp=T/MaxReassignSize=512/
        // preserve=F/verifySsl=F/NameConflict=keepBoth/DNS=T）。
        assert!(window.get_autoSpeedUp());
        assert_eq!(window.get_maxReassignSize(), 512);
        assert!(!window.get_preserveLastModified());
        assert!(!window.get_verifySsl());
        assert_eq!(window.get_nameConflictIndex(), 0);
        assert!(window.get_useSystemDns());
        // 第 35 步：缺省容错——Auto 模式 → choice=1；自定义字段保持
        // 控件旧值（进程启动即缺省空串）。
        assert_eq!(window.get_proxyChoice(), 1);
        assert_eq!(window.get_proxyHost(), "");
        // 第 36 步：缺省容错（Enabled=F / Port=16800 / Token="" /
        // EmulateFingerprint=F）。
        assert!(!window.get_aria2Enabled());
        assert_eq!(window.get_aria2RpcPort(), 16800);
        assert_eq!(window.get_aria2RpcToken(), "");
        assert!(!window.get_aria2EmulateFingerprint());
        // 第 37 步：缺省容错（Enabled=T / Draft=F / Port=14370）。
        assert!(window.get_browserEnabled());
        assert!(!window.get_browserDraftTaken());
        assert_eq!(window.get_browserPort(), 14370);
        // 第 38 步：缺省容错（urlScheme=T / 更新=T / 自启=F /
        // CloseMode="Ask"→索引 0 / 剪贴板=T）。
        assert!(window.get_urlSchemeEnabled());
        assert!(window.get_checkUpdateAtStartup());
        assert!(!window.get_runAtLogin());
        assert_eq!(window.get_closeModeIndex(), 0);
        assert!(window.get_clipboardListener());
        // 第 39 步：缺省容错（DpiScale=0 → 0%「自动」）。
        assert_eq!(window.get_uiScalePercent(), 0);
        // 第 40 步：缺省容错（BrowserExtensionPairToken 缺省 ""）。
        assert_eq!(window.get_pairToken(), "");
        // 第 41 步：缺省容错（ClientProfile 缺省 "auto" → 索引 0）。
        assert_eq!(window.get_clientProfileIndex(), 0);

        let window = crate::create_headless();
        let hits: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let speed_hits: Rc<RefCell<Vec<i64>>> = Rc::new(RefCell::new(Vec::new()));
        let name_hits: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let proxy_hits: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let token_hits: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let draft_hits: Rc<RefCell<Vec<bool>>> = Rc::new(RefCell::new(Vec::new()));
        let close_hits: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let dpi_hits: Rc<RefCell<Vec<f64>>> = Rc::new(RefCell::new(Vec::new()));
        // 第 40 步：动作卡命中记录。
        let token_copy_hits: Rc<RefCell<Vec<()>>> = Rc::new(RefCell::new(Vec::new()));
        let token_regen_hits: Rc<RefCell<Vec<()>>> = Rc::new(RefCell::new(Vec::new()));
        let store_hits: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let chromium_hits: Rc<RefCell<Vec<()>>> = Rc::new(RefCell::new(Vec::new()));
        let profile_hits: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let actions = Rc::new(crate::SettingsActions {
            folder: Box::new({
                let h = hits.clone();
                move |s| h.borrow_mut().push(s.into())
            }),
            max_task_num: Box::new(|_| {}),
            pre_block_num: Box::new(|_| {}),
            category_enabled: Box::new(|_| {}),
            // 第 33 步：限速动作收配置口径字节；用 invocable 回调
            // 验证显示 KB/s → 字节换算通路（slint 生成 API 回调可
            // 从 Rust invoke）。
            speed_limitation: Box::new({
                let h = speed_hits.clone();
                move |bytes| h.borrow_mut().push(bytes)
            }),
            // 第 34 步：GeneralDownload 简版动作（no-op 注册仅验证
            // 通路；NameConflict 收字面量）。
            auto_speed_up: Box::new(|_| {}),
            max_reassign_size: Box::new(|_| {}),
            preserve_last_modified: Box::new(|_| {}),
            verify_ssl: Box::new(|_| {}),
            name_conflict: Box::new({
                let h = name_hits.clone();
                move |s| h.borrow_mut().push(s.into())
            }),
            use_system_dns: Box::new(|_| {}),
            // 第 35 步：代理动作收配置字面量（Off/Auto/有效 URL）。
            proxy_changed: Box::new({
                let h = proxy_hits.clone();
                move |s| h.borrow_mut().push(s.into())
            }),
            // 第 36 步：Aria2 组动作（令牌收原样串验证通路）。
            aria2_enabled: Box::new(|_| {}),
            aria2_rpc_port: Box::new(|_| {}),
            aria2_rpc_token: Box::new({
                let h = token_hits.clone();
                move |s| h.borrow_mut().push(s.into())
            }),
            aria2_emulate_fingerprint: Box::new(|_| {}),
            // 第 37 步：浏览器扩展组动作（草稿模式收 bool 验证通路）。
            browser_enabled: Box::new(|_| {}),
            browser_draft_taken: Box::new({
                let h = draft_hits.clone();
                move |v| h.borrow_mut().push(v)
            }),
            browser_port: Box::new(|_| {}),
            // 第 40 步：动作卡 invoke 通路验证。
            pair_token_copy: Box::new({
                let h = token_copy_hits.clone();
                move || h.borrow_mut().push(())
            }),
            pair_token_regenerate: Box::new({
                let h = token_regen_hits.clone();
                move || h.borrow_mut().push(())
            }),
            store_url_open: Box::new({
                let h = store_hits.clone();
                move |u| h.borrow_mut().push(u)
            }),
            chromium_install: Box::new({
                let h = chromium_hits.clone();
                move || h.borrow_mut().push(())
            }),
            chromium_crx_export: Box::new(|| {}),
            // 第 41 步：模拟身份（索引→字面量通路验证）。
            client_profile: Box::new({
                let h = profile_hits.clone();
                move |s| h.borrow_mut().push(s.into())
            }),
            // 第 38 步：关联+软件组动作（CloseMode 收字面量验证
            // 索引⇄字面量通路）。
            url_scheme_enabled: Box::new(|_| {}),
            check_update_at_startup: Box::new(|_| {}),
            run_at_login: Box::new(|_| {}),
            close_mode: Box::new({
                let h = close_hits.clone();
                move |s| h.borrow_mut().push(s.into())
            }),
            clipboard_listener: Box::new(|_| {}),
            // 第 39 步：界面缩放动作收配置 0-5 浮点。
            dpi_scale: Box::new({
                let h = dpi_hits.clone();
                move |v| h.borrow_mut().push(v)
            }),
        });
        crate::install_settings_actions(&window, actions);
        window.invoke_speedLimitationChanged(102400);
        assert_eq!(
            speed_hits.borrow().last().copied(),
            Some(104_857_600),
            "显示 KB/s 上限应换算回配置字节上限"
        );
        // 第 34 步：NameConflict invoke 验证索引→字面量紧凑换算通路
        // （镜像 qconfig.set(item, itemData(index))）。
        window.invoke_nameConflictChanged(2);
        assert_eq!(name_hits.borrow().last().map(|s| s.as_str()), Some("ask"));
        // 第 35 步：代理 invoke 通路——choice 0 直接发 "Off"；
        // 自定义字段无效 URL 不落盘（leaveEvent 镜像），有效才拼装。
        window.invoke_proxyChoiceChanged(0);
        assert_eq!(proxy_hits.borrow().last().map(|s| s.as_str()), Some("Off"));
        let before = proxy_hits.borrow().len();
        window.set_proxyChoice(2);
        window.invoke_proxyFieldsChanged(0, "1.2.3.4".into(), "70000".into(), "".into(), "".into());
        assert_eq!(
            proxy_hits.borrow().len(),
            before,
            "无效端口（70000）不应写配置"
        );
        window.invoke_proxyFieldsChanged(3, "1.2.3.4".into(), "8080".into(), "".into(), "".into());
        assert_eq!(
            proxy_hits.borrow().last().map(|s| s.as_str()),
            Some("http://1.2.3.4:8080")
        );
        window.invoke_proxyChoiceChanged(1);
        assert_eq!(proxy_hits.borrow().last().map(|s| s.as_str()), Some("Auto"));
        // 第 36 步：令牌 invoke 通路（edited/accepted 回发、install 层
        // 每次写——Python editingFinished 语义的更新鲜镜像）。
        window.invoke_aria2RpcTokenChanged("t0ken".into());
        assert_eq!(
            token_hits.borrow().last().map(|s| s.as_str()),
            Some("t0ken")
        );
        // 第 37 步：草稿模式 invoke 通路（Switch toggled → bool）。
        window.invoke_browserDraftTakenChanged(true);
        assert!(draft_hits.borrow().last().copied().unwrap_or(false));
        // 第 38 步：CloseMode invoke 通路（索引 1→"Background"，镜像
        // qconfig.set(item, EnumSerializer.serialize)）。
        window.invoke_closeModeChanged(1);
        assert_eq!(
            close_hits.borrow().last().map(|s| s.as_str()),
            Some("Background")
        );
        // 第 39 步：界面缩放 invoke 通路（显示 75% → 配置 0.75，
        // 镜像 Python leaveEvent cfg.set(value/100)）。
        window.invoke_uiScaleChanged(75);
        assert_eq!(
            dpi_hits.borrow().last().copied(),
            Some(0.75),
            "显示 % 应换算回配置 0-5 浮点"
        );
        // 第 40 步：动作卡 invoke 通路（复制/重新生成各 1 次、商店
        // URL 原样串、一键安装 1 次）。
        window.invoke_copyPairToken();
        assert_eq!(token_copy_hits.borrow().len(), 1);
        window.invoke_regeneratePairToken();
        assert_eq!(token_regen_hits.borrow().len(), 1);
        window.invoke_openStoreUrl(
            "https://addons.mozilla.org/firefox/addon/ghost-downloader/".into(),
        );
        assert_eq!(
            store_hits.borrow().last().map(|s| s.as_str()),
            Some("https://addons.mozilla.org/firefox/addon/ghost-downloader/")
        );
        window.invoke_chromiumInstall();
        assert_eq!(chromium_hits.borrow().len(), 1);
        // 第 41 步：模拟身份 invoke 通路（索引 3 → "edge"，镜像
        // buildProfileMenu 家族字面量选项）。
        window.invoke_clientProfileChanged(3);
        assert_eq!(
            profile_hits.borrow().last().map(|s| s.as_str()),
            Some("edge")
        );
        window.set_downloadFolder("/edited/path".into());
        assert_eq!(window.get_downloadFolder(), "/edited/path");
    }

    /// 段高亮：filterMode → 派生选中态。缺省 all；未知字面量全不
    /// 选中（slint 不做容错——容错语义在 filter.rs from_route 镜像层）。
    #[test]
    fn filter_mode_segment_highlight_follows_route_headless() {
        let window = crate::create_headless();
        assert!(window.get_highlightAll());
        assert!(!window.get_highlightActive());
        assert!(!window.get_highlightCompleted());
        window.set_filterMode("active".into());
        assert!(!window.get_highlightAll());
        assert!(window.get_highlightActive());
        assert!(!window.get_highlightCompleted());
        window.set_filterMode("weird".into());
        assert!(!window.get_highlightAll());
        assert!(!window.get_highlightActive());
        assert!(!window.get_highlightCompleted());
    }

    /// 排序 ComboBox 索引：缺省字段 0（按添加时间）、顺序 1（倒序）
    /// round-trip；字段索引 → SortField 的映射在 sort::from_menu_index。
    #[test]
    fn sort_combo_indexes_round_trip_headless() {
        let window = crate::create_headless();
        assert_eq!(window.get_sortFieldIndex(), 0);
        assert_eq!(window.get_sortOrderIndex(), 1);
        window.set_sortFieldIndex(3);
        window.set_sortOrderIndex(0);
        assert_eq!(window.get_sortFieldIndex(), 3);
        assert_eq!(window.get_sortOrderIndex(), 0);
    }

    /// refresh 串接排序：筛选后按 Name 倒序成行（五个排序分支的纯变换
    /// 语义由 sort.rs 测试锁定，这里锁「筛选 → 排序 → 装配」链路）。
    #[test]
    fn refresh_task_rows_applies_sort_headless() {
        use gd3_core::task::TaskRecord;
        use serde_json::json;

        let records: Vec<TaskRecord> = serde_json::from_value(json!([
            {
                "name": "b.iso", "url": "https://e/1", "packId": "p",
                "taskId": "t1", "status": "WAITING", "fileSize": 1024,
                "createdAt": 300,
            },
            {
                "name": "a.bin", "url": "https://e/2", "packId": "p",
                "taskId": "t2", "status": "RUNNING", "fileSize": 2048,
                "createdAt": 100,
                "steps": [
                    {"stepIndex": 0, "progress": 10, "speed": 0, "receivedBytes": 8},
                ],
            },
            {
                "name": "c.m3u8", "url": "https://e/3", "packId": "p",
                "taskId": "t3", "status": "PAUSED", "fileSize": 4096,
                "createdAt": 200,
            },
        ]))
        .expect("反序列化失败");
        let window = crate::create_headless();
        crate::refresh_task_rows(
            &window,
            &records,
            crate::filter::TaskFilter {
                mode: crate::filter::FilterMode::All,
                category: "",
                search: "",
            },
            crate::sort::SortField::Name,
            false,
            &[],
            |_| false,
        );
        let model = window.get_tasks();
        assert_eq!(model.row_count(), 3);
        let names: Vec<String> = (0..model.row_count())
            .filter_map(|index| model.row_data(index).map(|row| row.name.to_string()))
            .collect();
        assert_eq!(names, ["c.m3u8", "b.iso", "a.bin"]);
    }
}
