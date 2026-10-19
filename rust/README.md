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

**尚未做**：`BrowserService` / `Aria2RpcService` 两个服务（还是桩，见下一节）；
托盘；`Ghost-Downloader-3.py` 的入口仍走 Qt。二进制入口见下一节。
（`RuntimeStatusService` 第 102 步搬完、第 107 步删掉空壳桩；`FeatureService` 的
解析器链第 108 步搬完，见「功能包内核」一节。）

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

### 分类匹配与输出目录（`category.rs`）

`app/services/category_service.py` 的匹配半边（第 95 步）。规则表与写路径在第 39/49 步
就已落地，这一步补的是「任务落到哪个目录」：

| Python | Rust | 口径 |
|---|---|---|
| `Path(name).suffixes` | `path_suffixes` / `normalized_suffixes` | 先去掉尾部分隔符再取 `name`；名字以 `.` 结尾 → 空；开头连续点全剥（`.bashrc` 无后缀、`.tar.gz` 只有 `gz`）；中间空段保留（`a..b` → `["", "b"]`） |
| `CategoryService.matchByName` | `match_by_name`（`file_select` 的同名函数是 `CategoryInfo` 投影版薄包装，内核同为 `match_by_name_in`） | 候选顺序「末两段拼接」→「末段」，逐个候选扫**全部分类**；`a.tar.gz` 先试 `tar.gz` 再试 `gz`，`a.mp3.mp4` 先试 `mp3.mp4` 再试 `mp4` |
| `categoryOf` | `category_of` | `files` 长度 > 1 → 空串（多文件任务不分类），否则按 `name` 猜 |
| `folderOf` | `folder_of` | 未知分类 / `folder` 为 `None` 或空串 → `None`；否则 `{default}` **全替换**成下载目录，返回裸字符串（不归一化） |
| `outputFolderOf` | `output_folder_of` | 关闭分类 → 原样；`category == None` 才按名猜；分类非空**且**输出目录 == 默认下载目录 → 换分类目录（`folderOf` 为 `None` 时保持原目录） |

`OutputFolder { category: Option<String>, folder: PathBuf }` 保留 Python 的 `None` / `""`
之别：`None` = 没指定（这时才会去猜），`Some("")` = 显式「不分类」（不再猜、也不换目录）。
「是不是默认下载目录」用 `Path` 相等判断，`/d/`、`/d/.`、`/d//` 都等价于 `/d`；
替换出来的目录过 `posix_normpath`（对应 Python `Path(str)` 的归一化），没替换时
逐字节保留入参。

`CategoryService`（第 95 步从 `engine.rs` 搬来，`engine` 仍 `pub use` 同一类型）
= 规则表 + `CategoryOutputSettings { is_category_enabled, download_folder }`
（`from_config` 读 `Category/EnableCategory` 与 `GeneralDownload/DownloadFolder`，
缺键按 `SPECS` 缺省）。`TaskService::add_with_conflict` 的**第一步**就是
`self.categories.output_folder_of(&record)` 并把 `category` / `output_folder` 写回，
与 `task_service.py:361` 同序。

已知差异：

- Python 的 `Engine` / `TaskService` 拿的是同一个 `CategoryService` 实例，改规则立刻
  全处可见；Rust 侧 `Engine::new` 传的是克隆，改完规则要 `Engine::push_categories()`
  （或 `TaskService::set_categories`）重新同步。`TaskService::default()` 的分类服务是
  「空规则 + 关闭」，所以没接线的调用方行为与之前完全一致；
- 前导 `//` 的路径：Rust 的 `Path` 压成单个根，Python `PurePosixPath("//d")` 保留双斜杠，
  这条差异没镜像（也不影响「同一目录」判定之外的行为）。

规模：`category.rs` **1180 行 / 14 单测**（第 95 步 +7）。

### 任务记账（`task_service.rs`）

`app/services/task_service.py` 的 `TaskService`（705 行）把「存储/队列记账 + 引擎交互 +
19 个信号」揉在一个类里。`task_service.rs` 只做**引擎无关**的那一半（引擎那一半在 `task_actions.rs`，第 100 步）：

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
- **重名冲突单独开一个入口**（第 93 步）：
  `add_with_conflict(record, choice, auto_start, free_space, delete_recoverably) ->
  Result<AddReport, CreateError>` 才是 Python `TaskService.add` 的完整口径——`mkdir -p`
  输出目录 → `create_placeholders` + `resolve_conflict` → 按 `replaced_tasks` 移除旧任务
  → `PlaceholderRegistry::set` → 复用 `schedule_add` 的磁盘检查与入队。
  `add()` / `add_with_free_space()` 保留「不建占位符、不碰磁盘」的旧语义，给启动恢复
  与测试用；
- `AddReport { name, replaced, overwrite_failed, outcome: Option<AddOutcome> }`：
  `outcome == None` 精确对应 Python 的 `nameConflicted` + `return False`（任务**没**加
  进去，等用户重新选），`replaced` 是 `taskRemoved` 要发的旧任务 id（按占位符顺序去重），
  `overwrite_failed` 是 `overwriteFailed` 信号的条件；
- **改名/移动占位符单独开一个入口**（第 94 步）：调用方先用 `task_mut(id)` 改好
  `output_folder` / `name`，再 `update_placeholders(id) -> Result<Option<UpdateReport>,
  CreateError>`。顺序对齐 `TaskStore.updatePlaceholders`（`task_service.py:91-121`）：
  路径没变直接返回 → `mkdir -p` 输出目录 → `resolve_conflict(KeepBoth, …)`（Python
  这里只 `except FileExistsError` 改名，既不查「未完成」也不进回收站）→ 名字或目录变了就
  把旧占位符与 `<path>.ghd` 逐个搬过去（先 `delete_path(新)` 再 `move_path`）→ 没搬走的
  旧占位符 `delete_placeholder` 清掉 → 重新登记 + 置 `pending_flush`。
  `UpdateReport { name, paths, created, moved, removed }` 把「实际动了什么」交给调用方
  （`name` 是最终名字，撞名时与传入的不同）；任务没有登记项时返回 `Ok(None)`
  （Python 是 `KeyError`）。
- **第一步是 `outputFolderOf`**（第 95 步）：`add_with_conflict()` 先算
  `self.categories.output_folder_of(&record)` 并把 `category` / `output_folder` 写回
  （分类功能开启且输出目录是默认下载目录时改道到分类目录），再做 `mkdir -p` /
  占位符 / 重名决策；`delete_recoverably` 由调用方注入（生产传
  `gd3_core::trash::delete_recoverably`，`Err` 按 `false` 处理）。
- **冲突探测**（第 96 步）：`probe_conflict(record) -> Option<ProbeTarget>` 是
  `TaskStore.probeConflict` 的镜像——先 `outputFolderOf` 拿到目录，再把每个占位符
  「相对 `outputFolder` 的部分」拼到该目录下，`sort()`（`Path` 逐组件比较）后返回
  **第一个**「`stat()` 成功且不属于未完成任务」的路径 + `is_folder` / `existing_size` /
  `modified_at` 四列。`probe_conflict_in(record, folder)` 是显式给目录的版本，
  `probe_conflict_by_id(id)` 给只拿得到 `taskId` 的调用方（冲突队列）。
  差异：四列来自同一次 `metadata`（Python 是 `isExisting` 一次 + 队列 `stat()` 一次）。
- **做种记账**（第 97 步）：`startSeeding` / `stopSeeding` / `_seed` / `_onSeeding*` 的
  状态机整体搬过来，`task.runSeeding()` 本体（libtorrent）仍是桩。`_seeding`
  （taskId → workId）同时充当 `Task.isSeeding`（Python 的非持久化字段）；
  `shouldSeed` 是持久化字段，所以 `startSeeding`/`stopSeeding` 置 `pending_flush`
  而 `_seed` 不置。信号同样退化成返回值：`seed` / `start_seeding` /
  `on_seeding_done` / `on_seeding_ended` 返回 `SeedingEvent::{Started,Stopped}`，
  `cancel_work` 返回 `CancelWork { work_id, stopped_seeding }`（协程取消归引擎）。
  `_onRunDone` 尾部的判定抽成 `should_start_seeding(id)`（`COMPLETED` + `canSeed` +
  `shouldSeed`），`resumeSaved` 的「已完成且文件还在 → 该做种」抽成
  `resume_seeding_candidates()`（`canSeed`/`hasOutputFile` 走 `facts::task_kind`，
  另加 `isExisting(outputPath)`）；`_refreshSpeedMeter` 退化成
  `speed_meter_should_run()` = `runningCount() > 0 || !_seeding.is_empty()`。
  **照抄了 Python 的一个缺陷**：`_cancelWork` 里
  `queue.workIdOf(id) or self._seeding.pop(id)` 的 `or` 会短路——队列里有 workId 时
  `_seeding` 里那条不会被摘掉（`isSeeding` 已清），残留项让速度表一直跑；
  单测 `cancel_work_prefers_the_queue_work_id` 钉住。
  `refresh()` 现在也把 `isSeeding` / `canSeed` / `hasOutputFile` 填成真实值
  （之前一律 `CardRuntime::default()`）。
- **校验记账 + `_rebalance`**（第 98 步）：`startChecksum` / `cancelChecksum` /
  `_onChecksum*` 与 `_rebalance` 的状态机搬过来；`toChecksum` 本体（手写 SHA-256/512）
  已在 `gd3-core::checksum`，由引擎调用。`_checksums`（taskId →
  `ChecksumWork { work_id, progress }`）是纯运行时表，`algorithm` 不落账
  （Python 的 `ChecksumWork` 也只有这两个字段，算法只用于引擎构造 `toChecksum`
  与 `_onChecksumDone` 的写回）。信号退化成返回值：`start_checksum` →
  `Option<ChecksumEvent>`（`None` = 任务不在），`cancel_checksum` → `Option<String>`
  （workId；`Some` 同时意味着调用方要发 `checksumStopped(task, None)`，没有登记项时
  Python 直接 `return`，既不取消协程也不发信号），`on_checksum_done` →
  `Option<ChecksumEvent::Completed>`，`on_checksum_failed` → `ChecksumEvent::Stopped
  { error }`（**不是 `Option`**：Python 无条件 `emit`）。
  `_rebalance` 抽成 `rebalance(max_task_num) -> RebalanceReport { cancelled, requeued,
  started }`：超出上限的**运行中**任务按 `running_ids()` 顺序 `cancel_work` + 放回等待
  队列，再 `_pump()` 一次。`cancelled` 用 `Vec<(String, CancelWork)>` 而不是并列两个
  vec，取消与放回等待是一对，拆开会丢失对应关系。`canPause` 走 `facts::can_pause`，
  不可暂停的任务原样留在运行中（Python 同样跳过）；**不置 `pending_flush`**——Python 的
  `setStatus(WAITING)` 不落盘，靠下一次真实改动写。
  差异：①`_onChecksumDone` / `_onChecksumFailed` 里的 `del self._checksums[taskId]`
  在 Python 缺项时抛 `KeyError`，Rust 退化成 `None`（不写 `checksums`、不置脏、
  不发信号）；②`onProgress` 闭包在 Python 里持有 `work` 对象，`cancelChecksum` 之后
  迟到的进度会写进孤儿对象（界面看不见），Rust 的 `set_checksum_progress` 只写登记表里
  的那条、返回 `bool` 说明是否真写了。
- **删除 / 重下 / 分类 / 批量**（第 99 步）：`delete` / `redownload` / `setCategory` /
  `startAll` / `pauseAll`，以及 `pause` 尾部的 `_pump()`。三个新报告类型
  `PauseReport { cancel, started }`、`DeleteReport { cancel, removed, checksum_work,
  delete_files, delete_denied, started }`、`RedownloadReport { checksum_work, cancel,
  status, started }`——信号照例退化成返回值，`started` 是尾部 `_pump()` 的派发结果
  （`taskStarted` 由调用方发）。`delete` 就地删占位符文件（`deletePlaceholder` +
  `<path>.ghd`，只删「空目录 / 零字节文件」，正在下载的内容不动），**已下载内容**
  留给调用方（`delete_files` 为真时执行 `Task.deleteFiles` 的语义，要等协程真停下——
  Python 放在 `finished` 回调里）；`_canDeleteIn` 只在 macOS 上可能为假（`read_dir`
  撞 `PermissionDenied`），非 macOS 恒真，所以 Linux 上 `delete_denied` 永远是 `false`。
  `redownload` 同步做完 `reset()` + `queue.wait` + `pump`（Python 在 `finished` 回调里，
  但 `_pump()` 只**返回**该派发的 id，顺序等价）。`setCategory` 只改 `category` + 置脏，
  **不重算 `outputFolder`、不动占位符**；卡片菜单的「未分类」传 `""`，落成 `Some("")`
  而不是 `None`（`outputFolderOf` 用 `is None` 判断要不要按名字猜分类）。
  `startAll` 只排 `PAUSED` / `WAITING` / `FAILED` 且**不改状态**（`_dispatch` 才
  `setStatus(RUNNING)`）；`pauseAll` 先快照 id 再遍历（`pause` 里的 `_pump` 会改队列），
  返回的 `started` 含「被 pump 派发又立刻被 pause」的中间任务（Python 同）。
  差异：`Task.deletePlaceholders` 里「未完成步骤的 `step.outputPath`」那一段仍没法镜像
  （`TaskStep` 基类在 Rust 侧没有 `outputPath`，那是 `HttpStep`/`yt_dlp` 等子类的属性），
  只做占位符文件那半边——与第 93 步 `placeholder_paths()` 只返回 `[outputPath]` 是
  同一个缺口。

### 动作层接线（`task_actions.rs`）

`actions.rs` 算「点一下该调哪个服务方法」，`task_service.rs` 做记账半边并把「要取消的
协程 / 要删的文件 / 该派发的 id」摊成报告——中间差一层：**谁把这些报告变成真的调用**。
`task_actions.rs` 就是这一层，判定标准是 Python 的**调用序列**与**信号发射顺序**：

```text
pause(task)        _cancelWork → setStatus(PAUSED) → _flushSoon → taskPaused → _pump
start(task)        (guard) → _schedule = queue.wait + _pump
startSeeding(task) shouldSeed=True → _flushSoon → _seed(isManual=True)
stopSeeding(task)  shouldSeed=False → _flushSoon → _cancelWork → _refreshSpeedMeter
delete(task, df)   cancelChecksum → _unwatchFile → [fileDeleteDenied] → _cancelWork
                   → onStopped{deleteFiles, deletePlaceholders} → _store.remove
                   → _flushSoon → taskRemoved → _pump
redownload(task)   cancelChecksum → _unwatchFile → _cancelWork
                   → onStopped{deleteFiles, reset, _flushSoon, _schedule}
setCategory(t, c)  task.category = c → _flushSoon
moveToFront(ids)   if queue.moveToFront(ids): queueChanged
startChecksum(t,a) _checksums[t]=work → submit(toChecksum(...)) → checksumStarted
cancelChecksum(t)  if work: cancel(workId) → checksumStopped(t, None)
edit(t, opts, nt)  cancelChecksum → needsDelete = nt and not canReuseProgress(nt)
                   → _cancelWork → onStopped{deleteFiles?, replaceWith?, setOptions,
                   updatePlaceholders, _flushSoon, _schedule}
updateSelection(t, idx)  [isRunningDeselected: _cancelWork] → apply{setSelection,
                   updatePlaceholders, [completedAt=0, _unwatchFile, _cancelWork],
                   _flushSoon} → [_schedule]
```

两个缺口与补法：

- **`&self` vs `&mut self`**：`actions::TaskService` 的方法收 `&self`（Slint 回调是单槽
  `Fn`，`TaskCommands` 用 `Rc<dyn actions::TaskService>` 持有），而 `TaskService` 的方法
  全要 `&mut self`。用 `TaskStore` 端口把「怎么拿到 `&mut TaskService`」抽象掉，三个
  `impl`：`RefCell<TaskService>`（UI 自持）、`RefCell<&mut TaskService>`（临时借引擎那份，
  `Engine::task_actions()` 走这条）、`Rc<RefCell<TaskService>>`（长期共享）；
- **没有协程运行时**：Python 每个动作都会 `coroutineRunner.submit/cancel`。Rust 侧退化成
  `TaskAction` 效果队列——`TaskActions` 只**登记**要做什么，宿主按序消费。13 个变体：
  `Started` / `CancelWork` / `Paused` / `Removed` / `DeleteDenied` / `DeleteFiles` /
  `DeleteFilesAt` / `UnwatchFile` / `SeedingStarted` / `SeedingStopped` /
  `ChecksumStarted` / `ChecksumStopped` / `QueueChanged`，每个都对应 Python 里的一行
  具体调用或一次信号。
  workId 由 `TaskActions` 自己发号（`run-1` / `seed-2` / `checksum-3`，一个全局计数器 +
  前缀区分类别），宿主只需把它当**不透明句柄**原样交回 `CancelWork`。

顺序就是 Python 的发射顺序（`taskPaused` 在 `_pump` 之前、`taskRemoved` 在
`taskStarted` 之前……）。唯一的语义差别是 `_cancelWork` 的 `finished=` 回调：Python 等
协程真的停下才跑 `onStopped`，这里把 `CancelWork` 排在 `DeleteFiles` 之前，**宿主必须
按序应用**（取消完成后再删文件），与第 99 步 `DeleteReport::delete_files` 的约定一致。

配套的两个记账原语（补在 `task_service.rs`）：

- `schedule(task_id, max_task_num) -> Vec<String>` = `_schedule` = `queue.wait` + `_pump`
  （`start` / `_onRunDone` 的做种分支 / `redownload` / `startAll` 都走它）；
- `dispatch(task_id, work_id) -> bool` = `_dispatch` 的记账半边 = `setStatus(RUNNING)` +
  `queue.run(id, workId)`，用来把 `pump()` 留下的**空 workId 占位**换成真 id；与
  `setStatus` 一样**不置 `pending_flush`**（Python 的 `_dispatch` 不 `_flushSoon`）。

另外 `pause` / `stop` / `resumeSaved` / `_rebalance` 改状态时统一走
`TaskRecord::set_status`：Python 的 `Task.setStatus` 会**连带改每个未完成 step 的状态**、
再由 `updateStatus()` 反推任务状态（步骤混着 `COMPLETED` 时甚至可能反推出 `WAITING`）。
直接赋 `record.status` 会漏掉这半边，`canPause`（读第一个 `RUNNING` 步骤）也就永远看到
旧值。

`edit` 与 `updateSelection` 是 `TaskActions` 的**固有方法**（不进 `actions::TaskService`
trait）：前者走 `featureService.createEditDialog`、后者走抽屉的文件列表，都不是卡片
动作分派。两者的 `finished=` 回调各带一句 `deleteFiles()`，而 `edit` 的那句在
`replaceWith(newTask)` **之前**——名字一换 `placeholderPaths` 就跟着换，宿主事后按
taskId 现算会删到新路径。所以 `edit` 的效果是 `DeleteFilesAt { task_id, paths }`
（`paths` 是改名前的快照），其余动作仍用 `DeleteFiles { task_id }`。

`updateSelection` 的三处口径（全部照抄 Python）：`wasCompleted` 在 `setSelection`
**之前**快照；「有选中未完成文件」看的是**新**选中集；`isRunningDeselected` 只看
**第一个** `RUNNING` step（`fileIndex is None` 判 `False`）。

还没接的：`actions::UiHooks`（剪贴板 / 抽屉 / 编辑对话框 / 打开文件）落到
`app.platform.desktop`，要等 Slint 窗口就位。

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

- **真的搬了**：`TaskService`（第 81/82 步）、`SpeedMeter`（整类在
  `speed_meter.rs`，第 104 步；由 `TaskService` 持有，第 105 步）、
  `NameConflictQueue`（复用 `name_conflict.rs`）、`CategoryService` 的 `_load`、
  `RuntimeStatusService`（第 102 步的 `runtime_status.rs`，第 107 步删掉空壳桩改为
  `pub use`）；
- **`FeatureService` 的解析器链是真的**（第 108 步，`feature_service.rs`）：`register`
  的稳定序、`parse` 的身份预设前置、`match` / `matchPassive`、`No parser matched`
  都在；`activate()` / `deactivate()` 在无 pack 时空转（Python 同）。**pack 本身搬不了**
  ——见「功能包内核」一节的结构性 N/A 表；
- **桩**：`BrowserService` / `Aria2RpcService` 只保留装配点。两者共用
  `featureService.parse` 这条端口（`browser_service.py:496` 与 `aria2_rpc.py:218`
  各调一次），所以它们能接上的前提是先有 pack；`aria2` 那一半的**应用侧**已在
  `aria2_actions.rs`（第 109 步，见「aria2 RPC 应用侧」一节），剩下的桩只负责装配。
  监听器只保留后端开关
  （`GD3_RUST_BROWSER` / `GD3_RUST_ARIA2` == `"1"` → `RustBrowserListener`，否则
  `PortListener`）与配置快照，套接字层在 `gd3-servers`；
- **配置是快照**：Python 的 `cfg.*` 是活对象（`PortListener` 连 `valueChanged`），
  Rust 侧还没有响应式层，所以 `nameConflict` / `maxTaskNum` 每次用时现读（与 `lambda`
  同构），监听器端口在构造时取快照；
- **冲突探测已接线**（第 96 步）：`TaskConflictActions::probe` 转调
  `TaskService::probe_conflict_by_id`（`TaskStore.probeConflict` +
  `categoryService.outputFolderOf`）。Python 的队列拿的是 `Task` 对象，Rust 侧只存
  `ConflictTask { task_id, file_size }`，所以按 id 回查——「同一实例」这条差异的
  又一例。占位符存在且不属于未完成任务时队列不再放行，等
  `Engine::apply_dialog_outcome` 收用户的选择。

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
| `shutil.move` | `move_path` | 先 `rename`；只有 `ErrorKind::CrossesDevices` 才回退成复制 + 删除 |

搬过来的几个 CPython 细节（都有单测钉住）：

- `urlsplit` 只 `lstrip`，尾部空格留在 `path` 里；query / fragment 不进 `path`；
- `unquote` 按 `%` 切段、每段**只解前两个十六进制字符**（`%4142` → `A42`），解不出的
  `%` 原样保留；`+` **不**解成空格（那是 `unquote_plus`）；
- `shutil.which` 有目录部分时只查那一条，否则遍历 `PATH`（空段 = 当前目录），判据是
  `exists && access(X_OK) && !isdir` —— 所以 `gd3-core` 在 unix 上多了一个 `libc`
  依赖，只为 `access(2)`（std 没有对应物，自己看 mode 位在非 root 下会判错）；
- `str(Path(p))` 的归一（`posix_normpath`，加在 `gd3-core::path`）只去空组件与 `.`、
  压缩斜杠，**不消解 `..`**，恰好两个前导斜杠保留。
- `move_path` 只对 `ErrorKind::CrossesDevices` 回退（`shutil.move` 是 `except OSError`
  全量回退），回退时目录递归复制 / 文件 `fs::copy`，**不保留 mtime**，符号链接按内容
  复制而不是重建链接——现有调用点只搬占位符与 `.ghd` 分片，这两条差异不影响行为。

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

## 桌面动作（`gd3-core::desktop`）

`app/platform/desktop.py`（405 行）里大半是 `QDesktopServices` / `QProcess` / `ctypes`
+ COM，能搬的只有「选哪条命令、参数是什么」这一层：

| Python | Rust | 要点 |
|---|---|---|
| `shutdown`（:145-153） | `desktop::shutdown_command` | `shutdown -h now` / `osascript … shut down` / `shutdown /s /t 0` |
| `restart`（:156-164） | `desktop::restart_command` | `shutdown -r now` / `osascript … restart` / `shutdown /r /t 0` |
| `sleep`（:167-175） | `desktop::sleep_command` | `systemctl suspend` / `pmset sleepnow` / `rundll32.exe powrprof.dll,SetSuspendState` |
| `openFile`（:121-123） | `desktop::open_file_command` | Linux `xdg-open`、macOS `open`；Windows 未迁移 |
| `openFolder`（:126-127） | `desktop::open_folder_command` | 同上 |
| `revealInFolder`（:130-142） | `desktop::reveal_in_folder` | 三分支：存在 → 打开父目录（macOS `open -R`）；父目录存在 → `openFolder(parent)`；都没有 → 不做 |
| `openChromiumUrl`（:178-234） | `desktop::open_chromium_url` | 按表序命中第一个，`chrome://` 换成该浏览器的 scheme |
| `Popen` / `QProcess.startDetached` | `desktop::spawn_detached` | `setsid(2)`（`pre_exec`）+ 三路 `Stdio::null()`，**不等**子进程 |
| `buildFontFamilies`（:15-45） | **未迁移** | 入参是 `QLocale` 的 `script()` / `territory()`，Slint 侧没有对应概念 |
| `deleteRecoverably`（:112-114） | `trash::delete_recoverably` | 见下一节「可恢复删除」 |
| `_sendForegroundInput` / `_revealInFolderShell` / `requestForeground` / `raiseWindow` / `startFileDrag` | **N/A** | 纯 Win32 `ctypes` + Qt 窗口 |

三个决策：

1. **`*_on(os, ...)` 纯函数 + 不带 `_on` 的包装。** 包装只做
   `std::env::consts::OS` 分发，所以三条平台的命令表都能在本机（Linux）被单测钉住，
   而不是靠 `cfg!` 分支——`cfg!` 在 CI 上只能覆盖一个平台。
2. **`openChromiumUrl` 的「浏览器装没装」用 `which: impl Fn(&str) -> bool` 注入。**
   单测传假 `which`，不依赖本机装了什么浏览器；`open_chromium_url` 用
   `filesystem::which`（新增，`shutil.which` 的纯 `PATH` 版）查真实 `PATH`。
   之所以不沿用 `find_executable(Path::new(""), name)`：那条路径会先试
   `""/<name>`，也就是**相对当前目录**的 `is_file()`，`shutil.which` 没这一步。
3. **`shutdown` / `restart` / `sleep` 返回 `Vec<String>` 而不是 `Option`。**
   Python 的 `match sys.platform` 只有 `win32` / `darwin` 两个具名分支，其余全部落进
   `case _`（POSIX 分支）——所以 `"freebsd13"`、`"openbsd7"` 甚至空串拿到的都是
   `shutdown -h now`，不是「不支持」。`openFile` / `openFolder` / `revealInFolder` /
   `openChromiumUrl` 的 `Option` 只用来表示**Windows 未迁移**（`None`）。

两个易错点：

- **`str(Path("foo").parent)` 是 `"."`**，而 Rust 的 `Path::new("foo").parent()` 是空路径
  （`display()` 出来是空串）；`Path::new("/").parent()` 是 `None`，Python 的
  `Path("/").parent` 还是 `Path("/")`。`revealInFolder` 的 Linux 分支把这个值直接塞进
  `xdg-open`，所以两种边界都用 `parent_display` 对齐了（有单测）；
- **`toBrowserUrl` 只剥一次 `chrome://`、大小写敏感**：`"chrome://"`（长度 9）换出来是
  `"{scheme}://"`；`"Chrome://x"` 原样返回。

与 Python 的已知差异：

1. **Windows 的 `openFile` / `openFolder` / `revealInFolder` / `openChromiumUrl` 未迁移**：
   Python 分别走 `QDesktopServices`（内部 `ShellExecute`）、`SHOpenFolderAndSelectItems`、
   `winreg` 查 `App Paths`，都要 Win32 API；三个电源命令是纯命令表，三平台都在；
2. `openFile` / `openFolder` 在 Linux 上直接写 `xdg-open`，而 Qt 在 `xdg-open` 缺失时会退回
   `gio open` / 桌面环境自己的 handler——这里 `spawn_detached` 返回 `NotFound`；
3. `openChromiumUrl` 只负责**挑**命令：Python 在 `Popen` 抛 `OSError` 时 `continue` 试下一个，
   这里把 `spawn_detached` 的 `Err` 交给调用方；
4. macOS 分支 Python 是 `subprocess.run(["open", "-a", app, url])` 看返回码（同步试装），
   这里统一抽象成 `which(app)`，不真去 `open -a` 试；
5. 与 Python 的 `Popen` 一样不等子进程，子进程退出后会短暂留一个僵尸直到父进程退出
   （未做 `SIGCHLD` 处理）。

规模：`desktop.rs` **604 行 / 14 单测**；`filesystem.rs` 新增 `which`（1 单测）。

## 可恢复删除（`gd3-core::trash`）

`app/platform/desktop.py:112-114` 只有两行，但它把整件事委托给了 Qt：

```python
def deleteRecoverably(path: str) -> bool:
    return QFile.moveToTrash(path)
```

所以要复刻的不是 Python，而是 **`QFile::moveToTrash`**。第 91 步用 PySide6 +
`QT_QPA_PLATFORM=offscreen` 真跑出全部口径（探针目录 `/root/gd3probe*`），
**没有一条是照 freedesktop 规范推测的**：

| 实测项 | 口径 |
|---|---|
| 回收站根 | `QStandardPaths::GenericDataLocation` + `/Trash`，即 `$XDG_DATA_HOME/Trash`（未设 → `$HOME/.local/share/Trash`） |
| `$XDG_DATA_HOME` 不存在 | 返回 `False`，**什么都不建**（不自动补父目录） |
| 目录结构 | `Trash/{files,info}`，三个目录都是 `0700` |
| `.trashinfo` | `0644`（umask 口径）；正文 `[Trash Info]\nPath=<编码>\nDeletionDate=<YYYY-MM-DDTHH:MM:SS>\n`，**本地时间、无时区** |
| `Path=`（home 回收站） | **原样**：传 `rel.txt` 就写 `rel.txt`，传 `sub/../rel2.txt` 就写原样 |
| `Path=`（顶层回收站） | **相对 `$topdir`**：`/tmp/a/b.txt` → `a/b.txt`（不带前导 `/`） |
| 百分号编码 | **保留** `A-Za-z0-9` `-` `.` `_` `~` 和 `/`，其余按 UTF-8 逐字节大写 `%XX`（`中` → `%E4%B8%AD`） |
| 冲突命名 | 第 0 次 `<name>`；第 i 次（i≥1）`<name>-<inode + i - 1>` |
| 源文件不存在 | 返回 `False`，但**回收站目录已被建好**（先建目录再 stat 源） |
| 符号链接 / 目录 | 链接按链接本身移入（不跟随）；目录整棵 `rename` |

顶层回收站（文件与 home 回收站不同设备）三分支，`/tmp` 上实测：

1. `$topdir/.Trash` 存在、是目录、**带 sticky 位**、**不是符号链接** →
   `$topdir/.Trash/<uid>/{files,info}`；
2. `.Trash` 存在但无 sticky → Qt 打 `Warning: '<dir>' doesn't have sticky bit set!`
   后退化成 `$topdir/.Trash-<uid>`；
3. `.Trash` 不存在 → 建 `$topdir/.Trash-<uid>`。

| Python | Rust |
|---|---|
| `deleteRecoverably`（:112-114） | `trash::delete_recoverably`（`bool` 口径） |
| `QFile.moveToTrash` | `trash::move_to_trash`（`io::Result<PathBuf>`，`Err` = `False`） |

辅助函数：`home_trash_dir` / `generic_data_dir` / `topdir_trash_dir` / `choose_trash_dir`
（纯路径选择）/ `resolve_trash_dir` → `TrashLocation` / `mount_point_of` / `shared_trash_ok` /
`encode_path` / `trash_info_text` / `trash_entry_name` / `current_uid` /
`deletion_date_at` / `deletion_date_now`。

三个决策：

1. **`TrashLocation { dir, topdir }` 而不是裸 `PathBuf`。** 因为 `Path=` 的口径两种回收站
   不一样（见上表），而「选哪个回收站」和「`Path=` 怎么写」必须由同一个判定结果驱动；
   拆开就会重算一遍设备/sticky，出现两处不一致。
2. **时间戳作参数传入**（`deletion_date: &str`），`deletion_date_now()` 只在
   `delete_recoverably` 里取一次。`localtime_r` 走 `libc`（`chrono` 不在离线缓存里），
   格式 `%Y-%m-%dT%H:%M:%S`。
3. **不做跨设备 `copy` 兜底。** 回收站目录本来就按「文件所在设备」选，`rename` 不会
   跨设备；Qt 在这里也只是失败回滚（`rename` 失败会把刚写的 `.trashinfo` 删掉）。

实测交叉验证（第 91 步，`/tmp` 是 tmpfs、`/root` 在 `/` 上，正好跨设备）：

```text
# Qt:      XDG_DATA_HOME=/root/gd3probe-py-xdg2, /tmp/gd3probe-py2/sub/deep/x y 中.txt
Path=gd3probe-py2/sub/deep/x%20y%20%E4%B8%AD.txt
# Rust:    /tmp/gd3probe-rust/f.txt
Path=gd3probe-rust/f.txt
# Qt（home 回收站，/root 上）:
Path=/root/gd3probe-py2/sub/y.txt
```

与 Python 的已知差异：

1. **Qt 在跨设备分支里会顺手建一个空的 `$XDG_DATA_HOME/Trash`**（内部流程先试 home
   回收站），这里不建——空目录对用户不可见，也没被规范要求；
2. `Path=` 里的非 UTF-8 字节：Python 走 `surrogateescape` 原样保留，Rust 走
   `to_string_lossy()`（替换成 U+FFFD）；
3. Qt 在无 sticky 的 `.Trash` 上打 `qWarning`，库侧没有 logger，分支选择由
   `shared_trash_ok` 的返回值暴露给调用方；
4. 冲突重试上限都是 100 次，但 Qt 之后**静默**返回 `False`，这里返回
   `AlreadyExists`（带候选名被占用的说明）。

规模：`trash.rs` **899 行 / 23 单测**。

## 占位符与重名冲突（`gd3-core::placeholder`）

`TaskStore.add`（`app/services/task_service.py:62`）在真正落盘前先「占坑」：给
`outputFolder/name` 建一个 0 字节文件（输出是目录时建目录），建不成就进冲突决策循环。

```python
task.outputFolder.mkdir(parents=True, exist_ok=True)
stem, ext = splitStemExt(task.name)
index = 0
while True:
    try:
        self._createPlaceholders(task)
        break
    except FileExistsError as error:
        takenPath = Path(error.filename)
    isUnfinished = self._isUnfinishedPlaceholder(takenPath)
    if not isUnfinished and choice == ASK:
        return None
    if not isUnfinished and choice == OVERWRITE:
        if self._deleteRecoverably(takenPath):
            continue
        isOverwriteFailed = True
        choice = KEEP_BOTH
    index += 1
    task.setName(f"{stem}({index}){ext}")
```

| Python | Rust |
|---|---|
| `NameConflictChoice`（`StrEnum`） | `NameConflictChoice` + `as_str` / `parse` |
| `Task.placeholderPaths` | `placeholder_paths`（只返回 `[folder/name]`，见差异 1） |
| `Path(f"{path}.ghd")` | `sidecar_path` |
| `deletePlaceholder`（`task.py:20`） | `delete_placeholder`（`io::Result`） |
| `_createPlaceholders` | `create_placeholders`（`Result<Vec<PathBuf>, CreateError>`） |
| `add` 的 `while True` 循环 | `resolve_conflict`（`Result<Option<ConflictOutcome>, CreateError>`） |
| `_placeholders` + `_taskByPlaceholder` | `PlaceholderRegistry` |
| `_isUnfinishedPlaceholder` | `PlaceholderRegistry::is_unfinished` |
| `add` 里的 `replacedTasks` 去重 | `PlaceholderRegistry::replaced_tasks` |
| `Task.outputPath` 算出来的路径 | `placeholder_paths_of(output_path)` |

冲突决策的五个分支，`resolve_conflict` 全部覆盖（都有单测钉住）：

| 情形 | 结果 |
|---|---|
| 占位符建成 | `Ok(Some{ name, overwrite_failed: false })` |
| `Ask` + 占位符属于已完成任务 | `Ok(None)` —— 调用方放弃本次添加 |
| `Overwrite` + 回收站移入成功 | `continue`：**同名**重试，`name` 不变 |
| `Overwrite` + 移入失败 | `overwrite_failed = true`，退化成 `KeepBoth` 改名 |
| 占位符属于未完成任务 | 不许覆盖、也不许 `Ask` 放弃，一律改名 |

三个决策：

1. **冲突决策做成「纯循环 + 闭包注入」。** `resolve_conflict` 不知道 `Task` 是什么，
   只通过 `create` / `is_unfinished` / `delete_recoverably` 三个闭包与外界交互。
   于是全部分支都能在 `gd3-core` 里被单测钉住，而 `TaskService` 的接线（含
   `tasks.jsonl` 落盘、`replacedTasks` 移除）留在 `gd3-ui`。
2. **`CreateError` 区分 `Taken` 与 `Io`。** Python 只有 `FileExistsError` 会被
   `except` 抓住继续决策，其余 `OSError`（父目录没了、ENOSPC、EACCES）直接冒泡把
   `add` 打断。用同一个 `TakenPath` 表达两者会让「权限不足」被误当成「重名」，
   在 `Overwrite` 分支里反复重试。
3. **`PlaceholderRegistry` 存 `taskId` 而不是任务对象。** Python 的
   `_taskByPlaceholder` 存 `Task`，`_isUnfinishedPlaceholder` 直接读 `task.status`；
   core 层不认识 `TaskRecord` 的生命周期，所以 `is_unfinished` 收一个
   `status_of: impl Fn(&str) -> Option<TaskStatus>`。

与 Python 的已知差异：

1. **`placeholder_paths` 只返回 `[outputFolder/name]`。** Python 的
   `Task.placeholderPaths` 还会遍历 `self.steps`，把落在 `outputFolder` 下的
   `step.outputPath` 追加进去；但 `TaskStep.outputPath` 基类返回 `""` 且全仓没有
   任何子类覆盖它（`grep` 只有 `app/models/task.py:105 class TaskStep`），所以那个
   循环永不追加。插件若自定义 step 覆盖 `outputPath`，这里要跟着扩。
2. **改名重试有上限**（`MAX_RENAME_ATTEMPTS = 10_000`）。Python 没有上限：文件名长到
   触发 `toSafeFilename` 的 200 字符截断时，`name(1).ext` 与 `name(2).ext` 截断后
   可能完全相同 → 死循环。到达上限后返回最后一次的 `Taken`。
3. `delete_placeholder` 返回 `io::Result` 而不是静默：Python 的 `deletePlaceholder`
   遇到 `OSError` 会抛，`_createPlaceholders` 的回滚循环里也会跟着中断。

接线（第 93 步）：`gd3_ui::task_service::TaskService::add_with_conflict` 把上面这些串成
Python `add` 的顺序——`mkdir -p` 输出目录 → `resolve_conflict`（`create` 闭包对每个候选
名字重算 `record.output_path_for(name)`，`is_unfinished` 闭包查 `PlaceholderRegistry` +
`TaskRecord.status`）→ `replaced_tasks` 逐个 `remove()` → `PlaceholderRegistry::set`。
`placeholder_paths_of` 是给「手上只有 `outputPath` 字符串」的调用方用的：`BilibiliTask`
多文件时 `outputPath` 取 `Path(name).stem`，改名重试必须整条路径重算。

规模：`placeholder.rs` **915 行 / 31 单测**；`task_service.rs` **3611 行 / 77 单测**
（其中 `add_with_conflict` 10 个、`update_placeholders` 6 个、分类接线 4 个、
做种 9 个、校验与 `_rebalance` 9 个、删除/重下/分类/批量 7 个、编辑与选择 5 个）；
`task_actions.rs` **1116 行 / 18 单测**（效果队列的顺序全部钉死）。

## 运行时状态（`gd3-ui::runtime_status`）

`app/services/runtime_status.py`（195 行）的移植。它把「某个运行时（ffmpeg / yt-dlp /
aria2 …）装没装、什么版本、有没有新版、正在装到几成」收在一处，设置页的 `RuntimeCard`
只读它的快照。Python 侧靠协程运行器：`submit(coro, done=…, failed=…)` 拿 `workId`，
`statusChanged.emit(status)` 通知 View。

Rust 没有协程运行时，沿用第 100/101 步的做法：**服务只算状态与意图，副作用摊成
`RuntimeEffect` 队列**，宿主按序消费：

| 效果 | 宿主该做什么 |
|---|---|
| `Emit(status)` | 更新 `RuntimeCard`（`setting_cards::runtime_view`） |
| `CancelWork(work_id)` | 取消对应工作（句柄由服务发号，宿主当不透明值） |
| `Run { work_id, runtime_id, kind }` | 去跑 `probe_version` / `fetch_latest_version` / `install`，再回调 `on_*` |

顺序与 Python 一致：`Emit` 总在 `Run` **之前**（`_probe` / `install` 都是先置状态再
`submit`），`CancelWork` 在最前（Python 先 `cancel(workId)` 再改状态）。

### 状态机（逐条对齐）

| Python | Rust | 备注 |
|---|---|---|
| `status(runtime)` | `status(&dyn BinaryRuntime)` | 没登记过就现造一个只带 id/name/path 的 |
| `refreshStatus` | `refresh_status` | 已知状态干净且路径没变 → **无效果**（不重复探测） |
| `invalidate` | `invalidate` | 先 `CancelWork` 再 `probe` |
| `install` | `install` | 已在装 → 无效果；版本号取当前状态的 `latestVersion`（可为 `""`） |
| `cancelInstall` | `cancel_install` | 没在装 → 无效果 |
| `_runInstall` 的 `reportProgress` | `report_progress` | 只在「正在装 + 整数进度变了」时 `Emit` |
| `_runInstall` 尾巴的 `isAppManaged()` 检查 | `check_installed` | 失败返回 `TaskError::install_not_found(name)` |
| `_onInstallFinished` | `on_install_finished` | 先摘 `workId` 再 `invalidate` → **不会**吐 `CancelWork` |
| `_onInstallFailed` | `on_install_failed` | 不在装 → 忽略（迟到的失败回调） |
| `_onProbeFinished` | `on_probe_finished` | `name` / `path` 是**探测时快照**；`isAppManaged()` 才继续 `fetchLatest` |
| `_onProbeFailed` | `on_probe_failed` | 记 `error`、丢掉持有对象 |
| `_onFetchFinished` | `on_fetch_finished` | 版本为空 → 不改状态；状态不存在 → 什么都不做 |
| `_onFetchFailed` | `on_fetch_failed` | **只清账**，不吐效果（Python 也只有两行 `pop`） |

### 关键决策

- **`workId` 由服务自己发号**（`runtime-1` / `runtime-2` …，一个全局计数器）：Python 是
  `coroutineRunner.submit` 的返回值，Rust 侧宿主就是运行器，服务替它发号才能记账
  （同第 100 步 `task_actions`：宿主把它当不透明句柄原样交回 `CancelWork`）。
- **`BinaryRuntime::runtime_id` 进 vtable**（去掉 `where Self: Sized`）：服务按
  `runtimeId` 记账，手上是 `Rc<dyn BinaryRuntime>`（Python 是 `dict[str, BinaryRuntime]`），
  带 `where Self: Sized` 就调不到。trait 仍然 object-safe。
- **`_runtimes` 用 `Rc<dyn BinaryRuntime>`**：Python 在异步间隙里攥着对象引用，
  `_onProbeFinished` 要问它 `isAppManaged()`、`_onInstallFinished` 要再 `invalidate`
  一次。`Rc` 是这一层唯一能表达「服务与调用方共享同一份对象」的写法。
- **`BinaryRuntime` 新增两个同步默认方法**：`fetch_latest_version`（缺省 `Ok("")`，
  对齐基类 `return ""`）、`install`（缺省 `Unsupported`，对齐 `raise NotImplementedError`）。
  两者在 Python 都是 `async`，Rust 同步阻塞，需要时由宿主丢线程。
- **`report_progress` 的 `percent as i32`**：Python 是 `int(percent)`（向零截断），对
  `NaN` 抛 `ValueError`；`as i32` 饱和到 `0`。实际无影响——进度由本仓自己的 `install`
  实现回报。
- **`to_card_status` / `card_view` 把本地化函数当参数**：`setting_cards::RuntimeStatus.error`
  是**已本地化文本**（Python `toLocalizedError(status.error)`），而 `gd3-ui` 只有错误键
  与参数、没有翻译表，所以翻译这一步由调用方注入。

规模：`runtime_status.rs` **932 行 / 22 单测**；
`gd3-core/src/pack.rs` 加两个默认方法 + `runtime_id` 进 vtable（**1180 行 / 38 单测**）；
`task_error.rs` 加 `install_not_found`。

## 任务草稿（`gd3-ui::task_draft` / `gd3-ui::sequence_matcher`）

`app/services/task_draft.py`（241 行）的移植：批量 URL 面板的状态机核心。每个 URL 对应
一个 `DraftItem`（正在解析 / 解析成功 / 解析失败 / 已确认），`setUrls` 对「旧 URL 列表
→ 新 URL 列表」做增量 diff，只对**变化的那几段**取消旧解析、提交新解析。

沿用第 100–102 步的形态：**服务只算状态与意图，副作用摊成 `DraftEffect` 队列**。

| 效果 | Python 信号 | 宿主该做什么 |
|---|---|---|
| `Parse { work_id, options }` | `coroutineRunner.submit(...)` | 跑 `featureService.parse(options)`，完成回调 `on_parsed` / `on_parse_failed` |
| `Cancel { work_id }` | `coroutineRunner.cancel(parseId)` | 取消对应解析（句柄由服务发号） |
| `Busy { busy }` | `parsingBusyChanged` | 批量面板的「解析中」指示 |
| `Succeeded { url, task }` | `parseSucceeded` | 更新该 URL 的草稿卡片 |
| `Failed { url, error }` | `parseFailed` | 显示错误 + 重试按钮 |
| `Changed` | `itemsChanged` | 重刷整个列表 |
| `Cleared` | `itemsCleared` | 清空列表 |
| `Confirmed { task, auto_start }` | `taskConfirmed` | 交给 `TaskService::add` |

### `setUrls` 的 diff（逐段对齐）

URL 先去重（保序，留第一次出现），再跑 `SequenceMatcher(旧, 新)`：

| opcode | 旧段 `previous[i1..i2]` | 新段 `urls[j1..j2]` |
|---|---|---|
| `equal` | 原样搬进新列表（**不重新解析**，已解析的 Task 也留着） | — |
| `replace` | 取消还在解析的 `parseId`，其余丢弃 | 每个 url 新建 item + 提交解析 |
| `delete` | 取消还在解析的 `parseId`，其余丢弃 | — |
| `insert` | — | 每个 url 新建 item + 提交解析 |

末尾固定发 `Busy` + `Changed`。

### `confirm` 的两遍循环

1. 有 `task` 的：`setOptions(buildOptions)` → `Confirmed`；
2. 还在解析的：把 `buildOptions` 与 `autoStart` 冻进 `confirmed_options` /
   `confirmed_auto_start`；
3. 第二遍把「在解析但没被确认」的取消掉（Python 的防御分支，正常流程走不到——
   `addParsedTasks` / `onParsed` 都会先把 `parseId` 清空）；
4. 清空 `items`：已确认但还在解析的 item 移进 `detached`，等回调；然后 `Busy` +
   `Cleared`。

### 关键决策

- **回调按 `parseId` 反查，而不是闭包捕获 item**：Python 把 item 塞进闭包，所以能改
  「已经从 `_items` 里摘掉」的那个对象；Rust 只能反查。而取消时 `parseId` 被清空，
  于是「取消后迟到的回调」自然查不到——与 Python 的 `if not item.parseId: return` 等价。
- **`detached` 显式保存游离项**：`confirm()` 之后还在解析的 item 无处安放（Python 靠
  闭包持有）。它们必然已 `confirmed_options.is_some()`，所以 `on_parsed` 命中时直接
  `Confirmed`，`on_parse_failed` 命中时直接丢弃（Python 只记一条 warning）。
- **`Parse` 携带原始 options 字典（含 `"url"`），不在这里转 `TaskOptions`**：本仓的
  `TaskOptions` 只建模 `outputFolder` / `category` 两键，提前转换会把 `headers` /
  `subworkerCount` 这些尚未建模的键丢掉，由端口自己按需取字段。
- **`submitErrors` 判 N/A**：`Parse` 只是入队，不会失败。`setUrls` 的「busy → 各
  submitError → changed」退化成「busy → changed」。
- **`work_id` 由服务发号**（`draft-1` / `draft-2` …，一个全局计数器），宿主当不透明句柄
  原样交回 `Cancel`。
- **`clear()` 不动 `detached`**：那里是**已确认**的任务，清草稿不该把它们的解析掐掉
  （Python 没有这个概念）。
- **`is_parsing()` 只看 `items`**：照抄 Python——`confirm()` 之后哪怕还有游离的解析在跑，
  `parsingBusyChanged` 也发 `false`。
- **`categoryOverride: Option<String>`**：`set_url_category(url, None)` 只是清掉覆盖，
  无法表达「把分类置成 None」——Python 同口径。
- **`addParsedTasks` 的 `tasks` 非空但全被跳过时仍发 `Busy` + `Changed`**：照抄 Python
  的两个 emit 在循环之后，不是「有变化才发」。

### `sequence_matcher`（`difflib` 的最小移植）

只覆盖 `TaskDraft.setUrls` 用到的那一种配置：`isjunk=None` + `autojunk=False`。这个前提
下 `__chain_b` 退化成「元素 → 下标列表」的普通 map，`find_longest_match` 里两条 junk
扩展循环是死代码，所以只保留三步：最长公共块、队列切分、相邻块合并，逐行对应 CPython
3.13 的 `/usr/lib/python3.13/difflib.py`。

- `matching_blocks` 末尾的哨兵 `Match { a: len, b: len, size: 0 }` 必须保留——opcodes
  靠它把「最后一个公共块之后的部分」也吐出来；
- `Match` 的 `Ord` 派生顺序即 Python 元组 `(a, b, size)` 的排序口径（`blocks.sort()` 与
  「相邻块合并」都依赖它）；
- `get_opcodes()` 不缓存（Python 的 `self.opcodes` 缓存没有复用场景）。

规模：`task_draft.rs` **1075 行 / 32 单测**；`sequence_matcher.rs` **387 行 / 11 单测**；
`gd3-core/src/task.rs` 加 `TaskOptions::from_options`（**513 行**）。

## 速度表（`gd3-ui::speed_meter`）

`app/services/speed_meter.py`（56 行）的移植。「全局下载速度」这一条数字：任务每收一块
数据就 `addSpeed(n)`，速度表每秒把累加到的字节数当成瞬时速度发布一次再清零——不做滑动
平均。它同时是**限速闸门**，任务在每个分片前问一次「现在超限了吗」。

| Python | Rust | 备注 |
|---|---|---|
| `start()` | `start() -> Vec<SpeedEffect>` | 已在跑 → 空；否则 `StartTicking{work_id}` |
| `stop()` | `stop() -> Vec<SpeedEffect>` | `[Cancel, Emit(0)]`，**无条件**发 0 |
| `addSpeed(n)` | `add_speed(n)` | 不看「在不在跑」 |
| `waitForSpeedLimit()` | `should_wait(enabled, limit)` | 谓词化 |
| `_tick()` | `tick() -> Vec<SpeedEffect>` | 先发布再清零；循环已停 → 空 |
| `_onTickFailed` | `on_tick_failed()` | 只置空 workId，不清 `bytes` / `current_speed` |

三条口径：

- **`_tickLoop` 摊成 `StartTicking{work_id}`**：宿主起 1 秒周期、每周期调 `tick()`，
  失败时调 `on_tick_failed()`。模块内不留计时器，单测不必等真实时间。
- **`waitForSpeedLimit` 退化成谓词**：`enabled` / `limit` 每轮由调用方从
  `settings::read_bool(cfg, "isSpeedLimitEnabled")` / `read_i64(cfg, "speedLimitation")`
  现读再传入，等价于 Python 的每轮重读 `cfg`，且模块不持有配置快照。
  判断是 `bytes > limit`（严格大于，等于上限不算超）。
- **字段全用 `Cell<T>`**：读侧（`current_speed` / `pending_bytes` / `is_running` /
  `should_wait`）只要 `&self`，写侧（`start` / `stop` / `add_speed` / `tick` /
  `on_tick_failed`）才要 `&mut self`。workId 由服务发号（`speed-1` / `speed-2` …）。

`on_tick_failed` **不清** `bytes` / `current_speed`（照抄 Python）：下一轮 `start()` 的第一次
`tick` 会把残留量发出去，这是既有行为，不修。

### 接线（`TaskService`）

Python 的 `_refreshSpeedMeter()` 是四个方法的尾语句，实读 `grep -n` 只有四处：
`stopSeeding`（:406）、`_seed`（:593）、`_onSeedingEnded`（:604）、`_pump`（:657）。
**`pause` / `resume` 里没有**——暂停时走 `_cancelWork` + `setStatus` + `_pump`，`_pump`
末尾那一发自然会算。

Rust 侧 `TaskService` 自己持有 `SpeedMeter`（Python 里是 `Engine` 构造、`:320` 注入），
四个调用点一一对应，另补三个宿主入口：`add_speed` / `tick_speed_meter` /
`on_speed_tick_failed`，以及谓词 `speed_meter_should_wait(enabled, limit)`。

**效果进缓冲而不是改四个返回类型**：那四个方法已经各有返回值（`CancelWork` /
`Option<SeedingEvent>` / `SeedingEvent` / `Vec<String>`），包一层报告结构会牵动
`engine.rs` 与既有轨迹断言，而语义收益为零。所以 `refresh_speed_meter()` 把
`start()` / `stop()` 的效果攒进 `speed_effects`，宿主在本轮动作结束后调
`take_speed_effects()` 一次取走——与 `pending_flush`（把 `_flushSoon()` 的 200ms 防抖
退化成标记）同一手法。四个调用点因此与 Python 一字不差，调用方不需要记得「pump 之后
还要再调一次」。

`stop()` 在没跑时不吐效果，所以「队列本来就空」的 `pump` 不会产生噪声。

### 接线（`Engine`）

Python 的 `Engine.stop()` 里也有一发 `self.speedMeter.stop()`（`engine.py:44`，在
`taskService.flush()` 之后、`browserListener.stop()` 之前）——它和 `TaskService` 拿的是
**同一个实例**，所以 Rust 侧速度表归 `TaskService` 所有，`Engine` 只转出口：

| Python | Rust |
|---|---|
| `engine.speedMeter` | `Engine::speed_meter()` → `TaskService::speed_meter()` |
| `engine.speedMeter.stop()` | `TaskService::stop_speed_meter()`（效果进缓冲） |
| `speedChanged` 的消费者 | `Engine::drain_speed_effects()` |
| 四个调用点里的 `_refreshSpeedMeter()` | 同上，效果由宿主取走 |

`Engine::drain_speed_effects()` 把 `TaskService::take_speed_effects()` 取到的效果逐条记进
装配轨迹（`speed:start:<workId>` / `speed:cancel:<workId>` / `speed:emit:<值>`），再把
`Vec<SpeedEffect>` 交回宿主——宿主据此起 1 秒周期、取消周期、刷数字；引擎测试据此钉住
「谁在什么时候起表」。

**只有 `Engine::pump()` 与 `Engine::stop()` 自动 drain**。另外三个调用点
（`stopSeeding` / `_seed` / `_onSeedingEnded`）走 `TaskActions`，宿主在那里
`take_speed_effects()`（第 100 步的效果队列口径）；没人取就一直攒，下次 `pump` / `stop`
一并吐出——顺序不变，只是延迟。

第 83 步曾在 `engine.rs` 里写过一份弱化的 `SpeedMeter`（`start() -> ()`、
`stop() -> Option<i64>`，没有 workId），第 106 步删掉，改用 `crate::speed_meter`：
同一份状态机不能有两份，否则「`Engine` 停表」与「`TaskService` 记账」会各说各话。

### 为什么不搬 `name_conflict_queue.py`

`app/services/name_conflict_queue.py`（73 行）的队列**第 65 步 b 就搬过了**，落在
`gd3-ui::name_conflict` 里：`NameConflictQueue::{add, set_choice, cancel, refresh}` +
`ConflictEvent::{RefreshPosted, ConflictChanged}` + `trait ConflictActions`（注入
`probeConflict` / `add`）。第 96 步把 `probeConflict` 接上之后，`engine.rs` 的
`on_name_conflicted` / `refresh_conflicts` / `apply_dialog_outcome` 三个入口全在跑；
`TaskConflictActions::add` 按 `task_id` 反查 `TaskRecord` 再 `tasks.add(record, true)`，
把「队列只拿得到 `ConflictTask`（两列）」这个缺口补掉了。

搬新模块前先 `grep -n 'pub struct <名字>' crates/*/src/*.rs`：进度笔记的标题按**Python
文件用途**起名（「重名冲突提示」），从标题看不出它把服务层队列一起搬了。

规模：`speed_meter.rs` **332 行 / 13 单测**；`task_service.rs` 3611 → **3864 行 / 86 单测**
（接线 + 9 条单测 + `stop_speed_meter`）；`engine.rs` 1195 → **1161 行 / 17 单测**
（删掉第 83 步那份弱化 `SpeedMeter` 与 2 条重复单测，补 `drain_speed_effects` 与 2 条
引擎级测试）。

## URL host 口径（`gd3-core::url`）

「从 URL 里抠 host」在 Python 侧就是 `urlparse(url).hostname or ""`，散落三处：
`feature_service.parse`（身份预设）、`http_pack`（加速站判断）、`github`（下载地址改写）。
第 108 步把共用的解析抽成 `gd3-core::url`（179 行 / 4 单测）：

```rust
pub fn host_of(url: &str) -> String;                 // 唯一 pub 项
pub(crate) struct ParsedUrl { scheme, host, path }
pub(crate) fn parse_url(url: &str) -> ParsedUrl;
```

`ParsedUrl` / `parse_url` 保持 `pub(crate)`：它们带着「path 被 lowercase」这个降级
（CPython 的 `urlsplit` 只 lowercase scheme 与 host），不该成为公共契约。

`hostname` 的边界按 CPython `_hostinfo` 实测复刻，**不是**「看起来合理」的写法：

| 输入 | `hostname` | 说明 |
|---|---|---|
| `https://host:notaport/x` | `host` | 端口在**第一个** `:` 处切掉，**不校验**端口是不是数字 |
| `https://host:8080/x` | `host` | 同上 |
| `https://[::1]:8080/x` | `::1` | IPv6 去掉方括号；没端口时也不带括号 |
| `https://user:pw@host/x` | `host` | userinfo 在**最后一个** `@` 处切 |
| `//host/x` | `host` | 无 scheme 也认 |
| `mailto:a@b.com` | `""` | scheme 不在 `uses_netloc` 里 → 没有 host |
| `ed2k://\|file\|x\|1\|h\|/` | `\|file\|x\|1\|h\|` | `_hostinfo` 不挑食，netloc 长什么样就返回什么样 |

**旧实现（原来 `github.rs` 里那份）用 `rfind(':')` + `is_ascii_digit` 判断端口**：
`https://host:notaport/x` 会得到 `host:notaport`，身份预设就漏命中。抽出来时一并修正；
`host_of_mirrors_urlparse_hostname` 里 20 组期望值都是 CPython 实取，不是手推。

## 身份预设命中（`gd3-ui::identity`）

`app/client.py::matchIdentityPreset(host)`（:83-94）是 `FeatureService.parse()` 的前置：
`options.clientProfile` 为空时，按 host 找一条预设，把 `clientProfile` / `userAgent` 补进
options。第 107 步把它搬成纯函数：

```rust
pub fn match_identity_preset<'a>(
    host: &str,
    presets: &'a [IdentityPreset],
) -> Option<&'a IdentityPreset>
```

三条口径（与 Python 逐条对齐）：

- **第一个命中就返回**，不是「最具体的那条」——`*.a.com` 排在 `x.a.com` 前面时，
  `x.a.com` 命中的是通配那条（Python 就是顺序遍历 + `return`）；
- `*.a.b` 同时匹配 `a.b`（裸后缀）与 `x.a.b`，但**不匹配** `xa.b`；
- `isEnabled` 为假整条跳过；空模式串只在 host 为空时命中（走 `elif host == pattern`）。

`presets` 由调用方给（`cfg.identityPresets` 的现值），模块不持有配置快照——与
`speed_meter::should_wait` 同一个口径。`host` 也由调用方从 URL 取：第 108 步之后用
`gd3_core::url::host_of(url)`（即 `urlparse(url).hostname or ""`，见上一节），
本函数不做 URL 解析。

## 功能包内核（`gd3-ui::feature_service`）

`app/services/feature_service.py`（约 240 行）里能搬的那部分：解析器链。597 行 / 15 单测。

```rust
pub trait TaskParser {
    fn priority(&self) -> i64 { 100 }                 // pack.py:42
    fn matches(&self, options: &Map<String, Value>) -> bool;
    fn matches_passive(&self, options: &Map<String, Value>) -> bool { self.matches(options) }
    fn parse(&self, options: &Map<String, Value>) -> Result<TaskRecord, TaskError>;
}

pub struct ParserChain { /* Vec<Box<dyn TaskParser>> */ }
pub struct FeatureService { parsers: ParserChain }
```

四条口径：

- **`register` 是稳定序**：插到第一个 `priority` 更大的解析器之前，等价于 Python
  `self._parsers.sort(key=lambda p: p.priority)`（`feature_service.py:61`）的稳定排序，
  但不必每次 O(n log n)；
- **`parse` 先做身份预设再匹配**（:77-89）：`clientProfile` 为空时
  `match_identity_preset(host_of(url))`，把预设里的 `clientProfile` / `userAgent` 填进
  options。`apply_identity_preset` 就地改 `&mut Map`（Python 的 `replace` 造新对象，
  但调用方拿到的本来就是副本，语义等价）；
- **无匹配 → `NoParserMatched { url }`**，经 `to_task_error()` 落成
  `TaskError::io("No parser matched: <url>")`，即 Python
  `toTaskError(ValueError(...))` 的 `发生了意外错误：No parser matched: <url>`；
- **`matches` / `matches_passive` 只塞 `{"url": url}`**：现有八个解析器的 `match` 只读
  `options.url`（`features/*/pack.py` 实测），读配置留给调用方。

`is_truthy` 完整镜像 Python 真值判定（`None` / `""` / `0` / `false` / 空容器为假），
因为预设里的 `clientProfile` 既可能是 `""` 也可能是缺键。

### 搬不了的部分（结构性 N/A，不是「还没做」）

| Python | 为什么 N/A |
|---|---|
| `load()` / `loadPacks()` | 要 `importlib` 加载 `features/` 下的 Python 子类 |
| `pack.parsers` / `pack.pages()` | 返回的是 Python 类对象 |
| `pack.config.createRuntimeCard` / `fileAssociationToggle` | 造 Qt 卡片、连 Qt 信号 |
| `pack.taskCardClass` / `draftCardClass` | 返回 Qt 卡片类 |
| `register` 里的 `pack.parse = self.parse` 等注入 | 给 Python 对象打属性 |
| `activate()` / `deactivate()` 的协程等待 | 没有 pack 就没有协程；两侧在无 pack 时都空转 |
| `_registerAssociations` | 走 `app.platform.file_association`（Qt 信号） |

`Engine::start()` / `Engine::stop()` 仍然调用 `activate()` / `deactivate()`
（空转，但调用点在 `engine.py:23` / `:44` 的位置上），`Engine::features_mut()` 是宿主
（扩展安装流程 / 测试）注册解析器的写入口。

## aria2 RPC 应用侧（`gd3-ui::aria2_actions`）

`app/services/aria2_rpc.py`（233 行）把「协议」与「应用」两层混在一个类里。协议层早在
Phase 5/6 就是 `gd3-servers::aria2`（426 行），所以本模块只搬应用侧的三个方法：
`applyTaskOptions` / `_onTaskParsed` / `_onTaskParseFailed`。381 行 / 10 单测。

| Python | Rust |
|---|---|
| `parseHead` / `buildHttpResponse` / `buildPreflightHeaders` / `toHttpStatus` | `gd3-servers::http`（Phase 5） |
| `_run` 的 JSON-RPC 方法表、`_addUri` 的参数校验、token、WebSocket 帧 | `gd3-servers::aria2`（`Aria2Action`，Phase 5/6） |
| `applyTaskOptions` / `_onTaskParsed` / `_onTaskParseFailed` | **本模块** |

```rust
pub struct Aria2Request {
    pub url: String, pub output_folder: String,
    pub headers: Vec<(String, String)>,   // 保留原始大小写与顺序
    pub client_profile: String, pub name: String,
}
pub enum Aria2Effect { DraftRequested(TaskRecord), AddTask(TaskRecord),
                       ParseFailed { url: String, error: TaskError } }
pub fn apply_task_options(features: &FeatureService, presets: &[IdentityPreset],
                          request: Aria2Request, should_draft_taken_download: bool)
                          -> Vec<Aria2Effect>;
```

四条口径：

- **`Aria2Request` 是 `Aria2TaskOptions` 的同形物，不跨 crate 引用**：`gd3-ui` 的依赖里
  没有 `gd3-servers`，也不该有（协议层不该把 tokio 拖进 UI crate）；宿主在两边的同名字段
  之间搬一次（`Aria2Action::AddTask { gid, options }` → `Aria2Request`）；
- **结果摊成效果队列**（同 `TaskAction` / `RuntimeEffect` 的口径）：`coroutineRunner.submit(
  ..., done=, failed=)` 的三条出路 = 三个变体，顺序与 Python 的调用顺序一致（至多一条）；
- **`out` 走 `Task.setName` 的口径**：`to_safe_filename(name, fallback, 200)`，`fallback`
  取**当前**名字（`self.name or "download"` 的求值时机），名字为空才退 `"download"`；
- **`Aria2Effect` 只 `derive(Debug, Clone)`**：`TaskRecord` 没有 `PartialEq`（`extra` /
  `checksums` 是 `serde_json::Value`），断言用 `matches!`。

### 照抄的 Python 缺陷

`Aria2RpcService.taskDraftRequested`（`aria2_rpc.py:108`）**在整个 Python 应用里没有任何
`.connect()`**——`grep -rn taskDraftRequested app/` 只命中 `browser_service.py` 里那个同名
的另一个 Signal。于是 `shouldDraftTakenDownload` 打开时，aria2 加进来的任务被 emit 到一个
没人听的信号，然后 `return`，**任务直接消失**。Rust 保留 `DraftRequested`：宿主照抄不接 →
与 Python 同病；接了 → 顺手修好。这条差异写在模块头，免得将来被当成漏了。

## 浏览器扩展载荷层（`gd3-ui::browser_payload`）

`app/services/browser_service.py`（597 行）混了三层，本模块只搬中间那层
（`browser_service.py:47-168`，808 行 / 18 单测）：

| Python | Rust |
|---|---|
| WebSocket 握手 + 帧（`_send` / `_sendError` 的封包部分） | `gd3-servers::websocket`（Phase 5/6） |
| **五个 `StrEnum` + `toStr` / `toInt` + 三个 `to*`** | **本模块** |
| `BrowserClientSession` / 配对 / hello / create_task 状态机 | 第 111 步（`browser_session`） |
| 托盘图标、配对对话框、`extensionUpdated` 的 UI 提示 | Phase 7 主窗口那条线 |

```rust
pub const PROTOCOL_VERSION: i64 = 2;

pub enum MessageType { /* 12 变体 */ }   pub enum ErrorCode { /* 3 */ }
pub enum BrowserAction { /* 6 */ }       pub enum TaskSource { /* 4 */ }
pub enum CreateTaskStatus { /* 3 */ }

pub struct PayloadDefaults {
    pub download_folder: String,
    pub pre_block_num: i64,
    pub current_headers: Map<String, Value>,
}
pub enum TaskOptionsPayload {   // Merge 装箱，见下
    Resource(ResourceOptions), Page(PageOptions), Merge(Box<MergeOptions>) }

pub fn to_resource_task_options(resource: &Map<String, Value>, defaults: &PayloadDefaults)
                                -> ResourceOptions;
pub fn to_task_options(source: TaskSource, payload: &Map<String, Value>,
                       defaults: &PayloadDefaults) -> TaskOptionsPayload;
pub fn to_task_summary(record: &TaskRecord) -> Map<String, Value>;
```

`to_task_summary` 的 13 个键与 Python 字面量同序：`taskId` / `name` / `status` / `progress` /
`receivedBytes` / `fileSize` / `speed` / `createdAt` / `canPause` / `canOpenFile` /
`canOpenFolder` / `fileExt` / `packName`。

六条口径：

- **`PayloadDefaults` 把三项配置打包成参数**：`downloadFolder` / `preBlockNum` /
  `currentHeaders()`。Rust 侧没有响应式配置层，调用方每轮现读再传（同
  `identity::parse_presets` 的口径）；`MergeTaskOptions` 不传 `headers` 时用的正是
  `currentHeaders()` 那套。嵌套在 merge 里的 `ResourceTaskOptions` 也走 dataclass 缺省
  （`downloadFolder` / `currentHeaders()` / `preBlockNum`），不是外层覆写后的值；
- **`TaskOptionsPayload` 用枚举对应三个 dataclass 子类**：Python `match source` 里那条
  `raise ValueError(f"unsupported task source: …")` 在 Rust 里不存在——`TaskSource` 是枚举，
  构造不出第四种；
- **`Merge` 变体装箱**：`MergeOptions` 比另外两个变体大 400 字节以上
  （`clippy::large_enum_variant`），载荷寿命很短，`Box` 一次分配不影响语义；
- **枚举叫 `BrowserAction` 而不是 `TaskAction`**：`gd3-ui` 已有 `actions::TaskAction`
  （第 100 步的任务动作效果），同名会让 `use` 撞车；
- **`toInt` 照抄 `isinstance(True, int)`**：JSON `true` → `1`；`false` 不满足 `> 0`，
  落回缺省；`3.0` 是浮点不算 `int`，也落回缺省；
- **`round(progress, 2)` 走 `format!("{value:.2}")` 再 parse 回 `f64`**：Python 是对二进制值的
  精确十进制展开做五成双。先乘 100 再 `round_ties_even` 会在 `round(42.675, 2)` 上给出
  `42.68`（`42.675 * 100.0` 恰好舍入成 `4267.5`），与 CPython 的 `42.67` 不符；
- **`parent` / `suffix` 用本步新增的 `gd3-core::path::posix_parent` / `posix_suffix`**：
  `pathlib` 的 `parent` **不消解 `..`**（`Path("/a/b/../c").parent == "/a/b/.."`），与
  `posix_normpath` 混用会算错；`suffix` 对 `.bashrc` / `b.` / `...` 都是 `""`。

三处容错差异（Python 会抛异常炸掉整条会话，Rust 退化成空）：

| 坏包 | Python | Rust |
|---|---|---|
| `headers` 不是对象 | `AttributeError` | 空 map |
| `resources[i]` 不是对象 | `AttributeError` | 当作没有该资源 |
| `path` 不是字符串 | `TypeError` | 用 `cfg.downloadFolder` |

`headers.get("user-agent")` 是**大小写敏感的精确匹配**（Python 原样），扩展发 `User-Agent`
时 `sourceUserAgent` 是空串——这条照抄，有单测钉住。

## 剩余工作（Phase 8）

**配置（`UserConfig.json`）和 Slint 入口要一起搬**。`app/config/cfg.py` 是
`qfluentwidgets.QConfig` 的子类：每一项都是 Qt 的 `ConfigItem`，设置页直接绑它的
`valueChanged`，而且**每次 `cfg.set()` 都会整文件回写**。只要 Qt 侧还在写这个文件，
Rust 侧再写同一个文件就会互相覆盖——所以 Rust 侧的 cfg 读写必须与「Slint 成为唯一 UI」
同一步落地，不能提前。同理，凡是「持久化进 cfg」的服务（`category_service` 的
`categoryRules` 等）都要排在它后面。
