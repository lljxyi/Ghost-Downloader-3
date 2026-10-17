//! 任务运行时事实的供给方（第 55 步）。
//!
//! `card::CardRuntime` / `drawer::DrawerRuntime` 要的输入分三类：
//!
//! 1. **记录里就有**：`status` / `fileSize` / `steps` / `completedAt` /
//!    `outputFolder` / `name` / `files` / `checksums` / `category`；
//! 2. **类属性**（`Task` / `TaskStep` 的 `ClassVar`）：`canSeed` /
//!    `hasOutputFile` / `canPause` 的默认值。这些不在 `tasks.jsonl` 里，
//!    只能从 `type` 标签反查登记表；
//! 3. **运行时/文件系统**：`isSeeding`（进程内状态）、`step.error`
//!    （`repr=False` 不落盘）、`isExisting(outputPath)`（文件系统）、
//!    `datetime.fromtimestamp(...)`（本地时区）。
//!
//! 本模块把 1+2 全部算掉，3 里的「文件系统」也一并做掉，剩下的
//! `isSeeding` / `lastError` / 时区偏移由调用方通过 [`LiveFacts`] 与参数给。
//!
//! 第 2 类的登记表是**手抄** Python 的类属性，风险是 Python 侧新增步骤类
//! 之后 Rust 静默按默认值算。所以金文件 `task_registry.jsonl` 把
//! `Task._registry` / `TaskStep._registry` 的全量类名 + 规则导出来，Rust
//! 侧逐条比对，新增类会立刻报红。

use crate::card::CardRuntime;
use crate::drawer::{DrawerRuntime, to_time_text};
use gd3_core::task::{TaskRecord, TaskStatus, TaskStep, short_type_name};
use gd3_core::task_error::TaskError;
use serde_json::Value;

// ---------------------------------------------------------------------------
// 类属性登记表（`Task` / `TaskStep` 的 ClassVar）
// ---------------------------------------------------------------------------

/// `Task` 的类属性对：`canSeed` / `hasOutputFile`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskKind {
    pub can_seed: bool,
    pub has_output_file: bool,
}

/// `Task.canSeed = True` 的任务类（其余全为 `False`）。
///
/// 取自 `Task._registry` 的实测导出：只有这两个包支持做种。
pub const SEEDING_TASKS: [&str; 2] = ["BTTask", "ED2kTask"];

/// `TaskStep.canPause` 的四种形态。
///
/// Python 侧 `canPause` 既可能是类属性（`True`/`False`），也可能是
/// `@property`（读实例字段），所以这里记「规则」而不是布尔。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepPauseRule {
    /// `canPause = True`（`TaskStep` 默认值，`BTTaskStep` / `ED2kTaskStep` 等）。
    Always,
    /// `canPause = False`（不可中断的步骤：合并、字幕、torrent 文件、抽取、转码）。
    Never,
    /// `@property canPause: return self.canUseRangeRequests`
    /// （`HttpTaskStep` 及其全部子类，以及 `FtpStep`）。
    RangeRequests,
    /// `@property canPause: return not self.task.isLive`（`M3U8TaskStep`）。
    NotLive,
}

/// `canPause = False` 的步骤类。
pub const NEVER_PAUSE_STEPS: [&str; 6] = [
    "BilibiliMergeStep",
    "BilibiliSubtitleStep",
    "BTTorrentFileStep",
    "FFmpegStep",
    "YouTubeExtractStep",
    "YouTubeMergeStep",
];

/// `canPause = canUseRangeRequests` 的步骤类（`HttpTaskStep` 全部子类 + `FtpStep`）。
pub const RANGE_PAUSE_STEPS: [&str; 10] = [
    "BilibiliAudioStep",
    "BilibiliCoverStep",
    "BilibiliVideoStep",
    "FFmpegResourceStep",
    "FtpStep",
    "HttpTaskStep",
    "HuggingFaceStep",
    "YouTubeCoverStep",
    "YouTubeResourceStep",
    "YouTubeSubtitleStep",
];

/// `Task` 的类属性（按 `type` 标签反查）。
pub fn task_kind(type_name: Option<&str>) -> TaskKind {
    let can_seed = type_name
        .map(short_type_name)
        .is_some_and(|name| SEEDING_TASKS.contains(&name));
    TaskKind {
        can_seed,
        // `Task.hasOutputFile = True` 且**没有任何子类覆盖**（实测 `Task._registry`
        // 全量导出），所以恒为真。留字段是为了跟 Python 的形状对齐。
        has_output_file: true,
    }
}

/// `TaskStep` 的可暂停规则（按 `type` 标签反查）。
pub fn step_pause_rule(type_name: Option<&str>) -> StepPauseRule {
    let Some(name) = type_name.map(short_type_name) else {
        return StepPauseRule::Always;
    };
    if NEVER_PAUSE_STEPS.contains(&name) {
        StepPauseRule::Never
    } else if RANGE_PAUSE_STEPS.contains(&name) {
        StepPauseRule::RangeRequests
    } else if name == "M3U8TaskStep" {
        StepPauseRule::NotLive
    } else {
        StepPauseRule::Always
    }
}

/// `M3U8Task.isLive`（只有 m3u8 包有这个字段，其余任务读出来是 `False`）。
pub fn task_is_live(record: &TaskRecord) -> bool {
    record
        .extra
        .get("isLive")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// 单个步骤的 `canPause`。
pub fn step_can_pause(step: &TaskStep, task_is_live: bool) -> bool {
    match step_pause_rule(step.type_name.as_deref()) {
        StepPauseRule::Always => true,
        StepPauseRule::Never => false,
        StepPauseRule::RangeRequests => step
            .extra
            .get("canUseRangeRequests")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        StepPauseRule::NotLive => !task_is_live,
    }
}

/// `Task.canPause`：取**第一个 RUNNING 步骤**的 `canPause`；没有 RUNNING 步骤则 `True`。
pub fn can_pause(record: &TaskRecord) -> bool {
    let is_live = task_is_live(record);
    for step in &record.steps {
        if step.status == TaskStatus::Running {
            return step_can_pause(step, is_live);
        }
    }
    true
}

// ---------------------------------------------------------------------------
// 文件系统 + 时间
// ---------------------------------------------------------------------------

/// 镜像 `app.platform.filesystem.isExisting`：`Path(path).stat()` 不抛异常。
///
/// Python 捕获 `OSError` 与 `ValueError`（路径含 NUL 时 `stat` 抛后者），
/// 两种情况 `fs::metadata` 都返回 `Err`，所以直接看 `is_ok`。
pub fn is_existing(path: &str) -> bool {
    std::fs::metadata(path).is_ok()
}

/// 镜像 `datetime.fromtimestamp(t).strftime("%Y-%m-%d %H:%M:%S")` 的
/// 「`completedAt` 为 0 则无文本」口径（`elif task.completedAt:` 是假值判断）。
pub fn completed_at_text(completed_at: i64, utc_offset_seconds: i64) -> Option<String> {
    if completed_at == 0 {
        None
    } else {
        Some(to_time_text(completed_at, utc_offset_seconds))
    }
}

// ---------------------------------------------------------------------------
// 运行时事实
// ---------------------------------------------------------------------------

/// 记录里没有、也不在文件系统里的那部分（进程内状态）。
#[derive(Clone, Debug, Default)]
pub struct LiveFacts {
    /// `Task.isSeeding`（`init=False`，不落盘；`startSeeding`/`stopSeeding` 改它）。
    pub is_seeding: bool,
    /// `Task.lastError`：`step.error` 是 `repr=False`，`fromDict` 之后恒为 `None`，
    /// 所以只可能来自活着的进程状态。
    pub last_error: Option<TaskError>,
}

/// 装配 `CardRuntime`：记录 + 类属性登记表 + 文件系统 + 调用方补的活状态。
pub fn card_runtime(record: &TaskRecord, live: &LiveFacts, utc_offset_seconds: i64) -> CardRuntime {
    let kind = task_kind(record.type_name.as_deref());
    CardRuntime {
        completed_at_text: completed_at_text(record.completed_at, utc_offset_seconds),
        can_pause: can_pause(record),
        can_seed: kind.can_seed,
        is_seeding: live.is_seeding,
        has_output_file: kind.has_output_file,
        output_exists: is_existing(&record.output_path()),
        last_error: live.last_error.clone(),
    }
}

/// 抽屉额外需要、但卡片层没有的输入。
#[derive(Clone, Debug)]
pub struct DrawerEnv {
    /// 本地时区偏移（秒）。跨 DST 的时间点由调用方按当时生效的偏移给。
    pub utc_offset_seconds: i64,
    /// `cfg.isCategoryEnabled.value`。
    pub category_enabled: bool,
    /// `toCategoryName(category)`；`None` = 任务无分类 → 显示「未分类」。
    pub category_name: Option<String>,
    /// 校验值输入框当前内容。
    pub expected_checksum: String,
}

/// 装配 `DrawerRuntime`。
pub fn drawer_runtime(record: &TaskRecord, live: &LiveFacts, env: &DrawerEnv) -> DrawerRuntime {
    DrawerRuntime {
        card: card_runtime(record, live, env.utc_offset_seconds),
        output_path: record.output_path(),
        utc_offset_seconds: env.utc_offset_seconds,
        category_enabled: env.category_enabled,
        category_name: env.category_name.clone(),
        expected_checksum: env.expected_checksum.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn step(type_name: &str, status: TaskStatus) -> TaskStep {
        serde_json::from_value(json!({
            "stepIndex": 0,
            "status": status,
            "type": type_name,
        }))
        .expect("step 反序列化失败")
    }

    fn record(type_name: &str, steps: Vec<TaskStep>) -> TaskRecord {
        serde_json::from_value(json!({
            "name": "n.bin",
            "url": "https://e/x",
            "packId": "http",
            "taskId": "tsk_1",
            "outputFolder": "/downloads",
            "steps": steps,
            "type": type_name,
        }))
        .expect("record 反序列化失败")
    }

    #[test]
    fn short_type_name_accepts_legacy_bare_names() {
        assert_eq!(
            short_type_name("features.http_pack.task.HttpTaskStep"),
            "HttpTaskStep"
        );
        assert_eq!(short_type_name("HttpTaskStep"), "HttpTaskStep");
    }

    #[test]
    fn task_kind_marks_only_seeding_packs() {
        assert!(task_kind(Some("features.bittorrent_pack.task.BTTask")).can_seed);
        assert!(task_kind(Some("features.ed2k_pack.task.ED2kTask")).can_seed);
        assert!(!task_kind(Some("features.http_pack.task.HttpTask")).can_seed);
        assert!(!task_kind(None).can_seed);
        assert!(task_kind(None).has_output_file);
    }

    #[test]
    fn can_pause_uses_first_running_step() {
        // 没有 RUNNING → True（即便有不可暂停的步骤在排队）
        let queued = record(
            "features.ffmpeg_pack.task.FFmpegTask",
            vec![step(
                "features.ffmpeg_pack.task.FFmpegStep",
                TaskStatus::Waiting,
            )],
        );
        assert!(can_pause(&queued));

        // 第一个 RUNNING 步骤说了算
        let mixed = record(
            "features.bili_pack.task.BilibiliTask",
            vec![
                step(
                    "features.bili_pack.task.BilibiliSubtitleStep",
                    TaskStatus::Running,
                ),
                step(
                    "features.bili_pack.task.BilibiliVideoStep",
                    TaskStatus::Running,
                ),
            ],
        );
        assert!(!can_pause(&mixed));

        let reversed = record(
            "features.bili_pack.task.BilibiliTask",
            vec![
                step(
                    "features.bili_pack.task.BilibiliVideoStep",
                    TaskStatus::Running,
                ),
                step(
                    "features.bili_pack.task.BilibiliSubtitleStep",
                    TaskStatus::Running,
                ),
            ],
        );
        // BilibiliVideoStep 继承 HttpTaskStep → canUseRangeRequests 缺省 False
        assert!(!can_pause(&reversed));
    }

    #[test]
    fn range_requests_rule_reads_the_step_field() {
        let mut step = step("features.http_pack.task.HttpTaskStep", TaskStatus::Running);
        assert!(!step_can_pause(&step, false));
        step.extra
            .insert("canUseRangeRequests".into(), Value::Bool(true));
        assert!(step_can_pause(&step, false));
    }

    #[test]
    fn not_live_rule_reads_the_task_field() {
        let m3u8 = step("features.m3u8_pack.task.M3U8TaskStep", TaskStatus::Running);
        assert!(step_can_pause(&m3u8, false));
        assert!(!step_can_pause(&m3u8, true));
    }

    #[test]
    fn unknown_step_types_default_to_pausable() {
        assert_eq!(step_pause_rule(None), StepPauseRule::Always);
        assert_eq!(
            step_pause_rule(Some("features.plugin_pack.task.CustomStep")),
            StepPauseRule::Always
        );
    }

    #[test]
    fn output_path_uses_the_record_folder() {
        let record = record("features.http_pack.task.HttpTask", vec![]);
        assert_eq!(record.output_path(), "/downloads/n.bin");
    }

    #[test]
    fn completed_at_text_is_none_for_zero() {
        assert_eq!(completed_at_text(0, 0), None);
        assert_eq!(
            completed_at_text(1_700_000_000, 0),
            Some("2023-11-14 22:13:20".to_string())
        );
        assert_eq!(
            completed_at_text(1_700_000_000, 3600),
            Some("2023-11-14 23:13:20".to_string())
        );
    }

    #[test]
    fn is_existing_reports_the_filesystem() {
        assert!(is_existing("/"));
        assert!(!is_existing("/definitely/not/here-4a2f"));
        assert!(!is_existing(""));
    }

    // -----------------------------------------------------------------------
    // 金文件比对（第 55 步）
    // -----------------------------------------------------------------------

    fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("{name} 缺失"))
    }

    fn rule_name(rule: StepPauseRule) -> &'static str {
        match rule {
            StepPauseRule::Always => "always",
            StepPauseRule::Never => "never",
            StepPauseRule::RangeRequests => "range",
            StepPauseRule::NotLive => "notLive",
        }
    }

    /// `task_registry.jsonl` 把 Python 的两张登记表全量导出来，这里逐条比对。
    ///
    /// 双向都要查：Python 新增一个步骤类而 Rust 表里没写 → 规则对不上；
    /// Python 删掉一个类而 Rust 表里还留着 → 反方向断言报红。
    #[test]
    fn registry_matches_python_fixture() {
        let text = fixture("task_registry.jsonl");
        let mut tasks = 0usize;
        let mut steps = 0usize;
        let mut seen_tasks: Vec<String> = Vec::new();
        let mut seen_steps: Vec<String> = Vec::new();

        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let row: Value = serde_json::from_str(line).expect("金文件行不是合法 JSON");
            let name = row["name"].as_str().expect("name 缺失").to_string();
            let full = row["type"].as_str().expect("type 缺失");

            match row["kind"].as_str().expect("kind 缺失") {
                "task" => {
                    tasks += 1;
                    let want_seed = row["canSeed"].as_bool().expect("canSeed 缺失");
                    let want_file = row["hasOutputFile"].as_bool().expect("hasOutputFile 缺失");
                    // 全名与裸类名都要认（旧 `tasks.jsonl` 只存裸类名）。
                    for tag in [full, name.as_str()] {
                        let kind = task_kind(Some(tag));
                        assert_eq!(kind.can_seed, want_seed, "canSeed：{name}");
                        assert_eq!(kind.has_output_file, want_file, "hasOutputFile：{name}");
                    }
                    seen_tasks.push(name);
                }
                "step" => {
                    steps += 1;
                    let want = row["rule"].as_str().expect("rule 缺失");
                    for tag in [full, name.as_str()] {
                        assert_eq!(
                            rule_name(step_pause_rule(Some(tag))),
                            want,
                            "canPause 规则：{name}"
                        );
                    }
                    seen_steps.push(name);
                }
                other => panic!("未知 kind：{other}"),
            }
        }

        // 8 个 Task 子类 + 基类；19 个 TaskStep 子类 + 基类。
        assert_eq!(tasks, 9, "Task 登记表条数");
        assert_eq!(steps, 20, "TaskStep 登记表条数");

        for name in SEEDING_TASKS {
            assert!(
                seen_tasks.iter().any(|seen| seen == name),
                "SEEDING_TASKS 里的 {name} 在 Python 登记表里已不存在"
            );
        }
        for name in NEVER_PAUSE_STEPS.iter().chain(RANGE_PAUSE_STEPS.iter()) {
            assert!(
                seen_steps.iter().any(|seen| seen == name),
                "canPause 规则表里的 {name} 在 Python 登记表里已不存在"
            );
        }
    }

    /// `task_facts.jsonl` 的 record 来自真 Python 对象的 `toDict()`，期望值来自真属性。
    ///
    /// `outputExists` 不在金文件里——它取决于跑测试的机器，由
    /// `is_existing_reports_the_filesystem` 单独覆盖。
    #[test]
    fn facts_match_python_fixture() {
        let text = fixture("task_facts.jsonl");
        let mut cases = 0usize;

        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let case: Value = serde_json::from_str(line).expect("金文件行不是合法 JSON");
            let name = case["name"].as_str().unwrap_or("");
            let record: TaskRecord =
                serde_json::from_value(case["record"].clone()).expect("record 反序列化失败");
            let live = LiveFacts {
                is_seeding: case["live"]["isSeeding"].as_bool().unwrap_or(false),
                last_error: None,
            };
            let utc_offset = case["utcOffset"].as_i64().unwrap_or(0);
            let want = &case["expected"];

            let runtime = card_runtime(&record, &live, utc_offset);
            assert_eq!(
                runtime.can_pause,
                want["canPause"].as_bool().expect("canPause 缺失"),
                "canPause：{name}"
            );
            assert_eq!(
                runtime.can_seed,
                want["canSeed"].as_bool().expect("canSeed 缺失"),
                "canSeed：{name}"
            );
            assert_eq!(
                runtime.has_output_file,
                want["hasOutputFile"].as_bool().expect("hasOutputFile 缺失"),
                "hasOutputFile：{name}"
            );
            assert_eq!(
                runtime.completed_at_text.as_deref(),
                want["completedAtText"].as_str(),
                "completedAtText：{name}"
            );
            assert_eq!(
                record.output_path(),
                want["outputPath"].as_str().expect("outputPath 缺失"),
                "outputPath：{name}"
            );

            // `drawer_runtime` 只是把同一批事实重新装配一遍，顺带查一遍。
            let env = DrawerEnv {
                utc_offset_seconds: utc_offset,
                category_enabled: false,
                category_name: None,
                expected_checksum: String::new(),
            };
            let drawer = drawer_runtime(&record, &live, &env);
            assert_eq!(
                drawer.output_path,
                want["outputPath"].as_str().expect("outputPath 缺失"),
                "drawer.outputPath：{name}"
            );
            assert_eq!(drawer.card.can_pause, runtime.can_pause, "drawer：{name}");
            assert_eq!(drawer.card.can_seed, runtime.can_seed, "drawer：{name}");
            assert_eq!(
                drawer.card.completed_at_text, runtime.completed_at_text,
                "drawer：{name}"
            );

            cases += 1;
        }

        assert_eq!(cases, 60, "金文件 case 数");
    }
}
