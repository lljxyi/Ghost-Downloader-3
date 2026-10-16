//! WebSocket 服务端：握手按 RFC 6455 自己做，帧编解码交给 `tokio-tungstenite`。
//!
//! 与 Python `app/services/websocket_stream.py` 的差别：Python 直接用 `websockets` 的
//! `ServerProtocol`，Rust 侧用现成的帧编解码器——自己实现帧解析属于重复造轮子。

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures_util::{SinkExt, StreamExt};
use sha1::{Digest, Sha1};
use std::io;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};

/// RFC 6455 固定 GUID。
const WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// 计算 `Sec-WebSocket-Accept`。
pub fn accept_key(sec_websocket_key: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(sec_websocket_key.as_bytes());
    hasher.update(WEBSOCKET_GUID.as_bytes());
    BASE64.encode(hasher.finalize())
}

/// 判断请求头是否是 WebSocket 升级请求。
pub fn is_upgrade(headers: &std::collections::HashMap<String, String>) -> bool {
    headers
        .get("upgrade")
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
}

pub struct WebSocket {
    stream: WebSocketStream<TcpStream>,
}

impl WebSocket {
    /// 完成握手并进入帧循环；握手失败返回 `None`（调用方直接断开）。
    pub async fn accept(
        mut socket: TcpStream,
        headers: &std::collections::HashMap<String, String>,
        max_message_size: usize,
    ) -> io::Result<Option<WebSocket>> {
        let Some(key) = headers.get("sec-websocket-key") else {
            return Ok(None);
        };

        let response = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
            accept_key(key)
        );
        socket.write_all(response.as_bytes()).await?;
        socket.flush().await?;

        let config = WebSocketConfig::default()
            .max_message_size(Some(max_message_size))
            .max_frame_size(Some(max_message_size));
        let stream = WebSocketStream::from_raw_socket(socket, Role::Server, Some(config)).await;
        Ok(Some(WebSocket { stream }))
    }

    /// 收下一条文本消息；对端关闭或出错返回 `None`。
    pub async fn recv(&mut self) -> Option<String> {
        loop {
            let message = self.stream.next().await?.ok()?;
            match message {
                Message::Text(text) => return Some(text.to_string()),
                Message::Binary(data) => return String::from_utf8(data.to_vec()).ok(),
                Message::Close(_) => return None,
                Message::Ping(payload) => {
                    let _ = self.stream.send(Message::Pong(payload)).await;
                }
                _ => continue,
            }
        }
    }

    pub async fn send(&mut self, text: &str) -> io::Result<()> {
        self.stream
            .send(Message::Text(text.into()))
            .await
            .map_err(io::Error::other)
    }

    pub async fn close(&mut self) {
        let _ = self.stream.close(None).await;
    }
}

/// 从流里读到 `\r\n\r\n` 为止；超长或对端提前断开返回 `None`。
pub async fn read_head(
    socket: &mut TcpStream,
    max_head_size: usize,
) -> io::Result<Option<Vec<u8>>> {
    let mut head = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    loop {
        let read = socket.read(&mut byte).await?;
        if read == 0 {
            return Ok(None);
        }
        head.push(byte[0]);
        if head.len() > max_head_size {
            return Ok(None);
        }
        if head.ends_with(b"\r\n\r\n") {
            return Ok(Some(head));
        }
    }
}

impl WebSocket {
    /// 交出内部帧流，供调用方自己 split（读写分离）。
    pub fn into_inner(self) -> WebSocketStream<TcpStream> {
        self.stream
    }
}
