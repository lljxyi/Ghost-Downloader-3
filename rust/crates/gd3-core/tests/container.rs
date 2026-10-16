//! `app/container.py` 的移植规格：直接搬 `tests/test_container.py` 的断言，
//! 用同一个真实 YouTube 头部 fixture（itag 160，730s，6.7MB）。

use gd3_core::container::{
    ContainerError, build_mp4_segment_range, build_webm_segment_range, parse_mp4_boxes, parse_sidx,
    parse_webm_cues,
};
use std::path::PathBuf;

fn video_header() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .join("tests")
        .join("fixtures")
        .join("youtube_video_header.bin");
    std::fs::read(&path).unwrap_or_else(|error| panic!("读不到 {}：{error}", path.display()))
}

#[test]
fn finds_required_boxes_in_order() {
    let header = video_header();
    let boxes = parse_mp4_boxes(&header);
    let ftyp = boxes.get("ftyp").expect("缺 ftyp");
    let moov = boxes.get("moov").expect("缺 moov");
    let sidx = boxes.get("sidx").expect("缺 sidx");
    assert!(ftyp.0 < moov.0);
    assert!(moov.0 < sidx.0);
}

#[test]
fn sidx_covers_full_duration_and_filesize() {
    let header = video_header();
    let boxes = parse_mp4_boxes(&header);
    let (sidx_offset, sidx_size) = boxes["sidx"];
    let references = parse_sidx(&header, sidx_offset, sidx_size).expect("解析 sidx 失败");

    assert_eq!(references.len(), 146);
    let total_duration: f64 = references.iter().map(|(duration, _)| duration).sum();
    assert!(
        (total_duration - 730.93).abs() < 0.1,
        "总时长 {total_duration}"
    );

    let header_size = (sidx_offset + sidx_size) as u64;
    let total_bytes = header_size + references.iter().map(|(_, size)| size).sum::<u64>();
    assert_eq!(total_bytes, 6_744_153);
}

#[test]
fn segment_60_to_65_lands_on_reference_boundaries() {
    let header = video_header();
    let boxes = parse_mp4_boxes(&header);
    let (sidx_offset, sidx_size) = boxes["sidx"];
    let references = parse_sidx(&header, sidx_offset, sidx_size).expect("解析 sidx 失败");
    let header_size = (sidx_offset + sidx_size) as u64;

    let segment = build_mp4_segment_range(&header, 60.0, 65.0).expect("切片失败");
    assert!(segment.seg_start > 0);
    assert!(segment.seg_end > segment.seg_start);
    assert!(segment.seg_end - segment.seg_start < 200_000);

    // segStart / segEnd 必须落在 reference 边界上，且覆盖请求的时间段
    let mut byte_pos = header_size;
    let mut time_sec = 0.0;
    let mut seg_start_time = None;
    let mut seg_end_time = None;
    for (duration, size) in &references {
        if byte_pos == segment.seg_start {
            seg_start_time = Some(time_sec);
        }
        byte_pos += size;
        if byte_pos == segment.seg_end {
            seg_end_time = Some(time_sec + duration);
            break;
        }
        time_sec += duration;
    }
    let seg_start_time = seg_start_time.expect("segStart 不在 reference 边界上");
    let seg_end_time = seg_end_time.expect("segEnd 不在 reference 边界上");
    assert!(seg_start_time <= 60.0);
    assert!(seg_end_time >= 65.0);
}

#[test]
fn patched_header_is_ftyp_plus_moov() {
    let header = video_header();
    let segment = build_mp4_segment_range(&header, 60.0, 65.0).expect("切片失败");

    assert_eq!(&segment.patched_header[..4], &[0x00, 0x00, 0x00, 0x1c]); // ftyp size=28
    assert_eq!(&segment.patched_header[4..8], b"ftyp");

    let boxes = parse_mp4_boxes(&header);
    let (moov_offset, moov_size) = boxes["moov"];
    assert_eq!(segment.patched_header.len(), moov_offset + moov_size);
}

#[test]
fn full_video_returns_full_range() {
    let header = video_header();
    let segment = build_mp4_segment_range(&header, 0.0, 731.0).expect("切片失败");

    let boxes = parse_mp4_boxes(&header);
    let (sidx_offset, sidx_size) = boxes["sidx"];
    assert_eq!(segment.seg_start, (sidx_offset + sidx_size) as u64);
    assert_eq!(segment.seg_end, 6_744_153);
}

#[test]
fn first_and_last_five_seconds_are_small_slices() {
    let header = video_header();
    let boxes = parse_mp4_boxes(&header);
    let (sidx_offset, sidx_size) = boxes["sidx"];
    let data_start = (sidx_offset + sidx_size) as u64;

    let first = build_mp4_segment_range(&header, 0.0, 5.0).expect("切片失败");
    assert_eq!(first.seg_start, data_start);
    assert!(first.seg_end - first.seg_start < 100_000);

    let last = build_mp4_segment_range(&header, 726.0, 731.0).expect("切片失败");
    assert_eq!(last.seg_end, 6_744_153);
    assert!(last.seg_end - last.seg_start < 100_000);
}

#[test]
fn missing_sidx_is_an_error() {
    // 只有 ftyp 的头部：Python 侧抛 ValueError("sidx box not found")
    let header = [
        0x00, 0x00, 0x00, 0x10, b'f', b't', b'y', b'p', 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    assert_eq!(
        build_mp4_segment_range(&header, 0.0, 5.0),
        Err(ContainerError::SidxNotFound)
    );
}

// ── WebM：Python 侧没有测试，这里用合成 EBML 覆盖 ─────────────

/// 单字节长度形式的 EBML 元素（载荷 < 127 字节）。
fn element(id: &[u8], payload: &[u8]) -> Vec<u8> {
    assert!(payload.len() < 0x7F);
    let mut out = id.to_vec();
    out.push(0x80 | payload.len() as u8);
    out.extend_from_slice(payload);
    out
}

fn cue_point(time_ms: u64, cluster_pos: u64) -> Vec<u8> {
    let track_positions = element(&[0xF1], &cluster_pos.to_be_bytes());
    let mut payload = element(&[0xB3], &time_ms.to_be_bytes());
    payload.extend(element(&[0xB7], &track_positions));
    element(&[0xBB], &payload)
}

/// 返回 `(头部, segment 数据起点, [(cue 时间, cluster 位置)])`。
fn webm_header() -> (Vec<u8>, usize, Vec<(u64, u64)>) {
    let cues: Vec<u8> = [
        cue_point(0, 0),
        cue_point(1_000, 500),
        cue_point(2_000, 1_200),
    ]
    .concat();
    let cues_element = element(&[0x1C, 0x53, 0xBB, 0x6B], &cues);
    let segment = element(&[0x18, 0x53, 0x80, 0x67], &cues_element);
    let ebml_header = element(&[0x1A, 0x45, 0xDF, 0xA3], &[0, 0, 0, 0]);

    let mut header = ebml_header;
    let segment_data_offset = header.len() + 4 + 1; // Segment id + 单字节长度
    header.extend(segment);
    (
        header,
        segment_data_offset,
        vec![(0, 0), (1_000, 500), (2_000, 1_200)],
    )
}

#[test]
fn parses_webm_cues() {
    let (header, segment_data_offset, expected) = webm_header();
    let (offset, cues) = parse_webm_cues(&header).expect("解析 cues 失败");
    assert_eq!(offset, segment_data_offset as u64);
    assert_eq!(cues, expected);
}

#[test]
fn webm_slice_covers_requested_time() {
    let (header, segment_data_offset, _) = webm_header();
    let segment = build_webm_segment_range(&header, 0.0, 2.0).expect("切片失败");

    // cue 0ms 落在起点，cue 2000ms 决定终点
    assert_eq!(segment.seg_start, segment_data_offset as u64);
    assert_eq!(segment.seg_end, segment_data_offset as u64 + 1_200);
    assert_eq!(segment.seg_start_time, 0.0);
    assert_eq!(segment.seg_duration, 2.0);
    assert_eq!(segment.patched_header, header[..segment_data_offset]);
}

#[test]
fn webm_slice_without_cues_is_an_error() {
    let empty_cues = element(&[0x1C, 0x53, 0xBB, 0x6B], &[]);
    let segment = element(&[0x18, 0x53, 0x80, 0x67], &empty_cues);
    let mut header = element(&[0x1A, 0x45, 0xDF, 0xA3], &[0, 0, 0, 0]);
    header.extend(segment);

    assert_eq!(
        build_webm_segment_range(&header, 0.0, 2.0),
        Err(ContainerError::NoCues)
    );
}

// ── 与 Python 金文件逐字节对比 ────────────────────────────────

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

#[test]
fn patched_header_matches_python_bytes() {
    let header = video_header();
    let expected = std::fs::read(fixture_path("mp4_patched_header_60_65.bin"))
        .expect("读不到金文件，先跑 generate_container_fixture.py");

    let segment = build_mp4_segment_range(&header, 60.0, 65.0).expect("切片失败");
    assert_eq!(
        segment.patched_header, expected,
        "改写后的头部与 Python 不一致"
    );
}

#[test]
fn segments_match_python_golden_numbers() {
    let header = video_header();
    let golden_text = std::fs::read_to_string(fixture_path("mp4_segments.json"))
        .expect("读不到金文件，先跑 generate_container_fixture.py");
    let golden: serde_json::Value = serde_json::from_str(&golden_text).expect("解析金文件失败");

    let boxes = parse_mp4_boxes(&header);
    for (name, value) in golden["boxes"].as_object().expect("缺 boxes") {
        let expected = (
            value[0].as_u64().expect("offset") as usize,
            value[1].as_u64().expect("size") as usize,
        );
        assert_eq!(
            boxes.get(name).copied(),
            Some(expected),
            "box {name} 位置不一致"
        );
    }

    for segment in golden["segments"].as_array().expect("缺 segments") {
        let start = segment["start"].as_f64().expect("start");
        let end = segment["end"].as_f64().expect("end");
        let actual = build_mp4_segment_range(&header, start, end).expect("切片失败");

        assert_eq!(
            actual.seg_start,
            segment["segStart"].as_u64().expect("segStart"),
            "{start}-{end} segStart"
        );
        assert_eq!(
            actual.seg_end,
            segment["segEnd"].as_u64().expect("segEnd"),
            "{start}-{end} segEnd"
        );
        assert_eq!(
            actual.seg_start_time,
            segment["segStartTime"].as_f64().expect("segStartTime"),
            "{start}-{end} segStartTime"
        );
        assert_eq!(
            actual.seg_duration,
            segment["segDuration"].as_f64().expect("segDuration"),
            "{start}-{end} segDuration"
        );
        assert_eq!(
            actual.patched_header.len(),
            segment["patchedHeaderLength"].as_u64().expect("长度") as usize
        );
    }
}
