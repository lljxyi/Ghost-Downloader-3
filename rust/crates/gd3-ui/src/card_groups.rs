//! 卡片分组容器（P1）：镜像 `app/view/components/card_groups.py`
//! `TitledCardGroup` / `DraftCardGroup` / `OptionCardGroup`。
//!
//! Python 先验（2026-10-18 实读源码，逐条对应）：
//!
//! - **标题前缀 4 个空格**：`setTitle` 是 `headerLabel.setText("    " + title)`
//!   （card_groups.py:37-38），`headerLabel` 固定高 30（:23）。
//! - **分组背景/边框**（:46-48）：`drawRoundedRect(rect().adjusted(1, 1, -1, -1), 5, 5)`，
//!   背景 `QColor(255, 255, 255, 13 if dark else 200)`、边框
//!   `QColor(0, 0, 0, 96 if dark else 24)`。
//! - **草稿组统计**（:86-106）：`total = success + fail`；`total == 0` 两个标签都藏；
//!   否则比例标签 `"{success}/{total}"` **仅 `fail > 0`** 显示；尺寸标签按
//!   `totalSize > 0` → `toReadableSize`、否则 `success > 0` → `"{success} 个任务"`、
//!   否则藏。比例标签配色 `QColor(196, 53, 1)`（浅）/ `QColor(252, 169, 3)`（深）。
//! - **`addCard(url, card)`**（:108-115）：同 url 先 `removeWidget + deleteLater`
//!   再登记，然后走 `super().addCard` **追加到末尾**——同 url 的卡片会**移位到尾部**。
//! - **`setUrls(urls)`**（:117-128）：先删不在 `set(urls)` 里的卡片，再对
//!   `[u for u in urls if u in self._cardByUrl]` 逐项 `if indexOf(card) != i:
//!   insertWidget(i, card)`——即按 urls 顺序重排（重复 url 会被移动两次，
//!   本模块照抄这个行为）。
//! - **高度**（:138-160）：`_cardHeight = card.height() or sizeHint().height() or
//!   CARD_HEIGHT(35)`；`sizeHint = max(padding + 前 MIN_VISIBLE(5) 张高度和,
//!   padding + 全部高度和)`；`minimumSizeHint = padding + 前 MIN_VISIBLE 张`。
//!   `_paddingHeight = margins.top + margins.bottom + headerLabel.height() +
//!   frameWidth * 2 + viewportMargins.top + viewportMargins.bottom`——这些是 Qt
//!   布局实测值，本模块一律当入参 `padding` 收。
//! - **`OptionCardGroup.options()`**（:183-187）：逐卡 `result.update(card.options())`
//!   ——**后者覆盖前者**，dict 键序取首次出现位置（Rust 侧用 Vec 保序 + 原位替换）。
//! - **`insertCard(index, card)`**（:179-181）：`list.insert` 语义（越界追加、
//!   负数从尾部数）。
//!
//! 不复刻项：`ScrollArea` / `QSizePolicy` / `QFont` 等 Qt 控件属性（属绘制层），
//! 以及 `setSizePolicy(Expanding, Maximum)` 这类布局约束。

use crate::format::to_readable_size;
use crate::option_cards::{OptionCard, OptionValue};

/// 标题前缀（`"    " + title`，card_groups.py:38）。
pub const TITLE_PREFIX: &str = "    ";
/// `headerLabel.setFixedHeight(30)`（:23）。
pub const HEADER_HEIGHT: i32 = 30;
/// `drawRoundedRect(..., 5, 5)`（:48）。
pub const GROUP_RADIUS: f64 = 5.0;
/// `rect().adjusted(1, 1, -1, -1)`（:48）。
pub const GROUP_INSET: (i32, i32, i32, i32) = (1, 1, -1, -1);
/// 统计标签字号 13（:63）与固定高 30（:64）。
pub const STATS_FONT_SIZE: i32 = 13;
pub const STATS_HEIGHT: i32 = 30;
/// 头部行 `setSpacing(8)`（:75）。
pub const HEADER_ROW_SPACING: i32 = 8;
/// 右侧补白 `QLabel("  ")`（:68）。
pub const RIGHT_PAD_TEXT: &str = "  ";
/// 两个分组的标题（`self.tr(...)`，:82 / :172）。
pub const TITLE_DRAFT: &str = "解析结果";
pub const TITLE_OPTIONS: &str = "下载设置";
/// `DraftCardGroup.CARD_HEIGHT = 35` / `MIN_VISIBLE = 5`（:52-53）。
pub const DRAFT_CARD_HEIGHT: i32 = 35;
pub const DRAFT_MIN_VISIBLE: usize = 5;

/// Qt `QColor` 的 RGBA 口径（本 crate 统一用元组表达）。
pub type Rgba = (u8, u8, u8, u8);

/// `painter.setBrush(QColor(255, 255, 255, 13 if dark else 200))`（:46）。
pub fn group_background(dark: bool) -> Rgba {
    (255, 255, 255, if dark { 13 } else { 200 })
}

/// `painter.setPen(QColor(0, 0, 0, 96 if dark else 24))`（:47）。
pub fn group_border(dark: bool) -> Rgba {
    (0, 0, 0, if dark { 96 } else { 24 })
}

/// 比例标签的双主题色（:60 `QColor(196, 53, 1)` / `QColor(252, 169, 3)`）。
pub fn ratio_color(dark: bool) -> Rgba {
    if dark {
        (252, 169, 3, 255)
    } else {
        (196, 53, 1, 255)
    }
}

/// `setTitle` 的实际文本（4 空格前缀）。
pub fn title_text(title: &str) -> String {
    format!("{TITLE_PREFIX}{title}")
}

/// `rect().adjusted(1, 1, -1, -1)` 的结果（x, y, w, h）。
///
/// Qt 的 `adjusted` 对宽高各减两次：`w - 2`、`h - 2`。
pub fn inset_rect(width: i32, height: i32) -> (i32, i32, i32, i32) {
    (
        GROUP_INSET.0,
        GROUP_INSET.1,
        width + GROUP_INSET.2 - GROUP_INSET.0,
        height + GROUP_INSET.3 - GROUP_INSET.1,
    )
}

/// `DraftCardGroup.updateStats` 的结果（两个标签的可见性与文本）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DraftStats {
    pub ratio_visible: bool,
    pub ratio_text: String,
    pub size_visible: bool,
    pub size_text: String,
}

/// 镜像 `DraftCardGroup.updateStats(success, fail, totalSize)`（:86-106）。
pub fn update_stats(success: usize, fail: usize, total_size: i64) -> DraftStats {
    let total = success + fail;
    if total == 0 {
        return DraftStats::default();
    }
    let (ratio_visible, ratio_text) = if fail > 0 {
        (true, format!("{success}/{total}"))
    } else {
        (false, String::new())
    };
    let (size_visible, size_text) = if total_size > 0 {
        (true, to_readable_size(total_size as f64))
    } else if success > 0 {
        (true, format!("{success} 个任务"))
    } else {
        (false, String::new())
    };
    DraftStats {
        ratio_visible,
        ratio_text,
        size_visible,
        size_text,
    }
}

/// 草稿组里的一张卡：`url` 是 `_cardByUrl` 的键，`height` / `size_hint`
/// 是 Qt 布局实测值（`0` = 尚未布局，等价 Python 的 falsy）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DraftCard {
    pub url: String,
    pub height: i32,
    pub size_hint: i32,
}

impl DraftCard {
    pub fn new(url: impl Into<String>, height: i32, size_hint: i32) -> Self {
        Self {
            url: url.into(),
            height,
            size_hint,
        }
    }

    /// `card.height() or card.sizeHint().height() or self.CARD_HEIGHT`（:159-160）。
    pub fn effective_height(&self) -> i32 {
        if self.height != 0 {
            self.height
        } else if self.size_hint != 0 {
            self.size_hint
        } else {
            DRAFT_CARD_HEIGHT
        }
    }
}

/// `DraftCardGroup` 的数据层：卡片列表 + `_cardByUrl` 的顺序语义。
///
/// Python 侧同时维护 `_cardByUrl` 与 `scrollLayout` 两个顺序源；这里只留
/// 一个 `Vec<DraftCard>`，因为 `setUrls` 之后两者是一致的（见模块头）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DraftCardGroupState {
    cards: Vec<DraftCard>,
}

impl DraftCardGroupState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cards(&self) -> &[DraftCard] {
        &self.cards
    }

    pub fn len(&self) -> usize {
        self.cards.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }

    /// 镜像 `addCard(url, card)`：同 url 先删旧卡，再追加到末尾。
    ///
    /// 返回被替换掉的旧卡下标（`None` = 新增）。
    pub fn add_card(&mut self, url: &str, height: i32, size_hint: i32) -> Option<usize> {
        let replaced = self.cards.iter().position(|card| card.url == url);
        if let Some(index) = replaced {
            self.cards.remove(index);
        }
        self.cards.push(DraftCard::new(url, height, size_hint));
        replaced
    }

    /// 镜像 `setUrls(urls)`：删多余卡片 + 按 urls 顺序重排。
    ///
    /// 重复 url 会被移动两次（Python 的 `insertWidget` 语义），照抄。
    pub fn set_urls(&mut self, urls: &[String]) {
        self.cards.retain(|card| urls.contains(&card.url));
        // `u for u in urls if u in self._cardByUrl`：`_cardByUrl` 在循环里不变
        // （只移动控件，不动字典），故先物化出待移动序列再逐个挪。
        let targets: Vec<String> = urls
            .iter()
            .filter(|url| self.cards.iter().any(|card| card.url == **url))
            .cloned()
            .collect();
        for (target, url) in targets.iter().enumerate() {
            let Some(current) = self.cards.iter().position(|card| card.url == *url) else {
                continue;
            };
            if current == target {
                continue;
            }
            let card = self.cards.remove(current);
            let index = target.min(self.cards.len());
            self.cards.insert(index, card);
        }
    }

    /// 镜像 `clear()`：清空布局与 `_cardByUrl`。
    pub fn clear(&mut self) {
        self.cards.clear();
    }

    /// 镜像 `sizeHint()`：`max(padding + 前 MIN_VISIBLE 张, padding + 全部)`
    /// （后者恒不小于前者，故结果就是 `padding + 全部高度和`）。
    ///
    /// `width` 取 `super().sizeHint()` 的宽度（Qt 布局实测值，调用方给）。
    pub fn size_hint(&self, padding: i32, width: i32) -> (i32, i32) {
        let content: i32 = self.cards.iter().map(DraftCard::effective_height).sum();
        let minimum: i32 = self
            .cards
            .iter()
            .take(DRAFT_MIN_VISIBLE)
            .map(DraftCard::effective_height)
            .sum();
        (width, (padding + minimum).max(padding + content))
    }

    /// 镜像 `minimumSizeHint()`：`padding + 前 MIN_VISIBLE 张`。
    pub fn minimum_size_hint(&self, padding: i32, width: i32) -> (i32, i32) {
        let content: i32 = self
            .cards
            .iter()
            .take(DRAFT_MIN_VISIBLE)
            .map(DraftCard::effective_height)
            .sum();
        (width, padding + content)
    }
}

/// `OptionCardGroup` 的数据层：卡片序列 + `options()` / `reset()` 聚合。
///
/// 卡片用 `Box<dyn OptionCard>` 存，语义（键名、取值、缺省）全在各卡实现里。
#[derive(Default)]
pub struct OptionCardGroupState {
    cards: Vec<Box<dyn OptionCard>>,
}

impl OptionCardGroupState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.cards.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }

    /// 镜像 `addCard`：追加到末尾。
    pub fn add_card(&mut self, card: Box<dyn OptionCard>) {
        self.cards.push(card);
    }

    /// 镜像 `insertCard(index, card)`：`list.insert` 语义——越界追加，
    /// 负数从尾部数（小于 `-len` 则落在 0）。
    pub fn insert_card(&mut self, index: i64, card: Box<dyn OptionCard>) {
        let len = self.cards.len() as i64;
        let index = if index < 0 { index + len } else { index };
        let index = index.clamp(0, len) as usize;
        self.cards.insert(index, card);
    }

    /// 镜像 `options()`：逐卡 `dict.update`——**后者覆盖前者**，键序保留
    /// 首次出现的位置。
    pub fn options(&self) -> Vec<(&'static str, OptionValue)> {
        let mut merged: Vec<(&'static str, OptionValue)> = Vec::new();
        for card in &self.cards {
            for (key, value) in card.options() {
                match merged.iter_mut().find(|(existing, _)| *existing == key) {
                    Some(slot) => slot.1 = value,
                    None => merged.push((key, value)),
                }
            }
        }
        merged
    }

    /// 镜像 `reset()`：逐卡回到构造时记下的缺省值。
    pub fn reset(&mut self) {
        for card in &mut self.cards {
            card.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::option_cards::{OutputFolderCard, SubworkerCountCard};

    #[test]
    fn title_gets_four_space_prefix_and_theme_colors() {
        assert_eq!(title_text("解析结果"), "    解析结果");
        assert_eq!(TITLE_DRAFT, "解析结果");
        assert_eq!(TITLE_OPTIONS, "下载设置");
        assert_eq!(group_background(true), (255, 255, 255, 13));
        assert_eq!(group_background(false), (255, 255, 255, 200));
        assert_eq!(group_border(true), (0, 0, 0, 96));
        assert_eq!(group_border(false), (0, 0, 0, 24));
        assert_eq!(ratio_color(false), (196, 53, 1, 255));
        assert_eq!(ratio_color(true), (252, 169, 3, 255));
    }

    #[test]
    fn inset_rect_shrinks_both_axes_by_two() {
        assert_eq!(inset_rect(100, 50), (1, 1, 98, 48));
        assert_eq!(inset_rect(0, 0), (1, 1, -2, -2));
    }

    #[test]
    fn update_stats_hides_everything_when_total_is_zero() {
        let stats = update_stats(0, 0, 4096);
        assert_eq!(stats, DraftStats::default());
        assert!(!stats.ratio_visible);
        assert!(!stats.size_visible);
    }

    #[test]
    fn update_stats_shows_ratio_only_when_some_failed() {
        let ok = update_stats(3, 0, 1024);
        assert!(!ok.ratio_visible);
        assert_eq!(ok.ratio_text, "");
        assert!(ok.size_visible);
        assert_eq!(ok.size_text, "1.00 KB");

        let mixed = update_stats(3, 2, 1024);
        assert!(mixed.ratio_visible);
        assert_eq!(mixed.ratio_text, "3/5");

        let all_failed = update_stats(0, 4, 0);
        assert!(all_failed.ratio_visible);
        assert_eq!(all_failed.ratio_text, "0/4");
        // totalSize == 0 且 success == 0 → 尺寸标签藏。
        assert!(!all_failed.size_visible);
        assert_eq!(all_failed.size_text, "");
    }

    #[test]
    fn update_stats_falls_back_to_task_count() {
        let stats = update_stats(2, 0, 0);
        assert!(stats.size_visible);
        assert_eq!(stats.size_text, "2 个任务");
        // totalSize > 0 优先于任务数（哪怕 success 为 0）。
        let sized = update_stats(0, 1, 1536);
        assert_eq!(sized.size_text, "1.50 KB");
    }

    #[test]
    fn card_height_falls_back_to_size_hint_then_constant() {
        assert_eq!(DraftCard::new("a", 40, 20).effective_height(), 40);
        assert_eq!(DraftCard::new("a", 0, 20).effective_height(), 20);
        assert_eq!(
            DraftCard::new("a", 0, 0).effective_height(),
            DRAFT_CARD_HEIGHT
        );
        assert_eq!(DRAFT_CARD_HEIGHT, 35);
    }

    #[test]
    fn add_card_replaces_same_url_and_moves_it_to_tail() {
        let mut group = DraftCardGroupState::new();
        assert_eq!(group.add_card("a", 40, 0), None);
        assert_eq!(group.add_card("b", 40, 0), None);
        assert_eq!(group.add_card("a", 50, 0), Some(0));
        assert_eq!(group.len(), 2);
        assert_eq!(
            group
                .cards()
                .iter()
                .map(|c| c.url.as_str())
                .collect::<Vec<_>>(),
            vec!["b", "a"]
        );
        // 新卡的几何值覆盖旧卡。
        assert_eq!(group.cards()[1].height, 50);
    }

    #[test]
    fn set_urls_drops_extras_and_reorders() {
        let mut group = DraftCardGroupState::new();
        for url in ["a", "b", "c"] {
            group.add_card(url, 35, 0);
        }
        group.set_urls(&["c".to_string(), "a".to_string()]);
        assert_eq!(
            group
                .cards()
                .iter()
                .map(|c| c.url.as_str())
                .collect::<Vec<_>>(),
            vec!["c", "a"]
        );
        // urls 里没有的卡片被删掉。
        assert!(!group.cards().iter().any(|c| c.url == "b"));
    }

    #[test]
    fn set_urls_with_duplicate_moves_twice_like_qt() {
        let mut group = DraftCardGroupState::new();
        for url in ["a", "b", "c"] {
            group.add_card(url, 35, 0);
        }
        // keep = set(urls) = {"a", "b"} → c 先被删掉（retain 在重排之前）。
        // i=0：a 在 0；i=1：b 在 1；i=2：a 在 0 ≠ 2 → 移到尾部，b 被挤到前面。
        group.set_urls(&["a".to_string(), "b".to_string(), "a".to_string()]);
        assert_eq!(
            group
                .cards()
                .iter()
                .map(|c| c.url.as_str())
                .collect::<Vec<_>>(),
            vec!["b", "a"]
        );
    }

    #[test]
    fn clear_empties_the_group() {
        let mut group = DraftCardGroupState::new();
        group.add_card("a", 35, 0);
        group.clear();
        assert!(group.is_empty());
    }

    #[test]
    fn size_hints_use_padding_plus_min_visible() {
        let mut group = DraftCardGroupState::new();
        // 7 张 35 高的卡 + 1 张未布局（落 CARD_HEIGHT）。
        for i in 0..7 {
            group.add_card(&format!("u{i}"), 35, 0);
        }
        group.add_card("unlaid", 0, 0);
        // 全部 8 张 × 35 = 280；前 5 张 = 175。
        assert_eq!(group.size_hint(100, 640), (640, 100 + 280));
        assert_eq!(group.minimum_size_hint(100, 640), (640, 100 + 175));
    }

    #[test]
    fn size_hint_uses_size_hint_when_height_is_zero() {
        let mut group = DraftCardGroupState::new();
        group.add_card("a", 0, 60);
        assert_eq!(group.size_hint(10, 320), (320, 70));
        assert_eq!(group.minimum_size_hint(10, 320), (320, 70));
    }

    #[test]
    fn option_group_merges_later_cards_over_earlier() {
        let mut group = OptionCardGroupState::new();
        group.add_card(Box::new(OutputFolderCard::new(Some("/a"), "/default", &[])));
        group.add_card(Box::new(SubworkerCountCard::new(4, 8)));
        let options = group.options();
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].0, "outputFolder");
        assert_eq!(options[1].0, "subworkerCount");
        assert_eq!(options[1].1, OptionValue::Number(4));

        // 后加的卡片覆盖同名键，但键序保留首次出现的位置。
        let mut group = OptionCardGroupState::new();
        group.add_card(Box::new(SubworkerCountCard::new(2, 8)));
        group.add_card(Box::new(OutputFolderCard::new(Some("/a"), "/default", &[])));
        group.add_card(Box::new(SubworkerCountCard::new(6, 8)));
        let options = group.options();
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].0, "subworkerCount");
        assert_eq!(options[0].1, OptionValue::Number(6));
        assert_eq!(options[1].0, "outputFolder");
    }

    #[test]
    fn option_group_insert_and_reset_follow_python_list_semantics() {
        let mut group = OptionCardGroupState::new();
        group.add_card(Box::new(SubworkerCountCard::new(4, 8)));
        // insert 到 0：新卡在前 → **键序**随卡片顺序改变（同名键会互相覆盖，
        // 所以这里用不同键才看得见顺序）。
        group.insert_card(
            0,
            Box::new(OutputFolderCard::new(Some("/a"), "/default", &[])),
        );
        let keys: Vec<&str> = group.options().iter().map(|(key, _)| *key).collect();
        assert_eq!(keys, vec!["outputFolder", "subworkerCount"]);

        // 同名键：`dict.update` 语义——后者覆盖前者，键序仍取首次出现的位置。
        group.add_card(Box::new(SubworkerCountCard::new(6, 8)));
        let options = group.options();
        assert_eq!(options.len(), 2);
        assert_eq!(options[1].0, "subworkerCount");
        assert_eq!(options[1].1, OptionValue::Number(6));

        // 越界追加（clamp 到 len）：新卡在尾部，故 outputFolder 取它的值。
        group.insert_card(
            99,
            Box::new(OutputFolderCard::new(Some("/b"), "/default", &[])),
        );
        assert_eq!(group.len(), 4);
        assert_eq!(group.options()[0].1, OptionValue::Path("/b".to_string()));

        // 负数从尾部数：-1 → len - 1，插到最后一个之前，故合并值仍是 /b。
        group.insert_card(
            -1,
            Box::new(OutputFolderCard::new(Some("/c"), "/default", &[])),
        );
        assert_eq!(group.len(), 5);
        assert_eq!(group.options()[0].1, OptionValue::Path("/b".to_string()));

        // 小于 -len 的索引落在 0（Python `list.insert(-99, x)` 插到头部）。
        group.insert_card(
            -99,
            Box::new(OutputFolderCard::new(Some("/z"), "/default", &[])),
        );
        assert_eq!(group.len(), 6);
        assert_eq!(group.options()[0].1, OptionValue::Path("/b".to_string()));
    }

    #[test]
    fn option_group_reset_restores_each_card_initial_value() {
        let mut card = SubworkerCountCard::new(4, 8);
        card.set_value(7);
        let mut group = OptionCardGroupState::new();
        group.add_card(Box::new(card));
        assert_eq!(group.options()[0].1, OptionValue::Number(7));

        group.reset();
        // `SubworkerCountCard.reset()` 回到构造时的 `default_value`（8），
        // 不是 `initial`（4）——与 Python 的 `slider.setValue(cfg.preBlockNum.value)`
        // 一致：缺省值由 cfg 给。
        assert_eq!(group.options()[0].1, OptionValue::Number(8));
    }
}
