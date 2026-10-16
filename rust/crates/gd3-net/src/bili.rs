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

use serde_json::Value;

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
