//! `cfg.set(...)` 的等价物（第 113 步）：`gd3-ui::config_store`。
//!
//! ## 这一层解决什么
//!
//! Python 侧所有配置写入都走 `qfluentwidgets` 的 `QConfig.set`
//! （`.venv/.../qfluentwidgets/common/config.py:288`），`cfg` 是 `Config(QConfig)`
//! 的实例，所以 `cfg.set(item, value)` 与 `qconfig.set(item, value)` 落到同一个
//! 文件（`qconfig.load(path, cfg)` 把 `qconfig._cfg` 指向了 `cfg`，见
//! `Ghost-Downloader-3.py:38`）。本模块把那条路径的三个可观察后果摊成 Rust
//! 可测的 API：
//!
//! 1. **值没变就不写盘、不发信号**（`if item.value == value: return`）；
//! 2. **值变了就整文件回写**（`save()` → `json.dump(toDict(), indent=4)`，键序
//!    由 `dir(class)` 的属性名字母序决定 —— 即 [`settings::SPECS`] 的顺序）；
//! 3. **`restart=True` 的三项发 `appRestartSig`**，`themeMode` / `themeColor`
//!    各发自己的信号。
//!
//! ## 与 `settings::write` 的分工
//!
//! `gd3_core::settings::write` 只做「correct → serialize → 写进 `UserConfig`」，
//! **不碰磁盘**。本模块在它外面套上「比较 / 整文件回写 / 变更描述」三件事，
//! validator 不重复实现一行。
//!
//! ## 实证口径（2026-10-19 实读 `qfluentwidgets/common/config.py`）
//!
//! ```python
//! def set(self, item, value, save=True, copy=True):
//!     if item.value == value:
//!         return
//!     try:
//!         item.value = deepcopy(value) if copy else value   # setter 里跑 validator.correct()
//!     except:
//!         item.value = value
//!     if save:
//!         self.save()
//!     if item.restart:
//!         self._cfg.appRestartSig.emit()
//!     if item is self._cfg.themeMode:
//!         self.theme = value
//!         self._cfg.themeChanged.emit(value)
//!     if item is self._cfg.themeColor:
//!         self._cfg.themeColorChanged.emit(value)
//! ```
//!
//! 四个反直觉但已实证的结论：
//!
//! - **比较发生在 `correct()` 之前**：当前值是 `3` 时 `cfg.set(cfg.maxTaskNum, 0)`
//!   会继续走完流程、夹成 `1` 再落盘；而 `cfg.set(cfg.maxTaskNum, 3.0)` 直接返回
//!   （`3 == 3.0` 为真）。所以 [`py_eq`] 必须按 Python 的 `==` 口径比数字/布尔。
//! - **`set` 里没有 `validate()`**：`QConfig.set` 只经过 `value` setter 里的
//!   `validator.correct()`，不调 `validate()`。越界值被**夹紧**，不是被拒收。
//! - **`correct()` 之后值没变也照样落盘**：`valueChanged` 不发（setter 里
//!   `if ov != v`），但 `save()` 无条件执行。本模块用 [`ConfigChange::emitted`]
//!   区分「落盘」与「发信号」这两件事。
//! - **`save()` 会 `mkdir(parents=True)`**，父目录不存在不是错误。
//!
//! ## 与 Python 的已知差异
//!
//! - **`correct()` 抛异常时不中断**：`cfg.set(cfg.maxTaskNum, "7")` 在 Python 里让
//!   `RangeValidator.correct` 抛 `TypeError`（`min(max(1, "7"), 10)`），异常穿透
//!   `set`、什么都没写；Rust 的 `Kind::correct` 退回缺省值并继续落盘。这与
//!   `gd3-core::settings` 模块头那条「Rust 容错 ≠ Python 中断」一致。
//! - **`themeColor` 的比较口径不同**：Python 的 `item.value` 是 `QColor`，
//!   `QColor('#ff009faa') == '#ff009faa'` 恒为假，所以每次
//!   `cfg.set(cfg.themeColor, ...)` 都会落盘；Rust 存的是 `#aarrggbb` 串，同值会
//!   提前返回。app 里没有 `cfg.set(cfg.themeColor, ...)`（主题色走
//!   `setThemeColor(color, save=False)`，**不落盘**），这条差异不可达。
//! - **磁盘上的未知键在 `load` 就丢**：Python 的 `QConfig.load` 只按 `item.key`
//!   匹配已知项，`toDict()` 又只从类属性重建，所以多出来的组/键既读不进内存、
//!   也不会被下次 `save()` 写回。Rust 侧 [`ConfigStore::load`] 直接归一化成
//!   「42 项全量」，等价。
//! - **没有全局单例**：Python 的 `qconfig` 是模块级实例，`load` 通过改
//!   `_cfg`/`file` 把写入目标接到 app 的 `cfg` 上。Rust 侧 [`ConfigStore`] 自己
//!   持有 `(config, path)`，不需要那层间接。
//!
//! ## 为什么不接进 `boot::run`
//!
//! 见 `rust/README.md`「剩余工作（Phase 8）」：Rust 侧写同一个 `UserConfig.json`
//! 必须与「Slint 成为唯一 UI」同一步，否则会和仍在运行的 Qt 侧互相覆盖。
//! 本模块先作为库 + 单测落地。

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use gd3_core::config::UserConfig;
use gd3_core::settings;
use serde_json::{Map, Value};

/// `cfg.py` 里 `restart=True` 的三项：`shouldVerifySsl`(`:250`)、
/// `dpiScale`(`:296`)、`language`(`:303`)。`QConfig.set` 在这三项落盘之后
/// 发 `appRestartSig`（`setting_page.py:398` 接成「需要重启」的提示条）。
pub const RESTART_ATTRS: &[&str] = &["dpiScale", "language", "shouldVerifySsl"];

/// 一次 `cfg.set` 的完整描述（值已经落盘）。
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigChange {
    /// `cfg.py` 的属性名（`maxTaskNum`、`language` …）。
    pub attr: &'static str,
    /// 组名（`GeneralDownload` …）。
    pub group: &'static str,
    /// 键名（`MaxTaskNum` …）。
    pub key: &'static str,
    /// 改之前的逻辑值。
    pub previous: Value,
    /// 改之后的逻辑值（已过 `correct()`）。
    pub value: Value,
    /// 落盘形态（已过 `serialize()`）。
    pub stored: Value,
    /// `valueChanged` 是否发射 —— 即 `correct()` **之后**逻辑值是否真的变了。
    /// 为 `false` 时仍然写了盘（Python 的 `save()` 在 setter 之外）。
    pub emitted: bool,
    /// 是否要发 `appRestartSig`。
    pub restart: bool,
}

impl ConfigChange {
    /// Python `if item is self._cfg.themeMode:` → 发 `themeChanged`。
    pub fn theme_changed(&self) -> bool {
        self.attr == "themeMode"
    }

    /// Python `if item is self._cfg.themeColor:` → 发 `themeColorChanged`。
    pub fn theme_color_changed(&self) -> bool {
        self.attr == "themeColor"
    }
}

/// [`ConfigStore::set`] 的两种失败。
#[derive(Debug)]
pub enum ConfigStoreError {
    /// 属性名不在 [`settings::SPECS`] 里。Python 侧走属性访问，编译期就不可能
    /// 出现这条路径，所以它只用于挡住 Rust 侧的拼写错误。
    UnknownAttribute(String),
    /// 整文件回写失败。**内存里的值已经改了**（Python 同样是先改值再 `save()`）。
    Save(io::Error),
}

impl fmt::Display for ConfigStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownAttribute(attr) => write!(f, "未知的配置项：{attr}"),
            Self::Save(error) => write!(f, "保存配置失败：{error}"),
        }
    }
}

impl std::error::Error for ConfigStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Save(error) => Some(error),
            Self::UnknownAttribute(_) => None,
        }
    }
}

/// 一份「已加载的 `cfg` + 它落盘的那个路径」。
///
/// 内存里的 [`UserConfig`] 恒为**全量 42 项**（[`settings::SPECS`] 的每一项都有
/// 值，缺键的项取缺省），与 Python 的 `ConfigItem` 表一一对应。
#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
    config: UserConfig,
}

impl ConfigStore {
    /// 不读盘：内存里直接是 [`settings::default_config`]（= Python 的 `cfg` 初值）。
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            config: settings::default_config(),
        }
    }

    /// 镜像 `qconfig.load(f"{APP_DATA_DIR}/UserConfig.json", cfg)`
    /// （`Ghost-Downloader-3.py:38`）：
    ///
    /// - 文件不存在 → 保留缺省值，**不写盘**（Python 的 `load` 也不写）；
    /// - 内容不是合法 JSON / 组层级不是对象 → `io::ErrorKind::InvalidData`；
    /// - 已知项按 `deserialize` + `correct` 收下，未知组/键丢掉。
    pub fn load(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        let loaded = match UserConfig::load(&path) {
            Ok(config) => config,
            Err(error) if error.kind() == io::ErrorKind::NotFound => UserConfig::new(),
            Err(error) => return Err(error),
        };
        Ok(Self {
            path,
            config: rebuild(&loaded),
        })
    }

    /// 重新从磁盘读一遍（`ConfigStore` 是不可变快照，改完文件要显式重载）。
    pub fn reload(&mut self) -> io::Result<()> {
        *self = Self::load(self.path.clone())?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn config(&self) -> &UserConfig {
        &self.config
    }

    /// 直接改内存（不落盘）。要给磁盘留下改动，之后必须显式 [`Self::save`]。
    pub fn config_mut(&mut self) -> &mut UserConfig {
        &mut self.config
    }

    /// 按属性名读逻辑值（缺键 → 缺省值），同 [`settings::read`]。
    pub fn get(&self, attr: &str) -> Option<Value> {
        settings::read(&self.config, attr)
    }

    /// 镜像 `QConfig.set`：
    ///
    /// - 返回值 `Ok(None)` = Python 的提前返回（逻辑值相等），**没写盘、没信号**；
    /// - `Ok(Some(change))` = 已整文件回写，`change` 描述该发哪些信号；
    /// - `Err(Save)` = 内存已改、磁盘没写成（Python 的异常同样在 setter 之后）。
    pub fn set(
        &mut self,
        attr: &str,
        value: &Value,
    ) -> Result<Option<ConfigChange>, ConfigStoreError> {
        let spec = settings::spec(attr)
            .ok_or_else(|| ConfigStoreError::UnknownAttribute(attr.to_string()))?;

        // Python: `if item.value == value: return` —— 比较在 correct() 之前。
        let previous = spec.read(&self.config);
        if py_eq(&previous, value) {
            return Ok(None);
        }

        // `ConfigItem.value` setter：`v = self.validator.correct(v)`。
        let logical = spec.kind.correct(value);
        let stored = spec.kind.serialize(&logical);
        // Python 先改内存，`save()` 在之后才可能抛。
        self.config.set(spec.group, spec.key, stored.clone());

        // setter 里 `if ov != v: valueChanged.emit(v)` —— 比的是 correct 之后的值。
        let emitted = !py_eq(&previous, &logical);
        let change = ConfigChange {
            attr: spec.attr,
            group: spec.group,
            key: spec.key,
            previous,
            emitted,
            value: logical,
            stored,
            restart: RESTART_ATTRS.contains(&spec.attr),
        };
        self.save().map_err(ConfigStoreError::Save)?;
        Ok(Some(change))
    }

    /// 整文件回写：`mkdir(parents=True, exist_ok=True)` + `json.dump(indent=4)`。
    ///
    /// 内容由 [`settings::SPECS`] 重建（同 Python 的 `toDict()` 遍历 `dir(class)`），
    /// 所以磁盘上多出来的未知键**不会**被写回 —— 这是 Python 的行为。
    pub fn save(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, self.snapshot().to_json_text())
    }

    /// 按 `toDict()` 口径重建一份完整配置（逻辑值 → 落盘形态）。
    ///
    /// 这就是 [`Self::save`] 写出去的内容；`boot::run` 想拿「可以交给 `Engine` 的
    /// 全量 cfg」时也用它。
    pub fn snapshot(&self) -> UserConfig {
        rebuild(&self.config)
    }
}

/// 把任意 `UserConfig` 归一化成「[`settings::SPECS`] 全量 + 落盘形态」。
fn rebuild(source: &UserConfig) -> UserConfig {
    let mut out = UserConfig::new();
    for spec in settings::SPECS {
        let logical = spec.read(source);
        spec.write(&mut out, &logical);
    }
    out
}

/// Python `==` 的镜像（只覆盖配置值会用到的形态）。
///
/// 关键点：`3 == 3.0` 为真、`True == 1` 为真（`bool` 是 `int` 的子类）、
/// `"3" == 3` 为假。`serde_json::Value` 的 `PartialEq` 三条都不成立，而
/// `QConfig.set` 的第一行就是这个比较。
pub fn py_eq(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Number(a), Value::Number(b)) => match (a.as_f64(), b.as_f64()) {
            (Some(a), Some(b)) => a == b,
            // 超出 f64 精度的大整数：退回 serde 的原口径。
            _ => a == b,
        },
        (Value::Bool(flag), Value::Number(number)) | (Value::Number(number), Value::Bool(flag)) => {
            let expected = if *flag { 1.0 } else { 0.0 };
            number.as_f64().is_some_and(|actual| actual == expected)
        }
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| py_eq(a, b))
        }
        // Python 的 dict 比较不看键序。
        (Value::Object(a), Value::Object(b)) => map_eq(a, b),
        _ => false,
    }
}

fn map_eq(left: &Map<String, Value>, right: &Map<String, Value>) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .all(|(key, value)| right.get(key).is_some_and(|other| py_eq(value, other)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    use serde_json::json;

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "gd3-ui-config-store-{}-{tag}-{seq}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    /// 一个「路径在临时目录里、内存是缺省值」的 store。
    fn store(tag: &str) -> (PathBuf, ConfigStore) {
        let dir = temp_dir(tag);
        let path = dir.join("UserConfig.json");
        let store = ConfigStore::new(&path);
        (dir, store)
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).expect("读回写出的配置")
    }

    #[test]
    fn a_missing_file_leaves_the_defaults_and_is_not_created() {
        let (_dir, store) = store("missing");
        let loaded = ConfigStore::load(store.path()).expect("缺文件也要成功");
        assert_eq!(loaded.config(), &settings::default_config());
        assert!(!loaded.path().exists());
        assert_eq!(loaded.get("maxTaskNum"), Some(json!(3)));
    }

    #[test]
    fn a_python_written_file_round_trips_byte_for_byte() {
        let (_dir, store) = store("roundtrip");
        std::fs::write(store.path(), settings::default_config().to_json_text())
            .expect("先写一份缺省档");
        let loaded = ConfigStore::load(store.path()).expect("load");
        loaded.save().expect("save");
        assert_eq!(
            read(store.path()),
            settings::default_config().to_json_text()
        );
    }

    #[test]
    fn set_rewrites_the_whole_file() {
        let (_dir, mut store) = store("set");
        let change = store
            .set("maxTaskNum", &json!(5))
            .expect("已知 attr")
            .expect("值变了");

        assert_eq!(change.attr, "maxTaskNum");
        assert_eq!(change.group, "GeneralDownload");
        assert_eq!(change.key, "MaxTaskNum");
        assert_eq!(change.previous, json!(3));
        assert_eq!(change.value, json!(5));
        assert_eq!(change.stored, json!(5));
        assert!(change.emitted);
        assert!(!change.restart);

        // 期望值由 `settings::write` 独立算出，不复用被测实现的 snapshot()。
        let mut expected = settings::default_config();
        settings::write(&mut expected, "maxTaskNum", &json!(5)).expect("已知 attr");
        assert_eq!(read(store.path()), expected.to_json_text());
    }

    #[test]
    fn an_unchanged_value_neither_writes_nor_signals() {
        let (_dir, mut store) = store("unchanged");
        assert_eq!(store.set("maxTaskNum", &json!(3)).expect("已知 attr"), None);
        assert!(!store.path().exists());
    }

    #[test]
    fn an_int_and_a_float_of_the_same_value_count_as_equal() {
        let (_dir, mut store) = store("float-eq");
        // DpiScale 的缺省落盘是 int `0`（`RangeValidator.correct(0.0)` 的边界是
        // int 字面量），而 SpinBox 发过来的是 float `0.0`。
        assert_eq!(store.get("dpiScale"), Some(json!(0)));
        assert_eq!(store.set("dpiScale", &json!(0.0)).expect("已知 attr"), None);
        assert!(!store.path().exists());
    }

    #[test]
    fn an_unknown_attribute_is_rejected_without_touching_the_file() {
        let (_dir, mut store) = store("unknown-attr");
        let error = store.set("noSuchItem", &json!(1)).expect_err("应当报错");
        assert!(matches!(
            error,
            ConfigStoreError::UnknownAttribute(ref attr) if attr == "noSuchItem"
        ));
        assert!(!store.path().exists());
        assert_eq!(store.get("noSuchItem"), None);
    }

    #[test]
    fn save_creates_the_missing_parent_directory() {
        let dir = temp_dir("mkdir");
        let path = dir.join("nested/deeper/UserConfig.json");
        let store = ConfigStore::new(&path);
        store
            .save()
            .expect("Python 的 save() 会 mkdir(parents=True)");
        assert_eq!(read(&path), settings::default_config().to_json_text());
    }

    #[test]
    fn a_failed_save_keeps_the_new_value_in_memory() {
        let dir = temp_dir("save-fail");
        let path = dir.join("occupied");
        std::fs::create_dir_all(&path).expect("占位目录");
        let mut store = ConfigStore::new(&path);

        let error = store
            .set("maxTaskNum", &json!(7))
            .expect_err("写目录必失败");
        assert!(matches!(error, ConfigStoreError::Save(_)));
        // Python 的 setter 在 save() 之前就改了 item.value。
        assert_eq!(store.get("maxTaskNum"), Some(json!(7)));
    }

    #[test]
    fn an_out_of_range_value_is_clamped_not_rejected() {
        let (_dir, mut store) = store("clamp");
        let change = store
            .set("maxTaskNum", &json!(99))
            .expect("已知 attr")
            .expect("值变了");
        assert_eq!(change.value, json!(10));
        assert_eq!(change.stored, json!(10));
        assert!(change.emitted);
        assert!(read(store.path()).contains("\"MaxTaskNum\": 10"));
    }

    #[test]
    fn a_value_that_corrects_back_to_the_current_one_is_saved_without_a_signal() {
        let (_dir, mut store) = store("corrected");
        let change = store
            .set("nameConflict", &json!("bogus"))
            .expect("已知 attr")
            .expect("入参 ≠ 当前值，所以走完流程");
        assert_eq!(change.previous, json!("keepBoth"));
        assert_eq!(change.value, json!("keepBoth"));
        assert!(!change.emitted, "correct() 之后没变 → valueChanged 不发");
        assert!(store.path().exists(), "但 save() 无条件执行");
    }

    #[test]
    fn restart_items_are_flagged() {
        let (_dir, mut store) = store("restart");
        let language = store
            .set("language", &json!("en_US"))
            .expect("已知 attr")
            .expect("值变了");
        assert!(language.restart);
        let max_task_num = store
            .set("maxTaskNum", &json!(4))
            .expect("已知 attr")
            .expect("值变了");
        assert!(!max_task_num.restart);
    }

    #[test]
    fn theme_items_are_flagged() {
        let (_dir, mut store) = store("theme");
        let mode = store
            .set("themeMode", &json!("Dark"))
            .expect("已知 attr")
            .expect("值变了");
        assert!(mode.theme_changed());
        assert!(!mode.theme_color_changed());

        let color = store
            .set("themeColor", &json!("#ff102030"))
            .expect("已知 attr")
            .expect("值变了");
        assert!(color.theme_color_changed());
        assert_eq!(color.stored, json!("#ff102030"));
    }

    #[test]
    fn unknown_keys_on_disk_are_dropped_by_the_load() {
        let (_dir, store) = store("drop-unknown");
        let mut on_disk = settings::default_config();
        on_disk.set("GeneralDownload", "FutureKey", json!(1));
        std::fs::write(store.path(), on_disk.to_json_text()).expect("写一份带未知键的档");

        let loaded = ConfigStore::load(store.path()).expect("load");
        // Python 的 `QConfig.load` 只按 `item.key` 匹配已知项，未知键进不了内存。
        assert_eq!(loaded.config().get("GeneralDownload", "FutureKey"), None);
        loaded.save().expect("save");
        assert!(!read(store.path()).contains("FutureKey"));
    }

    #[test]
    fn reload_picks_up_an_external_write() {
        let (_dir, mut store) = store("reload");
        let mut on_disk = settings::default_config();
        settings::write(&mut on_disk, "maxTaskNum", &json!(9)).expect("已知 attr");
        std::fs::write(store.path(), on_disk.to_json_text()).expect("外部写入");

        assert_eq!(store.get("maxTaskNum"), Some(json!(3)), "尚未重载");
        store.reload().expect("reload");
        assert_eq!(store.get("maxTaskNum"), Some(json!(9)));
    }

    #[test]
    fn py_eq_mirrors_python_equality() {
        assert!(py_eq(&json!(3), &json!(3.0)));
        assert!(py_eq(&json!(true), &json!(1)));
        assert!(!py_eq(&json!("3"), &json!(3)));
        assert!(!py_eq(&json!(false), &json!(1)));
        assert!(py_eq(&json!({"a": [1, 2]}), &json!({"a": [1.0, 2]})));
        assert!(!py_eq(&json!([1, 2]), &json!([1, 2, 3])));
        assert!(py_eq(&Value::Null, &Value::Null));
        assert!(!py_eq(&Value::Null, &json!(0)));
    }
}
