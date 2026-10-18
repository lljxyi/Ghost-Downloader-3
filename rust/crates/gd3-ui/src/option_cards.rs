//! 镜像 `app/view/components/option_cards.py`（303 行）：建任务抽屉里的五张
//! 选项卡（下载路径 / 预分配线程数 / 模拟身份 / 下载链接 / 请求标头）+
//! 共用的 `OptionCard` 契约与 profile 二级菜单。
//!
//! Qt 侧每张卡都是一个 `QWidget`，本模块只保留**数据契约**：
//!
//! - [`OptionCard`] trait：`options()`（喂给任务草稿的 dict）与 `reset()`；
//! - [`ProfileMenuEntry`] + [`build_profile_menu`]：`buildProfileMenu()` 的
//!   静态结构（auto / raw / 分隔 / 9 个家族子菜单，每个子菜单是
//!   「最新」+ 分隔 + 版本列表）；
//! - 五张卡的构造入参（初始值、缺省值、预设列表）与 `options()` 输出。
//!
//! ## 实证结论（逐条对着源码抄的）
//!
//! - `toProfileLabel` 直接复用 [`crate::profile::label_of`]（同一份
//!   `COMBO_ITEMS` 口径，含特定版本的 `chrome120` → `chrome 120` 拆分）。
//! - `buildProfileMenu(includeAuto=False)` 的首项**值**是空串 `""`（不是
//!   `auto`），显示串仍是「自动（匹配来源）」；`includeAuto=True` 时首项
//!   带 `FluentIcon.ROBOT`，raw 项带 `FluentIcon.CANCEL`，家族项**都没有图标**。
//! - 子菜单标题走 `PROFILE_FAMILY_LABELS.get(family, family)`，即
//!   [`crate::profile::FAMILY_LABELS`] 查不到时**原样显示家族字面量**。
//! - `ClientProfileCard` 的预设项：`if presets:` 才 `addSeparator()` 再逐条
//!   `Action(preset["name"])`；预设项**没有图标**，标签是 `preset["name"]`
//!   （不是 `toProfileLabel`）。
//! - `ClientProfileCard.__init__` 的按钮初始文本 =
//!   `_presetLabelFor(initial, initialUserAgent) or toProfileLabel(initial)`；
//!   `_presetLabelFor` 在 `userAgent` 为空时**直接返回空串**（不查预设），
//!   否则按 `clientProfile` + `userAgent` **双字段**相等匹配预设。
//! - `options()` 的 `userAgent` 键**只在 `_userAgent` 非空时出现**
//!   （预设未命中时键不存在，不是空串）。
//! - `SubworkerCountCard` 的 `value = initial or cfg.preBlockNum.value`：
//!   传 0 取缺省 8，传负数**是有效值**（`-5 or 8` → `-5`），随后被
//!   `slider.setValue()` 夹到 `[1, 256]`；`setRange` 在 `setValue` **之前**，
//!   所以夹取一定生效。
//! - `OutputFolderCard.options()` 返回 `{"outputFolder": Path(path)}`：
//!   `Path()` 会做 posix 归一（去重复斜杠 / 去 `.` / 空串变 `.`），本侧
//!   保留原始串并把归一化留给消费方（任务草稿尚未落地，见模块尾注）。
//! - `HeadersEditCard.options()` 返回 `{"headers": HeadersEditor.headers()}`，
//!   即 [`crate::headers::headers_map`] 的口径（名称 lower、键值都 strip、
//!   任一为空的行丢弃、同名后者覆盖、键序取首次出现位置）。
//! - `OptionCard.paintEvent` 只画顶部 1px 分隔线：深色 `(0,0,0,96)`、
//!   浅色 `(0,0,0,24)`。
//! - 五张卡的布局常量：`setFixedHeight(50)`（HeadersEditCard 除外，它是
//!   `Expanding/Expanding`）、图标 `16×16`、内容边距 `(24,5,24,5)`、间距 15；
//!   线程数卡额外有 `valueLabel + addSpacing(6) + slider + addSpacing(16)`，
//!   滑块 `setMinimumWidth(268)`、`setSingleStep(1)`。
//!
//! ## 已知偏差
//!
//! `Path()` 的 posix 归一没有在 `options()` 里做（`OptionValue::Path` 存原串）。
//! 理由：归一化属于「写进任务草稿」那一刻的语义，Rust 侧用 `PathBuf` 时不做
//! 同样的折叠；等任务草稿（P2）落地时在写入点统一处理，避免两处各自归一。

use crate::editors::{folder_commit, folder_path, refresh_history, save_history};
use crate::headers::headers_map;
use crate::identity::IdentityPreset;
use crate::profile::{FAMILY_LABELS, label_of, versions_of};
use crate::pystr::py_strip;

/// `OptionCard` 五张卡的 `setFixedHeight(50)`。
pub const CARD_HEIGHT: i32 = 50;
/// `IconWidget.setFixedSize(16, 16)`。
pub const ICON_SIZE: i32 = 16;
/// 卡片内容边距左右（`setContentsMargins(24, 5, 24, 5)`）。
pub const CARD_MARGIN_H: i32 = 24;
/// 卡片内容边距上下。
pub const CARD_MARGIN_V: i32 = 5;
/// 卡片布局间距（`layout.setSpacing(15)`）。
pub const CARD_SPACING: i32 = 15;
/// `slider.setMinimumWidth(268)`。
pub const SLIDER_MIN_WIDTH: i32 = 268;
/// `slider.setSingleStep(1)`。
pub const SLIDER_SINGLE_STEP: i32 = 1;
/// 线程数卡 `addSpacing(6)`（值标签与滑块之间）。
pub const VALUE_LABEL_SPACING: i32 = 6;
/// 线程数卡 `addSpacing(16)`（滑块右侧）。
pub const SLIDER_TRAILING_SPACING: i32 = 16;
/// `button.setMinimumWidth(200)`。
pub const PROFILE_BUTTON_MIN_WIDTH: i32 = 200;
/// `cfg.preBlockNum = RangeConfigItem(..., 8, RangeValidator(1, 256))` 的下界。
pub const PRE_BLOCK_MIN: i32 = 1;
/// 同上界。
pub const PRE_BLOCK_MAX: i32 = 256;
/// 同上缺省值。
pub const PRE_BLOCK_DEFAULT: i32 = 8;
/// `HeadersEditCard` 的 `setContentsMargins(24, 10, 24, 12)`。
pub const HEADERS_MARGIN: (i32, i32, i32, i32) = (24, 10, 24, 12);
/// `HeadersEditCard` 的 `setSpacing(10)`。
pub const HEADERS_SPACING: i32 = 10;
/// `HeadersEditor` 的标题行间距（`titleRowLayout.setSpacing(15)`）。
pub const HEADERS_TITLE_SPACING: i32 = 15;

/// Qt `QColor` 的 `(r, g, b, a)`。
pub type Rgba = (u8, u8, u8, u8);

/// `OptionCard.paintEvent` 的顶部 1px 分隔线颜色。
pub fn separator_color(dark: bool) -> Rgba {
    (0, 0, 0, if dark { 96 } else { 24 })
}

/// 分隔线高度（`fillRect(0, 0, width, 1, ...)`）。
pub const SEPARATOR_HEIGHT: i32 = 1;

/// profile 菜单项的图标（Qt 侧是 `FluentIcon` 枚举成员）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileIcon {
    /// 无图标（`Action(text, parent)`）。
    None,
    /// `FluentIcon.ROBOT`（auto 项）。
    Robot,
    /// `FluentIcon.CANCEL`（raw 项）。
    Cancel,
}

impl ProfileIcon {
    /// Slint 侧使用的图标名。
    pub fn name(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Robot => Some("ROBOT"),
            Self::Cancel => Some("CANCEL"),
        }
    }
}

/// `buildProfileMenu` 生成的一层菜单项（`RoundMenu` / `Action` / 分隔线）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileMenuEntry {
    /// `Action(icon, label)`，点击后 `onPick(value)`。
    Action {
        /// 显示文本。
        label: String,
        /// 回传给 `_onPick` 的 profile 字面量。
        value: String,
        /// 图标。
        icon: ProfileIcon,
    },
    /// `menu.addSeparator()`。
    Separator,
    /// `menu.addMenu(submenu)`：标题 + 子项。
    Submenu {
        /// 家族显示名（`PROFILE_FAMILY_LABELS.get(family, family)`）。
        label: String,
        /// 子项：「最新」+ 分隔 + 各版本。
        items: Vec<ProfileMenuEntry>,
    },
}

impl ProfileMenuEntry {
    fn action(label: String, value: &str, icon: ProfileIcon) -> Self {
        Self::Action {
            label,
            value: value.to_string(),
            icon,
        }
    }

    /// 若本项是可点击项，返回它的 profile 字面量。
    pub fn value(&self) -> Option<&str> {
        match self {
            Self::Action { value, .. } => Some(value),
            Self::Separator | Self::Submenu { .. } => None,
        }
    }
}

/// `buildProfileMenu(parent, onPick, *, includeAuto=True)` 的静态结构。
pub fn build_profile_menu(include_auto: bool) -> Vec<ProfileMenuEntry> {
    let mut menu = Vec::new();
    if include_auto {
        menu.push(ProfileMenuEntry::action(
            label_of("auto"),
            "auto",
            ProfileIcon::Robot,
        ));
    } else {
        menu.push(ProfileMenuEntry::action(
            label_of(""),
            "",
            ProfileIcon::None,
        ));
    }
    menu.push(ProfileMenuEntry::action(
        label_of("raw"),
        "raw",
        ProfileIcon::Cancel,
    ));
    menu.push(ProfileMenuEntry::Separator);
    for (family, label) in FAMILY_LABELS {
        let mut items = vec![ProfileMenuEntry::action(
            label_of(family),
            family,
            ProfileIcon::None,
        )];
        items.push(ProfileMenuEntry::Separator);
        for version in versions_of(family) {
            items.push(ProfileMenuEntry::action(
                label_of(version),
                version,
                ProfileIcon::None,
            ));
        }
        menu.push(ProfileMenuEntry::Submenu {
            label: label.to_string(),
            items,
        });
    }
    menu
}

/// 卡片 `options()` 的值类型（镜像 Python 返回 dict 的 value）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OptionValue {
    /// 字符串（`clientProfile` / `userAgent` / `url`）。
    Text(String),
    /// 整数（`subworkerCount`）。
    Number(i32),
    /// 路径原串（`outputFolder`，见模块头「已知偏差」）。
    Path(String),
    /// 标头键值对（`headers`，名称已 lower + strip）。
    Headers(Vec<(String, String)>),
}

/// `OptionCard` 的契约：`options()` + `reset()`。
pub trait OptionCard {
    /// 喂给任务草稿的键值对（**保持 Python dict 的插入顺序**）。
    fn options(&self) -> Vec<(&'static str, OptionValue)>;

    /// 回到构造时记下的缺省值。
    fn reset(&mut self);
}

/// `OutputFolderCard`。
#[derive(Clone, Debug)]
pub struct OutputFolderCard {
    text: String,
    path: String,
    history: Vec<String>,
    default_folder: String,
}

impl OutputFolderCard {
    /// `OutputFolderCard(parent, *, initial=None)`：`initial` 为空时取
    /// `cfg.downloadFolder.value`；随后 `refreshHistory()` 装历史。
    pub fn new(initial: Option<&str>, default_folder: &str, history: &[String]) -> Self {
        let text = initial.unwrap_or(default_folder).to_string();
        Self {
            path: folder_path(&text),
            text,
            history: refresh_history(history),
            default_folder: default_folder.to_string(),
        }
    }

    /// 输入框里的原始文本。
    pub fn text(&self) -> &str {
        &self.text
    }

    /// `folderPicker.path()`（strip 后）。
    pub fn path(&self) -> &str {
        &self.path
    }

    /// 下拉里的历史目录。
    pub fn history(&self) -> &[String] {
        &self.history
    }

    /// 用户编辑文本（尚未提交）。
    pub fn set_text(&mut self, text: &str) {
        self.text = text.to_string();
    }

    /// `_commit`：空路径不发 `pathChanged`；否则 `pathChanged` →
    /// `saveHistory(path)`。
    pub fn commit(&mut self) -> Option<String> {
        let path = folder_commit(&self.text)?;
        self.path = path.clone();
        self.history = save_history(&self.history, &path);
        Some(path)
    }
}

impl OptionCard for OutputFolderCard {
    fn options(&self) -> Vec<(&'static str, OptionValue)> {
        vec![("outputFolder", OptionValue::Path(self.path.clone()))]
    }

    fn reset(&mut self) {
        self.text = self.default_folder.clone();
        self.path = folder_path(&self.text);
    }
}

/// `SubworkerCountCard`。
#[derive(Clone, Debug)]
pub struct SubworkerCountCard {
    value: i32,
    default_value: i32,
}

impl SubworkerCountCard {
    /// `value = initial or cfg.preBlockNum.value`，再被
    /// `slider.setValue()` 夹到 `[PRE_BLOCK_MIN, PRE_BLOCK_MAX]`。
    pub fn new(initial: i32, default_value: i32) -> Self {
        let mut card = Self {
            value: 0,
            default_value,
        };
        let value = if initial != 0 { initial } else { default_value };
        card.set_value(value);
        card
    }

    /// `slider.setValue(value)`（QSlider 自带夹取）。
    pub fn set_value(&mut self, value: i32) {
        self.value = value.clamp(PRE_BLOCK_MIN, PRE_BLOCK_MAX);
    }

    /// `slider.value()`。
    pub fn value(&self) -> i32 {
        self.value
    }

    /// `valueLabel.setNum(value)`。
    pub fn value_label_text(&self) -> String {
        self.value.to_string()
    }
}

impl OptionCard for SubworkerCountCard {
    fn options(&self) -> Vec<(&'static str, OptionValue)> {
        vec![("subworkerCount", OptionValue::Number(self.value))]
    }

    fn reset(&mut self) {
        self.set_value(self.default_value);
    }
}

/// `ClientProfileCard`。
#[derive(Clone, Debug)]
pub struct ClientProfileCard {
    value: String,
    user_agent: String,
    presets: Vec<IdentityPreset>,
}

impl ClientProfileCard {
    /// `ClientProfileCard(parent, *, initial="", initialUserAgent="")`：
    /// 预设列表取 `cfg.identityPresets.value`。
    pub fn new(initial: &str, initial_user_agent: &str, presets: Vec<IdentityPreset>) -> Self {
        Self {
            value: initial.to_string(),
            user_agent: initial_user_agent.to_string(),
            presets,
        }
    }

    /// `self._value`。
    pub fn value(&self) -> &str {
        &self.value
    }

    /// `self._userAgent`。
    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }

    /// 按钮文本：`_presetLabelFor(...) or toProfileLabel(initial)`。
    pub fn button_label(&self) -> String {
        let preset = self.preset_label_for(&self.value, &self.user_agent);
        if preset.is_empty() {
            label_of(&self.value)
        } else {
            preset
        }
    }

    /// `_presetLabelFor(clientProfile, userAgent)`：`userAgent` 为空时
    /// **不查预设**，直接空串；否则双字段相等才算命中。
    pub fn preset_label_for(&self, client_profile: &str, user_agent: &str) -> String {
        if user_agent.is_empty() {
            return String::new();
        }
        self.presets
            .iter()
            .find(|preset| {
                preset.client_profile == client_profile && preset.user_agent == user_agent
            })
            .map_or_else(String::new, |preset| preset.name.clone())
    }

    /// 下拉菜单：`buildProfileMenu(includeAuto=True)` + 有预设时追加
    /// 分隔线与预设项（预设项文本 = `preset["name"]`）。
    pub fn menu(&self) -> Vec<ProfileMenuEntry> {
        let mut menu = build_profile_menu(true);
        if !self.presets.is_empty() {
            menu.push(ProfileMenuEntry::Separator);
            for preset in &self.presets {
                menu.push(ProfileMenuEntry::Action {
                    label: preset.name.clone(),
                    value: preset.client_profile.clone(),
                    icon: ProfileIcon::None,
                });
            }
        }
        menu
    }

    /// `_onPick(value)`：清空 userAgent，按钮文本走 `toProfileLabel`。
    pub fn pick(&mut self, value: &str) {
        self.value = value.to_string();
        self.user_agent.clear();
    }

    /// `_onPickPreset(preset)`：值取预设的 `clientProfile` / `userAgent`，
    /// 按钮文本取 `preset["name"]`。
    pub fn pick_preset(&mut self, preset: &IdentityPreset) {
        self.value = preset.client_profile.clone();
        self.user_agent = preset.user_agent.clone();
    }

    /// 按菜单项选择（预设项与 profile 项走不同分支）。
    pub fn pick_menu_entry(&mut self, entry: &ProfileMenuEntry) {
        let Some(value) = entry.value() else {
            return;
        };
        let preset = self
            .presets
            .iter()
            .find(|preset| preset.name == entry_label(entry))
            .cloned();
        match preset {
            Some(preset) if !matches!(entry, ProfileMenuEntry::Submenu { .. }) => {
                self.pick_preset(&preset);
            }
            _ => self.pick(value),
        }
    }
}

fn entry_label(entry: &ProfileMenuEntry) -> &str {
    match entry {
        ProfileMenuEntry::Action { label, .. } | ProfileMenuEntry::Submenu { label, .. } => label,
        ProfileMenuEntry::Separator => "",
    }
}

impl OptionCard for ClientProfileCard {
    fn options(&self) -> Vec<(&'static str, OptionValue)> {
        let mut result = vec![("clientProfile", OptionValue::Text(self.value.clone()))];
        if !self.user_agent.is_empty() {
            result.push(("userAgent", OptionValue::Text(self.user_agent.clone())));
        }
        result
    }

    fn reset(&mut self) {
        self.value.clear();
        self.user_agent.clear();
    }
}

/// `UrlEditCard`。
#[derive(Clone, Debug)]
pub struct UrlEditCard {
    text: String,
    initial: String,
}

impl UrlEditCard {
    /// `UrlEditCard(parent, *, initial="")`。
    pub fn new(initial: &str) -> Self {
        Self {
            text: initial.to_string(),
            initial: initial.to_string(),
        }
    }

    /// `urlEdit.text()`。
    pub fn text(&self) -> &str {
        &self.text
    }

    /// 用户编辑。
    pub fn set_text(&mut self, text: &str) {
        self.text = text.to_string();
    }
}

impl OptionCard for UrlEditCard {
    fn options(&self) -> Vec<(&'static str, OptionValue)> {
        vec![("url", OptionValue::Text(py_strip(&self.text).to_string()))]
    }

    fn reset(&mut self) {
        self.text = self.initial.clone();
    }
}

/// `HeadersEditCard`（表格视图下的行模型）。
#[derive(Clone, Debug)]
pub struct HeadersEditCard {
    rows: Vec<(String, String)>,
    defaults: Vec<(String, String)>,
}

impl HeadersEditCard {
    /// `HeadersEditCard(parent, *, initial=None)`：`initial is None` 时
    /// `headersEditor.reset()`，否则 `setHeaders(initial)`。
    pub fn new(defaults: &[(String, String)], initial: Option<&[(String, String)]>) -> Self {
        let mut card = Self {
            rows: Vec::new(),
            defaults: defaults.to_vec(),
        };
        match initial {
            None => card.reset(),
            Some(rows) => card.set_headers(rows),
        }
        card
    }

    /// `_setRows(rows)`：逐行 `_addRow` 之后**补一个空行**。
    pub fn set_headers(&mut self, rows: &[(String, String)]) {
        self.rows = rows.to_vec();
        self.rows.push((String::new(), String::new()));
    }

    /// 表格里的行（含末尾空行）。
    pub fn rows(&self) -> &[(String, String)] {
        &self.rows
    }

    /// 编辑第 `index` 行（名称 / 值）。
    pub fn set_row(&mut self, index: usize, name: &str, value: &str) {
        if let Some(row) = self.rows.get_mut(index) {
            row.0 = name.to_string();
            row.1 = value.to_string();
        }
    }

    /// 删掉第 `index` 行。
    pub fn remove_row(&mut self, index: usize) {
        if index < self.rows.len() {
            self.rows.remove(index);
        }
    }

    /// `HeadersEditor.headers()`（`_currentRows` 里非全空的行）。
    pub fn headers(&self) -> Vec<(String, String)> {
        let rows: Vec<(String, String)> = self
            .rows
            .iter()
            .filter(|(name, value)| !(name.is_empty() && value.is_empty()))
            .cloned()
            .collect();
        headers_map(&rows)
    }
}

impl OptionCard for HeadersEditCard {
    fn options(&self) -> Vec<(&'static str, OptionValue)> {
        vec![("headers", OptionValue::Headers(self.headers()))]
    }

    fn reset(&mut self) {
        self.set_headers(&self.defaults.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset(name: &str, profile: &str, ua: &str) -> IdentityPreset {
        IdentityPreset {
            name: name.to_string(),
            client_profile: profile.to_string(),
            user_agent: ua.to_string(),
            hosts: Vec::new(),
            is_enabled: true,
        }
    }

    #[test]
    fn card_metrics_match_python() {
        assert_eq!(CARD_HEIGHT, 50);
        assert_eq!(ICON_SIZE, 16);
        assert_eq!(CARD_MARGIN_H, 24);
        assert_eq!(CARD_MARGIN_V, 5);
        assert_eq!(CARD_SPACING, 15);
        assert_eq!(SLIDER_MIN_WIDTH, 268);
        assert_eq!(SLIDER_SINGLE_STEP, 1);
        assert_eq!(VALUE_LABEL_SPACING, 6);
        assert_eq!(SLIDER_TRAILING_SPACING, 16);
        assert_eq!(PROFILE_BUTTON_MIN_WIDTH, 200);
        assert_eq!(
            (PRE_BLOCK_MIN, PRE_BLOCK_MAX, PRE_BLOCK_DEFAULT),
            (1, 256, 8)
        );
        assert_eq!(HEADERS_MARGIN, (24, 10, 24, 12));
        assert_eq!(HEADERS_SPACING, 10);
        assert_eq!(HEADERS_TITLE_SPACING, 15);
        assert_eq!(SEPARATOR_HEIGHT, 1);
        assert_eq!(separator_color(true), (0, 0, 0, 96));
        assert_eq!(separator_color(false), (0, 0, 0, 24));
    }

    #[test]
    fn profile_menu_mirrors_build_profile_menu() {
        let menu = build_profile_menu(true);
        // auto + raw + 分隔 + 9 个子菜单。
        assert_eq!(menu.len(), 12);
        assert_eq!(menu[0].value(), Some("auto"));
        assert_eq!(
            menu[0],
            ProfileMenuEntry::Action {
                label: "自动（匹配来源）".into(),
                value: "auto".into(),
                icon: ProfileIcon::Robot,
            }
        );
        assert_eq!(menu[1].value(), Some("raw"));
        assert_eq!(
            menu[1],
            ProfileMenuEntry::Action {
                label: "不模拟（原样发送）".into(),
                value: "raw".into(),
                icon: ProfileIcon::Cancel,
            }
        );
        assert_eq!(menu[2], ProfileMenuEntry::Separator);

        let ProfileMenuEntry::Submenu { label, items } = &menu[3] else {
            panic!("第三个子项应是 chrome 子菜单");
        };
        assert_eq!(label, "Chrome");
        // 「最新」+ 分隔 + 41 个版本。
        assert_eq!(items.len(), 2 + 41);
        assert_eq!(items[0].value(), Some("chrome"));
        assert_eq!(
            items[0],
            ProfileMenuEntry::Action {
                label: "Chrome（最新）".into(),
                value: "chrome".into(),
                icon: ProfileIcon::None,
            }
        );
        assert_eq!(items[1], ProfileMenuEntry::Separator);
        assert_eq!(items[2].value(), Some("Chrome149"));
        assert_eq!(entry_label(&items[2]), "Chrome 149");

        // 无 auto 时首项值是空串，但显示串仍是「自动（匹配来源）」；
        // 条目数恒为 1（auto/空）+ 1（raw）+ 1（分隔）+ 9（家族）= 12。
        let menu = build_profile_menu(false);
        assert_eq!(menu.len(), 12);
        assert_eq!(
            menu[0],
            ProfileMenuEntry::Action {
                label: "自动（匹配来源）".into(),
                value: String::new(),
                icon: ProfileIcon::None,
            }
        );
        assert_eq!(menu[0].value(), Some(""));
    }

    #[test]
    fn profile_menu_submenu_sizes_match_registry() {
        let expected = [41usize, 19, 17, 1, 16, 20, 6, 3, 8];
        let menu = build_profile_menu(true);
        let mut seen = 0;
        for entry in &menu {
            if let ProfileMenuEntry::Submenu { label, items } = entry {
                assert_eq!(items.len(), 2 + expected[seen], "{label}");
                seen += 1;
            }
        }
        assert_eq!(seen, 9);
        // Safari 的版本号含下划线，显示成点。
        assert_eq!(label_of("Safari26_4"), "Safari 26.4");
        assert_eq!(versions_of("weird").len(), 0);
    }

    #[test]
    fn output_folder_card_options_and_reset() {
        let mut card = OutputFolderCard::new(
            None,
            "/default/download",
            &[String::new(), "/history".to_string()],
        );
        assert_eq!(card.path(), "/default/download");
        // refreshHistory 只跳空串。
        assert_eq!(card.history(), ["/history"]);
        assert_eq!(
            card.options(),
            vec![(
                "outputFolder",
                OptionValue::Path("/default/download".to_string())
            )]
        );

        card.set_text("  /edited  ");
        assert_eq!(card.path(), "/default/download");
        assert_eq!(card.commit(), Some("/edited".to_string()));
        assert_eq!(card.path(), "/edited");
        assert_eq!(card.history(), ["/edited", "/history"]);

        card.reset();
        assert_eq!(card.text(), "/default/download");
        assert_eq!(card.path(), "/default/download");
        // reset 不动历史（Python 只 setPath）。
        assert_eq!(card.history(), ["/edited", "/history"]);
    }

    #[test]
    fn output_folder_commit_ignores_blank_text() {
        let mut card = OutputFolderCard::new(Some("/x"), "/default", &[]);
        card.set_text("   ");
        assert_eq!(card.commit(), None);
        assert_eq!(card.path(), "/x");
        assert!(card.history().is_empty());
    }

    #[test]
    fn subworker_card_clamps_like_qslider() {
        let card = SubworkerCountCard::new(0, PRE_BLOCK_DEFAULT);
        assert_eq!(card.value(), 8);
        assert_eq!(card.value_label_text(), "8");

        // 负数在 Python 里是有效值（`-5 or 8` → -5），随后被夹到 1。
        let card = SubworkerCountCard::new(-5, PRE_BLOCK_DEFAULT);
        assert_eq!(card.value(), 1);
        // 超上界夹到 256。
        let card = SubworkerCountCard::new(999, PRE_BLOCK_DEFAULT);
        assert_eq!(card.value(), 256);
        // 正常值原样。
        let card = SubworkerCountCard::new(12, PRE_BLOCK_DEFAULT);
        assert_eq!(
            card.options(),
            vec![("subworkerCount", OptionValue::Number(12))]
        );

        let mut card = SubworkerCountCard::new(12, PRE_BLOCK_DEFAULT);
        card.set_value(300);
        assert_eq!(card.value(), 256);
        card.reset();
        assert_eq!(card.value(), 8);
    }

    #[test]
    fn client_profile_card_picks_and_resets() {
        let presets = vec![preset("百度网盘", "chrome", "UA-1")];
        let mut card = ClientProfileCard::new("chrome", "UA-1", presets.clone());
        // 命中预设 → 按钮文本是预设名。
        assert_eq!(card.button_label(), "百度网盘");
        assert_eq!(
            card.options(),
            vec![
                ("clientProfile", OptionValue::Text("chrome".into())),
                ("userAgent", OptionValue::Text("UA-1".into())),
            ]
        );

        card.pick("edge");
        assert_eq!(card.button_label(), "Edge（最新）");
        assert_eq!(card.user_agent(), "");
        // userAgent 为空时 options() 里没有该键。
        assert_eq!(
            card.options(),
            vec![("clientProfile", OptionValue::Text("edge".into()))]
        );

        card.pick_preset(&presets[0]);
        assert_eq!(card.button_label(), "百度网盘");
        assert_eq!(card.user_agent(), "UA-1");

        card.reset();
        assert_eq!(card.value(), "");
        assert_eq!(card.button_label(), "自动（匹配来源）");
        assert_eq!(
            card.options(),
            vec![("clientProfile", OptionValue::Text(String::new()))]
        );
    }

    #[test]
    fn client_profile_preset_label_requires_both_fields() {
        let card = ClientProfileCard::new("", "", vec![preset("P", "chrome", "UA")]);
        // userAgent 为空 → 不查预设。
        assert_eq!(card.preset_label_for("chrome", ""), "");
        assert_eq!(card.preset_label_for("chrome", "UA"), "P");
        // 只有 clientProfile 相等不算命中。
        assert_eq!(card.preset_label_for("edge", "UA"), "");
        assert_eq!(card.preset_label_for("chrome", "UA-2"), "");
    }

    #[test]
    fn client_profile_menu_appends_presets() {
        let without = ClientProfileCard::new("", "", Vec::new());
        assert_eq!(without.menu().len(), 12);

        let with = ClientProfileCard::new(
            "",
            "",
            vec![
                preset("百度网盘", "chrome", "UA-1"),
                preset("手机", "okhttp", "UA-2"),
            ],
        );
        let menu = with.menu();
        assert_eq!(menu.len(), 15); // 12 + 分隔 + 2 预设
        assert_eq!(menu[12], ProfileMenuEntry::Separator);
        assert_eq!(
            menu[13],
            ProfileMenuEntry::Action {
                label: "百度网盘".into(),
                value: "chrome".into(),
                icon: ProfileIcon::None,
            }
        );
        assert_eq!(entry_label(&menu[14]), "手机");

        // 点预设项 → 值 + UA 都来自预设。
        let mut card = ClientProfileCard::new("", "", vec![preset("百度网盘", "chrome", "UA-1")]);
        card.pick_menu_entry(&menu[13]);
        assert_eq!(card.value(), "chrome");
        assert_eq!(card.user_agent(), "UA-1");
        assert_eq!(card.button_label(), "百度网盘");

        // 点普通 profile 项 → 清空 UA。
        let raw = build_profile_menu(true)[1].clone();
        card.pick_menu_entry(&raw);
        assert_eq!(card.value(), "raw");
        assert_eq!(card.user_agent(), "");
        // 分隔线点不动。
        card.pick_menu_entry(&ProfileMenuEntry::Separator);
        assert_eq!(card.value(), "raw");
    }

    #[test]
    fn url_card_trims_on_options() {
        let mut card = UrlEditCard::new("https://example.com/a");
        assert_eq!(
            card.options(),
            vec![(
                "url",
                OptionValue::Text("https://example.com/a".to_string())
            )]
        );
        card.set_text("  https://example.com/b  ");
        assert_eq!(card.text(), "  https://example.com/b  ");
        assert_eq!(
            card.options(),
            vec![(
                "url",
                OptionValue::Text("https://example.com/b".to_string())
            )]
        );
        card.reset();
        assert_eq!(card.text(), "https://example.com/a");
    }

    #[test]
    fn headers_card_rows_and_reset() {
        let defaults = vec![
            ("User-Agent".to_string(), "UA".to_string()),
            ("accept".to_string(), "*/*".to_string()),
        ];
        let mut card = HeadersEditCard::new(&defaults, None);
        // 默认行 + 末尾空行。
        assert_eq!(card.rows().len(), 3);
        assert_eq!(card.rows()[2], (String::new(), String::new()));
        assert_eq!(
            card.headers(),
            vec![
                ("user-agent".to_string(), "UA".to_string()),
                ("accept".to_string(), "*/*".to_string()),
            ]
        );
        assert_eq!(
            card.options(),
            vec![(
                "headers",
                OptionValue::Headers(vec![
                    ("user-agent".to_string(), "UA".to_string()),
                    ("accept".to_string(), "*/*".to_string()),
                ])
            )]
        );

        // 名称 strip + lower，值 strip，任一为空的行丢弃。
        card.set_headers(&[
            ("  Referer  ".to_string(), " https://x ".to_string()),
            ("empty".to_string(), "  ".to_string()),
            ("User-Agent".to_string(), "UA2".to_string()),
        ]);
        assert_eq!(
            card.headers(),
            vec![
                ("referer".to_string(), "https://x".to_string()),
                ("user-agent".to_string(), "UA2".to_string()),
            ]
        );

        card.reset();
        assert_eq!(card.rows().len(), 3);
        assert_eq!(card.rows()[0].0, "User-Agent");
        assert_eq!(card.rows()[2], (String::new(), String::new()));
    }

    #[test]
    fn headers_card_initial_overrides_defaults() {
        let defaults = vec![("User-Agent".to_string(), "UA".to_string())];
        let initial = vec![("X-A".to_string(), "1".to_string())];
        let card = HeadersEditCard::new(&defaults, Some(&initial));
        assert_eq!(card.rows().len(), 2);
        assert_eq!(card.headers(), vec![("x-a".to_string(), "1".to_string())]);
        // 编辑 / 删除行。
        let mut card = card;
        card.set_row(0, "X-B", "2");
        assert_eq!(card.headers(), vec![("x-b".to_string(), "2".to_string())]);
        card.remove_row(0);
        assert!(card.headers().is_empty());
    }
}
