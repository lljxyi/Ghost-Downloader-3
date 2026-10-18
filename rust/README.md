# Ghost Downloader Rust 内核

Python 实现的 Rust 迁移，按阶段推进。

**已完成**：Phase 1（领域模型 + 持久化）、Phase 2（PyO3 影子模式）、Phase 3（HTTP 传输引擎 + 接入 pack + wreq 指纹）、Phase 4（回环监听 + aria2 JSON-RPC + 浏览器扩展 WebSocket v2，均已接入 Python）、Phase 5–6（26 个 `GD3_RUST_*` 影子开关，全部默认关，扫尾评估确认无新下沉项）、Phase 7（`gd3-ui`：`app/view` 的语义镜像层 + 6 个 Slint 文件）。

## 结构

| crate | 内容 |
|---|---|
| `crates/gd3-core` | 领域模型（TaskRecord / TaskStep / TaskFile / TaskError）、`tasks.jsonl`、`.ghd` 分片记录、fMP4/WebM 字节区间数学 |
| `crates/gd3-net` | HTTP 分片传输引擎：分片划分与续传、稀疏写入、重试与降级、限流钩子 |
| `crates/gd3-servers` | 回环端口监听（四态）、HTTP/1.1 头解析、aria2 JSON-RPC（HTTP + WebSocket）、浏览器扩展的 WebSocket 握手与帧 |
| `crates/gd3-py` | PyO3 桥，供尚未迁移的 Python 代码调用 |

## 构建与测试

需要 Rust 稳定工具链 + MSVC 链接器（VS Build Tools 的 VC++ 与 Windows SDK），
外加三个构建前置：**cmake** 与 **nasm**（`btls-sys` 要现编 BoringSSL）、
**libclang**（bindgen 要它解析 BoringSSL 头文件）。缺任一个都会在 `btls-sys` 处失败。

`rust/setup-env.ps1` 负责把这三样摆到 PATH / `LIBCLANG_PATH` 上，缺什么会直接说清楚：

```powershell
. .\rust\setup-env.ps1                              # 点源，环境留在当前会话
cargo test -p gd3-core -p gd3-net -p gd3-servers    # 领域逻辑、金文件对比、引擎与协议集成测试
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

它默认从 `~/.local/tools/{cmake,nasm}` 找便携版（`-ToolsRoot` 可改），
`libclang` 与 `PYO3_PYTHON` 取自 `.venv`。`crates/gd3-py` 是 cdylib，
`cargo test` 不覆盖它，用 maturin 构建（见下）。

## 影子模式

Python 仍拥有领域对象与全部领域决策，只把与领域无关的部分交给 Rust。
`crates/gd3-py` 是 cdylib，`cargo test` 不覆盖它，用 maturin 构建：

```powershell
uvx maturin develop --uv --manifest-path rust/crates/gd3-py/Cargo.toml
$env:GD3_RUST_PERSIST = "1"   # tasks.jsonl 交给 Rust
$env:GD3_RUST_HTTP = "1"      # HTTP Step 传输交给 Rust
$env:GD3_RUST_ARIA2 = "1"     # aria2 监听器交给 Rust
$env:GD3_RUST_BROWSER = "1"   # 浏览器扩展监听器交给 Rust
$env:GD3_RUST_FILENAME = "1"  # 文件名净化与拆分交给 Rust
$env:GD3_RUST_PROBE = "1"     # 远程文件探测（name/size/range）交给 Rust
$env:GD3_RUST_HF_TREE = "1"   # HuggingFace 仓库树枚举交给 Rust
uv run pytest -q
```

不设环境变量时走纯 Python 路径。五个开关各自只有一个接入点：
`app/services/rust_store.py`、`features/http_pack/rust_engine.py`、
`app/services/rust_aria2.py`、`app/services/rust_browser.py`、`app/platform/rust_filesystem.py`。

五个开关全开与全关的全量套件结果逐条一致（失败/错误集合相同）。Rust 模式下多出的 3 个 skip 是
`tests/test_http_subworker.py` 里打桩 Python 内部实现的白盒测试（ENOSPC、停滞恢复、死连接重试），
它们测的是 Python 实现本身。

### 从 Rust 线程回调 Python 的两个坑

1. **派发必须线程安全**：`on_state` / `on_disconnected` 这类回调来自 tokio 工作线程，
   适配器统一经 `coroutineRunner.post` 搬回主线程（应用里是 `QTimer.singleShot`）。
   测试里因此不能用 `loop.call_soon`，要用 `loop.call_soon_threadsafe`。
2. **析构 pyclass 前要放掉 GIL**：`Runtime::drop` 会 join 工作线程，而工作线程可能正卡在
   `Python::attach` 上等 GIL——拿着 GIL 等它们退出就是死锁（实测：浏览器监听器停掉后进程
   不退出）。两个 listener 的 `Drop` 都用 `Python::try_attach(|py| py.detach(...))` 关运行时。

## 兼容性契约

- 磁盘格式（`tasks.jsonl`、`.ghd`、`UserConfig.json`）与线上协议**保持不变**，两侧实现可互相读写，随时可切回。
- `tests/fixtures/` 下的金文件由 Python 实现生成（`generate_*.py`），Rust 侧只读，用于逐字节对比；规格漂移会立刻红灯。
- 规格来源：`tests/test_container.py`、`tests/test_http_subworker.py`、`app/models/task.py`、`app/services/task_service.py`、`features/http_pack/task.py`。
- 领域词汇以仓库根 `CONTEXT.md` 为准。Rust 侧函数与字段用 snake_case，键名靠 `#[serde(rename_all = "camelCase")]` 对齐 Python。

## 接入点

两个监听器的开关都在 `app/engine.py` 的 `createBrowserListener` / `createAria2Listener` 里，
`Engine.start()` / `stop()` 不用改。接线本身由 `tests/test_engine_listeners.py` 双模式覆盖：
它建一个真的 `Engine`，用应用里真实的 `BrowserService` / `Aria2RpcService` 各跑一次往返，
再整体停掉（pyclass 析构那条死锁就是靠这条路径暴露的）。

`gd3-servers` 另有 29 个独立测试，镜像 `tests/test_port_listener.py` 与 `tests/test_aria2_rpc.py`。

有意差别（写在对应测试旁）：

- `::1` 在本机被策略拒绝（WSAEACCES），IPv6 断言按环境门控——Python 基线里的同名测试在这里也失败；
- 关闭连接时 drop 套接字可能给对端 RST 而非 FIN，「已关闭」接受 0 字节或连接错误两种表现；
- 不走 body 的分支（OPTIONS / 404）先排空请求体再响应：Windows 上带着未读数据关闭套接字会发 RST，
  客户端会丢掉已收到的响应（这是移植时实测踩到的真问题）。

## TLS 指纹

引擎用 `wreq` + `wreq-util` 发请求，与 Python 侧同源。已验证（`https://tls.peet.ws/api/all`，profile=`chrome`）：

| 指标 | Python | Rust 引擎 |
|---|---|---|
| ja4 | `t13d1516h2_8daaf6152771_d8a2da3f94cd` | 同 |
| HTTP/2 (Akamai) | `1:65536;2:0;4:6291456;6:262144\|15663105\|0\|m,a,s,p` | 同 |
| User-Agent | `…Chrome/149.0.0.0 Safari/537.36` | 同 |

`profile=raw` 时不做模拟，指纹与上表不同（与 Python 的 `raw` 语义一致）。

构建前置：`cmake`（编译 BoringSSL）、`nasm`、`libclang`（bindgen）。缺少任一个都会在 `btls-sys` 处失败。

## 与 Python 的已知差异（Phase 3 之后要补）

- **身份覆盖**：wreq-util 3.0.0-rc.14 只有 chrome/firefox/safari/opera/okhttp 五个家族，**没有 Edge 与 Android 系列**；这类 profile 名会退回 Chrome 最新版（Python 侧有 Edge148 等）。`resolve_emulation` 里的版本表是手写的，升级 wreq-util 时要同步。
- **ja3 每次随机**：wreq 会打乱 TLS 扩展顺序，所以 ja3_hash 不可比；验证指纹要比 **ja4 + HTTP/2(Akamai) + UA**。
- **稀疏文件**：Python 预分配前会 `FSCTL_SET_SPARSE`；Rust 只 `set_len`，NTFS 上功能等价但未做该优化。
- **磁盘满**：错误码表已与 Python 的 `FATAL_IO_ERRNO` 逐码对齐并有测试，但没有端到端注入磁盘满的测试（Python 侧那条测试是打桩自己实现的）。

### 读超时：别用 `ClientBuilder::timeout`

Python 的 `buildClient(readTimeout=30)` 是**读阶段空闲超时**，对应 wreq 的 `read_timeout`
（每次读重置，见 `client/layer/timeout/body.rs`）。`ClientBuilder::timeout` 是**整请求超时**
（连接 → 响应体读完），拿它当读超时会掐死所有超过该时长的下载——这是移植时实测踩到的真问题，
`slow_but_progressing_body_survives_the_read_timeout` 就是盯这条的。

### 运行中重新切分与自动加速

一个分片收工时会去「偷」最慢那段的后半（Python `_reassignSubworker`），速度平稳时还会
主动多切几刀（`_autoSpeedUp`）。两条都在，规格对着 `tests/test_http_subworker.py` 的
`TestReassignment` 逐条移植：

- 算术在 `ranges::split_remaining`（运行前补齐与运行中切分共用一份）；
- 共享可变计划在 `plan.rs`：`Segment.end` 是原子的，因为正在收流的分片必须**立刻**看到被改小的末尾；
- 新分片不会立刻开跑，`SplitScheduler` 按 `SPLIT_START_INTERVAL`（0.2 秒）错开启动时刻；
- 门槛与开关来自 `cfg.maxReassignSize` / `cfg.autoSpeedUp`，由 `features/http_pack/rust_engine.py` 传给 `gd3_py.http_run`。

## Phase 7：UI 层（`gd3-ui`）

`crates/gd3-ui` 是 `app/view`（约 11 200 行 Qt 代码）的**语义镜像层**。它**不**替换 Qt：
Qt 仍是可运行的那一侧，Slint 侧目前是纯库（没有 `main`、没有事件循环），每个 Python
组件的「决策部分」被抽成可断言的纯函数与状态机。

判定标准不是像素一致，而是**语义一致**：配置键名、默认值、取值范围、格式化输出
（`toReadableSize` / `toDockSpeed` / 速度与 ETA 文案）必须逐条对上，`UserConfig.json`
两侧可互相读写。Qt 的实测值（窗口几何、字体测宽、暗色标志、剪贴板内容）一律当**入参**
传进函数，不在 Rust 侧重算——这样单测不需要 Qt，也不需要显示器。

| 阶段 | Python 源 | Rust 模块 |
|---|---|---|
| P0 主链路 | `windows/main_window.py`、`cards/task_cards.py`、`dialogs/task_drawer.py` | `window.rs`、`card.rs`、`drawer.rs`、`row.rs`、`facts.rs`、`actions.rs`、`filter.rs`、`sort.rs`、`speed.rs` |
| P1 设置页 | `pages/setting_page.py`、`components/setting_cards.py`、`setting_card_group.py`、`card_groups.py` | `settings_group.rs`、`settings_spec.rs`、`setting_cards.rs`、`setting_card_group.rs`、`card_groups.rs`、`option_cards.rs`、`category.rs`、`category_settings.rs`、`close_mode.rs`、`proxy.rs`、`profile.rs`、`identity.rs`、`headers.rs`、`menu_view.rs`、`card_menu.rs` |
| P2 建任务 | `dialogs/task_draft.py`、`plan_task.py`、`batch_url.py`、`file_select.py`、`name_conflict.py` | `draft.rs`、`plan.rs`、`batch_url.rs`、`file_select.rs`、`name_conflict.rs`、`conflict.rs` |
| P3 其它对话框 | `edit_task.py`、`pack_info.py`、`subtitle_select.py`、`extension_install.py`、`release_info.py` | 同名模块 |
| P4 组件库 | `components/` 下 12 个文件 | `range_slider.rs`、`track_bar.rs`、`band_selector.rs`、`bootstrap_list.rs`、`markdown_viewer.rs`、`token_line_edit.rs`、`labels.rs`、`editors.rs`、`progress_toast.rs`、`install_button.rs`、`tree_view.rs`、`banners.rs`、`scroll_area.rs` |
| P5 首次启动 | `windows/oobe_window.py` | `oobe.rs` |
| P6 平台外壳 | `shell/tray.py`、`dock.py`、`mac_status_item.py` | `shell.rs` |
| P7 补漏 | `shell/clipboard_listener.py`、`cards/draft_cards.py` | `clipboard.rs`、`draft_card.rs` |

规模：**59 个模块**（58 个 `pub` + 私有 `format`）/ **36 838 行 `src/`**（含 `lib.rs` 3 491 行）、
**6 个 Slint 文件 2 858 行**、**653 个单测**（`cargo test -p gd3-ui --lib`）。

`app/view` 已逐文件对照：`qfw_patch.py`(75) 是纯 Qt monkeypatch（改 `qfluentwidgets` 内部行为），
无业务语义可镜像，判定 **N/A**；`clipboard_listener.py` / `draft_cards.py` 由
`clipboard.rs` / `draft_card.rs` 覆盖（P7 补漏）。

Slint 文件在 `crates/gd3-ui/ui/`：`app_shell.slint`（壳）、`appwindow.slint`（主窗口）、
`task_page.slint`、`task_drawer.slint`、`settings_page.slint`、`card_menu.slint`。
无头冒烟测试在 `lib.rs` 的 `mod smoke`（`MinimalSoftwareWindow`），覆盖组件树实例化与
属性读写、任务行模型往返、`TaskRecord → 行` 的端到端推流。

```bash
cargo test -p gd3-ui                       # 库 + 冒烟
cargo test -p gd3-ui --lib                 # 只跑单测（653）
cargo clippy -p gd3-ui --all-targets -- -D warnings
```

已知偏差（每条都在模块头或测试旁标注）：

- **无译文层**：`toLocalizedError` / `QCoreApplication.translate` 的文本按中文原文硬编码，
  或由调用方当入参传入；
- **Qt 专有行为不模拟**：`QComboBox.currentText()` 用字符串承载、`nextCheckState` 的空实现
  用返回值表达、`QWidget` 的布局缓存用显式 `i32`（`0` = 未布局）承载、`QColor` 透明度用 `u8`；
- **Python 会抛异常的地方退化为缺省值**：`options.index()` 缺失、`size_hint` 高度为 0、
  非法 IPv6 括号、Markdown 缺片段标记——都在注释里写明与 Python 的差异；
- `FAMILY_VERSIONS` 是 `wreq::emulation::Emulation` 的成员快照，wreq 升级后要重新核对。

**尚未做**：引擎里的各服务实现（`BrowserService` / `Aria2RpcService` / `FeatureService` /
`RuntimeStatusService` 都还是桩，见下一节），托盘；`Ghost-Downloader-3.py`
的入口仍走 Qt。二进制入口见下一节。

## Phase 8：启动入口（`gd3-ui` 二进制）

`crates/gd3-ui/src/main.rs` 只有一行（转 `boot::main_entry`），启动序列在
`crates/gd3-ui/src/boot.rs`，对应仓库根的 `Ghost-Downloader-3.py`（268 行）：

```text
Paths（paths.py）→ load_config（qconfig.load）→ 单实例锁（instance.sock）
                  → Engine::new（装配）
                  → Engine::start（loadSaved + resumeSaved + 起监听器）
                  → 窗口 + push_settings + 任务行 → 事件循环
                  → 取交接消息 + 放锁 → Engine::stop（taskService.stop → flush → 收尾 → flush）
```

抢不到锁 = 已经有实例在跑：参数交接过去后以 **255** 退出，`Engine` 根本不会构造
（见「单实例锁」一节）。

- **路径语义**照抄 Qt 的「字符串拼接」而不是 `Path::join`：本机 `HOME` 为空时
  `QStandardPaths` 给 `HomeLocation='/'`、`GenericDataLocation='//.local/share'`、
  `DownloadLocation='//Downloads'`，双斜杠必须保留（`home_path_join` 有单测钉住）；
- **配置**：文件不存在 → 用 `settings::default_config_with_download_folder(DOWNLOAD_DIR)`，
  且**不写盘**（Python 的 `qconfig.load` 同样不写）；文件损坏 → 退回缺省并标记 `broken`；
- **事件循环**：有 `DISPLAY`/`WAYLAND_DISPLAY` 走 winit 后端（`window.run()`），
  否则走 `MinimalSoftwareWindow`，跑 `--frames` 帧后退出；
- **任务**：`Engine::start()` 读 `<APP_DATA_DIR>/tasks.jsonl`（坏行只计数），
  `resume_saved()` 把 `WAITING`/`RUNNING` 重新入队并把 `RUNNING` 回落到 `WAITING`；
  **只入队不派发**（Python 的 `_schedule` 会顺带 `_pump`，派发要真的起协程，等下载引擎迁过来）。
  启动**不回写** `tasks.jsonl`；退出时 `Engine::stop()` 把 `RUNNING`/`WAITING` 落成 `PAUSED`
  ——与 Python 一致（一次「启动完立刻退出」也会留下 `PAUSED`）。
- **CLI**：`--silence`（对应 Python 的 `--silence`）、`--version`、`--help` 之外，
  另有四个 Rust 侧可测性开关 `--headless` / `--frames` / `--data-dir` / `--tasks-file`
  （Python 没有）。

```bash
cargo run -p gd3-ui --bin gd3-ui -- --headless --frames 3 --data-dir /tmp/gd3-data
# Ghost Downloader v4.4.0 launched (headless, 3 frame(s), config=default, data=/tmp/gd3-data, tasks=0, resumed=0)
cargo run -p gd3-ui --bin gd3-ui -- --version   # v4.4.0（app/config/constants.py）
```

### 任务记账（`task_service.rs`）

`app/services/task_service.py` 的 `TaskService`（705 行）把「存储/队列记账 + 引擎交互 +
19 个信号」揉在一个类里。`task_service.rs` 只做**引擎无关**的那一半：

- 任务列表是 `Vec<TaskRecord>`（插入序 = Python `dict` 的迭代序），队列记账转调
  `gd3_core::queue::TaskQueue`；
- 信号退化成返回值：`pump(maxTaskNum)` 返回「该派发哪些 id」，
  `on_run_finished(id, maxTaskNum)` 返回 `RunOutcome { started, all_completed }`
  ——**`all_completed` 必须在派发之后算**，否则「刚做完一个、正好接上下一个等待任务」
  会被误判成全完成；
- `_flushSoon()` 的 200ms 防抖退化成 `pending_flush` 标记，由调用方 tick 后 `flush()`；
- `add()` 的磁盘检查走 `add_with_free_space(record, autoStart, free_space)`：`free_space`
  由调用方用 `disk_usage` 算，`None` 等价于 Python 的 `except OSError: pass`；
- **`autoStart` 即入队，与 `fileSize` 无关**——Python 的 `if task.fileSize > 0:` 只包住
  磁盘检查，`_schedule()` 在块外无条件调用（首版按「`fileSize > 0` 才入队」写错了）。

### 引擎装配（`engine.rs`）

`app/engine.py` 只有 74 行，没有任何业务逻辑：它按固定顺序把服务拼起来（外加唯一一条
信号连线 `taskService.nameConflicted → nameConflictQueue.add`），再按固定顺序
`start`/`stop`。`engine.rs` 把这份**装配契约**搬过来——服务内部还是桩，但「谁在谁之后、
谁连谁、停机按什么顺序收尾」由 `Engine::trace()`（`Vec<String>` 标签）逐条钉死：

```text
construct:CategoryService → construct:SpeedMeter → construct:TaskService
→ construct:NameConflictQueue → connect:taskService.nameConflicted -> nameConflictQueue.add
→ construct:RuntimeStatusService → construct:FeatureService → construct:BrowserService
→ construct:browserListener → construct:Aria2RpcService → construct:aria2Listener
```

- **真的搬了**：`TaskService`（第 81/82 步）、`SpeedMeter`（整类）、
  `NameConflictQueue`（复用 `name_conflict.rs`）、`CategoryService` 的 `_load`；
- **桩**：`RuntimeStatusService` / `FeatureService` / `BrowserService` / `Aria2RpcService`
  只保留装配点；监听器只保留后端开关（`GD3_RUST_BROWSER` / `GD3_RUST_ARIA2` == `"1"`
  → `RustBrowserListener`，否则 `PortListener`）与配置快照，套接字层在 `gd3-servers`；
- **配置是快照**：Python 的 `cfg.*` 是活对象（`PortListener` 连 `valueChanged`），
  Rust 侧还没有响应式层，所以 `nameConflict` / `maxTaskNum` 每次用时现读（与 `lambda`
  同构），监听器端口在构造时取快照；
- `probeConflict` 恒 `None`（占位文件未迁移）→ 冲突队列不会弹窗，按 `keepBoth` 放行。

### 单实例锁（`single_instance.rs`）

`app/platform/application.py::SingletonApplication` 是**两套机制叠加**：锁用
`QSharedMemory(DESKTOP_ID)`（`attach()` 成功 = 已有实例 → `sys.exit(-1)`），通道用
Windows `WM_COPYDATA` / Linux `org.freedesktop.Application.Open`+`Activate`。这里换成一个
**Unix domain socket**（`APP_DATA_DIR/instance.sock`）：`bind()` 成功即抢到锁，
`connect()` 成功即「已有实例」，同一条连接顺带把参数送过去——一个机制同时给出锁与通道，
不依赖 D-Bus、不依赖显示器，构建机上能测。

```text
gd3-ipc/1 activate\n              ← 只唤醒已有窗口（Activate / WM_USER_WAKE）
gd3-ipc/1 open\n<uri>\n<uri>\n    ← 把 URI 交给已有实例（Open / COPYDATA_OPEN_FILES）
```

- 第二个实例：`argv` 里有 wake URI → **一个字节都不发**（同 Python），否则有 URI 发
  `open`、没 URI 发 `activate`；然后以 **255** 退出（`sys.exit(-1)` 的 `-1 & 0xff`）；
- 主实例的收件箱由后台线程填，`Instance::take_handoffs()` 破坏性取出；消费方（打开链接 /
  激活窗口）还没迁过来，所以消息先攒着，事件循环结束后进 `RunSummary.handoffs`；
- **陈旧 socket**：`bind()` 报 `AddrInUse` 后先 `connect()` 探活，连不上（`ConnectionRefused`
  /`NotFound`）就删掉重试一次（进程被 `SIGKILL` 的残留）；
- 与 Python 的差异：锁在 `APP_DATA_DIR` 下（换 `--data-dir` 就换锁，Python 的
  `QSharedMemory` key 是全局的）、Windows 后端没实现、**不注册 D-Bus 服务名**、
  `SIGINT` 处理（`_onInterrupt`）没搬。

未迁移（后续小步）：D-Bus 服务名注册（`org.freedesktop.Application`）、
托盘 / keepalive / 通知、功能包加载、更新检查、OOBE 分支、翻译器。

## 平台辅助（`gd3-core::filesystem`）

`app/platform/filesystem.py` 的六个纯探测/换算函数已经搬完（`toSafeFilename` /
`splitStemExt` 早就在 `gd3-core::filename`）：

| Python | Rust | 要点 |
|---|---|---|
| `isExisting` | `is_existing` | `stat` 成功即真（跟随符号链接） |
| `isFolder` | `is_folder` | `S_ISDIR` |
| `toPosixPath` | `to_posix_path` | 先按 `PurePosixPath` 归一，再把 `\` 换成 `/` |
| `localFilePath` | `local_file_path` | 自带 `urlsplit` + `url2pathname`（`unquote`） |
| `findExecutable` | `find_executable` | 先看 `installFolder/`，再走 `shutil.which` 的 POSIX 分支 |
| `deletePath` | `delete_path` | 目录 `rmtree`、其余 `unlink`；指向目录的符号链接只删链接 |

搬过来的几个 CPython 细节（都有单测钉住）：

- `urlsplit` 只 `lstrip`，尾部空格留在 `path` 里；query / fragment 不进 `path`；
- `unquote` 按 `%` 切段、每段**只解前两个十六进制字符**（`%4142` → `A42`），解不出的
  `%` 原样保留；`+` **不**解成空格（那是 `unquote_plus`）；
- `shutil.which` 有目录部分时只查那一条，否则遍历 `PATH`（空段 = 当前目录），判据是
  `exists && access(X_OK) && !isdir` —— 所以 `gd3-core` 在 unix 上多了一个 `libc`
  依赖，只为 `access(2)`（std 没有对应物，自己看 mode 位在非 root 下会判错）；
- `str(Path(p))` 的归一（`posix_normpath`，加在 `gd3-core::path`）只去空组件与 `.`、
  压缩斜杠，**不消解 `..`**，恰好两个前导斜杠保留。

Python 侧**没有**加影子开关：这几个函数都是一次 syscall 的包装，两侧行为一致，也没有
性能动机；`localFilePath` 目前在 Python 里还没有调用方，等 `pack.py` 的
`BinaryRuntime` 搬过来再一起接。

## 插件清单与运行时（`gd3-core::pack` / `gd3-core::install`）

`app/models/pack.py` 里**不依赖 Qt** 的那半截，加上 `app/install.py` 的换文件逻辑：

| Python | Rust | 要点 |
|---|---|---|
| `FileType`（:22-27） | `pack::FileType` | 纯数据（后缀 / 展示名 / MIME / 图标） |
| `UriScheme`（:30-33） | `pack::UriScheme` | 纯数据 |
| `VersionInfo`（:114-117） | `pack::VersionInfo` | `detail` 默认空串 |
| `PackManifest.fromDir`（:186-247） | `pack::PackManifest::from_dir` | 失败 `None`；要日志文本用 `from_dir_detailed` |
| `BinaryRuntime`（:120-177） | `pack::BinaryRuntime`（trait） | 类属性 → 带默认实现的 trait 方法 |
| `deleteInstalled`（`install.py:15`） | `install::delete_installed` | 改名 `<name>.<8hex>.old`，再扫同名残留 |
| `installFile`（`install.py:21`） | `install::install_file` | `deleteInstalled` + `os.replace` |
| `installArchive`（`install.py:27-79`） | **未迁移** | 要 `zipfile` / `tarfile("r:gz")`，随 `update_service` 排期 |

`TaskParser` / `PackConfig` / `FeaturePack` / `PackPage` / `PackServices` 不搬：
`PackConfig.__init_subclass__` 把子类里的 `ConfigItem` 注册进 `cfg`（键名前缀
`pack_<类名>_`），`FeaturePack` 的方法返回 Qt 控件与信号——插件加载本身留在 Python，
这里只钉住**跨语言要共享的数据形状**。

`fromDir` 的七条校验（先到先错，`ManifestError` 的 `Display` 逐字照抄日志格式串）：

1. 缺 `manifest.toml`；2. 读 / UTF-8 解码 / TOML 解析失败；3. 顶层没有 `[pack]` 表；
4. `entry` 非字符串或 `strip()` 后为空（**默认 `"pack.py"`**）；5. 入口文件不存在
（只有 `.py` 才退到同名 `.pyc`，报错路径仍是原始的 `packDir/entry`）；6. `class`
非字符串或 `strip()` 后为空；7. `dependencies` 非数组，或含非字符串 / 空字符串。
`version` / `gdMinVersion` 非字符串时**静默**退化成 `""`。

几个容易写错的点（都有单测钉住）：

- `strip()` 只用于**判空**：拼路径用的是未 strip 的 `entry`，所以
  `entry = " pack.py "` 会去找带空格的文件名；`class` 也是原样保留（含前后空格）；
- `BinaryRuntime::runtime_id` 用 `std::any::type_name::<Self>()`，`where Self: Sized`
  让它不进 vtable，trait 仍然 object-safe（Python 那边是 `list[BinaryRuntime]`
  异构实例）；两边都是「类型全名」但**字符串不同**，不能混用作持久化键；
- `probe_version` 是同步 `Command`（Python 是 `async`）：起进程失败给 `Err`
  （Python 的 `FileNotFoundError` 冒到 `_onProbeFailed`），非 0 退出码给
  `Ok(VersionInfo::default())`；stdout 按 `errors="ignore"` 丢非法序列（**不**插
  `U+FFFD`），再取第一行 `strip()`；
- `delete_installed` 的残留扫描按「前缀 + 后缀 + 长度」判定，等价于
  `glob.escape(name) + ".*.old"`：`glob.escape` 只把 `[` / `]` / `*` / `?` 包成
  字符类，真通配符只剩那一个 `*`，而目录项不含 `/`；
- `path` 为空 / `/` / `.` 时 Python 的 `with_name` 抛 `ValueError`（整个函数中断），
  Rust 的 `file_name()` 是 `None` → 直接返回 `Ok(())`（记为已知差异）。

规模：`pack.rs` **1146 行 / 37 单测**、`install.rs` **286 行 / 12 单测**。
`pack.rs` 里另有一份私有的 `py_isspace` / `py_strip` / `split_lines` / `decode_ignore`
——`gd3-core::hls` 与 `gd3-ui::pystr` 已各有一份，抽公共模块会牵动既有调用点，留作后续。

## 插件加载与版本门禁（`gd3-core::loader` / `gd3-core::version`）

`app/loader.py` 里**不依赖 `importlib`** 的部分，加上 `app/update.py` 的版本比较：

| Python | Rust | 要点 |
|---|---|---|
| `parseVersion`（`update.py:11-27`） | `version::parse_version` | 第 62 步写在 `gd3-ui::setting_cards`，这里搬进 core 后改 `pub use` |
| `isNewer`（`update.py:30-31`） | `version::is_newer` | `parseVersion(latest) > parseVersion(current)` |
| `seedPacks`（`loader.py:17-41`） | `loader::seed_packs` | 返回 `SeedReport { synced }`，日志由调用方打 |
| `loadPacks`（`loader.py:44-68`） | `loader::scan_packs` + `loader::load_packs` | 只到「排序好的 manifest 列表」 |
| `orderedByDependency`（`loader.py:71-104`） | `loader::ordered_by_dependency` | 返回 `OrderReport { ordered, skipped }`，**不是** `Result` |
| `loadManifest`（`loader.py:107-140`） | **未迁移** | `importlib` + 动态子类，插件实例化留在 Python |

`loadPacks` 的最后一步是 `[pack for m in ordered if (pack := loadManifest(m, services))]`，
而 `loadManifest` 是「按文件路径加载 Python 模块 → `getattr(module, className)` →
`PackClass(services)`」。`FeaturePack` 本身就是 Python 类（返回 Qt 控件与信号），
Rust 侧没有对应概念，所以只搬到 manifest 排序为止，实例化留给 Python。

精确口径（都有单测钉住）：

- 目录扫描 = `sorted(iterdir())` 后跳过**非目录** / `.` 开头 / `_pending` 结尾；
- `gdMinVersion` 非空且 `parseVersion(app) < parseVersion(gdMinVersion)` → 进 `rejected`，
  不算错误（Python 只 log 一条 `跳过 FeaturePack {}：需要 GD ≥ {}，当前 {}`）；
- `featuresDir` 不存在 → `dir_missing = true`（Python 只 log 一条 warning，返回空列表）；
- 排序是 DFS 三色，**失败只影响顶层那个 manifest**：环 / 缺依赖 / 依赖已被跳过都只把
  **顶层名**记进 `skipped`，然后 `visiting.clear()` 继续下一个顶层项，`visited` 与
  `ordered` 里已完成的保持不动；
- `byName` 是 `{m.name: m}`（**同名后者胜**），`ordered` 里存的是 `byName[name]`
  而不是循环变量 —— 所以同名 manifest 只入列一次、且入列的是最后一个；
- 环的起点是 `visiting` 里该名字**首次**出现处，末尾再补一次自己
  （`a -> b -> c -> b` 报的是 `循环依赖: b -> c -> b`）；缺依赖报的 `{name}` 是
  **当前正在访问的节点**，不是顶层那个。

`seedPacks` 的四个易错点：

1. `seedDir` 不存在或 `seedDir.resolve() == targetDir.resolve()` → 在 `mkdir` **之前**
   返回，所以目标目录不会被创建；
2. 种子侧 manifest 解析失败或 `version` 为空 → 跳过（`_pending` 结尾的目录也在这里被滤掉）；
3. 用户侧已有同目录、manifest 合法、`version` 非空、且
   `parseVersion(user) >= parseVersion(seed)` → 保留用户版本（`1.0` 与 `1.0.0` 因为去尾零
   而**相等**，所以不会覆盖）；
4. 否则 `rmtree` + `copytree`，`copytree` 默认 `symlinks=False`：符号链接按目标内容拷。
   `copy_tree` 因此用 `Path::is_dir()`（跟随链接）+ `fs::copy`，并且 `dst` 必须不存在
   （`copytree` 先 `os.mkdir`，目标已存在报 `FileExistsError`）。

与 Python 的已知差异：

1. `Path.resolve()` 会解析符号链接。`resolve_path` 先用 `fs::canonicalize`（目标存在时
   等价），失败退回 `std::path::absolute` + 词法归一（只压 `.` / `..`）。只有「目标目录
   尚不存在**且**路径里带符号链接」这种组合才与 Python 不同；
2. `copytree` 会 `copystat` 保留目录 mtime / 权限，`copy_tree` 只保留文件权限位
   （`fs::copy` 自带），目录权限按 umask 新建；
3. 目录项排序：Python 按 `PurePath` 的 `_parts_normcase` 元组比较（POSIX 上等价于按名字
   比较码点），Rust 用 `to_string_lossy()` 后按 `String` 比较（等价于按 UTF-8 字节比较）；
   合法 UTF-8 下一致，非 UTF-8 名字会退化；
4. Python 的 `str.isdigit()` 对 Unicode 十进制数字为真，`int(ch)` 也接受它们 ——
   CPython 3.13 实测 `parseVersion("１.２")` 得 `(1, 2)`；Rust 的 `char::to_digit(10)`
   只认 ASCII，`parse_version("１.２")` 得 `()`。版本串实际只用 ASCII 数字。

规模：`loader.rs` **1029 行 / 37 单测**、`version.rs` **110 行 / 4 单测**。

## 文件校验（`gd3-core::checksum`）

`app/checksum.py` 只有 33 行，但它是「任务卡上的算法菜单」与「`matchChecksum` 挑算法」
的唯一实现，**必须真能算**，不能只搬纯逻辑：

| Python | Rust | 要点 |
|---|---|---|
| `parseChecksum`（:27-28） | `checksum::parse_checksum` | `(text.split() or [""])[0].lower()` |
| `matchChecksum`（:31-33） | `checksum::match_checksum` | 按**插入顺序**返回第一个命中的算法名 |
| `toChecksum`（:14-24） | `checksum::to_checksum` | 同步阻塞，`on_progress` 收 `done * 100 / size` |
| `COMMON_ALGORITHMS` / `CHUNK_SIZE` | 同名常量 | `("md5","sha1","sha256","sha512")` / `4 * 1024 * 1024` |
| `SHAKE_LENGTHS` | `SHAKE_LENGTHS` | 常量保留，但 `to_checksum` 对它们返回 `Unsupported` |
| `ALGORITHMS`（`sorted(hashlib.algorithms_available)`） | **未迁移** | 环境相关（本机 40 项），改用 `SUPPORTED_ALGORITHMS` |

四个易错点：

1. `str.split()` 的空白口径是 `str.isspace()`，**包含** `\x1c`-`\x1f`，而 Rust 的
   `char::is_whitespace` 不包含 → 用私有的 `py_isspace`；
2. `f.read(CHUNK_SIZE)` 走 `BufferedReader`，**除非 EOF 否则返回满块**；直接用
   `File::read` 的短读会让进度回调的次数与取值和 Python 不一致 → `read_chunk` 循环填满；
3. `size = path.stat().st_size` 在**开文件之前**取；空文件一块都读不到，所以
   `done * 100 // size` 根本不会执行（Python 侧也就不存在 `ZeroDivisionError`）；
4. `toChecksum` 在 Python 是 `async`（每块 `asyncio.to_thread`）；Rust 侧同步阻塞，
   取消靠调用方丢弃任务。

### 为什么手写 SHA-256 / SHA-512

本机离线（`static.crates.io` 403），`~/.cargo/registry/cache` 里有 `md-5` 与 `sha1`
（且已在 `Cargo.lock` 里），**没有 `sha2`**。`sha256` 是下载校验最常用的算法，缺位等于
功能回归，所以按 FIPS 180-4 实现了这两个摘要：全部常量（K 表 / 初始状态）由 Python
按定义现算生成，避免手抄；向量对着 `hashlib` 钉住（空串、`abc`、`hello world\n`、
55/56/63/64/111/112/127/128 字节填充边界、4MiB+7 跨块大文件）。

**`sha2` crate 可用后应整体替换掉 `checksum.rs` 里的 `mod sha256` / `mod sha512`。**

`shake_128` / `shake_256` 是 Keccak，代码量与风险都远高于 SHA-2，本轮不实现：
`to_checksum` 对它们返回 `ChecksumError::Unsupported`（`Display` 照抄 CPython 的
`unsupported hash type xxx`）。Python 侧能算，这是已知差异。

其它已知差异：`hashlib.new` 的算法名大小写不敏感，Rust 侧只认小写四个（Python 调用点
传进来的本来也都是 `ALGORITHMS` 里的小写名）；算法菜单在 Rust 侧将只有
`COMMON_ALGORITHMS` 那四项，没有「更多」子菜单。

规模：`checksum.rs` **914 行 / 10 单测**（其中 4 条是 `hashlib` 向量，2 条是
SHA-2 增量一致性）。

## Linux 桌面集成（`gd3-core::file_association` / `gd3-core::run_at_login`）

`app/platform/file_association.py`（148 行）与 `app/platform/run_at_login.py`（79 行）
都是「按平台分三支」的文件。可搬的只有 Linux 两支 + macOS 的 plist 文本：

| Python | Rust | 要点 |
|---|---|---|
| `loadMimeTypes`（:19-30） | `file_association::load_mime_types` | 只看**第一行** `MimeType=`，按 `;` 切、丢空段 |
| `saveMimeTypes`（:33-66） | `file_association::save_mime_types` | 写 / 删两份文件，**返回待执行命令** |
| `_registerLinux`（:69-79） | `file_association::register_file_types` | 只保留 `x-scheme-handler/*` + 各 `FileType.mime_type` |
| `registerUrlScheme`（:91-104） | `file_association::register_url_scheme` | 并上 `x-scheme-handler/<scheme>` |
| `unregisterUrlScheme`（:106-119） | `file_association::unregister_url_scheme` | 去掉它，其它保留 |
| `_setLinux`（:59-79） | `run_at_login::set_autostart` | `~/.config/autostart/<desktop_id>.desktop` |
| `_setMacOS`（:37-56） | `run_at_login::set_launch_agent` | `<home>/Library/LaunchAgents/<label>.plist` |
| `setRunAtLogin`（:10-16） | `run_at_login::set_run_at_login` | 按 `std::env::consts::OS` 分发 |
| `_setWindows`（:19-34，`winreg`） | `run_at_login::windows_run_value` | 只算注册表数据，**写注册表判 N/A** |

三个决策：

1. **`home` / `desktop_id` / `executable_path` / `version` 全部作参数传入。**
   `gd3-core` 不硬编码 `gd3-ui` 里的 `DESKTOP_ID` / `VERSION`，更不猜 `EXECUTABLE_PATH`
   （Python 那份是 `app/config/paths.py` 的运行时探测结果：`$APPIMAGE` → `sys.argv[0]`
   （frozen）→ `sys.executable`）。照第 87 步 `app_version` 的做法。
   macOS 的 plist 路径在 Python 里写死成 `/Users/{getpwuid(...).pw_name}/...`，
   Rust 侧换成 `home`（macOS 上两者相同，且可测）。
2. **`save_mime_types` 把 `subprocess.run` 拆成返回值 `Vec<Vec<String>>`。**
   `update-desktop-database <applications 目录>` 总排第一（即使 mimes 为空、文件已被删），
   之后每个 mime 一条 `xdg-mime default <desktop_id>.desktop <mime>`。Python 在
   `FileNotFoundError` 时 `break`，调用方按「`NotFound` 就停」复刻即可。
3. **`for mime in mimes` 用 `BTreeSet`（有序）而不是 Python 的 `set`（哈希序）。**
   `xdg-mime` 的调用顺序变了，落盘内容不变（`';'.join(sorted(mimes))` 本来就排序）。

四个易错点：

- `loadMimeTypes` 在循环里**直接 `return`**：只有第一个 `MimeType=` 行有效；
- `_registerLinux` 会**丢掉**已有 MIME 里所有非 `x-scheme-handler/*` 的项
  （`saveMimeTypes(schemes | {ft.mimeType for ft in fileTypes})`），这是 Python 原样行为，
  有单测钉住；
- 空集合时只 `unlink(missing_ok=True)` 两份文件、**不 `mkdir`、不删目录**，
  但 `update-desktop-database` 照样跑；
- `run_at_login` 禁用时同样只 `unlink`，`~/.config/autostart` / `LaunchAgents` 留着。

与 Python 的已知差异：

1. **Windows 注册表未迁移**（需要 `windows-sys` / `winreg` crate，本机离线不可用）。
   `set_run_at_login` 在 Windows 上返回 `io::ErrorKind::Unsupported` 而**不是静默成功**——
   让用户看得见失败，而不是以为设置生效了；`windows_run_value` 保留数据格式
   （`"<EXECUTABLE_PATH>" --silence`）+ 单测，等注册表 API 可用时接上；
2. Python 的 `sys.platform` 三分支没有 `else`，未知平台静默不做；Rust 侧同样返回 `Ok(())`；
3. 两个 Python 文件都**没有 `try/except`**（`file_association.py` 的
   `register` / `registerUrlScheme` 外层那层 `try/except Exception` + log 在调用方），
   所以这里一律返回 `io::Result`，日志由调用方打；
4. 落盘文本的字节数由 CPython 3.13 实测核对：Linux `.desktop` **217 字节**、
   macOS plist **398 字节**（`ExecutablePath = "/opt/ghost/GhostDownloader"`），
   两条断言逐字比较。

规模：`file_association.rs` **548 行 / 14 单测**、`run_at_login.rs` **415 行 / 14 单测**。

## 剩余工作（Phase 8）

**配置（`UserConfig.json`）和 Slint 入口要一起搬**。`app/config/cfg.py` 是
`qfluentwidgets.QConfig` 的子类：每一项都是 Qt 的 `ConfigItem`，设置页直接绑它的
`valueChanged`，而且**每次 `cfg.set()` 都会整文件回写**。只要 Qt 侧还在写这个文件，
Rust 侧再写同一个文件就会互相覆盖——所以 Rust 侧的 cfg 读写必须与「Slint 成为唯一 UI」
同一步落地，不能提前。同理，凡是「持久化进 cfg」的服务（`category_service` 的
`categoryRules` 等）都要排在它后面。
