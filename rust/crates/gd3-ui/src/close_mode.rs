//! 关闭主窗口模式（第 38 步）：镜像 OptionsConfigItem
//! `Software/CloseMode`（cfg.py:309-312，缺省 CloseMode.ASK）与
//! ComboBoxSettingCard 的索引 ⇄ 字面量序（qfluentwidgets
//! setting_card.py:401-448 实证：索引 i ⇄ options[i]，Change 存
//! `EnumSerializer` 序列化字面量——cfg.py:65-69 实证 CloseMode
//! 枚举值即配置字面量 "Ask"/"Background"/"Quit"）。未知字面量/
//! 越界索引 Rust 镜像层兜底不复刻 Python KeyError 崩溃（同
//! conflict.rs/proxy.rs 惯例）。
//!
//! ```text
//! UI 文案（.tr 字面量）：["关闭时询问", "继续在后台运行", "退出程序"]
//! 配置字面量：["Ask", "Background", "Quit"]，缺省 "Ask"
//! ```

pub const OPTIONS: [&str; 3] = ["Ask", "Background", "Quit"];
pub const DEFAULT_OPTION: &str = "Ask";

/// 配置字面量 → UI 索引（未知字面量兜底 0=Ask）。
pub fn index_of_option(option: &str) -> usize {
    OPTIONS
        .iter()
        .position(|candidate| *candidate == option)
        .unwrap_or(0)
}

/// UI 索引 → 配置字面量（越界兜底 Ask）。
pub fn option_of_index(index: i64) -> &'static str {
    OPTIONS
        .get(index.max(0) as usize)
        .copied()
        .unwrap_or(DEFAULT_OPTION)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_mode_index_round_trip_and_fallback() {
        for (index, option) in OPTIONS.iter().enumerate() {
            assert_eq!(index_of_option(option), index);
            assert_eq!(option_of_index(index as i64), *option);
        }
        // 未知字面量/越界索引兜底，不复刻 Python KeyError 崩溃。
        assert_eq!(index_of_option("Weird"), 0);
        assert_eq!(option_of_index(7), "Ask");
        assert_eq!(option_of_index(-1), "Ask");
        assert_eq!(option_of_index(1), "Background");
        assert_eq!(option_of_index(2), "Quit");
    }
}
