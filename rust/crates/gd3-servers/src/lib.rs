//! 回环监听与协议适配器：镜像 `app/services/port_listener.py` 与 `app/services/aria2_rpc.py`。

pub mod aria2;
pub mod http;
pub mod listener;
pub mod websocket;

use gd3_core::TaskError;

pub use aria2::{Aria2Action, Aria2Config, Aria2Response, Aria2Service, Aria2TaskOptions};
pub use http::{HttpRequest, MAX_HEAD_SIZE, build_preflight_headers, build_response, parse_head};
pub use listener::{ConnectionHandler, PortListener, to_listen_error};
pub use websocket::WebSocket;

/// 监听状态（CONTEXT.md 的 Listen State）：关闭、启动中、监听中、失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenStatus {
    Off,
    Starting,
    Listening,
    Failed,
}

impl ListenStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ListenStatus::Off => "off",
            ListenStatus::Starting => "starting",
            ListenStatus::Listening => "listening",
            ListenStatus::Failed => "failed",
        }
    }
}

/// 监听状态的唯一事实：状态 + 端口 + 失败原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenState {
    pub status: ListenStatus,
    pub port: u16,
    pub error: Option<TaskError>,
}

impl ListenState {
    pub fn off() -> Self {
        ListenState {
            status: ListenStatus::Off,
            port: 0,
            error: None,
        }
    }

    pub fn starting(port: u16) -> Self {
        ListenState {
            status: ListenStatus::Starting,
            port,
            error: None,
        }
    }

    pub fn listening(port: u16) -> Self {
        ListenState {
            status: ListenStatus::Listening,
            port,
            error: None,
        }
    }

    pub fn failed(port: u16, error: TaskError) -> Self {
        ListenState {
            status: ListenStatus::Failed,
            port,
            error: Some(error),
        }
    }

    pub fn is_listening(&self) -> bool {
        self.status == ListenStatus::Listening
    }
}
