# Rust 重构说明

本文说明 Ghost-Downloader-3 的 Rust 重写：为什么要做、怎么做的、做到哪了、还剩什么。

分支：`beta`（由 `rust-rewrite` 合并而来）

---

## 1. 摘要

用 **Rust 重写了整个下载内核与 UI**，但**没有推倒重来**：采用绞杀者（strangler）模式，
Python 与 Rust 两套实现长期并存，用 26 个环境变量影子开关逐块切换，每一块都有
「两侧产物逐字节/逐断言一致」的验证。默认全部关闭，随时可以整体退回纯 Python。

| 指标 | 数值 |
|---|---|
| 提交数 | 259（`119e770` 之后） |
| 记录步骤 | 139 步 |
| Rust 代码 | 约 96,900 行 / 5 个 crate |
| Slint UI 代码 | 约 3,700 行 |
| Rust 测试 | 1,828 个，全绿 |
| Python 测试 | 1,370 passed，失败集合与重构前**逐条一致** |
| 影子开关 | 26 个，全部默认关 |

**核心承诺：磁盘格式与线上协议一个字节都没改。** `tasks.jsonl`、`.ghd`、
`UserConfig.json`、aria2 JSON-RPC、浏览器扩展 WebSocket v2 —— 两侧可互相读写，
任何时刻都能切回纯 Python 而不丢数据。

---

## 2. 为什么用绞杀者而不是重写

原项目约 34,000 行 Python（PySide6 + asyncio）。直接重写有两个不可接受的风险：

1. **行为漂移**：下载器的大量逻辑是「与某个具体站点的具体怪癖对齐」的产物，
   重写时凭理解重写几乎必然丢细节；
2. **不可回退**：一旦新实现出问题，没有可用的旧版本兜底。

所以每一块迁移都遵守同一条纪律：

- **Python 是规格**。Rust 实现前先把 Python 的行为实证清楚（读源码、写探针、生成金文件），
  实证结论写进模块头注释；
- **金文件由 Python 生成，Rust 只读**。`tests/fixtures/generate_*.py` 产出，
  Rust 测试逐条比对，不许手改；
- **双模式回归**：开关关 / 开关开各跑一遍 Python 全量测试，
  失败集合必须与基线**完全相同**（不能多也不能少）；
- **一个开关一个接入点**。开关打开只改一处分支，方便定位问题。

---

## 3. 架构

```
rust/
├── crates/
│   ├── gd3-core      领域模型、tasks.jsonl、.ghd、容器解析、各站点纯变换层
│   ├── gd3-net       HTTP 分片传输引擎（wreq/BoringSSL，TLS 指纹与原版一致）
│   ├── gd3-servers   回环端口监听、HTTP/1.1 头解析、aria2 JSON-RPC、扩展 WS v2
│   ├── gd3-py        PyO3 桥（cdylib，maturin 构建，供 Python 侧调用）
│   └── gd3-ui        Slint UI（语义镜像层 + 可运行应用）
└── Cargo.toml        workspace
```

### 为什么用 wreq 而不是 reqwest

唯一会让真实站点行为退化的缺口是 **TLS 指纹**。原版用 curl_cffi 模拟浏览器指纹，
换成 reqwest 会立刻在某些站点上被区别对待。`wreq` 基于 BoringSSL，能复刻指纹。

实测对齐结果（`https://tls.peet.ws/api/all`，profile=chrome）：

- `ja4 = t13d1516h2_8daaf6152771_d8a2da3f94cd` —— 与 Python 一致
- HTTP/2 Akamai 指纹 —— 一致
- User-Agent —— 一致

**ja3 无法比对**：wreq 会打乱 TLS 扩展顺序，所以 ja3 每次不同。验证只能看 ja4。

代价：`wreq` 是预发布版本（6.0.0-rc.31），升级后必须重新核对指纹。

---

## 4. 影子开关

26 个 `GD3_RUST_*` 环境变量，**全部默认关闭**。

| 领域 | 开关 |
|---|---|
| 存储 / 网络 / 监听 | `PERSIST` `HTTP` `ARIA2` |
| http_pack | `PROBE` `PROBE_SIZE` `GITHUB` `GITHUB_PROBE` |
| huggingface | `HF_TREE` `HF_URL` `HF_HEAD` `HF_FILES` |
| bili | `BILI_URL` `BILI_PARSE` `BILI_PAGES` `BILI_VIEW` `BILI_SIGN` `BILI_PLAYURL` `BILI_SUBTITLES` |
| 其它 pack | `FTP_URL` `ED2K_LINK` `ED2K_PROGRESS` `M3U8_OUTPUT` `M3U8_PARSE` |
| 其它 | `FILENAME` `BROWSER` |

用法：

```bash
# 单块验证
GD3_RUST_PROBE=1 python -m pytest -q

# 全开
GD3_RUST_PROBE=1 GD3_RUST_HTTP=1 ... python -m pytest -q
```

---

## 5. 进度

| 阶段 | 内容 | 状态 |
|---|---|---|
| 1 | `gd3-core`：任务模型、`tasks.jsonl`、`.ghd`、fMP4/WebM 区间数学 | 完成 |
| 2 | `gd3-py` PyO3 桥 + `rust_store.py` 影子模式 | 完成 |
| 3 | `gd3-net` HTTP 引擎（分片/续传/重试/降级/限速/自动加速）+ wreq 指纹 | 完成 |
| 4 | `gd3-servers`：端口监听、HTTP 头解析、aria2 RPC、扩展 WS v2 | 完成 |
| 5 | `http_pack` 迁移（probe 解析层 + 网络层） | 完成 |
| 6 | 其余 pack 的纯变换层下沉（扫尾评估确认无新下沉项） | 完成 |
| 7 | `gd3-ui` 语义镜像层（设置页、任务卡片、抽屉、对话框、组件库、托盘语义） | 完成 |
| 8 | 把 Slint 接成可运行应用（`main`、事件循环、TaskService、cfg 读写、真实下载） | 完成 |

Phase 8 的验收（全部实测）：

- `cargo run -p gd3-ui` 起窗口并干净退出；`--headless` 冒烟通过
- `UserConfig.json` 读 → 改 → 写回**逐字节一致**（18 组真配置往返验证）
- 本地 HTTP 服务 → 加任务 → 真下载 → **逐字节校验通过**，`.ghd` 正确清理
- 任务状态写回 `tasks.jsonl`，**Python 侧能读回**
- 无头冒烟覆盖：启动 / 加任务 / 进度 / 暂停恢复 / 完成
- `GHOST_UI=qt` 仍可用（Qt 入口一字未动）

---

## 6. Windows 构建

Phase 1–8 全程在 Linux 上开发，**从未做过 Windows 构建**。本次首次构建时暴露了两个
平台问题，已修复：

### 6.1 `single_instance` 只有 Unix 实现

原实现用 `std::os::unix::net::{UnixListener, UnixStream}`，Windows 上编译不过。

补了 Windows 后端，语义等价、不引入新依赖：

- **锁**：`OpenOptions::share_mode(0)` 独占打开锁文件；第二个进程拿到
  `ERROR_SHARING_VIOLATION` 即判定已有实例；
- **通道**：交接载荷写入同目录的 `instance.handoff`，主实例后台线程每 50 ms 轮询；
- **释放**：独占句柄由内核在进程退出（含崩溃）时回收，因此**比 Unix 版少一个陈旧锁问题**。

### 6.2 `has_display()` 是 Unix 语义

原实现只检查 `DISPLAY` / `WAYLAND_DISPLAY`，Windows 上必然为假 →
**GUI 一律退化成无头模式**（进程启动即退出）。改为平台感知：

- Windows：恒为 `true`（桌面会话由 winit 直接接管）；非交互场景用 `--headless` 显式指定
- 其它平台：保持原逻辑

### 6.3 构建前置

```
Rust 1.99.1 (x86_64-pc-windows-msvc)
MSVC 14.44 + Windows SDK 10.0.26100
cmake 3.31.6 + nasm 2.16.03 + libclang   ← BoringSSL(btls-sys) 需要
```

产物 `gd3-ui.exe` 约 26.7 MB，静态链接 Slint 与 BoringSSL。

---

## 7. 待办

### 高优先级（影响可用性）

1. **三个入口还没接解析器**：位置参数 URL、浏览器扩展推送、aria2 JSON-RPC。
   解析器已实现（`gd3-ui::task_create`），但 `Engine::new` 故意不装 ——
   同步探测会卡 UI 线程。需要一条后台 worker 线程 + 通道，`draft_worker` 已有可复用模式。
2. **`gd3-py` 不被 `cargo test` 覆盖**：它是 cdylib，只能用 maturin 构建后由 Python 侧
   测试间接验证。CI 里要显式加这一步。
3. **没有 CI**：目前全靠手工跑质量门。建议加 GitHub Actions：
   fmt / clippy / test / Python 全量 / Windows 构建。

### 中优先级（补齐能力）

4. `tar.xz` / `dmg` 解包（更新器用）：Linux 侧缺 `xz2` 依赖，macOS 的 `dmg` 需 `hdiutil`。
5. 自绘菜单（`AcrylicMenu` 等三种）、菜单项图标（`FluentIcon`）、
   `_refreshMenuItems` 的启停用状态、`messageClicked` 回调。
6. 磁盘满的端到端注入测试（错误码表已逐码对齐，但没有注入测试）。
7. 像素级/交互级回归：当前判定标准是**语义镜像**（配置键名、默认值、取值范围、
   格式化输出一致），布局常量按源码抄并有断言，但视觉差异未逐像素比对。

### 低优先级（打磨）

8. `wreq` 升级到正式版后重新核对 TLS 指纹。
9. `wreq-util` 缺 Edge / Android 身份，这类 profile 会退回 Chrome 最新版。
10. 稀疏文件：Python 预分配前调 `FSCTL_SET_SPARSE`，Rust 只 `set_len`
    （NTFS 上功能等价，但语义不完全相同）。

---

## 8. 已知差异与风险

### 语义层偏差（每条都在模块头或测试旁标注）

- **无译文层**：`toLocalizedError` / `QCoreApplication.translate` 的文本按中文原文硬编码
  或由调用方当入参传入。切换语言时 Rust 侧不会自动跟随。
- **Python 抛异常处退化为缺省值**：`options.index()` 缺失 → 兜底 0；`size_hint` 高度为 0
  → `(16,16)`；非法 IPv6 括号 → `is_subscription_url` 返回 `false`；
  Markdown 缺片段标记 → `None`（Python 抛 `IndexError`）。
- **Qt 专有行为不模拟**：`QComboBox.currentText()` 用字符串承载、`nextCheckState`
  的空实现用返回值表达、`QWidget` 布局缓存用显式 `i32`、`QColor` 透明度用 `u8`。
- **`icon_or_tag` 用白名单**（`category_settings`）：`ICON_CHOICES ∪ {"TAG"}`，
  是唯一已知的枚举收敛偏差。

### 技术风险

- **`wreq` 是预发布版本**：升级后必须重新核对指纹。
- **UI 单测依赖「Qt 实测值当入参」的约定**：窗口几何、字体测宽、暗色标志都由调用方传入。
  真正接线时若传错（例如把逻辑像素当物理像素），单测不会报警 —— 这是后续维护的主要坑。
- **磁盘满**：错误码表已对齐，但无端到端注入测试。

---

## 9. 怎么验证

```bash
# Rust
cd rust
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p gd3-core -p gd3-net -p gd3-servers -p gd3-ui

# Python（开关全关，应与基线一致）
python -m pytest -q --continue-on-collection-errors

# 单块双模式对比
GD3_RUST_PROBE=1 python -m pytest -q
```

### 基线红灯（不是回归，本机环境导致）

- `test_aria2_rpc.py::TestTransport::test_getGlobalOption_over_http`（路径分隔符）
- `test_port_listener.py::test_listens_on_ipv4_and_ipv6_loopback`（本机 `::1` 被拒）
- `test_runtime_install.py` 3 项
- `test_github_probe.py::testProxySiteGetsNoCredentials`
- android 相关约 34 项 error（Linux 上本来就不跑）

### 回滚

所有影子开关默认关，**不设任何环境变量就是纯 Python 行为**。UI 侧用
`GHOST_UI=qt` 切回原 Qt 界面。磁盘格式从未改变，不存在数据迁移问题。

---

## 10. 许可

GPL-3.0，与上游一致。
