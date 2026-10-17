//! 任务领域模型：镜像 `app/models/task.py` 的持久化形态。
//!
//! 磁盘上的 `tasks.jsonl` 由 Python 实现写出，两侧要能互相读写，所以字段顺序、
//! `type` 标签（FeaturePack 子类）与未知字段都按原样保留。
//! Rust 侧字段用 snake_case，`rename_all = "camelCase"` 负责还原成 Python 的键名。

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// 任务状态。持久化为枚举名（Python 侧 `TaskStatus.name`）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum TaskStatus {
    #[default]
    Waiting,
    Running,
    Paused,
    Completed,
    Failed,
}

/// 进度百分比。
///
/// Python 侧 `progress` 可能是 int（`0`、`100`）也可能是 float（`42.5`），两者在
/// JSON 里写法不同。要逐字节对齐旧文件，就得记住数字的原始类型。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Progress {
    Int(i64),
    Float(f64),
}

impl Progress {
    pub fn percent(self) -> f64 {
        match self {
            Progress::Int(value) => value as f64,
            Progress::Float(value) => value,
        }
    }

    pub fn from_percent(value: f64) -> Self {
        if value.fract() == 0.0 {
            Progress::Int(value as i64)
        } else {
            Progress::Float(value)
        }
    }
}

impl Default for Progress {
    fn default() -> Self {
        Progress::Int(0)
    }
}

impl Serialize for Progress {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Progress::Int(value) => serializer.serialize_i64(*value),
            Progress::Float(value) => serializer.serialize_f64(*value),
        }
    }
}

impl<'de> Deserialize<'de> for Progress {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ProgressVisitor;

        impl<'de> Visitor<'de> for ProgressVisitor {
            type Value = Progress;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("进度数字")
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Progress, E> {
                Ok(Progress::Int(value))
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Progress, E> {
                Ok(Progress::Int(value as i64))
            }

            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Progress, E> {
                Ok(Progress::Float(value))
            }
        }

        deserializer.deserialize_any(ProgressVisitor)
    }
}

/// 多文件任务里的一个可选下载单元。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskFile {
    pub index: i64,
    pub relative_path: String,
    #[serde(default)]
    pub size: i64,
    #[serde(default = "is_selected")]
    pub selected: bool,
    #[serde(default)]
    pub downloaded_bytes: i64,
    #[serde(default)]
    pub completed: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 任务内的一个可执行步骤。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStep {
    pub step_index: i64,
    #[serde(default)]
    pub file_index: Option<i64>,
    #[serde(default = "new_step_id")]
    pub step_id: String,
    #[serde(default)]
    pub status: TaskStatus,
    #[serde(default)]
    pub progress: Progress,
    #[serde(default)]
    pub received_bytes: i64,
    #[serde(default)]
    pub speed: i64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
}

/// 一个 Task Record。字段顺序与 `app/models/task.py` 的 dataclass 顺序一致。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRecord {
    pub name: String,
    pub url: String,
    pub pack_id: String,
    #[serde(default = "new_task_id")]
    pub task_id: String,
    #[serde(default)]
    pub status: TaskStatus,
    #[serde(default)]
    pub steps: Vec<TaskStep>,
    #[serde(default = "now_seconds")]
    pub created_at: i64,
    #[serde(default)]
    pub completed_at: i64,
    #[serde(default)]
    pub output_folder: String,
    #[serde(default)]
    pub file_size: i64,
    #[serde(default)]
    pub files: Option<Vec<TaskFile>>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub should_seed: bool,
    #[serde(default)]
    pub checksums: Map<String, Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
}

/// `type` 标签的最后一段。
///
/// Python `_typeByName` 用全名查登记表，查不到时按 `cls.__name__` 兜底
/// （`app/models/serialization.py`），等价于「只认最后一段」。
pub fn short_type_name(type_name: &str) -> &str {
    type_name.rsplit('.').next().unwrap_or(type_name)
}

fn is_selected() -> bool {
    true
}

fn new_task_id() -> String {
    format!("tsk_{}", Uuid::new_v4().simple())
}

fn new_step_id() -> String {
    format!("stp_{}", Uuid::new_v4().simple())
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or_default()
}

/// 状态归一：镜像 `Task.updateStatus` 及其前置步骤。
///
/// Python 侧在 `__post_init__`（含 `fromDict`）里就会调用它，所以加载旧记录会改写 Task 状态。
/// Rust 把「读盘」与「归一」分成两步：`load_records` 只忠实回写，需要 Python 语义的调用方
/// 在加载后显式调用 `update_status`。
impl TaskRecord {
    /// 未被选中的 Selectable File 对应的 Step 不参与状态推导。
    pub fn is_step_selected(&self, step: &TaskStep) -> bool {
        let Some(files) = self.files.as_ref().filter(|files| !files.is_empty()) else {
            return true;
        };
        let Some(file_index) = step.file_index else {
            return true;
        };
        files
            .iter()
            .find(|file| file.index == file_index)
            .is_some_and(|file| file.selected)
    }

    /// `Task.isOutputFolder`：`len(files or []) > 1`。
    pub fn is_output_folder(&self) -> bool {
        self.files.as_ref().is_some_and(|files| files.len() > 1)
    }

    /// `Task.outputPath`：`str(Path(outputFolder) / name)`。
    ///
    /// 走 `path::posix_join` 而不是 `Path::join`——`pathlib` 会丢空组件与
    /// `.`、保留恰好两个前导斜杠，`Path::join` 不会。
    ///
    /// `BilibiliTask` **覆盖**了它：多文件（`isOutputFolder`）时取
    /// `outputFolder / Path(name).stem`——分集任务下载到以任务名（去掉扩展名）
    /// 命名的目录里。
    pub fn output_path(&self) -> String {
        if self.is_output_folder() && self.short_type_name() == Some("BilibiliTask") {
            return crate::path::posix_join(
                &self.output_folder,
                crate::path::posix_stem(&self.name),
            );
        }
        crate::path::posix_join(&self.output_folder, &self.name)
    }

    /// `type` 标签的最后一段（旧文件可能只存裸类名）。
    pub fn short_type_name(&self) -> Option<&str> {
        self.type_name.as_deref().map(short_type_name)
    }

    /// 把 Step 的字节数与完成情况汇总回 TaskFile。
    pub fn update_files_from_steps(&mut self) {
        if self.files.as_ref().is_none_or(|files| files.is_empty()) {
            return;
        }

        let mut received: HashMap<i64, i64> = HashMap::new();
        let mut completed: HashMap<i64, bool> = HashMap::new();
        for step in &self.steps {
            let Some(file_index) = step.file_index else {
                continue;
            };
            *received.entry(file_index).or_insert(0) += step.received_bytes;
            let entry = completed.entry(file_index).or_insert(true);
            *entry = *entry && step.status == TaskStatus::Completed;
        }

        if let Some(files) = self.files.as_mut() {
            for file in files {
                if let Some(bytes) = received.get(&file.index) {
                    file.downloaded_bytes = *bytes;
                    file.completed = completed.get(&file.index).copied().unwrap_or(false);
                }
            }
        }
    }

    /// `(平均进度, 总速度, 总字节数)`，只统计选中文件对应的 Step。
    pub fn current_snapshot(&self) -> (f64, i64, i64) {
        let selected_files = self.files.as_ref().filter(|files| !files.is_empty());
        let steps: Vec<&TaskStep> = match selected_files {
            None => self.steps.iter().collect(),
            Some(files) => {
                let selected: Vec<i64> = files
                    .iter()
                    .filter(|file| file.selected)
                    .map(|file| file.index)
                    .collect();
                self.steps
                    .iter()
                    .filter(|step| {
                        step.file_index
                            .is_none_or(|file_index| selected.contains(&file_index))
                    })
                    .collect()
            }
        };

        if steps.is_empty() {
            return (0.0, 0, 0);
        }
        let progress: f64 = steps.iter().map(|step| step.progress.percent()).sum();
        let speed: i64 = steps.iter().map(|step| step.speed).sum();
        let received_bytes: i64 = steps.iter().map(|step| step.received_bytes).sum();
        (progress / steps.len() as f64, speed, received_bytes)
    }

    /// 从 Step 状态推出 Task 状态。
    pub fn update_status(&mut self) -> TaskStatus {
        self.update_files_from_steps();

        let statuses: Vec<TaskStatus> = self
            .steps
            .iter()
            .filter(|step| self.is_step_selected(step))
            .map(|step| step.status)
            .collect();

        if statuses.is_empty() {
            return self.status;
        }

        if statuses.contains(&TaskStatus::Failed) {
            self.status = TaskStatus::Failed;
        } else if statuses
            .iter()
            .all(|status| *status == TaskStatus::Completed)
        {
            self.status = TaskStatus::Completed;
            if self.completed_at == 0 {
                self.completed_at = now_seconds();
            }
            if self.file_size <= 0 {
                let (_, _, received_bytes) = self.current_snapshot();
                if received_bytes > 0 {
                    self.file_size = received_bytes;
                }
            }
        } else if statuses.contains(&TaskStatus::Running) {
            self.status = TaskStatus::Running;
        } else if statuses.iter().all(|status| *status == TaskStatus::Paused) {
            self.status = TaskStatus::Paused;
        } else {
            self.status = TaskStatus::Waiting;
        }
        self.status
    }

    /// 把选中 Step 推到目标状态后再归一（`Task.setStatus`）。
    pub fn set_status(&mut self, status: TaskStatus) -> TaskStatus {
        let selected: Vec<usize> = (0..self.steps.len())
            .filter(|index| self.is_step_selected(&self.steps[*index]))
            .collect();

        if selected.is_empty() {
            self.status = status;
            return self.status;
        }

        for index in selected {
            let step = &mut self.steps[index];
            if step.status == TaskStatus::Completed {
                continue;
            }
            if status == TaskStatus::Running && step.status == TaskStatus::Failed {
                step.status = TaskStatus::Waiting;
                step.progress = Progress::Int(0);
                step.received_bytes = 0;
                step.speed = 0;
            }
            step.status = status;
            step.speed = 0;
            if status == TaskStatus::Completed {
                step.progress = Progress::Int(100);
            }
        }
        self.update_status()
    }

    /// 重新下载：清空进度与校验值，保留 Name 与 Output Folder。
    pub fn reset(&mut self) -> TaskStatus {
        self.completed_at = 0;
        self.should_seed = true;
        self.checksums.clear();

        if self.steps.is_empty() {
            self.status = TaskStatus::Waiting;
            return self.status;
        }

        for step in &mut self.steps {
            step.status = TaskStatus::Waiting;
            step.progress = Progress::Int(0);
            step.received_bytes = 0;
            step.speed = 0;
        }
        self.update_status()
    }
}
