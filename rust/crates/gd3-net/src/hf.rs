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

/// `parseHuggingFaceUrl` 的成功结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HfUrlInfo {
    pub org: String,
    pub repo: String,
    pub repo_type: String,
    pub revision: String,
    pub file_path: String,
}

/// 极简 URL 拆分（镜像 Python `urlparse` 里用到的两个字段：hostname / path，
/// 不 lowercase hostname —— Python 的 `.lower()` 调用点自己做）。
fn split_url(url: &str) -> (String, String) {
    let rest = match url.find(':') {
        Some(idx)
            if url[..idx]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.') =>
        {
            &url[idx + 1..]
        }
        _ => url,
    };
    let (netloc, path) = match rest.strip_prefix("//") {
        Some(after) => match after.find(['/', '?', '#']) {
            Some(end) => (&after[..end], &after[end..]),
            None => (after, ""),
        },
        None => ("", rest),
    };
    let path = path.split(['?', '#']).next().unwrap_or("");
    // hostname：去掉 userinfo、IPv6 括号与端口
    let host = netloc.rsplit('@').next().unwrap_or("");
    let host = if let Some(close) = host.rfind(']') {
        &host[..=close]
    } else {
        // 对齐 Python `_hostinfo`：只在 `:` 之后全是数字时才算端口剥掉
        match host.rfind(':') {
            Some(idx)
                if !host[idx + 1..].is_empty()
                    && host[idx + 1..].chars().all(|c| c.is_ascii_digit()) =>
            {
                &host[..idx]
            }
            _ => host,
        }
    };
    (host.to_ascii_lowercase(), path.to_string())
}

/// 镜像 `features/huggingface_pack/pack.py::parseHuggingFaceUrl`：
/// host 判定（含代理站）+ 行内复刻 `REPO_PATTERN`。`proxy_site` 为空表示未选代理。
pub fn parse_url_entry(url: &str, proxy_site: &str) -> Option<HfUrlInfo> {
    let (host, path) = split_url(url);
    let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
    if host != "huggingface.co" {
        let proxy_site = proxy_site.trim();
        if proxy_site.is_empty() {
            return None;
        }
        let hostname_end = proxy_site.find("://").map(|i| i + 3).unwrap_or(0);
        let mut after = &proxy_site[hostname_end..];
        after = after.split('/').next().unwrap_or(after);
        // 对齐 Python `urlparse(proxySite).hostname`：去 userinfo、去端口
        // （IPv6 保留括号——永远不会等于 huggingface.co，无需更精细）。
        after = after.rsplit('@').next().unwrap_or(after);
        let proxy_host = match after.rfind(']') {
            Some(close) => &after[..=close],
            None => after.split(':').next().unwrap_or(""),
        };
        let proxy_host = proxy_host.to_ascii_lowercase();
        if host != proxy_host {
            return None;
        }
    }

    // `^/(?P<type>(?:datasets|spaces)/)?(?P<org>[^/]+)/(?P<repo>[^/]+)`
    // `(?:/(?:resolve|blob|tree)/(?P<rev>[^/]+)(?:/(?P<path>.+))?)?$`
    let rest = path.strip_prefix('/')?;
    let (repo_type, rest) = if let Some(rest) = rest.strip_prefix("datasets/") {
        ("dataset", rest)
    } else if let Some(rest) = rest.strip_prefix("spaces/") {
        ("space", rest)
    } else {
        ("model", rest)
    };
    let (org, rest) = rest.split_once('/')?;
    // `org/repo`（末尾无 `/`）匹配；`org/repo/` 要求后面必须接动词，不匹配
    let (repo, tail) = match rest.split_once('/') {
        Some((repo, tail)) => (repo, Some(tail)),
        None => (rest, None),
    };
    if org.is_empty() || repo.is_empty() {
        return None;
    }
    // tail：`resolve|blob|tree/<rev>[/path]` 或空串
    let (revision, file_path) = match tail {
        None => ("main", ""),
        Some(tail) => {
            let mut it = tail.splitn(2, '/');
            let verb = it.next().unwrap_or("");
            if !matches!(verb, "resolve" | "blob" | "tree") {
                return None;
            }
            // `resolve/`（rev 缺失）不匹配 `[^/]+`
            let tail = it.next()?;
            let (rev, file) = match tail.split_once('/') {
                // 有 "/" 但后半为空：尾斜杠 —— Python 正则 `(.+)` 要求 path 至少
                // 1 字符，整体匹配失败返回 None（回溯也无法挽救）。
                Some((rev, "")) => {
                    let _ = rev;
                    return None;
                }
                Some((rev, file)) => (rev, file),
                None => (tail, ""),
            };
            if rev.is_empty() {
                return None;
            }
            (rev, file)
        }
    };
    Some(HfUrlInfo {
        org: org.to_string(),
        repo: repo.to_string(),
        repo_type: repo_type.to_string(),
        revision: revision.to_string(),
        file_path: if file_path.is_empty() {
            "".to_string()
        } else {
            file_path.to_string()
        },
    })
}

/// `typePrefix` 映射：镜像 `HuggingFaceParser._fileUrl` / `parse` 里的
/// `{"dataset": "datasets/", "space": "spaces/"}.get(repoType, "")`。
pub fn type_prefix(repo_type: &str) -> &'static str {
    match repo_type {
        "dataset" => "datasets/",
        "space" => "spaces/",
        _ => "",
    }
}

/// 镜像 `HuggingFaceParser._fileUrl`。
pub fn file_url(
    base: &str,
    repo_type: &str,
    repo_id: &str,
    revision: &str,
    file_path: &str,
) -> String {
    format!(
        "{}/{index}{repo_id}/resolve/{revision}/{file_path}",
        base,
        index = type_prefix(repo_type)
    )
}

/// 一条待下载文件（`parse` 的 entries → files 纯变换层的输出）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HfFileInfo {
    pub index: usize,
    pub path: String,
    pub size: u64,
    pub download_url: String,
}

/// 镜像 `HuggingFaceParser.parse` 里 `for i, entry in enumerate(entries)` 循环：
///
/// - `path = entry.get("path", "")`：缺失是空串（不报错）；
/// - `size = entry.get("size", 0)`：缺失是 0；
/// - `lfs` 若是对象且带 `size`，覆盖 `size`（`lfs.get("size", size)`——
///   lfs 对象缺 `size` 键时保留原 size）。
///
/// `entries` 是 （可能来自 Python `json.loads` 的）JSON 数组，逐条按顺序编号。
pub fn build_files(
    entries: &[serde_json::Value],
    base: &str,
    repo_id: &str,
    repo_type: &str,
    revision: &str,
) -> Vec<HfFileInfo> {
    entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let path = entry
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let mut size = entry.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
            if let Some(lfs) = entry.get("lfs").filter(|v| v.is_object()) {
                if let Some(lfs_size) = lfs.get("size").and_then(|v| v.as_u64()) {
                    size = lfs_size;
                }
            }
            let download_url = file_url(base, repo_type, repo_id, revision, &path);
            HfFileInfo {
                index: i,
                path,
                size,
                download_url,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(url: &str) -> Option<(String, String, String, String, String)> {
        parse_url_entry(url, "").map(|info| {
            (
                info.org,
                info.repo,
                info.repo_type,
                info.revision,
                info.file_path,
            )
        })
    }

    fn of_with(url: &str, proxy: &str) -> Option<(String, String, String, String, String)> {
        parse_url_entry(url, proxy).map(|info| {
            (
                info.org,
                info.repo,
                info.repo_type,
                info.revision,
                info.file_path,
            )
        })
    }

    #[test]
    fn normal_model_url() {
        assert_eq!(
            of("https://huggingface.co/org/repo"),
            Some((
                "org".into(),
                "repo".into(),
                "model".into(),
                "main".into(),
                "".into()
            ))
        );
        assert_eq!(
            of("https://huggingface.co/org/repo/resolve/main/a/b.model"),
            Some((
                "org".into(),
                "repo".into(),
                "model".into(),
                "main".into(),
                "a/b.model".into()
            ))
        );
        assert_eq!(
            of("https://huggingface.co/datasets/org/repo"),
            Some((
                "org".into(),
                "repo".into(),
                "dataset".into(),
                "main".into(),
                "".into()
            ))
        );
        assert_eq!(
            of("https://huggingface.co/spaces/org/repo/blob/v1.0/file"),
            Some((
                "org".into(),
                "repo".into(),
                "space".into(),
                "v1.0".into(),
                "file".into()
            ))
        );
    }

    #[test]
    fn boundaries() {
        assert!(of("").is_none());
        assert!(of("ftp://huggingface.co/org/repo").is_some());
        assert!(of("https://a.co/org/repo").is_none());
        assert!(of("https://a.co/org/repo").is_none());
        assert!(of("https://huggingface.co/org/repo/x/file").is_none());
        assert!(of("https://huggingface.co/org").is_none());
        assert!(of("https://huggingface.co/org/repo/resolve").is_none());
        assert!(of("https://huggingface.co/org/repo/resolve/").is_none());
        assert!(of_with("https://a.co/org/repo", "https://a.co").is_some());
        assert!(of_with("https://a.co/org/repo", "https://b.co").is_none());
        assert!(of("https://www.huggingface.co/org/repo").is_some());
        // 尾斜杠：Python 正则 `(.+)` 要求 path 至少 1 字符 → 整体失败
        assert!(of("https://huggingface.co/org/repo/resolve/main/").is_none());
        assert!(of("https://huggingface.co/org/repo/resolve/main/").is_none());
        // 空 rev（`resolve//x`）
        assert!(of("https://huggingface.co/org/repo/resolve//x").is_none());
        // `/`x` 形如 `/x/` 仍要保住 path 尾斜杠的合法情形（`/a/` 是 path 的一部分）
        assert_eq!(
            of("https://huggingface.co/org/repo/resolve/v1/a/"),
            Some((
                "org".into(),
                "repo".into(),
                "model".into(),
                "v1".into(),
                "a/".into()
            ))
        );
        // `resolve/main//x` → filePath 贪婪吃到 `/x`
        assert_eq!(
            of("https://huggingface.co/org/repo/resolve/main//x"),
            Some((
                "org".into(),
                "repo".into(),
                "model".into(),
                "main".into(),
                "/x".into()
            ))
        );
        // 大小写敏感：verb 与 datasets 前缀必须小写
        assert!(of("https://huggingface.co/org/repo/RESOLVE/main/f").is_none());
        // 大小写只对 verb / datasets|spaces 前缀敏感；org/repo 的 `[^/]+` 不限
        assert_eq!(
            of("https://huggingface.co/ORG/repo"),
            Some((
                "ORG".into(),
                "repo".into(),
                "model".into(),
                "main".into(),
                "".into()
            ))
        );
        // 代理站带端口 / 路径 / userinfo：只比 hostname
        assert!(of_with("https://a.co:8443/org/repo", "https://a.co:8443/path").is_some());
        assert!(of_with("https://user:pass@a.co/org/repo", "a.co").is_some());
        // query/fragment 不进 path
        assert_eq!(
            of("https://huggingface.co/org/repo/resolve/main/f?x=1#y"),
            Some((
                "org".into(),
                "repo".into(),
                "model".into(),
                "main".into(),
                "f".into()
            ))
        );
    }

    #[test]
    fn build_files_and_file_url() {
        use serde_json::json;
        // 基本映射 + lfs 覆盖 + 缺失字段
        let files = build_files(
            &[
                json!({"path": "a.bin", "size": 5}),
                json!({"path": "b/c.d", "size": 7, "lfs": {"size": 12}}),
                json!({"path": "e.txt"}),
                json!({"size": 3}),
                json!({"path": "f", "lfs": {}}),
            ],
            "https://hf.co",
            "org/repo",
            "model",
            "main",
        );
        let urls: Vec<&str> = files.iter().map(|f| f.download_url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://hf.co/org/repo/resolve/main/a.bin",
                "https://hf.co/org/repo/resolve/main/b/c.d",
                "https://hf.co/org/repo/resolve/main/e.txt",
                "https://hf.co/org/repo/resolve/main/",
                "https://hf.co/org/repo/resolve/main/f",
            ]
        );
        let sizes: Vec<u64> = files.iter().map(|f| f.size).collect();
        assert_eq!(sizes, [5, 12, 0, 3, 0]); // 缺 size→0；lfs 无 size 键保留原值
        assert_eq!(
            files.iter().map(|f| f.index).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4]
        );

        // typePrefix
        assert_eq!(
            file_url("https://hf.co", "dataset", "o/r", "main", "x"),
            "https://hf.co/datasets/o/r/resolve/main/x"
        );
        assert_eq!(
            file_url("https://hf.co", "space", "o/r", "main", "x"),
            "https://hf.co/spaces/o/r/resolve/main/x"
        );
        assert_eq!(
            file_url("https://hf.co", "other", "o/r", "main", "x"),
            "https://hf.co/o/r/resolve/main/x"
        );
    }
}
