//! `.ghd` 记录读写：格式与 `features/http_pack/task.py` 一致（24 字节 `<QQQ`）。

use gd3_core::ghd::{PartRecord, RECORD_SIZE, load_records, save_records};

fn temp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(name)
}

#[test]
fn round_trips_records() {
    let path = temp_path("gd3-test-part.ghd");
    let records = vec![
        PartRecord {
            start: 0,
            position: 1024,
            end: 4095,
        },
        PartRecord {
            start: 4096,
            position: 4096,
            end: 8191,
        },
    ];

    save_records(&path, &records).expect("写入失败");
    let loaded = load_records(&path).expect("读取失败");

    assert_eq!(loaded, records);
    assert_eq!(loaded[0].received_bytes(), 1024);
    assert_eq!(loaded[1].received_bytes(), 0);
}

#[test]
fn matches_python_struct_layout() {
    // 对应 Python 的 pack("<QQQ", 0, 1024, 4095)
    let path = temp_path("gd3-test-part-layout.ghd");
    save_records(
        &path,
        &[PartRecord {
            start: 0,
            position: 1024,
            end: 4095,
        }],
    )
    .expect("写入失败");

    let bytes = std::fs::read(&path).expect("读回失败");
    assert_eq!(bytes.len(), RECORD_SIZE);
    assert_eq!(&bytes[0..8], &0u64.to_le_bytes());
    assert_eq!(&bytes[8..16], &1024u64.to_le_bytes());
    assert_eq!(&bytes[16..24], &4095u64.to_le_bytes());
}

#[test]
fn overwrite_truncates_old_records() {
    let path = temp_path("gd3-test-part-truncate.ghd");
    save_records(
        &path,
        &[
            PartRecord {
                start: 0,
                position: 1,
                end: 1,
            },
            PartRecord {
                start: 2,
                position: 2,
                end: 3,
            },
        ],
    )
    .expect("写入失败");

    save_records(
        &path,
        &[PartRecord {
            start: 0,
            position: 1,
            end: 1,
        }],
    )
    .expect("写入失败");
    assert_eq!(
        load_records(&path).expect("读取失败").len(),
        1,
        "旧记录必须被截掉"
    );
}

#[test]
fn missing_file_yields_no_records() {
    let path = temp_path("gd3-test-part-absent.ghd");
    let _ = std::fs::remove_file(&path);
    assert!(load_records(&path).expect("读取失败").is_empty());
}

#[test]
fn truncated_tail_is_an_error() {
    // Python 侧把这种尾巴当成"没有进度"直接重下；Rust 返回错误交给调用方决定。
    let path = temp_path("gd3-test-part-truncated.ghd");
    std::fs::write(&path, [0u8; 30]).expect("写入失败");
    assert!(load_records(&path).is_err());
}
