use std::path::PathBuf;

fn main() {
    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));

    // 主窗口（`include_modules!()` 走 `SLINT_INCLUDE_GENERATED`）。
    slint_build::compile(manifest_dir.join("ui/appwindow.slint")).expect("slint 编译失败");

    // 托盘（第 136 步）：`SystemTrayIcon` 必须是独立顶层组件，不能塞进
    // `AppWindow` 里，所以单独编译一份，产物由 `src/tray.rs` 显式 `include!`。
    // `compile_with_output_path` 不打印 cargo 指令，`rerun-if-changed` 得自己给。
    let tray_slint = manifest_dir.join("ui/system_tray.slint");
    println!("cargo:rerun-if-changed={}", tray_slint.display());
    slint_build::compile_with_output_path(
        tray_slint,
        out_dir.join("system_tray.rs"),
        slint_build::CompilerConfiguration::new(),
    )
    .expect("slint 托盘编译失败");
}
