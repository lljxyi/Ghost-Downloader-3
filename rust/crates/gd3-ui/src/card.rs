//! 任务卡片派生层（`app/view/cards/task_cards.py` 镜像；第 50 步）。
//!
//! Python 侧卡片是「数据驱动的表」：`FieldSpec.formats` / `ButtonSpec.states`
//! 两张 `{status: callable}` 表 + `refresh()` 里那段
//! `formats.get(task.status) or formats.get(None)` 的分发。本模块把这套
//! 表与分发**摊平成函数**，逐条对齐 `task_cards.py`：
//!
//! - 信息行三段 `SPEED_FIELD` / `ETA_FIELD` / `SIZE_FIELD`（顺序即
//!   `TaskCard.infoFields`）：`speed` 与 `eta` 只在 RUNNING 出现，
//!   `size` 走 `{None: toSizeText}` 全状态出现、`None` 时隐藏；
//! - 状态行 `_refreshForStatus`（RUNNING 隐藏；COMPLETED 三分支
//!   「文件不存在」/「完成于 …」/「任务已经完成」；FAILED 走
//!   `toLocalizedError` 兜底「下载过程中发生错误，请稍后重试」；
//!   PAUSED / WAITING 固定文案）；
//! - 多文件卡片的 `MultiFileTaskCard._refreshForStatus` 覆盖：
//!   `files>1` 且非「文件不存在」且状态 ∈ {WAITING, COMPLETED} 时把状态行
//!   改成「{已选}/{总数} 个文件」；
//! - 进度条形态：`_createProgressBar` 的 `fileSize in {UNKNOWN(0),
//!   NOT_SUPPORTED(-1)}` → 不确定进度条；`_refreshForStatus` 的
//!   show/hide/setError/pause 三态；
//! - 按钮状态 `_refreshButtons`：`states.get(status) or states.get(None)`
//!   命中后 `icon=None` 表示「沿用 spec.icon」（toggle 的 PLAY）。
//!
//! 分层边界（与「兜底/IO/译文不在核心层」一致）：
//! - `output_exists`：`isExisting(outputPath)` 是文件系统调用，由调用方给；
//! - `completed_at_text`：Python 是 `datetime.fromtimestamp(...).strftime
//!   ("%Y-%m-%d %H:%M:%S")`（**本地时区**），格式化归调用方，本层只拼
//!   「完成于 {text}」模板；
//! - `is_seeding` / `can_seed` / `has_output_file` / `can_pause`：`Task` 的
//!   运行时属性与类属性，`tasks.jsonl` 里没有，由调用方给；
//! - `error_text` 只做 `toLocalizedError` 的「无译文层」部分：`translate
//!   (message) or message` 取 message，再 `format_map(params)` 替换
//!   `{key}`。缺参数时 Python 会抛 `KeyError`，本层保留字面 `{key}`
//!   （我方 `TaskError` 构造器恒带全参数，走不到这个岔口）。

use crate::format::{to_readable_size, to_readable_time};
use gd3_core::task::TaskStatus;
use gd3_core::task_error::TaskError;

/// 卡片渲染所需的运行时输入（`Task` 的运行时属性 + 文件系统事实 + 快照）。
#[derive(Clone, Debug)]
pub struct CardTask {
    pub status: TaskStatus,
    pub file_size: i64,
    /// 已按 `%Y-%m-%d %H:%M:%S` 本地时间格式化的完成时间；`None` = 无
    /// （Python `task.completedAt` 为 0）。
    pub completed_at_text: Option<String>,
    /// `Task.canPause`：有 RUNNING 步骤时取该步骤的 `canPause`，否则 True。
    pub can_pause: bool,
    /// `Task.canSeed`（类属性，默认 False）。
    pub can_seed: bool,
    /// `Task.isSeeding`（运行时，非持久化）。
    pub is_seeding: bool,
    /// `Task.hasOutputFile`（类属性，默认 True）。
    pub has_output_file: bool,
    /// `isExisting(task.outputPath)`（文件系统事实）。
    pub output_exists: bool,
    /// `Task.lastError`（最后一个 FAILED 步骤的 error，否则任一 error）。
    pub last_error: Option<TaskError>,
    pub selected_files: usize,
    pub total_files: usize,
    /// `currentSnapshot()` 的三元组。
    pub progress: f64,
    pub speed: i64,
    pub received: i64,
}

impl CardTask {
    /// 镜像 `_refreshForStatus` 里的 `_isFileMissing`：只有 COMPLETED 且
    /// 有输出文件但文件不存在才算。
    pub fn file_missing(&self) -> bool {
        self.status == TaskStatus::Completed && self.has_output_file && !self.output_exists
    }
}

/// 信息行一段（`FieldSpec.name` + 文本；`None` = 该段隐藏）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InfoField {
    pub name: &'static str,
    pub text: Option<String>,
}

/// 状态行（`statusLabel`）：`text` + 是否走告警色
/// （COMPLETED 的「文件不存在」分支 `setTextColor(QColor(200,160,80), …)`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusLine {
    pub text: String,
    pub warn: bool,
}

/// 进度条可见性动作。Python `_refreshForStatus` 只在 RUNNING 里 `show()`、
/// COMPLETED 里 `hide()`，FAILED 与 PAUSED/WAITING 分支**根本不碰**可见性
/// （沿用上一次的值），故这里记动作而不是布尔。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgressVisibility {
    Show,
    Hide,
    Keep,
}

impl ProgressVisibility {
    /// 无状态重渲染时的兜底：只有 `Hide` 才算隐藏。
    pub fn visible(self) -> bool {
        self != ProgressVisibility::Hide
    }
}

/// 进度条状态：`value` 是 `setValue(int(progress))` 的整数值，
/// `indeterminate` 决定是 `IndeterminateProgressBar` 还是 `ProgressBar`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProgressState {
    pub value: i64,
    pub indeterminate: bool,
    pub visibility: ProgressVisibility,
    pub error: bool,
    pub paused: bool,
}

/// `ButtonSpec.icon`：`_refreshButtons` 在 `state.icon is None` 时回落到它。
/// 值取 `FluentIcon.<NAME>.name`（与磁盘/金文件口径一致）。
pub const TOGGLE_ICON: &str = "PLAY";
pub const SELECT_FILES_ICON: &str = "LIBRARY";
pub const CHECKSUM_ICON: &str = "FINGERPRINT";
pub const OPEN_FILE_ICON: &str = "LINK";

/// 按钮状态（`ButtonState`）。`icon` 是**生效图标**：
/// `setIcon(state.icon if state.icon is not None else spec.icon)` 之后的那个。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ButtonState {
    pub icon: &'static str,
    pub visible: bool,
    pub enabled: bool,
}

impl ButtonState {
    /// `ButtonState()` 缺省：可见、可用，图标回落 spec.icon。
    pub const fn default_with(icon: &'static str) -> ButtonState {
        ButtonState {
            icon,
            visible: true,
            enabled: true,
        }
    }
}

/// 镜像 `toEtaText`：`fileSize>0 and speed>0` →
/// `toReadableTime(int((fileSize - received) / speed))`；否则 `"--"`。
/// `int()` 是向零截断，故用 `as i64`（f64 → i64 在 Rust 里也是向零截断）。
pub fn to_eta_text(file_size: i64, speed: i64, received: i64) -> String {
    if file_size > 0 && speed > 0 {
        return to_readable_time(((file_size - received) as f64 / speed as f64) as i64);
    }
    "--".into()
}

/// 镜像 `refresh()` 的信息行循环：逐段取 `formats.get(status) or
/// formats.get(None)`，无格式化函数 → 隐藏；格式化返回 `None` → 隐藏。
///
/// 顺序固定 `speed` / `eta` / `size`（`TaskCard.infoFields`）。
pub fn info_fields(task: &CardTask) -> Vec<InfoField> {
    let running = task.status == TaskStatus::Running;
    let speed = running.then(|| format!("{}/s", to_readable_size(task.speed as f64)));
    let eta = running.then(|| to_eta_text(task.file_size, task.speed, task.received));
    let size = crate::row::size_text(task.status, task.file_size, task.received);
    vec![
        InfoField {
            name: "speed",
            text: speed,
        },
        InfoField {
            name: "eta",
            text: eta,
        },
        InfoField {
            name: "size",
            text: size,
        },
    ]
}

/// 镜像 `toLocalizedError` 的「无译文层」路径：`translate(message) or
/// message` → `message`，再 `format_map(params)` 替换 `{key}`。
///
/// 未提供的键保留字面 `{key}`（Python 抛 `KeyError`；我方构造器恒带全参）。
pub fn error_text(error: &TaskError) -> String {
    let mut out = String::with_capacity(error.message.len());
    let mut rest = error.message;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let key = &after[..close];
        match error.params.iter().find(|(name, _)| *name == key) {
            Some((_, value)) => out.push_str(value),
            None => {
                out.push('{');
                out.push_str(key);
                out.push('}');
            }
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// 镜像 `_refreshForStatus` 的状态行 + `MultiFileTaskCard` 的覆盖。
///
/// RUNNING → `None`（`statusLabel.hide()`）；其余按分支出文案。
pub fn status_line(task: &CardTask) -> Option<StatusLine> {
    let base = match task.status {
        TaskStatus::Running => return None,
        TaskStatus::Completed => {
            if task.file_missing() {
                StatusLine {
                    text: "文件不存在".into(),
                    warn: true,
                }
            } else if let Some(text) = task.completed_at_text.as_deref() {
                StatusLine {
                    text: format!("完成于 {text}"),
                    warn: false,
                }
            } else {
                StatusLine {
                    text: "任务已经完成".into(),
                    warn: false,
                }
            }
        }
        TaskStatus::Failed => StatusLine {
            text: match &task.last_error {
                Some(error) => error_text(error),
                None => "下载过程中发生错误，请稍后重试".into(),
            },
            warn: false,
        },
        TaskStatus::Paused => StatusLine {
            text: "任务已经暂停".into(),
            warn: false,
        },
        TaskStatus::Waiting => StatusLine {
            text: "任务正在等待".into(),
            warn: false,
        },
    };
    // MultiFileTaskCard._refreshForStatus：`files` 多于一个、不是「文件
    // 不存在」、状态 ∈ {WAITING, COMPLETED} → 状态行改成文件计数
    // （`setText`，不重置颜色、不动可见性）。
    if task.total_files > 1
        && !task.file_missing()
        && matches!(task.status, TaskStatus::Waiting | TaskStatus::Completed)
    {
        return Some(StatusLine {
            text: format!("{}/{} 个文件", task.selected_files, task.total_files),
            warn: base.warn,
        });
    }
    Some(base)
}

/// 镜像 `_createProgressBar` + `_refreshForStatus` 的进度条三态。
pub fn progress_state(task: &CardTask) -> ProgressState {
    ProgressState {
        // Python `int(progress)`：f64 → i64 同为向零截断。
        value: task.progress as i64,
        // SpecialFileSize.UNKNOWN = 0、NOT_SUPPORTED = -1。
        indeterminate: matches!(task.file_size, 0 | -1),
        // RUNNING 才 show()、COMPLETED 才 hide()，其余分支不碰可见性。
        visibility: match task.status {
            TaskStatus::Running => ProgressVisibility::Show,
            TaskStatus::Completed => ProgressVisibility::Hide,
            TaskStatus::Failed | TaskStatus::Paused | TaskStatus::Waiting => {
                ProgressVisibility::Keep
            }
        },
        error: task.status == TaskStatus::Failed,
        paused: matches!(
            task.status,
            TaskStatus::Completed | TaskStatus::Paused | TaskStatus::Waiting
        ),
    }
}

/// 镜像 `TOGGLE_BUTTON.states`（spec 图标 PLAY）：
/// RUNNING → PAUSE + `canPause`；COMPLETED → 做种中给 PAUSE、否则沿用
/// PLAY，可用性 `canSeed and (not hasOutputFile or isExisting(outputPath))`。
pub fn toggle_button(task: &CardTask) -> ButtonState {
    match task.status {
        TaskStatus::Running => ButtonState {
            icon: "PAUSE",
            visible: true,
            enabled: task.can_pause,
        },
        TaskStatus::Completed => ButtonState {
            icon: if task.is_seeding {
                "PAUSE"
            } else {
                TOGGLE_ICON
            },
            visible: true,
            enabled: task.can_seed && (!task.has_output_file || task.output_exists),
        },
        _ => ButtonState::default_with(TOGGLE_ICON),
    }
}

/// 镜像 `SELECT_FILES_BUTTON.states`：`visible = bool(files) and len>1`。
pub fn select_files_button(task: &CardTask) -> ButtonState {
    ButtonState {
        icon: SELECT_FILES_ICON,
        visible: task.total_files > 1,
        enabled: true,
    }
}

/// 镜像 `CHECKSUM_BUTTON.states`：多文件时隐藏（改用「选择文件」）；
/// COMPLETED 才可点，且要求有输出文件且文件存在。
pub fn checksum_button(task: &CardTask) -> ButtonState {
    let visible = task.total_files <= 1;
    let enabled =
        task.status == TaskStatus::Completed && task.has_output_file && task.output_exists;
    ButtonState {
        icon: CHECKSUM_ICON,
        visible,
        enabled,
    }
}

/// 镜像 `OPEN_FILE_BUTTON.states`：非 COMPLETED 一律禁用；COMPLETED 时
/// `not hasOutputFile or isExisting(outputPath)`。
pub fn open_file_button(task: &CardTask) -> ButtonState {
    let enabled =
        task.status == TaskStatus::Completed && (!task.has_output_file || task.output_exists);
    ButtonState {
        icon: OPEN_FILE_ICON,
        visible: true,
        enabled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn task(status: TaskStatus) -> CardTask {
        CardTask {
            status,
            file_size: 0,
            completed_at_text: None,
            can_pause: true,
            can_seed: false,
            is_seeding: false,
            has_output_file: true,
            output_exists: true,
            last_error: None,
            selected_files: 0,
            total_files: 0,
            progress: 0.0,
            speed: 0,
            received: 0,
        }
    }

    /// 金文件里的一条 case → `CardTask`（缺字段按 Python 缺省口径兜底）。
    fn card_from_json(case: &Value) -> CardTask {
        let status = match case["status"].as_str().expect("status 缺失") {
            "WAITING" => TaskStatus::Waiting,
            "RUNNING" => TaskStatus::Running,
            "PAUSED" => TaskStatus::Paused,
            "COMPLETED" => TaskStatus::Completed,
            "FAILED" => TaskStatus::Failed,
            other => panic!("未知状态：{other}"),
        };
        let last_error = case
            .get("lastError")
            .filter(|value| !value.is_null())
            .map(|value| {
                let message = value["message"].as_str().expect("message 缺失");
                // `TaskError.message` 是 &'static str：金文件里的字面量必须
                // 是本模块认识的常量之一，用 Box::leak 换 'static 以保持
                // 强类型（测试进程内泄漏可忽略）。
                let leaked: &'static str = Box::leak(message.to_string().into_boxed_str());
                let params = value["params"]
                    .as_object()
                    .map(|map| {
                        map.iter()
                            .map(|(key, param)| {
                                let leaked_key: &'static str =
                                    Box::leak(key.clone().into_boxed_str());
                                (leaked_key, param.as_str().unwrap_or("").to_string())
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                TaskError::new(leaked, params)
            });
        CardTask {
            status,
            file_size: case["fileSize"].as_i64().unwrap_or(0),
            completed_at_text: case["completedAtText"].as_str().map(str::to_string),
            can_pause: case["canPause"].as_bool().unwrap_or(true),
            can_seed: case["canSeed"].as_bool().unwrap_or(false),
            is_seeding: case["isSeeding"].as_bool().unwrap_or(false),
            has_output_file: case["hasOutputFile"].as_bool().unwrap_or(true),
            output_exists: case["outputExists"].as_bool().unwrap_or(true),
            last_error,
            selected_files: case["selectedFiles"].as_u64().unwrap_or(0) as usize,
            total_files: case["totalFiles"].as_u64().unwrap_or(0) as usize,
            progress: case["progress"].as_f64().unwrap_or(0.0),
            speed: case["speed"].as_i64().unwrap_or(0),
            received: case["received"].as_i64().unwrap_or(0),
        }
    }

    fn button_from_json(value: &Value) -> (String, bool, bool) {
        (
            value["icon"].as_str().unwrap_or("").to_string(),
            value["visible"].as_bool().unwrap_or(true),
            value["enabled"].as_bool().unwrap_or(true),
        )
    }

    /// 卡片派生层与 CPython 逐条对比（`card.jsonl`）：金文件由
    /// `generate_card_fixture.py` 跑真 `TaskCard.refresh()` /
    /// `_refreshButtons()`（桩替换 widget 与文件系统事实）生成。
    #[test]
    fn card_layer_matches_python_fixture() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/card.jsonl");
        let text = std::fs::read_to_string(&path).expect("card.jsonl 缺失");
        let mut cases = 0usize;
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let case: Value = serde_json::from_str(line).expect("金文件行不是合法 JSON");
            let label = case["label"].as_str().unwrap_or("");
            let task = card_from_json(&case);

            // 信息行三段。
            let fields = info_fields(&task);
            let want = case["fields"].as_array().expect("fields 必须是数组");
            assert_eq!(fields.len(), want.len(), "信息行段数：{label}");
            for (got, expected) in fields.iter().zip(want) {
                assert_eq!(
                    got.name,
                    expected["name"].as_str().unwrap_or(""),
                    "段名：{label}"
                );
                assert_eq!(
                    got.text.as_deref(),
                    expected["text"].as_str(),
                    "段文本（{}）：{label}",
                    got.name
                );
                // Python 只有「有格式化函数且返回非 None」才 `show()`，故
                // 可见性恒等于「文本存在」。
                assert_eq!(
                    got.text.is_some(),
                    expected["visible"].as_bool().unwrap_or(false),
                    "段可见性与文本一致（{}）：{label}",
                    got.name
                );
            }

            // 状态行。
            let line_want = &case["statusLine"];
            match (status_line(&task), line_want.is_null()) {
                (None, true) => {}
                (Some(got), false) => {
                    assert_eq!(
                        got.text,
                        line_want["text"].as_str().unwrap_or(""),
                        "状态行：{label}"
                    );
                    assert_eq!(
                        got.warn,
                        line_want["warn"].as_bool().unwrap_or(false),
                        "告警色：{label}"
                    );
                }
                (got, _) => panic!("状态行有无不一致：{label} 实得 {got:?} 期望 {line_want}"),
            }

            // 进度条。
            let progress = progress_state(&task);
            let want_progress = &case["progressBar"];
            assert_eq!(
                progress.value,
                want_progress["value"].as_i64().unwrap_or(0),
                "进度值：{label}"
            );
            assert_eq!(
                progress.indeterminate,
                want_progress["indeterminate"].as_bool().unwrap_or(false),
                "不确定进度条：{label}"
            );
            let want_visibility = match want_progress["visibility"].as_str().unwrap_or("keep") {
                "show" => ProgressVisibility::Show,
                "hide" => ProgressVisibility::Hide,
                "keep" => ProgressVisibility::Keep,
                other => panic!("未知可见性动作：{other}"),
            };
            assert_eq!(
                progress.visibility, want_visibility,
                "进度条可见性：{label}"
            );
            assert_eq!(
                progress.error,
                want_progress["error"].as_bool().unwrap_or(false),
                "进度条错误态：{label}"
            );
            assert_eq!(
                progress.paused,
                want_progress["paused"].as_bool().unwrap_or(false),
                "进度条暂停态：{label}"
            );

            // 文件缺失标记。
            assert_eq!(
                task.file_missing(),
                case["fileMissing"].as_bool().unwrap_or(false),
                "文件缺失：{label}"
            );

            // 四个带 states 的按钮。
            for (name, got) in [
                ("toggle", toggle_button(&task)),
                ("selectFiles", select_files_button(&task)),
                ("checksum", checksum_button(&task)),
                ("openFile", open_file_button(&task)),
            ] {
                let (icon, visible, enabled) = button_from_json(&case["buttons"][name]);
                assert_eq!(got.icon, icon, "按钮图标（{name}）：{label}");
                assert_eq!(got.visible, visible, "按钮可见（{name}）：{label}");
                assert_eq!(got.enabled, enabled, "按钮可用（{name}）：{label}");
            }
            cases += 1;
        }
        // 防「金文件被清空/被裁剪仍全绿」。
        assert_eq!(cases, 44, "金文件条数变化需同步更新断言");
    }

    #[test]
    fn eta_text_mirrors_python_truncation() {
        assert_eq!(to_eta_text(4096, 1024, 512), "3s");
        assert_eq!(to_eta_text(0, 1024, 512), "--");
        assert_eq!(to_eta_text(4096, 0, 512), "--");
        // 已收超过总量：Python 会算出负数（int() 向零截断）。
        assert_eq!(to_eta_text(100, 1000, 3500), "-3s");
    }

    #[test]
    fn error_text_formats_params_without_translator() {
        let error = TaskError::server_error(500);
        assert_eq!(error_text(&error), "服务器返回了错误（500）");
        let plain = TaskError::disk_full();
        assert_eq!(error_text(&plain), "磁盘空间不足");
        // 缺参数保留字面（Python 抛 KeyError，我方构造器不会走到）。
        let missing = TaskError::new("未知 {detail}", Vec::new());
        assert_eq!(error_text(&missing), "未知 {detail}");
    }

    #[test]
    fn status_line_covers_every_status() {
        let mut running = task(TaskStatus::Running);
        running.file_size = 4096;
        assert_eq!(status_line(&running), None, "RUNNING 隐藏状态行");

        let mut completed = task(TaskStatus::Completed);
        completed.completed_at_text = Some("2026-01-02 03:04:05".into());
        assert_eq!(
            status_line(&completed).map(|line| line.text),
            Some("完成于 2026-01-02 03:04:05".into())
        );

        let mut missing = task(TaskStatus::Completed);
        missing.output_exists = false;
        missing.completed_at_text = Some("2026-01-02 03:04:05".into());
        let line = status_line(&missing).expect("应有状态行");
        assert_eq!(line.text, "文件不存在");
        assert!(line.warn, "文件不存在走告警色");

        let failed = task(TaskStatus::Failed);
        assert_eq!(
            status_line(&failed).map(|line| line.text),
            Some("下载过程中发生错误，请稍后重试".into())
        );

        let mut failed_with_error = task(TaskStatus::Failed);
        failed_with_error.last_error = Some(TaskError::port_in_use(16800));
        assert_eq!(
            status_line(&failed_with_error).map(|line| line.text),
            Some("端口 16800 被占用，请更换端口".into())
        );

        assert_eq!(
            status_line(&task(TaskStatus::Paused)).map(|line| line.text),
            Some("任务已经暂停".into())
        );
        assert_eq!(
            status_line(&task(TaskStatus::Waiting)).map(|line| line.text),
            Some("任务正在等待".into())
        );
    }

    #[test]
    fn multi_file_count_overrides_waiting_and_completed_only() {
        let mut waiting = task(TaskStatus::Waiting);
        waiting.total_files = 3;
        waiting.selected_files = 2;
        assert_eq!(
            status_line(&waiting).map(|line| line.text),
            Some("2/3 个文件".into())
        );

        let mut paused = task(TaskStatus::Paused);
        paused.total_files = 3;
        paused.selected_files = 2;
        assert_eq!(
            status_line(&paused).map(|line| line.text),
            Some("任务已经暂停".into()),
            "PAUSED 不覆盖"
        );

        // COMPLETED 覆盖「完成于 …」；但「文件不存在」优先，不覆盖。
        let mut completed = task(TaskStatus::Completed);
        completed.total_files = 2;
        completed.selected_files = 1;
        completed.completed_at_text = Some("2026-01-02 03:04:05".into());
        assert_eq!(
            status_line(&completed).map(|line| line.text),
            Some("1/2 个文件".into())
        );
        completed.output_exists = false;
        let line = status_line(&completed).expect("应有状态行");
        assert_eq!(line.text, "文件不存在");
        assert!(line.warn);
    }

    #[test]
    fn progress_state_mirrors_special_file_sizes() {
        let mut running = task(TaskStatus::Running);
        running.progress = 42.9;
        running.file_size = 4096;
        let state = progress_state(&running);
        assert_eq!(state.value, 42, "int() 向零截断");
        assert!(!state.indeterminate);
        assert_eq!(
            state.visibility,
            ProgressVisibility::Show,
            "RUNNING 才 show()"
        );
        assert!(!state.error);
        assert!(!state.paused);

        // UNKNOWN=0 与 NOT_SUPPORTED=-1 都是不确定进度条。
        for size in [0, -1] {
            let mut running = task(TaskStatus::Running);
            running.file_size = size;
            assert!(
                progress_state(&running).indeterminate,
                "fileSize={size} 应走不确定进度条"
            );
        }

        let failed = task(TaskStatus::Failed);
        assert!(progress_state(&failed).error);
        assert_eq!(
            progress_state(&failed).visibility,
            ProgressVisibility::Keep,
            "FAILED 分支不碰可见性"
        );
        assert_eq!(
            progress_state(&task(TaskStatus::Completed)).visibility,
            ProgressVisibility::Hide,
            "COMPLETED 才 hide()"
        );
        assert_eq!(
            progress_state(&task(TaskStatus::Paused)).visibility,
            ProgressVisibility::Keep
        );
        // 无状态重渲染兜底：只有 Hide 算隐藏。
        assert!(ProgressVisibility::Show.visible());
        assert!(ProgressVisibility::Keep.visible());
        assert!(!ProgressVisibility::Hide.visible());
        for status in [
            TaskStatus::Completed,
            TaskStatus::Paused,
            TaskStatus::Waiting,
        ] {
            assert!(progress_state(&task(status)).paused, "pause()：{status:?}");
        }
        assert!(!progress_state(&task(TaskStatus::Running)).paused);
    }

    #[test]
    fn toggle_button_mirrors_states_table() {
        // RUNNING：PAUSE + canPause。
        let mut running = task(TaskStatus::Running);
        running.can_pause = false;
        let state = toggle_button(&running);
        assert_eq!(state.icon, "PAUSE");
        assert!(!state.enabled);

        // COMPLETED：canSeed 且（无输出文件或文件存在）。
        let mut completed = task(TaskStatus::Completed);
        completed.can_seed = true;
        completed.is_seeding = true;
        let state = toggle_button(&completed);
        assert_eq!(state.icon, "PAUSE", "做种中给 PAUSE");
        assert!(state.enabled);
        completed.output_exists = false;
        assert!(!toggle_button(&completed).enabled, "文件缺失则禁用");

        // 非做种：沿用 spec 图标 PLAY。
        let mut completed = task(TaskStatus::Completed);
        completed.can_seed = true;
        assert_eq!(toggle_button(&completed).icon, TOGGLE_ICON);

        // 其它状态：缺省（PLAY、可见、可用）。
        for status in [TaskStatus::Waiting, TaskStatus::Paused, TaskStatus::Failed] {
            assert_eq!(
                toggle_button(&task(status)),
                ButtonState::default_with(TOGGLE_ICON)
            );
        }
    }

    #[test]
    fn file_buttons_mirror_states_table() {
        // 多文件时「选择文件」可见、「校验」隐藏。
        let mut multi = task(TaskStatus::Waiting);
        multi.total_files = 2;
        assert!(select_files_button(&multi).visible);
        assert!(!checksum_button(&multi).visible);

        let single = task(TaskStatus::Waiting);
        assert!(!select_files_button(&single).visible);
        assert!(checksum_button(&single).visible);
        // 非 COMPLETED：校验与打开文件都禁用。
        assert!(!checksum_button(&single).enabled);
        assert!(!open_file_button(&single).enabled);

        let mut completed = task(TaskStatus::Completed);
        completed.total_files = 1;
        assert!(checksum_button(&completed).enabled);
        assert!(open_file_button(&completed).enabled);
        completed.output_exists = false;
        assert!(!checksum_button(&completed).enabled);
        assert!(!open_file_button(&completed).enabled, "文件缺失则禁用");

        // hasOutputFile=false 时打开文件不受文件存在性约束。
        let mut folder = task(TaskStatus::Completed);
        folder.has_output_file = false;
        assert!(open_file_button(&folder).enabled);
    }
}
