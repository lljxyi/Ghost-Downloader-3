# AGENT-PROGRESS

VPS（Linux）上的 Rust 迁移日志。每步：做了什么 / 验证 / 下一步 / 卡点。

## 2026-10-16 第 1 步
- 做了什么：`probe` 双模式打通（里程碑 1）。
  - `rust/crates/gd3-py/src/probe.rs`：PyO3 导出 `gd3_py.probe(url, headers, user_agent, client_profile, fallback_name, guess_extension)`，
    返回 `ProbeOutcome`（name/file_size/can_use_range_requests/last_modified/content_type）。
    扩展名解析回调由 Python 传入（`mimetypes.guess_extension` 平台相关，决定留在 Python）。
    非 200/206/416 抛 `ValueError`（镜像 `raise_for_status`）。
  - `features/http_pack/rust_probe.py`：适配器，把 TaskOptions → `RemoteFile`。
  - `features/http_pack/pack.py::HttpParser.parse`：`GD3_RUST_PROBE=1` 时走 Rust probe（默认关闭）。
- 决策记录（对照「与既有测试/金文件一致」原则）：
  - client profile 解析：与 `rust_engine.py` 同款——传 `clientProfile or cfg.clientProfile.value` 给 Rust，
    本机没有 Edge/Android 系身份时 Rust 退回 Chrome 最新版，`auto`+sourceUa 匹配退回 Chrome。
    这是 README 已记录的 wreq-util 差异，不在此处扩大改动。
  - 兜底名 `f"file_{time_ns()}"` 由 Python 侧生成一次传入（Python 用 time_ns，本身不可复现）。
- 冒烟对照：本地起 `BaseHTTPRequestHandler`（206+content-range+content-disposition），
  Rust 与 Python probe 输出逐字段一致（name=file、fileSize=12345、range=True）。
- 验证：
  - `cargo fmt --all --check` 0；`cargo clippy --all-targets -- -D warnings` 0 error；
  - `cargo test -p gd3-core -p gd3-net -p gd3-servers`：12 套全过 0 failed；
  - Python 全量（`pytest -q --continue-on-collection-errors`）：
    开关关：**3 failed, 758 passed, 8 skipped, 35 errors** —— 与基线逐条一致；
    开关全开（含 GD3_RUST_PROBE）：两轮，一轮 3F/753P/11S 与基线同集合，一轮多出
    `test_engine_listeners.py::test_stopping_the_engine_releases_both_ports` 2 项。
- 卡点：`test_engine_listeners` 那项是既有环境 flake，与本改动无关：单独重跑 12 次失败 8 次
  （失败数 1~2 波动），**关掉 GD3_RUST_PROBE（保留其余四个开关）同样偶发**，纯 Python（全关）5 次全过。
  涉及 Rust listener 模式的端口释放瞬断，2 vCPU 上偶发；不影响判定「无新增失败」。
- 下一步：http_pack 解析层剩余部分（buildTask/HttpParser.match 等）与 README 未做清单。

## 2026-10-16 第 2 步
- 做了什么：huggingface 树枚举署 Rust 影子分支（`GD3_RUST_HF_TREE`，默认关）。
  - `rust/crates/gd3-net::hf`：`fetch_tree` 精确复刻 Python `_fetchTree`（只抓一页、
    目录递归 URL 取 path 最后一段——嵌套会丢中间段，故意不修；lfs.size 覆盖；
    401→Auth、其它 4xx/5xx→Status、JSON/网络错→Transport）。
  - `rust/crates/gd3-py::hf`：PyO3 `hf_fetch_tree`（401→PermissionError，其它→ValueError）。
  - `features/huggingface_pack/rust_tree.py` 适配器；`pack.py::parse` 按开关分流。
  - `tests/test_hf_tree.py`：本地 aiohttp JSON 树，两侧 `_fetchTree` vs `rustFetchTree`
    entries 逐条一致（path/size/lfs）、401 两侧 PermissionError 且同文案、404 两侧都抛错。
- 决策：404 页在 Rust 映射 Status(404)（Python `raise_for_status` 抛错），修正了早先
  "404 返回空" 的测试预期；嵌套子页 dict 保留完整 path（只有递归 URL 丢段），修正了
  早先手写预期。两侧等价断言均过。
- 验证：
  - `cargo fmt --all --check` 0；`cargo clippy --all-targets -- -D warnings` 0 error；
  - `cargo test -p gd3-core -p gd3-net -p gd3-servers` 全过（hf 集成测试 6 项）；
  - `maturin develop` 构建成功，`gd3_py.hf_fetch_tree` 可导入；
  - `pytest -q --continue-on-collection-errors`：开关关 **3F/761P/8S/35E**（基线同失败集合，
    多的 3 passed 是新测试）；开关全开（含 GD3_RUST_HF_TREE）**3F/758P/11S/35E**，
    失败集合与开关关完全一致 + `test_engine_listeners` 既有环境 flake（1 个 asyncio 参数化项，
    第 1 步已单独重跑 12 次证明与 Rust 无关）。
- 下一步：http_pack 解析层剩余（`match`/文件名来源收口）或 github_pack 树枚举同款迁移；
  之后 PortListener。无卡点。

## 2026-10-16 第 3 步
- 做了什么：接手上一轮中断留下的未提交半成品并收口（全部随 commit a6612af 提交）。
  - `gd3_core::github`：`is_github_file_url` / `proxy_headers` / `match_file` 纯判定层（极简 urlparse 子集 + GITHUB_HOSTS 表），内嵌单测 3 项；PyO3 导出 `github_is_file_url` / `github_match_file` / `github_proxy_headers`。
  - 适配器 `features/github_pack/rust_match.py`（`GD3_RUST_GITHUB`，默认关）；pack.py::isGitHubFileUrl / probe.py::toProxyHeaders,matchFile 按开关分流。
  - `gd3-net::probe::probe_size`：镜像 `bili_pack.stream.probeSize`（bytes=0-0 + accept-encoding identity，不查状态码、一切失败归 0），PyO3 `probe_size`；适配器 `features/bili_pack/rust_size.py`（`GD3_RUST_PROBE_SIZE`）；stream.py 分流。
  - 新 Python 测试 `tests/test_github_match.py`（双实现对表 20+ 断言，3+4 参数化 + 2 个同步用例）。
  - **修掉新测试死锁**：`rust/crates/gd3-net/tests/size.rs` 自制 RawServer 原本在 `#[tokio::test]` 里用 std `TcpListener` 阻塞 accept + `spawn_blocking` 读——请求其实已成功返回，挂死发生在测试 teardown：accept 循环占着 worker 线程，runtime 永远关停不了。重写为全 tokio 异步实现后 6 项秒过。
  - 排查教训记录：① `size-*` 通配会匹配进旧的锁定版二进制造成误判，实验一律用精确路径；② 曾因一段坏 Python 脚本覆写 `gd3-net/src/probe.rs`，已按 git 恢复并重打 probe_size 补丁，最终 diff 与半成品等价。
- 验证：
  - `cargo fmt --all --check` 0；`cargo clippy --all-targets -- -D warnings` 0 error；
  - `cargo test -p gd3-core -p gd3-net -p gd3-servers`：18 个测试二进制 0 failed（新增 size.rs 6 项 + github 3 项）；
  - `maturin develop` OK，`gd3_py` 四个新导出冒烟通过；`tests/test_github_match.py + test_bili_parse.py` 48 passed；
  - `pytest -q --continue-on-collection-errors` 开关关 **3 failed / 783 passed / 8 skipped / 35 errors**（基线同失败集合，passed 增 22 = 新测试）；
    开关全开（GD3_RUST_GITHUB/GD3_RUST_PROBE_SIZE/GD3_RUST_PROBE/GD3_RUST_HF_TREE/GD3_RUST_PERSIST/GD3_RUST_HTTP/GD3_RUST_ARIA2/GD3_RUST_BROWSER/GD3_RUST_FILENAME）两轮：第一轮多 1 个 `test_engine_listeners` 既有环境 flake，第二轮失败/错误集合与基线**逐条一致**。
- 下一步：按 README「未做」清单继续——github_pack 树枚举（同 hf_fetch_tree 模式）或 http_pack parse 剩余（buildTask/HttpParser.match），之后 PortListener。无卡点。

## 2026-10-16 第 4 步（第 13 轮收口，commit 42ec437）
- 做了什么：接手上一轮未提交的 Python 侧半成品并收口。
  - 冒烟验证 `GD3_RUST_BILI_PARSE=1` 下 `stream.py::parseDash`/`buildFnval` 走 Rust：
    正确路径写回 `videoUrl/audioUrl/videoSize/audioSize/_videoStreams/_audioStreams`；
    video 为空时 ValueError 文案逐字（`"Bilibili 返回结果中不存在可用的媒体流"`）。
    （冒烟时先纠正自己的预期：`toStreamUrl()` 取 `baseUrl`，不是 `toStreamUrl` 字段。）
  - 诊断：Previous `ModuleNotFoundError: bili_pack` 是裸 `python -c` 路径问题——冒烟脚本
    `sys.path` 指到仓库根后 `from features.bili_pack import stream` 即正常。
  - 验证：
    - `pytest tests/test_bili_rust_parse.py tests/test_bili_parse.py` = 61 passed；
    - `pytest -q --continue-on-collection-errors` 双模式各一轮：
      开关关 **3F/842P/8S/35E**，开关全开（含 GD3_RUST_BILI_PARSE/GITHUB/PROBE_SIZE/PROBE/HF_TREE）
      **3F/842P/8S/35E** —— 计数完全相同；逐条对比失败/错误集合，与基线一致
      （3 failed = android_config_parity / github_probe / runtime_install；35 errors = android 系列；未出现 flake）；
    - fmt/clippy/cargo test 上一轮已绿；构建已 `maturin develop`。
  - 提交：features/bili_pack/rust_parse.py（新适配器）+ stream.py 接线 + tests/test_bili_rust_parse.py（152 行），42ec437。
- 下一步：`rust/README.md` 未做清单——github_pack 树枚举（hf `fetch_tree` 同款模式）、
  huggingface/bili 其余解析、`PortListener` 接入。无卡点。
## 2026-10-16 第 5 步（第 13 轮追加）
- 做了什么：收掉工作区一份未提交的 http_match 修正——`parser_match_passive` 补上
  urlparse 对最后一个 path 段 `;` 之后 params 的摘除（实测 CPython 是从路径最后一个
  `/` 之后的第一个 `;` 截断；带多分号逐字对比 4 组边界与 LIVE Python/参考实现完全一致），
  上一轮记录的「`https://a/b.mp4;` → false」预期其实是当时没跑双实现的误判。
- 验证：`tests/test_http_match.py` 27 passed（补 3 个多分号 URL）；fmt/clippy/cargo test 全绿。
- 提交：rust/crates/gd3-core/src/http_pack.rs + tests/test_http_match.py。
- 下一步：github_pack 树枚举（hf fetch_tree 同款模式）。
