//! 设置页卡片分组容器（第 61 步）：镜像 `app/view/components/setting_card_group.py`
//! `CollapsibleSettingCardGroup` 与 `app/view/pages/setting_page.py` 的搜索 /
//! 排序 / 空态逻辑。
//!
//! Python 先验（2026-10-17 实读源码，逐条对应）：
//!
//! - **折叠态存 `UI/ExpandedSettingGroups`（StringList，缺省 []）**：
//!   `_collapsed = objectName() not in cfg.expandedSettingGroups.value`
//!   （setting_card_group.py:151）——即「列表里没有 key 就是折叠」。
//! - **点展开写回**（:242-247）：展开时 append、折叠时 remove，**保持既有顺序**。
//! - **搜索**（:207-215）：`text.strip().lower()` 后逐卡判
//!   `needle in title.lower() or needle in content.lower()`；卡片可见 =
//!   `not text or isMatch`；分组可见 = `hasMatch or not text`；展开 =
//!   `(bool(text) and hasMatch) or key in expanded`——**搜索只改视觉展开态，
//!   不写 cfg**。
//! - **可折叠卡（Proxy/Identity/HeadersPreset）的搜索文本取头部标题/副标题**
//!   （`isSearchMatch` :108-111 把 `CollapsibleSettingCard` 换成 `card.card`），
//!   本模块用已摊平的 `(title, content)` 表达。
//! - **启动排序**（setting_page.py:587-609）：`stored` 中仍存在的 key 按 stored
//!   顺序在前，其余按创建顺序在后；`about` 从「其余」里摘出并强制排到最后。
//! - **拖拽排序写回**（:287-290）：`currentKeys + [k for k in stored if k not in
//!   currentKeys]`——**过期 key 追加在尾部保留**，不丢弃。
//! - **上下箭头可用性**（:280-284）：首项禁上、末项禁下。
//!
//! 不复刻项：`LabelElideFilter`（文本省略，属绘制层）、`QPropertyAnimation`
//! 200ms 缓动（属动画层）、背景色 `13/170` 与分隔线 alpha `96/24`（属主题层）。

/// 页面固定分组 key（`CollapsibleSettingCardGroup(self.tr(...), key, ...)`
/// 的第二个实参，setting_page.py:62-69）。
pub const GROUP_GENERAL: &str = "general";
pub const GROUP_CATEGORY: &str = "category";
pub const GROUP_ASSOCIATION: &str = "association";
pub const GROUP_BROWSER: &str = "browser";
pub const GROUP_ARIA2RPC: &str = "aria2rpc";
pub const GROUP_PERSONALIZATION: &str = "personalization";
pub const GROUP_SOFTWARE: &str = "software";
pub const GROUP_ABOUT: &str = "about";

/// 固定分组创建顺序（`_initLayout` :389-401 的 addSettingGroup 顺序；
/// 功能包分组由 `_featureService.settingGroups` 插在 software 与 about 之间）。
pub const PAGE_GROUPS: [&str; 8] = [
    GROUP_GENERAL,
    GROUP_CATEGORY,
    GROUP_ASSOCIATION,
    GROUP_BROWSER,
    GROUP_ARIA2RPC,
    GROUP_PERSONALIZATION,
    GROUP_SOFTWARE,
    GROUP_ABOUT,
];

/// 分组标题（`.tr` 字面量）。
pub fn group_title(key: &str) -> &'static str {
    match key {
        GROUP_GENERAL => "综合下载设置",
        GROUP_CATEGORY => "下载分类",
        GROUP_ASSOCIATION => "关联设置",
        GROUP_BROWSER => "浏览器扩展",
        GROUP_ARIA2RPC => "Aria2 RPC 兼容",
        GROUP_PERSONALIZATION => "个性化",
        GROUP_SOFTWARE => "应用",
        GROUP_ABOUT => "关于",
        _ => "",
    }
}

/// 一张卡的搜索文本：`(标题, 副标题)`，均为 `.tr` 后的字面量。
/// 可折叠卡的标题/副标题取头部（见模块头第 4 条）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardText {
    pub title: String,
    pub content: String,
}

impl CardText {
    pub fn new(title: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            content: content.into(),
        }
    }
}

/// 一个分组及其卡片搜索文本（顺序即组内卡序）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupCards {
    pub key: String,
    pub cards: Vec<CardText>,
}

impl GroupCards {
    pub fn new(key: impl Into<String>, cards: Vec<CardText>) -> Self {
        Self {
            key: key.into(),
            cards,
        }
    }
}

/// 搜索后的分组状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupSearch {
    pub key: String,
    /// `hasMatch or not text`
    pub visible: bool,
    /// `not ((bool(text) and hasMatch) or key in expanded)`
    pub collapsed: bool,
    /// 逐卡可见性（`not text or isMatch`）
    pub card_visible: Vec<bool>,
}

/// `_collapsed` 的读侧：列表里没有 key 即折叠。
pub fn is_collapsed(key: &str, expanded: &[String]) -> bool {
    !expanded.iter().any(|item| item == key)
}

/// `_onExpandClicked` 的写侧：展开 append、折叠 remove，保持既有顺序。
pub fn toggle_expand(key: &str, expanded: &[String], now_expanded: bool) -> Vec<String> {
    let mut items = expanded.to_vec();
    if now_expanded {
        if !items.iter().any(|item| item == key) {
            items.push(key.to_string());
        }
    } else {
        items.retain(|item| item != key);
    }
    items
}

/// `_restoreOrder`：stored 中仍存在的 key 按 stored 顺序在前，其余按创建
/// 顺序在后，`about_key` 从「其余」摘出并强制排到最后。
pub fn restore_order(created: &[String], stored: &[String], about_key: &str) -> Vec<String> {
    let mut order: Vec<String> = stored
        .iter()
        .filter(|key| created.iter().any(|c| c == *key))
        .cloned()
        .collect();
    let mut rest: Vec<String> = created
        .iter()
        .filter(|key| !order.iter().any(|o| o == *key))
        .cloned()
        .collect();
    if let Some(index) = rest.iter().position(|key| key == about_key) {
        let about = rest.remove(index);
        rest.push(about);
    }
    order.extend(rest);
    order
}

/// `_saveOrder`：当前顺序在前，过期 key 追加在尾部保留。
pub fn save_order(current: &[String], stored: &[String]) -> Vec<String> {
    let mut items: Vec<String> = current.to_vec();
    for key in stored {
        if !current.iter().any(|item| item == key) {
            items.push(key.clone());
        }
    }
    items
}

/// `updateArrows`：`(上可点, 下可点)`。
pub fn update_arrows(index: usize, len: usize) -> (bool, bool) {
    (index > 0, index + 1 < len)
}

/// `setSearchText` 的归一化：`text.strip().lower()`。
/// Python `str.strip()` 与 `str::trim` 的空白集合一致（Unicode
/// `White_Space`），`str.lower()` 与 `to_lowercase()` 在土耳其语 `İ` 等
/// 少数点上有差异，设置页文案不含这些字符。
pub fn normalize_search(text: &str) -> String {
    text.trim().to_lowercase()
}

/// `isSearchMatch`：`needle in title.lower() or needle in content.lower()`。
pub fn matches_card(card: &CardText, needle: &str) -> bool {
    card.title.to_lowercase().contains(needle) || card.content.to_lowercase().contains(needle)
}

/// 单组 `setSearchText`。`needle` 须是 `normalize_search` 的结果。
pub fn search_group(group: &GroupCards, needle: &str, expanded: &[String]) -> GroupSearch {
    let empty = needle.is_empty();
    let card_visible: Vec<bool> = group
        .cards
        .iter()
        .map(|card| empty || matches_card(card, needle))
        .collect();
    let has_match = card_visible.iter().any(|visible| *visible);
    GroupSearch {
        key: group.key.clone(),
        visible: has_match || empty,
        collapsed: !((!empty && has_match) || !is_collapsed(&group.key, expanded)),
        card_visible,
    }
}

/// 页面级 `setSearchText`：返回逐组状态与 `hasMatch`（任一分组可见）。
pub fn search_all(
    groups: &[GroupCards],
    text: &str,
    expanded: &[String],
) -> (Vec<GroupSearch>, bool) {
    let needle = normalize_search(text);
    let results: Vec<GroupSearch> = groups
        .iter()
        .map(|group| search_group(group, &needle, expanded))
        .collect();
    let has_match = results.iter().any(|result| result.visible);
    (results, has_match)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn group_titles_match_python_literals() {
        assert_eq!(group_title(GROUP_GENERAL), "综合下载设置");
        assert_eq!(group_title(GROUP_CATEGORY), "下载分类");
        assert_eq!(group_title(GROUP_ASSOCIATION), "关联设置");
        assert_eq!(group_title(GROUP_BROWSER), "浏览器扩展");
        assert_eq!(group_title(GROUP_ARIA2RPC), "Aria2 RPC 兼容");
        assert_eq!(group_title(GROUP_PERSONALIZATION), "个性化");
        assert_eq!(group_title(GROUP_SOFTWARE), "应用");
        assert_eq!(group_title(GROUP_ABOUT), "关于");
        // 未知 key（功能包自定义分组）不编造标题。
        assert_eq!(group_title("pack_bili"), "");
    }

    #[test]
    fn collapse_state_reads_expanded_list() {
        // 缺省 [] → 全部折叠（cfg.py:322 StringList 缺省空列表）。
        assert!(is_collapsed(GROUP_GENERAL, &[]));
        assert!(!is_collapsed(GROUP_GENERAL, &keys(&[GROUP_GENERAL])));
    }

    #[test]
    fn toggle_expand_keeps_existing_order() {
        let expanded = keys(&[GROUP_BROWSER, GROUP_GENERAL]);
        // 展开新 key → 追加在尾部。
        assert_eq!(
            toggle_expand(GROUP_ABOUT, &expanded, true),
            keys(&[GROUP_BROWSER, GROUP_GENERAL, GROUP_ABOUT])
        );
        // 已在列表再展开 → 不重复。
        assert_eq!(
            toggle_expand(GROUP_BROWSER, &expanded, true),
            keys(&[GROUP_BROWSER, GROUP_GENERAL])
        );
        // 折叠 → 移除，其余顺序不变。
        assert_eq!(
            toggle_expand(GROUP_BROWSER, &expanded, false),
            keys(&[GROUP_GENERAL])
        );
        // 折叠不在列表的 key → 无变化。
        assert_eq!(
            toggle_expand(GROUP_ABOUT, &expanded, false),
            keys(&[GROUP_BROWSER, GROUP_GENERAL])
        );
    }

    #[test]
    fn restore_order_puts_stored_first_and_about_last() {
        let created = keys(&[
            GROUP_GENERAL,
            GROUP_CATEGORY,
            GROUP_ASSOCIATION,
            GROUP_BROWSER,
            GROUP_ARIA2RPC,
            GROUP_PERSONALIZATION,
            GROUP_SOFTWARE,
            GROUP_ABOUT,
        ]);
        // 无存储顺序 → 创建顺序原样（about 本就在末尾）。
        assert_eq!(restore_order(&created, &[], GROUP_ABOUT), created);
        // stored 中存在的 key 提到前面，其余按创建顺序，about 强制最后。
        assert_eq!(
            restore_order(&created, &keys(&[GROUP_SOFTWARE]), GROUP_ABOUT),
            keys(&[
                GROUP_SOFTWARE,
                GROUP_GENERAL,
                GROUP_CATEGORY,
                GROUP_ASSOCIATION,
                GROUP_BROWSER,
                GROUP_ARIA2RPC,
                GROUP_PERSONALIZATION,
                GROUP_ABOUT,
            ])
        );
        // stored 里已不存在的 key 被过滤；多个 key 保持 stored 顺序。
        assert_eq!(
            restore_order(
                &created,
                &keys(&["gone", GROUP_ABOUT, GROUP_SOFTWARE]),
                GROUP_ABOUT
            ),
            keys(&[
                GROUP_ABOUT,
                GROUP_SOFTWARE,
                GROUP_GENERAL,
                GROUP_CATEGORY,
                GROUP_ASSOCIATION,
                GROUP_BROWSER,
                GROUP_ARIA2RPC,
                GROUP_PERSONALIZATION,
            ])
        );
    }

    #[test]
    fn restore_order_keeps_pack_groups_before_about() {
        // 功能包分组在 created 里排在 software 之后、about 之前（Python 侧
        // `_featureService.settingGroups` 插在两者之间），未进 stored → 落在「其余」。
        let created = keys(&[GROUP_GENERAL, GROUP_SOFTWARE, "pack_bili", GROUP_ABOUT]);
        assert_eq!(
            restore_order(&created, &keys(&[GROUP_GENERAL]), GROUP_ABOUT),
            keys(&[GROUP_GENERAL, GROUP_SOFTWARE, "pack_bili", GROUP_ABOUT])
        );
    }

    #[test]
    fn save_order_appends_stale_keys() {
        assert_eq!(
            save_order(
                &keys(&[GROUP_CATEGORY, GROUP_GENERAL]),
                &keys(&[GROUP_GENERAL, "gone", GROUP_CATEGORY])
            ),
            keys(&[GROUP_CATEGORY, GROUP_GENERAL, "gone"])
        );
        assert_eq!(
            save_order(&keys(&[GROUP_GENERAL]), &[]),
            keys(&[GROUP_GENERAL])
        );
    }

    #[test]
    fn arrows_disable_at_edges() {
        assert_eq!(update_arrows(0, 3), (false, true));
        assert_eq!(update_arrows(1, 3), (true, true));
        assert_eq!(update_arrows(2, 3), (true, false));
        assert_eq!(update_arrows(0, 1), (false, false));
    }

    fn sample_groups() -> Vec<GroupCards> {
        vec![
            GroupCards::new(
                GROUP_GENERAL,
                vec![
                    CardText::new("下载路径", "文件默认保存位置"),
                    CardText::new("代理", "设置下载时希望使用的代理"),
                    CardText::new("下载时验证 SSL 证书", "文件无法下载时，可尝试关闭该选项"),
                ],
            ),
            GroupCards::new(
                GROUP_SOFTWARE,
                vec![
                    CardText::new("开机启动", "在系统启动时静默运行 Ghost Downloader"),
                    CardText::new(
                        "剪贴板监听",
                        "剪贴板监听器将自动检测剪贴板中的链接并添加下载任务",
                    ),
                ],
            ),
            GroupCards::new(GROUP_ABOUT, vec![CardText::new("关于", "Version")]),
        ]
    }

    #[test]
    fn search_filters_cards_and_groups() {
        let groups = sample_groups();
        let (results, has_match) = search_all(&groups, "代理", &[]);
        assert!(has_match);
        assert_eq!(results[0].key, GROUP_GENERAL);
        assert!(results[0].visible);
        // 只有「代理」卡命中；同组的 SSL 卡不含「代理」但副标题含「代理」？不含。
        assert_eq!(results[0].card_visible, vec![false, true, false]);
        // 命中即展开，即使不在 expandedSettingGroups 里。
        assert!(!results[0].collapsed);
        // 未命中分组：隐藏；折叠态沿用 cfg（空列表 → 折叠）。
        assert!(!results[1].visible);
        assert!(results[1].collapsed);
        assert_eq!(results[1].card_visible, vec![false, false]);
    }

    #[test]
    fn search_is_case_insensitive_and_trims() {
        let groups = sample_groups();
        let (results, _) = search_all(&groups, "  ssl  ", &[]);
        assert_eq!(results[0].card_visible, vec![false, false, true]);
        // 副标题命中（「Ghost Downloader」→ 小写 needle）：software 组只有
        // 「开机启动」卡副标题含该串，「剪贴板监听」卡不含。
        let (results, _) = search_all(&groups, "ghost downloader", &[]);
        assert_eq!(results[1].card_visible, vec![true, false]);
        assert!(results[1].visible);
        // 不命中的分组（about）隐藏。
        assert!(!results[2].visible);
        // 纯空白 = 无搜索词 → 全部卡片可见、分组全可见。
        let (results, has_match) = search_all(&groups, "   ", &[]);
        assert!(has_match);
        assert!(results.iter().all(|result| result.visible));
        assert!(
            results
                .iter()
                .all(|result| result.card_visible.iter().all(|v| *v))
        );
    }

    #[test]
    fn search_does_not_write_expanded_but_keeps_cfg_expansion() {
        let groups = sample_groups();
        // 未命中的分组若在 expandedSettingGroups 里，仍保持展开（只是隐藏）。
        let (results, _) = search_all(&groups, "ssl", &keys(&[GROUP_SOFTWARE]));
        assert!(results[0].visible); // general 命中
        assert!(!results[0].collapsed); // 搜索命中即展开，不写 cfg
        assert!(!results[1].visible); // software 未命中 → 隐藏
        assert!(!results[1].collapsed); // 展开态来自 cfg，不是搜索
        assert!(!results[2].visible); // about 未命中
        assert!(results[2].collapsed); // 未命中且不在 cfg → 折叠
        // 清空搜索后回到 cfg 口径：general 展开、software 折叠。
        let (results, _) = search_all(&groups, "", &keys(&[GROUP_GENERAL]));
        assert!(!results[0].collapsed);
        assert!(results[1].collapsed);
    }

    #[test]
    fn search_reports_no_match_for_page_empty_state() {
        let groups = sample_groups();
        let (results, has_match) = search_all(&groups, "不存在的设置项", &[]);
        assert!(!has_match);
        assert!(results.iter().all(|result| !result.visible));
    }
}
