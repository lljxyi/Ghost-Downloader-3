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

pub mod actions;
pub mod aria2_actions;
pub mod band_selector;
pub mod banners;
pub mod batch_url;
pub mod boot;
pub mod bootstrap_list;
pub mod card;
pub mod card_groups;
pub mod card_menu;
pub mod category;
pub mod category_settings;
pub mod clipboard;
pub mod close_mode;
pub mod conflict;
pub mod draft;
pub mod draft_card;
pub mod drawer;
pub mod edit_task;
pub mod editors;
pub mod engine;
pub mod extension_install;
pub mod facts;
pub mod feature_service;
pub mod file_select;
pub mod filter;
mod format;
pub mod headers;
pub mod identity;
pub mod install_button;
pub mod labels;
pub mod markdown_viewer;
pub mod menu_view;
pub mod name_conflict;
pub mod oobe;
pub mod option_cards;
pub mod pack_info;
pub mod plan;
pub mod profile;
pub mod progress_toast;
pub mod proxy;
pub mod pyshlex;
pub mod pystr;
pub mod range_slider;
pub mod release_info;
pub mod row;
pub mod runtime_status;
pub mod scroll_area;
pub mod sequence_matcher;
pub mod setting_card_group;
pub mod setting_cards;
pub mod settings_group;
pub mod settings_spec;
pub mod shell;
pub mod single_instance;
pub mod sort;
pub mod speed;
pub mod speed_meter;
pub mod subtitle_select;
pub mod task_actions;
pub mod task_draft;
pub mod task_service;
pub mod token_line_edit;
pub mod track_bar;
pub mod tree_view;
pub mod window;

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
/// 行内容由 `row::to_task_row`（→ `card.rs` 派生层）算好；这里只做 model 装配。
/// （slint 1.18 生成的模型属性 setter 是无返回值的，见 target build 产物实证。）
pub fn set_task_rows(window: &AppWindow, rows: Vec<TaskRow>) {
    let model = slint::ModelRc::new(slint::VecModel::from(rows));
    window.set_tasks(model);
}

/// 推流路径：`Vec<TaskRecord>` → 卡片派生层（`card.rs`）→ TaskRow → model 装配。
///
/// `runtime` 由调用方给定记录里没有的运行时事实（`isSeeding`/`canPause`/
/// `canSeed`/`hasOutputFile`/输出文件是否存在/完成时间/`lastError`），
/// 这里只做「记录 → 卡片输入 → 行」的桥。
pub fn push_task_records(
    window: &AppWindow,
    records: &[gd3_core::task::TaskRecord],
    runtime: impl Fn(&gd3_core::task::TaskRecord) -> card::CardRuntime,
) {
    let rows: Vec<TaskRow> = records
        .iter()
        .map(|record| row::to_task_row(record, &runtime(record)))
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
    runtime: impl Fn(&gd3_core::task::TaskRecord) -> card::CardRuntime,
) {
    // 筛选（保序）→ 排序（镜像 task_page.py::_refreshList 末段）→ 成行。
    // waitingOrder 由调用方给（taskService.waitingOrder()）。
    let mut refs = filter::filtered(records, filter);
    sort::sort_refs(&mut refs, sort_field, sort_ascending, waiting_order);
    let rows: Vec<TaskRow> = refs
        .iter()
        .map(|record| row::to_task_row(record, &runtime(record)))
        .collect();
    set_task_rows(window, rows);
}

/// 第 53 步：把 `drawer.rs` 的五张卡片派生结果一次装配到 `TaskDrawer`。
///
/// 顺序与 `TaskDrawer.__init__` 的 `self.cards` 一致（Header / Error /
/// Info / Checksum / FileList）。`expectedChecksum` 走 `<=>` 双链，打开时
/// 按运行时值初始化；后续键入由调用方在 `expectedChecksumEdited` 里重算。
///
/// 末尾才置 `drawerVisible = true`——镜像 Qt 先刷状态再 `show()`，
/// 避免滑入过程中闪一帧空卡片。
pub fn open_task_drawer(
    window: &AppWindow,
    record: &gd3_core::task::TaskRecord,
    runtime: &drawer::DrawerRuntime,
) {
    let header = drawer::header_card(record, runtime);
    let error = drawer::error_bar(record, runtime);
    let info = drawer::info_card(record, runtime);
    let checksum = drawer::checksum_card(record, &runtime.expected_checksum);
    let files = drawer::file_list_card(record);

    window.set_drawerTaskId(record.task_id.as_str().into());
    // HeaderCard
    window.set_drawerTaskName(header.name.as_str().into());
    window.set_drawerProgressValue(header.progress_value as i32);
    window.set_drawerProgressVisible(header.progress_visible);
    window.set_drawerProgressError(header.progress_error);
    window.set_drawerStatusText(header.status_text.as_str().into());
    // ErrorBar
    window.set_drawerErrorVisible(error.visible);
    window.set_drawerErrorTitle(error.title.into());
    window.set_drawerErrorText(error.text.as_str().into());
    // InfoCard
    let info_rows: Vec<DrawerInfoRow> = info
        .rows
        .iter()
        .map(|row| DrawerInfoRow {
            key: row.name.into(),
            caption: row.caption.into(),
            value: row.text.as_str().into(),
            shown: row.visible,
        })
        .collect();
    window.set_drawerInfoRows(slint::ModelRc::new(slint::VecModel::from(info_rows)));
    // ChecksumCard
    window.set_drawerChecksumVisible(checksum.visible);
    let checksum_rows: Vec<DrawerChecksumRow> = checksum
        .rows
        .iter()
        .map(|row| DrawerChecksumRow {
            algorithm: row.algorithm.as_str().into(),
            value: row.value.as_str().into(),
        })
        .collect();
    window.set_drawerChecksumRows(slint::ModelRc::new(slint::VecModel::from(checksum_rows)));
    window.set_drawerChecksumError(checksum.matched.error);
    window.set_drawerChecksumMatched(checksum.matched.matched.is_some());
    window.set_drawerChecksumTooltip(checksum.matched.tooltip.as_str().into());
    window.set_drawerExpectedChecksum(runtime.expected_checksum.as_str().into());
    // FileListCard
    window.set_drawerFileListVisible(files.visible);
    let file_rows: Vec<DrawerFileRow> = files
        .rows
        .iter()
        .map(|row| DrawerFileRow {
            path: row.path.as_str().into(),
            size: row.size.as_str().into(),
            status: row.status.as_str().into(),
        })
        .collect();
    window.set_drawerFileRows(slint::ModelRc::new(slint::VecModel::from(file_rows)));

    window.set_drawerVisible(true);
}

/// 第 53 步：关闭抽屉并把 `taskId` 归位。
///
/// Slint 侧的关闭按钮/遮罩点击已自行把 `drawerVisible` 置 false（镜像 Qt
/// `reject()`），这里保证调用方从任何入口调都收敛到同一状态（幂等）。
pub fn close_task_drawer(window: &AppWindow) {
    window.set_drawerVisible(false);
    window.set_drawerTaskId("".into());
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
    // 第 42 步：请求标头预设（cfg.py:328-333 实证：预设列表存 JSON
    // 串、"0" 串索引；解析/逐项校验/缺省兜「默认+BASE_HEADERS」在
    // headers.rs 镜像 HeadersPresetListValidator.correct）。
    let headers_presets =
        headers::parse_presets(config.get_str("Network", "HeadersPresets").unwrap_or(""));
    let headers_index = headers::current_index(
        // 磁盘存的是 int（`"CurrentHeadersPreset": 0`），`get_str` 读不到。
        config
            .get_py_str("Network", "CurrentHeadersPreset")
            .as_deref(),
        headers_presets.len(),
    );
    window.set_headersPresetNames(slint::ModelRc::new(slint::VecModel::from(
        headers_presets
            .iter()
            .map(|preset| preset.name.as_str().into())
            .collect::<Vec<_>>(),
    )));
    window.set_headersPresetIndex(headers_index as i32);
    window.set_headersPresetHeaderCount(
        headers_presets
            .get(headers_index)
            .map(|preset| preset.headers.len())
            .unwrap_or(0) as i32,
    );
    window.set_headersPresetCanRemove(headers_presets.len() > 1);
    // 第 43 步：身份预设列表（cfg.py:334-340 实证：缺键 → 百度网盘
    // 客户端预设；读盘仅 JSON 反序列化不过 validator.correct，镜像在
    // identity.rs；行结构镜像 PresetRowWidget——名称/三段描述/开关）。
    let identity_presets = identity::parse_presets(config.get_str("Network", "IdentityPresets"));
    window.set_identityPresets(slint::ModelRc::new(slint::VecModel::from(
        identity_presets
            .iter()
            .map(|preset| IdentityPresetData {
                name: preset.name.as_str().into(),
                desc: identity::row_desc(preset).into(),
                enabled: preset.is_enabled,
            })
            .collect::<Vec<_>>(),
    )));
    // 第 44 步：库分类规则（Category/CategoryRules：缺键/空串/[]/
    // 非 JSON → 八预设镜像 category.rs parse_rules；行摘要镜像
    // _toSummary；缺 categoryId 保留空串——随机 ID 不复刻）。
    let category_rules = category::parse_rules(config.get_str("Category", "CategoryRules"));
    window.set_categoryRows(slint::ModelRc::new(slint::VecModel::from(
        category_rules
            .iter()
            .map(|rule| CategoryRowData {
                name: rule.name.as_str().into(),
                desc: category::summary_of(&rule.extensions).into(),
                id: rule.category_id.as_str().into(),
            })
            .collect::<Vec<_>>(),
    )));
    window.set_categoryRuleCount(category_rules.len() as i32);
    // 第 45 步：图标下拉 choices（ICON_CHOICES 逐字 25 项）。
    window.set_categoryIconChoices(slint::ModelRc::new(slint::VecModel::from(
        category::ICON_CHOICES
            .iter()
            .map(|choice| choice.to_string().into())
            .collect::<Vec<_>>(),
    )));
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

/// 第 46 步：打开标头预设添加/编辑覆盖层（index None=添加，镜像 _onAddClicked
/// preset={"name": "", headers: currentHeaders()}——起点用选中预设的
/// 标头回显；带值=编辑，镜像 _onEditClicked presets[index]）。
pub fn open_headers_preset_edit(
    window: &AppWindow,
    config: &gd3_core::config::UserConfig,
    index: Option<usize>,
) {
    let presets = headers::parse_presets(config.get_str("Network", "HeadersPresets").unwrap_or(""));
    let selected = index.unwrap_or_else(|| {
        headers::current_index(
            config
                .get_py_str("Network", "CurrentHeadersPreset")
                .as_deref(),
            presets.len(),
        )
    });
    let preset = presets.get(selected);
    // 添加（index None）名称留空——镜像 _onAddClicked 的 name=""；
    // 编辑（带 index）回显 presets[index].name。索引越界时 preset 为
    // None，名称与标头都落空串（越界守卫归调用方）。
    let name = match (index, preset) {
        (Some(_), Some(preset)) => preset.name.clone(),
        _ => String::new(),
    };
    window.set_headersEditIndex(index.map(|i| i as i32).unwrap_or(-1));
    window.set_headersEditName(name.into());
    let pairs = preset
        .map(|p| headers::headers_to_text(&p.headers))
        .unwrap_or_default();
    window.set_headersEditPairs(pairs.clone().into());
    // 第 48 步：初算重复标头警告（手改过的 cfg 可能带重名）。
    window.set_headersEditDuplicateWarning(headers::duplicate_warning(&pairs).into());
    window.set_headersEditNameError(false);
    window.set_headersEditVisible(true);
}

/// 第 47 步：打开身份预设添加/编辑覆盖层（index None=添加镜像
/// `PresetEditDialog(preset=None)`：名称/hosts/UA 空、TLS 指纹落「跟随
/// 全局默认」（`_profileValue = ""`）；带值=编辑回显 `presets[index]`
/// （`_populate` 名称/hosts 逐条/UA；索引越界同添加态，越界守卫归
/// 调用方）。
pub fn open_identity_preset_edit(
    window: &AppWindow,
    config: &gd3_core::config::UserConfig,
    index: Option<usize>,
) {
    let presets = identity::parse_presets(config.get_str("Network", "IdentityPresets"));
    let preset = index.and_then(|i| presets.get(i));
    // 菜单模型镜像 `buildProfileMenu(includeAuto=False)`：首项空串「跟随
    // 全局默认」+ raw + 九家族「最新」共 11 项。未知字面量（特定版本如
    // chrome120）追加一项——Python 侧按钮文本走 toProfileLabel，菜单里
    // 却没有该项，这里补上避免静默丢值。
    let mut labels: Vec<slint::SharedString> = (0..=10i64)
        .map(|i| identity::profile_label_of(i).into())
        .collect();
    let mut values: Vec<slint::SharedString> = (0..=10i64)
        .map(|i| identity::profile_value_of(i).into())
        .collect();
    let profile = preset.map(|p| p.client_profile.clone()).unwrap_or_default();
    let profile_index = match identity::profile_index_of(&profile) {
        Some(i) => i,
        None => {
            labels.push(profile::label_of(&profile).into());
            values.push(profile.as_str().into());
            (values.len() - 1) as i64
        }
    };
    window.set_identityEditProfileLabels(slint::ModelRc::new(slint::VecModel::from(labels)));
    window.set_identityEditProfileValues(slint::ModelRc::new(slint::VecModel::from(values)));
    window.set_identityEditProfileIndex(profile_index as i32);
    window.set_identityEditProfile(profile.as_str().into());
    window.set_identityEditIndex(index.map(|i| i as i32).unwrap_or(-1));
    window.set_identityEditName(preset.map(|p| p.name.clone()).unwrap_or_default().into());
    window.set_identityEditHosts(
        preset
            .map(|p| identity::hosts_to_text(&p.hosts))
            .unwrap_or_default()
            .into(),
    );
    window.set_identityEditUserAgent(
        preset
            .map(|p| p.user_agent.clone())
            .unwrap_or_default()
            .into(),
    );
    window.set_identityEditHostsError(false);
    window.set_identityEditHelpVisible(false);
    window.set_identityEditVisible(true);
}

/// 第 47 步：把覆盖层「确定」产物落到预设列表——index<0 追加（镜像
/// `_onAddClicked` 的 `presets.append(dialog.preset())`），否则替换
/// （`_onEditClicked` 的 `presets[index] = dialog.preset()`，越界直接
/// 返回）。编辑态 isEnabled 继承既有值（镜像 `preset()` 的
/// `result["isEnabled"] = self._preset.get("isEnabled", True)`；添加态
/// 草稿里已是 True）。返回是否落地。
pub fn apply_identity_preset_draft(
    presets: &mut Vec<identity::IdentityPreset>,
    index: i64,
    mut draft: identity::IdentityPreset,
) -> bool {
    if index < 0 {
        presets.push(draft);
        return true;
    }
    let Ok(slot) = usize::try_from(index) else {
        return false;
    };
    let Some(existing) = presets.get_mut(slot) else {
        return false;
    };
    draft.is_enabled = existing.is_enabled;
    *existing = draft;
    true
}

/// 第 45 步：打开分类添加/编辑表单（直写窗口字段+置可见；`rule`
/// None=添加镜像 Dialog(category=None)：字段前置空、icon 落 0）。
pub fn open_category_edit(window: &AppWindow, rule: Option<&category::CategoryRule>) {
    let Some(rule) = rule else {
        window.set_categoryEditId("".into());
        window.set_categoryEditName("".into());
        window.set_categoryEditIconIndex(0);
        window.set_categoryEditExtensions("".into());
        window.set_categoryEditFolder("".into());
        window.set_categoryEditVisible(true);
        return;
    };
    window.set_categoryEditId(rule.category_id.as_str().into());
    window.set_categoryEditName(rule.name.as_str().into());
    // 图标预选镜像 _populate：findData 命中否则 0（未知 icon 落首项）。
    let index = category::ICON_CHOICES
        .iter()
        .position(|choice| *choice == rule.icon)
        .unwrap_or(0) as i32;
    window.set_categoryEditIconIndex(index);
    window.set_categoryEditExtensions(rule.extensions.join(",").into());
    window.set_categoryEditFolder(rule.folder.clone().unwrap_or_default().into());
    window.set_categoryEditVisible(true);
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
    // 第 42 步：标头预设动作——选中/删除收**当前预设索引**（cfg 写与
    // 索引调整、删除确认框归调用方，镜像 _onPick/_onRemoveClicked）。
    pub headers_preset_pick: Box<dyn Fn(i64)>,
    pub headers_preset_remove: Box<dyn Fn(i64)>,
    // 第 46 步：标头预设添加/编辑动作（添加收 ()；编辑收 int 索引
    // 原样——越界守卫/缺失回退归调用方；「取消」收 ()；「确定」回发
    // (索引, draft_to_header_preset 归一化预设)，cfg 写与重推归调用方）。
    pub headers_preset_add_requested: Box<dyn Fn()>,
    pub headers_preset_edit_requested: Box<dyn Fn(i64)>,
    pub headers_preset_edit_closed: Box<dyn Fn()>,
    pub headers_preset_edit_submit: Box<dyn Fn((i64, headers::HeaderPreset))>,
    // 第 43 步：身份预设动作——开关收 (索引, bool)、删除收当前索引
    // （cfg 写、索引边界检查、删除确认框归调用方，镜像
    // _onToggled/_onRemoveClicked；编辑/添加对话框留欠账步）。
    pub identity_preset_toggle: Box<dyn Fn(i64, bool)>,
    pub identity_preset_remove: Box<dyn Fn(i64)>,
    // 第 47 步：身份预设添加/编辑动作（添加收 ()；编辑收 int 索引原样；
    // 「取消」收 ()；「确定」回发 (索引, draft_to_identity_preset 归一化
    // 预设)——编辑态 isEnabled 由调用方经 apply_identity_preset_draft
    // 继承，cfg 写与 categoriesChanged 式重推归调用方）。
    pub identity_preset_add_requested: Box<dyn Fn()>,
    pub identity_preset_edit_requested: Box<dyn Fn(i64)>,
    pub identity_preset_edit_closed: Box<dyn Fn()>,
    pub identity_preset_edit_submit: Box<dyn Fn((i64, identity::IdentityPreset))>,
    // 第 44 步：库分类动作（恢复默认收 ()、删除收 categoryId 原样；
    // cfg 写与重推归调用方）。
    pub category_reset: Box<dyn Fn()>,
    pub category_remove: Box<dyn Fn(String)>,
    // 第 45 步：分类添加/编辑动作（添加/取消收 ()；编辑入口收
    // categoryId 原样——查行与开框归调用方；确定收归一化后的
    // 规则，cfg 写与 categoriesChanged 重推归调用方）。
    pub category_add_requested: Box<dyn Fn()>,
    pub category_edit_requested: Box<dyn Fn(String)>,
    pub category_edit_closed: Box<dyn Fn()>,
    pub category_edit_submit: Box<dyn Fn(category::CategoryRule)>,
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
    // 第 42 步：标头预设注册（索引原样转发——镜像 _onPick 写
    // CurrentHeadersPreset、_onRemoveClicked 移除后重排；cfg 写与删除
    // 确认框 MessageBox 归调用方）。
    let headers_pick = actions.clone();
    window.on_headersPresetPicked(move |i| (headers_pick.headers_preset_pick)(i as i64));
    let headers_remove = actions.clone();
    window
        .on_headersPresetRemoveRequested(move |i| (headers_remove.headers_preset_remove)(i as i64));
    // 第 46 步：标头预设添加/编辑入口与确定草稿（确定回发
    // draft_to_header_preset 归一化预设；cfg 写与重推归调用方）。
    let headers_add = actions.clone();
    window.on_headersPresetAddRequested(move || (headers_add.headers_preset_add_requested)());
    let headers_edit = actions.clone();
    window.on_headersPresetEditRequested(move |i| {
        (headers_edit.headers_preset_edit_requested)(i as i64)
    });
    let headers_closed = actions.clone();
    window.on_headersPresetEditClosed(move || (headers_closed.headers_preset_edit_closed)());
    let headers_submit = actions.clone();
    window.on_headersPresetEditAccepted(move |d| {
        (headers_submit.headers_preset_edit_submit)((
            d.index as i64,
            headers::draft_to_header_preset(&d.name, &d.pairs),
        ))
    });
    // 第 48 步：标头文本编辑中实时重算重复标头警告——纯呈现层派生
    // （`duplicate_names` 汇总），不进 `SettingsActions`：Python 的逐行
    // `row.setDuplicate` 红框在 Slint 的 `TextEdit` 上无法逐行染色，
    // 降级成一行汇总文字。
    let headers_dup_win = window.clone_strong();
    window.on_headersPresetPairsEdited(move |text| {
        headers_dup_win.set_headersEditDuplicateWarning(headers::duplicate_warning(&text).into());
    });
    // 第 43 步：身份预设注册（索引越界检查归调用方，镜像 Python
    // `if index < 0 or index >= len(presets): return` 守卫）。
    let identity_toggle = actions.clone();
    window.on_identityPresetToggled(move |i, checked| {
        (identity_toggle.identity_preset_toggle)(i as i64, checked)
    });
    let identity_remove = actions.clone();
    window.on_identityPresetRemoveRequested(move |i| {
        (identity_remove.identity_preset_remove)(i as i64)
    });
    // 第 47 步：身份预设添加/编辑注册（「确定」先过 `validate()` 口径：
    // 至少一个 Host，否则只置 hostsError 不收起、不发事件——精确镜像
    // 需 parse_hosts 归一化后判空，故收起责任落在 Rust 侧；通过后清错、
    // 收起覆盖层并回发 draft_to_identity_preset 归一化预设）。
    let identity_add = actions.clone();
    window.on_identityPresetAddRequested(move || (identity_add.identity_preset_add_requested)());
    let identity_edit = actions.clone();
    window.on_identityPresetEditRequested(move |i| {
        (identity_edit.identity_preset_edit_requested)(i as i64)
    });
    let identity_closed = actions.clone();
    window.on_identityPresetEditClosed(move || (identity_closed.identity_preset_edit_closed)());
    let identity_submit = actions.clone();
    let identity_win = window.clone_strong();
    window.on_identityPresetEditAccepted(move |d| {
        if !identity::hosts_valid(&identity::parse_hosts(&d.hosts)) {
            identity_win.set_identityEditHostsError(true);
            return;
        }
        identity_win.set_identityEditHostsError(false);
        identity_win.set_identityEditVisible(false);
        (identity_submit.identity_preset_edit_submit)((
            d.index as i64,
            identity::draft_to_identity_preset(&d.name, &d.hosts, &d.profile, &d.userAgent, true),
        ))
    });
    // 第 44 步：库分类动作注册（删除收 SharedString → String 原样）。
    let category_reset = actions.clone();
    window.on_categoryResetRequested(move || (category_reset.category_reset)());
    let category_remove = actions.clone();
    window.on_categoryRemoveRequested(move |id| (category_remove.category_remove)(id.to_string()));
    // 第 45 步：分类添加/编辑动作注册（确定草稿经五字段构造即
    // category::draft_to_rule 归一化后转发——对话框端只回发原样
    // 字段，容错口径在镜像层）。
    let category_add = actions.clone();
    window.on_categoryAddRequested(move || (category_add.category_add_requested)());
    let category_edit = actions.clone();
    window.on_categoryEditRequested(move |id| {
        (category_edit.category_edit_requested)(id.to_string())
    });
    let category_close = actions.clone();
    window.on_categoryEditClosed(move || (category_close.category_edit_closed)());
    let category_submit = actions.clone();
    window.on_categoryEditAccepted(move |draft| {
        (category_submit.category_edit_submit)(category::draft_to_rule(
            &draft.id,
            &draft.name,
            &draft.icon,
            &draft.extensions,
            &draft.folder,
        ))
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
///
/// 第 51 步补上卡片按钮五路：`toggle`（按状态分派暂停/继续/做种，
/// 镜像 `_onToggleClicked`）、`select_files`、`checksum`、`open_file`、
/// `open_folder`。它们都只带 taskId，语义（打开哪条路径、是否弹选择框）
/// 全在调用方。
pub struct TaskActions {
    pub search: Box<dyn Fn(&str)>,
    pub filter_mode: Box<dyn Fn(&str)>,
    pub category_selected: Box<dyn Fn(i32)>,
    pub sort_field: Box<dyn Fn(i32)>,
    pub sort_order: Box<dyn Fn(i32)>,
    pub pause: Box<dyn Fn(&str)>,
    pub delete: Box<dyn Fn(&str)>,
    pub toggle: Box<dyn Fn(&str)>,
    pub select_files: Box<dyn Fn(&str)>,
    pub checksum: Box<dyn Fn(&str)>,
    pub open_file: Box<dyn Fn(&str)>,
    pub open_folder: Box<dyn Fn(&str)>,
    /// 第 53 步：卡片「详情」→ 打开任务抽屉（镜像 `_openDrawer`）。
    pub open_drawer: Box<dyn Fn(&str)>,
}

/// 把动作集注册到 AppWindow 的回调上（on_searchChanged 等生成名
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
    // 第 51 步：卡片按钮五路（Slint 只发 taskId 字面量）。
    let toggle = actions.clone();
    window.on_taskToggle(move |task_id| (toggle.toggle)(&task_id));
    let select_files = actions.clone();
    window.on_taskSelectFiles(move |task_id| (select_files.select_files)(&task_id));
    let checksum = actions.clone();
    window.on_taskChecksum(move |task_id| (checksum.checksum)(&task_id));
    let open_file = actions.clone();
    window.on_taskOpenFile(move |task_id| (open_file.open_file)(&task_id));
    let open_folder = actions.clone();
    window.on_taskOpenFolder(move |task_id| (open_folder.open_folder)(&task_id));
    let open_drawer = actions.clone();
    window.on_taskOpenDrawer(move |task_id| (open_drawer.open_drawer)(&task_id));
}

// ---------------------------------------------------------------------------
// 第 60 步：卡片任务级回调直接接服务层（P0 收口）
// ---------------------------------------------------------------------------
//
// 第 51/57 步把回调接到了 `TaskActions`，但那只是「把 taskId 交给调用方」；
// 语义（按状态分派、删除前落盘配置、校验三条短路）调用方还得再算一遍。
// 这一节把 `actions.rs` 的语义**装到窗口上**：注册即通路，调用方只提供事实
// （`TaskFacts`）与服务实现。

/// 一个任务的运行时事实：`tasks.jsonl` 里没有、只能问调用方的那部分。
///
/// 与 `card::CardRuntime` / `drawer::DrawerRuntime` 同一原则——本 crate 不碰
/// 文件系统、不读配置、不查 store。
#[derive(Clone, Debug, Default)]
pub struct TaskFacts {
    /// `task.status`
    pub status: gd3_core::task::TaskStatus,
    /// `task.isSeeding`（类属性）
    pub is_seeding: bool,
    /// `task.url`（「复制下载链接」用）
    pub url: String,
    /// `task.outputPath`（打开文件 / 在文件夹中显示用）
    pub output_path: String,
    /// `taskService.checksumProgress(task) is not None`
    pub checksum_in_progress: bool,
    /// `isFolder(outputPath)`
    pub is_folder: bool,
    /// `isExisting(outputPath)`
    pub output_exists: bool,
    /// `ALGORITHMS = sorted(hashlib.algorithms_available)`——平台相关，调用方给。
    pub algorithms: Vec<String>,
    /// `task.checksums` 的键集。
    pub checksums: Vec<String>,
}

/// `Box<dyn Fn(&str) -> Option<TaskFacts>>`：按 taskId 取事实。
///
/// 抽成别名是被 `clippy::type_complexity` 逼的（与 `DrawerTextAction` 同一处理）。
pub type TaskFactsLookup = Box<dyn Fn(&str) -> Option<TaskFacts>>;
/// `Box<dyn Fn(&str, Vec<CardMenuEntry>)>`：校验菜单落点（弹菜单归调用方）。
pub type TaskChecksumMenu = Box<dyn Fn(&str, Vec<card_menu::CardMenuEntry>)>;
/// `Box<dyn Fn(&str, &str)>`：卡片状态行（`TaskCard._setStatus`）。
pub type TaskStatusHint = Box<dyn Fn(&str, &str)>;

/// 任务级回调的服务层装配（第 60 步）。
///
/// 与 [`TaskActions`] 的关系：同一个窗口上两者的回调**按名互斥**（Slint 生成的
/// 回调是单槽，后注册者生效）。`TaskCommands` 覆盖
/// `taskToggle` / `taskPause` / `taskDelete` / `taskChecksum` /
/// `taskOpenFile` / `taskOpenFolder` / `taskOpenDrawer` 七路；
/// `searchChanged` / `filterModeChanged` / `categoryFilterChanged` /
/// `sortFieldChanged` / `sortOrderChanged` / `taskSelectFiles` 仍归 `TaskActions`。
pub struct TaskCommands {
    pub service: Rc<dyn actions::TaskService>,
    pub hooks: Rc<dyn actions::UiHooks>,
    /// 按 taskId 取事实；`None` = 任务已经不在了（回调直接丢弃）。
    pub facts: TaskFactsLookup,
    /// 校验菜单的落点：`ShowMenu` 分支把菜单树交给调用方去弹——锚点（校验按钮
    /// 的屏幕位置）只有视图知道，本 crate 不算几何。
    pub checksum_menu: TaskChecksumMenu,
    /// 卡片状态行（`TaskCard._setStatus`）：校验按钮的「文件不存在，无法校验」走这里。
    pub status_hint: TaskStatusHint,
    /// 删除确认框的勾选初值（`cfg.shouldDeleteFilesOnRemove.value`）。
    pub delete_files_default: Box<dyn Fn() -> bool>,
    /// 待确认删除的 taskId。Qt 的 `MessageBox.exec()` 是阻塞的，Slint 侧消息框是
    /// 异步的：点「删除」先记下 id，点「确定」时再取。
    pending_delete: std::cell::RefCell<Option<String>>,
}

impl TaskCommands {
    pub fn new(
        service: Rc<dyn actions::TaskService>,
        hooks: Rc<dyn actions::UiHooks>,
        facts: TaskFactsLookup,
        checksum_menu: TaskChecksumMenu,
        status_hint: TaskStatusHint,
        delete_files_default: Box<dyn Fn() -> bool>,
    ) -> Self {
        Self {
            service,
            hooks,
            facts,
            checksum_menu,
            status_hint,
            delete_files_default,
            pending_delete: std::cell::RefCell::new(None),
        }
    }

    /// 是否正有一个删除确认框等着回话（测试与调用方分派用）。
    pub fn has_pending_delete(&self) -> bool {
        self.pending_delete.borrow().is_some()
    }
}

/// 把任务级七路回调装到窗口上（见 [`TaskCommands`] 的回调清单）。
pub fn install_task_commands(window: &AppWindow, commands: Rc<TaskCommands>) {
    // 暂停/继续/做种：按状态分派（`_onToggleClicked`）。
    let toggle = commands.clone();
    window.on_taskToggle(move |task_id| {
        let Some(facts) = (toggle.facts)(&task_id) else {
            return;
        };
        actions::apply_toggle(&*toggle.service, &task_id, facts.status, facts.is_seeding);
    });

    // 「暂停」按钮是显式一路（`task_page.py` 的暂停按钮直调 `pause`）。
    let pause = commands.clone();
    window.on_taskPause(move |task_id| (pause.service).pause(&task_id));

    // 删除：先弹确认框（勾选初值取 cfg），确认结果走 [`resolve_delete_dialog`]。
    let delete = commands.clone();
    let weak = window.as_weak();
    window.on_taskDelete(move |task_id| {
        let spec = actions::delete_confirm_dialog((delete.delete_files_default)());
        *delete.pending_delete.borrow_mut() = Some(task_id.to_string());
        if let Some(window) = weak.upgrade() {
            show_dialog(&window, &spec);
        }
    });

    // 校验：三条短路（`_onChecksumClicked`）。
    let checksum = commands.clone();
    window.on_taskChecksum(move |task_id| {
        let Some(facts) = (checksum.facts)(&task_id) else {
            return;
        };
        let outcome = card_menu::checksum_click(
            facts.checksum_in_progress,
            facts.is_folder,
            facts.output_exists,
            || {
                card_menu::checksum_menu(&card_menu::ChecksumMenuInput {
                    algorithms: &facts.algorithms,
                    checksums: &facts.checksums,
                })
            },
        );
        if let Some(text) = actions::apply_checksum(&*checksum.service, &task_id, &outcome) {
            (checksum.status_hint)(&task_id, text);
        } else if let card_menu::ChecksumOutcome::ShowMenu(entries) = outcome {
            (checksum.checksum_menu)(&task_id, entries);
        }
    });

    // 打开文件 / 在文件夹中显示：路径来自事实（`openFile` / `revealInFolder`）。
    let open_file = commands.clone();
    window.on_taskOpenFile(move |task_id| {
        if let Some(facts) = (open_file.facts)(&task_id) {
            (open_file.hooks).open_file(&facts.output_path);
        }
    });
    let open_folder = commands.clone();
    window.on_taskOpenFolder(move |task_id| {
        if let Some(facts) = (open_folder.facts)(&task_id) {
            (open_folder.hooks).reveal_in_folder(&facts.output_path);
        }
    });

    // 详情 → 抽屉（`_openDrawer`）。
    let open_drawer = commands.clone();
    window.on_taskOpenDrawer(move |task_id| (open_drawer.hooks).open_drawer(&task_id));
}

/// 删除确认框的回话落点（由调用方的 `dialogButtonClicked` 分派）。
///
/// 镜像 `_onDeleteClicked` 的后半段：
/// `if dialog.exec(): cfg.set(shouldDeleteFilesOnRemove, checked); taskService.delete(task, checked)`。
///
/// - `accepted == false`（取消/关闭）→ 什么都不做，返回 `None`；
/// - `accepted == true` → 勾选值取窗口上的 `dialogCheckboxChecked`（`<=>` 双向绑定，
///   用户在框里改的就是它），删任务并返回该值——**调用方要把它写回
///   `cfg.shouldDeleteFilesOnRemove`**（Python 侧就是 `cfg.set`）。
///
/// 勾选值必须在 `hide_dialog` **之前**读：`hide_dialog` 会把复选框复位成
/// `false`（镜像 Qt 每次 `exec()` 都是新对话框对象）。
pub fn resolve_delete_dialog(
    window: &AppWindow,
    commands: &TaskCommands,
    accepted: bool,
) -> Option<bool> {
    let task_id = commands.pending_delete.borrow_mut().take()?;
    let delete_files = window.get_dialogCheckboxChecked();
    hide_dialog(window);
    if !accepted {
        return None;
    }
    Some(actions::apply_delete(
        &*commands.service,
        &task_id,
        delete_files,
    ))
}

/// 抽屉动作集（第 53 步）：五路回调 → 调用方语义。
///
/// 全部只带 taskId（+ 需要复制的文本）：剪贴板读写、文件管理器、校验值
/// 重算与重绘都在调用方（`task_drawer.py` 里这些是 `QApplication.clipboard()`、
/// `revealInFolder`、`_refreshMatch` 的职责，不是派生层）。
///
/// 两个收 `(taskId, text)` 的回调抽成 `DrawerTextAction`——裸
/// `Box<dyn Fn(&str, &str)>` 会被 `clippy::type_complexity` 拦。
pub type DrawerTextAction = Box<dyn Fn(&str, &str)>;

pub struct DrawerActions {
    pub close: Box<dyn Fn(&str)>,
    pub expected_edited: DrawerTextAction,
    pub paste_checksum: Box<dyn Fn(&str)>,
    pub copy_text: DrawerTextAction,
    pub reveal_in_folder: Box<dyn Fn(&str)>,
}

/// 把抽屉动作集注册到 AppWindow 的五个抽屉回调上。
pub fn install_drawer_actions(window: &AppWindow, actions: std::rc::Rc<DrawerActions>) {
    let close = actions.clone();
    window.on_drawerCloseRequested(move |task_id| (close.close)(&task_id));
    let expected_edited = actions.clone();
    window.on_drawerExpectedChecksumEdited(move |task_id, text| {
        (expected_edited.expected_edited)(&task_id, &text)
    });
    let paste_checksum = actions.clone();
    window
        .on_drawerPasteChecksumRequested(move |task_id| (paste_checksum.paste_checksum)(&task_id));
    let copy_text = actions.clone();
    window.on_drawerCopyTextRequested(move |task_id, text| (copy_text.copy_text)(&task_id, &text));
    let reveal_in_folder = actions.clone();
    window.on_drawerRevealInFolderRequested(move |task_id| {
        (reveal_in_folder.reveal_in_folder)(&task_id)
    });
}

// ---------------------------------------------------------------------------
// 第 57 步：主窗口外壳装配（导航栏 / 标题栏搜索框 / 通知条 / 消息框 / 拖放）
// ---------------------------------------------------------------------------
//
// 这一层的职责只有「形状 → model/属性」：所有文案、按钮集合与判定都在
// `window.rs`（Slint-free，金文件逐条钉死），这里不重复任何业务规则。

/// 导航 model：`window::nav_items_at` 的一侧 → Slint 结构体。
///
/// `selected` 只对 `selectable` 的项可能为真——「新建任务」在 Qt 侧
/// 从未调用 `setSelected`（`_addPage` 只对真页面调）。
pub fn nav_model(
    position: window::NavPosition,
    current_route: &str,
) -> slint::ModelRc<NavItemData> {
    let items: Vec<NavItemData> = window::nav_items_at(position)
        .into_iter()
        .map(|item| NavItemData {
            routeKey: item.route_key.into(),
            text: item.title.into(),
            icon: item.icon.into(),
            selectable: item.selectable,
            selected: item.selectable && item.route_key == current_route,
        })
        .collect();
    slint::ModelRc::new(slint::VecModel::from(items))
}

/// 一次写齐导航栏两侧（镜像 `_initLayout` 收尾时的 `navigationInterface`
/// 状态）。
pub fn set_nav_items(window: &AppWindow, current_route: &str) {
    window.set_navTop(nav_model(window::NavPosition::Top, current_route));
    window.set_navBottom(nav_model(window::NavPosition::Bottom, current_route));
}

/// 切页：`currentRoute` + 搜索框口径 + 导航选中态。
///
/// 镜像 `switchTo(page)` + `_updateSearchTarget(page)`：后者无论如何都会
/// 先 `self.searchEdit.clear()`，故这里也清空 `searchText`。
pub fn switch_page(window: &AppWindow, route_key: &str) {
    window.set_currentRoute(route_key.into());
    let target = window::search_target(route_key);
    window.set_searchVisible(target.placeholder.is_some());
    window.set_searchPlaceholder(target.placeholder.unwrap_or_default().into());
    window.set_searchText("".into());
    set_nav_items(window, route_key);
}

/// `InfoBarSpec` / `DialogSpec` 的按钮 → Slint model。`index` 就是
/// `addWidget(...)` 的顺序，回调只回传序号，分派归调用方。
pub fn bar_buttons(buttons: &[window::BarButton]) -> slint::ModelRc<BarButtonData> {
    let rows: Vec<BarButtonData> = buttons
        .iter()
        .enumerate()
        .map(|(index, button)| BarButtonData {
            text: button.text.as_str().into(),
            index: index as i32,
            primary: button.primary,
        })
        .collect();
    slint::ModelRc::new(slint::VecModel::from(rows))
}

/// 弹通知条（镜像 `InfoBar(...)` + `addWidget` 序列 + `show()`）。
pub fn show_info_bar(window: &AppWindow, spec: &window::InfoBarSpec) {
    window.set_infoBarTitle(spec.title.as_str().into());
    window.set_infoBarText(spec.text.as_str().into());
    window.set_infoBarButtons(bar_buttons(&spec.buttons));
    window.set_infoBarVisible(true);
}

/// 收起通知条（关闭按钮 / `_onUpdateChanged` 里 `close()` 的对应物）。
pub fn hide_info_bar(window: &AppWindow) {
    window.set_infoBarVisible(false);
}

/// 弹消息框。`checkbox == None` 时 `dialogCheckboxLabel` 写空串——
/// Slint 侧约定「空串 = 没有复选框」。
pub fn show_dialog(window: &AppWindow, spec: &window::DialogSpec) {
    window.set_dialogTitle(spec.title.as_str().into());
    window.set_dialogText(spec.text.as_str().into());
    let (label, checked) = match &spec.checkbox {
        Some((label, checked)) => (label.as_str(), *checked),
        None => ("", false),
    };
    window.set_dialogCheckboxLabel(label.into());
    window.set_dialogCheckboxChecked(checked);
    window.set_dialogButtons(bar_buttons(&spec.buttons));
    window.set_dialogVisible(true);
}

/// 收起消息框并清掉复选框态——镜像 Qt 侧每次 `exec()` 都是新对话框对象，
/// 勾选态不会残留到下一次。
pub fn hide_dialog(window: &AppWindow) {
    window.set_dialogVisible(false);
    window.set_dialogCheckboxLabel("".into());
    window.set_dialogCheckboxChecked(false);
}

/// 读回消息框第 `index` 个按钮的文案（调用方按文案分派，等价于 Qt 侧
/// `dialog.yesButton.clicked.connect(...)` 的按钮身份）。
pub fn dialog_button_text(window: &AppWindow, index: usize) -> Option<String> {
    use slint::Model;
    window
        .get_dialogButtons()
        .row_data(index)
        .map(|button| button.text.to_string())
}

/// 拖放载荷的扁平快照。
///
/// `data-transfer` 在 Slint 里不透明（`i-slint-compiler-1.18.1/
/// builtin_elements.rs` 只把它暴露给宿主语言），所以宿主在回调里一次读全，
/// 之后的判定仍走 `window::drop_decision` / `window::drop_urls`。
///
/// **与 Qt 的差异**：`mimeData().urls()` 能给任意 URL（浏览器拖链接也是），
/// `DataTransfer` 只给**本地文件路径**，故 `has_urls` 按
/// `has_file_paths()` 判定——非文件 URL 的拖放在 Slint 侧拿不到，
/// 需要等 slint 暴露 URL 列表能力再补。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DropPayload {
    pub file_paths: Vec<String>,
    pub text: Option<String>,
}

impl DropPayload {
    /// 从宿主拿到的 `data-transfer` 读一次（读操作可能有 I/O，故只读一遍）。
    pub fn from_transfer(data: &slint::DataTransfer) -> Self {
        let file_paths = data
            .file_paths()
            .map(|paths| {
                paths
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        let text = data.plain_text().ok().map(|text| text.to_string());
        Self { file_paths, text }
    }

    /// 镜像 `event.mimeData()` 的三件套（`hasUrls` / `hasText` / `text`）。
    pub fn drop_input(
        &self,
        is_file_drag_active: bool,
        mask_dialog_visible: bool,
    ) -> window::DropInput<'_> {
        window::DropInput {
            is_file_drag_active,
            has_urls: !self.file_paths.is_empty(),
            has_text: self.text.is_some(),
            text: self.text.as_deref().unwrap_or(""),
            mask_dialog_visible,
        }
    }
}

/// 外壳动作集（第 57 步）：导航 / 搜索 / 通知条 / 消息框 / 拖放。
///
/// 全部只带字面量（routeKey、按钮序号、勾选态、拖放载荷）——建任务、
/// 切页副作用、落盘、开浏览器都在调用方。
pub struct ShellActions {
    pub nav_item: Box<dyn Fn(&str)>,
    pub search: Box<dyn Fn(&str)>,
    pub info_bar_button: Box<dyn Fn(i32)>,
    pub info_bar_closed: Box<dyn Fn()>,
    pub dialog_button: Box<dyn Fn(i32)>,
    pub dialog_checkbox: Box<dyn Fn(bool)>,
    /// `can-drop`：载荷是否可接（调用方拼 `DropInput` 后问 `drop_decision`）。
    pub can_drop: Box<dyn Fn(&DropPayload) -> bool>,
    /// `dropped`：取址（`drop_urls`）后交给 `addUrls`。
    pub drop: Box<dyn Fn(&DropPayload)>,
}

/// 把外壳动作集注册到 AppWindow 的回调上。
pub fn install_shell_actions(window: &AppWindow, actions: std::rc::Rc<ShellActions>) {
    let nav_item = actions.clone();
    window.on_navItemClicked(move |route_key| (nav_item.nav_item)(&route_key));
    let search = actions.clone();
    window.on_searchEdited(move |text| (search.search)(&text));
    let info_bar_button = actions.clone();
    window.on_infoBarButtonClicked(move |index| (info_bar_button.info_bar_button)(index));
    let info_bar_closed = actions.clone();
    window.on_infoBarClosed(move || (info_bar_closed.info_bar_closed)());
    let dialog_button = actions.clone();
    window.on_dialogButtonClicked(move |index| (dialog_button.dialog_button)(index));
    let dialog_checkbox = actions.clone();
    window.on_dialogCheckboxToggled(move |checked| (dialog_checkbox.dialog_checkbox)(checked));
    let can_drop = actions.clone();
    window.on_canDropPayload(move |data| (can_drop.can_drop)(&DropPayload::from_transfer(&data)));
    let drop = actions.clone();
    window.on_dropPayload(move |data| (drop.drop)(&DropPayload::from_transfer(&data)));
}

/// 菜单动作集（第 59 步）：点中一项 / 关闭。
///
/// 与 [`ShellActions`] 同一原则：只回传「点了哪一项的什么意图」
/// （[`menu_view::CardMenuView::click`] 解析出的 [`card_menu::CardClick`]），
/// 真服务调用（剪贴板、建编辑框、`moveToFront` …）在调用方。
pub struct CardMenuActions {
    pub activated: Box<dyn Fn(&card_menu::CardClick)>,
    pub dismissed: Box<dyn Fn()>,
}

/// `menu_view::MenuRow` → Slint 的 `CardMenuRowData`。
///
/// `icon: None` 写成空串（Slint 没有 Option 属性，`""` = 没有图标位）。
pub fn card_menu_rows(rows: &[menu_view::MenuRow]) -> slint::ModelRc<CardMenuRowData> {
    let items: Vec<CardMenuRowData> = rows
        .iter()
        .map(|row| CardMenuRowData {
            kind: row.kind.code(),
            depth: i32::from(row.depth),
            text: row.text.as_str().into(),
            icon: row.icon.as_deref().unwrap_or("").into(),
            checked: row.checked,
            checkable: row.checkable,
            expanded: row.expanded,
        })
        .collect();
    slint::ModelRc::new(slint::VecModel::from(items))
}

/// 把视图摊平后的行与面板高度写进 UI（展开/收起后重画用）。
pub fn render_card_menu(window: &AppWindow, view: &menu_view::CardMenuView) {
    let rows = view.rows();
    window.set_cardMenuHeight(menu_view::panel_height(&rows));
    window.set_cardMenuRows(card_menu_rows(&rows));
}

/// 弹卡片右键菜单（镜像 `createContextMenu()` + `menu.exec(globalPos)`）。
///
/// 每次弹都新建视图与回调：Qt 侧 `createContextMenu()` 也是每次造一个新
/// `RoundMenu`，子菜单展开态不跨次残留。
///
/// 点击分派（`cardMenuActivated`）：
/// 1. 子菜单行 → [`menu_view::CardMenuView::toggle`] 切换展开并重画，菜单继续开着；
/// 2. 动作行 → 收起菜单 + 回调 [`CardMenuActions::activated`]；
/// 3. 分隔符 / 越界 → 什么都不做（Slint 侧已经不给分隔符发事件，这里兜底）。
pub fn show_card_menu(
    window: &AppWindow,
    view: menu_view::CardMenuView,
    x: i32,
    y: i32,
    actions: std::rc::Rc<CardMenuActions>,
) {
    render_card_menu(window, &view);
    window.set_cardMenuX(x);
    window.set_cardMenuY(y);
    window.set_cardMenuOpened(true);

    let state = std::rc::Rc::new(std::cell::RefCell::new(view));
    let weak = window.as_weak();
    let activated = actions.clone();
    window.on_cardMenuActivated(move |index| {
        if index < 0 {
            return;
        }
        let row = index as usize;
        // 子菜单行：切换展开态后重画，菜单不关。
        let toggled = state.borrow_mut().toggle(row);
        if toggled {
            if let Some(window) = weak.upgrade() {
                render_card_menu(&window, &state.borrow());
            }
            return;
        }
        let click = state.borrow().click(row);
        if click.is_none() {
            // 分隔符 / 越界：Qt 侧点了没反应，菜单也不关。
            return;
        }
        if let Some(window) = weak.upgrade() {
            hide_card_menu(&window);
        }
        if let Some(click) = click {
            (activated.activated)(&click);
        }
    });

    let dismissed = actions;
    let weak = window.as_weak();
    window.on_cardMenuDismissed(move || {
        if let Some(window) = weak.upgrade() {
            hide_card_menu(&window);
        }
        (dismissed.dismissed)();
    });
}

/// 收起卡片菜单并清掉行列表（镜像 `menu.close()`）。
pub fn hide_card_menu(window: &AppWindow) {
    window.set_cardMenuOpened(false);
    window.set_cardMenuHeight(0);
    window.set_cardMenuRows(slint::ModelRc::new(slint::VecModel::from(Vec::<
        CardMenuRowData,
    >::new())));
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
    ///
    /// 第 51 步：`TaskRow` 换成卡片字段集（信息行三段 + 状态行 + 进度条 +
    /// 四按钮），这里只锁「结构体字面量能进 model 再读回来」，语义断言在
    /// `row.rs` / `card.rs` 的单测与金文件里。
    #[test]
    fn task_page_rows_round_trip_headless() {
        let window = crate::create_headless();
        crate::set_task_rows(
            &window,
            vec![
                crate::TaskRow {
                    taskId: "tsk_1".into(),
                    name: "a.bin".into(),
                    status: "WAITING".into(),
                    speedText: "".into(),
                    speedVisible: false,
                    etaText: "".into(),
                    etaVisible: false,
                    sizeText: "0.00 B/--".into(),
                    sizeVisible: true,
                    statusLine: "任务正在等待".into(),
                    statusLineVisible: true,
                    statusWarn: false,
                    progressValue: 0,
                    progressIndeterminate: true,
                    progressVisible: true,
                    progressError: false,
                    toggleIcon: "PLAY".into(),
                    toggleEnabled: true,
                    selectFilesVisible: false,
                    checksumVisible: true,
                    checksumEnabled: false,
                    openFileEnabled: false,
                },
                crate::TaskRow {
                    taskId: "tsk_2".into(),
                    name: "b.iso".into(),
                    status: "RUNNING".into(),
                    speedText: "1.00 KB/s".into(),
                    speedVisible: true,
                    etaText: "3s".into(),
                    etaVisible: true,
                    sizeText: "512.00 B/4.00 KB".into(),
                    sizeVisible: true,
                    statusLine: "".into(),
                    statusLineVisible: false,
                    statusWarn: false,
                    progressValue: 25,
                    progressIndeterminate: false,
                    progressVisible: true,
                    progressError: false,
                    toggleIcon: "PAUSE".into(),
                    toggleEnabled: true,
                    selectFilesVisible: false,
                    checksumVisible: true,
                    checksumEnabled: false,
                    openFileEnabled: false,
                },
            ],
        );
        let model = window.get_tasks();
        assert_eq!(model.row_count(), 2);
        let first = model.row_data(0).expect("首行缺失");
        assert_eq!(first.taskId, "tsk_1");
        assert_eq!(first.name, "a.bin");
        assert_eq!(first.statusLine, "任务正在等待");
        assert!(first.statusLineVisible);
        assert_eq!(first.status, "WAITING");
        assert_eq!(first.progressValue, 0);
        assert!(first.progressIndeterminate);
        assert_eq!(first.toggleIcon, "PLAY");
        let second = model.row_data(1).expect("次行缺失");
        assert_eq!(second.taskId, "tsk_2");
        assert_eq!(second.speedText, "1.00 KB/s");
        assert_eq!(second.progressValue, 25);
        assert!(!second.statusLineVisible);
    }

    /// 端到端推流：TaskRecord（磁盘同态 JSON 构造）→ push_task_records
    /// → card.rs 派生层 → Slint model 逐段核对。
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
        // 运行时事实：只有 tsk_b 在「做种中」。
        crate::push_task_records(&window, &records, |record| crate::card::CardRuntime {
            is_seeding: record.task_id == "tsk_b",
            ..crate::card::CardRuntime::default()
        });
        let model = window.get_tasks();
        assert_eq!(model.row_count(), 2);
        let first = model.row_data(0).expect("首行缺失");
        assert_eq!(first.taskId, "tsk_a");
        assert_eq!(first.speedText, "1.00 KB/s");
        assert_eq!(first.etaText, "3s");
        assert_eq!(first.sizeText, "512.00 B/4.00 KB");
        // RUNNING → 状态行隐藏、进度条显示、toggle 变 PAUSE。
        assert!(!first.statusLineVisible);
        assert!(first.progressVisible);
        assert_eq!(first.progressValue, 50);
        assert_eq!(first.status, "RUNNING");
        assert_eq!(first.toggleIcon, "PAUSE");
        let second = model.row_data(1).expect("次行缺失");
        // COMPLETED 无 completedAt → 「任务已经完成」；做种中 → toggle 变 PAUSE。
        assert_eq!(second.statusLine, "任务已经完成");
        assert!(second.statusLineVisible);
        assert!(!second.progressVisible);
        assert_eq!(second.sizeText, "1.00 MB");
        assert_eq!(second.toggleIcon, "PAUSE");
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
            |_| crate::card::CardRuntime::default(),
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
            |_| crate::card::CardRuntime::default(),
        );
        assert_eq!(window.get_tasks().row_count(), 0);
    }

    /// 回调注册冒烟：install_task_actions 把四个动作挂上不 panic
    /// （生成名 on_searchChanged 等保留 slint 命名）。
    ///
    /// 第 51 步：卡片按钮五路（toggle / select_files / checksum / open_file /
    /// open_folder）一并注册，并用 `invoke_*` 逐路打一遍验证回调真的连上。
    #[test]
    fn install_task_actions_registers_handlers_headless() {
        use std::cell::RefCell;
        use std::rc::Rc;

        let window = crate::create_headless();
        let hits: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let record = |tag: &'static str, hits: Rc<RefCell<Vec<String>>>| {
            Box::new(move |task_id: &str| hits.borrow_mut().push(format!("{tag}:{task_id}")))
        };
        let actions = Rc::new(crate::TaskActions {
            search: Box::new({
                let h = hits.clone();
                move |s| h.borrow_mut().push(s.into())
            }),
            filter_mode: Box::new(|_| {}),
            category_selected: Box::new(|_| {}),
            sort_field: Box::new(|_| {}),
            sort_order: Box::new(|_| {}),
            pause: record("pause", hits.clone()),
            delete: record("delete", hits.clone()),
            toggle: record("toggle", hits.clone()),
            select_files: record("select_files", hits.clone()),
            checksum: record("checksum", hits.clone()),
            open_file: record("open_file", hits.clone()),
            open_folder: record("open_folder", hits.clone()),
            open_drawer: record("open_drawer", hits.clone()),
        });
        crate::install_task_actions(&window, actions);
        // 注册即通路：逐路 invoke 一次，断言动作集被按 taskId 打到。
        window.invoke_taskPause("t1".into());
        window.invoke_taskDelete("t1".into());
        window.invoke_taskToggle("t2".into());
        window.invoke_taskSelectFiles("t2".into());
        window.invoke_taskChecksum("t2".into());
        window.invoke_taskOpenFile("t2".into());
        window.invoke_taskOpenFolder("t2".into());
        window.invoke_taskOpenDrawer("t2".into());
        assert_eq!(
            *hits.borrow(),
            vec![
                "pause:t1",
                "delete:t1",
                "toggle:t2",
                "select_files:t2",
                "checksum:t2",
                "open_file:t2",
                "open_folder:t2",
                "open_drawer:t2",
            ]
        );
    }

    /// 第 53 步：`open_task_drawer` 一次把五张卡片派生结果装配到窗口。
    ///
    /// 逐段核对 Header / Error / Info / Checksum / FileList 的属性与 model
    /// 行内容（派生语义本身由 `drawer.rs` 的 48 条金文件锁定，这里锁
    /// 「派生结果 → Slint 属性/model」这条通路）。
    #[test]
    fn open_task_drawer_assembles_states_headless() {
        use gd3_core::task::TaskRecord;
        use serde_json::json;

        let record: TaskRecord = serde_json::from_value(json!({
            "name": "big.iso", "url": "https://e/1", "packId": "pack",
            "taskId": "t1", "status": "RUNNING", "fileSize": 4096,
            "createdAt": 0,
            "steps": [{"stepIndex": 0, "progress": 25, "speed": 1024, "receivedBytes": 1024}],
            "checksums": {"sha256": "abc"},
        }))
        .expect("TaskRecord 反序列化失败");
        let runtime = crate::drawer::DrawerRuntime {
            output_path: "/downloads/big.iso".into(),
            utc_offset_seconds: 0,
            category_enabled: true,
            category_name: Some("视频".into()),
            expected_checksum: " abc ".into(),
            ..crate::drawer::DrawerRuntime::default()
        };
        let window = crate::create_headless();
        crate::open_task_drawer(&window, &record, &runtime);

        // HeaderCard
        assert!(window.get_drawerVisible());
        assert_eq!(window.get_drawerTaskId(), "t1");
        assert_eq!(window.get_drawerTaskName(), "big.iso");
        assert_eq!(window.get_drawerProgressValue(), 25);
        assert!(window.get_drawerProgressVisible());
        assert!(!window.get_drawerProgressError());
        assert_eq!(
            window.get_drawerStatusText(),
            "1.00 KB/s · 1.00 KB / 4.00 KB · 3s"
        );

        // ErrorBar（非 FAILED → 整条隐藏）
        assert!(!window.get_drawerErrorVisible());
        assert_eq!(window.get_drawerErrorTitle(), "下载失败");
        assert_eq!(window.get_drawerErrorText(), "");

        // InfoCard：六行，顺序与 `_initLayout` 的 addWidget 顺序一致。
        let rows = window.get_drawerInfoRows();
        assert_eq!(rows.row_count(), 6);
        let row = |index: usize| rows.row_data(index).expect("信息行缺失");
        let expected = [
            ("url", "下载链接", "https://e/1", true),
            ("path", "保存位置", "/downloads/big.iso", true),
            ("size", "大小", "4.00 KB", true),
            ("category", "分类", "视频", true),
            ("created", "创建时间", "1970-01-01 00:00:00", true),
            ("completed", "完成时间", "", false),
        ];
        for (index, (key, caption, value, shown)) in expected.iter().enumerate() {
            let row = row(index);
            assert_eq!(row.key, *key, "第 {index} 行 key");
            assert_eq!(row.caption, *caption, "第 {index} 行 caption");
            assert_eq!(row.value, *value, "第 {index} 行 value");
            assert_eq!(row.shown, *shown, "第 {index} 行 shown");
        }

        // ChecksumCard：命中 sha256 → 无错误、色点亮、提示带大写算法名。
        assert!(window.get_drawerChecksumVisible());
        let checksums = window.get_drawerChecksumRows();
        assert_eq!(checksums.row_count(), 1);
        let checksum = checksums.row_data(0).expect("校验行缺失");
        assert_eq!(checksum.algorithm, "SHA256");
        assert_eq!(checksum.value, "abc");
        assert!(!window.get_drawerChecksumError());
        assert!(window.get_drawerChecksumMatched());
        assert_eq!(window.get_drawerChecksumTooltip(), "与 SHA256 一致");
        // `expectedEdit.text()` 由运行时值初始化（原样，不 strip）。
        assert_eq!(window.get_drawerExpectedChecksum(), " abc ");

        // FileListCard：单文件 → `len(files) <= 1` → 不可见且无行。
        assert!(!window.get_drawerFileListVisible());
        assert_eq!(window.get_drawerFileRows().row_count(), 0);
    }

    /// 第 53 步：`close_task_drawer` 收敛到「不可见 + taskId 清空」。
    #[test]
    fn close_task_drawer_hides_and_clears_task_id_headless() {
        let window = crate::create_headless();
        window.set_drawerVisible(true);
        window.set_drawerTaskId("t9".into());
        crate::close_task_drawer(&window);
        assert!(!window.get_drawerVisible());
        assert_eq!(window.get_drawerTaskId(), "");
    }

    /// 第 55 步端到端：`facts::card_runtime` 供事实 → `push_task_records` 成行。
    ///
    /// 锁的是「记录 → 运行时事实（类属性登记表 + 文件系统 + 活状态）→ Slint
    /// 行」整条链；单点语义由 `facts.rs` 的两份金文件与 `card.rs` 覆盖。
    #[test]
    fn facts_runtime_drives_task_rows_headless() {
        use crate::facts::LiveFacts;
        use gd3_core::task::TaskRecord;
        use serde_json::json;

        // 造一个真实存在的输出文件，让 `isExisting(outputPath)` 走到真。
        let folder = std::env::temp_dir().join("gd3-facts-e2e");
        std::fs::create_dir_all(&folder).expect("临时目录创建失败");
        std::fs::write(folder.join("movie.bin"), b"x").expect("临时文件写入失败");
        let folder = folder.to_string_lossy().into_owned();

        let records: Vec<TaskRecord> = serde_json::from_value(json!([
            {
                // 做种包 + COMPLETED + 输出文件存在 → 可做种、三个按钮都可点。
                "name": "movie.bin", "url": "magnet:?xt=1", "packId": "bittorrent",
                "taskId": "t_bt", "status": "COMPLETED", "fileSize": 4096,
                "outputFolder": folder, "completedAt": 1_700_000_000,
                "type": "bittorrent_pack.task.BTTask",
            },
            {
                // `HttpTaskStep.canPause` 读 `canUseRangeRequests`（缺省 False）
                // → RUNNING 卡的暂停按钮禁用。
                "name": "r.bin", "url": "https://e/2", "packId": "http",
                "taskId": "t_http", "status": "RUNNING", "fileSize": 8192,
                "outputFolder": folder,
                "steps": [{
                    "stepIndex": 0, "status": "RUNNING", "progress": 40,
                    "speed": 2048, "receivedBytes": 3277,
                    "type": "http_pack.task.HttpTaskStep",
                }],
                "type": "http_pack.task.HttpTask",
            },
            {
                // 输出文件不存在 → `file_missing()` → 「文件不存在」+ 三个按钮全禁用。
                "name": "gone.bin", "url": "https://e/3", "packId": "http",
                "taskId": "t_missing", "status": "COMPLETED", "fileSize": 1024,
                "outputFolder": folder, "completedAt": 1_700_000_000,
                "type": "http_pack.task.HttpTask",
            },
        ]))
        .expect("TaskRecord 数组反序列化失败");

        let window = crate::create_headless();
        crate::push_task_records(&window, &records, |record| {
            crate::facts::card_runtime(record, &LiveFacts::default(), 0)
        });

        let model = window.get_tasks();
        assert_eq!(model.row_count(), 3);

        // 做种包：COMPLETED 无做种 → 播放图标；输出存在 → 按钮可点。
        let bt = model.row_data(0).expect("首行缺失");
        assert_eq!(bt.taskId, "t_bt");
        assert_eq!(bt.status, "COMPLETED");
        assert_eq!(bt.toggleIcon, "PLAY");
        assert!(bt.toggleEnabled);
        assert!(bt.checksumEnabled);
        assert!(bt.openFileEnabled);
        assert!(!bt.progressVisible);
        // completedAt 走 `facts` 的时区参数（这里偏移 0）。
        assert_eq!(bt.statusLine, "完成于 2023-11-14 22:13:20");
        assert!(!bt.statusWarn);

        // 非做种包 + RUNNING：暂停按钮被 `canUseRangeRequests=False` 禁用。
        let http = model.row_data(1).expect("次行缺失");
        assert_eq!(http.taskId, "t_http");
        assert_eq!(http.toggleIcon, "PAUSE");
        assert!(!http.toggleEnabled);
        assert!(!http.statusLineVisible);
        assert!(http.progressVisible);
        assert_eq!(http.progressValue, 40);
        assert_eq!(http.speedText, "2.00 KB/s");
        assert_eq!(http.etaText, "2s");
        assert_eq!(http.sizeText, "3.20 KB/8.00 KB");
        // 非 COMPLETED → 校验/打开文件都不可点。
        assert!(!http.checksumEnabled);
        assert!(!http.openFileEnabled);

        // 输出文件不存在：`is_existing` 真的查了文件系统。
        let missing = model.row_data(2).expect("末行缺失");
        assert_eq!(missing.taskId, "t_missing");
        assert_eq!(missing.statusLine, "文件不存在");
        assert!(missing.statusWarn);
        assert!(!missing.checksumEnabled);
        assert!(!missing.openFileEnabled);
    }

    /// 第 53 步：抽屉五路回调注册即通路（逐路 `invoke_*` 断言动作被按
    /// taskId / 文本打到）。
    #[test]
    fn install_drawer_actions_registers_handlers_headless() {
        use std::cell::RefCell;
        use std::rc::Rc;

        let window = crate::create_headless();
        let hits: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let actions = Rc::new(crate::DrawerActions {
            close: {
                let h = hits.clone();
                Box::new(move |task_id| h.borrow_mut().push(format!("close:{task_id}")))
            },
            expected_edited: {
                let h = hits.clone();
                Box::new(move |task_id, text| {
                    h.borrow_mut().push(format!("expected:{task_id}:{text}"))
                })
            },
            paste_checksum: {
                let h = hits.clone();
                Box::new(move |task_id| h.borrow_mut().push(format!("paste:{task_id}")))
            },
            copy_text: {
                let h = hits.clone();
                Box::new(move |task_id, text| h.borrow_mut().push(format!("copy:{task_id}:{text}")))
            },
            reveal_in_folder: {
                let h = hits.clone();
                Box::new(move |task_id| h.borrow_mut().push(format!("reveal:{task_id}")))
            },
        });
        crate::install_drawer_actions(&window, actions);
        window.invoke_drawerCloseRequested("t1".into());
        window.invoke_drawerExpectedChecksumEdited("t1".into(), "abc".into());
        window.invoke_drawerPasteChecksumRequested("t1".into());
        window.invoke_drawerCopyTextRequested("t1".into(), "https://e/1".into());
        window.invoke_drawerRevealInFolderRequested("t1".into());
        assert_eq!(
            *hits.borrow(),
            vec![
                "close:t1",
                "expected:t1:abc",
                "paste:t1",
                "copy:t1:https://e/1",
                "reveal:t1",
            ]
        );
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
        // 第 42 步：标头预设（JSON 串两预设 + 索引指到第二项）。
        config.set_str(
            "Network",
            "HeadersPresets",
            r#"[{"name":"A","headers":{"x-a":"1"}},
                {"name":"B","headers":{"y-b":"2","z-c":"3"}}]"#,
        );
        config.set_i64("Network", "CurrentHeadersPreset", 1);
        // 第 43 步：身份预设（JSON 列表两项：完整百度预设 + 精简项）。
        config.set_str(
            "Network",
            "IdentityPresets",
            r#"[{"name":"百度","clientProfile":"chrome",
                 "userAgent":"pan.baidu.com","hosts":["h1","h2","h3","h4"],
                 "isEnabled":false},
                {"name":"B","clientProfile":"raw","userAgent":"",
                 "hosts":[]}]"#,
        );
        // 第 44 步：库分类（JSON 两项：自定义项 + 缺 categoryId 项）。
        config.set_str(
            "Category",
            "CategoryRules",
            r#"[{"categoryId":"x","name":"自定义","icon":"",
                 "extensions":[".MP4","mkv","mkv"],"folder":""},
                {"name":"无 id"}]"#,
        );
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
        // 第 42 步：有值推送（JSON 串解析：预设 A/B、当前索引 1、
        // 计数取选中项 2 条、多预设可删）。
        let preset_names = window.get_headersPresetNames();
        assert_eq!(preset_names.row_count(), 2);
        assert_eq!(preset_names.row_data(0).as_deref(), Some("A"));
        assert_eq!(preset_names.row_data(1).as_deref(), Some("B"));
        assert_eq!(window.get_headersPresetIndex(), 1);
        assert_eq!(window.get_headersPresetHeaderCount(), 2);
        assert!(window.get_headersPresetCanRemove());
        // 第 43 步：有值推送（行模型逐行核对：名称/描述/开关）。
        let identity_rows = window.get_identityPresets();
        assert_eq!(identity_rows.row_count(), 2);
        assert_eq!(identity_rows.row_data(0).as_ref().unwrap().name, "百度");
        assert_eq!(
            identity_rows.row_data(0).as_ref().unwrap().desc,
            "h1, h2, h3 · Chrome（最新） · pan.baidu.com"
        );
        assert!(!identity_rows.row_data(0).as_ref().unwrap().enabled);
        assert_eq!(identity_rows.row_data(1).as_ref().unwrap().name, "B");
        assert_eq!(
            identity_rows.row_data(1).as_ref().unwrap().desc,
            "不模拟（原样发送）"
        );
        assert!(identity_rows.row_data(1).as_ref().unwrap().enabled);
        // 第 44 步：有值推送（逐项 fromDict 归一化：去点/小写/去重、
        // icon 缺省 DOCUMENT、folder 空串 → None、缺 id 保留空串）。
        let category_rows = window.get_categoryRows();
        assert_eq!(category_rows.row_count(), 2);
        assert_eq!(category_rows.row_data(0).as_ref().unwrap().name, "自定义");
        assert_eq!(category_rows.row_data(0).as_ref().unwrap().desc, "mp4, mkv");
        assert_eq!(category_rows.row_data(1).as_ref().unwrap().id, "");
        assert_eq!(window.get_categoryRuleCount(), 2);
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
        // 第 42 步：缺省容错（空串 → 「默认」+BASE_HEADERS 8 条、
        // 单预设不可删、索引 0）。
        let default_names = window.get_headersPresetNames();
        assert_eq!(default_names.row_count(), 1);
        assert_eq!(default_names.row_data(0).as_deref(), Some("默认"));
        assert_eq!(window.get_headersPresetIndex(), 0);
        assert_eq!(window.get_headersPresetHeaderCount(), 8);
        assert!(!window.get_headersPresetCanRemove());
        // 第 46 步：缺省容错（编辑覆盖层收起、索引 -1=添加）。
        assert!(!window.get_headersEditVisible());
        assert_eq!(window.get_headersEditIndex(), -1);
        // 第 48 步：缺省无重复标头警告。
        assert_eq!(window.get_headersEditDuplicateWarning(), "");
        // 第 43 步：缺省容错（缺键 → 百度网盘客户端预设单行，启用）。
        let identity_rows = window.get_identityPresets();
        assert_eq!(identity_rows.row_count(), 1);
        assert_eq!(
            identity_rows.row_data(0).as_ref().unwrap().name,
            "百度网盘客户端"
        );
        assert_eq!(
            identity_rows.row_data(0).as_ref().unwrap().desc,
            "*.pcs.baidu.com · 不模拟（原样发送） · pan.baidu.com"
        );
        assert!(identity_rows.row_data(0).as_ref().unwrap().enabled);
        // 第 47 步：缺省容错（身份预设编辑覆盖层收起、索引 -1=添加）。
        // profile labels/values 双模型不在此推——它们只在开框时由
        // open_identity_preset_edit 按当前值现推（含未知字面量追加项）。
        assert!(!window.get_identityEditVisible());
        assert_eq!(window.get_identityEditIndex(), -1);
        assert_eq!(window.get_identityEditProfileLabels().row_count(), 0);
        // 第 44 步：缺省容错（缺键 → CATEGORY_PRESETS 八预设逐字，
        // 计数 8）。
        let category_rows = window.get_categoryRows();
        assert_eq!(category_rows.row_count(), 8);
        assert_eq!(category_rows.row_data(0).as_ref().unwrap().name, "视频");
        assert_eq!(
            category_rows.row_data(0).as_ref().unwrap().desc,
            "mp4, mkv, avi, mov 等 20 项"
        );
        assert_eq!(category_rows.row_data(7).as_ref().unwrap().name, "其他");
        assert_eq!(category_rows.row_data(7).as_ref().unwrap().desc, "无扩展名");
        assert_eq!(window.get_categoryRuleCount(), 8);
        // 第 45 步：缺省容错（图标 choices 25 项逐字、覆盖层收起）。
        assert_eq!(window.get_categoryIconChoices().row_count(), 25);
        assert!(!window.get_categoryEditVisible());

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
        // 第 42 步：标头预设动作命中记录。
        let headers_pick_hits: Rc<RefCell<Vec<i64>>> = Rc::new(RefCell::new(Vec::new()));
        let headers_remove_hits: Rc<RefCell<Vec<i64>>> = Rc::new(RefCell::new(Vec::new()));
        // 第 46 步：标头预设添加/编辑动作命中记录。
        let headers_add_hits: Rc<RefCell<Vec<()>>> = Rc::new(RefCell::new(Vec::new()));
        let headers_edit_hits: Rc<RefCell<Vec<i64>>> = Rc::new(RefCell::new(Vec::new()));
        let headers_close_hits: Rc<RefCell<Vec<()>>> = Rc::new(RefCell::new(Vec::new()));
        let headers_submit_hits: Rc<RefCell<Vec<(i64, crate::headers::HeaderPreset)>>> =
            Rc::new(RefCell::new(Vec::new()));
        // 第 43 步：身份预设动作命中记录。
        let identity_toggle_hits: Rc<RefCell<Vec<(i64, bool)>>> = Rc::new(RefCell::new(Vec::new()));
        let identity_remove_hits: Rc<RefCell<Vec<i64>>> = Rc::new(RefCell::new(Vec::new()));
        let identity_add_hits: Rc<RefCell<Vec<()>>> = Rc::new(RefCell::new(Vec::new()));
        let identity_edit_hits: Rc<RefCell<Vec<i64>>> = Rc::new(RefCell::new(Vec::new()));
        let identity_close_hits: Rc<RefCell<Vec<()>>> = Rc::new(RefCell::new(Vec::new()));
        let identity_submit_hits: Rc<RefCell<Vec<(i64, crate::identity::IdentityPreset)>>> =
            Rc::new(RefCell::new(Vec::new()));
        // 第 44 步：库分类动作命中记录。
        let category_reset_hits: Rc<RefCell<Vec<()>>> = Rc::new(RefCell::new(Vec::new()));
        let category_remove_hits: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        // 第 45 步：分类添加/编辑动作命中记录（提交收归一化规则）。
        let category_add_hits: Rc<RefCell<Vec<()>>> = Rc::new(RefCell::new(Vec::new()));
        let category_edit_hits: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let category_close_hits: Rc<RefCell<Vec<()>>> = Rc::new(RefCell::new(Vec::new()));
        let category_submit_hits: Rc<RefCell<Vec<crate::category::CategoryRule>>> =
            Rc::new(RefCell::new(Vec::new()));
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
            // 第 42 步：标头预设动作收索引原样（验证通路）。
            headers_preset_pick: Box::new({
                let h = headers_pick_hits.clone();
                move |i| h.borrow_mut().push(i)
            }),
            headers_preset_remove: Box::new({
                let h = headers_remove_hits.clone();
                move |i| h.borrow_mut().push(i)
            }),
            // 第 46 步：添加/编辑入口与确定草稿（采集调用发生即可）。
            headers_preset_add_requested: Box::new({
                let h = headers_add_hits.clone();
                move || h.borrow_mut().push(())
            }),
            headers_preset_edit_requested: Box::new({
                let h = headers_edit_hits.clone();
                move |i| h.borrow_mut().push(i)
            }),
            headers_preset_edit_closed: Box::new({
                let h = headers_close_hits.clone();
                move || h.borrow_mut().push(())
            }),
            headers_preset_edit_submit: Box::new({
                let h = headers_submit_hits.clone();
                move |pair| h.borrow_mut().push(pair)
            }),
            // 第 43 步：身份预设动作（开关收 (索引, bool) 验证通路）。
            identity_preset_toggle: Box::new({
                let h = identity_toggle_hits.clone();
                move |i, v| h.borrow_mut().push((i, v))
            }),
            identity_preset_remove: Box::new({
                let h = identity_remove_hits.clone();
                move |i| h.borrow_mut().push(i)
            }),
            // 第 47 步：身份预设添加/编辑入口与确定草稿。
            identity_preset_add_requested: Box::new({
                let h = identity_add_hits.clone();
                move || h.borrow_mut().push(())
            }),
            identity_preset_edit_requested: Box::new({
                let h = identity_edit_hits.clone();
                move |i| h.borrow_mut().push(i)
            }),
            identity_preset_edit_closed: Box::new({
                let h = identity_close_hits.clone();
                move || h.borrow_mut().push(())
            }),
            identity_preset_edit_submit: Box::new({
                let h = identity_submit_hits.clone();
                move |pair| h.borrow_mut().push(pair)
            }),
            // 第 44 步：库分类动作（通路验证）。
            category_reset: Box::new({
                let h = category_reset_hits.clone();
                move || h.borrow_mut().push(())
            }),
            category_remove: Box::new({
                let h = category_remove_hits.clone();
                move |s| h.borrow_mut().push(s)
            }),
            // 第 45 步：添加/编辑动作（提交收 draft_to_rule 归一化产物）。
            category_add_requested: Box::new({
                let h = category_add_hits.clone();
                move || h.borrow_mut().push(())
            }),
            category_edit_requested: Box::new({
                let h = category_edit_hits.clone();
                move |s| h.borrow_mut().push(s)
            }),
            category_edit_closed: Box::new({
                let h = category_close_hits.clone();
                move || h.borrow_mut().push(())
            }),
            category_edit_submit: Box::new({
                let h = category_submit_hits.clone();
                move |rule| h.borrow_mut().push(rule)
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
        // 第 42 步：标头预设 invoke 通路（选中/删除索引原样，镜像
        // _onPick 写 CurrentHeadersPreset、_onRemoveClicked 移除）。
        window.invoke_headersPresetPicked(1);
        assert_eq!(headers_pick_hits.borrow().last().copied(), Some(1));
        window.invoke_headersPresetRemoveRequested(0);
        assert_eq!(headers_remove_hits.borrow().last().copied(), Some(0));
        // 第 46 步：添加/编辑 invoke 通路；确定草稿 → draft_to_header_
        // preset 归一化预设（名称 strip + 逐行 parseHeaderLine 拆对，
        // 无冒号/名称含空白行镜像丢弃）。
        window.invoke_headersPresetAddRequested();
        assert_eq!(headers_add_hits.borrow().len(), 1);
        window.invoke_headersPresetEditRequested(0);
        assert_eq!(headers_edit_hits.borrow().last().copied(), Some(0));
        window.invoke_headersPresetEditAccepted(crate::HeadersEditDraft {
            index: 0,
            name: "  A2  ".into(),
            pairs: "accept-encoding: deflate, br, gzip\r\nx-a: 1\nc:\n".into(),
        });
        let (submitted_i, submitted_p) = headers_submit_hits.borrow().last().cloned().unwrap();
        assert_eq!(submitted_i, 0);
        assert_eq!(submitted_p.name, "A2");
        // 第 48 步：标头走 `headers()` 口径——空值行（`c:`）被丢弃。
        assert_eq!(
            submitted_p.headers,
            vec![
                (
                    "accept-encoding".to_string(),
                    "deflate, br, gzip".to_string()
                ),
                ("x-a".to_string(), "1".to_string()),
            ]
        );
        // 第 48 步：文本编辑回调实时重算重复标头警告（纯呈现层派生）。
        window.invoke_headersPresetPairsEdited("x-a: 1\nX-A: 2".into());
        assert_eq!(
            window.get_headersEditDuplicateWarning(),
            "重复标头：x-a（后一条覆盖前一条）"
        );
        window.invoke_headersPresetPairsEdited("x-a: 1".into());
        assert_eq!(window.get_headersEditDuplicateWarning(), "");
        window.invoke_headersPresetEditClosed();
        assert_eq!(headers_close_hits.borrow().len(), 1);
        // 第 43 步：身份预设 invoke 通路（开关 (索引, bool)、删除索引）。
        window.invoke_identityPresetToggled(1, false);
        assert_eq!(
            identity_toggle_hits.borrow().last().copied(),
            Some((1, false))
        );
        window.invoke_identityPresetRemoveRequested(0);
        assert_eq!(identity_remove_hits.borrow().last().copied(), Some(0));
        // 第 47 步：身份预设添加/编辑 invoke 通路——「确定」先过
        // validate() 口径（hosts 归一化后判空）：拒绝时只置 hostsError、
        // 不收起、不发事件；通过后清错、收起覆盖层并回发归一化预设。
        window.invoke_identityPresetAddRequested();
        assert_eq!(identity_add_hits.borrow().len(), 1);
        window.invoke_identityPresetEditRequested(2);
        assert_eq!(identity_edit_hits.borrow().last().copied(), Some(2));
        window.set_identityEditVisible(true);
        window.invoke_identityPresetEditAccepted(crate::IdentityEditDraft {
            index: 1,
            name: "  我的预设  ".into(),
            hosts: "  , ; \n".into(),
            profile: "raw".into(),
            userAgent: "  UA  ".into(),
        });
        assert!(identity_submit_hits.borrow().is_empty(), "无 host 不应提交");
        assert!(window.get_identityEditHostsError(), "应置 hosts 错误");
        assert!(window.get_identityEditVisible(), "校验失败不收起覆盖层");
        window.invoke_identityPresetEditAccepted(crate::IdentityEditDraft {
            index: 1,
            name: "  我的预设  ".into(),
            hosts: "a.com\n*.b.com  a.com".into(),
            profile: "raw".into(),
            userAgent: "  UA  ".into(),
        });
        let (submitted_i, submitted_p) = identity_submit_hits.borrow().last().cloned().unwrap();
        assert_eq!(submitted_i, 1);
        assert_eq!(submitted_p.name, "我的预设", "名称 py_strip");
        assert_eq!(submitted_p.client_profile, "raw");
        assert_eq!(submitted_p.user_agent, "UA", "UA py_strip");
        assert_eq!(submitted_p.hosts, vec!["a.com", "*.b.com"], "去重保序");
        assert!(submitted_p.is_enabled, "草稿态恒 True，编辑态由调用方继承");
        assert!(!window.get_identityEditHostsError(), "通过后清错");
        assert!(!window.get_identityEditVisible(), "通过后收起覆盖层");
        window.invoke_identityPresetEditClosed();
        assert_eq!(identity_close_hits.borrow().len(), 1);
        // 第 44 步：库分类 invoke 通路（恢复默认 ()、删除收 id 原样）。
        window.invoke_categoryResetRequested();
        assert_eq!(category_reset_hits.borrow().len(), 1);
        window.invoke_categoryRemoveRequested("x".into());
        assert_eq!(
            category_remove_hits.borrow().last().map(|s| s.as_str()),
            Some("x")
        );
        // 第 45 步：添加/编辑 invoke 通路；draft_to_rule 归一化产物
        // 断言（名称空兜/扩展名去点去空/不存在项）。图标 choices
        // 25 项先落在有值推送段（push 断言见缺省段）。
        window.invoke_categoryAddRequested();
        assert_eq!(category_add_hits.borrow().len(), 1);
        window.invoke_categoryEditRequested("x".into());
        assert_eq!(
            category_edit_hits.borrow().last().map(|s| s.as_str()),
            Some("x")
        );
        window.invoke_categoryEditAccepted(crate::CategoryEditDraft {
            id: "x".into(),
            name: "自定义".into(),
            icon: "DOCUMENT".into(),
            extensions: ".MP4, mkv, tar.gz".into(),
            folder: "{default}/X".into(),
        });
        let got = category_submit_hits.borrow().last().cloned().unwrap();
        assert_eq!(got.name, "自定义");
        assert_eq!(got.icon, "DOCUMENT");
        assert_eq!(
            got.extensions,
            vec!["mp4".to_string(), "mkv".to_string(), "tar.gz".to_string()]
        );
        assert_eq!(got.folder.as_deref(), Some("{default}/X"));
        assert_eq!(got.category_id, "x");
        window.invoke_categoryEditClosed();
        assert_eq!(category_close_hits.borrow().len(), 1);
        window.set_downloadFolder("/edited/path".into());
        assert_eq!(window.get_downloadFolder(), "/edited/path");
    }

    /// 第 45 步：open_category_edit 带值/添加两态直写字段+置可见。
    #[test]
    fn open_category_edit_form_states_headless() {
        use gd3_core::config::UserConfig;

        let window = crate::create_headless();
        crate::push_settings(&window, &UserConfig::new(), "/default/download");
        crate::open_category_edit(&window, None);
        assert!(window.get_categoryEditVisible());
        assert_eq!(window.get_categoryEditId(), "");
        assert_eq!(window.get_categoryEditName(), "");
        assert_eq!(window.get_categoryEditIconIndex(), 0);

        crate::open_category_edit(
            &window,
            Some(&crate::category::CategoryRule {
                category_id: "x".into(),
                name: "自定义".into(),
                icon: "VIDEO".into(),
                extensions: vec!["mp4".into(), "mkv".into()],
                folder: Some("{default}/V".into()),
            }),
        );
        assert_eq!(window.get_categoryEditId(), "x");
        assert_eq!(window.get_categoryEditName(), "自定义");
        assert_eq!(window.get_categoryEditIconIndex(), 2, "VIDEO 下标 2");
        assert_eq!(window.get_categoryEditExtensions(), "mp4,mkv");
        assert_eq!(window.get_categoryEditFolder(), "{default}/V");

        // 预选镜像 findData：未知 icon 落首项；缺 folder 空串。
        crate::open_category_edit(
            &window,
            Some(&crate::category::CategoryRule {
                category_id: "y".into(),
                name: "n".into(),
                icon: "NOT_EXIST".into(),
                extensions: vec![],
                folder: None,
            }),
        );
        assert_eq!(window.get_categoryEditIconIndex(), 0);
        assert_eq!(window.get_categoryEditFolder(), "");
    }

    /// 第 46 步：open_headers_preset_edit 两态直写（添加=空名+当前
    /// 选中预设的标头起点；编辑=带 index 的带值回显）。
    #[test]
    fn open_headers_preset_edit_form_states_headless() {
        use gd3_core::config::UserConfig;

        let window = crate::create_headless();
        let mut config = UserConfig::new();
        config.set_str(
            "Network",
            "HeadersPresets",
            r#"[{"name": "A", "headers": {"x-a": "1"}},
                {"name": "B", "headers": {"y-b": "2", "z-c": "3"}}]"#,
        );
        config.set_i64("Network", "CurrentHeadersPreset", 1);
        // 添加（镜像 _onAddClicked：名称空 + currentHeaders() 起点）。
        crate::open_headers_preset_edit(&window, &config, None);
        assert!(window.get_headersEditVisible());
        assert_eq!(window.get_headersEditIndex(), -1);
        assert_eq!(window.get_headersEditName(), "");
        // 第 48 步：`toHeadersText` 末尾不带换行（Python `"\n".join`）。
        assert_eq!(
            window.get_headersEditPairs(),
            "y-b: 2\nz-c: 3",
            "添加起点应为选中预设 B 的标头回显"
        );
        // 编辑（镜像 _onEditClicked：预设[index] 带值回显）。
        crate::open_headers_preset_edit(&window, &config, Some(0));
        assert_eq!(window.get_headersEditIndex(), 0);
        assert_eq!(window.get_headersEditName(), "A");
        assert_eq!(window.get_headersEditPairs(), "x-a: 1");
    }

    /// 第 47 步：open_identity_preset_edit 两态直写（添加=全空 + 「跟随
    /// 全局默认」；编辑=带值回显，未知 profile 字面量追加一项）。
    #[test]
    fn open_identity_preset_edit_form_states_headless() {
        use gd3_core::config::UserConfig;

        let window = crate::create_headless();
        let mut config = UserConfig::new();
        config.set_str(
            "Network",
            "IdentityPresets",
            r#"[{"name": "A", "clientProfile": "raw", "userAgent": "UA-A",
                 "hosts": ["a.com", "*.b.com"], "isEnabled": false},
                {"name": "B", "clientProfile": "chrome120", "userAgent": "",
                 "hosts": ["b.com"]}]"#,
        );
        // 添加（镜像 PresetEditDialog(preset=None)：字段全空、profile ""）。
        crate::open_identity_preset_edit(&window, &config, None);
        assert!(window.get_identityEditVisible());
        assert_eq!(window.get_identityEditIndex(), -1);
        assert_eq!(window.get_identityEditName(), "");
        assert_eq!(window.get_identityEditHosts(), "");
        assert_eq!(window.get_identityEditUserAgent(), "");
        assert_eq!(window.get_identityEditProfile(), "");
        assert_eq!(window.get_identityEditProfileIndex(), 0);
        // 菜单模型镜像 buildProfileMenu(includeAuto=False)：11 项，首项
        // 「跟随全局默认」（磁盘空串字面量）。
        assert_eq!(window.get_identityEditProfileLabels().row_count(), 11);
        assert_eq!(
            window
                .get_identityEditProfileLabels()
                .row_data(0)
                .as_deref(),
            Some("跟随全局默认")
        );
        assert_eq!(
            window
                .get_identityEditProfileValues()
                .row_data(1)
                .as_deref(),
            Some("raw")
        );
        // 编辑（镜像 _populate：名称/hosts 逐行/UA 回显 + profile 预选）。
        crate::open_identity_preset_edit(&window, &config, Some(0));
        assert_eq!(window.get_identityEditIndex(), 0);
        assert_eq!(window.get_identityEditName(), "A");
        assert_eq!(window.get_identityEditHosts(), "a.com\n*.b.com");
        assert_eq!(window.get_identityEditUserAgent(), "UA-A");
        assert_eq!(window.get_identityEditProfile(), "raw");
        assert_eq!(window.get_identityEditProfileIndex(), 1);
        // 特定版本字面量不在 includeAuto=False 菜单里 → 追加第 12 项并
        // 原样保留值（避免静默丢值）。
        crate::open_identity_preset_edit(&window, &config, Some(1));
        assert_eq!(window.get_identityEditProfile(), "chrome120");
        assert_eq!(window.get_identityEditProfileIndex(), 11);
        assert_eq!(window.get_identityEditProfileLabels().row_count(), 12);
        assert_eq!(
            window
                .get_identityEditProfileValues()
                .row_data(11)
                .as_deref(),
            Some("chrome120")
        );
    }

    /// 第 47 步：apply_identity_preset_draft 镜像 `_onAddClicked` 追加 /
    /// `_onEditClicked` 替换 + isEnabled 继承 + 越界守卫。
    #[test]
    fn apply_identity_preset_draft_mirrors_row_handlers() {
        use crate::identity::{draft_to_identity_preset, parse_presets};

        let mut presets = parse_presets(None);
        assert_eq!(presets.len(), 1);
        // 添加：追加，isEnabled 用草稿值（preset() 无 _preset → True）。
        let draft = draft_to_identity_preset("新预设", "x.com", "", "", true);
        assert!(crate::apply_identity_preset_draft(&mut presets, -1, draft));
        assert_eq!(presets.len(), 2);
        assert_eq!(presets[1].name, "新预设");
        // 编辑：替换但继承既有 isEnabled（缺省百度预设 enabled=false 后）。
        presets[0].is_enabled = false;
        let draft = draft_to_identity_preset("改名", "y.com", "raw", "UA", true);
        assert!(crate::apply_identity_preset_draft(&mut presets, 0, draft));
        assert_eq!(presets.len(), 2, "编辑不追加");
        assert_eq!(presets[0].name, "改名");
        assert_eq!(presets[0].hosts, vec!["y.com"]);
        assert!(!presets[0].is_enabled, "编辑态继承既有 isEnabled");
        // 越界（镜像 `if index < 0 or index >= len(presets): return`）。
        let draft = draft_to_identity_preset("越界", "z.com", "", "", true);
        assert!(!crate::apply_identity_preset_draft(&mut presets, 9, draft));
        assert_eq!(presets.len(), 2);
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
            |_| crate::card::CardRuntime::default(),
        );
        let model = window.get_tasks();
        assert_eq!(model.row_count(), 3);
        let names: Vec<String> = (0..model.row_count())
            .filter_map(|index| model.row_data(index).map(|row| row.name.to_string()))
            .collect();
        assert_eq!(names, ["c.m3u8", "b.iso", "a.bin"]);
    }

    /// 第 57 步：外壳导航 model 装配 + 切页联动（选中态 / 搜索框口径）。
    ///
    /// `navTop` 两项（下载任务 / 新建任务）、`navBottom` 一项（设置）；
    /// 「新建任务」`selectable=false` 故永不选中。
    #[test]
    fn shell_nav_and_search_follow_route_headless() {
        let window = crate::create_headless();
        crate::set_nav_items(&window, crate::window::PAGE_TASK);

        let top = window.get_navTop();
        assert_eq!(top.row_count(), 2);
        let first = top.row_data(0).expect("首项缺失");
        assert_eq!(first.routeKey, "TaskPage");
        assert_eq!(first.text, "下载任务");
        assert_eq!(first.icon, "DOWNLOAD");
        assert!(first.selectable);
        assert!(first.selected, "当前页应选中");
        let second = top.row_data(1).expect("次项缺失");
        assert_eq!(second.routeKey, "addTaskButton");
        assert_eq!(second.text, "新建任务");
        assert!(!second.selectable);
        assert!(!second.selected, "「新建任务」不可选中");

        let bottom = window.get_navBottom();
        assert_eq!(bottom.row_count(), 1);
        let settings = bottom.row_data(0).expect("设置项缺失");
        assert_eq!(settings.routeKey, "SettingPage");
        assert_eq!(settings.text, "设置");
        assert!(settings.selectable);
        assert!(!settings.selected, "当前不在设置页");

        // 切到设置页：选中态转移 + 搜索框口径跟着变（`searchEdit.clear()`）。
        window.set_searchText("旧关键字".into());
        crate::switch_page(&window, crate::window::PAGE_SETTINGS);
        assert_eq!(window.get_currentRoute(), "SettingPage");
        assert!(window.get_searchVisible());
        assert_eq!(window.get_searchPlaceholder(), "搜索设置");
        assert_eq!(window.get_searchText(), "", "切页会清空搜索框");
        assert!(!window.get_navTop().row_data(0).unwrap().selected);
        assert!(window.get_navBottom().row_data(0).unwrap().selected);

        // 未知路由（pack 页面）：搜索框隐藏、placeholder 清空、无选中项。
        crate::switch_page(&window, "SomePackPage");
        assert!(!window.get_searchVisible());
        assert_eq!(window.get_searchPlaceholder(), "");
        assert!(!window.get_navTop().row_data(0).unwrap().selected);
        assert!(!window.get_navBottom().row_data(0).unwrap().selected);
    }

    /// 第 57 步：通知条与消息框的形状装配（按钮序号 + 主按钮 + 复选框）。
    #[test]
    fn shell_info_bar_and_dialog_assembly_headless() {
        let window = crate::create_headless();

        // 更新提示条：三个按钮，`立即下载` 是主按钮。
        crate::show_info_bar(&window, &crate::window::update_available_info_bar("1.2.3"));
        assert!(window.get_infoBarVisible());
        assert_eq!(
            window.get_infoBarTitle(),
            crate::window::UPDATE_AVAILABLE_TITLE
        );
        assert_eq!(
            window.get_infoBarText(),
            crate::window::update_available_content("1.2.3")
        );
        let buttons = window.get_infoBarButtons();
        assert_eq!(buttons.row_count(), 3);
        let download = buttons.row_data(0).expect("首按钮缺失");
        assert_eq!(download.text, crate::window::UPDATE_DOWNLOAD_BUTTON);
        assert_eq!(download.index, 0);
        assert!(download.primary);
        let detail = buttons.row_data(1).expect("次按钮缺失");
        assert_eq!(detail.index, 1);
        assert!(!detail.primary);
        assert_eq!(buttons.row_data(2).unwrap().index, 2);

        crate::hide_info_bar(&window);
        assert!(!window.get_infoBarVisible());

        // 关闭确认框：两按钮 + 「记住我的选择」复选框（缺省不勾）。
        crate::show_dialog(&window, &crate::window::close_ask_dialog());
        assert!(window.get_dialogVisible());
        assert_eq!(window.get_dialogTitle(), crate::window::CLOSE_ASK_TITLE);
        assert_eq!(
            window.get_dialogCheckboxLabel(),
            crate::window::CLOSE_ASK_REMEMBER
        );
        assert!(!window.get_dialogCheckboxChecked());
        assert_eq!(window.get_dialogButtons().row_count(), 2);
        assert_eq!(
            crate::dialog_button_text(&window, 0).as_deref(),
            Some(crate::window::CLOSE_ASK_YES)
        );
        assert_eq!(
            crate::dialog_button_text(&window, 9),
            None,
            "越界序号返回 None"
        );

        // 异常框：没有复选框（空串 = 没有）。
        crate::show_dialog(&window, &crate::window::exception_dialog("boom"));
        assert_eq!(window.get_dialogCheckboxLabel(), "");
        assert_eq!(
            window.get_dialogText(),
            crate::window::exception_text("boom")
        );

        // 收起时清掉复选框态（Qt 侧每次 exec() 都是新对象）。
        window.set_dialogCheckboxLabel("残留".into());
        window.set_dialogCheckboxChecked(true);
        crate::hide_dialog(&window);
        assert!(!window.get_dialogVisible());
        assert_eq!(window.get_dialogCheckboxLabel(), "");
        assert!(!window.get_dialogCheckboxChecked());
    }

    /// 第 57 步：拖放载荷 → `DropInput` 映射（`has_urls` 由本地文件路径决定）。
    ///
    /// `DataTransfer` 无法在测试里构造，故直接喂 `DropPayload`；Qt 侧的
    /// `mimeData().urls()` 与 Slint 的 `file_paths()` 差异记在 `DropPayload`
    /// 的文档注释里。
    #[test]
    fn drop_payload_maps_to_drop_input() {
        let files = crate::DropPayload {
            file_paths: vec!["/tmp/a.bin".into()],
            text: None,
        };
        let input = files.drop_input(true, false);
        assert!(input.is_file_drag_active);
        assert!(input.has_urls);
        assert!(!input.has_text);
        assert_eq!(input.text, "");
        assert!(!input.mask_dialog_visible);

        let text = crate::DropPayload {
            file_paths: vec![],
            text: Some("https://example/a.bin".into()),
        };
        let input = text.drop_input(false, true);
        assert!(!input.has_urls);
        assert!(input.has_text);
        assert_eq!(input.text, "https://example/a.bin");
        assert!(input.mask_dialog_visible);

        // 空载荷：两个 flag 都假 → `drop_decision` 走「忽略」分支。
        let payload = crate::DropPayload::default();
        let empty = payload.drop_input(false, false);
        assert!(!empty.has_urls);
        assert!(!empty.has_text);
        assert_eq!(
            crate::window::drop_decision(&empty, |_| true),
            crate::window::DropDecision::Ignore
        );
    }

    /// 第 59 步：右键菜单覆盖层的装配（摊平行 → Slint model → 点击回传）。
    ///
    /// 钉住四件事：行字段映射（含 `icon: None` → 空串）、面板高度、
    /// 子菜单就地展开（菜单不关）、动作行收起菜单并回传 [`CardClick`]。
    #[test]
    fn card_menu_overlay_assembly_headless() {
        use crate::card_menu::{CardClick, MenuCategory};
        use crate::menu_view::CardMenuView;
        use std::cell::RefCell;
        use std::rc::Rc;

        let window = crate::create_headless();
        let categories = vec![
            MenuCategory {
                id: "cat_video".into(),
                name: "视频".into(),
                icon: "VIDEO".into(),
            },
            MenuCategory {
                id: "cat_other".into(),
                name: "杂项".into(),
                icon: "NOT_A_REAL_ICON".into(),
            },
        ];
        let icons = vec!["VIDEO".to_string(), "TAG".to_string()];
        let view = CardMenuView::new(crate::card_menu::context_menu(
            &crate::card_menu::ContextMenuInput {
                can_edit: true,
                status: gd3_core::task::TaskStatus::Running,
                category_enabled: true,
                categories: &categories,
                known_icons: &icons,
            },
        ));

        let clicks: Rc<RefCell<Vec<CardClick>>> = Rc::new(RefCell::new(Vec::new()));
        let dismissals = Rc::new(RefCell::new(0));
        let make_actions = || {
            let clicks = clicks.clone();
            let dismissals = dismissals.clone();
            Rc::new(crate::CardMenuActions {
                activated: Box::new(move |click: &CardClick| {
                    clicks.borrow_mut().push(click.clone());
                }),
                dismissed: Box::new(move || {
                    *dismissals.borrow_mut() += 1;
                }),
            })
        };

        crate::show_card_menu(&window, view.clone(), 320, 240, make_actions());
        assert!(window.get_cardMenuOpened());
        assert_eq!(window.get_cardMenuX(), 320);
        assert_eq!(window.get_cardMenuY(), 240);
        assert_eq!(
            window.get_cardMenuHeight(),
            crate::menu_view::panel_height(&view.rows()),
            "面板高度必须与摊平行高度之和一致"
        );

        let rows = window.get_cardMenuRows();
        assert_eq!(rows.row_count(), 7);
        let detail = rows.row_data(0).expect("首行缺失");
        assert_eq!(detail.kind, 0);
        assert_eq!(detail.text, crate::card_menu::MENU_DETAIL);
        assert_eq!(detail.icon, "INFO");
        assert_eq!(detail.depth, 0);
        assert!(!detail.checkable);
        assert!(!detail.checked);
        let separator = rows.row_data(1).expect("分隔符缺失");
        assert_eq!(separator.kind, 1);
        assert_eq!(separator.text, "", "分隔符没有文案");
        assert_eq!(separator.icon, "", "`None` 图标写成空串");
        let submenu = rows.row_data(6).expect("子菜单行缺失");
        assert_eq!(submenu.kind, 2);
        assert_eq!(submenu.text, crate::card_menu::MENU_MOVE_TO_CATEGORY);
        assert!(!submenu.expanded);

        // 点分隔符：Qt 侧点了没反应，菜单不关、无动作。
        window.invoke_cardMenuActivated(1);
        assert!(window.get_cardMenuOpened());
        assert!(clicks.borrow().is_empty());

        // 点子菜单行：就地展开并重画，菜单继续开着。
        window.invoke_cardMenuActivated(6);
        assert!(window.get_cardMenuOpened());
        assert!(clicks.borrow().is_empty());
        let rows = window.get_cardMenuRows();
        assert_eq!(rows.row_count(), 11, "展开后多 4 行");
        assert!(rows.row_data(6).expect("子菜单行").expanded);
        let uncategorized = rows.row_data(7).expect("未分类行缺失");
        assert_eq!(uncategorized.depth, 1);
        assert_eq!(uncategorized.text, crate::card_menu::MENU_UNCATEGORIZED);
        assert_eq!(
            rows.row_data(9).expect("分类行缺失").text,
            "视频",
            "分类显示名"
        );
        assert_eq!(
            rows.row_data(10).expect("分类行缺失").icon,
            "TAG",
            "非法图标名在 card_menu 侧已兜底"
        );

        // 点展开后的分类项：收起菜单 + 回传 SetCategory。
        window.invoke_cardMenuActivated(9);
        assert!(!window.get_cardMenuOpened());
        assert_eq!(window.get_cardMenuRows().row_count(), 0);
        assert_eq!(window.get_cardMenuHeight(), 0);
        assert_eq!(
            clicks.borrow().as_slice(),
            [CardClick::SetCategory("cat_video".into())]
        );

        // 再弹一次：展开态不残留（每次都是新视图，与 Qt 造新 RoundMenu 一致）。
        crate::show_card_menu(&window, view.clone(), 10, 10, make_actions());
        assert_eq!(window.get_cardMenuRows().row_count(), 7);

        // 点面板外：收起 + 回调 dismissed。
        window.invoke_cardMenuDismissed();
        assert!(!window.get_cardMenuOpened());
        assert_eq!(*dismissals.borrow(), 1);
        assert_eq!(clicks.borrow().len(), 1, "关闭不产生动作");

        // 校验菜单：所有动作行可勾选，已算过的算法打勾。
        let algorithms: Vec<String> = ["md5", "sha256"].iter().map(|s| s.to_string()).collect();
        let checksums = vec!["md5".to_string()];
        let view = CardMenuView::new(crate::card_menu::checksum_menu(
            &crate::card_menu::ChecksumMenuInput {
                algorithms: &algorithms,
                checksums: &checksums,
            },
        ));
        crate::show_card_menu(&window, view, 0, 0, make_actions());
        let rows = window.get_cardMenuRows();
        assert_eq!(rows.row_count(), 3);
        let md5 = rows.row_data(0).expect("MD5 行缺失");
        assert!(md5.checkable);
        assert!(md5.checked);
        assert_eq!(md5.icon, "", "两参 `Action(text, parent)` 没有图标");
        assert!(!rows.row_data(1).expect("SHA256 行").checked);
        assert_eq!(rows.row_data(2).expect("更多行").kind, 2);

        // 点算法项：回传 StartChecksum(algorithm) 并收起菜单。
        window.invoke_cardMenuActivated(1);
        assert!(!window.get_cardMenuOpened());
        assert_eq!(
            clicks.borrow().last(),
            Some(&CardClick::StartChecksum("sha256".into()))
        );
    }

    /// 第 60 步冒烟：任务级七路回调直接落到服务层。
    ///
    /// 断言的粒度是**调用序列**，与 `actions.rs` 的单测同一口径；这里额外验证
    /// 「注册即通路」（`invoke_*` 打得进来）与删除确认框的异步两段式
    /// （`taskDelete` 只挂起 + 弹框，`dialogButtonClicked` 才真删）。
    #[test]
    fn task_commands_dispatch_headless() {
        use crate::actions::{TaskService, UiHooks};
        use crate::card_menu::CardMenuEntry;
        use crate::window::DIALOG_OK;
        use gd3_core::task::TaskStatus;
        use slint::ComponentHandle;
        use std::cell::RefCell;
        use std::collections::HashMap;
        use std::rc::Rc;

        /// 同一对象同时扮演服务与钩子，日志分开记，便于按序断言。
        #[derive(Default)]
        struct Rec {
            service: RefCell<Vec<String>>,
            hooks: RefCell<Vec<String>>,
            hints: RefCell<Vec<String>>,
            menus: RefCell<Vec<usize>>,
        }

        impl TaskService for Rec {
            fn pause(&self, task_id: &str) {
                self.service.borrow_mut().push(format!("pause({task_id})"));
            }
            fn start(&self, task_id: &str) {
                self.service.borrow_mut().push(format!("start({task_id})"));
            }
            fn start_seeding(&self, task_id: &str) {
                self.service
                    .borrow_mut()
                    .push(format!("startSeeding({task_id})"));
            }
            fn stop_seeding(&self, task_id: &str) {
                self.service
                    .borrow_mut()
                    .push(format!("stopSeeding({task_id})"));
            }
            fn delete(&self, task_id: &str, delete_files: bool) {
                self.service
                    .borrow_mut()
                    .push(format!("delete({task_id}, {delete_files})"));
            }
            fn redownload(&self, task_id: &str) {
                self.service
                    .borrow_mut()
                    .push(format!("redownload({task_id})"));
            }
            fn move_to_front(&self, task_id: &str) {
                self.service
                    .borrow_mut()
                    .push(format!("moveToFront({task_id})"));
            }
            fn set_category(&self, task_id: &str, category_id: &str) {
                self.service
                    .borrow_mut()
                    .push(format!("setCategory({task_id}, {category_id})"));
            }
            fn start_checksum(&self, task_id: &str, algorithm: &str) {
                self.service
                    .borrow_mut()
                    .push(format!("startChecksum({task_id}, {algorithm})"));
            }
            fn cancel_checksum(&self, task_id: &str) {
                self.service
                    .borrow_mut()
                    .push(format!("cancelChecksum({task_id})"));
            }
        }

        impl UiHooks for Rec {
            fn copy_url(&self, url: &str) {
                self.hooks.borrow_mut().push(format!("setUrls({url})"));
            }
            fn open_drawer(&self, task_id: &str) {
                self.hooks.borrow_mut().push(format!("drawer({task_id})"));
            }
            fn edit_task(&self, task_id: &str) {
                self.hooks
                    .borrow_mut()
                    .push(format!("editDialog({task_id})"));
            }
            fn open_file(&self, path: &str) {
                self.hooks.borrow_mut().push(format!("openFile({path})"));
            }
            fn reveal_in_folder(&self, path: &str) {
                self.hooks
                    .borrow_mut()
                    .push(format!("revealInFolder({path})"));
            }
        }

        let window = crate::create_headless();
        let rec: Rc<Rec> = Rc::new(Rec::default());

        // 事实表：t1 完成且做种中、t2 完成未做种、t3 运行中、t4 暂停、
        // t5 校验中、t6 输出不存在、t7 可校验。
        let mut table: HashMap<String, crate::TaskFacts> = HashMap::new();
        let facts = |id: &str, status: TaskStatus| crate::TaskFacts {
            status,
            output_path: format!("/dl/{id}.bin"),
            output_exists: true,
            algorithms: vec!["md5".into(), "sha256".into()],
            checksums: vec!["md5".into()],
            ..crate::TaskFacts::default()
        };
        table.insert(
            "t1".into(),
            crate::TaskFacts {
                is_seeding: true,
                ..facts("t1", TaskStatus::Completed)
            },
        );
        table.insert("t2".into(), facts("t2", TaskStatus::Completed));
        table.insert("t3".into(), facts("t3", TaskStatus::Running));
        table.insert("t4".into(), facts("t4", TaskStatus::Paused));
        table.insert(
            "t5".into(),
            crate::TaskFacts {
                checksum_in_progress: true,
                ..facts("t5", TaskStatus::Completed)
            },
        );
        table.insert(
            "t6".into(),
            crate::TaskFacts {
                output_exists: false,
                ..facts("t6", TaskStatus::Completed)
            },
        );
        table.insert("t7".into(), facts("t7", TaskStatus::Completed));
        let table = Rc::new(table);

        let service: Rc<dyn TaskService> = rec.clone();
        let hooks: Rc<dyn UiHooks> = rec.clone();
        let commands = Rc::new(crate::TaskCommands::new(
            service,
            hooks,
            Box::new({
                let table = table.clone();
                move |id: &str| table.get(id).cloned()
            }),
            Box::new({
                let rec = rec.clone();
                move |_id: &str, entries: Vec<CardMenuEntry>| {
                    rec.menus.borrow_mut().push(entries.len())
                }
            }),
            Box::new({
                let rec = rec.clone();
                move |id: &str, text: &str| rec.hints.borrow_mut().push(format!("{id}:{text}"))
            }),
            Box::new(|| false),
        ));
        crate::install_task_commands(&window, commands.clone());

        // 调用方的 `dialogButtonClicked` 分派（本 crate 不占这个槽，见 `TaskCommands`）。
        let resolved: Rc<RefCell<Vec<Option<bool>>>> = Rc::new(RefCell::new(Vec::new()));
        let weak = window.as_weak();
        let cmds = commands.clone();
        let slot = resolved.clone();
        window.on_dialogButtonClicked(move |index| {
            if let Some(window) = weak.upgrade() {
                let accepted = crate::dialog_button_text(&window, index as usize)
                    .map(|text| text == DIALOG_OK)
                    .unwrap_or(false);
                let out = crate::resolve_delete_dialog(&window, &cmds, accepted);
                slot.borrow_mut().push(out);
            }
        });

        // ---- 暂停/继续/做种：四路分派 ----
        window.invoke_taskToggle("t1".into());
        window.invoke_taskToggle("t2".into());
        window.invoke_taskToggle("t3".into());
        window.invoke_taskToggle("t4".into());
        // 未知任务：静默丢弃，不打服务层。
        window.invoke_taskToggle("gone".into());
        assert_eq!(
            *rec.service.borrow(),
            vec![
                "stopSeeding(t1)".to_string(),
                "startSeeding(t2)".to_string(),
                "pause(t3)".to_string(),
                "start(t4)".to_string(),
            ]
        );

        // ---- 显式暂停 ----
        window.invoke_taskPause("t3".into());
        assert_eq!(rec.service.borrow().last(), Some(&"pause(t3)".to_string()));

        // ---- 删除：先挂起 + 弹框，确认后才删 ----
        assert!(!window.get_dialogVisible());
        window.invoke_taskDelete("t2".into());
        assert!(commands.has_pending_delete());
        assert!(window.get_dialogVisible());
        assert_eq!(window.get_dialogTitle(), "删除任务");
        assert_eq!(window.get_dialogCheckboxLabel(), "同时删除已下载的文件");
        assert!(
            !window.get_dialogCheckboxChecked(),
            "初值取 cfg，缺省 false"
        );
        assert!(
            !rec.service
                .borrow()
                .iter()
                .any(|c| c.starts_with("delete("))
        );
        window.set_dialogCheckboxChecked(true);
        window.invoke_dialogButtonClicked(0); // 「确定」
        assert!(!commands.has_pending_delete());
        assert!(!window.get_dialogVisible());
        assert_eq!(
            *resolved.borrow(),
            vec![Some(true)],
            "返回值 = 调用方要写回 cfg.shouldDeleteFilesOnRemove 的勾选值"
        );
        assert_eq!(
            rec.service.borrow().last(),
            Some(&"delete(t2, true)".to_string())
        );

        // 取消：什么都不做，返回 None。
        window.invoke_taskDelete("t4".into());
        window.invoke_dialogButtonClicked(1); // 「取消」
        assert_eq!(*resolved.borrow(), vec![Some(true), None]);
        assert_eq!(
            rec.service.borrow().last(),
            Some(&"delete(t2, true)".to_string()),
            "取消不该删任务"
        );

        // ---- 校验三条短路 ----
        window.invoke_taskChecksum("t5".into());
        assert_eq!(
            rec.service.borrow().last(),
            Some(&"cancelChecksum(t5)".to_string())
        );
        window.invoke_taskChecksum("t6".into());
        assert_eq!(
            *rec.hints.borrow(),
            vec!["t6:文件不存在，无法校验".to_string()]
        );
        window.invoke_taskChecksum("t7".into());
        // md5 + sha256 + 「更多」子菜单 = 3 行。
        assert_eq!(*rec.menus.borrow(), vec![3]);

        // ---- 打开文件 / 文件夹 / 抽屉 ----
        window.invoke_taskOpenFile("t7".into());
        window.invoke_taskOpenFolder("t7".into());
        window.invoke_taskOpenDrawer("t7".into());
        assert_eq!(
            *rec.hooks.borrow(),
            vec![
                "openFile(/dl/t7.bin)".to_string(),
                "revealInFolder(/dl/t7.bin)".to_string(),
                "drawer(t7)".to_string(),
            ]
        );
    }
}
