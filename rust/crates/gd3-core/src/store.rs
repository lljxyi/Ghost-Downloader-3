//! `tasks.jsonl` 读写：镜像 `app/services/task_service.py` 的 TaskStore。
//!
//! 一行一个 Task Record，UTF-8，写盘走 `.tmp` + 替换，读盘时坏行跳过并记录原因。

use crate::task::TaskRecord;
use serde::Serialize;
use serde_json::ser::{Formatter, Serializer};
use std::fs;
use std::io::{self, Write};
use std::path::Path;

/// Python `json.dumps` 默认分隔符是 `", "` 与 `": "`，serde_json 默认不带空格。
pub struct PythonFormatter;

impl Formatter for PythonFormatter {
    fn begin_array_value<W>(&mut self, writer: &mut W, first: bool) -> io::Result<()>
    where
        W: ?Sized + Write,
    {
        if first {
            Ok(())
        } else {
            writer.write_all(b", ")
        }
    }

    fn begin_object_key<W>(&mut self, writer: &mut W, first: bool) -> io::Result<()>
    where
        W: ?Sized + Write,
    {
        if first {
            Ok(())
        } else {
            writer.write_all(b", ")
        }
    }

    fn begin_object_value<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: ?Sized + Write,
    {
        writer.write_all(b": ")
    }
}

/// 序列化为一行（不含换行符）。
pub fn to_line(record: &TaskRecord) -> serde_json::Result<String> {
    let mut bytes = Vec::new();
    let mut serializer = Serializer::with_formatter(&mut bytes, PythonFormatter);
    record.serialize(&mut serializer)?;
    Ok(String::from_utf8(bytes).expect("serde_json 只产出 UTF-8"))
}

pub struct LoadOutcome {
    pub records: Vec<TaskRecord>,
    /// 逐行解析失败的原因，顺序与文件一致。Python 侧只记日志后跳过该行。
    pub failures: Vec<String>,
}

pub fn load_records(path: &Path) -> io::Result<LoadOutcome> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(LoadOutcome {
                records: Vec::new(),
                failures: Vec::new(),
            });
        }
        Err(error) => return Err(error),
    };

    let mut outcome = LoadOutcome {
        records: Vec::new(),
        failures: Vec::new(),
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<TaskRecord>(line) {
            Ok(record) => outcome.records.push(record),
            Err(error) => outcome.failures.push(error.to_string()),
        }
    }
    Ok(outcome)
}

/// Python 用文本模式 `open(..., "w")` 写盘，Windows 上 `\n` 会被翻译成 `\r\n`。
/// 要逐字节对齐两侧文件，就得跟随同一套换行规则。
const LINE_ENDING: &str = if cfg!(windows) { "\r\n" } else { "\n" };

pub fn save_records(path: &Path, records: &[TaskRecord]) -> io::Result<()> {
    let mut lines = String::new();
    for record in records {
        let line = to_line(record).map_err(io::Error::other)?;
        lines.push_str(&line);
        lines.push_str(LINE_ENDING);
    }

    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder)?;
    }

    let Some(file_name) = path.file_name() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "tasks.jsonl 路径缺少文件名",
        ));
    };
    let temp_path = path.with_file_name(format!("{}.tmp", file_name.to_string_lossy()));

    fs::write(&temp_path, lines.as_bytes())?;
    fs::rename(&temp_path, path)?;
    Ok(())
}
