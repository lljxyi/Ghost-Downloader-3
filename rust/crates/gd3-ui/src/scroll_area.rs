//! 组件库（P4，第 69 步）：镜像 `app/view/components/scroll_area.py`（15 行）。
//!
//! ## 实证口径（2026-10-18 实读源码）
//!
//! 这个文件**按平台换基类**：
//!
//! ```text
//! if sys.platform == "darwin":  from qfluentwidgets import ScrollArea
//! else:                         class ScrollArea(SmoothScrollArea)  # + 自定义透明背景
//! ```
//!
//! 也就是说 macOS 上直接用 qfluentwidgets 自带的 `ScrollArea`（它有自己的
//! `enableTransparentBackground`），其余平台用 `SmoothScrollArea` 子类，并
//! **重写** `enableTransparentBackground` 去设两份样式表：
//! `QScrollArea{border: none; background: transparent}` 与 viewport 的
//! `background: transparent`。
//!
//! 本模块只保留「哪套样式」这个可断言事实，样式字符串逐字节照抄。

/// `self.setStyleSheet("QScrollArea{border: none; background: transparent}")`
pub const TRANSPARENT_SCROLL_AREA_STYLE: &str =
    "QScrollArea{border: none; background: transparent}";
/// `self.viewport().setStyleSheet("background: transparent")`
pub const TRANSPARENT_VIEWPORT_STYLE: &str = "background: transparent";

/// 平台分支选的基类名（macOS 走 qfluentwidgets 原生实现，其余走自定义子类）。
pub fn base_class(macos: bool) -> &'static str {
    if macos {
        "ScrollArea"
    } else {
        "SmoothScrollArea"
    }
}

/// `enableTransparentBackground()` 的两份样式表；macOS 上由基类自己处理，
/// 自定义实现不参与（返回 `None`）。
pub fn transparent_styles(macos: bool) -> Option<(&'static str, &'static str)> {
    if macos {
        None
    } else {
        Some((TRANSPARENT_SCROLL_AREA_STYLE, TRANSPARENT_VIEWPORT_STYLE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_are_byte_exact() {
        assert_eq!(
            TRANSPARENT_SCROLL_AREA_STYLE,
            "QScrollArea{border: none; background: transparent}"
        );
        assert_eq!(TRANSPARENT_VIEWPORT_STYLE, "background: transparent");
    }

    #[test]
    fn platform_picks_base_class_and_style_ownership() {
        assert_eq!(base_class(true), "ScrollArea");
        assert_eq!(base_class(false), "SmoothScrollArea");
        assert_eq!(transparent_styles(true), None);
        assert_eq!(
            transparent_styles(false),
            Some((
                "QScrollArea{border: none; background: transparent}",
                "background: transparent"
            ))
        );
    }
}
