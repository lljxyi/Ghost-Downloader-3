//! HLS 清单解析（镜像第三方 `m3u8` 库 `parser.py` / `model.py`）。
//!
//! 影子开关 `GD3_RUST_M3U8_PARSE`，服务两个调用面：
//! - `features/m3u8_pack/pack.py` 的 isLive 判定块（`m3u8.loads` 的三个观测点：
//!   `playlist.segments` 非空、`playlist.is_endlist`、`playlists[0].absolute_uri`
//!   的 raw `uri`；absolute_uri 的 urljoin 留在 Python 适配层）；
//! - `M3U8Parser._hlsStreams`(变体的 `resolution`/`codecs`/`frame_rate`)。
//!
//! 镜像语义（均对照 CPython 3.13 + venv 里的 `m3u8` 库逐条实测）：
//! - `string_to_lines = content.strip().splitlines()`：先整文 strip（Unicode
//!   White_Space + `\x1c`–`\x1f`），再按 CPython splitlines 的分隔符切
//!   （`\n`/`\r`/`\r\n`/`\v`/`\f`/`\x1c`–`\x1e`/`\u{85}`/`\u{2028}`/`\u{2029}`）；
//! - 每行再 `line.strip()`；tag 识别一律 `startswith`（大小写敏感：
//!   `#Ext-X-Stream-Inf` 不识别；`#EXT-X-ENDLISTX` 识别且置 endlist=True）；
//! - 属性列表用 `ATTRIBUTELISTPATTERN.split(...)[1::2]` —— 这里手工模拟
//!   正则的贪心跳分隔符语义（见 `split_tokens`）；
//! - `remove_quotes`：首尾同属 `"`/`'` 两个字符才剥（一次，不是循环剥）；
//! - `normalize_attribute = name.replace("-", "_").lower().strip()`（先 lower 后 strip）；
//! - 属性 cast 在该行扫描到该属性时立即发生（错误立刻上抛，中断整个 parse）；
//!   变体构造（resolution 的 int 对、缺 `bandwidth` 的 KeyError）在全部行解析完
//!   之后按 playlists 顺序发生，单条内先 resolution 再查 bandwidth；
//!   keys（EXT-X-KEY 缺 METHOD 的 TypeError 的 model 时点）排在 playlists 之前；
//! - 类型转换语义：`int(str)`（10 进制，Unicode 空白 strip、可选正负号、数字间
//!   单个 `_`）；`int(float(x))`（bandwidth 路径：`inf` → OverflowError
//!   "cannot convert float infinity to integer"，`nan` → ValueError
//!   "cannot convert float NaN to integer"）；`float(str)`（ASCII 数字体 +
//!   空白 strip、正负号、`.`、指数、`_` 数字间规则、`inf`/`infinity`/`nan`）；
//! - `#EXT-X-KEY` 缺 METHOD → `TypeError: Key.__init__() missing 1 required
//!   positional argument: 'method'`（Key.__init__ 有 `**kwargs`，多余属性不报错）；
//!   KEY param 缺 `=` → `ValueError: not enough values to unpack (expected 2,
//!   got 1)`；I-FRAME/IMAGE 缺 URI → `KeyError: 'uri'`；
//! - 段状态：`#EXT-X-BYTERANGE`/`#EXTINF` 建 pending 段并设 expect_segment；
//!   `#EXT-X-BITRATE` 建 pending 段但不设 expect_segment；EOF 未消费则追加计数；
//! - 报错文本的 repr 用 CPython `repr()` 的子集镜像（见 `python_repr`）。
//!
//! 已知偏差（金文件语料刻意不覆盖，实例语料罕见）：
//! - `#EXT-X-DATERANGE` / `#EXT-X-PROGRAM-DATE-TIME`（fromisoformat）、
//!   `#EXT-X-START`（time_offset float）、`#EXT-X-MAP`（model 层
//!   InitializationSection）、`#EXT-X-SERVER-CONTROL`/`PART`/`PRELOAD-HINT`/
//!   `SESSION-DATA`/`TILES` 等少数 tag 家族的 cast 报错不镜像（Python 会报、
//!   Rust 按静默 tag 跳过）；
//! - IMAGE playlist 的 model 层 resolution cast 不镜像（I-FRAME 的镜像了）；
//! - KEY 行出现 `BASE-URI=`（撞上 Key.__init__ 的 base_uri 形参）时不镜像
//!   「duplicate argument」类 TypeError；
//! - `int("١٢٣")` 这类 Unicode 十进制数字不镜像（按 ASCII 数字体处理），超
//!   i64 的大数结果 clamp（宽度/高度观测面不取这些，值本身不影响 parity）。

use std::fmt;

/// `m3u8` 库加载/构造过程中的 Python 异常镜像。
///
/// 适配层按 `kind` 挑同型异常类再抛（`ValueError`/`OverflowError`/`TypeError`/
/// `KeyError`/`IndexError`），`message` 是 Python `str(e)` 的原文：
/// KeyError/IndexError 带引号（`'bandwidth'`），TypeError/ValueError 不带。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HlsError {
    pub kind: String,
    pub message: String,
}

impl HlsError {
    fn new(kind: &str, message: impl Into<String>) -> Self {
        HlsError {
            kind: kind.to_string(),
            message: message.into(),
        }
    }
}

impl fmt::Display for HlsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

/// 变体的观测字段（`Playlist.uri` + `Playlist.stream_info` 的子集）。
/// `uri` 是清单里的 uri 原文，没有做 urljoin（absolute_uri 在 Python 适配层算）。
#[derive(Debug, Clone, PartialEq)]
pub struct HlsVariant {
    pub uri: String,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub codecs: Option<String>,
    pub frame_rate: Option<f64>,
}

/// `m3u8.loads(body)` 在本迁移里的可见子集。
#[derive(Debug, Clone, PartialEq)]
pub struct HlsProbe {
    pub is_endlist: bool,
    pub segment_count: u64,
    pub variants: Vec<HlsVariant>,
}

/// 解析一段 HLS 清单（镜像 `m3u8.loads(body)`，`strict=False`）。
pub fn probe(body: &str) -> Result<HlsProbe, HlsError> {
    let state = scan(body)?;

    // model 层构造顺序（`M3U8._initialize_attributes`）：keys → segments →
    // simple attrs → media → playlists → iframes → images。
    for params in state.keys.iter().flatten() {
        if params.iter().all(|attr| attr.name != "method") {
            return Err(HlsError::new(
                "TypeError",
                "Key.__init__() missing 1 required positional argument: 'method'",
            ));
        }
    }

    let mut variants = Vec::with_capacity(state.playlists.len());
    for entry in &state.playlists {
        let (width, height) = resolution_pair(entry.info.resolution.as_deref(), true)?;
        if !entry.info.band_ok {
            return Err(HlsError::new("KeyError", "'bandwidth'"));
        }
        variants.push(HlsVariant {
            uri: entry.uri.clone(),
            width,
            height,
            codecs: entry.info.codecs.clone(),
            frame_rate: entry.info.frame_rate,
        });
    }
    for entry in &state.iframes {
        let _ = resolution_pair(entry.as_deref(), false)?;
    }
    for entry in &state.images {
        let _ = resolution_pair(entry.as_deref(), false)?;
    }

    Ok(HlsProbe {
        is_endlist: state.is_endlist,
        segment_count: state.segment_count,
        variants,
    })
}

// ---------------------------------------------------------------------------
// 解析器状态
// ---------------------------------------------------------------------------

/// 单个属性 token（`ATTRIBUTELISTPATTERN` 的捕获 + `param.split("=", 1)`）。
#[derive(Clone, Debug)]
struct Attr {
    name: String,
    value: String,
}

/// 一个待挂到 URI 行上的 STREAM-INF 状态（`state["stream_info"]`）。
#[derive(Default)]
struct StreamInfo {
    resolution: Option<String>,
    codecs: Option<String>,
    frame_rate: Option<f64>,
    band_ok: bool,
}

/// 一条已完成配对的变体（`data["playlists"]` 条目）。
struct PlaylistEntry {
    uri: String,
    info: StreamInfo,
}

#[derive(Default)]
struct State {
    keys: Vec<Option<Vec<Attr>>>,
    current_key: Option<Vec<Attr>>,
    pending_stream_info: Option<StreamInfo>,
    playlists: Vec<PlaylistEntry>,
    iframes: Vec<Option<String>>, // raw resolution
    images: Vec<Option<String>>,  // raw resolution
    is_endlist: bool,
    segment_count: u64,
    pending: bool, // `state["segment"]` 存在
    expect_segment: bool,
    expect_playlist: bool,
}

// ---------------------------------------------------------------------------
// 扫描阶段（`parser.py::parse` 的镜像）
// ---------------------------------------------------------------------------

fn scan(body: &str) -> Result<State, HlsError> {
    let mut state = State::default();
    let lines = split_lines(body.trim_matches(is_python_space));
    for raw in lines {
        let line = raw.trim_matches(is_python_space);
        if line.starts_with("#EXT-X-BYTERANGE") {
            // `_parse_byterange`：建 pending 段 + expect_segment。
            state.pending = true;
            state.expect_segment = true;
        } else if line.starts_with("#EXT-X-BITRATE") {
            state.pending = true;
            py_simple_param(line)?;
        } else if line.starts_with("#EXT-X-TARGETDURATION")
            || line.starts_with("#EXT-X-MEDIA-SEQUENCE")
            || line.starts_with("#EXT-X-DISCONTINUITY-SEQUENCE")
        {
            py_simple_param(line)?;
        } else if line.starts_with("#EXT-X-PROGRAM-DATE-TIME")
            || line.starts_with("#EXT-X-DISCONTINUITY")
            || line.starts_with("#EXT-X-CUE-OUT-CONT")
            || line.starts_with("#EXT-X-CUE-OUT")
            || line.starts_with("#EXT-OATCLS-SCTE35:")
            || line.starts_with("#EXT-X-ASSET:")
            || line.starts_with("#EXT-X-CUE-IN")
            || line.starts_with("#EXT-X-CUE-SPAN")
            || line.starts_with("#EXT-X-VERSION")
            || line.starts_with("#EXT-X-ALLOW-CACHE")
        {
            // 静默类 tag（部分 cast 报错不镜像，见模块头「已知偏差」）。
        } else if line.starts_with("#EXT-X-KEY") {
            parse_key(line, &mut state)?;
        } else if line.starts_with("#EXTINF") {
            parse_extinf(line, &mut state)?;
        } else if line.starts_with("#EXT-X-STREAM-INF") {
            let info = parse_stream_inf(line)?;
            state.pending_stream_info = Some(info);
            state.expect_playlist = true;
        } else if line.starts_with("#EXT-X-I-FRAME-STREAM-INF") {
            state.iframes.push(parse_i_frame(line)?);
        } else if line.starts_with("#EXT-X-MEDIA")
            || line.starts_with("#EXT-X-PLAYLIST-TYPE")
            || line.starts_with("#EXT-X-I-FRAMES-ONLY")
            || line.starts_with("#EXT-X-INDEPENDENT-SEGMENTS")
        {
            // 静默类 tag。
        } else if line.starts_with("#EXT-X-ENDLIST") {
            state.is_endlist = true;
        } else if line.starts_with("#EXT-X-MAP")
            || line.starts_with("#EXT-X-START")
            || line.starts_with("#EXT-X-SERVER-CONTROL")
            || line.starts_with("#EXT-X-PART-INF")
            || line.starts_with("#EXT-X-RENDITION-REPORT")
            || line.starts_with("#EXT-X-PART")
            || line.starts_with("#EXT-X-SKIP")
            || line.starts_with("#EXT-X-SESSION-DATA")
            || line.starts_with("#EXT-X-SESSION-KEY")
            || line.starts_with("#EXT-X-PRELOAD-HINT")
            || line.starts_with("#EXT-X-DATERANGE")
            || line.starts_with("#EXT-X-GAP")
            || line.starts_with("#EXT-X-CONTENT-STEERING")
        {
            // 静默类 tag。
        } else if line.starts_with("#EXT-X-IMAGE-STREAM-INF") {
            state.images.push(parse_image(line)?);
        } else if line.starts_with("#EXT-X-IMAGES-ONLY") || line.starts_with("#EXT-X-TILES") {
            // 静默类 tag。
        } else if line.starts_with("#EXTM3U") || line.is_empty() {
            // 忽略。
        } else if !line.starts_with('#') && state.expect_segment {
            consume_ts_chunk(&mut state);
        } else if !line.starts_with('#') && state.expect_playlist {
            let info = state
                .pending_stream_info
                .take()
                .expect("expect_playlist 蕴含已设置 stream_info");
            state.playlists.push(PlaylistEntry {
                uri: line.to_string(),
                info,
            });
            state.expect_playlist = false;
        }
    }
    // `parse` 收尾：未消费的 pending 段追加。
    if state.pending {
        state.segment_count += 1;
    }
    Ok(state)
}

// ---------------------------------------------------------------------------
// 行级处理器
// ---------------------------------------------------------------------------

/// `_parse_key`：属性必须能按 `=` 切成两段；值走 `remove_quotes`；
/// 相同 key（dict 相等）不重复入 `data["keys"]`。
fn parse_key(line: &str, state: &mut State) -> Result<(), HlsError> {
    let mut key = Vec::new();
    for token in split_tokens(&line.replace("#EXT-X-KEY:", "")) {
        let (name, value) = match token.split_once('=') {
            Some(pair) => pair,
            None => {
                return Err(HlsError::new(
                    "ValueError",
                    "not enough values to unpack (expected 2, got 1)",
                ));
            }
        };
        key.push(Attr {
            name: normalize_attribute(name),
            value: remove_quotes(value).to_string(),
        });
    }
    state.current_key = Some(key.clone());
    let is_duplicate = state
        .keys
        .iter()
        .any(|existing| attr_lists_eq(existing.as_deref(), Some(&key)));
    if !is_duplicate {
        state.keys.push(Some(key));
    }
    Ok(())
}

/// 键值对列表按 Python dict 相等比较（有序不敏感）。
fn attr_lists_eq(a: Option<&[Attr]>, b: Option<&[Attr]>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => {
            if a.len() != b.len() {
                return false;
            }
            a.iter().all(|entry| {
                b.iter()
                    .any(|other| entry.name == other.name && entry.value == other.value)
            })
        }
        _ => false,
    }
}

/// `_parse_extinf`：全文 replace 后按第一个 `,` 切，duration 走 `float` cast。
fn parse_extinf(line: &str, state: &mut State) -> Result<(), HlsError> {
    let rest = line.replace("#EXTINF:", "");
    let duration = match rest.split_once(',') {
        Some((duration, _title)) => duration,
        None => rest.as_str(),
    };
    py_float(duration)?;
    state.pending = true;
    state.expect_segment = true;
    Ok(())
}

/// `_parse_stream_inf`：cast 在 token 出现顺序上就地发生；`state["stream_info"]`
/// 整体替换（属性列表为空时不报错，只留 `{}`，缺 BANDWIDTH 在 model 时点报）。
fn parse_stream_inf(line: &str) -> Result<StreamInfo, HlsError> {
    let mut info = StreamInfo::default();
    if line.starts_with("#EXT-X-STREAM-INF:") {
        for (name, value) in raw_params(line, "#EXT-X-STREAM-INF")? {
            match name.as_str() {
                "codecs" => info.codecs = Some(remove_quotes(&value).to_string()),
                "audio" | "video" | "video_range" | "subtitles" | "pathway_id"
                | "stable_variant_id" => {}
                "program_id" | "average_bandwidth" => {
                    py_int(&value)?;
                }
                "bandwidth" => {
                    py_int_float(&value)?;
                    info.band_ok = true;
                }
                "frame_rate" => info.frame_rate = Some(py_float(&value)?),
                "resolution" => info.resolution = Some(value.to_string()),
                _ => {}
            }
        }
    }
    Ok(info)
}

/// `_parse_i_frame_stream_inf` / `_parse_image_stream_inf`：两个 handler 的
/// 可观测面一致（int cast + 缺 URI 的 KeyError + raw resolution 交付 model 层）。
fn parse_i_frame_or_image(line: &str, prefix: &str) -> Result<Option<String>, HlsError> {
    let mut resolution: Option<String> = None;
    let mut has_uri = false;
    if line.starts_with(&format!("{prefix}:")) {
        for (name, value) in raw_params(line, prefix)? {
            match name.as_str() {
                "codecs" | "pathway_id" | "stable_variant_id" => {}
                "uri" => has_uri = true,
                "program_id" | "bandwidth" | "average_bandwidth" => {
                    py_int(&value)?;
                }
                "resolution" => resolution = Some(value.to_string()),
                _ => {}
            }
        }
    }
    if !has_uri {
        return Err(HlsError::new("KeyError", "'uri'"));
    }
    Ok(resolution)
}

fn parse_i_frame(line: &str) -> Result<Option<String>, HlsError> {
    parse_i_frame_or_image(line, "#EXT-X-I-FRAME-STREAM-INF")
}

fn parse_image(line: &str) -> Result<Option<String>, HlsError> {
    parse_i_frame_or_image(line, "#EXT-X-IMAGE-STREAM-INF")
}

/// `_parse_ts_chunk`：消费 pending 段（append 进 `data["segments"]`，计数 +1）；
/// 无 current_key 时补一个 None key。
fn consume_ts_chunk(state: &mut State) {
    state.segment_count += 1;
    let has_living_key = matches!(&state.current_key, Some(key) if !key.is_empty());
    if !has_living_key && !state.keys.iter().any(|key| key.is_none()) {
        state.keys.push(None);
    }
    state.pending = false;
    state.expect_segment = false;
}

/// `_parse_simple_parameter(cast_to=int)`：`value = line.split(":", 1)[1]`
/// （缺 `:` 时 ValueError），再 `value.strip().lower()` 后走 `int`。
fn py_simple_param(line: &str) -> Result<(), HlsError> {
    let value = match line.split_once(':') {
        Some((_param, value)) => value,
        None => {
            return Err(HlsError::new(
                "ValueError",
                "not enough values to unpack (expected 2, got 1)",
            ));
        }
    };
    let value = value.trim_matches(is_python_space).to_lowercase();
    py_int(&value)?;
    Ok(())
}

/// `_parse_attribute_list` 的 token 展开：prefix strip（全文 replace）→
/// token 切分 → `param.split("=", 1)`（缺 `=` 时 name = ""）→ normalize。
/// prefix 不匹配（`line.startswith(prefix + ":")` 为假）返回空列表，无 cast。
fn raw_params(line: &str, prefix: &str) -> Result<Vec<(String, String)>, HlsError> {
    let body = line.replace(&format!("{prefix}:"), "");
    let mut out = Vec::new();
    for token in split_tokens(&body) {
        let (name, value) = match token.split_once('=') {
            Some(pair) => pair,
            None => ("", token.as_str()),
        };
        out.push((normalize_attribute(name), value.to_string()));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// CPython 文本/数字语义 helper
// ---------------------------------------------------------------------------

/// Python `str.strip()`（无参）的白名单：Unicode White_Space + `\x1c`–`\x1f`。
fn is_python_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `key = normalize_attribute(name)`：`replace("-", "_")` → 全小写 → strip。
fn normalize_attribute(name: &str) -> String {
    name.replace('-', "_")
        .to_lowercase()
        .trim_matches(is_python_space)
        .to_string()
}

/// m3u8 库的 `remove_quotes`：`startswith(('"', "'")) and endswith(('"', "'"))`
/// 才剥（首尾**各自**属引号集合即可，混搭也剥；一次，不是循环剥）。
/// 单字符真引号（`value[0] == value[-1]`）也剥成空串。
fn remove_quotes(value: &str) -> &str {
    let is_quote = |c: Option<char>| matches!(c, Some('"') | Some('\''));
    if is_quote(value.chars().next()) && is_quote(value.chars().last()) {
        // 引号字符都是单字节，len>=2 时切片安全；len<=2 时剥成空串。
        if value.len() > 2 {
            &value[1..value.len() - 1]
        } else {
            ""
        }
    } else {
        value
    }
}

/// CPython `str.splitlines` 的分隔符集合（`\r\n` 算一个）。
fn split_lines(content: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < content.len() {
        let (span_end, sep_end) = match content[i..].chars().next() {
            Some('\n') => (i, i + 1),
            Some('\r') => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    (i, i + 2)
                } else {
                    (i, i + 1)
                }
            }
            Some('\u{b}') | Some('\u{c}') | Some('\u{1c}') | Some('\u{1d}') | Some('\u{1e}')
            | Some('\u{85}') | Some('\u{2028}') | Some('\u{2029}') => {
                let c = content[i..].chars().next().unwrap();
                (i, i + c.len_utf8())
            }
            _ => {
                let c = content[i..].chars().next().unwrap();
                i += c.len_utf8();
                continue;
            }
        };
        lines.push(&content[start..span_end]);
        start = sep_end;
        i = start;
    }
    lines.push(&content[start..]);
    lines
}

/// `ATTRIBUTELISTPATTERN`（`((?:[^,"']|"[^"]*"|'[^']*')+)+` 能匹配的最长 token 流）。
/// 手工模拟 re.split 的捕获排列：医形引擎逐字符寻位，遇 `,` 或无配对引号的引号
/// 就停 token，从下一个字符重新尝试。
fn split_tokens(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let mut token = String::new();
        let mut j = i;
        while j < chars.len() {
            let c = chars[j];
            if c == '"' || c == '\'' {
                match chars[j + 1..].iter().position(|other| *other == c) {
                    Some(offset) => {
                        let close = j + 1 + offset;
                        token.extend(chars[j..=close].iter());
                        j = close + 1;
                    }
                    None => break,
                }
            } else if c == ',' {
                break;
            } else {
                token.push(c);
                j += 1;
            }
        }
        if token.is_empty() {
            i += 1;
        } else {
            tokens.push(token);
            i = j;
        }
    }
    tokens
}

/// model 层的 resolution cast：`stream_info`（带 strip('"')）与 I-FRAME/IMAGE（裸值）。
/// 报错顺序照 Python：先 `int(values[0])`，缺第二值给 IndexError。
fn resolution_pair(
    text: Option<&str>,
    strip_quotes: bool,
) -> Result<(Option<i64>, Option<i64>), HlsError> {
    let Some(text) = text else {
        return Ok((None, None));
    };
    let text = if strip_quotes {
        text.trim_matches('"')
    } else {
        text
    };
    let values: Vec<&str> = text.split('x').collect();
    let width = py_int(values[0])?;
    let Some(second) = values.get(1) else {
        return Err(HlsError::new("IndexError", "list index out of range"));
    };
    let height = py_int(second)?;
    Ok((Some(width), Some(height)))
}

/// CPython `int(str)`（10 进制）：Unicode 空白 strip、正负号、ASCII 数一体、
/// 数字间单个 `_`；超 i64 的值 clamp（偏差见模块头）。
/// 报错 repr 用传给 `int()` 的**原文**（CPython 观测：外侧空白保留，如
/// `int(' 1__2 ')` → `invalid literal for int() with base 10: ' 1__2 '`）。
fn py_int(text: &str) -> Result<i64, HlsError> {
    let stripped = text.trim_matches(is_python_space);
    let negative = stripped.starts_with('-');
    let body = stripped.strip_prefix(['+', '-']).unwrap_or(stripped);
    let digits = validate_int_body(body, text)?;
    match digits.parse::<i128>() {
        Ok(value) => Ok(value.clamp(i64::MIN as i128, i64::MAX as i128) as i64),
        Err(_) => {
            // 位数上溢：Python 是任意精度，这里取 i64 极值（观测面不取该值）。
            Ok(if negative { i64::MIN } else { i64::MAX })
        }
    }
}

/// int 数一体校验：ASCII 数字 + 数字间单个 `_`（`_` 两侧必须是数字）；
/// 至少一位数字。返回去掉下划线后的数一体。
fn validate_int_body(body: &str, repr_source: &str) -> Result<String, HlsError> {
    let mut has_digit = false;
    for c in body.chars() {
        match c {
            '0'..='9' => has_digit = true,
            '_' => {}
            _ => return Err(int_literal_error(repr_source)),
        }
    }
    if bad_underscores(body) || !has_digit {
        return Err(int_literal_error(repr_source));
    }
    let mut cleaned = String::with_capacity(body.len());
    for c in body.chars() {
        if c != '_' {
            cleaned.push(c);
        }
    }
    Ok(cleaned)
}

/// `int(float(x))`（bandwidth 路径）：float 解析成功后按 Python
/// `float.__trunc__` 的报错面分类；值不再被观测（只关心成功/失败）。
fn py_int_float(text: &str) -> Result<(), HlsError> {
    match py_float(text)? {
        value if value.is_infinite() => Err(HlsError::new(
            "OverflowError",
            "cannot convert float infinity to integer",
        )),
        value if value.is_nan() => Err(HlsError::new(
            "ValueError",
            "cannot convert float NaN to integer",
        )),
        _ => Ok(()),
    }
}

/// CPython `float(str)`：Unicode 空白 strip、正负号、`inf`/`infinity`/`nan`
/// （大小写不限）、ASCII 数一体（`.` + 指数 + `_` 数字间规则）。
/// 内嵌空白非法（`float('1 e2')` 报错）；报错 repr 用**原文**（含外侧空白）。
fn py_float(text: &str) -> Result<f64, HlsError> {
    let stripped = text.trim_matches(is_python_space);
    let negative = stripped.starts_with('-');
    let body = stripped.strip_prefix(['+', '-']).unwrap_or(stripped);
    match body.to_ascii_lowercase().as_str() {
        "inf" | "infinity" => {
            return Ok(if negative {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            });
        }
        "nan" => return Ok(f64::NAN),
        _ => {}
    }
    let body = validate_float_body(body, text)?;
    let value = body.parse::<f64>().map_err(|_| float_literal_error(text))?;
    Ok(if negative { -value } else { value })
}

fn float_literal_error(repr_source: &str) -> HlsError {
    HlsError::new(
        "ValueError",
        format!(
            "could not convert string to float: {}",
            python_repr(repr_source)
        ),
    )
}

fn int_literal_error(repr_source: &str) -> HlsError {
    HlsError::new(
        "ValueError",
        format!(
            "invalid literal for int() with base 10: {}",
            python_repr(repr_source)
        ),
    )
}

/// float 数一体校验（正文已过 sign/inf/nan 分支）：允许数字、`_`（两侧数字）、
/// 至多一个 `.`、至多一个 `e`/`E`；尾数总位数 ≥ 1；指数段 `e[+-]?digits`
/// （指数段不得有 `.`，digits 至少一位，无内嵌空白）。返回可交给
/// `f64::from_str` 的规范体（下划线移除）。
fn validate_float_body(body: &str, repr_source: &str) -> Result<String, HlsError> {
    // 允许 `looksLike` 数字骨架：数字/`_`/`./e/E` 任意位置，`+`/`-` 仅允许
    // 紧跟 `e`/`E`（顶部 sign 已在 `py_float` 剥离）。
    let body_chars: Vec<char> = body.chars().collect();
    for (i, c) in body_chars.iter().enumerate() {
        let sign_ok = matches!(c, '+' | '-') && i > 0 && matches!(body_chars[i - 1], 'e' | 'E');
        if !matches!(c, '0'..='9' | '_' | '.' | 'e' | 'E') && !sign_ok {
            return Err(float_literal_error(repr_source));
        }
    }
    if bad_underscores(body) {
        return Err(float_literal_error(repr_source));
    }
    let (mantissa, exponent) = match body.find(['e', 'E']) {
        Some(pos) => (&body[..pos], Some(&body[pos + 1..])),
        None => (body, None),
    };
    let (value_before, value_after) = match mantissa.split_once('.') {
        Some((before, after)) => (before, Some(after)),
        None => (mantissa, None),
    };
    let mantissa_digits = value_before.chars().filter(|c| c.is_ascii_digit()).count()
        + value_after
            .unwrap_or("")
            .chars()
            .filter(|c| c.is_ascii_digit())
            .count();
    let exponent_invalid = |raw: &str| -> bool {
        // `_` 的合法性即全量 `bad_underscores` 已按字符位置判定，这里只排除 `.`
        // 与空白，digits 至少一位。
        let digits = raw.strip_prefix(['+', '-']).unwrap_or(raw);
        raw.contains('.')
            || digits.is_empty()
            || digits.bytes().any(|b| !b.is_ascii_digit() && b != b'_')
    };
    if mantissa_digits == 0
        || value_after.is_some_and(|after| after.contains('.'))
        || exponent.is_some_and(exponent_invalid)
    {
        return Err(float_literal_error(repr_source));
    }
    let mut normalized = String::with_capacity(body.len());
    normalized.push_str(&value_before.replace('_', ""));
    if let Some(after) = value_after {
        normalized.push('.');
        normalized.push_str(&after.replace('_', ""));
    }
    if let Some(raw) = exponent {
        let digits_part = raw.strip_prefix(['+', '-']).unwrap_or(raw);
        normalized.push('e');
        if raw.starts_with('-') {
            normalized.push('-');
        } else if raw.starts_with('+') {
            normalized.push('+');
        }
        normalized.push_str(&digits_part.replace('_', ""));
    }
    Ok(normalized)
}

/// `_` 两侧（按字符位置）必须都是 ASCII 数字，否则不合格：
/// `1_2` OK；`1__2`/`_12`/`12_`/`1_.5`/`1._5`/`.5_`/`._5`/`0_e1` 不合格。
fn bad_underscores(body: &str) -> bool {
    let chars: Vec<char> = body.chars().collect();
    chars.iter().enumerate().any(|(i, c)| {
        *c == '_'
            && (i == 0
                || !chars[i - 1].is_ascii_digit()
                || i + 1 >= chars.len()
                || !chars[i + 1].is_ascii_digit())
    })
}

/// CPython `repr(str)` 的镜像（单/双引号选择 + 控制字符/不可打印字符转义）。
fn python_repr(text: &str) -> String {
    let has_single = text.contains('\'');
    let has_double = text.contains('"');
    let (open, close) = if has_single && !has_double {
        ('"', '"')
    } else {
        ('\'', '\'')
    };
    let mut out = String::with_capacity(text.len() + 2);
    out.push(open);
    for c in text.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\\' => out.push_str("\\\\"),
            _ if !is_repr_printable(c) => {
                if (u32::from(c)) < 0x100 {
                    out.push_str(&format!("\\x{:02x}", u32::from(c)));
                } else if u32::from(c) <= 0xffff {
                    out.push_str(&format!("\\u{:04x}", u32::from(c)));
                } else {
                    out.push_str(&format!("\\U{:08x}", u32::from(c)));
                }
            }
            _ if c == open => {
                out.push('\\');
                out.push(open);
            }
            _ => out.push(c),
        }
    }
    out.push(close);
    out
}

/// CPython `str.isprintable` 的子集镜像（用于 repr）：Cc 类（含 `\x1c`–`\x1f`、
/// C1 区）与非 ` ` 的 Z 系列（如 `\u2028`、`\u00a0`）按不可打印处理。
fn is_repr_printable(c: char) -> bool {
    !c.is_control()
        && !matches!(
            c,
            '\u{85}' | '\u{a0}' | '\u{1680}' | '\u{2000}'
                ..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}'
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int_message(text: &str) -> String {
        py_int(text).unwrap_err().message
    }

    fn float_message(text: &str) -> String {
        py_float(text).unwrap_err().message
    }

    #[test]
    fn py_int_follows_cpython() {
        for text in ["1_2", " 12 ", "\u{00a0}12", "+1_2"] {
            assert_eq!(py_int(text).unwrap(), 12, "int({text:?})");
        }
        assert_eq!(py_int("-0").unwrap(), 0);
        assert_eq!(py_int("0012").unwrap(), 12);
        for text in ["1__2", "_12", "12_", "1e2", "1.0", "0x10", "", "+"] {
            assert!(
                int_message(text).starts_with("invalid literal for int() with base 10: '"),
                "int({text:?}) -> {:?}",
                int_message(text)
            );
        }
        assert_eq!(
            int_message("1__2"),
            "invalid literal for int() with base 10: '1__2'"
        );
        assert_eq!(
            int_message(" 1__2 "),
            "invalid literal for int() with base 10: ' 1__2 '"
        );
        assert_eq!(
            int_message("- 12"),
            "invalid literal for int() with base 10: '- 12'"
        );
        assert_eq!(
            int_message("١٢٣"),
            "invalid literal for int() with base 10: '١٢٣'"
        );
    }

    #[test]
    fn py_float_follows_cpython() {
        assert_eq!(py_float("1.5").unwrap(), 1.5);
        assert_eq!(py_float("1.").unwrap(), 1.0);
        assert_eq!(py_float(".5").unwrap(), 0.5);
        assert_eq!(py_float("1.e2").unwrap(), 100.0);
        assert_eq!(py_float(" 1.5e2 ").unwrap(), 150.0);
        assert_eq!(py_float("1_0.5").unwrap(), 10.5);
        assert_eq!(py_float("1.5e+2_0").unwrap(), 1.5e+20);
        assert_eq!(py_float("1e400").unwrap(), f64::INFINITY);
        assert_eq!(py_float("-.5").unwrap(), -0.5);
        assert_eq!(py_float("-inf").unwrap(), f64::NEG_INFINITY);
        assert!(py_float("nan").unwrap().is_nan());
        for text in [
            "e1", "1e", "1e_2", "1 e2", "1 .5", "1. 5", "1.5e 2", "1e +2", "- 1", "1_.5", "1._5",
            "1.5_", ".5_", "._5", "0e", "0_e1", "1ee2", "1e+", ".e1", "1e-x", "+", "", "x", ".",
        ] {
            assert!(
                float_message(text).starts_with("could not convert string to float: '"),
                "float({text:?}) -> {:?}",
                float_message(text)
            );
        }
        assert_eq!(
            float_message(" 1e "),
            "could not convert string to float: ' 1e '"
        );
        assert_eq!(
            float_message("1e_2"),
            "could not convert string to float: '1e_2'"
        );
    }

    #[test]
    fn py_int_float_bridges_cpython_float_truncation() {
        assert!(py_int_float("3.7").is_ok());
        assert_eq!(
            py_int_float("inf").unwrap_err().message,
            "cannot convert float infinity to integer"
        );
        assert_eq!(
            py_int_float("1e999").unwrap_err().message,
            "cannot convert float infinity to integer"
        );
        assert_eq!(
            py_int_float("nan").unwrap_err().message,
            "cannot convert float NaN to integer"
        );
        assert_eq!(
            py_int_float("x").unwrap_err().message,
            "could not convert string to float: 'x'"
        );
    }

    #[test]
    fn split_tokens_mirrors_the_regex() {
        assert_eq!(
            split_tokens("BANDWIDTH=2000,RESOLUTION=1920x1080"),
            ["BANDWIDTH=2000", "RESOLUTION=1920x1080"]
        );
        assert_eq!(
            split_tokens(r#"CODECS="a,b","c""#),
            [r#"CODECS="a,b""#, r#""c""#]
        );
        // 引号无配对：引擎从下一字符重试，`unclosed` 与 `B` 各成 token。
        assert_eq!(split_tokens("\"unclosed,B"), ["unclosed", "B"]);
        assert_eq!(split_tokens("a,\"b"), ["a", "b"]);
        assert_eq!(split_tokens("X="), ["X="]);
    }

    #[test]
    fn resolution_pair_bounds() {
        assert_eq!(
            resolution_pair(Some("1920x1080"), true).unwrap(),
            (Some(1920), Some(1080))
        );
        assert_eq!(
            resolution_pair(Some("1920x1080"), false).unwrap(),
            (Some(1920), Some(1080))
        );
        assert_eq!(
            resolution_pair(Some("3.0"), true).unwrap_err().message,
            "invalid literal for int() with base 10: '3.0'"
        );
        assert_eq!(
            resolution_pair(Some("1920"), true).unwrap_err().message,
            "list index out of range"
        );
    }

    #[test]
    fn probe_reports_media_playlist_observables() {
        let probe = probe(
            "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-ENDLIST\n#EXTINF:5.5,\ntitle\nseg1.ts\n",
        )
        .unwrap();
        assert!(probe.is_endlist);
        assert_eq!(probe.segment_count, 1);
        assert!(probe.variants.is_empty());
    }

    #[test]
    fn probe_reports_master_playlist_variants() {
        let probe = probe(
            "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1280000,AVERAGE-BANDWIDTH=1024_000,CODECS=\"avc1.4d401f\",RESOLUTION=1920x1080\nmain.m3u8\n",
        )
        .unwrap();
        assert_eq!(probe.variants.len(), 1);
        let variant = &probe.variants[0];
        assert_eq!(variant.uri, "main.m3u8");
        assert_eq!(variant.width, Some(1920));
        assert_eq!(variant.height, Some(1080));
        assert_eq!(variant.codecs.as_deref(), Some("avc1.4d401f"));
    }

    #[test]
    fn probe_surfaces_model_level_errors_in_order() {
        // 缺 BANDWIDTH（model 时点）。
        let error =
            probe("#EXTM3U\n#EXT-X-STREAM-INF:RESOLUTION=1920x1080\nmain.m3u8\n").unwrap_err();
        assert_eq!(error.kind, "KeyError");
        assert_eq!(error.message, "'bandwidth'");
        // 缺 METHOD（keys 先于 playlists）。
        let error =
            probe("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=100\n#EXT-X-KEY:URI=\"x\"\nmain.m3u8\n")
                .unwrap_err();
        assert_eq!(error.kind, "TypeError");
        // I-FRAME 缺 URI。
        let error = probe("#EXTM3U\n#EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=9\n").unwrap_err();
        assert_eq!(error.kind, "KeyError");
        assert_eq!(error.message, "'uri'");
    }

    #[test]
    fn remove_quotes_mirrors_parser_utils() {
        assert_eq!(remove_quotes("\"a\""), "a");
        assert_eq!(remove_quotes("'a'"), "a");
        assert_eq!(remove_quotes("\"a'"), "a");
        assert_eq!(remove_quotes("'a\""), "a");
        assert_eq!(remove_quotes("'a"), "'a");
        assert_eq!(remove_quotes("\""), "");
        assert_eq!(remove_quotes(""), "");
    }

    #[test]
    fn python_repr_matches_cpython() {
        assert_eq!(python_repr("x"), "'x'");
        assert_eq!(python_repr("'"), "\"'\"");
        assert_eq!(python_repr("\""), "'\"'");
        assert_eq!(python_repr("'\""), "'\\'\"'");
        assert_eq!(python_repr("\n"), "'\\n'");
        assert_eq!(python_repr("\u{0}"), "'\\x00'");
        assert_eq!(python_repr("\u{85}"), "'\\x85'");
        assert_eq!(python_repr("\u{a0}"), "'\\xa0'");
    }
}
