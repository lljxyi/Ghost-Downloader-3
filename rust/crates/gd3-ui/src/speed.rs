//! 限速值换算（Phase 7c 第 33 步）——镜像
//! app/view/components/setting_cards.py `SpinBoxSettingCard`
//! division=1/1024 的双向语义（协议不变：UserConfig 存字节）。

/// 配置键（cfg.py:247-249 缺省与范围）。
pub const DEFAULT_BYTES: i64 = 4_194_304;
pub const CONFIG_MIN_BYTES: i64 = 1_024;
pub const CONFIG_MAX_BYTES: i64 = 104_857_600;

/// SpinBox 显示口径（setting_cards.py:69 int(r * division)）。
pub const DISPLAY_MIN_KB: i64 = 1;
pub const DISPLAY_MAX_KB: i64 = 102_400;
/// singleStep 在显示值上（setting_cards.py:64/89，displayed KB/s 步进）。
pub const DISPLAY_STEP_KB: i64 = 512;

/// 字节 → 显示 KB/s：Python `int(value * 1/1024)`（正数即地板整除）。
/// 注意配置范围下限 1024 保证显示 ≥1；0 语义同 Python 镜像（512→0），
/// 不在此层做 clamp——范围约束归 SpinBox/RangeValidator。
pub fn to_display_kb(bytes: i64) -> i64 {
    bytes / 1024
}

/// 显示 KB/s → 字节：Python `int(value / division)` = int(kb * 1024)，
/// 整数精确无损。
pub fn from_display_kb(kb: i64) -> i64 {
    kb * 1024
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_mirrors_python_division_1_1024() {
        // 缺省 4194304 → 4096 KB/s（SpinBox 缺省显示值）。
        assert_eq!(to_display_kb(DEFAULT_BYTES), 4096);
        // 范围两端 round-trip（int(r*division) 截断=精确值）。
        assert_eq!(to_display_kb(CONFIG_MIN_BYTES), 1);
        assert_eq!(to_display_kb(CONFIG_MAX_BYTES), DISPLAY_MAX_KB);
        assert_eq!(
            from_display_kb(to_display_kb(CONFIG_MIN_BYTES)),
            CONFIG_MIN_BYTES
        );
        assert_eq!(
            from_display_kb(to_display_kb(CONFIG_MAX_BYTES)),
            CONFIG_MAX_BYTES
        );
        assert_eq!(from_display_kb(4096), DEFAULT_BYTES);
        // 镜像事实：范围外小值不 clamp（512→0），约束归 SpinBox。
        assert_eq!(to_display_kb(512), 0);
    }
}
