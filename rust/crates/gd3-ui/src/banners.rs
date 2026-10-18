//! 组件库（P4，第 69 步）：镜像 `app/view/components/banners.py`（23 行）
//! 的 [`WarningBanner`] 绘制口径。
//!
//! ## 实证口径（2026-10-18 实读源码）
//!
//! `paintEvent` 只画一个抗锯齿圆角矩形：
//! `radius` 由构造函数默认 `4`；深色主题 `QColor(67, 53, 25)`（琥珀暗底），
//! 浅色主题 `QColor(255, 244, 206)`（琥珀亮底）；无边框（`setPen(NoPen)`）。
//! 填充区域是 `self.contentsRect()`（即去掉布局边距后的区域）。

/// `def __init__(self, parent=None, radius: float = 4)`（banners.py:10）
pub const DEFAULT_RADIUS: f64 = 4.0;

/// `isDarkTheme()` 两支的 `QColor` 分量（`(r, g, b)`）。
pub fn background(dark: bool) -> (u8, u8, u8) {
    if dark { (67, 53, 25) } else { (255, 244, 206) }
}

/// 同上，Slint 侧直接吃 `#RRGGBB`。
pub fn background_hex(dark: bool) -> &'static str {
    if dark { "#433519" } else { "#FFF4CE" }
}

/// `drawRoundedRect(QRectF(self.contentsRect()), self._radius, self._radius)`
/// 的圆角半径（`radius` 是构造参数，默认 [`DEFAULT_RADIUS`]）。
pub fn corner_radius(radius: Option<f64>) -> f64 {
    radius.unwrap_or(DEFAULT_RADIUS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_match_python_qcolor_components() {
        assert_eq!(background(true), (67, 53, 25));
        assert_eq!(background(false), (255, 244, 206));
        assert_eq!(background_hex(true), "#433519");
        assert_eq!(background_hex(false), "#FFF4CE");
    }

    #[test]
    fn radius_defaults_to_four() {
        assert_eq!(DEFAULT_RADIUS, 4.0);
        assert_eq!(corner_radius(None), 4.0);
        assert_eq!(corner_radius(Some(8.0)), 8.0);
        assert_eq!(corner_radius(Some(0.0)), 0.0);
    }
}
