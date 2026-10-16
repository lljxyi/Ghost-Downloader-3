//! `.ghd` 分片进度记录：镜像 `features/http_pack/task.py` 的 `_loadRecord` / `_supervise`。
//!
//! 每个 subworker 24 字节 `<QQQ`（start、position、end），顺序即 subworker 顺序，
//! 与最终文件同名同目录（`{最终路径}.ghd`）。运行中每秒整表重写（seek 0 + truncate），
//! 所以进程被杀可能留下不足 24 字节的尾巴。
//!
//! Python 侧遇到这种尾巴会丢弃全部记录、从零重下；这里返回错误，由调用方决定是重下还是抢救前 N 条。

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

pub const RECORD_SIZE: usize = 24;

/// 一个 subworker 负责的字节区间与已写到的位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartRecord {
    pub start: u64,
    pub position: u64,
    pub end: u64,
}

impl PartRecord {
    pub fn received_bytes(self) -> u64 {
        self.position.saturating_sub(self.start)
    }
}

pub fn load_records(path: &Path) -> io::Result<Vec<PartRecord>> {
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };

    if data.len() % RECORD_SIZE != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} 长度不是 {} 的整数倍", path.display(), RECORD_SIZE),
        ));
    }

    let mut records = Vec::with_capacity(data.len() / RECORD_SIZE);
    for chunk in data.chunks_exact(RECORD_SIZE) {
        records.push(PartRecord {
            start: u64::from_le_bytes(chunk[0..8].try_into().expect("8 字节")),
            position: u64::from_le_bytes(chunk[8..16].try_into().expect("8 字节")),
            end: u64::from_le_bytes(chunk[16..24].try_into().expect("8 字节")),
        });
    }
    Ok(records)
}

/// 整表覆盖写：与 Python 的 `open(..., "wb")` + `truncate()` 语义一致。
pub fn save_records(path: &Path, records: &[PartRecord]) -> io::Result<()> {
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder)?;
    }

    let mut buffer = Vec::with_capacity(records.len() * RECORD_SIZE);
    for record in records {
        buffer.extend_from_slice(&record.start.to_le_bytes());
        buffer.extend_from_slice(&record.position.to_le_bytes());
        buffer.extend_from_slice(&record.end.to_le_bytes());
    }

    let mut file = File::create(path)?;
    file.write_all(&buffer)?;
    file.flush()
}
