//! 任务错误：镜像 `app/models/task.py` 的 `TaskError`。
//!
//! `message` 是 i18n 键，`params` 供格式化——UI 才能沿用同一份翻译。
//! 文案与 Python 侧逐字一致，改这里就要同步改 `app/i18n.py` 的 `TaskErrors`。

use std::fmt;

/// 取消（暂停）时的错误文案，按它区分「暂停」与「失败」。
pub const CANCELLED_MESSAGE: &str = "下载已取消";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskError {
    pub message: &'static str,
    pub params: Vec<(&'static str, String)>,
}

impl TaskError {
    pub fn new(message: &'static str, params: Vec<(&'static str, String)>) -> Self {
        TaskError { message, params }
    }

    pub fn server_error(status: u16) -> Self {
        TaskError {
            message: "服务器返回了错误（{status}）",
            params: vec![("status", status.to_string())],
        }
    }

    pub fn disk_full() -> Self {
        TaskError {
            message: "磁盘空间不足",
            params: Vec::new(),
        }
    }

    pub fn range_rejected(status: u16) -> Self {
        TaskError {
            message: "服务器拒绝了范围请求，状态码：{status}",
            params: vec![("status", status.to_string())],
        }
    }

    pub fn unexpected_status(status: u16) -> Self {
        TaskError {
            message: "服务器返回了异常状态码：{status}",
            params: vec![("status", status.to_string())],
        }
    }

    pub fn cancelled() -> Self {
        TaskError {
            message: CANCELLED_MESSAGE,
            params: Vec::new(),
        }
    }

    /// 端口被占用（`app/services/port_listener.py:toListenError`）。
    pub fn port_in_use(port: u16) -> Self {
        TaskError {
            message: "端口 {port} 被占用，请更换端口",
            params: vec![("port", port.to_string())],
        }
    }

    /// 没有权限监听端口。
    pub fn port_denied(port: u16) -> Self {
        TaskError {
            message: "没有权限监听端口 {port}，请更换端口",
            params: vec![("port", port.to_string())],
        }
    }

    /// 其它监听失败。
    pub fn port_unavailable(port: u16) -> Self {
        TaskError {
            message: "无法监听端口 {port}，详情见日志",
            params: vec![("port", port.to_string())],
        }
    }

    /// 安装完运行时后仍找不到可执行文件
    /// （`app/services/runtime_status.py:110` 的 `TaskError("安装后未找到 {name}")`）。
    pub fn install_not_found(name: &str) -> Self {
        TaskError {
            message: "安装后未找到 {name}",
            params: vec![("name", name.to_string())],
        }
    }

    pub fn io(error: impl fmt::Display) -> Self {
        TaskError {
            message: "发生了意外错误：{detail}",
            params: vec![("detail", error.to_string())],
        }
    }

    /// 是否是被取消（暂停）导致的中止。
    pub fn is_cancelled(&self) -> bool {
        self.message == CANCELLED_MESSAGE
    }

    /// 按 `message` 里的 `{name}` 占位符替换成 `params`（对应 Python 的 `TaskError.__str__`）。
    pub fn format(&self) -> String {
        let mut text = self.message.to_string();
        for (name, value) in &self.params {
            text = text.replace(&format!("{{{name}}}"), value);
        }
        text
    }
}

impl fmt::Display for TaskError {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str(&self.format())
    }
}

impl std::error::Error for TaskError {}
