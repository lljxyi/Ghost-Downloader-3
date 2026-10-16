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
