#!/usr/bin/env python3
"""生成 `hls.jsonl`（m3u8 parse 的金文件）。

用法（需要能 import `m3u8` 库的解释器，即项目 venv）：

    .venv/bin/python rust/crates/gd3-core/tests/fixtures/generate_hls_fixture.py

每行一个 JSON 对象：

- Python `m3u8.loads(body)` 抛异常 → `{"name", "body", "error":
  "类型: 消息"}`（`f"{type(e).__name__}: {e}"` 的原文）；
- 否则 `{"name", "body", "is_endlist", "segment_count",
  "variants": [{"uri", "width", "height", "codecs", "frame_rate"}, ...]}`。

Rust 侧 `tests/hls.rs` 逐条比对 `gd3_core::hls::probe`。
"""

import json
from pathlib import Path

import m3u8

CASES = {
    # -- 媒体清单 --
    "vod_basic": "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXTINF:5.5,\ntitle\nseg1.ts\n#EXT-X-ENDLIST\n",
    "live_no_endlist": "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXTINF:1,\na.ts\n#EXTINF:2,\nb.ts\n",
    "trailing_extinf": "#EXTM3U\n#EXTINF:5.5,\nseg1.ts\n#EXTINF:3,\n",
    "trailing_byterange": "#EXTM3U\n#EXT-X-BYTERANGE:4500@0\n",
    "byterange_with_uri": "#EXTM3U\n#EXT-X-BYTERANGE:4500@0\nseg1.ts\n",
    "empty_body": "",
    "only_text": "seg1.ts\nseg2.ts\n",
    "comments_only": "# just a comment\n# another\n",
    "blank_after_extinf": "#EXTM3U\n#EXTINF:1,\n   \na.ts\n",
    "blank_between": "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\n\nv.m3u8\n",
    "crlf": "#EXTM3U\r\n#EXT-X-ENDLIST\r\n#EXTINF:1,\r\na.ts\r\n",
    "padded_body": "  #EXTM3U\n#EXTINF:1,\na.ts\n\n",
    "stream_inf_blank_crlf": "#EXTM3U\r\n\r\n#EXT-X-STREAM-INF:BANDWIDTH=1,RESOLUTION=16x16\r\n\r\nv.m3u8\r\n",
    # -- simple parameter（cast_to=int）--
    "media_sequence_underscore": "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:99_0\n",
    "media_sequence_bad": "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:1e2\n",
    "media_sequence_negative": "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:-5\n",
    "targetduration_bad": "#EXTM3U\n#EXT-X-TARGETDURATION:5.5\n",
    "targetduration_underscore": "#EXTM3U\n#EXT-X-TARGETDURATION:6_0\n",
    "disc_seq_bad": "#EXTM3U\n#EXT-X-DISCONTINUITY-SEQUENCE:02x\n",
    "bitrate_bad": "#EXTM3U\n#EXT-X-BITRATE:abc\n",
    "version_underscore": "#EXTM3U\n#EXT-X-VERSION:1_2\n",
    "version_no_colon": "#EXTM3U\n#EXT-X-VERSION\n",
    # -- simple parameter（cast_to=str）--
    "allow_cache_bad": "#EXTM3U\n#EXT-X-ALLOW-CACHE:no\n",
    "allow_cache_blank_value": "#EXTM3U\n#EXT-X-ALLOW-CACHE:\n",
    "allow_cache_no_colon": "#EXTM3U\n#EXT-X-ALLOW-CACHE\n",
    "playlist_type_no_colon": "#EXTM3U\n#EXT-X-PLAYLIST-TYPE\n",
    # -- 静默类 tag --
    "discontinuity_cue": "#EXTM3U\n#EXT-X-DISCONTINUITY\n#EXTINF:1,\na.ts\n",
    "media_tag_silent": "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",NAME=\"Track\"\n#EXTINF:1,\na.ts\n",
    "unknown_tags": "#EXTM3U\n#EXT-X-DATERANGE:ID=x\n#FOO:bar\n#EXTINF:1,\na.ts\n",
    "images_only": "#EXTM3U\n#EXT-X-IMAGES-ONLY\n#EXT-X-ENDLIST\n",
    "tile_tags": "#EXTM3U\n#EXT-X-TILES:RESOLUTION=320x180,LAYOUT=5x5,DURATION=6.006\n#EXTINF:1,\na.ts\n",
    # -- 密钥 --
    "key_basic": "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"x.key\",IV=0xdead\n#EXTINF:1,\na.ts\n",
    "key_duplicates": "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"k1\"\n#EXT-X-KEY:METHOD=AES-128,URI=\"k1\"\n",
    "key_missing_method": "#EXTM3U\n#EXT-X-KEY:URI=\"x.key\"\n#EXTINF:1,\na.ts\n",
    "key_missing_equals": "#EXTM3U\n#EXT-X-KEY:METHODAES\n",
    "key_empty_params": "#EXTM3U\n#EXT-X-KEY:\n",
    "key_weird_name": "#EXTM3U\n#EXT-X-KEY:=\n",
    "key_next_segment": "#EXTM3U\n#EXT-X-KEY:METHOD=NONE\n#EXT-X-KEY:URI=\"x.key\"\n#EXTINF:1,\nfirstseg.ts\n",
    "key_method_none_after_key": "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"k\"\n#EXTINF:1,\nfirstseg.ts\n#EXT-X-KEY:METHOD=NONE\n#EXTINF:2,\nsecondseg\n",
    # -- EXTINF --
    "extinf_no_comma": "#EXTM3U\n#EXTINF:5.5\na.ts\n",
    "extinf_empty_duration": "#EXTM3U\n#EXTINF:,t\na.ts\n",
    "extinf_negative": "#EXTM3U\n#EXT-X-ENDLIST\n#EXTINF:-1.5,\na.ts\n",
    # -- 变体清单（模型层 resolution cast）--
    "master_basic": "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1280000,RESOLUTION=1920x1080,CODECS=\"avc1.4d401f\",FRAME-RATE=29.97\nmain.m3u8\n",
    "master_multi": "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1,RESOLUTION=640x360\nlow.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=2,RESOLUTION=1920x1080,CODECS=\"avc1\"\nhi.m3u8\n",
    "master_resolution_quoted": "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1,RESOLUTION=\"1920x1080\"\nv.m3u8\n",
    "master_resolution_float": "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1,RESOLUTION=3.0\nv.m3u8\n",
    "master_resolution_no_x": "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1,RESOLUTION=1920\nv.m3u8\n",
    "master_resolution_negative": "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1,RESOLUTION=-1920x1080\nv.m3u8\n",
    "master_resolution_empty": "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1,RESOLUTION=\nv.m3u8\n",
    "master_resolution_three_x": "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1,RESOLUTION=1920x1080x4\nv.m3u8\n",
    "master_resolution_x": "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1,RESOLUTION=x\nv.m3u8\n",
    "master_resolution_unclosed_quote": "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1,RESOLUTION=\"1920x1080\nv.m3u8\n",
    "master_resolution_with_spaces": "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1,RESOLUTION= 1920 x 1080 \nv.m3u8\n",
    "master_missing_bandwidth": "#EXTM3U\n#EXT-X-STREAM-INF:RESOLUTION=1920x1080\nv.m3u8\n",
    "master_no_uri": "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\n#EXT-X-ENDLIST\n",
    "master_bandwidth_float": "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=100.5\nv.m3u8\n",
    "master_bandwidth_infinity": "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=inf\nv.m3u8\n",
    "master_bandwidth_nan": "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=nan\nv.m3u8\n",
    "master_bandwidth_underscore": "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1_024_000\nv.m3u8\n",
    "master_bandwidth_bad_underscore": "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=10__24\nv.m3u8\n",
    "master_bandwidth_exponent": "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1e2\nv.m3u8\n",
    "master_avg_bw_int": "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1,AVERAGE-BANDWIDTH=1024_000\nv.m3u8\n",
    "master_avg_bw_bad": "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1,AVERAGE-BANDWIDTH=1e\nv.m3u8\n",
    "master_program_id": "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1,PROGRAM-ID=1_0\nv.m3u8\n",
    "master_program_id_cast": "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1,PROGRAM-ID=1e2\nv.m3u8\n",
    "master_program_id_hex": "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1,PROGRAM-ID=0x10\nv.m3u8\n",
    "master_frame_rate_bad": "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1,FRAME-RATE=1e\nv.m3u8\n",
    "master_codecs_unclosed": "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1,CODECS=\"avc1\nv.m3u8\n",
    "master_codecs_mismatch": "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1,CODECS=\"avc1'\nv.m3u8\n",
    # -- I-FRAME / IMAGE --
    "iframe_basic": "#EXTM3U\n#EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=200,RESOLUTION=640x360,URI=\"i.m3u8\"\n",
    "iframe_bad_resolution_with_uri": "#EXTM3U\n#EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=1,RESOLUTION=3.0,URI=\"x\"\n",
    "iframe_missing_uri": "#EXTM3U\n#EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=9\n",
    "iframe_program_id_bad": "#EXTM3U\n#EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=9,PROGRAM-ID=1.0,URI=\"x\"\n",
    "image_basic": "#EXTM3U\n#EXT-X-IMAGE-STREAM-INF:BANDWIDTH=200,RESOLUTION=640x360,URI=\"i.jpg\"\n",
    "image_missing_uri": "#EXTM3U\n#EXT-X-IMAGE-STREAM-INF:BANDWIDTH=9\n",
}


def main() -> None:
    out_path = Path(__file__).with_name("hls.jsonl")
    lines = []
    for name, body in CASES.items():
        try:
            playlist = m3u8.loads(body)
        except Exception as exc:  # noqa: BLE001
            lines.append(
                {
                    "name": name,
                    "body": body,
                    "error": f"{type(exc).__name__}: {exc}",
                }
            )
            continue
        lines.append(
            {
                "name": name,
                "body": body,
                "is_endlist": playlist.is_endlist,
                "segment_count": len(playlist.segments),
                "variants": [
                    dijk(
                        variant.uri,
                        variant.stream_info.resolution,
                        variant.stream_info.codecs,
                        variant.stream_info.frame_rate,
                    )
                    for variant in variant_list(playlist)
                ],
            }
        )
    out_path.write_text(
        "".join(json.dumps(line, ensure_ascii=False) + "\n" for line in lines),
        encoding="utf-8",
    )
    errors = sum(1 for line in lines if "error" in line)
    print(f"{out_path}：{len(lines)} 条（error {errors} / ok {len(lines) - errors}）")


def dijk(uri, resolution, codecs, frame_rate):
    return {
        "uri": uri,
        "width": resolution[0] if resolution else None,
        "height": resolution[1] if resolution else None,
        "codecs": codecs,
        "frame_rate": frame_rate,
    }


def variant_list(playlist):
    return playlist.playlists if hasattr(playlist, "playlists") else []


if __name__ == "__main__":
    main()
