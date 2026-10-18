//! FeaturePack 的**数据契约**：镜像 `app/models/pack.py` 里不依赖 Qt 的那部分。
//!
//! | Python | Rust |
//! |---|---|
//! | `FileType`（:22-27） | [`FileType`] |
//! | `UriScheme`（:30-33） | [`UriScheme`] |
//! | `VersionInfo`（:114-117） | [`VersionInfo`] |
//! | `PackManifest.fromDir`（:186-247） | [`PackManifest::from_dir`] |
//! | `BinaryRuntime`（:120-177） | [`BinaryRuntime`] |
//! | `deleteInstalled` / `installFile` | [`crate::install`] |
//!
//! ## 没搬的部分（都是「FeaturePack 由 Python 模块实现」这层协议）
//!
//! - `TaskParser` / `PackConfig` / `FeaturePack` / `PackPage` / `PackServices`：
//!   `PackConfig.__init_subclass__` 把子类里的 `ConfigItem` 注册进 `cfg`
//!   （键名前缀 `pack_<类名>_`），`FeaturePack` 的方法返回 Qt 控件 / 信号。
//!   Rust 侧没有「动态 Python 子类」这个概念，插件加载本身留在 Python；
//!   这里只钉住**跨语言要共享的数据形状**（manifest / 文件类型 / URI 方案 /
//!   运行时描述）。
//! - `installArchive`（`app/install.py:27-79`）：要 `zipfile` / `tarfile("r:gz")`
//!   解码，随 `update_service` 一起排期。
//!
//! ## `fromDir` 的精确口径（`app/models/pack.py:186-247`）
//!
//! 校验顺序固定，**先到先错**，每条失败对应一句日志（[`ManifestError`] 的
//! `Display` 逐字照抄 Python 的格式串）：
//!
//! 1. `manifest.toml` 不存在 → `缺少 manifest.toml`；
//! 2. 读 / UTF-8 解码 / TOML 解析失败 → `无法读取 manifest`；
//! 3. 顶层没有 `[pack]` 表（或不是表） → `缺少 [pack] 节`；
//! 4. `pack.entry` 存在但不是字符串，或 `strip()` 后为空 → `entry 无效`；
//! 5. `packDir / entry` 不存在，且 `entry` 以 `.py` 结尾时退到同名 `.pyc` 仍不存在
//!    → `入口文件不存在`（报错里的路径是**原始的** `packDir / entry`，不是 `.pyc`）；
//! 6. `pack.class` 不是字符串或 `strip()` 后为空 → `缺少 class 字段`；
//! 7. `pack.dependencies` 不是数组，或含非字符串 / 空字符串 → `dependencies 无效`；
//! 8. `pack.version` / `pack.gdMinVersion` 不是字符串时**静默**退化成 `""`（不报错）。
//!
//! 注意第 4 条只把 `strip()` 的结果用于**判空**：真正拼路径的是未 strip 的
//! `entry`，所以 `entry = "  pack.py  "` 会去找带空格的文件名（有单测钉住）。
//!
//! ## 与 Python 的已知差异
//!
//! 1. `tomllib.loads` 与 `toml` crate 对**非法文档**的接受面不完全一致（例如
//!    `tomllib` 拒绝 BOM 而 `toml` crate 接受）。差异只影响「坏 manifest 是报
//!    `无法读取` 还是被读出来」，两侧对合法 manifest 的结果一致。
//! 2. `read_text(encoding="utf-8")` 失败时 Python 抛 `UnicodeDecodeError`，这里
//!    归到同一个 `无法读取` 分支；`repr(e)` 的文本无法逐字复刻（`reason` 用
//!    Rust 的 `Display`）。
//! 3. `packDir.name` 对非 UTF-8 目录名：Python 用 `surrogateescape` 原样保留，
//!    Rust 走 `to_string_lossy()`（坏字节变 `U+FFFD`）。
//! 4. `runtimeId` 是 `f"{cls.__module__}.{cls.__qualname__}"`；Rust 用
//!    `std::any::type_name::<Self>()`（形如 `gd3_ui::runtime::FfmpegRuntime`），
//!    两边都是「类型全名」，但**字符串不同**，不能混用作持久化键。
//!    （[`BinaryRuntime::runtime_id`] 在 vtable 里，`&dyn BinaryRuntime` 也能调。）
//! 5. `probeVersion` / `fetchLatestVersion` / `install` 在 Python 是 `async`；
//!    Rust 全是同步阻塞，调用方需要时自己丢到线程里。
//!
//! 规格来源：`app/models/pack.py:22-247`、`app/install.py:15-25`。

use std::any::type_name;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::filesystem::find_executable;
use crate::install::delete_installed;

/// `app/models/pack.py:22-27` `FileType`：某种文件后缀归哪个 pack 管。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileType {
    pub extensions: Vec<String>,
    pub display_name: String,
    pub mime_type: String,
    pub icon: String,
}

/// `app/models/pack.py:30-33` `UriScheme`：某个自定义 scheme 归哪个 pack 管。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UriScheme {
    pub scheme: String,
    pub display_name: String,
}

/// `app/models/pack.py:114-117` `VersionInfo`：`probeVersion()` 的返回。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VersionInfo {
    pub version: String,
    pub detail: String,
}

/// `app/models/pack.py:184-247` `PackManifest`：`manifest.toml` 的解析结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackManifest {
    /// `packDir.name`（**不是** manifest 里的字段）。
    pub name: String,
    /// `pack.class`：入口模块里要实例化的类名。
    pub class_name: String,
    /// `packDir / pack.entry`（可能退到同名 `.pyc`）。
    pub entry_path: PathBuf,
    pub folder: PathBuf,
    pub dependencies: Vec<String>,
    pub version: String,
    pub gd_min_version: String,
}

/// `fromDir` 的七种失败原因，`Display` 逐字镜像 Python 的日志格式串。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManifestError {
    /// `manifest.toml` 不存在。
    Missing { folder: PathBuf },
    /// 读文件 / UTF-8 解码 / TOML 解析失败（Python 的 `无法读取 manifest` 分支）。
    Unreadable { manifest: PathBuf, reason: String },
    /// 顶层没有 `[pack]` 表。
    NoPackSection { manifest: PathBuf },
    /// `pack.entry` 不是字符串或 strip 后为空。
    InvalidEntry { manifest: PathBuf },
    /// 入口文件不存在（含 `.py` → `.pyc` 的退化）。
    MissingEntryFile { path: PathBuf },
    /// `pack.class` 不是字符串或 strip 后为空。
    MissingClass { manifest: PathBuf },
    /// `pack.dependencies` 不是数组，或含非字符串 / 空字符串。
    InvalidDependencies { manifest: PathBuf },
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { folder } => {
                write!(f, "FeaturePack 缺少 manifest.toml: {}", folder.display())
            }
            Self::Unreadable { manifest, reason } => {
                write!(f, "无法读取 manifest {}: {reason}", manifest.display())
            }
            Self::NoPackSection { manifest } => {
                write!(f, "manifest 缺少 [pack] 节: {}", manifest.display())
            }
            Self::InvalidEntry { manifest } => {
                write!(f, "manifest entry 无效: {}", manifest.display())
            }
            Self::MissingEntryFile { path } => {
                write!(f, "入口文件不存在: {}", path.display())
            }
            Self::MissingClass { manifest } => {
                write!(f, "manifest 缺少 class 字段: {}", manifest.display())
            }
            Self::InvalidDependencies { manifest } => {
                write!(f, "manifest dependencies 无效: {}", manifest.display())
            }
        }
    }
}

impl std::error::Error for ManifestError {}

impl PackManifest {
    /// 镜像 `PackManifest.fromDir`：失败一律 `None`（Python 那边也是「log 一条然后
    /// 返回 None」）。要日志文本用 [`PackManifest::from_dir_detailed`]。
    pub fn from_dir(folder: impl AsRef<Path>) -> Option<Self> {
        Self::from_dir_detailed(folder).ok()
    }

    /// 同 [`PackManifest::from_dir`]，但把失败原因带出来（[`ManifestError`] 的
    /// `Display` 就是 Python 那句日志）。
    pub fn from_dir_detailed(folder: impl AsRef<Path>) -> Result<Self, ManifestError> {
        let folder = folder.as_ref();
        let manifest = folder.join("manifest.toml");
        if !manifest.exists() {
            return Err(ManifestError::Missing {
                folder: folder.to_path_buf(),
            });
        }
        let root = read_toml(&manifest).map_err(|reason| ManifestError::Unreadable {
            manifest: manifest.clone(),
            reason,
        })?;
        let pack = root
            .as_table()
            .and_then(|table| table.get("pack"))
            .and_then(toml::Value::as_table)
            .ok_or_else(|| ManifestError::NoPackSection {
                manifest: manifest.clone(),
            })?;

        let entry = match pack.get("entry") {
            None => "pack.py",
            Some(toml::Value::String(text)) => text.as_str(),
            Some(_) => return Err(ManifestError::InvalidEntry { manifest }),
        };
        if py_strip(entry).is_empty() {
            return Err(ManifestError::InvalidEntry { manifest });
        }

        // `.py` → `.pyc` 的退化只发生在**原样**的 entry 上（未 strip）。
        let mut entry_path = folder.join(entry);
        if !entry_path.exists() && entry.ends_with(".py") {
            entry_path = folder.join(format!("{}.pyc", &entry[..entry.len() - 3]));
        }
        if !entry_path.exists() {
            return Err(ManifestError::MissingEntryFile {
                path: folder.join(entry),
            });
        }

        let class_name = match pack.get("class") {
            Some(toml::Value::String(text)) if !py_strip(text).is_empty() => text.clone(),
            _ => return Err(ManifestError::MissingClass { manifest }),
        };

        let dependencies = match pack.get("dependencies") {
            None => Vec::new(),
            Some(toml::Value::Array(items)) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    match item {
                        toml::Value::String(text) if !text.is_empty() => out.push(text.clone()),
                        _ => return Err(ManifestError::InvalidDependencies { manifest }),
                    }
                }
                out
            }
            Some(_) => return Err(ManifestError::InvalidDependencies { manifest }),
        };

        Ok(Self {
            name: folder
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            class_name,
            entry_path,
            folder: folder.to_path_buf(),
            dependencies,
            version: str_field(pack, "version"),
            gd_min_version: str_field(pack, "gdMinVersion"),
        })
    }
}

/// `packSection.get(key, "")`，非字符串静默退化成 `""`（不报错）。
fn str_field(pack: &toml::Table, key: &str) -> String {
    match pack.get(key) {
        Some(toml::Value::String(text)) => text.clone(),
        _ => String::new(),
    }
}

/// `manifestPath.read_text(encoding="utf-8")` + `tomllib.loads`。
fn read_toml(manifest: &Path) -> Result<toml::Value, String> {
    let bytes = fs::read(manifest).map_err(|e| e.to_string())?;
    let text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
    toml::from_str::<toml::Value>(&text).map_err(|e| e.to_string())
}

/// CPython `str.isspace()` 口径（`\x1c`-`\x1f` 也算空白）。
///
/// `gd3-core::hls` 与 `gd3-ui::pystr` 各有一份同样的小工具；抽公共模块会牵动
/// 已有调用点，先按「谁用谁留」处理。
fn py_isspace(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

/// 镜像 CPython `str.strip()`。
fn py_strip(text: &str) -> &str {
    text.trim_matches(py_isspace)
}

/// 镜像 CPython `str.splitlines()` 的行界集合（`\r\n` 算一个）。
pub(crate) fn split_lines(text: &str) -> Vec<String> {
    fn is_boundary(c: char) -> bool {
        matches!(
            c,
            '\n' | '\r'
                | '\x0b'
                | '\x0c'
                | '\x1c'
                | '\x1d'
                | '\x1e'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        )
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if is_boundary(c) {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            lines.push(std::mem::take(&mut current));
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// `bytes.decode("utf-8", errors="ignore")`：丢掉非法序列，**不**插替换字符。
fn decode_ignore(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut rest = bytes;
    loop {
        match std::str::from_utf8(rest) {
            Ok(text) => {
                out.push_str(text);
                break;
            }
            Err(err) => {
                let valid = err.valid_up_to();
                out.push_str(std::str::from_utf8(&rest[..valid]).expect("valid prefix"));
                match err.error_len() {
                    // 尾部被截断的序列：`errors="ignore"` 直接丢掉。
                    Some(len) => rest = &rest[valid + len..],
                    None => break,
                }
            }
        }
    }
    out
}

/// `app/models/pack.py:120-177` `BinaryRuntime` 的基类行为。
///
/// Python 是「类属性 + 抽象方法」；Rust 用 trait 的默认方法给出同样的默认实现。
/// 具体运行时（ffmpeg / yt-dlp …）只需要实现 [`BinaryRuntime::name`]、
/// [`BinaryRuntime::install_folder`]、[`BinaryRuntime::installed_paths`]。
///
/// 保持 object-safe：Python 那边是 `list[BinaryRuntime]`（异构实例），
/// 所以 `Box<dyn BinaryRuntime>` 必须能用。
pub trait BinaryRuntime {
    /// 类属性 `name`：展示名，也是 `binaryName` 为空时的兜底命令名。
    fn name(&self) -> &str;
    /// 类属性 `binaryName`（`""` = 用 `name`）。
    fn binary_name(&self) -> &str {
        ""
    }
    /// 类属性 `canInstall`：是否支持由应用自己下载安装。
    fn can_install(&self) -> bool {
        false
    }
    /// 自描述展示信息（Python 用 `QT_TRANSLATE_NOOP` 声明原文，展示端翻译）。
    fn title(&self) -> &str {
        ""
    }
    fn description(&self) -> &str {
        ""
    }
    /// FluentIcon 成员名，View 侧解析。
    fn icon(&self) -> &str {
        ""
    }
    fn is_recommended(&self) -> bool {
        false
    }

    /// 抽象 `installFolder`：应用自己装的运行时落在哪个目录。
    fn install_folder(&self) -> PathBuf;
    /// 抽象 `installedPaths`：`delete()` 要清掉哪些路径。
    fn installed_paths(&self) -> Vec<PathBuf>;

    /// `runtimeId`：`f"{cls.__module__}.{cls.__qualname__}"` 的 Rust 对应物。
    ///
    /// **进 vtable**：`gd3_ui::runtime_status` 要按 `runtimeId` 记账，而它拿到的
    /// 是 `Rc<dyn BinaryRuntime>`（Python 那边是 `dict[str, BinaryRuntime]`），
    /// 所以不能带 `where Self: Sized`。trait 仍然 object-safe。
    fn runtime_id(&self) -> &'static str {
        type_name::<Self>()
    }

    /// `path()`：先查 `installFolder()`，再查 `PATH`（[`find_executable`]）。
    fn path(&self) -> String {
        let binary = self.binary_name();
        let command = if binary.is_empty() {
            self.name()
        } else {
            binary
        };
        find_executable(&self.install_folder(), command)
    }

    /// `isAppManaged()`：`path()` 非空且落在 `installFolder()` 之内。
    fn is_app_managed(&self) -> bool {
        let path = self.path();
        !path.is_empty() && Path::new(&path).starts_with(self.install_folder())
    }

    /// `probeVersion()`：跑 `<path> --version`，取 stdout 第一行 `.strip()`。
    ///
    /// - `path()` 为空 → `Ok(VersionInfo::default())`（Python 的 `if not path`）；
    /// - 退出码非 0 → `Ok(default)`；
    /// - 起进程失败 → `Err`（Python 那边异常冒到 `_onProbeFailed`）。
    fn probe_version(&self) -> io::Result<VersionInfo> {
        let path = self.path();
        if path.is_empty() {
            return Ok(VersionInfo::default());
        }
        let output = Command::new(&path)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()?;
        if !output.status.success() {
            return Ok(VersionInfo::default());
        }
        let text = decode_ignore(&output.stdout);
        Ok(VersionInfo {
            version: split_lines(&text)
                .first()
                .map(|line| py_strip(line).to_string())
                .unwrap_or_default(),
            detail: String::new(),
        })
    }

    /// `fetchLatestVersion()`：默认 `""`（Python 基类 `return ""`）。
    ///
    /// Python 是 `async`，Rust 同步阻塞；拿不到远端版本不是错误，返回 `Ok("")`。
    fn fetch_latest_version(&self) -> io::Result<String> {
        Ok(String::new())
    }

    /// `install(version, onProgress)`：默认 `Unsupported`（Python 基类
    /// `raise NotImplementedError`）。
    ///
    /// `on_progress` 收 0–100 的百分比（Python `reportProgress(percent: float)`），
    /// 由实现自己决定回调次数；调用方负责把它折进 `RuntimeStatus.progress`。
    fn install(&self, _version: &str, _on_progress: &mut dyn FnMut(f64)) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this runtime cannot install itself",
        ))
    }

    /// `delete()`：逐个 [`delete_installed`]。
    fn delete(&self) -> io::Result<()> {
        for path in self.installed_paths() {
            delete_installed(&path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = env::temp_dir().join(format!("gd3pk-{}-{tag}-{id}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_bytes(path: &Path, body: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, body).expect("write file");
    }

    fn write_text(path: &Path, body: &str) {
        write_bytes(path, body.as_bytes());
    }

    /// 建一个 pack 目录并写入 `manifest.toml`。
    fn pack_dir(tag: &str, manifest: &str) -> TempDir {
        let dir = TempDir::new(tag);
        write_text(&dir.path().join("manifest.toml"), manifest);
        dir
    }

    /// 可执行的 shell 脚本（`find_executable` 要求 x 位）。
    fn write_script(path: &Path, body: &str) {
        write_text(path, body);
        let mut perms = fs::metadata(path).expect("stat").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }

    #[test]
    fn plain_data_types_default_to_empty() {
        assert_eq!(
            FileType::default(),
            FileType {
                extensions: Vec::new(),
                display_name: String::new(),
                mime_type: String::new(),
                icon: String::new(),
            }
        );
        assert_eq!(
            UriScheme::default(),
            UriScheme {
                scheme: String::new(),
                display_name: String::new(),
            }
        );
        // `VersionInfo("")` 的 detail 默认空串。
        assert_eq!(
            VersionInfo::default(),
            VersionInfo {
                version: String::new(),
                detail: String::new(),
            }
        );
    }

    #[test]
    fn manifest_error_display_matches_python_logs() {
        let manifest = Path::new("/tmp/p/manifest.toml");
        assert_eq!(
            ManifestError::Missing {
                folder: PathBuf::from("/tmp/p")
            }
            .to_string(),
            "FeaturePack 缺少 manifest.toml: /tmp/p"
        );
        assert_eq!(
            ManifestError::Unreadable {
                manifest: manifest.to_path_buf(),
                reason: "boom".into()
            }
            .to_string(),
            "无法读取 manifest /tmp/p/manifest.toml: boom"
        );
        assert_eq!(
            ManifestError::NoPackSection {
                manifest: manifest.to_path_buf()
            }
            .to_string(),
            "manifest 缺少 [pack] 节: /tmp/p/manifest.toml"
        );
        assert_eq!(
            ManifestError::InvalidEntry {
                manifest: manifest.to_path_buf()
            }
            .to_string(),
            "manifest entry 无效: /tmp/p/manifest.toml"
        );
        assert_eq!(
            ManifestError::MissingEntryFile {
                path: PathBuf::from("/tmp/p/pack.py")
            }
            .to_string(),
            "入口文件不存在: /tmp/p/pack.py"
        );
        assert_eq!(
            ManifestError::MissingClass {
                manifest: manifest.to_path_buf()
            }
            .to_string(),
            "manifest 缺少 class 字段: /tmp/p/manifest.toml"
        );
        assert_eq!(
            ManifestError::InvalidDependencies {
                manifest: manifest.to_path_buf()
            }
            .to_string(),
            "manifest dependencies 无效: /tmp/p/manifest.toml"
        );
    }

    #[test]
    fn from_dir_requires_manifest_toml() {
        let dir = TempDir::new("no-manifest");
        let err = PackManifest::from_dir_detailed(dir.path()).expect_err("no manifest");
        assert_eq!(
            err,
            ManifestError::Missing {
                folder: dir.path().to_path_buf()
            }
        );
        assert!(PackManifest::from_dir(dir.path()).is_none());
    }

    #[test]
    fn from_dir_rejects_unreadable_manifest() {
        let dir = pack_dir("bad-toml", "this is not toml [");
        let err = PackManifest::from_dir_detailed(dir.path()).expect_err("bad toml");
        assert!(matches!(err, ManifestError::Unreadable { .. }), "{err:?}");

        let dir = pack_dir("bad-utf8", "pack = 1");
        write_bytes(&dir.path().join("manifest.toml"), b"\xff\xfe");
        let err = PackManifest::from_dir_detailed(dir.path()).expect_err("bad utf8");
        assert!(matches!(err, ManifestError::Unreadable { .. }), "{err:?}");
    }

    #[test]
    fn from_dir_requires_pack_table() {
        // 空文件：顶层是空表，没有 `pack`。
        let dir = pack_dir("empty", "");
        assert!(matches!(
            PackManifest::from_dir_detailed(dir.path()),
            Err(ManifestError::NoPackSection { .. })
        ));

        // `pack` 存在但不是表（Python 的 `isinstance(packSection, dict)`）。
        let dir = pack_dir("pack-str", "pack = \"x\"\n");
        assert!(matches!(
            PackManifest::from_dir_detailed(dir.path()),
            Err(ManifestError::NoPackSection { .. })
        ));
    }

    #[test]
    fn from_dir_rejects_non_string_entry() {
        let dir = pack_dir("entry-int", "[pack]\nentry = 1\nclass = \"P\"\n");
        assert!(matches!(
            PackManifest::from_dir_detailed(dir.path()),
            Err(ManifestError::InvalidEntry { .. })
        ));
    }

    #[test]
    fn from_dir_rejects_blank_entry() {
        for (tag, body) in [
            ("entry-empty", "[pack]\nentry = \"\"\nclass = \"P\"\n"),
            ("entry-blank", "[pack]\nentry = \"   \"\nclass = \"P\"\n"),
            ("entry-nbsp", "[pack]\nentry = \"\\u00a0\"\nclass = \"P\"\n"),
        ] {
            let dir = pack_dir(tag, body);
            assert!(
                matches!(
                    PackManifest::from_dir_detailed(dir.path()),
                    Err(ManifestError::InvalidEntry { .. })
                ),
                "{tag}"
            );
        }
    }

    #[test]
    fn from_dir_defaults_entry_to_pack_py() {
        let dir = pack_dir("default-entry", "[pack]\nclass = \"P\"\n");
        write_text(&dir.path().join("pack.py"), "");

        let manifest = PackManifest::from_dir(dir.path()).expect("manifest");
        assert_eq!(manifest.entry_path, dir.path().join("pack.py"));
        assert_eq!(manifest.class_name, "P");
    }

    #[test]
    fn from_dir_falls_back_to_pyc_when_py_is_missing() {
        let dir = pack_dir("pyc", "[pack]\nentry = \"main.py\"\nclass = \"P\"\n");
        write_text(&dir.path().join("main.pyc"), "");

        let manifest = PackManifest::from_dir(dir.path()).expect("manifest");
        assert_eq!(manifest.entry_path, dir.path().join("main.pyc"));
    }

    #[test]
    fn from_dir_prefers_py_over_pyc() {
        let dir = pack_dir("both", "[pack]\nentry = \"main.py\"\nclass = \"P\"\n");
        write_text(&dir.path().join("main.py"), "");
        write_text(&dir.path().join("main.pyc"), "");

        let manifest = PackManifest::from_dir(dir.path()).expect("manifest");
        assert_eq!(manifest.entry_path, dir.path().join("main.py"));
    }

    #[test]
    fn from_dir_reports_original_entry_path_when_pyc_also_missing() {
        let dir = pack_dir(
            "pyc-missing",
            "[pack]\nentry = \"main.py\"\nclass = \"P\"\n",
        );
        // 报错里的路径是 `packDir / entry`，**不是**退化的 `.pyc`。
        assert_eq!(
            PackManifest::from_dir_detailed(dir.path()),
            Err(ManifestError::MissingEntryFile {
                path: dir.path().join("main.py")
            })
        );
    }

    #[test]
    fn from_dir_only_degrades_pyc_for_py_entries() {
        // `.txt` 不做退化：同名 `.txtc` 存在也不算。
        let dir = pack_dir("txt", "[pack]\nentry = \"main.txt\"\nclass = \"P\"\n");
        write_text(&dir.path().join("main.txtc"), "");
        assert!(matches!(
            PackManifest::from_dir_detailed(dir.path()),
            Err(ManifestError::MissingEntryFile { .. })
        ));
    }

    #[test]
    fn from_dir_uses_unstripped_entry_for_the_path() {
        // `strip()` 只用于判空：拼路径用的是原串（带空格）。
        let dir = pack_dir("spaced", "[pack]\nentry = \" pack.py \"\nclass = \"P\"\n");
        assert_eq!(
            PackManifest::from_dir_detailed(dir.path()),
            Err(ManifestError::MissingEntryFile {
                path: dir.path().join(" pack.py ")
            })
        );

        write_text(&dir.path().join(" pack.py "), "");
        let manifest = PackManifest::from_dir(dir.path()).expect("manifest");
        assert_eq!(manifest.entry_path, dir.path().join(" pack.py "));
    }

    #[test]
    fn from_dir_requires_class() {
        for (tag, body) in [
            ("class-missing", "[pack]\nentry = \"pack.py\"\n"),
            ("class-int", "[pack]\nentry = \"pack.py\"\nclass = 1\n"),
            (
                "class-blank",
                "[pack]\nentry = \"pack.py\"\nclass = \"  \"\n",
            ),
        ] {
            let dir = pack_dir(tag, body);
            write_text(&dir.path().join("pack.py"), "");
            assert!(
                matches!(
                    PackManifest::from_dir_detailed(dir.path()),
                    Err(ManifestError::MissingClass { .. })
                ),
                "{tag}"
            );
        }
    }

    #[test]
    fn from_dir_keeps_class_name_verbatim() {
        // 与 entry 不同，`class` 不做 strip（Python 只用 strip 判空）。
        let dir = pack_dir("class-spaced", "[pack]\nclass = \" P \"\n");
        write_text(&dir.path().join("pack.py"), "");
        let manifest = PackManifest::from_dir(dir.path()).expect("manifest");
        assert_eq!(manifest.class_name, " P ");
    }

    #[test]
    fn from_dir_rejects_bad_dependencies() {
        for (tag, body) in [
            ("deps-str", "[pack]\nclass = \"P\"\ndependencies = \"a\"\n"),
            ("deps-int", "[pack]\nclass = \"P\"\ndependencies = [1]\n"),
            (
                "deps-empty",
                "[pack]\nclass = \"P\"\ndependencies = [\"a\", \"\"]\n",
            ),
        ] {
            let dir = pack_dir(tag, body);
            write_text(&dir.path().join("pack.py"), "");
            assert!(
                matches!(
                    PackManifest::from_dir_detailed(dir.path()),
                    Err(ManifestError::InvalidDependencies { .. })
                ),
                "{tag}"
            );
        }
    }

    #[test]
    fn from_dir_keeps_dependency_order() {
        let dir = pack_dir(
            "deps-ok",
            "[pack]\nclass = \"P\"\ndependencies = [\"b\", \"a\", \"c\"]\n",
        );
        write_text(&dir.path().join("pack.py"), "");
        let manifest = PackManifest::from_dir(dir.path()).expect("manifest");
        assert_eq!(manifest.dependencies, vec!["b", "a", "c"]);
    }

    #[test]
    fn from_dir_silently_degrades_non_string_version_fields() {
        let dir = pack_dir(
            "versions",
            "[pack]\nclass = \"P\"\nversion = 1\ngdMinVersion = []\n",
        );
        write_text(&dir.path().join("pack.py"), "");
        let manifest = PackManifest::from_dir(dir.path()).expect("manifest");
        assert_eq!(manifest.version, "");
        assert_eq!(manifest.gd_min_version, "");
    }

    #[test]
    fn from_dir_takes_name_and_folder_from_the_directory() {
        let dir = pack_dir(
            "named",
            "[pack]\nclass = \"P\"\nversion = \"1.2\"\ngdMinVersion = \"4.0\"\n",
        );
        write_text(&dir.path().join("pack.py"), "");
        let manifest = PackManifest::from_dir(dir.path()).expect("manifest");
        assert_eq!(
            manifest.name,
            dir.path().file_name().unwrap().to_string_lossy()
        );
        assert_eq!(manifest.folder, dir.path());
        assert_eq!(manifest.version, "1.2");
        assert_eq!(manifest.gd_min_version, "4.0");
        assert!(manifest.dependencies.is_empty());
    }

    #[test]
    fn from_dir_mirrors_pack_py_full_happy_path() {
        let dir = pack_dir(
            "full",
            r#"
[pack]
entry = "feature.py"
class = "DemoPack"
version = "0.3.1"
gdMinVersion = "4.2.0"
dependencies = ["base"]
"#,
        );
        write_text(&dir.path().join("feature.py"), "");
        let manifest = PackManifest::from_dir(dir.path()).expect("manifest");
        assert_eq!(manifest.class_name, "DemoPack");
        assert_eq!(manifest.entry_path, dir.path().join("feature.py"));
        assert_eq!(manifest.dependencies, vec!["base"]);
        assert_eq!(manifest.version, "0.3.1");
        assert_eq!(manifest.gd_min_version, "4.2.0");
    }

    #[test]
    fn decode_ignore_drops_invalid_sequences() {
        // `errors="ignore"` 丢字节，不插 U+FFFD。
        assert_eq!(decode_ignore(b"a\xffb"), "ab");
        assert_eq!(decode_ignore(b"\xc3\x28"), "(");
        // 尾部被截断的序列整段丢掉。
        assert_eq!(decode_ignore(b"a\xe4\xb8"), "a");
        assert_eq!(decode_ignore(b"ok"), "ok");
        assert_eq!(decode_ignore(b""), "");
        // 多字节字符跨越坏字节时仍然正确切分。
        assert_eq!(decode_ignore(b"\xe4\xb8\xad\xff\xe6\x96\x87"), "中文");
    }

    #[test]
    fn py_strip_matches_cpython() {
        assert_eq!(py_strip(" \t\x1c\u{85}\u{a0}a\u{3000}"), "a");
        assert_eq!(py_strip("a b"), "a b");
        assert_eq!(py_strip(""), "");
        assert!(py_isspace('\x1c'));
        assert!(py_isspace('\x1f'));
        assert!(py_isspace('\u{a0}'));
        assert!(!py_isspace('a'));
    }

    #[test]
    fn split_lines_matches_cpython() {
        // CPython 3.13 实测：`\x0b\x0c\x1c\x1d\x85\u2028` 都是行界。
        let lines = split_lines("a\x0bb\x0cc\x1cd\x1de\u{85}f\u{2028}g");
        assert_eq!(lines, vec!["a", "b", "c", "d", "e", "f", "g"]);
        // `\r\n` 算一个界。
        assert_eq!(split_lines("a\r\nb"), vec!["a", "b"]);
        // 尾部的单个界不产生空行。
        assert_eq!(split_lines("a\n"), vec!["a"]);
        assert!(split_lines("").is_empty());
        // 中间的空行保留。
        assert_eq!(split_lines("a\n\nb"), vec!["a", "", "b"]);
    }

    /// 只用默认实现的最小运行时。
    struct ProbeRuntime {
        folder: PathBuf,
        name: &'static str,
    }

    impl ProbeRuntime {
        fn new(folder: PathBuf, name: &'static str) -> Self {
            Self { folder, name }
        }
    }

    impl BinaryRuntime for ProbeRuntime {
        fn name(&self) -> &str {
            self.name
        }
        fn install_folder(&self) -> PathBuf {
            self.folder.clone()
        }
        fn installed_paths(&self) -> Vec<PathBuf> {
            Vec::new()
        }
    }

    /// 覆盖 `path()`，用来单独验证 `is_app_managed` / `probe_version` 的默认实现。
    struct FixedPathRuntime {
        folder: PathBuf,
        fixed: String,
        installed: Vec<PathBuf>,
    }

    impl BinaryRuntime for FixedPathRuntime {
        fn name(&self) -> &str {
            "gd3-fixed"
        }
        fn install_folder(&self) -> PathBuf {
            self.folder.clone()
        }
        fn installed_paths(&self) -> Vec<PathBuf> {
            self.installed.clone()
        }
        fn path(&self) -> String {
            self.fixed.clone()
        }
    }

    #[test]
    fn binary_runtime_class_attributes_default_to_empty() {
        let runtime = ProbeRuntime::new(PathBuf::from("/tmp/none"), "gd3-probe");
        assert_eq!(runtime.name(), "gd3-probe");
        assert_eq!(runtime.binary_name(), "");
        assert!(!runtime.can_install());
        assert_eq!(runtime.title(), "");
        assert_eq!(runtime.description(), "");
        assert_eq!(runtime.icon(), "");
        assert!(!runtime.is_recommended());
        // `runtimeId` 是类型全名（与 Python 的 module.qualname 对应）。
        assert!(runtime.runtime_id().ends_with("ProbeRuntime"));
    }

    #[test]
    fn binary_runtime_default_lifecycle_is_sync_and_inert() {
        let runtime = ProbeRuntime::new(PathBuf::from("/tmp/none"), "gd3-probe");
        // `fetchLatestVersion()`：基类 `return ""`。
        assert_eq!(runtime.fetch_latest_version().expect("fetch"), "");
        // `install()`：基类 `raise NotImplementedError`。
        let error = runtime
            .install("1.0", &mut |_| {})
            .expect_err("base install is unsupported");
        assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
        // `runtime_id()` 在 vtable 里：`&dyn BinaryRuntime` 也能调。
        let dynamic: &dyn BinaryRuntime = &runtime;
        assert!(dynamic.runtime_id().ends_with("ProbeRuntime"));
    }

    #[test]
    fn binary_runtime_path_prefers_install_folder() {
        let dir = TempDir::new("path");
        let folder = dir.path().join("bin");
        let name = "gd3probe-in-folder";
        write_script(&folder.join(name), "#!/bin/sh\nexit 0\n");

        let runtime = ProbeRuntime::new(folder.clone(), name);
        assert_eq!(
            runtime.path(),
            crate::filesystem::to_posix_path(folder.join(name))
        );
        assert!(runtime.is_app_managed());
    }

    #[test]
    fn binary_runtime_binary_name_wins_over_name() {
        let dir = TempDir::new("alias");
        let folder = dir.path().join("bin");
        let runtime = ProbeRuntime::new(folder.clone(), "gd3-probe-plain");

        struct Aliased {
            inner: ProbeRuntime,
        }
        impl BinaryRuntime for Aliased {
            fn name(&self) -> &str {
                self.inner.name()
            }
            fn binary_name(&self) -> &str {
                "gd3probe-alias"
            }
            fn install_folder(&self) -> PathBuf {
                self.inner.install_folder()
            }
            fn installed_paths(&self) -> Vec<PathBuf> {
                Vec::new()
            }
        }

        write_script(&folder.join("gd3probe-alias"), "#!/bin/sh\nexit 0\n");
        let aliased = Aliased { inner: runtime };
        assert_eq!(
            aliased.path(),
            crate::filesystem::to_posix_path(folder.join("gd3probe-alias"))
        );
    }

    #[test]
    fn binary_runtime_path_is_empty_when_nothing_matches() {
        let dir = TempDir::new("absent");
        // 名字里带进程 id，不会撞上 PATH 里已有的命令。
        let name = "gd3probe-absent-xyzzy";
        let runtime = ProbeRuntime::new(dir.path().to_path_buf(), name);
        assert_eq!(runtime.path(), "");
        assert!(!runtime.is_app_managed());
    }

    #[test]
    fn is_app_managed_rejects_prefix_siblings() {
        // Python 的 `Path.is_relative_to`：`/opt/xy` 不是 `/opt/x` 的子路径。
        let managed = FixedPathRuntime {
            folder: PathBuf::from("/opt/x"),
            fixed: "/opt/xy/ffmpeg".into(),
            installed: Vec::new(),
        };
        assert!(!managed.is_app_managed());

        let inside = FixedPathRuntime {
            folder: PathBuf::from("/opt/x"),
            fixed: "/opt/x/ffmpeg".into(),
            installed: Vec::new(),
        };
        assert!(inside.is_app_managed());

        let empty = FixedPathRuntime {
            folder: PathBuf::from("/opt/x"),
            fixed: String::new(),
            installed: Vec::new(),
        };
        assert!(!empty.is_app_managed());

        // 裸命令名（相对路径）不算「应用自己装的」。
        let bare = FixedPathRuntime {
            folder: PathBuf::from("/opt/x"),
            fixed: "ffmpeg".into(),
            installed: Vec::new(),
        };
        assert!(!bare.is_app_managed());
    }

    #[test]
    fn probe_version_reads_the_first_line() {
        let dir = TempDir::new("probe");
        let folder = dir.path().join("bin");
        let name = "gd3probe-version";
        write_script(
            &folder.join(name),
            "#!/bin/sh\nprintf 'ffmpeg version 7.1\\nCopyright (c) 2000-2024\\n'\n",
        );

        let runtime = ProbeRuntime::new(folder, name);
        let info = runtime.probe_version().expect("probe");
        assert_eq!(info.version, "ffmpeg version 7.1");
        assert_eq!(info.detail, "");
    }

    #[test]
    fn probe_version_strips_the_first_line() {
        let dir = TempDir::new("probe-strip");
        let folder = dir.path().join("bin");
        let name = "gd3probe-strip";
        write_script(&folder.join(name), "#!/bin/sh\nprintf '  1.2.3  \\n'\n");

        let runtime = ProbeRuntime::new(folder, name);
        assert_eq!(runtime.probe_version().expect("probe").version, "1.2.3");
    }

    #[test]
    fn probe_version_returns_default_without_path() {
        let dir = TempDir::new("probe-none");
        let runtime = ProbeRuntime::new(dir.path().to_path_buf(), "gd3probe-absent-xyzzy");
        // `if not path: return VersionInfo("")` —— 不报错。
        assert_eq!(
            runtime.probe_version().expect("probe"),
            VersionInfo::default()
        );
    }

    #[test]
    fn probe_version_returns_default_on_non_zero_exit() {
        let dir = TempDir::new("probe-fail");
        let folder = dir.path().join("bin");
        let name = "gd3probe-fail";
        write_script(&folder.join(name), "#!/bin/sh\nprintf 'boom\\n'\nexit 3\n");

        let runtime = ProbeRuntime::new(folder, name);
        assert_eq!(
            runtime.probe_version().expect("probe"),
            VersionInfo::default()
        );
    }

    #[test]
    fn probe_version_returns_default_when_stdout_is_empty() {
        let dir = TempDir::new("probe-empty");
        let folder = dir.path().join("bin");
        let name = "gd3probe-empty";
        write_script(&folder.join(name), "#!/bin/sh\nexit 0\n");

        let runtime = ProbeRuntime::new(folder, name);
        assert_eq!(
            runtime.probe_version().expect("probe"),
            VersionInfo::default()
        );
    }

    #[test]
    fn probe_version_ignores_invalid_utf8() {
        let dir = TempDir::new("probe-utf8");
        let folder = dir.path().join("bin");
        let name = "gd3probe-utf8";
        // 前置两个 0xFF 字节：`errors="ignore"` 丢掉后剩 ` 1.2.3`。
        write_script(
            &folder.join(name),
            "#!/bin/sh\nprintf '\\377\\377 1.2.3\\n'\n",
        );

        let runtime = ProbeRuntime::new(folder, name);
        assert_eq!(runtime.probe_version().expect("probe").version, "1.2.3");
    }

    #[test]
    fn probe_version_errors_when_spawn_fails() {
        let runtime = FixedPathRuntime {
            folder: PathBuf::from("/tmp"),
            fixed: "/nonexistent/gd3probe-binary".into(),
            installed: Vec::new(),
        };
        // Python 那边 `create_subprocess_exec` 抛 `FileNotFoundError`，冒到 `_onProbeFailed`。
        assert!(runtime.probe_version().is_err());
    }

    #[test]
    fn delete_removes_every_installed_path() {
        let dir = TempDir::new("delete");
        let first = dir.path().join("runtime-a");
        let second = dir.path().join("runtime-b");
        write_script(&first.join("ffmpeg"), "#!/bin/sh\nexit 0\n");
        write_script(&second, "#!/bin/sh\nexit 0\n");

        let runtime = FixedPathRuntime {
            folder: dir.path().to_path_buf(),
            fixed: String::new(),
            installed: vec![first.clone(), second.clone()],
        };
        runtime.delete().expect("delete");

        assert!(!first.exists());
        assert!(!second.exists());
    }

    #[test]
    fn delete_cleans_leftovers_of_previous_runs() {
        let dir = TempDir::new("delete-old");
        let target = dir.path().join("ffmpeg");
        write_script(&target, "#!/bin/sh\nexit 0\n");
        write_script(
            &dir.path().join("ffmpeg.deadbeef.old"),
            "#!/bin/sh\nexit 0\n",
        );

        let runtime = FixedPathRuntime {
            folder: dir.path().to_path_buf(),
            fixed: String::new(),
            installed: vec![target],
        };
        runtime.delete().expect("delete");

        assert!(!dir.path().join("ffmpeg").exists());
        assert!(!dir.path().join("ffmpeg.deadbeef.old").exists());
    }
}
