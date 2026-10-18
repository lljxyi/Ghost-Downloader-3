//! `gd3-ui` 二进制入口。
//!
//! 只做「argv → `boot::main_entry`」这一层；启动序列全部在 `gd3_ui::boot` 里，
//! 这样单元测试能直接覆盖（不需要 spawn 子进程）。
//!
//! （`src/main.rs` 与 `src/lib.rs` 并存时 cargo 自动生成同名 bin `gd3-ui`，
//! 所以 `Cargo.toml` 不需要显式 `[[bin]]`。）

fn main() -> std::process::ExitCode {
    gd3_ui::boot::main_entry(std::env::args().skip(1).collect())
}
