//! `update_service::UpdateRuntime` 的 **tokio 宿主适配**：把状态机吐出来的
//! [`UpdateEffect`] 真的执行掉（抓 `versions.json` / 下 release asset / 下 raw 文件 /
//! 算 sha256 / 解 zip / 删文件），再把结果喂回状态机，直到没有新动作。
//!
//! | Python（`app/services/update_service.py`） | 这里 |
//! |---|---|
//! | `_fetchVersions`（`:184-190`）→ `fetchJson` | [`UpdateExecutor::check`] 处理 [`UpdateEffect::FetchVersions`] |
//! | `_downloadApp` / `_downloadPack` 里的 `fetchReleaseAsset` / `fetchRawFile` | [`UpdateEffect::FetchAsset`] / [`UpdateEffect::FetchRaw`] |
//! | `toChecksum(path, "sha256", …)` | [`UpdateEffect::Checksum`]（比较在状态机里做） |
//! | `extractZip` / `extractTarXz` / `extractDmg` | [`UpdateEffect::Unpack`] |
//! | `STAGING_DIR.mkdir` / `shutil.rmtree` / `unlink(missing_ok=True)` | `Mkdir` / `RemoveDir` / `RemoveFile` |
//! | `_startUpdater` 的 `subprocess.Popen` | **只记录**（见下） |
//!
//! ## 结构决策
//!
//! - **效果串成队列、一条条 await**：状态机是同步的，Python 那边 `_download` 也是
//!   `await` 串行。这里用一个 `VecDeque` 把「状态机新吐的动作」接着跑，直到队列空。
//!   所以 [`UpdateExecutor::run`] 的入参只有「第一批动作」，后续全靠反馈。
//! - **进度回调先攒后放**：`fetchReleaseAsset` 的 `onProgress` 是同步闭包，而
//!   `download_progress` 要 `&mut UpdateRuntime`——两者不能同时借用。所以回调先把
//!   百分比塞进 `Mutex<Vec<f64>>`，下载返回后再按原顺序回放给状态机。UI 看到的
//!   顺序与 Python 一致，只是推送时刻集中在下载结束那一刻。
//! - **`Transport` 注入**：真实现是 `WreqTransport`，测试用假实现。没有这层，
//!   「探测→下载→校验→解包→READY」这条链路只能靠真网络验证。
//! - **`Unpack` 复用 `extension_crx` 的 ZIP 读取器**：`entries` / `read_entry` /
//!   `sanitize` 已经在第 121 步按 `zipfile` 的口径验过（deflate + 路径净化 + 中央目录序），
//!   没必要再写第二份。
//!
//! ## 与 Python 的已知差异
//!
//! - **`tar.xz` / `dmg` 解包未接**：离线环境没有 `xz2` 依赖；`dmg` 需要 macOS 的
//!   `hdiutil`。两者都返回 [`TaskError::io`]，状态机照常落 `FAILED`（Python 会成功）。
//! - **`StartUpdater` 只记录不 spawn**：updater 会顶掉当前进程，什么时候拉起应由 UI 层
//!   在用户确认后决定。这里把 [`UpdaterCommand`] 放进 [`UpdateRun::spawns`] 交出去。
//! - **`_applyPack` 的 `pendingDir.mkdir(parents=True, exist_ok=True)`** 由解包里的
//!   `create_dir_all` 覆盖，行为等价。
//! - **效果预算**：队列跑满 [`MAX_EFFECTS`] 条仍未收敛就中止并记进
//!   [`UpdateRun::errors`]（Python 没有这个保护，但链路上不存在环，正常跑不到）。

use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gd3_core::TaskError;
use gd3_core::checksum;
use gd3_core::pack::PackManifest;
use gd3_core::sources::{Repo, app_repo};
use gd3_net::sources::{self, Transport, WreqTransport};

use crate::extension_crx;
use crate::update_service::{
    Host, LocalPack, UnpackKind, UpdateEffect, UpdateRuntime, UpdateStep, UpdaterCommand,
    UpdaterSkip, pack_dir_candidate,
};

/// 一轮 `run` 最多执行多少个动作（防状态机与宿主互相喂死循环）。
pub const MAX_EFFECTS: usize = 4096;

/// 一轮「跑到底」的产物。
#[derive(Debug, Default)]
pub struct UpdateRun {
    /// 状态机发出的全部通知（顺序与 Python `_emit` 一致）。
    pub steps: Vec<UpdateStep>,
    /// `StartUpdater`：要拉起的 updater 进程（**本模块不 spawn**）。
    pub spawns: Vec<UpdaterCommand>,
    /// `SkipUpdater`：`_startUpdater` 的两个「记日志然后 return」分支。
    pub skips: Vec<UpdaterSkip>,
    /// 宿主侧无法归到某个 target 的错误（例如 pack 解包的异常）。
    pub errors: Vec<String>,
}

impl UpdateRun {
    /// 最后一条涉及 `target_id` 的状态（没有则 `None`）。
    pub fn last_state(&self, target_id: &str) -> Option<crate::update_service::UpdateState> {
        self.steps
            .iter()
            .rev()
            .find(|step| step.target_id == target_id)
            .map(|step| step.state)
    }
}

/// 效果执行器：一次构造，反复用于 check / download / apply。
pub struct UpdateExecutor {
    transport: Arc<dyn Transport>,
    repo: Repo,
    branch: String,
    features_dir: PathBuf,
}

impl UpdateExecutor {
    /// 显式给齐四个要素（测试与自建宿主都用这个）。
    pub fn new(
        transport: Arc<dyn Transport>,
        repo: Repo,
        branch: impl Into<String>,
        features_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            transport,
            repo,
            branch: branch.into(),
            features_dir: features_dir.into(),
        }
    }

    /// 生产环境：`WreqTransport(profile)` + [`app_repo`] + `main` + `host.features_dir`。
    pub fn with_wreq(profile: &str, host: &Host) -> Self {
        Self::new(
            Arc::new(WreqTransport::new(profile)),
            app_repo(),
            "main",
            host.features_dir.clone(),
        )
    }

    /// 本地已安装的 pack（`_check` 里 `sorted(FEATURES_DIR.iterdir())` 的那一圈）。
    ///
    /// 排序按**完整路径**（与 `sorted(Path)` 一致）；`manifest.toml` 缺失、`version`
    /// 为空、`manifest.toml` 解析失败都跳过 —— 与 Python 的 `continue` 等价。
    pub fn local_packs(&self) -> Vec<LocalPack> {
        let Ok(entries) = fs::read_dir(&self.features_dir) else {
            return Vec::new();
        };
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        dirs.sort();
        dirs.into_iter()
            .filter_map(|dir| {
                let name = dir.file_name()?.to_string_lossy().into_owned();
                if !pack_dir_candidate(&name) {
                    return None;
                }
                let manifest = PackManifest::from_dir(&dir)?;
                if manifest.version.is_empty() {
                    return None;
                }
                Some(LocalPack::from_manifest(&manifest))
            })
            .collect()
    }

    /// `check()` + 执行到底。
    pub async fn check(&self, runtime: &mut UpdateRuntime) -> UpdateRun {
        let effects = runtime.check();
        self.run(runtime, "app", effects).await
    }

    /// `download(targetId)` + 执行到底（`can_download` 不成立时什么也不做）。
    pub async fn download(&self, runtime: &mut UpdateRuntime, target_id: &str) -> UpdateRun {
        let effects = runtime.download(target_id);
        self.run(runtime, target_id, effects).await
    }

    /// `apply()` + 执行到底（pack 解到 `*_pending`，updater 命令交给宿主）。
    pub async fn apply(&self, runtime: &mut UpdateRuntime) -> UpdateRun {
        let effects = runtime.apply();
        self.run(runtime, "app", effects).await
    }

    /// 把一批动作跑完，边跑边把结果喂回状态机。
    ///
    /// `target_id` 只用于**归因**：动作本身不带 target，出错时按调用方给的 target 落
    /// `FAILED`（Python 里异常发生在哪个 `_download*` 就落哪个 target）。
    pub async fn run(
        &self,
        runtime: &mut UpdateRuntime,
        target_id: &str,
        effects: Vec<UpdateEffect>,
    ) -> UpdateRun {
        let mut queue: VecDeque<UpdateEffect> = effects.into();
        let mut run = UpdateRun::default();
        let mut budget = MAX_EFFECTS;
        while let Some(effect) = queue.pop_front() {
            if budget == 0 {
                run.errors
                    .push(format!("更新链路超过 {MAX_EFFECTS} 个动作，已中止"));
                break;
            }
            budget -= 1;
            let produced = self.execute(runtime, target_id, effect, &mut run).await;
            queue.extend(produced);
        }
        run
    }

    async fn execute(
        &self,
        runtime: &mut UpdateRuntime,
        target_id: &str,
        effect: UpdateEffect,
        run: &mut UpdateRun,
    ) -> Vec<UpdateEffect> {
        match effect {
            UpdateEffect::Emit(step) => {
                run.steps.push(step);
                Vec::new()
            }
            UpdateEffect::FetchVersions => {
                // `_fetchVersions` 只吃 `RuntimeError`（= 竞速全败），其余异常
                // 在 `get_json` 里已经被当成「这个源失败」丢掉了。
                let packs = self.local_packs();
                let fetched = sources::fetch_json(
                    &*self.transport,
                    &self.repo,
                    &self.branch,
                    "versions.json",
                )
                .await;
                match fetched {
                    Ok((data, _source)) => runtime.versions_ready(Some(data), &packs),
                    Err(_) => runtime.versions_ready(None, &packs),
                }
            }
            UpdateEffect::Mkdir(path) => match fs::create_dir_all(&path) {
                Ok(()) => Vec::new(),
                Err(error) => runtime.download_failed(target_id, TaskError::io(error)),
            },
            // `shutil.rmtree` 只在 `newDir.exists()` 时调用，宿主按「不存在也算成功」执行。
            UpdateEffect::RemoveDir(path) => match fs::remove_dir_all(&path) {
                Ok(()) => Vec::new(),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
                Err(error) => runtime.download_failed(target_id, TaskError::io(error)),
            },
            // `Path.unlink(missing_ok=True)`。
            UpdateEffect::RemoveFile(path) => match fs::remove_file(&path) {
                Ok(()) => Vec::new(),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
                Err(error) => runtime.download_failed(target_id, TaskError::io(error)),
            },
            UpdateEffect::FetchAsset { tag, file, dest } => {
                let progress = Mutex::new(Vec::new());
                let fetched = {
                    let callback = |value: f64| {
                        progress.lock().expect("进度锁被污染").push(value);
                    };
                    sources::fetch_release_asset(
                        &*self.transport,
                        &self.repo,
                        &tag,
                        &file,
                        &dest,
                        Some(&callback),
                    )
                    .await
                    .map(|_url| ())
                };
                after_download(runtime, target_id, &progress, fetched)
            }
            UpdateEffect::FetchRaw { path, dest } => {
                let progress = Mutex::new(Vec::new());
                let fetched = {
                    let callback = |value: f64| {
                        progress.lock().expect("进度锁被污染").push(value);
                    };
                    sources::fetch_raw_file(
                        &*self.transport,
                        &self.repo,
                        &self.branch,
                        &path,
                        &dest,
                        Some(&callback),
                    )
                    .await
                    .map(|_source| ())
                };
                after_download(runtime, target_id, &progress, fetched)
            }
            UpdateEffect::Checksum { path, .. } => {
                // 期望值比较在状态机（`checksum_accepted`）里做，这里只算摘要。
                match checksum::to_checksum(&path, "sha256", |_percent| {}) {
                    Ok(hex) => runtime.checksum_finished(target_id, &hex),
                    Err(error) => runtime.download_failed(target_id, TaskError::io(error)),
                }
            }
            UpdateEffect::Unpack {
                kind,
                archive,
                target,
            } => {
                let pending = runtime.is_pending(target_id);
                match unpack(kind, &archive, &target) {
                    Ok(()) => {
                        let mut effects = runtime.unpack_finished(target_id, None);
                        if !pending {
                            // `_applyPack` 里 `zipPath.unlink()`：完整包的删除由状态机发。
                            effects.push(UpdateEffect::RemoveFile(archive));
                        }
                        effects
                    }
                    Err(detail) => {
                        let error = TaskError::io(detail);
                        if pending {
                            runtime.unpack_finished(target_id, Some(error))
                        } else {
                            run.errors.push(format!(
                                "解包 {} 失败：{}",
                                archive.display(),
                                error.format()
                            ));
                            Vec::new()
                        }
                    }
                }
            }
            UpdateEffect::StartUpdater(command) => {
                // updater 会顶掉当前进程：交给宿主在合适时机 spawn。
                run.spawns.push(command);
                Vec::new()
            }
            UpdateEffect::SkipUpdater(skip) => {
                run.skips.push(skip);
                Vec::new()
            }
        }
    }
}

/// 下载收尾：成功先回放进度，再让状态机算校验和/落 READY；失败直接 `FAILED`。
fn after_download(
    runtime: &mut UpdateRuntime,
    target_id: &str,
    progress: &Mutex<Vec<f64>>,
    fetched: Result<(), TaskError>,
) -> Vec<UpdateEffect> {
    match fetched {
        Ok(()) => {
            let values: Vec<f64> = progress
                .lock()
                .map(|guard| guard.clone())
                .unwrap_or_default();
            let mut effects = Vec::with_capacity(values.len() + 1);
            for value in values {
                effects.extend(runtime.download_progress(target_id, value));
            }
            effects.extend(runtime.download_finished(target_id));
            effects
        }
        Err(error) => runtime.download_failed(target_id, error),
    }
}

/// `extractZip` / `extractTarXz` / `extractDmg` 的宿主半边。
fn unpack(kind: UnpackKind, archive: &Path, folder: &Path) -> Result<(), String> {
    match kind {
        UnpackKind::Zip => {
            let bytes =
                fs::read(archive).map_err(|error| format!("{}：{error}", archive.display()))?;
            unpack_zip(&bytes, folder)
                .map(|_written| ())
                .map_err(|error| format!("{}：{error}", archive.display()))
        }
        UnpackKind::TarXz => Err("tar.xz 解包尚未接（离线环境缺 xz2 依赖）".to_string()),
        UnpackKind::Dmg => Err("dmg 解包尚未接（仅 macOS，需要 hdiutil）".to_string()),
    }
}

/// `zipfile.ZipFile(path).extractall(folder)`：复用 CRX 那套 ZIP 读取器。
///
/// 与 `extension_crx::unpack` 的区别只有「不做 CRX 头剥离」，其余（中央目录序、
/// 路径净化、父目录按需创建）完全照抄。
fn unpack_zip(zip: &[u8], folder: &Path) -> Result<Vec<String>, extension_crx::CrxError> {
    let entries = extension_crx::entries(zip)?;
    fs::create_dir_all(folder)?;
    let mut written = Vec::with_capacity(entries.len());
    for entry in &entries {
        let relative = extension_crx::sanitize(&entry.name);
        if relative.as_os_str().is_empty() {
            continue;
        }
        let target = folder.join(&relative);
        if entry.is_dir {
            fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&target, extension_crx::read_entry(zip, entry)?)?;
        written.push(relative.to_string_lossy().into_owned());
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    use gd3_net::sources::{BoxFetch, FetchError};
    use serde_json::{Value, json};

    use super::*;
    use crate::update_service::UpdateState;

    /// 可注入的 `Transport`：按 URL **后缀**查 JSON / 文件体。
    #[derive(Default)]
    struct Fake {
        json: BTreeMap<String, Value>,
        files: BTreeMap<String, Vec<u8>>,
        calls: Mutex<Vec<String>>,
    }

    impl Fake {
        fn new() -> Self {
            Self::default()
        }

        fn json(mut self, suffix: &str, value: Value) -> Self {
            self.json.insert(suffix.to_string(), value);
            self
        }

        fn file(mut self, suffix: &str, body: &[u8]) -> Self {
            self.files.insert(suffix.to_string(), body.to_vec());
            self
        }

        fn record(&self, call: String) {
            self.calls.lock().expect("调用记录锁被污染").push(call);
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().expect("调用记录锁被污染").clone()
        }

        fn body_of(&self, url: &str) -> Option<&Vec<u8>> {
            self.files
                .iter()
                .find(|(suffix, _)| url.ends_with(suffix.as_str()))
                .map(|(_, body)| body)
        }
    }

    impl Transport for Fake {
        fn get_json<'a>(
            &'a self,
            url: &'a str,
            _headers: &'a [(String, String)],
            _timeout: Duration,
        ) -> BoxFetch<'a, Result<Value, FetchError>> {
            Box::pin(async move {
                self.record(format!("json {url}"));
                match self
                    .json
                    .iter()
                    .find(|(suffix, _)| url.ends_with(suffix.as_str()))
                {
                    Some((_, value)) => Ok(value.clone()),
                    None => Err(FetchError::Status(404)),
                }
            })
        }

        fn probe<'a>(
            &'a self,
            url: &'a str,
            _headers: &'a [(String, String)],
            _timeout: Duration,
        ) -> BoxFetch<'a, Result<(), FetchError>> {
            Box::pin(async move {
                self.record(format!("probe {url}"));
                match self.body_of(url) {
                    Some(_) => Ok(()),
                    None => Err(FetchError::Status(404)),
                }
            })
        }

        fn download<'a>(
            &'a self,
            url: &'a str,
            output: &'a Path,
            progress: Option<&'a (dyn Fn(f64) + Send + Sync)>,
        ) -> BoxFetch<'a, Result<(), FetchError>> {
            Box::pin(async move {
                self.record(format!("download {url}"));
                let Some(body) = self.body_of(url) else {
                    return Err(FetchError::Status(404));
                };
                fs::write(output, body)
                    .map_err(|error| FetchError::Transport(error.to_string()))?;
                if let Some(callback) = progress {
                    callback(100.0);
                }
                Ok(())
            })
        }
    }

    fn scratch(tag: &str) -> (Host, PathBuf) {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "gd3-ui-update-exec-{}-{tag}-{seq}",
            std::process::id()
        ));
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
            pid: 4321,
            interpreter: dir.join("python"),
        };
        (host, dir)
    }

    fn executor(transport: Fake, host: &Host) -> UpdateExecutor {
        UpdateExecutor::new(
            Arc::new(transport),
            app_repo(),
            "main",
            host.features_dir.clone(),
        )
    }

    /// 装一个本地 pack（`manifest.toml` + 入口文件），返回目录。
    fn install_pack(host: &Host, name: &str, version: &str) -> PathBuf {
        let dir = host.features_dir.join(name);
        fs::create_dir_all(&dir).expect("建 pack 目录");
        fs::write(
            dir.join("manifest.toml"),
            format!(
                "[pack]\nentry = \"pack.py\"\nclass = \"{name}Pack\"\nversion = \"{version}\"\ngdMinVersion = \"4.0.0\"\n"
            ),
        )
        .expect("写 manifest.toml");
        fs::write(dir.join("pack.py"), "# pack\n").expect("写入口");
        dir
    }

    /// 最小 stored（method 0）ZIP：`extension_crx::entries` 与 CPython `zipfile` 都读得动。
    fn stored_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in entries {
            let offset = out.len() as u32;
            let size = data.len() as u32;
            out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            out.extend_from_slice(&20u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);

            central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u32.to_le_bytes());
            central.extend_from_slice(&size.to_le_bytes());
            central.extend_from_slice(&size.to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u32.to_le_bytes());
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let cd_offset = out.len() as u32;
        let cd_size = central.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    fn sha256_of(bytes: &[u8], dir: &Path) -> String {
        let path = dir.join("sha-probe.bin");
        fs::write(&path, bytes).expect("写探针文件");
        let hex = checksum::to_checksum(&path, "sha256", |_| {}).expect("算 sha256");
        fs::remove_file(&path).expect("删探针文件");
        hex
    }

    fn packs(version: &str, sha: &str) -> Value {
        json!({
            "app": {"version": "4.4.0"},
            "packs": {"youtube": {"version": version, "file": "youtube.zip", "sha256": sha}}
        })
    }

    #[tokio::test]
    async fn a_local_pack_and_a_newer_remote_version_become_available() {
        let (host, _dir) = scratch("check");
        install_pack(&host, "youtube", "1.1.0");
        // `_pending` 目录与隐藏目录不进扫描（`pack_dir_candidate`）
        fs::create_dir_all(host.features_dir.join("youtube_pending")).expect("建 _pending");
        fs::create_dir_all(host.features_dir.join(".git")).expect("建隐藏目录");

        let transport = Fake::new().json(
            "main/versions.json",
            json!({
                "app": {"version": "4.5.0"},
                "packs": {"youtube": {"version": "1.2.0", "file": "youtube.zip", "sha256": ""}}
            }),
        );
        let executor = executor(transport, &host);
        let mut runtime = UpdateRuntime::new(host.clone());
        let run = executor.check(&mut runtime).await;

        assert_eq!(run.errors, Vec::<String>::new());
        assert_eq!(run.last_state("app"), Some(UpdateState::Available));
        assert_eq!(run.last_state("youtube"), Some(UpdateState::Available));
        let info = runtime.table().get("youtube").expect("youtube 在表里");
        assert_eq!(info.current_version, "1.1.0");
        assert_eq!(info.latest_version, "1.2.0");
        assert_eq!(executor.local_packs().len(), 1);
    }

    #[tokio::test]
    async fn a_missing_versions_file_leaves_app_idle_with_the_error() {
        let (host, _dir) = scratch("check-fail");
        let executor = executor(Fake::new(), &host);
        let mut runtime = UpdateRuntime::new(host.clone());
        let run = executor.check(&mut runtime).await;

        assert_eq!(run.last_state("app"), Some(UpdateState::Idle));
        let info = runtime.table().get("app").expect("app 在表里");
        assert_eq!(
            info.error.as_ref().map(TaskError::format).as_deref(),
            Some("无法获取版本信息")
        );
    }

    #[tokio::test]
    async fn a_pack_download_that_matches_its_checksum_becomes_ready() {
        let (host, dir) = scratch("pack-ok");
        install_pack(&host, "youtube", "1.1.0");
        let body = b"pack-bytes".to_vec();
        let sha = sha256_of(&body, &dir);
        let transport = Fake::new()
            .json("main/versions.json", packs("1.2.0", &sha))
            .file("youtube.zip", &body);
        let executor = executor(transport, &host);
        let mut runtime = UpdateRuntime::new(host.clone());
        executor.check(&mut runtime).await;

        let run = executor.download(&mut runtime, "youtube").await;

        assert_eq!(run.errors, Vec::<String>::new());
        assert_eq!(run.last_state("youtube"), Some(UpdateState::Ready));
        let staged = host.staging_dir.join("youtube.zip");
        assert_eq!(fs::read(&staged).expect("读到 staging"), body);
        // `download` 的初始 DOWNLOADING + 回放的进度回调
        let downloading: Vec<_> = run
            .steps
            .iter()
            .filter(|step| step.target_id == "youtube" && step.state == UpdateState::Downloading)
            .collect();
        assert_eq!(downloading.len(), 2);
        assert_eq!(downloading[1].fields.progress, Some(100.0));
        assert!(
            run.steps
                .iter()
                .filter(|step| step.state == UpdateState::Ready)
                .count()
                == 1
        );
    }

    #[tokio::test]
    async fn a_mismatched_checksum_removes_the_archive_and_fails() {
        let (host, dir) = scratch("pack-bad-sha");
        install_pack(&host, "youtube", "1.1.0");
        let wrong = sha256_of(b"something-else", &dir);
        let transport = Fake::new()
            .json("main/versions.json", packs("1.2.0", &wrong))
            .file("youtube.zip", b"pack-bytes");
        let executor = executor(transport, &host);
        let mut runtime = UpdateRuntime::new(host.clone());
        executor.check(&mut runtime).await;

        let run = executor.download(&mut runtime, "youtube").await;

        assert_eq!(run.last_state("youtube"), Some(UpdateState::Failed));
        assert!(!host.staging_dir.join("youtube.zip").exists());
        let info = runtime.table().get("youtube").expect("youtube 在表里");
        assert_eq!(
            info.error.as_ref().map(TaskError::format).as_deref(),
            Some("校验失败")
        );
    }

    #[tokio::test]
    async fn a_failed_download_marks_the_target_failed() {
        let (host, _dir) = scratch("pack-404");
        install_pack(&host, "youtube", "1.1.0");
        // 只给 versions.json，不给 youtube.zip 的文件体 → 探测全败
        let transport = Arc::new(Fake::new().json("main/versions.json", packs("1.2.0", "")));
        let executor = UpdateExecutor::new(
            Arc::clone(&transport) as Arc<dyn Transport>,
            app_repo(),
            "main",
            host.features_dir.clone(),
        );
        let mut runtime = UpdateRuntime::new(host.clone());
        executor.check(&mut runtime).await;

        let run = executor.download(&mut runtime, "youtube").await;

        assert_eq!(run.last_state("youtube"), Some(UpdateState::Failed));
        let info = runtime.table().get("youtube").expect("youtube 在表里");
        assert_eq!(
            info.error.as_ref().map(TaskError::format).as_deref(),
            Some("无法下载 XiaoYouChR/Ghost-Downloader-3/main/dist/packs/youtube.zip")
        );
        assert!(!host.staging_dir.join("youtube.zip").exists());
        // 两条 raw 源都探测过（github + gitcode），但一次下载都没发起
        let calls = transport.calls();
        assert_eq!(
            calls
                .iter()
                .filter(|call| call.starts_with("probe "))
                .count(),
            2,
            "{calls:?}"
        );
        assert!(!calls.iter().any(|call| call.starts_with("download ")));
        assert!(
            executor
                .local_packs()
                .iter()
                .all(|pack| pack.version == "1.1.0")
        );
    }

    #[tokio::test]
    async fn apply_unpacks_a_ready_pack_into_pending_and_removes_the_zip() {
        let (host, dir) = scratch("pack-apply");
        install_pack(&host, "youtube", "1.1.0");
        let archive = stored_zip(&[
            ("pack.py", b"# new pack\n"),
            ("assets/", b""),
            ("assets/icon.png", b"png"),
        ]);
        let sha = sha256_of(&archive, &dir);
        let transport = Fake::new()
            .json("main/versions.json", packs("1.2.0", &sha))
            .file("youtube.zip", &archive);
        let executor = executor(transport, &host);
        let mut runtime = UpdateRuntime::new(host.clone());
        executor.check(&mut runtime).await;
        executor.download(&mut runtime, "youtube").await;

        let run = executor.apply(&mut runtime).await;

        assert_eq!(run.errors, Vec::<String>::new());
        assert!(run.spawns.is_empty());
        let pending = host.features_dir.join("youtube_pending");
        assert_eq!(
            fs::read(pending.join("pack.py")).expect("读到解包的 pack.py"),
            b"# new pack\n"
        );
        assert_eq!(
            fs::read(pending.join("assets/icon.png")).expect("读到子目录里的文件"),
            b"png"
        );
        // `_applyPack` 的 `zipPath.unlink()`
        assert!(!host.staging_dir.join("youtube.zip").exists());
    }

    #[tokio::test]
    async fn an_app_full_update_downloads_checksums_and_unpacks_into_app_new() {
        let (host, dir) = scratch("app-full");
        let archive = stored_zip(&[("Ghost-Downloader-3/gd3", b"binary")]);
        let sha = sha256_of(&archive, &dir);
        let transport = Fake::new()
            .json(
                "main/versions.json",
                json!({
                    "app": {
                        "version": "4.5.0",
                        "full": {"Linux-x86_64": {"file": "gd3.zip", "sha256": sha}}
                    }
                }),
            )
            .file("gd3.zip", &archive);
        let executor = executor(transport, &host);
        let mut runtime = UpdateRuntime::new(host.clone());
        let checked = executor.check(&mut runtime).await;
        assert_eq!(checked.last_state("app"), Some(UpdateState::Available));

        let run = executor.download(&mut runtime, "app").await;

        assert_eq!(run.errors, Vec::<String>::new());
        assert_eq!(run.last_state("app"), Some(UpdateState::Ready));
        let unpacked = host
            .staging_dir
            .join(crate::update_service::NEW_APP_DIR_NAME)
            .join("Ghost-Downloader-3/gd3");
        assert_eq!(fs::read(&unpacked).expect("读到解包的二进制"), b"binary");
        // 归档在解包成功后由状态机删掉
        assert!(!host.staging_dir.join("gd3.zip").exists());
        assert!(run.spawns.is_empty(), "apply 之前不该拉起 updater");
    }

    #[tokio::test]
    async fn a_tar_xz_full_update_fails_because_xz_is_not_wired() {
        let (host, dir) = scratch("app-xz");
        let sha = sha256_of(b"xz-bytes", &dir);
        let transport = Fake::new()
            .json(
                "main/versions.json",
                json!({
                    "app": {
                        "version": "4.5.0",
                        "full": {"Linux-x86_64": {"file": "gd3.tar.xz", "sha256": sha}}
                    }
                }),
            )
            .file("gd3.tar.xz", b"xz-bytes");
        let executor = executor(transport, &host);
        let mut runtime = UpdateRuntime::new(host.clone());
        executor.check(&mut runtime).await;

        let run = executor.download(&mut runtime, "app").await;

        assert_eq!(run.last_state("app"), Some(UpdateState::Failed));
        let info = runtime.table().get("app").expect("app 在表里");
        let message = info
            .error
            .as_ref()
            .map(TaskError::format)
            .unwrap_or_default();
        assert!(message.contains("xz2"), "错误应当是 xz 未接：{message}");
        // 解包失败时归档保留（Python 的「留着好排查」）
        assert!(host.staging_dir.join("gd3.tar.xz").is_file());
    }

    #[tokio::test]
    async fn apply_asks_for_the_updater_command_when_the_app_is_ready() {
        let (host, dir) = scratch("app-updater");
        let archive = stored_zip(&[("gd3", b"binary")]);
        let sha = sha256_of(&archive, &dir);
        let transport = Fake::new()
            .json(
                "main/versions.json",
                json!({
                    "app": {
                        "version": "4.5.0",
                        "full": {"Linux-x86_64": {"file": "gd3.zip", "sha256": sha}}
                    }
                }),
            )
            .file("gd3.zip", &archive);
        let executor = executor(transport, &host);
        let mut runtime = UpdateRuntime::new(host.clone());
        executor.check(&mut runtime).await;
        executor.download(&mut runtime, "app").await;
        // `_startUpdater` 要求 updater 存在；这里给一个假的可执行文件
        fs::create_dir_all(&host.executable_dir).expect("建 bin");
        fs::write(host.executable_dir.join("updater"), b"#!/bin/sh\n").expect("写 updater");

        let run = executor.apply(&mut runtime).await;

        assert_eq!(run.errors, Vec::<String>::new());
        assert_eq!(run.spawns.len(), 1);
        assert_eq!(
            run.spawns[0].args[0],
            host.executable_dir.join("updater").display().to_string()
        );
        assert!(run.skips.is_empty());
    }

    #[test]
    fn unpack_reports_the_two_unwired_kinds() {
        let (host, dir) = scratch("unpack-kinds");
        let archive = dir.join("payload.bin");
        fs::write(&archive, b"whatever").expect("写假归档");
        let tar = unpack(UnpackKind::TarXz, &archive, &host.staging_dir).unwrap_err();
        let dmg = unpack(UnpackKind::Dmg, &archive, &host.staging_dir).unwrap_err();
        assert!(tar.contains("tar.xz"), "{tar}");
        assert!(dmg.contains("dmg"), "{dmg}");
        assert!(!host.staging_dir.exists());
    }
}
