//! 同名文件冲突选项（Phase 7c 第 34 步）——镜像
//! cfg.py `nameConflict = OptionsConfigItem("GeneralDownload",
//! "NameConflict", "keepBoth", OptionsValidator([...]))` 与
//! setting_card.py `ComboBoxSettingCard` 的索引映射（协议不变：磁盘
//! 存选项字面量字符串）。

/// ComboBox 选项顺序 = ComboBoxSettingCard addItem 的 zip(texts,
/// options) 顺序（texts=setting_page.py .tr：保留两者/覆盖/询问）。
pub const OPTIONS: [&str; 3] = ["keepBoth", "overwrite", "ask"];

/// 配置键缺省（cfg.py 实证）。
pub const DEFAULT_OPTION: &str = "keepBoth";

/// 选项字面量 → 索引：镜像 `setCurrentText(optionToText[value])`。
/// 未知字面量容错回 0（Python 端 KeyError 崩溃是缺陷面，Rust 镜像层
/// 不复刻崩溃，容错语义同 filter.rs from_route 惯例）。
pub fn index_of_option(option: &str) -> i64 {
    OPTIONS.iter().position(|o| *o == option).unwrap_or(0) as i64
}

/// 索引 → 选项字面量：镜像 `qconfig.set(item, itemData(index))`。
/// 越界容错回缺省 keepBoth（Python 端 itemData 越界是 None，UI 层
/// 不发越界索引；Rust 镜像层兜底不猜崩）。
pub fn option_of_index(index: i64) -> &'static str {
    OPTIONS
        .get(index as usize)
        .copied()
        .unwrap_or(DEFAULT_OPTION)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn option_index_mapping_mirrors_combobox_order() {
        assert_eq!(index_of_option("keepBoth"), 0);
        assert_eq!(index_of_option("overwrite"), 1);
        assert_eq!(index_of_option("ask"), 2);
        assert!(OPTIONS.iter().all(|o| index_of_option(o) >= 0));
        for (i, option) in OPTIONS.iter().enumerate() {
            assert_eq!(option_of_index(i as i64), *option);
        }
        // 容错语义：未知字面量→0；越界索引→keepBoth。
        assert_eq!(index_of_option("weird"), 0);
        assert_eq!(option_of_index(-1), DEFAULT_OPTION);
        assert_eq!(option_of_index(9), DEFAULT_OPTION);
    }
}
