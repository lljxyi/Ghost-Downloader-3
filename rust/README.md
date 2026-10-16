# Ghost Downloader Rust 内核

Python 实现的 Rust 迁移，按阶段推进。

**已完成**：Phase 1（领域模型 + 持久化）、Phase 2（PyO3 影子模式）、Phase 3（HTTP 传输引擎 + 接入 pack + wreq 指纹）、Phase 4（回环监听 + aria2 JSON-RPC + 浏览器扩展 WebSocket v2，均已接入 Python）。

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

## 下一步的顺序（已定）

**Phase 5：pack 的 parse 层**（URL → 任务）。挑它的原因是依赖最干净：pack 只**读** `cfg`
（clientProfile、代理、并发数），这些值从 Python 侧当参数传进来就行。
先从 `http_pack` 开始（它的传输引擎已经在 Rust 里），再按「纯 HTTP + JSON」的顺序推
github / huggingface / bili，最后才是 m3u8 / ftp / ed2k / bittorrent。

**配置（`UserConfig.json`）排在 UI 阶段一起搬**，不提前。`app/config/cfg.py` 是
`qfluentwidgets.QConfig` 的子类：每一项都是 Qt 的 `ConfigItem`，设置页直接绑它的
`valueChanged`，而且**每次 `cfg.set()` 都会整文件回写**。在 UI 还是 Qt 的期间，
Rust 再写同一个文件只会互相覆盖——所以它是 UI 的伴生物，不是独立的一步。
同理，凡是「持久化进 cfg」的服务（`category_service` 的 `categoryRules` 等）都要排在它后面。
