//! TLS 指纹/UA 身份选项（Phase 7c 第 41 步）——镜像
//! cfg.py `clientProfile = ConfigItem("Network", "ClientProfile",
//! "auto", ClientProfileValidator())` 与 IdentitySettingCard 内
//! `buildProfileMenu(includeAuto=True)` 菜单顺序（协议不变：磁盘
//! 存 profile 字面量字符串，引擎由 gd3-net `resolve_emulation`
//! 解析成 wreq-util 身份）。

/// 菜单结构与 Python `buildProfileMenu` 一致：auto、raw、分隔、
/// 逐家族「最新」（`profileFamilies()` 实测顺序 = chrome/edge/
/// firefox/firefox-android/opera/safari/safari-ios/safari-ipad/
/// okhttp；家族字面量即"最新"选项值，特定版本选择留欠账步）。
pub const FAMILY_LABELS: [(&str, &str); 9] = [
    ("chrome", "Chrome"),
    ("edge", "Edge"),
    ("firefox", "Firefox"),
    ("firefox-android", "Firefox Android"),
    ("opera", "Opera"),
    ("safari", "Safari"),
    ("safari-ios", "Safari iOS"),
    ("safari-ipad", "Safari iPad"),
    ("okhttp", "OkHttp"),
];

/// 配置键缺省（cfg.py 实证，ClientProfileValidator.correct 也兜 "auto"）。
pub const DEFAULT_PROFILE: &str = "auto";

/// ComboBox 选项显示串（镜像 `toProfileLabel`：auto→「自动（匹配
/// 来源）」、raw→「不模拟（原样发送）」、家族→「家族（最新）」）。
pub const COMBO_ITEMS: [&str; 11] = [
    "自动（匹配来源）",
    "不模拟（原样发送）",
    "Chrome（最新）",
    "Edge（最新）",
    "Firefox（最新）",
    "Firefox Android（最新）",
    "Opera（最新）",
    "Safari（最新）",
    "Safari iOS（最新）",
    "Safari iPad（最新）",
    "OkHttp（最新）",
];

/// profile 字面量 → 索引：空串与 auto 归 0（"" 经
/// toProfileLabel 同显「自动（匹配来源）」）。
pub fn index_of(value: &str) -> i64 {
    match value {
        "" | "auto" => 0,
        "raw" => 1,
        other => FAMILY_LABELS
            .iter()
            .position(|(name, _)| *name == other)
            .map_or(0, |p| (p + 2) as i64),
    }
}

/// profile 字面量 → 显示串（option_cards.py:23-34 `toProfileLabel`
/// 全分支镜像）：auto/raw/家族「最新」；特定版本字面量（如
/// chrome120）按尾部数字+下划线段拆「chrome 120」（"." 替换 "_"）。
/// 第 43 步身份预设行描述复用。
pub fn label_of(value: &str) -> String {
    match value {
        "" | "auto" => COMBO_ITEMS[0].into(),
        "raw" => COMBO_ITEMS[1].into(),
        other => FAMILY_LABELS
            .iter()
            .find(|(name, _)| *name == other)
            .map_or_else(
                || {
                    let head = other.trim_end_matches(|c: char| c.is_ascii_digit() || c == '_');
                    let version = other[head.len()..].replace('_', ".");
                    if version.is_empty() {
                        other.into()
                    } else {
                        format!("{head} {version}")
                    }
                },
                |(_, label)| format!("{label}（最新）"),
            ),
    }
}

/// 索引 → profile 字面量：镜像 `_onPick`（auto/raw/家族字面量）。
/// 越界兜底 auto（与 cfg.py ClientProfileValidator.correct 一致，
/// 不复刻 UI 越界崩溃面）。
pub fn value_of(index: i64) -> &'static str {
    match index {
        0 => DEFAULT_PROFILE,
        1 => "raw",
        i => FAMILY_LABELS
            .get((i - 2) as usize)
            .map_or(DEFAULT_PROFILE, |(name, _)| *name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_index_mapping_mirrors_menu_order() {
        assert_eq!(index_of("auto"), 0);
        assert_eq!(index_of(""), 0);
        assert_eq!(index_of("raw"), 1);
        // 家族字面量即「最新」选项，顺序 = profileFamilies() 实测。
        assert_eq!(index_of("chrome"), 2);
        assert_eq!(index_of("edge"), 3);
        assert_eq!(index_of("firefox-android"), 5);
        assert_eq!(index_of("okhttp"), 10);
        // 容错语义：未知字面量→0（correct 兜 auto）。
        assert_eq!(index_of("weird"), 0);
        assert_eq!(value_of(0), "auto");
        assert_eq!(value_of(1), "raw");
        assert_eq!(value_of(3), "edge");
        assert_eq!(value_of(10), "okhttp");
        assert_eq!(value_of(-1), "auto");
        assert_eq!(value_of(11), "auto");
        // 第 43 步：toProfileLabel 显示串全分支（含特定版本拆分）。
        assert_eq!(label_of("auto"), COMBO_ITEMS[0]);
        assert_eq!(label_of(""), COMBO_ITEMS[0]);
        assert_eq!(label_of("raw"), COMBO_ITEMS[1]);
        assert_eq!(label_of("edge"), "Edge（最新）");
        assert_eq!(label_of("chrome120"), "chrome 120");
        assert_eq!(label_of("chrome120_1"), "chrome 120.1");
        assert_eq!(label_of("weird"), "weird");
        // 全表 round-trip：索引→字面量→索引恒等。
        for i in 0..COMBO_ITEMS.len() as i64 {
            assert_eq!(index_of(value_of(i)), i);
        }
    }
}
