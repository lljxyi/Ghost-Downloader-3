//! `app/loader.py` 的 Rust 对应物（`loadManifest` 除外）。
//!
//! | Python | Rust |
//! |---|---|
//! | `seedPacks`（:17-41） | [`seed_packs`] |
//! | `loadPacks`（:44-68） | [`load_packs`]（只到「排序好的 manifest 列表」） |
//! | `orderedByDependency`（:71-104） | [`ordered_by_dependency`] |
//! | `loadManifest`（:107-140） | **未迁移**（`importlib` + 动态子类，留在 Python） |
//!
//! `loadPacks` 的最后一步是 `[pack for m in ordered if (pack := loadManifest(m, services))]`，
//! 而 `loadManifest` 是「按文件路径加载 Python 模块、`getattr(module, className)`、
//! 再 `PackClass(services)`」——`FeaturePack` 本身就是 Python 类（返回 Qt 控件与信号），
//! Rust 侧没有对应概念。所以这里只搬到**排序好的 manifest** 为止，插件实例化留给 Python。
//!
//! ## `seedPacks` 的精确口径
//!
//! - `seedDir` 不存在，或 `seedDir.resolve() == targetDir.resolve()` → 什么都不做；
//! - `targetDir` 先 `mkdir(parents=True, exist_ok=True)`；
//! - 遍历 `sorted(seedDir.iterdir())`，跳过**非目录**、名字以 `.` 开头、名字以
//!   `_pending` 结尾的项；
//! - 种子侧 manifest 解析失败或 `version` 为空 → 跳过；
//! - 用户侧已有同目录且 manifest 合法、`version` 非空、且
//!   `parseVersion(user) >= parseVersion(seed)` → 保留用户版本；
//! - 否则 `rmtree(target)` + `copytree(seedDir/pack, target)`，并记一条
//!   `种子 Pack 已同步: {name} {version}`。
//!
//! `copytree` 的默认参数是 `symlinks=False`：符号链接会被**解引用**（拷目标的内容），
//! 目录也递归进去。Rust 的 [`copy_tree`] 用 `Path::is_dir()`（跟随链接）+ `fs::copy`
//! （跟随链接）给出同样结果。
//!
//! ## `loadPacks` 的精确口径
//!
//! - `featuresDir` 不存在 → 只 log 一条 warning，返回空列表（**不是错误**）；
//! - 同样的目录名过滤（`.` 开头 / `_pending` 结尾 / 非目录）；
//! - `PackManifest.fromDir` 返回 `None` → 静默跳过（`fromDir` 自己已经 log 过）；
//! - `gdMinVersion` 非空且 `parseVersion(VERSION) < parseVersion(gdMinVersion)` → 跳过并 log
//!   `跳过 FeaturePack {}：需要 GD ≥ {}，当前 {}`；
//! - 最后按 [`ordered_by_dependency`] 排序。
//!
//! ## `orderedByDependency` 的精确口径
//!
//! 深度优先 + 三色标记，**失败只影响顶层那个 manifest**：
//!
//! - 命中 `visited` → 直接返回（重复依赖不会重复入列）；
//! - 命中 `skipped` → `ValueError("{name} 依赖的 FeaturePack 已被跳过")`；
//! - 命中 `visiting` → `ValueError("循环依赖: a -> b -> c -> a")`（环从 `visiting` 里
//!   该名字**首次**出现处开始，末尾再补一次自己）；
//! - 依赖不在 `byName` 里 → `ValueError("{name} 依赖未找到的 FeaturePack: {dep}")`，
//!   注意 `{name}` 是**当前正在访问的节点**，不是顶层那个；
//! - 顶层循环里任何一个异常 → 只把**循环变量那个** `m.name` 记进 `skipped`，然后
//!   `visiting.clear()`（`visited` / `ordered` 里已完成的保持不动）；
//! - 返回前过滤掉 `skipped` 里的名字。
//!
//! 两个 Python 细节照抄：`byName` 是 `{m.name: m for m in manifests}`（**同名后者胜**，
//! 所以同名 manifest 只会入列一次、且入列的是最后一个）；`ordered` 里存的是
//! `byName[name]`（不是循环变量）。
//!
//! ## 与 Python 的已知差异
//!
//! 1. `Path.resolve()` 会解析符号链接；Rust 侧 [`resolve_path`] 先用
//!    `fs::canonicalize`（存在时等价），失败则退回 `std::path::absolute` + 词法归一
//!    （只压 `.` / `..`，不解析链接）。只有「目标目录尚不存在且路径里带符号链接」这种
//!    组合才会与 Python 不同。
//! 2. `copytree` 在 Python 里会 `copystat` 保留目录 mtime/权限；[`copy_tree`] 只保留
//!    文件的权限位（`fs::copy` 自带），目录权限按 umask 新建。
//! 3. 目录项排序：Python 按 `PurePath` 的 `_parts_normcase` 元组比较（POSIX 上等价于
//!    按名字比较码点）；Rust 用 `to_string_lossy()` 后按 `String` 比较（等价于按 UTF-8
//!    字节比较）。合法 UTF-8 下两者一致，非 UTF-8 名字会退化。
//!
//! 规格来源：`app/loader.py:17-104`、`app/update.py:11-31`。

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::pack::PackManifest;
use crate::version::parse_version;

/// `gdMinVersion` 门禁跳过的 pack（对应 `跳过 FeaturePack {}：需要 GD ≥ {}，当前 {}`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionRejection {
    pub name: String,
    /// manifest 里声明的 `gdMinVersion`。
    pub required: String,
}

/// [`scan_packs`] 的结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanReport {
    /// `featuresDir.exists()` 为假（Python 只 log 一条 warning，不算错误）。
    pub dir_missing: bool,
    /// 目录顺序（已按名字排序）下解析成功的 manifest。
    pub manifests: Vec<PackManifest>,
    pub rejected: Vec<VersionRejection>,
}

/// [`ordered_by_dependency`] 的三种失败，`Display` 逐字镜像 Python 的格式串。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DependencyError {
    /// `{name} 依赖的 FeaturePack 已被跳过`
    SkippedDependency { name: String },
    /// `循环依赖: a -> b -> a`
    Cycle { cycle: Vec<String> },
    /// `{name} 依赖未找到的 FeaturePack: {dep}`
    MissingDependency { name: String, dep: String },
}

impl fmt::Display for DependencyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SkippedDependency { name } => {
                write!(f, "{name} 依赖的 FeaturePack 已被跳过")
            }
            Self::Cycle { cycle } => write!(f, "循环依赖: {}", cycle.join(" -> ")),
            Self::MissingDependency { name, dep } => {
                write!(f, "{name} 依赖未找到的 FeaturePack: {dep}")
            }
        }
    }
}

impl std::error::Error for DependencyError {}

/// 一个被跳过的 manifest 及原因（Python 那边是 `logger.opt(exception=e).error(...)`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DependencySkip {
    /// **顶层循环变量**那个 manifest 的名字（不是报错信息里的名字）。
    pub name: String,
    pub error: DependencyError,
}

/// [`ordered_by_dependency`] / [`load_packs`] 的结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OrderReport {
    /// 依赖排序后的列表（已滤掉 `skipped`）。
    pub ordered: Vec<PackManifest>,
    pub skipped: Vec<DependencySkip>,
}

/// [`load_packs`] 的结果：扫描 + 版本门禁 + 依赖排序。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadReport {
    pub dir_missing: bool,
    pub ordered: Vec<PackManifest>,
    pub rejected: Vec<VersionRejection>,
    pub skipped: Vec<DependencySkip>,
}

/// 一个同步完成的种子 pack（对应 `种子 Pack 已同步: {} {}` 那条日志）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeededPack {
    pub name: String,
    pub version: String,
}

/// [`seed_packs`] 的结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SeedReport {
    pub synced: Vec<SeededPack>,
}

/// 镜像 `loadPacks` 的前两步：目录扫描 + `gdMinVersion` 门禁。
pub fn scan_packs(features_dir: &Path, app_version: &str) -> ScanReport {
    let mut report = ScanReport::default();
    let Ok(entries) = sorted_subdirs(features_dir) else {
        report.dir_missing = true;
        return report;
    };
    let app = parse_version(app_version);
    for path in entries {
        let Some(manifest) = PackManifest::from_dir(&path) else {
            continue;
        };
        if !manifest.gd_min_version.is_empty() && app < parse_version(&manifest.gd_min_version) {
            report.rejected.push(VersionRejection {
                name: manifest.name.clone(),
                required: manifest.gd_min_version.clone(),
            });
            continue;
        }
        report.manifests.push(manifest);
    }
    report
}

/// 镜像 `loadPacks` 的完整纯逻辑部分（不含 `loadManifest`）。
pub fn load_packs(features_dir: &Path, app_version: &str) -> LoadReport {
    let scan = scan_packs(features_dir, app_version);
    let order = ordered_by_dependency(&scan.manifests);
    LoadReport {
        dir_missing: scan.dir_missing,
        ordered: order.ordered,
        rejected: scan.rejected,
        skipped: order.skipped,
    }
}

/// 镜像 `orderedByDependency`：深度优先拓扑排序 + 环检测。
///
/// 失败**不**中断整个排序（Python 只把顶层那个 manifest 记进 `skipped`），所以返回
/// 的是「排好的 + 被跳过的」而不是 `Result`。
pub fn ordered_by_dependency(manifests: &[PackManifest]) -> OrderReport {
    // `{m.name: m}`：同名后者胜。
    let mut by_name: HashMap<&str, usize> = HashMap::with_capacity(manifests.len());
    for (index, manifest) in manifests.iter().enumerate() {
        by_name.insert(manifest.name.as_str(), index);
    }

    let mut state = VisitState {
        by_name: &by_name,
        manifests,
        visiting: Vec::new(),
        visited: HashSet::new(),
        skipped: HashSet::new(),
        ordered: Vec::new(),
    };

    let mut report = OrderReport::default();
    for manifest in manifests {
        if let Err(error) = visit(&manifest.name, &mut state) {
            // Python：`skipped.add(m.name)` + `visiting.clear()`。
            state.skipped.insert(manifest.name.clone());
            state.visiting.clear();
            report.skipped.push(DependencySkip {
                name: manifest.name.clone(),
                error,
            });
        }
    }

    report.ordered = state
        .ordered
        .into_iter()
        .filter(|manifest| !state.skipped.contains(&manifest.name))
        .collect();
    report
}

struct VisitState<'a> {
    by_name: &'a HashMap<&'a str, usize>,
    manifests: &'a [PackManifest],
    visiting: Vec<String>,
    visited: HashSet<String>,
    skipped: HashSet<String>,
    ordered: Vec<PackManifest>,
}

fn visit(name: &str, state: &mut VisitState<'_>) -> Result<(), DependencyError> {
    if state.visited.contains(name) {
        return Ok(());
    }
    if state.skipped.contains(name) {
        return Err(DependencyError::SkippedDependency {
            name: name.to_string(),
        });
    }
    if let Some(position) = state.visiting.iter().position(|item| item == name) {
        let mut cycle: Vec<String> = state.visiting[position..].to_vec();
        cycle.push(name.to_string());
        return Err(DependencyError::Cycle { cycle });
    }

    state.visiting.push(name.to_string());
    // 依赖列表要先克隆：`visit` 需要 `&mut state`。
    let Some(&index) = state.by_name.get(name) else {
        // 只有顶层调用可能命中这里（依赖在下面已检查过），Python 那边是 `KeyError`。
        state.visiting.pop();
        return Err(DependencyError::MissingDependency {
            name: name.to_string(),
            dep: name.to_string(),
        });
    };
    let dependencies = state.manifests[index].dependencies.clone();
    for dep in &dependencies {
        if !state.by_name.contains_key(dep.as_str()) {
            // 注意 `name` 是**当前节点**，不是顶层那个。
            return Err(DependencyError::MissingDependency {
                name: name.to_string(),
                dep: dep.clone(),
            });
        }
        visit(dep, state)?;
    }
    state.visiting.pop();
    state.visited.insert(name.to_string());
    state.ordered.push(state.manifests[index].clone());
    Ok(())
}

/// 镜像 `seedPacks`：把随应用分发的种子 pack 同步到用户目录。
///
/// 只返回**同步了哪些**（日志由调用方打）；文件系统错误原样冒出去
/// （Python 那边也没有 `try/except`）。
pub fn seed_packs(seed_dir: &Path, target_dir: &Path) -> io::Result<SeedReport> {
    let mut report = SeedReport::default();
    if !seed_dir.exists() || resolve_path(seed_dir) == resolve_path(target_dir) {
        return Ok(report);
    }
    fs::create_dir_all(target_dir)?;

    for pack_dir in sorted_subdirs(seed_dir)? {
        let Some(seed) = PackManifest::from_dir(&pack_dir) else {
            continue;
        };
        if seed.version.is_empty() {
            continue;
        }

        let target = target_dir.join(&seed.name);
        if target.exists()
            && let Some(user) = PackManifest::from_dir(&target)
            && !user.version.is_empty()
            && parse_version(&user.version) >= parse_version(&seed.version)
        {
            continue;
        }

        if target.exists() {
            fs::remove_dir_all(&target)?;
        }
        copy_tree(&pack_dir, &target)?;
        report.synced.push(SeededPack {
            name: seed.name,
            version: seed.version,
        });
    }
    Ok(report)
}

/// `sorted(dir.iterdir())` 之后的过滤：只要目录、名字不以 `.` 开头、不以 `_pending` 结尾。
///
/// 目录不存在 → `Err`（`seedPacks` 在调用前已判过 `exists()`；`loadPacks` 把 `Err`
/// 当成「目录不存在」）。
fn sorted_subdirs(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        entries.push((name, entry.path()));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(entries
        .into_iter()
        .filter(|(name, path)| {
            path.is_dir() && !name.starts_with('.') && !name.ends_with("_pending")
        })
        .map(|(_, path)| path)
        .collect())
}

/// `shutil.copytree(src, dst)`（默认 `symlinks=False`）。
///
/// `dst` 必须不存在（`copytree` 先 `os.mkdir`）；符号链接按目标内容拷。
fn copy_tree(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// `Path.resolve()` 的近似：能 `canonicalize` 就用它，否则 `absolute` + 词法归一。
fn resolve_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }
    match std::path::absolute(path) {
        Ok(absolute) => lexical_normalize(&absolute),
        Err(_) => path.to_path_buf(),
    }
}

/// 只压 `.` / `..` 与重复分隔符，**不**解析符号链接（`Path.resolve` 的词法部分）。
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = env::temp_dir().join(format!("gd3ld-{}-{tag}-{id}", std::process::id()));
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

    fn write_text(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, body).expect("write file");
    }

    /// 在 `root/<name>` 建一个合法 pack，`extra` 追加到 `[pack]` 表里。
    fn make_pack(root: &Path, name: &str, extra: &str) -> PathBuf {
        let dir = root.join(name);
        write_text(
            &dir.join("manifest.toml"),
            &format!("[pack]\nclass = \"Pack\"\nentry = \"pack.py\"\n{extra}"),
        );
        write_text(&dir.join("pack.py"), "");
        dir
    }

    fn manifest_with(root: &Path, name: &str, version: &str, deps: &[&str]) -> PackManifest {
        let mut extra = String::new();
        if !version.is_empty() {
            extra.push_str(&format!("version = \"{version}\"\n"));
        }
        if !deps.is_empty() {
            let list = deps
                .iter()
                .map(|dep| format!("\"{dep}\""))
                .collect::<Vec<_>>()
                .join(", ");
            extra.push_str(&format!("dependencies = [{list}]\n"));
        }
        let dir = make_pack(root, name, &extra);
        PackManifest::from_dir(&dir).expect("valid manifest")
    }

    fn names(manifests: &[PackManifest]) -> Vec<&str> {
        manifests.iter().map(|m| m.name.as_str()).collect()
    }

    fn synced_names(synced: &[SeededPack]) -> Vec<&str> {
        synced.iter().map(|s| s.name.as_str()).collect()
    }

    // ---------- scan_packs ----------

    #[test]
    fn scan_packs_reports_missing_dir() {
        let root = TempDir::new("scan-missing");
        let report = scan_packs(&root.path().join("nope"), "4.4.0");
        assert!(report.dir_missing);
        assert!(report.manifests.is_empty());
        assert!(report.rejected.is_empty());
    }

    #[test]
    fn scan_packs_filters_non_dirs_dot_names_and_pending() {
        let root = TempDir::new("scan-filter");
        make_pack(root.path(), "keep", "");
        make_pack(root.path(), "later_pending", "");
        make_pack(root.path(), ".hidden", "");
        write_text(&root.path().join("plain.txt"), "x");

        let report = scan_packs(root.path(), "4.4.0");
        assert!(!report.dir_missing);
        assert_eq!(names(&report.manifests), vec!["keep"]);
    }

    #[test]
    fn scan_packs_sorts_by_name() {
        let root = TempDir::new("scan-sort");
        for name in ["zeta", "alpha", "mid"] {
            make_pack(root.path(), name, "");
        }
        let report = scan_packs(root.path(), "4.4.0");
        assert_eq!(names(&report.manifests), vec!["alpha", "mid", "zeta"]);
    }

    #[test]
    fn scan_packs_enforces_gd_min_version() {
        let root = TempDir::new("scan-gate");
        make_pack(root.path(), "no_min", "");
        make_pack(root.path(), "same", "gdMinVersion = \"4.4.0\"\n");
        make_pack(root.path(), "lower", "gdMinVersion = \"4.3.9\"\n");
        make_pack(root.path(), "higher", "gdMinVersion = \"4.4.1\"\n");
        make_pack(root.path(), "much_higher", "gdMinVersion = \"5.0\"\n");

        let report = scan_packs(root.path(), "4.4.0");
        assert_eq!(names(&report.manifests), vec!["lower", "no_min", "same"]);
        assert_eq!(
            report.rejected,
            vec![
                VersionRejection {
                    name: "higher".to_string(),
                    required: "4.4.1".to_string(),
                },
                VersionRejection {
                    name: "much_higher".to_string(),
                    required: "5.0".to_string(),
                },
            ]
        );
    }

    #[test]
    fn scan_packs_treats_empty_gd_min_version_as_no_gate() {
        let root = TempDir::new("scan-empty-gate");
        make_pack(root.path(), "empty", "gdMinVersion = \"\"\n");
        let report = scan_packs(root.path(), "0.0.0");
        assert_eq!(names(&report.manifests), vec!["empty"]);
        assert!(report.rejected.is_empty());
    }

    #[test]
    fn scan_packs_silently_drops_broken_manifests() {
        let root = TempDir::new("scan-broken");
        make_pack(root.path(), "good", "");
        // 没有 manifest.toml。
        fs::create_dir_all(root.path().join("no_manifest")).expect("mkdir");
        // 不是合法 TOML。
        write_text(&root.path().join("bad_toml/manifest.toml"), "[pack\n");
        // 缺 class。
        write_text(
            &root.path().join("no_class/manifest.toml"),
            "[pack]\nentry = \"pack.py\"\n",
        );
        write_text(&root.path().join("no_class/pack.py"), "");
        // entry 文件不存在。
        write_text(
            &root.path().join("no_entry/manifest.toml"),
            "[pack]\nclass = \"P\"\nentry = \"pack.py\"\n",
        );

        let report = scan_packs(root.path(), "4.4.0");
        assert_eq!(names(&report.manifests), vec!["good"]);
        assert!(report.rejected.is_empty());
    }

    #[test]
    fn scan_packs_reads_version_and_dependencies() {
        let root = TempDir::new("scan-fields");
        make_pack(
            root.path(),
            "p",
            "version = \"1.2.3\"\ndependencies = [\"a\", \"b\"]\n",
        );
        let report = scan_packs(root.path(), "4.4.0");
        let manifest = &report.manifests[0];
        assert_eq!(manifest.name, "p");
        assert_eq!(manifest.class_name, "Pack");
        assert_eq!(manifest.version, "1.2.3");
        assert_eq!(manifest.dependencies, vec!["a", "b"]);
    }

    // ---------- ordered_by_dependency ----------

    #[test]
    fn orders_a_linear_chain() {
        let root = TempDir::new("order-chain");
        let a = manifest_with(root.path(), "a", "", &[]);
        let b = manifest_with(root.path(), "b", "", &["a"]);
        let c = manifest_with(root.path(), "c", "", &["b"]);
        let report = ordered_by_dependency(&[c, b, a]);
        assert_eq!(names(&report.ordered), vec!["a", "b", "c"]);
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn dedupes_a_diamond() {
        let root = TempDir::new("order-diamond");
        let a = manifest_with(root.path(), "a", "", &[]);
        let b = manifest_with(root.path(), "b", "", &["a"]);
        let c = manifest_with(root.path(), "c", "", &["a"]);
        let d = manifest_with(root.path(), "d", "", &["b", "c"]);
        let report = ordered_by_dependency(&[d, c, b, a]);
        assert_eq!(names(&report.ordered), vec!["a", "b", "c", "d"]);
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn independent_packs_keep_input_order() {
        let root = TempDir::new("order-stable");
        let b = manifest_with(root.path(), "b", "", &[]);
        let a = manifest_with(root.path(), "a", "", &[]);
        let report = ordered_by_dependency(&[b, a]);
        assert_eq!(names(&report.ordered), vec!["b", "a"]);
    }

    #[test]
    fn empty_input_yields_empty_report() {
        let report = ordered_by_dependency(&[]);
        assert!(report.ordered.is_empty());
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn self_dependency_is_reported_as_a_cycle() {
        let root = TempDir::new("order-self");
        let a = manifest_with(root.path(), "a", "", &["a"]);
        let report = ordered_by_dependency(&[a]);
        assert!(report.ordered.is_empty());
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].name, "a");
        assert_eq!(
            report.skipped[0].error,
            DependencyError::Cycle {
                cycle: vec!["a".to_string(), "a".to_string()],
            }
        );
        assert_eq!(report.skipped[0].error.to_string(), "循环依赖: a -> a");
    }

    #[test]
    fn two_node_cycle_skips_both_sides() {
        let root = TempDir::new("order-cycle2");
        let a = manifest_with(root.path(), "a", "", &["b"]);
        let b = manifest_with(root.path(), "b", "", &["a"]);
        let report = ordered_by_dependency(&[a, b]);
        assert!(report.ordered.is_empty());
        assert_eq!(report.skipped.len(), 2);
        assert_eq!(report.skipped[0].name, "a");
        assert_eq!(
            report.skipped[0].error,
            DependencyError::Cycle {
                cycle: vec!["a".to_string(), "b".to_string(), "a".to_string()],
            }
        );
        // 第二轮 b：依赖 a 已被标 skipped → 失败原因变成 SkippedDependency。
        assert_eq!(report.skipped[1].name, "b");
        assert_eq!(
            report.skipped[1].error,
            DependencyError::SkippedDependency {
                name: "a".to_string(),
            }
        );
    }

    #[test]
    fn cycle_starts_at_the_first_occurrence_of_the_repeated_name() {
        let root = TempDir::new("order-cycle3");
        let a = manifest_with(root.path(), "a", "", &["b"]);
        let b = manifest_with(root.path(), "b", "", &["c"]);
        let c = manifest_with(root.path(), "c", "", &["b"]);
        let report = ordered_by_dependency(&[a, b, c]);
        assert_eq!(report.skipped.len(), 3);
        assert_eq!(report.skipped[0].name, "a");
        // 环从 `visiting` 里 b 首次出现处开始，末尾再补一次 b。
        assert_eq!(report.skipped[0].error.to_string(), "循环依赖: b -> c -> b");
    }

    #[test]
    fn missing_dependency_names_the_current_node() {
        let root = TempDir::new("order-missing");
        let a = manifest_with(root.path(), "a", "", &["ghost"]);
        let report = ordered_by_dependency(&[a]);
        assert!(report.ordered.is_empty());
        assert_eq!(report.skipped[0].name, "a");
        assert_eq!(
            report.skipped[0].error,
            DependencyError::MissingDependency {
                name: "a".to_string(),
                dep: "ghost".to_string(),
            }
        );
        assert_eq!(
            report.skipped[0].error.to_string(),
            "a 依赖未找到的 FeaturePack: ghost"
        );
    }

    #[test]
    fn missing_dependency_in_a_nested_node_reports_the_nested_name() {
        let root = TempDir::new("order-missing-nested");
        let a = manifest_with(root.path(), "a", "", &["b"]);
        let b = manifest_with(root.path(), "b", "", &["ghost"]);
        let report = ordered_by_dependency(&[a, b]);
        assert_eq!(report.skipped[0].name, "a");
        assert_eq!(
            report.skipped[0].error.to_string(),
            "b 依赖未找到的 FeaturePack: ghost"
        );
        // b 自己也是顶层，依赖 ghost 仍然缺失。
        assert_eq!(report.skipped[1].name, "b");
        assert_eq!(
            report.skipped[1].error.to_string(),
            "b 依赖未找到的 FeaturePack: ghost"
        );
    }

    #[test]
    fn skipped_dependency_propagates_to_dependents() {
        let root = TempDir::new("order-skip-prop");
        let a = manifest_with(root.path(), "a", "", &["ghost"]);
        let b = manifest_with(root.path(), "b", "", &["a"]);
        let report = ordered_by_dependency(&[a, b]);
        assert!(report.ordered.is_empty());
        assert_eq!(report.skipped.len(), 2);
        assert_eq!(
            report.skipped[1].error,
            DependencyError::SkippedDependency {
                name: "a".to_string(),
            }
        );
        assert_eq!(
            report.skipped[1].error.to_string(),
            "a 依赖的 FeaturePack 已被跳过"
        );
    }

    #[test]
    fn duplicate_names_keep_the_last_manifest() {
        let left = TempDir::new("order-dup-l");
        let right = TempDir::new("order-dup-r");
        let first = manifest_with(left.path(), "a", "1.0", &[]);
        let second = manifest_with(right.path(), "a", "2.0", &[]);
        let b = manifest_with(left.path(), "b", "", &["a"]);
        let report = ordered_by_dependency(&[first, second, b]);
        assert_eq!(names(&report.ordered), vec!["a", "b"]);
        // 同名后者胜：入列的是第二个 manifest。
        assert_eq!(report.ordered[0].version, "2.0");
    }

    // ---------- load_packs ----------

    #[test]
    fn load_packs_combines_scan_and_order() {
        let root = TempDir::new("load-all");
        make_pack(root.path(), "base", "version = \"1.0\"\n");
        make_pack(
            root.path(),
            "extra",
            "version = \"1.0\"\ndependencies = [\"base\"]\n",
        );
        make_pack(root.path(), "future", "gdMinVersion = \"9.9\"\n");

        let report = load_packs(root.path(), "4.4.0");
        assert!(!report.dir_missing);
        assert_eq!(names(&report.ordered), vec!["base", "extra"]);
        assert_eq!(report.rejected.len(), 1);
        assert_eq!(report.rejected[0].name, "future");
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn load_packs_reports_missing_dir_without_error() {
        let root = TempDir::new("load-missing");
        let report = load_packs(&root.path().join("absent"), "4.4.0");
        assert!(report.dir_missing);
        assert!(report.ordered.is_empty());
        assert!(report.rejected.is_empty());
    }

    // ---------- seed_packs ----------

    #[test]
    fn seeds_a_missing_target_pack() {
        let root = TempDir::new("seed-new");
        let seed = root.path().join("seed");
        let target = root.path().join("user");
        make_pack(&seed, "p", "version = \"1.0\"\n");
        fs::create_dir_all(&target).expect("mkdir target");

        let report = seed_packs(&seed, &target).expect("seed");
        assert_eq!(
            report.synced,
            vec![SeededPack {
                name: "p".to_string(),
                version: "1.0".to_string(),
            }]
        );
        assert!(target.join("p/manifest.toml").is_file());
        assert!(target.join("p/pack.py").is_file());
    }

    #[test]
    fn creates_the_target_dir_when_absent() {
        let root = TempDir::new("seed-mkdir");
        let seed = root.path().join("seed");
        let target = root.path().join("user/deep");
        make_pack(&seed, "p", "version = \"1.0\"\n");

        let report = seed_packs(&seed, &target).expect("seed");
        assert_eq!(synced_names(&report.synced), vec!["p"]);
        assert!(target.join("p/manifest.toml").is_file());
    }

    #[test]
    fn missing_seed_dir_is_a_noop() {
        let root = TempDir::new("seed-absent");
        let target = root.path().join("user");
        let report = seed_packs(&root.path().join("absent"), &target).expect("seed");
        assert!(report.synced.is_empty());
        // Python 在 `mkdir` 之前就 return 了，目标目录不会被创建。
        assert!(!target.exists());
    }

    #[test]
    fn same_source_and_target_is_a_noop() {
        let root = TempDir::new("seed-same");
        make_pack(root.path(), "p", "version = \"1.0\"\n");
        let report = seed_packs(root.path(), root.path()).expect("seed");
        assert!(report.synced.is_empty());
    }

    #[test]
    fn keeps_a_newer_user_version() {
        let root = TempDir::new("seed-user-newer");
        let seed = root.path().join("seed");
        let target = root.path().join("user");
        make_pack(&seed, "p", "version = \"1.0\"\n");
        make_pack(&target, "p", "version = \"1.5\"\n");
        write_text(&target.join("p/user_marker.txt"), "keep me");

        let report = seed_packs(&seed, &target).expect("seed");
        assert!(report.synced.is_empty());
        assert!(target.join("p/user_marker.txt").is_file());
    }

    #[test]
    fn keeps_an_equal_user_version() {
        let root = TempDir::new("seed-user-equal");
        let seed = root.path().join("seed");
        let target = root.path().join("user");
        make_pack(&seed, "p", "version = \"1.0\"\n");
        make_pack(&target, "p", "version = \"1.0.0\"\n");
        write_text(&target.join("p/user_marker.txt"), "keep me");

        // `parseVersion` 去掉尾部 0 → 两侧相等 → `>=` 成立 → 保留用户版本。
        let report = seed_packs(&seed, &target).expect("seed");
        assert!(report.synced.is_empty());
        assert!(target.join("p/user_marker.txt").is_file());
    }

    #[test]
    fn overwrites_an_older_user_version() {
        let root = TempDir::new("seed-user-older");
        let seed = root.path().join("seed");
        let target = root.path().join("user");
        make_pack(&seed, "p", "version = \"1.0\"\n");
        make_pack(&target, "p", "version = \"0.9\"\n");
        write_text(&target.join("p/user_marker.txt"), "drop me");

        let report = seed_packs(&seed, &target).expect("seed");
        assert_eq!(synced_names(&report.synced), vec!["p"]);
        assert!(!target.join("p/user_marker.txt").exists());
        let user = PackManifest::from_dir(target.join("p")).expect("user manifest");
        assert_eq!(user.version, "1.0");
    }

    #[test]
    fn overwrites_when_the_user_manifest_is_broken() {
        let root = TempDir::new("seed-user-broken");
        let seed = root.path().join("seed");
        let target = root.path().join("user");
        make_pack(&seed, "p", "version = \"1.0\"\n");
        fs::create_dir_all(target.join("p")).expect("mkdir");
        write_text(&target.join("p/manifest.toml"), "[pack\n");
        write_text(&target.join("p/pack.py"), "");

        let report = seed_packs(&seed, &target).expect("seed");
        assert_eq!(synced_names(&report.synced), vec!["p"]);
        let user = PackManifest::from_dir(target.join("p")).expect("user manifest");
        assert_eq!(user.version, "1.0");
    }

    #[test]
    fn overwrites_when_the_user_version_is_empty() {
        let root = TempDir::new("seed-user-noversion");
        let seed = root.path().join("seed");
        let target = root.path().join("user");
        make_pack(&seed, "p", "version = \"1.0\"\n");
        make_pack(&target, "p", "");

        let report = seed_packs(&seed, &target).expect("seed");
        assert_eq!(synced_names(&report.synced), vec!["p"]);
        let user = PackManifest::from_dir(target.join("p")).expect("user manifest");
        assert_eq!(user.version, "1.0");
    }

    #[test]
    fn skips_seed_packs_without_version() {
        let root = TempDir::new("seed-no-version");
        let seed = root.path().join("seed");
        let target = root.path().join("user");
        make_pack(&seed, "p", "");
        fs::create_dir_all(&target).expect("mkdir");

        let report = seed_packs(&seed, &target).expect("seed");
        assert!(report.synced.is_empty());
        assert!(!target.join("p").exists());
    }

    #[test]
    fn skips_pending_dot_names_and_plain_files_in_the_seed_dir() {
        let root = TempDir::new("seed-filter");
        let seed = root.path().join("seed");
        let target = root.path().join("user");
        make_pack(&seed, "keep", "version = \"1.0\"\n");
        make_pack(&seed, "drop_pending", "version = \"1.0\"\n");
        make_pack(&seed, ".hidden", "version = \"1.0\"\n");
        write_text(&seed.join("README.txt"), "x");
        fs::create_dir_all(&target).expect("mkdir");

        let report = seed_packs(&seed, &target).expect("seed");
        assert_eq!(synced_names(&report.synced), vec!["keep"]);
        assert!(!target.join("drop_pending").exists());
        assert!(!target.join(".hidden").exists());
        assert!(!target.join("README.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn dereferences_symlinks_when_copying() {
        let root = TempDir::new("seed-symlink");
        let seed = root.path().join("seed");
        let target = root.path().join("user");
        let pack = make_pack(&seed, "p", "version = \"1.0\"\n");
        let outside = root.path().join("outside.txt");
        write_text(&outside, "linked content");
        std::os::unix::fs::symlink(&outside, pack.join("link.txt")).expect("symlink");
        fs::create_dir_all(&target).expect("mkdir");

        let report = seed_packs(&seed, &target).expect("seed");
        assert_eq!(synced_names(&report.synced), vec!["p"]);
        let copied = target.join("p/link.txt");
        assert!(copied.is_file());
        // `copytree` 默认 `symlinks=False`：拷的是目标内容，不是链接。
        assert!(
            !copied
                .symlink_metadata()
                .expect("stat")
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&copied).expect("read"), "linked content");
    }

    // ---------- copy_tree ----------

    #[test]
    fn copy_tree_copies_nested_directories() {
        let root = TempDir::new("copy-nested");
        let src = root.path().join("src");
        write_text(&src.join("a.txt"), "a");
        write_text(&src.join("sub/b.txt"), "b");
        let dst = root.path().join("dst");

        copy_tree(&src, &dst).expect("copy");
        assert_eq!(fs::read_to_string(dst.join("a.txt")).expect("read"), "a");
        assert_eq!(
            fs::read_to_string(dst.join("sub/b.txt")).expect("read"),
            "b"
        );
    }

    #[test]
    fn copy_tree_refuses_an_existing_target() {
        let root = TempDir::new("copy-exists");
        let src = root.path().join("src");
        write_text(&src.join("a.txt"), "a");
        let dst = root.path().join("dst");
        fs::create_dir_all(&dst).expect("mkdir");

        // `copytree` 先 `os.mkdir`，目标已存在就报 `FileExistsError`。
        assert!(copy_tree(&src, &dst).is_err());
    }

    // ---------- resolve_path / lexical_normalize ----------

    #[test]
    fn lexical_normalize_drops_cur_dir_and_collapses_parent() {
        assert_eq!(
            lexical_normalize(Path::new("/a/./b/../c")),
            PathBuf::from("/a/c")
        );
        assert_eq!(
            lexical_normalize(Path::new("/a/b/../..")),
            PathBuf::from("/")
        );
    }

    #[test]
    fn resolve_path_falls_back_to_a_lexical_absolute_path() {
        assert_eq!(
            resolve_path(Path::new("/tmp/gd3-no-such-dir/../x")),
            PathBuf::from("/tmp/x")
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolve_path_follows_symlinks_when_the_target_exists() {
        let root = TempDir::new("resolve-link");
        let real = root.path().join("real");
        fs::create_dir_all(&real).expect("mkdir");
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");

        assert_eq!(
            resolve_path(&link),
            fs::canonicalize(&real).expect("canonical")
        );
    }
}
