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
- `probeConflict` 恒 `None`——`gd3-core::placeholder` 与
  `TaskService::add_with_conflict` 都已就位（第 92/93 步），但 `Engine` 还没把
  「冲突队列 → 重新 add」这条线接起来，所以冲突队列仍不弹窗、按 `keepBoth` 放行。

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

规模：`placeholder.rs` **915 行 / 31 单测**；`task_service.rs` **1773 行 / 42 单测**
（其中 `add_with_conflict` 10 个、`update_placeholders` 6 个、分类接线 4 个）。

## 剩余工作（Phase 8）

**配置（`UserConfig.json`）和 Slint 入口要一起搬**。`app/config/cfg.py` 是
`qfluentwidgets.QConfig` 的子类：每一项都是 Qt 的 `ConfigItem`，设置页直接绑它的
`valueChanged`，而且**每次 `cfg.set()` 都会整文件回写**。只要 Qt 侧还在写这个文件，
Rust 侧再写同一个文件就会互相覆盖——所以 Rust 侧的 cfg 读写必须与「Slint 成为唯一 UI」
同一步落地，不能提前。同理，凡是「持久化进 cfg」的服务（`category_service` 的
`categoryRules` 等）都要排在它后面。
