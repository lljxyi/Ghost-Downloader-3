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

规模：**69 个模块**（68 个 `pub` + 私有 `format`，加 `lib.rs` / `main.rs` 共 **71 个 `.rs`**）/
**49 342 行 `src/`**（含 `lib.rs` 3 501 行）、**6 个 Slint 文件 2 858 行**、
**932 个单测**（`cargo test -p gd3-ui --lib`）。

`app/view` 已逐文件对照：`qfw_patch.py`(75) 是纯 Qt monkeypatch（改 `qfluentwidgets` 内部行为），
无业务语义可镜像，判定 **N/A**；`clipboard_listener.py` / `draft_cards.py` 由
`clipboard.rs` / `draft_card.rs` 覆盖（P7 补漏）。

Slint 文件在 `crates/gd3-ui/ui/`：`app_shell.slint`（壳）、`appwindow.slint`（主窗口）、
`task_page.slint`、`task_drawer.slint`、`settings_page.slint`、`card_menu.slint`。
无头冒烟测试在 `lib.rs` 的 `mod smoke`（`MinimalSoftwareWindow`），覆盖组件树实例化与
属性读写、任务行模型往返、`TaskRecord → 行` 的端到端推流。

```bash
cargo test -p gd3-ui                       # 库 + 冒烟
cargo test -p gd3-ui --lib                 # 只跑单测（932）
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

**尚未做**：`config_store` 未接进 `boot::run`（接线条件见「配置落盘」一节末尾）；
`loadCrx` 的 crx 解包与 `fileWatcher` 归宿主；`update_service` 的执行层第 135 步已接
（`gd3-ui::update_exec`，见「更新检查的执行层」一节）；`Ghost-Downloader-3.py` 的入口用 `GHOST_UI` 切换
（缺省 `qt`，`slint` 时 `exec` 本二进制，见「前端分派」一节）。
二进制入口见下一节。
（`RuntimeStatusService` 第 102 步搬完、第 107 步删掉空壳桩；`FeatureService` 的
解析器链第 108 步搬完；`BrowserService` / `Aria2RpcService` 第 112 步收敛，见「服务装配」
一节；托盘的语义第 75 步由 `shell.rs` 覆盖，装配与接线第 136 步落在
`gd3-ui::tray` + `ui/system_tray.slint`。）

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
  `resume_saved()` 把 `WAITING`/`RUNNING` 重新入队并把 `RUNNING` 回落到 `WAITING`。
  启动**不回写** `tasks.jsonl`；退出时 `Engine::stop()` 把 `RUNNING`/`WAITING` 落成 `PAUSED`
  ——与 Python 一致（一次「启动完立刻退出」也会留下 `PAUSED`）。
- **下载运行期**（第 126/127 步，见「下载运行期」一节）：`Runtime` 每 100ms tick 一次
  （`pump` → `drain` → 速度表 → 有改动才落盘 → 刷列表），工作线程里是真 `gd3-net`。
  有头由窗口事件循环驱动；无头只有 `--until-idle` 才真的推运行期。
- **CLI**：`--silence`（对应 Python 的 `--silence`）、`--version`、`--help` 之外，
  另有 Rust 侧可测性开关 `--headless` / `--frames` / `--data-dir` / `--tasks-file` /
  `--until-idle` / `--idle-timeout` / `--set ATTR=VALUE`（Python 都没有）。

```bash
cargo run -p gd3-ui --bin gd3-ui -- --headless --frames 3 --data-dir /tmp/gd3-data
# Ghost Downloader v4.4.0 launched (headless, 3 frame(s), config=default, data=/tmp/gd3-data,
#   tasks=0, resumed=0, completed=0, ticks=0)
cargo run -p gd3-ui --bin gd3-ui -- --version   # v4.4.0（app/config/constants.py）
# 真下载（`tasks.jsonl` 里已有 WAITING 任务时）：
cargo run -p gd3-ui --bin gd3-ui -- --data-dir /tmp/gd3-data --until-idle --idle-timeout 120
# 改一项配置（走 settings_writer，与设置页同一条路）：
cargo run -p gd3-ui --bin gd3-ui -- --headless --frames 0 --data-dir /tmp/gd3-data --set maxTaskNum=5
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
`desktop_keepalive` / 通知、功能包加载、更新检查、OOBE 分支、翻译器。
托盘已在第 136 步接上（`gd3-ui::tray`，只在有头分支创建，见「托盘」一节）。

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
（`browser_service.py:47-168`，812 行 / 18 单测）：

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

## 浏览器扩展会话状态机（`gd3-ui::browser_session`）

`browser_service.py:29-597` 的最后一层：配对请求的生命周期、`hello` 的协议版本门禁、
`createTask` 的「直加 / 草稿」分叉、`taskAction` 的六种动作、快照广播的去重与合并
（2123 行 / 41 单测）。

```rust
pub type SessionId = u64;

pub struct ClientSession { peer_address, is_authenticated, is_subscribed_to_tasks,
                           last_snapshot: Option<String>, extension_version, install_type }
pub struct PairRequest   { request_id, peer_address, extension_version, client_kind }

pub enum SessionEffect {
    Send { session, text }, Close { session }, PairRequestChanged(Option<PairRequest>),
    ConnectionChanged, ProtocolMismatched, TaskDraftRequested(Vec<TaskRecord>),
    ExtensionUpdated(String), InstallExtension { session },
    Task(TaskAction),                    // 复用 task_actions 的词汇表
    Dispatch { task_ids: Vec<String> },  // _pump() 派发出来的 id
    OpenFile { path }, RevealInFolder { path },
}

pub struct SessionContext<'a> {          // handle_message 需要的外部世界
    parsers: &'a ParserChain, presets: &'a [IdentityPreset],
    tasks: &'a mut TaskService, conflict_choice: NameConflictChoice,
    delete_recoverably: &'a mut dyn FnMut(&Path) -> bool,
}

impl BrowserHub {
    pub fn new(token: impl Into<String>, defaults: PayloadDefaults) -> Self;
    pub fn token(&self) -> &str;   pub fn set_token(..);   pub fn set_defaults(..);
    pub fn set_should_draft(..);   pub fn set_max_task_num(..);
    pub fn session(&self, SessionId) -> Option<&ClientSession>;   pub fn sessions(&self) -> &[(SessionId, ClientSession)];
    pub fn pair_request(&self) -> Option<&PairRequest>;
    pub fn connection_summary(&self) -> (String, String);
    pub fn is_updating_extension(&self) -> bool;

    pub fn on_connected(&mut self, SessionId, peer_address: impl Into<String>);
    pub fn on_disconnected(&mut self, SessionId) -> Vec<SessionEffect>;
    pub fn regenerate_token(&mut self) -> (String, Vec<SessionEffect>);
    pub fn approve_pair(&mut self, request_id: &str) -> Vec<SessionEffect>;
    pub fn reject_pair(&mut self, request_id: &str) -> Vec<SessionEffect>;
    pub fn note_tasks_changed(&mut self) -> bool;
    pub fn send_snapshots(&mut self, &TaskService) -> Vec<SessionEffect>;
    pub fn tasks_changed(&mut self, &TaskService) -> Vec<SessionEffect>;
    pub fn extension_installed(&mut self, SessionId, ok: bool) -> Vec<SessionEffect>;
    pub fn handle_message(&mut self, SessionId, raw: &[u8], &mut SessionContext<'_>)
        -> Vec<SessionEffect>;
}
```

七条结构决策：

- **会话键是 `SessionId = u64`，不是连接对象**：Python 拿 `WebSocketStream` 的对象身份当
  `dict` 键，本 crate 不依赖网络层，宿主给每条连接发号。会话表用
  `Vec<(SessionId, ClientSession)>` 保插入序——Python 的 `dict` 也是插入序，快照广播的
  会话顺序因此一致。
- **副作用全摊成 `SessionEffect`**：`_send` / `stream.close()` / 四个 Qt 信号 /
  `openFile` / `revealInFolder` 都只产出效果，宿主执行。单测因此不需要网络、不需要桌面，
  也不会真的打开文件。
- **`TaskService` 报告的「宿主半边」复用 `task_actions::TaskAction`**：`delete` /
  `redownload` / `pause` 在 Rust 只做记账（第 99 步），`Task.deleteFiles`、
  `fileDeleteDenied`、`_unwatchFile`、`checksumStopped`、`_cancelWork`、`taskRemoved`
  都得由宿主做。这里不另造词汇表——浏览器动作与卡片动作调的是同一批服务方法，摊出来的
  效果自然同形。四个「报告 → 效果」映射（`cancel_effects` / `pause_effects` /
  `delete_effects` / `redownload_effects`）抽成 `task_actions` 的 `pub fn`，两条线共用
  一份实现，发射顺序只有一处定义。
- **`_pump()` 派发的 id 单独走 `Dispatch { task_ids }`**：`TaskAction::Started` 需要
  `workId`，发号与提交协程归宿主，`browser_session` 不该造号。
- **`install()` 的 crx 解包留给宿主**：`_onHello` 发现扩展是 `development` 且**旧于**
  `LATEST_EXTENSION_VERSION` 时产出 `InstallExtension`，宿主解包完回调
  `extension_installed(session, ok)`。
- **`_parse` 是同步的**：`featureService.parse` 在 Python 是 `async`，本仓
  `TaskParser::parse` 同步返回（第 108 步），所以「提交 → 回调 `_onTaskParsed`」两步合成
  一次调用。
- **`_sendSnapshots` 的延迟合并显式化**：`note_tasks_changed()`（判重）+
  `send_snapshots()`（真发），`tasks_changed()` 是两者的同步合成。宿主若要合并突发信号，
  只用前两个即可。

照抄的四个 Python 缺陷：

| 缺陷 | 位置 | 后果 |
|---|---|---|
| `_isUpdatingExtension` 成功后不复位 | `_onHello` → `install()` | 同一进程内不会二次热更 |
| `_onTaskParsed` 里 `taskService.add` 出错什么都不回 | `done=` 回调 | 扩展收不到 `REJECTED` |
| `decryptionKeys` 是空操作 | `Task.setOptions` 只认 `outputFolder` / `category` | 字段被丢弃 |
| `regenerateToken` 不踢会话出表 | 只 `_close` + 置 `isAuthenticated = False` | 断开时才 `pop` |

两处「Python 抛异常、Rust 里不存在」的路径：

| 路径 | Python | Rust |
|---|---|---|
| `toTaskOptions` 抛 `AttributeError` / `TypeError` | `except Exception` → `REJECTED` + `repr(e)` | 不会发生（第 110 步的容错转换） |
| `_onTaskAction` 内部异常 | `except Exception` → `repr(e)` | 不会发生（`TaskService` 返回 `Option` / `bool`） |

这两条不是「还没做」：Rust 侧把异常折成了返回值，对应的拒绝分支没有输入可喂。

### 测试（41 条，按状态机分段）

| 测试 | 钉住的行为 |
|---|---|
| `a_stale_protocol_version_is_refused_and_reported` | 协议版本不符 → `REJECTED` + `ProtocolMismatched` + `Close` |
| `a_wrong_token_is_refused_without_the_protocol_signal` | 令牌错只 `REJECTED` + `Close`，不报协议信号 |
| `an_error_without_a_request_id_omits_the_key` | `requestId` 空串时错误帧不带该键 |
| `hello_authenticates_and_acks_the_capabilities` | `hello` → `is_authenticated` + `helloAck` 的 6 个动作 + 配置回显 |
| `a_store_build_never_triggers_a_hot_update` | `store` 版扩展永不热更 |
| `a_newer_development_build_is_installed_once` | `development` 且旧于 `LATEST` → 一次 `InstallExtension` |
| `a_finished_install_reloads_the_extension_and_keeps_the_flag` | 成功 → `ExtensionUpdated`，标志**保持置位** |
| `a_failed_install_only_clears_the_flag` | 失败 → 只清标志，不发 `ExtensionUpdated` |
| `anything_but_hello_before_authentication_is_rejected` | 未认证的 `taskAction` / `subscribeTasks` 都被拒 |
| `a_broken_frame_gets_a_bad_request` | 非 JSON / 非对象 / 无 `type` → `BAD_REQUEST` |
| `a_frame_from_an_unknown_session_is_ignored` | 未知 `SessionId` 不产出任何效果 |
| `a_second_session_cannot_claim_the_pending_pair_request` | 配对请求被别的会话占着时不覆盖 |
| `the_same_session_can_replace_its_own_pair_request` | 同一会话可刷新自己的请求 |
| `approving_a_pair_request_sends_the_token` | `approvePair` → `paired` 帧带令牌 + 置认证 |
| `rejecting_a_pair_request_sends_no_token` | `rejectPair` → `pairRejected` 帧，不带令牌 |
| `an_unknown_pair_request_id_does_nothing` | 过期的 `requestId` 静默 |
| `regenerating_the_token_closes_every_session` | 新令牌 + 每个已认证会话一条 `Close` |
| `regenerating_the_token_is_silent_when_nobody_was_authenticated` | 无人认证时不发 `ConnectionChanged` |
| `disconnecting_the_pairing_session_clears_the_request` | 配对中会话断开 → `PairRequestChanged(None)` |
| `subscribing_sends_a_snapshot_newest_first` | 快照按 `createdAt` 降序，13 键/项 |
| `an_unchanged_snapshot_is_not_resent` | 同一快照文本不重发（`last_snapshot` 去重） |
| `only_subscribed_authenticated_sessions_get_snapshots` | 未订阅 / 未认证都不发 |
| `a_pending_snapshot_swallows_the_burst` | `note_tasks_changed` 返回 `true` 只一次 |
| `create_task_adds_the_task_and_reports_the_id` | `createTask` → `CREATED` + 任务 id + `add_with_conflict` |
| `a_draft_request_never_touches_the_task_list` | `draft` → `TaskDraftRequested`，任务表不变 |
| `the_configured_default_decides_when_the_frame_says_nothing` | 帧里没有 `draft` 时取 `cfg.shouldDraftTakenDownload` |
| `a_title_keeps_the_existing_suffix` | 标题已带 `existingSuffix` 时不重复拼 |
| `an_unknown_source_is_rejected` | `source: ""` / 未知值 → 「未知的任务来源」 |
| `a_create_task_without_a_payload_is_rejected` | 缺 `payload` → `error` 帧「无效的请求」 |
| `a_create_task_without_a_parser_match_is_rejected` | 解析器不认 → `rejected` + `No parser matched: <url>` |
| `toggling_a_completed_task_reports_a_failure` | `Completed` 上 `togglePause` → 「任务已完成」 |
| `toggling_a_running_task_pauses_it` | 运行中 → `Paused`，成功帧不带 `message` |
| `toggling_a_paused_task_queues_it` | 暂停 → `start` + `pump`，任务离开等待队列、`running_count() == 1` |
| `cancel_asks_the_host_to_delete_files_and_remove_does_not` | `cancel` → `DeleteFiles`；`remove` 不删文件 |
| `redownloading_resets_the_task` | `redownload` → 状态回 `Waiting` |
| `opening_a_missing_file_reports_that_it_is_not_ready` | 文件不存在 → 「任务尚未完成」 |
| `opening_an_existing_file_produces_the_effect` | 存在 → `OpenFile` |
| `opening_the_folder_reveals_the_file` | `openFolder` → `RevealInFolder` |
| `opening_the_folder_of_a_missing_directory_fails` | 目录不存在 → 拒绝 |
| `an_unknown_task_or_action_is_reported` | 未知 `taskId` → 「任务不存在」；未知 `action` → 「不支持的操作」 |
| `a_task_action_without_a_request_id_is_a_bad_request` | 缺 `requestId` → `bad_request`「缺少 requestId」，任务不动 |

### 本轮修掉的两个真缺陷

1. **`togglePause` 的「恢复」分支只 `start` 不 `pump`**：Python 的 `TaskService.start`
   就是 `_schedule`（`queue.wait` + `_pump()`，`task_service.py:392-395`）。Rust 侧
   `TaskService::start` 只 `queue.wait`，所以必须再 `pump(max_task_num)`——否则空出来的
   槽永远不派发，扩展点「恢复」后任务卡在 `Paused`。
2. **`cancel` / `remove` 的删文件没人做**：`TaskService::delete` 只做记账（第 99 步），
   原本 `Some(report)` 直接丢掉 `delete_files` / `delete_denied` → 扩展点「取消」后
   文件留在盘上、`fileDeleteDenied` 也发不出去。现在摊成
   `TaskAction::DeleteFiles` / `DeleteDenied` 交给宿主。

## 服务装配（`Engine` 的 `BrowserService` / `Aria2RpcService`）

第 109–111 步把 `browser_service.py` 与 `aria2_rpc.py` 的逻辑搬完了，但 `engine.rs` 里
`BrowserService` / `Aria2RpcService` 还是两个零大小桩。第 112 步把它们收敛成「只剩接线」的
薄结构——**一行逻辑都不复制**，全部转调已有的三个模块。

| Python | Rust | 状态 |
|---|---|---|
| `browser_service.py` 的帧层 | `gd3-servers::websocket` | Phase 5/6 |
| `browser_service.py` 的载荷层 | `gd3-ui::browser_payload` | 第 110 步 |
| `browser_service.py` 的会话状态机 | `gd3-ui::browser_session`（`BrowserHub`） | 第 111 步 |
| `aria2_rpc.py` 的协议层 | `gd3-servers::aria2` | 第 109 步 |
| `aria2_rpc.py` 的应用层 | `gd3-ui::aria2_actions` | 第 109 步 |
| **两个服务的装配 + 配置快照** | **`gd3-ui::engine`（本节）** | **第 112 步** |

### 结构

```rust
pub struct BrowserService { hub: BrowserHub }        // derive(Debug, Clone)

impl BrowserService {
    pub fn new(config: &mut UserConfig, settings: &EngineConfig) -> BrowserService;
    pub fn hub(&self) -> &BrowserHub;
    pub fn hub_mut(&mut self) -> &mut BrowserHub;
    pub fn token(&self) -> &str;
    pub fn apply_config(&mut self, config: &mut UserConfig, settings: &EngineConfig);
}

pub struct Aria2RpcService { should_draft_taken_download: bool }
// derive(Clone, Copy, Debug, Default, PartialEq, Eq)

impl Aria2RpcService {
    pub fn new(should_draft_taken_download: bool) -> Self;
    pub fn should_draft_taken_download(&self) -> bool;
    pub fn set_should_draft_taken_download(&mut self, flag: bool);
    pub fn apply_task_options(
        &self,
        features: &FeatureService,
        presets: &[IdentityPreset],
        request: Aria2Request,
    ) -> Vec<Aria2Effect>;
}
```

`Engine` 新增八个方法：

```rust
pub fn browser_mut(&mut self) -> &mut BrowserService;
pub fn handle_browser_message(&mut self, session: SessionId, raw: &[u8]) -> Vec<SessionEffect>;
pub fn browser_connected(&mut self, session: SessionId, peer_address: impl Into<String>);
pub fn browser_disconnected(&mut self, session: SessionId) -> Vec<SessionEffect>;
pub fn browser_tasks_changed(&mut self) -> Vec<SessionEffect>;
pub fn regenerate_browser_token(&mut self) -> (String, Vec<SessionEffect>);
pub fn apply_aria2_request(&self, request: Aria2Request) -> Vec<Aria2Effect>;
pub fn refresh_services(&mut self);
```

模块级助手（都不导出，`Engine` 与测试共用一份）：

```rust
fn browser_token(config: &mut UserConfig) -> String;
fn payload_defaults(config: &UserConfig) -> PayloadDefaults;
fn should_draft_taken_download(config: &UserConfig) -> bool;
fn presets_of(config: &UserConfig) -> Vec<IdentityPreset>;
fn store_name_conflict_choice(config: &UserConfig) -> gd3_core::NameConflictChoice;
```

### 六条结构决策

1. **构造期副作用照搬**：`browser_service.py:189-190` 的
   `if not cfg.browserExtensionPairToken.value: cfg.set(..., token_urlsafe(16))` 是
   `__init__` 里唯一的写操作，**不在任何消息路径上**。所以 `BrowserService::new` 收
   `&mut UserConfig`，令牌为空时就地生成并写回。`token_urlsafe(16)` 是 16 字节 →
   22 个 URL-safe base64 字符，`gd3_core::pair_token::generate()` 同口径（第 111 步已核）。
   **落盘归 Phase 8**：Rust 侧只改内存里这份克隆，传进来的 `UserConfig` 不动。
2. **`Aria2RpcService` 只留一项状态**：`_onTaskParsed` 里除了
   `cfg.shouldDraftTakenDownload.value` 之外没有别的服务状态（`_coroutineRunner` /
   `_parse` / `_addTask` 都换成了效果队列与 [`FeatureService`] 参数）。
3. **`Engine` 提供两个「借用合成」入口**：`handle_browser_message` 一次调用内同时借
   `features` / `tasks` / `browser` 三个**不同字段**，装好 `SessionContext` 交给
   `BrowserHub::handle_message`，效果队列交出去后借用结束——形态与
   `Engine::task_actions`（`RefCell<&mut TaskService>`）一致。
4. **`delete_recoverably` 接真实现**：`xdg_data_home` 取
   `crate::boot::generic_data_location()`（`$XDG_DATA_HOME` 或 `~/.local/share`），
   uid 取 `gd3_core::trash::current_uid()`。Python 的 `deleteRecoverably` 走
   `QFile.moveToTrash`，第 87 步已对齐；这里不再退化成 `|_| false`。
5. **`apply_aria2_request` 现读 `shouldDraftTakenDownload`**：Python 在
   `_onTaskParsed`（`aria2_rpc.py:220`）里每次现读 `cfg`，所以设置页改完立刻生效。
   `Aria2RpcService` 自己那份快照是给「没有 `Engine` 的宿主」用的，靠
   `refresh_services()` 同步——两条路径都在测试里钉住了。
6. **配置变更靠显式 `refresh_services()`**：Python 每处现读 `cfg`，Rust 侧
   `BrowserHub` 持的是快照（`token` / `defaults` / `should_draft` / `max_task_num`）。
   `refresh_services()` 重取这四项 + `Aria2RpcService` 的开关，**不动监听器**——
   监听器的启停是 `Engine::start` / `stop` 的事。

### 与 Python 的已知差异

- **11 个信号连线不存在**：`taskAdded` / `taskRemoved` / … / `speedChanged` 在
  Python 里接 `_onTasksChanged`，Rust 侧由宿主在任务变化时直接调
  `Engine::browser_tasks_changed()`（`QTimer.singleShot(0, ...)` 的延迟合并已在
  第 111 步拆成 `note_tasks_changed` / `send_snapshots` 两步）。
- **`cfg.set` 的落盘**：见决策 1。
- **`loadCrx`（crx 解包）与 `fileWatcher` 仍归宿主**：`zip` crate 不在离线缓存里，
  `install()` 只产出 `SessionEffect::InstallExtension`，宿主解包完回调
  `BrowserHub::extension_installed`。

### 九条新单测

| 测试 | 钉住什么 |
|---|---|
| `browser_service_generates_a_pair_token_on_construction` | 空令牌 → 22 字符 + 写回引擎那份 cfg + 不动入参克隆 |
| `an_existing_pair_token_is_kept` | 非空令牌不重生成 |
| `payload_defaults_read_the_three_config_items` | `downloadFolder` / `preBlockNum` / `currentHeaders()` 三项 |
| `a_browser_hello_reaches_the_hub_with_the_engine_token` | `browser_connected` + `handle_browser_message` 端到端：`ConnectionChanged` + `Send` + 认证置位 + `store` 不触发热更 |
| `a_browser_hello_with_a_stale_token_is_closed` | 令牌不符 → 末尾 `Close`，`is_authenticated` 仍 `false` |
| `regenerating_the_browser_token_writes_back_to_the_config` | 新令牌 ≠ 旧令牌、22 字符、hub 与 cfg 同步 |
| `an_aria2_request_without_parsers_reports_a_parse_failure` | 空解析器链 → `Aria2Effect::ParseFailed`（不是 `AddTask`） |
| `refresh_services_reloads_the_aria2_draft_flag` | 快照未刷新时仍是旧值，`refresh_services()` 之后才变 |
| `the_store_choice_mirrors_the_config_choice` | `gd3_ui` 与 `gd3_core` 两份 `NameConflictChoice` 的转换（含非法值收敛） |

## 配置落盘（`gd3-ui::config_store`）

Python 侧**所有**配置写入都走 `qfluentwidgets` 的 `QConfig.set`
（`.venv/lib/python3.13/site-packages/qfluentwidgets/common/config.py:288`）。`cfg` 是
`app/config/cfg.py:260` 的 `Config(QConfig)`，`Ghost-Downloader-3.py:38` 的
`qconfig.load(f"{APP_DATA_DIR}/UserConfig.json", cfg)` 把模块级 `qconfig._cfg` 指向它，
所以 `cfg.set(item, v)` 与 `qconfig.set(item, v)` 落同一个文件。第 113 步把这条路径搬成
`crates/gd3-ui/src/config_store.rs`。

| Python | Rust | 状态 |
|---|---|---|
| `ConfigItem` 表（40 个 app 项 + `themeColor` / `fontFamilies` / `themeMode`） | `gd3_core::settings::SPECS` | 第 78 步 |
| `validator.correct()` / `ConfigItem.value` setter | `settings::Kind::correct` / `Spec::write` | 第 78 步 |
| `json.dump(toDict(), indent=4, ensure_ascii=False)` | `UserConfig::to_json_text` | 第 78 步 |
| `qconfig.load(path, cfg)` | `ConfigStore::load` | 第 113 步 |
| `QConfig.set` + `QConfig.save` | `ConfigStore::set` / `save` | 第 113 步 |

### 结构

```rust
pub const RESTART_ATTRS: &[&str] = &["dpiScale", "language", "shouldVerifySsl"];

pub struct ConfigChange {
    pub attr: &'static str,      // cfg.py 的属性名
    pub group: &'static str,     // 组名
    pub key: &'static str,       // 键名
    pub previous: Value,         // 改之前的逻辑值
    pub value: Value,            // 改之后的逻辑值（已过 correct）
    pub stored: Value,           // 落盘形态（已过 serialize）
    pub emitted: bool,           // valueChanged 是否发射
    pub restart: bool,           // 是否发 appRestartSig
}
impl ConfigChange {
    pub fn theme_changed(&self) -> bool;        // attr == "themeMode"
    pub fn theme_color_changed(&self) -> bool;  // attr == "themeColor"
}

pub enum ConfigStoreError { UnknownAttribute(String), Save(io::Error) }

pub struct ConfigStore { path: PathBuf, config: UserConfig }  // derive(Debug, Clone)
impl ConfigStore {
    pub fn new(path: impl Into<PathBuf>) -> Self;                 // 不读盘 = cfg 初值
    pub fn load(path: impl Into<PathBuf>) -> io::Result<Self>;    // qconfig.load
    pub fn reload(&mut self) -> io::Result<()>;
    pub fn path(&self) -> &Path;
    pub fn config(&self) -> &UserConfig;
    pub fn config_mut(&mut self) -> &mut UserConfig;              // 不落盘
    pub fn get(&self, attr: &str) -> Option<Value>;
    pub fn set(&mut self, attr: &str, value: &Value)
        -> Result<Option<ConfigChange>, ConfigStoreError>;
    pub fn save(&self) -> io::Result<()>;
    pub fn snapshot(&self) -> UserConfig;                         // toDict() 口径
}

pub fn py_eq(left: &Value, right: &Value) -> bool;                // Python `==`
```

`set` 的返回三态与 Python 一一对应：`Ok(None)` = `if item.value == value: return`
（没写盘、没信号）；`Ok(Some(_))` = 已整文件回写；`Err(Save)` = 内存已改、磁盘没写成
（Python 的 `save()` 同样在 setter 之后才抛）。

### 四条实证语义（2026-10-19 实读 `qfluentwidgets/common/config.py`）

`QConfig.set` 的完整源码：

```python
def set(self, item, value, save=True, copy=True):
    if item.value == value:
        return
    try:
        item.value = deepcopy(value) if copy else value   # setter 里跑 validator.correct()
    except:
        item.value = value
    if save:
        self.save()                                       # 无条件整文件回写
    if item.restart:
        self._cfg.appRestartSig.emit()
    if item is self._cfg.themeMode:
        self.theme = value
        self._cfg.themeChanged.emit(value)
    if item is self._cfg.themeColor:
        self._cfg.themeColorChanged.emit(value)
```

1. **比较发生在 `correct()` 之前**。当前值 `3` 时 `cfg.set(cfg.maxTaskNum, 0)` 会走完全程、
   夹成 `1` 再落盘；而 `cfg.set(cfg.maxTaskNum, 3.0)` 直接返回（`3 == 3.0`）。
   这就是 `py_eq` 存在的原因：`serde_json::Value` 的 `PartialEq` 认为 `3 != 3.0`、
   `true != 1`，而 `QConfig.set` 第一行恰好就是这个比较。
2. **`set` 里没有 `validate()`**：只经过 `value` setter 的 `validator.correct()`。
   越界值被**夹紧**（`maxTaskNum = 99` → `10`），不是被拒收。
3. **`correct()` 之后值没变也照样落盘**：setter 里 `if ov != v` 挡住 `valueChanged`，
   但 `save()` 在 setter 之外无条件执行。`ConfigChange::emitted` 与「是否落盘」是两件事。
4. **`save()` 会 `mkdir(parents=True, exist_ok=True)`**：父目录不存在不是错误。

`restart=True` 只有三项（`shouldVerifySsl` `cfg.py:250`、`dpiScale` `:296`、
`language` `:303`），`setting_page.py:398` 把它接成「需要重启」的提示条。

### 与 Python 的已知差异

- **`correct()` 抛异常时不中断**：`cfg.set(cfg.maxTaskNum, "7")` 在 Python 里让
  `RangeValidator.correct` 抛 `TypeError`（`min(max(1, "7"), 10)`），异常穿透 `set`、
  什么都没写；Rust 的 `Kind::correct` 退回缺省值并继续落盘。同 `gd3-core::settings`
  模块头那条「Rust 容错 ≠ Python 中断」。
- **`themeColor` 的比较口径不同**：Python 的 `item.value` 是 `QColor`，
  `QColor("#ff009faa") == "#ff009faa"` 恒假，所以每次 `cfg.set(cfg.themeColor, ...)`
  都会落盘；Rust 存 `#aarrggbb` 串，同值提前返回。app 里没有
  `cfg.set(cfg.themeColor, ...)`（主题色走 `setThemeColor(color, save=False)`，
  **不落盘**），这条差异不可达。
- **磁盘上的未知键在 `load` 就丢**：`QConfig.load` 只按 `item.key` 匹配已知项，
  `toDict()` 又只从类属性重建，多出来的组/键既读不进内存也不会被下次 `save()` 写回。
  `ConfigStore::load` 直接归一化成「42 项全量」，等价。
- **没有全局单例**：Python 的 `qconfig` 是模块级实例，靠改 `_cfg` / `file` 把写入目标
  接到 app 的 `cfg` 上；`ConfigStore` 自己持有 `(config, path)`，不需要那层间接。

### 为什么现在不接 `boot::run`

见下一节：Rust 侧写同一个 `UserConfig.json` 必须与「Slint 成为唯一 UI」同一步落地，
否则会和仍在运行的 Qt 侧互相覆盖。接线条件与落点：

1. `boot::run` 里把 `Paths::resolve` 之后的 `load_config` 换成
   `ConfigStore::load(paths.data_dir.join("UserConfig.json"))`，`Engine::new` 收
   `store.config()` 的克隆；
2. 设置页的每次提交改成 `store.set(attr, &value)`，按 `ConfigChange` 分发
   `valueChanged` / `appRestartSig` / `themeChanged` 三个信号，再调
   `Engine::refresh_services()`；
3. `BrowserService::new` 里「令牌为空就地生成」要改成「生成 + `store.set("browserExtensionPairToken", …)`」，
   否则新令牌活不过一次重启。

### 十五条新单测

`config_store.rs` **552 行 / 15 单测**。

| 测试 | 钉住什么 |
|---|---|
| `a_missing_file_leaves_the_defaults_and_is_not_created` | 缺文件 → 缺省值且**不建文件** |
| `a_python_written_file_round_trips_byte_for_byte` | 缺省档 load→save 逐字节不变 |
| `set_rewrites_the_whole_file` | 一次 `set` 写出全量 42 项，期望值由 `settings::write` 独立算出 |
| `an_unchanged_value_neither_writes_nor_signals` | 同值 → `Ok(None)`，文件不出现 |
| `an_int_and_a_float_of_the_same_value_count_as_equal` | `dpiScale` 缺省落盘是 int `0`，SpinBox 发 `0.0` → 提前返回 |
| `an_unknown_attribute_is_rejected_without_touching_the_file` | 拼写错误 → `UnknownAttribute`，不写盘 |
| `save_creates_the_missing_parent_directory` | `mkdir(parents=True)` |
| `a_failed_save_keeps_the_new_value_in_memory` | `Err(Save)` 但内存保留新值（Python 同序） |
| `an_out_of_range_value_is_clamped_not_rejected` | `99` → `10`，夹紧 |
| `a_value_that_corrects_back_to_the_current_one_is_saved_without_a_signal` | `emitted == false` 但仍落盘 |
| `restart_items_are_flagged` | 三项 `restart`，其余 `false` |
| `theme_items_are_flagged` | `themeMode` / `themeColor` 各发各的信号 |
| `unknown_keys_on_disk_are_dropped_by_the_load` | 磁盘多出来的组/键在 load 时丢 |
| `reload_picks_up_an_external_write` | `reload()` 重读外部改动 |
| `py_eq_mirrors_python_equality` | `3 == 3.0` / `true == 1` / `"3" != 3` / dict 不看键序 |

## 设置页回写（`gd3-ui::settings_writer`）

设置页的每个控件在第 32–39 步就接成了 `SettingsActions` 的一个闭包，但闭包体一直是空的
（`install_settings_actions` 只在测试里被调用）。第 113 步把 `QConfig.set` 的语义落成
`ConfigStore`，这一步把两端接起来：**动作 → 属性名 → `ConfigStore::set`**，于是设置页的
编辑终于能落盘。`crates/gd3-ui/src/settings_writer.rs` **892 行 / 16 单测**（第 118 步接线时补了 `adopt` 与它的一条单测）。

| Python | Rust | 状态 |
|---|---|---|
| `qconfig.set(item, value)`（`qfluentwidgets/common/config.py:288`） | `ConfigStore::set` | 第 113 步 |
| `SpinBoxSettingCard` → `cfg.set(item, int(v/division))`（`setting_cards.py:85`） | `SettingsWriter` 的整数动作 | 第 117 步 |
| `PercentSpinBoxSettingCard` → `cfg.set(item, v/100)`（`:115`） | `dpi_scale` | 第 117 步 |
| `LineEditSettingCard` → `cfg.set(item, text())`（`:138`） | `aria2_rpc_token` 等文本动作 | 第 117 步 |
| `cfg.set(cfg.downloadFolder, path)`（`setting_page.py:427`） | `download_folder` | 第 117 步 |
| `cfg.set(cfg.browserExtensionPairToken, token_urlsafe(16))`（`browser_service.py:190`） | `regenerate_pair_token` | 第 117 步 |
| 列表写（`headersPreset*` / `identityPreset*` / `category*`） | `defer("<slint 名>")` | 待办 |
| 平台副作用（剪贴板 / 商店 / 扩展解包） | `defer("<slint 名>")` | 待办 |

### 27 个标量动作 ↔ 属性名

`SettingsActions` 共 49 个字段，这一步填了 27 个（全部「一个动作 → 一个 cfg 键」）。

| 字段 | 方法 | cfg 属性名 | 收什么单位 |
|---|---|---|---|
| `folder` | `download_folder` | `downloadFolder` | 路径串（`FolderValidator` 归一成正斜杠绝对路径） |
| `max_task_num` | `max_task_num` | `maxTaskNum` | 1–10 整数 |
| `pre_block_num` | `pre_block_num` | `preBlockNum` | 1–64 整数 |
| `category_enabled` | `category_enabled` | `isCategoryEnabled` | bool |
| `speed_limitation` | `speed_limitation` | `speedLimitation` | **字节**（1024–104857600） |
| `auto_speed_up` | `auto_speed_up` | `autoSpeedUp` | bool |
| `max_reassign_size` | `max_reassign_size` | `maxReassignSize` | 64–… 整数 |
| `preserve_last_modified` | `preserve_last_modified` | `shouldPreserveLastModified` | bool |
| `verify_ssl` | `verify_ssl` | `shouldVerifySsl` | bool（`restart=True`） |
| `name_conflict` | `name_conflict` | `nameConflict` | 字面量 `keepBoth` / `overwrite` / `ask` |
| `use_system_dns` | `use_system_dns` | `shouldUseSystemDns` | bool |
| `proxy_changed` | `proxy_server` | `proxyServer` | 字面量 `Off` / `Auto` / 拼好的 URL |
| `aria2_enabled` | `aria2_enabled` | `isAria2RpcEnabled` | bool |
| `aria2_rpc_port` | `aria2_rpc_port` | `aria2RpcPort` | 1024–65535 |
| `aria2_rpc_token` | `aria2_rpc_token` | `aria2RpcToken` | 原样串（无 validator，不 trim） |
| `aria2_emulate_fingerprint` | `aria2_emulate_fingerprint` | `aria2RpcEmulateFingerprint` | bool |
| `browser_enabled` | `browser_enabled` | `isBrowserExtensionEnabled` | bool |
| `browser_draft_taken` | `browser_draft_taken` | `shouldDraftTakenDownload` | bool |
| `browser_port` | `browser_port` | `browserExtensionPort` | 1024–65535 |
| `pair_token_regenerate` | `regenerate_pair_token` | `browserExtensionPairToken` | 无入参，返回新令牌 |
| `client_profile` | `client_profile` | `clientProfile` | 字面量 `auto` / `raw` / 家族名 |
| `url_scheme_enabled` | `url_scheme_enabled` | `isUrlSchemeRegistered` | bool |
| `check_update_at_startup` | `check_update_at_startup` | `shouldCheckUpdateAtStartup` | bool |
| `run_at_login` | `run_at_login` | `shouldRunAtLogin` | bool |
| `close_mode` | `close_mode` | `closeMode` | 字面量 `Ask` / `Background` / `Quit` |
| `clipboard_listener` | `clipboard_listener` | `isClipboardListenerEnabled` | bool |
| `dpi_scale` | `dpi_scale` | `dpiScale` | 0.0–5.0 浮点（`restart=True`） |

**「动作收配置单位」** 这条惯例在这里兑现：`speed_limitation` 收字节而不是 KB/s，
`dpi_scale` 收 0–5 浮点而不是百分数，`name_conflict` / `close_mode` / `client_profile` 收
字面量而不是下拉框索引。换算全在 `install_settings_actions` 那一层
（`speed::from_display_kb` / `conflict::option_of_index` / `close_mode` 的索引表 …）。

### 22 个未接的动作（`defer` 清单）

`defer` 不是静默丢弃 —— `take_deferred()` 会把动作名交出去，测试里钉住了这份名单。

| 类别 | 动作（Slint 回调名） | 为什么留后 |
|---|---|---|
| 列表写 | `headersPresetPicked` / `headersPresetRemoveRequested` / `headersPresetAddRequested` / `headersPresetEditRequested` / `headersPresetEditClosed` / `headersPresetEditAccepted` | 要读列表、改一项、再整表写回（`JsonConfigSerializer`），删除前还有确认框 |
| 列表写 | `identityPresetToggled` / `identityPresetRemoveRequested` / `identityPresetAddRequested` / `identityPresetEditRequested` / `identityPresetEditClosed` / `identityPresetEditAccepted` | 同上（`isEnabled` 由调用方经 `apply_identity_preset_draft` 继承） |
| 列表写 | `categoryResetRequested` / `categoryRemoveRequested` / `categoryAddRequested` / `categoryEditRequested` / `categoryEditClosed` / `categoryEditAccepted` | 同上（`categoryRules` 是 `JsonList`） |
| 平台副作用 | `copyPairToken` / `openStoreUrl` / `chromiumInstall` / `exportCrx` | 剪贴板、拉起浏览器、解包 CRX 归平台层 |

### 结构

```rust
pub struct SettingsWriter {
    store: RefCell<ConfigStore>,           // 已加载的 cfg + 落盘路径
    changes: RefCell<Vec<ConfigChange>>,   // 已落盘的改动（宿主取走）
    failures: RefCell<Vec<ConfigStoreError>>,
    deferred: RefCell<Vec<&'static str>>,  // 待办清单
}
```

`RefCell` 是必需的：Slint 的回调都是 `Fn`（不可变捕获），而 `ConfigStore::set` 要
`&mut self`。单线程 UI 上不存在跨线程争用。

对外只有「读 + 取走」两类方法（`path` / `config` / `get` / `take_changes` /
`take_failures` / `take_deferred`），写入一律经 `actions()` 装出来的闭包或那 27 个方法。

### 三条结构决策

1. **写盘复用 `ConfigStore::set`**，不在这里重写 `correct()` / `serialize()` /
   「相等早退」/「整文件回写」四件事。第 113 步已经逐条对齐过 `QConfig.set`；
   这里多写一遍只会多一份会漂的副本。
2. **效果靠 `take_changes()` 显式取走，不在这里发信号**。Python 的 `valueChanged` 是
   同步回调，宿主（`boot::run`）拿到 `ConfigChange` 后要决定「重推 UI / 重启服务 /
   提示重启」，这些不属于配置层。
3. **`actions()` 每个闭包只做一次方法调用**。`Rc<Self>` 先 clone 成 49 个绑定再分别
   `move` 进闭包 —— 显式但不绕，`install_settings_actions` 那边看到的就是
   「一个字段一个动作」。

### 与 Python 的已知差异

- **写盘失败的收集方式**：Python 在 `cfg.set` 里直接抛 `OSError`（调用栈中断）；
  这里塞进 `take_failures()`，宿主下一帧统一处理。内存里的值**已经改了**
  （与 Python 的「先改值再 `save()`」一致），但 `ConfigChange` 不会产出
  —— `ConfigStore::set` 在 `save()` 失败时返回 `Err`，改动记录随之丢失。
- **重推 UI 不在写盘路径上**：Python 的 `valueChanged` 会让卡片自己回读 `correct()`
  之后的值（例如端口被夹到 1024），这里要宿主调 `push_settings` 重推。
- **`pair_token_regenerate` 是 `Fn()`**：Python 里重新生成后要 `setText` 到只读框上；
  这里新令牌从 `take_changes()[0].value` 取，闭包返回值被丢弃。

### 十六条新单测

`settings_writer.rs` **892 行 / 16 单测**。全部只碰临时目录里的 `UserConfig.json`，
**不创建 Slint 窗口**（直接调 `(actions.max_task_num)(7)` 这种装箱闭包）。

| 测试 | 钉住什么 |
|---|---|
| `a_scalar_action_writes_the_whole_file` | 一次动作写出全量 42 项；期望文件由 `settings::write` 独立算出 |
| `an_unchanged_value_leaves_the_file_alone` | 同值 → 无改动、无失败、**文件不出现** |
| `an_out_of_range_value_is_clamped_before_it_lands` | `99` → `10`、`80` → `1024` |
| `the_speed_action_takes_bytes` | 收字节；下限夹到 `1024` |
| `text_actions_store_the_literal` | 六个文本/枚举动作的属性名与落盘字面量 |
| `the_pair_token_action_reports_a_fresh_token` | 22 字符 + 落盘 + `previous == ""` |
| `the_zero_dpi_scale_is_already_the_default` | 缺省落盘是 int `0`，`0.0` 相等早退 |
| `the_restart_actions_are_flagged` | `shouldVerifySsl` / `dpiScale` 带 `restart`，`maxTaskNum` 不带 |
| `a_save_failure_is_collected_and_the_value_stays_in_memory` | 父目录被文件占住 → `Save` 失败进 `take_failures`，内存保留新值 |
| `every_scalar_action_reaches_the_store` | 27 个动作全表遍历，属性名逐个对上，且不进待办清单 |
| `the_deferred_actions_name_what_is_still_missing` | 22 个待办名精确匹配，且不碰文件、不进改动列表 |
| `the_config_snapshot_follows_the_writes` | `config()` 快照跟着写 |
| `changes_are_taken_once` | 取走即清空 |
| `open_leaves_a_missing_file_alone` | `open` 不建文件（`qconfig.load` 同序） |
| `an_unknown_attribute_never_comes_out_of_the_actions` | 拼错属性名 → `UnknownAttribute` 进 failures，不 panic |
| `adopt_takes_a_token_that_the_engine_generated` | `Engine::refresh_services` 换掉的令牌被吞回文件；第二次无变化 |

## 设置页接线（`gd3-ui::settings_bridge`）

第 117 步让「动作 → cfg 键」能落盘，但生产路径上 `install_settings_actions` 仍然没人调，
写盘之后的「重推 UI / 刷新服务 / 提示重启」也没有归属。这一步把两端装到窗口上：
`crates/gd3-ui/src/settings_bridge.rs` **434 行 / 11 单测**。

```text
Slint 回调 → SettingsActions → SettingsWriter::set → 落盘
                                      ↓ take_changes()
                          SettingsBridge::drain ─┬→ 宿主钩子（刷新服务 / 提示）
                                                 └→ push_settings（重推整页）
```

### 为什么是轮询，不是给 49 个闭包各包一层

Python 的 `ConfigItem.valueChanged` 是**同步信号**：`cfg.set` 里改完值立刻回调，
卡片自己回读 `correct()` 之后的值（`setting_cards.py:71`）。Slint 的 callback 只连到
我们自己的闭包，没有「属性变化信号」这一层，所以用 `slint::Timer` 按
`POLL_INTERVAL`（50ms）轮询一次。

| 方案 | 代价 |
|---|---|
| 49 个闭包各包一层「写盘 + 重推」 | 49 处重复，漏一个就静默不刷新 |
| 一个 50ms 定时器（本步） | 最多一帧延迟；好处是**一处**收口 |

`drain` 幂等：没有待处理的东西时 `repushed == false`、不调钩子、不重推。

### 定时器不持 `Rc<Self>`

回调里只拿 `Weak<SettingsBridge>` + `Weak<AppWindow>`，所以
「bridge → timer → 回调 → bridge」不构成引用环。`stop()` 把 `timer` drop 掉即彻底停；
宿主只要保证 `Rc<SettingsBridge>` 活到事件循环结束。

### 宿主的钩子（`set_drain_handler`）

轮询只负责「取走 + 重推」，**刷新服务 / 提示重启 / 提示失败**由宿主决定。
`boot::run` 装的处理器做四件事：

1. `failures` 非空 → `eprintln!("配置保存失败：…")`（Python 在这里抛 `OSError`）；
2. `deferred` 非空 → `eprintln!("设置页动作尚未接线：…")`（22 个待办动作，见
   [`settings_writer`] 的边界表）；
3. `restart_attrs()` 非空 → `eprintln!("以下设置重启后生效：…")`（`dpiScale` /
   `language` / `shouldVerifySsl`）；
4. 有改动或失败 → `*engine.config_mut() = writer.config()` → `refresh_services()`
   → `writer.adopt(engine.config())`。

**钩子在重推之前调用**，所以它改出来的值（`adopt` 换掉令牌）也能进同一帧的
`push_settings`。钩子内不要再调 `drain`（`RefCell` 已经可变借用）。

### `boot::run` 的三处改动

| 改动 | 为什么 |
|---|---|
| `ConfigStore::from_config(path, &loaded.config)` | 复用 `load_config` 已经读好的配置。再 `load` 一次会丢掉「文件不存在 → 缺省 `DownloadFolder` 取 `DOWNLOAD_DIR`」与「文件损坏 → 退回缺省」的区分 |
| `Engine` 放进 `Rc<RefCell<Engine>>` | 轮询回调要 `&mut engine` 做 `refresh_services()`，而 `window.run()` 阻塞在同一栈帧里 |
| `writer.adopt(engine.config())`（`Engine::new` 之后） | Python 的 `BrowserService.__init__`（`browser_service.py:189-190`）在令牌为空时 `cfg.set(..., token_urlsafe(16))` 并落盘；引擎持的是克隆，要同步回来 |

无头分支在帧循环之后补一次 `bridge.drain(&window)`（帧数很少时 50ms 定时器可能还没到点）。

### 一条实测纠错：首次启动会写出 `UserConfig.json`

`boot::tests::headless_run_smoke` 原来断言「跑完 `run()` 之后 `UserConfig.json` 仍然不存在」。
接上 `adopt` 之后这条挂了 —— 而**挂得对**：Python 在 `Engine.__init__` 里就构造
`BrowserService`（`app/engine.py:30`），它的 `__init__` 生成令牌并 `cfg.set(..., save=True)`。
所以首次启动写出配置文件是 Python 的既有行为。**改断言不改实现**：

```rust
let written = UserConfig::load(&dir.join(USER_CONFIG_FILE)).expect("启动后应当写出配置");
assert_eq!(written.get_str("Browser", "BrowserExtensionPairToken").unwrap().chars().count(), 22);
assert_eq!(settings::read_i64(&written, "maxTaskNum"), Some(3));
```

### 与 Python 的已知差异

- **重推是整页**：Python 只让**那一张卡**回读自己的值；这里统一 `push_settings` 推全量 42 项。
  同一个值再写一次不触发 Slint 的属性通知，所以正在编辑的输入框不会被重置；真正会变的
  是 `correct()` 夹紧过的项。
- **`deferred` 只记录不执行**：用户点了还没接的按钮时，这里把名字交出去，宿主 `eprintln!`。
- **服务刷新的时机**：Python 的服务每次现读全局 `cfg`，没有「刷新」这一步；这里引擎持克隆，
  只能在 drain 之后显式同步一次（最多晚 50ms）。

### 十一条新单测

| 测试 | 钉住什么 |
|---|---|
| `install_wires_the_actions_to_the_writer` | 窗口回调 → writer → 落盘 → drain 重推 |
| `drain_pushes_the_value_that_correction_produced` | 端口 `80` 被夹到 `1024`，界面跟着变 |
| `a_second_drain_does_nothing` | 幂等（`is_empty` / `!repushed` / 无重启项） |
| `an_unchanged_value_is_not_pushed_again` | 缺省值早退：不落盘、不重推 |
| `a_failed_save_is_reported_and_still_repushes` | 父目录被文件占住 → failures 一条 + 仍然重推 |
| `deferred_actions_are_reported_not_executed` | 两个待办名按点击顺序列出，不碰文件 |
| `restart_items_are_listed_for_the_host` | `shouldVerifySsl` 进重启清单，`maxTaskNum` 不进 |
| `the_drain_handler_sees_the_change_before_the_repush` | 钩子拿到 `changes` 且能读到 writer 的最新值 |
| `the_drain_handler_is_not_called_for_an_empty_drain` | 空 drain 不调钩子 |
| `the_poll_timer_drains_on_its_own` | 1ms 间隔的定时器自己把改动推上窗口 |
| `stop_stops_the_polling` | `stop()` 之后改动留在 writer 里等宿主，UI 不动 |

定时器测试用 `slint::platform::update_timers_and_animations()` + 2ms sleep 手动泵帧
（`HeadlessPlatform` 没覆写 `now()`，定时器走真实单调时钟）。

## 设置页回写的列表半边（`gd3-ui::settings_writer`，第 119 步）

第 117 步只做了「一个动作 → 一个属性名 → 一个值」。剩下 22 个动作全部 `defer`。
这一步把其中 9 个「列表写回」接成真写，剩 13 个（4 个平台副作用 + 9 个对话框开合）
继续待办 —— 后者要 `&AppWindow` 改编辑态属性，而 `SettingsWriter` 不持窗口。

列表写回的共性：**读列表 → 改一项 → 整表写回**。Python 的写法一律是

```python
presets = list(cfg.headersPresets.value)   # 浅拷贝
presets[index] = ...                       # 改一项
cfg.set(cfg.headersPresets, presets)       # 整表写回（触发 correct + serialize + 整文件回写）
```

| 动作 | Python 源 | 写回 |
|---|---|---|
| `headers_preset_pick` | `setting_cards.py:715-717` | `currentHeadersPreset` |
| `headers_preset_remove` | `setting_cards.py:738-775` | `headersPresets`，必要时 `currentHeadersPreset` 回退一格 |
| `headers_preset_submit` | `setting_cards.py:719-736` | 追加（`index == -1`）/ 替换 |
| `identity_preset_toggle` | `setting_cards.py:588-593` | 复制该行、改 `isEnabled`、整表写回 |
| `identity_preset_remove` | `setting_cards.py:595-614` | 删一项、整表写回（**不动**当前索引） |
| `identity_preset_submit` | `setting_cards.py:566-586` | 追加 / 替换，替换时继承 `isEnabled` |
| `category_reset` | `category_settings.py:147-148` | 八预设整表写回 |
| `category_remove` | `category_settings.py:144-145` | 命中才写（Python 用列表推导，重复 id 一起删） |
| `category_submit` | `category_settings.py:129-142` | 按 `categoryId` 命中 → 替换；否则追加 |

`ConfigStore::set` 已经承担 `correct()` / `serialize()` / 相等早退 / 整文件回写，
所以这九个方法只是「拼出新的整表 `Value`」。两个私有助手撑起半边：

```rust
fn list(&self, attr: &str) -> Vec<Value>          // 取逻辑值（已过 correct）
fn set_list(&self, attr: &'static str, items: Vec<Value>)
```

### 照抄 Python 的三处细节

**① `currentHeadersPreset` 存原始索引，不是夹紧后的。** `_onRemoveClicked` 里写的是
`max(0, current - 1)`，其中 `current = cfg.currentHeadersPreset.value`（`:756`）——
没有先夹到 `len - 1`。所以 `current = 5`、列表只剩 3 项时删第 0 项，落盘是 4；
读侧 `currentHeadersPresetIndex()`（`cfg.py:347-348`）再夹成 `len - 1`。
两段夹紧各在各的位置，少一段就会把「用户选的是第 5 项」这个意图擦掉。

**② 列表读写走 `Value` 而不是强类型结构。** Python 复制的是 dict 本身：未知键、
键序都留着。`_onToggled` 更明确 —— `{**preset, "isEnabled": checked}`（`:592`）
展开后 `isEnabled` 若已存在则**保持原位置**，`serde_json::Map` 在 `preserve_order`
下 `insert` 已存在的键正是同一语义（值换、位置不换）。换成
`IdentityPreset` 往返会丢掉未来新增的键。

**③ `headers_preset_remove` 有 `len > 1` 函数级守卫。** Python 不检查，它靠
`canRemove = len(presets) > 1`（`:705`）把删除按钮置灰。Rust 侧没有按钮状态可依赖，
所以把同一条件写成守卫：列表被删空时 `HeadersPresetListValidator.correct` 会补回
「默认」预设（`settings.rs:739`），等于**删了又长出来**。

### `category_*` 反过来走强类型

`CategoryService._save()` 写的是 `[asdict(c) for c in self._categories]`
（`category_service.py:144-146`），`_load` 又是 `Category.fromDict(...)` —— 未知键在
加载时就丢了。所以这里与 Python 同样「过一遍 dataclass」：

```rust
fn categories(&self) -> Vec<CategoryRule> {
    let raw = self.get("categoryRules").map(|value| value.to_string());
    category::parse_rules(raw.as_deref())        // 空表 → 八预设（`_load` 的回落）
}
fn write_categories(&self, rules: Vec<CategoryRule>) {
    let text = category::rules_to_json(&rules);  // 已被单测钉成 json.dumps 口径
    self.set("categoryRules", serde_json::from_str(&text).unwrap());
}
```

`rules_to_json` 的输出串与 `Kind::JsonList` 的 `serialize`（`py_json_dumps`）同口径，
往返不改变字节，所以这里多一次 `from_str` 不会让落盘文本漂移。

新分类的 id 来自 `Category.categoryId` 的 `default_factory`
（`f"cat_{uuid4().hex}"`，`category_service.py:103`）。`category.rs` 原先的注释就写着
「读侧不生成随机 ID，保留空串由写侧处理」—— 这一步补上了那个欠账
`category::new_category_id()`（`cat_` + 32 位小写十六进制），并在 `gd3-ui` 打开
`uuid.workspace = true`。

### `identity_preset_submit` 的替换分支必须继承 `isEnabled`

Slint 的身份预设草稿恒带 `is_enabled: true`（`*EditAccepted` 只有 name / profile /
UA / hosts 四个输入）。但 Python 的编辑对话框会把原值抄回来：

```python
# preset_edit.py:113-123
result = {..., "isEnabled": True}
if self._preset is not None:
    result["isEnabled"] = self._preset.get("isEnabled", True)
```

不继承的话，**编辑一个已停用的预设会把它悄悄启用**。所以替换分支读旧行的
`isEnabled`（缺键当 `true`）覆盖草稿值；新建分支用草稿自己的值。

### 与 Python 的已知差异

- **越界容错**：Python 的 `_onEditClicked` 直接 `presets[index]`，越界抛 `IndexError`；
  这里 `index < 0 || index >= len` 一律静默返回。
- **`category_submit` 合并了添加/编辑两条路**：Python 的 `_onAddClicked` 无条件追加、
  `_onEditClicked` 查不到就 `return`。合并后「`categoryId` 命中 → 替换，否则追加」，
  差别只在「编辑一个已被删掉的分类」这种陈旧 id 场景：Python 不写，这里会追加。
- **`headers_preset_submit` 的 `index == -1` 表示新建**：Slint 的 `headersEditIndex`
  缺省 `-1`（`appwindow.slint:84`），Python 是 `_onAddClicked` / `_onEditClicked` 两个入口。

### 测试（14 条新增，`settings_writer` 共 30 条）

| 测试 | 钉住的行为 |
|---|---|
| `picking_a_headers_preset_writes_the_raw_index` | 原样写 2；`RangeValidator` 把 120 夹成 99 |
| `removing_a_headers_preset_pulls_the_current_index_back_one` | 删第 0 项后 `current` 1 → 0 |
| `removing_a_later_headers_preset_keeps_the_current_index` | 删 `index > current` 不动 `current` |
| `the_last_headers_preset_cannot_be_removed` | `len <= 1` 时零改动 |
| `removing_a_headers_preset_ignores_out_of_range_indexes` | `-1` / `9` 都零改动 |
| `submitting_a_headers_preset_appends_or_replaces` | 追加项只有 `name`/`headers` 两个键；替换改第 0 项 |
| `submitting_a_headers_preset_ignores_out_of_range_indexes` | `index = 5`（列表 1 项）零改动 |
| `toggling_an_identity_preset_keeps_the_other_keys` | 只改 `isEnabled`，键数不变；越界零改动 |
| `removing_an_identity_preset_can_empty_the_list` | 可删到空表（无回落） |
| `submitting_an_identity_preset_inherits_the_stored_enabled_flag` | 停用预设编辑后仍停用；新建走草稿值 |
| `resetting_categories_writes_the_eight_presets` | 8 项、`cat_video` / `视频` / `{default}/Video`、5 个键 |
| `removing_a_category_only_writes_on_a_hit` | 未命中零改动；命中后 7 项 |
| `submitting_a_category_replaces_a_known_id` | `cat_video` 命中 → 替换、仍是 8 项 |
| `submitting_a_category_generates_an_id_for_new_rules` | 空 id → `cat_` + 32 位小写十六进制 |
| `the_deferred_actions_name_what_is_still_missing` | 13 个待办名按点击顺序列出，不碰文件 |

### 验证

- `cargo fmt --all` + `--check`：通过；
- `cargo check -p gd3-ui --all-targets`：零错零警告；
- `cargo test -p gd3-ui --lib settings_writer`：**30 passed**；
- `cargo test -p gd3-ui --lib`：**1022 passed / 0 failed**（1008 + 14）；
- `cargo test --workspace`：**1577 passed / 0 failed / 28 段**（1563 + 14）；
- `cargo clippy --workspace --all-targets -- -D warnings`：**0 warning**；
- `settings_bridge::deferred_actions_are_reported_not_executed` 从
  `categoryResetRequested` 换成 `categoryAddRequested` —— 前者这一步开始真写盘，
  后者才是「开对话框」的待办动作。测试挂得对，改的是测试。

## 更新源纯逻辑层（`gd3-core::sources`）

`app/sources.py`(295) 是「双源竞速」的唯一入口：`github` 与 `gitcode` 各一套端点，
每个 `fetch*` 先用 `buildClient()` 起 HTTP，再在多个镜像之间
`asyncio.gather(..., return_exceptions=True)`（全跑完取最优）或
`asyncio.staggered.staggered_race(...)`（错开 `STAGGER_DELAY` 起跑，第一个成功者胜）。
第 114 步只搬**不碰网络**的那半截，落成 `crates/gd3-core/src/sources.rs`（608 行 / 20 单测）。
放 `gd3-core` 而不是 `gd3-ui`：它是纯逻辑（无网络、无 Qt），而下一步的
`update_service` 在 `gd3-ui`，依赖方向天然正确。

| Python | Rust | 状态 |
|---|---|---|
| `Repo` / `nameOn` / `buildSources` | `Repo` / `name_on` / `build_sources` | 第 114 步 |
| `SourceEndpoints` / `SOURCES` / `SOURCE_ORDER` / `STAGGER_DELAY` | 同名 `SourceEndpoints` / `SOURCES` / `SOURCE_ORDER` / `STAGGER_DELAY` | 第 114 步 |
| `buildDownloadUrl` / `buildRawUrl` / `buildPypiUrl` | `build_download_url` / `build_raw_url` / `build_pypi_url` | 第 114 步 |
| `fetchPypiFile` 的候选表 | `pypi_file_candidates` | 第 114 步 |
| 六处 `raise TaskError("…")` | 六个构造函数 | 第 114 步 |
| `fetchLatestRelease` 的挑选 | `newest_by_version` | 第 114 步 |
| `staggered_race` 的挑选 | `first_success` | 第 114 步 |
| `fetchPypiReleases` 的合并 | `merge_pypi_releases` | 第 114 步 |
| `Release` / `ReleaseAsset` / `fromResponse` | `gd3-ui::release_info` | 第 68 步 |
| `buildClient` / `fetchFile` / `resp.json()` | **未迁移** | 归宿主 |

### 结构

```rust
pub struct SourceEndpoints { pub api, download, raw, raw_infix: &'static str }
pub const SOURCES: [(&str, SourceEndpoints); 2];   // gitcode 在前（Python dict 的插入序）
pub const SOURCE_ORDER: [&str; 2];                 // ["github", "gitcode"]
pub const STAGGER_DELAY: f64;                      // 0.05
pub fn endpoints(source: &str) -> Option<&'static SourceEndpoints>;

pub struct Repo { pub name: String, pub mirrors: Vec<(String, String)> }
impl Repo {
    pub fn new(name: impl Into<String>, mirrors: Vec<(String, String)>) -> Self;
    pub fn name_on(&self, source: &str) -> Option<&str>;
    pub fn build_sources(&self) -> Vec<&'static str>;
}
pub fn app_repo() -> Repo;                         // update.py:9 的 APP_REPO

pub const PYPI_MIRRORS: [(&str, &str); 2];
pub const PYPI_FILES: &str;
pub const PYPI_FILES_MIRROR: &str;
pub fn build_download_url(repo: &Repo, tag: &str, asset: &str, source: &str) -> Option<String>;
pub fn build_raw_url(repo: &Repo, branch: &str, path: &str, source: &str) -> Option<String>;
pub fn build_pypi_url(package: &str, source: &str) -> String;
pub fn pypi_file_candidates(url: &str) -> Vec<String>;

// 六条失败文案：Python 直接 raise，这里摊成构造函数，宿主拿不到任何源时用它
pub fn latest_release_failure(repo_name: &str) -> TaskError;
pub fn json_failure(repo_name: &str, branch: &str, path: &str) -> TaskError;
pub fn raw_file_failure(repo_name: &str, branch: &str, path: &str) -> TaskError;
pub fn download_failure(repo_name: &str, tag: &str, asset: &str) -> TaskError;
pub fn pypi_releases_failure(package: &str) -> TaskError;
pub fn pypi_file_failure(url: &str) -> TaskError;

// 三个聚合口径
pub fn first_success<T, E>(completions: Vec<(usize, Result<T, E>)>) -> Option<(usize, T)>;
pub fn newest_by_version<T, E>(
    results: Vec<Result<T, E>>,
    version_of: impl Fn(&T) -> &str,
) -> Option<T>;
pub fn merge_pypi_releases<E>(
    results: Vec<Result<Map<String, Value>, E>>,
) -> Option<Map<String, Value>>;
```

### 六条实证语义（2026-10-19 实读 `app/sources.py` + CPython 3.13 实测）

1. **`SOURCES` 的插入序是 `gitcode` 在前**（`sources.py:38`），`SOURCE_ORDER` 才是
   `("github", "gitcode")`（`:53`）。`buildSources()` 按 `SOURCE_ORDER` 过滤，所以尝试顺序
   恒为 github → gitcode，与 dict 的书写顺序无关。Rust 侧两个常量各按各的顺序落地：
   `SOURCES` 保 dict 序只为了让「表内容」能逐字对比。
2. **`nameOn("github")` 恒返回 `self.name`**，不看 `mirrors["github"]`——`mirrors` 里放
   `github` 键是死数据（`app/update.py:9` 的 `APP_REPO` 正是如此）。其它源走
   `mirrors.get(source)`，缺键 → `None` → `buildSources()` 跳过该源。
3. **`buildPypiUrl` 的未知源回退到 PyPI 官方**（`PYPI_MIRRORS.get(source, PYPI_MIRRORS["github"])`），
   而 `buildDownloadUrl` / `buildRawUrl` 的未知源是 `KeyError`（`SOURCES[source]`）。
   Rust 侧前者照抄回退，后者返回 `None`。
4. **`staggered_race` 全失败时返回 `(None, None, exceptions)`，不抛**：实测
   `[bad, bad]` → `(None, None, [RuntimeError, RuntimeError])`、`[]` → `(None, None, [])`。
   `sources.py` 的 `if index is None: raise TaskError(...)` 就是这条路径。
5. **`staggered_race` 的胜者不一定是下标最小的**：实测 `[slow, fast]`（`delay=0.01`）→
   `('fast', 1, [CancelledError(), None])`——第一个协程还没失败、但已过 `delay` 秒时下一个
   也会起跑，谁先完成谁赢；赢家那格是 `None`，**被取消的输家是 `CancelledError()`**。
   所以 `first_success` 把「完成顺序」当入参，纯逻辑层不模拟时序。
6. **`fetchPypiReleases` 的合并方向是反的**：`for result in reversed(succeeded): releases |= result`
   ——先合并**后**到的源，再合并靠前的源，于是 `SOURCE_ORDER` 里靠前的 github 覆盖 gitcode；
   键序按首次插入位置保留（实测 `['1.0', '2.0', '3.0']`，`2.0` 取 github 的值）。
   `serde_json` 开了 `preserve_order`，`Map` 就是 `IndexMap`，口径一致。

`gather` 那条路径（`fetchLatestRelease`，`:126-134`）是**严格大于**：
`if parseVersion(result.version) > parseVersion(chosen.version)`，同版本取**先出现**者。
`parse_version` 已在 `gd3-core::version`（去尾零、元组比较），`newest_by_version` 直接复用，
用闭包取版本串——`Release` 不重复定义，两个 crate 都能调。

### 与 Python 的已知差异

- **未知源的 `KeyError` 退化成 `None`**：`buildDownloadUrl(repo, …, source="x")` 在 Python
  抛 `KeyError("x")`，Rust 返回 `None`。调用方只可能从 `Repo::build_sources()` 取值，
  两者不可达。
- **`mirrors` 用 `Vec` 而不是 `Map`**：Python 的 dict 保插入序、重复键后者覆盖；
  `name_on` 用 `iter().rev().find()` 复刻「后者覆盖」。用 `Map` 会静默丢掉重复键这条语义。
- **`merge_pypi_releases` 的失败类型是泛型**：Python 的 `return_exceptions=True` 会把任意
  `BaseException` 塞进结果表，Rust 侧只关心「是不是 `Err`」。
- **`fetchLatestRelease` 的 `headers` 分叉没搬**：`source == "github"` 时加
  `accept: application/vnd.github+json`，这属于 HTTP 层（`gd3-net`），不在本模块。
- **网络与解包全归宿主**：`buildClient` / `fetchFile` / `resp.json()` / `staggered_race` 的
  真调度都不在这里；`update_service` 接线时由宿主提供「候选表 + 完成顺序」，
  纯逻辑层只回答「拿到这些结果之后选谁」。

### 二十条新单测

`crates/gd3-core/src/sources.rs` **608 行 / 20 单测**（`cargo test -p gd3-core --lib` 由 313 变 333）。

| 测试 | 钉住什么 |
|---|---|
| `source_order_differs_from_the_dict_order` | `SOURCES[0]` 是 gitcode、`SOURCE_ORDER[0]` 是 github |
| `the_endpoint_table_is_mirrored_verbatim` | 8 个端点字段逐字 + `STAGGER_DELAY == 0.05` |
| `github_ignores_the_mirror_entry` | `mirrors["github"]` 是死数据 |
| `a_missing_mirror_drops_the_source` | 缺镜像 → 该源被 `build_sources()` 跳过 |
| `an_unknown_mirror_key_is_carried_but_never_tried` | `name_on("other")` 有值但不在 `SOURCE_ORDER` 里 |
| `a_duplicate_mirror_key_keeps_the_last_value` | `Vec` 复刻 dict 的「后者覆盖」 |
| `download_urls_match_the_python_strings` | 两源下载地址金值（`github` / `gitcode`） |
| `raw_urls_match_the_python_strings` | 两源 raw 地址金值（含 `/-/git/raw/` 中缀） |
| `an_unknown_source_has_no_endpoints` | 未知源 → `endpoints` / `build_*` 全 `None` |
| `pypi_urls_fall_back_to_the_official_index` | 未知源回退 + gitcode 的 `/json/` 特例 |
| `pypi_file_candidates_only_mirror_pythonhosted_urls` | 只有 `files.pythonhosted.org` 才加镜像候选 |
| `the_app_repo_mirrors_update_py` | `APP_REPO` 的名字、单镜像、`build_sources()` |
| `newest_by_version_skips_failures` | 跳过 `Err` 后取版本最大者 |
| `newest_by_version_keeps_the_first_on_a_tie` | `v4.4.0` 与 `4.4` 同版本 → 取先出现者 |
| `newest_by_version_returns_none_when_every_source_fails` | 全 `Err` / 空表 → `None` |
| `first_success_returns_the_first_completion` | 先完成的下标 1 胜出，不是下标 0 |
| `first_success_returns_none_when_every_source_fails` | 全 `Err` / 空表 → `None` |
| `merge_pypi_releases_lets_the_earlier_source_win` | 靠前源覆盖 + 键序 `['1.0','2.0','3.0']` |
| `merge_pypi_releases_returns_none_when_every_source_fails` | 全 `Err` → `None` |
| `failure_messages_match_the_python_wording` | 六条 `TaskError` 的 `format()` 逐字 |

## 更新源的网络半边（`gd3-net::sources`）

第 114 步的 `gd3_core::sources` 只搬了纯逻辑（URL 拼接、失败文案、挑选口径）。这一步补上
`app/sources.py` 剩下的半边：**竞速、`gather` 合并、流式落盘**。
`crates/gd3-net/src/sources.rs`（**1204 行 / 26 单测**）不碰 `gd3-ui`，`update_service`
接线时依赖 `gd3-net` 即可。

### 对照表

| Python | 这里 | 说明 |
|---|---|---|
| `fetchJson(repo, branch, path)`（`sources.py:135`） | `fetch_json` | `staggered_race` 两个 raw 地址，返回 `(JSON, 获胜源)` |
| `fetchLatestRelease(repo)`（`:101`） | `fetch_latest_release` | `gather` 两个 API，取 `parseVersion` 最大者 |
| `probeDownloadUrl(repo, tag, asset)`（`:191`） | `probe_download_url` | `Range: bytes=0-0` 竞速，返回可用 URL |
| `fetchReleaseAsset(repo, tag, asset, path, onProgress)`（`:204`） | `fetch_release_asset` | 先探测再流式下载，返回实际用的 URL |
| `fetchPypiReleases(package)`（`:241`） | `fetch_pypi_releases` | `gather` 两个索引，按 `SOURCE_ORDER` 反序合并 |
| `fetchPypiFile(url, path, onProgress)`（`:265`） | `fetch_pypi_file` | `pythonhosted` 多一个镜像候选，竞速探测后下载 |
| `app/client.py::fetchFile(url, path, onProgress)`（`:162`） | `write_stream` + `Transport::download` | 流循环与传输层分开，前者可离线测 |

`fetchRawFile`（`sources.py:162`）**没有对应函数**：`gd3-core` 里的 raw 下载走 `HttpEngine`，
那条路径已有自己的重试与分片语义，不该被这里的「Range 探测 + 单流」覆盖。

### 结构

| 位置 | 内容 |
|---|---|
| `sources.rs:60-64` | `JSON_TIMEOUT`(15s) / `PROBE_TIMEOUT`(10s) / `FETCH_READ_TIMEOUT`(30s) —— 对应 `buildClient(timeout=15)` / `(timeout=10)` / `(readTimeout=30)` |
| `:91` | `FetchError { Status(u16), Transport(String) }` + `Display` / `Error` |
| `:110` | `BoxFetch<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>` |
| `:115` | `trait Transport { get_json / probe / download }`，三个方法都返回 `BoxFetch` |
| `:142` | `WreqTransport { profile }` —— 真实现，`build_client` + `crate::engine::to_header_map` |
| `:224` / `:231` | `describe()`（任意 `Display` → `FetchError::Transport`）/ `content_length()` |
| `:243` | `write_stream()` —— `fetchFile` 的流循环 |
| `:302` / `:309` | `Attempt<'a, T>` / `race_first_ok()` —— `staggered_race` 的等价物 |
| `:359` / `:367` / `:379` | `raw_attempts()` / `download_attempts()` / `boxed()` |
| `:387`–`:570` | 七个 `pub async fn` + 私有 `Candidate` |
| `:573` | `mod tests`（26 条） |

**为什么 HTTP 要走 `Transport` 抽象**：`staggered_race` 的「谁先完成谁赢」与 `gather` 后的
「按 `SOURCE_ORDER` 合并」都是**时序语义**。没有这层，就只能靠真网络去验证，而真网络既慢
又不确定。桩可以精确指定每个源的延迟与失败形态。

**为什么 `fetch_latest_release` 返回原始 JSON 而不是 `Release`**：`Release` 定义在
`gd3_ui::release_info`（第 68 步），`gd3-net` 不能反向依赖 `gd3-ui`。所以这里返回
`(Value, &'static str)`，`version_of` 闭包由调用方给（就是 `tag_name` → `name` → 空串那三步）。

**为什么 `staggered_race` 不 spawn**：用 `FuturesUnordered` + `select!` 手写，attempt 可以
借用 `&dyn Transport`，不必先 `Arc` 一遍。胜出即 `return`，其余 future 随 `FuturesUnordered`
一起 drop —— 与 Python 取消输家等价。

**为什么 `fetchFile` 要拆**：真正的 `wreq` 调用只剩「建 client → 发请求 → 拿 `bytes_stream()`」，
空块跳过 / 进度百分比 / 父目录创建全在 `write_stream` 里，能离线测。

### 六条实证语义

1. **`buildClient` 的三个超时口径不同**：`fetchJson` / `fetchLatestRelease` 用 `timeout=15`
   （整体），`probeDownloadUrl` 用 `timeout=10`，`fetchFile` 用 `readTimeout=30` —— **不给总超时**，
   只限单次读。所以 `FETCH_READ_TIMEOUT` 传给 `wreq` 的读超时，不是 `timeout`。
2. **`raise_for_status()` 只对 4xx/5xx 抛**：3xx 会继续 `resp.json()`。这里 `>= 400` 才判
   `FetchError::Status`，与 `gd3-net::hf` 的口径一致。
3. **`int(headers.get("content-length", b"0"))` 在非数字上抛 `ValueError`**，会把整个下载打断。
   这里退回 0（= 不回调进度）—— 与「没给 content-length」同一条路径。
4. **`fetchPypiReleases` 的合并方向是反的**（第 114 步已记）：`for result in reversed(succeeded)`
   → 靠前的源覆盖靠后的，键序按首次插入位置。`merge_pypi_releases` 直接复用。
5. **`gather` 那条路径是严格大于**：`if parseVersion(result.version) > parseVersion(chosen.version)`，
   同版本取**先出现**者（`newest_by_version`）。
6. **`fetchLatestRelease` 的 `accept` 头只给 github**：
   `headers = {"accept": "application/vnd.github+json"} if source == "github" else {}`。

### 与 Python 的已知差异

- **`content-length` 解析失败当 0**（见上第 3 条）。
- **非 2xx 一律 `Status`**：Python 3xx 会继续解析，这里只有 `>= 400` 才算错，与 Python 一致；
  差异在于 `Status` 是**枚举值**而不是异常对象，调用方拿不到 `resp.text`。
- **未知源的 URL 拼接被跳过**：`build_download_url` / `build_raw_url` 对未知源返回 `None`
  （第 114 步的刻意设计），所以那种源不会进竞速。
- **`fetch_latest_release` 的版本提取只认字符串**：Python 是 `str(data.get(key) or "")`，
  数字会被 `str()` 化；这里非字符串当空串（`{"tag_name": 5}` 这种输入不存在）。
- **`write_stream` 出错也 `flush()`**：Python 的 `with open(...)` 在异常路径上会 `close()`
  （= flush）。`tokio::fs::File` 自带缓冲，不 flush 会丢掉最后一段 —— 这是**照抄语义**，
  不是新增行为。
- **`WreqTransport` 不设 `readTimeout` 之外的连接池参数**：`build_client` 的 profile 决定指纹，
  其余走默认。

### 二十六条新单测

`crates/gd3-net/src/sources.rs` **1204 行 / 26 单测**（`cargo test -p gd3-net --lib` 由 73 变 99）。

| 分组 | 测试 | 钉住什么 |
|---|---|---|
| 竞速 | `fetch_json_takes_the_first_source_that_answers` | 首个 `Ok` 胜出，返回源名 |
| | `fetch_json_prefers_the_faster_source` | github 80ms / gitcode 0ms / `STAGGER_DELAY` 50ms → gitcode 赢 |
| | `fetch_json_reports_the_python_failure` | 全失败 → `json_failure` 文案 |
| 探测 | `probe_download_url_asks_for_a_single_byte` | 两个源都收到 `Range: bytes=0-0` |
| | `probe_download_url_reports_the_download_failure` | 全失败 → `download_failure` 文案 |
| 资产 | `fetch_release_asset_downloads_the_probed_url` | 探测胜者的 URL 被下载 |
| | `fetch_release_asset_does_not_download_when_the_probe_fails` | 探测失败 → 不建文件 |
| | `fetch_release_asset_reports_a_download_failure` | 下载失败 → 错误透传 |
| PyPI 索引 | `fetch_pypi_releases_lets_the_earlier_source_win` | 键序 `["1.0","3.0","2.0"]`，`1.0` 取 github 的值 |
| | `fetch_pypi_releases_keeps_going_with_one_source_down` | 一个源失败仍合并另一个 |
| | `fetch_pypi_releases_reports_the_failure` | 全失败 → `pypi_releases_failure` |
| | `fetch_pypi_releases_rejects_a_payload_without_releases` | 缺 `releases` 键算失败项 |
| PyPI 文件 | `fetch_pypi_file_falls_back_to_the_mirror` | `pythonhosted` 地址会加 BFSU 候选 |
| | `fetch_pypi_file_does_not_mirror_other_urls` | 其它地址只有一个候选，失败文案用原 URL |
| | `fetch_pypi_file_reports_the_failure` | 全失败 → `pypi_file_failure` |
| release | `fetch_latest_release_picks_the_newest_version` | `parse_version` 最大者 |
| | `fetch_latest_release_skips_a_failing_source` | 一个源失败仍选另一个 |
| | `fetch_latest_release_reports_the_failure` | 全失败 → `latest_release_failure` |
| | `fetch_latest_release_sends_the_github_accept_header` | github 带 `accept`、gitcode 不带 |
| 流式落盘 | `write_stream_reports_progress_against_content_length` | `received / total * 100` |
| | `write_stream_skips_the_callback_without_a_content_length` | `total == 0` 不回调 |
| | `write_stream_creates_the_parent_directory` | `mkdir(parents=True)` |
| | `write_stream_reports_a_transport_error` | 流中断 → `Transport`，已写内容保留 |
| 杂项 | `content_length_falls_back_to_zero` | 缺头 / 非数字 → 0 |
| | `fetch_error_renders_both_shapes` | `HTTP 404` / 原文 |
| | `wreq_transport_keeps_its_profile` | `profile()` 回原值 |

### 本轮修掉的两个真缺陷

两个都是**先写完、测试才照出来**的，值得记：

1. **`race_first_ok` 一个 attempt 都没起跑**：原写法是循环里 `sleep(Duration::ZERO)` 当「立即」，
   而 tokio 定时器有 1ms 粒度，零时长 `sleep` 首次 poll 仍是 `Pending`；此时
   `FuturesUnordered` 是空的，`inflight.next()` 立刻给 `Ready(None)`，`biased` 选中后者直接
   `break` → 恒 `None`。8 条测试全挂在这一条。改法：**循环外先 `inflight.push(make(0))`**，
   `next = 1`。
2. **`write_stream` 出错丢数据**：`tokio::fs::File` 自带缓冲，原来在 `Err` 分支直接
   `return Err(..)`，文件为空。Python 的 `with open(...)` 在异常路径上会 `close()`（= flush）。
   改法：`failure: Option<FetchError>` + `break`，循环后**无条件 `flush()`** 再返回。

## 更新检查（`gd3-ui::update_service`）

`app/services/update_service.py`(335) 把四件事混在一个类里：①状态表（`_infos` +
`_emit` 的合并语义）、②`versions.json` 的解析与挑选、③HTTP 下载 + 校验 + 解包、
④`subprocess.Popen` 起 updater。`crates/gd3-ui/src/update_service.rs`（**1492 行 /
49 单测**，第 115 步）只搬**决策**（写完是 2408 行，`UpdateRuntime` 的状态机是第 124 步
补的），执行全归宿主 —— **执行层第 135 步落在 `gd3-ui::update_exec`**，见
「更新检查的执行层」一节。

### 边界

| Python | Rust | 状态 |
|---|---|---|
| `UpdateState` / `UpdateInfo` | 同名 | 本步 |
| `_emit` 的合并与缺省值 | `UpdateTable::emit` + `EmitFields` | 本步 |
| `_check` | `plan_check` → `Vec<UpdateStep>` | 本步 |
| `_downloadApp` 的挑选 | `plan_app_download` → `AppDecision` | 本步 |
| `_downloadPack` 的挑选 | `plan_pack_download` → `PackPlan` | 本步 |
| `toChecksum != parseChecksum(sha)` | `checksum_accepted` | 本步 |
| `apply` | `plan_apply` → `Vec<ApplyAction>` | 本步 |
| `_startUpdater` 的 argv | `updater_command` → `UpdaterCommand` | 本步 |
| `installPendingPacks` 的改名 | `pending_pack_id` | 本步 |
| `buildPlatformKey` | `build_platform_key` | 本步 |
| `fetchJson` / `fetchReleaseAsset` / `fetchRawFile` | `gd3-core::sources` + `gd3-net` | 第 114 步 |
| `zipfile` / `tarfile` / `hdiutil` / `Popen` / `toChecksum` 的实算 | **未迁移** | 归宿主 |

### 八条实证语义（实读 `app/services/update_service.py`）

1. **`_emit` 只合并五个键**：`replace(current, state=state, **{k: v for k, v in kwargs
   if k in ("label", "currentVersion", "latestVersion", "progress", "error")})`。没传的
   键**保留旧值**——`_emit("app", IDLE)` 不会清空 `label` / `currentVersion`。新建时的
   缺省是 `label = targetId`、`currentVersion = VERSION if targetId == "app" else ""`、
   `latestVersion = ""`、`progress = 0`、`error = None`。
2. **`_infos` 是 dict，`apply()` 按插入序遍历**：先跑所有 `READY` 的 pack，再处理 `app`。
   `_emit` 对已存在的键**原地更新**、不改位置。顺序是硬语义——`_startUpdater` 会拉起
   updater 顶掉当前进程，必须排在所有 pack 之后。
3. **`patch.get("from") == VERSION` 是精确相等**，不是 `isNewer`：补丁只对「正好是
   当前版本」的安装生效。
4. **`if patch` / `if not full` 走的是 Python 真值**：`patches[platformKey]` 是 `{}` 时
   整条补丁路径不生效；`full[platformKey]` 是 `{}` / `""` / `0` 时一律
   `TaskError("当前平台无可用更新")`。Rust 侧用私有的 `py_truthy` 复刻。
5. **`sha` 为空串就跳过校验**（`if sha and ...`）；`parseChecksum` 取第一个空白分隔
   字段并 `lower()`，所以 `"abc123  app.exe"` 与 `"ABC123"` 等价。
6. **`gdMinVersion` 的跳过条件是 `VERSION <= gdMin` 且 `gdMin != VERSION`**：
   `not isNewer(remoteGdMin, VERSION)` 排掉「比当前旧」，再排掉相等。
7. **`buildPlatformKey` 的两个 map 都是 `KeyError`**（`OS_MAP[sys.platform]` 先求值），
   退化成 `None`；`platform_key_failure` 复刻 `toTaskError` 后的
   `发生了意外错误：'i686'`（`str(KeyError(x))` 带引号）。
8. **`tag = f"v{version}"`**：`versions.json` 里的 `version` 不带 `v` 前缀。
   解包三分支里 **darwin 优先于后缀**（macOS 的 `full[platformKey].file` 是 `.zip`
   也走 `hdiutil`），其余按 `Path.suffix == ".xz"` 分 `tarfile` / `zipfile`。

### 三条结构决策

1. **`plan_check` / `plan_apply` 返回动作表而不是直接改状态**：纯函数、可断言 emit
   顺序；宿主按序喂 `UpdateTable::emit` 并转发 `changed` 信号。
2. **`EmitFields` 用 `Option` 表达「没传」**：`None` = 保留旧值，正好对上
   `kwargs.get(...)` 的语义；`UpdateTable::emit` 是唯一改表入口。
3. **`updater_command` 的返回值带 `Detach`**：`Win32` =
   `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`，`Unix` = `start_new_session=True`。
   两个「记日志然后 return」的分支用 `UpdaterSkip::{Missing, NoPayload}` 区分。

### 与 Python 的已知差异

- **`toTaskError` 归宿主**：`_download` 的 `except Exception` 那一层不在本模块，
  宿主把 IO / HTTP 错误包成 `TaskError` 再 `emit(FAILED)`。本模块只产出文案
  （`versions_failure` / `checksum_failure` / `platform_failure` / `dmg_no_app` /
  `hdiutil_failure` / `platform_key_failure`）。
- **非 dict 的 `data.get("app", {})`**：Python 抛 `AttributeError`，Rust 的
  `Value::get` 对非对象返回 `None`，当空表处理。
- **`_emit(error=None)` 与「不传 error」在 `replace` 分支有差别**（前者清空）；
  Python 侧没有调用点走那条路，Rust 用 `Option<TaskError>` 表示「不覆盖」。
- **`pending_pack_id("_pending")` 返回 `None`**：Python 的 `removesuffix` 给出空串，
  `target = featuresDir / ""` == `featuresDir`，`rmtree` 会把整个 FEATURES_DIR 删掉。
  该分支是纯数据丢失，不复刻。
- **`packData.get("file", default)` 的非字符串值**：Python 会把 `5` 拼成
  `dist/packs/5`，Rust 只认字符串，其余回退 `<packId>.zip`。
- **解包与进程启动不在这里**：`zip` / `xz2` crate 不在离线缓存里，`zipfile` /
  `tarfile("r:xz")` / `hdiutil` / `Popen` 全归宿主。第 135 步的 `update_exec` 把 zip 解包
  接上了（复用 `extension_crx` 的读取器）、`xz2` / `hdiutil` 仍然缺、`Popen` 只交命令不拉起。

### 四十九条新单测

`crates/gd3-ui/src/update_service.rs` **1492 行 / 49 单测**（`cargo test -p gd3-ui --lib`
由 932 变 981）。

| 组 | 钉住什么 |
|---|---|
| `emit_defaults_*` / `emit_keeps_*` / `emit_overwrites_*` / `emit_records_*` | 五键合并、`app` 与 pack 的 `label` / `currentVersion` 缺省、`error` 落地 |
| `emit_keeps_the_insertion_order` | 同键原地更新、不改位置 |
| `can_download_only_accepts_available` | 只放行 `AVAILABLE` |
| `update_state_ints_match_the_int_enum` | `IntEnum` 的 1..6 |
| `build_platform_key_*` / `platform_key_failure_*` | 两 map 的五个映射、未知键 → `None`、失败文案带引号 |
| `pack_dir_candidate_filters_hidden_and_pending` | `.` 开头与 `_pending` 结尾 |
| `plan_check_*`（10 条） | `CHECKING` 先行、fetch 失败、版本比较、android 早退、pack 的六种跳过条件、`gdMinVersion` 三种边界 |
| `plan_app_download_*`（9 条） | 补丁优先 / `from` 不等 / 空 patch 表 / 完整包 / `.xz` / darwin 优先 / 缺 `full` / 缺 `file` |
| `plan_pack_download_*` | 缺省文件名、远端路径前缀、落地名恒为 `<packId>.zip` |
| `checksum_accepted_*` | 空 sha 跳过、大小写不敏感、只取第一段、不匹配 |
| `plan_apply_*` | pack 先于 updater、非 `READY` 不动作、没有 `app` 条目不拉 updater |
| `pending_pack_id_strips_the_suffix` | 后缀剥离、空 id 拒绝、重复后缀 |
| `updater_name_*` / `app_dir_*` / `updater_command_*`（5 条） | `updater.exe` 与 `updater`、macOS 上溯两级、argv 五项、补丁优先于 `app_new`、`Detach` 分平台、两个 skip 分支 |

## 设置页覆盖层的开合（`gd3-ui::settings_dialogs`，第 120 步）

第 119 步之后还剩 13 个 `defer` 项。这一步接掉 9 个「开对话框 / 关对话框」动作，剩 4 个
平台副作用（剪贴板 / 商店 URL / CRX 解包 / 拉起浏览器）。

### 为什么另起一层，而不是塞进 `SettingsWriter`

这九个动作一个都不写 cfg：它们只改 `AppWindow` 上的 `headersEditVisible` /
`headersEditIndex` / `identityEditVisible` / `categoryEditId` 之类属性。第 117 步把它们
放进 `SettingsActions` 并 `defer`，是因为动作集当时是「Slint 回调 → Rust 闭包」的唯一
通路；但 `SettingsWriter` 只持 `ConfigStore`，拿不到 `&AppWindow`。

这一步的选择是**把这九个字段从 `SettingsActions` 删掉**（49 → 40 字段），新建
`settings_dialogs::install(&window, Rc<SettingsWriter>)` 直接装这九个回调：

```rust
pub fn install(window: &AppWindow, writer: Rc<SettingsWriter>);
pub fn open_headers(window: &AppWindow, writer: &SettingsWriter, index: Option<i64>);
pub fn open_identity(window: &AppWindow, writer: &SettingsWriter, index: Option<i64>);
pub fn open_category_new(window: &AppWindow);
pub fn open_category(window: &AppWindow, writer: &SettingsWriter, category_id: &str);
pub fn close_headers / close_identity / close_category(window: &AppWindow);
```

`install_settings_actions` 继续装 `*EditAccepted`（提交草稿要 `SettingsActions`）。分工
必须按**回调**切而不是按「主题」切：`window.on_<callback>` 对同一个回调二次安装是
**替换**而不是叠加，同一个回调装两处会静默丢掉前一处。

| Slint 回调 | Python 源 | 这里 |
|---|---|---|
| `headersPresetAddRequested` | `setting_cards.py:719-727` | `open_headers(.., None)` |
| `headersPresetEditRequested` | `setting_cards.py:729-736` | `open_headers(.., Some(i))` |
| `headersPresetEditClosed` | 覆盖层「取消」 | `close_headers` |
| `identityPresetAddRequested` | `setting_cards.py:565-574` | `open_identity(.., None)` |
| `identityPresetEditRequested` | `setting_cards.py:576-586` | `open_identity(.., Some(i))` |
| `identityPresetEditClosed` | 覆盖层「取消」 | `close_identity` |
| `categoryAddRequested` | `category_settings.py:129-133` | `open_category_new` |
| `categoryEditRequested` | `category_settings.py:135-142` | `open_category` |
| `categoryEditClosed` | 覆盖层「取消」 | `close_category` |

每个闭包各持一份 `Weak<AppWindow>` 与 `Rc<SettingsWriter>`：`window.run()` 期间回调活到
窗口销毁之后，持强引用会成环。

### 三条越界守卫

- **身份预设照抄 Python 的守卫**：`_onEditClicked` 自己写着 `if index < 0 or
  index >= len(presets): return`（`setting_cards.py:578-579`）。
- **标头预设没有可照抄的守卫**：Python 那边直接 `presets[index]`（`:732`），越界会抛
  `IndexError`。这里静默返回 —— Slint 的 `headersEditIndex` 是普通整数属性，越界索引
  只可能来自手改过的模型。
- **库分类查不到就不开**：`categoryById` 返回 `None` 时 `return`
  （`category_settings.py:137-138`）。

列表长度一律从 `SettingsWriter::get` 的**逻辑值**取（也就是 `correct()` 之后的那份），
与 `open_*_edit` 里重新 `parse_*` 的输入同源。为此把
`SettingsWriter::categories`（`settings_writer.rs:536`）从私有改成 `pub` ——
`CategoryService._load` 口径的「空表 → 八预设」在这一层，重复实现会漂。

### 顺手补的既有缺陷：确定之后不收起覆盖层

Slint 的覆盖层不会因为 `*EditAccepted` 自己收起来。第 47 步的 identity 分支已经显式
`set_identityEditVisible(false)`，但第 45 步的 category 与第 46 步的 headers 一直漏着 ——
点「确定」写完 cfg，框还杵在那儿。这一步在 `install_settings_actions` 的两个 accept
闭包里各补一行：

```rust
let headers_win = window.clone_strong();
window.on_headersPresetEditAccepted(move |d| {
    headers_win.set_headersEditVisible(false);   // ← Python 的 dialog.exec() 返回真值即关框
    ...
});
```

### 十二条新单测

`settings_dialogs.rs` 的 12 条测试走「headless 窗口 + 临时目录 writer」：

| 覆盖面 | 断言 |
|---|---|
| 标头添加态 | `visible`、`index == -1`、名称空、标头回显 `currentHeaders()` |
| 标头编辑态 | `index == 0`、名称「默认」、标头非空 |
| 标头越界 | 索引 9 → 不开框、`headersEditIndex` 一个都没动 |
| 标头取消 | 开→关，`visible == false` |
| 身份添加态 | 名称/hosts/UA 空、TLS 指纹落「跟随全局默认」（索引 0） |
| 身份编辑态 | 经公开动作 `identity_preset_edit_submit(-1, ..)` 追加一项后回显 |
| 身份越界 | 索引 5 → 不开框 |
| 身份取消 | 开→关 |
| 分类添加态 | `categoryEditId == ""`、字段全空、icon 索引 0 |
| 分类编辑态 | `cat_video` → id/名称/扩展名逗号连接/`{default}/Video` |
| 分类查不到 | `cat_nope` → 不开框 |
| 分类取消 | 开→关 |

一条实测纠错：`identityPresets` 的缺省**不是空表**，`Network/IdentityPresets` 缺省就带
一项「百度网盘客户端」（`tests/fixtures/user_config_default_linux.json`）。第一版测试
按「空表」写，`list_len == 1` 的断言挂在 2 上 —— 改测试（按实际条数追加），不改实现。

### 与 Python 的已知差异

- **`headers_preset_edit_requested` 的越界**：Python 抛 `IndexError`，这里静默忽略。
- **`category_edit_requested` 的陈旧 id**：Python 的 `CategoryService` 持内存表，删掉的
  分类立刻查不到；这里每次都从 cfg 重读，语义相同（都是「查不到就不开」）。
- **收框时机**：Python 是模态 `dialog.exec()`，收框与写 cfg 在同一栈帧；这里 `*EditAccepted`
  先收框再调动作，宿主看到改动时框已经关了（差一帧以内）。

## 浏览器扩展的 CRX 容器（`gd3-ui::extension_crx`，第 121 步）

设置页的四张扩展卡（复制配对令牌 / 商店链接 / 本地安装 / 导出 crx）都绕不开
`app/assets/chrome_extension.crx`：Python 侧 `loadCrx()`（`desktop.py:118`）读
`:/res/chrome_extension.crx`，`BrowserService.install()`（`browser_service.py:196-205`）
先拆 CRX 头再 `zipfile.ZipFile(...).extractall(folder)`。离线缓存里没有 `zip` crate，
但 `flate2` 在（1.1.10，本来就在 `Cargo.lock` 里，slint 的传递依赖），所以这一步
**自己读 ZIP 中央目录**，把容器这一层做实。`crates/gd3-ui/src/extension_crx.rs`
（**695 行 / 13 单测**）。

### 覆盖面

| Python 源 | 语义 | 这里 |
|---|---|---|
| `desktop.py:118` `loadCrx()` | `QResource(":/res/chrome_extension.crx")` 的字节 | `crx_bytes()`（`include_bytes!`，341 464 字节） |
| `browser_service.py:200` | `struct.unpack_from("<I", crx, 8)` 取头部长度 | `payload(crx) -> &[u8]` |
| `browser_service.py:201` | `extensionFolder.mkdir(parents=True, exist_ok=True)` | `unpack` 第一句 |
| `browser_service.py:202-203` | `zipfile.ZipFile(...).extractall(folder)` | `entries(zip)` + `unpack(crx, folder)` |
| `setting_page.py:487-495` | `f.write(loadCrx())` | `export(crx, path)` |

`Paths::extension_folder()`（`boot.rs`）= `APP_DATA_DIR / "browser_extension"`，镜像
`startup.py:49` 传给 `Engine` 的 `extensionFolder`。

### CRX v3 的头部

```text
偏移 0  "Cr24"
偏移 4  版本（u32 LE）= 3
偏移 8  headerSize（u32 LE）→ 载荷从 12 + headerSize 开始
```

本仓的 crx：`headerSize = 1310`，载荷 340 142 字节，是标准 ZIP。**v2 的头部布局不同**
（`pubkeyLen` / `sigLen` 两个字段），这里明确报 `UnsupportedVersion(2)` 而不是瞎猜。

### 解包口径（实测，不是猜的）

用 `.venv/bin/python` 对真实 crx 跑了一遍 `extractall`，再逐项对照：

| 实测事实 | 影响 |
|---|---|
| 载荷 **42 项 = 11 个目录 + 31 个文件**，全部 `compress_type = 8`（deflate） | 只需 stored / deflate 两条解压分支 |
| 目录靠**名字以 `/` 结尾**判定（`ZipInfo.is_dir()`），不看 `external_attr` | `Entry::is_dir = name.ends_with('/')` |
| `extractall` **不还原权限与 mtime**：目录 `0o755`、文件 `0o644`（umask 决定），mtime 是解包时刻 | 不 `chmod`、不 `utime` |
| 顺序是 `infolist()` 序（= 中央目录序），父目录由 `os.makedirs(upperdirs)` 兜底 | 按中央目录序落盘，`create_dir_all(parent)` |
| `manifest.json` 2084 字节、`background.js` 101 558 字节 | 单测钉住这两个数（解压正确性的端到端证据） |

### 路径净化：照抄 `_extract_member` 的 POSIX 分支

CPython 的处理是**丢分量**，不是「解析」：

```python
arcname = member.filename.replace('/', os.sep)      # POSIX 无操作
arcname = os.path.splitdrive(arcname)[1]            # POSIX 无操作
invalid = ('', os.path.curdir, os.path.pardir)
arcname = os.sep.join(x for x in arcname.split(os.sep) if x not in invalid)
```

所以 `a/../b` → `a/b`、`/abs/x` → `abs/x`、`a//b` → `a/b`、`../..` → 空；而
`..\..\x` 在 POSIX 上**不拆反斜杠**，整串是一个文件名。`sanitize()` 逐条照做，
单测 `sanitize_drops_python_invalid_parts` 用九行表钉住。

`escaping_paths_are_clamped_to_the_folder` 是安全侧的证据：`../evil.txt` / `/abs.txt` /
`a/../../b.txt` 三个条目解包后全部落在 `folder` 内，上级目录一个文件都没多。

### 为什么自己写 ZIP 读

- `zip` crate 不在离线缓存里（`sha2` / `xz2` 同样缺），这一步又必须能跑通；
- 需要的子集极小：EOCD 定位 + 中央目录遍历 + 本地头定位 + inflate，**约 200 行**；
- `flate2` 已在锁里，`DeflateDecoder` 直接可用，不引入新依赖版本。

代价是**不校验 CRC32**：Python 的 `ZipExtFile` 读完会比对 CRC，这里只比对「解压后
长度与中央目录里的 `file_size`」。扩展包是打进二进制的自家产物，CRC 校验换不来额外
保障，却要再拖 `crc32fast`。

### 十三条单测

| 覆盖面 | 断言 |
|---|---|
| 容器头 | `Cr24` / 版本 3 / `headerSize == 1310` / 载荷以本地头签名开头 |
| 中央目录 | 42 项、11 目录 + 31 文件、全 method 8、首项 `_locales/`（**不是字典序**） |
| 真实解包 | 落盘 11 目录 + 31 文件、`manifest.json` 2084、`background.js` 101 558 |
| 幂等 | 解包两次，返回清单与磁盘树都不变 |
| 路径净化 | 九行表（见上） |
| 坏魔数 | `BadMagic` |
| 短头 / v2 | `Truncated` / `UnsupportedVersion(2)` |
| stored 分支 | 合成 ZIP（method 0）原样拷贝 |
| deflate 分支 | 合成 ZIP（method 8），压缩后确实变小 + 解压等值 |
| 未知方法 | method 12 → `UnsupportedMethod(12)` |
| 逃逸路径 | 三个逃逸条目全部夹在 `folder` 内 |
| 缺 EOCD | 截尾 ZIP → `Zip(_)`；`b"PK"` → `Truncated(_)` |
| 导出 | `export()` 写出的字节与 `crx_bytes()` 逐字节相同 |

合成 ZIP 由测试内的 `ZipBuilder` 造（本地头 + 中央目录 + EOCD，CRC 留 0，因为不校验），
`wrap()` 再套一个假 CRX 头 —— 这样「解压分支」与「真实包」两条路径都有覆盖。

### 与 Python 的已知差异

- **不校验 CRC32**（见上）。
- **不支持 ZIP64**：crx 341 KB，`allowZip64` 走不到；`entries` 见到 `0xffff` 哨兵直接报错。
- **头部越界报错**：Python 的 `crx[12 + headerSize:]` 越界时静默给空串（后面才 `BadZipFile`），
  这里立刻 `Truncated("CRX 载荷")`。
- **`unpack` 返回落盘清单**：Python 的 `extractall` 无返回值；清单给下一步的「解包后弹
  安装引导」用，也方便断言顺序。
- **净化后为空的条目跳过**：Python 会拿 `folder` 本身当目标（目录项无害，文件项
  `IsADirectoryError`）。自家包里没有这种条目。

### 验证

- `cargo fmt --all` + `--check`：通过；
- `cargo check -p gd3-ui --lib`：24.38s，零错零警告；
- `cargo test -p gd3-ui --lib extension_crx`：**13 passed**（`/tmp/t121b.log`）；
- `cargo test -p gd3-ui --lib`：**1047 passed / 0 failed**（1034 + 13，`/tmp/t121lib.log`）；
- `cargo test --workspace`：**1602 passed / 0 failed / 28 段**（`/tmp/t121ws.log`）；
- `cargo clippy --workspace --all-targets -- -D warnings`：**0 warning**（`/tmp/c121.log`）；
- Python 侧没改一行。

### 下一步（第 122 步）

四个平台副作用的接线（`take_deferred()` 里剩下的全部）：`copyPairToken`（写剪贴板）、
`openStoreUrl`（拉起浏览器）、`chromiumInstall`（`extension_crx::unpack` + 安装引导框）、
`exportCrx`（保存对话框 + `extension_crx::export`）。

## 浏览器扩展卡的动作（`gd3-ui::extension_actions`，第 122 步）

设置页的「浏览器扩展」组有五张按钮卡，它们的回调**都不写 cfg**（除重新生成令牌），
要碰的是外设：剪贴板、默认浏览器、扩展目录、保存对话框。第 40 步把它们塞进
`SettingsActions` 当五个空壳，第 119–120 步用 `SettingsWriter::defer` 记下「点了但没接」。
这一步把五个动作真正接上，并把 `defer` 机制整体删掉。

### 五个 Python 回调逐行对照

| Python | 行号 | 这里 |
|---|---|---|
| `_onCopyTokenClicked` | `setting_page.py:437-441` | `ExtensionActions::copy_token` |
| `_onRegenerateTokenClicked` | `:443-448` | `ExtensionActions::regenerate_token` |
| 商店链接（`HyperlinkButton.setUrl`） | `:642/646/650` → `openStoreUrl` | `ExtensionActions::open_store` |
| `_onChromiumInstallClicked` | `:450-468` | `ExtensionActions::install` / `install_from` |
| `_onExportExtensionClicked` | `:487-495` | `ExtensionActions::export` |

三处细节按 Python 原样照抄：

- **重新生成令牌也要复制**：Python 是「先 `cfg.set(cfg.browserExtensionPairToken, …)`，
  再 `QApplication.clipboard().setText(token)`」，两个 InfoBar 的标题/正文分别是
  「已重新生成配对令牌」+「新令牌已复制到剪贴板」。所以这个动作的**入口必须和复制捆在
  同一个闭包里** —— 它从 `SettingsActions` 搬到了本模块，cfg 写仍走
  [`SettingsWriter::regenerate_pair_token`]。
- **复制失败不弹提示**：`copy_token` 只在 `copy_text` 返回 `true` 时才 `notify`。Python
  的 `QApplication.clipboard()` 永远可用，所以那边没有这条分支；这里必须有。
- **导出成功不弹 InfoBar**：Python 的 `_onExportExtensionClicked` 静默写盘。这里只把
  `Notice::Exported` 交给宿主（`SystemEffects` 打 stderr），不假设有 InfoBar。

### 为什么要单独一个模块

`SettingsActions` 的 40 个字段是「一个属性名 + 一个值」的清一色闭包，由
`settings_writer::actions()` 一次性装出来；这五个动作没有属性名，还要拿路径、拿
`Rc<SettingsWriter>`、拿平台副作用，塞在同一个结构里会让「动作集 = cfg 写」这条边界失效。
于是 `SettingsActions` **40 → 35 字段**，`extension_actions::install` 独占这五个回调。

Slint 的 `on_<callback>` 二次安装是**替换而非叠加**，所以「谁装谁负责」在这里是硬约束：
`install_settings_actions` 里已经删掉了这五行，两个 `install` 各管各的。

### `Effects`：四类外设的注入点

```rust
pub trait Effects {
    fn copy_text(&self, text: &str) -> bool;                 // QApplication.clipboard().setText
    fn open_url(&self, url: &str) -> bool;                   // QDesktopServices.openUrl
    fn choose_save_path(&self, suggested: &str) -> SaveChoice; // QFileDialog.getSaveFileName
    fn notify(&self, notice: Notice);                        // InfoBar / ExtensionInstallDialog
}
```

单测实现这四个方法就能把整条链路跑通，不需要 X11 / Wayland / 真对话框；
生产实现 `SystemEffects` 用外部命令 + `eprintln!`。

`SaveChoice` 三分支（`Path` / `Cancelled` / `Unavailable`）比 Qt 的
`(path, filter)` 多一档：**没有可用的对话框**要能和「用户点了取消」区分开，否则宿主
没法提示。`Notice` 八变体的 `text()` 给出 `InfoBar` 的两行文案。

### 剪贴板：外部命令，不是 `arboard`

`clipboard.rs` 是**监听器**状态机（读剪贴板、判断是不是下载链接），没有写入端。
`arboard 3.7.0` 在离线缓存里，但会拖 `x11rb` / `wl-clipboard-rs` 一串依赖 ——
为「把一段 22 字符的令牌写进剪贴板」付这个代价不值。改用系统工具：

| 平台 | 候选命令 | `which` 过滤 |
|---|---|---|
| Linux | `wl-copy` → `xclip -selection clipboard` → `xsel --clipboard --input` | 是（按顺序试，第一条成功即停） |
| macOS | `pbcopy` | **否** |
| Windows | `clip.exe` | **否** |

`pbcopy` / `clip.exe` 是系统自带工具，从桌面环境拉起时 `PATH` 可能被精简，
所以不查 `PATH` 直接试；Linux 的三条是可选安装，必须查。`copy_text_with` 把
「逐条试 + 第一条成功即停」写成 `any`，命令表与执行分离后可以注入假命令测试。

`run_with_stdin` 是「stdin 管道 + `write_all` + `wait`」：写失败就把子进程 `kill` 掉，
避免 `wl-copy` 这种「读 stdin 读到 EOF 才写剪贴板」的工具挂住。

### 默认浏览器：`gd3-core::desktop::open_url_command_on`

Python 的 `QDesktopServices.openUrl(QUrl(url))` 是「交给桌面环境的默认处理程序」，
与 `open_chromium_url`（挑一个已装的 Chromium 系浏览器 + 改写 `chrome://` 前缀）
是两件事，不能互相顶替。新增：

| 平台 | 命令 |
|---|---|
| Linux / 其它 | `xdg-open <url>` |
| macOS | `open <url>` |
| Windows | `rundll32.exe url.dll,FileProtocolHandler <url>` |

Windows 这条值得记一笔：本模块的 `open_target_command_on`（`openFile` / `openFolder`）
对 Windows 返回 `None`，注释写的是「`ShellExecute` 未迁移」。但那里传的是**文件路径**，
用 `rundll32` 会丢掉「在文件夹里选中」的语义；这里传的是 **URL**，`FileProtocolHandler`
就是 `ShellExecuteW(.., "open", url, ..)` 的等价物，不需要 `windows` crate。

### 保存对话框：`zenity` / `kdialog` / `osascript`

| 平台 | 候选 | 说明 |
|---|---|---|
| Linux | `zenity --file-selection --save --confirm-overwrite --filename=… --file-filter=…` → `kdialog --getsavefilename …` | 装了哪个用哪个 |
| macOS | `osascript -e 'POSIX path of (choose file name with prompt … default name …)'` | 需要「自动化」权限 |
| Windows | 无 | 返回 `SaveChoice::Unavailable` |

过滤器字面量 `Chromium Extension(*.crx)` 与默认文件名 `Extension.crx` 都是 Python 的
原样串（`setting_page.py:490`），单测钉住。`run_capture` 读 stdout、trim、**非零退出当
取消**（`zenity` 取消返回 1，`kdialog` 也是）。

### 解包与导出：复用 `extension_crx`

`install()` = `extension_crx::unpack(crx_bytes(), &folder)`，`folder` 来自
`Paths::extension_folder()`（`app_data_dir/browser_extension`，镜像 `startup.py:49`
传给 `Engine` 的 `extensionFolder`）。解包成功发 `Notice::ExtensionInstalled(folder)`，
失败发 `Notice::UnpackFailed(error.to_string())`（标题「解包失败」，镜像 Python 的
`InfoBar.error("解包失败", toLocalizedError(toTaskError(error)))`）。

`install_from(&[u8])` 让坏包分支可测 —— 单测喂一个 `Cr24` 但载荷是垃圾的容器，
断言 `false` + `UnpackFailed`。

`export()` = `choose_save_path("Extension.crx")` → `extension_crx::export(crx, path)`。

### 删掉的 `defer` 机制

`defer()` 是第 119 步的临时脚手架：动作集里没接的动作记个名字，`take_deferred()` 把
名单交给宿主打日志。这一步之后动作集里不再有「记一笔就算」的洞，于是整条链路删除：

| 位置 | 删掉的东西 |
|---|---|
| `settings_writer.rs` | `deferred` 字段、`take_deferred()`、`defer()`、模块头的「还没接的部分」段、4 个 `defer(...)` 闭包 |
| `settings_bridge.rs` | `SettingsDrain::deferred` 字段、`is_empty()` 里的一项、`drain()` 里的 `take_deferred()`、模块头两条说明 |
| `boot.rs` | drain 钩子里 `for action in &drained.deferred` 那个循环 |
| 测试 | `the_deferred_actions_name_what_is_still_missing`、`deferred_actions_are_reported_not_executed`、两处 `assert!(writer.take_deferred().is_empty())` |

`SettingsActions` 的 `pair_token_regenerate` 字段一起删了 —— 它的 cfg 写半边仍在
`SettingsWriter::regenerate_pair_token`，`all_scalar_actions_write_their_attribute`
改成直接调方法验落盘（27 个属性名的顺序不变）。

### 十三条单测

| 覆盖面 | 断言 |
|---|---|
| 复制令牌 | 写进剪贴板的正是配置里的令牌 + `Notice::TokenCopied` |
| 复制失败 | 剪贴板写失败 → 不 `notify`、返回 `false` |
| 重新生成 | cfg 里的令牌换了、写盘一次、剪贴板里是新令牌 |
| 商店链接 | URL 原样转给 `open_url` |
| 安装 | 341 464 字节的自家 crx 解出 31 个文件到 `<tmp>/ext`、`Notice::ExtensionInstalled` |
| 安装失败 | 坏载荷 → `false` + `UnpackFailed`（含错误文案） |
| 导出 | 选定路径后字节与 `crx_bytes()` 逐字节相同 |
| 导出取消 / 无对话框 | `ExportCancelled` / `SaveDialogUnavailable` 两个 `Notice` |
| 导出写失败 | 目标是已存在的目录 → `ExportFailed` |
| 剪贴板命令表 | 三平台 + `which` 过滤 + Wayland 优先 |
| 逐条重试 | `a` 失败 → `b` 成功即停；全失败返回 `false`；空表返回 `false` |
| 保存对话框命令 | `zenity` 优先、`kdialog` 次之、都没有 → `None`、Windows → `None` |
| `Notice::text()` | 八变体的标题/正文（含 Python 的两条原样文案） |

### 与 Python 的已知差异

- **没有 Qt 就没有 `QFileDialog`**：Linux 用 `zenity` / `kdialog`，macOS 用 `osascript`，
  都没有就返回 `Unavailable`（Python 永远有对话框）。宿主能据此提示，而不是静默失败。
- **剪贴板靠外部命令**：一条都没有时返回 `false`，用户看到的是「没反应」——
  Python 的 `QApplication.clipboard()` 不会失败。
- **安装引导框在第 123 步补上**：Python 在 `install()` 成功后 `ExtensionInstallDialog(path).exec()`
  弹五步引导。这一步只发 `Notice::ExtensionInstalled(folder)`，`on_chromiumInstall` 的
  「成功才弹框」由第 123 步的 `crate::extension_install::show` 接（见下节）。
- **`export()` 不弹 InfoBar**：与 Python 一致（那边静默写盘），但这里多了
  `Exported` / `ExportFailed` 两种 `Notice` 供宿主决定。

### 验证

- `cargo fmt --all` + `--check`：通过；
- `cargo check -p gd3-ui --all-targets`：**2m 56s，零错零警告**；
- `cargo test -p gd3-ui --lib extension_actions`：**13 passed**；
- `cargo test -p gd3-core --lib`：**334 passed / 0 failed**（新增 `open_url_commands_match_python`）；
- `cargo test -p gd3-ui --lib`：**1058 passed / 0 failed**（1047 + 13 新增 − 2 删除）；
- `cargo test --workspace`：**1614 passed / 0 failed / 28 段**（1602 + 13 + 1 − 2）；
- `cargo clippy --workspace --all-targets -- -D warnings`：**0 warning**，`Finished dev profile
  in 3m 26s`；
- Python 侧没改一行。

## 扩展安装引导框（`gd3-ui::extension_install`，第 123 步）

`app/view/dialogs/extension_install.py`（133 行）是 `install()` 成功之后弹的那个五步
引导框：标题 + 16:9 演示图 + 五条带 chip 的步骤 + 「打开扩展页面并定位目录」/「关闭」。
第 122 步只发了一条 `Notice::ExtensionInstalled(folder)`，这一步把框补上：
`ui/extension_install.slint`（139 行）给形状，本模块给文案与副作用，`boot.rs` 把两边接上。

### 逐字口径（实读源码，`translate` = identity）

| 项 | 值 | 源 |
|---|---|---|
| 标题 | `扩展已解包，按以下步骤安装` | `:24`（`SubtitleLabel`） |
| 预览尺寸 | `QSize(720, 405)` | `:14` |
| 最小宽度 | `720 + 48` = **768** | `:33`（+48 = `scrollLayout` 左右各 24） |
| 最大高度 | `405 + 385` = **790** | `:35`（385 = 48 边距 + 60 空隙 + 28 标题 + 168 步骤 + 81 按钮组） |
| 占位文案 | `演示动画即将上线` | `:54`（`QMovie.isValid()` 为假） |
| yes | `打开扩展页面并定位目录` | `:38` |
| cancel | `关闭` | `:39` |
| chip 底色 | 暗 `rgba(255, 255, 255, 0.1)` / 亮 `rgba(0, 0, 0, 0.06)` | `:64` |
| 占位框样式 | `QLabel { background: …; color: …; border-radius: 8px; }` | `:59-61` |

五步（`:70-76`，序号来自 `enumerate(steps, 1)`）：

1. `在浏览器中打开 {chip("chrome://extensions", mono=True)}`
2. `开启 {chip("开发者模式")}`
3. `将 {chip("扩展文件夹")} 拖入浏览器窗口`
4. `启用扩展`
5. `打开 {chip("扩展弹窗")}，点击 {chip("自动配对")}`

`chip()`（`:66-68`）产出 `<span style="background-color: {chipBg};{family}">&nbsp;{text}&nbsp;</span>`，
`family` 只在 `mono=True` 时是 ` font-family: 'Cascadia Code', Consolas, monospace;`
（**开头那个空格是 Python 原文**，单测钉住）。

### `Segment`：把「模板字面量」和「chip」摊成同一种数据

Python 的富文本是一串 `f"…{chip(…)}…"` 拼出来的，纯文本只能另写一遍。这里把两种东西
都摊成 [`Segment`]（`text` + `mono` + `chip`），于是：

- `Step::plain_text()` —— 拼 `text`，给 Slint 的 `InstallStep.text`；
- `Step::rich_text(dark)` —— 拼 `Segment::render`，**与 Python 的 `stepLabel.setText()` 逐字节相同**，
  单测直接比整串（含 `&nbsp;` 与分号位置）。

这样「显示的文字」只有一份来源，改文案不会漏掉一半。

### 点 yes 之后：`plan_confirm` / `confirm`

`_onInstall`（`:123-133`）的顺序是死的：`revealInFolder(path)` →
`openChromiumUrl("chrome://extensions")` → **返回假**才 `clipboard.setText` +
`InfoBar.info(请手动打开浏览器, chrome://extensions 已复制到剪贴板，请粘贴到地址栏, 5000, TOP)`。

把它拆成两半：

- [`plan_confirm(folder, opened)`] —— 纯函数，`opened` 是「浏览器开没开」这个入参，
  返回 `Vec<Effect>`（`RevealInFolder` / `OpenChromiumUrl` / `CopyText` / `Notify`），
  两个分支都能在单测里逐项比；
- [`confirm(effects, folder)`] —— 执行入口，副作用经 [`GuideEffects`] 注入，返回
  「浏览器有没有打开」。

**`revealInFolder` 的返回值被忽略**，与 Python 一致（定位失败也照样开扩展页）。

[`GuideEffects`] 与 `extension_actions::Effects` 故意分开：这里多一个
`reveal_in_folder`、少一个保存对话框，`notify` 只要两行文字（`InfoBar.info` 没有按钮、
没有复选框）。生产实现 `SystemGuide` 用 `gd3_core::desktop::{reveal_in_folder,
open_chromium_url}` + `extension_actions::copy_text` + `eprintln!`。

### `confirmed` 不关框

`_bind`（`:120-121`）先 `yesButton.clicked.disconnect()` 再连 `_onInstall` ——
**点 yes 只做副作用，框还开着**；关框只有 cancel（`MessageBoxBase.__onCancelButtonClicked`
→ `reject()`）。所以 `install()` 装的 `extensionInstallDismissed` 才 `hide()`，
`extensionInstallConfirmed` 只跑 `confirm()`。单测 `confirmed_runs_the_effects_but_keeps_the_dialog_open`
钉这一条。

### 接线

- `extension_actions::ExtensionActions::folder()` —— 解包目标（第 122 步是私有字段），
  `on_chromiumInstall` 里解包成功后 `crate::extension_install::show(&window, unpack.folder())`；
  解包失败不进这个分支（`install()` 内部已发 `Notice::UnpackFailed`）。
- `boot.rs` 两个分支（headless / 正常）各加
  `crate::extension_install::install(&window, Rc::new(SystemGuide))`。

### 与 Qt 侧的已知差异

- **预览是静态 PNG 而不是 webp 动画**：`.slint` 用 `@image-url` 编译期嵌入
  `app/assets/install_chrome_extension_guidance.png`（1415×670 静帧，76 KB；
  `.webp` 是 720×405 的动画，1.3 MB）。webp 动画要解码器 + 逐帧推进，Slint 侧没有
  等价物；`image-fit: contain` 保住 16:9 观感。`previewText` 属性保留 Python 的
  `演示动画即将上线` 兜底。
- **没有模态事件循环**：`MessageBoxBase.exec()` 换成 `extensionInstallVisible` 覆盖层 +
  `confirmed` / `dismissed` 两个回调。
- **`revealInFolder` 在 Windows 上返回假**：`gd3_core::desktop::reveal_in_folder` 走
  `open_target_command_on`，那条对 Windows 还没迁移（`ShellExecute` 未接），与第 122 步
  `open_url` 用 `rundll32` 是两码事。Linux / macOS 正常。
- **`InfoBar` 换成 `eprintln!`**：与 `SystemEffects::notify` 同一口径，等通知条接线那一步再换。

### 验证

- `cargo fmt --all` + `--check`：通过；
- `cargo check -p gd3-ui --all-targets`：**2m 47s，零错零警告**（`/tmp/c123d.log`）；
- `cargo test -p gd3-ui --lib extension_install`：**17 passed**（`/tmp/t123a.log`）；
- `cargo test -p gd3-ui --lib`：**1067 passed / 0 failed**（`/tmp/t123b.log`；1058 − 8 旧 + 17 新）；
- `cargo test --workspace`：**1623 passed / 0 failed / 28 段**（`/tmp/t123ws.log`）；
- `cargo clippy --workspace --all-targets -- -D warnings`：**0 warning**，`Finished dev profile in 3m 34s`（`/tmp/c123clip.log`）；
- Python 侧没改一行。

## 更新检查的执行骨架（`gd3-ui::update_service`，第 124 步）

第 115 步把 `app/services/update_service.py` 的**计划层**搬完了：`plan_check` /
`plan_app_download` / `plan_pack_download` / `plan_apply` / `updater_command` 都是
「给定输入 → 算出应该发生什么」的纯函数，49 条单测逐条钉住。但 Python 的
`UpdateService` 是 async 的：`_check` 会 `await fetchJson`，`_download` 会
`await fetchReleaseAsset`，中间状态全摊在协程的局部变量里。

Rust 侧不能这么写——`gd3-ui` 刻意不依赖 tokio（同 `aria2_actions.rs` 的口径）。
这一步补的是**执行骨架**：把四条入口串成同步状态机，异步动作全部摊成
`UpdateEffect` 交给宿主，宿主执行完再把结果喂回来。

### 状态流转

```text
check() ──[Emit(CHECKING), FetchVersions]──▶ 宿主抓 versions.json
       ◀── versions_ready(Option<Value>, &[LocalPack]) ── [Emit(…)]

download(id) ──[Emit(DOWNLOADING), Mkdir, FetchAsset|FetchRaw]──▶ 宿主下载
       ◀── download_progress(id, p) ── [Emit(DOWNLOADING, progress)]
       ◀── download_finished(id) ── [Checksum] 或 [Emit(READY)]
       ◀── checksum_finished(id, actual) ── 对：[Emit(READY)] / 完整包：[RemoveDir, Unpack]
       ◀── unpack_finished(id, error) ── [RemoveFile, Emit(READY)]
       ◀── download_failed(id, error) ── [Emit(FAILED)]

apply() ──[Unpack(Zip, staging/<id>.zip, features/<id>_pending)…, StartUpdater|SkipUpdater]
```

`Emit` 变体里带着的那条 `UpdateStep` **已经写进 `UpdateRuntime::table()`**，宿主只需
把它转发给 UI；其余九种变体都是宿主必须真做的事。

### `UpdateEffect` 的十个变体

| 变体 | 对应 Python | 谁执行 |
|---|---|---|
| `Emit(UpdateStep)` | `_emit(...)` | 转发给 UI |
| `FetchVersions` | `fetchJson(APP_REPO, "main", "versions.json")` | `gd3-net::sources` |
| `Mkdir(PathBuf)` | `STAGING_DIR.mkdir(parents=True, exist_ok=True)` | 宿主 |
| `RemoveDir(PathBuf)` | `if newDir.exists(): shutil.rmtree(newDir)` | 宿主 |
| `RemoveFile(PathBuf)` | `path.unlink(missing_ok=True)` | 宿主 |
| `FetchAsset { tag, file, dest }` | `fetchReleaseAsset(APP_REPO, tag, file, dest, onProgress)` | `gd3-net::sources` |
| `FetchRaw { path, dest }` | `fetchRawFile(APP_REPO, "main", path, dest, onProgress)` | `gd3-net::sources` |
| `Checksum { path, expected }` | `toChecksum(path, "sha256", …)` | `gd3-core::checksum` |
| `Unpack { kind, archive, target }` | `extractZip` / `extractTarXz` / `extractDmg` | 平台工具 |
| `StartUpdater(UpdaterCommand)` | `subprocess.Popen(args, **detach)` | 宿主 |
| `SkipUpdater(UpdaterSkip)` | `_startUpdater` 的两个「记日志然后 return」 | 宿主记日志 |

`Checksum` 带的是**原始校验和串**（`"abc123  file.zip"`）而不是解析后的十六进制：
`parse_checksum` 在 `gd3-core` 里，比较也留在状态机（`checksum_accepted`），宿主只负责
算 sha256 并回报十六进制。

### `Pending`：Python 的局部变量必须显式存下来

Python 的 `_downloadApp` 把「下什么、下到哪、校验和多少、解到哪个目录」摊在局部变量里，
`await` 之后继续用。异步化之后这些变量必须活过整个下载链路，所以有一个私有枚举：

```rust
enum Pending {
    Patch { dest, sha256 },                                  // 下完 → 校验 → READY，不解包
    Full  { dest, sha256, unpack, new_dir },                 // 下完 → 校验 → RemoveDir → Unpack
    Pack  { dest, sha256 },                                  // 下完 → 校验 → READY（解包推迟到 apply）
}
```

`pending` 是 `BTreeMap<String, Pending>`：`is_pending(id)` 就是查询入口，四条
`*_finished` 都靠它决定「这一步该干什么」。没有记录就直接返回空 `Vec`——重复回调
（宿主重试、UI 连点）不会把表改坏。

### 三个容易写错的语义

1. **`versions_ready` 要 `.skip(1)`**：`plan_check` 的第一条恒是 `CHECKING`，那是
   `check()` 已经发过的。宿主若照单全收，UI 会收到两条同状态通知，进度条闪一下。
   代码里有 `debug_assert_eq!` 守住「第一条确实是 CHECKING」，将来 `plan_check` 改
   形状会立刻炸在测试里。
2. **`RemoveDir` 无条件发**：Python 是 `if newDir.exists(): rmtree`。这里把存在性判定
   推给宿主——`remove_dir_all` 对不存在的路径返回 `Ok(())` 的语义刚好等价，
   状态机少一次文件系统调用，单测也不用造目录。
3. **`unpack_finished` 失败时不删归档**：Python 的 `unlink` 在 `extract` 之后，异常时
   归档留着好排查。所以失败分支只发一条 `Emit(FAILED)`，成功分支才是
   `[RemoveFile, Emit(READY)]`。

另外 `versions_ready(None, …)`（抓 `versions.json` 失败）**不动 `self.versions`**：
Python 的 `_fetchVersions` 抛异常时 `_versionsData` 保持上一次的值，重试还能用。

### 二十条单测

| 测试 | 钉住什么 |
|---|---|
| `check_emits_checking_and_asks_for_the_versions_file` | 两条效果 + label 带版本号 + 表已更新 |
| `versions_ready_does_not_repeat_the_checking_step` | `.skip(1)`；AVAILABLE 的 label / latestVersion |
| `versions_ready_reports_a_fetch_failure` | 抓取失败 → IDLE + 「无法获取版本信息」 |
| `a_failed_fetch_keeps_the_previous_versions_data` | 失败不清 `_versionsData` |
| `download_refuses_a_target_that_is_not_available` | 无记录 / `CHECKING` 都不下载 |
| `download_asks_for_the_full_package_of_the_platform` | `Emit` + `Mkdir` + `FetchAsset` 的 tag/file/dest |
| `the_full_package_is_checksummed_unpacked_and_cleaned_up` | 校验大小写不敏感；`RemoveDir` → `Unpack` → `RemoveFile` → `READY` |
| `a_patch_is_never_unpacked` | 补丁落到 `patch.hdiff`；sha 为空直接 `READY` |
| `a_missing_platform_package_fails_the_target` | `full[platformKey]` 缺失 → FAILED（`Mkdir` 仍在失败之前） |
| `a_pack_downloads_from_the_dist_directory` | `dist/packs/youtube.zip` → `staging/youtube.zip` |
| `a_bad_checksum_removes_the_file_and_fails` | 对不上：`RemoveFile` + 「校验失败」 |
| `an_unpack_failure_keeps_the_archive` | 失败分支只有一条 `Emit` |
| `a_download_error_marks_the_target_failed` | `except Exception` 的出口 |
| `progress_keeps_the_downloading_state` | 进度只改 `progress`，状态仍是 `DOWNLOADING` |
| `apply_unpacks_the_packs_before_starting_the_updater` | pack 解到 `features/<id>_pending`，updater 最后 |
| `apply_skips_the_updater_when_the_binary_is_missing` | `SkipUpdater(Missing(…))` |
| `apply_does_nothing_without_ready_targets` | 空表 → 空效果 |
| `the_host_reports_the_platform_key` | `Host::platform_key()` + `is_win32` / `is_darwin` |
| `an_unknown_platform_key_fails_the_app_download` | `buildPlatformKey` 抛 `KeyError` 的 `toTaskError` 形态 |
| `the_staging_path_is_used_for_every_artifact` | 所有产物都落在 `STAGING_DIR` 下 |

### 与 Python 的已知差异

1. **`download()` 先发 `Mkdir` 再判平台**：Python 的 `_download` 在
   `try: STAGING_DIR.mkdir(...)` 之后才进 `_downloadApp`，平台不支持时目录也已经建了。
   Rust 侧顺序一致，单测里因此是 `Emit` + `Mkdir` + `Emit(FAILED)` 三条。
2. **`_applyPack` 的 `if not zipPath.is_file(): return` 归宿主**：状态机照发 `Unpack`，
   宿主发现归档不在就跳过——这样「文件系统状态」不会渗进状态机，单测不用造 zip。
3. **`SkipUpdater` 只表达「为什么没拉起」**：Python 的两条分支各写一行日志，日志文案
   留给宿主（`UpdaterSkip` 的两个变体名已经足够区分）。
4. **没有 `finally` 里的 `pending` 清理**：Python 靠协程结束自动回收；这里每个终结分支
   （成功 / 校验失败 / 解包失败 / 下载异常）都显式 `pending.remove`，单测逐条验
   `is_pending() == false`。

### 验证

- `cargo fmt --all` + `--check`：通过；
- `cargo check -p gd3-ui --all-targets`：**3m 08s，零错零警告**（`/tmp/c124a.log`）；
- `cargo test -p gd3-ui --lib runtime_tests`：**20 passed**（`/tmp/t124b.log`）；
- `cargo test -p gd3-ui --lib`：**1087 passed / 0 failed**（`/tmp/t124c.log`；1067 + 20）；
- `cargo test --workspace`：**1643 passed / 0 failed / 28 段**（`/tmp/t124ws.log`）；
- `cargo clippy --workspace --all-targets -- -D warnings`：**0 warning**，
  `Finished dev profile in 25m 42s`（`/tmp/c124clip.log`；比第 123 步的 3m 34s 慢得多，
  因为 `--workspace` 测试跑完后 clippy 的缓存键全变了，整条依赖链重新 check）；
- Python 侧没改一行。

## 剩余工作（Phase 8）

**配置（`UserConfig.json`）和 Slint 入口要一起搬**。`app/config/cfg.py` 是
`qfluentwidgets.QConfig` 的子类：每一项都是 Qt 的 `ConfigItem`，设置页直接绑它的
`valueChanged`，而且**每次 `cfg.set()` 都会整文件回写**。只要 Qt 侧还在写这个文件，
Rust 侧再写同一个文件就会互相覆盖——所以 Rust 侧的 cfg 读写必须与「Slint 成为唯一 UI」
同一步落地，不能提前。同理，凡是「持久化进 cfg」的服务（`category_service` 的
`categoryRules` 等）都要排在它后面。

这条已经落地：第 113 步 `config_store`（读写语义）→ 第 117 步 `settings_writer`
（动作 → cfg 键）→ 第 118 步 `settings_bridge`（装到窗口上 + 宿主钩子）→ 第 119 步
`settings_writer` 的列表写回 → 第 120 步 `settings_dialogs`（九个覆盖层开合）→
第 122 步 `extension_actions`（五个平台副作用，`defer` 机制随之删除）。
**设置页的动作集已经全部接线**，剩下的是：

1. **更新检查接线**：第 115 步的计划层（`plan_*`）与第 124 步的执行层
   （`UpdateRuntime` 的十个 `UpdateEffect`）都已就位，剩下的是**宿主适配**：
   `FetchAsset` / `FetchRaw` 走 `gd3-net::sources::{fetch_release_asset,
   fetch_raw_file}`、`Checksum` 走 `gd3_core::checksum`、`Unpack` 的 zip 走
   `extension_crx`（CRX 与 zip 载荷同构，`entries` / `read_entry` 可直接复用）。
   这层需要 tokio，必须放在 `gd3-ui` 之外的宿主层；剩 tar.xz / dmg 受 `xz2`
   离线缺包限制。之后才是更新卡片 / 进度条 UI。
2. ~~**安装引导框**~~：第 123 步完成（`gd3-ui::extension_install` +
   `ui/extension_install.slint`，17 单测），`on_chromiumInstall` 解包成功即弹五步引导。
3. **P2–P6 剩余 UI**：外壳动作集（导航 / 搜索 / 通知条 / 消息框 / 拖放），
   草稿对话框里还没接的五个子对话框（目录选择、编辑、批量添加、导入文件、分类菜单），
   其它对话框、组件库、OOBE、平台外壳。建任务链路（第 137 步）、后台解析（第 138 步）、
   草稿对话框宿主（第 139 步）都已就位。

## Phase 8 进展（第 126–139 步）

### 下载运行期（`task_runner.rs` + `runtime.rs`）

`task_runner.rs`（第 126 步）是「谁去下载」：计划层（`pending_positions` /
`step_output_path` / `plan_job`，对应 `Task.pendingSteps` / `Task.toFilePath` /
`HttpTask.run` 的参数摊平）、`RunnerEvent`（每条都带 `workId`，暂停后重派发的旧事件
整条丢掉）、`Spawner` trait（生产是 `ThreadSpawner` = 一个多线程 tokio runtime，
测试是 `FakeSpawner`）、`TaskRunner`（`pump` / `dispatch` / `drain` / `pause` /
`cancel_all` / `shutdown`）。**一个任务一个 job，步骤按 `stepIndex` 顺序跑**（同
Python 的 `Task.run`）。

`runtime.rs`（第 127 步）是「谁按 tick 推它」：

```text
tick = 现读 cfg（maxTaskNum 改了下一 tick 就生效）
     → pump（派发等待队列）
     → drain（收工作线程事件：进度 / 步骤完成 / 任务完成 / 失败 / 取消）
     → 每秒一次 tick_speed_meter → Engine::drain_speed_effects
     → report.changed 才 flush(tasks.jsonl)
     → report.changed 或 有进度 才 refresh（重画任务列表）
```

口径（与 Python 的差异都写在模块头里）：

- **只有收尾才落盘**：进度不改盘（Python 的 `_flushSoon` 也只在
  `_onRunDone` / `_onRunFailed` / `pause` 里）；
- **`.ghd` 只在整任务 `COMPLETED` 时删**（暂停 / 失败都保留，续传靠它）；
- **`Cancelled` 不做队列记账**：`TaskService::pause` / `cancel_work` 已经做过，
  再做一次就是双重 `queue.done`；
- **速度表按 tick 判 1 秒**（Python 是每个任务一个协程定时器），数字进
  `RunnerTick::speed`；
- `dispatch` 里 **先置 `RUNNING` 再算 job**（`pending_positions()` 在任务不是
  `RUNNING` 时返回空表 —— 顺序反了会变成「每个任务零步骤」）。

无头没有窗口事件循环（`slint::Timer` 不会自己走），所以 `Runtime::run_until_idle`
手动按同一间隔调 `tick()`；**不带 `--until-idle` 的 `--frames` 冒烟不推运行期**
（保持「只启动不派发」的语义）。停机顺序是 `runtime.shutdown(5s)`（取消工作线程、
保住 `.ghd`）**先于** `Engine::stop()`（落盘）——反过来的话 `TaskService` 已经收工，
晚到的运行期事件没人收。

端到端验收：`tests/runner_e2e.rs`（5 条，`TaskRunner` 直驱；其中两条是第 133 步的
mtime 断言，一条是第 134 步的限速断言）、
`tests/runtime_e2e.rs`（2 条，`boot::run(--until-idle)` 与 `Runtime::pause`/`start`）、
`tests/test_config_roundtrip.py`（6 条 cfg 往返）。

### 前端分派（`GHOST_UI`）

**不删 Qt 入口**。`app/platform/ui_backend.py`（不 import Qt）在
`Ghost-Downloader-3.py` 的 `__main__` **第一行**分派：

| `GHOST_UI` | 行为 |
|---|---|
| 未设 / `qt` | 原 PySide6 入口（缺省，可回退） |
| `slint` | `os.execv` 换成 `gd3-ui`（二进制找法：`GD3_UI_BIN` → 可执行文件同目录 → `rust/target/{release,debug}`） |
| 其它 | 当作 `qt`，stderr 一条警告 |

必须在 `setupEnvironment()` / `SingletonApplication` **之前**：那两步会 `qconfig.load`
同一个 `UserConfig.json` 并抢一把 Qt 的单实例锁。

**单一写者**：`GHOST_UI=slint` 时 `UserConfig.json` / `tasks.jsonl` / 单实例锁的写者是
**Rust**；`GHOST_UI=qt`（缺省）时是 **Python**。两边不能同时跑（锁不一样，整文件回写
会互相覆盖）。

### 配置整文件回写：不认识的组必须原样保留（第 130 步）

`ConfigStore::save` 是**整文件回写**（Python 的 `cfg.set` 也是）。第 130 步之前它照
`settings::SPECS`（42 项 / 9 个组）重建整份文件，于是磁盘上它不认识的组全被删掉。
真机上那些组是**合法配置**：`features/*/config.py` 的 `PackConfig` 子类在
`__init_subclass__`（`app/models/pack.py:62-71`）里把每个 `ConfigItem` `setattr` 到
**全局 `cfg.__class__`**，所以 `toDict()` 会连 `Bilibili` / `BitTorrent` / `ED2k` /
`FFmpeg` / `FTP` / `GitHub` / `HuggingFace` / `M3U8` / `YtDlp` 一起写盘。
实测（真 Python 写的档）：`--set maxTaskNum=7` 之后文件 **163 行 → 66 行**，
等于用户改任何一项设置就把 BT / ED2K / 网盘配置整段删掉。

现在的口径（[`rebuild`]）：源里的组按**原位置**输出，已知组用归一化值 + 源里多出来的
键原样附在组尾，Rust 不认识的组整体原样带上；源里没有的已知组按 `SPECS` 顺序补在末尾。
代价写进了模块头「与 Python 的已知差异」：连「谁都没注册过的野键」也会被带回去
（Python 下次 `save()` 会丢掉）—— 单一写者场景下宁可留着，也不要静默删用户数据。

验收：`gd3-ui/src/config_store.rs` 的 `unknown_pack_groups_are_preserved_verbatim` /
`unknown_keys_on_disk_survive_the_round_trip`，以及 `tests/test_config_roundtrip.py`
（fixture 显式导入全部 `features/*/config.py`，所以写出的档是 18 组的真形态，
且**与测试执行顺序无关**）。

### 平台外壳（托盘 / Dock / 状态项）：**降级**

`shell.rs`（第 75 步）已经有全套**语义**（`tray_menu()` 五项顺序、`toTraySpeed` /
`toDockSpeed` 的两套小数口径、`dock.onSpeedChanged` 的 `shouldShowDockSpeed and
shouldShowDockIcon` 条件、Windows 菜单定位钳制、激活原因到信号的映射），但
**更正（第 132 步实测）**：Slint 1.18.1 **有**原生托盘元素 `SystemTrayIcon`
（`i-slint-core-1.18.1/items/system_tray.rs`，后端 `appkit` / `windows` / `ksni`），
而且 `system-tray` 就在 `slint` 的 default features 里 —— `gd3-ui` 当前的依赖已经把它
编译进去了。所以这项不必「降级到底」：在 `.slint` 里派生一个
`inherits SystemTrayIcon` 的组件、把 `shell.rs` 算好的五项菜单接上去即可，Linux 走
`ksni`（StatusNotifierItem over D-Bus）。`run_event_loop()` 在有可见托盘图标时也会
继续跑，正好配 `CloseMode=Background`。**当前仍未接**，`CloseMode=Background` 还是
「藏窗口 + 一条 stderr 提示」。原方案（万一要用平台原生 API）：

| 平台 | 要用的 API | 现状 |
|---|---|---|
| Linux | `libappindicator` / `ksni`（StatusNotifierItem over D-Bus），菜单走 `com.canonical.dbusmenu` | 未接（`SystemTrayIcon` + `ksni` 后端现成） |
| Windows | Win32 `Shell_NotifyIcon` + `TrackPopupMenu`（`tray.py` 的 `AcrylicMenu` 定位钳制对应 `TrackPopupMenuEx` 的屏幕工作区） | 未接（`SystemTrayIcon` + windows 后端现成） |
| macOS | AppKit `NSStatusItem` + `NSMenu`（`mac_status_item.py`） | 未接（`SystemTrayIcon` + AppKit 后端现成） |

接线方式（**不改现有契约**）：加一个 `TrayHost` trait（`show_menu(entries)` /
`set_tooltip(text)` / `set_icon(bytes)` / `set_dock_speed(text)`），宿主按平台选实现，
`NullTray`（默认）只把 `shell.rs` 算好的东西丢掉。`shell.rs` 的返回类型已经是
平台无关的，所以接的时候**不需要动它**。

### 跑 Python 全量必须带 `--continue-on-collection-errors`

本机没有 `android/`（`.gitignore` 里，且 `9c17647 refactor: remove Qt Android` 已删），
而 `tests/test_android_pack_import.py` 在模块级读 `android/app/build.gradle.kts` →
收集期报错，默认参数下 pytest 直接 `Interrupted: 1 error during collection`、
**一个测试都不跑**。所以对比基线要用：

```bash
cd /root/Ghost-Downloader-3
.venv/bin/python -m pytest -q --continue-on-collection-errors 2>&1 \
  | grep -E '^(FAILED|ERROR) ' | sort > /root/run-failures.txt
comm -13 /root/baseline-failures.txt /root/run-failures.txt   # 只看新增
```

（`/root/baseline-failures.txt` 第 25 行就是这个收集错误，说明基线也是这么跑的。）

### 关窗语义与窗口分支验收（第 132 步）

Slint 的 `on_close_requested` **缺省返回 `CloseRequestResponse::HideWindow`**
（`i-slint-core-1.18.1/window.rs:2421`）：窗口隐藏，事件循环照跑。`gd3-ui` 还有
`Runtime` 的 `slint::Timer`，所以「点 ✕」若不接管，进程会变成看不见的僵尸。
第 132 步按 Python `_onCloseClicked`（`app/view/windows/main_window.py:440-475`）
补齐 `gd3-ui::close_request`：

| `Software/CloseMode` | Python | `gd3-ui` |
|---|---|---|
| 任意，但消息框开着 | 先 `reject()` 那个框，窗口留着 | `CloseDecision::RejectDialog` |
| `Ask`（缺省） | `MessageBox`：退出程序 / 继续在后台运行 + 记住我的选择 | `window::close_ask_dialog()` |
| `Background` | `self.close()` 隐藏，托盘图标留着 | `Window::hide()` + 首次降级时一条 stderr 提示 |
| `Quit` | `self.close()` + `QApplication.quit()` | `quit_event_loop()` |

未知字面量 / 缺键按 `Ask` 兜底（`close_mode.rs` 的既有口径，不复刻 Python 的
`KeyError`）。`dialogButtonClicked` 是**单槽**回调，`close_request::install` 独占；
将来别的对话框要分派按钮时先问 `handle_dialog_button`（返回 `false` = 不归它管，
交给下一个处理器）。darwin 的两个特例（`mode != QUIT` 时只 `hide` 并先存几何、
全屏时 `showNormal` + 1 秒后重来）依赖 AppKit 窗口状态，暂未镜像。

**两个坑（都会让人误判）**：

1. **`window.run()` 不等价于「跑到用户退出」**。`ComponentHandle::run()` =
   `show` + `run_event_loop()` + `hide`，而 `run_event_loop()` 在**最后一个窗口隐藏时
   自己结束** —— `CloseMode=Background` 会变成「藏了窗口然后静默退出」。改用
   `window.show()` + `slint::run_event_loop_until_quit()`（官方文档：
   “continues to run even when no windows or system tray icons are visible, until
   `quit_event_loop()` is called”）。
2. **`xdotool windowclose` 是 `XDestroyWindow`**，不产生关闭请求，`on_close_requested`
   根本不会被调用（实测：窗口没了、处理器一次没进）。真正的「点 ✕」是给窗口发
   `WM_PROTOCOLS` / `WM_DELETE_WINDOW` 的 `ClientMessage`；第 132 步用
   libX11 + ctypes 写了个几十行的小工具才拿到真事件。

窗口分支的验收（`Xvfb :99` + `WM_DELETE_WINDOW`，2 MiB 限速载荷）：

| 场景 | 期望 | 实测 |
|---|---|---|
| `Quit` | 进程干净退出 | `rc=0`，事件循环返回，停机序列 flush 完才走 |
| `Ask` | 窗口留住 + 弹框 | 窗口仍 `IsViewable`、进程活；按钮文案/勾选落盘由单测覆盖 |
| `Background` | 窗口消失、进程继续下载 | 无可见窗口、进程活，任务照样 `COMPLETED`、`cmp` 逐字节一致 |

顺带两条口径：手写 `tasks.jsonl` 种子时 `completedAt` 必须写 `0`
（Python 是 `Task.completedAt: int = 0`），写 `null` 会被 `TaskRecord` 的 `i64` 拒掉、
整条任务按加载失败丢掉（摘要里 `tasks=0`）；`.ghd` **只在范围请求模式下**写，
非 Range 服务器上暂停后没有 `.ghd` 是预期行为。

### `shouldPreserveLastModified`：跑完把 mtime 设回去（第 133 步）

Python 在两处做同一件事（`features/http_pack/rust_engine.py:103-110` 的
`_restoreMtime`，以及 `features/http_pack/task.py:472-480` 里任务被删记录时的重复实现）：

```python
if not cfg.shouldPreserveLastModified.value or not step.lastModified:
    return
try:
    mtime = parsedate_to_datetime(step.lastModified).timestamp()
    os.utime(Path(step.outputPath), (mtime, mtime))
except Exception as e:
    logger.opt(exception=e).warning("设置文件修改时间失败 {}", step.outputPath)
```

Rust 侧没有要新增的头部/探测逻辑：`lastModified` 是**建任务时**（`pack.py:177` 取响应头
`last-modified`、`rust_probe.py:48`）写进 `tasks.jsonl` 的字符串，`task_runner` 从
`step.extra` 读就行。缺的两块是：一个 HTTP-date 解析器（std 没有），和一个「只改 mtime」
的落盘点。

- 新模块 **`gd3-core::httpdate`**：`parse` / `parse_unix_seconds` 认 HTTP-date 的三种历史
  格式（IMF-fixdate `Sun, 06 Nov 1994 08:49:37 GMT`、RFC 850
  `Sunday, 06-Nov-94 08:49:37 GMT`、asctime `Sun Nov  6 08:49:37 1994`），带数值/缩写时区；
  `apply_last_modified(path, raw)` 才是落地那步。两位年份的世纪归属照抄
  `email._parseaddr._parsedate_tz`（`0..=68` → 2000 年代，`69..=99` → 1900 年代）。
  单测里有一张**逐条对齐 CPython** 的表（`matches_python_parsedate`，12 个样本都是
  `parsedate_to_datetime(s).timestamp()` 的实跑值），避免「自洽但错」。
- **`task_runner`**：`RunnerSettings.preserve_last_modified`（读属性名
  `shouldPreserveLastModified`，缺省 `false`）→ `plan_job` 快照进
  `DownloadJob.preserve_last_modified`，`step.extra["lastModified"]` 进
  `DownloadStep.last_modified`（空串按缺失处理，`HttpTaskStep.lastModified` 的缺省就是
  `""`）。`run_job` 在 `engine.run()` 返回 `Ok(())` 之后、发 `StepFinished` 之前调
  `restore_last_modified`：**失败只打 stderr**（`设置文件修改时间失败 <path>：<err>`），
  绝不让任务失败 —— 很多挂载点不支持改时间戳，而「文件已下完」是既定事实。

| 差异 | 说明 |
|---|---|
| 只改 mtime，不改 atime | `os.utime` 两个都设；std 只有 `File::set_modified`，设 atime 要 `filetime` 这类外部依赖，为一个看得见的属性不值得 |
| 表外时区缩写按 UTC | Python 得到 naive datetime，`.timestamp()` 再按**本地** `TZ` 解释（结果随机器变）；这里固定 UTC，可复现 |
| 数值时区也接受 `+08:00` | Python 的 `parsedate` 只吃 `+0800`；放宽只会让更多输入可用 |
| 星期名只跳过、不校验 | 与 `parsedate_tz` 一致 |
| 只读文件照样能改 mtime | 先试写句柄，失败退只读句柄（`os.utime` 只要属主，不需要写权限） |

验收（第 133 步实跑）：

- 单测：`gd3-core::httpdate` 11 条全绿；`task_runner` 34 条全绿（新增「空串按缺失」
  「开关开/关只影响 mtime」「设置页的 `GeneralDownload/PreserveLastModified` 与 runner 读的
  属性名指向同一个键」）。
- e2e（生产路径 `TaskRunner::new` + `gd3_net::HttpEngine` + 本地 HTTP 服务）：
  `runner_e2e::preserving_last_modified_stamps_the_real_download` → 文件逐字节一致且
  mtime = `784111777`（1994-11-06T08:49:37Z）；`the_default_settings_leave_the_mtime_alone`
  → 缺省不动 mtime。
- 真二进制：`gd3-ui --headless --until-idle --set shouldPreserveLastModified=true`
  → `stat` 显示 `1994-11-06 08:49:37`、`cmp` 逐字节一致、rc=0，`UserConfig.json` 里
  `PreserveLastModified` 落在 Python 的同一位置（`DeleteFilesOnRemove` 与
  `shouldVerifySsl` 之间）；不带 `--set` 的默认配置跑同一条任务 → mtime 是当前时间。

### 限速闸门：`waitForSpeedLimit` 接进下载循环（第 134 步）

Python 侧的速度表兼限速器（`app/services/speed_meter.py`，56 行）有两半：

```python
def addSpeed(self, byteCount): self._bytes += byteCount          # 每个数据块
async def waitForSpeedLimit(self):
    while cfg.isSpeedLimitEnabled.value and self._bytes > cfg.speedLimitation.value:
        await asyncio.sleep(0.1)                                  # 每个数据块之后
async def _tickLoop(self):                # 每秒把 _bytes 清零
    while True: await asyncio.sleep(1); self._coroutineRunner.post(self._tick)
```

第 104 步已经把「速度表」这一半做成了 `gd3-ui::speed_meter`（`Cell` 版，活在 UI 线程，
`add_speed` / `should_wait` / `tick` 都在那儿）。缺的是**真的能拦住工作线程**的那一半：
`gd3-net` 早就有 `SpeedGate` trait（`engine.rs:59`，每个数据块后 `gate.wait().await`），
但没人实现它 —— `HttpEngine.speed_gate` 一直是 `None`。

- 新模块 **`gd3-ui::speed_limit`**：`SpeedLimit` 用四个原子量（`bytes` / `enabled` /
  `limit` / `closed`）+ 一个窗口起点做跨线程共享，`impl gd3_net::SpeedGate`，
  `wait()` 就是 `while self.exceeded() { tokio::time::sleep(100ms).await }`。
- **`task_runner`**：`RunnerSettings` 从七项变九项（新增 `speed_limit_enabled` ←
  `isSpeedLimitEnabled`、`speed_limit_bytes` ← `speedLimitation`，缺省
  `false` / 4 MiB）；`TaskRunner` 持一份 `Arc<SpeedLimit>`，`set_settings` 每轮把
  两个值推进去（等价于 Python 的「每轮重读 `cfg`」），`Spawner::spawn` 多一个 `gate`
  参数（工作线程拿到**同一份**，Python 也只有一个 `speedMeter`）；`run_job` 把它装进
  `engine.speed_gate`，并在进度回调里 `add(chunk_delta)`。
- `runtime::tick_host` 每秒 `runner.speed_limit().reset()`，让清零时刻与顶栏速度表同一拍。

踩到的坑（都已修，且都留了测试）：

1. **闸门不能依赖宿主 tick**。第一版把窗口清零完全交给 UI 线程的 1 秒节拍，e2e 测试
   （只 `drain`、不起 Runtime）直接把一个 512 KiB 的下载挂死在那儿 120 秒。改成闸门
   **自带时钟**：`exceeded()` 先看窗口是不是过期（`WINDOW = 1s`），过期就滚 —— 外部行为
   与「每秒清零」完全一致，但正确性不再建立在「宿主一定在 tick」这个假设上（无头测试、
   停机之后都没有 tick）。
2. **滚窗口的目标必须严格前进**。`compare_exchange` 在「值没变」时也会成功，
   同一毫秒里两次滚动就会把窗口清两遍（刚加进来的字节被抹掉）。目标取
   `now.max(start + 1)`，`reset()` 在 `epoch` 的第一毫秒也能真的清零。
3. **停机要主动放行**。`TaskRunner::shutdown` 先 `speed_limit.close()` 再 `cancel_all()`：
   否则正卡在闸门上的工作线程要等满一个窗口才可能出来。

验收（第 134 步实跑）：

- 单测：`speed_limit` 8 条（严格大于、`reset`、**自己滚窗口**、并发滚动只清一次、
  不超限立即返回、清零后放行、`close` 放行、关闭时不拦人）+ `task_runner` 2 条
  （闸门与 spawner 共享同一份且跟随配置；`shutdown` 会 `close` 它）。
- e2e（生产路径）：`runner_e2e::the_speed_limit_actually_throttles_a_real_download`
  —— 同一份 512 KiB 载荷跑两遍（不限速 vs 128 KiB·s⁻¹），断言**倍数**而不是绝对秒数：
  限速耗时 ≥ 不限速的 4 倍且 ≥ 1 s（本机实测 2.0–3.2 s vs 0.25 s 上下，写死秒数会飘红
  —— 第一次就是这么红的）。两遍内容都逐字节一致。
- 真二进制：1 MiB 载荷，`--set isSpeedLimitEnabled=true --set speedLimitation=131072`
  → **4.35 s**（≈235 KiB/s）；同样的任务不限额 → **0.25 s**；两次的产物都 `cmp` 一致。
  实测略高于上限是**预期**：判据在「写完之后、下一个数据块之前」，一个窗口最多超出
  一个数据块，Python 的 `waitForSpeedLimit` 位置一模一样。

### 更新检查的执行层：`UpdateEffect` 真的被执行（第 135 步，`gd3-ui::update_exec`）

第 115 步搬完的是**计划层**（`plan_check` / `plan_app_download` / `plan_pack_download` /
`plan_apply`），第 124 步搬完的是**状态机**（`UpdateRuntime`：十个 `UpdateEffect` 出口、
`versions_ready` / `download_finished` / `checksum_finished` / `unpack_finished` /
`apply` 入口）。缺的一直是中间那层：**谁去把这些动作做掉**。Python 里它是
`UpdateService` 的 `async` 方法（`fetchJson` / `fetchReleaseAsset` / `fetchRawFile` /
`toChecksum` / `extractZip` / `shutil.rmtree` / `subprocess.Popen`）。

第 135 步补上这层：

- **新模块 `gd3-ui::update_exec`**：`UpdateExecutor` 持 `Arc<dyn Transport>` + `Repo` +
  `branch` + `features_dir`，对外三个入口 `check` / `download` / `apply`，都落到
  `run(runtime, target_id, effects)`。`run` 把动作排进 `VecDeque`，**一条条 await**，
  每条执行完把状态机新吐的动作接回队尾 —— 直到队列空（`MAX_EFFECTS = 4096` 兜底）。
  这与 Python 的 `await` 串行完全同形。
- **`gd3-net::sources::fetch_raw_file`**：`fetchRawFile`（`sources.py:163`）此前没有对应
  函数（`update_service` 的 `FetchRaw` 走的就是它）。实现是「raw 竞速探测 → 获胜 URL 上
  `Transport::download` → 返回获胜源名」，失败文案用 `gd3_core::sources::raw_file_failure`
  （「无法下载 …」，注意与 `fetch_json` 的「无法获取 …」不是同一句）。
- **校验和**在宿主算（`gd3_core::checksum::to_checksum`），**比对**在状态机
  （`checksum_accepted`）—— 和 Python 的 `toChecksum(...) != parseChecksum(sha)` 分工一致，
  所以「校验失败删归档 + `FAILED`」这条仍由状态机发 `RemoveFile`。
- **解包复用 `extension_crx` 的 ZIP 读取器**：`entries` / `read_entry` / `sanitize` 第 121 步
  已按 `zipfile.extractall` 的口径验过（deflate + 路径净化 + 中央目录序 + 父目录按需建），
  所以 `unpack_zip` 只是「去掉 CRX 头那一步」。`_applyPack` 的 `zipPath.unlink()` 由
  executor 在「非 pending 的 `Unpack` 成功后」补一条 `RemoveFile`（完整包的删除由状态机发）。

| 差异 | 说明 |
|---|---|
| 进度回调**先攒后放** | `onProgress` 是同步闭包、`download_progress` 要 `&mut UpdateRuntime`，两个借用不能同时存在。回调把百分比塞进 `Mutex<Vec<f64>>`，下载返回后按原顺序回放 → UI 看到的顺序与 Python 相同，只是推送时刻集中在下载结束 |
| `tar.xz` / `dmg` 解包**未接** | 离线环境没有 `xz2`；`dmg` 要 macOS 的 `hdiutil`。两者返回 `TaskError::io`，状态机照常 `FAILED`，归档保留（Python 的「留着好排查」） |
| `StartUpdater` **只记录不 spawn** | updater 会顶掉当前进程，拉起时机应由 UI 层在用户确认后决定；`UpdaterCommand` 放进 `UpdateRun::spawns` 交出去 |
| `RemoveDir` / `RemoveFile` **不存在也算成功** | 对应 Python 的 `if newDir.exists()` 与 `unlink(missing_ok=True)` |

验收（第 135 步实跑）：

- 单测 `update_exec` **10 条**：本地 pack 与远程更高版本 → 双 `AVAILABLE`（`_pending` /
  隐藏目录不进扫描）；`versions.json` 抓不到 → `IDLE` + 「无法获取版本信息」；sha 对得上 →
  `READY` 且 staging 里字节一致、进度回放两条 `DOWNLOADING`（第二条 `progress=100`）；
  sha 对不上 → `FAILED` + 归档被删；下载 404 → `FAILED` + 「无法下载 …/dist/packs/youtube.zip」
  （两条 raw 源都探测过、一次下载都没发起）；`apply` → `features/youtube_pending/` 里
  `pack.py` + `assets/icon.png` 就位、zip 被删；app 完整包 → 解到 `staging/app_new/`、
  归档删、`spawns` 空；`tar.xz` → `FAILED` 且错误里带 `xz2`、归档保留；
  app 就绪 + updater 在场 → `apply` 交出 `UpdaterCommand`；`UnpackKind` 两路未接的错误文案。
- **真网络 e2e**（`tests/update_exec_real.rs`，`#[ignore]` 因为要外网，本轮实跑过）：
  生产路径 `WreqTransport` + `app_repo()` + `main` → 抓真实 `versions.json`
  （`app 4.4.0 → 4.4.3`、`bittorrent_pack 1.0.0 → 1.1.1` 双 `AVAILABLE`）→ 真下 20 657 字节
  zip（进度 79.3% / 100%）→ sha256 对上 → `READY` → `apply` 解出 12 项
  （`manifest.toml` / `pack.py` / `lists/` …）→ zip 被删，`errors` 空。

### 托盘：`SystemTrayIcon` 真的挂上去了（第 136 步，`gd3-ui::tray`）

Phase 8f 之前一直记成「Slint 无原生能力，允许降级」。第 132 步读源码时发现这个结论过时了：
Slint 1.18 自带 `SystemTrayIcon`（Linux 走 `ksni` = StatusNotifierItem、macOS 走
`NSStatusItem`、Windows 走 `Shell_NotifyIconW`），而且 `system-tray` 就在 `slint` 的
default features 里 —— 也就是说早就编进二进制了。第 136 步把它接上：

- **`ui/system_tray.slint`**（新文件）：`export component Gd3TrayIcon inherits
  SystemTrayIcon`。`SystemTrayIcon` 必须是**独立顶层组件**（不能塞进 `AppWindow`，
  它的 `Menu` 也不能放进 `if`/`for`），所以 `build.rs` 里多了第二条
  `slint_build::compile_with_output_path("ui/system_tray.slint", $OUT_DIR/system_tray.rs)`，
  由 `tray.rs` 显式 `include!`（`include_modules!()` 只认 `SLINT_INCLUDE_GENERATED`
  那一份）。副作用：`compile_with_output_path` **不打印 cargo 指令**，`rerun-if-changed`
  得自己写，否则改 `.slint` 不重编。
- **五项菜单与 `shell::tray_menu()` 逐项对齐**（仪表盘 / 全部开始 / 全部暂停 / 分隔线 /
  退出程序），文案与顺序由一条漂移测试盯着：它 `include_str!` 那个 `.slint`，扫出
  `MenuItem.title` 与 `MenuSeparator` 的序列，和 `shell::tray_menu()` 比 —— 改了
  `shell.rs` 却没改 `.slint`（或反过来）直接红。
- **左键单击不需要在 Rust 里接**：`.slint` 里 `clicked => { root.dashboard-clicked(); }`
  已经把 `SystemTrayIcon.clicked`（Qt 的 `ActivationReason.Trigger`）转成同一个回调，
  与 `shell::tray_activated` 的「只有 Trigger 才算激活」一致。
- **提示气泡跟着速度表走**：`Runtime` 多了一个 tick 钩子（`set_tick_hook`）。
  运行期没有「服务层信号」这一层，`tick` 是既有的收口点，钩子在 tick **之后**调用，
  读得到这一轮的 `RunnerTick::speed`；`tray::apply_tooltip` 只在 `speed` 有值时改文案
  （`shell::tray_tooltip`：`speed > 0` → `"Ghost Downloader\n↓ 1.50 KB/s"`，归零退回名字）。
- **「全部开始 / 全部暂停」需要两层**（`runtime::start_all` / `pause_all`）：
  `TaskService::start_all` 只把 `PAUSED`/`WAITING`/`FAILED` 挪进等待队列（Python 的
  `_schedule` + `_pump`），**真正的「建 job + 起工作线程」在 `TaskRunner::dispatch`**。
  只调服务层会让队列里多出一批「占着槽位、没人干活」的任务（槽位被
  `queue.run(id, "")` 占上，之后 `tasks.pump` 不会再挑它们）—— 第一版就是这么写的，
  两条单测当场红。`pause_all` 同理：先快照 id，再逐个走 `TaskRunner::pause`
  （先取消工作线程，再服务层记账），服务层 `pause` 顺带 `_pump` 出来的新 id 也要过运行器。
- **只建在有头分支**：托盘 `ksni` 的服务循环要事件循环，无头冒烟既没有它也没有桌面会话。
  创建失败（连不上会话总线 / 没有 `StatusNotifierWatcher`）只 `eprintln!` 一行，
  窗口照常工作 —— Qt 侧同样不会因为托盘失败就拒绝启动。

| 差异 | 说明 |
|---|---|
| 三个自绘菜单类不搬 | `RoundMenu` / `TrayRoundMenu` / `AcrylicMenu` 连同 `adjustPosition` 的坐标钳制、亚克力底色、`itemHeight(28)` / `iconSize(14)` 都归桌面环境（菜单由 SNI / AppKit / 系统通知区画），`shell::tray_menu_flavor` 的判据保留供对照 |
| 菜单项没有图标 | Qt 用 `FluentIcon`（`GHOST`/`PLAY`/`PAUSE`/`CLOSE`）；Slint 的 `MenuItem.icon` 要自己准备位图资源，本步没做（只有文字） |
| `messageClicked` 没有对应物 | Slint 的托盘只有 `clicked`，通知气泡归桌面环境 |
| 菜单项的启用/禁用没做 | `_refreshMenuItems`（macOS 状态项）在 `PAUSED/FAILED/WAITING` 时亮「全部开始」，那需要把任务状态推进菜单 —— 属于 `status_menu` 的活 |

验收（第 136 步实跑）：

- 单测 `tray` **5 条**：`.slint` 菜单与 `shell::tray_menu()` 的漂移比对；
  `tray_action` 给得出信号的四项都有人接；提示气泡（初值 = `Ghost Downloader`、
  `None` 不动、1536 → `1.50 KB/s`、0 → 退回名字）；回调真的接上了
  （`invoke_dashboard_clicked` → 窗口从隐藏变可见，`invoke_start_all_clicked` /
  `invoke_pause_all_clicked` / `invoke_quit_clicked` 在空任务表上安静跑完、不 panic）；
  `SystemTrayIcon` 在无头平台上也构造得出来（注册失败只是 Slint 的一条 debug 日志）。
- `runtime` 新增 **4 条**：`start_all` 派发 `PAUSED`/`FAILED`、不动 `COMPLETED`；
  `pause_all` 取消每个在跑的工作线程并置 `PAUSED`；没有在跑/等待的任务就一条都不动；
  tick 钩子每轮都跑且读得到速度。顺带把测试脚手架拆成 `harness` / `harness_with`
  —— `add(record, true)` 会把**每条**记录塞进等待队列（与记录的 `status` 无关），
  想复现「库里躺着一条完成的任务」必须关掉它。
- **真二进制 + `Xvfb :99`**（`WM_DELETE_WINDOW` 的 ClientMessage，不是 `xdotool
  windowclose`）：托盘创建路径真的被走到且失败是软的 —— 无会话总线时
  `Slint: Failed to create system tray icon: … Failed to connect to address
  unix:path=/run/user/0/bus`；起一个 `dbus-daemon --session`（没有
  `org.kde.StatusNotifierWatcher`）时变成
  `failed to register to the StatusNotifierWatcher: … ServiceUnknown`。
  两种情况下窗口都正常显示、`--set closeMode=Quit` 发 `WM_DELETE_WINDOW` 后
  **rc=0**（停机序列照跑），`closeMode=Ask`（缺省）按原语义只弹框不退。
- 质量门：`fmt --check` OK；`clippy --workspace --all-targets -D warnings` **exit 0**；
  `cargo test -p gd3-core -p gd3-net -p gd3-servers -p gd3-ui` **GATE_EXIT=0**、
  29 个 target 全 ok、**1783 passed**（第 135 步 1774，+9）；Python 全量
  → 3 failed / 1370 passed / 8 skipped / 35 errors，与基线**逐条相同**。

### 建任务链路：`URL → TaskRecord`（第 137 步，`gd3-ui::task_create`）

到第 136 步为止，Rust 只跑**已经在** `tasks.jsonl` 里的任务：不解析 URL、不探测、
不造记录 —— 建任务那一段一直留在 Python（`features/http_pack/pack.py`）。这一步把
`buildTask` 与 `HttpParser` 搬过来，整条流水线是：

```text
ParserChain::parse(options)                  ← 第 108 步已有
  └─ HttpParser::parse(options)              ← 本步
       ├─ ResourceTaskOptions(name, size>0)  → 直接当 RemoteFile（不探测）
       └─ 否则 probe(options)                → gd3_net::probe::probe（第 113 步）
       └─ build_task(options, file)          ← 本步
```

- **`build_task` 与 Python 逐键同序**（`serde_json` 开了 `preserve_order`，
  `TaskStep.extra` 的插入顺序就是 JSON 顺序）。单测拿一份真 Python
  跑出来的 `toDict()` 当基准，逐字节比对：`stepIndex: 1`、`httpByteOffset: 0`、
  `isAccelerated: false`、`type: features.http_pack.task.HttpTaskStep` 等等一个不差。
  `subworkers` / `error` 不落盘（Python 的 `field(repr=False)`），`shouldSeed` 是
  `false`（播种标记归 `TaskService::add`）。
- **`TaskOptions` → [`CreateOptions`]**：`fromOptions` 的 `filterFields` 语义（只认自己
  那批键）+ 四个 `field(default_factory=...)` 由 [`CreateDefaults`] 承担。
  `CreateDefaults::from_config(&UserConfig)` 一次读齐 `downloadFolder` /
  `headersPresets`+`currentHeadersPreset`（即 `cfg.currentHeaders()`）/
  `clientProfile` / `preBlockNum`。
- **探测做成可注入的 [`Prober`]**：`parse` 是同步签名，生产实现 `NetProber` 自带一个
  current-thread runtime 并 `block_on`；单测塞桩，一条网络都不发。
- **`create_tasks` 是三条入口共用的胶水**（装解析器 + 逐条 `parse` + `add`），
  单测验过「1 条 http 建成、1 条 ftp 记失败、落盘可读回」。

| 差异 | 说明 |
|---|---|
| `ResourceTaskOptions` 靠键存在性判 | Python 有类型（`isinstance`），Rust 的 options 是 `Map`：`name`/`size`/`canUseRangeRequests` 任一存在即视为 resource |
| `headers` 只收字符串值 | Python `dict(options.headers)` 原样保留任意值（非 str 会在 httpx 里炸），Rust 直接跳过 |
| `clientProfile` 两处用法分开 | 探测用 `options.clientProfile or cfg.clientProfile`（空则回落），落盘写原始值（可能空串）——与 Python 同 |
| `guess_extension` 不在本模块 | `gd3-net::probe` 的 `ExtensionResolver` 由调用方注入（mimetypes 是平台相关的表），不注入则兜底名没有扩展名 |
| 探测是阻塞的 | Python 是 `await`；Rust 侧 `TaskParser::parse` 同步 → 宿主该在后台线程里调（**不要**在 tokio 运行时线程里调 `NetProber`） |

**入口还没接**：位置参数 URL / 浏览器扩展 / aria2 RPC 三处仍然只有空的解析器链
（`Engine::new` 不装解析器），草稿对话框的 `DraftParser` 要的是「提交 → 后台探测 →
回填 `on_parsed`」，得配一条 worker 线程 + 通道。链路本身已通，入口接线是下一步。

验收（第 137 步实跑）：

- 单测 `task_create` **10 条**：与 Python `toDict()` 逐字节对齐；resource 名走
  `toSafeFilename`（`weird/name?.mp4` → `weird_name_.mp4`）且不探测；`size <= 0`
  仍然探测；探测入参回落 cfg（`clientProfile` / `headers` / `fallbackName` 前缀）；
  探测错误原样冒泡；`match`/`matchPassive` 转调 `gd3-core::http_pack`；进
  `ParserChain` 后同一条路；`headers` 保序且丢非字符串；`CreateDefaults::from_config`
  读的是宿主配置；`create_tasks` 的建成 / 失败 / 落盘三条断言。
- e2e `tests/task_create_e2e.rs` **2 条**（本地 HTTP 服务器 + **真** `NetProber`）：
  1. URL →（真探测）→ 记录 → 写进 `tasks.jsonl` → `boot::run(--until-idle)` 下完 →
     逐字节一致、状态回写 `COMPLETED`；
  2. 同一条记录直接进 `TaskService` → `Runtime::tick` 推着下完、`.ghd` 删掉。
- 质量门：`fmt --check` OK；`clippy --workspace --all-targets -D warnings` **exit 0**；
  `cargo test -p gd3-core -p gd3-net -p gd3-servers -p gd3-ui` **GATE_EXIT=0**、
  30 个 target 全 ok、**1795 passed**（第 136 步 1783，+12）；Python 全量
  → 3 failed / 1370 passed / 8 skipped / 35 errors，与基线**逐条相同**。

### 后台解析：`draft_worker`（第 138 步，`gd3-ui::draft_worker`）

第 137 步交付了「URL → TaskRecord」的链路，但 `TaskParser::parse` 是**同步**的，
真实现 `NetProber` 会 `block_on` 一次探测（秒级，30 s 超时）——直接在 UI 线程里调就是
窗口卡死。草稿对话框在 Python 侧是 `coroutineRunner.submit(coro, done=…, failed=…)`
（`app/view/dialogs/task_draft.py`），天生异步，所以这一步补上「提交 → 后台探测 →
结果回灌」的那层：一个工作线程 + 两个 `std::sync::mpsc` 通道。

- **[`ParseWorker`]**：`spawn(CreateDefaults)` / `with_parser(HttpParser)` 起一条
  `gd3-draft-parse` 线程；`submit(work_id, options) -> bool`、`cancel(work_id)`、
  `drain() -> Vec<ParseOutcome>`（UI tick 每轮非阻塞取）、`recv_timeout`（测试用）、
  `shutdown()`（关提交端并 join）。
- **[`ParseOutcome`]**：`Parsed { work_id, task }` / `Failed { work_id, error }`。
  `work_id` 是 [`crate::task_draft::TaskDraft`] 发的不透明句柄，原样交回。
- **`drain_into(worker, &mut TaskDraftUi, host)`**：把回来的结果灌进对话框状态机
  （`on_parsed` / `on_parse_failed`），返回的效果照旧走
  [`task_draft_ui::handle`]——那条路第 138 步之前就存在，只是结果来自通道而不是递归。
- **[`WorkerSubmitter`]**：宿主实现 `DraftHost::submit_parse` 的最小粘合
  （`submit` / `cancel` 转调），真正的宿主还要自己实现 `can_edit` / `confirmed` 等钩子。
- **`ParseSubmit`（异步是 opt-in）**：`DraftHost::submit_parse` 有默认实现
  `ParseSubmit::Sync(self.parse(options))`，所有既有的同步宿主/测试**行为一字不变**；
  真后台宿主返回 `ParseSubmit::Async`，`TaskDraftUi::run` 就只推一条
  `UiEffect::ParseSubmitted { work_id }` 并立刻返回。`DraftEffect::Cancel` 现在会
  转成 `DraftHost::cancel_parse`（之前被吞在 `UiEffect::Draft` 里，后台线程会白跑）。
- `TaskDraftUi::handle` 改 `pub`：宿主 tick 里「先 `drain_into` 再 `handle`」，
  窗口那层不新增可测性很差的胶水函数。

| 差异 | 说明 |
|---|---|
| 串行而非并发 | Python 每条 URL 一个 `Task`；这里一条线程按 FIFO 跑，一个 `NetProber`/runtime 复用到底。批量粘 20 条是慢一点，不是错 |
| 取消是协作式的 | `block_on` 里的探测打断不了，只能「结果回来时丢掉」。提交前取消、落地前取消都会丢；项上 `parse_id` 清空后迟到结果无副作用 |
| 不做 `Drop` join | 正卡在探测里的线程会把宿主拖死，所以 `ParseWorker` 故意不实现 `Drop`：丢掉时提交端消失，线程跑完手上这条自然退出 |
| `ParseOutcome` 装 `Box<TaskRecord>` | `TaskRecord` 几百字节，过通道只搬指针（`clippy::large_enum_variant`）；`ParseSubmit::Sync` 同理 |

**入口还没接**：`Engine::new` 仍然不装解析器，`boot` 也没有 `task_draft_ui::install`
——真窗口里的草稿对话框（`ui/task_draft.slint` 已在 `appwindow.slint` 里）还差
「装回调 + 一个真 `DraftHost` + Slint 定时器里 `drain_into`」。这一步交付的是那层机制。

验收（第 138 步实跑）：

- 单测 `draft_worker` **12 条**（桩探测器，零网络）：成功/失败回灌、提交前取消与
  **跑着取消**（`Gate` 把探测卡在 `probe` 里，时序是钉死的，不靠赛跑）、取消集合的
  回收、FIFO 串行顺序、提交端消失后 `submit == false`、`drain_into` 喂对话框 /
  报失败给宿主、确认后保留探测记录、给已清空草稿的迟到结果无害。
- e2e `tests/draft_worker_e2e.rs` **1 条**（本地 HTTP 服务器 + **真** `NetProber`）：
  粘 URL → `parse_needed` 立刻返回 `ParseSubmitted`（那一刻 `view().rows` 还是空的
  ——UI 线程没被探测挡住）→ tick 里 `drain_into` 拿到真记录（名字 / 大小 / Range
  支持都来自真探测）→ `accept` → 记录进 `TaskService` → `Runtime` 推着下完、逐字节一致。
- 质量门：`fmt --check` OK；`clippy --workspace --all-targets -D warnings` **exit 0**；
  `cargo test -p gd3-core -p gd3-net -p gd3-servers -p gd3-ui` **GATE_EXIT=0**、
  31 个 target 全 ok、**1808 passed**（第 137 步 1795，+13）；Python 全量
  → 3 failed / 1370 passed / 8 skipped / 35 errors，与基线**逐条相同**（`comm -3` 0 行）。

### 草稿对话框装进真窗口：`draft_host`（第 139 步，`gd3-ui::draft_host`）

第 137 步给了「URL → TaskRecord」，第 138 步给了「后台探测 + 回灌」，但
`task_draft_ui::install` 一次都没被调用过——窗口里的草稿对话框（`ui/task_draft.slint`
早就在 `appwindow.slint` 里）始终是死的。这一步补上宿主。

- **[`UiDraftHost`]**：生产环境的 `DraftHost`。
  - `submit_parse` / `cancel_parse` → 后台 [`crate::draft_worker::ParseWorker`]；
  - `confirmed` → `TaskService::add(task, autoStart)`（入队；真正的派发在下一轮
    `Runtime::tick` 的 `TaskRunner::pump`），重复任务打一行 stderr、磁盘不够弹通知条；
  - `has_conflict` → `TaskService::probe_conflict`（`draft_cards.py:87` 的冲突图标）；
  - `notify_parse_failure` → 窗口通知条（`infoBarVisible/Title/Text`）；
  - 宿主只持窗口的**弱**引用（窗口回调里存着宿主的 `Rc`，强引用会绕成环）。
- **`DraftUi::build`**：一次装齐「状态机 + 宿主 + 回灌定时器 + 窗口回调」。
  分类规则来自 `engine.category()`，`taskDraftFolderText` 缺省取
  `cfg.downloadFolder`，`preBlockNum` 取 `cfg.preBlockNum`。回灌定时器
  `slint::Timer` 重复 `DRAIN_INTERVAL_MS = 100` 毫秒跑 `pump`（Python 是
  `coroutineRunner` 直接回调，Slint 只能在 UI 线程的定时器里收）。
- **[`seed_urls`]**：`main_window.addUrls(urls)`——开框 + 立刻解析（不走 1 秒防抖）；
  **[`pump`]**：`drain_into` + `task_draft_ui::handle`（没有效果就不刷新，免得重灌
  模型让输入框闪）。
- **`boot` 接线**：`ParseWorker::spawn(CreateDefaults::from_config(cfg))` 成功就
  `DraftUi::build(...)`（失败只打一行，窗口照常）；有头分支在 `window.show()` 之后
  调 `open_with_urls(&cli.urls)`（= `Ghost-Downloader-3.py:191` 的
  `openUriRequested → addUrls`）。无头分支**不**灌 URL：那是冒烟模式，
  不该在单测里发真探测。

| 差异 | 说明 |
|---|---|
| 原生对话框那五个钩子 | `pick_folder` / `edit_task` / `batch_url` / `import_files` / `category_menu` 在 Slint 侧要么没有原生文件对话框、要么对应组件还没接进窗口，统一记一行 stderr 返回；对话框本身照常可用（下载路径能直接改） |
| 通知条不自动消失 | Python 的 `InfoBar` 有计时器；这里只置位 `infoBarVisible`，由通知条自己的关闭按钮收起 |
| 定时器而不是回调 | Python 探测完直接回调，这里 UI 线程按 100ms 收一次；粘 URL 后最多多等一眨眼 |
| `folder_history` 传空 | Qt 的 `ConfigItem.history`（目录下拉历史）还没建模 |
| `file_types` 为空 | `FeatureService` 还没有 `fileTypes()`，所以「导入文件」按钮不显示 |

**还没接**：「新建任务」导航项（`ROUTE_ADD_TASK`）没有触发点——`boot` 还没装
`lib.rs::install_shell_actions`（导航 / 搜索 / 通知条 / 消息框 / 拖放那一套），
所以目前只有 CLI URL / 单实例交接能把对话框叫出来。

验收（第 139 步实跑）：

- 单测 `draft_host` **6 条**（桩探测器 + 无头窗口）：确认入库且重复不再加一条、
  `submit_parse` 走后台并把 `work_id` 原样交回、`has_conflict` 认得出已存在的同名文件、
  `seed_urls` 开框 + 立刻提交、`pump` 把后台结果灌成草稿行（`rows[0].name` 对得上、
  `parsing=false`）、解析失败落到通知条三个属性。
- **真二进制 + `Xvfb :99`**：`--data-dir /tmp/...` + 位置参数
  `http://127.0.0.1:18777/f.bin`，18 秒后仍活着（`timeout` rc=124 = 窗口正常、
  无 panic），本地服务器日志里有**一条** `GET /f.bin`（= 后台探测真的发出去了，
  且**没有**自动下载——对话框在等确认）。
- 质量门：`fmt --check` OK；`clippy --workspace --all-targets -D warnings` **exit 0**；
  `cargo test -p gd3-core -p gd3-net -p gd3-servers -p gd3-ui` **GATE_EXIT=0**、
  31 个 target 全 ok、**1814 passed**（第 138 步 1808，+6）；Python 全量
  → 3 failed / 1370 passed / 8 skipped / 35 errors，与基线**逐条相同**（`comm -3` 0 行）。

### 尚未接的

剩下的都是 P2–P6 的 UI：外壳动作集（导航 / 搜索 / 通知条 / 消息框 / 拖放的
`install_shell_actions`，含「新建任务」入口）、草稿对话框里还没接的五个子对话框、
OOBE / 组件库 / 其它对话框。
`shouldPreserveLastModified`（第 133 步）、限速闸门（第 134 步）、`update_service` 的执行层
（第 135 步）、托盘（第 136 步）、建任务链路（第 137 步）、后台解析（第 138 步）都已落地。
窗口分支（winit）
第 132 步已在 `Xvfb :99` 上验过：起窗、下载、三种关窗模式都对。
