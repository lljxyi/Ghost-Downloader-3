// Ghost-Downloader-3 Slint UI 的 Rust 侧入口。
//
// Phase 7a：只装最小窗口骨架 + 无头实例化冒烟（cargo test 可在
// 无显示器的构建机上跑，走 software renderer + 自制 Platform）。
// 逐页从 Qt 迁过来时在 ui/ 增加组件、在本 crate 挂回调。
//
// slint 1.18.1 API 实证锚点（registry i-slint-core-1.18.1）：
// - platform.rs: `set_platform(platform: Box<dyn Platform + 'static>) -> Result<(), SetPlatformError>`
//   （1.18 没有 `is_using_platform_set`；`SetPlatformError::AlreadySet` 即「已装好」信号）
// - Platform 唯一必需方法：`create_window_adapter -> Result<Rc<dyn WindowAdapter>, PlatformError>`
// - MinimalSoftwareWindow::new(repaint_buffer_type: RepaintBufferType) -> Rc<Self>（自身已包 Rc）

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PlatformError, WindowAdapter};
use std::rc::Rc;

slint::include_modules!();

/// 无头平台：不依赖任何显示服务（测试/裸构建机都跑得起来）。
struct HeadlessPlatform;

impl Platform for HeadlessPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
    }
}

/// 无头实例化 AppWindow（进程内 set_platform 只成功一次，重复=AlreadySet）。
pub fn create_headless() -> AppWindow {
    // AlreadySet 说明平台已装好（比如同进程第二个实例），直接复用即可。
    let _ = slint::platform::set_platform(Box::new(HeadlessPlatform));
    let window = AppWindow::new().expect("AppWindow 实例化失败");
    window.show().expect("show 失败");
    window
}

#[cfg(test)]
mod smoke {
    /// 无头实例化：组件树的属性可读写、状态文本可镜像给 Rust 侧。
    #[test]
    fn appwindow_instantiates_headless_and_rust_side_sets_status() {
        let window = crate::create_headless();
        assert_eq!(window.get_statusText(), "Ghost-Downloader-3 (Slint)");
        window.set_statusText("headless smoke".into());
        assert_eq!(window.get_statusText(), "headless smoke");
    }
}
