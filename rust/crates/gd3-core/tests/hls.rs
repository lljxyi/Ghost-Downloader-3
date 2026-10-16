//! m3u8 parse 扫描面（`m3u8.loads` 的观测面）与 Python 的逐条对比。
//!
//! 金文件 `tests/fixtures/hls.jsonl` 由 `tests/fixtures/generate_hls_fixture.py`
//! 用项目 venv 的 `m3u8` 库（`m3u8.loads`）生成；Rust 侧只读。

use gd3_core::hls::probe;
use serde_json::Value;

fn fixture() -> Vec<(String, Value)> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hls.jsonl");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读金文件失败 {}：{error}", path.display()));
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let value: Value = serde_json::from_str(line).expect("金文件行不是合法 JSON");
            let name = value["name"].as_str().expect("name").to_string();
            (name, value)
        })
        .collect()
}

fn variant_json(variants: &[gd3_core::hls::HlsVariant]) -> Vec<Value> {
    variants
        .iter()
        .map(|variant| {
            serde_json::json!({
                "uri": variant.uri,
                "width": variant.width,
                "height": variant.height,
                "codecs": variant.codecs,
                "frame_rate": variant.frame_rate,
            })
        })
        .collect()
}

fn variants_of(case: &Value) -> Vec<Value> {
    let Some(variants) = case.get("variants") else {
        return Vec::new();
    };
    variants.as_array().expect("variants 是数组").clone()
}

#[test]
fn probe_matches_m3u8_loads() {
    let cases = fixture();
    let mut checked_ok = 0;
    let mut checked_error = 0;
    let mut mismatches: Vec<String> = Vec::new();
    for (name, case) in &cases {
        let body = case["body"].as_str().expect("body");
        let actual = probe(body);
        if let Some(expected_error) = case.get("error") {
            let expected = expected_error.as_str().expect("error 是字符串");
            match actual {
                Err(error) => {
                    if format!("{}: {}", error.kind, error.message) != expected {
                        let found = format!("{}: {}", error.kind, error.message);
                        mismatches.push(format!(
                            "{name}: 错误文本 Rust={found:?} Python={expected:?}"
                        ));
                    }
                }
                Ok(_) => mismatches.push(format!("{name}: Python 报 {expected:?}，Rust 却成功")),
            }
            checked_error += 1;
            continue;
        }

        let mut problems: Vec<String> = Vec::new();
        let probe = match actual {
            Ok(probe) => probe,
            Err(error) => {
                mismatches.push(format!(
                    "{name}: Python 成功，Rust 却报 {}: {}",
                    error.kind, error.message
                ));
                continue;
            }
        };
        let expected = case["is_endlist"].as_bool().expect("is_endlist");
        if probe.is_endlist != expected {
            problems.push(format!(
                "is_endlist Rust={} Python={}",
                probe.is_endlist, expected
            ));
        }
        let expected = case["segment_count"].as_u64().expect("segment_count");
        if probe.segment_count != expected {
            problems.push(format!(
                "segment_count Rust={} Python={}",
                probe.segment_count, expected
            ));
        }
        let expected_variants = variants_of(case);
        let actual_variants = variant_json(&probe.variants);
        if actual_variants != expected_variants {
            problems.push(format!(
                "variants Rust={actual_variants:?} Python={expected_variants:?}"
            ));
        }
        if !problems.is_empty() {
            mismatches.push(format!("{name}: {}", problems.join("; ")));
        }
        checked_ok += 1;
    }

    for mismatch in &mismatches {
        eprintln!("MISMATCH {mismatch}");
    }
    assert!(mismatches.is_empty(), "{} 条不一致", mismatches.len());
    assert!(checked_ok >= 40, "金文件里的成功用例太少：{checked_ok}");
    assert!(
        checked_error >= 20,
        "金文件里的失败用例太少：{checked_error}"
    );
    assert_eq!(checked_ok + checked_error, cases.len());
}
