//! 真网络的更新链路冒烟（默认 `#[ignore]`，需要外网）。
//!
//! 走的是生产路径：`WreqTransport` + `app_repo()` + `main`，抓真实的
//! `versions.json`、下真实的 pack 资产、算真实 sha256。离线环境跑不了，所以标
//! `#[ignore]`；手工验证用：
//!
//! ```bash
//! source /root/gd3-env.sh
//! cd rust
//! timeout 600 cargo test -p gd3-ui --test update_exec_real -- --ignored --nocapture
//! ```
//!
//! 断言口径：
//! - `check()` 把 app 判成 AVAILABLE（本仓 `VERSION` 是 4.4.0，线上 ≥ 4.4.3）；
//! - 本地装一个旧版 `bittorrent_pack`（1.0.0）后，远程 1.1.1 被判成 AVAILABLE；
//! - `download()` 把真 zip 落到 staging、sha256 对得上、状态变 READY；
//! - `apply()` 把 zip 解到 `features/bittorrent_pack_pending/`（`manifest.toml` 在位）。

use std::fs;

use gd3_ui::update_exec::UpdateExecutor;
use gd3_ui::update_service::{Host, UpdateRuntime, UpdateState};

fn temp_host(tag: &str) -> (Host, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("gd3-ui-update-real-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("建临时目录");
    let host = Host {
        app_version: "4.4.0".to_string(),
        os: "linux".to_string(),
        machine: "x86_64".to_string(),
        is_android: false,
        staging_dir: dir.join("update_staging"),
        features_dir: dir.join("features"),
        executable_dir: dir.join("bin"),
        pid: std::process::id(),
        interpreter: dir.join("python"),
    };
    (host, dir)
}

/// 本地装一个旧版 pack（只有 manifest 与入口文件，内容无所谓）。
fn install_local_pack(host: &Host, name: &str, version: &str) {
    let dir = host.features_dir.join(name);
    fs::create_dir_all(&dir).expect("建 pack 目录");
    fs::write(
        dir.join("manifest.toml"),
        format!(
            "[pack]\nentry = \"pack.py\"\nclass = \"{name}Pack\"\nversion = \"{version}\"\ngdMinVersion = \"4.4.0\"\n"
        ),
    )
    .expect("写 manifest.toml");
    fs::write(dir.join("pack.py"), "# installed\n").expect("写入口");
}

#[tokio::test]
#[ignore = "需要外网：真抓 versions.json 与 release 资产"]
async fn a_real_check_and_pack_download_against_the_production_sources() {
    let (host, dir) = temp_host("pack");
    install_local_pack(&host, "bittorrent_pack", "1.0.0");
    let executor = UpdateExecutor::with_wreq("chrome", &host);
    let mut runtime = UpdateRuntime::new(host.clone());

    let checked = executor.check(&mut runtime).await;
    println!("check: {:?}", checked.steps.len());
    for step in &checked.steps {
        println!("  {} -> {:?}", step.target_id, step.state);
    }
    assert_eq!(
        checked.last_state("app"),
        Some(UpdateState::Available),
        "线上版本应当高于 4.4.0"
    );
    assert_eq!(
        checked.last_state("bittorrent_pack"),
        Some(UpdateState::Available),
        "本地 1.0.0 < 线上版本"
    );

    let downloaded = executor.download(&mut runtime, "bittorrent_pack").await;
    for step in &downloaded.steps {
        println!(
            "  {} -> {:?} progress={:?}",
            step.target_id, step.state, step.fields.progress
        );
    }
    assert_eq!(
        downloaded.last_state("bittorrent_pack"),
        Some(UpdateState::Ready),
        "下载 + sha256 校验应当通过"
    );
    let archive = host.staging_dir.join("bittorrent_pack.zip");
    let size = fs::metadata(&archive).expect("staging 里有 zip").len();
    println!("downloaded {} bytes", size);
    assert!(size > 1024, "zip 太小：{size} 字节");

    let applied = executor.apply(&mut runtime).await;
    println!(
        "apply: spawns={} errors={:?}",
        applied.spawns.len(),
        applied.errors
    );
    assert_eq!(applied.errors, Vec::<String>::new());
    let pending = host.features_dir.join("bittorrent_pack_pending");
    assert!(
        pending.join("manifest.toml").is_file(),
        "解包后应当有 manifest.toml：{}",
        pending.display()
    );
    let mut names: Vec<String> = fs::read_dir(&pending)
        .expect("读 pending 目录")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    println!("pending 内容: {names:?}");
    assert!(!archive.exists(), "`_applyPack` 会删掉 zip");

    fs::remove_dir_all(&dir).expect("清临时目录");
}
