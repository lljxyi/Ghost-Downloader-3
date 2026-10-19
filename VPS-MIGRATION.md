# Ghost-Downloader-3 Rust 重构 —— VPS 工作环境

迁移日期：2026-10-16 ｜ 主机：`3.135.205.209`（AWS，Debian 13 trixie，x86_64）

## 结论：可以迁移

判断依据（都是实测过的，不是推断）：

| 检查项 | 结果 |
|---|---|
| Rust 工具链能否编译本项目 | ✅ `cargo build -p gd3-py` 通过（含 BoringSSL），冷编译 3m31s |
| Python 3.13 + PySide6 能否无头运行 | ✅ PySide6 6.10.3，`QT_QPA_PLATFORM=offscreen` 下全量套件跑通 |
| 双模式回归是否成立 | ✅ 开关全开/全关的失败集**完全一致** |
| PI Agent 有没有 Linux 版 | ✅ 官方发布 `pi-host-0.20.1-linux-x64.tar.gz`（headless host） |
| 默认/备用模型 API 是否可达 | ✅ 两个都返回 200，模型 id 都在各自目录里 |

## 装了什么

| 组件 | 版本 | 位置 |
|---|---|---|
| PI Agent Host（`pi-host`） | 0.20.1 | `~/.pi-desktop/pi-host/current`（自带 Node 与 host-core） |
| Rust（含 clippy/rustfmt） | 1.99.1 | `~/.cargo` |
| uv | 0.13.1 | `~/.local/bin` |
| git / gcc / cmake / clang / libclang | 2.47.3 / 14.2.0 / 3.31.6 / 19.1.7 | 系统 apt |
| Python（项目 venv） | 3.13.5 | `~/Ghost-Downloader-3/.venv` |
| swap | 4 GB | `/swapfile`（1GB 内存编 BoringSSL 会 OOM） |
| patchelf | 0.18.0 | maturin 装扩展要用 |

仓库在 `~/Ghost-Downloader-3`（含 `.git`，工作树已归一化成 LF——Windows 上带的 CRLF 会让 git 把每个文件都看成改过）。

## PI Agent

### 服务

已注册成 systemd 服务，开机自启、崩溃重拉：

```bash
systemctl status pi-host            # 状态
systemctl restart pi-host           # 重启（会打印新配对令牌）
journalctl -u pi-host -n 30 --no-pager | grep -E 'PI_HOST_(READY|PAIRING_TOKEN)'
```

- `PI_HOST_READY` 给 `hostId` / `host` / `port`（只绑 `127.0.0.1`，不对外暴露）
- `PI_HOST_PAIRING_TOKEN` 是一次性令牌，**10 分钟有效**，过期就重启换一个

在 PI-Desktop 里把 `root@3.135.205.209` 配成 host，用上面的令牌配对即可。

### 已导入的 provider

用官方 `pi-host provider-import`（往 host 的 admin socket 发
`{"op":"providers.import","payload":{...}}`）导入，**默认已指向主 provider 的 `claude-opus-5`**：

| 用途 | 名称 | Base URL | 模型 | providerId |
|---|---|---|---|---|
| 默认 | ai.lfree.org | `https://ai.lfree.org/bot/wRS0PYHzfthj/v1` | `claude-opus-5` | `1dd54b5b-280e-479d-9bd1-6845c22d710f` |
| 备用 | wb.ein.us.ci | `https://wb.ein.us.ci/v1` | `global:deepseek-v4.1-flash` | `6088a636-1e62-4296-b040-5969364a703b` |

密钥不在文件里，落在 host 的 `~/.pi-desktop/secrets/`（按机器密钥加密）。
默认值存在 `pi.sqlite` 的 `kv`（`ns=app`）：`defaultProviderId` / `defaultModelId`。

换 key 或加 provider 时：把载荷写成 JSON（字段必须落在 `INPUT_FIELDS` 白名单里，
多一个字段就 `INVALID_REQUEST`），然后

```bash
pi-host provider-import < payload.json     # 用完记得删 payload.json（里面有明文 key）
```

载荷结构：

```json
{
  "version": 1,
  "providers": [
    {"sourceId": "任意唯一串",
     "input": {"name": "...", "vendorKey": "custom", "type": "openai_compatible",
               "protocol": "openai_compatible", "baseUrl": "https://.../v1",
               "authKind": "api_key", "apiStyle": "chat_completions",
               "models": [{"id": "...", "contextWindow": 200000, "maxTokens": 32000,
                           "thinkingLevels": ["off","low","medium","high","xhigh","max"],
                           "defaultThinkingLevel": "high"}],
               "defaultModelId": "...", "secretValue": "sk-..."}}
  ],
  "defaultModel": {"sourceId": "同上", "modelId": "..."}
}
```

## 日常命令

```bash
cd ~/Ghost-Downloader-3

# Rust
cd rust && cargo test -p gd3-core -p gd3-net -p gd3-servers   # 95 个测试
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check

# Rust 扩展装进 venv（改了 rust/ 之后要重跑）
VIRTUAL_ENV=$PWD/.venv uvx maturin develop --uv --manifest-path rust/crates/gd3-py/Cargo.toml

# Python 全量套件
QT_QPA_PLATFORM=offscreen uv run pytest -q --continue-on-collection-errors

# 双模式（五个影子开关）
QT_QPA_PLATFORM=offscreen \
  GD3_RUST_PERSIST=1 GD3_RUST_HTTP=1 GD3_RUST_ARIA2=1 GD3_RUST_BROWSER=1 GD3_RUST_FILENAME=1 \
  uv run pytest -q --continue-on-collection-errors
```

编译 Rust 时的环境变量（`btls-sys` 要 libclang）：

```bash
export LIBCLANG_PATH=/usr/lib/llvm-19/lib
export PYO3_PYTHON=$HOME/Ghost-Downloader-3/.venv/bin/python
export CARGO_BUILD_JOBS=2      # 2 vCPU / 1GB 内存，别开满
```

## 基线（Linux，2026-10-16）

| 模式 | 结果 |
|---|---|
| 开关全关（纯 Python） | **3 failed, 758 passed, 8 skipped, 35 errors** |
| 五开关全开（走 Rust） | **3 failed, 754 passed, 11 skipped, 35 errors** |

失败集**逐条一致**；Rust 模式多出的 3 个 skip 是 `tests/test_http_subworker.py` 里打桩
Python 实现的白盒测试（ENOSPC、停滞恢复、死连接重试），它们测的是 Python 实现本身。

三个失败都与本次迁移无关：

1. `test_android_config_parity.py::test_android_config_covers_every_desktop_setting_except_desktop_only`
   —— `android/` 是独立私有仓库，本机没有（Windows 上同样失败）。
2. `test_github_probe.py::testProxySiteGetsNoCredentials` —— 既有红灯（Windows 上也红）。
3. `test_runtime_install.py::test_delete_raises_when_a_path_cannot_be_deleted`
   —— 这条在 Windows 上是 skip（POSIX 专属）；在 Linux 上会跑，但**以 root 运行时
   `chmod` 拦不住删除**，所以失败。用非 root 用户跑就能过。

35 个 error 全是 `test_android_*` 收集失败，与 Windows 一致。

## 与 Windows 的差异（迁移时踩到的）

1. **`tasks.jsonl` 的行尾随平台**：Python 文本模式写盘，Windows 是 CRLF、Linux 是 LF。
   Rust 侧 `store.rs` 的 `LINE_ENDING` 已经按平台处理；但金文件是 Windows 生成的（CRLF），
   导致 `round_trips_python_tasks_jsonl_byte_for_byte` 在 Linux 上误报。
   已把该测试改成「行尾必须与本平台一致」+「逐行字节与金文件相同」，两边都能过。
2. **工作树行尾**：Windows 工作树是 CRLF、`HEAD` 里是 LF，直接拷到 Linux 会让
   `git status` 把 368 个文件都标成改过。已用 `normalize_eol.py` 归一化成 LF，
   并重建 `.git/index`（Windows 带来的 index 统计缓存失效）。
3. **`::1` 与 POSIX 权限**：Windows 上被策略拒绝/被跳过的那几条测试，在 Linux 上会真的跑。

## 迁移后本机还需要什么

后续工作都在本机之外跑，所以本机只剩「保留备份」的用途：

- `~/Ghost-Downloader-3` 源码只有 **19 MB**（不含 `rust/target` 7.5 GB、`.venv` 884 MB、`.git` 74 MB）
- Rust 工具链（`~/.cargo` 490 MB + `~/.rustup` 627 MB）与 `~/.local/tools`（cmake+nasm 139 MB）
  只有在**本机**还要编 Rust 时才有用

## Phase 5 进展（2026-10-16）

### 已完成：`gd3-net::probe` 的解析层

`features/http_pack/pack.py::probe` 拆成两半，先落地了**头解析**这一半
（`rust/crates/gd3-net/src/probe.rs`）：

- `content_disposition_filename` —— 镜像 Python 的 `Message.get_params` +
  `collapse_rfc2231_value`；
- `range_total` / `body_length` —— `content-range` 与 `content-length` 的取值；
- `url_path` / `query_param` / `extract_filename_param` —— URL 那几级文件名来源。

规格由 **40 条金文件**固定：`tests/fixtures/generate_cd_fixture.py` 用 Python 生成
`content_disposition.jsonl`，Rust 侧逐条对比。

### 两个查证出来的结论（影响后续设计）

1. **`mimetypes.guess_extension` 是平台相关的，不能直接 port。**
   Linux 上它读 `/etc/mime.types`（本机 1608 条），Windows 上读注册表；
   同一个 content-type 可能给出不同扩展名。所以 Rust 侧把扩展名解析做成**注入的回调**，
   表留在 Python。
2. **`http_pack.probe` 原本没有任何直接测试。** 全仓库只有 `test_github_probe.py`
   引用了 `http_pack.pack`，且只用 `RemoteFile` 这个 dataclass。也就是说这段网络逻辑
   此前只有间接覆盖——所以 Rust 侧的测试是**新写的规格**，金文件是唯一的对照物。

### content-disposition 的几个不直觉行为（实测确认，别按直觉改）

- `filename*`（RFC 2231）会**覆盖**同名的 `filename`，与出现顺序无关；
- 普通值会被 `unquote` **两次**（一次在 `collapse_rfc2231_value` 里，一次在调用处）；
- `\"` 还原成 `"`，但**其他反斜杠原样保留**（`"..\..\evil.mp4"` 不变）；
- 参数名大小写不敏感，同名的取**最后一个**；
- charset 不认识时退回「百分号解码后的原样文本」。

### 下一步

probe 的**网络层**（`bytes=1-1` 主探 + `bytes=0-0` 兜底 + 416 分支）需要给
`tests/support/test_server.rs` 加一个「自定义响应头」模式，然后写集成测试；
之后是 PyO3 桥 + `GD3_RUST_PROBE` 影子开关 + 双模式回归。


---

## 运营提示：给自动化 agent 的补充指令（每轮都要看）

这一节是运维方（人类）随时会追加指令的地方，**每轮开工都读一遍**。

### 1. 先固化基线，别再追偶发失败

开工第一件事：跑一次全量测试，把失败集合存下来，之后只做对比：

```bash
cd /root/Ghost-Downloader-3
.venv/bin/python -m pytest -q 2>&1 | grep -E '^(FAILED|ERROR)' | sort > /root/baseline-failures.txt
wc -l /root/baseline-failures.txt
```

**已知偶发（不是回归，别去修，更不要反复重跑）**：

- `tests/test_engine_listeners.py::test_stopping_the_engine_releases_both_ports[asyncio]`
- `tests/test_engine_listeners.py::test_stopping_the_engine_releases_both_ports[uvloop]`

这两项在本机会随机失败（端口释放时序 + gc 资源清理），同一份代码重跑会时过时不过。
**判定标准：只要失败项都在 `/root/baseline-failures.txt` 里，就算通过。**
不要为了确认它而把同一条命令跑十几遍 —— 那是纯粹的浪费。

跑测试时**别只 `tail -1`**：摘要行会被 `ResourceWarning` 之类的东西挤掉，
要留失败明细就用 `2>&1 | grep -E '^(FAILED|ERROR)|passed|failed'`。

### 2. 改文件：超过 80 行禁止用 Edit（硬规则，不是建议）

**对超过 80 行的文件，禁止使用 Edit 工具。** 只能二选一：

- **A. 整块重写**（Write 整个文件，最稳）；
- **B. 精确字符串替换脚本**（带唯一性断言，见下方模板）。

小文件（≤80 行）可以用 Edit，但**每改一处必须立刻重读文件**再算下一处行号。

#### 为什么是硬规则

截至第 23 步，Edit 锚点失效已经造成 **11 个回合整回合中止**
（错误码 `MUTATION_RETRY_BUDGET_EXHAUSTED`），波及
`hls.rs`（2 次）、`ed2k.rs`、`github.rs`、`ftp.rs`、`hf.rs`、
`gd3-py/src/lib.rs`、`gd3-net/src/lib.rs`、`AGENT-PROGRESS.md`。

每中止一次 = 一个回合白费 + 上下文重建 + 可能触发一次模型切换。
这是本项目目前**唯一持续的效率损耗**，所以不再作为建议。

#### 替换脚本模板（复制即用）

```bash
python3 - <<'PY'
from pathlib import Path

p = Path("rust/crates/gd3-net/src/foo.rs")
text = p.read_text(encoding="utf-8")

edits = [
    ("旧片段1", "新片段1"),
    ("旧片段2", "新片段2"),
]

for i, (old, new) in enumerate(edits, 1):
    n = text.count(old)
    assert n == 1, f"第 {i} 处锚点匹配 {n} 次，中止"
    text = text.replace(old, new)

p.write_text(text, encoding="utf-8")
print("OK")
PY
```

要点：

1. **一个脚本改多处**，减少工具调用次数；
2. `assert n == 1` 是关键——锚点不唯一就立刻停下，而不是猜；
3. 改完马上跑 `cargo fmt && cargo test`，不要攒着；
4. 新建文件用 Write，不要用 Edit 从空文件开始。

### 3. 进度笔记是硬要求

每完成一个可验证的步骤，**立刻**追加到 `AGENT-PROGRESS.md`（不是攒到最后）：

```markdown
## 第 N 步 — <一句话>
- 改动：<文件 + 做了什么>
- 验证：<命令 + 结果数字>
- 下一步：<下一件事>
- 卡点：<没有就写「无」>
```

没有进度笔记，运维方就无法判断你在推进还是在原地打转。

### 4. 小步提交

每完成一个小步骤（哪怕只是一个文件接通）就 `git commit`。
回合被中止时未提交的工作很容易在下一轮被覆盖或遗忘。**仍然不要 `git push`。**

### 5. 里程碑切细（重要）

不要把一个里程碑当成一整块做。**每完成一小步就验证 + 提交**，这样即使回合被中止，
已完成的成果也不会丢。以 milestone 1 为例，应当拆成：

| 小步 | 完成标准 | 验证方式 |
|---|---|---|
| 1a | `gd3-py` 能导出 probe，Python 侧 `import` 成功 | `.venv/bin/python -c "from ghost_downloader_rust import probe"` 不报错 |
| 1b | `GD3_RUST_PROBE=1` 时 `pack.py` 走 Rust 分支 | 开/关开关各跑一次 http_pack 相关测试 |
| 1c | 双模式回归：失败集合与基线完全一致 | 对比 `/root/baseline-failures.txt` |

**每完成一个小步就 `git commit`**，commit message 写清「做了什么 + 验证命令 + 结果数字」。

### 6. 提交粒度（硬要求）

**每接通一个文件就提交一次**，不要攒到里程碑结束。

- 新建文件后 → 提交
- 一个文件改完且质量门通过 → 提交
- 回合结束前 → 确认没有「改了一半但没提交」的文件

理由：回合可能因 Edit 失败、连接中断、超时被中止；未提交的改动在下一轮很容易
被覆盖或遗忘。**仍然不要 `git push`。**

### 7. 回合内自检清单

每轮结束前确认：

1. `git status` 里没有「改了一半」的文件（要么提交，要么还原）
2. `AGENT-PROGRESS.md` 已追加本轮的进展
3. 仓库处于可编译、测试全绿状态

### 8. 反兔子洞（硬规则）

自动化 agent 最容易死在「一个测试挂住 -> 反复调试 3 小时」。实测发生过一次：
服务端已响应并关闭、客户端却挂住，追了 3 小时没提交任何东西。所以：

1. **同一个问题最多投入 20 分钟。** 超过就停下来，按下面处理：
   - 在 `AGENT-PROGRESS.md` 里记下现象、已排除的假设、你的怀疑方向；
   - 把该测试标记为 `#[ignore]`（Rust）或 `@pytest.mark.skip`（Python），
     注释里写清「挂起，待查」和现场链接；
   - **继续推进别的工作**，不要原地打转。
2. **测试挂住优先怀疑测试脚手架，而不是产品代码。** 本项目里测试服务器是手写的
   HTTP/1.1 实现，容易在「带未读 body 关连接」这类地方产生 RST/挂起。
3. **不要用通配符跑测试二进制**（`cargo test --test 'size-*'` 会匹配到旧产物），
   一律给精确路径。
4. **每 20 分钟必须有一次提交或一次进度笔记**。没有产出的时间就是浪费。
5. 判断「是不是我引入的」：先把改动 `git stash`，跑同一个测试。原始代码也挂 -> 不是你的问题，
   记下来跳过。

### 9. 会话与回合的边界

回合可能因为超时或连接断开被外部打断。被打断后**不要试图恢复上一轮的推理状态**，
直接看 `git status` 与 `AGENT-PROGRESS.md`，把「已经写好但没提交的东西」先跑质量门
然后提交，再决定下一步。半成品停留在工作区是最糟的状态。

### 10. 时间戳必须用 `date` 取，不要猜（重要）

你的上下文里**没有可靠的当前时间**。实测因此把笔记日期写成了 `2026-11-02` / `2026-11-03`，
而真实日期是 `2026-10-17` —— 差了半个月。

**写任何时间戳之前，先跑一次：**

```bash
date '+%Y-%m-%d %H:%M'
```

适用范围：
- `AGENT-PROGRESS.md` 的步骤标题
- commit message 里提到的时间
- 任何「距今多久 / 是否超时」的判断

不要凭推理编日期，也不要复用上一轮笔记里的日期。宁可只写「第 N 步」不写日期，
也不要写错日期——错的时间戳会让运维方误判时序。

### 11. 跑测试一律带 `timeout`（硬规则）

实测发生过两次测试进程永久挂死，其中一次挂了 **11.8 小时**、另一次 82 分钟，
直接把整个回合拖住。所以：

```bash
# 全量 Python（15 分钟上限）
timeout 900 .venv/bin/python -m pytest -q --continue-on-collection-errors

# Rust（20 分钟上限；首次编译慢，给足）
timeout 1200 cargo test -p gd3-core -p gd3-net -p gd3-servers
```

- **不要不带 `timeout` 就跑全量测试。**
- 超时（退出码 124）不是"结果未知"，而是**明确的挂起信号**：立刻按第 8 节
  「反兔子洞」处理——记录现象、标 `#[ignore]`/`skip`、换活干。
- 别用 `cargo test --test 'xxx-*'` 这类通配符：会匹配进历史遗留的旧二进制，
  那些旧二进制可能还卡着死锁（实测有 11.8 小时的僵尸）。一律给精确路径。
- 如果怀疑有僵尸进程占着资源，先看：
  ```bash
  ps -eo pid,etimes,cmd --sort=-etimes | awk '$2 > 1800' | grep -E 'cargo|pytest|deps/'
  ```
  确认是自己这轮起的再杀，不要动别人的。

### 12. Phase 8 目标：把 Slint 接成可运行应用（当前主线）

Phase 1–7 已完成。Phase 7 产出的是**纯库**：没有 `main`、没有事件循环、没接
`TaskService`、没读写 `UserConfig.json`，`Ghost-Downloader-3.py` 入口仍走 Qt。
Phase 8 就是把这些接上。按顺序推进，每步可独立验证：

| 子阶段 | 目标 | 验收方式 |
|---|---|---|
| **8a 可运行骨架** | `gd3-ui` 有 `main`：起窗口、事件循环、干净退出 | `cargo run -p gd3-ui` 能起窗口；无头环境用 `--headless` 冒烟 |
| **8b 任务数据接线** | 接 `gd3-core` 的 TaskService，列表显示 `tasks.jsonl` 里的真实任务 | 造几条任务记录 -> 启动 -> 列表内容正确 |
| **8c cfg 读写接管** | `UserConfig.json` 由 Rust 双向读写 | 读->改->写回逐字节一致（时间戳类除外）；Python 侧能读回 |
| **8d 页面接线** | 设置页回写、对话框开合、任务抽屉动作、草稿创建接真实数据 | 每个动作有断言：改前/改后 cfg 或 tasks 的差异 |
| **8e 下载引擎接线** | 把 `gd3-net` 接上，任务能真实下载 | 起本地 HTTP 服务 -> 加任务 -> 下载完成 -> 文件逐字节校验 |
| **8f 平台外壳** | 托盘 / Dock | Slint 1.18 无原生能力。**允许降级**：留 TODO + 记录平台 API 方案，不要硬造 |
| **8g 端到端验收** | 完整链路跑通 | 见「完工判定」 |

**已自行推进的部分**（第 79–124 步）：`settings_writer` / `settings_bridge` /
`settings_dialogs` / `extension_crx` / `extension_actions` / `extension_install` /
`update_service` / `gd3-net::sources` / `task_draft_ui`。

#### 与 Qt 的共存（硬要求）

**不要删 Qt 入口。** 用环境变量切换：`GHOST_UI=slint` 走 Rust/Slint，
`GHOST_UI=qt` 走原 Qt（默认，保持可回退）。

`cfg` 回写必须遵守**单一写者**：同一时刻只能有一侧写 `UserConfig.json`，
否则整文件回写会互相覆盖。接线时在文档里写清「谁是当前写者」。

#### 完工判定（Phase 8）

1. `cargo run -p gd3-ui` 能起窗口并干净退出；`--headless` 冒烟通过
2. 用 `UserConfig.json` 启动，读->改->写回逐字节一致（时间戳类字段除外）
3. 起本地 HTTP 服务 -> 添加任务 -> 真实下载完成 -> 文件逐字节校验通过
4. 任务状态写回 `tasks.jsonl`，**Python 侧能读**
5. 无头冒烟覆盖：启动 / 加任务 / 进度更新 / 暂停恢复 / 完成
6. `GHOST_UI=qt` 仍可用（可回退）
7. 质量门全绿

满足后：更新 `rust/README.md` -> 覆盖 `AGENT-DONE`（写清 Phase 8 做完什么、
还剩什么、已知差异与风险）-> 停止推进等人工验收。
