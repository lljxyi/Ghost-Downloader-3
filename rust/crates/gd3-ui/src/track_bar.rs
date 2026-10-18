//! 组件库（P4，第 71 步）：镜像 `app/view/components/track_bar.py`（132 行）——
//! 视频/音频（带菜单）+ 字幕/封面（纯勾选）四按钮组。
//!
//! ## 实证口径（2026-10-18 实读源码）
//!
//! - `TrackButton._postInit`：`setCheckable(True)`、`setFixedSize(28, 28)`、
//!   装 `ToolTipFilter`。`nextCheckState()` 被 override 成 `pass` ——
//!   **鼠标点击不翻转勾选态**（`QAbstractButton::click()` 走
//!   `nextCheckState()`，这里什么都不做）；字幕/封面因此只能由程序
//!   `setChecked()` 改变。
//! - `_drawIcon`：`isinstance(icon, FluentIconBase) and not self.isChecked()`
//!   → `icon.render(painter, rect, fill="#808080")`。四个按钮的图标都是
//!   `FluentIcon`（`FluentIconBase` 子类），所以未勾选一律灰 `#808080`。
//! - `setTrackEnabled(enabled)`：先 `setEnabled`，`not enabled` 再
//!   `setChecked(False)`；此时 `toggled` 回调 `_preventAllOff` 因
//!   `sender.isEnabled()` 为假而直接 return → **禁用可以真的熄灯**。
//! - `TrackBar._preventAllOff`：`checked` 为真直接 return；否则
//!   primary = (video, audio, cover)，三盏全灭时把 sender 拉回勾选
//!   （`setChecked(True)` 再发一次 `toggled(True)`，回调随即 return）。
//!   字幕不在 primary 里，但「三盏 primary 全灭」时它同样会被拉回。
//! - 初始态：`videoButton` + `audioButton` 勾选，其余未勾选；spinner
//!   20×20、`setStrokeWidth(3)`、初始隐藏；布局
//!   `setContentsMargins(0,0,0,0)`、`setSpacing(4)`，顺序为
//!   spinner → video → audio → subtitle → cover。
//! - `MenuTrackButton.setOptions(options, selected)`：`selected` 命中
//!   （`any(v == selected)`）才写入；否则**仅当已勾选且 options 非空**，
//!   且当前 `_selectedValue` 为空或已不在 options 里时回退
//!   `options[0][0]`。未勾选时非法 `selected` 被完全忽略。
//! - 菜单：首项「不下载」（`checked = not isChecked()`）+ 分隔线 +
//!   逐项（`checked = isChecked() and value == selected`）。
//!   `_onTrackDeselected` 先 `setChecked(False)`，**拉回成功**（即仍
//!   checked）就不清 `_selectedValue`；`_onOptionPicked` 无条件先写
//!   `_selectedValue`、再补勾选、最后发 `optionPicked(value)`。
//!
//! ## 与 Qt 的差异
//!
//! `setChecked` 在 Qt 里值没变就不发 `toggled`；本模块
//! [`TrackBar::set_checked`] 同口径，返回 `false` 表示「没有信号」。

/// `setFixedSize(28, 28)`
pub const TRACK_BUTTON_SIZE: i32 = 28;
/// 未勾选图标的填充色（`fill="#808080"`）
pub const UNCHECKED_ICON_FILL: &str = "#808080";
/// `layout.setSpacing(4)`
pub const TRACK_BAR_SPACING: i32 = 4;
/// `spinner.setFixedSize(20, 20)`
pub const SPINNER_SIZE: i32 = 20;
/// `spinner.setStrokeWidth(3)`
pub const SPINNER_STROKE_WIDTH: i32 = 3;
/// 菜单首项文字（`self.tr("不下载")`）
pub const OFF_LABEL: &str = "不下载";

/// 四个轨道的顺序即 `TrackBar.__init__` 的创建顺序。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TrackKind {
    Video,
    Audio,
    Subtitle,
    Cover,
}

impl TrackKind {
    /// 布局顺序：video → audio → subtitle → cover。
    pub const ALL: [TrackKind; 4] = [
        TrackKind::Video,
        TrackKind::Audio,
        TrackKind::Subtitle,
        TrackKind::Cover,
    ];

    pub fn index(self) -> usize {
        match self {
            TrackKind::Video => 0,
            TrackKind::Audio => 1,
            TrackKind::Subtitle => 2,
            TrackKind::Cover => 3,
        }
    }

    /// `setToolTip(self.tr(...))`
    pub fn tooltip(self) -> &'static str {
        match self {
            TrackKind::Video => "视频",
            TrackKind::Audio => "音频",
            TrackKind::Subtitle => "字幕",
            TrackKind::Cover => "封面",
        }
    }

    /// `FluentIcon.VIDEO / HEADPHONE / LANGUAGE / PHOTO`
    pub fn icon(self) -> &'static str {
        match self {
            TrackKind::Video => "VIDEO",
            TrackKind::Audio => "HEADPHONE",
            TrackKind::Subtitle => "LANGUAGE",
            TrackKind::Cover => "PHOTO",
        }
    }

    /// `_preventAllOff` 的 `primaryButtons`：**字幕不在内**。
    pub fn is_primary(self) -> bool {
        !matches!(self, TrackKind::Subtitle)
    }

    /// 只有视频/音频是 `MenuTrackButton`（`clicked` 连到 `_openMenu`）。
    pub fn has_menu(self) -> bool {
        matches!(self, TrackKind::Video | TrackKind::Audio)
    }
}

/// 单个按钮的状态（`TrackButton` / `MenuTrackButton` 的字段并集）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackButtonState {
    kind: TrackKind,
    checked: bool,
    enabled: bool,
    /// `MenuTrackButton._options`（非菜单按钮恒空）
    options: Vec<(String, String)>,
    /// `MenuTrackButton._selectedValue`
    selected: Option<String>,
}

impl TrackButtonState {
    pub fn kind(&self) -> TrackKind {
        self.kind
    }

    pub fn is_checked(&self) -> bool {
        self.checked
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn options(&self) -> &[(String, String)] {
        &self.options
    }

    pub fn selected_value(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// `_drawIcon`：未勾选时给 `#808080`，勾选时用图标自带颜色。
    pub fn icon_fill(&self) -> Option<&'static str> {
        if self.checked {
            None
        } else {
            Some(UNCHECKED_ICON_FILL)
        }
    }
}

/// 菜单里的一项（`offAction` 或某个 option 的 `Action`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItem {
    /// 菜单项文字
    pub label: String,
    /// `Some` = option 的 value；`None` = 「不下载」
    pub value: Option<String>,
    pub checked: bool,
}

/// `_openMenu` 装配出来的菜单：`items[0]` 恒为「不下载」，
/// 其后 `separator_after_first` 为真时插一条分隔线。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackMenu {
    pub items: Vec<MenuItem>,
    pub separator_after_first: bool,
}

/// 四个按钮 + spinner 的宿主。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackBar {
    buttons: [TrackButtonState; 4],
    spinner_visible: bool,
}

impl Default for TrackBar {
    fn default() -> Self {
        Self::new()
    }
}

impl TrackBar {
    /// `TrackBar.__init__`：建四个按钮、连 `toggled`、勾上视频+音频。
    pub fn new() -> Self {
        let mut buttons = TrackKind::ALL.map(|kind| TrackButtonState {
            kind,
            checked: false,
            enabled: true,
            options: Vec::new(),
            selected: None,
        });
        buttons[TrackKind::Video.index()].checked = true;
        buttons[TrackKind::Audio.index()].checked = true;
        Self {
            buttons,
            spinner_visible: false,
        }
    }

    pub fn button(&self, kind: TrackKind) -> &TrackButtonState {
        &self.buttons[kind.index()]
    }

    pub fn buttons(&self) -> &[TrackButtonState; 4] {
        &self.buttons
    }

    pub fn spinner_visible(&self) -> bool {
        self.spinner_visible
    }

    pub fn set_spinner_visible(&mut self, visible: bool) {
        self.spinner_visible = visible;
    }

    /// `setChecked(v)`：值没变就不发 `toggled`，返回是否真改了。
    ///
    /// 改完立刻走一遍 `_preventAllOff`（Qt 的 `toggled` 直连）。
    pub fn set_checked(&mut self, kind: TrackKind, checked: bool) -> bool {
        if self.button(kind).checked == checked {
            return false;
        }
        self.buttons[kind.index()].checked = checked;
        self.prevent_all_off(kind);
        true
    }

    /// `setTrackEnabled(enabled)`：禁用时连带取消勾选（且不被拉回）。
    pub fn set_track_enabled(&mut self, kind: TrackKind, enabled: bool) {
        self.buttons[kind.index()].enabled = enabled;
        if !enabled {
            self.set_checked(kind, false);
        }
    }

    /// `_preventAllOff(checked)`：`sender` 由 `kind` 代替。
    fn prevent_all_off(&mut self, kind: TrackKind) {
        if self.button(kind).checked {
            return;
        }
        if !self.button(kind).enabled {
            return;
        }
        if TrackKind::ALL
            .iter()
            .filter(|k| k.is_primary())
            .any(|k| self.button(*k).checked)
        {
            return;
        }
        self.set_checked(kind, true);
    }

    /// `MenuTrackButton.setOptions(options, selected)`。
    pub fn set_options(
        &mut self,
        kind: TrackKind,
        options: &[(&str, &str)],
        selected: Option<&str>,
    ) {
        let owned: Vec<(String, String)> = options
            .iter()
            .map(|(value, label)| ((*value).to_string(), (*label).to_string()))
            .collect();
        let state = &mut self.buttons[kind.index()];
        state.options = owned;
        if let Some(sel) = selected
            && state.options.iter().any(|(value, _)| value == sel)
        {
            state.selected = Some(sel.to_string());
            return;
        }
        if state.checked && !state.options.is_empty() {
            let stale = match &state.selected {
                None => true,
                Some(current) => !state.options.iter().any(|(value, _)| value == current),
            };
            if stale {
                state.selected = Some(state.options[0].0.clone());
            }
        }
    }

    /// `_openMenu`：首项「不下载」+ 分隔线 + 逐项。
    pub fn menu_items(&self, kind: TrackKind) -> TrackMenu {
        let state = self.button(kind);
        let mut items = vec![MenuItem {
            label: OFF_LABEL.to_string(),
            value: None,
            checked: !state.checked,
        }];
        for (value, label) in &state.options {
            items.push(MenuItem {
                label: label.clone(),
                value: Some(value.clone()),
                checked: state.checked && state.selected.as_deref() == Some(value.as_str()),
            });
        }
        TrackMenu {
            items,
            separator_after_first: true,
        }
    }

    /// `_onTrackDeselected`：拉回成功就不清 `_selectedValue`。
    pub fn on_track_deselected(&mut self, kind: TrackKind) {
        self.set_checked(kind, false);
        if !self.button(kind).checked {
            self.buttons[kind.index()].selected = None;
        }
    }

    /// `_onOptionPicked(value)`：写 `_selectedValue`、补勾选；
    /// 调用方随后发 `optionPicked(value)`（无条件发，与源码一致）。
    pub fn on_option_picked(&mut self, kind: TrackKind, value: &str) {
        self.buttons[kind.index()].selected = Some(value.to_string());
        self.set_checked(kind, true);
    }

    /// `click()`：`nextCheckState` 是 `pass` → 永不翻转勾选；
    /// 返回是否该弹菜单（即 `clicked` 是否连到 `_openMenu`）。
    pub fn click(&self, kind: TrackKind) -> bool {
        kind.has_menu()
    }

    /// 布局顺序（spinner 在最前）：`spinner → video → audio → subtitle → cover`。
    pub fn layout_order(&self) -> [Option<TrackKind>; 5] {
        [
            None,
            Some(TrackKind::Video),
            Some(TrackKind::Audio),
            Some(TrackKind::Subtitle),
            Some(TrackKind::Cover),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 尺寸/间距/spinner 常量与 tooltip、图标名逐条对齐源码。
    #[test]
    fn constants_tooltips_and_icons() {
        assert_eq!(TRACK_BUTTON_SIZE, 28);
        assert_eq!(TRACK_BAR_SPACING, 4);
        assert_eq!((SPINNER_SIZE, SPINNER_STROKE_WIDTH), (20, 3));
        assert_eq!(UNCHECKED_ICON_FILL, "#808080");
        assert_eq!(OFF_LABEL, "不下载");

        let tips: Vec<&str> = TrackKind::ALL.iter().map(|k| k.tooltip()).collect();
        assert_eq!(tips, ["视频", "音频", "字幕", "封面"]);
        let icons: Vec<&str> = TrackKind::ALL.iter().map(|k| k.icon()).collect();
        assert_eq!(icons, ["VIDEO", "HEADPHONE", "LANGUAGE", "PHOTO"]);
        assert!(TrackKind::Video.has_menu());
        assert!(TrackKind::Audio.has_menu());
        assert!(!TrackKind::Subtitle.has_menu());
        assert!(!TrackKind::Cover.has_menu());
        // primary = video/audio/cover；字幕不在内。
        assert!(TrackKind::Video.is_primary());
        assert!(TrackKind::Audio.is_primary());
        assert!(TrackKind::Cover.is_primary());
        assert!(!TrackKind::Subtitle.is_primary());
    }

    /// 初始态：视频+音频勾选，spinner 隐藏，全部可用。
    #[test]
    fn initial_state_checks_video_and_audio() {
        let bar = TrackBar::new();
        assert!(bar.button(TrackKind::Video).is_checked());
        assert!(bar.button(TrackKind::Audio).is_checked());
        assert!(!bar.button(TrackKind::Subtitle).is_checked());
        assert!(!bar.button(TrackKind::Cover).is_checked());
        assert!(TrackKind::ALL.iter().all(|k| bar.button(*k).is_enabled()));
        assert!(!bar.spinner_visible());
        assert!(
            TrackKind::ALL
                .iter()
                .all(|k| bar.button(*k).options().is_empty())
        );
        assert!(
            TrackKind::ALL
                .iter()
                .all(|k| bar.button(*k).selected_value().is_none())
        );
    }

    /// 最后一盏 primary 熄不掉（会被拉回）。
    #[test]
    fn last_primary_cannot_be_turned_off() {
        let mut bar = TrackBar::new();
        assert!(bar.set_checked(TrackKind::Video, false));
        assert!(!bar.button(TrackKind::Video).is_checked());

        // audio 是最后一盏亮着的 primary → 拉回。
        bar.set_checked(TrackKind::Audio, false);
        assert!(bar.button(TrackKind::Audio).is_checked());

        // 先点亮 cover 再熄 audio 就允许。
        bar.set_checked(TrackKind::Cover, true);
        bar.set_checked(TrackKind::Audio, false);
        assert!(!bar.button(TrackKind::Audio).is_checked());
        assert!(bar.button(TrackKind::Cover).is_checked());

        // cover 成了最后一盏 → 同样熄不掉。
        bar.set_checked(TrackKind::Cover, false);
        assert!(bar.button(TrackKind::Cover).is_checked());
    }

    /// 字幕非 primary，但三盏 primary 全灭时它也会被拉回。
    #[test]
    fn subtitle_is_pulled_back_when_all_primaries_are_off() {
        let mut bar = TrackBar::new();
        bar.set_checked(TrackKind::Subtitle, true);
        // 用「禁用」把三盏 primary 真的熄掉（禁用路径不走 _preventAllOff）。
        for kind in [TrackKind::Video, TrackKind::Audio, TrackKind::Cover] {
            bar.set_track_enabled(kind, false);
            assert!(!bar.button(kind).is_checked());
        }
        bar.set_checked(TrackKind::Subtitle, false);
        assert!(bar.button(TrackKind::Subtitle).is_checked());
    }

    /// 禁用会取消勾选且不被拉回；重新启用不恢复勾选。
    #[test]
    fn disabling_unchecks_without_prevention() {
        let mut bar = TrackBar::new();
        bar.set_track_enabled(TrackKind::Video, false);
        assert!(!bar.button(TrackKind::Video).is_checked());
        assert!(bar.button(TrackKind::Audio).is_checked());

        bar.set_track_enabled(TrackKind::Video, true);
        assert!(bar.button(TrackKind::Video).is_enabled());
        assert!(!bar.button(TrackKind::Video).is_checked());

        // 未勾选的按钮被禁用是空操作。
        let before = bar.button(TrackKind::Subtitle).is_checked();
        bar.set_track_enabled(TrackKind::Subtitle, false);
        assert_eq!(bar.button(TrackKind::Subtitle).is_checked(), before);
    }

    /// 重复 `set_checked` 同值不发信号（返回 false）。
    #[test]
    fn repeated_set_checked_is_a_noop() {
        let mut bar = TrackBar::new();
        assert!(!bar.set_checked(TrackKind::Video, true));
        assert!(!bar.set_checked(TrackKind::Subtitle, false));
        assert!(bar.set_checked(TrackKind::Subtitle, true));
    }

    /// `setOptions` 的 selected 校验与回退。
    #[test]
    fn set_options_selection_rules() {
        let options = [("v1", "1080P"), ("v2", "720P")];
        let mut bar = TrackBar::new();

        // 命中 → 直接写入。
        bar.set_options(TrackKind::Video, &options, Some("v2"));
        assert_eq!(bar.button(TrackKind::Video).selected_value(), Some("v2"));

        // 未勾选 + 非法 selected → 完全忽略，保留原值。
        bar.set_checked(TrackKind::Video, false);
        bar.set_options(TrackKind::Video, &options, Some("nope"));
        assert_eq!(bar.button(TrackKind::Video).selected_value(), Some("v2"));

        // 勾选 + selected 为空 + 当前值已不在 options → 回退第一项。
        bar.set_checked(TrackKind::Video, true);
        bar.set_options(TrackKind::Video, &[("v9", "4K")], None);
        assert_eq!(bar.button(TrackKind::Video).selected_value(), Some("v9"));

        // 勾选 + selected 为空 + 当前值仍在 → 不动。
        bar.set_options(TrackKind::Video, &[("v9", "4K"), ("v1", "1080P")], None);
        assert_eq!(bar.button(TrackKind::Video).selected_value(), Some("v9"));

        // 未勾选 + selected 为空 + 当前值有效 → 不动。
        bar.set_checked(TrackKind::Video, false);
        bar.set_options(TrackKind::Video, &[("v9", "4K")], None);
        assert_eq!(bar.button(TrackKind::Video).selected_value(), Some("v9"));
    }

    /// 菜单装配：不下载 + 分隔线 + 逐项勾选态。
    #[test]
    fn menu_items_mirror_open_menu() {
        let options = [("v1", "1080P"), ("v2", "720P")];
        let mut bar = TrackBar::new();

        // 未勾选：不下载勾上，选项全不勾。
        // （`TrackBar` 初始就勾着 video+audio，这里先熄掉 video——audio
        // 还亮着，所以熄得掉。）
        bar.set_checked(TrackKind::Video, false);
        bar.set_options(TrackKind::Video, &options, None);
        let menu = bar.menu_items(TrackKind::Video);
        assert!(menu.separator_after_first);
        assert_eq!(menu.items.len(), 3);
        assert_eq!(menu.items[0].label, "不下载");
        assert_eq!(menu.items[0].value, None);
        assert!(menu.items[0].checked);
        assert_eq!(
            menu.items[1],
            MenuItem {
                label: "1080P".into(),
                value: Some("v1".into()),
                checked: false,
            }
        );

        // 勾选 + selected=v2 → 只有 v2 勾上，不下载不勾。
        bar.set_checked(TrackKind::Video, true);
        bar.set_options(TrackKind::Video, &options, Some("v2"));
        let menu = bar.menu_items(TrackKind::Video);
        assert!(!menu.items[0].checked);
        assert!(!menu.items[1].checked);
        assert!(menu.items[2].checked);
        assert_eq!(menu.items[2].value.as_deref(), Some("v2"));

        // 字幕没有菜单：只有「不下载」一项。
        let plain = bar.menu_items(TrackKind::Subtitle);
        assert_eq!(plain.items.len(), 1);
        assert!(plain.items[0].checked);
    }

    /// `_onOptionPicked` / `_onTrackDeselected` 的次序与副作用。
    #[test]
    fn option_pick_and_deselect() {
        let mut bar = TrackBar::new();
        bar.set_options(TrackKind::Video, &[("v1", "1080P"), ("v2", "720P")], None);
        assert_eq!(bar.button(TrackKind::Video).selected_value(), Some("v1"));

        bar.set_checked(TrackKind::Video, false);
        bar.on_option_picked(TrackKind::Video, "v2");
        assert!(bar.button(TrackKind::Video).is_checked());
        assert_eq!(bar.button(TrackKind::Video).selected_value(), Some("v2"));

        // audio 还亮着，所以 video 能真的熄灯 → 清 selected。
        bar.on_track_deselected(TrackKind::Video);
        assert!(!bar.button(TrackKind::Video).is_checked());
        assert_eq!(bar.button(TrackKind::Video).selected_value(), None);

        // 最后一盏 primary 熄不掉 → selected 保留。
        // 此刻 video 已熄、cover 从未亮过，熄 audio 就是三盏 primary 全灭，
        // `_preventAllOff` 会把 sender 拉回勾选。
        assert!(!bar.button(TrackKind::Video).is_checked());
        assert!(!bar.button(TrackKind::Cover).is_checked());
        bar.set_options(TrackKind::Audio, &[("a1", "320K")], None);
        assert_eq!(bar.button(TrackKind::Audio).selected_value(), Some("a1"));
        bar.on_track_deselected(TrackKind::Audio);
        assert!(bar.button(TrackKind::Audio).is_checked());
        assert_eq!(bar.button(TrackKind::Audio).selected_value(), Some("a1"));
    }

    /// 点击不翻转勾选，只有带菜单的按钮返回「弹菜单」。
    #[test]
    fn click_never_toggles() {
        let bar = TrackBar::new();
        for kind in TrackKind::ALL {
            let before = bar.button(kind).is_checked();
            assert_eq!(bar.click(kind), kind.has_menu());
            assert_eq!(bar.button(kind).is_checked(), before);
        }
    }

    /// 图标填充色随勾选态切换。
    #[test]
    fn icon_fill_is_gray_only_when_unchecked() {
        let mut bar = TrackBar::new();
        assert_eq!(bar.button(TrackKind::Video).icon_fill(), None);
        assert_eq!(
            bar.button(TrackKind::Cover).icon_fill(),
            Some(UNCHECKED_ICON_FILL)
        );
        bar.set_checked(TrackKind::Cover, true);
        assert_eq!(bar.button(TrackKind::Cover).icon_fill(), None);
    }
}
