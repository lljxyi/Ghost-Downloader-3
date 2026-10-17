//! 任务调度队列：镜像 `app/services/task_service.py` 的 `TaskQueue`。
//!
//! 这一层是纯逻辑，没有 Qt 依赖（Python 侧的 `Signal` 是项目自研的
//! `app.signal`，不是 Qt），所以可以整段搬过来逐条对齐。
//!
//! 两个内部容器的顺序语义要特别小心：
//!
//! - `_waiting` 是 `list[str]`，`wait()` 去重后**追加到尾部**，`nextWaiting()`
//!   从**头部**弹出；
//! - `_running` 是 `dict[str, str]`，`run()` 对已有键**就地改值并保持原插入位**，
//!   所以 `runningIds()` 的顺序不是「最近启动优先」，而是「首次启动顺序」。
//!   Rust 用 `Vec<(taskId, workId)>` 线性查改来保住这个顺序。
//!
//! 服务层那半个 `_rebalance` / `_pump` 也一并镜像成 `rebalance` /
//! `next_to_dispatch`，但它们**只算队列那一半**：真正「取消协程」「起任务」
//! 由调用方做（`workId` 只有调用方拿得到）。

/// 等待队列 + 运行中映射。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskQueue {
    waiting: Vec<String>,
    /// `(taskId, workId)`，按 taskId 首次进入的顺序排列。
    running: Vec<(String, String)>,
}

impl TaskQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// 入队（去重，追加到尾部）。镜像 `wait()`。
    pub fn wait(&mut self, task_id: &str) {
        if !self.is_waiting(task_id) {
            self.waiting.push(task_id.to_string());
        }
    }

    /// 出队（等待队列移除首个匹配 + 运行中移除）。镜像 `cancel()`。
    pub fn cancel(&mut self, task_id: &str) {
        if let Some(index) = self.waiting.iter().position(|id| id == task_id) {
            self.waiting.remove(index);
        }
        self.running.retain(|(id, _)| id != task_id);
    }

    /// 标记运行中。已有键**就地改 workId，保持原插入位**（Python dict 语义）。
    pub fn run(&mut self, task_id: &str, work_id: &str) {
        if let Some(slot) = self.running.iter_mut().find(|(id, _)| id == task_id) {
            slot.1 = work_id.to_string();
        } else {
            self.running
                .push((task_id.to_string(), work_id.to_string()));
        }
    }

    /// 运行结束。镜像 `done()`。
    pub fn done(&mut self, task_id: &str) {
        self.running.retain(|(id, _)| id != task_id);
    }

    /// 镜像 `workIdOf()`。
    pub fn work_id_of(&self, task_id: &str) -> Option<&str> {
        self.running
            .iter()
            .find(|(id, _)| id == task_id)
            .map(|(_, work_id)| work_id.as_str())
    }

    /// 镜像 `isRunning()`。
    pub fn is_running(&self, task_id: &str) -> bool {
        self.running.iter().any(|(id, _)| id == task_id)
    }

    /// 镜像 `isWaiting()`。
    pub fn is_waiting(&self, task_id: &str) -> bool {
        self.waiting.iter().any(|id| id == task_id)
    }

    /// 镜像 `waitingOrder()`。
    pub fn waiting_order(&self) -> Vec<String> {
        self.waiting.clone()
    }

    /// 把已在等待队列里的任务提到最前面。镜像 `moveToFront()`。
    ///
    /// 返回的 `true` 表示确实动过队列（有命中），调用方据此决定是否发
    /// `queueChanged`。注意**目标顺序取等待队列自身的顺序**，不是参数顺序。
    pub fn move_to_front(&mut self, task_ids: &[String]) -> bool {
        let targets: Vec<String> = self
            .waiting
            .iter()
            .filter(|id| task_ids.contains(id))
            .cloned()
            .collect();
        if targets.is_empty() {
            return false;
        }
        self.waiting.retain(|id| !task_ids.contains(id));
        self.waiting.splice(0..0, targets);
        true
    }

    /// 镜像 `runningCount()`。
    pub fn running_count(&self) -> usize {
        self.running.len()
    }

    /// 镜像 `runningIds()`（首次启动顺序）。
    pub fn running_ids(&self) -> Vec<String> {
        self.running.iter().map(|(id, _)| id.clone()).collect()
    }

    /// 从等待队列头部弹出一个。镜像 `nextWaiting()`。
    pub fn next_waiting(&mut self) -> Option<String> {
        if self.waiting.is_empty() {
            None
        } else {
            Some(self.waiting.remove(0))
        }
    }

    /// `_rebalance` 里 `runningIds()[maxTaskNum:]` 那一段。
    ///
    /// 超出并发上限的运行中任务（按 `runningIds()` 的插入序）。
    pub fn overflow_ids(&self, max_task_num: usize) -> Vec<String> {
        if max_task_num >= self.running.len() {
            Vec::new()
        } else {
            self.running[max_task_num..]
                .iter()
                .map(|(id, _)| id.clone())
                .collect()
        }
    }

    /// `_pump()` 的循环体：反复 `nextWaiting()` 直到并发位用满。
    ///
    /// `_pump` 每轮 `nextWaiting()` 之后立刻 `_dispatch()`（内部调 `run()`），
    /// 所以「已占用的并发位」= 当前运行数 + 本次已弹出的个数。返回的 taskId
    /// **已经从等待队列里弹出**，调用方必须对每个都 `run(id, workId)`。
    pub fn next_to_dispatch(&mut self, max_task_num: usize) -> Vec<String> {
        let mut popped = Vec::new();
        while self.running.len() + popped.len() < max_task_num {
            match self.next_waiting() {
                Some(task_id) => popped.push(task_id),
                None => break,
            }
        }
        popped
    }

    /// `_rebalance` 里「把超额任务退回等待队列」那一段（不含随后的 `_pump`）。
    ///
    /// 返回被退回的 taskId。`can_pause` 对应 `task.canPause`——不能暂停的任务
    /// 留在运行中，不再往下退（Python 侧就是 `continue`）。
    pub fn rebalance(
        &mut self,
        max_task_num: usize,
        can_pause: impl Fn(&str) -> bool,
    ) -> Vec<String> {
        let mut moved = Vec::new();
        for task_id in self.overflow_ids(max_task_num) {
            if can_pause(&task_id) {
                // `_cancelWork()` 里就是 `queue.cancel()` + 稍后的 `queue.wait()`。
                self.cancel(&task_id);
                self.wait(&task_id);
                moved.push(task_id);
            }
        }
        moved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn wait_dedupes_and_appends() {
        let mut queue = TaskQueue::new();
        queue.wait("a");
        queue.wait("b");
        queue.wait("a");
        assert_eq!(queue.waiting_order(), ids(&["a", "b"]));
        assert!(queue.is_waiting("a"));
        assert!(!queue.is_waiting("c"));
    }

    #[test]
    fn cancel_removes_from_both_containers() {
        let mut queue = TaskQueue::new();
        queue.wait("a");
        queue.wait("b");
        queue.run("c", "w1");
        queue.cancel("a");
        queue.cancel("c");
        assert_eq!(queue.waiting_order(), ids(&["b"]));
        assert_eq!(queue.running_count(), 0);
        // 不存在的键是 no-op。
        queue.cancel("zzz");
        assert_eq!(queue.waiting_order(), ids(&["b"]));
    }

    #[test]
    fn run_keeps_insertion_order_when_overwriting() {
        let mut queue = TaskQueue::new();
        queue.run("a", "w1");
        queue.run("b", "w2");
        queue.run("a", "w3");
        assert_eq!(queue.running_ids(), ids(&["a", "b"]));
        assert_eq!(queue.work_id_of("a"), Some("w3"));
        assert_eq!(queue.work_id_of("b"), Some("w2"));
        assert_eq!(queue.work_id_of("c"), None);
    }

    #[test]
    fn move_to_front_uses_waiting_order_and_reports_hits() {
        let mut queue = TaskQueue::new();
        for id in ["a", "b", "c", "d"] {
            queue.wait(id);
        }
        assert!(queue.move_to_front(&ids(&["d", "b"])));
        assert_eq!(queue.waiting_order(), ids(&["b", "d", "a", "c"]));
        assert!(!queue.move_to_front(&ids(&["zzz"])));
        assert_eq!(queue.waiting_order(), ids(&["b", "d", "a", "c"]));
    }

    #[test]
    fn next_waiting_pops_from_head() {
        let mut queue = TaskQueue::new();
        assert_eq!(queue.next_waiting(), None);
        queue.wait("a");
        queue.wait("b");
        assert_eq!(queue.next_waiting().as_deref(), Some("a"));
        assert_eq!(queue.next_waiting().as_deref(), Some("b"));
        assert_eq!(queue.next_waiting(), None);
    }

    #[test]
    fn overflow_and_next_to_dispatch_are_complements() {
        let mut queue = TaskQueue::new();
        for (index, id) in ["a", "b", "c"].iter().enumerate() {
            queue.run(id, &format!("w{index}"));
        }
        queue.wait("d");
        queue.wait("e");
        assert_eq!(queue.overflow_ids(2), ids(&["c"]));
        assert_eq!(queue.overflow_ids(3), Vec::<String>::new());
        assert_eq!(queue.overflow_ids(9), Vec::<String>::new());
        // 只剩 1 个并发位 → 从头部弹 1 个，剩下的还在等。
        assert_eq!(queue.next_to_dispatch(4), ids(&["d"]));
        assert_eq!(queue.waiting_order(), ids(&["e"]));
        // 并发位已满 → 一个都不弹。
        assert_eq!(queue.next_to_dispatch(3), Vec::<String>::new());
        assert_eq!(queue.next_to_dispatch(0), Vec::<String>::new());
    }

    #[test]
    fn rebalance_pushes_excess_back_to_the_tail() {
        let mut queue = TaskQueue::new();
        for (index, id) in ["a", "b", "c", "d"].iter().enumerate() {
            queue.run(id, &format!("w{index}"));
        }
        queue.wait("e");
        // 上限 2 → 超额的是 runningIds()[2:] = c、d；c 不能暂停，留在运行中。
        let moved = queue.rebalance(2, |id| id != "c");
        assert_eq!(moved, ids(&["d"]));
        assert_eq!(queue.running_ids(), ids(&["a", "b", "c"]));
        // 退回的顺序 = runningIds 的插入序，追加到等待队列尾部（e 在前）。
        assert_eq!(queue.waiting_order(), ids(&["e", "d"]));
        // 运行数 3 已超上限 → pump 一个都补不上。
        assert_eq!(queue.next_to_dispatch(2), Vec::<String>::new());
        // 上限抬到 4 → 只补得进 1 个（运行数 3）。
        assert_eq!(queue.next_to_dispatch(4), ids(&["e"]));
        // 上限 5 → 再补 1 个。
        assert_eq!(queue.next_to_dispatch(5), ids(&["d"]));
    }
}
