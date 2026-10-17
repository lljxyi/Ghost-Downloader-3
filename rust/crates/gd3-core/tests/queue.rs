//! `TaskQueue` 与 `_rebalance` / `_pump` 的金文件回放（第 54 步）。
//!
//! 金文件由 `tests/fixtures/generate_task_queue_fixture.py` 跑真
//! `app.services.task_service.TaskQueue` 生成，每个 case 是一串 op + 每步
//! 返回值 + 末态快照。

use gd3_core::TaskQueue;
use serde_json::{Map, Value, json};
use std::path::PathBuf;

const FIXTURE: &str = "tests/fixtures/task_queue.jsonl";
const EXPECTED_CASES: usize = 18;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)
}

fn read_cases() -> Vec<Value> {
    let text = std::fs::read_to_string(fixture_path()).expect("读金文件失败");
    // 金文件里没有裸 `\u{2028}`，但仍然按 `\n` 切（`str::lines` 也只认 `\n`）。
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("金文件行不是合法 JSON"))
        .collect()
}

/// 回放器：`TaskQueue` + 假派发器（`workId` 用递增序号）。
struct Replay {
    queue: TaskQueue,
    serial: usize,
}

impl Replay {
    fn new() -> Self {
        Self {
            queue: TaskQueue::new(),
            serial: 0,
        }
    }

    /// 顶替 `TaskService._dispatch()`：起任务 → `queue.run(taskId, workId)`。
    fn dispatch(&mut self, task_id: &str) {
        self.serial += 1;
        self.queue.run(task_id, &format!("w{}", self.serial));
    }

    /// `_pump()` 的循环体。
    fn pump(&mut self, max_task_num: usize) -> Vec<String> {
        let mut dispatched = Vec::new();
        for task_id in self.queue.next_to_dispatch(max_task_num) {
            self.dispatch(&task_id);
            dispatched.push(task_id);
        }
        dispatched
    }

    /// `_rebalance()`：先退超额，再 `_pump()`。
    fn rebalance(
        &mut self,
        max_task_num: usize,
        can_pause: &[String],
    ) -> (Vec<String>, Vec<String>) {
        let moved = self.queue.rebalance(max_task_num, |id| {
            can_pause.iter().any(|target| target == id)
        });
        let dispatched = self.pump(max_task_num);
        (moved, dispatched)
    }

    fn apply(&mut self, op: &Value) -> Value {
        let name = op["op"].as_str().expect("op 缺少名字");
        let task_id = || op["taskId"].as_str().expect("op 缺少 taskId").to_string();
        match name {
            "wait" => {
                self.queue.wait(&task_id());
                Value::Null
            }
            "cancel" => {
                self.queue.cancel(&task_id());
                Value::Null
            }
            "run" => {
                let work_id = op["workId"].as_str().expect("run 缺少 workId");
                self.queue.run(&task_id(), work_id);
                Value::Null
            }
            "done" => {
                self.queue.done(&task_id());
                Value::Null
            }
            "workIdOf" => match self.queue.work_id_of(&task_id()) {
                Some(work_id) => Value::String(work_id.to_string()),
                None => Value::Null,
            },
            "isRunning" => Value::Bool(self.queue.is_running(&task_id())),
            "isWaiting" => Value::Bool(self.queue.is_waiting(&task_id())),
            "waitingOrder" => json!(self.queue.waiting_order()),
            "moveToFront" => {
                let ids: Vec<String> = op["taskIds"]
                    .as_array()
                    .expect("moveToFront 缺少 taskIds")
                    .iter()
                    .map(|value| value.as_str().expect("taskIds 元素不是字符串").to_string())
                    .collect();
                Value::Bool(self.queue.move_to_front(&ids))
            }
            "runningCount" => json!(self.queue.running_count()),
            "runningIds" => json!(self.queue.running_ids()),
            "nextWaiting" => match self.queue.next_waiting() {
                Some(next) => Value::String(next),
                None => Value::Null,
            },
            "pump" => {
                let max = op["maxTaskNum"].as_u64().expect("pump 缺少 maxTaskNum") as usize;
                json!(self.pump(max))
            }
            "rebalance" => {
                let max = op["maxTaskNum"]
                    .as_u64()
                    .expect("rebalance 缺少 maxTaskNum") as usize;
                let can_pause: Vec<String> = op
                    .get("canPause")
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .map(|value| {
                                value.as_str().expect("canPause 元素不是字符串").to_string()
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let (moved, dispatched) = self.rebalance(max, &can_pause);
                json!({ "moved": moved, "dispatched": dispatched })
            }
            other => panic!("未知 op：{other}"),
        }
    }

    fn snapshot(&self) -> Value {
        let running: Vec<Value> = self
            .queue
            .running_ids()
            .into_iter()
            .map(|task_id| {
                let work_id = self
                    .queue
                    .work_id_of(&task_id)
                    .unwrap_or_default()
                    .to_string();
                json!([task_id, work_id])
            })
            .collect();
        let mut map = Map::new();
        map.insert("waiting".to_string(), json!(self.queue.waiting_order()));
        map.insert("running".to_string(), Value::Array(running));
        Value::Object(map)
    }
}

#[test]
fn task_queue_matches_python_fixture() {
    let cases = read_cases();
    assert_eq!(cases.len(), EXPECTED_CASES, "金文件用例数变了");

    for case in &cases {
        let name = case["name"].as_str().expect("case 缺少 name");
        let mut replay = Replay::new();
        let ops = case["ops"].as_array().expect("case 缺少 ops");
        let expected = case["results"].as_array().expect("case 缺少 results");
        assert_eq!(ops.len(), expected.len(), "{name}: ops/results 长度不一致");

        for (index, (op, want)) in ops.iter().zip(expected.iter()).enumerate() {
            let got = replay.apply(op);
            assert_eq!(&got, want, "{name}: 第 {index} 步 {op} 结果不符");
        }
        assert_eq!(&replay.snapshot(), &case["final"], "{name}: 末态不符");
    }
}

/// 单独钉住 `next_to_dispatch` 的两条约定：从**头部**弹出（不是只看一眼），
/// 且不超过剩余并发位。
#[test]
fn next_to_dispatch_pops_from_head_within_limit() {
    let mut queue = TaskQueue::new();
    queue.wait("a");
    queue.wait("b");
    queue.wait("c");
    queue.run("x", "w0");

    // 3 个并发位、已占 1 个 → 弹 2 个，剩下的 c 还在等。
    assert_eq!(
        queue.next_to_dispatch(3),
        vec!["a".to_string(), "b".to_string()]
    );
    assert_eq!(queue.waiting_order(), vec!["c"]);
    // 并发上限 1 已经占满 → 一个都不弹。
    assert_eq!(queue.running_ids(), vec!["x"]);
    assert_eq!(queue.next_to_dispatch(1), Vec::<String>::new());
    // 上限抬到 2 → 再弹一个；弹完等待队列就空了。
    assert_eq!(queue.next_to_dispatch(2), vec!["c".to_string()]);
    assert_eq!(queue.next_to_dispatch(2), Vec::<String>::new());
}
