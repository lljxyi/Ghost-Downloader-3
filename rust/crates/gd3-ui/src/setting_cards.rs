//! 设置页剩余卡片（P1）：镜像 `app/view/components/setting_cards.py`（949 行）。
//!
//! 分工：`proxy.rs`（第 35 步）已覆盖 `ProxySettingCard` 的**配置口径**
//! （`_loadProxy` 分支 / `_showProxyUrl` / `_buildProxyUrl` / `validate`），
//! 本模块补 UI 侧的**状态机与格式化**，以及其余六类卡片。
//!
//! Python 先验（2026-10-18 实读源码，逐条对应）：
//!
//! - **`HOST_PATTERN` / `PORT_PATTERN`**（:36-40）：host 是「IPv4 四段
//!   0-255」或「域名 `label(.-)…` + 2-6 位字母 TLD」；port 只要求
//!   `^\d{1,5}$`——**没有范围校验**。故 `"99999"` 在 Python 侧不算错误，
//!   `proxy::port_valid`（1-65535）与本模块的 `port_pattern_ok` 是两套口径，
//!   前者给 `validate` 用、后者给红下划线用，别互相替换。
//! - **`ErrorVisibleLineEdit.paintEvent`**（:43-58）：`hasFocus()` 或
//!   `isError()` 才画；形状是 `addRoundedRect(ml, h-10, w, 10, r=5)` 减去
//!   `addRect(ml, h-10, w, 8)`，剩下的可见带 `y = h-2`、`height = 2`、
//!   有效圆角 `min(5, 2/2) = 1`（与 `token_line_edit` 同款减法）。
//! - **`SpinBoxSettingCard`**（:61-90）：`setRange(int(r0*division),
//!   int(r1*division))`、`setValue(int(value*division))`、
//!   `_onValueChanged` 同样 `int(value*division)`、`leaveEvent` 回写
//!   `int(spinBox.value()/division)`——**三处 `int()` 都是向零截断**。
//!   `eventFilter` 对 `Wheel` 返回 True（滚轮**不改值**，避免滚动页面误触）。
//! - **`PercentSpinBoxSettingCard`**（:93-121）：`decimals = 0`、
//!   `singleStep = 25`、`suffix = " %"`、`range(r0*100, r1*100)`、
//!   `value*100`；`leaveEvent` 回写 `value/100`——**这里不截断**（cfg 里
//!   存的是 0..1 浮点）。
//! - **`LineEditSettingCard`**（:124-146）：`isPassword` 决定
//!   `PasswordLineEdit`；`editingFinished` → `cfg.set(configItem, text())`，
//!   **不 strip**。
//! - **`ProxySettingCard`**（:149-322）：`_onRadioClicked` 写
//!   `"Auto"`/`"Off"`，自定义分支若 ip 是占位文案「未检测到代理」先清空再
//!   `_onProxyFieldChanged`；`_refreshCompatBanner` 按来源取 scheme
//!   （Off→空 / Auto→`urlsplit(proxy()).scheme.lower()` / Custom→
//!   `protocolCombo.currentText().lower()`），有 `proxySchemes is not None`
//!   且不含该 scheme 的包 → 文案
//!   `"{包名、连接} 不支持当前代理协议，将直连；使用 SOCKS5 可兼容全部下载方式"`
//!   （包名 **`packId.upper()`**）；`_onProxyFieldChanged` 非自定义只清错，
//!   自定义时 url 合法 → `choiceLabel = url`，否则
//!   `choiceLabel = customRadio.text()` 且
//!   `ipError = bool(ip) and not HOST.match(ip)`（port 同理，都先 `strip()`）；
//!   `leaveEvent` **只有自定义且 url 合法**才写 cfg。
//!   `setExpand(False)` 会 `_loadProxy()` 回滚未提交的编辑。
//! - **`SelectFolderSettingCard`**（:325-357）：初始 content =
//!   `configItem.value`；`_setPath(path)` 写 cfg + `setContent(path)` +
//!   `pathChanged`；「恢复默认」直接 `_setPath(defaultPath)`（**不确认**）。
//! - **`PresetRowWidget`**（:360-431）：描述三段（hosts 前 3 个 `", "` /
//!   `toProfileLabel(clientProfile)` / UA `len<=24` 否则 `[:21]+"..."`），
//!   每段前是 12 号 `IconBodyLabel`，段间 `addSpacing(8)`，末尾
//!   `addStretch(1)`；`_refreshOpacity(enabled)` 把名称与描述段全部
//!   `setEnabled(enabled)`（开关关掉时整行变灰）。
//! - **`IdentitySettingCard`**（:434-577）：`_onToggled` 写
//!   `{**preset, isEnabled: checked}`；`_onRemoveClicked` 先
//!   `_rowWidgets.pop(index)` 再把**后续行的 `_index` 重编号**，然后
//!   `presets.pop(index)` 写回；`_onAddClicked` 追加并**在折叠时自动展开**。
//! - **`HeadersPresetRow`**（:580-660）：`countLabel = "{n} 条标头"`、
//!   `radioButton.setChecked(isCurrent)`、`removeButton.setEnabled(canRemove)`
//!   且 `canRemove = len(presets) > 1`（**全局**口径，不是按行）。
//! - **`HeadersPresetSettingCard._onRemoveClicked`**（:735-757）：删行后
//!   `if index <= current: current = max(0, current - 1)`，再取
//!   `newCurrent = currentHeadersPresetIndex()`（= `min(current, len-1)`），
//!   重设每行单选与删除可用性，`choiceLabel = presets[newCurrent]["name"]`。
//! - **`SelectFileCard`**（:760-795）：初始 content =
//!   `(configItem.value or hint) if configItem else hint`；`_setPath` 后
//!   content = `path or hint`（清空回到提示文案）。
//! - **`RuntimeCard.updateStatus`**（:845-949）：`refreshButton` 只在
//!   `not isBusy and not isInstalling` 时可用；安装中按
//!   `progress <= 0` / `< 100` / `>= 100` 给三档文案并**提前 return**；
//!   忙态给「正在检测运行时...」再提前 return；正常态按
//!   `error` → 本地化错误、已装 → `版本: X（最新: Y）` + `路径: Z`、
//!   未装 → `未检测到可用的 {name}`；更新可用（已装 + app 托管 + 有最新
//!   版本 + 本地无版本或本地更旧）→ 按钮文案「更新到 X」并显示；否则
//!   未装或非托管 → 「一键安装」且 `visible = canInstall`；其余隐藏。
//!   删除按钮可见 = `isInstalled and isManaged`。
//!   `isNewer` 用 `app/update.py::parseVersion`：`lstrip("vV")` → 累积
//!   数字 → `.` 分隔 → 去尾部 0（元组按字典序比较，短前缀更小）。
//!
//! 不镜像：`QPainter` 绘制、`ToolTipFilter` 安装、`InfoBar` 弹出、
//! `MessageBox` 确认框（语义在调用方）、`cfg.set` 的落盘时机。

use crate::headers::HeaderPreset;
use crate::identity::{IdentityPreset, clip_user_agent};
use crate::proxy::{PROTOCOLS, build_url, parse_fields, validate};
use regex::Regex;
use std::sync::OnceLock;

/// `ErrorVisibleLineEdit` 的下划线源矩形：`QRectF(ml, h - 10, w, 10)`。
pub const ERROR_LINE_DEPTH: i32 = 10;
/// 减去的 `rectPath` 高度（`addRect(ml, h - 10, w, 8)`）。
pub const ERROR_LINE_CUT: i32 = 8;
/// `addRoundedRect(..., 5, 5)`。
pub const ERROR_LINE_RADIUS: i32 = 5;

/// `spinBox.setMinimumWidth(180)` / `self.hBoxLayout.addSpacing(24)`。
pub const SPIN_MIN_WIDTH: i32 = 180;
pub const SPIN_TRAILING_SPACING: i32 = 24;
/// `DoubleSpinBox.setSuffix(" %")` / `setDecimals(0)` / `singleStep = 25`。
pub const PERCENT_SUFFIX: &str = " %";
pub const PERCENT_DECIMALS: i32 = 0;
pub const PERCENT_SINGLE_STEP: f64 = 25.0;

/// `lineEdit.setMinimumWidth(180)` / `addSpacing(16)`。
pub const LINE_EDIT_MIN_WIDTH: i32 = 180;
pub const LINE_EDIT_TRAILING_SPACING: i32 = 16;

/// `_showProxyUrl(None)` 的占位文案（:265）。
pub const PROXY_NOT_DETECTED: &str = "未检测到代理";
/// 三个单选文案（:156-158）。
pub const RADIO_OFF: &str = "不使用代理";
pub const RADIO_AUTO: &str = "自动检测系统代理";
pub const RADIO_CUSTOM: &str = "使用自定义代理";
/// 兼容性提示模板（:288）。
pub const COMPAT_BANNER_TEMPLATE: &str =
    "{0} 不支持当前代理协议，将直连；使用 SOCKS5 可兼容全部下载方式";
/// 包名连接符（`"、".join(...)`）。
pub const PACK_NAME_SEPARATOR: &str = "、";

/// `HOST_PATTERN`（:36-38）逐字镜像：IPv4 四段 0-255 或域名 + 2-6 位字母 TLD。
fn host_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"^(?:(?:(?:25[0-5]|2[0-4]\d|[01]?\d\d?)\.){3}(?:25[0-5]|2[0-4]\d|[01]?\d\d?)|(?:[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?\.)+[a-zA-Z]{2,6})$",
        )
        .expect("HOST_PATTERN 编译失败")
    })
}

/// `PORT_PATTERN = compile(r"^\d{1,5}$")`（:40）——**只查位数**，
/// 不做 1-65535 范围校验（`proxy::port_valid` 才做范围）。
fn port_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\d{1,5}$").expect("PORT_PATTERN 编译失败"))
}

/// `HOST_PATTERN.match(text)`。
pub fn host_pattern_ok(text: &str) -> bool {
    host_pattern().is_match(text)
}

/// `PORT_PATTERN.match(text)`。
pub fn port_pattern_ok(text: &str) -> bool {
    port_pattern().is_match(text)
}

/// `ErrorVisibleLineEdit.paintEvent` 的可见带（x 恒为 `margin_left`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ErrorUnderline {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub radius: i32,
}

/// `if not self.hasFocus() and not self.isError(): return`。
pub fn underline_visible(has_focus: bool, is_error: bool) -> bool {
    has_focus || is_error
}

/// `path - rectPath` 剩下的 2px 带（与 `token_line_edit::underline` 同款）。
pub fn underline(width: i32, height: i32, margin_left: i32, margin_right: i32) -> ErrorUnderline {
    let band = ERROR_LINE_DEPTH - ERROR_LINE_CUT;
    ErrorUnderline {
        x: margin_left,
        y: height - ERROR_LINE_DEPTH + ERROR_LINE_CUT,
        width: width - margin_left - margin_right,
        height: band,
        radius: ERROR_LINE_RADIUS.min(band / 2),
    }
}

/// `setRange(int(r0 * division), int(r1 * division))`。
///
/// Python `int()` 向零截断，Rust `as i32` 同口径（NaN → 0，越界饱和）。
pub fn spin_range(low: f64, high: f64, division: f64) -> (i32, i32) {
    ((low * division) as i32, (high * division) as i32)
}

/// `setValue(int(configItem.value * division))` 与 `_onValueChanged` 同一口径。
pub fn spin_display(value: f64, division: f64) -> i32 {
    (value * division) as i32
}

/// `leaveEvent` 回写：`int(spinBox.value() / division)`。
pub fn spin_commit(spin_value: i32, division: f64) -> i32 {
    (spin_value as f64 / division) as i32
}

/// `PercentSpinBoxSettingCard` 的 `setRange(r0 * 100, r1 * 100)`。
pub fn percent_range(low: f64, high: f64) -> (f64, f64) {
    (low * 100.0, high * 100.0)
}

/// `setValue(configItem.value * 100)`。
pub fn percent_display(value: f64) -> f64 {
    value * 100.0
}

/// `leaveEvent` 回写：`spinBox.value() / 100`（**不截断**）。
pub fn percent_commit(spin_value: f64) -> f64 {
    spin_value / 100.0
}

/// `eventFilter` 对 `QEvent.Type.Wheel` 返回 `True` → 滚轮**不改值**。
pub const WHEEL_ADJUSTS_VALUE: bool = false;

/// `editingFinished` → `cfg.set(configItem, self.lineEdit.text())`：原样，不 strip。
pub fn line_edit_commit(text: &str) -> String {
    text.to_string()
}

/// 三个单选的文案（顺序即 `buttonGroup` 加入顺序）。
pub fn radio_label(choice: i32) -> &'static str {
    match choice {
        crate::proxy::CHOICE_OFF => RADIO_OFF,
        crate::proxy::CHOICE_AUTO => RADIO_AUTO,
        _ => RADIO_CUSTOM,
    }
}

/// 兼容性检查用的包信息（`p.packId` + `p.proxySchemes`）。
#[derive(Clone, Copy, Debug)]
pub struct PackCompat<'a> {
    pub pack_id: &'a str,
    pub proxy_schemes: Option<&'a [&'a str]>,
}

/// `[p.packId.upper() for p in packs if p.proxySchemes is not None
/// and scheme not in p.proxySchemes]`。
pub fn incompatible_packs(packs: &[PackCompat<'_>], scheme: &str) -> Vec<String> {
    packs
        .iter()
        .filter(|pack| match pack.proxy_schemes {
            Some(schemes) => !schemes.contains(&scheme),
            None => false,
        })
        .map(|pack| pack.pack_id.to_uppercase())
        .collect()
}

/// `self._compatLabel.setText(self.tr("{0} ...").format(names))`。
pub fn compat_banner_text(names: &[String]) -> String {
    COMPAT_BANNER_TEMPLATE.replace("{0}", &names.join(PACK_NAME_SEPARATOR))
}

/// CPython `urlsplit(url).scheme.lower()` 的口径（只取 scheme）。
///
/// 规则（CPython 3.13 `urlsplit`）：先 `lstrip` C0 控制符与空格、删掉所有
/// `\t\r\n`；再找第一个 `:`，`:` 之前不得出现 `/?#`；scheme 非空、首字符
/// 为 ASCII 字母、其余属于 `[A-Za-z0-9+.-]`；否则 scheme 为空串。
pub fn url_scheme(url: &str) -> String {
    let cleaned: String = url
        .trim_start_matches(|c: char| c <= ' ' || c == '\u{7f}')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
        .collect();
    let Some(colon) = cleaned.find(':') else {
        return String::new();
    };
    let head = &cleaned[..colon];
    if head.is_empty() || head.contains(['/', '?', '#']) {
        return String::new();
    }
    let mut chars = head.chars();
    let first = chars.next().expect("非空已判");
    if !first.is_ascii_alphabetic() {
        return String::new();
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        return String::new();
    }
    head.to_ascii_lowercase()
}

/// `ProxySettingCard` 的 UI 状态（`choiceLabel` + 五个输入控件 + 红框 + banner）。
#[derive(Clone, Debug, PartialEq)]
pub struct ProxyCardState {
    /// `CHOICE_OFF` / `CHOICE_AUTO` / `CHOICE_CUSTOM`。
    pub choice: i32,
    /// `protocolCombo.currentText()`；`""` 表示 `setCurrentIndex(-1)`。
    pub protocol: String,
    pub ip: String,
    pub port: String,
    pub user: String,
    pub pass: String,
    pub choice_label: String,
    pub ip_error: bool,
    pub port_error: bool,
    pub banner: Option<String>,
}

impl ProxyCardState {
    /// `_loadProxy()` + 末尾的 `_refreshCompatBanner()`。
    ///
    /// `detected` = `proxy()`（系统代理 URL，`None` 等价 Python 的假值）。
    pub fn load(config_value: &str, detected: Option<&str>, packs: &[PackCompat<'_>]) -> Self {
        let mut state = Self {
            choice: crate::proxy::choice_of_value(config_value),
            protocol: PROTOCOLS[0].to_string(),
            ip: String::new(),
            port: String::new(),
            user: String::new(),
            pass: String::new(),
            choice_label: String::new(),
            ip_error: false,
            port_error: false,
            banner: None,
        };
        match state.choice {
            crate::proxy::CHOICE_AUTO => {
                state.radio_clicked(crate::proxy::CHOICE_AUTO, detected, packs)
            }
            crate::proxy::CHOICE_OFF => {
                state.radio_clicked(crate::proxy::CHOICE_OFF, detected, packs)
            }
            _ => {
                state.show_proxy_url(Some(config_value));
                state.radio_clicked(crate::proxy::CHOICE_CUSTOM, detected, packs);
            }
        }
        state
    }

    /// `_onRadioClicked(button)`。
    pub fn radio_clicked(&mut self, choice: i32, detected: Option<&str>, packs: &[PackCompat<'_>]) {
        self.choice = choice;
        self.choice_label = radio_label(choice).to_string();
        if choice == crate::proxy::CHOICE_AUTO {
            self.show_proxy_url(detected);
        } else if choice == crate::proxy::CHOICE_CUSTOM {
            if self.ip == PROXY_NOT_DETECTED {
                self.ip.clear();
            }
            self.field_changed(detected, packs);
            return;
        }
        self.refresh_banner(detected, packs);
    }

    /// `_showProxyUrl(url)`：`urlsplit` 逐字段填回，空 url 走占位分支。
    pub fn show_proxy_url(&mut self, url: Option<&str>) {
        let Some(url) = url.filter(|text| !text.is_empty()) else {
            // `setCurrentText("")` 在非可编辑下拉里落到 index -1 → 文本为空。
            self.protocol.clear();
            self.ip = PROXY_NOT_DETECTED.to_string();
            self.port.clear();
            self.user.clear();
            self.pass.clear();
            return;
        };
        let fields = parse_fields(url);
        // `urlsplit` 会把 scheme 转小写；不在下拉项里时 `setCurrentText`
        // 找不到项、保留初值（索引 0 = socks4）。
        let scheme = url_scheme(url);
        self.protocol = if PROTOCOLS.iter().any(|item| *item == scheme) {
            scheme
        } else {
            PROTOCOLS[fields.protocol_index as usize].to_string()
        };
        self.ip = fields.host;
        self.port = fields.port;
        self.user = fields.user;
        self.pass = fields.pass;
    }

    /// `_buildProxyUrl()`。
    pub fn url(&self) -> String {
        build_url(&self.protocol, &self.ip, &self.port, &self.user, &self.pass)
    }

    /// `customWidget.setEnabled(isCustom)` / `credWidget.setEnabled(isCustom)`。
    pub fn custom_enabled(&self) -> bool {
        self.choice == crate::proxy::CHOICE_CUSTOM
    }

    /// `_refreshCompatBanner()`。
    pub fn refresh_banner(&mut self, detected: Option<&str>, packs: &[PackCompat<'_>]) {
        let scheme = if self.choice == crate::proxy::CHOICE_OFF {
            String::new()
        } else if self.choice == crate::proxy::CHOICE_AUTO {
            // Python 每次都重新读 `proxy()`，不缓存。
            url_scheme(detected.unwrap_or(""))
        } else {
            self.protocol.to_lowercase()
        };
        if scheme.is_empty() {
            self.banner = None;
            return;
        }
        let names = incompatible_packs(packs, &scheme);
        self.banner = if names.is_empty() {
            None
        } else {
            Some(compat_banner_text(&names))
        };
    }

    /// `_onProxyFieldChanged()`：非自定义只清错；自定义按 url 合法性分流。
    pub fn field_changed(&mut self, detected: Option<&str>, packs: &[PackCompat<'_>]) {
        if self.choice != crate::proxy::CHOICE_CUSTOM {
            self.ip_error = false;
            self.port_error = false;
            return;
        }
        let url = self.url();
        if validate(&url) {
            self.choice_label = url;
            self.ip_error = false;
            self.port_error = false;
        } else {
            self.choice_label = RADIO_CUSTOM.to_string();
            let ip = crate::pystr::py_strip(&self.ip);
            let port = crate::pystr::py_strip(&self.port);
            self.ip_error = !ip.is_empty() && !host_pattern_ok(ip);
            self.port_error = !port.is_empty() && !port_pattern_ok(port);
        }
        self.refresh_banner(detected, packs);
    }

    /// `leaveEvent`：只有自定义且 url 合法才写 cfg。
    pub fn leave_commit(&self) -> Option<String> {
        if self.choice != crate::proxy::CHOICE_CUSTOM {
            return None;
        }
        let url = self.url();
        if validate(&url) { Some(url) } else { None }
    }
}

/// `PresetRowWidget` 描述段的图标（`FluentIcon.GLOBE/FINGERPRINT/PEOPLE`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DescIcon {
    Globe,
    Fingerprint,
    People,
}

/// 描述段：文本 + 图标（段间 `addSpacing(8)`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DescSegment {
    pub text: String,
    pub icon: DescIcon,
}

/// `PresetRowWidget._initLayout` 的三段（hosts / profile / UA），只取非空段。
pub fn desc_segments(preset: &IdentityPreset) -> Vec<DescSegment> {
    let mut segments = Vec::new();
    if !preset.hosts.is_empty() {
        segments.push(DescSegment {
            text: preset
                .hosts
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            icon: DescIcon::Globe,
        });
    }
    if !preset.client_profile.is_empty() {
        segments.push(DescSegment {
            text: crate::profile::label_of(&preset.client_profile),
            icon: DescIcon::Fingerprint,
        });
    }
    if !preset.user_agent.is_empty() {
        segments.push(DescSegment {
            text: clip_user_agent(&preset.user_agent),
            icon: DescIcon::People,
        });
    }
    segments
}

/// `_refreshOpacity(enabled)`：名称与描述段全部 `setEnabled`。
pub fn row_enabled(enabled: bool) -> bool {
    enabled
}

/// `preset.get("name", self.tr("未命名预设"))`。
pub const UNNAMED_PRESET: &str = "未命名预设";

/// `preset_name(preset)`：名称空串时用占位文案（删除确认框里用）。
pub fn preset_name(preset: &IdentityPreset) -> String {
    if preset.name.is_empty() {
        UNNAMED_PRESET.to_string()
    } else {
        preset.name.clone()
    }
}

/// `IdentitySettingCard._onToggled`：`{**presets[index], isEnabled: checked}`。
///
/// 索引越界返回 `None`（Python 直接 return，不写 cfg）。
pub fn toggle_identity(
    presets: &[IdentityPreset],
    index: usize,
    checked: bool,
) -> Option<Vec<IdentityPreset>> {
    if index >= presets.len() {
        return None;
    }
    let mut items = presets.to_vec();
    items[index].is_enabled = checked;
    Some(items)
}

/// `IdentitySettingCard._onRemoveClicked`：`presets.pop(index)`。
pub fn remove_identity(presets: &[IdentityPreset], index: usize) -> Option<Vec<IdentityPreset>> {
    if index >= presets.len() {
        return None;
    }
    let mut items = presets.to_vec();
    items.remove(index);
    Some(items)
}

/// `IdentitySettingCard._onAddClicked`：`presets.append(dialog.preset())`。
pub fn add_identity(presets: &[IdentityPreset], preset: IdentityPreset) -> Vec<IdentityPreset> {
    let mut items = presets.to_vec();
    items.push(preset);
    items
}

/// `HeadersPresetRow` 的计数文案：`"{0} 条标头".format(len(headers))`。
pub fn headers_count_label(count: usize) -> String {
    format!("{count} 条标头")
}

/// `HeadersPresetRow(isCurrent=i == current, canRemove=len(presets) > 1)`。
pub fn headers_row_state(index: usize, current: usize, total: usize) -> (bool, bool) {
    (index == current, total > 1)
}

/// `HeadersPresetSettingCard._onRemoveClicked` 的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadersRemoval {
    pub presets: Vec<HeaderPreset>,
    /// 写回 `cfg.currentHeadersPreset` 的新值。
    pub current_value: i64,
    /// `currentHeadersPresetIndex()` 的落点（单选按钮位置）。
    pub current_index: usize,
    /// `len(self._rowWidgets) > 1`。
    pub can_remove: bool,
    /// `choiceLabel` 的文案。
    pub choice_name: String,
}

/// `HeadersPresetSettingCard._onRemoveClicked`（:735-757）。
///
/// `current_value` 是 `cfg.currentHeadersPreset.value`（**删之前**读的）。
/// 索引越界或删完为空返回 `None`（Python 会在 `presets[newCurrent]` 处
/// `IndexError`，这里退化为不动作）。
pub fn remove_headers_preset(
    presets: &[HeaderPreset],
    current_value: i64,
    index: usize,
) -> Option<HeadersRemoval> {
    if index >= presets.len() {
        return None;
    }
    let mut items = presets.to_vec();
    items.remove(index);
    let current = if index as i64 <= current_value {
        (current_value - 1).max(0)
    } else {
        current_value
    };
    let len = items.len();
    if len == 0 {
        return None;
    }
    let current_index = crate::headers::current_index(Some(&current.to_string()), len);
    Some(HeadersRemoval {
        can_remove: len > 1,
        choice_name: items[current_index].name.clone(),
        presets: items,
        current_value: current,
        current_index,
    })
}

/// `SelectFolderSettingCard` 的初始 content（= `configItem.value`）。
pub fn folder_content(value: &str) -> String {
    value.to_string()
}

/// `SelectFileCard` 的初始 content：
/// `(configItem.value or hint) if configItem else hint`。
pub fn file_initial_content(value: Option<&str>, hint: &str) -> String {
    match value {
        Some(text) if !text.is_empty() => text.to_string(),
        _ => hint.to_string(),
    }
}

/// `_setPath` 之后的 content：`path or hint`。
pub fn file_content(path: &str, hint: &str) -> String {
    if path.is_empty() {
        hint.to_string()
    } else {
        path.to_string()
    }
}

/// 版本解析与比较：实现已搬到 [`gd3_core::version`]（`gd3-core::loader` 的
/// `gdMinVersion` 门禁也要用，而 `gd3-core` 不能反向依赖 `gd3-ui`）。
pub use gd3_core::version::{is_newer, parse_version};

/// `RuntimeCard` 的输入（`runtimeStatusService.status(runtime)` 的快照）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuntimeStatus {
    pub name: String,
    /// `""` = 未安装（Python `bool(status.path)`）。
    pub path: String,
    pub version: String,
    pub detail: String,
    pub latest_version: String,
    pub is_busy: bool,
    pub is_installing: bool,
    pub progress: i32,
    /// 已本地化的错误文本（Python `toLocalizedError(status.error)`）。
    pub error: Option<String>,
}

/// `RuntimeCard.updateStatus` 之后的控件状态。
///
/// `Option` 字段 = Python 该分支**没有**触碰的属性（保持原值）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuntimeCardView {
    pub content: String,
    pub install_text: Option<String>,
    pub install_visible: bool,
    pub install_installing: Option<bool>,
    pub install_progress: Option<i32>,
    pub install_tooltip: Option<String>,
    pub delete_visible: bool,
    pub refresh_enabled: bool,
}

/// `RuntimeCard.updateStatus(status)`（:864-949）。
///
/// `can_install` = `runtime.canInstall`，`is_managed` = `runtime.isAppManaged()`。
pub fn runtime_view(
    status: &RuntimeStatus,
    can_install: bool,
    is_managed: bool,
) -> RuntimeCardView {
    let refresh_enabled = !status.is_busy && !status.is_installing;
    let is_installed = !status.path.is_empty();
    let has_latest = !status.latest_version.is_empty();
    let is_update_available = is_installed
        && is_managed
        && has_latest
        && (status.version.is_empty() || is_newer(&status.version, &status.latest_version));

    if status.is_installing {
        let content = if status.progress <= 0 {
            "正在准备...".to_string()
        } else if status.progress < 100 {
            format!("正在下载... {}%", status.progress)
        } else {
            "正在安装...".to_string()
        };
        return RuntimeCardView {
            content,
            install_text: None,
            install_visible: can_install,
            install_installing: Some(true),
            install_progress: Some(status.progress),
            install_tooltip: Some("取消安装".to_string()),
            delete_visible: false,
            refresh_enabled,
        };
    }

    let mut view = RuntimeCardView {
        install_installing: Some(false),
        install_tooltip: Some(String::new()),
        refresh_enabled,
        ..RuntimeCardView::default()
    };

    if status.is_busy {
        view.content = "正在检测运行时...".to_string();
        view.install_visible = false;
        view.delete_visible = false;
        return view;
    }

    view.content = if let Some(error) = status.error.as_ref() {
        error.clone()
    } else if is_installed {
        let mut version_display = status.version.clone();
        if !status.detail.is_empty() {
            version_display = if version_display.is_empty() {
                status.detail.clone()
            } else {
                format!("{version_display} | {}", status.detail)
            };
        }
        let line1 = if !version_display.is_empty() && has_latest {
            format!("版本: {version_display}（最新: {}）", status.latest_version)
        } else if !version_display.is_empty() {
            format!("版本: {version_display}")
        } else if has_latest {
            format!("最新版本: {}", status.latest_version)
        } else {
            String::new()
        };
        let line2 = format!("路径: {}", status.path);
        if line1.is_empty() {
            line2
        } else {
            format!("{line1}\n{line2}")
        }
    } else {
        format!("未检测到可用的 {}", status.name)
    };

    if is_update_available {
        view.install_text = Some(format!("更新到 {}", status.latest_version));
        view.install_visible = true;
    } else if !is_installed || !is_managed {
        view.install_text = Some("一键安装".to_string());
        view.install_visible = can_install;
    } else {
        view.install_visible = false;
    }
    view.delete_visible = is_installed && is_managed;
    view
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_and_port_patterns_match_python() {
        assert!(host_pattern_ok("127.0.0.1"));
        assert!(host_pattern_ok("example.com"));
        assert!(host_pattern_ok("a-b.example.co"));
        // 单段主机名不是域名（Python 同样不匹配）。
        assert!(!host_pattern_ok("localhost"));
        assert!(!host_pattern_ok("256.0.0.1"));
        // 端口只查位数，不查范围。
        assert!(port_pattern_ok("99999"));
        assert!(!port_pattern_ok("123456"));
        assert!(!port_pattern_ok("80a"));
        // 与 proxy::port_valid 的口径差异：99999 在那边是错的。
        assert!(!crate::proxy::port_valid("99999"));
    }

    #[test]
    fn underline_is_the_two_pixel_band() {
        // h=40 → 圆角矩形 (ml, 30, w, 10) 减矩形 (ml, 30, w, 8) → y=38, h=2。
        let band = underline(300, 40, 12, 0);
        assert_eq!(band.y, 38);
        assert_eq!(band.height, 2);
        assert_eq!(band.radius, 1);
        assert_eq!((band.x, band.width), (12, 288));
        // 左右边距都减。
        assert_eq!(underline(200, 24, 10, 6).width, 184);
    }

    #[test]
    fn underline_visible_only_with_focus_or_error() {
        assert!(!underline_visible(false, false));
        assert!(underline_visible(true, false));
        assert!(underline_visible(false, true));
        assert!(underline_visible(true, true));
    }

    #[test]
    fn spin_box_truncates_toward_zero() {
        // range 1..256 × 0.1 → int(0.1)=0、int(25.6)=25。
        assert_eq!(spin_range(1.0, 256.0, 0.1), (0, 25));
        // value × division 同样截断。
        assert_eq!(spin_display(2.5, 1.0), 2);
        assert_eq!(spin_display(0.9, 10.0), 9);
        // 回写 int(spin/division)：19 / 10 = 1.9 → 1。
        assert_eq!(spin_commit(19, 10.0), 1);
        assert_eq!(spin_commit(20, 10.0), 2);
        // 负值向零截断（Python int(-1.5) == -1）。
        assert_eq!(spin_commit(-15, 10.0), -1);
    }

    #[test]
    fn percent_spin_box_keeps_fraction() {
        assert_eq!(percent_range(0.1, 0.9), (10.0, 90.0));
        assert_eq!(percent_display(0.35), 35.0);
        // 不截断：19.0 / 100 = 0.19。
        assert_eq!(percent_commit(19.0), 0.19);
        assert_eq!(PERCENT_SUFFIX, " %");
        assert_eq!(PERCENT_DECIMALS, 0);
        assert_eq!(PERCENT_SINGLE_STEP, 25.0);
        const { assert!(!WHEEL_ADJUSTS_VALUE) };
    }

    #[test]
    fn line_edit_commit_does_not_strip() {
        assert_eq!(line_edit_commit("  a b  "), "  a b  ");
    }

    #[test]
    fn url_scheme_mirrors_cpython_urlsplit() {
        assert_eq!(url_scheme("SOCKS5://1.2.3.4:1080"), "socks5");
        assert_eq!(url_scheme("http://a:b@h:80"), "http");
        assert_eq!(url_scheme(""), "");
        // 没有冒号 → 空。
        assert_eq!(url_scheme("1.2.3.4:1080"), "");
        // 冒号前有 '/' → 不是 scheme。
        assert_eq!(url_scheme("a/b:c"), "");
        // 首字符不是字母 → 空（"1.2.3.4:1080" 同理）。
        assert_eq!(url_scheme("9x:y"), "");
        // 前导空格与 C0 控制符被剥掉，制表/换行被删。
        assert_eq!(url_scheme(" \u{1}https://h"), "https");
        assert_eq!(url_scheme("ht\ttp://h"), "http");
        // 单段主机名带端口会被当成 scheme（CPython 同样如此）。
        assert_eq!(url_scheme("localhost:8080"), "localhost");
    }

    #[test]
    fn proxy_state_loads_auto_and_off() {
        let auto = ProxyCardState::load("Auto", Some("http://127.0.0.1:7890"), &[]);
        assert_eq!(auto.choice, crate::proxy::CHOICE_AUTO);
        assert_eq!(auto.choice_label, RADIO_AUTO);
        assert_eq!(auto.protocol, "http");
        assert_eq!(
            (auto.ip.as_str(), auto.port.as_str()),
            ("127.0.0.1", "7890")
        );
        assert!(!auto.custom_enabled());
        assert_eq!(auto.leave_commit(), None);

        let off = ProxyCardState::load("Off", Some("http://127.0.0.1:7890"), &[]);
        assert_eq!(off.choice, crate::proxy::CHOICE_OFF);
        assert_eq!(off.choice_label, RADIO_OFF);
        // Off 分支不调 `_showProxyUrl`，字段保持初始空值。
        assert_eq!(off.ip, "");
        assert_eq!(off.port, "");
    }

    #[test]
    fn proxy_state_without_detected_proxy_uses_placeholder() {
        let auto = ProxyCardState::load("Auto", None, &[]);
        assert_eq!(auto.ip, PROXY_NOT_DETECTED);
        assert_eq!(auto.protocol, "");
        assert_eq!(auto.port, "");
        assert_eq!(auto.banner, None);
    }

    #[test]
    fn proxy_state_loads_custom_value() {
        let state = ProxyCardState::load("socks5://u:p@1.2.3.4:1080", None, &[]);
        assert_eq!(state.choice, crate::proxy::CHOICE_CUSTOM);
        assert!(state.custom_enabled());
        assert_eq!(state.protocol, "socks5");
        assert_eq!(
            (state.ip.as_str(), state.port.as_str()),
            ("1.2.3.4", "1080")
        );
        assert_eq!((state.user.as_str(), state.pass.as_str()), ("u", "p"));
        // 合法 → choiceLabel 就是拼出来的 url。
        assert_eq!(state.choice_label, "socks5://u:p@1.2.3.4:1080");
        assert_eq!(
            state.leave_commit().as_deref(),
            Some("socks5://u:p@1.2.3.4:1080")
        );
    }

    #[test]
    fn proxy_field_errors_follow_non_empty_rule() {
        let mut state = ProxyCardState::load("socks5://1.2.3.4:1080", None, &[]);
        // 空字段不算错（bool(ip) 为假）。
        state.ip = String::new();
        state.port = String::new();
        state.field_changed(None, &[]);
        assert!(!state.ip_error);
        assert!(!state.port_error);
        assert_eq!(state.choice_label, RADIO_CUSTOM);
        assert_eq!(state.leave_commit(), None);

        // 非法 host + 5 位端口（"99999" 不算错）。
        state.ip = "  bad host  ".to_string();
        state.port = "99999".to_string();
        state.field_changed(None, &[]);
        assert!(state.ip_error);
        assert!(!state.port_error);
        assert_eq!(state.choice_label, RADIO_CUSTOM);

        // 6 位端口才算错。
        state.ip = "example.com".to_string();
        state.port = "123456".to_string();
        state.field_changed(None, &[]);
        assert!(!state.ip_error);
        assert!(state.port_error);
    }

    #[test]
    fn proxy_field_changed_clears_errors_when_not_custom() {
        let mut state = ProxyCardState::load("Auto", None, &[]);
        state.ip_error = true;
        state.port_error = true;
        state.field_changed(None, &[]);
        assert!(!state.ip_error);
        assert!(!state.port_error);
        // 非自定义时 choiceLabel 不被改写。
        assert_eq!(state.choice_label, RADIO_AUTO);
    }

    #[test]
    fn proxy_compat_banner_lists_incompatible_packs() {
        let all = ["socks5", "http"];
        let packs = [
            PackCompat {
                pack_id: "aria2",
                proxy_schemes: Some(&all),
            },
            PackCompat {
                pack_id: "browser",
                proxy_schemes: None,
            },
            PackCompat {
                pack_id: "ftp",
                proxy_schemes: Some(&["socks5"][..]),
            },
        ];
        assert_eq!(incompatible_packs(&packs, "socks5"), Vec::<String>::new());
        assert_eq!(incompatible_packs(&packs, "http"), vec!["FTP".to_string()]);
        assert_eq!(
            compat_banner_text(&["ARIA2".to_string(), "FTP".to_string()]),
            "ARIA2、FTP 不支持当前代理协议，将直连；使用 SOCKS5 可兼容全部下载方式"
        );
        // 空 scheme（Off）不提示。
        let mut state = ProxyCardState::load("Off", Some("http://1.2.3.4:1"), &packs);
        assert_eq!(state.banner, None);
        // 自定义 http → FTP 不支持。
        state.radio_clicked(crate::proxy::CHOICE_CUSTOM, None, &packs);
        state.protocol = "http".to_string();
        state.ip = "1.2.3.4".to_string();
        state.port = "1080".to_string();
        state.field_changed(None, &packs);
        assert_eq!(
            state.banner.as_deref(),
            Some("FTP 不支持当前代理协议，将直连；使用 SOCKS5 可兼容全部下载方式")
        );
    }

    #[test]
    fn proxy_auto_banner_uses_detected_scheme() {
        let packs = [PackCompat {
            pack_id: "aria2",
            proxy_schemes: Some(&["socks5"][..]),
        }];
        let state = ProxyCardState::load("Auto", Some("HTTP://1.2.3.4:8080"), &packs);
        assert_eq!(
            state.banner.as_deref(),
            Some("ARIA2 不支持当前代理协议，将直连；使用 SOCKS5 可兼容全部下载方式")
        );
    }

    #[test]
    fn desc_segments_follow_three_part_rule() {
        let preset = IdentityPreset {
            name: "p".into(),
            client_profile: "chrome".into(),
            user_agent: "x".repeat(30),
            hosts: vec!["a".into(), "b".into(), "c".into(), "d".into()],
            is_enabled: true,
        };
        let segments = desc_segments(&preset);
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].text, "a, b, c");
        assert_eq!(segments[0].icon, DescIcon::Globe);
        assert_eq!(segments[1].icon, DescIcon::Fingerprint);
        // UA > 24 → 前 21 + "..."。
        assert_eq!(segments[2].text, format!("{}...", "x".repeat(21)));
        assert_eq!(segments[2].icon, DescIcon::People);

        // 空字段整段省略。
        let bare = IdentityPreset {
            name: "p".into(),
            client_profile: String::new(),
            user_agent: String::new(),
            hosts: Vec::new(),
            is_enabled: false,
        };
        assert!(desc_segments(&bare).is_empty());
        assert!(!row_enabled(bare.is_enabled));
    }

    #[test]
    fn identity_toggle_and_remove_guard_index() {
        let presets = vec![IdentityPreset {
            name: "a".into(),
            client_profile: "raw".into(),
            user_agent: String::new(),
            hosts: Vec::new(),
            is_enabled: true,
        }];
        let toggled = toggle_identity(&presets, 0, false).expect("索引合法");
        assert!(!toggled[0].is_enabled);
        // 原列表不变（Python 是 list() 浅拷贝后写回）。
        assert!(presets[0].is_enabled);
        assert_eq!(toggle_identity(&presets, 1, false), None);
        assert_eq!(remove_identity(&presets, 1), None);
        assert_eq!(remove_identity(&presets, 0), Some(Vec::new()));
        assert_eq!(add_identity(&presets, presets[0].clone()).len(), 2);
        assert_eq!(preset_name(&presets[0]), "a");
        let unnamed = IdentityPreset {
            name: String::new(),
            ..presets[0].clone()
        };
        assert_eq!(preset_name(&unnamed), UNNAMED_PRESET);
    }

    #[test]
    fn headers_rows_and_removal_follow_python() {
        assert_eq!(headers_count_label(3), "3 条标头");
        assert_eq!(headers_row_state(1, 1, 2), (true, true));
        // 只剩一条时删除按钮全部禁用。
        assert_eq!(headers_row_state(0, 0, 1), (true, false));

        let presets = vec![
            HeaderPreset {
                name: "A".into(),
                headers: vec![],
            },
            HeaderPreset {
                name: "B".into(),
                headers: vec![],
            },
            HeaderPreset {
                name: "C".into(),
                headers: vec![],
            },
        ];
        // 删 index=1、current=1 → index <= current → current = 0。
        let removal = remove_headers_preset(&presets, 1, 1).expect("合法");
        assert_eq!(removal.presets.len(), 2);
        assert_eq!(removal.current_value, 0);
        assert_eq!(removal.current_index, 0);
        assert_eq!(removal.choice_name, "A");
        assert!(removal.can_remove);

        // 删 index=0、current=2 → 同样 -1 到 1，落在 C。
        let removal = remove_headers_preset(&presets, 2, 0).expect("合法");
        assert_eq!(removal.current_value, 1);
        assert_eq!(removal.choice_name, "C");

        // 删 index=2、current=0 → index > current，current 不动。
        let removal = remove_headers_preset(&presets, 0, 2).expect("合法");
        assert_eq!(removal.current_value, 0);
        assert_eq!(removal.choice_name, "A");

        // 越界不动作。
        assert_eq!(remove_headers_preset(&presets, 0, 3), None);
        // 删到只剩一条：can_remove 变 false。
        let single = vec![presets[0].clone()];
        assert_eq!(remove_headers_preset(&single, 0, 0), None);
    }

    #[test]
    fn select_file_content_falls_back_to_hint() {
        assert_eq!(file_initial_content(Some("/a"), "hint"), "/a");
        assert_eq!(file_initial_content(Some(""), "hint"), "hint");
        assert_eq!(file_initial_content(None, "hint"), "hint");
        assert_eq!(file_content("/b", "hint"), "/b");
        assert_eq!(file_content("", "hint"), "hint");
        assert_eq!(folder_content("/c"), "/c");
    }

    #[test]
    fn parse_version_mirrors_update_py() {
        assert_eq!(parse_version("v1.2.3"), vec![1, 2, 3]);
        assert_eq!(parse_version("VV2.0.0"), vec![2]);
        assert_eq!(parse_version("1.0.0.0"), vec![1]);
        assert_eq!(parse_version(""), Vec::<u64>::new());
        // 非数字开头 → 空（"latest" 完全解析不出）。
        assert_eq!(parse_version("latest"), Vec::<u64>::new());
        // 遇到非法字符即停：`1.2b3` → [1, 2]。
        assert_eq!(parse_version("1.2b3"), vec![1, 2]);
        // 连续点：第一个点后 hasDigit 为假，第二个点走 else → break。
        assert_eq!(parse_version("1..2"), vec![1]);
    }

    #[test]
    fn is_newer_compares_parsed_tuples() {
        assert!(is_newer("1.2.3", "1.2.4"));
        assert!(!is_newer("1.2.4", "1.2.3"));
        // 去尾零后相等。
        assert!(!is_newer("1.2.0", "1.2"));
        // 短前缀更小（Python 元组语义）。
        assert!(is_newer("1.2", "1.2.0.1"));
        assert!(!is_newer("", ""));
    }

    fn base_status() -> RuntimeStatus {
        RuntimeStatus {
            name: "ffmpeg".into(),
            ..RuntimeStatus::default()
        }
    }

    #[test]
    fn runtime_card_not_installed_offers_install() {
        let view = runtime_view(&base_status(), true, true);
        assert_eq!(view.content, "未检测到可用的 ffmpeg");
        assert_eq!(view.install_text.as_deref(), Some("一键安装"));
        assert!(view.install_visible);
        assert_eq!(view.install_installing, Some(false));
        assert_eq!(view.install_tooltip.as_deref(), Some(""));
        assert!(!view.delete_visible);
        assert!(view.refresh_enabled);

        // 不能安装 → 按钮隐藏。
        let view = runtime_view(&base_status(), false, true);
        assert!(!view.install_visible);
    }

    #[test]
    fn runtime_card_installed_shows_version_and_path() {
        let status = RuntimeStatus {
            path: "/opt/ffmpeg".into(),
            version: "6.0".into(),
            latest_version: "6.0".into(),
            ..base_status()
        };
        let view = runtime_view(&status, true, true);
        assert_eq!(view.content, "版本: 6.0（最新: 6.0）\n路径: /opt/ffmpeg");
        // 已装 + 托管 + 无更新 → 安装按钮隐藏，删除按钮可见。
        assert_eq!(view.install_text, None);
        assert!(!view.install_visible);
        assert!(view.delete_visible);
    }

    #[test]
    fn runtime_card_update_available_shows_update_text() {
        let status = RuntimeStatus {
            path: "/opt/ffmpeg".into(),
            version: "5.9".into(),
            detail: "static".into(),
            latest_version: "6.0".into(),
            ..base_status()
        };
        let view = runtime_view(&status, true, true);
        assert_eq!(
            view.content,
            "版本: 5.9 | static（最新: 6.0）\n路径: /opt/ffmpeg"
        );
        assert_eq!(view.install_text.as_deref(), Some("更新到 6.0"));
        assert!(view.install_visible);
    }

    #[test]
    fn runtime_card_installing_short_circuits() {
        let preparing = RuntimeStatus {
            is_installing: true,
            progress: 0,
            ..base_status()
        };
        let view = runtime_view(&preparing, true, true);
        assert_eq!(view.content, "正在准备...");
        assert_eq!(view.install_installing, Some(true));
        assert_eq!(view.install_progress, Some(0));
        assert_eq!(view.install_tooltip.as_deref(), Some("取消安装"));
        assert!(!view.refresh_enabled);
        assert!(!view.delete_visible);
        // 安装分支不改按钮文案。
        assert_eq!(view.install_text, None);

        let downloading = RuntimeStatus {
            is_installing: true,
            progress: 42,
            ..base_status()
        };
        assert_eq!(
            runtime_view(&downloading, true, true).content,
            "正在下载... 42%"
        );

        let finishing = RuntimeStatus {
            is_installing: true,
            progress: 100,
            ..base_status()
        };
        assert_eq!(runtime_view(&finishing, true, true).content, "正在安装...");
    }

    #[test]
    fn runtime_card_busy_and_error_branches() {
        let busy = RuntimeStatus {
            is_busy: true,
            ..base_status()
        };
        let view = runtime_view(&busy, true, true);
        assert_eq!(view.content, "正在检测运行时...");
        assert!(!view.install_visible);
        assert!(!view.delete_visible);
        assert!(!view.refresh_enabled);
        // 忙态不改按钮文案（Python 也没设）。
        assert_eq!(view.install_text, None);

        let failed = RuntimeStatus {
            error: Some("下载失败".into()),
            ..base_status()
        };
        let view = runtime_view(&failed, true, true);
        assert_eq!(view.content, "下载失败");
        // 错误分支仍按 isInstalled 走按钮可见性。
        assert_eq!(view.install_text.as_deref(), Some("一键安装"));
    }

    #[test]
    fn runtime_card_unmanaged_installed_shows_install_button() {
        let status = RuntimeStatus {
            path: "/usr/bin/ffmpeg".into(),
            version: "6.0".into(),
            ..base_status()
        };
        let view = runtime_view(&status, true, false);
        // 非 app 托管：不给「更新」，退回「一键安装」，删除按钮隐藏。
        assert_eq!(view.install_text.as_deref(), Some("一键安装"));
        assert!(view.install_visible);
        assert!(!view.delete_visible);
    }
}
