//! 运行时状态服务：镜像 `app/services/runtime_status.py`（`RuntimeStatusService`）。
//!
//! 规格来源：`app/services/runtime_status.py:1-195`。
//!
//! ## 结构
//!
//! Python 那边是「一个协程运行器 + 一个信号」：`_coroutineRunner.submit(coro, done=…,
//! failed=…)` 拿 `workId`，`statusChanged.emit(status)` 通知 View。Rust 没有协程运行时，
//! 沿用第 100/101 步的做法：**服务只算状态与意图，副作用摊成 [`RuntimeEffect`] 队列**，
//! 宿主按序消费：
//!
//! | 效果 | 宿主该做什么 |
//! |---|---|
//! | [`RuntimeEffect::Emit`] | 更新设置页那张 `RuntimeCard`（[`crate::setting_cards::runtime_view`]） |
//! | [`RuntimeEffect::CancelWork`] | 取消对应的工作（`work_id` 是本服务发的**不透明句柄**） |
//! | [`RuntimeEffect::Run`] | 真去跑 `probe_version` / `fetch_latest_version` / `install`，然后回调对应的 `on_*` |
//!
//! ## 顺序
//!
//! 与 Python 一致：`Emit` 总在 `Run` **之前**（`_probe` / `install` 都是先置状态再
//! `submit`），`CancelWork` 在最前（Python 先 `cancel(workId)` 再改状态）。
//!
//! ## workId
//!
//! Python 的 `workId` 由 `coroutineRunner.submit` 发号；这里由服务自己发号
//! （`runtime-1` / `runtime-2` …，**一个全局计数器**，不是每类独立递增），宿主当不透明
//! 句柄原样交回 `CancelWork`（同第 100 步 `task_actions` 的约定）。
//!
//! ## 与 Python 的已知差异
//!
//! 1. `reportProgress` 里 Python 用 `_coroutineRunner.post(...)`（线程安全地排到主循环），
//!    其余回调直接 `emit`；Rust 侧两种都退化成返回值，差别消失。
//! 2. `toTaskError(error)` 接受任意异常，这里由调用方转成 [`TaskError`]
//!    （`TaskError::io(e)` 即 `发生了意外错误：{detail}`）。
//! 3. `int(percent)` 对 `NaN` 抛 `ValueError`，`percent as i32` 饱和到 `0`（无实际影响：
//!    进度由本仓自己的 `install` 实现回报）。

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use gd3_core::pack::{BinaryRuntime, VersionInfo};
use gd3_core::task_error::TaskError;

/// `RuntimeStatus`（`runtime_status.py:14-24`）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuntimeStatus {
    /// `runtimeId`：`BinaryRuntime::runtime_id()`（类型全名）。
    pub runtime_id: String,
    pub name: String,
    /// `""` = 没找到可执行文件。
    pub path: String,
    pub version: String,
    pub detail: String,
    pub latest_version: String,
    pub error: Option<TaskError>,
    pub is_busy: bool,
    pub is_installing: bool,
    /// `int(percent)`：0–100。
    pub progress: i32,
}

impl RuntimeStatus {
    /// `RuntimeStatus(runtimeId, name)`（其余走缺省）。
    pub fn new(runtime_id: impl Into<String>, name: impl Into<String>) -> Self {
        RuntimeStatus {
            runtime_id: runtime_id.into(),
            name: name.into(),
            ..RuntimeStatus::default()
        }
    }

    /// 折成设置页 `RuntimeCard` 的输入（[`crate::setting_cards::RuntimeStatus`]）。
    ///
    /// 那边 `error` 是**已本地化文本**（Python `toLocalizedError(status.error)`），
    /// 所以本地化函数由调用方注入（`gd3-ui` 没有 i18n 表，只有键与参数）。
    pub fn to_card_status(
        &self,
        localize: impl FnOnce(&TaskError) -> String,
    ) -> crate::setting_cards::RuntimeStatus {
        crate::setting_cards::RuntimeStatus {
            name: self.name.clone(),
            path: self.path.clone(),
            version: self.version.clone(),
            detail: self.detail.clone(),
            latest_version: self.latest_version.clone(),
            is_busy: self.is_busy,
            is_installing: self.is_installing,
            progress: self.progress,
            error: self.error.as_ref().map(localize),
        }
    }

    /// `RuntimeCard.updateStatus(status)`：`can_install` / `is_managed` 现读运行时。
    pub fn card_view(
        &self,
        runtime: &dyn BinaryRuntime,
        localize: impl FnOnce(&TaskError) -> String,
    ) -> crate::setting_cards::RuntimeCardView {
        crate::setting_cards::runtime_view(
            &self.to_card_status(localize),
            runtime.can_install(),
            runtime.is_app_managed(),
        )
    }
}

/// 要宿主去跑的活（`_probe` / `_fetchLatest` / `_runInstall` 的协程体）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeWorkKind {
    /// `runtime.probeVersion()` → 回调 [`RuntimeStatusService::on_probe_finished`]。
    Probe,
    /// `runtime.fetchLatestVersion()` → 回调
    /// [`RuntimeStatusService::on_fetch_finished`]。
    FetchLatest,
    /// `runtime.install(version, reportProgress)` → 回调
    /// [`RuntimeStatusService::on_install_finished`]，装完再过
    /// [`RuntimeStatusService::check_installed`]。
    Install { version: String },
}

/// 服务吐出的副作用，宿主按序应用。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeEffect {
    /// `statusChanged.emit(status)`。
    Emit(RuntimeStatus),
    /// `self._coroutineRunner.cancel(workId)`。
    CancelWork(String),
    /// `self._coroutineRunner.submit(...)`。
    Run {
        /// 本次工作的句柄（宿主原样交回 `CancelWork`）。
        work_id: String,
        /// 哪个运行时。
        runtime_id: String,
        /// 跑什么。
        kind: RuntimeWorkKind,
    },
}

/// `RuntimeStatusService`（`runtime_status.py:27-195`）。
///
/// 状态表按 `runtimeId` 索引；`runtimes` 对应 Python 的 `_runtimes`（跨「异步间隙」
/// 持有对象引用，`_onProbeFinished` / `_onInstallFinished` 还要用它问
/// `isAppManaged()` 或再 `invalidate` 一次）。
#[derive(Default)]
pub struct RuntimeStatusService {
    statuses: BTreeMap<String, RuntimeStatus>,
    work_ids: BTreeMap<String, String>,
    runtimes: BTreeMap<String, Rc<dyn BinaryRuntime>>,
    installing: BTreeSet<String>,
    issued: u64,
}

impl RuntimeStatusService {
    pub fn new() -> Self {
        RuntimeStatusService::default()
    }

    /// 已登记的状态（`None` = 还没探测过）。
    pub fn stored_status(&self, runtime_id: &str) -> Option<&RuntimeStatus> {
        self.statuses.get(runtime_id)
    }

    /// 该运行时当前挂着的工作句柄。
    pub fn work_id(&self, runtime_id: &str) -> Option<&str> {
        self.work_ids.get(runtime_id).map(String::as_str)
    }

    /// `runtimeId in self._installingIds`。
    pub fn is_installing(&self, runtime_id: &str) -> bool {
        self.installing.contains(runtime_id)
    }

    /// 跨异步间隙还持有几个运行时对象（Python `len(self._runtimes)`）。
    pub fn held_runtimes(&self) -> usize {
        self.runtimes.len()
    }

    /// `status(runtime)`：已登记的就返回它，否则现造一个只带 id / name / path 的。
    pub fn status(&self, runtime: &dyn BinaryRuntime) -> RuntimeStatus {
        let runtime_id = runtime.runtime_id();
        if let Some(status) = self.statuses.get(runtime_id) {
            return status.clone();
        }
        RuntimeStatus {
            runtime_id: runtime_id.to_string(),
            name: runtime.name().to_string(),
            path: runtime.path(),
            ..RuntimeStatus::default()
        }
    }

    /// `refreshStatus(runtime)`：已知状态干净且路径没变就什么都不做。
    pub fn refresh_status(&mut self, runtime: &Rc<dyn BinaryRuntime>) -> Vec<RuntimeEffect> {
        let runtime_id = runtime.runtime_id();
        if let Some(current) = self.statuses.get(runtime_id)
            && !current.is_busy
            && current.error.is_none()
        {
            let path = runtime.path();
            if current.path == path {
                return Vec::new();
            }
        }
        self.probe(runtime)
    }

    /// `invalidate(runtime)`：掐掉在跑的工作、丢掉持有对象，然后重新探测。
    pub fn invalidate(&mut self, runtime: &Rc<dyn BinaryRuntime>) -> Vec<RuntimeEffect> {
        let runtime_id = runtime.runtime_id();
        let mut effects = Vec::new();
        if let Some(work_id) = self.work_ids.remove(runtime_id) {
            effects.push(RuntimeEffect::CancelWork(work_id));
        }
        self.runtimes.remove(runtime_id);
        effects.extend(self.probe(runtime));
        effects
    }

    /// `install(runtime)`：已在装就忽略；否则掐旧工作、置「安装中」、派安装。
    ///
    /// 传给 `install` 的版本号是**当前状态里的 `latestVersion`**（可能为 `""`，
    /// 表示「装最新」）。
    pub fn install(&mut self, runtime: &Rc<dyn BinaryRuntime>) -> Vec<RuntimeEffect> {
        let runtime_id = runtime.runtime_id();
        if self.installing.contains(runtime_id) {
            return Vec::new();
        }
        let mut effects = Vec::new();
        if let Some(work_id) = self.work_ids.remove(runtime_id) {
            effects.push(RuntimeEffect::CancelWork(work_id));
        }
        let latest_version = self
            .statuses
            .get(runtime_id)
            .map(|status| status.latest_version.clone())
            .unwrap_or_default();
        self.installing.insert(runtime_id.to_string());
        self.runtimes
            .insert(runtime_id.to_string(), Rc::clone(runtime));

        let status = RuntimeStatus {
            is_installing: true,
            ..RuntimeStatus::new(runtime_id, runtime.name())
        };
        self.statuses.insert(runtime_id.to_string(), status.clone());
        effects.push(RuntimeEffect::Emit(status));

        let work_id = self.issue_work_id();
        self.work_ids
            .insert(runtime_id.to_string(), work_id.clone());
        effects.push(RuntimeEffect::Run {
            work_id,
            runtime_id: runtime_id.to_string(),
            kind: RuntimeWorkKind::Install {
                version: latest_version,
            },
        });
        effects
    }

    /// `cancelInstall(runtime)`：没在装就忽略。
    pub fn cancel_install(&mut self, runtime: &Rc<dyn BinaryRuntime>) -> Vec<RuntimeEffect> {
        let runtime_id = runtime.runtime_id();
        if !self.installing.contains(runtime_id) {
            return Vec::new();
        }
        let mut effects = Vec::new();
        if let Some(work_id) = self.work_ids.remove(runtime_id) {
            effects.push(RuntimeEffect::CancelWork(work_id));
        }
        self.installing.remove(runtime_id);
        self.runtimes.remove(runtime_id);

        let status = RuntimeStatus {
            path: runtime.path(),
            ..RuntimeStatus::new(runtime_id, runtime.name())
        };
        self.statuses.insert(runtime_id.to_string(), status.clone());
        effects.push(RuntimeEffect::Emit(status));
        effects
    }

    /// `_runInstall` 里传给 `runtime.install` 的 `reportProgress(percent)`。
    ///
    /// 只在「正在装 且 整数进度变了」时吐一条 `Emit`。
    pub fn report_progress(&mut self, runtime_id: &str, percent: f64) -> Vec<RuntimeEffect> {
        let progress = percent as i32;
        let Some(current) = self.statuses.get(runtime_id) else {
            return Vec::new();
        };
        if !current.is_installing || current.progress == progress {
            return Vec::new();
        }
        let mut status = current.clone();
        status.progress = progress;
        self.statuses.insert(runtime_id.to_string(), status.clone());
        vec![RuntimeEffect::Emit(status)]
    }

    /// `_runInstall` 的尾巴：装完还找不到可执行文件就是失败。
    ///
    /// Python 是 `if not runtime.isAppManaged(): raise TaskError("安装后未找到 {name}",
    /// name=runtime.name)`，异常走 `_onInstallFailed`；Rust 返回 `Err` 由宿主转交
    /// [`RuntimeStatusService::on_install_failed`]。
    pub fn check_installed(runtime: &dyn BinaryRuntime) -> Result<(), TaskError> {
        if runtime.is_app_managed() {
            Ok(())
        } else {
            Err(TaskError::install_not_found(runtime.name()))
        }
    }

    /// `_onInstallFinished(_, runtimeId)`：清账后按 Python 的写法再 `invalidate` 一次。
    pub fn on_install_finished(&mut self, runtime_id: &str) -> Vec<RuntimeEffect> {
        self.installing.remove(runtime_id);
        self.work_ids.remove(runtime_id);
        let Some(runtime) = self.runtimes.get(runtime_id).cloned() else {
            return Vec::new();
        };
        self.invalidate(&runtime)
    }

    /// `_onInstallFailed(error, runtimeId)`：没在装就忽略（迟到的失败回调）。
    pub fn on_install_failed(&mut self, runtime_id: &str, error: TaskError) -> Vec<RuntimeEffect> {
        if !self.installing.contains(runtime_id) {
            return Vec::new();
        }
        self.installing.remove(runtime_id);
        self.work_ids.remove(runtime_id);
        self.runtimes.remove(runtime_id);
        let Some(current) = self.statuses.get(runtime_id) else {
            return Vec::new();
        };
        let mut status = current.clone();
        status.is_installing = false;
        status.error = Some(error);
        self.statuses.insert(runtime_id.to_string(), status.clone());
        vec![RuntimeEffect::Emit(status)]
    }

    /// `_onProbeFinished(versionInfo, runtimeId, name, path)`。
    ///
    /// `name` / `path` 是**探测时快照**（Python 通过 `submit` 的额外关键字参数带过去）。
    pub fn on_probe_finished(
        &mut self,
        runtime_id: &str,
        name: &str,
        path: &str,
        info: VersionInfo,
    ) -> Vec<RuntimeEffect> {
        self.work_ids.remove(runtime_id);
        let status = RuntimeStatus {
            path: path.to_string(),
            version: info.version,
            detail: info.detail,
            ..RuntimeStatus::new(runtime_id, name)
        };
        self.statuses.insert(runtime_id.to_string(), status.clone());
        let mut effects = vec![RuntimeEffect::Emit(status)];

        match self.runtimes.get(runtime_id).cloned() {
            Some(runtime) if runtime.is_app_managed() => {
                effects.extend(self.fetch_latest(&runtime));
            }
            _ => {
                self.runtimes.remove(runtime_id);
            }
        }
        effects
    }

    /// `_onProbeFailed(error, runtimeId, name, path)`。
    pub fn on_probe_failed(
        &mut self,
        runtime_id: &str,
        name: &str,
        path: &str,
        error: TaskError,
    ) -> Vec<RuntimeEffect> {
        self.work_ids.remove(runtime_id);
        self.runtimes.remove(runtime_id);
        let status = RuntimeStatus {
            path: path.to_string(),
            error: Some(error),
            ..RuntimeStatus::new(runtime_id, name)
        };
        self.statuses.insert(runtime_id.to_string(), status.clone());
        vec![RuntimeEffect::Emit(status)]
    }

    /// `_onFetchFinished(latestVersion, runtimeId)`：拿到的版本为空就不改状态。
    pub fn on_fetch_finished(
        &mut self,
        runtime_id: &str,
        latest_version: &str,
    ) -> Vec<RuntimeEffect> {
        self.work_ids.remove(runtime_id);
        self.runtimes.remove(runtime_id);
        let Some(current) = self.statuses.get(runtime_id) else {
            return Vec::new();
        };
        if latest_version.is_empty() {
            return Vec::new();
        }
        let mut status = current.clone();
        status.latest_version = latest_version.to_string();
        self.statuses.insert(runtime_id.to_string(), status.clone());
        vec![RuntimeEffect::Emit(status)]
    }

    /// `_onFetchFailed(error, runtimeId)`：只清账，**不改状态、不发信号**
    /// （Python 的两行 `pop`，`error` 直接丢掉）。
    pub fn on_fetch_failed(&mut self, runtime_id: &str) {
        self.work_ids.remove(runtime_id);
        self.runtimes.remove(runtime_id);
    }

    /// `_probe(runtime)`：置「检测中」、派探测。
    fn probe(&mut self, runtime: &Rc<dyn BinaryRuntime>) -> Vec<RuntimeEffect> {
        let runtime_id = runtime.runtime_id().to_string();
        let status = RuntimeStatus {
            path: runtime.path(),
            is_busy: true,
            ..RuntimeStatus::new(runtime_id.clone(), runtime.name())
        };
        self.runtimes.insert(runtime_id.clone(), Rc::clone(runtime));
        self.statuses.insert(runtime_id.clone(), status.clone());
        let mut effects = vec![RuntimeEffect::Emit(status)];

        let work_id = self.issue_work_id();
        self.work_ids.insert(runtime_id.clone(), work_id.clone());
        effects.push(RuntimeEffect::Run {
            work_id,
            runtime_id,
            kind: RuntimeWorkKind::Probe,
        });
        effects
    }

    /// `_fetchLatest(runtime)`：只派活，不动状态。
    fn fetch_latest(&mut self, runtime: &Rc<dyn BinaryRuntime>) -> Vec<RuntimeEffect> {
        let runtime_id = runtime.runtime_id().to_string();
        let work_id = self.issue_work_id();
        self.work_ids.insert(runtime_id.clone(), work_id.clone());
        vec![RuntimeEffect::Run {
            work_id,
            runtime_id,
            kind: RuntimeWorkKind::FetchLatest,
        }]
    }

    fn issue_work_id(&mut self) -> String {
        self.issued += 1;
        format!("runtime-{}", self.issued)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::io;
    use std::path::PathBuf;

    struct FakeRuntime {
        id: &'static str,
        name: &'static str,
        path: RefCell<String>,
        managed: bool,
        can_install: bool,
        version: VersionInfo,
        probe_error: Option<&'static str>,
        latest: String,
        latest_error: Option<&'static str>,
        installs: RefCell<Vec<String>>,
        progress: RefCell<Vec<f64>>,
    }

    fn fake(id: &'static str, name: &'static str) -> FakeRuntime {
        FakeRuntime {
            id,
            name,
            path: RefCell::new(String::new()),
            managed: false,
            can_install: true,
            version: VersionInfo {
                version: "1.0".to_string(),
                detail: String::new(),
            },
            probe_error: None,
            latest: String::new(),
            latest_error: None,
            installs: RefCell::new(Vec::new()),
            progress: RefCell::new(Vec::new()),
        }
    }

    impl BinaryRuntime for FakeRuntime {
        fn name(&self) -> &str {
            self.name
        }
        fn can_install(&self) -> bool {
            self.can_install
        }
        fn install_folder(&self) -> PathBuf {
            PathBuf::from("/nonexistent-gd3-runtime")
        }
        fn installed_paths(&self) -> Vec<PathBuf> {
            Vec::new()
        }
        fn runtime_id(&self) -> &'static str {
            self.id
        }
        fn path(&self) -> String {
            self.path.borrow().clone()
        }
        fn is_app_managed(&self) -> bool {
            self.managed
        }
        fn probe_version(&self) -> io::Result<VersionInfo> {
            match self.probe_error {
                Some(message) => Err(io::Error::other(message)),
                None => Ok(self.version.clone()),
            }
        }
        fn fetch_latest_version(&self) -> io::Result<String> {
            match self.latest_error {
                Some(message) => Err(io::Error::other(message)),
                None => Ok(self.latest.clone()),
            }
        }
        fn install(&self, version: &str, on_progress: &mut dyn FnMut(f64)) -> io::Result<()> {
            self.installs.borrow_mut().push(version.to_string());
            on_progress(40.0);
            self.progress.borrow_mut().push(40.0);
            Ok(())
        }
    }

    fn shared(runtime: FakeRuntime) -> Rc<dyn BinaryRuntime> {
        Rc::new(runtime)
    }

    fn version(text: &str) -> VersionInfo {
        VersionInfo {
            version: text.to_string(),
            detail: String::new(),
        }
    }

    #[test]
    fn status_falls_back_to_a_bare_snapshot() {
        let runtime = shared(fake("Fake", "ffmpeg"));
        let service = RuntimeStatusService::new();
        let status = service.status(runtime.as_ref());
        assert_eq!(status, RuntimeStatus::new("Fake", "ffmpeg"));
        assert!(!status.is_busy);
        assert_eq!(service.held_runtimes(), 0);
    }

    #[test]
    fn refresh_status_emits_busy_then_runs_a_probe() {
        let runtime = shared(fake("Fake", "ffmpeg"));
        let mut service = RuntimeStatusService::new();
        let effects = service.refresh_status(&runtime);
        assert_eq!(
            effects,
            vec![
                RuntimeEffect::Emit(RuntimeStatus {
                    is_busy: true,
                    ..RuntimeStatus::new("Fake", "ffmpeg")
                }),
                RuntimeEffect::Run {
                    work_id: "runtime-1".to_string(),
                    runtime_id: "Fake".to_string(),
                    kind: RuntimeWorkKind::Probe,
                },
            ]
        );
        assert_eq!(service.work_id("Fake"), Some("runtime-1"));
        assert_eq!(service.held_runtimes(), 1);
        assert!(service.status(runtime.as_ref()).is_busy);
    }

    #[test]
    fn refresh_status_is_a_no_op_when_the_path_is_unchanged() {
        let runtime = shared(fake("Fake", "ffmpeg"));
        let mut service = RuntimeStatusService::new();
        service.refresh_status(&runtime);
        service.on_probe_finished("Fake", "ffmpeg", "", version("1.0"));
        assert!(service.refresh_status(&runtime).is_empty());
    }

    #[test]
    fn refresh_status_probes_again_after_the_path_moves() {
        let fake = Rc::new(fake("Fake", "ffmpeg"));
        let runtime: Rc<dyn BinaryRuntime> = fake.clone();
        let mut service = RuntimeStatusService::new();
        service.refresh_status(&runtime);
        service.on_probe_finished("Fake", "ffmpeg", "", version("1.0"));

        *fake.path.borrow_mut() = "/usr/bin/ffmpeg".to_string();
        let effects = service.refresh_status(&runtime);
        assert_eq!(effects.len(), 2);
        assert_eq!(
            effects[0],
            RuntimeEffect::Emit(RuntimeStatus {
                path: "/usr/bin/ffmpeg".to_string(),
                is_busy: true,
                ..RuntimeStatus::new("Fake", "ffmpeg")
            })
        );
    }

    #[test]
    fn refresh_status_probes_again_after_a_failed_probe() {
        let runtime = shared(fake("Fake", "ffmpeg"));
        let mut service = RuntimeStatusService::new();
        service.refresh_status(&runtime);
        service.on_probe_failed("Fake", "ffmpeg", "", TaskError::io("boom"));
        assert_eq!(service.refresh_status(&runtime).len(), 2);
    }

    #[test]
    fn on_probe_finished_fetches_the_latest_version_for_managed_runtimes() {
        let mut fake = fake("Fake", "ffmpeg");
        fake.managed = true;
        let runtime = shared(fake);
        let mut service = RuntimeStatusService::new();
        service.refresh_status(&runtime);

        let effects = service.on_probe_finished("Fake", "ffmpeg", "/opt/ffmpeg", version("1.0"));
        assert_eq!(
            effects,
            vec![
                RuntimeEffect::Emit(RuntimeStatus {
                    path: "/opt/ffmpeg".to_string(),
                    version: "1.0".to_string(),
                    ..RuntimeStatus::new("Fake", "ffmpeg")
                }),
                RuntimeEffect::Run {
                    work_id: "runtime-2".to_string(),
                    runtime_id: "Fake".to_string(),
                    kind: RuntimeWorkKind::FetchLatest,
                },
            ]
        );
        assert_eq!(service.held_runtimes(), 1);
        assert_eq!(service.work_id("Fake"), Some("runtime-2"));
    }

    #[test]
    fn on_probe_finished_drops_unmanaged_runtimes() {
        let runtime = shared(fake("Fake", "ffmpeg"));
        let mut service = RuntimeStatusService::new();
        service.refresh_status(&runtime);
        let effects = service.on_probe_finished("Fake", "ffmpeg", "", version("1.0"));
        assert_eq!(effects.len(), 1);
        assert_eq!(service.held_runtimes(), 0);
        assert_eq!(service.work_id("Fake"), None);
    }

    #[test]
    fn on_probe_failed_records_the_error_and_drops_the_runtime() {
        let runtime = shared(fake("Fake", "ffmpeg"));
        let mut service = RuntimeStatusService::new();
        service.refresh_status(&runtime);
        let effects =
            service.on_probe_failed("Fake", "ffmpeg", "/usr/bin/ffmpeg", TaskError::io("boom"));
        assert_eq!(
            effects,
            vec![RuntimeEffect::Emit(RuntimeStatus {
                path: "/usr/bin/ffmpeg".to_string(),
                error: Some(TaskError::io("boom")),
                ..RuntimeStatus::new("Fake", "ffmpeg")
            })]
        );
        assert_eq!(service.held_runtimes(), 0);
        assert_eq!(service.work_id("Fake"), None);
    }

    #[test]
    fn install_emits_installing_then_runs_with_the_latest_version() {
        // 托管运行时：探测完才会去拉最新版本，`runtime-1` 探测、`runtime-2` 拉版本、
        // `runtime-3` 安装——与 `_onProbeFinished` 里 `isAppManaged()` 才 `_fetchLatest` 对齐。
        let mut fake_runtime = fake("Fake", "ffmpeg");
        fake_runtime.managed = true;
        let runtime = shared(fake_runtime);
        let mut service = RuntimeStatusService::new();
        service.refresh_status(&runtime);
        service.on_probe_finished("Fake", "ffmpeg", "", version("1.0"));
        service.on_fetch_finished("Fake", "2.0");

        let effects = service.install(&runtime);
        assert_eq!(
            effects,
            vec![
                RuntimeEffect::Emit(RuntimeStatus {
                    is_installing: true,
                    ..RuntimeStatus::new("Fake", "ffmpeg")
                }),
                RuntimeEffect::Run {
                    work_id: "runtime-3".to_string(),
                    runtime_id: "Fake".to_string(),
                    kind: RuntimeWorkKind::Install {
                        version: "2.0".to_string()
                    },
                },
            ]
        );
        assert!(service.is_installing("Fake"));
    }

    #[test]
    fn install_is_ignored_while_already_installing() {
        let runtime = shared(fake("Fake", "ffmpeg"));
        let mut service = RuntimeStatusService::new();
        assert_eq!(service.install(&runtime).len(), 2);
        assert!(service.install(&runtime).is_empty());
    }

    #[test]
    fn report_progress_only_emits_when_the_integer_progress_changes() {
        let runtime = shared(fake("Fake", "ffmpeg"));
        let mut service = RuntimeStatusService::new();
        service.install(&runtime);

        assert!(service.report_progress("Fake", 0.5).is_empty());
        assert_eq!(
            service.report_progress("Fake", 12.7),
            vec![RuntimeEffect::Emit(RuntimeStatus {
                is_installing: true,
                progress: 12,
                ..RuntimeStatus::new("Fake", "ffmpeg")
            })]
        );
        assert!(service.report_progress("Fake", 12.9).is_empty());
        assert!(service.report_progress("Missing", 50.0).is_empty());
    }

    #[test]
    fn report_progress_ignores_statuses_that_are_not_installing() {
        let runtime = shared(fake("Fake", "ffmpeg"));
        let mut service = RuntimeStatusService::new();
        service.refresh_status(&runtime);
        service.on_probe_finished("Fake", "ffmpeg", "", version("1.0"));
        assert!(service.report_progress("Fake", 50.0).is_empty());
    }

    #[test]
    fn cancel_install_cancels_the_work_then_restores_the_idle_status() {
        let runtime = shared(fake("Fake", "ffmpeg"));
        let mut service = RuntimeStatusService::new();
        service.install(&runtime);

        let effects = service.cancel_install(&runtime);
        assert_eq!(
            effects,
            vec![
                RuntimeEffect::CancelWork("runtime-1".to_string()),
                RuntimeEffect::Emit(RuntimeStatus::new("Fake", "ffmpeg")),
            ]
        );
        assert!(!service.is_installing("Fake"));
        assert_eq!(service.work_id("Fake"), None);
        assert!(service.cancel_install(&runtime).is_empty());
    }

    #[test]
    fn on_install_finished_invalidates_and_probes_again() {
        let runtime = shared(fake("Fake", "ffmpeg"));
        let mut service = RuntimeStatusService::new();
        service.install(&runtime);

        // `on_install_finished` 先摘 workId 再 `invalidate`，所以不会吐 `CancelWork`。
        let effects = service.on_install_finished("Fake");
        assert_eq!(
            effects,
            vec![
                RuntimeEffect::Emit(RuntimeStatus {
                    is_busy: true,
                    ..RuntimeStatus::new("Fake", "ffmpeg")
                }),
                RuntimeEffect::Run {
                    work_id: "runtime-2".to_string(),
                    runtime_id: "Fake".to_string(),
                    kind: RuntimeWorkKind::Probe,
                },
            ]
        );
        assert!(!service.is_installing("Fake"));
    }

    #[test]
    fn on_install_finished_of_an_unknown_runtime_does_nothing() {
        let mut service = RuntimeStatusService::new();
        assert!(service.on_install_finished("Missing").is_empty());
    }

    #[test]
    fn on_install_failed_marks_the_error_and_clears_the_flag() {
        let runtime = shared(fake("Fake", "ffmpeg"));
        let mut service = RuntimeStatusService::new();
        service.install(&runtime);

        let effects = service.on_install_failed("Fake", TaskError::install_not_found("ffmpeg"));
        assert_eq!(
            effects,
            vec![RuntimeEffect::Emit(RuntimeStatus {
                error: Some(TaskError::install_not_found("ffmpeg")),
                ..RuntimeStatus::new("Fake", "ffmpeg")
            })]
        );
        assert!(!service.is_installing("Fake"));
        assert_eq!(service.held_runtimes(), 0);
        // 迟到的失败回调被忽略（Python 的 `if runtimeId not in self._installingIds: return`）。
        assert!(
            service
                .on_install_failed("Fake", TaskError::io("late"))
                .is_empty()
        );
    }

    #[test]
    fn on_fetch_finished_skips_the_emit_for_an_empty_version() {
        let runtime = shared(fake("Fake", "ffmpeg"));
        let mut service = RuntimeStatusService::new();
        service.refresh_status(&runtime);
        service.on_probe_finished("Fake", "ffmpeg", "", version("1.0"));

        assert!(service.on_fetch_finished("Fake", "").is_empty());
        assert_eq!(
            service.on_fetch_finished("Fake", "3.0"),
            vec![RuntimeEffect::Emit(RuntimeStatus {
                version: "1.0".to_string(),
                latest_version: "3.0".to_string(),
                ..RuntimeStatus::new("Fake", "ffmpeg")
            })]
        );
        assert!(service.on_fetch_finished("Missing", "3.0").is_empty());
    }

    #[test]
    fn on_fetch_failed_only_clears_the_bookkeeping() {
        let runtime = shared(fake("Fake", "ffmpeg"));
        let mut service = RuntimeStatusService::new();
        service.refresh_status(&runtime);
        assert_eq!(service.held_runtimes(), 1);

        service.on_fetch_failed("Fake");
        assert_eq!(service.work_id("Fake"), None);
        assert_eq!(service.held_runtimes(), 0);
        assert!(service.status(runtime.as_ref()).is_busy);
    }

    #[test]
    fn check_installed_reports_the_runtime_name() {
        let unmanaged = fake("Fake", "ffmpeg");
        assert_eq!(
            RuntimeStatusService::check_installed(&unmanaged),
            Err(TaskError::install_not_found("ffmpeg"))
        );

        let mut managed = fake("Fake", "ffmpeg");
        managed.managed = true;
        assert_eq!(RuntimeStatusService::check_installed(&managed), Ok(()));
    }

    #[test]
    fn card_status_bridges_to_the_setting_card_snapshot() {
        let runtime = fake("Fake", "ffmpeg");
        let status = RuntimeStatus {
            error: Some(TaskError::io("boom")),
            ..RuntimeStatus::new("Fake", "ffmpeg")
        };
        let view = status.card_view(&runtime, |error| error.format());
        assert_eq!(view.content, "发生了意外错误：boom");
        assert!(view.install_visible);
        assert!(view.refresh_enabled);
    }

    #[test]
    fn the_host_can_drive_install_and_fold_progress_back() {
        let fake = Rc::new(fake("Fake", "ffmpeg"));
        let runtime: Rc<dyn BinaryRuntime> = fake.clone();
        let mut service = RuntimeStatusService::new();
        service.install(&runtime);

        let mut effects = Vec::new();
        {
            let mut report =
                |percent: f64| effects.extend(service.report_progress("Fake", percent));
            runtime.install("2.0", &mut report).expect("install");
        }

        assert_eq!(fake.installs.borrow().as_slice(), ["2.0"]);
        assert_eq!(fake.progress.borrow().as_slice(), [40.0]);
        assert_eq!(
            effects,
            vec![RuntimeEffect::Emit(RuntimeStatus {
                is_installing: true,
                progress: 40,
                ..RuntimeStatus::new("Fake", "ffmpeg")
            })]
        );
        // `managed == false`：装完仍找不到 → 失败（`安装后未找到 {name}`）。
        assert_eq!(
            RuntimeStatusService::check_installed(runtime.as_ref()),
            Err(TaskError::install_not_found("ffmpeg"))
        );
    }

    #[test]
    fn work_ids_are_unique_across_kinds() {
        let mut fake_runtime = fake("Fake", "ffmpeg");
        fake_runtime.managed = true;
        let runtime = shared(fake_runtime);
        let mut service = RuntimeStatusService::new();

        let probe = service.refresh_status(&runtime);
        let fetch = service.on_probe_finished("Fake", "ffmpeg", "", version("1.0"));
        let install = service.install(&runtime);

        let mut ids = Vec::new();
        for effect in [probe, fetch, install].concat() {
            if let RuntimeEffect::Run { work_id, .. } = effect {
                ids.push(work_id);
            }
        }
        assert_eq!(ids, ["runtime-1", "runtime-2", "runtime-3"]);
    }
}
