//! Phase 8 主链路：应用启动序列（`Ghost-Downloader-3.py` 的 Rust 对应物）。
//!
//! Python 入口在仓库根的 `Ghost-Downloader-3.py`（268 行），`__main__` 分支的序列是：
//!
//! 1. `setupEnvironment()`：`qconfig.load(f"{APP_DATA_DIR}/UserConfig.json", cfg)`；
//!    `cfg.dpiScale.value != 0` 时设 `QT_ENABLE_HIGHDPI_SCALING`/`QT_SCALE_FACTOR`；
//! 2. `SingletonApplication(sys.argv, DESKTOP_ID)`：QSharedMemory 单实例 + D-Bus 接收器；
//! 3. `isSilent = "--silence" in sys.argv or any(isWakeUri(arg) for arg in sys.argv[1:])`；
//! 4. `startApp(app, isSilent)`：字体族 → `loadResources` → `createEngine`
//!    → 剪贴板监听/重名提示 → 主窗口（首次启动先 OOBE）→ 托盘 → 更新检查；
//! 5. `app.exec()`；退出时 `engine.stop()` / `updateService.apply()` / `coroutineRunner.stop()`。
//!
//! 本步（第 80 步）只做「能跑起来」的最小闭环，其余按 Phase 8 后续小步补：
//!
//! ```text
//! 路径解析 → 配置装载 → 单实例锁 → 窗口实例化（配置推送到 UI）→ 事件循环
//! ```
//!
//! 已迁移的单实例锁见 [`crate::single_instance`]（`APP_DATA_DIR/instance.sock`）。
//!
//! 未迁移（后续小步）：D-Bus 服务名注册（`org.freedesktop.Application`）、`app/engine.py`
//! 剩余服务、`desktop_keepalive`/通知、功能包加载、更新检查、OOBE 分支、
//! 多语言翻译器。托盘已在第 136 步接上（[`crate::tray`]，只在有头分支创建）。
//!
//! # 路径语义（`app/config/paths.py` 58 行）
//!
//! Python 侧全部走 Qt：
//!
//! - `EXECUTABLE_DIR = Path(".")`（非编译模式，**相对路径**）；
//! - `APP_DATA_DIR = EXECUTABLE_DIR/"GhostDownloader"` **当且仅当该目录已存在**，
//!   否则 `<GenericDataLocation>/GhostDownloader`；`isPortable()` 就是这两者相等；
//! - `USER_DATA_DIR` 恒为 `<GenericDataLocation>/GhostDownloader`；
//! - `FEATURES_DIR = EXECUTABLE_DIR/"features"`（非编译模式）；
//! - `DOWNLOAD_DIR = <DownloadLocation>`。
//!
//! 本机（VPS，`HOME` 为空）实测 Qt 的取值是：
//!
//! ```text
//! HomeLocation         '/'
//! GenericDataLocation  '//.local/share'
//! DownloadLocation     '//Downloads'
//! ```
//!
//! 即 Qt 在 `HOME` 为空时把 home 当 `"/"`，再用**字符串拼接**（不做路径归一）得到
//! `"//..."`。所以这里也照抄「拼接」而不是 `Path::join`（后者会归一掉双斜杠）。
//!
//! # 已知差异
//!
//! - Python 无 `--data-dir`/`--headless`/`--frames`，这是 Rust 侧为无显示器构建机
//!   加的可测性开关（`--headless` 走 `MinimalSoftwareWindow`，跑固定帧数后退出）；
//! - `HOME` 未设时 Python 用 `QStandardPaths`，这里退回 `"/"`（与上面实测一致）；
//! - 配置解析失败时 Python 的 `@exceptionHandler()` 会中断 `QConfig.load`、保持缺省，
//!   这里同样退回缺省，但**不写盘**（Python 也不写）；
//! - 引擎装配走 [`crate::engine::Engine`]（镜像 `app/engine.py`）：`Engine::new` 拼服务、
//!   `Engine::start()` 装载 `tasks.jsonl` + `resumeSaved` + 起两个监听器、`Engine::stop()`
//!   停机收尾（含两次 `flush`）。`resume_saved` 只入队，**不 pump**——`_pump` 需要真的
//!   派发协程，等下载引擎迁过来再接（第 82 步结论）；
//! - 停机序列在**无头模式也会跑**（Python 没有无头模式，但「启动完立刻退出」在 Python 里
//!   同样会走 `Engine.stop()`）：所以一次无头冒烟会把 `RUNNING`/`WAITING` 的任务落成
//!   `PAUSED`，与真实退出行为一致；
//! - `--tasks-file` 是 Rust 侧的可测性开关（Python 侧固定用 `APP_DATA_DIR`）。

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gd3_core::config::UserConfig;
use gd3_core::settings;
use gd3_core::task::TaskStatus;
use serde_json::Value;
// `window()`/`run()` 来自 `ComponentHandle`，不在 prelude 里。
use slint::ComponentHandle;

/// 便携目录名 / 数据目录名（`paths.py` 里硬编码的 `"GhostDownloader"`）。
pub const APP_NAME: &str = "GhostDownloader";
/// `app/config/constants.py::VERSION`（注意 pyproject 的 `version` 是另一个数）。
pub const VERSION: &str = "4.4.0";
/// `app/config/constants.py::DESKTOP_ID`。
pub const DESKTOP_ID: &str = "io.github.xiaoyouchr.GhostDownloader";
/// `app/platform/url_scheme.py::URL_SCHEME`。
pub const URL_SCHEME: &str = "ghostdownloader";
/// `paths.py` 与 `Ghost-Downloader-3.py` 共用的配置文件名。
pub const USER_CONFIG_FILE: &str = "UserConfig.json";
/// `task_service.py::TaskStore.__init__` 里硬编码的 `APP_DATA_DIR / "tasks.jsonl"`。
pub const TASKS_FILE: &str = "tasks.jsonl";
/// `startup.py:49` 传给 `Engine` 的 `extensionFolder` 叶子名。
pub const EXTENSION_FOLDER: &str = "browser_extension";
/// `--until-idle` 的缺省上限（秒）。Python 没有这个开关，取这个值只为「不无限挂住」。
pub const IDLE_TIMEOUT: u64 = 300;
/// 停机时等工作线程收工的时长（超过就放手，`.ghd` 已经保住，下次启动续传）。
pub const WORKER_SHUTDOWN_WAIT: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// 路径
// ---------------------------------------------------------------------------

/// `paths.py` 五个目录的 Rust 形态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// `EXECUTABLE_DIR`（非编译模式 = `"."`，相对路径）。
    pub executable_dir: PathBuf,
    /// `APP_DATA_DIR`：便携模式 = `EXECUTABLE_DIR/GhostDownloader`，否则在用户数据目录下。
    pub app_data_dir: PathBuf,
    /// `USER_DATA_DIR`：恒为 `<GenericDataLocation>/GhostDownloader`。
    pub user_data_dir: PathBuf,
    /// `DOWNLOAD_DIR`：新任务的缺省下载目录。
    pub download_dir: PathBuf,
    /// `FEATURES_DIR`（非编译模式 = `EXECUTABLE_DIR/features`）。
    pub features_dir: PathBuf,
    /// `isPortable()`：`APP_DATA_DIR == PORTABLE_DIR`。
    pub portable: bool,
}

/// `HOME` 为空/未设时 Qt 实测给 `"/"`（见模块头）。
fn home_dir() -> String {
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() => h,
        _ => "/".to_string(),
    }
}

/// Qt 风格的 home 拼接：**不做路径归一**，所以 `home == "/"` 会得到 `"//Downloads"`。
fn home_path_join(home: &str, leaf: &str) -> PathBuf {
    PathBuf::from(format!("{home}/{leaf}"))
}

/// `QStandardPaths::GenericDataLocation`：`$XDG_DATA_HOME` 或 `<home>/.local/share`。
pub fn generic_data_location() -> PathBuf {
    match std::env::var("XDG_DATA_HOME") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => home_path_join(&home_dir(), ".local/share"),
    }
}

/// `QStandardPaths::DownloadLocation`：`$XDG_DOWNLOAD_DIR` 或 `<home>/Downloads`。
///
/// （Qt 还会读 `~/.config/user-dirs.dirs`；本机该文件不存在，未镜像那一层。）
pub fn default_download_dir() -> PathBuf {
    match std::env::var("XDG_DOWNLOAD_DIR") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => home_path_join(&home_dir(), "Downloads"),
    }
}

impl Paths {
    /// 镜像 `paths.py` 的模块级求值。
    ///
    /// `data_home` / `download_dir` 是给测试与 `--data-dir` 用的覆盖点
    /// （`None` = 走 Qt 口径的环境变量）。
    pub fn resolve(
        executable_dir: &Path,
        data_home: Option<&Path>,
        download_dir: Option<&Path>,
    ) -> Self {
        let generic = match data_home {
            Some(p) => p.to_path_buf(),
            None => generic_data_location(),
        };
        let portable_dir = executable_dir.join(APP_NAME);
        let portable = portable_dir.is_dir();
        let app_data_dir = if portable {
            portable_dir
        } else {
            generic.join(APP_NAME)
        };
        Self {
            executable_dir: executable_dir.to_path_buf(),
            app_data_dir,
            user_data_dir: generic.join(APP_NAME),
            download_dir: match download_dir {
                Some(p) => p.to_path_buf(),
                None => default_download_dir(),
            },
            features_dir: executable_dir.join("features"),
            portable,
        }
    }

    /// 测试/`--data-dir` 用：把数据目录钉死在给定路径（非便携模式）。
    pub fn with_app_data_dir(app_data_dir: &Path) -> Self {
        Self {
            executable_dir: PathBuf::from("."),
            app_data_dir: app_data_dir.to_path_buf(),
            user_data_dir: app_data_dir.to_path_buf(),
            download_dir: default_download_dir(),
            features_dir: PathBuf::from("./features"),
            portable: false,
        }
    }

    /// `f"{APP_DATA_DIR}/UserConfig.json"`。
    pub fn user_config_path(&self) -> PathBuf {
        self.app_data_dir.join(USER_CONFIG_FILE)
    }

    /// `APP_DATA_DIR / "tasks.jsonl"`。
    pub fn tasks_path(&self) -> PathBuf {
        self.app_data_dir.join(TASKS_FILE)
    }

    /// `APP_DATA_DIR / "browser_extension"`：`startup.py:49` 传给 `Engine` 的
    /// `extensionFolder`，`browser_service.py:201` 把扩展解到这里。
    pub fn extension_folder(&self) -> PathBuf {
        self.app_data_dir.join(EXTENSION_FOLDER)
    }

    /// 单实例锁的 socket 路径（`APP_DATA_DIR / "instance.sock"`）。
    ///
    /// Python 用 `QSharedMemory(DESKTOP_ID)` 这个全局 key；这里放在数据目录下，
    /// 同样是「每用户一份」，但换 `--data-dir` 就换锁（见 [`crate::single_instance`]）。
    pub fn lock_path(&self) -> PathBuf {
        self.app_data_dir.join(crate::single_instance::LOCK_FILE)
    }
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

/// 命令行解析结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cli {
    /// `--silence`（Python：跳过启动画面 / OOBE）。
    pub silent: bool,
    /// 强制无头（Rust 扩展）。
    pub headless: bool,
    /// 无头模式驱动的帧数（Rust 扩展；缺省 1）。
    pub frames: Option<u32>,
    /// 数据目录覆盖（Rust 扩展，测试用）。
    pub data_dir: Option<PathBuf>,
    /// `tasks.jsonl` 覆盖（Rust 扩展，测试用；缺省 `<APP_DATA_DIR>/tasks.jsonl`）。
    pub tasks_file: Option<PathBuf>,
    /// 推到没有任务在跑再退出（Rust 扩展，冒烟用；隐含无头）。
    pub until_idle: bool,
    /// `--until-idle` 的上限秒数（缺省 [`IDLE_TIMEOUT`]）。
    pub idle_timeout: Option<u64>,
    /// `--set ATTR=VALUE`（可重复）：按 [`settings::SPECS`] 的属性名写一项配置。
    /// 原始形态存在这里（解析期只校验 `ATTR=VALUE` 的形状，取值到写盘时才解析）。
    pub sets: Vec<String>,
    pub help: bool,
    pub version: bool,
    /// 位置参数里识别出的 URL / 文件 URI（`application.py::urisFromArgv`）。
    pub urls: Vec<String>,
    /// 原始位置参数（不含开关）。单实例交接要它来判断「argv 里有没有 wake URI」。
    pub positional: Vec<String>,
}

pub const HELP: &str = "\
Ghost Downloader (Rust/Slint)

用法: gd3-ui [选项] [URL|文件 ...]

选项:
  --silence            静默启动（跳过启动画面与首次引导）
  --headless           强制无头模式（不依赖显示服务，跑固定帧数后退出）
  --frames <N>         无头模式驱动的帧数（缺省 1）
  --data-dir <PATH>    覆盖数据目录（测试用，Python 侧无此参数）
  --tasks-file <PATH>  覆盖任务列表文件（测试用，Python 侧无此参数）
  --until-idle         跑到没有任务在跑再退出（隐含无头；真下载，冒烟用）
  --idle-timeout <SEC> --until-idle 的上限秒数（缺省 300）
  --set ATTR=VALUE     写一项配置（可重复；取值先按 JSON 解析，失败即裸字符串；
                       属性名见 settings::SPECS，如 --set maxTaskNum=5）
  -V, --version        打印版本
  -h, --help           打印本帮助
";

/// `url_scheme.py::isLaunchUri`。
pub fn is_launch_uri(uri: &str) -> bool {
    uri.starts_with(&format!("{URL_SCHEME}://"))
}

/// `url_scheme.py::isWakeUri`：`uri.rstrip("/") == "ghostdownloader://wake"`。
pub fn is_wake_uri(uri: &str) -> bool {
    uri.trim_end_matches('/') == format!("{URL_SCHEME}://wake")
}

/// `application.py::urisFromArgv` 的镜像。
///
/// - 跳过 `-` 开头的参数与 `ghostdownloader://...` 启动 URI；
/// - `arg.find(':') > 1` 视为 URL（所以 `"a:b"` 不算，`"C:/x"` 也不算 —— 与 Python 一致）；
/// - 否则若该路径是文件，取 `Path.resolve().as_uri()`。
pub fn uris_from_argv(args: &[String]) -> Vec<String> {
    let mut uris = Vec::new();
    for arg in args {
        if arg.starts_with('-') || is_launch_uri(arg) {
            continue;
        }
        match arg.find(':') {
            Some(colon) if colon > 1 => uris.push(arg.clone()),
            _ => {
                let path = Path::new(arg);
                if path.is_file()
                    && let Ok(abs) = path.canonicalize()
                {
                    uris.push(file_uri(&abs));
                }
            }
        }
    }
    uris
}

/// `Path.as_uri()` 的 posix 分支：`"file://" + quote(str(path))`，`safe='/'`。
fn file_uri(path: &Path) -> String {
    let text = path.to_string_lossy();
    let mut out = String::with_capacity(text.len() + 7);
    out.push_str("file://");
    for byte in text.bytes() {
        let ch = byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '/' | '_' | '.' | '-' | '~') {
            out.push(ch);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// 解析参数（不含 argv[0]）。
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Cli, String> {
    let args: Vec<String> = args.into_iter().collect();
    let mut cli = Cli::default();
    let mut positional: Vec<String> = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        match arg {
            "--silence" => cli.silent = true,
            "--headless" => cli.headless = true,
            "--help" | "-h" => cli.help = true,
            "--version" | "-V" => cli.version = true,
            "--frames" => {
                index += 1;
                let value = args.get(index).ok_or("--frames 缺少取值")?;
                cli.frames = Some(parse_frames(value)?);
            }
            "--data-dir" => {
                index += 1;
                let value = args.get(index).ok_or("--data-dir 缺少取值")?;
                cli.data_dir = Some(PathBuf::from(value));
            }
            "--tasks-file" => {
                index += 1;
                let value = args.get(index).ok_or("--tasks-file 缺少取值")?;
                cli.tasks_file = Some(PathBuf::from(value));
            }
            "--until-idle" => cli.until_idle = true,
            "--set" => {
                index += 1;
                let value = args.get(index).ok_or("--set 缺少取值")?;
                cli.sets.push(parse_set_pair(value)?);
            }
            "--idle-timeout" => {
                index += 1;
                let value = args.get(index).ok_or("--idle-timeout 缺少取值")?;
                cli.idle_timeout = Some(parse_idle_timeout(value)?);
            }
            other if other.starts_with("--frames=") => {
                cli.frames = Some(parse_frames(&other["--frames=".len()..])?);
            }
            other if other.starts_with("--data-dir=") => {
                cli.data_dir = Some(PathBuf::from(&other["--data-dir=".len()..]));
            }
            other if other.starts_with("--tasks-file=") => {
                cli.tasks_file = Some(PathBuf::from(&other["--tasks-file=".len()..]));
            }
            other if other.starts_with("--idle-timeout=") => {
                cli.idle_timeout = Some(parse_idle_timeout(&other["--idle-timeout=".len()..])?);
            }
            other if other.starts_with("--set=") => {
                cli.sets.push(parse_set_pair(&other["--set=".len()..])?);
            }
            other if other.starts_with('-') && other != "-" => {
                return Err(format!("未知参数：{other}"));
            }
            _ => positional.push(args[index].clone()),
        }
        index += 1;
    }
    // Python：`isSilent = "--silence" in sys.argv or any(isWakeUri(arg) for arg in sys.argv[1:])`
    if positional.iter().any(|arg| is_wake_uri(arg)) {
        cli.silent = true;
    }
    cli.urls = uris_from_argv(&positional);
    cli.positional = positional;
    Ok(cli)
}

fn parse_frames(value: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .map_err(|_| format!("--frames 需要非负整数，收到：{value}"))
}

fn parse_idle_timeout(value: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|_| format!("--idle-timeout 需要非负整数，收到：{value}"))
}

/// `--set ATTR=VALUE`：这里只校验形状（`ATTR` 非空且有 `=`），取值留给
/// [`parse_set_value`]。
fn parse_set_pair(value: &str) -> Result<String, String> {
    match value.split_once('=') {
        Some((attr, _)) if !attr.trim().is_empty() => Ok(value.to_string()),
        _ => Err(format!("--set 需要 ATTR=VALUE 形态，收到：{value}")),
    }
}

/// `--set` 的取值：先当 JSON 解析（`5` / `true` / `"文本"` / `[]`），失败就当裸
/// 字符串（路径、`auto` 这类枚举字面量）。
///
/// 与 Python 侧口径一致：`cfg.set` 收到什么类型就是什么类型，字符串在 JSON 里要带
/// 引号，裸写也当字符串。
fn parse_set_value(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
}

// ---------------------------------------------------------------------------
// 配置装载
// ---------------------------------------------------------------------------

/// 配置装载结果。
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub config: UserConfig,
    /// 磁盘上没有 `UserConfig.json`（Python 的 `qconfig.load` 这时直接返回、**不写盘**）。
    pub created: bool,
    /// 文件存在但读/解析失败 → 退回缺省（Python 的 `@exceptionHandler()` 会中断 load）。
    pub broken: bool,
}

/// 镜像 `qconfig.load(f"{APP_DATA_DIR}/UserConfig.json", cfg)`。
///
/// 文件不存在 → 用缺省配置（`DownloadFolder` 取 `DOWNLOAD_DIR`，与
/// `Config.DownloadFolder` 的 `ConfigItem(default=DOWNLOAD_DIR)` 一致）；
/// 文件损坏 → 同样退回缺省（Python 在首个非法值处中断整个 load，效果相同）。
pub fn load_config(paths: &Paths) -> LoadedConfig {
    let defaults =
        || settings::default_config_with_download_folder(&paths.download_dir.to_string_lossy());
    match UserConfig::load(&paths.user_config_path()) {
        Ok(config) => LoadedConfig {
            config,
            created: false,
            broken: false,
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => LoadedConfig {
            config: defaults(),
            created: true,
            broken: false,
        },
        Err(_) => LoadedConfig {
            config: defaults(),
            created: false,
            broken: true,
        },
    }
}

// ---------------------------------------------------------------------------
// 启动
// ---------------------------------------------------------------------------

/// 是否有可用的显示服务（决定走 winit 后端还是无头软件渲染）。
///
/// Windows 上没有 `DISPLAY`/`WAYLAND_DISPLAY` 这套约定：桌面会话由 winit 直接接管，
/// 交互式进程必然有窗口可用，所以恒为 `true`。非交互场景（服务、计划任务）想跑无头，
/// 请显式传 `--headless`。
#[cfg(windows)]
pub fn has_display() -> bool {
    true
}

/// 是否有可用的显示服务（决定走 winit 后端还是无头软件渲染）。
#[cfg(not(windows))]
pub fn has_display() -> bool {
    ["DISPLAY", "WAYLAND_DISPLAY"].iter().any(|key| {
        std::env::var(key)
            .map(|value| !value.is_empty())
            .unwrap_or(false)
    })
}

/// 启动摘要（无头模式打印一行 + 测试断言）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSummary {
    pub headless: bool,
    pub frames: u32,
    pub config_created: bool,
    pub config_broken: bool,
    pub app_data_dir: PathBuf,
    pub download_dir: PathBuf,
    pub urls: Vec<String>,
    pub silent: bool,
    /// 装载后的任务数（`tasks.jsonl` 里的有效行）。
    pub tasks: usize,
    /// 坏行数（Python 侧只记日志后跳过）。
    pub task_load_failures: usize,
    /// `resumeSaved()` 重新入队的任务数。
    pub resumed: usize,
    /// 停机后处于 `COMPLETED` 的任务数（无头冒烟据此判定「真的下完了」）。
    pub completed: usize,
    /// `--until-idle` 推了多少个 tick（没开就是 0）。
    pub runner_ticks: u32,
    /// 运行期间第二个实例送来的交接消息（`take_handoffs` 的结果）。
    pub handoffs: Vec<crate::single_instance::Handoff>,
}

/// [`run`] 的两种结局。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    /// 本进程是主实例：跑完事件循环、停机、落盘。
    Ran(RunSummary),
    /// 已经有实例在跑：参数（尽力）已交接过去，调用方应当以
    /// [`crate::single_instance::SECOND_INSTANCE_EXIT`] 退出。
    AlreadyRunning(crate::single_instance::Secondary),
}

/// 启动闭环：路径 → 配置 → 单实例锁 → 引擎 → 窗口 → 事件循环 → 停机。
///
/// 无头模式（`--headless` 或无显示服务）用 `MinimalSoftwareWindow` 跑 `frames` 帧后返回；
/// 有显示服务时走 winit 后端、阻塞到窗口关闭（`window.run()`）。两条路径最后都走
/// [`crate::engine::Engine::stop`]。
pub fn run(cli: &Cli) -> Result<RunOutcome, String> {
    let paths = match &cli.data_dir {
        Some(dir) => Paths::with_app_data_dir(dir),
        None => Paths::resolve(Path::new("."), None, None),
    };
    let loaded = load_config(&paths);

    // 单实例锁（`SingletonApplication._lockSingleInstance`）：Python 在
    // `setupEnvironment()` 之后、`startApp()` 之前上锁。第二个实例把参数交接给主实例
    // 之后直接退出（Python：`sys.exit(-1)`）。
    let mut instance = match crate::single_instance::acquire_with_handoff(
        &paths.lock_path(),
        crate::single_instance::plan_from_argv(&cli.positional),
    ) {
        Ok(crate::single_instance::Acquire::Primary(instance)) => instance,
        Ok(crate::single_instance::Acquire::Secondary(secondary)) => {
            return Ok(RunOutcome::AlreadyRunning(secondary));
        }
        Err(err) => return Err(format!("单实例锁创建失败：{err}")),
    };
    let default_download = paths.download_dir.to_string_lossy().to_string();
    // `--until-idle` 隐含无头：它就是给「没有显示服务也要真下载」的冒烟用的。
    let headless = cli.headless || cli.until_idle || !has_display();
    let frames = cli.frames.unwrap_or(1);
    let tasks_path = cli.tasks_file.clone().unwrap_or_else(|| paths.tasks_path());

    // 设置页回写：复用 `load_config` 已经读好的配置，不再读第二次盘 —— 否则
    // 「文件不存在 → 缺省 DownloadFolder 取 DOWNLOAD_DIR」与「文件损坏 → 退回缺省」
    // 这两种情形的区分会丢。
    let writer = Rc::new(crate::settings_writer::SettingsWriter::new(
        crate::config_store::ConfigStore::from_config(paths.user_config_path(), &loaded.config),
    ));
    let bridge =
        crate::settings_bridge::SettingsBridge::new(Rc::clone(&writer), default_download.clone());

    // 引擎放进 `Rc<RefCell<_>>`：设置页的轮询回调要 `&mut engine` 做
    // `refresh_services()`，而 `window.run()` 阻塞在同一栈帧里。
    let engine = Rc::new(RefCell::new(crate::engine::Engine::new(
        &loaded.config,
        &tasks_path,
    )));
    // Python 的 `BrowserService.__init__`（`browser_service.py:189-190`）在令牌为空时
    // `cfg.set(cfg.browserExtensionPairToken, token_urlsafe(16))`，那一步立刻整文件落盘
    // ——所以**首次启动就会写出 `UserConfig.json`**。引擎持的是配置克隆，这里同步回
    // writer（`adopt` 只在真的变了时才写盘）。
    writer.adopt(engine.borrow().config());

    let started = engine
        .borrow_mut()
        .start()
        .map_err(|err| format!("任务列表读取失败：{err}"))?;

    // 写盘之后的落地（`settings_writer.rs` 模块头的第 2 条）：Python 的 `cfg` 是全局
    // 单例、服务每次现读；这里引擎持克隆，所以要把新值推进去再刷新服务。
    bridge.set_drain_handler({
        let engine = Rc::clone(&engine);
        let writer = Rc::downgrade(&writer);
        move |drained| {
            for failure in &drained.failures {
                eprintln!("配置保存失败：{failure}");
            }
            let restart = drained.restart_attrs();
            if !restart.is_empty() {
                eprintln!("以下设置重启后生效：{}", restart.join(" / "));
            }
            if drained.changes.is_empty() && drained.failures.is_empty() {
                return;
            }
            let Some(writer) = writer.upgrade() else {
                return;
            };
            let mut engine = engine.borrow_mut();
            *engine.config_mut() = writer.config();
            engine.refresh_services();
            // `refresh_services` 在令牌为空时会换掉它（`browser_service.py:190`）。
            writer.adopt(engine.config());
        }
    });

    // `--set ATTR=VALUE`：与设置页走**同一条**写盘路径（`SettingsWriter` →
    // `ConfigStore::set` → 整文件回写）。写完再把新值推给引擎 —— 引擎持的是克隆，
    // 不显式同步就只有 UI 变了（同 `set_drain_handler` 里的口径）。
    //
    // 顺序在 `writer.adopt` 之后：`adopt` 要把引擎首次生成的
    // `browserExtensionPairToken` 落盘，用户显式 `--set` 的优先级更高。
    for raw in &cli.sets {
        let (attr, value) = raw
            .split_once('=')
            .expect("parse_args 已经校验过 --set 的形状");
        writer.set_named(attr.trim(), &parse_set_value(value.trim()));
    }
    if !cli.sets.is_empty() {
        let failures = writer.take_failures();
        if !failures.is_empty() {
            return Err(format!(
                "--set 写盘失败：{}",
                failures
                    .iter()
                    .map(|failure| failure.to_string())
                    .collect::<Vec<_>>()
                    .join("；")
            ));
        }
        let mut engine = engine.borrow_mut();
        *engine.config_mut() = writer.config();
        engine.refresh_services();
    }

    // 窗口先建：`Runtime` 要拿它装 tick 定时器。两条驱动路径（无头手动推 / 有头
    // 事件循环）除驱动方式外完全一样，所以装配只写一次。
    let window = if headless {
        crate::create_headless()
    } else {
        crate::AppWindow::new().map_err(|err| format!("窗口创建失败：{err}"))?
    };
    bridge.install(&window);
    crate::settings_dialogs::install(&window, Rc::clone(&writer));
    crate::extension_actions::install(
        &window,
        crate::extension_actions::ExtensionActions::new(
            Rc::clone(&writer),
            paths.extension_folder(),
            Rc::new(crate::extension_actions::SystemEffects),
        ),
    );
    crate::extension_install::install(&window, Rc::new(crate::extension_install::SystemGuide));
    crate::push_settings(&window, &writer.config(), &default_download);
    // 关窗语义（`Software/CloseMode`）：Slint 缺省只隐藏窗口，而 `Runtime` 的
    // 定时器会让事件循环活着 —— 必须显式接管，否则关窗后进程变成看不见的僵尸。
    //
    // 装的是 `install_shared`：`dialogButtonClicked` 这个**单槽**回调留给外壳动作集
    // （第 140 步），关窗链是它分派链的第一站（见 `shell_host::ShellHost::dialog_button`）。
    let close_hooks = crate::close_request::install_shared(&window, Rc::clone(&writer));

    // 草稿对话框（`TaskDraftDialog`）：建任务链路（第 137 步）+ 后台解析
    // （第 138 步）在这里装成真窗口上的对话框（第 139 步）。解析线程起不来只影响
    // 「粘 URL 自动解析」，窗口照常（与托盘同口径的软失败）。
    let draft = {
        let config = engine.borrow().config().clone();
        let defaults = crate::task_create::CreateDefaults::from_config(&config);
        let subworker = gd3_core::settings::read_i64(&config, "preBlockNum").unwrap_or(8) as i32;
        match crate::draft_worker::ParseWorker::spawn(defaults) {
            Ok(worker) => Some(Rc::new(crate::draft_host::DraftUi::build(
                &window,
                Rc::clone(&engine),
                std::sync::Arc::new(worker),
                &default_download,
                &[],
                subworker,
            ))),
            Err(err) => {
                eprintln!("后台解析线程起不来（草稿对话框不可用）：{err}");
                None
            }
        }
    };

    // 外壳动作集（第 140 步）：导航栏（含「新建任务」入口）/ 标题栏搜索框 / 通知条
    // 按钮 / 消息框按钮 / 拖放。`dialogButtonClicked` / `dialogCheckboxToggled` 两个
    // 单槽都归它，关窗链以 `CloseHooks` 的形式串在分派链最前面。
    let shell = crate::shell_host::ShellHost::new(
        Rc::clone(&engine),
        draft.clone(),
        Rc::clone(&close_hooks),
    );
    crate::shell_host::install(&window, Rc::clone(&shell));

    // 下载运行期（`task_runner` → 工作线程 → `gd3-net`）。设置每 tick 现读
    // `engine.config()`，所以设置页改 `maxTaskNum` 下一 tick 就生效（Python 的
    // `_rebalance`）。
    let mut runtime = crate::runtime::Runtime::new(
        Rc::clone(&engine),
        Rc::new(RefCell::new(crate::task_runner::TaskRunner::new(
            crate::task_runner::RunnerSettings::from_config(engine.borrow().config()),
        ))),
        window.clone_strong(),
    );
    runtime.refresh();

    let mut runner_ticks = 0u32;
    if headless {
        // 无头：`slint::Timer` 不会自己走，所以 `--until-idle` 手动按 tick 间隔推进
        // 运行期（真下载）；不带这个开关时保持 `--frames` 的「只启动不派发」冒烟语义。
        if cli.until_idle {
            runner_ticks = runtime.run_until_idle(Duration::from_secs(
                cli.idle_timeout.unwrap_or(IDLE_TIMEOUT),
            ));
        }
        for _ in 0..frames {
            slint::platform::update_timers_and_animations();
            window.window().request_redraw();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        // 帧数很少时 50ms 的轮询定时器可能还没到点，这里补一次（幂等）。
        bridge.drain(&window);
        bridge.stop();
    } else {
        // 有头：窗口事件循环驱动 tick 定时器（`Runtime::install`），阻塞到窗口关闭
        // （Python 的 `app.exec()`）。
        //
        // 用 `run_event_loop_until_quit()` 而不是 `window.run()`：后者内部是
        // `show` + `run_event_loop()` + `hide`，而 `run_event_loop()` 在**最后一个窗口
        // 隐藏时**就自己结束 —— `CloseMode=Background`（藏窗口、进程继续下载）会因此
        // 变成静默退出。第 132 步实测：Background 下进程 rc=0 直接走了。
        runtime.install();
        // 托盘（`tray.py` 的 `SystemTrayIcon`）：`SystemTrayIcon` 是独立顶层组件，
        // 只在这里创建 —— 无头分支既没有事件循环，也没有桌面会话的
        // StatusNotifier watcher。创建失败（比如 `ksni` 连不上会话总线）只记一行，
        // 窗口照常工作（Qt 侧同样不会因为托盘失败而拒绝启动）。
        let tray = match crate::tray::install(
            &window,
            Rc::clone(runtime.engine()),
            Rc::clone(runtime.runner()),
        ) {
            Ok(tray) => Some(tray),
            Err(err) => {
                eprintln!("托盘不可用（窗口照常运行）：{err}");
                None
            }
        };
        // 提示气泡跟着速度表走（`tray.py:201-205` 的 `speedChanged -> setToolTip`）。
        // 运行期没有「服务层信号」，所以借 tick 这个已有的收口点。
        if let Some(tray) = &tray {
            let tray_weak = tray.as_weak();
            runtime.set_tick_hook(Rc::new(move |tick: &crate::runtime::RunnerTick| {
                if let Some(tray) = tray_weak.upgrade() {
                    crate::tray::apply_tooltip(&tray, tick.speed);
                }
            }));
        }
        window
            .show()
            .map_err(|err| format!("窗口显示失败：{err}"))?;
        // 位置参数里的 URL 直接进草稿对话框（`main_window.addUrls(urls)` →
        // `Ghost-Downloader-3.py:191`）。无头分支不这么做：那是冒烟模式，
        // 不该在单测里发真探测。
        if let Some(draft) = &draft {
            draft.open_with_urls(&cli.urls);
        }
        let loop_result = slint::run_event_loop_until_quit();
        runtime.clear_tick_hook();
        let _ = window.hide();
        drop(tray);
        // 对话框的定时器随它一起停（`slint::Timer` 一 drop 就停）。
        drop(draft);
        loop_result.map_err(|err| format!("事件循环失败：{err}"))?;
        bridge.stop();
    }

    // 交接消息（第二个实例送来的 URI / 唤醒）只能在这里取：消费方（打开链接 / 激活
    // 窗口）要等任务草稿链路迁过来才有地方接。Python 在 `exec()` 的 `finally` 里解锁，
    // 位置同样在停机序列之前。
    let handoffs = instance.take_handoffs();
    instance.release();

    // 停机序列（`engine.py:41-49`）：先把在跑的工作线程取消掉（`.ghd` 保住），再走
    // `taskService.stop` → flush → 速度表/监听器/功能包收尾 → 再 flush。窗口一关就走，
    // 与 Qt 的 `aboutToQuit` 同位置。顺序不能反：`Engine.stop` 之后 `TaskService` 已经
    // 收工，晚到的运行期事件没人收。
    for task_id in runtime.shutdown(WORKER_SHUTDOWN_WAIT) {
        eprintln!("任务 {task_id} 未在停机等待内收工（`.ghd` 已保住，下次启动可续传）");
    }
    drop(window);
    engine
        .borrow_mut()
        .stop()
        .map_err(|err| format!("任务列表落盘失败：{err}"))?;

    Ok(RunOutcome::Ran(RunSummary {
        headless,
        frames: if headless { frames } else { 0 },
        config_created: loaded.created,
        config_broken: loaded.broken,
        app_data_dir: paths.app_data_dir,
        download_dir: paths.download_dir,
        urls: cli.urls.clone(),
        silent: cli.silent,
        tasks: engine.borrow().tasks().len(),
        task_load_failures: started.load_failures.len(),
        resumed: started.resumed.len(),
        completed: engine
            .borrow()
            .tasks()
            .records()
            .iter()
            .filter(|record| record.status == TaskStatus::Completed)
            .count(),
        runner_ticks,
        handoffs,
    }))
}

/// `main.rs` 的全部逻辑（放在 lib 里才能被测试与 `cargo run` 共用）。
pub fn main_entry(args: Vec<String>) -> std::process::ExitCode {
    use std::process::ExitCode;

    let cli = match parse_args(args) {
        Ok(cli) => cli,
        Err(message) => {
            eprintln!("gd3-ui: {message}");
            eprint!("{HELP}");
            return ExitCode::from(2);
        }
    };
    if cli.help {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    if cli.version {
        println!("Ghost Downloader v{VERSION}");
        return ExitCode::SUCCESS;
    }
    match run(&cli) {
        Ok(RunOutcome::AlreadyRunning(_)) => {
            // Python：`sys.exit(-1)`，静默退出（参数已经交接给主实例）。
            ExitCode::from(crate::single_instance::SECOND_INSTANCE_EXIT)
        }
        Ok(RunOutcome::Ran(summary)) => {
            if summary.headless {
                println!(
                    "Ghost Downloader v{} launched (headless, {} frame(s), config={}, data={}, tasks={}, resumed={}, completed={}, ticks={})",
                    VERSION,
                    summary.frames,
                    if summary.config_created {
                        "default"
                    } else if summary.config_broken {
                        "invalid->default"
                    } else {
                        "loaded"
                    },
                    summary.app_data_dir.display(),
                    summary.tasks,
                    summary.resumed,
                    summary.completed,
                    summary.runner_ticks,
                );
            }
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("gd3-ui: {message}");
            ExitCode::FAILURE
        }
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    use serde_json::json;

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("gd3-ui-boot-{}-{tag}-{seq}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    /// 测试只关心「真的跑起来了」那一种结局。
    fn ran(cli: &Cli) -> RunSummary {
        match run(cli).expect("无头启动应成功") {
            RunOutcome::Ran(summary) => summary,
            RunOutcome::AlreadyRunning(secondary) => {
                panic!("不该变成第二个实例：{secondary:?}")
            }
        }
    }

    #[test]
    fn parses_every_supported_flag() {
        let cli = parse_args(args(&[
            "--silence",
            "--headless",
            "--frames",
            "4",
            "--data-dir",
            "/tmp/gd3-data",
        ]))
        .expect("解析应成功");
        assert!(cli.silent);
        assert!(cli.headless);
        assert_eq!(cli.frames, Some(4));
        assert_eq!(cli.data_dir, Some(PathBuf::from("/tmp/gd3-data")));
        assert!(!cli.help && !cli.version);
    }

    #[test]
    fn parses_equals_form() {
        let cli = parse_args(args(&["--frames=7", "--data-dir=/tmp/x"])).expect("解析应成功");
        assert_eq!(cli.frames, Some(7));
        assert_eq!(cli.data_dir, Some(PathBuf::from("/tmp/x")));
    }

    #[test]
    fn rejects_unknown_flag() {
        let err = parse_args(args(&["--nope"])).expect_err("应报错");
        assert!(err.contains("未知参数"), "{err}");
    }

    #[test]
    fn rejects_missing_or_bad_frame_count() {
        assert!(parse_args(args(&["--frames"])).is_err());
        assert!(parse_args(args(&["--frames", "abc"])).is_err());
        assert!(parse_args(args(&["--frames", "-1"])).is_err());
    }

    #[test]
    fn wake_uri_turns_on_silent() {
        let cli = parse_args(args(&["ghostdownloader://wake"])).expect("解析应成功");
        assert!(cli.silent);
        // 启动 URI 不进 urls（`urisFromArgv` 跳过 `ghostdownloader://...`）。
        assert!(cli.urls.is_empty());
    }

    #[test]
    fn wake_uri_allows_trailing_slashes_only() {
        assert!(is_wake_uri("ghostdownloader://wake"));
        assert!(is_wake_uri("ghostdownloader://wake///"));
        assert!(!is_wake_uri("GhostDownloader://wake"));
        assert!(!is_wake_uri("ghostdownloader://wake?x=1"));
        assert!(is_launch_uri("ghostdownloader://wake"));
        assert!(!is_launch_uri("https://example.com"));
    }

    #[test]
    fn uris_from_argv_mirrors_python() {
        let dir = temp_dir("uris");
        let file = dir.join("a b.txt");
        std::fs::write(&file, b"x").expect("写临时文件");
        let argv = vec![
            "--silence".to_string(),
            "https://example.com/x?y=1".to_string(),
            "ghostdownloader://wake".to_string(),
            file.to_string_lossy().to_string(),
            "a:b".to_string(),
            "-z".to_string(),
        ];
        let uris = uris_from_argv(&argv);
        assert_eq!(uris.len(), 2, "{uris:?}");
        assert_eq!(uris[0], "https://example.com/x?y=1");
        assert!(uris[1].starts_with("file://"), "{}", uris[1]);
        assert!(uris[1].ends_with("/a%20b.txt"), "{}", uris[1]);
    }

    #[test]
    fn home_join_does_not_normalise_double_slash() {
        // 本机实测 `QStandardPaths::DownloadLocation` = `//Downloads`（HOME 为空 → home="/"）。
        assert_eq!(
            home_path_join("/", "Downloads"),
            PathBuf::from("//Downloads")
        );
        assert_eq!(
            home_path_join("/root", ".local/share"),
            PathBuf::from("/root/.local/share")
        );
    }

    #[test]
    fn portable_mode_when_executable_dir_has_data_dir() {
        let exe = temp_dir("portable");
        std::fs::create_dir_all(exe.join(APP_NAME)).expect("建便携目录");
        let data_home = temp_dir("portable-home");
        let download = temp_dir("portable-dl");
        let paths = Paths::resolve(&exe, Some(&data_home), Some(&download));
        assert!(paths.portable);
        assert_eq!(paths.app_data_dir, exe.join(APP_NAME));
        assert_eq!(paths.user_data_dir, data_home.join(APP_NAME));
        assert_eq!(paths.features_dir, exe.join("features"));
        assert_eq!(paths.download_dir, download);
        assert_eq!(
            paths.user_config_path(),
            exe.join(APP_NAME).join(USER_CONFIG_FILE)
        );
    }

    #[test]
    fn non_portable_mode_uses_generic_data_location() {
        let exe = temp_dir("nonportable");
        let data_home = temp_dir("nonportable-home");
        let download = temp_dir("nonportable-dl");
        let paths = Paths::resolve(&exe, Some(&data_home), Some(&download));
        assert!(!paths.portable);
        assert_eq!(paths.app_data_dir, data_home.join(APP_NAME));
        assert_eq!(paths.user_data_dir, data_home.join(APP_NAME));
        // `startup.py:49`：`APP_DATA_DIR / "browser_extension"`。
        assert_eq!(
            paths.extension_folder(),
            data_home.join(APP_NAME).join(EXTENSION_FOLDER)
        );
    }

    #[test]
    fn missing_config_yields_defaults_without_writing() {
        let dir = temp_dir("missing");
        let paths = Paths::with_app_data_dir(&dir);
        let loaded = load_config(&paths);
        assert!(loaded.created);
        assert!(!loaded.broken);
        assert_eq!(
            settings::read_i64(&loaded.config, "maxTaskNum"),
            Some(3),
            "缺省 MaxTaskNum 应为 3"
        );
        assert_eq!(
            settings::read_text(&loaded.config, "downloadFolder"),
            Some(paths.download_dir.to_string_lossy().to_string())
        );
        assert!(
            !paths.user_config_path().exists(),
            "Python 的 qconfig.load 在文件缺失时不写盘"
        );
    }

    #[test]
    fn existing_config_is_loaded_and_not_flagged_created() {
        let dir = temp_dir("existing");
        let paths = Paths::with_app_data_dir(&dir);
        let mut config = settings::default_config();
        settings::write(&mut config, "maxTaskNum", &serde_json::json!(7));
        config.save(&paths.user_config_path()).expect("写配置");

        let loaded = load_config(&paths);
        assert!(!loaded.created);
        assert!(!loaded.broken);
        assert_eq!(settings::read_i64(&loaded.config, "maxTaskNum"), Some(7));
    }

    #[test]
    fn broken_config_falls_back_to_defaults() {
        let dir = temp_dir("broken");
        let paths = Paths::with_app_data_dir(&dir);
        std::fs::write(paths.user_config_path(), b"{").expect("写坏配置");
        let loaded = load_config(&paths);
        assert!(!loaded.created);
        assert!(loaded.broken, "文件存在但解析失败 → broken=true");
        assert_eq!(settings::read_i64(&loaded.config, "maxTaskNum"), Some(3));
    }

    #[test]
    fn headless_run_smoke() {
        let dir = temp_dir("run");
        let cli = Cli {
            headless: true,
            frames: Some(2),
            data_dir: Some(dir.clone()),
            urls: vec!["https://example.com/a".to_string()],
            ..Cli::default()
        };
        let summary = ran(&cli);
        assert!(summary.headless);
        assert_eq!(summary.frames, 2);
        assert!(summary.config_created);
        assert!(!summary.config_broken);
        assert_eq!(summary.app_data_dir, dir);
        assert_eq!(summary.urls, vec!["https://example.com/a".to_string()]);
        assert!(!summary.silent);
        // Python 的 `BrowserService.__init__` 在令牌为空时生成一个并 `cfg.set` 落盘，
        // 所以首次启动会写出 `UserConfig.json`（第 118 步接线后才与 Python 对齐）。
        let written = UserConfig::load(&dir.join(USER_CONFIG_FILE)).expect("启动后应当写出配置");
        let token = written
            .get_str("Browser", "BrowserExtensionPairToken")
            .expect("令牌应当落盘");
        assert_eq!(token.chars().count(), 22, "token_urlsafe(16) 是 22 个字符");
        assert_eq!(settings::read_i64(&written, "maxTaskNum"), Some(3));
    }

    #[test]
    fn headless_run_with_zero_frames_is_allowed() {
        let dir = temp_dir("run0");
        let cli = Cli {
            headless: true,
            frames: Some(0),
            data_dir: Some(dir),
            ..Cli::default()
        };
        let summary = ran(&cli);
        assert_eq!(summary.frames, 0);
    }

    /// `tasks.jsonl` 的一行（其余字段走 serde 缺省）。
    fn task_line(task_id: &str, status: &str) -> String {
        format!(
            r#"{{"name":"{task_id}.bin","url":"https://example.com/{task_id}","packId":"","taskId":"{task_id}","status":"{status}","fileSize":10}}"#
        )
    }

    #[test]
    fn parses_tasks_file_flag_in_both_forms() {
        let cli = parse_args(args(&["--tasks-file", "/tmp/t.jsonl"])).expect("解析应成功");
        assert_eq!(cli.tasks_file, Some(PathBuf::from("/tmp/t.jsonl")));
        let cli = parse_args(args(&["--tasks-file=/tmp/t.jsonl"])).expect("解析应成功");
        assert_eq!(cli.tasks_file, Some(PathBuf::from("/tmp/t.jsonl")));
        assert!(parse_args(args(&["--tasks-file"])).is_err(), "缺取值应报错");
    }

    #[test]
    fn lock_path_lives_in_app_data_dir() {
        let dir = temp_dir("lock-path");
        let paths = Paths::with_app_data_dir(&dir);
        assert_eq!(
            paths.lock_path(),
            dir.join(crate::single_instance::LOCK_FILE)
        );
        assert_eq!(crate::single_instance::LOCK_FILE, "instance.sock");
    }

    #[test]
    fn second_instance_in_the_same_data_dir_hands_off() {
        use crate::single_instance::{Acquire, Handoff};

        let dir = temp_dir("second");
        let lock = dir.join(crate::single_instance::LOCK_FILE);
        let mut instance = match crate::single_instance::acquire(&lock).expect("抢锁") {
            Acquire::Primary(instance) => instance,
            Acquire::Secondary(_) => panic!("应当抢到锁"),
        };
        let cli = Cli {
            headless: true,
            frames: Some(0),
            data_dir: Some(dir.clone()),
            positional: vec!["https://example.com/x".to_string()],
            ..Cli::default()
        };
        match run(&cli).expect("第二个实例") {
            RunOutcome::Ran(_) => panic!("第二个实例不该跑起来"),
            RunOutcome::AlreadyRunning(secondary) => {
                assert_eq!(
                    secondary.handoff,
                    Some(Handoff::OpenUris(vec!["https://example.com/x".to_string()]))
                );
                assert!(secondary.delivered, "载荷应当写进 socket");
            }
        }
        // 收件箱由另一个线程填，等一会儿。
        let mut handoffs = Vec::new();
        for _ in 0..200 {
            handoffs = instance.take_handoffs();
            if !handoffs.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            handoffs,
            vec![Handoff::OpenUris(vec!["https://example.com/x".to_string()])]
        );
        instance.release();
    }

    #[test]
    fn tasks_path_defaults_to_app_data_dir() {
        let dir = temp_dir("tasks-path");
        let paths = Paths::with_app_data_dir(&dir);
        assert_eq!(paths.tasks_path(), dir.join(TASKS_FILE));
        assert_eq!(TASKS_FILE, "tasks.jsonl");
    }

    #[test]
    fn headless_run_loads_tasks_from_app_data_dir() {
        let dir = temp_dir("tasks");
        let tasks_path = dir.join(TASKS_FILE);
        let body = [
            task_line("a", "WAITING"),
            task_line("b", "RUNNING"),
            task_line("c", "COMPLETED"),
            "{bad json".to_string(),
        ]
        .join("\n")
            + "\n";
        std::fs::write(&tasks_path, body.as_bytes()).expect("写任务文件");

        let cli = Cli {
            headless: true,
            frames: Some(1),
            data_dir: Some(dir.clone()),
            ..Cli::default()
        };
        let summary = ran(&cli);
        assert_eq!(summary.tasks, 3, "坏行不算任务");
        assert_eq!(summary.task_load_failures, 1, "坏行只计数");
        assert_eq!(summary.resumed, 2, "WAITING + RUNNING 重新入队");
        // 启动本身不写盘；写盘发生在停机（`Engine.stop` 的第一次 flush）。
        // 与 Python 一致：`stop()` 把 RUNNING/WAITING 标成 PAUSED，COMPLETED 不动。
        let text = std::fs::read_to_string(&tasks_path).expect("读回");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        assert!(lines[0].contains("\"PAUSED\""), "{text}");
        assert!(lines[1].contains("\"PAUSED\""), "{text}");
        assert!(lines[2].contains("\"COMPLETED\""), "{text}");
    }

    #[test]
    fn parses_set_flags_in_both_forms() {
        let cli = parse_args(args(&[
            "--set",
            "maxTaskNum=5",
            "--set=downloadFolder=/tmp/dl",
        ]))
        .expect("解析应成功");
        assert_eq!(
            cli.sets,
            vec![
                "maxTaskNum=5".to_string(),
                "downloadFolder=/tmp/dl".to_string()
            ]
        );
        assert!(Cli::default().sets.is_empty());
    }

    #[test]
    fn rejects_a_set_flag_without_an_equals_sign() {
        let err = parse_args(args(&["--set", "maxTaskNum"])).expect_err("应报错");
        assert!(err.contains("ATTR=VALUE"), "{err}");
        assert!(parse_args(args(&["--set", "=5"])).is_err(), "属性名不能空");
        assert!(parse_args(args(&["--set"])).is_err(), "缺取值");
        assert!(parse_args(args(&["--set="])).is_err());
    }

    #[test]
    fn set_values_take_json_when_they_look_like_json() {
        assert_eq!(parse_set_value("5"), json!(5));
        assert_eq!(parse_set_value("true"), json!(true));
        assert_eq!(parse_set_value("[1, 2]"), json!([1, 2]));
        assert_eq!(parse_set_value("\"text\""), json!("text"));
        // 裸写就是字符串（路径里不可能带引号）。
        assert_eq!(parse_set_value("/tmp/a b"), json!("/tmp/a b"));
        assert_eq!(parse_set_value("auto"), json!("auto"));
    }

    #[test]
    fn headless_run_applies_set_to_the_config_file() {
        let dir = temp_dir("set");
        let cli = Cli {
            headless: true,
            frames: Some(0),
            data_dir: Some(dir.clone()),
            sets: vec![
                "maxTaskNum=5".to_string(),
                "clientProfile=chrome".to_string(),
            ],
            ..Cli::default()
        };
        let summary = ran(&cli);
        assert!(summary.config_created, "首次启动会把配置写出来");

        let written =
            UserConfig::load(&dir.join(USER_CONFIG_FILE)).expect("--set 之后应当有配置文件");
        assert_eq!(settings::read_i64(&written, "maxTaskNum"), Some(5));
        assert_eq!(
            settings::read_text(&written, "clientProfile").as_deref(),
            Some("chrome")
        );
        // 没动过的项保持缺省。
        assert_eq!(
            settings::read_i64(&written, "preBlockNum"),
            settings::read_i64(&settings::default_config(), "preBlockNum")
        );
    }

    #[test]
    fn an_unknown_set_attribute_fails_the_run() {
        let dir = temp_dir("set-unknown");
        let cli = Cli {
            headless: true,
            frames: Some(0),
            data_dir: Some(dir),
            sets: vec!["noSuchAttribute=1".to_string()],
            ..Cli::default()
        };
        let err = run(&cli).expect_err("未知属性名要报错");
        assert!(err.contains("--set 写盘失败"), "{err}");
        assert!(err.contains("noSuchAttribute"), "{err}");
    }

    #[test]
    fn tasks_file_override_wins_over_app_data_dir() {
        let dir = temp_dir("tasks-override");
        let tasks_path = dir.join("elsewhere.jsonl");
        std::fs::write(&tasks_path, (task_line("x", "WAITING") + "\n").as_bytes())
            .expect("写任务文件");
        let cli = Cli {
            headless: true,
            frames: Some(0),
            data_dir: Some(dir.join("data")),
            tasks_file: Some(tasks_path),
            ..Cli::default()
        };
        let summary = ran(&cli);
        assert_eq!(summary.tasks, 1);
        assert_eq!(summary.resumed, 1);
    }
}
