//! 单实例锁 + 第二次启动的参数交接（`app/platform/application.py::SingletonApplication` 的镜像）。
//!
//! Python 的机制是两套叠加：
//!
//! - **锁**：`QSharedMemory(DESKTOP_ID)`，`attach()` 成功 = 已有实例 → `sys.exit(-1)`；
//! - **通道**：Windows 用 `WM_COPYDATA`（`COPYDATA_OPEN_FILES`）/ `WM_USER_WAKE`，
//!   Linux 用会话总线上的 `org.freedesktop.Application.Open` / `Activate`。
//!
//! 这里换成一个 **Unix domain socket**：`bind()` 成功即抢到锁，`connect()` 成功即
//! 「已有实例」，同一条连接顺带把参数送过去 —— 一个机制同时给出锁与通道，不依赖
//! D-Bus、不依赖显示器，能在构建机上测。
//!
//! # 协议
//!
//! ```text
//! gd3-ipc/1 activate\n              ← 只唤醒已有窗口（Python 的 Activate / WM_USER_WAKE）
//! gd3-ipc/1 open\n<uri>\n<uri>\n    ← 把 URI 交给已有实例（Python 的 Open / COPYDATA_OPEN_FILES）
//! ```
//!
//! 首行必须是 `gd3-ipc/1`、第二行是动作名；不认识的载荷直接丢弃（半截连接、外部进程
//! 误连都不算消息）。`open` 的 URI 逐行读到 EOF，空行与 `ghostdownloader://wake` 被过滤
//! （镜像 `_DesktopBusReceiver.Open` 的第一行）；过滤后为空 → 丢弃；含
//! `ghostdownloader://...` 启动 URI → 升级成 `activate`（同 `Open` 里的 `isLaunchUri`
//! 分支）。全空载荷 = 唤醒连接，不产生消息。
//!
//! # 与 Python 的差异
//!
//! - **锁的位置**：Python 是 `QSharedMemory` 的全局 key；这里是 `APP_DATA_DIR/instance.sock`。
//!   两者都是「每用户一份」，但换 `--data-dir` 就会换锁（Python 不会）—— 便携模式与普通
//!   模式因此可以同时跑；
//! - **Windows 后端没实现**（`WM_COPYDATA` 那套要 `win32gui`）；本项目只在 Linux 上跑；
//! - **不注册 D-Bus 服务名**：Python 在 Linux 上把 `DESKTOP_ID` 注册成
//!   `org.freedesktop.Application`，让桌面环境直接调 `Open`/`Activate`。这里没有注册，
//!   从桌面双击 `ghostdownloader://` 链接进来仍然是「第二个实例 → socket 交接」，
//!   结果等价（都到主实例），但少一次 D-Bus 名字占用；
//! - **交接消息在事件循环结束之后才被取出**（`RunSummary.handoffs`）：消费方（打开链接 /
//!   激活窗口）要等任务草稿链路迁过来才有地方接，所以长跑期间到达的消息先留在收件箱；
//! - **`SIGINT` 处理（`_onInterrupt`）没有搬**：Rust 侧没有 Qt 事件循环，Ctrl-C 直接终止
//!   进程；socket 文件靠下面的陈旧检测回收；
//! - **退出码**：Python `sys.exit(-1)` → 这里 `ExitCode::from(255)`（`-1 & 0xff`）。
//!
//! # 陈旧 socket
//!
//! 进程被 `SIGKILL` 掉会留下 socket 文件，此后 `bind()` 报 `AddrInUse`。所以
//! [`acquire_with_handoff`] 在 `AddrInUse` 之后先 `connect()` 探活：连不上
//! （`ConnectionRefused`/`NotFound`）就认定是陈旧文件，删掉重试一次 `bind`。
//! 这与 Python 的 `QSharedMemory` 陈旧处理（先 `attach`/`detach` 清一遍）意图相同。

use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use crate::boot::{is_launch_uri, is_wake_uri, uris_from_argv};

/// 锁文件 / socket 文件名（放在 `APP_DATA_DIR` 下）。
pub const LOCK_FILE: &str = "instance.sock";

/// 协议首行。
pub const PROTOCOL: &str = "gd3-ipc/1";

/// 第二个实例的退出码（Python `sys.exit(-1)`）。
pub const SECOND_INSTANCE_EXIT: u8 = 255;

/// 单条消息的读取上限（一堆 URI 而已，超过就当垃圾丢弃）。
const MAX_PAYLOAD: u64 = 1 << 20;

// ---------------------------------------------------------------------------
// 交接内容
// ---------------------------------------------------------------------------

/// 第二个实例要交给主实例的东西。
///
/// 对应 Python 的两条信号：`activationRequested`（把窗口提到前台）与
/// `openUriRequested(uris)`（把链接塞进任务草稿）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handoff {
    /// 只唤醒已有窗口（`Activate` / `WM_USER_WAKE`）。
    Activate,
    /// 把 URI 交给已有实例（`Open` / `COPYDATA_OPEN_FILES`）。
    OpenUris(Vec<String>),
}

impl Handoff {
    /// 序列化成线格式（见模块头的协议一节）。
    pub fn to_payload(&self) -> String {
        let mut out = String::from(PROTOCOL);
        match self {
            Handoff::Activate => out.push_str("\nactivate\n"),
            Handoff::OpenUris(uris) => {
                out.push_str("\nopen\n");
                for uri in uris {
                    out.push_str(uri);
                    out.push('\n');
                }
            }
        }
        out
    }

    /// 反序列化 + 镜像 `_DesktopBusReceiver.Open` 的过滤规则。
    ///
    /// 返回 `None` = 不是一条有效消息（空载荷、协议不符、过滤后没剩下 URI）。
    pub fn parse_payload(text: &str) -> Option<Handoff> {
        let mut lines = text.split('\n');
        if lines.next()? != PROTOCOL {
            return None;
        }
        match lines.next()? {
            "activate" => Some(Handoff::Activate),
            "open" => {
                let uris: Vec<String> = lines
                    .filter(|line| !line.is_empty() && !is_wake_uri(line))
                    .map(str::to_string)
                    .collect();
                if uris.is_empty() {
                    return None;
                }
                if uris.iter().any(|uri| is_launch_uri(uri)) {
                    return Some(Handoff::Activate);
                }
                Some(Handoff::OpenUris(uris))
            }
            _ => None,
        }
    }
}

/// `_lockSingleInstance` 里「要不要给主实例发东西」的那一段。
///
/// `args` 是**位置参数**（不含 argv[0]，与 [`uris_from_argv`] 同口径）：
///
/// - `argv[1:]` 里有 wake URI → `None`（Python：不发任何 IPC 直接退出）；
/// - 否则有 URI → `OpenUris`；没有 URI → `Activate`（Python：`Open` vs `Activate`）。
pub fn plan_from_argv(args: &[String]) -> Option<Handoff> {
    if args.iter().any(|arg| is_wake_uri(arg)) {
        return None;
    }
    let uris = uris_from_argv(args);
    if uris.is_empty() {
        Some(Handoff::Activate)
    } else {
        Some(Handoff::OpenUris(uris))
    }
}

// ---------------------------------------------------------------------------
// 抢锁
// ---------------------------------------------------------------------------

/// 第二个实例的结果：`handoff` 是「本打算发什么」，`delivered` 是「真的写进去了没有」。
///
/// Python 侧 `interface.call(...)` 的返回值不看、`SendMessage` 也不看 —— 发送失败不
/// 影响「已经有实例了，我该退出」这个结论。`delivered` 只用于测试与排障。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Secondary {
    pub handoff: Option<Handoff>,
    pub delivered: bool,
}

/// [`acquire_with_handoff`] 的结果。
pub enum Acquire {
    /// 本进程是主实例，持锁直到 [`Instance::release`] / `Drop`。
    Primary(Instance),
    /// 已经有实例在跑；参数（尽力）已交接过去，调用方应当退出。
    Secondary(Secondary),
}

/// 抢锁；成为第二个实例时把 `handoff` 交给主实例。
pub fn acquire_with_handoff(path: &Path, handoff: Option<Handoff>) -> std::io::Result<Acquire> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    match UnixListener::bind(path) {
        Ok(listener) => Ok(Acquire::Primary(Instance::spawn(path, listener))),
        Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
            match connect_and_send(path, handoff.as_ref()) {
                Ok(delivered) => Ok(Acquire::Secondary(Secondary { handoff, delivered })),
                Err(err)
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                    ) =>
                {
                    // 陈旧 socket：没人监听 → 删掉重试一次。
                    let _ = fs::remove_file(path);
                    let listener = UnixListener::bind(path)?;
                    Ok(Acquire::Primary(Instance::spawn(path, listener)))
                }
                Err(err) => Err(err),
            }
        }
        Err(err) => Err(err),
    }
}

/// 只抢锁、不交接（测试与「不需要传参」的入口用）。
pub fn acquire(path: &Path) -> std::io::Result<Acquire> {
    acquire_with_handoff(path, None)
}

/// 连上去把载荷写进去；`handoff` 为 `None` 时只探活（Python 在 wake URI 分支里
/// 一个字节都不发，但仍然要判断「有没有实例在跑」）。
fn connect_and_send(path: &Path, handoff: Option<&Handoff>) -> std::io::Result<bool> {
    let mut stream = UnixStream::connect(path)?;
    let Some(handoff) = handoff else {
        return Ok(false);
    };
    stream.write_all(handoff.to_payload().as_bytes())?;
    stream.flush()?;
    Ok(true)
}

// ---------------------------------------------------------------------------
// 主实例
// ---------------------------------------------------------------------------

/// 主实例手里的锁 + 收件箱。`Drop` 时自动释放（与 Python `exec()` 的
/// `finally: _unlockSingleInstance()` 同位置）。
pub struct Instance {
    path: PathBuf,
    inbox: Arc<Mutex<Vec<Handoff>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Instance {
    fn spawn(path: &Path, listener: UnixListener) -> Self {
        let inbox = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let inbox = Arc::clone(&inbox);
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                for stream in listener.incoming() {
                    // `release()` 会自连一次把 `accept()` 唤醒，此时直接收工。
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(mut stream) = stream else { continue };
                    let mut text = String::new();
                    if (&mut stream)
                        .take(MAX_PAYLOAD)
                        .read_to_string(&mut text)
                        .is_err()
                    {
                        continue;
                    }
                    if let Some(handoff) = Handoff::parse_payload(&text) {
                        inbox.lock().expect("收件箱锁中毒").push(handoff);
                    }
                }
            })
        };
        Self {
            path: path.to_path_buf(),
            inbox,
            stop,
            thread: Some(thread),
        }
    }

    /// socket 路径（= 锁的位置）。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 收件箱的共享句柄（给事件循环里的轮询用；当前没有消费方）。
    pub fn inbox(&self) -> Arc<Mutex<Vec<Handoff>>> {
        Arc::clone(&self.inbox)
    }

    /// 取走已到达的交接消息（破坏性：取过就没了）。
    pub fn take_handoffs(&self) -> Vec<Handoff> {
        let mut guard = self.inbox.lock().expect("收件箱锁中毒");
        std::mem::take(&mut *guard)
    }

    /// 释放锁：停线程 → join → 删 socket 文件。可重复调用。
    pub fn release(&mut self) {
        let Some(handle) = self.thread.take() else {
            return;
        };
        self.stop.store(true, Ordering::SeqCst);
        // 自连一次解开阻塞的 `accept()`。
        let _ = UnixStream::connect(&self.path);
        let _ = handle.join();
        let _ = fs::remove_file(&self.path);
    }

    /// 锁是否已经放掉。
    pub fn is_released(&self) -> bool {
        self.thread.is_none()
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        self.release();
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;
    use std::time::{Duration, Instant};

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("gd3-ui-si-{}-{tag}-{seq}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    /// 收件箱是另一个线程在填，测试要等一会儿。
    fn wait_handoffs(instance: &Instance, want: usize) -> Vec<Handoff> {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut got = Vec::new();
        while got.len() < want && Instant::now() < deadline {
            got.extend(instance.take_handoffs());
            if got.len() < want {
                thread::sleep(Duration::from_millis(5));
            }
        }
        got
    }

    #[test]
    fn primary_acquires_and_secondary_hands_off() {
        let dir = temp_dir("handoff");
        let path = dir.join(LOCK_FILE);
        let instance = match acquire(&path).expect("抢锁") {
            Acquire::Primary(instance) => instance,
            Acquire::Secondary(_) => panic!("第一个进程应当是主实例"),
        };
        assert_eq!(instance.path(), path);

        let uris = vec![
            "https://example.com/a".to_string(),
            "https://example.com/b".to_string(),
        ];
        let second =
            acquire_with_handoff(&path, Some(Handoff::OpenUris(uris.clone()))).expect("第二个实例");
        match second {
            Acquire::Primary(_) => panic!("第二个进程不该抢到锁"),
            Acquire::Secondary(secondary) => {
                assert_eq!(secondary.handoff, Some(Handoff::OpenUris(uris.clone())));
                assert!(secondary.delivered, "载荷应当写进去了");
            }
        }
        assert_eq!(
            wait_handoffs(&instance, 1),
            vec![Handoff::OpenUris(uris)],
            "主实例应当收到交接"
        );
    }

    #[test]
    fn secondary_without_handoff_still_detects_the_running_instance() {
        let dir = temp_dir("probe");
        let path = dir.join(LOCK_FILE);
        let instance = match acquire(&path).expect("抢锁") {
            Acquire::Primary(instance) => instance,
            Acquire::Secondary(_) => panic!("应当抢到锁"),
        };
        // Python 在 wake URI 分支里什么都不发，但仍要退出。
        let second = acquire(&path).expect("探活");
        match second {
            Acquire::Primary(_) => panic!("不该抢到锁"),
            Acquire::Secondary(secondary) => {
                assert_eq!(secondary.handoff, None);
                assert!(!secondary.delivered);
            }
        }
        assert!(wait_handoffs(&instance, 1).is_empty(), "不该收到消息");
    }

    #[test]
    fn release_removes_socket_file_and_allows_reacquire() {
        let dir = temp_dir("release");
        let path = dir.join(LOCK_FILE);
        let mut instance = match acquire(&path).expect("抢锁") {
            Acquire::Primary(instance) => instance,
            Acquire::Secondary(_) => panic!("应当抢到锁"),
        };
        assert!(path.exists());
        instance.release();
        assert!(instance.is_released());
        assert!(!path.exists(), "释放后应当删掉 socket 文件");
        instance.release(); // 可重复调用

        let again = acquire(&path).expect("重新抢锁");
        assert!(matches!(again, Acquire::Primary(_)));
    }

    #[test]
    fn stale_socket_is_reclaimed() {
        let dir = temp_dir("stale");
        let path = dir.join(LOCK_FILE);
        // 造一个「监听者已经没了、文件还在」的陈旧 socket。
        {
            let listener = UnixListener::bind(&path).expect("建 socket");
            drop(listener);
        }
        assert!(path.exists(), "Linux 上 drop 不会删 socket 文件");

        match acquire(&path).expect("抢锁") {
            Acquire::Primary(instance) => assert_eq!(instance.path(), path),
            Acquire::Secondary(_) => panic!("陈旧 socket 应当被回收成主实例"),
        }
    }

    #[test]
    fn take_handoffs_is_destructive() {
        let dir = temp_dir("drain");
        let path = dir.join(LOCK_FILE);
        let instance = match acquire(&path).expect("抢锁") {
            Acquire::Primary(instance) => instance,
            Acquire::Secondary(_) => panic!("应当抢到锁"),
        };
        acquire_with_handoff(&path, Some(Handoff::Activate)).expect("第二个实例");
        assert_eq!(wait_handoffs(&instance, 1), vec![Handoff::Activate]);
        assert!(instance.take_handoffs().is_empty(), "取过就没了");
    }

    #[test]
    fn payload_roundtrip() {
        let cases = [
            Handoff::Activate,
            Handoff::OpenUris(vec!["https://example.com/a".to_string()]),
            Handoff::OpenUris(vec![
                "https://example.com/a".to_string(),
                "file:///tmp/x.txt".to_string(),
            ]),
        ];
        for handoff in cases {
            let text = handoff.to_payload();
            assert_eq!(
                Handoff::parse_payload(&text),
                Some(handoff.clone()),
                "{text}"
            );
        }
    }

    #[test]
    fn parse_payload_mirrors_the_dbus_open_slot() {
        // 空载荷（唤醒连接）→ 不是消息。
        assert_eq!(Handoff::parse_payload(""), None);
        // 协议不符 / 半截连接 → 不是消息。
        assert_eq!(Handoff::parse_payload("hello\n"), None);
        assert_eq!(Handoff::parse_payload("gd3-ipc/1\n"), None);
        assert_eq!(Handoff::parse_payload("gd3-ipc/1\nopen\n"), None);
        assert_eq!(Handoff::parse_payload("gd3-ipc/1\nnope\n"), None);
        // 全是 wake URI → 过滤后为空 → 不是消息。
        assert_eq!(
            Handoff::parse_payload("gd3-ipc/1\nopen\nghostdownloader://wake\n\n"),
            None
        );
        // 混了 wake URI → 只留下真 URI。
        assert_eq!(
            Handoff::parse_payload("gd3-ipc/1\nopen\nghostdownloader://wake\nhttps://a\n"),
            Some(Handoff::OpenUris(vec!["https://a".to_string()]))
        );
        // 含启动 URI → 升级成 activate。
        assert_eq!(
            Handoff::parse_payload("gd3-ipc/1\nopen\nghostdownloader://open?u=x\n"),
            Some(Handoff::Activate)
        );
    }

    #[test]
    fn plan_from_argv_mirrors_python() {
        // 有 wake URI → 什么都不发。
        assert_eq!(
            plan_from_argv(&args(&["ghostdownloader://wake", "https://a"])),
            None
        );
        // 有 URI → OpenUris。
        assert_eq!(
            plan_from_argv(&args(&["https://a"])),
            Some(Handoff::OpenUris(vec!["https://a".to_string()]))
        );
        // 只有启动 URI / 开关 → Activate。
        assert_eq!(plan_from_argv(&args(&[])), Some(Handoff::Activate));
        assert_eq!(
            plan_from_argv(&args(&["--silence", "ghostdownloader://open"])),
            Some(Handoff::Activate)
        );
    }

    #[test]
    fn acquire_creates_missing_parent_directory() {
        let dir = temp_dir("mkdir");
        let path = dir.join("nested").join("deeper").join(LOCK_FILE);
        match acquire(&path).expect("抢锁") {
            Acquire::Primary(instance) => {
                assert!(path.exists());
                drop(instance);
            }
            Acquire::Secondary(_) => panic!("应当抢到锁"),
        }
    }
}
