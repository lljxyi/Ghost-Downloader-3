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
//! 路径解析 → 配置装载 → 窗口实例化（配置推送到 UI）→ 事件循环
//! ```
//!
//! 未迁移（后续小步）：单实例锁、D-Bus/URL scheme 接收、`app/engine.py` 全部服务、
//! 托盘/`desktop_keepalive`/通知、功能包加载、更新检查、OOBE 分支、多语言翻译器。
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
//! - 启动时读 `<APP_DATA_DIR>/tasks.jsonl`（与 Python `TaskStore._path` 同路径）并调
//!   `TaskService::resume_saved()` 重新入队；但**没有引擎**，所以只入队不派发
//!   （Python 的 `_schedule` 会顺带 `_pump`，那一步等引擎迁过来）；
//! - `--tasks-file` 是 Rust 侧的可测性开关（Python 侧固定用 `APP_DATA_DIR`）。

use std::path::{Path, PathBuf};

use gd3_core::config::UserConfig;
use gd3_core::settings;
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
    pub help: bool,
    pub version: bool,
    /// 位置参数里识别出的 URL / 文件 URI（`application.py::urisFromArgv`）。
    pub urls: Vec<String>,
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
            other if other.starts_with("--frames=") => {
                cli.frames = Some(parse_frames(&other["--frames=".len()..])?);
            }
            other if other.starts_with("--data-dir=") => {
                cli.data_dir = Some(PathBuf::from(&other["--data-dir=".len()..]));
            }
            other if other.starts_with("--tasks-file=") => {
                cli.tasks_file = Some(PathBuf::from(&other["--tasks-file=".len()..]));
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
    Ok(cli)
}

fn parse_frames(value: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .map_err(|_| format!("--frames 需要非负整数，收到：{value}"))
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
}

/// 启动闭环：路径 → 配置 → 窗口 → 事件循环。
///
/// 无头模式（`--headless` 或无显示服务）用 `MinimalSoftwareWindow` 跑 `frames` 帧后返回；
/// 有显示服务时走 winit 后端、阻塞到窗口关闭（`window.run()`）。
pub fn run(cli: &Cli) -> Result<RunSummary, String> {
    let paths = match &cli.data_dir {
        Some(dir) => Paths::with_app_data_dir(dir),
        None => Paths::resolve(Path::new("."), None, None),
    };
    let loaded = load_config(&paths);
    let default_download = paths.download_dir.to_string_lossy().to_string();
    let headless = cli.headless || !has_display();
    let frames = cli.frames.unwrap_or(1);
    let tasks_path = cli.tasks_file.clone().unwrap_or_else(|| paths.tasks_path());
    let (mut service, task_failures) = crate::task_service::TaskService::load(&tasks_path)
        .map_err(|err| format!("任务列表读取失败：{err}"))?;
    let resumed = service.resume_saved();

    if headless {
        let window = crate::create_headless();
        crate::push_settings(&window, &loaded.config, &default_download);
        service.refresh(&window);
        for _ in 0..frames {
            slint::platform::update_timers_and_animations();
            window.window().request_redraw();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    } else {
        let window = crate::AppWindow::new().map_err(|err| format!("窗口创建失败：{err}"))?;
        crate::push_settings(&window, &loaded.config, &default_download);
        service.refresh(&window);
        window.run().map_err(|err| format!("事件循环失败：{err}"))?;
    }

    Ok(RunSummary {
        headless,
        frames: if headless { frames } else { 0 },
        config_created: loaded.created,
        config_broken: loaded.broken,
        app_data_dir: paths.app_data_dir,
        download_dir: paths.download_dir,
        urls: cli.urls.clone(),
        silent: cli.silent,
        tasks: service.len(),
        task_load_failures: task_failures.len(),
        resumed: resumed.len(),
    })
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
        Ok(summary) => {
            if summary.headless {
                println!(
                    "Ghost Downloader v{} launched (headless, {} frame(s), config={}, data={}, tasks={}, resumed={})",
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
        let summary = run(&cli).expect("无头启动应成功");
        assert!(summary.headless);
        assert_eq!(summary.frames, 2);
        assert!(summary.config_created);
        assert!(!summary.config_broken);
        assert_eq!(summary.app_data_dir, dir);
        assert_eq!(summary.urls, vec!["https://example.com/a".to_string()]);
        assert!(!summary.silent);
        assert!(!dir.join(USER_CONFIG_FILE).exists());
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
        let summary = run(&cli).expect("无头启动应成功");
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
        let before = std::fs::read(&tasks_path).expect("读回");

        let cli = Cli {
            headless: true,
            frames: Some(1),
            data_dir: Some(dir.clone()),
            ..Cli::default()
        };
        let summary = run(&cli).expect("无头启动应成功");
        assert_eq!(summary.tasks, 3, "坏行不算任务");
        assert_eq!(summary.task_load_failures, 1, "坏行只计数");
        assert_eq!(summary.resumed, 2, "WAITING + RUNNING 重新入队");
        assert_eq!(
            std::fs::read(&tasks_path).expect("读回"),
            before,
            "启动不回写 tasks.jsonl（Python 只在改动后 flush）"
        );
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
        let summary = run(&cli).expect("无头启动应成功");
        assert_eq!(summary.tasks, 1);
        assert_eq!(summary.resumed, 1);
    }
}
