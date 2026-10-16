//! HuggingFace 仓库树枚举：镜像 `features/huggingface_pack/pack.py` 的
//! `HuggingFaceParser._fetchTree`。
//!
//! 只做「网络 + JSON 解析」，返回按 Python 遍历顺序排好的文件条目；
//! `HuggingFaceFile`（index / downloadUrl）与 `HuggingFaceTask` 仍由 Python 构造。
//!
//! 与 Python 逐字对应的语义（照着 `_fetchTree` 的真实现抄，别去"修正"它们）：
//!
//! - 外层 `while url:` 两个分支都在第一轮 `break`，所以每个调用点只发**一次**请求，
//!   分页响应的后续页不会被跟进；
//! - `directory` 条目递归的子 URL 是 `f"{apiUrl}/{路径最后一段}"`——嵌套目录
//!   丢掉中间路径段，这是 Python 侧的现状，两侧必须一致；
//! - 非 list 的 JSON（如错误对象）直接返回已收集的条目，可能是空的；
//! - 条目顺序 = 页内顺序，且每遇到一个 directory 就先展开它再继续后面的条目。
//!
//! 错误：401 → `HfTreeError::Auth`（Python 侧转 `PermissionError`）；
//! 其余非 2xx；JSON 解不开。

use wreq::Client;

/// 一个 `type == "file"` 的树条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HfTreeEntry {
    pub path: String,
    pub size: u64,
    /// `lfs` 对象里的 `size`（Python 侧 `entry.get("size")` 在有 lfs 时被覆盖）。
    pub lfs_size: Option<u64>,
}

/// 树枚举失败原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HfTreeError {
    /// 401（Python 的 `PermissionError`）。
    Auth,
    /// 非 2xx 的状态码（Python 的 `raise_for_status`）。
    Status(u16),
    /// 网络或 JSON 层失败（Python 的各种异常；bridge 里转 `ValueError`）。
    Transport(String),
}

/// 从 `api_url` 枚举一棵（按 Python 现状实现的）树。
///
/// `headers` 里通常带 `authorization: Bearer …`；UA / 指纹由构建的 client 决定
/// （bridge 侧传应用 profile）。
pub async fn fetch_tree(
    client: &Client,
    headers: &[(String, String)],
    api_url: &str,
) -> Result<Vec<HfTreeEntry>, HfTreeError> {
    fetch_tree_at(client, headers, api_url, api_url).await
}

/// 与 Python `_fetchTree(self, client, apiUrl)` 同构：`api_url` 是递归时的
/// 「顶层基址」，`url` 是这一帧实际要请求的地址。返回 future 用 `Box::pin`
/// 装箱（async 递归）。
fn fetch_tree_at<'a>(
    client: &'a Client,
    headers: &'a [(String, String)],
    api_url: &'a str,
    url: &'a str,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<Vec<HfTreeEntry>, HfTreeError>> + Send + 'a>,
> {
    Box::pin(async move {
        let mut request_headers = wreq::header::HeaderMap::new();
        for (name, value) in headers {
            let name = wreq::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|error| HfTreeError::Transport(error.to_string()))?;
            let value = wreq::header::HeaderValue::from_str(value)
                .map_err(|error| HfTreeError::Transport(error.to_string()))?;
            request_headers.insert(name, value);
        }

        let response = client
            .get(url)
            .headers(request_headers)
            .send()
            .await
            .map_err(|error| HfTreeError::Transport(error.to_string()))?;

        let status = response.status().as_u16();
        if status == 401 {
            return Err(HfTreeError::Auth);
        }
        if (400..600).contains(&status) {
            return Err(HfTreeError::Status(status));
        }

        // Python：`page = await response.json()`，解析失败是 ValueError → Transport
        let bytes = response
            .bytes()
            .await
            .map_err(|error| HfTreeError::Transport(error.to_string()))?;
        let page: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|error| HfTreeError::Transport(error.to_string()))?;

        let mut entries = Vec::new();
        // `isinstance(page, list)`；其余（dict、数字…）原样 break，返回已收集条目
        if let serde_json::Value::Array(items) = page {
            for item in &items {
                let kind = item
                    .get("type")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                match kind {
                    "file" => entries.push(entry_of(item)),
                    "directory" => {
                        // `entry['path'].rsplit('/', 1)[-1]`：嵌套目录取最后一段
                        let path = item
                            .get("path")
                            .and_then(|value| value.as_str())
                            .ok_or_else(|| HfTreeError::Transport("missing path".to_string()))?;
                        let name = path.rsplit('/').next().unwrap_or("");
                        let mut sub_entries =
                            fetch_tree_at(client, headers, api_url, &format!("{api_url}/{name}"))
                                .await?;
                        entries.append(&mut sub_entries);
                    }
                    _ => {}
                }
            }
        }
        Ok(entries)
    })
}

/// `entry.get("path", "")` / `entry.get("size", 0)` / `lfs.get("size")`。
///
/// 差异说明：Python 的 directory 分支会用 `entry['path']`（缺 path 直接 KeyError）；
/// file 分支只走 `entry.get("path", "")`，缺 path 给空串。
fn entry_of(item: &serde_json::Value) -> HfTreeEntry {
    let path = item
        .get("path")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let size = item
        .get("size")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let lfs_size = item
        .get("lfs")
        .filter(|value| value.is_object())
        .and_then(|lfs| lfs.get("size"))
        .and_then(|value| value.as_u64());
    HfTreeEntry {
        path: path.to_string(),
        size,
        lfs_size,
    }
}
