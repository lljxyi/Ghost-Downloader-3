//! fMP4 / WebM 时间段下载的字节区间数学：镜像 `app/container.py`。
//!
//! 纯函数，无 I/O：从流头部算出「哪一段字节覆盖请求的时间段」，并把 moov 里的
//! 时长与 chunk 偏移改写成相对片段起点，让 ffmpeg 能直接读这个片段。
//! 规格来自 `tests/test_container.py`（真实 YouTube 头部 fixture）。

use std::collections::HashMap;
use std::fmt;

/// 一个可直接播放的字节区间。
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentRange {
    /// 片段头部：ftyp + moov，chunk 偏移与时长已改写到片段坐标系。
    pub patched_header: Vec<u8>,
    pub seg_start: u64,
    pub seg_end: u64,
    pub seg_start_time: f64,
    pub seg_duration: f64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ContainerError {
    /// 头部里没有 sidx box（Python 侧抛 `ValueError("sidx box not found")`）。
    SidxNotFound,
    /// 读到了预期之外的文件末尾。
    UnexpectedEnd,
    /// EBML 结构不合法。
    InvalidEbml(&'static str),
    /// 不是 Segment 元素。
    ExpectedSegment,
    /// WebM 头部没有 cue 点（Python 侧会 `IndexError`）。
    NoCues,
}

impl fmt::Display for ContainerError {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ContainerError::SidxNotFound => write!(formatter, "sidx box not found"),
            ContainerError::UnexpectedEnd => write!(formatter, "unexpected end of data"),
            ContainerError::InvalidEbml(reason) => write!(formatter, "invalid EBML: {reason}"),
            ContainerError::ExpectedSegment => write!(formatter, "expected Segment element"),
            ContainerError::NoCues => write!(formatter, "no cue points"),
        }
    }
}

impl std::error::Error for ContainerError {}

type Result<T> = std::result::Result<T, ContainerError>;

fn u32_at(data: &[u8], pos: usize) -> Result<u32> {
    data.get(pos..pos + 4)
        .map(|bytes| u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        .ok_or(ContainerError::UnexpectedEnd)
}

fn i64_at(data: &[u8], pos: usize) -> Result<i64> {
    data.get(pos..pos + 8)
        .map(|bytes| i64::from_be_bytes(bytes.try_into().expect("8 字节")))
        .ok_or(ContainerError::UnexpectedEnd)
}

fn u16_at(data: &[u8], pos: usize) -> Result<u16> {
    data.get(pos..pos + 2)
        .map(|bytes| u16::from_be_bytes([bytes[0], bytes[1]]))
        .ok_or(ContainerError::UnexpectedEnd)
}

fn byte_at(data: &[u8], pos: usize) -> Result<u8> {
    data.get(pos).copied().ok_or(ContainerError::UnexpectedEnd)
}

fn box_type(data: &[u8], pos: usize) -> Result<&[u8]> {
    data.get(pos + 4..pos + 8)
        .ok_or(ContainerError::UnexpectedEnd)
}

/// 时间段的起点与终点（秒），Python 侧收的是 int。
pub fn build_mp4_segment_range(
    header_data: &[u8],
    start_time: f64,
    end_time: f64,
) -> Result<SegmentRange> {
    let boxes = parse_mp4_boxes(header_data);
    let (moov_offset, moov_size) = boxes.get("moov").copied().unwrap_or((0, 0));
    let (sidx_offset, sidx_size) = boxes.get("sidx").copied().unwrap_or((0, 0));
    if sidx_size == 0 {
        return Err(ContainerError::SidxNotFound);
    }

    let data_start = (sidx_offset + sidx_size) as u64;
    let references = parse_sidx(header_data, sidx_offset, sidx_size)?;

    let (seg_start, seg_end, seg_start_time, seg_duration) =
        build_byte_range(&references, data_start, start_time, end_time);

    // ftyp + moov：丢掉 sidx（它的字节偏移对部分文件已失效，
    // fMP4 的 moof 是自描述的，ffmpeg 顺序读即可）
    let init_end = moov_offset + moov_size;
    let mut patched = header_data
        .get(..init_end)
        .ok_or(ContainerError::UnexpectedEnd)?
        .to_vec();
    let moov_data = header_data
        .get(moov_offset..init_end)
        .ok_or(ContainerError::UnexpectedEnd)?;
    patch_chunk_offsets(
        &mut patched,
        moov_offset,
        moov_data,
        0,
        moov_data.len(),
        init_end as i64 - seg_start as i64,
    )?;
    patch_moov_duration(&mut patched, moov_offset, moov_data, seg_duration)?;

    Ok(SegmentRange {
        patched_header: patched,
        seg_start,
        seg_end,
        seg_start_time,
        seg_duration,
    })
}

// ── MP4 box 解析 ─────────────────────────────────────────────

pub fn parse_mp4_boxes(data: &[u8]) -> HashMap<String, (usize, usize)> {
    let mut boxes = HashMap::new();
    let mut pos = 0usize;
    while pos + 8 <= data.len() {
        let size = match u32_at(data, pos) {
            Ok(size) => size as usize,
            Err(_) => break,
        };
        let name = String::from_utf8_lossy(&data[pos + 4..pos + 8]).into_owned();
        if size < 8 {
            break;
        }
        boxes.insert(name, (pos, size));
        pos += size;
    }
    boxes
}

/// 返回 `(子片段时长秒, 子片段字节数)`。
pub fn parse_sidx(data: &[u8], offset: usize, _size: usize) -> Result<Vec<(f64, u64)>> {
    let mut pos = offset + 8;
    let version = byte_at(data, pos)?;
    pos += 4; // version + flags
    pos += 4; // reference_ID
    let timescale = u32_at(data, pos)?;
    pos += 4;

    if version == 0 {
        pos += 8; // earliest_presentation_time + first_offset，各 32 位
    } else {
        pos += 16; // 各 64 位
    }

    pos += 2; // reserved
    let reference_count = u16_at(data, pos)?;
    pos += 2;

    let mut references = Vec::with_capacity(reference_count as usize);
    for _ in 0..reference_count {
        let field1 = u32_at(data, pos)?;
        let referenced_size = (field1 & 0x7FFF_FFFF) as u64;
        let subsegment_duration = u32_at(data, pos + 4)?;
        pos += 12; // referenced_size + subsegment_duration + SAP 字段
        let duration_sec = subsegment_duration as f64 / timescale as f64;
        references.push((duration_sec, referenced_size));
    }

    Ok(references)
}

/// 找出覆盖 `[start_time, end_time]` 的字节区间。
pub fn build_byte_range(
    references: &[(f64, u64)],
    header_size: u64,
    start_time: f64,
    end_time: f64,
) -> (u64, u64, f64, f64) {
    let mut byte_pos = header_size;
    let mut time_sec = 0.0;
    let mut seg_start = header_size;
    let mut seg_end = header_size;
    let mut seg_start_time = 0.0;
    let mut seg_end_time = 0.0;
    let mut found_start = false;

    for (duration, size) in references {
        let ref_end = time_sec + duration;
        if !found_start && ref_end > start_time {
            seg_start = byte_pos;
            seg_start_time = time_sec;
            found_start = true;
        }
        byte_pos += size;
        if ref_end >= end_time {
            seg_end = byte_pos;
            seg_end_time = ref_end;
            break;
        }
        if found_start {
            seg_end = byte_pos;
            seg_end_time = ref_end;
        }
        time_sec = ref_end;
    }

    if seg_end <= seg_start {
        seg_end = byte_pos;
    }

    (
        seg_start,
        seg_end,
        seg_start_time,
        seg_end_time - seg_start_time,
    )
}

fn patch_moov_duration(
    header: &mut [u8],
    moov_offset: usize,
    moov_data: &[u8],
    seg_duration: f64,
) -> Result<()> {
    let mut movie_timescale = 0u32;
    let mut pos = 8usize;
    let end = moov_data.len();
    while pos + 8 <= end {
        let size = u32_at(moov_data, pos)? as usize;
        let kind = box_type(moov_data, pos)?;
        if size < 8 || pos + size > end {
            break;
        }
        if kind == b"mvhd" {
            let version = byte_at(moov_data, pos + 8)?;
            movie_timescale = u32_at(moov_data, pos + if version == 0 { 20 } else { 28 })?;
            let new_duration =
                (seg_duration * movie_timescale as f64) as i64 + movie_timescale as i64;
            if version == 0 {
                let abs_off = moov_offset + pos + 24;
                write_u32(header, abs_off, new_duration as u32)?;
            } else {
                let abs_off = moov_offset + pos + 32;
                write_i64(header, abs_off, new_duration)?;
            }
            break;
        }
        pos += size;
    }

    if movie_timescale == 0 {
        return Ok(());
    }

    pos = 8;
    while pos + 8 <= end {
        let size = u32_at(moov_data, pos)? as usize;
        let kind = box_type(moov_data, pos)?;
        if size < 8 || pos + size > end {
            break;
        }
        if kind == b"trak" {
            patch_track_duration(
                header,
                moov_offset,
                moov_data,
                pos + 8,
                pos + size,
                seg_duration,
                movie_timescale,
            )?;
        }
        pos += size;
    }
    Ok(())
}

fn patch_track_duration(
    header: &mut [u8],
    moov_offset: usize,
    data: &[u8],
    start: usize,
    end: usize,
    seg_duration: f64,
    movie_timescale: u32,
) -> Result<()> {
    let mut pos = start;
    while pos + 8 <= end {
        let size = u32_at(data, pos)? as usize;
        let kind = box_type(data, pos)?;
        if size < 8 || pos + size > end {
            break;
        }
        if kind == b"tkhd" {
            let version = byte_at(data, pos + 8)?;
            let new_duration =
                (seg_duration * movie_timescale as f64) as i64 + movie_timescale as i64;
            if version == 0 {
                write_u32(header, moov_offset + pos + 28, new_duration as u32)?;
            } else {
                write_i64(header, moov_offset + pos + 36, new_duration)?;
            }
        } else if kind == b"mdia" {
            patch_mdhd_duration(header, moov_offset, data, pos + 8, pos + size, seg_duration)?;
        }
        pos += size;
    }
    Ok(())
}

fn patch_mdhd_duration(
    header: &mut [u8],
    moov_offset: usize,
    data: &[u8],
    start: usize,
    end: usize,
    seg_duration: f64,
) -> Result<()> {
    let mut pos = start;
    while pos + 8 <= end {
        let size = u32_at(data, pos)? as usize;
        let kind = box_type(data, pos)?;
        if size < 8 || pos + size > end {
            break;
        }
        if kind == b"mdhd" {
            let version = byte_at(data, pos + 8)?;
            if version == 0 {
                let timescale = u32_at(data, pos + 20)?;
                let new_duration = (seg_duration * timescale as f64) as i64 + timescale as i64;
                write_u32(header, moov_offset + pos + 24, new_duration as u32)?;
            } else {
                let timescale = u32_at(data, pos + 28)?;
                let new_duration = (seg_duration * timescale as f64) as i64 + timescale as i64;
                write_i64(header, moov_offset + pos + 32, new_duration)?;
            }
            break;
        }
        pos += size;
    }
    Ok(())
}

fn patch_chunk_offsets(
    header: &mut [u8],
    base_offset: usize,
    data: &[u8],
    start: usize,
    end: usize,
    delta: i64,
) -> Result<()> {
    const CONTAINER_BOXES: [&[u8]; 5] = [b"moov", b"trak", b"mdia", b"minf", b"stbl"];
    let mut pos = start;
    while pos + 8 <= end {
        let size = u32_at(data, pos)? as usize;
        let kind = box_type(data, pos)?;
        if size < 8 || pos + size > end {
            break;
        }
        if CONTAINER_BOXES.contains(&kind) {
            patch_chunk_offsets(header, base_offset, data, pos + 8, pos + size, delta)?;
        } else if kind == b"stco" {
            patch_offset_entries(header, base_offset + pos, data, pos, size, delta, 4)?;
        } else if kind == b"co64" {
            patch_offset_entries(header, base_offset + pos, data, pos, size, delta, 8)?;
        }
        pos += size;
    }
    Ok(())
}

fn patch_offset_entries(
    header: &mut [u8],
    abs_offset: usize,
    data: &[u8],
    pos: usize,
    size: usize,
    delta: i64,
    entry_size: usize,
) -> Result<()> {
    const BOX_HEADER_SIZE: usize = 12; // box header (8) + version (1) + flags (3)
    let entry_count = u32_at(data, pos + BOX_HEADER_SIZE)?;
    let entries_start = pos + BOX_HEADER_SIZE + 4;

    for index in 0..entry_count as usize {
        let off = entries_start + index * entry_size;
        if off + entry_size > pos + size {
            break;
        }
        let write_off = abs_offset + (off - pos);
        if entry_size == 4 {
            let value = u32_at(data, off)? as i64;
            write_u32(header, write_off, (value + delta) as u32)?;
        } else {
            let value = i64_at(data, off)?;
            write_i64(header, write_off, value.wrapping_add(delta))?;
        }
    }
    Ok(())
}

fn write_u32(header: &mut [u8], offset: usize, value: u32) -> Result<()> {
    let target = header
        .get_mut(offset..offset + 4)
        .ok_or(ContainerError::UnexpectedEnd)?;
    target.copy_from_slice(&value.to_be_bytes());
    Ok(())
}

fn write_i64(header: &mut [u8], offset: usize, value: i64) -> Result<()> {
    let target = header
        .get_mut(offset..offset + 8)
        .ok_or(ContainerError::UnexpectedEnd)?;
    target.copy_from_slice(&value.to_be_bytes());
    Ok(())
}

// ── WebM / EBML 解析 ─────────────────────────────────────────

pub fn parse_ebml_var_int(data: &[u8], pos: usize) -> Result<(u64, usize)> {
    let first = byte_at(data, pos)?;
    if first == 0 {
        return Err(ContainerError::InvalidEbml("invalid EBML varint"));
    }
    let mut length = 1usize;
    let mut mask = 0x80u8;
    while length <= 8 && (first & mask) == 0 {
        length += 1;
        mask >>= 1;
    }
    let mut value = (first & (mask - 1)) as u64;
    for index in 1..length {
        value = (value << 8) | byte_at(data, pos + index)? as u64;
    }
    Ok((value, pos + length))
}

pub fn parse_ebml_element_id(data: &[u8], pos: usize) -> Result<(u64, usize)> {
    let first = byte_at(data, pos)?;
    if first == 0 {
        return Err(ContainerError::InvalidEbml("invalid EBML element ID"));
    }
    let mut length = 1usize;
    let mut mask = 0x80u8;
    while length <= 4 && (first & mask) == 0 {
        length += 1;
        mask >>= 1;
    }
    let mut value = 0u64;
    for index in 0..length {
        value = (value << 8) | byte_at(data, pos + index)? as u64;
    }
    Ok((value, pos + length))
}

pub fn parse_ebml_uint(data: &[u8], pos: usize, size: usize) -> Result<u64> {
    let mut value = 0u64;
    for index in 0..size {
        value = (value << 8) | byte_at(data, pos + index)? as u64;
    }
    Ok(value)
}

const EBML_ID_SEGMENT: u64 = 0x1853_8067;
const EBML_ID_CUES: u64 = 0x1C53_BB6B;
const EBML_ID_CUE_POINT: u64 = 0xBB;
const EBML_ID_CUE_TIME: u64 = 0xB3;
const EBML_ID_CUE_TRACK_POSITIONS: u64 = 0xB7;
const EBML_ID_CUE_CLUSTER_POSITION: u64 = 0xF1;

const EBML_MASTER_IDS: [u64; 4] = [
    EBML_ID_SEGMENT,
    EBML_ID_CUES,
    EBML_ID_CUE_POINT,
    EBML_ID_CUE_TRACK_POSITIONS,
];

/// 返回 `(Segment 数据起点, [(cue 时间毫秒, cluster 相对位置)])`。
pub fn parse_webm_cues(data: &[u8]) -> Result<(u64, Vec<(u64, u64)>)> {
    let mut pos = 0usize;

    let (_, next_pos) = parse_ebml_element_id(data, pos)?;
    pos = next_pos;
    let (element_size, next_pos) = parse_ebml_var_int(data, pos)?;
    pos = next_pos + element_size as usize;

    let (element_id, next_pos) = parse_ebml_element_id(data, pos)?;
    if element_id != EBML_ID_SEGMENT {
        return Err(ContainerError::ExpectedSegment);
    }
    pos = next_pos;
    let (_, next_pos) = parse_ebml_var_int(data, pos)?;
    let segment_data_offset = next_pos as u64;
    pos = next_pos;

    let mut cues = Vec::new();
    let limit = data.len().min(segment_data_offset as usize + 65536);

    while pos < limit {
        let Ok((element_id, next_pos)) = parse_ebml_element_id(data, pos) else {
            break;
        };
        let Ok((element_size, next_pos)) = parse_ebml_var_int(data, next_pos) else {
            break;
        };

        if element_id == EBML_ID_CUES {
            cues = parse_cues_element(data, next_pos, next_pos + element_size as usize)?;
            break;
        } else if EBML_MASTER_IDS.contains(&element_id) {
            pos = next_pos;
        } else {
            pos = next_pos + element_size as usize;
        }
    }

    Ok((segment_data_offset, cues))
}

fn parse_cues_element(data: &[u8], start: usize, end: usize) -> Result<Vec<(u64, u64)>> {
    let mut cues = Vec::new();
    let mut pos = start;
    while pos < end {
        let Ok((element_id, next_pos)) = parse_ebml_element_id(data, pos) else {
            break;
        };
        let Ok((element_size, next_pos)) = parse_ebml_var_int(data, next_pos) else {
            break;
        };
        if element_id == EBML_ID_CUE_POINT
            && let Ok((cue_time, cue_pos)) =
                parse_cue_point(data, next_pos, next_pos + element_size as usize)
        {
            cues.push((cue_time, cue_pos));
        }
        pos = next_pos + element_size as usize;
    }
    Ok(cues)
}

fn parse_cue_point(data: &[u8], start: usize, end: usize) -> Result<(u64, u64)> {
    let mut cue_time = 0u64;
    let mut cluster_pos = 0u64;
    let mut pos = start;
    while pos < end {
        let Ok((element_id, next_pos)) = parse_ebml_element_id(data, pos) else {
            break;
        };
        let Ok((element_size, next_pos)) = parse_ebml_var_int(data, next_pos) else {
            break;
        };
        if element_id == EBML_ID_CUE_TIME {
            cue_time = parse_ebml_uint(data, next_pos, element_size as usize)?;
        } else if element_id == EBML_ID_CUE_TRACK_POSITIONS
            && let Ok(position) =
                parse_cue_track_positions(data, next_pos, next_pos + element_size as usize)
        {
            cluster_pos = position;
        }
        pos = next_pos + element_size as usize;
    }
    Ok((cue_time, cluster_pos))
}

fn parse_cue_track_positions(data: &[u8], start: usize, end: usize) -> Result<u64> {
    let mut pos = start;
    while pos < end {
        let Ok((element_id, next_pos)) = parse_ebml_element_id(data, pos) else {
            break;
        };
        let Ok((element_size, next_pos)) = parse_ebml_var_int(data, next_pos) else {
            break;
        };
        if element_id == EBML_ID_CUE_CLUSTER_POSITION {
            return parse_ebml_uint(data, next_pos, element_size as usize);
        }
        pos = next_pos + element_size as usize;
    }
    Err(ContainerError::InvalidEbml("no cluster position"))
}

/// WebM 时间段：`headerData[:segStart]` 加 cue 点推出的字节区间。
///
/// Python 侧在 cues 为空时会 `IndexError`，这里返回 `NoCues`。
pub fn build_webm_segment_range(
    header_data: &[u8],
    start_time: f64,
    end_time: f64,
) -> Result<SegmentRange> {
    let (segment_data_offset, cues) = parse_webm_cues(header_data)?;
    let Some(last_cue) = cues.last() else {
        return Err(ContainerError::NoCues);
    };

    let start_ms = start_time * 1000.0;
    let end_ms = end_time * 1000.0;

    let mut seg_start = segment_data_offset + last_cue.1;
    let mut seg_end = seg_start;
    let mut seg_start_time = 0.0;
    let mut seg_end_time = 0.0;

    for (index, (cue_time, cue_pos)) in cues.iter().enumerate() {
        let abs_pos = segment_data_offset + cue_pos;
        if (*cue_time as f64) <= start_ms {
            seg_start = abs_pos;
            seg_start_time = *cue_time as f64 / 1000.0;
        }
        if (*cue_time as f64) <= end_ms {
            let next = cues.get(index + 1);
            seg_end = next.map_or(abs_pos, |(_, position)| segment_data_offset + position);
            seg_end_time =
                next.map_or(*cue_time as f64 / 1000.0, |(time, _)| *time as f64 / 1000.0);
        }
    }

    if seg_end <= seg_start {
        seg_end = header_data.len() as u64;
    }

    let header_size = seg_start as usize;
    let seg_duration = if seg_end_time > seg_start_time {
        seg_end_time - seg_start_time
    } else {
        0.0
    };

    Ok(SegmentRange {
        patched_header: header_data
            .get(..header_size)
            .ok_or(ContainerError::UnexpectedEnd)?
            .to_vec(),
        seg_start,
        seg_end,
        seg_start_time,
        seg_duration,
    })
}
