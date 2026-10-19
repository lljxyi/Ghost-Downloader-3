//! 更新检查与下载的**纯逻辑层**（第 115 步）：镜像 `app/services/update_service.py`(335)
//! 里不碰网络、不碰文件系统、不起进程的那部分。
//!
//! ## 边界
//!
//! Python 的 `UpdateService` 把四件事混在一个类里：①状态表（`_infos` + `_emit` 的
//! 合并语义）、②`versions.json` 的解析与挑选、③HTTP 下载 + 校验 + 解包、
//! ④`subprocess.Popen` 起 updater。本模块搬 ①② 与 ③④ 的**决策**，执行全归宿主：
//!
//! | Python | Rust | 状态 |
//! |---|---|---|
//! | `UpdateState`（`:95-101`） | [`UpdateState`] | 本步 |
//! | `UpdateInfo`（`:104-112`） | [`UpdateInfo`] | 本步 |
//! | `_emit`（`:317-335`） | [`UpdateTable::emit`] / [`EmitFields`] | 本步 |
//! | `_check`（`:139-180`） | [`plan_check`] | 本步 |
//! | `_downloadApp` 的挑选（`:206-258`） | [`plan_app_download`] | 本步 |
//! | `_downloadPack` 的挑选（`:260-274`） | [`plan_pack_download`] | 本步 |
//! | `toChecksum != parseChecksum(sha)` | [`checksum_accepted`] | 本步 |
//! | `apply`（`:129-135`） | [`plan_apply`] | 本步 |
//! | `_startUpdater` 的 argv（`:287-315`） | [`updater_command`] | 本步 |
//! | `installPendingPacks` 的改名（`:83-92`） | [`pending_pack_id`] | 本步 |
//! | `buildPlatformKey`（`:37-38`） | [`build_platform_key`] | 本步 |
//! | `fetchJson` / `fetchReleaseAsset` / `fetchRawFile` | `gd3-core::sources` + `gd3-net` | 第 114 步 |
//! | `toChecksum` 的实际计算 | `gd3-core::checksum` | Phase 5 |
//! | `zipfile` / `tarfile` / `hdiutil` / `Popen` | **未迁移** | 归宿主 |
//!
//! `isNewer` 用 `gd3_core::version::is_newer`（`current, latest` → `latest > current`），
//! `parseChecksum` 用 `gd3_core::checksum::parse_checksum`（取第一个空白分隔字段并小写），
//! 一行都没重写。
//!
//! ## 实证口径（2026-10-19 实读 `app/services/update_service.py`）
//!
//! - **`_emit` 的合并只认五个键**：`replace(current, state=state, **{k: v for k, v in
//!   kwargs.items() if k in ("label", "currentVersion", "latestVersion", "progress",
//!   "error")})`。没传的键**保留旧值**——所以 `_emit("app", IDLE)` 不会把
//!   `label` / `currentVersion` 清空。新建时的缺省值是
//!   `label = targetId`、`currentVersion = VERSION if targetId == "app" else ""`、
//!   `latestVersion = ""`、`progress = 0`、`error = None`。
//! - **`_infos` 是 dict，`apply()` 按插入序遍历**：先跑所有 `READY` 的 pack，
//!   再处理 `app`。`_emit` 对已存在的键**原地更新**，不改变位置。
//! - **`sorted(FEATURES_DIR.iterdir())` 按路径字符串排序**：宿主扫目录时必须排好序
//!   再传进来，否则 pack 的 emit 顺序会漂。
//! - **`patch.get("from") == VERSION` 是精确相等**，不是 `isNewer`——所以补丁只对
//!   「正好是当前版本」的安装生效。
//! - **`sha` 为空串就跳过校验**（`if sha and ...`）；`parseChecksum` 取第一个空白
//!   分隔字段并 `lower()`，所以 `"abc123  app.exe"` 与 `"ABC123"` 等价。
//! - **`gdMinVersion` 的跳过条件是 `gdMin != VERSION and VERSION < gdMin`**：
//!   `not isNewer(remoteGdMin, VERSION)` 即 `VERSION <= gdMin`，再排掉相等。
//! - **`buildPlatformKey` 的两个 map 都是 `KeyError`**（`OS_MAP[sys.platform]` /
//!   `MACHINE_MAP[platform.machine()]`）→ Rust 返回 `None`。
//! - **`tag = f"v{version}"`**：`versions.json` 里的 `version` 不带 `v` 前缀。
//!
//! ## 与 Python 的已知差异
//!
//! - **`data.get("app", {})` 在非 dict 时 Python 抛 `AttributeError`**（`int` 没有
//!   `.get`）；Rust 的 `Value::get` 对非对象返回 `None`，当空表处理。
//! - **`_download` 的 `except Exception → toTaskError(e)` 归宿主**：IO / HTTP 错误
//!   由调用方包成 `TaskError` 再 `emit(FAILED)`，本模块只产出 `TaskError` 文案。
//! - **`_emit` 显式传 `error=None` 与不传在 `replace` 分支有差别**（前者会把
//!   `error` 清空）；Python 侧没有调用点走那条路，Rust 用 `Option<TaskError>`
//!   表示「不覆盖」。
//! - **解包与进程启动不在这里**：`zipfile` / `tarfile("r:xz")` / `hdiutil` /
//!   `subprocess.Popen` 都归宿主（`zip` / `xz2` crate 不在离线缓存里）。

use std::path::{Path, PathBuf};

use serde_json::Value;

use gd3_core::checksum::parse_checksum;
use gd3_core::task_error::TaskError;
use gd3_core::version::is_newer;

/// `STAGING_DIR = APP_DATA_DIR / "update_staging"`（`:31`）的目录名。
pub const STAGING_DIR_NAME: &str = "update_staging";
/// `fetchJson(APP_REPO, "main", "versions.json")`（`:184`）。
pub const VERSIONS_BRANCH: &str = "main";
/// 同上。
pub const VERSIONS_PATH: &str = "versions.json";
/// `STAGING_DIR / "patch.hdiff"`（`:217`）。
pub const PATCH_FILE_NAME: &str = "patch.hdiff";
/// `STAGING_DIR / "app_new"`（`:246`）。
pub const NEW_APP_DIR_NAME: &str = "app_new";
/// `featuresDir.glob("*_pending")`（`:86`）。
pub const PACK_PENDING_SUFFIX: &str = "_pending";
/// `fetchRawFile(..., f"dist/packs/{filename}", ...)`（`:265`）的目录前缀。
pub const PACK_DIST_PREFIX: &str = "dist/packs/";

/// `OS_MAP`（`:33`）。
pub const OS_MAP: [(&str, &str); 3] = [
    ("win32", "Windows"),
    ("darwin", "macOS"),
    ("linux", "Linux"),
];
/// `MACHINE_MAP`（`:34`）。
pub const MACHINE_MAP: [(&str, &str); 4] = [
    ("AMD64", "x86_64"),
    ("x86_64", "x86_64"),
    ("aarch64", "arm64"),
    ("arm64", "arm64"),
];

/// `buildPlatformKey()`（`:39-40`）：`f"{OS_MAP[sys.platform]}-{MACHINE_MAP[machine]}"`。
///
/// 两个查表在 Python 里都是 `KeyError`，这里退化成 `None`。
pub fn build_platform_key(os: &str, machine: &str) -> Option<String> {
    let os = OS_MAP.iter().find(|(key, _)| *key == os).map(|(_, v)| *v)?;
    let machine = MACHINE_MAP
        .iter()
        .find(|(key, _)| *key == machine)
        .map(|(_, v)| *v)?;
    Some(format!("{os}-{machine}"))
}

/// `UpdateState`（`:95-101`）——`IntEnum`，`auto()` 从 1 起。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum UpdateState {
    Idle,
    Checking,
    Available,
    Downloading,
    Ready,
    Failed,
}

impl UpdateState {
    /// `int(UpdateState.X)`：`IDLE` 是 1。
    pub fn as_int(self) -> u8 {
        match self {
            UpdateState::Idle => 1,
            UpdateState::Checking => 2,
            UpdateState::Available => 3,
            UpdateState::Downloading => 4,
            UpdateState::Ready => 5,
            UpdateState::Failed => 6,
        }
    }
}

/// `UpdateInfo`（`:104-112`）：一个目标（`app` 或某个 pack）的更新状态。
#[derive(Clone, Debug, PartialEq)]
pub struct UpdateInfo {
    pub target_id: String,
    pub label: String,
    pub current_version: String,
    pub latest_version: String,
    pub state: UpdateState,
    pub progress: f64,
    pub error: Option<TaskError>,
}

/// `_emit(targetId, state, **kwargs)` 的 kwargs：`None` = 「没传」= 保留旧值。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EmitFields {
    pub label: Option<String>,
    pub current_version: Option<String>,
    pub latest_version: Option<String>,
    pub progress: Option<f64>,
    pub error: Option<TaskError>,
}

impl EmitFields {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn current_version(mut self, version: impl Into<String>) -> Self {
        self.current_version = Some(version.into());
        self
    }

    pub fn latest_version(mut self, version: impl Into<String>) -> Self {
        self.latest_version = Some(version.into());
        self
    }

    pub fn progress(mut self, progress: f64) -> Self {
        self.progress = Some(progress);
        self
    }

    pub fn error(mut self, error: TaskError) -> Self {
        self.error = Some(error);
        self
    }
}

/// `self._infos: dict[str, UpdateInfo]`：保插入序、同键原地更新。
#[derive(Clone, Debug, PartialEq)]
pub struct UpdateTable {
    /// `VERSION`——新建 `app` 那条记录时填进 `currentVersion`。
    app_version: String,
    infos: Vec<UpdateInfo>,
}

impl UpdateTable {
    pub fn new(app_version: impl Into<String>) -> Self {
        Self {
            app_version: app_version.into(),
            infos: Vec::new(),
        }
    }

    pub fn app_version(&self) -> &str {
        &self.app_version
    }

    pub fn get(&self, target_id: &str) -> Option<&UpdateInfo> {
        self.infos.iter().find(|info| info.target_id == target_id)
    }

    /// `self._infos.values()`——按插入序。
    pub fn infos(&self) -> &[UpdateInfo] {
        &self.infos
    }

    pub fn len(&self) -> usize {
        self.infos.len()
    }

    pub fn is_empty(&self) -> bool {
        self.infos.is_empty()
    }

    /// `_emit`（`:317-335`）：不存在就按缺省值新建，存在就只覆盖传了的键。
    pub fn emit(&mut self, target_id: &str, state: UpdateState, fields: EmitFields) -> &UpdateInfo {
        let index = self
            .infos
            .iter()
            .position(|info| info.target_id == target_id);
        match index {
            None => {
                let current_version = fields.current_version.unwrap_or_else(|| {
                    if target_id == "app" {
                        self.app_version.clone()
                    } else {
                        String::new()
                    }
                });
                self.infos.push(UpdateInfo {
                    target_id: target_id.to_string(),
                    label: fields.label.unwrap_or_else(|| target_id.to_string()),
                    current_version,
                    latest_version: fields.latest_version.unwrap_or_default(),
                    state,
                    progress: fields.progress.unwrap_or(0.0),
                    error: fields.error,
                });
                self.infos.last().expect("刚推入")
            }
            Some(index) => {
                let info = &mut self.infos[index];
                info.state = state;
                if let Some(label) = fields.label {
                    info.label = label;
                }
                if let Some(version) = fields.current_version {
                    info.current_version = version;
                }
                if let Some(version) = fields.latest_version {
                    info.latest_version = version;
                }
                if let Some(progress) = fields.progress {
                    info.progress = progress;
                }
                if let Some(error) = fields.error {
                    info.error = Some(error);
                }
                &self.infos[index]
            }
        }
    }

    /// `_download` 的准入（`:190-192`）：`info is None or info.state != AVAILABLE` → 不做。
    pub fn can_download(&self, target_id: &str) -> bool {
        self.get(target_id)
            .is_some_and(|info| info.state == UpdateState::Available)
    }
}

/// 一条「要发出去的 `_emit`」——`plan_check` 的产出，宿主按序喂给 [`UpdateTable::emit`]。
#[derive(Clone, Debug, PartialEq)]
pub struct UpdateStep {
    pub target_id: String,
    pub state: UpdateState,
    pub fields: EmitFields,
}

impl UpdateStep {
    fn new(target_id: impl Into<String>, state: UpdateState, fields: EmitFields) -> Self {
        Self {
            target_id: target_id.into(),
            state,
            fields,
        }
    }
}

/// 本地已安装的 pack，只留 `_check` 用到的三个字段（`PackManifest` 的子集）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalPack {
    pub name: String,
    pub class_name: String,
    pub version: String,
}

impl LocalPack {
    pub fn new(
        name: impl Into<String>,
        class_name: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            class_name: class_name.into(),
            version: version.into(),
        }
    }

    pub fn from_manifest(manifest: &gd3_core::pack::PackManifest) -> Self {
        Self {
            name: manifest.name.clone(),
            class_name: manifest.class_name.clone(),
            version: manifest.version.clone(),
        }
    }
}

/// `_check` 的目录过滤（`:162`）里**与文件系统无关**的那三条：
/// `packDir.is_dir()`、`not packDir.name.startswith(".")`、`not ...endswith("_pending")`。
///
/// `is_dir()` 由宿主在扫目录时保证；名字判定放这里，好让测试钉住。
pub fn pack_dir_candidate(name: &str) -> bool {
    !name.starts_with('.') && !name.ends_with(PACK_PENDING_SUFFIX)
}

/// `_check`（`:139-180`）的完整 emit 序列。
///
/// `data` 为 `None` = `_fetchVersions` 失败（`fetchJson` 抛 `RuntimeError` 被吞）；
/// `local_packs` 必须已按目录名排好序（Python 的 `sorted(FEATURES_DIR.iterdir())`）；
/// `is_android` = `IS_ANDROID`。
pub fn plan_check(
    data: Option<&Value>,
    local_packs: &[LocalPack],
    is_android: bool,
    app_version: &str,
) -> Vec<UpdateStep> {
    let mut steps = vec![UpdateStep::new(
        "app",
        UpdateState::Checking,
        EmitFields::new().label(format!("Ghost Downloader {app_version}")),
    )];

    let Some(data) = data else {
        steps.push(UpdateStep::new(
            "app",
            UpdateState::Idle,
            EmitFields::new().error(versions_failure()),
        ));
        return steps;
    };

    let app_data = data.get("app");
    let latest = app_data
        .and_then(|value| value.get("version"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if !latest.is_empty() && is_newer(app_version, latest) {
        steps.push(UpdateStep::new(
            "app",
            UpdateState::Available,
            EmitFields::new()
                .label(format!("Ghost Downloader {latest}"))
                .latest_version(latest),
        ));
    } else {
        steps.push(UpdateStep::new("app", UpdateState::Idle, EmitFields::new()));
    }

    if is_android {
        return steps;
    }

    let packs_data = data.get("packs");
    for pack in local_packs {
        if pack.version.is_empty() {
            continue;
        }
        let Some(remote) = packs_data.and_then(|value| value.get(&pack.name)) else {
            continue;
        };
        let remote_version = remote.get("version").and_then(Value::as_str).unwrap_or("");
        if remote_version.is_empty() || !is_newer(&pack.version, remote_version) {
            continue;
        }
        let remote_gd_min = remote
            .get("gdMinVersion")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !remote_gd_min.is_empty()
            && !is_newer(remote_gd_min, app_version)
            && remote_gd_min != app_version
        {
            continue;
        }
        steps.push(UpdateStep::new(
            &pack.name,
            UpdateState::Available,
            EmitFields::new()
                .label(format!("{} {remote_version}", pack.class_name))
                .current_version(&pack.version)
                .latest_version(remote_version),
        ));
    }

    steps
}

// ── 下载决策：`_downloadApp` / `_downloadPack` ──────────────────────────────

/// Python 的「真值」：`None` / `False` / `0` / `0.0` / `""` / 空容器都是 falsy。
///
/// `_downloadApp` 有两处靠它分叉：`if patch and patch.get("from") == VERSION`
/// （`patches[platformKey]` 是空 dict 时整条补丁路径不生效）、`if not full: ... FAILED`。
fn py_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

/// `obj.get(key, "")`：非字符串一律当空串。
fn str_field(object: &Value, key: &str) -> String {
    object
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// `Path(name).suffix`——带点、无后缀返回空串（CPython 的 `_splitext` 口径）。
///
/// `"app.tar.xz"` → `".xz"`；`".xz"` → `""`（前导点不算）；`"foo."` → `""`。
fn py_suffix(name: &str) -> &str {
    let base = name.rsplit('/').next().unwrap_or(name);
    match base.rfind('.') {
        Some(index) if index > 0 && index + 1 < base.len() => &base[index..],
        _ => "",
    }
}

/// `_downloadApp` 选中的下载物。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppKind {
    /// `patch["from"] == VERSION`：只下 `patch.hdiff`，不解包。
    Patch,
    /// 完整包：下载 + 解包到 `app_new`。
    Full,
}

/// 完整包的解包方式（`_downloadApp` 的三分支：darwin / `.xz` / 其余）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnpackKind {
    /// `extractDmg`：`hdiutil attach` + `copytree(symlinks=True)`。
    Dmg,
    /// `extractTarXz`：`tarfile.open(archivePath, "r:xz")`。
    TarXz,
    /// `extractZip`：`zipfile.ZipFile(archivePath).extractall`。
    Zip,
}

/// `_downloadApp` 的下载计划。
#[derive(Clone, Debug, PartialEq)]
pub struct AppPlan {
    pub kind: AppKind,
    /// `tag = f"v{version}"`——`versions.json` 的 `version` 不带 `v` 前缀。
    pub tag: String,
    /// `patch["file"]` 或 `full["file"]`：`fetchReleaseAsset` 的 asset 名。
    pub file: String,
    /// `patch.get("sha256", "")` 或 `full.get("sha256", "")`；空串 = 跳过校验。
    pub sha256: String,
    /// 落地文件名：补丁恒为 `patch.hdiff`，完整包就是 `full["file"]`。
    pub staging_name: String,
    /// 仅完整包有；补丁是 `None`。
    pub unpack: Option<UnpackKind>,
}

/// `_downloadApp` 的产物：要么给出下载计划，要么直接 FAILED。
#[derive(Clone, Debug, PartialEq)]
pub enum AppDecision {
    Download(AppPlan),
    Failed(TaskError),
}

/// `_downloadApp`（`:206-258`）的挑选部分。
///
/// `app_data` = `versionsData.get("app")`；`platform_key` 由
/// [`build_platform_key`] 算好后传进来（返回 `None` 时用 [`platform_key_failure`]）；
/// `is_darwin` = `sys.platform == "darwin"`。
pub fn plan_app_download(
    app_data: Option<&Value>,
    app_version: &str,
    platform_key: &str,
    is_darwin: bool,
) -> AppDecision {
    let version = app_data
        .and_then(|value| value.get("version"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let tag = format!("v{version}");

    let patch = app_data
        .and_then(|value| value.get("patches"))
        .and_then(|value| value.get(platform_key));
    // `if patch and ...`：空 dict 的 patch 表在 Python 里是 falsy。
    let patch = match patch {
        Some(value) if py_truthy(value) => Some(value),
        _ => None,
    };
    if let Some(patch) = patch
        && patch.get("from").and_then(Value::as_str) == Some(app_version)
    {
        // `patch["file"]` 的 KeyError 在 Python 里由 `_download` 的
        // `except Exception as e: toTaskError(e)` 兜住。
        let Some(file) = patch.get("file").and_then(Value::as_str) else {
            return AppDecision::Failed(TaskError::io("'file'"));
        };
        return AppDecision::Download(AppPlan {
            kind: AppKind::Patch,
            tag,
            file: file.to_string(),
            sha256: str_field(patch, "sha256"),
            staging_name: PATCH_FILE_NAME.to_string(),
            unpack: None,
        });
    }

    let full = app_data
        .and_then(|value| value.get("full"))
        .and_then(|value| value.get(platform_key));
    // `if not full: ... FAILED`——`None` / `{}` / `""` / `0` 全落这里。
    let full = match full {
        Some(value) if py_truthy(value) => value,
        _ => return AppDecision::Failed(platform_failure()),
    };
    let Some(file) = full.get("file").and_then(Value::as_str) else {
        return AppDecision::Failed(TaskError::io("'file'"));
    };
    let unpack = if is_darwin {
        UnpackKind::Dmg
    } else if py_suffix(file) == ".xz" {
        UnpackKind::TarXz
    } else {
        UnpackKind::Zip
    };
    AppDecision::Download(AppPlan {
        kind: AppKind::Full,
        tag,
        file: file.to_string(),
        sha256: str_field(full, "sha256"),
        staging_name: file.to_string(),
        unpack: Some(unpack),
    })
}

/// `_downloadPack`（`:260-274`）的下载计划。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackPlan {
    pub pack_id: String,
    /// `packData.get("file", f"{packId}.zip")`。
    pub filename: String,
    /// `f"dist/packs/{filename}"`：`fetchRawFile` 的 path 参数。
    pub remote_path: String,
    /// `STAGING_DIR / f"{packId}.zip"` 的文件名（注意与 `filename` 无关）。
    pub staging_name: String,
    /// `packData.get("sha256", "")`；空串 = 跳过校验。
    pub sha256: String,
}

/// `_downloadPack` 的挑选部分——没有失败分支，缺字段全靠缺省值。
pub fn plan_pack_download(packs_data: Option<&Value>, pack_id: &str) -> PackPlan {
    let pack_data = packs_data.and_then(|value| value.get(pack_id));
    let filename = pack_data
        .and_then(|value| value.get("file"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("{pack_id}.zip"));
    let sha256 = pack_data
        .map(|value| str_field(value, "sha256"))
        .unwrap_or_default();
    PackPlan {
        pack_id: pack_id.to_string(),
        remote_path: format!("{PACK_DIST_PREFIX}{filename}"),
        staging_name: format!("{pack_id}.zip"),
        filename,
        sha256,
    }
}

/// `if sha and await toChecksum(...) != parseChecksum(sha)` 的取反。
///
/// `expected_sha` 为空串 = 没有校验和 = 直接接受；否则按 `parse_checksum` 的
/// 口径（第一个空白分隔字段 + 小写）比，大小写不敏感。
pub fn checksum_accepted(expected_sha: &str, actual_hex: &str) -> bool {
    expected_sha.is_empty() || actual_hex.eq_ignore_ascii_case(&parse_checksum(expected_sha))
}

// ── 应用：`apply` / `installPendingPacks` ──────────────────────────────────

/// `apply()`（`:129-135`）要执行的一个动作。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplyAction {
    /// `_applyPack(packId)`：把 `STAGING_DIR/<packId>.zip` 解到
    /// `FEATURES_DIR/<packId>_pending`，解完删 zip。
    Pack(String),
    /// `_startUpdater()`。
    StartUpdater,
}

/// `apply()`：先按插入序跑所有 `READY` 的 pack，最后才处理 `app`。
///
/// 顺序是硬语义——`app` 的 `_startUpdater` 会拉起 updater 顶掉当前进程，
/// 必须排在所有 pack 之后。
pub fn plan_apply(table: &UpdateTable) -> Vec<ApplyAction> {
    let mut actions: Vec<ApplyAction> = table
        .infos()
        .iter()
        .filter(|info| info.state == UpdateState::Ready && info.target_id != "app")
        .map(|info| ApplyAction::Pack(info.target_id.clone()))
        .collect();
    if table
        .get("app")
        .is_some_and(|info| info.state == UpdateState::Ready)
    {
        actions.push(ApplyAction::StartUpdater);
    }
    actions
}

/// `installPendingPacks`（`:83-92`）的改名：`pending.name.removesuffix("_pending")`。
///
/// 名字不以 `_pending` 结尾 → `None`（`glob("*_pending")` 不会给出这种名字）。
/// **与 Python 的差异**：`removesuffix` 对 `"_pending"` 本身返回空串，Python 会
/// `target = featuresDir / ""` == `featuresDir`，`rmtree` 把整个 FEATURES_DIR 删掉；
/// 这里对空 packId 返回 `None`，宿主跳过即可（该分支是纯数据丢失，不复刻）。
pub fn pending_pack_id(name: &str) -> Option<&str> {
    let pack_id = name.strip_suffix(PACK_PENDING_SUFFIX)?;
    if pack_id.is_empty() {
        None
    } else {
        Some(pack_id)
    }
}

// ── updater 进程：`_startUpdater` ──────────────────────────────────────────

/// `subprocess.Popen` 的脱离方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detach {
    /// `creationflags=DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`
    Win32,
    /// `start_new_session=True`（`setsid`）
    Unix,
}

/// `_startUpdater` 拼出来的命令行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdaterCommand {
    pub args: Vec<String>,
    pub detach: Detach,
}

/// `_startUpdater` 的两个「记日志然后 return」的分支。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdaterSkip {
    /// `updater not found: {path}`
    Missing(PathBuf),
    /// `no patch or newDir found for update`
    NoPayload,
}

/// `"updater.exe" if sys.platform == "win32" else "updater"`（`:288`）。
pub fn updater_name(is_win32: bool) -> &'static str {
    if is_win32 { "updater.exe" } else { "updater" }
}

/// `EXECUTABLE_DIR.parent.parent if sys.platform == "darwin" else EXECUTABLE_DIR`（`:294`）。
///
/// macOS 下 `EXECUTABLE_DIR` 是 `X.app/Contents/MacOS`，updater 要替换的是 `X.app`。
/// `Path("/").parent` 在 Python 里仍是 `/`，这里同样兜底成自身。
pub fn app_dir(is_darwin: bool, executable_dir: &Path) -> PathBuf {
    if !is_darwin {
        return executable_dir.to_path_buf();
    }
    let once = executable_dir.parent().unwrap_or(executable_dir);
    once.parent().unwrap_or(once).to_path_buf()
}

/// `_startUpdater`（`:287-315`）的 argv 与脱离方式。
///
/// 三个文件系统判定（`updaterPath.is_file()` / `patchPath.is_file()` /
/// `stagingNewDir.is_dir()`）就地做，因为它们只决定分支、不产生副作用。
/// `interpreter` = `Path(sys.executable).resolve()`（解析归宿主）。
pub fn updater_command(
    is_win32: bool,
    is_darwin: bool,
    executable_dir: &Path,
    staging_dir: &Path,
    pid: u32,
    interpreter: &Path,
) -> Result<UpdaterCommand, UpdaterSkip> {
    let updater_path = executable_dir.join(updater_name(is_win32));
    if !updater_path.is_file() {
        return Err(UpdaterSkip::Missing(updater_path));
    }

    let patch_path = staging_dir.join(PATCH_FILE_NAME);
    let new_dir = staging_dir.join(NEW_APP_DIR_NAME);
    let payload = if patch_path.is_file() {
        patch_path
    } else if new_dir.is_dir() {
        new_dir
    } else {
        return Err(UpdaterSkip::NoPayload);
    };

    let args = vec![
        updater_path.to_string_lossy().into_owned(),
        pid.to_string(),
        app_dir(is_darwin, executable_dir)
            .to_string_lossy()
            .into_owned(),
        interpreter.to_string_lossy().into_owned(),
        payload.to_string_lossy().into_owned(),
    ];
    let detach = if is_win32 {
        Detach::Win32
    } else {
        Detach::Unix
    };
    Ok(UpdaterCommand { args, detach })
}

// ── 失败文案 ──────────────────────────────────────────────────────────────

/// `TaskError("无法获取版本信息")`（`:145`）：`_fetchVersions` 返回 `None`。
pub fn versions_failure() -> TaskError {
    TaskError::new("无法获取版本信息", Vec::new())
}

/// `TaskError("校验失败")`（`:224` / `:243` / `:271`）：`sha256` 对不上。
pub fn checksum_failure() -> TaskError {
    TaskError::new("校验失败", Vec::new())
}

/// `TaskError("当前平台无可用更新")`（`:232`）：`full[platformKey]` 缺失。
pub fn platform_failure() -> TaskError {
    TaskError::new("当前平台无可用更新", Vec::new())
}

/// `TaskError("DMG 中未找到 .app")`（`:72`）。
pub fn dmg_no_app() -> TaskError {
    TaskError::new("DMG 中未找到 .app", Vec::new())
}

/// `RuntimeError(f"hdiutil attach failed ({proc.returncode})")`（`:68`）。
///
/// 它不是 `TaskError`，会走 `toTaskError` 的兜底分支——所以这里直接给
/// `TaskError::io`（`发生了意外错误：{detail}`），与 `toTaskError` 逐字一致。
pub fn hdiutil_failure(returncode: i32) -> TaskError {
    TaskError::io(format!("hdiutil attach failed ({returncode})"))
}

/// `buildPlatformKey()` 抛 `KeyError` 时的 `toTaskError` 形态。
///
/// Python 的 `str(KeyError("i686"))` 是 `'i686'`（带引号），`toTaskError` 再包成
/// `发生了意外错误：'i686'`。哪个 map 先失败就报哪个键：`OS_MAP[sys.platform]`
/// 在 `MACHINE_MAP[platform.machine()]` 之前求值。
pub fn platform_key_failure(os: &str, machine: &str) -> TaskError {
    let missing = if OS_MAP.iter().any(|(key, _)| *key == os) {
        machine
    } else {
        os
    };
    TaskError::io(format!("'{missing}'"))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use serde_json::json;

    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "gd3-ui-update-service-{}-{tag}-{seq}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    /// 一份「app 4.5.0 + 四个 pack」的 `versions.json`。
    fn versions() -> Value {
        json!({
            "app": {"version": "4.5.0"},
            "packs": {
                "pack-a": {"version": "1.1.0"},
                "pack-b": {"version": "2.0.0", "gdMinVersion": "4.6.0"},
                "pack-c": {"version": "3.0.0", "gdMinVersion": "4.4.0"},
                "pack-d": {"version": "4.0.0", "gdMinVersion": "4.3.0"},
            }
        })
    }

    /// `versions.json` 的 `app` 段：一条对得上当前版本的补丁 + 两个完整包。
    fn app_data() -> Value {
        json!({
            "version": "4.5.0",
            "patches": {
                "Windows-x86_64": {"from": "4.4.0", "file": "gd3-patch.hdiff", "sha256": "AB"},
            },
            "full": {
                "Windows-x86_64": {"file": "gd3-win.zip", "sha256": "cd"},
                "Linux-x86_64": {"file": "gd3-linux.zip"},
            }
        })
    }

    fn targets(steps: &[UpdateStep]) -> Vec<(&str, UpdateState)> {
        steps
            .iter()
            .map(|step| (step.target_id.as_str(), step.state))
            .collect()
    }

    #[test]
    fn emit_defaults_for_a_new_app_entry() {
        let mut table = UpdateTable::new("4.4.0");
        let info = table
            .emit(
                "app",
                UpdateState::Checking,
                EmitFields::new().label("Ghost Downloader 4.4.0"),
            )
            .clone();
        assert_eq!(info.target_id, "app");
        assert_eq!(info.label, "Ghost Downloader 4.4.0");
        assert_eq!(info.current_version, "4.4.0");
        assert_eq!(info.latest_version, "");
        assert_eq!(info.state, UpdateState::Checking);
        assert_eq!(info.progress, 0.0);
        assert_eq!(info.error, None);
    }

    #[test]
    fn emit_defaults_a_new_pack_label_to_its_id() {
        let mut table = UpdateTable::new("4.4.0");
        let info = table
            .emit("pack-a", UpdateState::Available, EmitFields::new())
            .clone();
        assert_eq!(info.label, "pack-a");
        assert_eq!(info.current_version, "");
    }

    #[test]
    fn emit_keeps_the_fields_it_was_not_given() {
        let mut table = UpdateTable::new("4.4.0");
        table.emit(
            "app",
            UpdateState::Available,
            EmitFields::new()
                .label("Ghost Downloader 4.5.0")
                .latest_version("4.5.0"),
        );
        let info = table
            .emit(
                "app",
                UpdateState::Downloading,
                EmitFields::new().progress(0.5),
            )
            .clone();
        assert_eq!(info.label, "Ghost Downloader 4.5.0");
        assert_eq!(info.latest_version, "4.5.0");
        assert_eq!(info.current_version, "4.4.0");
        assert_eq!(info.progress, 0.5);
        assert_eq!(info.state, UpdateState::Downloading);
    }

    #[test]
    fn emit_overwrites_only_the_given_fields() {
        let mut table = UpdateTable::new("4.4.0");
        table.emit(
            "app",
            UpdateState::Downloading,
            EmitFields::new().progress(0.25).error(checksum_failure()),
        );
        let info = table
            .emit(
                "app",
                UpdateState::Ready,
                EmitFields::new()
                    .label("Ghost Downloader 4.5.0")
                    .current_version("4.4.1")
                    .latest_version("4.5.0"),
            )
            .clone();
        assert_eq!(info.label, "Ghost Downloader 4.5.0");
        assert_eq!(info.current_version, "4.4.1");
        assert_eq!(info.latest_version, "4.5.0");
        assert_eq!(info.progress, 0.25);
        assert_eq!(info.error, Some(checksum_failure()));
    }

    #[test]
    fn emit_keeps_the_insertion_order() {
        let mut table = UpdateTable::new("4.4.0");
        table.emit("app", UpdateState::Idle, EmitFields::new());
        table.emit("pack-a", UpdateState::Idle, EmitFields::new());
        table.emit("pack-b", UpdateState::Idle, EmitFields::new());
        table.emit("app", UpdateState::Downloading, EmitFields::new());
        assert_eq!(
            table
                .infos()
                .iter()
                .map(|info| info.target_id.as_str())
                .collect::<Vec<_>>(),
            ["app", "pack-a", "pack-b"]
        );
        assert_eq!(table.len(), 3);
        assert!(!table.is_empty());
    }

    #[test]
    fn emit_records_the_error() {
        let mut table = UpdateTable::new("4.4.0");
        let info = table
            .emit(
                "pack-a",
                UpdateState::Failed,
                EmitFields::new().error(checksum_failure()),
            )
            .clone();
        assert_eq!(info.error, Some(checksum_failure()));
        assert_eq!(
            info.error.as_ref().map(TaskError::format),
            Some("校验失败".to_string())
        );
    }

    #[test]
    fn can_download_only_accepts_available() {
        let mut table = UpdateTable::new("4.4.0");
        assert!(!table.can_download("app"));
        table.emit("app", UpdateState::Idle, EmitFields::new());
        assert!(!table.can_download("app"));
        table.emit("app", UpdateState::Available, EmitFields::new());
        assert!(table.can_download("app"));
        table.emit("app", UpdateState::Downloading, EmitFields::new());
        assert!(!table.can_download("app"));
    }

    #[test]
    fn update_state_ints_match_the_int_enum() {
        let states = [
            UpdateState::Idle,
            UpdateState::Checking,
            UpdateState::Available,
            UpdateState::Downloading,
            UpdateState::Ready,
            UpdateState::Failed,
        ];
        assert_eq!(states.map(UpdateState::as_int), [1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn build_platform_key_maps_both_tables() {
        assert_eq!(
            build_platform_key("win32", "AMD64").as_deref(),
            Some("Windows-x86_64")
        );
        assert_eq!(
            build_platform_key("win32", "x86_64").as_deref(),
            Some("Windows-x86_64")
        );
        assert_eq!(
            build_platform_key("darwin", "arm64").as_deref(),
            Some("macOS-arm64")
        );
        assert_eq!(
            build_platform_key("darwin", "aarch64").as_deref(),
            Some("macOS-arm64")
        );
        assert_eq!(
            build_platform_key("linux", "x86_64").as_deref(),
            Some("Linux-x86_64")
        );
    }

    #[test]
    fn build_platform_key_rejects_unknown_inputs() {
        assert_eq!(build_platform_key("freebsd", "AMD64"), None);
        assert_eq!(build_platform_key("linux", "i686"), None);
        assert_eq!(build_platform_key("freebsd", "i686"), None);
    }

    #[test]
    fn platform_key_failure_names_the_first_missing_key() {
        assert_eq!(
            platform_key_failure("freebsd", "i686").format(),
            "发生了意外错误：'freebsd'"
        );
        assert_eq!(
            platform_key_failure("linux", "i686").format(),
            "发生了意外错误：'i686'"
        );
    }

    #[test]
    fn pack_dir_candidate_filters_hidden_and_pending() {
        assert!(pack_dir_candidate("pack-a"));
        assert!(!pack_dir_candidate(".git"));
        assert!(!pack_dir_candidate("pack-a_pending"));
        assert!(!pack_dir_candidate("_pending"));
    }

    #[test]
    fn plan_check_reports_checking_then_the_fetch_failure() {
        let steps = plan_check(None, &[], false, "4.4.0");
        assert_eq!(
            targets(&steps),
            [("app", UpdateState::Checking), ("app", UpdateState::Idle)]
        );
        assert_eq!(
            steps[0].fields.label.as_deref(),
            Some("Ghost Downloader 4.4.0")
        );
        assert_eq!(steps[1].fields.error, Some(versions_failure()));
    }

    #[test]
    fn plan_check_marks_the_app_available() {
        let steps = plan_check(Some(&versions()), &[], false, "4.4.0");
        assert_eq!(
            targets(&steps),
            [
                ("app", UpdateState::Checking),
                ("app", UpdateState::Available)
            ]
        );
        assert_eq!(
            steps[1].fields.label.as_deref(),
            Some("Ghost Downloader 4.5.0")
        );
        assert_eq!(steps[1].fields.latest_version.as_deref(), Some("4.5.0"));
        assert_eq!(steps[1].fields.current_version, None);
    }

    #[test]
    fn plan_check_keeps_the_app_idle_when_the_version_matches() {
        let steps = plan_check(Some(&versions()), &[], false, "4.5.0");
        assert_eq!(steps[1].state, UpdateState::Idle);
        assert_eq!(steps[1].fields, EmitFields::new());
    }

    #[test]
    fn plan_check_ignores_an_empty_latest_version() {
        let data = json!({"app": {"version": ""}});
        let steps = plan_check(Some(&data), &[], false, "4.4.0");
        assert_eq!(steps[1].state, UpdateState::Idle);
        let data = json!({});
        let steps = plan_check(Some(&data), &[], false, "4.4.0");
        assert_eq!(steps[1].state, UpdateState::Idle);
    }

    #[test]
    fn plan_check_returns_before_packs_on_android() {
        let packs = [LocalPack::new("pack-a", "Pack A", "1.0.0")];
        let steps = plan_check(Some(&versions()), &packs, true, "4.4.0");
        assert_eq!(steps.len(), 2);
    }

    #[test]
    fn plan_check_emits_the_available_packs_in_order() {
        let packs = [
            LocalPack::new("pack-a", "Pack A", "1.0.0"),
            LocalPack::new("pack-b", "Pack B", "1.0.0"),
        ];
        let steps = plan_check(Some(&versions()), &packs, false, "4.4.0");
        // pack-b 要求 GD ≥ 4.6.0，当前 4.4.0 → 跳过。
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[2].target_id, "pack-a");
        assert_eq!(steps[2].state, UpdateState::Available);
        assert_eq!(steps[2].fields.label.as_deref(), Some("Pack A 1.1.0"));
        assert_eq!(steps[2].fields.current_version.as_deref(), Some("1.0.0"));
        assert_eq!(steps[2].fields.latest_version.as_deref(), Some("1.1.0"));
    }

    #[test]
    fn plan_check_skips_a_pack_without_a_local_version() {
        let packs = [LocalPack::new("pack-a", "Pack A", "")];
        assert_eq!(
            plan_check(Some(&versions()), &packs, false, "4.4.0").len(),
            2
        );
    }

    #[test]
    fn plan_check_skips_a_pack_missing_from_versions_json() {
        let packs = [LocalPack::new("pack-z", "Pack Z", "1.0.0")];
        assert_eq!(
            plan_check(Some(&versions()), &packs, false, "4.4.0").len(),
            2
        );
    }

    #[test]
    fn plan_check_skips_packs_that_are_not_older() {
        let packs = [
            LocalPack::new("pack-a", "Pack A", "1.1.0"),
            LocalPack::new("pack-a", "Pack A", "9.0.0"),
        ];
        assert_eq!(
            plan_check(Some(&versions()), &packs, false, "4.4.0").len(),
            2
        );
    }

    #[test]
    fn plan_check_skips_a_pack_that_needs_a_newer_app() {
        let packs = [LocalPack::new("pack-b", "Pack B", "1.0.0")];
        assert_eq!(
            plan_check(Some(&versions()), &packs, false, "4.4.0").len(),
            2
        );
    }

    #[test]
    fn plan_check_allows_a_pack_whose_gd_min_version_matches() {
        let packs = [LocalPack::new("pack-c", "Pack C", "1.0.0")];
        let steps = plan_check(Some(&versions()), &packs, false, "4.4.0");
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[2].fields.latest_version.as_deref(), Some("3.0.0"));
    }

    #[test]
    fn plan_check_allows_a_pack_whose_gd_min_version_is_older() {
        let packs = [LocalPack::new("pack-d", "Pack D", "1.0.0")];
        let steps = plan_check(Some(&versions()), &packs, false, "4.4.0");
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[2].fields.latest_version.as_deref(), Some("4.0.0"));
    }

    #[test]
    fn plan_check_ignores_packs_when_versions_json_has_none() {
        let data = json!({"app": {"version": "4.5.0"}});
        let packs = [LocalPack::new("pack-a", "Pack A", "1.0.0")];
        assert_eq!(plan_check(Some(&data), &packs, false, "4.4.0").len(), 2);
    }

    #[test]
    fn plan_app_download_prefers_a_matching_patch() {
        let data = app_data();
        let decision = plan_app_download(Some(&data), "4.4.0", "Windows-x86_64", false);
        let AppDecision::Download(plan) = decision else {
            panic!("应当选中补丁：{decision:?}");
        };
        assert_eq!(plan.kind, AppKind::Patch);
        assert_eq!(plan.tag, "v4.5.0");
        assert_eq!(plan.file, "gd3-patch.hdiff");
        assert_eq!(plan.sha256, "AB");
        assert_eq!(plan.staging_name, "patch.hdiff");
        assert_eq!(plan.unpack, None);
    }

    #[test]
    fn plan_app_download_ignores_a_patch_for_another_version() {
        let data = app_data();
        let decision = plan_app_download(Some(&data), "4.3.0", "Windows-x86_64", false);
        let AppDecision::Download(plan) = decision else {
            panic!("应当退回完整包：{decision:?}");
        };
        assert_eq!(plan.kind, AppKind::Full);
        assert_eq!(plan.file, "gd3-win.zip");
        assert_eq!(plan.sha256, "cd");
        assert_eq!(plan.staging_name, "gd3-win.zip");
    }

    #[test]
    fn plan_app_download_ignores_an_empty_patch_table() {
        let data = json!({
            "version": "4.5.0",
            "patches": {"Windows-x86_64": {}},
            "full": {"Windows-x86_64": {"file": "gd3-win.zip"}},
        });
        let decision = plan_app_download(Some(&data), "4.4.0", "Windows-x86_64", false);
        let AppDecision::Download(plan) = decision else {
            panic!("空 patch 表是 falsy：{decision:?}");
        };
        assert_eq!(plan.kind, AppKind::Full);
    }

    #[test]
    fn plan_app_download_falls_back_to_the_full_archive() {
        let data = app_data();
        let decision = plan_app_download(Some(&data), "4.4.0", "Linux-x86_64", false);
        let AppDecision::Download(plan) = decision else {
            panic!("没有补丁就走完整包：{decision:?}");
        };
        assert_eq!(plan.kind, AppKind::Full);
        assert_eq!(plan.unpack, Some(UnpackKind::Zip));
        assert_eq!(plan.sha256, "");
    }

    #[test]
    fn plan_app_download_unpacks_xz_with_tar() {
        let data = json!({
            "version": "4.5.0",
            "full": {"Linux-x86_64": {"file": "gd3-linux.tar.xz"}},
        });
        let decision = plan_app_download(Some(&data), "4.4.0", "Linux-x86_64", false);
        let AppDecision::Download(plan) = decision else {
            panic!("应当给出完整包：{decision:?}");
        };
        assert_eq!(plan.unpack, Some(UnpackKind::TarXz));
    }

    #[test]
    fn plan_app_download_prefers_the_dmg_on_darwin() {
        let data = json!({
            "version": "4.5.0",
            "full": {"macOS-arm64": {"file": "gd3-mac.zip"}},
        });
        let decision = plan_app_download(Some(&data), "4.4.0", "macOS-arm64", true);
        let AppDecision::Download(plan) = decision else {
            panic!("应当给出完整包：{decision:?}");
        };
        // darwin 分支先判，后缀是 .zip 也走 dmg。
        assert_eq!(plan.unpack, Some(UnpackKind::Dmg));
    }

    #[test]
    fn plan_app_download_fails_without_a_full_archive() {
        let data = app_data();
        let decision = plan_app_download(Some(&data), "4.4.0", "macOS-arm64", false);
        assert_eq!(decision, AppDecision::Failed(platform_failure()));
        // `full` 整段缺失也一样。
        let data = json!({"version": "4.5.0"});
        let decision = plan_app_download(Some(&data), "4.4.0", "Linux-x86_64", false);
        assert_eq!(decision, AppDecision::Failed(platform_failure()));
    }

    #[test]
    fn plan_app_download_reports_a_missing_file_key() {
        let data = json!({
            "version": "4.5.0",
            "patches": {"Linux-x86_64": {"from": "4.4.0"}},
        });
        assert_eq!(
            plan_app_download(Some(&data), "4.4.0", "Linux-x86_64", false),
            AppDecision::Failed(TaskError::io("'file'"))
        );
        let data = json!({
            "version": "4.5.0",
            "full": {"Linux-x86_64": {"sha256": "ab"}},
        });
        assert_eq!(
            plan_app_download(Some(&data), "4.4.0", "Linux-x86_64", false),
            AppDecision::Failed(TaskError::io("'file'"))
        );
    }

    #[test]
    fn plan_pack_download_defaults_the_filename() {
        let plan = plan_pack_download(None, "pack-a");
        assert_eq!(plan.pack_id, "pack-a");
        assert_eq!(plan.filename, "pack-a.zip");
        assert_eq!(plan.remote_path, "dist/packs/pack-a.zip");
        assert_eq!(plan.staging_name, "pack-a.zip");
        assert_eq!(plan.sha256, "");
        // 有 packs 表但没有这个 pack，也是同一份缺省。
        let data = json!({"pack-b": {"version": "1.0.0"}});
        assert_eq!(plan_pack_download(Some(&data), "pack-a"), plan);
    }

    #[test]
    fn plan_pack_download_uses_the_declared_file() {
        let data = json!({"pack-a": {"file": "bundle-2.0.zip", "sha256": "AB CD"}});
        let plan = plan_pack_download(Some(&data), "pack-a");
        assert_eq!(plan.filename, "bundle-2.0.zip");
        assert_eq!(plan.remote_path, "dist/packs/bundle-2.0.zip");
        // 落地名恒为 `<packId>.zip`，与远端文件名无关。
        assert_eq!(plan.staging_name, "pack-a.zip");
        assert_eq!(plan.sha256, "AB CD");
    }

    #[test]
    fn checksum_accepted_skips_an_empty_expectation() {
        assert!(checksum_accepted("", ""));
        assert!(checksum_accepted("", "deadbeef"));
    }

    #[test]
    fn checksum_accepted_compares_case_insensitively() {
        assert!(checksum_accepted("ABCDEF", "abcdef"));
        assert!(checksum_accepted("abcdef", "ABCDEF"));
        // `parseChecksum` 只取第一个空白分隔字段。
        assert!(checksum_accepted("abcdef  gd3-win.zip", "ABCDEF"));
    }

    #[test]
    fn checksum_accepted_rejects_a_mismatch() {
        assert!(!checksum_accepted("abcdef", "abcdee"));
        assert!(!checksum_accepted("abcdef", ""));
    }

    #[test]
    fn plan_apply_runs_the_packs_before_the_updater() {
        let mut table = UpdateTable::new("4.4.0");
        table.emit("app", UpdateState::Ready, EmitFields::new());
        table.emit("pack-a", UpdateState::Ready, EmitFields::new());
        table.emit("pack-b", UpdateState::Ready, EmitFields::new());
        assert_eq!(
            plan_apply(&table),
            vec![
                ApplyAction::Pack("pack-a".to_string()),
                ApplyAction::Pack("pack-b".to_string()),
                ApplyAction::StartUpdater,
            ]
        );
    }

    #[test]
    fn plan_apply_skips_entries_that_are_not_ready() {
        let mut table = UpdateTable::new("4.4.0");
        table.emit("app", UpdateState::Available, EmitFields::new());
        table.emit("pack-a", UpdateState::Downloading, EmitFields::new());
        table.emit("pack-b", UpdateState::Failed, EmitFields::new());
        assert!(plan_apply(&table).is_empty());
    }

    #[test]
    fn plan_apply_needs_an_app_entry_to_start_the_updater() {
        let mut table = UpdateTable::new("4.4.0");
        table.emit("pack-a", UpdateState::Ready, EmitFields::new());
        assert_eq!(
            plan_apply(&table),
            vec![ApplyAction::Pack("pack-a".to_string())]
        );
    }

    #[test]
    fn pending_pack_id_strips_the_suffix() {
        assert_eq!(pending_pack_id("pack-a_pending"), Some("pack-a"));
        assert_eq!(pending_pack_id("pack-a"), None);
        assert_eq!(pending_pack_id("_pending"), None);
        assert_eq!(
            pending_pack_id("pack-a_pending_pending"),
            Some("pack-a_pending")
        );
    }

    #[test]
    fn updater_name_depends_on_the_platform() {
        assert_eq!(updater_name(true), "updater.exe");
        assert_eq!(updater_name(false), "updater");
    }

    #[test]
    fn app_dir_walks_up_two_levels_on_macos() {
        let exe = Path::new("/Applications/Ghost Downloader.app/Contents/MacOS");
        assert_eq!(
            app_dir(true, exe),
            PathBuf::from("/Applications/Ghost Downloader.app")
        );
        assert_eq!(app_dir(false, exe), exe);
        assert_eq!(app_dir(true, Path::new("/")), PathBuf::from("/"));
    }

    /// 造一对 `executable_dir` / `staging_dir`（都为空）。
    fn updater_dirs(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let dir = temp_dir(tag);
        let exe = dir.join("exe");
        std::fs::create_dir_all(&exe).expect("建 exe 目录");
        let staging = dir.join("staging");
        std::fs::create_dir_all(&staging).expect("建 staging 目录");
        (dir, exe, staging)
    }

    fn write_updater(exe_dir: &Path, is_win32: bool) -> PathBuf {
        let path = exe_dir.join(updater_name(is_win32));
        std::fs::write(&path, b"binary").expect("写 updater");
        path
    }

    #[test]
    fn updater_command_prefers_the_patch() {
        let (_dir, exe, staging) = updater_dirs("updater-patch");
        let updater = write_updater(&exe, false);
        std::fs::write(staging.join(PATCH_FILE_NAME), b"diff").expect("写补丁");
        std::fs::create_dir_all(staging.join(NEW_APP_DIR_NAME)).expect("建 app_new");

        let command = updater_command(
            false,
            false,
            &exe,
            &staging,
            4242,
            Path::new("/usr/bin/python3"),
        )
        .expect("应当给出命令");
        assert_eq!(
            command.args,
            vec![
                updater.to_string_lossy().into_owned(),
                "4242".to_string(),
                exe.to_string_lossy().into_owned(),
                "/usr/bin/python3".to_string(),
                staging.join(PATCH_FILE_NAME).to_string_lossy().into_owned(),
            ]
        );
        assert_eq!(command.detach, Detach::Unix);
    }

    #[test]
    fn updater_command_falls_back_to_the_new_dir() {
        let (_dir, exe, staging) = updater_dirs("updater-newdir");
        write_updater(&exe, false);
        std::fs::create_dir_all(staging.join(NEW_APP_DIR_NAME)).expect("建 app_new");

        let command = updater_command(false, false, &exe, &staging, 1, Path::new("/bin/gd3"))
            .expect("应当给出命令");
        let expected = staging
            .join(NEW_APP_DIR_NAME)
            .to_string_lossy()
            .into_owned();
        assert_eq!(command.args.last(), Some(&expected));
    }

    #[test]
    fn updater_command_reports_a_missing_updater() {
        let (_dir, exe, staging) = updater_dirs("updater-missing");
        assert_eq!(
            updater_command(false, false, &exe, &staging, 1, Path::new("/bin/gd3")),
            Err(UpdaterSkip::Missing(exe.join("updater")))
        );
    }

    #[test]
    fn updater_command_reports_a_missing_payload() {
        let (_dir, exe, staging) = updater_dirs("updater-nopayload");
        write_updater(&exe, false);
        assert_eq!(
            updater_command(false, false, &exe, &staging, 1, Path::new("/bin/gd3")),
            Err(UpdaterSkip::NoPayload)
        );
    }

    #[test]
    fn updater_command_detaches_per_platform() {
        let (_dir, exe, staging) = updater_dirs("updater-win32");
        let updater = write_updater(&exe, true);
        std::fs::write(staging.join(PATCH_FILE_NAME), b"diff").expect("写补丁");

        let command = updater_command(
            true,
            false,
            &exe,
            &staging,
            7,
            Path::new("C:\\Python\\python.exe"),
        )
        .expect("应当给出命令");
        assert_eq!(command.detach, Detach::Win32);
        assert_eq!(command.args[0], updater.to_string_lossy().into_owned());
        assert!(command.args[0].ends_with("updater.exe"));
    }
}
