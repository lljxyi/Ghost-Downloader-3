//! 浏览器扩展会话状态机：`app/services/browser_service.py:29-597`。
//!
//! `browser_service.py`（597 行）分三层，本模块是最后一层：
//!
//! | Python | Rust |
//! |---|---|
//! | WebSocket 握手 + 帧（`handle` / `_send` 的封包部分） | `gd3-servers::websocket`（Phase 5/6） |
//! | 五个 `StrEnum` + `toStr` / `toInt` + 三个 `to*` 载荷转换 | [`crate::browser_payload`]（第 110 步） |
//! | **`BrowserClientSession` / 配对 / hello / create_task / task_action 状态机** | **本模块** |
//! | 托盘图标、配对对话框、`extensionUpdated` 的 UI 提示 | Phase 7 主窗口那条线 |
//!
//! ## 与 Python 的结构差异（都不是「还没做」）
//!
//! - **会话用 [`SessionId`] 标识**：Python 拿 `WebSocketStream` 的**对象身份**当键
//!   （`dict[WebSocketStream, BrowserClientSession]`），本 crate 不依赖网络层，宿主给每条
//!   连接分配一个 `u64`。会话表用 `Vec<(SessionId, ClientSession)>` 保插入序——Python 的
//!   `dict` 也是插入序，快照广播的会话顺序因此一致。
//! - **所有副作用摊成 [`SessionEffect`]**：Python 直接 `stream.send(...)`、发 Qt 信号、
//!   调 `openFile` / `revealInFolder`。这里只产出效果，由宿主执行，所以单测不需要网络、
//!   不需要桌面环境、也不会真的打开文件。
//! - **`install()` 的异步解包留给宿主**：`_onHello` 发现扩展是 `development` 版且
//!   **比 `LATEST_EXTENSION_VERSION` 旧**时（`isNewer(extension, LATEST)` 的定义是
//!   `parseVersion(latest) > parseVersion(current)`，`update.py:30`），Python
//!   `submit(self.install())`；这里产出
//!   [`SessionEffect::InstallExtension`]，宿主解包完再回调
//!   [`BrowserHub::extension_installed`]（成功/失败各一条）。
//! - **`_parse` 是同步的**：`featureService.parse` 在 Python 是 `async`，本仓解析器
//!   （`TaskParser::parse`，第 108 步）同步返回，所以 `_onCreateTask` 里
//!   「提交 → 回调 `_onTaskParsed`」两步在这里合成一次调用。
//! - **`_sendSnapshots` 的延迟合并显式化**：Python 的 `_onTasksChanged` 置位后
//!   `post(_sendSnapshots)`；这里拆成 [`BrowserHub::note_tasks_changed`]（判重）+
//!   [`BrowserHub::send_snapshots`]（真发），[`BrowserHub::tasks_changed`] 是两者的同步合成。
//!   宿主若要合并突发信号，只用前两个即可。
//! - **`toTaskOptions` 不会失败**：Python 的转换可能抛 `AttributeError` / `TypeError`，
//!   被 `_onCreateTask` 的 `except Exception` 兜成 `REJECTED` + `repr(e)`；本仓的
//!   [`to_task_options`] 是容错的（第 110 步的三处容错差异），因此没有这条拒绝路径。
//! - **`_onTaskAction` 不捕获异常**：Python 用 `try/except Exception` 把动作失败兜成
//!   `repr(e)`；本仓的 `TaskService` 动作方法返回 `Option` / `bool` 而不抛，`Err` 分支不存在。
//! - **`TaskService` 报告的「宿主半边」复用 [`crate::task_actions::TaskAction`]**：
//!   `delete` / `redownload` / `pause` 在 Rust 只做记账（第 99 步），
//!   `Task.deleteFiles`、`fileDeleteDenied`、`_unwatchFile`、`checksumStopped`、
//!   `_cancelWork`、`taskRemoved` 都得由宿主执行。这里不另造词汇表，直接用
//!   [`SessionEffect::Task`] 包 `TaskAction`，与卡片/抽屉那条线同形；尾部 `_pump()`
//!   派发的 id 走 [`SessionEffect::Dispatch`]（发号 `workId` 与提交协程归宿主）。
//!   动作的**服务调用序列**与 [`crate::task_actions::TaskActions`] 完全一致。
//!
//! ## 照抄的行为（含缺陷）
//!
//! - **`_isUpdatingExtension` 成功后不复位**：Python 只在 `_onExtensionExtractFailed` 里
//!   清标志，成功路径永远置位 → 同一进程内不会二次热更。照抄。
//! - **`_onTaskParsed` 里 `taskService.add` 出错时什么都不回**：Python 的异常从 `done=`
//!   回调冒泡到 `coroutineRunner`，没人接；这里 `Err` 也不产出效果。
//! - **`decryptionKeys` 实际是空操作**：`task.setOptions({"decryptionKeys": ...})` 里
//!   `Task.setOptions` 只认 `outputFolder` / `category`，`TaskStep.setOptions` 是 `pass`。
//!   照抄，注释保留原因。
//! - **`regenerateToken` 不踢会话出表**：只 `_close` + 置 `isAuthenticated = False`，
//!   会话仍在 `_sessions` 里（断开时才 `pop`）。
//!
//! ## 序列化口径
//!
//! Python 用 `json.dumps(payload, ensure_ascii=False)`（分隔符 `", "` / `": "`）；这里用
//! `serde_json::to_string`（紧凑分隔符、同样不转义非 ASCII）。差别只在空白，扩展侧按 JSON
//! 解析不受影响；快照去重比较的是本进程自己序列化出来的串，同样自洽。

use gd3_core::NameConflictChoice;
use gd3_core::filename::to_safe_filename;
use gd3_core::filesystem::is_existing;
use gd3_core::pair_token;
use gd3_core::path::{posix_parent, posix_suffix};
use gd3_core::task::{TaskRecord, TaskStatus};
use gd3_core::version::is_newer;
use serde_json::{Value, json};

use crate::boot::VERSION;
use crate::browser_payload::{
    BrowserAction, CreateTaskStatus, ErrorCode, MessageType, PROTOCOL_VERSION, PayloadDefaults,
    TaskSource, to_int, to_str, to_task_options, to_task_summary,
};
use crate::facts;
use crate::feature_service::{ParserChain, is_truthy};
use crate::identity::IdentityPreset;
use crate::oobe::LATEST_EXTENSION_VERSION;
use crate::task_actions::{TaskAction, delete_effects, pause_effects, redownload_effects};
use crate::task_service::TaskService;

/// 宿主给每条浏览器扩展连接分配的标识（替代 Python 的 `WebSocketStream` 对象身份）。
pub type SessionId = u64;

/// `Task.setName` 的 `maxLength`（`gd3-core::filename::to_safe_filename` 的第三参）。
const NAME_MAX_LENGTH: usize = 200;

/// `capabilities.taskActions` 的顺序 = Python `[a.value for a in TaskAction]`（声明序）。
const TASK_ACTIONS: [BrowserAction; 6] = [
    BrowserAction::TogglePause,
    BrowserAction::Cancel,
    BrowserAction::Remove,
    BrowserAction::Redownload,
    BrowserAction::OpenFile,
    BrowserAction::OpenFolder,
];

/// `BrowserClientSession`（`browser_service.py:29-38`）。
///
/// 少了 `stream` 字段——那是宿主的连接句柄，本模块只用 [`SessionId`] 指代。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientSession {
    pub peer_address: String,
    pub is_authenticated: bool,
    pub is_subscribed_to_tasks: bool,
    /// 上一次广播给这个会话的快照文本（相同就不重发）。
    pub last_snapshot: Option<String>,
    pub extension_version: String,
    pub install_type: String,
}

/// `PairRequest`（`browser_service.py:40-45`，`frozen` dataclass）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairRequest {
    pub request_id: String,
    pub peer_address: String,
    pub extension_version: String,
    pub client_kind: String,
}

/// 会话状态机产出的全部副作用。
///
/// 不 derive `PartialEq`：`TaskDraftRequested` 里的 `TaskRecord` 没有 `PartialEq`。
#[derive(Debug, Clone)]
pub enum SessionEffect {
    /// 一条文本帧（`json.dumps(payload, ensure_ascii=False)` 之后的样子）。
    Send { session: SessionId, text: String },
    /// `stream.close()`。
    Close { session: SessionId },
    /// `pairRequestChanged.emit(request)`。
    PairRequestChanged(Option<PairRequest>),
    /// `connectionChanged.emit()`。
    ConnectionChanged,
    /// `protocolMismatched.emit()`。
    ProtocolMismatched,
    /// `taskDraftRequested.emit([task])`。
    TaskDraftRequested(Vec<TaskRecord>),
    /// `extensionUpdated.emit(LATEST_EXTENSION_VERSION)`。
    ExtensionUpdated(String),
    /// `submit(self.install())`：宿主解包 crx，完成后回调
    /// [`BrowserHub::extension_installed`]。
    InstallExtension { session: SessionId },
    /// `TaskService` 报告里宿主还没做的部分，直接复用 [`crate::task_actions::TaskAction`]
    /// 的词汇表：浏览器动作与卡片动作调的是同一批服务方法，摊出来的效果自然同形
    /// （`checksumStopped` / `_unwatchFile` / `fileDeleteDenied` / `_cancelWork` /
    /// `Task.deleteFiles` / `taskRemoved` / `taskPaused`）。
    ///
    /// 这是第 111 步补的缺口：`TaskService::delete` 只做记账（第 99 步），
    /// `DeleteReport::delete_files` 与 `DeleteReport::delete_denied` 必须由宿主执行，
    /// 否则扩展点「取消」后文件留在盘上、`fileDeleteDenied` 也发不出去。
    Task(TaskAction),
    /// `_pump()` 派发出来的 id（`_dispatch` 的另一半）：宿主发号（workId）+
    /// `TaskService::dispatch` + 提交协程，等价于
    /// [`crate::task_actions::TaskActions`] 的 `dispatch`。空列表不产出。
    Dispatch { task_ids: Vec<String> },
    /// `openFile(path)`。
    OpenFile { path: String },
    /// `revealInFolder(path)`。
    RevealInFolder { path: String },
}

/// `handle_message` 需要的外部世界。
///
/// `parsers` / `presets` 对应 Python 的 `self._parse`（`FeatureService.parse`）与它的
/// 身份预设；`tasks` 对应 `self._taskService`；`conflict_choice` 是 Python
/// `_nameConflictChoice()` 的当前值（用 `gd3_core` 的那份枚举——`TaskService::add_with_conflict`
/// 吃的就是它；`gd3_ui::name_conflict::NameConflictChoice` 是配置/对话框那条线的同名镜像，
/// 宿主从 `Engine::name_conflict_choice()` 转一次）；`delete_recoverably` 是 `TaskStore.add` 覆盖旧占位符
/// 时用的回收站删除（生产传 `gd3_core::trash::delete_recoverably`，`Err` 按 `false` 处理）。
pub struct SessionContext<'a> {
    pub parsers: &'a ParserChain,
    pub presets: &'a [IdentityPreset],
    pub tasks: &'a mut TaskService,
    pub conflict_choice: NameConflictChoice,
    pub delete_recoverably: &'a mut dyn FnMut(&std::path::Path) -> bool,
}

/// `BrowserService` 的会话状态机部分。
///
/// 信号（`pairRequestChanged` / `connectionChanged` / …）改由 [`SessionEffect`] 表达；
/// Qt 侧的两个属性 `connectionSummary` / `pairRequest` 仍在 [`BrowserHub::connection_summary`]
/// 与 [`BrowserHub::pair_request`]。
#[derive(Debug, Clone)]
pub struct BrowserHub {
    sessions: Vec<(SessionId, ClientSession)>,
    pair_request: Option<PairRequest>,
    pair_session: Option<SessionId>,
    is_snapshot_pending: bool,
    is_updating_extension: bool,
    token: String,
    defaults: PayloadDefaults,
    should_draft: bool,
    max_task_num: usize,
}

impl BrowserHub {
    /// `BrowserService.__init__` 的状态部分。
    ///
    /// 三个配置项由宿主注入（Python 直接读 `cfg`）：`token` =
    /// `cfg.browserExtensionPairToken`、`defaults` = `downloadFolder` / `preBlockNum` /
    /// `currentHeaders()`、`should_draft` = `cfg.shouldDraftTakenDownload`、
    /// `max_task_num` = `cfg.maxTaskNum`。后两项的缺省值取 Python 配置的缺省
    /// （`False` / `3`）。
    pub fn new(token: impl Into<String>, defaults: PayloadDefaults) -> Self {
        BrowserHub {
            sessions: Vec::new(),
            pair_request: None,
            pair_session: None,
            is_snapshot_pending: false,
            is_updating_extension: false,
            token: token.into(),
            defaults,
            should_draft: false,
            max_task_num: 3,
        }
    }

    /// `cfg.browserExtensionPairToken.value`。
    pub fn token(&self) -> &str {
        &self.token
    }

    /// 宿主在配置变更后同步令牌（Python 每处现读 `cfg`）。
    pub fn set_token(&mut self, token: impl Into<String>) {
        self.token = token.into();
    }

    /// `cfg.downloadFolder` / `cfg.preBlockNum` / `currentHeaders()` 的当前值。
    pub fn set_defaults(&mut self, defaults: PayloadDefaults) {
        self.defaults = defaults;
    }

    /// `cfg.shouldDraftTakenDownload.value`。
    pub fn set_should_draft(&mut self, should_draft: bool) {
        self.should_draft = should_draft;
    }

    /// `cfg.maxTaskNum.value`。
    pub fn set_max_task_num(&mut self, max_task_num: usize) {
        self.max_task_num = max_task_num;
    }

    /// 单条会话（`self._sessions.get(stream)`）。
    pub fn session(&self, session: SessionId) -> Option<&ClientSession> {
        self.sessions
            .iter()
            .find(|(id, _)| *id == session)
            .map(|(_, client)| client)
    }

    /// 全部会话，按插入序（`self._sessions.items()`）。
    pub fn sessions(&self) -> &[(SessionId, ClientSession)] {
        &self.sessions
    }

    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// `pairRequest` 属性。
    pub fn pair_request(&self) -> Option<&PairRequest> {
        self.pair_request.as_ref()
    }

    /// `connectionSummary` 属性：第一条已认证会话的 `(installType, extensionVersion)`。
    pub fn connection_summary(&self) -> (String, String) {
        for (_, session) in &self.sessions {
            if session.is_authenticated {
                return (
                    session.install_type.clone(),
                    session.extension_version.clone(),
                );
            }
        }
        (String::new(), String::new())
    }

    /// `_isUpdatingExtension`（宿主用它禁用「检查扩展更新」按钮）。
    pub fn is_updating_extension(&self) -> bool {
        self.is_updating_extension
    }

    // ── 连接生命周期 ──────────────────────────────────────────────────────

    /// `_onConnected(stream, peerAddress)`：登记一条新会话，不产出效果。
    pub fn on_connected(&mut self, session: SessionId, peer_address: impl Into<String>) {
        let client = ClientSession {
            peer_address: peer_address.into(),
            ..ClientSession::default()
        };
        match self.sessions.iter_mut().find(|(id, _)| *id == session) {
            Some((_, existing)) => *existing = client,
            None => self.sessions.push((session, client)),
        }
    }

    /// `_onDisconnected(stream)`：摘掉会话；若是配对中的那条，顺带清掉配对请求。
    pub fn on_disconnected(&mut self, session: SessionId) -> Vec<SessionEffect> {
        let mut effects = Vec::new();
        let Some(index) = self.sessions.iter().position(|(id, _)| *id == session) else {
            return effects;
        };
        let (_, client) = self.sessions.remove(index);
        if self.pair_session == Some(session) {
            effects.push(self.set_pair_request(None, None));
        }
        if client.is_authenticated {
            effects.push(SessionEffect::ConnectionChanged);
        }
        effects
    }

    // ── 配对 ──────────────────────────────────────────────────────────────

    /// `regenerateToken()`：换令牌 + 关掉所有会话 + 清认证位。
    ///
    /// 返回新令牌（宿主负责写回 `cfg.browserExtensionPairToken`）与效果队列。
    /// Python 只 `_close` + 置 `isAuthenticated = False`，**不**把会话移出表。
    pub fn regenerate_token(&mut self) -> (String, Vec<SessionEffect>) {
        let token = pair_token::generate();
        self.token = token.clone();
        let had_authenticated = self.sessions.iter().any(|(_, s)| s.is_authenticated);
        let mut effects = Vec::new();
        for (id, client) in self.sessions.iter_mut() {
            effects.push(SessionEffect::Close { session: *id });
            client.is_authenticated = false;
        }
        if had_authenticated {
            effects.push(SessionEffect::ConnectionChanged);
        }
        (token, effects)
    }

    /// `approvePair(requestId)`。
    pub fn approve_pair(&mut self, request_id: &str) -> Vec<SessionEffect> {
        let mut effects = Vec::new();
        let Some(session) = self.take_pair_session(request_id, &mut effects) else {
            return effects;
        };
        let payload = json!({
            "type": MessageType::PairResult.as_str(),
            "requestId": request_id,
            "ok": true,
            "token": self.token,
            "message": "配对成功",
        });
        effects.push(self.send(session, payload));
        effects
    }

    /// `rejectPair(requestId)`。
    pub fn reject_pair(&mut self, request_id: &str) -> Vec<SessionEffect> {
        let mut effects = Vec::new();
        let Some(session) = self.take_pair_session(request_id, &mut effects) else {
            return effects;
        };
        let payload = json!({
            "type": MessageType::PairResult.as_str(),
            "requestId": request_id,
            "ok": false,
            "message": "已拒绝配对请求",
        });
        effects.push(self.send(session, payload));
        effects
    }

    /// `_takePairSession(requestId)`：命中就清掉配对请求，并把 `pairRequestChanged` 放进队列。
    fn take_pair_session(
        &mut self,
        request_id: &str,
        effects: &mut Vec<SessionEffect>,
    ) -> Option<SessionId> {
        match &self.pair_request {
            Some(request) if request.request_id == request_id => {}
            _ => return None,
        }
        let session = self.pair_session;
        effects.push(self.set_pair_request(None, None));
        session
    }

    /// `_setPairRequest(request, session)`：写两个字段 + 发 `pairRequestChanged`。
    fn set_pair_request(
        &mut self,
        request: Option<PairRequest>,
        session: Option<SessionId>,
    ) -> SessionEffect {
        self.pair_request = request.clone();
        self.pair_session = session;
        SessionEffect::PairRequestChanged(request)
    }

    // ── 快照 ──────────────────────────────────────────────────────────────

    /// `_onTasksChanged` 的判重部分：`true` = 需要跑一次 `_sendSnapshots`。
    ///
    /// Python 在 `_isSnapshotPending` 为真时直接丢弃这次信号（合并突发变更）。
    pub fn note_tasks_changed(&mut self) -> bool {
        if self.is_snapshot_pending {
            return false;
        }
        self.is_snapshot_pending = true;
        true
    }

    /// `_sendSnapshots()`：给每个「已认证 + 已订阅」的会话推一份去重后的快照。
    pub fn send_snapshots(&mut self, tasks: &TaskService) -> Vec<SessionEffect> {
        self.is_snapshot_pending = false;
        let subscribers: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|(_, session)| session.is_authenticated && session.is_subscribed_to_tasks)
            .map(|(id, _)| *id)
            .collect();
        if subscribers.is_empty() {
            return Vec::new();
        }

        // `sorted(tasks, key=lambda t: t.createdAt, reverse=True)`：Python 的 `sort` 稳定，
        // `reverse=True` 不反转相等元素的相对顺序，`sort_by_key` 同样是稳定排序。
        let mut records: Vec<&TaskRecord> = tasks.records().iter().collect();
        records.sort_by_key(|record| std::cmp::Reverse(record.created_at));
        let summaries: Vec<Value> = records
            .iter()
            .map(|record| Value::Object(to_task_summary(record)))
            .collect();
        let text = dumps(&json!({
            "type": MessageType::TaskSnapshot.as_str(),
            "tasks": summaries,
        }));

        let mut effects = Vec::new();
        for session in subscribers {
            let last = self
                .session(session)
                .and_then(|client| client.last_snapshot.clone());
            if last.as_deref() == Some(text.as_str()) {
                continue;
            }
            if let Some((_, client)) = self.sessions.iter_mut().find(|(id, _)| *id == session) {
                client.last_snapshot = Some(text.clone());
            }
            effects.push(SessionEffect::Send {
                session,
                text: text.clone(),
            });
        }
        effects
    }

    /// `_onTasksChanged` + `_sendSnapshots` 的同步合成（宿主在信号回调里直接用这个）。
    pub fn tasks_changed(&mut self, tasks: &TaskService) -> Vec<SessionEffect> {
        if !self.note_tasks_changed() {
            return Vec::new();
        }
        self.send_snapshots(tasks)
    }

    // ── 扩展热更 ──────────────────────────────────────────────────────────

    /// `_onExtensionExtracted` / `_onExtensionExtractFailed`。
    ///
    /// 成功：会话还在表里就发 `RELOAD` + `extensionUpdated`；`is_updating_extension`
    /// **不复位**（Python 的缺陷，照抄）。失败：只复位标志，不产出效果。
    pub fn extension_installed(
        &mut self,
        session: SessionId,
        extracted: bool,
    ) -> Vec<SessionEffect> {
        if !extracted {
            self.is_updating_extension = false;
            return Vec::new();
        }
        if self.session(session).is_none() {
            return Vec::new();
        }
        let payload = json!({ "type": MessageType::Reload.as_str() });
        vec![
            self.send(session, payload),
            SessionEffect::ExtensionUpdated(LATEST_EXTENSION_VERSION.to_string()),
        ]
    }

    // ── 入站消息 ──────────────────────────────────────────────────────────

    /// `_onMessageReceived(stream, message)`。
    pub fn handle_message(
        &mut self,
        session: SessionId,
        raw: &[u8],
        context: &mut SessionContext<'_>,
    ) -> Vec<SessionEffect> {
        let mut effects = Vec::new();
        if self.session(session).is_none() {
            return effects;
        }

        let data = match serde_json::from_slice::<Value>(raw) {
            Ok(value) => value,
            Err(_) => {
                effects.push(self.send_error(
                    session,
                    "无效的消息格式",
                    None,
                    ErrorCode::BadRequest,
                ));
                return effects;
            }
        };
        let Some(data) = data.as_object() else {
            effects.push(self.send_error(session, "无效的消息结构", None, ErrorCode::BadRequest));
            return effects;
        };
        let Some(message_type) = MessageType::parse(&to_str(data, "type")) else {
            effects.push(self.send_error(session, "未知的消息类型", None, ErrorCode::BadRequest));
            return effects;
        };

        // 这两类**先于**认证判定（Python 的顺序：pair_request → hello → 认证门）。
        if message_type == MessageType::PairRequest {
            effects.extend(self.on_pair_requested(session, data));
            return effects;
        }
        if message_type == MessageType::Hello {
            effects.extend(self.on_hello(session, data));
            return effects;
        }

        let authenticated = self
            .session(session)
            .map(|client| client.is_authenticated)
            .unwrap_or(false);
        if !authenticated {
            effects.push(self.send_error(
                session,
                "请先完成握手认证",
                None,
                ErrorCode::Unauthorized,
            ));
            effects.push(SessionEffect::Close { session });
            return effects;
        }

        match message_type {
            MessageType::SubscribeTasks => {
                if let Some((_, client)) = self.sessions.iter_mut().find(|(id, _)| *id == session) {
                    client.is_subscribed_to_tasks = true;
                    client.last_snapshot = None;
                }
                effects.extend(self.send_snapshots(context.tasks));
            }
            MessageType::CreateTask => effects.extend(self.on_create_task(session, data, context)),
            MessageType::TaskAction => effects.extend(self.on_task_action(session, data, context)),
            _ => {}
        }
        effects
    }

    /// `_onPairRequested`：同一会话可以反复改自己的配对请求，别的会话会被挡回去。
    fn on_pair_requested(
        &mut self,
        session: SessionId,
        data: &serde_json::Map<String, Value>,
    ) -> Vec<SessionEffect> {
        let request_id = to_str(data, "requestId");
        if self.pair_request.is_some() && self.pair_session != Some(session) {
            let payload = json!({
                "type": MessageType::PairResult.as_str(),
                "requestId": request_id,
                "ok": false,
                "message": "已有配对请求待处理",
            });
            return vec![self.send(session, payload)];
        }
        let peer_address = self
            .session(session)
            .map(|client| client.peer_address.clone())
            .unwrap_or_default();
        let request = PairRequest {
            request_id,
            peer_address,
            extension_version: to_str(data, "extensionVersion"),
            client_kind: to_str(data, "clientKind"),
        };
        vec![self.set_pair_request(Some(request), Some(session))]
    }

    /// `_onHello`：协议版本 → 令牌 → 置认证位 + `HELLO_ACK` →（development 版）热更。
    fn on_hello(
        &mut self,
        session: SessionId,
        data: &serde_json::Map<String, Value>,
    ) -> Vec<SessionEffect> {
        let mut effects = Vec::new();
        let request_id = to_str(data, "requestId");
        let request_id = if request_id.is_empty() {
            None
        } else {
            Some(request_id)
        };

        if to_int(data, "protocolVersion", 0) != PROTOCOL_VERSION {
            effects.push(self.send_error(
                session,
                "协议版本不匹配",
                request_id.as_deref(),
                ErrorCode::ProtocolMismatch,
            ));
            effects.push(SessionEffect::Close { session });
            effects.push(SessionEffect::ProtocolMismatched);
            return effects;
        }

        if to_str(data, "token") != self.token {
            effects.push(self.send_error(
                session,
                "配对令牌无效",
                request_id.as_deref(),
                ErrorCode::Unauthorized,
            ));
            effects.push(SessionEffect::Close { session });
            return effects;
        }

        let extension_version = to_str(data, "extensionVersion");
        let install_type = to_str(data, "installType");
        if let Some((_, client)) = self.sessions.iter_mut().find(|(id, _)| *id == session) {
            client.is_authenticated = true;
            client.extension_version = extension_version.clone();
            client.install_type = install_type.clone();
        }
        effects.push(SessionEffect::ConnectionChanged);

        let actions: Vec<&str> = TASK_ACTIONS.iter().map(|action| action.as_str()).collect();
        let payload = json!({
            "type": MessageType::HelloAck.as_str(),
            "protocolVersion": PROTOCOL_VERSION,
            "appVersion": VERSION,
            "capabilities": {
                "taskSnapshots": true,
                "taskActions": actions,
            },
        });
        effects.push(self.send(session, payload));

        if install_type == "development"
            && is_newer(&extension_version, LATEST_EXTENSION_VERSION)
            && !self.is_updating_extension
        {
            self.is_updating_extension = true;
            effects.push(SessionEffect::InstallExtension { session });
        }
        effects
    }

    /// `_onCreateTask`：校验 → `toTaskOptions` → `parse` → `_onTaskParsed`。
    fn on_create_task(
        &mut self,
        session: SessionId,
        data: &serde_json::Map<String, Value>,
        context: &mut SessionContext<'_>,
    ) -> Vec<SessionEffect> {
        let mut effects = Vec::new();
        let request_id = to_str(data, "requestId");
        let payload = data.get("payload").and_then(Value::as_object);
        // `toStr(data, "source", TaskSource.RESOURCE)`：Python 的 `toStr` 先 `data.get(key)`
        // 再判 `isinstance(value, str)`，缺省**只在值不是字符串时**生效。所以 `source: ""`
        // 原样返回空串，随后 `TaskSource("")` 抛 `ValueError` → 「未知的任务来源」；
        // 缺键 / 非字符串才落 `resource`。这里逐字照抄，不能写成「空串就落缺省」。
        let raw_source = match data.get("source") {
            Some(Value::String(text)) => text.clone(),
            _ => TaskSource::Resource.as_str().to_string(),
        };
        let title = to_str(data, "title");
        // `draft = data.get("draft")` 拿的是**原值**，`shouldDraft` 直接拿它判真值：
        // 只有 `None` 才落 `cfg.shouldDraftTakenDownload`。所以 `draft: 1` / `"x"` 算真、
        // `draft: 0` 算假——不是「非 bool 就落缺省」。
        let should_draft = match data.get("draft") {
            None => self.should_draft,
            Some(value) => is_truthy(Some(value)),
        };

        let Some(payload) = payload else {
            effects.push(self.send_error(session, "无效的请求", None, ErrorCode::BadRequest));
            return effects;
        };
        if request_id.is_empty() {
            effects.push(self.send_error(session, "无效的请求", None, ErrorCode::BadRequest));
            return effects;
        }
        let Some(source) = TaskSource::parse(&raw_source) else {
            effects.push(self.send_error(session, "未知的任务来源", None, ErrorCode::BadRequest));
            return effects;
        };

        // `payload.get("decryptionKeys") or []`：只在 `_onTaskParsed` 里用，而那条路是空操作。
        let options = to_task_options(source, payload, &self.defaults);
        let mut options_map = options.to_map();

        match context.parsers.parse(&mut options_map, context.presets) {
            Ok(record) => effects.extend(self.on_task_parsed(
                record,
                session,
                &request_id,
                &title,
                should_draft,
                context,
            )),
            Err(error) => effects.push(self.send_create_task_result(
                session,
                &request_id,
                CreateTaskStatus::Rejected,
                "",
                &error.to_string(),
            )),
        }
        effects
    }

    /// `_onTaskParsed`：改名 → 草稿或直加 → 回 `create_task_result`。
    fn on_task_parsed(
        &mut self,
        mut record: TaskRecord,
        session: SessionId,
        request_id: &str,
        title: &str,
        should_draft: bool,
        context: &mut SessionContext<'_>,
    ) -> Vec<SessionEffect> {
        let mut effects = Vec::new();

        // `task.setOptions({"decryptionKeys": decryptionKeys})`：`Task.setOptions` 只认
        // `outputFolder` / `category`，`TaskStep.setOptions` 是 `pass` —— 空操作，照抄。

        if !title.is_empty() {
            // `existingSuffix = Path(task.name).suffix`
            let existing_suffix = posix_suffix(&record.name);
            let candidate = if !existing_suffix.is_empty()
                && !title
                    .to_lowercase()
                    .ends_with(&existing_suffix.to_lowercase())
            {
                format!("{title}{existing_suffix}")
            } else {
                title.to_string()
            };
            // `toSafeFilename(name, fallback=self.name or "download")`：fallback 取改名前的名字。
            let fallback = if record.name.is_empty() {
                "download".to_string()
            } else {
                record.name.clone()
            };
            record.name = to_safe_filename(&candidate, &fallback, NAME_MAX_LENGTH);
        }

        if should_draft {
            effects.push(self.send_create_task_result(
                session,
                request_id,
                CreateTaskStatus::Drafted,
                "",
                "",
            ));
            effects.push(SessionEffect::TaskDraftRequested(vec![record]));
            return effects;
        }

        let task_id = record.task_id.clone();
        // `if not self._taskService.add(task)`：False 只在重名冲突（用户选择「询问」）时出现。
        match context.tasks.add_with_conflict(
            record,
            context.conflict_choice,
            true,
            None,
            &mut *context.delete_recoverably,
        ) {
            Ok(report) if report.outcome.is_none() => {
                effects.push(self.send_create_task_result(
                    session,
                    request_id,
                    CreateTaskStatus::Drafted,
                    "",
                    "",
                ));
            }
            Ok(_) => {
                effects.push(self.send_create_task_result(
                    session,
                    request_id,
                    CreateTaskStatus::Created,
                    &task_id,
                    "",
                ));
            }
            // Python 里 `add` 抛的 `OSError` 从 `done=` 回调冒泡，没人接：什么也不回。
            Err(_) => {}
        }
        effects
    }

    /// `_onTaskAction`：六种动作 + 三种失败文案。
    fn on_task_action(
        &mut self,
        session: SessionId,
        data: &serde_json::Map<String, Value>,
        context: &mut SessionContext<'_>,
    ) -> Vec<SessionEffect> {
        let mut effects = Vec::new();
        let request_id = to_str(data, "requestId");
        let task_id = to_str(data, "taskId");
        let raw_action = to_str(data, "action");

        if request_id.is_empty() {
            effects.push(self.send_error(session, "缺少 requestId", None, ErrorCode::BadRequest));
            return effects;
        }
        let Some(action) = BrowserAction::parse(&raw_action) else {
            effects.push(self.send_result(
                session,
                MessageType::TaskActionResult,
                &request_id,
                false,
                "不支持的操作",
                "",
            ));
            return effects;
        };

        let Some(record) = context.tasks.task(&task_id) else {
            effects.push(self.send_result(
                session,
                MessageType::TaskActionResult,
                &request_id,
                false,
                "任务不存在",
                "",
            ));
            return effects;
        };
        let status = record.status;
        let can_pause = facts::can_pause(record);
        let output_path = record.output_path();

        match action {
            BrowserAction::TogglePause => match status {
                TaskStatus::Running => {
                    if !can_pause {
                        effects.push(self.send_result(
                            session,
                            MessageType::TaskActionResult,
                            &request_id,
                            false,
                            "当前任务不支持暂停",
                            "",
                        ));
                        return effects;
                    }
                    if let Some(report) = context.tasks.pause(&task_id, self.max_task_num) {
                        push_task_effects(&mut effects, pause_effects(&task_id, &report));
                        push_dispatch(&mut effects, report.started);
                    }
                }
                TaskStatus::Completed => {
                    effects.push(self.send_result(
                        session,
                        MessageType::TaskActionResult,
                        &request_id,
                        false,
                        "任务已完成",
                        "",
                    ));
                    return effects;
                }
                _ => {
                    // Python `TaskService.start` = `_schedule` = `queue.wait` + `_pump()`
                    // （`task_service.py:392-395`）：命中 `isRunning` / `isWaiting` 时连
                    // `_pump` 都不走，`TaskService::start` 返回 `false` 即此。
                    if context.tasks.start(&task_id) {
                        let started = context.tasks.pump(self.max_task_num);
                        push_dispatch(&mut effects, started);
                    }
                }
            },
            BrowserAction::Cancel | BrowserAction::Remove => {
                // Python：`delete(task, shouldDeleteFiles=True/False)`。报告的固定半边
                // 摊成 [`TaskAction`]，尾部 `_pump()` 的派发另走 [`SessionEffect::Dispatch`]。
                let delete_files = action == BrowserAction::Cancel;
                if let Some(report) =
                    context
                        .tasks
                        .delete(&task_id, delete_files, self.max_task_num)
                {
                    push_task_effects(&mut effects, delete_effects(&task_id, &report));
                    push_dispatch(&mut effects, report.started);
                }
            }
            BrowserAction::Redownload => {
                if let Some(report) = context.tasks.redownload(&task_id, self.max_task_num) {
                    push_task_effects(&mut effects, redownload_effects(&task_id, &report));
                    push_dispatch(&mut effects, report.started);
                }
            }
            BrowserAction::OpenFile => {
                if !is_existing(&output_path) {
                    effects.push(self.send_result(
                        session,
                        MessageType::TaskActionResult,
                        &request_id,
                        false,
                        "文件尚未生成",
                        "",
                    ));
                    return effects;
                }
                effects.push(SessionEffect::OpenFile {
                    path: output_path.clone(),
                });
            }
            BrowserAction::OpenFolder => {
                let folder = posix_parent(&output_path);
                if !is_existing(&folder) {
                    effects.push(self.send_result(
                        session,
                        MessageType::TaskActionResult,
                        &request_id,
                        false,
                        "目录不存在",
                        "",
                    ));
                    return effects;
                }
                effects.push(SessionEffect::RevealInFolder { path: output_path });
            }
        }

        effects.push(self.send_result(
            session,
            MessageType::TaskActionResult,
            &request_id,
            true,
            "",
            "",
        ));
        effects
    }

    // ── 封包 ──────────────────────────────────────────────────────────────

    /// `_send(session, payload)`。
    fn send(&self, session: SessionId, payload: Value) -> SessionEffect {
        SessionEffect::Send {
            session,
            text: dumps(&payload),
        }
    }

    /// `_sendError`：`requestId` 为空串时**不带**这个键（Python 的 `if requestId`）。
    fn send_error(
        &self,
        session: SessionId,
        message: &str,
        request_id: Option<&str>,
        code: ErrorCode,
    ) -> SessionEffect {
        let mut payload = serde_json::Map::new();
        payload.insert("type".to_string(), Value::from(MessageType::Error.as_str()));
        payload.insert("message".to_string(), Value::from(message));
        payload.insert("code".to_string(), Value::from(code.as_str()));
        if let Some(request_id) = request_id {
            payload.insert("requestId".to_string(), Value::from(request_id));
        }
        self.send(session, Value::Object(payload))
    }

    /// `_sendResult`：`message` / `taskId` 为空串时**不带**对应的键。
    fn send_result(
        &self,
        session: SessionId,
        message_type: MessageType,
        request_id: &str,
        ok: bool,
        message: &str,
        task_id: &str,
    ) -> SessionEffect {
        let mut payload = serde_json::Map::new();
        payload.insert("type".to_string(), Value::from(message_type.as_str()));
        payload.insert("requestId".to_string(), Value::from(request_id));
        payload.insert("ok".to_string(), Value::Bool(ok));
        if !message.is_empty() {
            payload.insert("message".to_string(), Value::from(message));
        }
        if !task_id.is_empty() {
            payload.insert("taskId".to_string(), Value::from(task_id));
        }
        self.send(session, Value::Object(payload))
    }

    /// `_sendCreateTaskResult`。
    fn send_create_task_result(
        &self,
        session: SessionId,
        request_id: &str,
        status: CreateTaskStatus,
        task_id: &str,
        message: &str,
    ) -> SessionEffect {
        let mut payload = serde_json::Map::new();
        payload.insert(
            "type".to_string(),
            Value::from(MessageType::CreateTaskResult.as_str()),
        );
        payload.insert("requestId".to_string(), Value::from(request_id));
        payload.insert("status".to_string(), Value::from(status.as_str()));
        if !task_id.is_empty() {
            payload.insert("taskId".to_string(), Value::from(task_id));
        }
        if !message.is_empty() {
            payload.insert("message".to_string(), Value::from(message));
        }
        self.send(session, Value::Object(payload))
    }
}

/// 把 [`TaskAction`] 逐个包成 [`SessionEffect::Task`]（顺序不变）。
fn push_task_effects(effects: &mut Vec<SessionEffect>, task: Vec<TaskAction>) {
    effects.extend(task.into_iter().map(SessionEffect::Task));
}

/// 空列表不产出（Python 的 `_pump()` 没派发时什么也不发）。
fn push_dispatch(effects: &mut Vec<SessionEffect>, task_ids: Vec<String>) {
    if !task_ids.is_empty() {
        effects.push(SessionEffect::Dispatch { task_ids });
    }
}

/// `json.dumps(payload, ensure_ascii=False)`（分隔符差异见模块头）。
fn dumps(payload: &Value) -> String {
    serde_json::to_string(payload).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature_service::TaskParser;
    use gd3_core::task_error::TaskError;
    use serde_json::Map;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    // ── 脚手架 ────────────────────────────────────────────────────────────

    /// 临时目录（`gd3-ui` 的 dev-dependencies 里没有 `tempfile`）。
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(tag: &str) -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let index = NEXT.fetch_add(1, Ordering::SeqCst);
            let path =
                std::env::temp_dir().join(format!("gd3ui-bs-{tag}-{}-{index}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("建临时目录");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// 只认非空 `url` 的解析器：名字取 `options.name`，输出目录取 `options.outputFolder`。
    ///
    /// 真实的八个解析器还没搬（Phase 5/6 只搬了链），单测用它钉住「载荷 → 解析器」
    /// 这一段的接线，不掺解析器自己的逻辑。
    struct UrlParser;

    impl TaskParser for UrlParser {
        fn matches(&self, options: &Map<String, Value>) -> bool {
            options
                .get("url")
                .and_then(Value::as_str)
                .is_some_and(|url| !url.is_empty())
        }

        fn parse(&self, options: &Map<String, Value>) -> Result<TaskRecord, TaskError> {
            let url = options
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let mut record = task_record(&format!("tsk_{url}"), TaskStatus::Waiting, 0);
            record.url = url;
            record.name = options
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .unwrap_or("download.bin")
                .to_string();
            record.output_folder = options
                .get("outputFolder")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            Ok(record)
        }
    }

    fn task_record(task_id: &str, status: TaskStatus, created_at: i64) -> TaskRecord {
        TaskRecord {
            name: format!("{task_id}.bin"),
            url: format!("https://example.com/{task_id}.bin"),
            pack_id: String::new(),
            task_id: task_id.to_string(),
            status,
            steps: Vec::new(),
            created_at,
            completed_at: 0,
            output_folder: String::new(),
            file_size: 0,
            files: None,
            category: None,
            should_seed: false,
            checksums: Map::new(),
            extra: Map::new(),
            type_name: None,
        }
    }

    /// 输出目录在 `dir` 里的记录。
    fn record_in(dir: &Path, task_id: &str, status: TaskStatus) -> TaskRecord {
        let mut record = task_record(task_id, status, 0);
        record.output_folder = dir.to_string_lossy().into_owned();
        record
    }

    fn payload_defaults(dir: &Path) -> PayloadDefaults {
        PayloadDefaults {
            download_folder: dir.to_string_lossy().into_owned(),
            pre_block_num: 4,
            current_headers: Map::new(),
        }
    }

    fn frame(value: Value) -> Vec<u8> {
        serde_json::to_vec(&value).expect("序列化请求帧")
    }

    fn hello_frame(version: i64, token: &str, extension: &str, install_type: &str) -> Vec<u8> {
        frame(json!({
            "type": "hello",
            "requestId": "hello-1",
            "protocolVersion": version,
            "token": token,
            "extensionVersion": extension,
            "installType": install_type,
        }))
    }

    /// 一条 hub + 任务服务 + 解析链 + 回收站桩，`handle_message` 的固定外部世界。
    struct World {
        dir: TempDir,
        hub: BrowserHub,
        parsers: ParserChain,
        tasks: TaskService,
    }

    impl World {
        fn new(tag: &str) -> Self {
            let dir = TempDir::new(tag);
            let hub = BrowserHub::new("tok", payload_defaults(dir.path()));
            let mut parsers = ParserChain::new();
            parsers.register(Box::new(UrlParser));
            World {
                dir,
                hub,
                parsers,
                tasks: TaskService::new(),
            }
        }

        fn connect(&mut self, session: SessionId) -> &mut Self {
            self.hub.on_connected(session, format!("peer-{session}"));
            self
        }

        fn hello(&mut self, session: SessionId) -> Vec<SessionEffect> {
            self.send(session, &hello_frame(2, "tok", "2.2.0", "store"))
        }

        fn send(&mut self, session: SessionId, raw: &[u8]) -> Vec<SessionEffect> {
            let mut trash = |_: &Path| false;
            let mut context = SessionContext {
                parsers: &self.parsers,
                presets: &[],
                tasks: &mut self.tasks,
                conflict_choice: NameConflictChoice::KeepBoth,
                delete_recoverably: &mut trash,
            };
            self.hub.handle_message(session, raw, &mut context)
        }
    }

    /// 全部 `Send` 效果解析回 JSON（按产出顺序）。
    fn sends(effects: &[SessionEffect]) -> Vec<Value> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                SessionEffect::Send { text, .. } => {
                    Some(serde_json::from_str(text).expect("Send 的文本应当是 JSON"))
                }
                _ => None,
            })
            .collect()
    }

    fn close_count(effects: &[SessionEffect]) -> usize {
        effects
            .iter()
            .filter(|effect| matches!(effect, SessionEffect::Close { .. }))
            .count()
    }

    /// 有没有满足条件的效果（`SessionEffect` 只 `Debug + Clone`，不能直接 `contains`）。
    fn has_effect(effects: &[SessionEffect], predicate: impl Fn(&SessionEffect) -> bool) -> bool {
        effects.iter().any(predicate)
    }

    fn pair_request_ids(effects: &[SessionEffect]) -> Vec<Option<String>> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                SessionEffect::PairRequestChanged(request) => {
                    Some(request.as_ref().map(|request| request.request_id.clone()))
                }
                _ => None,
            })
            .collect()
    }

    // ── hello ─────────────────────────────────────────────────────────────

    #[test]
    fn a_stale_protocol_version_is_refused_and_reported() {
        let mut world = World::new("proto");
        world.connect(1);
        let effects = world.send(1, &hello_frame(1, "tok", "2.2.0", "store"));

        assert_eq!(close_count(&effects), 1);
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, SessionEffect::ProtocolMismatched)),
            "协议不匹配要单独发信号（Qt 侧弹提示）"
        );
        let sends = sends(&effects);
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0]["type"], "error");
        assert_eq!(sends[0]["message"], "协议版本不匹配");
        assert_eq!(sends[0]["code"], "protocol_mismatch");
        assert_eq!(sends[0]["requestId"], "hello-1");
        assert!(!world.hub.session(1).expect("会话").is_authenticated);
    }

    #[test]
    fn a_wrong_token_is_refused_without_the_protocol_signal() {
        let mut world = World::new("token");
        world.connect(1);
        let effects = world.send(1, &hello_frame(2, "nope", "2.2.0", "store"));

        assert_eq!(close_count(&effects), 1);
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, SessionEffect::ProtocolMismatched))
        );
        let sends = sends(&effects);
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0]["message"], "配对令牌无效");
        assert_eq!(sends[0]["code"], "unauthorized");
        assert!(!world.hub.session(1).expect("会话").is_authenticated);
    }

    #[test]
    fn an_error_without_a_request_id_omits_the_key() {
        let mut world = World::new("noreq");
        world.connect(1);
        let raw = frame(json!({
            "type": "hello",
            "protocolVersion": 2,
            "token": "nope",
            "extensionVersion": "2.2.0",
            "installType": "store",
        }));
        let sends = sends(&world.send(1, &raw));
        assert_eq!(sends.len(), 1);
        assert!(
            sends[0].get("requestId").is_none(),
            "Python 的 `if requestId` 不会塞空串"
        );
    }

    #[test]
    fn hello_authenticates_and_acks_the_capabilities() {
        let mut world = World::new("hello");
        world.connect(1);
        let effects = world.hello(1);

        assert!(matches!(effects[0], SessionEffect::ConnectionChanged));
        let sends = sends(&effects);
        assert_eq!(sends.len(), 1);
        let ack = &sends[0];
        assert_eq!(ack["type"], "hello_ack");
        assert_eq!(ack["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(ack["appVersion"], VERSION);
        assert_eq!(ack["capabilities"]["taskSnapshots"], json!(true));
        let actions: Vec<&str> = ack["capabilities"]["taskActions"]
            .as_array()
            .expect("taskActions 是数组")
            .iter()
            .map(|action| action.as_str().expect("动作名是字符串"))
            .collect();
        assert_eq!(
            actions,
            vec![
                "toggle_pause",
                "cancel",
                "remove",
                "redownload",
                "open_file",
                "open_folder"
            ],
            "顺序 = Python `[a.value for a in TaskAction]` 的声明序"
        );

        let session = world.hub.session(1).expect("会话");
        assert!(session.is_authenticated);
        assert_eq!(session.extension_version, "2.2.0");
        assert_eq!(session.install_type, "store");
        assert_eq!(
            world.hub.connection_summary(),
            ("store".to_string(), "2.2.0".to_string())
        );
    }

    #[test]
    /// `1.0.0` 比 `LATEST_EXTENSION_VERSION` 旧，唯一拦住热更的是 `installType`。
    fn a_store_build_never_triggers_a_hot_update() {
        let mut world = World::new("store");
        world.connect(1);
        let effects = world.send(1, &hello_frame(2, "tok", "1.0.0", "store"));
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, SessionEffect::InstallExtension { .. })),
            "只有 development 版才热更"
        );
        assert!(!world.hub.is_updating_extension());
    }

    #[test]
    fn a_newer_development_build_is_installed_once() {
        let mut world = World::new("hot");
        world.connect(1);
        let effects = world.send(1, &hello_frame(2, "tok", "1.0.0", "development"));
        let installs: Vec<SessionId> = effects
            .iter()
            .filter_map(|effect| match effect {
                SessionEffect::InstallExtension { session } => Some(*session),
                _ => None,
            })
            .collect();
        assert_eq!(installs, vec![1]);
        assert!(world.hub.is_updating_extension());

        // 再来一次 hello：`_isUpdatingExtension` 已经置位，不重复提交。
        let again = world.send(1, &hello_frame(2, "tok", "1.0.0", "development"));
        assert!(
            !again
                .iter()
                .any(|effect| matches!(effect, SessionEffect::InstallExtension { .. }))
        );
    }

    #[test]
    fn a_finished_install_reloads_the_extension_and_keeps_the_flag() {
        let mut world = World::new("reload");
        world.connect(1);
        world.send(1, &hello_frame(2, "tok", "1.0.0", "development"));

        let effects = world.hub.extension_installed(1, true);
        let sends = sends(&effects);
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0]["type"], "reload");
        assert!(effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::ExtensionUpdated(version) if version == LATEST_EXTENSION_VERSION
        )));
        assert!(
            world.hub.is_updating_extension(),
            "照抄 Python：成功路径不复位标志，同一进程内不会二次热更"
        );
    }

    #[test]
    fn a_failed_install_only_clears_the_flag() {
        let mut world = World::new("fail");
        world.connect(1);
        world.send(1, &hello_frame(2, "tok", "1.0.0", "development"));
        let effects = world.hub.extension_installed(1, false);
        assert!(effects.is_empty());
        assert!(!world.hub.is_updating_extension());
    }

    // ── 认证门与坏帧 ──────────────────────────────────────────────────────

    #[test]
    fn anything_but_hello_before_authentication_is_rejected() {
        let mut world = World::new("gate");
        world.connect(1);
        let effects = world.send(1, &frame(json!({"type": "subscribe_tasks"})));

        let sends = sends(&effects);
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0]["message"], "请先完成握手认证");
        assert_eq!(sends[0]["code"], "unauthorized");
        assert_eq!(close_count(&effects), 1);
        assert!(!world.hub.session(1).expect("会话").is_subscribed_to_tasks);
    }

    #[test]
    fn a_broken_frame_gets_a_bad_request() {
        let mut world = World::new("broken");
        world.connect(1);

        for (raw, expected) in [
            (b"not json".to_vec(), "无效的消息格式"),
            (frame(json!([1, 2, 3])), "无效的消息结构"),
            (frame(json!({"type": "bogus"})), "未知的消息类型"),
        ] {
            let effects = world.send(1, &raw);
            let sends = sends(&effects);
            assert_eq!(sends.len(), 1, "{expected}");
            assert_eq!(sends[0]["type"], "error");
            assert_eq!(sends[0]["message"], expected);
            assert_eq!(sends[0]["code"], "bad_request");
            assert!(sends[0].get("requestId").is_none());
            assert_eq!(close_count(&effects), 0, "坏帧不断连接");
        }
    }

    #[test]
    fn a_frame_from_an_unknown_session_is_ignored() {
        let mut world = World::new("ghost");
        assert!(
            world
                .send(7, &hello_frame(2, "tok", "2.2.0", "store"))
                .is_empty()
        );
    }

    // ── 配对 ──────────────────────────────────────────────────────────────

    #[test]
    fn a_second_session_cannot_claim_the_pending_pair_request() {
        let mut world = World::new("pair");
        world.connect(1);
        world.connect(2);

        let first = world.send(
            1,
            &frame(json!({
                "type": "pair_request",
                "requestId": "r1",
                "extensionVersion": "2.2.0",
                "clientKind": "chrome",
            })),
        );
        assert_eq!(pair_request_ids(&first), vec![Some("r1".to_string())]);
        match &first[0] {
            SessionEffect::PairRequestChanged(Some(request)) => {
                assert_eq!(request.peer_address, "peer-1");
                assert_eq!(request.extension_version, "2.2.0");
                assert_eq!(request.client_kind, "chrome");
            }
            other => panic!("应当产出配对请求，实际 {other:?}"),
        }

        let second = world.send(
            2,
            &frame(json!({"type": "pair_request", "requestId": "r2"})),
        );
        let sends = sends(&second);
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0]["type"], "pair_result");
        assert_eq!(sends[0]["requestId"], "r2");
        assert_eq!(sends[0]["ok"], json!(false));
        assert_eq!(sends[0]["message"], "已有配对请求待处理");
        assert_eq!(
            world
                .hub
                .pair_request()
                .map(|request| request.request_id.as_str()),
            Some("r1"),
            "第一条请求仍在"
        );
    }

    #[test]
    fn the_same_session_can_replace_its_own_pair_request() {
        let mut world = World::new("repair");
        world.connect(1);
        world.send(
            1,
            &frame(json!({"type": "pair_request", "requestId": "r1"})),
        );
        let effects = world.send(
            1,
            &frame(json!({"type": "pair_request", "requestId": "r2"})),
        );
        assert_eq!(pair_request_ids(&effects), vec![Some("r2".to_string())]);
    }

    #[test]
    fn approving_a_pair_request_sends_the_token() {
        let mut world = World::new("approve");
        world.connect(1);
        world.send(
            1,
            &frame(json!({"type": "pair_request", "requestId": "r1"})),
        );

        let effects = world.hub.approve_pair("r1");
        assert_eq!(pair_request_ids(&effects), vec![None], "先清请求");
        let sends = sends(&effects);
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0]["type"], "pair_result");
        assert_eq!(sends[0]["requestId"], "r1");
        assert_eq!(sends[0]["ok"], json!(true));
        assert_eq!(sends[0]["token"], "tok");
        assert_eq!(sends[0]["message"], "配对成功");
        assert!(world.hub.pair_request().is_none());
    }

    #[test]
    fn rejecting_a_pair_request_sends_no_token() {
        let mut world = World::new("reject");
        world.connect(1);
        world.send(
            1,
            &frame(json!({"type": "pair_request", "requestId": "r1"})),
        );

        let effects = world.hub.reject_pair("r1");
        assert_eq!(pair_request_ids(&effects), vec![None]);
        let sends = sends(&effects);
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0]["ok"], json!(false));
        assert_eq!(sends[0]["message"], "已拒绝配对请求");
        assert!(sends[0].get("token").is_none());
    }

    #[test]
    fn an_unknown_pair_request_id_does_nothing() {
        let mut world = World::new("nopair");
        world.connect(1);
        assert!(world.hub.approve_pair("nope").is_empty());
        assert!(world.hub.reject_pair("nope").is_empty());
    }

    #[test]
    fn regenerating_the_token_closes_every_session() {
        let mut world = World::new("regen");
        world.connect(1);
        world.hello(1);
        world.connect(2);

        let (token, effects) = world.hub.regenerate_token();
        assert_ne!(token, "tok");
        assert_eq!(token.len(), 22, "token_urlsafe(16) 是 22 个字符");
        assert_eq!(world.hub.token(), token);
        assert_eq!(close_count(&effects), 2);
        assert!(matches!(
            effects.last(),
            Some(SessionEffect::ConnectionChanged)
        ));
        assert_eq!(world.hub.session_count(), 2, "Python 不把会话移出表");
        assert!(!world.hub.session(1).expect("会话").is_authenticated);
    }

    #[test]
    fn regenerating_the_token_is_silent_when_nobody_was_authenticated() {
        let mut world = World::new("regen2");
        world.connect(1);
        let (_, effects) = world.hub.regenerate_token();
        assert_eq!(close_count(&effects), 1);
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, SessionEffect::ConnectionChanged))
        );
    }

    #[test]
    fn disconnecting_the_pairing_session_clears_the_request() {
        let mut world = World::new("disc");
        world.connect(1);
        world.send(
            1,
            &frame(json!({"type": "pair_request", "requestId": "r1"})),
        );

        let effects = world.hub.on_disconnected(1);
        assert_eq!(pair_request_ids(&effects), vec![None]);
        assert!(world.hub.pair_request().is_none());
        assert_eq!(world.hub.session_count(), 0);
    }

    // ── 快照 ──────────────────────────────────────────────────────────────

    #[test]
    fn subscribing_sends_a_snapshot_newest_first() {
        let mut world = World::new("snap");
        world.connect(1);
        world.hello(1);
        world.tasks = TaskService::from_records(vec![
            task_record("old", TaskStatus::Waiting, 1),
            task_record("new", TaskStatus::Completed, 5),
        ]);

        let effects = world.send(1, &frame(json!({"type": "subscribe_tasks"})));
        assert!(world.hub.session(1).expect("会话").is_subscribed_to_tasks);
        let sends = sends(&effects);
        assert_eq!(sends.len(), 1);
        let snapshot = &sends[0];
        assert_eq!(snapshot["type"], "task_snapshot");
        let tasks = snapshot["tasks"].as_array().expect("tasks 是数组");
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0]["taskId"], "new", "createdAt 大的在前");
        assert_eq!(tasks[1]["taskId"], "old");

        let keys: Vec<&str> = tasks[0]
            .as_object()
            .expect("摘要对象")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            vec![
                "taskId",
                "name",
                "status",
                "progress",
                "receivedBytes",
                "fileSize",
                "speed",
                "createdAt",
                "canPause",
                "canOpenFile",
                "canOpenFolder",
                "fileExt",
                "packName",
            ]
        );
        assert_eq!(tasks[0]["status"], "completed");
        assert_eq!(tasks[0]["fileExt"], "bin");
    }

    #[test]
    fn an_unchanged_snapshot_is_not_resent() {
        let mut world = World::new("dedupe");
        world.connect(1);
        world.hello(1);
        world.tasks = TaskService::from_records(vec![task_record("a", TaskStatus::Waiting, 1)]);
        world.send(1, &frame(json!({"type": "subscribe_tasks"})));

        assert!(world.hub.tasks_changed(&world.tasks).is_empty());
        // 换成两个订阅者：都没变化，一样一条不发。
        world.connect(2);
        world.hello(2);
        world.send(2, &frame(json!({"type": "subscribe_tasks"})));

        if let Some(record) = world.tasks.task_mut("a") {
            record.status = TaskStatus::Running;
        }
        let effects = world.hub.tasks_changed(&world.tasks);
        let recipients: Vec<SessionId> = effects
            .iter()
            .filter_map(|effect| match effect {
                SessionEffect::Send { session, .. } => Some(*session),
                _ => None,
            })
            .collect();
        assert_eq!(recipients, vec![1, 2], "两个订阅者都收到，按插入序");
    }

    #[test]
    fn only_subscribed_authenticated_sessions_get_snapshots() {
        let mut world = World::new("subs");
        world.tasks = TaskService::from_records(vec![task_record("a", TaskStatus::Waiting, 1)]);
        world.connect(1);
        world.hello(1);
        world.send(1, &frame(json!({"type": "subscribe_tasks"})));
        world.connect(2);
        world.hello(2);

        if let Some(record) = world.tasks.task_mut("a") {
            record.status = TaskStatus::Completed;
        }
        let effects = world.hub.tasks_changed(&world.tasks);
        let recipients: Vec<SessionId> = effects
            .iter()
            .filter_map(|effect| match effect {
                SessionEffect::Send { session, .. } => Some(*session),
                _ => None,
            })
            .collect();
        assert_eq!(recipients, vec![1]);
    }

    #[test]
    fn a_pending_snapshot_swallows_the_burst() {
        let mut world = World::new("burst");
        assert!(world.hub.note_tasks_changed());
        assert!(!world.hub.note_tasks_changed(), "置位期间的信号直接丢弃");
        assert!(world.hub.send_snapshots(&world.tasks).is_empty());
        assert!(world.hub.note_tasks_changed(), "发完复位");
    }

    // ── create_task ───────────────────────────────────────────────────────

    #[test]
    fn create_task_adds_the_task_and_reports_the_id() {
        let mut world = World::new("create");
        world.connect(1);
        world.hello(1);
        let path = world.dir.path().to_string_lossy().into_owned();
        let effects = world.send(
            1,
            &frame(json!({
                "type": "create_task",
                "requestId": "r1",
                "source": "resource",
                "payload": {
                    "url": "https://example.com/a.zip",
                    "filename": "a.zip",
                    "path": path,
                },
            })),
        );

        let sends = sends(&effects);
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0]["type"], "create_task_result");
        assert_eq!(sends[0]["requestId"], "r1");
        assert_eq!(sends[0]["status"], "created");
        assert_eq!(sends[0]["taskId"], "tsk_https://example.com/a.zip");
        assert_eq!(world.tasks.len(), 1);
        let record = world
            .tasks
            .task("tsk_https://example.com/a.zip")
            .expect("任务");
        assert_eq!(record.name, "a.zip");
        assert_eq!(record.output_folder, path);
        assert!(world.dir.path().join("a.zip").is_file(), "占位符建出来了");
    }

    #[test]
    fn a_draft_request_never_touches_the_task_list() {
        let mut world = World::new("draft");
        world.connect(1);
        world.hello(1);
        let effects = world.send(
            1,
            &frame(json!({
                "type": "create_task",
                "requestId": "r1",
                "payload": {"url": "https://example.com/a.zip", "filename": "a.zip"},
                "draft": true,
            })),
        );

        let sends = sends(&effects);
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0]["status"], "drafted");
        assert!(sends[0].get("taskId").is_none());
        let drafted: Vec<&Vec<TaskRecord>> = effects
            .iter()
            .filter_map(|effect| match effect {
                SessionEffect::TaskDraftRequested(records) => Some(records),
                _ => None,
            })
            .collect();
        assert_eq!(drafted.len(), 1);
        assert_eq!(drafted[0].len(), 1);
        assert_eq!(drafted[0][0].name, "a.zip");
        assert_eq!(world.tasks.len(), 0);
    }

    #[test]
    fn the_configured_default_decides_when_the_frame_says_nothing() {
        let mut world = World::new("defaultdraft");
        world.hub.set_should_draft(true);
        world.connect(1);
        world.hello(1);
        let sends = sends(&world.send(
            1,
            &frame(json!({
                "type": "create_task",
                "requestId": "r1",
                "payload": {"url": "https://example.com/a.zip", "filename": "a.zip"},
            })),
        ));
        assert_eq!(sends[0]["status"], "drafted");
        assert_eq!(world.tasks.len(), 0);
    }

    #[test]
    fn a_title_keeps_the_existing_suffix() {
        let mut world = World::new("title");
        world.connect(1);
        world.hello(1);

        world.send(
            1,
            &frame(json!({
                "type": "create_task",
                "requestId": "r1",
                "title": "My Video",
                "payload": {"url": "https://example.com/v1", "filename": "clip.mp4"},
            })),
        );
        world.send(
            1,
            &frame(json!({
                "type": "create_task",
                "requestId": "r2",
                "title": "clip.MP4",
                "payload": {"url": "https://example.com/v2", "filename": "clip.mp4"},
            })),
        );

        assert_eq!(
            world
                .tasks
                .task("tsk_https://example.com/v1")
                .expect("任务")
                .name,
            "My Video.mp4"
        );
        assert_eq!(
            world
                .tasks
                .task("tsk_https://example.com/v2")
                .expect("任务")
                .name,
            "clip.MP4",
            "标题已经带了同样的后缀（大小写不敏感）就不再拼一次"
        );
    }

    #[test]
    fn an_unknown_source_is_rejected() {
        let mut world = World::new("source");
        world.connect(1);
        world.hello(1);

        for source in ["bogus", ""] {
            let sends = sends(&world.send(
                1,
                &frame(json!({
                    "type": "create_task",
                    "requestId": "r1",
                    "source": source,
                    "payload": {"url": "https://example.com/a.zip"},
                })),
            ));
            assert_eq!(sends.len(), 1, "source = {source:?}");
            assert_eq!(sends[0]["message"], "未知的任务来源");
            assert_eq!(sends[0]["code"], "bad_request");
        }
    }

    #[test]
    fn a_create_task_without_a_payload_is_rejected() {
        let mut world = World::new("nopayload");
        world.connect(1);
        world.hello(1);
        let sends =
            sends(&world.send(1, &frame(json!({"type": "create_task", "requestId": "r1"}))));
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0]["message"], "无效的请求");
    }

    #[test]
    fn a_create_task_without_a_parser_match_is_rejected() {
        let mut world = World::new("nomatch");
        world.parsers = ParserChain::new();
        world.connect(1);
        world.hello(1);
        let sends = sends(&world.send(
            1,
            &frame(json!({
                "type": "create_task",
                "requestId": "r1",
                "payload": {"url": "https://example.com/a.zip"},
            })),
        ));
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0]["type"], "create_task_result");
        assert_eq!(sends[0]["status"], "rejected");
        assert_eq!(
            sends[0]["message"],
            "No parser matched: https://example.com/a.zip"
        );
        assert_eq!(world.tasks.len(), 0);
    }

    // ── task_action ───────────────────────────────────────────────────────

    fn action_frame(task_id: &str, action: &str) -> Vec<u8> {
        frame(json!({
            "type": "task_action",
            "requestId": "a1",
            "taskId": task_id,
            "action": action,
        }))
    }

    fn action_world(tag: &str, status: TaskStatus) -> World {
        let mut world = World::new(tag);
        world.tasks = TaskService::from_records(vec![record_in(world.dir.path(), "t1", status)]);
        world.connect(1);
        world.hello(1);
        world
    }

    #[test]
    fn toggling_a_completed_task_reports_a_failure() {
        let mut world = action_world("completed", TaskStatus::Completed);
        let sends = sends(&world.send(1, &action_frame("t1", "toggle_pause")));
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0]["type"], "task_action_result");
        assert_eq!(sends[0]["ok"], json!(false));
        assert_eq!(sends[0]["message"], "任务已完成");
        assert_eq!(
            world.tasks.task("t1").expect("任务").status,
            TaskStatus::Completed
        );
    }

    #[test]
    fn toggling_a_running_task_pauses_it() {
        let mut world = action_world("running", TaskStatus::Running);
        let sends = sends(&world.send(1, &action_frame("t1", "toggle_pause")));
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0]["ok"], json!(true));
        assert!(sends[0].get("message").is_none());
        assert_eq!(
            world.tasks.task("t1").expect("任务").status,
            TaskStatus::Paused
        );
    }

    #[test]
    fn toggling_a_paused_task_queues_it() {
        let mut world = action_world("paused", TaskStatus::Paused);
        let effects = world.send(1, &action_frame("t1", "toggle_pause"));
        let sends = sends(&effects);
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0]["ok"], json!(true));
        assert_eq!(
            world.tasks.task("t1").expect("任务").status,
            TaskStatus::Paused,
            "`pump` 只占运行槽（空 workId），置 `RUNNING` 归宿主派发"
        );
        assert_eq!(world.tasks.running_count(), 1, "已经占了运行槽");
        // Python `start` = `_schedule` = `queue.wait` + `_pump()`：派发出来的 id 交给宿主。
        assert!(
            has_effect(&effects, |effect| matches!(
                effect,
                SessionEffect::Dispatch { task_ids } if task_ids == &vec!["t1".to_string()]
            )),
            "pump 派发出来的 id 走 Dispatch"
        );
    }

    /// `cancel` 要宿主删文件、`remove` 不要；两者都产出 `_unwatchFile` + `taskRemoved`。
    ///
    /// `TaskService::delete` 只记账（第 99 步），`Task.deleteFiles` 由宿主按效果执行，
    /// 所以这里断言的是**效果**而不是磁盘上的文件——照抄 Python 的调用序列
    /// （`delete(task, shouldDeleteFiles=True/False)`）。
    #[test]
    fn cancel_asks_the_host_to_delete_files_and_remove_does_not() {
        let mut world = action_world("remove", TaskStatus::Completed);
        let effects = world.send(1, &action_frame("t1", "remove"));
        assert_eq!(sends(&effects)[0]["ok"], json!(true));
        assert_eq!(world.tasks.len(), 0);
        assert!(!has_effect(&effects, |effect| matches!(
            effect,
            SessionEffect::Task(TaskAction::DeleteFiles { .. })
        )));
        assert!(has_effect(&effects, |effect| matches!(
            effect,
            SessionEffect::Task(TaskAction::UnwatchFile { .. })
        )));
        assert!(has_effect(&effects, |effect| matches!(
            effect,
            SessionEffect::Task(TaskAction::Removed { task_id }) if task_id == "t1"
        )));

        let mut world = action_world("cancel", TaskStatus::Completed);
        let effects = world.send(1, &action_frame("t1", "cancel"));
        assert_eq!(sends(&effects)[0]["ok"], json!(true));
        assert_eq!(world.tasks.len(), 0);
        assert!(
            has_effect(&effects, |effect| matches!(
                effect,
                SessionEffect::Task(TaskAction::DeleteFiles { task_id }) if task_id == "t1"
            )),
            "cancel 要把删文件交给宿主"
        );
    }

    #[test]
    fn redownloading_resets_the_task() {
        let mut world = action_world("redownload", TaskStatus::Completed);
        let sends = sends(&world.send(1, &action_frame("t1", "redownload")));
        assert_eq!(sends[0]["ok"], json!(true));
        assert_eq!(
            world.tasks.task("t1").expect("任务").status,
            TaskStatus::Waiting
        );
    }

    #[test]
    fn opening_a_missing_file_reports_that_it_is_not_ready() {
        let mut world = action_world("openfile", TaskStatus::Completed);
        let sends = sends(&world.send(1, &action_frame("t1", "open_file")));
        assert_eq!(sends[0]["ok"], json!(false));
        assert_eq!(sends[0]["message"], "文件尚未生成");
    }

    #[test]
    fn opening_an_existing_file_produces_the_effect() {
        let mut world = action_world("openok", TaskStatus::Completed);
        let path = world.dir.path().join("t1.bin");
        std::fs::write(&path, b"x").expect("写文件");
        let effects = world.send(1, &action_frame("t1", "open_file"));
        assert!(effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::OpenFile { path: opened } if opened == &path.to_string_lossy()
        )));
        let sends = sends(&effects);
        assert_eq!(sends[0]["ok"], json!(true));
    }

    #[test]
    fn opening_the_folder_reveals_the_file() {
        let mut world = action_world("folder", TaskStatus::Completed);
        let path = world.dir.path().join("t1.bin");
        std::fs::write(&path, b"x").expect("写文件");
        let effects = world.send(1, &action_frame("t1", "open_folder"));
        assert!(effects.iter().any(|effect| matches!(
            effect,
            SessionEffect::RevealInFolder { path: revealed } if revealed == &path.to_string_lossy()
        )));
        assert_eq!(sends(&effects)[0]["ok"], json!(true));
    }

    #[test]
    fn opening_the_folder_of_a_missing_directory_fails() {
        let mut world = World::new("nodir");
        let mut record = task_record("t1", TaskStatus::Completed, 0);
        record.output_folder = world.dir.path().join("gone").to_string_lossy().into_owned();
        world.tasks = TaskService::from_records(vec![record]);
        world.connect(1);
        world.hello(1);

        let sends = sends(&world.send(1, &action_frame("t1", "open_folder")));
        assert_eq!(sends[0]["ok"], json!(false));
        assert_eq!(sends[0]["message"], "目录不存在");
    }

    #[test]
    fn an_unknown_task_or_action_is_reported() {
        let mut world = action_world("unknown", TaskStatus::Waiting);

        let unknown_task = sends(&world.send(1, &action_frame("nope", "cancel")));
        assert_eq!(unknown_task[0]["ok"], json!(false));
        assert_eq!(unknown_task[0]["message"], "任务不存在");

        let unknown_action = sends(&world.send(1, &action_frame("t1", "bogus")));
        assert_eq!(unknown_action[0]["ok"], json!(false));
        assert_eq!(unknown_action[0]["message"], "不支持的操作");
    }

    #[test]
    fn a_task_action_without_a_request_id_is_a_bad_request() {
        let mut world = action_world("noreq", TaskStatus::Waiting);
        let sends = sends(&world.send(
            1,
            &frame(json!({"type": "task_action", "taskId": "t1", "action": "cancel"})),
        ));
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0]["type"], "error");
        assert_eq!(sends[0]["message"], "缺少 requestId");
        assert_eq!(sends[0]["code"], "bad_request");
        assert_eq!(world.tasks.len(), 1, "任务没被删掉");
    }
}
