//! Bilibili 流解析层：镜像 `features/bili_pack/stream.py` 的纯函数
//! `toStreamUrl` / `buildSize` / `matchStream` / `buildFnval` / `parseDash`。
//!
//! 这些函数只读 JSON 与少量配置值（`alternativeQuality`、HDR/Dolby 开关），
//! 所以配置一律由 bridge 从 Python 传进来，Rust 不碰 `cfg`。
//! `BiliPage` 的字段赋值（`videoUrl` 等）留在 Python——适配器拿到
//! `DashOutcome` 后自己写回。
//!
//! 照着 Python 现状抄、不要"修正"的点：
//! - `toStreamUrl` 先 `baseUrl`（仅 camelCase 键）再 `base_url`，备份同理，取第一个真值；
//!   （非字符串的真值——Python 会原样返回上游对象——在此不镜像：实际 JSON 里这两个键只会是字符串或缺失）
//! - `buildSize` 用 Python `int(x)` 语义（字符串能解析也接受、浮点向零截断），
//!   `bandwidth * duration // 8` 是地板除；
//! - `matchStream` 的 `ValueError` 文案逐字一致；
//! - `s.get("id") == target` 是 Python 的原始值相等比较（字符串 ≠ int、`True==1`、`64.0==64`）；
//! - 质量筛选只在 `quality is not None 且 acceptQuality 非空` 时生效，
//!   且允许 `id == 目标` 但 `toStreamUrl` 为空时继续找下一条。

//! ### 收集阶段：`fetchView`（`features/bili_pack/pack.py::fetchView`）
//!
//! 两个请求共用 `api_headers`（Python 里 cookie 是 client 默认头）：
//! 1. `videoId` 为空且 `seasonId` 真值：先取 seasons_archives_list 第一页，
//!    拿第一个 `archives[].bvid`；空列表 / 无 bvid → `ValueError("未获取到合集信息")`；
//! 2. `av…` 用 `avid={videoId[2:]}`，否则 `bvid={videoId}`；
//! 3. `code not in {None, 0}` → `ValueError(message or "获取 Bilibili 视频信息失败")`，
//!    返回 `data or {}`。
//!
//!    HTTP 失败与 JSON 解析失败都按 `ValueError` 文案 Transport 抛出
//!    （Python 抛 httpx 异常，桥侧只保证非 2xx / 解析错误也是异常，不逐字对齐 httpx 文案）。

use serde_json::Value;
use wreq::Client;

/// `parseDash` 的产出（Python 侧写回 `BiliPage` 的四个字段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashOutcome {
    pub video_url: String,
    pub audio_url: String,
    pub video_size: i64,
    pub audio_size: i64,
}

/// 镜像 `toStreamUrl(s: dict) -> str`。
pub fn to_stream_url(stream: &Value) -> String {
    let base = stream.get("baseUrl").or_else(|| stream.get("base_url"));
    if let Some(url) = base {
        if let Some(url) = url.as_str() {
            if !url.is_empty() {
                return url.to_string();
            }
        }
    }
    let backups = stream.get("backupUrl").or_else(|| stream.get("backup_url"));
    if let Some(value) = backups {
        if let Some(list) = value.as_array() {
            for item in list {
                if let Some(url) = item.as_str() {
                    if !url.is_empty() {
                        return url.to_string();
                    }
                }
            }
        }
    }
    String::new()
}

/// Python `int(x)`：接受整数、字符串（十进制）与浮点（向零截断）。
/// 其它类型（含 `null` / `bool`）按"无法解析"退回 0（Python 的 `or 0` 兜底）。
fn python_int(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Number(number)) => {
            if let Some(int_value) = number.as_i64() {
                int_value
            } else if let Some(float_value) = number.as_f64() {
                float_value.trunc() as i64
            } else {
                0
            }
        }
        Some(Value::String(text)) => text.trim().parse::<i64>().unwrap_or(0),
        // Python 的 bool 是 int：int(True)=1 / int(False)=0
        Some(Value::Bool(bool_value)) => i64::from(*bool_value),
        _ => 0,
    }
}

/// 镜像 `buildSize(stream: dict | None, duration: int) -> int`。
pub fn build_size(stream: Option<&Value>, duration: i64) -> i64 {
    let Some(stream) = stream.filter(|stream| !stream.is_null()) else {
        return 0;
    };
    if duration <= 0 {
        return 0;
    }
    let bandwidth = python_int(stream.get("bandwidth"));
    // Python `bandwidth * duration // 8` 是地板除（对负数向 -∞）
    (bandwidth * duration).div_euclid(8)
}

/// Python `s.get("id") == target`：严格类型比较——字符串永远不等于 int，
/// `bool` 按 int（`True==1`），浮点按数值（`64.0==64`），缺失/`null` 为 `False`。
fn python_id_is(stream: &Value, target: i64) -> bool {
    match stream.get("id") {
        Some(Value::Number(number)) => {
            if let Some(int_value) = number.as_i64() {
                int_value == target
            } else if let Some(float_value) = number.as_f64() {
                float_value == target as f64
            } else {
                false
            }
        }
        Some(Value::Bool(bool_value)) => i64::from(*bool_value) == target,
        _ => false,
    }
}

/// Python `[s.get("id") for s in audioStreams if s.get("id") is not None]`
/// 的数值化版本：只保留能按 Python int 参与相等比较的类型（int/bool/浮点）。
fn python_id_value(stream: &Value) -> Option<i64> {
    match stream.get("id") {
        Some(Value::Number(number)) => {
            if let Some(int_value) = number.as_i64() {
                Some(int_value)
            } else {
                number
                    .as_f64()
                    .map(|float_value| float_value.trunc() as i64)
            }
        }
        Some(Value::Bool(bool_value)) => Some(i64::from(*bool_value)),
        _ => None,
    }
}

/// 镜像 `matchStream`：返回命中的下标；错误文案与 Python 逐字一致。
///
/// `alternative` 是 `bilibiliConfig.alternativeQuality.value`（"max" 或其它 → min）。
pub fn match_stream(
    streams: &[Value],
    quality: Option<i64>,
    accept_quality: &[i64],
    alternative: &str,
) -> Result<usize, String> {
    if streams.is_empty() {
        return Err("Bilibili 返回结果中不存在可用的媒体流".to_string());
    }

    if let Some(target) = quality {
        if !accept_quality.is_empty() {
            let mut target_quality = target;
            if !accept_quality.contains(&target_quality) {
                target_quality = if alternative == "max" {
                    *accept_quality.iter().max().unwrap()
                } else {
                    *accept_quality.iter().min().unwrap()
                };
            }
            for (index, stream) in streams.iter().enumerate() {
                if python_id_is(stream, target_quality) && !to_stream_url(stream).is_empty() {
                    return Ok(index);
                }
            }
        }
    } else {
        // quality 为 None 或 accept_quality 为空：跳过筛选直接落到兜底
    }

    for (index, stream) in streams.iter().enumerate() {
        if !to_stream_url(stream).is_empty() {
            return Ok(index);
        }
    }
    Err("未找到可用的媒体流".to_string())
}

/// 镜像 `buildFnval(qn)`（`fnval` 位标志）。两个 cfg 布尔由 bridge 传入。
pub fn build_fnval(qn: i64, include_hdr: bool, include_dolby: bool) -> i64 {
    let mut fnval = 16i64;
    if include_hdr {
        fnval |= 64;
    }
    if include_dolby {
        fnval |= 256 | 512;
    }
    if qn == 128 {
        fnval |= 1024;
    }
    if qn == 120 {
        fnval |= 128;
    }
    fnval
}

/// 镜像 `parseDash(page, dash, *, qn, acceptQuality, audioQn=0)`。
///
/// 返回要写回 `BiliPage` 的四个字段；`_videoStreams` / `_audioStreams` 保持引用
/// `dash`（Python 侧本来就有完整 dict），所以不在这里返回。
///
/// 错误文案与 Python 一致：videoStreams 为空 / matchStream 失败。
pub fn parse_dash(
    dash: &Value,
    duration: i64,
    qn: i64,
    accept_quality: &[i64],
    audio_qn: i64,
    alternative: &str,
) -> Result<DashOutcome, String> {
    let video_streams = dash
        .get("video")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    let audio_streams = dash
        .get("audio")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    if video_streams.is_empty() {
        return Err("Bilibili 返回结果中不存在可用的媒体流".to_string());
    }
    let video = match_stream(&video_streams, Some(qn), accept_quality, alternative)?;
    let audio = if audio_streams.is_empty() {
        None
    } else {
        let audio_ids: Vec<i64> = audio_streams.iter().filter_map(python_id_value).collect();
        Some(match_stream(
            &audio_streams,
            if audio_qn != 0 { Some(audio_qn) } else { None },
            if audio_qn != 0 { &audio_ids } else { &[] },
            alternative,
        )?)
    };

    let video_stream = &video_streams[video];
    let audio_stream = audio.map(|index| &audio_streams[index]);
    Ok(DashOutcome {
        video_url: to_stream_url(video_stream),
        audio_url: audio_stream.map(to_stream_url).unwrap_or_default(),
        video_size: build_size(Some(video_stream), duration),
        audio_size: audio_stream
            .map(|stream| build_size(Some(stream), duration))
            .unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn stream(id: i64, url: &str) -> Value {
        json!({"id": id, "baseUrl": url})
    }

    #[test]
    fn to_stream_url_prefers_camel_then_snake() {
        assert_eq!(
            to_stream_url(&json!({"baseUrl": "http://a", "backupUrl": ["http://b"]})),
            "http://a"
        );
        assert_eq!(
            to_stream_url(&json!({"base_url": "http://a", "backup_url": ["", "http://b"]})),
            "http://a"
        );
        assert_eq!(
            to_stream_url(&json!({"base_url": "", "backup_url": ["http://b", "x"]})),
            "http://b"
        );
        assert_eq!(to_stream_url(&json!({})), "");
        assert_eq!(to_stream_url(&json!({"backupUrl": ["", null]})), "");
    }

    #[test]
    fn build_size_mirrors_python_semantics() {
        assert_eq!(build_size(None, 100), 0);
        assert_eq!(build_size(Some(&json!(null)), 100), 0);
        assert_eq!(build_size(Some(&json!({"bandwidth": 2000})), 0), 0);
        assert_eq!(build_size(Some(&json!({"bandwidth": 2000})), 8), 2000);
        assert_eq!(build_size(Some(&json!({"bandwidth": 2001})), 8), 2001);
        // 地板除：负数也向 -∞（-18009 // 8 = -2252，不是 -2251）
        assert_eq!(build_size(Some(&json!({"bandwidth": -2001})), 8), -2001);
        assert_eq!(build_size(Some(&json!({"bandwidth": -2001})), 9), -2252);
        // 字符串按 int() 接受
        assert_eq!(build_size(Some(&json!({"bandwidth": "3000"})), 8), 3000);
        assert_eq!(build_size(Some(&json!({"bandwidth": "junk"})), 8), 0);
        // 浮点向零截断
        assert_eq!(build_size(Some(&json!({"bandwidth": 3.9})), 8), 3);
    }

    #[test]
    fn match_stream_errors_are_verbatim() {
        assert_eq!(
            match_stream(&[], None, &[], "max").unwrap_err(),
            "Bilibili 返回结果中不存在可用的媒体流"
        );
        let no_url = vec![stream(90, "")];
        assert_eq!(
            match_stream(&no_url, None, &[], "max").unwrap_err(),
            "未找到可用的媒体流"
        );
    }

    #[test]
    fn match_stream_id_comparison_is_python_strict() {
        // 字符串 id 不能与 int target 相等（两侧都落到兜底）
        let streams = vec![json!({"id": "64", "baseUrl": "http://a"})];
        assert_eq!(match_stream(&streams, Some(64), &[64], "max").unwrap(), 0);
        // 浮点 64.0 == 64
        let streams = vec![json!({"id": 64.0, "baseUrl": "http://a"})];
        assert_eq!(match_stream(&streams, Some(64), &[64], "max").unwrap(), 0);
        // 没 url 的继续找下一条
        let streams = vec![stream(64, ""), stream(64, "http://b")];
        assert_eq!(match_stream(&streams, Some(64), &[64], "max").unwrap(), 1);
    }

    #[test]
    fn match_stream_quality_fallback() {
        let streams = vec![stream(80, "http://a"), stream(64, "http://b")];
        // 目标质量命中
        assert_eq!(
            match_stream(&streams, Some(64), &[80, 64], "max").unwrap(),
            1
        );
        // 目标不在 acceptQuality：max → 80
        assert_eq!(
            match_stream(&streams, Some(32), &[80, 64], "max").unwrap(),
            0
        );
        // 目标不在 acceptQuality：非 max → min(64)
        assert_eq!(
            match_stream(&streams, Some(32), &[80, 64], "min").unwrap(),
            1
        );
        let streams_broken_first = vec![stream(80, ""), stream(64, "http://b")];
        assert_eq!(
            match_stream(&streams_broken_first, Some(80), &[80, 64], "max").unwrap(),
            1
        );
        // 不做筛选：quality None / acceptQuality 空
        assert_eq!(
            match_stream(&streams_broken_first, None, &[], "max").unwrap(),
            1
        );
    }

    #[test]
    fn build_fnval_flags() {
        assert_eq!(build_fnval(80, false, false), 16);
        assert_eq!(build_fnval(80, true, false), 80);
        assert_eq!(build_fnval(80, false, true), 16 | 256 | 512);
        assert_eq!(build_fnval(128, false, false), 16 | 1024);
        assert_eq!(build_fnval(120, false, false), 16 | 128);
        assert_eq!(build_fnval(120, true, true), 16 | 64 | 256 | 512 | 128);
    }

    #[test]
    fn parse_dash_harvests_fields() {
        let dash = json!({
            "video": [{"id": 80, "baseUrl": "http://v"}, {"id": 64, "baseUrl": "http://v2"}],
            "audio": [{"id": 30216, "baseUrl": "http://a", "bandwidth": 32000}],
        });
        let outcome = parse_dash(&dash, 8, 80, &[80, 64], 30216, "max").unwrap();
        assert_eq!(outcome.video_url, "http://v");
        assert_eq!(outcome.audio_url, "http://a");
        assert_eq!(outcome.video_size, 0);
        assert_eq!(outcome.audio_size, 32000);

        // 没有 audio
        let dash = json!({
            "video": [{"id": 80, "baseUrl": "http://v", "bandwidth": 1234}],
        });
        let outcome = parse_dash(&dash, 8, 80, &[80], 0, "max").unwrap();
        assert_eq!(outcome.audio_url, "");
        assert_eq!(outcome.audio_size, 0);
        assert_eq!(outcome.video_size, 1234);
    }

    #[test]
    fn parse_dash_video_only_is_python_compatible() {
        // Python test_parseDash_allows_video_only：无 audio 也不报错
        let dash = json!({"video": [{"id": 80, "baseUrl": "http://v"}]});
        let outcome = parse_dash(&dash, 8, 80, &[80], 0, "max").unwrap();
        assert_eq!(outcome.video_url, "http://v");
        assert_eq!(outcome.audio_url, "");
    }

    #[test]
    fn parse_dash_no_video_stream_is_verbatim() {
        let dash = json!({"video": [], "audio": [{"id": 30216}]});
        assert_eq!(
            parse_dash(&dash, 8, 80, &[80], 30216, "max").unwrap_err(),
            "Bilibili 返回结果中不存在可用的媒体流"
        );
    }
}

// ---------------------------------------------------------------------------
// `parseBiliUrl` / `parsePageSpec`（features/bili_pack/parse.py）
// ---------------------------------------------------------------------------

/// `BiliUrl` 的镜像：字段与 Python dataclass 同名同默认。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BiliUrlOutcome {
    pub video_id: String,
    pub mid: i64,
    pub season_id: i64,
    pub selected_pages: Option<Vec<i64>>,
}

/// 拆出 urllib.parse 用得到的两个字段：path（首段 URL，`?`/`#` 之前的部）
/// 与 query（`?` 之后 `#` 的之前）。不 lowercase——两侧比较都大小写敏感。
fn split_path_query(input: &str) -> (String, String) {
    // scheme 摘除：与 hf::split_url 相同的最小 urlparse 镜像
    let rest = match input.find(':') {
        Some(idx)
            if input[..idx]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.') =>
        {
            &input[idx + 1..]
        }
        _ => input,
    };
    let rest = match rest.strip_prefix("//") {
        Some(after) => match after.find('/') {
            Some(slash) => &after[slash..],
            None => "",
        },
        None => rest,
    };
    let rest = rest.split('#').next().unwrap_or("");
    let (path, query) = match rest.find('?') {
        Some(idx) => (&rest[..idx], &rest[idx + 1..]),
        None => (rest, ""),
    };
    (path.to_string(), query.to_string())
}

/// 镜像 `urllib.parse.unquote_plus`：`+` → 空格，`%XX` → 对应字节。
/// 非法残序（`%` 后不是 Hexadecimal）原样保留。
fn unquote_plus(text: &str) -> String {
    if !text.contains(['%', '+']) {
        return text.to_string();
    }
    let mut out = Vec::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = &text[i + 1..i + 3];
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 镜像 `urllib.parse.parse_qs`（keep_blank_values=False）：
/// 空段丢弃、无 `=` 或空值丢弃，键值 unquote_plus；保序。
fn parse_qs(query: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for piece in query.split('&') {
        if piece.is_empty() {
            continue;
        }
        let (name, value) = match piece.split_once('=') {
            Some((name, value)) if !value.is_empty() => (name, value),
            _ => continue,
        };
        pairs.push((unquote_plus(name), unquote_plus(value)));
    }
    pairs
}

fn query_first(query: &str, key: &str) -> String {
    parse_qs(query)
        .into_iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value)
        .unwrap_or_default()
}

/// 镜像 `parsePageSpec`：`p` 参数 `1,3-5` 展开成有序页码；缺 `p` → None。
/// `int()` 语义镜像——`ValueError` 原样上抛（文案与 Python 一致）。
fn parse_page_spec(query: &str) -> Result<Option<Vec<i64>>, String> {
    let page_param = query_first(query, "p").trim().to_string();
    if page_param.is_empty() {
        return Ok(None);
    }
    let mut selected = Vec::new();
    for part in page_param.split(',') {
        let part = part.trim();
        if part.contains('-') {
            let (start_text, end_text) = part.split_once('-').unwrap();
            let mut start = python_str_to_int(start_text.trim())?;
            let mut end = python_str_to_int(end_text.trim())?;
            if start > end {
                std::mem::swap(&mut start, &mut end);
            }
            selected.extend(start..=end);
        } else {
            selected.push(python_str_to_int(part)?);
        }
    }
    Ok(Some(selected))
}

/// 镜像 Python `int(text)`（十进制）：允许两侧空白/正负号，失败报同文案。
fn python_str_to_int(text: &str) -> Result<i64, String> {
    let trimmed = text.trim();
    match trimmed.parse::<i64>() {
        Ok(value) => Ok(value),
        _ => {
            let _ = trimmed;
            Err(format!("invalid literal for int() with base 10: '{text}'"))
        }
    }
}

/// 镜像 `parseBiliUrl`：三种路径规则逐一 `re.match`（锚定开头），全不中报
/// `ValueError("不是有效的 Bilibili 视频链接")` 文案原样返回 Err。
pub fn parse_url(input: &str) -> Result<BiliUrlOutcome, String> {
    let (path, query) = split_path_query(input);

    // r"/video/(BV[a-zA-Z0-9]+|av\d+)"
    if let Some(rest) = path.strip_prefix("/video/") {
        let video_id = if let Some(bv) = rest.strip_prefix("BV") {
            let n = bv
                .find(|c: char| !c.is_ascii_alphanumeric())
                .unwrap_or(bv.len());
            if n > 0 {
                format!("BV{}", &bv[..n])
            } else {
                String::new()
            }
        } else if let Some(av) = rest.strip_prefix("av") {
            let n = av.find(|c: char| !c.is_ascii_digit()).unwrap_or(av.len());
            if n > 0 {
                format!("av{}", &av[..n])
            } else {
                String::new()
            }
        } else {
            String::new()
        };
        if !video_id.is_empty() {
            let selected_pages = parse_page_spec(&query)?;
            return Ok(BiliUrlOutcome {
                video_id,
                selected_pages,
                ..BiliUrlOutcome::default()
            });
        }
    }

    // r"/(\d+)/lists/(\d+)"（全部锚定开头，首个字符必须就是数字段）
    let after_video = path.strip_prefix('/');
    if let Some(rest) = after_video {
        let digits_end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        if digits_end > 0 {
            let mid_text = &rest[..digits_end];
            let after = &rest[digits_end..];
            // `/lists/(\d+)`
            if let Some(after) = after.strip_prefix("/lists/") {
                let season_end = after
                    .find(|c: char| !c.is_ascii_digit())
                    .unwrap_or(after.len());
                if season_end > 0 {
                    let mid: i64 = mid_text.parse().unwrap_or(0);
                    let season_id: i64 = after[..season_end].parse().unwrap_or(0);
                    return Ok(BiliUrlOutcome {
                        mid,
                        season_id,
                        ..BiliUrlOutcome::default()
                    });
                }
            }
            // `/(\d+)/channel/collectiondetail`
            if after == "/channel/collectiondetail" {
                let sid = query_first(&query, "sid");
                if sid.chars().all(|c| c.is_ascii_digit()) && !sid.is_empty() {
                    let mid: i64 = mid_text.parse().unwrap_or(0);
                    let season_id: i64 = sid.parse().unwrap_or(0);
                    return Ok(BiliUrlOutcome {
                        mid,
                        season_id,
                        ..BiliUrlOutcome::default()
                    });
                }
            }
        }
    }

    Err("不是有效的 Bilibili 视频链接".to_string())
}

#[cfg(test)]
mod url_tests {
    use super::*;

    fn of(url: &str) -> Result<BiliUrlOutcome, String> {
        parse_url(url)
    }

    #[test]
    fn video_urls() {
        assert_eq!(
            of("https://www.bilibili.com/video/BV1abc123/?p=2"),
            Ok(BiliUrlOutcome {
                video_id: "BV1abc123".into(),
                selected_pages: Some(vec![2]),
                ..Default::default()
            })
        );
        assert_eq!(
            of("https://b23.tv/video/av170001/doc"),
            Ok(BiliUrlOutcome {
                video_id: "av170001".into(),
                selected_pages: None,
                ..Default::default()
            })
        );
        // page 范围与逗号展开
        assert_eq!(
            of("https://a/video/BV1?p=1,3-5"),
            Ok(BiliUrlOutcome {
                video_id: "BV1".into(),
                selected_pages: Some(vec![1, 3, 4, 5]),
                ..Default::default()
            })
        );
        // `av` 后无数字 / `BV` 后无字符 → 视频规则不中
        assert!(of("https://a/video/avx").is_err());
        assert!(of("https://a/video/BV").is_err());
    }

    #[test]
    fn list_and_collection_urls() {
        assert_eq!(
            of("https://space.bilibili.com/487/lists/920"),
            Ok(BiliUrlOutcome {
                mid: 487,
                season_id: 920,
                ..Default::default()
            })
        );
        assert_eq!(
            of("https://space.bilibili.com/487/channel/collectiondetail?sid=920"),
            Ok(BiliUrlOutcome {
                mid: 487,
                season_id: 920,
                ..Default::default()
            })
        );
        // sid 非法 / 缺失 → 掉到最终报错
        assert!(of("https://space.bilibili.com/487/channel/collectiondetail?sid=x").is_err());
        assert!(of("https://space.bilibili.com/x/lists/920").is_err());
    }

    #[test]
    fn invalid_urls() {
        assert!(of("").is_err());
        assert!(of("https://www.bilibili.com/").is_err());
        assert_eq!(
            of("https://example.com/foo").unwrap_err(),
            "不是有效的 Bilibili 视频链接"
        );
    }

    #[test]
    fn page_spec_parsing() {
        // %2C 与 + 都要 unquote_plus
        assert_eq!(parse_page_spec("p=1%2C2").unwrap(), Some(vec![1, 2]));
        assert_eq!(parse_page_spec("p=%31").unwrap(), Some(vec![1]));
        assert_eq!(parse_page_spec("").unwrap(), None);
        // int() 报错文案逐字
        assert_eq!(
            parse_page_spec("p=x").unwrap_err(),
            "invalid literal for int() with base 10: 'x'"
        );
    }
}

// ---------------------------------------------------------------------------
// buildPages / buildSeasonPages / buildPage / selections —— 镜像
// `features/bili_pack/parse.py` 的 JSON 变换层（`GD3_RUST_BILI_PAGES`）。
//
// 内嵌选页与真值语义照 Python 现状抄：
// - `x or ""` / `or {}` / `or []`：Python 真值（空串、0、False、null、空容器都为假）；
// - `int(x)`：int/float(向零截断)/bool/十进制字符串，失败抛 Python `int()` 文案；
// - `raw["cid"]` 缺失 → KeyError（桥侧映射成 Python `KeyError('cid')`）；
// - 非 list 的 "pages"/"sections"（字符串、dict 等）在本实现按缺失处理——
//   现实 JSON 不会出现（记为已知偏差）。
// ---------------------------------------------------------------------------

/// buildPage 的产出：一个扁平 dict，适配器拿去构造 `BiliPage`。
#[derive(Debug, Clone, PartialEq)]
pub struct PageOutcome {
    pub index: i64,
    pub cid: i64,
    pub page_part: String,
    pub page_number: i64,
    pub bvid: String,
    pub episode_title: String,
    pub section_title: String,
    pub cover_url: String,
    pub duration: i64,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PagesOutcome {
    pub pages: Vec<PageOutcome>,
    pub title: String,
    pub cover_url: String,
}

/// 错误通道：`KeyError("cid")` 与 `ValueError(int 文案)` 分开，桥侧映射对应异常。
#[derive(Debug, Clone, PartialEq)]
pub enum BuildPagesError {
    KeyError(String),
    PyValue(String),
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => {
            let zero = n.as_i64() == Some(0)
                || n.as_u64() == Some(0)
                || n.as_f64().map(|f| f == 0.0).unwrap_or(false);
            !zero
        }
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Python `x or default`：x 为真值取 x，否则取 default。返回 None 表示取 default。
fn or_default(value: Option<&Value>) -> Option<&Value> {
    match value {
        Some(v) if truthy(v) => Some(v),
        _ => None,
    }
}

/// Python `str(x)` 的可镜像子集：字符串原样；数字/bool 走 repr；其它按 Python 会给
/// `"{}"` 格式的子集（这里只会遇到 null 经 `or ""` 变空串前的原始值——null 已被
/// `or` 吃掉）。真值非字符串数字的情况记为已知偏差（Python repr 与逐字节相等）。
fn python_str(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(b) => python_bool_str(*b),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn python_bool_str(value: bool) -> String {
    if value { "True".into() } else { "False".into() }
}

/// Python `int(x)` 严格版：字符串走 `int()` 文案，浮点截断，bool 是 int。
fn python_int_strict(value: &Value) -> Result<i64, BuildPagesError> {
    match value {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i)
            } else if let Some(f) = n.as_f64() {
                // Python 对 NaN/inf 抛 ValueError(不能转 int 之类) —— 现实 JSON 无
                Ok(f.trunc() as i64)
            } else {
                Err(BuildPagesError::PyValue(
                    "int() argument must be a string, a bytes-like object or a real number, not 'float'".into(),
                ))
            }
        }
        Value::Bool(b) => Ok(i64::from(*b)),
        Value::String(_) => {
            python_str_to_int(value.as_str().expect("字符串分支")).map_err(BuildPagesError::PyValue)
        }
        _ => Err(BuildPagesError::PyValue(
            "int() argument must be a string, a bytes-like object or a real number, not 'NoneType'"
                .into(),
        )),
    }
}

/// `toHttps(url)`。
pub fn to_https(url: &str) -> String {
    let url = url.trim();
    if let Some(rest) = url.strip_prefix("http://") {
        return format!("https://{}", rest);
    }
    url.to_string()
}

/// `buildPage(index, raw, bvid=..., episodeTitle=..., sectionTitle=..., coverUrl=...)`。
fn build_page(
    index: i64,
    raw: &Value,
    bvid: &str,
    episode_title: &str,
    section_title: &str,
    cover_url: &str,
) -> Result<PageOutcome, BuildPagesError> {
    let page_value = raw.get("page");
    let selected_raw_number = page_value.as_ref().copied(); // 起点：raw.get("page")
    let raw_obj = raw;
    let page_number = or_as_int_or_index(selected_raw_number, index, raw_obj)?;
    // `int(raw["cid"])`：缺失 → KeyError("cid")
    let cid_value = raw
        .get("cid")
        .ok_or_else(|| BuildPagesError::KeyError("cid".into()))?;
    let cid = python_int_strict(cid_value)?;
    let part = raw.get("part").map(|v| v.as_str().unwrap_or(""));
    let page_part = part.unwrap_or("").trim().to_string();
    let duration = match or_default(raw.get("duration")) {
        Some(v) => python_int_strict(v)?,
        None => 0,
    };
    Ok(PageOutcome {
        index,
        cid,
        page_part,
        page_number,
        bvid: bvid.to_string(),
        episode_title: episode_title.to_string(),
        section_title: section_title.to_string(),
        cover_url: cover_url.to_string(),
        duration,
        selected: false,
    })
}

/// `int(raw.get("page") or index + 1)`：真值 page 走 int()，否则 index+1。
fn or_as_int_or_index(
    page: Option<&Value>,
    index: i64,
    raw: &Value,
) -> Result<i64, BuildPagesError> {
    let _ = raw;
    match or_default(page) {
        Some(v) => python_int_strict(v),
        None => Ok(index + 1),
    }
}

/// `episodeCover(episode)`。
fn episode_cover(episode: &Value) -> String {
    let arc = or_default(episode.get("arc")).and_then(|v| v.as_object());
    let pic = arc
        .and_then(|arc| or_default(arc.get("pic")))
        .map(python_str)
        .unwrap_or_default();
    if !pic.is_empty() {
        return to_https(&pic);
    }
    let cover = or_default(episode.get("cover"))
        .map(python_str)
        .unwrap_or_default();
    if !cover.is_empty() {
        return to_https(&cover);
    }
    let pic2 = or_default(episode.get("pic"))
        .map(python_str)
        .unwrap_or_default();
    to_https(&pic2)
}

/// `buildSeasonPages(season)`。
fn build_season_pages(season: &Value) -> Result<Vec<PageOutcome>, BuildPagesError> {
    let sections = season
        .get("sections")
        .and_then(|v| v.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(&[]);
    let has_many_sections = sections.len() > 1;
    let mut pages: Vec<PageOutcome> = Vec::new();
    for section in sections {
        let section_title = if has_many_sections {
            or_default(section.get("title"))
                .map(python_str)
                .unwrap_or_default()
                .trim()
                .to_string()
        } else {
            String::new()
        };
        for episode in section
            .get("episodes")
            .and_then(|v| v.as_array())
            .map(|a| a.as_slice())
            .unwrap_or(&[])
        {
            let bvid = or_default(episode.get("bvid"))
                .map(python_str)
                .unwrap_or_default();
            let episode_title = or_default(episode.get("title"))
                .map(python_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            let mut owned_pages: Vec<Value> = episode
                .get("pages")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            if owned_pages.is_empty() {
                if let Some(page) = or_default(episode.get("page")) {
                    owned_pages = vec![page.clone()];
                }
            }
            let raw_pages: Vec<&Value> = owned_pages.iter().collect();
            let cover_url = episode_cover(episode);
            for raw in raw_pages {
                let index = pages.len() as i64;
                pages.push(build_page(
                    index,
                    raw,
                    &bvid,
                    &episode_title,
                    &section_title,
                    &cover_url,
                )?);
            }
        }
    }
    Ok(pages)
}

/// `setVideoSelection(pages, selectedPages)`。
fn set_video_selection(pages: &mut [PageOutcome], selected: Option<&[i64]>) {
    let wanted: Option<Vec<i64>> = match selected {
        Some(list) => Some(list.to_vec()),
        None => Some(pages.iter().map(|p| p.page_number).collect()),
    };
    let set: std::collections::HashSet<i64> = wanted.iter().flatten().copied().collect();
    for page in pages.iter_mut() {
        page.selected = set.contains(&page.page_number);
    }
}

/// `setSeasonSelection(pages, biliUrl, currentBvid)`。
fn set_season_selection(
    pages: &mut [PageOutcome],
    season_id: i64,
    selected_pages: Option<&[i64]>,
    current_bvid: &str,
) {
    if season_id != 0 {
        for page in pages.iter_mut() {
            page.selected = true;
        }
        return;
    }
    for page in pages.iter_mut() {
        if page.bvid != current_bvid {
            page.selected = false;
        } else if selected_pages.is_none() {
            page.selected = true;
        } else {
            page.selected = selected_pages.unwrap_or(&[]).contains(&page.page_number);
        }
    }
}

/// 镜像 `buildPages(viewData, biliUrl)`。
pub fn build_pages(
    view_data: &Value,
    video_id: &str,
    mid: i64,
    season_id: i64,
    selected_pages: Option<&[i64]>,
) -> Result<PagesOutcome, BuildPagesError> {
    let season = or_default(view_data.get("ugc_season"));
    let _ = mid;
    if let Some(season_value) = season {
        let title = or_default(season_value.get("title"))
            .map(python_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        let title = if title.is_empty() {
            "bilibili_video".to_string()
        } else {
            title
        };
        let cover_raw = or_default(mode_cover(season_value, view_data))
            .map(python_str)
            .unwrap_or_default();
        let cover_url = to_https(&cover_raw);
        let mut pages = build_season_pages(season_value)?;
        let current_bvid = or_default(view_data.get("bvid"))
            .map(python_str)
            .unwrap_or_default();
        let current_bvid = if current_bvid.is_empty() {
            video_id.to_string()
        } else {
            current_bvid
        };
        set_season_selection(&mut pages, season_id, selected_pages, &current_bvid);
        Ok(PagesOutcome {
            pages,
            title,
            cover_url,
        })
    } else {
        let title = match or_default(view_data.get("title")) {
            Some(v) => python_str(v),
            None => String::new(),
        };
        let title = title.trim().to_string();
        let title = if title.is_empty() {
            "bilibili_video".to_string()
        } else {
            title
        };
        let cover_raw = or_default(view_data.get("pic"))
            .map(python_str)
            .unwrap_or_default();
        let cover_url = to_https(&cover_raw);
        let bvid_raw = or_default(view_data.get("bvid"))
            .map(python_str)
            .unwrap_or_default();
        let bvid = if bvid_raw.is_empty() {
            video_id.to_string()
        } else {
            bvid_raw
        };
        let owned: Vec<Value> = view_data
            .get("pages")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let raws: Vec<&Value> = owned.iter().collect();
        let mut pages = Vec::with_capacity(raws.len());
        for (index, raw) in raws.iter().enumerate() {
            pages.push(build_page(index as i64, raw, &bvid, "", "", "")?);
        }
        set_video_selection(&mut pages, selected_pages);
        Ok(PagesOutcome {
            pages,
            title,
            cover_url,
        })
    }
}

/// `season.get("cover") or viewData.get("pic") or ""`。
fn mode_cover<'a>(season: &'a Value, view_data: &'a Value) -> Option<&'a Value> {
    or_default(season.get("cover")).or_else(|| or_default(view_data.get("pic")))
}

#[cfg(test)]
mod build_pages_tests {
    use super::*;
    use serde_json::json;
    fn pages_of(view: Value, video_id: &str, selected: Option<&[i64]>) -> PagesOutcome {
        build_pages(&view, video_id, 0, 0, selected).expect("成功")
    }

    #[test]
    fn normal_video_pages_and_selection() {
        let view = json!({
            "bvid": "BV1x", "title": "  Hello  ", "pic": "http://a/b.jpg",
            "pages": [
                {"page": 2, "cid": 100, "part": " Alfa ", "duration": 30},
                {"cid": 101, "part": "Beta"}
            ]
        });
        let out = pages_of(view, "BV1fallback", Some(&[2]));
        assert_eq!(out.title, "Hello");
        assert_eq!(out.cover_url, "https://a/b.jpg");
        assert_eq!(out.pages[0].page_number, 2);
        assert_eq!(out.pages[0].page_part, "Alfa");
        assert_eq!(out.pages[1].page_number, 2);
        assert!(out.pages[0].selected);
        assert!(out.pages[1].selected);
    }

    #[test]
    fn no_selection_means_all_selected() {
        let view = json!({"bvid": "BV1", "pages": [{"cid": 1}, {"cid": 2}]});
        let out = pages_of(view, "", None);
        assert_eq!(out.pages[0].page_number, 1);
        assert_eq!(out.pages[1].page_number, 2);
        assert_eq!(out.pages[1].cid, 2);
        assert!(out.pages.iter().all(|p| p.selected));
    }

    #[test]
    fn season_selection_rules() {
        let view = json!({
            "bvid": "BVcurrent",
            "ugc_season": {
                "title": "S1", "cover": "http://c.jpg",
                "sections": [{
                    "title": "sec",
                    "episodes": [
                        {"bvid": "BVcurrent", "pages": [{"cid": 1, "page": 1}]},
                        {"bvid": "BVother", "pages": [{"cid": 2}]}
                    ]
                }]
            }
        });
        let out = pages_of(view.clone(), "", Some(&[]));
        assert_eq!(out.title, "S1");
        assert_eq!(out.cover_url, "https://c.jpg");
        assert_eq!(out.pages.len(), 2);
        assert_eq!(out.pages[0].cover_url, "");
        // seasonId 存在 → 全选
        let whole = build_pages(&view, "", 9, 7, Some(&[])).expect("ok");
        assert!(whole.pages.iter().all(|p| p.selected));
    }

    #[test]
    fn episode_cover_fallbacks() {
        let view = json!({
            "ugc_season": {
                "sections": [{
                    "episodes": [
                        {"arc": {"pic": "http://arc.jpg"}, "pages": [{"cid": 1}]},
                        {"cover": "http://cover.jpg", "pages": [{"cid": 2}]},
                        {"pic": "http://pic.jpg", "pages": [{"cid": 3}]},
                        {"pages": [{"cid": 4}]}
                    ]
                }]
            }
        });
        let out = pages_of(view, "", Some(&[]));
        assert_eq!(out.pages[0].cover_url, "https://arc.jpg");
        assert_eq!(out.pages[1].cover_url, "https://cover.jpg");
        assert_eq!(out.pages[2].cover_url, "https://pic.jpg");
        assert_eq!(out.pages[3].cover_url, "");
    }

    #[test]
    fn missing_cid_is_key_error() {
        let view = json!({"bvid": "BV1", "pages": [{"part": "x no cid..."}]});
        let err = build_pages(&view, "", 0, 0, None).expect_err("key error");
        assert_eq!(err, BuildPagesError::KeyError("cid".into()));
    }

    #[test]
    fn empty_everything_defaults() {
        let out = pages_of(json!({}), "", None);
        assert_eq!(out.title, "bilibili_video");
        assert!(out.pages.is_empty());
        assert_eq!(out.cover_url, "");
    }
}

// ==== fetchView（features/bili_pack/pack.py::fetchView） ====

#[derive(Debug)]
pub enum ViewError {
    /// 传输 / 非 2xx / JSON 解析失败（Python 抛 httpx 异常，文案不逐字对齐）。
    Transport(String),
    /// Python `ValueError`，文案逐字。
    ValueError(String),
}

impl std::fmt::Display for ViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ViewError::Transport(m) => write!(f, "{m}"),
            ViewError::ValueError(m) => write!(f, "{m}"),
        }
    }
}

/// Python `f"?mid={mid}&season_id={seasonId}&page_num=1&page_size=1"`。
pub fn season_list_url(mid: i64, season_id: i64) -> String {
    format!(
        "https://api.bilibili.com/x/polymer/web-space/seasons_archives_list\
         ?mid={mid}&season_id={season_id}&page_num=1&page_size=1"
    )
}

/// `av…` → `avid=` 去掉前两位；其余 → `bvid=`。`videoId` 期望非空（上层已判定）。
pub fn view_api_url(video_id: &str) -> String {
    if let Some(rest) = video_id.strip_prefix("av") {
        format!("https://api.bilibili.com/x/web-interface/view?avid={rest}")
    } else {
        format!("https://api.bilibili.com/x/web-interface/view?bvid={video_id}")
    }
}

/// `archives = (payload.get("data") or {}).get("archives") or []`；
/// 空或第一个没有 `bvid` → `ValueError("未获取到合集信息")`。
pub fn pick_season_video_id(payload: &Value) -> Result<String, ViewError> {
    let empty = vec![];
    let archives = payload
        .get("data")
        .and_then(|data| data.get("archives"))
        .and_then(|item| item.as_array())
        .unwrap_or(&empty);
    let bvid = archives
        .first()
        .and_then(|item| item.get("bvid"))
        .and_then(|item| item.as_str())
        .unwrap_or("");
    if bvid.is_empty() {
        return Err(ViewError::ValueError("未获取到合集信息".into()));
    }
    Ok(bvid.to_string())
}

/// `code not in {None, 0}` → `ValueError(message or "获取 Bilibili 视频信息失败")`；
/// 否则返回 `data or {}`。
pub fn check_view_payload(payload: &Value) -> Result<Value, ViewError> {
    let code = payload.get("code");
    if code.is_some() && code != Some(&Value::Null) && code != Some(&Value::from(0)) {
        let message = payload
            .get("message")
            .and_then(|item| item.as_str())
            .filter(|item| !item.is_empty())
            .unwrap_or("获取 Bilibili 视频信息失败");
        return Err(ViewError::ValueError(message.into()));
    }
    match payload.get("data") {
        Some(data) if !data.is_null() => Ok(data.clone()),
        _ => Ok(Value::Object(serde_json::Map::new())),
    }
}

async fn get_json(
    client: &Client,
    api_headers: &[(String, String)],
    url: &str,
) -> Result<Value, ViewError> {
    let mut headers = wreq::header::HeaderMap::new();
    for (name, value) in api_headers {
        let name = wreq::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|error| ViewError::Transport(error.to_string()))?;
        let value = wreq::header::HeaderValue::from_str(value)
            .map_err(|error| ViewError::Transport(error.to_string()))?;
        headers.insert(name, value);
    }
    let response = client
        .get(url)
        .headers(headers)
        .send()
        .await
        .map_err(|error| ViewError::Transport(error.to_string()))?;
    let status = response.status().as_u16();
    if (400..600).contains(&status) {
        return Err(ViewError::Transport(format!("HTTP {status}")));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| ViewError::Transport(error.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|error| ViewError::Transport(error.to_string()))
}

/// `fetchView(client, biliUrl)` 的 Rust 版：`videoId`、`mid`、`seasonId` 真值由桥传入。
pub async fn fetch_view(
    client: &Client,
    api_headers: &[(String, String)],
    video_id: &str,
    mid: i64,
    season_id: i64,
) -> Result<Value, ViewError> {
    let mut video_id = video_id.to_string();
    if video_id.is_empty() && season_id != 0 {
        let payload = get_json(client, api_headers, &season_list_url(mid, season_id)).await?;
        video_id = pick_season_video_id(&payload)?;
    }
    let payload = get_json(client, api_headers, &view_api_url(&video_id)).await?;
    check_view_payload(&payload)
}

#[cfg(test)]
mod view_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn view_url_uses_avid_for_av_id() {
        assert_eq!(
            view_api_url("av123"),
            "https://api.bilibili.com/x/web-interface/view?avid=123"
        );
        assert_eq!(
            view_api_url("BV1abc"),
            "https://api.bilibili.com/x/web-interface/view?bvid=BV1abc"
        );
        // Python `videoId[2:]` 只适用于前缀 `av`；其他串按 bvid 原样传
        assert_eq!(
            view_api_url("av"),
            "https://api.bilibili.com/x/web-interface/view?avid="
        );
    }

    #[test]
    fn season_list_url_has_same_query_shape() {
        assert_eq!(
            season_list_url(487, 920),
            "https://api.bilibili.com/x/polymer/web-space/seasons_archives_list\
             ?mid=487&season_id=920&page_num=1&page_size=1"
        );
    }

    #[test]
    fn pick_season_uses_first_archive_bvid() {
        let payload = json!({"data": {"archives": [{"bvid": "BV1x"}, {}]}});
        assert!(matches!(
            pick_season_video_id(&payload),
            Ok(v) if v == "BV1x"
        ));
    }

    #[test]
    fn pick_season_error_messages_are_verbatim() {
        for payload in [
            json!({}),
            json!({"data": {}}),
            json!({"data": {"archives": []}}),
            json!({"data": {"archives": [{}]}}),
        ] {
            assert!(matches!(
                pick_season_video_id(&payload),
                Err(ViewError::ValueError(m)) if m == "未获取到合集信息"
            ));
        }
    }

    #[test]
    fn check_payload_codes() {
        // code 缺失 / null / 0 都放行，取 data or {}
        for payload in [
            json!({"code": 0, "data": {"title": "t"}}),
            json!({"code": null, "data": {"title": "t"}}),
            json!({"data": {"title": "t"}}),
        ] {
            assert!(matches!(
                check_view_payload(&payload),
                Ok(data) if data["title"] == "t"
            ));
        }
        assert!(matches!(
            check_view_payload(&json!({})),
            Ok(data) if data.as_object().unwrap().is_empty()
        ));
        assert!(matches!(
            check_view_payload(&json!({"data": null})),
            Ok(data) if data.as_object().unwrap().is_empty()
        ));
        assert!(matches!(
            check_view_payload(&json!({"code": -404, "message": "稿件不可见"})),
            Err(ViewError::ValueError(m)) if m == "稿件不可见"
        ));
        assert!(matches!(
            check_view_payload(&json!({"code": -404})),
            Err(ViewError::ValueError(m)) if m == "获取 Bilibili 视频信息失败"
        ));
    }
}

// ==== account 纯函数（features/bili_pack/account.py） ====

/// `MIXIN_KEY_ENC_TAB` 逐字照抄。
const MIXIN_KEY_ENC_TAB: [usize; 64] = [
    46, 47, 18, 2, 53, 8, 23, 32, 15, 50, 10, 31, 58, 3, 45, 35, 27, 43, 5, 49, 33, 9, 42, 19, 29,
    28, 14, 39, 12, 38, 41, 13, 37, 48, 7, 16, 24, 55, 40, 61, 26, 17, 0, 1, 60, 51, 30, 4, 22, 25,
    54, 21, 56, 59, 6, 63, 57, 62, 11, 36, 20, 34, 44, 52,
];

/// `_updateWbiKeys` 的 key 派生：`img_url/sub_url` 各取末段去掉 `.` 后缀
/// （空 URL → 空 key），img 与 sub 都非空才有 key；
/// `reduce(lambda s, i: s + orig[i], TAB, "")[:32]`。
///
/// `Ok(None)` 对应 Python 里 `imgKey and subKey` 为假（调用方保持原 key）；
/// `Err(index)` 对应 Python 的 `orig[index]` 抛 `IndexError`（两段合起来不足 64 字符，
/// 只有伪造的 URL 才会出现）——桥侧按原样抛 `IndexError`。
pub fn mixin_key_from_urls(img_url: &str, sub_url: &str) -> Result<Option<String>, usize> {
    fn key_of(url: &str) -> String {
        let last = url.rsplit('/').next().unwrap_or("");
        let stem = last.split('.').next().unwrap_or("");
        stem.to_string()
    }
    let img_key = key_of(img_url);
    let sub_key = key_of(sub_url);
    if img_key.is_empty() || sub_key.is_empty() {
        return Ok(None);
    }
    let orig = format!("{img_key}{sub_key}");
    let chars: Vec<char> = orig.chars().collect();
    let mut mixin = String::new();
    for index in MIXIN_KEY_ENC_TAB {
        match chars.get(index) {
            Some(ch) => mixin.push(*ch),
            None => return Err(index),
        }
    }
    Ok(Some(mixin.chars().take(32).collect()))
}

/// Python `urllib.parse.quote_plus`（`urlencode` 的默认 `quote_via`）：
/// `ALWAYS_SAFE` = 字母数字 + `_.-~` 原样，空格 → `+`，其余按 UTF-8 字节 `%XX`。
/// 注意 `*` **不在**安全集里（Python 3.7 起），会被编成 `%2A`。
fn quote_plus(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-' | b'~') {
            out.push(byte as char);
        } else if byte == b' ' {
            out.push('+');
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

use md5::{Digest, Md5};

/// `md5(hexdigest())` 小写串。`entries` 已在桥侧排序并清洗。
pub fn wbi_rid(entries: &[(String, String)], mixin: &str) -> String {
    let query: Vec<String> = entries
        .iter()
        .map(|(name, value)| format!("{}={}", quote_plus(name), quote_plus(value)))
        .collect();
    let plain = format!("{}{mixin}", query.join("&"));
    let mut hasher = Md5::new();
    hasher.update(plain.as_bytes());
    let digest = hasher.finalize();
    digest
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect::<String>()
}

pub const COOKIE_ORDER: [&str; 5] = [
    "SESSDATA",
    "bili_jct",
    "DedeUserID",
    "DedeUserID__ckMd5",
    "sid",
];

/// CPython `str.strip()` 的空白集：Unicode `White_Space` 之外还含 `\x1c`–`\x1f`
/// （Rust 的 `char::is_whitespace` 不含这四个，直接 trim 会留下它们）。
fn is_python_space(ch: char) -> bool {
    ch.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&ch)
}

/// `toCookie(raw)`：分号整理、重名取后者、`COOKIE_ORDER` 排前其余按原序排后。
pub fn to_cookie(raw: &str) -> String {
    let mut parts: Vec<(String, String)> = Vec::new();
    let normalized = raw.replace(['\r', '\n'], ";");
    for part in normalized.split(';') {
        let part = part.trim_matches(is_python_space);
        if part.is_empty() {
            continue;
        }
        let Some((name, value)) = part.split_once('=') else {
            continue;
        };
        let (name, value) = (
            name.trim_matches(is_python_space),
            value.trim_matches(is_python_space),
        );
        if name.is_empty() || value.is_empty() {
            continue;
        }
        if let Some(slot) = parts.iter_mut().find(|(n, _)| n == name) {
            slot.1 = value.to_string();
        } else {
            parts.push((name.to_string(), value.to_string()));
        }
    }
    if parts.is_empty() {
        return String::new();
    }
    let mut ordered: Vec<(String, String)> = Vec::new();
    for want in COOKIE_ORDER {
        if let Some(pos) = parts.iter().position(|(name, _)| name == want) {
            ordered.push(parts.remove(pos));
        }
    }
    ordered.extend(parts);
    ordered
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod account_tests {
    use super::*;
    #[test]
    fn mixin_key_matches_table_mapping() {
        let orig: String = (0..64).map(|i| char::from(b'a' + (i as u8 % 26))).collect();
        let img = format!("/{0}.jpg", &orig[..32]);
        let sub = format!("/{0}.png", &orig[32..]);
        let key = mixin_key_from_urls(&img, &sub).unwrap().unwrap();
        let full = MIXIN_KEY_ENC_TAB
            .iter()
            .map(|&i| orig.chars().nth(i).unwrap())
            .collect::<String>();
        let expected: String = full.chars().take(32).collect();
        assert_eq!(key, expected);
    }

    #[test]
    fn mixin_key_needs_both_urls() {
        assert_eq!(mixin_key_from_urls("", "x.png"), Ok(None));
        assert_eq!(mixin_key_from_urls("/img.jpg", ""), Ok(None));
        // 真 key 是 32 字符（`orig` 刚好 64）；`img.v2.jpg` 取到 `img`。
        let (img, sub) = ("a".repeat(32), "b".repeat(32));
        let (img_url, sub_url) = (format!("/{img}.v2.jpg"), format!("/{sub}.png"));
        assert!(mixin_key_from_urls(&img_url, &sub_url).unwrap().is_some());
    }

    #[test]
    fn mixin_key_shorter_than_the_table_is_an_index_error() {
        // Python: `reduce(lambda s, i: s + orig[i], TAB, "")` 会先撞上越界索引
        // （TAB 首项是 46，`"ab"` 只有 2 个字符）。
        assert_eq!(mixin_key_from_urls("/a.jpg", "/b.png"), Err(46));
    }

    #[test]
    fn quote_plus_matches_python() {
        assert_eq!(quote_plus("hello world"), "hello+world");
        assert_eq!(quote_plus("a&b=c"), "a%26b%3Dc");
        assert_eq!(quote_plus("a*b-c_d.e~f"), "a%2Ab-c_d.e~f");
        assert_eq!(quote_plus("中文"), "%E4%B8%AD%E6%96%87");
        assert_eq!(quote_plus(""), "");
    }

    #[test]
    fn to_cookie_orders_dedups_and_cleans() {
        assert_eq!(
            to_cookie(" junk=0;  sid=9; SESSDATA=abc; sid=1; bili_jct=f  "),
            "SESSDATA=abc; bili_jct=f; sid=1; junk=0"
        );
        assert_eq!(to_cookie("  ; junk=  ; "), "");
        // CPython 的 `str.strip()` 连 `\x1c`–`\x1f` 一起剥掉。
        assert_eq!(to_cookie("sid=1\u{1c}"), "sid=1");
        assert_eq!(to_cookie("\u{1c}sid=1\u{1d}"), "sid=1");
        assert_eq!(to_cookie("sid=9"), "sid=9");
    }

    #[test]
    fn wbi_rid_is_lowercase_hex32() {
        let rid = wbi_rid(
            &[("cid".into(), "11".into()), ("qn".into(), "80".into())],
            "mix",
        );
        assert_eq!(rid.chars().count(), 32);
        assert!(
            rid.chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        );
    }
}

/// `fetchPlayurl` 的默认错误文案（`message` 缺失 / 假值时的兜底）。
pub const PLAYURL_DEFAULT_ERROR: &str = "获取 Bilibili 音视频流失败";

/// `fetchPlayurl` 取流响应判定层的产出。
///
/// Python 侧 `playurlGate(playPayload, qn)` 返回
/// `'fallback' | ('error', message) | ('ok', dash, acceptQuality)`；
/// `AttributeError` / `TypeError` 单独成变体（桥侧按原型抛出，文案逐字一致）。
#[derive(Debug, Clone, PartialEq)]
pub enum PlayurlGate {
    Ok {
        /// `dash = pageData.get("dash") or {}`（缺省/假值归一化为 `{}`）。
        dash: Value,
        accept_quality: Vec<Value>,
    },
    Fallback,
    /// `ValueError(message)`——`message or 默认`：消息值原样（可以是任何 JSON 值）。
    Error(Value),
    AttributeError(String),
    TypeError(String),
}

/// JSON 值对应的 Python 类型名（`AttributeError`/`TypeError` 文案用）。
fn py_json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(number) => {
            if number.is_f64() {
                "float"
            } else {
                "int"
            }
        }
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// `x.get(...)` 的 AttributeError 文案。
fn get_error(value: &Value) -> PlayurlGate {
    PlayurlGate::AttributeError(format!(
        "'{}' object has no attribute 'get'",
        py_json_type(value)
    ))
}

/// `list(x)` 不可迭代时的 TypeError 文案。
fn not_iterable(value: &Value) -> PlayurlGate {
    PlayurlGate::TypeError(format!("'{}' object is not iterable", py_json_type(value)))
}

/// `x in {None, 0}`：None / 数字 0 / `False` 通过；
/// list/dict 不可哈希 → `TypeError: unhashable type: …`（成员测试先于相等比较）。
fn code_in_none_zero(code: Option<&Value>) -> Result<bool, PlayurlGate> {
    match code {
        None | Some(Value::Null) => Ok(true),
        Some(Value::Number(number)) => Ok(number.as_f64().map(|f| f == 0.0).unwrap_or(false)),
        Some(Value::Bool(b)) => Ok(!b),
        Some(Value::Array(_)) => Err(PlayurlGate::TypeError(
            "unhashable type: 'list'".to_string(),
        )),
        Some(Value::Object(_)) => Err(PlayurlGate::TypeError(
            "unhashable type: 'dict'".to_string(),
        )),
        Some(_) => Ok(false),
    }
}

/// `fetchPlayurl` 响应判定层（`features/bili_pack/stream.py::playurlGate` 的镜像）。
///
/// 顺序严格对齐 Python（这里每一步都可能先抛）：
/// `code in {None, 0}` → qn 兜底 / `message or 默认` →
/// `pageData or {}` → `dash or {}` → `list(video or [])` 非空 →
/// qn 兜底 → `list(accept_quality or [])`。
pub fn playurl_gate(play_payload: &Value, qn: i64) -> PlayurlGate {
    let root = match play_payload {
        Value::Object(map) => map,
        other => return get_error(other),
    };

    match code_in_none_zero(root.get("code")) {
        Err(error) => return error,
        Ok(false) => {
            if qn != 80 {
                return PlayurlGate::Fallback;
            }
            let message = or_default(root.get("message")).cloned();
            return PlayurlGate::Error(
                message.unwrap_or_else(|| Value::from(PLAYURL_DEFAULT_ERROR)),
            );
        }
        Ok(true) => {}
    }

    // pageData = playPayload.get("data") or {}
    let page_map: Option<&serde_json::Map<String, Value>> = match root.get("data") {
        Some(value) if truthy(value) => match value {
            Value::Object(map) => Some(map),
            other => return get_error(other),
        },
        _ => None,
    };

    // dash = pageData.get("dash") or {}
    let dash: Option<&serde_json::Map<String, Value>> = match page_map.and_then(|m| m.get("dash")) {
        Some(value) if truthy(value) => match value {
            Value::Object(map) => Some(map),
            other => return get_error(other),
        },
        _ => None,
    };

    // videoStreams = list(dash.get("video") or []) → 非空与否
    let has_streams = match dash.and_then(|m| m.get("video")) {
        None | Some(Value::Null) => false,
        Some(value) if !truthy(value) => false,
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        Some(other) => return not_iterable(other),
    };
    if !has_streams && qn != 80 {
        return PlayurlGate::Fallback;
    }

    // acceptQuality = list(pageData.get("accept_quality") or [])
    let accept_quality: Vec<Value> = match page_map.and_then(|m| m.get("accept_quality")) {
        None => Vec::new(),
        Some(value) if !truthy(value) => Vec::new(),
        Some(Value::Array(a)) => a.to_vec(),
        Some(Value::String(s)) => s.chars().map(|c| Value::String(c.to_string())).collect(),
        Some(Value::Object(o)) => o.keys().map(|key| Value::from(key.as_str())).collect(),
        Some(other) => return not_iterable(other),
    };

    PlayurlGate::Ok {
        dash: match dash {
            Some(map) => Value::Object(map.clone()),
            None => Value::Object(serde_json::Map::new()),
        },
        accept_quality,
    }
}

#[cfg(test)]
mod playurl_gate_tests {
    use super::*;
    use serde_json::json;

    fn dash_ok(dash: Value, quality: Vec<Value>) -> PlayurlGate {
        PlayurlGate::Ok {
            dash,
            accept_quality: quality,
        }
    }

    #[test]
    fn ok_full_keeps_dash_and_quality() {
        let payload = json!({
            "code": 0,
            "data": {
                "dash": {"video": [{"id": 80}], "audio": [{"id": 30280}]},
                "accept_quality": [120, 80],
            },
        });
        assert_eq!(
            playurl_gate(&payload, 120),
            dash_ok(
                json!({"video": [{"id": 80}], "audio": [{"id": 30280}]}),
                vec![json!(120), json!(80)]
            )
        );
    }

    #[test]
    fn ok_normalizes_falsy_dash_and_quality() {
        // dash/accept_quality 缺失或假值 → {} / []（qn=80 不走兜底）
        assert_eq!(playurl_gate(&json!({}), 80), dash_ok(json!({}), vec![]));
        assert_eq!(
            playurl_gate(&json!({"code": 0, "data": serde_json::Value::Null}), 80),
            dash_ok(json!({}), vec![])
        );
        assert_eq!(
            playurl_gate(
                &json!({"code": 0, "data": {"dash": serde_json::Value::Null}}),
                80
            ),
            dash_ok(json!({}), vec![])
        );
        assert_eq!(
            playurl_gate(
                &json!({"code": 0, "data": {"dash": {}, "accept_quality": 0}}),
                80
            ),
            dash_ok(json!({}), vec![])
        );
    }

    #[test]
    fn ok_code_zero_variants() {
        // `code in {None, 0}`：0 / 0.0 / False / 缺失 / None 都通过
        for code in [
            None,
            Some(json!(0)).as_ref(),
            Some(json!(0.0)).as_ref(),
            Some(json!(false)).as_ref(),
        ] {
            let mut payload = json!({"data": {"dash": {"video": [1]}}});
            if let Some(value) = code.cloned() {
                payload["code"] = value;
            }
            assert!(
                matches!(playurl_gate(&payload, 80), PlayurlGate::Ok { .. }),
                "{code:?}"
            );
        }
        // 非零 → qn≠80 兜底，qn=80 报错
        assert_eq!(
            playurl_gate(&json!({"code": 5, "message": "m"}), 120),
            PlayurlGate::Fallback
        );
        assert_eq!(
            playurl_gate(&json!({"code": 5, "message": "m"}), 80),
            PlayurlGate::Error(json!("m"))
        );
    }

    #[test]
    fn code_error_message_or_default() {
        assert_eq!(
            playurl_gate(&json!({"code": 1}), 80),
            PlayurlGate::Error(json!(PLAYURL_DEFAULT_ERROR))
        );
        assert_eq!(
            playurl_gate(&json!({"code": 1, "message": ""}), 80),
            PlayurlGate::Error(json!(PLAYURL_DEFAULT_ERROR))
        );
        assert_eq!(
            playurl_gate(&json!({"code": 1, "message": false}), 80),
            PlayurlGate::Error(json!(PLAYURL_DEFAULT_ERROR))
        );
        assert_eq!(
            playurl_gate(&json!({"code": "0", "message": "风控"}), 80),
            PlayurlGate::Error(json!("风控"))
        );
        assert_eq!(
            playurl_gate(&json!({"code": 1, "message": 123}), 80),
            PlayurlGate::Error(json!(123))
        );
        assert_eq!(
            playurl_gate(&json!({"code": true, "message": "m"}), 80),
            PlayurlGate::Error(json!("m"))
        );
    }

    #[test]
    fn code_unhashable_list_dict() {
        assert_eq!(
            playurl_gate(&json!({"code": []}), 120),
            PlayurlGate::TypeError("unhashable type: 'list'".into())
        );
        assert_eq!(
            playurl_gate(&json!({"code": {}}), 120),
            PlayurlGate::TypeError("unhashable type: 'dict'".into())
        );
        // 可哈希的假值字符串按 == 判定失败，不可哈希则先于比较抛出
        assert_eq!(
            playurl_gate(&json!({"code": ""}), 120),
            PlayurlGate::Fallback
        );
    }

    #[test]
    fn root_not_dict_is_attribute_error() {
        let cases: Vec<(Value, &str)> = vec![
            (json!(5), "int"),
            (json!(1.5), "float"),
            (json!("plain"), "str"),
            (json!([1]), "list"),
            (json!(null), "NoneType"),
            (json!(true), "bool"),
        ];
        for (payload, kind) in cases {
            assert_eq!(
                playurl_gate(&payload, 80),
                PlayurlGate::AttributeError(format!("'{kind}' object has no attribute 'get'")),
                "{kind}"
            );
        }
    }

    #[test]
    fn truthy_non_dict_data_or_dash_is_attribute_error() {
        for bad in [json!(5), json!(1.5), json!("x"), json!([1]), json!(true)] {
            assert!(
                matches!(
                    playurl_gate(&json!({"code": 0, "data": bad}), 80),
                    PlayurlGate::AttributeError(_)
                ),
                "{bad:?}"
            );
            assert!(
                matches!(
                    playurl_gate(&json!({"code": 0, "data": {"dash": bad}}), 80),
                    PlayurlGate::AttributeError(_)
                ),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn video_streams_presence_and_fallback() {
        // 空/缺省 → qn≠80 兜底；qn=80 放行（空 dash 交给后续 parseDash 报错）
        assert_eq!(
            playurl_gate(&json!({"code": 0, "data": {"dash": {"video": []}}}), 120),
            PlayurlGate::Fallback
        );
        assert_eq!(
            playurl_gate(&json!({"code": 0, "data": {}}), 120),
            PlayurlGate::Fallback
        );
        assert!(matches!(
            playurl_gate(&json!({"code": 0, "data": {"dash": {"video": []}}}), 80),
            PlayurlGate::Ok { ref dash, .. } if dash.get("video").is_some()
        ));
        // str/dict 可迭代（Python list(str)=字符、list(dict)=键）
        assert!(matches!(
            playurl_gate(&json!({"code": 0, "data": {"dash": {"video": "ab"}}}), 120),
            PlayurlGate::Ok { .. }
        ));
        assert!(matches!(
            playurl_gate(
                &json!({"code": 0, "data": {"dash": {"video": {"a": 1}}}}),
                120
            ),
            PlayurlGate::Ok { .. }
        ));
        // 不可迭代 → TypeError（逐类型）
        assert_eq!(
            playurl_gate(&json!({"code": 0, "data": {"dash": {"video": 5}}}), 120),
            PlayurlGate::TypeError("'int' object is not iterable".into())
        );
        assert_eq!(
            playurl_gate(&json!({"code": 0, "data": {"dash": {"video": 1.5}}}), 120),
            PlayurlGate::TypeError("'float' object is not iterable".into())
        );
        assert_eq!(
            playurl_gate(&json!({"code": 0, "data": {"dash": {"video": true}}}), 120),
            PlayurlGate::TypeError("'bool' object is not iterable".into())
        );
    }

    #[test]
    fn accept_quality_list_semantics() {
        // list(str)=字符、list(dict)=键、list(bool/num) 抛 TypeError
        assert_eq!(
            playurl_gate(
                &json!({"code": 0, "data": {"dash": {"video": [1]}, "accept_quality": "xy"}}),
                80
            ),
            dash_ok(json!({"video": [1]}), vec![json!("x"), json!("y")])
        );
        assert_eq!(
            playurl_gate(
                &json!({"code": 0, "data": {"dash": {"video": [1]}, "accept_quality": {"k": 1}}}),
                80
            ),
            dash_ok(json!({"video": [1]}), vec![json!("k")])
        );
        assert_eq!(
            playurl_gate(
                &json!({"code": 0, "data": {"dash": {"video": [1]}, "accept_quality": 5}}),
                80
            ),
            PlayurlGate::TypeError("'int' object is not iterable".to_string())
        );
        // 兜底优先于 accept_quality 的求值错误
        assert_eq!(
            playurl_gate(&json!({"code": 0, "data": {"accept_quality": 5}}), 120),
            PlayurlGate::Fallback
        );
    }
}
