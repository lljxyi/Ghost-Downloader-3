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

## 2026-10-16 第 6 步（commit 2d7a248）
- 做了什么：HuggingFace URL 解析层（`parseHuggingFaceUrl`）署 Rust 影子分支（`GD3_RUST_HF_URL`，默认关）。
  - `gd3_net::hf::parse_url_entry(url, proxy_site)`：不引 regex crate，行内手写
    `REPO_PATTERN` 的线性匹配（host 判定与 Python 相同：scheme 无关、hostname
    小写 + 去 www；代理站只比 hostname——去 scheme/userinfo/端口，路径保留）。
    语义要点（边界测试盯住）：`org/repo/`（尾斜杠、无动词）不匹配；`resolve/`
    与 `resolve/main/`（path 至少 1 字符）不匹配；`resolve/main//x` 的 filePath
    贪婪保留 `/x`；大小写只对 verb/`datasets|spaces` 前缀敏感、org/repo 不限。
  - PyO3 `hf_parse_url` → `features/huggingface_pack/rust_url.py`；pack.py 顶层
    `parseHuggingFaceUrl` 按开关分流（代理站由 `selectedProxySite()` 传入）。
  - `tests/test_hf_url.py`：独立 pyRef 防两实现一起错；20 URL × 3 代理站全对表。
- 修掉的本轮坑：Rust 侧 `repo` 无尾随段漏判（`split_once` None 分支）；`/org/repo/`
  与 `/org/repo` 需区分（Python 正则要求 `/` 后必接动词）。
- 验证：cargo fmt/clippy（0 error）/cargo test（140 passed）全绿；maturin 重建；
  `tests/test_hf_url.py tests/test_hf_tree.py` = 6 passed；
  `pytest -q` 单独跑两轮（开关关/全开含新开关）：**3F/846P/8S/35E，失败集逐条一致**。
  （_connected 注意：两轮 pytest 连着跑会互相污染状态出现 41F 的假警报——必须每轮单独跑。）
- 下一步：bili `parseBiliUrl`（同款纯判定层）或 hf `parse` 剩余（buildPages/fetchView 的 JSON 变换层抽象），之后 PortListener。

## 2026-10-16 第 7 步（commit d78df9d）
- 做了什么：`tests/test_hf_url.py` 升级为参数化加强版（67 case）：每条 URL×代理站
  独立成一个用例（失败定位更准），并新增 pack 级入口（无开关/开关皆然）冒烟。
  保留独立 pyRef 实现。
- 验证：`pytest tests/test_hf_url.py` = 67 passed；git status 干净。
- 下一步：bili `parseBiliUrl` 纯判定层（同款模式），随后 PortListener。

## 2026-10-16 第 8 步（commit a38417b，第 1 轮续）
- 做了什么：bili `parseBiliUrl` 纯判定层收口（接第 7 步的半成品）。
  - Rust：`gd3_net::bili` 追加 `parse_url`（三规则线性匹配：`/video/(BV...|avN)`、
    `/N/lists/N`、`/N/channel/collectiondetail?sid=`），自写 `split_path_query`/
    `unquote_plus`/`parse_qs`（keep_blank_values=False）/`parse_page_spec`/
    `python_str_to_int`（错误文案逐字镜像 Python int()），4 项内嵌单测。
    修掉早稿 `BV1abc1` 截断 bug：匹配长度算在去前缀 body 上、切片也在 body 上做。
  - PyO3 `bili_parse_url`（失败抛同文案 ValueError）+ `features/bili_pack/rust_url.py`
    （GD3_RUST_BILI_URL，默认关）+ `parse.py::parseBiliUrl` 分流；maturin develop 重建。
  - `tests/test_bili_url.py`：21 passed（20 URL 双实现对表，错误文案 re.escape 逐字）。
  - LIVE Python 语义确认：`BV1av12` 第一候选即中；`p=1,`（尾空段）与 `p=x` 抛
    `int()` 英文文案（不报"不是有效的"）；`487/lists/920`（无前导斜杠）不匹配；
    `parsePageSpec("p=1%2C2+3,4-a")` 报 `invalid literal ... '2 3'`。
- 验证：fmt/clippy/cargo test 全绿（0 failed）；双模式全量回归按规则单独跑待下一批
  合并做（本轮只动了默认关的新分支，pytest 全量两轮已在上一步基线一致，本步新增
  测试文件独立跑通过）。
- 下一步：hf `parse` 剩余（buildPages/fetchView JSON 变换层）或 `PortListener` 接入。
- 补充（同轮）：bili URL 层双模式全量回归已单独跑完两轮
  （开关关 / GD3_RUST_BILI_URL=1）：均为 **3 failed / 933 passed / 8 skipped / 35 errors**，
  FAILED+ERROR 集合 diff 为空（38 条 = 基线 android_config_parity / github_probe /
  runtime_install + android 系列 errors，与基线一致）。本轮基线计数从 846P 涨到 933P
  = 本步 21 + 此前各步累计新增测试。

## 2026-10-16 第 9 步（commit 4178f2d，第 2 轮）
- 做了什么：hf `parse` 的 entries → files 纯变换层下沉 Rust 影子分支
  （`GD3_RUST_HF_FILES`，默认关）。
  - `gd3_net::hf::build_files(entries, base, repo_id, repo_type, revision)`：
    镜像 `for i, entry in enumerate(entries)` 循环——`path` 缺失为空串（不报错）、
    `size` 缺失为 0、`lfs` 为对象且带 `size` 时覆盖（lfs 缺 size 键保留原值）；
    `file_url`/`type_prefix` 镜像 `_fileUrl`（dataset→datasets/、space→spaces/）。
    内嵌单测 1 项（含 lfs=非 dict、lfs={}、只写 size 条目的 URL 尾斜杠形态）。
  - PyO3 `hf_build_files`（depythonize 收 Vec<serde_json::Value>，返回
    (path, size, downloadUrl) 元组列）+ `features/huggingface_pack/rust_files.py`
    适配器 + `pack.py` 接线（`base` 提前到 apiUrl 计算处，复用同一值）。
  - `tests/test_hf_files.py`：11 case（10 参数化 + 边界组），独立 pyRef，双模式
    各跑一遍全过（11 + 11 passed）。
- 验证：fmt 0；clippy 0；cargo test 0 failed；maturin develop 重建后桥冒烟正确
  （dataset 前缀 + lfs 覆盖逐条核对）；双模式全量回归单独跑两轮：
  开关关 / GD3_RUST_HF_FILES=1 均 **3 failed / 944 passed / 8 skipped / 35 errors**，
  FAILED+ERROR 集合 diff 为空（与基线一致；933→944P = 本步 11 + 上轮收口的 bili URL 测试 21→已计入…——
  修正：+11 即本步新增）。
- 下一步：按 `rust/README.md`「下一步的顺序」剩余可下沉项（bili/github 其余解析层 }),
  之后就是 UI（Phase 7 Slint）与 cfg 伴生工作。

## 2026-10-16 第 10 步（commit c2d1b3d，第 3 轮）
- 做了什么：bili `buildPages` JSON 变换层下沉 Rust 影子分支（`GD3_RUST_BILI_PAGES`，默认关）
  - Rust：`gd3_net::bili::build_pages`（镜像 season/pages 遍历、`x or default` 真值、
    选页规则 setVideoSelection/setSeasonSelection、episodeCover 三级 fallback、toHttps、
    `int(raw["cid"])` 缺失→KeyError("cid")、int 文案逐字）+ PyO3 `bili_build_pages`
    （KeyError→PyKeyError；返回扁平 dict {pages,title,coverUrl}）。内嵌单测 6 项全过。
  - 接线：`features/bili_pack/rust_pages.py` 适配器（同步直调，与 rust_url 同模式；
    parse.py::buildPages 开头按开关分流）。适配器初稿误写 async + to_thread，
    调用方全为同步，改同步直调。
  - `tests/test_bili_pages.py`：9 参数化 parity（正常视频/点名选页/season 多分区/
    seasonId 全选/单分区连载/空 viewData 默认标题/空 pages）+ missing-cid KeyError +
    关态断言，双实现（monkeypatch 环境变量切分支）逐字段对表：37 passed（含 test_bili_parse 26）。
  - 收尾坑：clippy 4 连（elided lifetime/let match/2×`clone_clone`，`.map(Clone::clone)` 换 `.cloned()`）；
    `test_disabled_mode_is_python` 在全开环境跑会假红（isEnabled()=True），已 monkeypatch.delenv。
- 验证：fmt 0 / clippy 0 / cargo test 三 crate 全绿（18 个 ok 无失败）；maturin develop 重建；
  双模式全量回归两轮：开关关 / 全部开关+新开关，均 **3 failed / 955 passed / 8 skipped / 34 errors**，
  FAILED+ERROR 集合 diff 为空。
- 环境注意：本轮回归时 `android/` 目录整个消失（非本会话改动，疑似并行 agent 操作），
  `test_android_pack_import.py` 因缺 `android/app/build.gradle.kts` 收集失败已 `--ignore`；
  android errors 基线由 35 → 34。若下轮 android 目录回来，恢复原口径。
- 下一步：PortListener 接入评估，或 github/tree 剩余解析层。

## 2026-10-16 第 11 步（第 4 轮）
- 做了什么：bili `fetchView` 网络层下沉 Rust 影子分支（`GD3_RUST_BILI_VIEW`，默认关）。
  - Rust：`gd3_net::bili::fetch_view`，分段纯函数 `season_list_url` / `view_api_url`（av→avid
    去前两位）/ `pick_season_video_id`（「未获取到合集信息」逐字）/ `check_view_payload`
    （code missing/null/0 放行、取 data or {}、message 兜底文案逐字）+ async `get_json`
    共用 api_headers；ViewError{Transport, ValueError}。内嵌单测 5 项（gd3-net lib 40 passed）。
  - PyO3 `bili_fetch_view(video_id, mid, season_id, api_headers, client_profile)`
    （wreq client + 每次 build runtime，与 hf_fetch_tree 同款限制）→ 返回 data dict。
  - `features/bili_pack/rust_view.py` 适配器（asyncio.to_thread；真实 seasonId 传入——
    早稿误传 `1 if seasonId else 0`，会把 URL 里的 season_id 写成 1，已修）；pack.py 接线。
  - 桥冒烟：真实 API 可达；错误路径 `ValueError 啥都木有`（bilibili message 原样透传）。
- 验证：fmt 0 / clippy 0 / 三 crate cargo test 全绿（18 ok）；maturin 重建；
  bili 相关定向测试 93 passed；双模式全量回归两轮均 **3F/955P/8S/34E**，集合 diff 为空
  （android/ 目录仍缺，继续 --ignore test_android_pack_import）。
- 文档坑：`//!` 有序列表后续行缩进 5 空格会报 overindented、不缩进报 without indentation——
  4 空格才算挂到列表项下。
- 下一步：m3u8 parse 层（依赖 `m3u8` 库语义，量大）或 ftp/其他纯层；bili 网络层其余
  （fetchPlayurl 请求侧）可继续同款。

## 2026-10-16 第 12 步（第 5 轮，commit eb9321e）
- 做了什么：bili WBI 签名/账号纯函数下沉 Rust 影子分支（`GD3_RUST_BILI_SIGN`，默认关）。
  - Rust：`gd3_net::bili` 新增 `sign_params` / `mixin_key_from_urls` / `to_cookie` / 手写
    `quote_plus`（安全集对齐 CPython `_ALWAYS_SAFE`：保留 `_.-~`，`*`→`%2A`）。
  - 4 处与 Python 的偏差在收口时修掉：`quote_plus` 安全集；`wts` 改**覆盖**语义
    （先 `retain` 掉同名再 push，否则重复 `wts=` 导致 w_rid 不一致）；`mixin_key_from_urls`
    改 `Result<Option<String>, usize>`，越界索引在桥侧抛 `IndexError: string index out of range`；
    `to_cookie` 空白集改用 CPython `str.strip()` 语义（含 `\x1c`-`\x1f`）。
  - 桥：`gd3_py.bili_sign_params(params, mixin_key, wts)`、`gd3_py.bili_mixin_key(img_url, sub_url)`、
    `gd3_py.bili_to_cookie(raw)`；Python 侧 `features/bili_pack/rust_sign.py` + `account.py`
    三处接线（`toCookie` / `_updateWbiKeys` / `signParams`）。
  - `tests/test_bili_sign.py`（34 case）用 `monkeypatch` 定住 `time.time`，保证 wts 可比。
- 验证：fmt 0 / clippy 0 / 三 crate cargo test 全绿；maturin 重建后桥冒烟逐字段一致；
  34 passed；双模式全量回归两轮（开关关 / 17 开关全开）失败集合 diff 仅
  `test_engine_listeners.py::test_stopping_the_engine_releases_both_ports[asyncio]`
  （VPS-MIGRATION 第 1 节记录的既有环境 flake），+3 skip 是 Python 实现自身的白盒测试。
- 基线快照：`/root/baseline-failures.txt`（开关关那轮的 FAILED+ERROR，38 行）。
- 下一步：github `probeUrls` 竞速层（github_pack 的最后一块）。
- 卡点：无。

## 2026-10-16 第 13 步（第 5 轮）：github `probeUrls` 竞速层摸底
- 做了什么：评估候选下沉目标（github probeUrls / m3u8 / ftp / bili fetchPlayurl / HF 单文件 head），
  并实测 `test_github_probe.py::testProxySiteGetsNoCredentials` 失败根因。
- 根因（已实测确认，不是本会话引入）：Python 侧 `wreq` 会把**非规范头名**小写化，
  `X-Other` 上线变成 `x-other`，而 `Authorization` / `Cookie` 保持原样。
  `GD3_RUST_PROBE=1` 走 Rust 网络层时表现**完全相同**（同样 KeyError），所以把
  probeUrls 搬进 Rust 不会改变这条基线红灯的状态。
- 结论：github probeUrls 是下一步的合适目标（纯 HTTP，无外部库语义依赖）。
## 2026-10-16 第 14 步（第 5 轮）：github `probeUrls` 竞速层下沉 Rust 影子分支
- 做了什么：github_pack 的最后一块（`features/github_pack/probe.py::probeUrls`，37 行起）下沉 Rust。
  - Rust 网络层 `rust/crates/gd3-net/src/github.rs`：`CANDIDATE_TIMEOUT = 5s`、
    `CandidateError{Unmatched, Transport(String), Timeout}`（`Display`：`Unmatched`→`"代理站响应不合格"`、
    `Timeout`→`""`）、`pub async fn probe_urls(original_url, urls, template: &ProbeOptions,
    resolve_extension) -> Result<(String, RemoteFile, Vec<String>), CandidateError>`。
    用 `tokio::task::JoinSet` 逐候选 spawn + `tokio::time::timeout`，按**完成顺序**取首个
    `match_file` 通过者，胜出后 `candidates.shutdown().await` 取消并等待其余候选
    （对应 Python 的 `finally: cancel + gather`）；`unmatched` 记录不合格/失败候选并最终从
    fallback 列表剔除；`urls` 为空返回 `Transport("没有候选 URL")`。
  - 桥 `gd3-py/src/github.rs`：新增 `create_exception!(gd3_py, GithubUnmatchedError, PyException, ...)`
    与 `#[pyfunction] github_probe_urls(original_url, urls, headers, user_agent, client_profile,
    fallback_name, guess_extension=None) -> PyResult<(String, ProbeOutcome, Vec<String>)>`；
    `py.detach(...)` 内 `runtime.block_on(probe_urls(...))`；`CandidateError::Unmatched` 抛
    `GithubUnmatchedError`，其余走 `to_error`。`lib.rs` 注册函数与异常类型。
  - `gd3-py/src/probe.rs` 重构（105 行）：抽出 `ProbeOutcome::from_file` 与
    `extension_resolver(guess_extension)` 供 github 复用，`probe()` 改为调用这两个。
  - Python 适配 `features/github_pack/rust_probe.py`（新，67 行）：`isEnabled()` 读
    `GD3_RUST_GITHUB_PROBE == "1"`；`probeUrls()` 用 `asyncio.to_thread(gd3_py.github_probe_urls, ...)`，
    `except gd3_py.GithubUnmatchedError` → `raise TaskError("代理站响应不合格") from error`；
    返回 `(winner, RemoteFile(...), fallbackUrls)`。`features/github_pack/probe.py::probeUrls`
    开头接线影子开关。模块文档写明两点已知差异：竞速内部探测固定走 Rust（不受 `GD3_RUST_PROBE` 影响）；
    兜底文件名整场只生成一次。`probeProxyLatencies` 不迁移（耗时不可比）。
- 验证：
  - `cargo test -p gd3-net --test github` → **6 passed**（`archive_urls_accept_any_non_text_content_type`、
    `all_unmatched_reports_unmatched`、`proxy_candidates_lose_credentials_but_keep_other_headers`、
    `forbidden_candidates_report_a_transport_error`、`the_original_candidate_keeps_credentials`、
    `unmatched_candidates_are_dropped_from_the_fallback_list`）；`tests/support/test_server.rs` 的
    `RecordedRequest` 增加 `headers` 字段。
  - `tests/test_github_probe_rust.py`（新，212 行）双实现 parity → **6 passed**：6 个用例
    （unmatched 剔除 / 传输失败剔除 / pending 候选保留在 fallback / 单候选即原始 URL /
    全失败抛 TaskError / 代理候选掉凭据）。用例构造成「只有一个候选能赢」（0.1s vs 0.5s 延迟差），
    不依赖竞速顺序；头比较用 dict（小写化后）且排除 `host`（每个候选独立端口，必然不同）。
  - `tests/test_github_probe.py` 双模式：开关关 `1 failed, 12 passed`；
    `GD3_RUST_GITHUB_PROBE=1` `1 failed, 12 passed` —— 失败同为 `testProxySiteGetsNoCredentials`
    （既有基线红灯，`KeyError: 'X-Other'`，状态未变）。
  - 实测确认两侧请求头逐字一致（临时 dump 脚本已删）：开关关/开关开除 `host` 与头顺序外完全相同；
    `RemoteFile(name='file.bin', fileSize=100, canUseRangeRequests=True, lastModified='',
    contentType='application/octet-stream')` 两侧一致。
- 下一步：HF 单文件 `head` 探测（`features/huggingface_pack/pack.py:89-98`，小）。
- 卡点：无。

## 2026-10-16 20:55 第 15 步
- 做了什么：HF 单文件 HEAD 探测下沉到 Rust 影子分支（开关 `GD3_RUST_HF_HEAD`，默认关）。
  - `rust/crates/gd3-net/src/hf.rs` 追加 `HfFileHead { file_size: i64, can_use_range: bool }`、
    `pub async fn head_file(client, headers, url) -> Result<HfFileHead, HfTreeError>`、
    私有 `fn parse_int(text: &str) -> Option<i64>`（ASCII 子集：trim、可选 `+`/`-`、数字间单个 `_`），
    内嵌单测 `parse_int_follows_cpython`。
  - `rust/crates/gd3-py/src/hf.rs` 追加 `#[pyfunction] hf_head_file(url, headers, client_profile)
    -> PyResult<(i64, bool)>`；`lib.rs` 在 `hf_build_files` 之后注册。
  - `rust/crates/gd3-net/tests/hf_head.rs`（新，6 个集成测试）：`content_length_and_accept_ranges_are_read`、
    `forbidden_responses_still_report_headers`、`accept_ranges_value_is_ignored`、
    `missing_content_length_is_zero`、`negative_content_length_is_kept`、
    `non_numeric_content_length_is_an_error`。
  - Python 适配 `features/huggingface_pack/rust_head.py`（新）：`isEnabled()` 读
    `GD3_RUST_HF_HEAD == "1"`；`headFile(downloadUrl, headers) -> (fileSize, canUseRange)`，
    内部 `asyncio.to_thread(gd3_py.hf_head_file, ...)`，profile 取 `cfg.clientProfile.value`。
    `features/huggingface_pack/pack.py:89-98` 接线影子开关（原 `client.head` 那段保留在 else 分支）。
  - `tests/test_hf_head.py`（新，154 行）双实现 parity → **7 passed**：裸 socket `RawHeadServer`
    工厂 fixture + `rawResponse()`（aiohttp 造不出「缺 accept-ranges」「非整数 content-length」这类响应），
    `runCase`/`runBoth` 逐例对比 `(task.fileSize, task.steps[0].canUseRangeRequests)`，
    异常路径返回 `("error", type(error).__name__, str(error))`。7 个用例：
    content-length+accept-ranges / 缺 accept-ranges / `Accept-Ranges: none` / 负数 content-length /
    缺 content-length / 404 不抛 / 非整数 content-length 抛 ValueError。
- 验证：
  - `cargo fmt --all --check` OK；`cargo clippy --all-targets -- -D warnings` 干净；
    `cargo test -p gd3-core -p gd3-net -p gd3-servers` 全绿（含 `gd3-net --test hf_head` 6 passed）。
  - `pytest tests/test_hf_head.py -q` → **7 passed**（1.51s）。
  - 双模式全量回归（均 `--ignore=tests/test_android_pack_import.py -p no:randomly`）：
    开关关 → **3 failed / 1002 passed / 8 skipped / 34 errors**，与 `/root/baseline-failures.txt`
    的 FAILED+ERROR 集合 diff **仅**多出被 ignore 的 `ERROR tests/test_android_pack_import.py`；
    19 个开关全开（含 `GD3_RUST_HF_HEAD=1`）→ **3 failed / 999 passed / 11 skipped / 34 errors**，
    与开关关那轮的 FAILED+ERROR 集合 diff **为空**（完全一致）。
- 下一步：Phase 5 继续。候选（按性价比）：m3u8 `parse`（`features/m3u8_pack`，依赖 `m3u8`/`mpegdash`
  库语义，风险高）、bili `fetchPlayurl`（`features/bili_pack/stream.py`，量大）、
  ed2k `kelpie/models.py::Link.parse`、`PortListener` 接入。
- 卡点：无（一次 flake：19 开关首轮全量跑在
  `tests/test_http_subworker.py::TestTransientRetry::test_500_retries_then_succeeds` 卡住 1 小时，
  随后同命令连跑 2 次均 33s 正常完成；单开关 `GD3_RUST_HTTP=1`、18 开关（无 HF_HEAD）、
  单开关 `GD3_RUST_HF_HEAD=1` 也都正常。判为既有环境 flake，未修，后续全量跑一律加 `timeout`）。

## 2026-10-16 22:00 第 16 步：收口上一轮留下的 m3u8 半成品（第 6 轮开工）
- 做了什么：新会话开工时发现工作区有未提交的 m3u8 输出解析层半成品
  （`rust/crates/gd3-core/src/m3u8.rs` 362 行、`gd3-py/src/m3u8.rs` 桥、
  `features/m3u8_pack/rust_output.py` 适配、`task.py` 三处接线、
  `tests/test_m3u8_output_rust.py` 61 用例），内容完整，跑通质量门后提交。
  新增依赖：`gd3-core` 引入 `regex` crate（正则照抄 Python 模式）。开关
  `GD3_RUST_M3U8_OUTPUT` 默认关。
- 验证：`cargo fmt --all --check` OK；`cargo clippy --all-targets -- -D warnings` 干净；
  `cargo test -p gd3-core -p gd3-net -p gd3-servers` 0 失败；maturin 重建桥 OK；
  `pytest tests/test_m3u8_output_rust.py` 61 passed；`tests/test_m3u8_error.py +
  test_m3u8_progress.py` 34 passed。双模式全量回归（`--ignore=android_pack_import`）：
  开关关 3F/1063P/8S/34E，FAILED+ERROR 与基线 diff 只差被 ignore 的 android 一条；
  20 开关全开 4F/1059P/11S/34E，diff 仅
  `test_engine_listeners.py::test_stopping_the_engine_releases_both_ports[uvloop]`
  ——运维文档记录的既有 flake，单跑 `2 passed` 复验非回归。
- 下一步：m3u8 pack 的下一层（`parse`，依赖 m3u8/mpegdash 库语义，先摸底再决定）；
  或按 README「未做」清单继续。
- 卡点：无。
## 2026-11-02 第 17 步：ed2k `Link.parse` 下沉 Rust（`GD3_RUST_ED2K_LINK`，默认关）
- 改动：
  - `rust/crates/gd3-core/src/ed2k.rs`（新）：镜像 `kelpie/models.py::Link.parse`，
    含镜像 CPython 的 `python_strip`（Unicode White_Space + \x1c–\x1f）、
    `python_unquote`（逐字节 %XX + 逐字节 U+FFFD 替换）、`python_lower`；
    size 先去前导零按位数封顶（MAX_SIZE 是 12 位数）再让 u128 承接，幂等承接 Python 任意精度 int。
  - `rust/crates/gd3-py/src/ed2k.rs`（新桥）：`ed2k_parse_link(text) -> (name, size, hash)`，
    失败抛 `Ed2kInvalidLinkError(message)`；`lib.rs` 注册（含异常类型导出）。
  - 金文件 `tests/fixtures/ed2k.jsonl`（53 条，Python 生成）+ generator
    `generate_ed2k_fixture.py`；`rust/crates/gd3-core/tests/ed2k.rs` 逐条比对。
  - Python 适配 `features/ed2k_pack/rust_link.py`（isEnabled/parse，
    异常翻 `Error(ErrorCode.INVALID_LINK, message)`，`toTaskError` 不动）；
    `features/ed2k_pack/pack.py::ED2kParser.parse` 加开关分支。
  - `tests/test_ed2k_link_rust.py`：金文件 53 条 + 8 条补例 + 常规用例，
    直接对跑 Python/Rust 两实现断言逐字段一致；开关默认值断言用
    `monkeypatch.delenv`（不与「全开回归」环境冲突）。
- 过程中的语义修正（对照 CPython 实测，上一轮的假设有误）：
  - 错误消息用 strip **之后**的 text（此前 `rot="   "` 用例暴露）；
  - 普通链接里 name 的 `%7C` 始终由 `unquote` 解码（`Link.parse` 文件注释里
    「%7C is part of the name」只是说明整体解码分支不触发——CPython 实测 name 已解码）;
  - hash 全大写合法（HASH_PATTERN 是 `[0-9A-Fa-f]{32}`）。
- 注意事项：本机 pytest 需先 `source /root/gd3-env.sh`（内含
  `QT_QPA_PLATFORM=offscreen`）。未 source 时 Qt（pytestqt `qapp` fixture）
  初始化即 SIGABRT（「could not connect to display」），与代码改动无关；
  本轮在 `/tmp/off-full.txt` 各留有一份现场(backtrace)确认。
- 验证：
  - `cargo fmt --all --check` OK；`cargo clippy --all-targets -- -D warnings` 干净；
  - `cargo test -p gd3-core -p gd3-net -p gd3-servers` 全 0 失败
    （含 `--test ed2k` 2 passed：53 条金文件逐条比对 + MAX_SIZE 断言）；
  - maturin 重建桥 OK；
  - `pytest tests/test_ed2k_link_rust.py tests/test_ed2k_task.py` → 181 passed；
    `tests/test_ed2k_kelpie.py` 8 skipped（kelpie 引擎未装，既有状态）；
  - 双模式全量回归（`--ignore=tests/test_android_pack_import.py -p no:randomly`）：
    开关关 → **3 failed / 1126 passed / 8 skipped / 34 errors**，FAILED+ERROR
    与 `/root/baseline-failures.txt` diff 仅少被 ignore 的 android 一条；
    21 开关全开 → **5 failed / 1121 passed / 11 skipped / 34 errors**，与开关关的
    FAILED+ERROR diff 仅两条：`test_switch_defaults_to_off`（by-design，修正后败局消失）
    与 `test_engine_listeners.py…[uvloop]`（既有 flake，单跑 2 passed 复验非回归）。
- 提交：`b5033bc`（核心层+金文件+Rust 测试）、`8a739cd`（适配器+接线+parity）、
  `1452b24`（monkeypatch 修正）。
- 下一步：m3u8 `parse` 摸底（`m3u8`/`mpegdash` 库语义，先实测再决定）；
  ed2k `parseProgress`（`kelpie.py::336`，纯 JSON 变换）；bili `fetchPlayurl`（量大）。
- 卡点：无。
## 2026-11-03 第 18 步：m3u8 清单 parse 扫描面下沉 Rust（`GD3_RUST_M3U8_PARSE`，默认关，第 10/11 轮收口）

- 做了什么：
  - 核心层收口（`rust/crates/gd3-core/src/hls.rs`，上轮半成品 + 本轮修正，`495f285`）：
    `#EXT-X-VERSION` 移入 int cast（`py_simple_param`，重构出 `py_simple_param_raw`）；
    `#EXT-X-ALLOW-CACHE`/`#EXT-X-PLAYLIST-TYPE` 只做 split-unpack ValueError 面（str cast）；
    修两个扫描层 bug：`py_int` 的 Ok 分支漏应用负号（`-1920` 回 `1920`，与上轮 `py_float` 同款，
    金文件 `master_resolution_negative` 暴露）；空 key dict（`#EXT-X-KEY:` 无参数）应容忍
    （model 层 `Key(...) if params else None` → None 不构造，`key_empty_params` 暴露）。
    附回归断言（`py_int("-1920")==-1920`、`py_int("- 1")` invalid literal 文案、空 key probe 不报错）。
  - 桥（`rust/crates/gd3-py/src/hls.rs` + `lib.rs` 注册，`6c35a96`）：
    `gd3_py.m3u8_probe(body) -> (tag, message, is_endlist, segment_count, [(uri,width,height,codecs,frame_rate),…])`，
    error 时 `("error", "Kind: message", false, 0, [])`，不抛 Err。
  - Rust 金文件对比（`tests/hls.rs` 重写为 mismatch 收集器，`495f285`）：75 条（46 ok / 29 error）
    逐条比对，dump 全部不一致而不是第一条就 panic（本轮用它一次抓出两个 bug）。
  - 适配器（`features/m3u8_pack/rust_playlist.py`，`e9b39ee`）：`loadsSnapshot(body)` +
    `isEnabled()`（`GD3_RUST_M3U8_PARSE`）+ `absoluteUri`（逐字镜像 `BasePathMixin.absolute_uri`
    的 urljoin+scheme 判定含报错文案）+ 异常同型翻译：按 Kind 挑 ValueError/OverflowError/
    TypeError/IndexError 原型重抛；KeyError 的 `str(e)` 是参数 repr（`'bandwidth'`），
    所以先 `ast.literal_eval` 解一层引号再抛——native `str`/`repr`/`f"{type}:{e}"` 三件套对齐。
  - 接线（`features/m3u8_pack/pack.py`，`0902878`）：三处 `m3u8.loads` 扫描面分流——
    主清单 live 判定（segments/变体 + `absoluteUri` 拼第一个变体 URL）、变体清单判活、
    `_hlsStreams` 的 streams 枚举。相对导入沿用 pack 的函数内延迟 import（与 task.py 同 pattern，
    顶层 `from . import X` 在这种动态加载包里 ImportError）。
  - Python parity（`tests/test_m3u8_parse_rust.py`，`0902878`）：金文件 75 条逐条对表
    （error 路径 type 名/str/repr 三件套、ok 路径 isEndlist/段数/rawUris/streams）+ absoluteUri
    与库 Segment mixin 对表（含空 base_uri 抛 ValueError 文案逐字）+ pack 级 `_hlsStreams`
    双模式冒烟 + 开关默认值（monkeypatch.delenv）。**82 passed**。
- 踩坑记录：
  - 测试里一开始用 `m3u8.M3U8(base_uri=…).uri` 想拿 mixin 的 absolute_uri——顶层对象没有
    `uri` 属性，要构造 `m3u8.model.Segment(uri=…, base_uri=…)`；且空 base_uri 时库也抛
    （不是返回相对结果），删掉错误预期。
  - maturin 记得同时加 `/root/.cargo/bin` 进 PATH（只加 .local/bin 会报 rustc 不在 PATH）。
- 验证：
  - `cargo fmt --all --check` OK；`cargo clippy --all-targets -- -D warnings` 干净
    （修掉桥里 `Option<f64>` 的 clone ×1）；`cargo test -p gd3-core -p gd3-net -p gd3-servers`
    全 0 失败（含 `--test hls` 1 passed：75 条金文件逐条比对）；
  - maturin 重建 OK；m3u8 四件定向（parse_rust/output/error/progress）= 177 passed；
  - 双模式全量回归（`--ignore=tests/test_android_pack_import.py -p no:randomly`）：
    开关关 → **3 failed / 1208 passed / 8 skipped / 34 errors**；
    22 开关全开 → **3 failed / 1205 passed / 11 skipped / 34 errors**；
    FAILED+ERROR 集合 diff **为空**（1208→1205 的差 = 走 Rust 分支后白盒 Python 分支测试
    skip，之前各步已建立）。失败 3 条仍是既有基线（android_config_parity / github_probe /
    runtime_install）。
- 提交：`495f285`（核心层 + 金文件 + Rust 对比测试）、`6c35a96`（桥）、`e9b39ee`（适配器）、
  `0902878`（接线 + parity）。
- 下一步：ed2k `parseProgress`（kelpie.py::336，纯 JSON 变换，小）；或 bili `fetchPlayurl`（量大）；
  mpegdash 评估。之后 Phase 7 Slint UI 与 `/root/Ghost-Downloader-3/AGENT-DONE`。
- 卡点：无（android 目录仍未回来，34 errors 口径不变）。
## 第 19 步：ed2k `parseProgress` 下沉（GD3_RUST_ED2K_PROGRESS，默认关）【完成】

- 核心层（`1f3eb84` + parity 修正 `d24afca`）：
  `rust/crates/gd3-core/src/ed2k.rs` 的 `parse_progress`/
  `parse_sources`/`parse_source`/`field_of`/`py_type_name`/
  `type_error_subscript`，字段全部 `serde_json::Value` 原样透传，
  sources 分解 6 元组；单测 3 个。
- 桥（`f54891f`）：`gd3_py.ed2k_parse_progress(message)` → 11 元组，
  KeyError/TypeError 原型直接抛出（PyKeyError/PyTypeError），
  str(e) 与 Python 一致，无翻译层。
- 适配器 + 接线（`dc4ba08`）：
  `features/ed2k_pack/kelpie/rust_progress.py`（kelpie 子包内，
  `isEnabled()`/`FIELD_NAMES`/`parseProgress` 拼回 Progress/Source 数据类）；
  `kelpie.py::parseProgress` 函数内延迟 import + `isEnabled()` 分流，
  原生分支原样保留。
- parity（`e94076c`）：金文件 `ed2k_progress.jsonl`（40 条，
  `generate_ed2k_progress_fixture.py` 用 Python 原实现生成），
  ok 路径 repr 逐字节、error 路径 type/str 逐字节三方对表，
  mismatch 收集器一次报全。
- 核心层 parity 修正（实证 CPython 3.13）：
  1. 字段求值顺序：10 字段先于 `sources`（原实现关键字求值顺序），
     `{'hash':'h'}` → KeyError 'size'、`{'sources':[1]}` → KeyError 'hash'；
  2. str 元素/非空字符串 sources/dict sources 的下标文案补
     `, not 'str'` 尾巴（`string indices must be integers, not 'str'`）。
- NaN/Infinity 决策：语料剔除。serde_json 值模型不可表达
  （Number::from_f64→None→Value::Null），且线上不可达——
  kelpie 引擎侧用严格 JSON 序列化（serde 遇 NaN 直接报错），
  progress 的 rate 字段实际均为 int。
- 质量门：fmt/clippy/test 全绿；maturin 重建 OK；
  双模式全量回归（--ignore=android_pack_import -p no:randomly）：
  关 → 3 failed / 1210 passed / 8 skipped / 34 errors；
  开（23 开关）→ 3 failed / 1207 passed / 11 skipped / 34 errors；
  FAILED+ERROR 集合 diff 为空，失败 3 条为既有基线
  （android_config_parity / github_probe / runtime_install）。
- 插曲：一次开模式全量在 2400s 超时（疑似 listeners 端口偶发挂住，
  与本轮改动无关——该用例独立复跑 1 passed），重跑即恢复，未追。
- 开关清单（23 个）：GD3_RUST_PERSIST HTTP ARIA2 BROWSER FILENAME
  PROBE HF_TREE GITHUB PROBE_SIZE BILI_PARSE HF_URL BILI_URL
  BILI_PAGES BILI_VIEW BILI_SIGN GITHUB_PROBE HF_HEAD ED2K_LINK
  HTTP_MATCH M3U8_OUTPUT M3U8_PARSE ED2K_PROGRESS
  （+ BILI_GITHUB 等历史项见各自步骤）。
- 下一步：bili `fetchPlayurl`（量大）或 mpegdash 评估；
  之后 Phase 7 Slint UI 与 AGENT-DONE。

## 第 20 步：bili `fetchPlayurl` 响应判定层下沉（GD3_RUST_BILI_PLAYURL，默认关）【完成】

（时间戳取自 `date`：2026-10-17 凌晨，与前同日会话）

- 范围决策：只下沉 `playPayload = await response.json()` 之后的**纯判定层**
  （code 兜底 / qn=80 递归 / message or 默认 / pageData / dash /
  videoStreams 可迭代 / acceptQuality list()）；网络 I/O（buildClient、
  412、raise_for_status）、wbi 签名（已下沉 BILI_SIGN）、parseDash
  （已下沉 BILI_PARSE）不动；字幕 fetch 留 Python（后续 20b 可选）。
  mpegdash（`_mpdStreams`）判定跳过：需在 Rust 重写整棵 MPD XML
  解析器且报错面无法逐字对齐，不建议下沉。
- 核心层（`8a1c228`）：`rust/crates/gd3-net/src/bili.rs` 追加
  `playurl_gate(playPayload, qn) -> PlayurlGate`
  （`Ok{dash, accept_quality}` / `Fallback` / `Error(message)` /
  `AttributeError(msg)` / `TypeError(msg)`），`PLAYURL_DEFAULT_ERROR` 常量
  「获取 Bilibili 音视频流失败」；语义要点（实证 CPython 3.13）：
  1. `code in {None, 0}`：None / 0 / 0.0 / False 通过；list/dict 不可哈希
     → TypeError `unhashable type: 'list'/'dict'`（先于 qn 兜底）；
     非 0 数字 / 字符串 / 非 False 布尔 → fail。
  2. 顺序敏感：`message or 默认`（真值原样透传，可为 int）→
     `pageData or {}`（真值非 dict → AttributeError `'X' object has no
     attribute 'get'`）→ `dash or {}` 同 → `list(video or [])` 非空判定
     （str=字符数、dict=键数；真值 int/float/bool → TypeError `'int'
     object is not iterable`；假值 0/False → 空列表）→ qn 兜底 →
     `list(accept_quality or [])`。**兜底优先于 accept_quality 求值错误**；
     `accept_quality != []` 用 `!=` 而非 is_empty。
  3. tuple 不可达剔除：JSON 序列化不报 tuple；Rust depythonize 把 tuple
     当 array 报 unhashable `'list'`，Python（可哈希）走兜底——线上
     payload 只来自 response.json() 无此形态。NaN/Infinity 同理（serde
     值模型不可表达）。
  gd3-net lib 单测 8 项（playurl_gate_tests）。
- 桥（`7af0067`）：`gd3_py.bili_playurl_gate(playPayload, qn)` →
  `('ok', dash, acceptQuality)` / `('fallback',)` / `('error', message)`
  元组；AttributeError/TypeError 按原型直接抛出（无 kind 翻译层）。
- 适配器 + 接线（`ff0915e`）：`features/bili_pack/rust_playurl.py`
  （isEnabled + playurlGate 元组透传）；`stream.py` 抽模块级纯函数
  `playurlGate(playPayload, qn)`（原生分支=原行序逐行一致，函数内
  延迟 import 分流；返回 `('fallback',)`/`('error', message)`，
  message 由调用方 `raise ValueError` 保非 str 原型）；fetchPlayurl 的
  判定块替换为 gate 调用（fallback → qn=80 递归；error → ValueError；
  ok → dash/acceptQuality 接 parseDash），`accept_description` 在
  return 内用 `(playPayload.get("data") or {})` 重取（gate 已保证此时
  data 为假值或 dict，安全），`page.subtitles`/fetchSubtitles 留原位。
- parity（`a407646`）：`tests/test_bili_playurl_rust.py`——沿用 bili
  「测试内参数化双实现对表」惯例（无 jsonl 金文件）：
  19 outcome + 10 error 用例（各测试 delenv 保原生分支可达；
  ok 路径整 tuple 值相等，error 路径比 type+str 逐字）+
  接线用例（开关开时 stream.playurlGate 走 Rust）+ 默认关断言。
  关/开模式各 30 passed。曾预判错 4 个语义（tuple 可哈希、dict video
  计非空、video=0/False 走兜底），按实证修正入库。
- 质量门：cargo fmt/clippy 干净；cargo test -p gd3-core -p gd3-net
  -p gd3-servers 全 0 失败（gd3-net lib 56 passed 含 8 个 playurl_gate）。
  bili 5 个测试子集（parse/rust_parse/pages/sign/url）双模式各
  127 passed。
- 双模式全量回归（--ignore=android_pack_import -p no:randomly，
  开关全 24 个）：
  关 → **3 failed / 1240 passed / 8 skipped / 34 errors**
  （1210+30 新增 parity 用例）；
  开 → 轮间波动 3/4/5 failed / 1235 passed / 11 skipped / 34–35 errors，
  FAILED+ERROR 名单与关模式 diff **为空**（有一轮 videos 名单多出的
  TEST 全部属于 `test_engine_listeners.py` 端口家族）。失败 3 条为
  既有基线（android_config_parity / github_probe / runtime_install）。
  engine_listeners 偶发核查（独立复跑）：开关全开 1 failed→复跑
  10 passed；**关掉新开关** 5 连跑 10P/2F+1E/2F/1F+1E/2F+2E——错误率
  与新开关无关，是本机端口释放既有偶发（同第 19 步记录），未追。
- 开关清单（24 个）：第 19 步 23 个 + `GD3_RUST_BILI_PLAYURL`
  （另 `GD3_RUST_HF_FILES` 在代码中有引用，此前清单写作 PROBE_SIZE
  邻项，注意齐全构建勿漏）。
- 下一步：bili `fetchSubtitles` 字典变换（20b，可选）；
  之后 Phase 7 Slint UI 与 `/root/Ghost-Downloader-3/AGENT-DONE`。

## 第 20b 步：bili `fetchSubtitles` 字典变换下沉（GD3_RUST_BILI_SUBTITLES，默认关）【完成】
（2026-10-17 01:41 前收口；HEAD=c40d5f1）
- 范围：只下沉 `fetchSubtitles` 拿到 payload 之后的**纯字典变换**
  （`stream.py::subtitleList(payload) → list[dict]`）；网络 I/O
  （client.get player/v2 API URL）与其外层 try/except 吞错留 Python。
  原型异常（AttributeError/TypeError）被 fetchSubtitles 的
  try/except 吞掉返回 `[]`——原生与 Rust 分支网络行为一致。
- 核心层（`42ccc4f`）：`rust/crates/gd3-net/src/bili.rs` 追加
  `SubtitleError{AttributeError, TypeError}`、`sub_get_error`/
  `sub_not_iterable`（复用 py_json_type/truthy/or_default）、
  `sub_descend`（`(x or {})` 链真值 dict 校验）、
  `number_eq_one`（type==1：Number as_f64==1.0 与 Bool(true) 均相等）、
  `subtitle_list(payload)`。`subtitle_list_tests` 7 用例；
  gd3-net lib **63 passed**。语义要点：
  1. `(payload.get("data") or {}).get("subtitle") or {}`——外层
     payload 非 dict（含 None/假值）→ AttributeError（无外层
     or 兜底）；data/subtitle 假值 or 链回退；真值非 dict →
     AttributeError `'X' object has no attribute 'get'`。
  2. rawList 经 `or []`（0/False/None/""/{} → 空）；
     真值数字/bool → TypeError not iterable；真值 str/dict →
     逐元素（字符/键）走 `s.get` → AttributeError `'str'`。
  3. 过滤 `s.get("lan") and s.get("subtitle_url")` 真值；
     `.get(k, default)` 键存在取值优先（lan_doc=None 不回退
     default）；isAi=`type==1`（1/1.0/True→True；"1"/2/0.5/
     False/None→False）；输出键序 lan/lan_doc/subtitle_url/isAi。
- 桥（`08f4fb1`）：`gd3_py.bili_subtitle_list(payload)` →
  pythonize 列表（键序一致）或直接抛 PyAttributeError/
  PyTypeError（文案逐字：10 类错误路径含
  `'float'/'bool'/'NoneType' object has no attribute 'get'` 的
  根值/链值/元素层级）；lib.rs 注册，maturin 重建 OK，
  clippy/fmt 干净。
- 适配器 + 接线（`062951c`）：`features/bili_pack/rust_subtitles.py`
  （isEnabled=env 动态读取 + subtitleList 透传）；`stream.py` 抽
  模块级纯函数 `subtitleList(payload)`（原生分支=原 rawList or 链 +
  列表推导逐行一致，函数内延迟 import 分流）；`fetchSubtitles`
  变换块替换为 `return subtitleList(payload)`（try/except 包裹不变）。
  冒烟 10 错误路径 + 正常态关/开双模式逐字一致；bili 6 测试子集
  双模式各 157 passed。
- parity（`c40d5f1`）：`tests/test_bili_subtitles_rust.py`——
  37 outcome + 17 error 锚点（Python 原生先验内嵌 want，Rust 再对
  want；error 先验 Python type/str 再验 Rust 逐字）+ 接线 + 默认关。
  关模式 46 passed / 开模式 45 passed+1 deselect。
  （入场时发现文件里 isAi 参数化残缺成 `kind`/`want` 未定义——
  展开为 8 组固定值用例后入库。）
- 质量门：cargo fmt/clippy 干净；cargo test -p gd3-core -p gd3-net
  -p gd3-servers 0 失败。
- 双模式全量回归（25 开关；--ignore=android_pack_import
  -p no:randomly）：
  关 → **3 failed / 1286 passed / 8 skipped / 34 errors**
  （1240+46 新 parity 用例，严丝合缝）；
  开 → 3 failed / 1283 passed / 11 skipped / 34 errors；
  FAILED+ERROR 名单（37 条）与第 20 步基线 p20_off_names.txt
  逐条 diff **为空**；开模式 +3 skip 形态与第 20 步记录一致。
  失败 3 条为既有基线（android_config_parity / github_probe /
  runtime_install）。
- 开关清单（**25 个**）：GD3_RUST_{PERSIST, HTTP, ARIA2, BROWSER,
  FILENAME, PROBE, PROBE_SIZE, HF_TREE, HF_FILES, HF_URL, HF_HEAD,
  GITHUB, GITHUB_PROBE, BILI_PARSE, BILI_URL, BILI_PAGES, BILI_VIEW,
  BILI_SIGN, BILI_PLAYURL, BILI_SUBTITLES, ED2K_LINK, ED2K_PROGRESS,
  HTTP_MATCH, M3U8_OUTPUT, M3U8_PARSE}。
- 下一步：其余纯字典/字符串变换小块扫尾（逐个评估可对齐性）；
  之后 **Phase 7 Slint UI** 与 `/root/Ghost-Downloader-3/AGENT-DONE`
  （进度文件完结条件）。

## 第 21 步：ftp URL → FtpConnectionInfo 纯变换下沉（GD3_RUST_FTP_URL，默认关）【完成】
（2026-10-17 04:05 收口；HEAD=f370b7c）
- 范围：只下沉 `ftp_pack/pack.py::FtpParser.parse` 的
  urlparse → unquote → PurePosixPath → FtpConnectionInfo **纯变换段**；
  FTP 连接/枚举网络 I/O、文件层、toSafeFilename、重命名留 Python。
  urlparse 的所有 `ValueError`（Invalid IPv6 URL、bracketed host 双校验、
  port cast/range、NFKC netloc）由核心层逐字文案返回、桥直接抛
  `PyValueError`——与原生传播路径一致（parse 外层无 try/except 吞错）。
- 核心层（`13db908`/`4cc841cb`）：`rust/crates/gd3-net/src/ftp.rs`
  （~713 行）+ `pub mod ftp;`。语义逐条对照磁盘 CPython 3.13
  （`/usr/lib/python3.13/urllib/parse.py`）+ 探针实测：
  1. `urlsplit` 仅左端去 C0+空格、`\t\r\n` 全删；`_check_bracketed_netloc`
     只在 `[`+`]` 同时入 netloc 时调用；`_checknetloc`：非 ASCII 且
     去 `@:?#` 后 NFKC 归一化**变了值**且归一化含 `/?#:@` 才报错
     （ASCII fast-path；所以 `host:21℀` 报 NFKC 错而 `host:21℄` 落到
     port cast 报错、`hⒶost` 正常通过 lower 为 `hⓐost`）。
  2. `_userinfo`=`rpartition('@')`+首次 `partition(':')`；**userinfo
     空串原样保留 ''**，`anonymous`/`anon@` 兜底在 pack.py sink 端
     `or` 完成（第 17 轮曾误内嵌核心层，第 18 轮冒烟发现后修正——
     `or_empty` 函数移除，测试断言随迁）。
  3. `hostname` 首次 `partition('%')` 头段 lower、zone 原样
     （'H%6Fst' → 'h%6Fst'：'%6F' 原样不动）。
  4. `valid_ipv6` 算法级复刻 ipaddress：zone=`partition('%')`（空或含
     第二个 `%` 非法）、45 字符限剥离 zone 后、冒号 ≥9 非法、嵌入
     IPv4 占 2 段（IPv4Address 严格解析：ascii digit、≤3 位、零前缀
     禁、≤255）、内部空段=压缩 `::` 至多一个、skip 分支 endpoint 空
     段降 0、非 skip 恰 8 段首尾非空、hextet 1..4 ascii hex。
  5. `unquote` per-ASCII-run：按 `_asciire` 切段独立 utf-8 replace
     解码、非 ASCII 段原样穿过（`%C3%A4%ee%` 混排 UTF-8 序列不跨界
     合并 → '/\ufffd%'）。
  6. port：ascii digits 才收、Ok(0)→default（`or` 假值语义）、
     hasPort=有显式端口段。
  gd3-net --lib **73 passed**（10 个 ftp 用例）。
- 桥（`c14fe8b`）：`gd3_py.ftp_source(url, defaultPort)` → 7 元组
  `(scheme, host, port, username, password, sourcePath, hasPort)`，
  Err 直接 PyValueError；lib.rs 注册，maturin 重建 OK。
- 适配器 + 接线（`ebfe8b4`）：`features/ftp_pack/rust_url.py`
  （isEnabled env 动态 + `source(url, defaultPort)` 透传）；
  `pack.py` 抽模块级纯函数 `parseFtpSource(url, defaultPort) -> tuple`
  （原生分支=原 urlparse+unquote+PurePosixPath+`or` 兜底**行序逐行
  一致**；Rust 分支=适配器 7 元组 + sink 端同样的 `or` 兜底行），
  `parse()` 首段替换为解包 + 局部
  `sourcePath = PurePosixPath(sourcePathStr)` 构造 FtpConnectionInfo，
  其余（stat/list/REST 探测/task 构造）不动。冒烟：31 用例双模式
  与原生先验零差异（正常值/错误文案）。
- parity（`f370b7c`）：`tests/test_ftp_url_rust.py`——61 用例
  （36 outcome + 20 error 逐字锚点 + 接线 + 默认关 + 桥原样契约）。
  关键先验修正过程：①初版 want 手写有 22 处笔误，脚本对原生核正；
  ②桥原样值（''/'anon@' 前）≠ parse 分派的兜底值——双侧统一走
  `parseFtpSource`（runBoth 内部 delenv/setenv 分派含 or 兜底），
  另设 `test_bridge_raw_contract` 锁桥契约；③`FTP://HOST/X Y#Z`
  的 path 是 `/X Y`（fragment 截断）等。关/开模式各 61 passed。
- 质量门：cargo fmt/clippy 干净；cargo test gd3-core/net/servers
  22 套件 0 失败。
- 双模式全量回归（26 开关；--ignore=android_pack_import
  -p no:randomly）：
  关 → **3 failed / 1347 passed / 8 skipped / 34 errors**
  （1286+61 新 parity 用例严丝合缝）；
  开 → 4 failed / 1343 passed / 11 skipped / 34 errors；
  FAILED+ERROR 名单（关 37 条）与第 20 步基线 p20_off_names.txt
  逐条 diff **为空**；开模式多出的
  `test_engine_listeners.py::test_stopping_the_engine_releases_both_ports[uvloop]`
  独立复跑 1 passed——即第 19/20 步已定性的本机端口释放偶发，未追。
  失败 3 条为既有基线（android_config_parity / github_probe /
  runtime_install）。
- 开关清单（**26 个**）：第 20b 步 25 个 + `GD3_RUST_FTP_URL`。
  基线文件：`$PI_SCRATCH_DIR/p21_off_names.txt`（关）、
  `p21_on_names.txt`（开）、p20_off_names.txt（关基线）。
- 已知偏差（不阻塞）：希腊 final sigma lower 语境规则未复刻（Σ 恒
  →σ，非 ASCII 主机名不出现在 ftp 流量）；isdigit 只认 ascii 数字
  （与 Python 非 ascii 数字同为 cast error 殊途同归）。
- 下一步：其余纯变换小块扫尾（如有）；之后 **Phase 7 Slint UI**
  与 `/root/Ghost-Downloader-3/AGENT-DONE`（进度文件完结条件）。

## 第 22 步（2026-10-17 04:33 当期 date；本步含两件事）：纯变换下沉扫尾评估【完成=无新下沉项】+ Phase 7a Slint UI 脚手架【完成】

### A. 剩余纯变换小块扫尾（结论：**无新下沉项，变换层下沉到此收口**）

第 19 轮扫描候选逐条评估（对照 app/ 与 features/ 全量 Python 面）：

1. **http_pack pack.py:144-164 filename 提取**——依赖 email 库
  （`collapse_rfc2231_value`/`parse_qs`）的重型 stdlib 语义
  （RFC2231 charset/language 合成、编码参数合并），算法级复刻成本
  远超其行数价值 → **留 Python**。
2. **yt_dlp_pack parse_qs 小工具**——`urllib.parse.parse_qs` 变体，
  边际价值低（其余 yt_dlp 面早已历史排除）→ **留 Python**。
3. **jack_yao**——纯网络目录拉取逻辑，非纯变换，不属于本阶段口径
  （Phase 6 只收纯变换）→ **排除**。
4. **bittorrent/mpegdash**——历史排除项，维持不动。
5. **UI 伴生（hf setting_cards 等）**——不剥离，随 Phase 7 UI 阶段
  一并按 Qt→Slint 逐页迁移。
6. **cfg/UserConfig.json 伴生**——决定随 UI 阶段中期走，不提前。

**判据**：纯变换 + 调用高频/值大 + stdlib 依赖轻。以上候选至少欠
两条，与「吃对齐成本 > 收益」的既有原则（email/NFKC/behave 系列
实证过三次）一致。26 个 `GD3_RUST_*` 影子开关为 Phase 6 终态。

### B. Phase 7a：`rust/crates/gd3-ui` Slint UI crate 脚手架【完成，commit 7d7cfd9】

- **slint 选型**：v1.18.1（registry 已缓存），GPL-3.0-only 许可路径
  （项目 GPLv3 兼容）；默认特性含 femtovg/software renderer/
  accessibility/system-tray。
- **脚手架内容**（新 crate `rust/crates/gd3-ui`，workspace members
  已注册）：
  - `ui/appwindow.slint`：AppWindow 窗口骨架 + `in property <string>
    statusText`，竖排容器里一个状态文本 Text。
  - `build.rs`：`slint_build::compile("ui/appwindow.slint")`。
  - `src/lib.rs`：`slint::include_modules!()` + 无头平台 +
    `pub fn create_headless() -> AppWindow` + 无头冒烟测试。
- **slint 1.18.1 API 实证关键点**（修正初稿 8 个编译错误的结果，
  registry 源码行级核对）：
  - `set_platform(platform: Box<dyn Platform + 'static>) ->
    Result<(), SetPlatformError>`（i-slint-core platform.rs:259）；
    **1.18 没有 `is_using_platform_set`**——用
    `let _ = set_platform(Box::new(...))`，`AlreadySet` 即复用信号。
  - `Platform` 唯一必需方法
    `create_window_adapter -> Result<Rc<dyn WindowAdapter>,
    PlatformError>`（platform.rs:30）。
  - `MinimalSoftwareWindow::new(repaint_buffer_type:
    RepaintBufferType) -> Rc<Self>`（i-slint-renderer-software
    minimal_software_window.rs:24，自身已包 Rc，不要再包一层）。
  - `RepaintBufferType` 变体：`NewBuffer`（#[default]）/`ReusedBuffer`
    /`SwappedBuffers`（partial_renderer.rs:380）；导出路径
    `slint::platform::software_renderer::{MinimalSoftwareWindow,
    RepaintBufferType}`（slint lib.rs:488，feature `renderer-software`
    默认开）。
  - 属性 getter 保留 slint 属性原始命名：`get_statusText`/
    `set_statusText`（snake_case 的 `get_status_text` 不存在）。
- **系统依赖**：fontconfig 编译缺失败已解（apt 装 pkg-config、
  libfontconfig1-dev、libfontconfig1）；UTF-8 源码注释注意 build
  输出本地化噪音无害。
- **质量门**：`cargo test -p gd3-ui` 1 passed（无头实例化 + 属性
  读写往返）；`cargo fmt --all --check` 干净；
  `cargo clippy -p gd3-ui --all-targets -- -D warnings` 干净。
- **工具链注**：编辑工具 Write/Edit 常规使用；Edit ops 头 `PUT N.=M:`
  语法在本轮再现解析失败（两次），改用 legacy old_string/new_string
  通过——后续小改以该形式为主。

### C. Phase 7 后续细切计划（下轮起）

- 顺序：**shell（窗口/系统托盘）→ 任务列表页 → 设置页 → 下载引擎
  状态桥接**（复用 gd3-core/gd3-net/gd3-servers，状态经 Rust 侧
  model 推给 Slint）；逐页 strangler，每页独立可回退。
- 量纲：app/view 共 11211 行、app 全部 19458 行 Qt 代码；`app/
  engine.py` 74 行、`loader.py` 140 行先接。
- cfg 与 account 数据伴生迁移放在列表页/设置页落地之后。
- 每页迁完跑 gd3-ui 冒烟 + fmt/clippy；全部 UI 迁移完 → Python UI
  文件停用（不是删除），写 `/root/Ghost-Downloader-3/AGENT-DONE` 结案。

## 第 23 步（2026-10-17 04:42；Phase 7b 起步）【完成】

### 任务列表页数据层切片（commit 2f7467f）

按第 22 步计划的「shell → 任务列表 → 设置 → 引擎桥接」顺序，本轮先做
任务列表页的**数据层骨架**——先固化行数据契约，Qt 卡片/图标/进度环视觉
后置：

- **ui/task_page.slint**：
  - `export struct TaskRow { taskId, name, statusText, progressText }`
    （对齐 app/view/pages/task_page.py 与 app/view/cards/task_cards.py
    的最小展示集）。
  - `TaskPage`：`in property <[TaskRow]> tasks` + `searchText`（占位，
    cfg 伴生迁移中期生效）+ VerticalBox 里搜索占位 Text 与 ListView。
  - 文案（状态/进度/速度）由调用方给定，Slint 侧不格式化——与
    「兜底不在核心层」同一分层原则。
- **ui/appwindow.slint**：挂 `in property <[TaskRow]> tasks` 并透传给
  内嵌 TaskPage。
- **src/lib.rs**：`pub fn set_task_rows(&AppWindow, Vec<TaskRow>)`
  ——Rust 侧 model 装配（ModelRc + VecModel）；无头冒烟测试从 1 增
  **2 passed**（行模型推送 → get_tasks() 逐行 round-trip）。

### slint 1.18 语法/API 实证（本步新增，行级核对）

- **ListView 内容**不是 `delegate X {}`（`delegate` 不是 1.18 关键字，
  解析错定位在同列），而是 `for row in model: Element { }` 重复元素
  （compiler 自带 qt/combobox.slint:79 用例佐证）。
- **布局**：`VerticalLayout/HorizontalLayout` 是 slint 语言内置类型、
  **不导出自 std-widgets**；std-widgets 里对应导出名是
  `VerticalBox/HorizontalBox`（导错名报 "No exported type"）。
- **跨文件 struct 导入**须显式列出：`import { TaskPage, TaskRow }
  from "task_page.slint"`——只 import 组件时对方 struct 报
  "Unknown type 'TaskRow'"。
- **生成 Rust 代码**：模型属性 setter 无返回值（`fn set_tasks(&self,
  ModelRc<TaskRow>)`，build 产物 appwindow.rs:1840 实证；此前猜的
  `SetPropertyError` 不存在于 crate 根）；数组属性 getter 直接回
  `ModelRc<TaskRow>` 无 Option 包装；逐行读需 `use slint::Model`
  （row_count/row_data 是 trait 方法），字段名保留 slint 命名
  （`first.taskId`）。

### 下一步（Phase 7b 续）

1. gd3-core `TaskRecord` → `TaskRow` 的 Rust 侧映射（含 gd3-ui
   依赖 gd3-core、状态文案/进度文案的语义口径对齐 task_cards.py 的
   FieldSpec/formats——先 Python 实测再镜像）。
2. 之后任务列表的交互回调（暂停/删除/分类筛选）逐个接。

## 第 24 步（2026-10-17 04:52；Phase 7b 数据映射）【完成】

### TaskRecord → TaskRow 的 Rust 侧映射（commit a9b7d1c）

gd3-ui 首次接入 gd3-core 依赖，UI 侧文案语义从 Python 先验镜像：

- **src/format.rs**：`to_readable_size(f64)` / `to_readable_time(i64)`
  算法级镜像 `app/format.py`。Python 先验字面量（10-17 实测：
  `toReadableSize(0/1/512/1024/1048576/2**40/1536/1.5*1024³)` →
  "0.00 B"/…/"1.00 TB"/"1.50 KB"/"1.50 GB"；`toReadableTime(0/59/60/125/
  3900)` → "0s"/"59s"/"1m0s"/"2m5s"/"1h5m0s"）全部锁进测试。
- **src/row.rs**：
  - `to_size_text(&TaskRecord, received)` 镜像
    `task_cards.py::toSizeText`：COMPLETED 且 fileSize>0 → 纯大小；
    否则「已收/总量」；fileSize=0 → "已收/--"。
  - `to_status_text(&TaskRecord, speed, received, is_seeding)` 镜像
    `task_drawer.py::DetailCard._toStatusText`：RUNNING →
    "速度/s · 已收 / 总量 · eta"（join " · "，eta = int 截断），
    completed → 做种中/已完成、failed → 失败、paused → 已暂停、
    缺省 → 等待中。
  - `to_task_row` / `to_task_row_with`：复用 gd3-core
    `TaskRecord::current_snapshot()`（不重算），装配 TaskRow。
  - **isSeeding 分层实证**：`Task.isSeeding` 为运行时属性、
    tasks.jsonl 记录无此字段 → COMPLETED 文案由调用方经
    `is_seeding` 传入，缺省 `to_task_row` 恒 false。
- **serde 字段名经测试锁定**：TaskRecord/TaskStep 均
  `rename_all = "camelCase"`（taskId/fileSize/url/name/packId、
  steps[].stepIndex/progress/speed/receivedBytes），TaskStatus
  serde 为 UPPERCASE——测试用 serde_json `json!` 以磁盘同态构造，
  8 测试一次全绿（含整行装配用例：speed 512+512 / received
  384+128 → "1.00 KB/s · 512.00 B / 4.00 KB · 3s"）。

### 工具坑（本步实证）

- **Edit ops 语法再澄清**：此前连续失败的真正原因是
  `PUT N.=M:`（冒号结尾）后面**必须跟至少一行 `+` 正文**——
  「字面替换 + 改行内容」不能用旧 `:content` 单行形式，需写
  `PUT N.=N:` 然后 `+ 新行`；`CUT` 单独一行时冒号可带可不带。
  legacy old_string/new_string 形式在本环境直接报 `ops required`，
  **不可用**。整块替换宁多给一行 `+` 也比挨个试省事。
- **替换范围要含括号**：`PUT N.=M` 是**含首尾**替换；漏掉块尾
  括号会残留孤立 `)`，rustfmt 会直接解析错。修完若 fmt --check
  报 Diff，先重读该处再修，不要盲跑 `cargo fmt`。
- 本步写入经重读核对，未出现杂字注入（write()- 注入问题仅见
  于 write()，Edit 未再复现）。

### 质量门

`cargo fmt --all --check` 干净、`cargo clippy -p gd3-ui
--all-targets -- -D warnings` 干净、`timeout 1200 cargo test -p
gd3-ui` 8 passed（smoke 2 + format 2 + row 4）。

### 下一步（Phase 7b 续）

1. 把 `row::to_task_row` 接进推流路径：调用方（桥接层）持
   `Vec<TaskRecord>` → `set_task_rows` 的端到端通路，无头测试从
   records 构造到 getter round-trip。
2. 列表页交互回调逐个接：暂停/删除/分类筛选。
3. 之后设置页 → 引擎状态桥接。

## 第 25 步（2026-10-17 05:00；Phase 7b 推流路径）【完成】

### records → rows → model 装配一条龙（push_task_records）

- **src/lib.rs** 新增 `pub fn push_task_records(&AppWindow,
  &[TaskRecord], is_seeding: impl Fn(&TaskRecord) -> bool)`：
  `gd3_core::task::TaskRecord` → `row::to_task_row_with` →
  `set_task_rows` 的端到端推流桥梁，`is_seeding` 决策留给调用方
  （isSeeding 是运行时属性，维持第 24 步分层不变）。
- smoke 新增端到端测试
  `push_task_records_shapes_records_into_rows_headless`：磁盘同态
  JSON 构造 RUNNING（steps 512+512/384+128）+ COMPLETED 两条记录
  → push（tsk_b 判做种）→ getter 逐行核对 statusText=
  "1.00 KB/s · 512.00 B / 4.00 KB · 3s" / "做种中"、progressText=
  "512.00 B/4.00 KB" / "1.00 MB"。**9 passed**（8→9）。
- 质量门：`cargo fmt --all`（小改）、clippy -D warnings 干净、
  `timeout 1200 cargo test -p gd3-ui` 9 passed。

### 工具坑（本步实证）

- **CUT/PUT 行号定位必须先 Read 拿准确行号**：凭旧 review 快照
  猜「块尾 } 在 105.106」实际删到的是测试尾部两条 assert，
  被迫两轮回滚修正。教训：写 ops 前一律 Read 尾部拿活行号，
  且 confirm 的 review diff 显示的顺序（add 在后）不代表文件
  实际行序。


## 第 26 步（2026-10-17 05:21；Phase 7b 列表页筛选语义 + 交互回调）【完成】

### 筛选语义 Rust 侧镜像（filter.rs，纯变换）

- `FilterMode{All,Active,Completed}` + `from_route`（镜像
  `ROUTE_TO_FILTER`，task_page.py:45-47 三字面量 all/active/
  completed；未知字面量容错回 All）。
- `matches(TaskStatus)`：ACTIVE = RUNNING ∪ WAITING ∪ PAUSED、
  COMPLETED = COMPLETED ∪ FAILED（镜像 FILTER_TO_STATUSES，40-41
  行）；All 全过。
- `TaskFilter<'a>{mode, category, search}` + `filtered()`：
  category 非空 → `category == filter`（Python 精确相等，None 不等于
  任何字符串）；search 非空 → name/url 小写化后子串匹配；条件叠加
  mode ∩ category ∩ search；**保持 records 原顺序**（Python 列表推导
  语义），纯变换不动排序——排序（SortField 五种）本次未迁，后续步接。
- 测试 3 个：五态 × 三模式状态集合表、路由字面量、组合筛选保序。

### UI 契约（task_page.slint / appwindow.slint）

- TaskPage 增 `filterMode: "all"`、`searchPlaceholder: "搜索任务"`；
  回调 `searchChanged/filterModeChanged/taskPause/taskDelete(string)`。
- 三个段按钮（全部/进行中/已完成）clicked 发路由字面量；LineEdit
  `edited + accepted` 双触发 `searchChanged`；**Slint 只发字段面量，
  执行全在调用方**（与「兜底不在核心层」同一分层原则）。
- filterMode 属性由调用方回写（供后续段高亮）；Text 调试行当前显示
  该属性，段高亮视觉在后续步展开（Qt 侧是 SegmentedToggleToolWidget）。
- AppWindow 同名四回调透传至根。

### 装配（lib.rs）

- `refresh_task_rows(window, records, TaskFilter, is_seeding)`：
  筛选 → `row::to_task_row_with` → `set_task_rows`。
- `TaskActions{search, filter_mode, pause, delete: Box<dyn Fn(&str)>}`
  + `install_task_actions(&AppWindow, Rc<TaskActions>)`：经生成回调
  `on_searchChanged/on_filterModeChanged/on_taskPause/on_taskDelete`
  注册（生成名保留 slint camelCase，build 产物 appwindow.rs 实证）。
- 冒烟 2 个：ACTIVE 筛选后仅剩可运行行 + 搜索不命中空列表；四回调
  注册不 panic（无头无法模拟点按，真实触发留屏上联调）。
- **14 passed**（9 → 14：+3 filter 纯变换、+2 冒烟）；fmt --all
  --check 干净、clippy -p gd3-ui --all-targets -D warnings 干净。

### 工具坑（本步实证）

- 本步再度踩「Edit 正文行漏 `+` 前缀」——ops 里 context 式续行会被
  解析成新 op 报 `unknown operation ///`；每行都要 `+` 开头。
- 一轮 install_task_actions 的整块替换把行序打乱（旧两行 + 新两行
  交错，`delete` 在绑定前使用），**编译期才暴露**；重读活文件定位
  后用精确行号 PUT 修净。再次确认：改完必须 Read 活文件核对行序，
  review diff 不显示交错。
- 文件尾部发现历史遗留的孤立 `}`（上轮 PUT 误删收括号残留），测试
  编译意外闭符报错后删除；`CUT` 单行用 `CUT N.=N`（裸 `CUT N` 报
  invalid locator）。
- 空数组断言 `assert_eq!(expr, [])` 类型推断失败（E0282），改
  `Vec::<&str>::new()`。
- 闭包参数 `taskId` 非 snake_case 警告，与 `-D warnings` 冲突：改名
  `task_id`（slint 生成回调签名不受影响，参数名是 Rust 侧自由变量）。

### 下一步（Phase 7b 续）

1. 分类筛选菜单（categoryFilter 属性，伴生 cfg.isCategoryEnabled +
   分类列表数据源），补齐 _refreshList 的 category 入口。
2. 段高亮/卡片视觉（TaskRow 展开进度环、状态色，对齐 task_cards.py）。
3. 暂停/删除动作接 task_service（桥接层：TaskActions 的 pause/delete
   落到 gd3-core 的暂停/删除语义 + 磁盘回写），然后设置页 → 引擎
   状态桥接。

## 第 27 步（2026-10-17 05:37；Phase 7b 分类筛选切片）【完成】

### 分类筛选菜单的 Rust 侧镜像（filter.rs）

- `CategoryRef{category_id, name}`：Python `Category`（category_service
  .py:102）只取 UI 菜单消费的两字段（展示名经 toCategoryName）。
- `menu_names()`：ComboBox 的 [string] 模型 = 「全部分类」表头
  （allAction，task_page.py:_rebuildCategoryFilterMenu）+ 各分类名，
  **索引序即菜单序**。
- `category_id_at(index)`：0 → ""（表头）、其余取
  `categories[index-1].categoryId`、越界/负值防御性回 ""。
- `sanitize_category_filter(current)`：镜像 `_rebuildCategoryFilterMenu`
  尾容错——当前过滤值不在合法集合（含 ""）内 → 回落 ""（分类被删
  场景），调用方需据此重刷。
- 测试：表头+两分类序列、索引映射 0/正/越界/负、失效清理三种。

### UI 契约（task_page.slint / appwindow.slint）

- TaskPage 增 `categoryNames: [string]`、`categoryEnabled: false`、
  `categoryIndex`（in-out，选中态由调用方回写管理）；回调
  `categoryFilterChanged(int)`。
- ComboBox `current-index <=> categoryIndex`、
  `visible: categoryEnabled`、`selected => categoryFilterChanged(
  self.current-index)`——**std ComboBox base 只支持 [string] 模型**
  （combobox-base.slint:5 model:[string] + 16 selected(string) +
  10 current-index），发索引而非 struct 是经实证的取舍；索引映射
  全在 Rust 侧 helper，Slint 保持哑管道。
- AppWindow 属性透传 + categoryIndex `<=>` 双向 + 回调透传。

### 装配（lib.rs）

- `set_category_names(window, Vec<String>)`：**生成 setter 要
  `ModelRc<SharedString>`，`ModelRc<VecModel<String>>` 直接塞会
  E0308**——helper 内 String→SharedString 转换（生成名实证
  set_categoryNames 保留 camelCase）。
- TaskActions 增 `category_selected: Box<dyn Fn(i32)>`（第五动作，
  回调参数 i32 是 Copy，**不能 `&index`**——E0308 实证）。
- smoke：menu_names → set_category_names → get_categoryNames 3 行
  且第 2 行「视频」、set/get_categoryIndex round-trip。
- **16 passed**（14 → 16：+1 filter 分类测试、+1 分类冒烟）；fmt
  --all --check、clippy -p gd3-ui --all-targets -D warnings 干净。

### 工具坑（本步实证，Edit ops 三连坑）

- **PUT N.=N: 尾行被吞**：替换范围含首尾，`+` 正文里若不把被替换
  行重抄一遍，该行就真没了——本轮先后吞掉「已完成的收 }」（slint
  工具栏）、filterMode 属性行、TaskPage { 开行、let 绑定行等。
  教训：**PUT 整块替换时，凡想保留的行必须在 + 正文里显式重写**；
  写完 Read 活文件核对，review diff 依旧会漏交错/漏吞行。
- 大块替换（46 行）后 review 只显示局部 hunk、加删计数对不上——
  文件实际被写成正确内容，但只能靠全文件 Read 复核认可。
- 交错残骸：一条 PUT 插入新块 + 旧块尾部同时存留（旧测试尾部
  5 行悬挂、双 `}`、`let pause` 重复），编译报 E0308 后逐个用
  精确行号 CUT/PUT 清除；`CUT` 单行 `CUT N.=N` 稳定可用。

### 下一步（Phase 7b 续）

1. 段高亮/卡片视觉：TaskRow 展开进度环/状态色，对齐 task_cards.py；
   filterMode 段按钮高亮（Qt SegmentedToggleToolWidget 等价物）。
2. 暂停/删除接任务动作真实语义（bridging 层：TaskActions 的
   pause/delete 落 gd3-core），列表排序（SortField 五种 + 升降序）
   可顺路先行（纯变换、零 UI 依赖）。
3. 之后设置页 → 引擎状态桥接 → Python UI 停用 + AGENT-DONE。

## 第 28 步 — 列表排序切片（SortField 五分支 + 排序 ComboBox×2 + refresh 串接）

### 改动

- **rust/crates/gd3-ui/src/sort.rs（新，~295 行）**：镜像
  `task_page.py::_refreshList` 末段（614-639 行）五个排序分支。
  - `SortField{CreatedAt,Name,Size,CompletedAt,QueuePosition}`（Default=
    CreatedAt）+ `from_menu_index(i32)`：菜单显示序 0=按添加时间/
    1=按完成时间/2=按名称/3=按大小/4=按队列，越界容错回 CreatedAt。
  - `sort_refs(&mut Vec<&TaskRecord>, field, ascending, waiting_order)`：
    - CreatedAt/Name（键 `t.name.lower()`）/Size：直接稳定排序。
    - CompletedAt：按 completedAt≠0 拆两组，各按 completedAt/createdAt
      排，desc 拼 pending+completed、asc 反过来（Python 632/636 行）。
    - QueuePosition：running(原序)/waiting(在队列列表里)/rest 三组；
      **waiting 恒按传入队列序升排、rest 恒按 createdAt 倒排
      （Python 写死 reverse=True，不受 ascending 影响）**，仅组间
      拼接随 ascending 翻转（desc=running+waiting+rest，asc 相反）。
  - 稳定性：Python `list.sort` reverse 下相等元素保原序 ↔ Rust 稳定
    sort_by_key + 反比较器等价（clippy 必看：**降序分支会被 `-D
    warnings` 强制改 `sort_by_key(key+Reverse)`——普通的
    `sort_by(|a,b| key(b).cmp(&key(a)))` 报 clippy::sort_by_key**）。
- **ui/task_page.slint / appwindow.slint**：排序两个 ComboBox——
  字段 `["按添加时间","按完成时间","按名称排序","按大小排序",
  "按队列顺序"]`、顺序 `["顺序","倒序"]`，缺省 fieldIndex=0、
  orderIndex=1（Python 缺省 CREATED_AT+倒序）。`[string]` 模型发
  selected 索引，复用第 27 步 ComboBox 模式；两回调 sortFieldChanged/
  sortOrderChanged(int) 双向透传。
- **lib.rs**：`refresh_task_rows` 增 `sort_field/ascending/waiting_order`
  三参，串接「筛选（保序）→ sort::sort_refs → 成行 → 装配」；
  TaskActions 增 `sort_field/sort_order: Box<dyn Fn(i32)>`（六动作）；
  install_task_actions 注册 on_sortFieldChanged/on_sortOrderChanged。

### 验证

- `cargo test -p gd3-ui`：**16 → 22 passed**（sort 纯变换 4 + 冒烟
  2 新增：sortFieldIndex/sortOrderIndex 缺省与 round-trip、【筛选→
  Name 倒序→装配】三记录链路）。0 failed。
- `cargo fmt --all --check`、`cargo clippy -p gd3-ui --all-targets
  -- -D warnings` 干净。

### 本步修的两个测试错误（产品代码一直是对的）

1. 队列测试数据：record helper 的 taskId = `tsk_{name}` **含扩展名**
   （`tsk_wait2.mp4` 不是 `tsk_wait2`）——队列列表漏文件名后缀导致
   WAITING 记录查不到 waitingIndex、全部落进 rest 组，输出
   `run1, rest_new, wait2, wait1, rest_old` 与 torn-down 断言完全吻合。
2. 升序期望错位：测试曾期望升序时 rest 组重建为 createdAt 升——
   权威源核对（task_page.py:634）rest 组**恒** `reverse=True`，仅组间
   拼接翻转；期望改为 `[rest_new, rest_old, wait2, wait1, run1]`。

### 工具坑（续记）

- PUT 整块替换第三次吞行：260.=263 替换注释+队列 vec 时把紧随的
  `sort_refs(...)` 调用行一起吞掉（del 列表可见、编译期才暴露）。
  规则再次确认：**PUT 范围内想保留的行必须在 + 正文显式重写**。
- python 替换脚本 edit 锚点缩进不一致（12 vs 8 空格）→ assert n==1
  中止、文件零损；修锚点重跑即可——脚本模式的这个安全网很好用。
- clippy needless_lifetimes：`sort_refs<'a>(&mut Vec<&'a TaskRecord>)`
  单生命周期可省（elide 后语义不变）。

### 卡点

无

### 下一步（Phase 7b 续）

1. 段高亮/卡片视觉：TaskRow 展开进度环/状态色（对齐 task_cards.py）；
   filterMode 段按钮高亮。
2. 暂停/删除接 gd3-core 真实语义（bridging 层）。
3. 之后设置页 → 引擎状态桥接 → Python UI 停用 + AGENT-DONE。

## 第 29 步 — TaskRow 结构化渲染字段（status 字面量 + progress 直通）

### 改动

- **ui/task_page.slint**：`TaskRow` 增两个结构化字段——
  - `status: string`：磁盘持久化枚举名（TaskStatus.name 口径），
    供状态色点与控件可见性派发；
  - `progress: float`：0-100 百分比。
  - 行渲染升级：状态色点（RUNNING=#3477FF/COMPLETED=#30BE60/
    FAILED=#F5403C/PAUSED=#FFC107/其它 #858585，**临时色、主题对齐
    留给后续视觉步**）+ std `ProgressIndicator`（仅 RUNNING 显示、
    `progress: row.progress / 100` 换算 0-1 契约），镜像 Python 卡片
    「RUNNING 走进度条、其余走 statusLabel」分发（task_cards.py
    295-332 行 refresh 分支 + 267 行 `setValue(int(progress))`）。

### slint/核心契约实证（本步新增）

- **std-widgets ProgressIndicator 契约 0-1**：cupertino stub
  progressindicator.slint 实证 `accessible-value-maximum: 1.0`、
  宽度按 `progress` 比例——Python QProgressBar 是 0-100，直通前要
  `/100`（gd3-core `current_snapshot()` 的 progress 也是 0-100，
  `(sum(step.percent)/len)` 语义，两端一致）。
- **slint `float` 是 f32**：`current_snapshot()` 的 f64 直塞 TaskRow
  构造报 E0308 `expected f32, found f64`——加 `as f32` 收窄（0-100
  量级无精度顾虑）。
- `status_literal(TaskStatus)->&'static str`：手写 match 而非 serde
  序列化（TaskStatus serde rename UPPERCASE 与磁盘同名，但手写省
  一次 JSON 往返、且全枚举测试锁定）。

### 验证

- `cargo test -p gd3-ui`：**22 → 23 passed**（+1 status_literal 全
  枚举覆盖；装配测试补 `row.progress==50.0`（steps 30+70 → 均值）、
  `row.status=="RUNNING"`；push smoke 补 status/progress 断言；
  rows round-trip 构造与断言×2 补字段）。0 failed。
- `cargo fmt --all --check`、`cargo clippy -p gd3-ui --all-targets
  -- -D warnings` 干净。

### 卡点

无

### 下一步（Phase 7b 续）

1. filterMode 段高亮（三段按钮选中态视觉 + 派生 bool 供测试锁）。
2. 暂停/删除接 gd3-core 真实语义（bridging 层决策：调用方是谁）。
3. 之后设置页 → 引擎状态桥接 → Python UI 停用 + AGENT-DONE。

## 第 30 步 — filterMode 段高亮（SegmentItem + 单向派生）

### 改动

- **ui/task_page.slint**：
  - 新自绘 `SegmentItem inherits Rectangle` 组件（等价 Python
    SegmentedToggleToolWidget 最小同构）：`label/selected` 两属性 +
    `clicked` 回调 + TouchArea；选中段临时高亮色 #E2E6Ed（主题
    对齐步再调）。三个原 Button 替换为三段实例，仍发
    `filterModeChanged("all"/"active"/"completed")` 字面量。
  - 三高亮属性改 `in`（由窗口层传入消费）；**派生绑定从 TaskPage
    移走**——`filterMode == "all"` 等唯一派生点在 AppWindow。
- **ui/appwindow.slint**：补 `filterMode`（in，缺省 "all"，此前从未
  暴露到窗口层）+ 三高亮 `in-out` 派生属性；TaskPage 三高亮单向
  传入（高亮值随之回读）。
- **lib.rs**：smoke `filter_mode_segment_highlight_follows_route_...`
  锁三组派生：缺省 all → (T,F,F)；set active → (F,T,F)；**未知
  字面量 "weird" → 全不选中**（slint 侧不做容错，容错语义在
  filter.rs from_route 镜像层）。

### 本步实证（新坑）

- **自定义 component 缺基类编译错**：`component SegmentItem {` 不写
  `inherits Rectangle` 时是基元组件，`background/border-radius` 全
  报 Unknown property——自绘组件必须显式 inherits。
- **`<=>` 双链初值竞态**：AppWindow 端 `in-out highlightAll;`（无派
  生绑定）与 TaskPage 端派生绑定经 `<=>` 相接后，窗口端默认 false
  互相覆盖，TaskPage 派生值丢失（测试第一时间失败：缺省高亮==
  false）。修法：**派生绑定收敛到唯一一层**——AppWindow 派生，
  TaskPage 只 `in` 消费，禁止同语义双端派生。
- **内部组件属性不自动上浮到 AppWindow**：第 26 步「filterMode 由
  调用方回写」从未真的可达（窗口层无此属性）——本轮补上。属性/回
  调都要在 AppWindow 显式声明/透传才入生成 API。

### 验证

- `cargo test -p gd3-ui`：**23 → 24 passed**（+1 高亮派生 smoke）。0 failed。
- `cargo fmt --all --check`、`cargo clippy -p gd3-ui --all-targets
  -- -D warnings` 干净。

### 卡点

无

### 下一步（Phase 7b 续）

1. 暂停/删除接 gd3-core 真实语义（bridging 层决策：调用方是谁、
   Rust UI 直执还是桥回 Python 服务）。
2. 之后设置页 → 引擎状态桥接 → Python UI 停用 + AGENT-DONE。

## 第 31 步 — UserConfig.json 存取下沉 gd3-core（config.rs）

### 改动

- **rust/crates/gd3-core/src/config.rs（新，~236 行）**：镜像
  qfluentwidgets `Config.save` 的磁盘格式（协议不变）。磁盘外形
  `{group: {key: "python repr 字符串"}}`——项值经 validator format
  后全为字符串：bool→`"True"/"False"`（**不是** JSON true/false）、
  int→`"3"`、list→`str([...])`。手写 JSON writer 逐字节镜像
  `json.dump(..., indent=4, ensure_ascii=False)`（无尾随换行；
  serde_json pretty 是 2 空格 + 小写布尔，不兼容）。API：
  `load/from_json/save/to_json_text`（非对象组、非字符串值容错跳
  过）+ typed `get/set_str/bool/i64`（缺组自建、缺键/非数字回 None）。
- **fixtures**：`tests/fixtures/user_config_sample.json` +
  `user_config_expected_save.json` 均由 `.venv` Python json.dumps
  (indent=4, ensure_ascii=False) 生成，测试逐字节对照 + 幂等 + 
  磁盘 round-trip + 未触及键原样保留断言。

### 本步实证

- **python repr 口径贯穿**：bool 值串是 `"True"/"False"`；parse 失
  败（如 `"3.0"` float 项）typed 回 None 不猜。
- **键序不名誉**：toDict 走 `dir()` 字母序存初档，之后文件自身顺序
  往返；Rust 写盘放 BTreeMap 稳定排序（语义键集不变，Python 端可
  读回——UserConfig.json 只有 Python 自己消费，不做顺序镜像）。
- **分层**：只做存取与值串镜像，**不做几十项配置盘点**——设置页
  用到哪项、先验哪项，避免双源漂移浪费。
- Edit 工具小坑续记：`PUT 7.`/`PUT 7` 都是 invalid locator，
  正确形式 `PUT <7:`（`<N` 独立locator，不是数字后缀）。

### 验证

- `cargo test -p gd3-core`：全绿（config::tests 2 passed，lib 
  整体 39 passed + 12/2/2/5/1/2/9 集成测试）。0 failed。
- `cargo fmt --all --check`、`cargo clippy -p gd3-core --all-targets
  -- -D warnings` 干净。

### 卡点

无

### 下一步（Phase 7c 续）

1. 设置页（Rust UI）：先验最小键集（GeneralDownload 下载路径/
   并发数/速度限额、Category 开关等）→ 设置页 slint 骨架 + 
   typed 装配。
2. 暂停/删除接真实语义（方案待定：引擎桥接放 Phase 8 一体考虑）。
3. 之后引擎状态桥接 → Python UI 停用 + AGENT-DONE。

## 第 32 步 — 设置页骨架（先验四键）(2026-10-17 06:33)

### 改动

- **ui/settings_page.slint（新，74 行）**：SettingsPage 四键——
  下载路径（LineEdit，edited+accepted 双触发）、最大任务数
  （SpinBox min1 max10）、预分配线程数（SpinBox min1 max256）、
  启用下载分类（Switch）。标题/副标题双行 Text 镜像
  setting_page.py .tr 字面量；控件只回发变化
  （downloadFolderChanged/maxTaskNumChanged/preBlockNumChanged/
  categoryEnabledChanged），落盘/生效全在调用方。
- **ui/appwindow.slint**：import SettingsPage + 四 in-out 属性 +
  四回调 + SettingsPage 实例 `<=>` 透传。
- **lib.rs**：`push_settings(window, config, default_download_folder)`
  typed 读 + `SettingsActions`（folder/max_task_num/pre_block_num/
  category_enabled）+ `install_settings_actions` + 冒烟
  `settings_page_push_and_install_headless`。

### 本步实证（新坑）

- **重复声明不可覆盖**：categoryEnabled 在窗口层声了两次（设置页
  in-out + 任务页原有 in）→ slint 编译错 `Cannot override property
  'categoryEnabled'`。修复=合并为窗口级单一份 in-out：
  Python 端两者本来就是**同一配置键** `cfg.isCategoryEnabled`
  （cfg.py:261 Category/EnableCategory；task_page.py:93/241 只消费
  可见性），设置页 `<=>` 双链写、TaskPage 单向消费。
- **键位以 cfg.py 为准**（四处实证核对）：GeneralDownload/
  GeneralDownload/{DownloadFolder,MaxTaskNum,PreBlockNum}+Category/EnableCategory；缺省
  MaxTaskNum=3、PreBlockNum=8、EnableCategory=False、
  DownloadFolder=str(DOWNLOAD_DIR)（Android 分支除外，调用方传）。

### 验证

- `cargo build -p gd3-ui` 编译通过；`cargo fmt --all --check`、
  `cargo clippy -p gd3-ui --all-targets -- -D warnings` 干净。
- `timeout 1200 cargo test -p gd3-ui`：**24 → 25 passed**
  （+1 settings 冒烟：有值推送、空配置容错、install 注册）。0 failed。

### 卡点

无

### 下一步（Phase 7c 续）

1. SpeedLimitation 卡（KB/s ⇄ 字节 1/1024 换算语义）单独步先验。
2. 其余设置组逐键扩；暂停/删除接真实语义（桥接策略 Phase 8 一体
   考虑）。
3. 之后引擎状态桥接 → Python UI 停用 + AGENT-DONE。

## 第 33 步 — SpeedLimitation 限速卡（2026-10-17 06:44）

### 改动

- **src/speed.rs（新）**：镜像 setting_cards.py `SpinBoxSettingCard`
  division=1/1024 双向语义。配置口径=字节（cfg.py:247-249 键
  GeneralDownload/SpeedLimitation，缺省 4194304，范围 1024-104857600，
  实证核对）；显示口径=KB/s（int(bytes/1024)，范围 1-102400，
  singleStep=512 在显示值上）。
- **ui/settings_page.slint**：第五张卡（标题「下载限速/当下载任务
  界面限速开关开启时，所有任务将根据此值进行限速」镜像 .tr）+
  SpinBox（min1 max102400 step512，`value <=> speedLimitationKb`）+
  独立 Text 后缀 " KB/s"（std SpinBox 无 suffix 属性）。
- **ui/appwindow.slint**：窗口级 `in-out speedLimitationKb`（**单位
  缀名**，防口径混用）+ speedLimitationChanged 回调 + 透传。
- **lib.rs**：push_settings 补限速键（缺省 4194304→显示 4096）；
  SettingsActions 增 `speed_limitation: Box<dyn Fn(i64)>` **收配置
  口径字节**（统一「动作收配置单位」原则，KB→字节换算归 install
  层 speed::from_display_kb——镜像 Python leaveEvent
  `cfg.set(item, int(value/division))`）。

### 本步实证

- **Put 块替换吞行再犯**（第 4 次计）：三处连环——smoke 下一测试
  的 doc 行+`#[test]` 被吞、仍接 dispose 旧字段为孤儿 fn、结构体
  `}`/重复字段错乱。全部靠 Read 活文件核对后自底向上逐 PUT 修复。
  教训：多 op 一次提交时行号参照同一旧状态，编辑区间越长越应拆
  单次单 op 或用更窄锚点。
- slint 生成回调可从 Rust 侧 `invoke_*` 直接调用（无头可验通路，
  「无法模拟点按」不再绝对）。

### 验证

- `cargo build` + `cargo fmt --all --check` +
  `cargo clippy -p gd3-ui --all-targets -- -D warnings` 干净。
- `timeout 1200 cargo test -p gd3-ui`：**25 → 26 passed**
  （+1 speed 换算：缺省 4194304→4096、范围两端 round-trip、
  512→0 不 clamp 镜像锁；smoke 补 8388608→8192 推送 +
  invoke 102400 KB→104857600 字节）。0 failed。

### 卡点

无

### 下一步（Phase 7c 续）

1. 其余设置组逐键扩（HistoryDownloadFolder/nameConflict 等先验
   再迁）；暂停/删除接真实语义（Phase 8 桥接一体考虑）。
2. 之后引擎状态桥接 → Python UI 停用 + AGENT-DONE。

## 第 34 步 — GeneralDownload 组剩余简版六键（2026-10-17 07:02）

### 改动

- **src/conflict.rs（新，49 行）**：镜像 qfluentwidgets
  `ComboBoxSettingCard`（setting_card.py:401-448 实证：addItem 按
  zip(texts, options) 序，索引 i <=> options[i]；Change 时存
  **字面量**）。`OPTIONS=["keepBoth","overwrite","ask"]`、
  缺省 keepBoth；`index_of_option`（未知→0）/
  `option_of_index`（越界→keepBoth）——Rust 镜像层容错，
  不复刻 Python 未知字面量 KeyError 崩溃（同 filter.rs
  from_route 惯例）。
- **ui/settings_page.slint**：六新属性+六回调+五张新卡——
  自动提速 Switch、最小再分配大小 SpinBox（min64 max102400
  step64 + \\" KB\\" 独立后缀 Text）、保留修改时间 Switch、
  验证 SSL Switch、同名文件 ComboBox（model
  [\\"保留两者\\",\\"覆盖\\",\\"询问\\"]，current-index 双链、映射归
  Rust）、使用系统 DNS Switch。**卡序与 Python _initCards
  组内顺序不一一对应（注记，全量重排留主题/布局步）。**
- **ui/appwindow.slint**：窗口级 6 in-out + 6 回调 + 12 行
  透传。
- **lib.rs**：push_settings 尾部 6 组 set（NameConflict 经
  `conflict::index_of_option`）；SettingsActions 增 6 动作
  （统一「动作收配置单位」：reassign_size 收 i64 配置值、
  name_conflict 收配置字面量，索引→字面量换算归 install
  层 `conflict::option_of_index`）。

### 键位实证（cfg.py）

- `AutoSpeedUp` 缺省 True；`MaxReassignSize` 缺省 512 范围
  64-102400（\\" KB\\" 后缀、singleStep=64、division=1 显示=配置值）；
  `PreserveLastModified` 缺省 F；SSL 键名字面**shouldVerifySsl**
  （小写 s 起）缺省 F；`NameConflict` 选项见上缺省 keepBoth；
  Network 组 `ShouldUseSystemDns` 缺省 True（UI 归组 GeneralDownload）。

### 本步实证

- **Put 块替换吞行第 5 次**：多 fn 三处连环（孤儿 fn/缺
  `}`/重复 doc）。修复后又因「删多余 `}`」误删限速卡闭括号，
  编译报 `expected '}'` at 1:1 + 重复属性——**awk 累计深度
  扫描（open 50 close 49）定位**：第 110 行注释前补回
  `        }`。教训：删除前先数括号归属，勿信 review 的
  del/add 展示顺序。
- 一次 Edit 多 op 的行号错位风险再次确认：改走单次单 op 或
  自底向上。

### 验证

- `cargo build` + `cargo fmt --all --check` +
  `cargo clippy -p gd3-ui --all-targets -- -D warnings` 干净。
- `timeout 1200 cargo test -p gd3-ui`：**26 → 27 passed**
  （+1 conflict 映射：round-trip/未知字面量/越界索引兜底；
  smoke 补六 setter 有值推送/缺省容错/invoke_nameConflictChanged(2)=>\\"ask\\"）。
  0 failed。

### 卡点

无

### 下一步（Phase 7c 续）

1. 其余设置组复杂卡先验再迁（ProxySettingCard/HistoryDownloadFolder/
   clientProfileCard）；暂停/删除接真实语义（Phase 8 桥接一体考虑）。
2. 之后引擎状态桥接 → Python UI 停用 + AGENT-DONE。

## 第 35 步 — ProxySettingCard（GeneralDownload/ProxyServer，cfg.py:251 缺省 "Auto"）

- 时间：2026-10-17 07:30（date 实取；代码提交 `4d10d2d`）

### 改动

- `rust/crates/gd3-ui/src/proxy.rs`（新，182 行）：
  - `PROTOCOLS = ["socks4","socks5","socks5h","http","https"]`（addItems 序，
    未知 scheme 落索引 0）；`VALUE_OFF/AUTO/DEFAULT("Auto")`；
    `CHOICE_OFF:0 / AUTO:1 / CUSTOM:2`；`ProxyFields`（derive Default）。
  - `validate` 逐字符移植 `ProxyValidator`（cfg.py:71-91）：`^...$` 锚定、
    可选 `\w+:[\w!@#$%^&*()]+@` 凭据段、host 为 IPv4 点分或域名、端口 1-65535；
    Auto/Off 恒有效（OnceLock Regex）。
  - `parse_fields` 镜像 urlsplit（第一个 @ 切凭据、最后一个 `:` 切 host:port）；
    `build_url` 镜像 `_buildProxyUrl`（凭据仅 user/password 非空才拼 `user:pass@`）；
    `host_valid/port_valid` 移植行内错误正则（setting_cards.py:28-32）。
- `ui/settings_page.slint`（+≈275 行内）：6 in-out 属性
  （proxyChoice/proxyProtocolIndex/proxyHost/proxyPort/proxyUser/proxyPass）
  + 2 回调（`proxyChoiceChanged(int)`、
  `proxyFieldsChanged(int,string,string,string,string)`）+ 新卡。
  - **fluent 风格 std-widgets 不导出 RadioButton（实证）→ 三选单选用
    ComboBox 镜像**：choiceLabel 显示「不使用代理/自动检测系统代理/
    使用自定义代理」，互斥语义等价。
  - 自定义字段 `enabled: proxyChoice == 2`；密码 LineEdit
    `input-type: password` 镜像 EchoMode.Password（实证 lineedit-base）。
- `ui/appwindow.slint`：窗口级 6 属性 + 2 回调 + 8 行透传。
- `src/lib.rs`：`pub mod proxy;`；push 仅在自定义模式解析推送四字段
  （Auto/Off 下自定义字段保持进程缺省空串）；SettingsActions 增
  `proxy_changed: Box<dyn Fn(&str)>` **收配置口径字面量**
  （"Auto"/"Off"/拼装 URL）；install 层：Off/Auto 直取，自定义先
  `proxy::validate`，无效不调用动作不落盘（**镜像 leaveEvent 语义**）。
- `Cargo.toml`：`regex.workspace = true`（Cargo.lock 同步）。

### 语义定案

- 配置口径永远是 `GeneralDownload/ProxyServer` 字面量；UI 发
  choice+四字段，拼装/校验/写决策在 install 层 proxy.rs。
- **已知轻微分歧**：Python 等到 leaveEvent 才写 custom，本实现每次
  变化且有效即写（更新鲜，等价安全）。
- 兼容横幅（依赖功能包 proxySchemes）与 Auto 模式系统代理检测
  （getproxies）非配置键，**留引擎桥接步**。

### 本步实证（工具坑增量）

- `&url` 借用活不过 match → 分支统一返回 String（.to_string())；
  win 被 move → 每闭包独立 clone_strong。
- clippy 4 键修正：未使用 port_regex（删）/derive Default/
  let-chain 合并（edition 2024）/needless_borrow。
- **行号时序漂移 3 次自愈**：多文件并行改后行号过期插错位置
  （253 穿插进闭包、582 插到第三窗口、url_regex 注释被误 CUT）——
  op 后立即 Read 现场 + CUT 重做，无残留。
- CUT locator 必须带 `.=M` 形式（裸 `CUT 253` 报 invalid locator）。

### 验证

- `cargo build` + `cargo fmt --all --check` +
  `cargo clippy -p gd3-ui --all-targets -- -D warnings` 干净。
- `timeout 1200 cargo test -p gd3-ui`：**27 → 28 passed**（+1
  round-trip/端口界/错误正则；smoke 补 socks5://u:p@1.2.3.4:1080
  推送解析 / 缺省 Auto→choice1 / invoke 通路：choice0→"Off"、
  无效端口 70000 不写配置、有效 http://1.2.3.4:8080 拼装、
  choice1→"Auto"）。0 failed。

### 卡点

无

### 下一步（Phase 7c 续）

1. 第 36 步候选：IdentitySettingCard（clientProfileCard，
   setting_cards.py:467 起 TLS 指纹/UA 预设）、HeadersPresetSettingCard、
   HistoryDownloadFolder，或浏览器扩展组/Aria2 RPC 组简版键
   （为 Phase 8 桥接铺路）。
2. 之后暂停/删除接真实语义（Phase 8 桥接一体考虑）→ 引擎状态桥接
   → Python UI 停用 + AGENT-DONE。

## 第 36 步 — Aria2 RPC 兼容组四简版卡（cfg.py:277-280）

- 时间：2026-10-17 07:40（date 实取；代码提交 `531870e`）

### 改动

- `ui/settings_page.slint`：4 in-out（aria2Enabled/aria2RpcPort/
  aria2RpcToken/aria2EmulateFingerprint）+ 4 回调 + 四卡
  （头注释更新为第 32-36 步）。
- `ui/appwindow.slint`：同名 4 属性 + 4 回调 + 12 行透传。
- `src/lib.rs`：
  - push_settings 尾部四组 set（缺省=cfg.py 口径：F/16800/""/F）。
  - SettingsActions 增 `aria2_enabled` / `aria2_rpc_port`
    （收 i64 配置单位，division=1 直接透传）/ `aria2_rpc_token`
    （收原样串，Python 端 cfg.set 不 trim）/ `aria2_emulate_fingerprint`。
  - install 层四注册（RPC 监听真启停等对齐留引擎桥接步）。

### 键位实证（cfg.py:277-280）

- `isAria2RpcEnabled` 缺省 False；`aria2RpcPort` RangeConfigItem
  缺省 16800、RangeValidator(1024, 65535)、singleStep=1、division=1
  显示=配置值；`aria2RpcToken` ConfigItem 缺省 ""（无校验/不 trim）；
  `aria2RpcEmulateFingerprint` 缺省 False。
- 令牌卡镜像 LineEditSettingCard（setting_cards.py:118-145）：
  placeholder「可选」、PasswordLineEdit → `input-type: password`、
  editingFinished 落盘——**已知轻微分歧**（同代理卡）：slint
  edited+accepted 均回发、install 层每次写（更新鲜，等价安全；
  slint LineEdit 无失焦 finishing 事件）。

### 本步实证

- >80 行文件按运营提示走「全量重写（两个 slint）+ 替换脚本
  （lib.rs，9 处锚点全部唯一命中一次通过）」，**本步零 Edit 事故**
  （对标此前 Edit ops 连环坑，实测脚本路线稳定）。
- 无新 widget 类型：SpinBox/Switch/LineEdit(password) 均复用已实证
  模式；settings_page.slint 后缀惯例（本卡无单位后缀）。

### 验证

- `cargo fmt --all --check` + `cargo clippy -p gd3-ui --all-targets
  -- -D warnings` 一次通过。
- `timeout 1200 cargo test -p gd3-ui`：**28 passed**（本步扩展现有
  settings smoke 而非新增测试函数：四键有值推送 Enabled=T/Port=
  10800/Token="secret"/Emulate=T、缺省容错 16800 等、令牌
  invoke_aria2RpcTokenChanged 通路断言）。0 failed。

### 卡点

无

### 下一步（Phase 7c 续）

1. 第 37 步候选：浏览器扩展组五卡（browserPairTokenCard/
   chromiumInstallCard PrimaryPush 需动作骨架、browserPortCard
   SpinBox 1764-65527 先验、browserEnableCard Switch、草稿模式
   Switch），或 GeneralDownload 组收尾（HistoryDownloadFolder/
   clientProfileCard/HeadersPresetSettingCard 复杂卡先验）。
2. 之后暂停/删除接真实语义（Phase 8 桥接一体考虑）→ 引擎状态桥接
   → Python UI 停用 + AGENT-DONE。

## 第 37 步 — 浏览器扩展组简版三卡（cfg.py:268-272）

- 时间：2026-10-17 07:46（date 实取；代码提交 `7046425`）

### 改动

- `ui/settings_page.slint`：3 in-out（browserEnabled/browserDraftTaken/
  browserPort）+ 3 回调 + 三卡（头注释更新为第 32-37 步）。
- `ui/appwindow.slint`：同名 3 属性 + 3 回调 + 9 行透传。
- `src/lib.rs`：push_settings 尾部三组 set；SettingsActions 增
  `browser_enabled` / `browser_draft_taken` / `browser_port`
  （收 i64 配置单位 division=1）；install 层三注册。

### 键位实证（cfg.py:268-272）

- `Browser/EnableBrowserExtension` 缺省 True；`Browser/
  EnableRaiseWindowWhenReceiveMsg` 缺省 False——**键名与 UI 文案
  「接管下载时进入草稿模式」不同源**（映射 shouldDraftTakenDownload
  → 这个键），注记；`Browser/Port` RangeConfigItem 缺省 14370、
  RangeValidator(1024, 65535)、singleStep=1、division=1。
- **动作卡欠账清单**（本组未迁，留动作骨架步）：配对令牌
  PrimaryPushSettingCard（复制令牌+重新生成 ToolButton）、
  storeInstallCard HyperlinkCard（FireFox/Edge/Chrome 三链接）、
  chromiumInstallCard PrimaryPush（一键安装+导出 CRX）。

### 本步实证

- 替换脚本路线第二步：9 处锚点全部唯一命中一次通过；两个 slint
  全量重写。零 Edit 事故，路线稳定复现。
- 无新 widget 类型（Switch/Switch/SpinBox 全复用已实证模式）。

### 验证

- `cargo fmt --all --check` + `cargo clippy -p gd3-ui --all-targets
  -- -D warnings` 一次通过。
- `timeout 1200 cargo test -p gd3-ui`：**28 passed**（扩展现有
  settings smoke：三键有值推送 Enabled=F/Draft=T/Port=15370、
  缺省容错 14370 等、invoke_browserDraftTakenChanged 通路断言）。
  0 failed。

### 卡点

无

### 下一步（Phase 7c 续）

1. 第 38 步候选：关联设置组（association，sys!=darwin 的 Switch
   们：browserUrlSchemeCard 等）先验；或设置页动作卡骨架（配对
   令牌/Chromium 一键安装的 pressed 动作）；或 GeneralDownload
   组复杂卡（clientProfileCard/HeadersPresetSettingCard/History）
   先验。之后暂停/删除接真实语义（Phase 8 桥接一体考虑）→ 引擎
   状态桥接 → Python UI 停用 + AGENT-DONE。

## 第 38 步 — 关联组 urlScheme 静态卡 + 软件组简版五卡（cfg.py）

- 时间：2026-10-17 07:54（date 实取；代码提交 `7fc3bcb`）

### 改动

- `src/close_mode.rs`（新，48 行）：镜像 OptionsConfigItem
  Software/CloseMode + ComboBoxSettingCard 索引⇄字面量
  （"Ask"/"Background"/"Quit"，缺省 Ask；EnumSerializer 存
  enum.value .venv 实证 cfg.py:65-69）；未知字面量/越界兜底
  （同 conflict.rs 惯例）+1 单测。
- `ui/settings_page.slint`：5 in-out（urlSchemeEnabled/
  checkUpdateAtStartup/runAtLogin/closeModeIndex/clipboardListener）
  + 5 回调 + 五卡（头注释 32-38 步）。
- `ui/appwindow.slint`：同名透传。
- `src/lib.rs`：`pub mod close_mode;`（conflict 前）；push 五组 set；
  SettingsActions 五动作（close_mode 收字面量，索引换算在 install
  层）；install 注册。

### 键位实证（cfg.py）

- `Browser/UrlSchemeRegistered` 缺省 True；`Software/
  CheckUpdateAtStartUp` 缺省 True；`Software/AutoRun`（开机启动）
  缺省 False；`Software/CloseMode` 缺省 CloseMode.ASK 选项字面量
  Ask/Background/Quit；`Software/ClipboardListener` 缺省 True。
- 平台守卫注记：urlScheme 动态卡 sys!=darwin 才构建（本机 Linux，
  Slint 静态显示）；closeMode/剪贴板安卓不显示。
- **欠账清单**：每包关联动态 Switch（featureService packs 的
  associateFileTypes/uriSchemes 真数据留引擎桥接步）、迁移模式
  PushCard、zoomCard/主题/语言/背景材质留主题布局步。

### 工具坑增量（第 3 次）

- 生成/替换文本中混入坏 token 两次（close_mode.rs 注释
  "ORIZATION" 与 settings_page 注释 "компанию"）——grep 非常规
  模式扫描现场、立即修复；后续写中文注释时先 grep "�" 与可疑
  西里尔串再进质量门。

### 验证

- `cargo fmt --all --check` + `cargo clippy -p gd3-ui --all-targets
  -- -D warnings` 一次通过。
- `timeout 1200 cargo test -p gd3-ui`：**28 → 29 passed**（+
  close_mode round-trip/兜底；smoke 补五键有值推送/缺省容错/
  invoke_closeModeChanged(1) => "Background"）。0 failed。

### 卡点

无

### 下一步（Phase 7c 续）

1. 第 39 步候选：下载路径卡动作骨架（FolderPicker 换行编辑已
   占位）、HistoryDownloadFolder、PercentSpinBox 界面缩放
   （DpiScale 0-5 → % 显示 0-500 步 25）；或开始暂停/删除真实
   语义（Phase 8 桥接一体考虑）。
2. 之后引擎状态桥接 → Python UI 停用 + AGENT-DONE。

## 第 39 步 — DpiScale 界面缩放卡 + gd3-core 最小 f64 扩展（cfg.py）

- 时间：2026-10-17 08:05（date 实取；代码提交 `75cdd33`）

### 改动

- `gd3-core/src/config.rs`（272 行）：`get_f64`（`get_str(...)?.parse()
  .ok()`，键缺失/非数字容错 None）+ `set_f64`（shortest repr，整值
  浮点补 `.0`：`fract()==0 && !text.contains('.')` 追加）；
  +1 测试 `f64_python_repr_round_trip`（0.25→"0.25"/2.0→"2.0"/
  5.0→"5.0"/1.75→"1.75"/非数字→None）。
- `ui/settings_page.slint`（484 行）：`uiScalePercent` in-out
  （缺省 0）+ `uiScaleChanged(int)` 回调 + PercentSpinBox 卡
  （min0 max500 step25 + " %" Text；文案「界面缩放/改变应用程
  序界面的缩放比例， 0% 为自动」；头注释 32-39 步）。
- `ui/appwindow.slint`（193 行）：同名透传。
- `src/lib.rs`（968 行）：push
  `set_uiScalePercent(get_f64*100.0 round as i32)`；
  `SettingsActions` 增 `dpi_scale: Box<dyn Fn(f64)>` 收配置口径
  0-5 浮点；install `(dpi_scale.dpi_scale)(percent as f64/100.0)`
  镜像 Python leaveEvent `cfg.set(int(spinBox.value()/division))`、
  division=1/100。

### 键位实证（cfg.py:296）

- `Personalization/DpiScale`：Range(0, 5) 缺省 0、restart=True。
- **UserConfig.json 实证**：值全为 python str 串（仅有 str/bool/i64
  类型）→ 浮点项磁盘口径 `str(float)`：整值补 `.0`（"2.0"），
  DpiScale 0.25 存 "0.25"；`get_f64` 容错解析对齐。
- 显示 % = 配置*100（round 防浮点尾差）、配置写 = %/100 浮点；
  restart=True 生效重启归引擎桥接步。

### 验证

- `cargo fmt --all --check` + `cargo clippy -p gd3-ui --all-targets
  -- -D warnings` 一次通过（首轮 fmt 差 2 处换行，fmt 后过）。
- `timeout 1200 cargo test -p gd3-ui`：**29 passed**（扩展现有
  settings smoke：set_f64("Personalization","DpiScale",1.25)→125、
  缺省 0、invoke uiScaleChanged(75)→0.75 通路断言）。0 failed。
- `timeout 1200 cargo test -p gd3-core`：lib 40 passed（含
  f64_repr，config 2→3）+ 集成全绿 0 failed。

### 卡点

无

### 下一步（Phase 7c 续）

1. 第 40 步候选：剩余复杂卡（clientProfileCard TLS 指纹/UA 预设、
   HeadersPresetSettingCard、HistoryDownloadFolder、
   categoryRulesCard）；或设置页动作卡骨架（配对令牌 PrimaryPush
   复制+重新生成/Chromium 一键安装 pressed）；或直接起手暂停/删除
   真实语义（Phase 8 桥接一体考虑）。
2. 之后引擎状态桥接 → Python UI 停用 + AGENT-DONE。

## 第 40 步 — 浏览器扩展动作卡骨架 + gd3-core pair_token（cfg.py）

- 时间：2026-10-17 08:20（date 实取；代码提交 `ca29c40`）

### 改动

- `gd3-core` 增依赖 `base64 = "0.22"`（gd3-servers 同版本已在
  lockfile，离线缓存可解）。
- `gd3-core/src/pair_token.rs`（新，40 行）：镜像
  `browser_service.regenerateToken` 的 `token_urlsafe(16)`——uuid v4
  → 16 密码学安全随机字节 → base64url 无填充 22 字符；+2 单测
  （22 长度/[A-Za-z0-9_-] 字母表/两次生成互异）。
- `ui/settings_page.slint`（547 行）：`pairToken` in-out +
  `copyPairToken`/`regeneratePairToken`/`openStoreUrl(string)`/
  `chromiumInstall`/`exportCrx` 五回调 + 三动作卡（Button 镜像
  PrimaryPush/HyperlinkCard，std 无 primary 变体与 ToolButton，
  图标留主题步）；头注释 32-40 步。
- `ui/appwindow.slint`（206 行）：同名透传。
- `src/lib.rs`（1040 行，python 替换脚本 9 处锚点全中一次通过）：
  push `set_pairToken(get_str 原样 into)`；SettingsActions 增五动作
  （复制/重新生成/一键安装/导出收 ()、商店收 URL 原样串）；
  install 注册；smoke 补有值/缺省/四 invoke 通路。

### 键位与语义实证

- `Browser/BrowserExtensionPairToken` 缺省 ""（cfg.py:269）。
- 三商店 URL 逐字实证 `constants.py:11-13`：Firefox
  `addons.mozilla.org/firefox/addon/ghost-downloader/`、Edge
  `microsoftedge.microsoft.com/addons/.../odaohmfj...`、Chrome
  `chromewebstore.google.com/.../lagbjgk...`。
- 语义分层：复制令牌=剪贴板写归平台桥接步；重新生成=() 动作、
  调用方生成+cfg.set+回推（断联已认证会话留引擎桥接步）；
  一键安装/导出 CRX=解包与拉浏览器归引擎桥接步；商店链接收
  URL 原样串（openUrl 归平台桥接步）。
- 初稿笔误：草稿模式文案行混入 `);`（Write 一次事实层语法
  错误），python 唯一性锚点即时修复，编译验证。

### 工具坑增量

- Write 全量重写 slint 时夹带书写错误（`);`），证明「内容先
  Read 取全」后生成仍需 post-write 审视；本次由 review diff
  展示即时暴露。
- SharedString ⇄ String 两处编译错误（set_ 收 Into<SharedString>、
  on_openStoreUrl 收 SharedString）一次修正。

### 验证

- `cargo fmt --all --check` + `cargo clippy -p gd3-core -p gd3-ui
  --all-targets -- -D warnings` clippy 与 fmt 门首次 2 处
  E0308 类型换算修正后一次通过。
- `timeout 1200 cargo test -p gd3-core -p gd3-ui`：gd3-core
  **40 → 42 passed**（+2 pair_token）；gd3-ui **29 passed**
  （扩展 settings smoke：pairToken 有值/缺省推送 + 复制/重新生成
  /商店 URL/一键安装 invoke 断言）。0 failed。
- 坏 token 扫描（U+FFFD/西里尔/非常规串）：现场 CLEAN。

### 卡点

无

### 下一步（Phase 7c 续）

1. 第 41 步候选：GeneralDownload 复杂卡（clientProfileCard TLS
   指纹/UA 预设、HeadersPresetSettingCard）；或 categoryRulesCard
   （依赖 categoryService 数据，或与引擎桥接步并轨）；或直接起手
   暂停/删除真实语义（Phase 8 桥接一体考虑）。
2. 之后引擎状态桥接 → Python UI 停用 + AGENT-DONE。

## 第 41 步 — 模拟身份全局默认卡（cfg.py Network/ClientProfile）

- 时间：2026-10-17 08:35（date 实取；代码提交 `b3110ba`）

### 改动

- `src/profile.rs`（新，~95 行）：镜像 `clientProfile =
  ConfigItem("Network","ClientProfile","auto",
  ClientProfileValidator())` 与 IdentitySettingCard 内
  `buildProfileMenu(includeAuto=True)` 菜单序。ComboBox 11 项 =
  auto「自动（匹配来源）」/ raw「不模拟（原样发送）」/
  profileFamilies() 实测九家族「最新」（chrome/edge/firefox/
  firefox-android/opera/safari/safari-ios/safari-ipad/okhttp，
  家族字面量即选项值）。`index_of` 未知字面量→0（对齐
  ClientProfileValidator.correct 兜 "auto"，空串同显 auto）；
  `value_of` 越界兜 auto 不复刻越界崩溃面；+1 round-trip 单测。
- `ui/settings_page.slint`（python 脚本 4 处锚点全中）：
  `clientProfileIndex` 属性 + `clientProfileChanged(int)` 回调 +
  卡（「模拟身份/TLS 指纹与 User-Agent 预设（全局默认，未被
  预设匹配时使用）」；头注释 32-41 步）。
- `ui/appwindow.slint`：同名透传。
- `src/lib.rs`（python 脚本 9 处锚点全中）：push
  `set_clientProfileIndex(get_str→index_of)`；SettingsActions 增
  `client_profile: Box<dyn Fn(&str)>` 收 profile 字面量；install
  层 value_of 换算（镜像 `_onPick`）；smoke：有值
  firefox-android→索引 5、缺省 auto→0、invoke 3→"edge"。

### 键位与对接实证

- registry：Python 侧 `buildRegistry` 由 wreq Emulation 枚举反举
  生成九家族共 131 版本；Rust 侧 gd3-net engine.rs
  `resolve_emulation` 已备 PROFILE_NAMES（每家族最新 1-2 项），
  引擎接口即字符串 profile 字面量——本步落盘串两边解析等容。
- **欠账清单**：identityPresets 列表编辑（PresetRowWidget 添加/
  编辑/删除/开关 + 编辑对话框）、headersPresets/HeadersCard、
  特定版本菜单展开（子菜单版本选择）。

### 工具坑增量

- 初稿又混出一个坏 token：round-trip 测试里伪 API 行
  （伪 API 名 `rank_hint_for`，非真实代码被断言引用即编译错）；
  先修再过。生成 Rust 测试
  断言同样是坏 token 高发区，post-write 必查。

### 验证

- `cargo fmt --all --check` + `cargo clippy -p gd3-ui
  --all-targets -- -D warnings` 一次通过。
- `timeout 1200 cargo test -p gd3-ui`：**29 → 30 passed**
 （+profile 菜单序 round-trip；smoke 补有值/缺省/invoke 三段
  断言）。0 failed。
- 坏 token 扫描：现场 CLEAN（仅 AGENT-PROGRESS 历史引用行）。

### 卡点

无

### 下一步（Phase 7c 续）

1. 第 42 步候选：headersPresets/HeadersCard（currentHeadersPreset
   Range 0-99 + BASE_HEADERS）、categoryRulesCard（或与引擎桥接
   并轨）、identityPresets 列表镜像层；或直接起手暂停/删除真实
   语义（Phase 8 桥接一体考虑）。
2. 之后引擎状态桥接 → Python UI 停用 + AGENT-DONE。

## 第 42 步：请求标头预设镜像层（cfg.py Network/HeadersPresets + CurrentHeadersPreset）

- 时间：2026-10-17 08:47（date 实取；代码提交 `6a61bd2`）

### 改动

- `src/headers.rs`（新，~150 行）：
  - `BASE_HEADERS` cfg.py:29-38 逐字 8 对（保持 dict 插入序）；
  - `parse_presets`：serde_json 解析（落盘是
    `json.dumps(list, ensure_ascii=False)`，JsonConfigSerializer
    cfg.py:151-165 实证）→ 逐项校验（name 为 str、headers 为全
    字符串值 dict，对齐 HeadersPresetListValidator._isValid）→
    无合法项兜 `[{"name":"默认","headers":BASE_HEADERS}]`（对齐
    correct 的 `presets or [默认]`，空串/非 JSON/非数组/空数组/
    全非法五路输入同兜，不复刻 Python 崩溃面）；
  - `current_index` 镜像 `currentHeadersPresetIndex()` 的
    min(value, len-1)（cfg.py:347），负值/非整串按 0 夹取容错
    （Range 0-99 落盘 "0" 串，get_i64 解析）；
  - 3 单测：合法项保序过滤（name 数字/headers 非字符串值整条
    剔除）、五路非法输入兜底、夹取口径。
- `Cargo.toml`：serde_json 提升为 gd3-ui 正式依赖（JSON 串落盘
  解析需要，原本只是 dev-dep；workspace 同版）。
- `ui/settings_page.slint`（python 脚本 3 处锚点全中）：
  `headersPresetNames`（[string] 模型）+ `headersPresetIndex` +
  `headersPresetHeaderCount` + `headersPresetCanRemove` 属性 +
  `headersPresetPicked(int)`/`headersPresetRemoveRequested(int)`
  回调 + 卡（「请求标头预设/新建任务的标头起点」；std 无
  RadioButton，逐行单选退化 ComboBox，镜像 CollapsibleSettingCard
  结构；头注释 32-42 步）。
- `ui/appwindow.slint`：四属性两回调透传。
- `src/lib.rs`（python 脚本 10 处锚点全中）：push 解析推送
  （names 模型/索引/选中项计数/可删派生）；SettingsActions 增
  `headers_preset_pick`/`headers_preset_remove`（统一收当前预设
  索引 i64——cfg 写、删除后索引重排、MessageBox 确认框归调用
  方，镜像 _onPick/_onRemoveClicked）；smoke 补有值（两预设
  A/B、索引 1、计数 2、可删）/缺省（默认 1 条 8 项、不可删）/
  invoke 选中与删除通路断言。

### 键位与 UI 实证

- `HeadersPresetSettingCard`（setting_cards.py:656-763）：
  CollapsibleSettingCard「请求标头预设」/「新建任务的标头起点」+
  choiceLabel（当前预设名）+ 添加预设按钮 + 逐行
  HeadersPresetRow（RadioButton + "{0} 条标头" 计数 + 编辑/删除
  ToolButton，canRemove=len>1）；
- `HeadersPresetEditDialog`（headers_preset_edit.py）：名称
  LineEdit（空→InfoBar 拒绝，落 "未命名预设"）+ HeadersEditor
  （BASE_HEADERS 起点）——本步不动，留欠账步。
- `currentHeaders()`（cfg.py:349-351）= 选中预设 headers 的
  dict 拷贝，供 HeadersEditCard（新建任务流）起步——随 Phase 8
  任务流迁移并轨。

### 工具坑增量

- clippy 两连：`items_after_test_module`（测试助手 trait 写在
  tests 模块后面，上移解决）+ `doc_lazy_continuation`（doc 注释
  列表后接段落需空 `//!` 行分隔）。均为格式规范类，一次即修。
- 生成文本笔误自查：headers.rs 初稿「min 达式」少字，写入前
  python 脚本一并修为「min 表达式」；坏 token 扫描 CLEAN。

### 验证

- `cargo fmt --all --check` + `cargo clippy -p gd3-ui
  --all-targets -- -D warnings` 通过（fmt 一次过，clippy 两轮）。
- `timeout 1200 cargo test -p gd3-ui`：**30 → 33 passed**
  （+headers.rs 3 单测；smoke 四段断言并入既有用例）。0 failed。
- post-write diff 复核：slint/lib.rs 各锚点内容与设计一致，
  无夹带。

### 卡点

无

### 下一步（Phase 7c 续）

1. 第 43 步候选：identityPresets 列表镜像层（同款
   ComboBox+删除形态；REQUIRED_KEYS={name,clientProfile,
   userAgent,hosts}、缺省百度网盘预设 cfg.py:335-340）、
   headers 预设添加/编辑对话框骨架、categoryRulesCard（或与
   引擎桥接并轨）。
2. 之后：暂停/删除真实语义（Phase 8 桥接一体）→ 引擎状态桥接
   → Python UI 停用 + AGENT-DONE。

## 第 43 步：身份预设列表镜像层（cfg.py Network/IdentityPresets）

- 时间：2026-10-17 09:07（date 实取；代码提交 `f43a171`）

### 改动

- `src/identity.rs`（新，~175 行）：
  - **读盘口径实证（qfluentwidgets QConfig.load →
    deserializeFrom）**：只走 serializer 反序列化，**不过
    validator.correct**——合法过滤仅在 qconfig.set 写路径生效。
    镜像：缺键 → 百度网盘客户端预设（cfg.py:335-340 逐字：
    name=百度网盘客户端 / clientProfile=raw / userAgent=
    pan.baidu.com / hosts=[*.pcs.baidu.com] / isEnabled=T）；
    JSON 解析失败/非数组 → `[]`（JsonConfigSerializer fallback
    `list()`）；逐项 `preset.get(...)` 容错取字段，isEnabled
    缺省 True（列表内为 JSON 原生 bool，非第 40/42 步的 JSON
    串包顶层 str）；
  - `row_desc` 镜像 PresetRowWidget.descLayout 三段：hosts 前 3
    个 ", " 连接 / profile 显示名 / UA 截断（>24 取 21+"..."），
    仅非空段出现；Slint 侧以「 · 」间隔、禁用行 opacity 0.4
    （_refreshOpacity 镜像）；
  - 4 单测：缺键兜百度预设+描述串、读盘不做 valid 过滤（含
    精简项与 4 hosts 只取前 3）、坏 JSON→空+isEnabled 非 bool
    兜 T、UA 截断长度。
- `src/profile.rs`：增 `label_of` 全分支镜像 toProfileLabel
  （option_cards.py:23-34）：auto/raw/家族「最新」+特定版本
  字面量拆分（尾部数字+下划线段，"_"→"."，chrome120→
  「chrome 120」、chrome120_1→「chrome 120.1」）+分支断言。
- `ui/settings_page.slint`（python 脚本 4 处锚点全中）：
  `IdentityPresetData` 结构 + `identityPresets` 行模型 +
  `identityPresetToggled(int,bool)`/`identityPresetRemoveRequested
  (int)` 回调 + 行列表卡（VerticalBox for 循环行：名称+描述+
  Switch+「删除」按钮；头注释 32-43 步）。
- `ui/appwindow.slint`：结构导入 + 三行透传。
- `src/lib.rs`（python 脚本 9 组锚点全中）：push 解析推送行
  模型；SettingsActions 增 `identity_preset_toggle`/
  `identity_preset_remove`（索引边界检查与删除确认框归调用
  方，镜像 _onToggled/_onRemoveClicked 的越界守卫）；smoke 补
  有值逐行断言（百度/chrome/hosts 4 取 3/禁用行）、缺省
  （缺键→百度预设单行启用）、invoke 开关 (1,false) 与删除 (0)。

### 新事实（影响后续步骤）

- **第 42 步口径隔断（记录，不改）**：headersPresets 的
  parse_presets 做了 correct 式过滤+兜底（比读盘实际多一层），
  identityPresets 按读盘真实口径不做过滤。两者差异在第 42 步
  注释内自述（「无合法项兜默认」），语义不冲突且更安全；如需
  严格对齐可在回调桥接步统一。
- 与第 41 步模拟身份卡同源（IdentitySettingCard 折叠卡 =
  全局默认下拉 + 预设行列表），Rust 侧现为两卡并列，合并成
  折叠卡留布局/主题步。

### 工具坑增量

- label_of 首版错用家族字面量拼「最新」显示（edge→「edge
  （最新）」），FAMILY_LABELS 第二元才是显示名，测试即抓即修。
- trim_end_matches 闭包需显式 `c: char` 类型标注（E0282）。
- clippy：doc 注释列表后接段落需空 `//!` 行（同第 42 步）+
  `useless_vec`（测试用 `vec![...]` 改字面量数组）。
- **commit message 内混入伪词**（"HALF"——应为「第 2 元显示
  名」），在 f43a171 提交串内残留；代码无恙，不追加修补提交
  （历史不可改写、无实质影响），记录在案。

### 验证

- `cargo fmt --all --check` + `cargo clippy -p gd3-ui
  --all-targets -- -D warnings` 通过。
- `timeout 1200 cargo test -p gd3-ui`：**33 → 37 passed**
  （identity.rs 4 单测 + profile.rs label_of 分支断言并入）。
  0 failed。
- post-write diff 复核：slint/lib.rs/profile.rs 各锚点与设计
  一致，无夹带。

### 卡点

无

### 下一步（Phase 7c 续）

1. 第 44 步候选：headers/identity 预设「添加/编辑」对话框骨架
   （PresetEditDialog/HeadersPresetEditDialog → Slint Dialog，
   含行编辑器与名称校验 InfoBar 镜像）、categoryRulesCard
   （或与引擎桥接并轨）、currentHeaders 读取随 Phase 8 任务造
   （NewTaskCard）并轨。
2. 之后：暂停/删除真实语义（Phase 8 桥接一体）→ 引擎状态桥接
   → Python UI 停用 + AGENT-DONE。

## 第 44 步：库分类规则镜像层（cfg.py Category/CategoryRules + CategoryService）

- 时间：2026-10-17 09:16（date 实取；代码提交 `cfd1006`）

### 改动

- `src/category.rs`（新，260 行）：
  - `CATEGORY_PRESETS` 八预设逐字镜像（category_service.py:16-99，
    fromDict 归一化后与源一致）：视频 VIDEO {default}/Video 20、
    音频 MUSIC /Audio 12、图片 PHOTO /Images 13、字幕 CHAT
    /Subtitles 10、文档 DOCUMENT /Documents 19、压缩包
    ZIP_FOLDER /Archives 24（**含 tar.gz/tar.bz2/tar.xz/tar.zst
    复合扩展字面量**）、程序 APPLICATION /Programs 21、其他 HELP
    无 folder 无扩展；
  - `parse_rules` 镜像 `CategoryService._load`（136-144）：缺键/
    空串/`[]`/非 JSON/空数组 → 八预设；非空逐项 fromDict 容错
    归一化（112-131）：extensions 逐项 str→strip→去点→小写→去重、
    icon 缺省 DOCUMENT、name 缺省 ""、folder 空串→None；
    **缺 categoryId 读侧保留空串不生成随机**（Python 端
    `cat_{uuid4().hex}` 随机性不复刻，空 id 由写侧调用方处理）；
  - `summary_of` 镜像 `_toSummary`：0 项「无扩展名」/≤4 项全量
    `", "` join/>4 项「{前4} 等 {N} 项」；
  - 3 单测：五路非法输入兜八预设（复合扩展保留）、disk 逐项
    归一化断言、summary 三分支。
- `ui/settings_page.slint`（python 脚本 4 处锚点全中）：
  `CategoryRowData` 结构（name/desc/id）+ `categoryRows`/
  `categoryRuleCount` 属性 + `categoryResetRequested()`/
  `categoryRemoveRequested(string)` 回调 + 行列表卡（插在「启用
  下载分类」HorizontalBox 块后：VerticalBox for 循环行——名称+
  摘要 Text+「删除」按钮，尾行「共 N 个分类」+「恢复默认」按钮；
  头注释 32-43→32-44 步）。
- `ui/appwindow.slint`：结构导入 + 四行透传。
- `src/lib.rs`（python 脚本 10 组锚点全中）：push 解析推送行模型
  （desc 预拼 `category::summary_of`）；SettingsActions 增
  `category_reset: Box<dyn Fn()>` / `category_remove: Box<dyn
  Fn(String)>`（cfg 写与 categoriesChanged 重推归调用方，同
  41/42/43 步「动作收配置单位」惯例；删除收 SharedString→String
  原样）；smoke 补有值逐项归一化断言（去点/小写/去重→["mp4",
  "mkv"]/空 folder→None/icon 缺省/缺 id 空串/计数 2）、缺省
  八预设逐字断言（视频 4 头「mp4, mkv, avi, mov 等 20 项」 summaries、
  其他「无扩展名」）、invoke 恢复默认 () 与删除 ("x") 通路。

### 新事实（影响后续步骤）

- 摘要格式坑：>4 项summary 是「{前4} 等 {N} 项」（N 为总数），
  smoke 首版把缺省「视频」行预期写成纯前 4 join，测试即抓即修。
- categoryRules 读侧与 identity 同口径（Load 不过
  CategoryListValidator.correct）；写路径 correct 过滤随添加/
  编辑对话框步再镜像。

### 工具坑增量

- clippy `doc_lazy_continuation` 第三次触发（模块注释列表行后接
  段落须空 `//!` 行）+ 注释 `///`/`//!` 混用行（改统一 `//!`）。
- 头两轮会话被压缩，本轮无净耗时损失；脚本 ROOT 变量首用
  `__file__` 推导落在 scratch 目录致 FileNotFoundError，改为
  硬编码仓库根后三文件锚点全中。

### 验证

- `cargo fmt --all --check` + `cargo clippy -p gd3-ui
  --all-targets -- -D warnings` 通过。
- `timeout 1200 cargo test -p gd3-ui`：**37 → 40 passed**
  （category.rs 3 单测 + smoke 增补并入）。0 failed。
- post-write diff 复核：category.rs/slint×2/lib.rs 与设计一致，
  无夹带；坏 token 扫描（Paras|comfirm|HALF|乱码区段）零命中。

### 卡点

无

### 下一步（Phase 7c 续）

1. 第 45 步候选：预设添加/编辑对话框骨架（HeadersPresetEditDialog
   /CategoryEditDialog → Slint Dialog，含名称校验与 categories
   重推镜像）；或暂停/删除真实语义（Phase 8 桥接一体化评估）。
2. 之后：引擎状态桥接 → Python UI 停用 + `AGENT-DONE`。

## 第 45 步：分类添加/编辑对话框骨架（app/view/dialogs/category_edit.py）

- 时间：2026-10-17 09:35（date 实取；代码提交 `ccd9dda`）

### 改动

- `src/category.rs`（+99 行）：
  - `ICON_CHOICES` 25 项逐字（category_edit.py:17-23）；push 推
    `[string]` 模型供 ComboBox；
  - `draft_to_rule` 镜像 `CategoryEditDialog.category()`（93-110）：
    名称 strip 空兜「未命名分类」；扩展名逐 token strip→去点→
    小写、空 token 丢弃、**不去重**（TokenLineEdit tokens 口径：
    回车/空格逐 token 实证 token_line_edit.py:99-108——每输入一段
    提交一个 token；简版改为逗号分隔 LineEdit）；folder strip 空
    兜 None；icon 兜 DOCUMENT（与 fromDict 同口径）；CategoryRule
    derive Clone（smoke 命中 vec 需要）；
  - **差异记录**：Qt 侧「显示名未改动则保留原文名」依赖
    QCoreApplication.translate 翻译层，Rust 无译文层直接采用所
    填名称（注释自述）；
  - 2 单测：draft 归一化（含不去重口径）+ ICON_CHOICES 长度/端点。
- `ui/settings_page.slint`（python 脚本 6 处锚点全中）：
  `CategoryEditDraft` 结构（id 空串=添加镜像 Dialog(category=None)）
  + 覆盖层七属性（choices/visible/id/name/iconIndex/extensions/
  folder）+ 四回调（add/editRequested/closed/accepted(Draft)）+
  行内「编辑」按钮 + 尾行「添加分类」按钮 + 内嵌覆盖层骨架（
  标题「添加分类/编辑分类」按 id 分支；占位文案逐字含「{default}」
  字面量）。
- `ui/appwindow.slint`：草稿结构导入 + 七属性四回调透传。
- `src/lib.rs`（锚点 7 组）：push 推送 ICON_CHOICES；
  `open_category_edit` 直写两态助手（带值 findData 预选镜像——
  未知 icon 落首项；None=添加置空置可见）；SettingsActions 增
  `category_add_requested`/`category_edit_requested`/`category_edit_
  closed`/`category_edit_submit`（确定收 draft_to_rule 归一化产物，
  cfg 写与 categoriesChanged 重推归调用方）；smoke 增 choices 25
  项/覆盖层收起缺省断言 + invoke 添加/编辑/确定（归一化产物逐
  字段断言）/取消通路 + open_category_edit 两态独立冒烟（含未知
  icon 落首项）。

### 新事实（影响后续步骤）

- **Slint 字面量花括号坑**：`{...}` 即插值；转义 `\{` 实测不被
  识别（报 Unknown unqualified identifier 'default'），`\u{7b}`/
  `\u{7d}` 可行——占位文案「{default}」以 unicode 转义书写。
- **内嵌覆盖层形态**：MessageBoxBase 弹窗骨架暂以内嵌
  Rectangle（rgba 背景 + 100% 宽高）替代，弹窗/焦点管理留布局步；
  HeadersPresetEditDialog/IdentityPresetEditDialog 同套骨架复用
  留下一步。
- smoke invoke 段的窗口是未 push 的全新实例——行模型断言须在有
  值推送窗口内做，invoke 段用字面 id（E0716 row_data 临时值
  同段修）。

### 工具坑增量

- category.rs 函数注释 `//!` 混用报 E0753（inner doc only before
  items），统一 `///`。
- 覆盖层插入首版多一个闭合括号 → parse error at 787，删一行复原。
- heredoc 写 python 补丁脚本因三引号内容含特殊序列失败（且
  主脚本已先行执行）——补丁改用 Write 工具落盘脚本文件，避免
  shell heredoc 承载复杂字面量。

### 验证

- `cargo fmt --all --check` + `cargo clippy -p gd3-ui
  --all-targets -- -D warnings` 通过。
- `timeout 1200 cargo test -p gd3-ui`：**40 → 43 passed**
  （category.rs 2 单测 + open_category_edit 冒烟并入）。0 failed。
- post-write diff 复核：category.rs/slint×2/lib.rs 与设计一致，
  无夹带；坏 token 扫描零命中。

### 卡点

无

### 下一步（Phase 7c 续）

1. 第 46 步候选：HeadersPresetEditDialog/IdentityPresetEditDialog
   同套覆盖层骨架复用（名称+headers/hosts 行编辑器；名称校验与
   「重名拒绝」镜像）；或暂停/删除真实语义（Phase 8 桥接一体化
   评估）。
2. 之后：引擎状态桥接 → Python UI 停用 + `AGENT-DONE`。

## 第 46 步：标头预设添加/编辑覆盖层（app/view/dialogs/headers_preset_edit.py）

- 时间：2026-10-17 10:43（date 实取）

### 改动

- `src/headers.rs`（+195 行）：
  - `split_lines` 全量镜像 CPython `str.splitlines()` 行界（`\r`、
    `\n`、`\v`、`\f`、`\x1c-\x1e`、`\u{85}`、`\u{2028}`、`\u{2029}`，
    `\r\n` 视为单个）——`str::lines` 只认 `\n`/`\r\n`，粘贴文本
    里的老式行尾会漏切；
  - `py_isspace`/`py_strip` 镜像 CPython `isspace()`/`strip()` 口径
    （`\x1c-\x1f` 在 Python 为空白、Rust `char::is_whitespace` 不含）；
  - `parse_header_line` 镜像 `parseHeaderLine`（headers_editor.py:33-40）：
    `find(':')` 缺失即丢、名称 strip 空或含空白字符即丢、值 strip
    （空值合法）；
  - `to_logical_lines` 镜像 `toLogicalLines`：CRLF 归一化 + 行尾
    `\`/`^` 续行接空格 + 逐行 strip 去空；
  - `parse_header_pairs` / `headers_to_text` 互逆（编辑回显与提交
    共用一套行口径）；`draft_to_header_preset` 镜像 `preset()`
    （名称 py_strip 空兜「未命名预设」）；`HeaderPreset` derive
    `Clone`（smoke 采集需要）；
  - 4 单测：`parse_header_pairs_mirrors_editor_rules`、
    `to_logical_lines_mirrors_cpython_splitlines_and_strip`（期望值
    由 `.venv` CPython 实测得出，含 `\x1b`/`\x0c`/`\x1c`/`\u{2028}`/
    `\u{85}`/`\r\n`/续行/纯空白 → `[]`）、
    `draft_to_header_preset_mirrors_dialog_preset`、
    `headers_to_text_round_trips_with_parse`。
- `ui/settings_page.slint`：`HeadersEditDraft` 结构（index -1=添加）+
  覆盖层五属性（visible/index/name/pairs/nameError）+ 四回调
  （addRequested/editRequested(int)/closed/accepted(Draft)）+ 行内
  「编辑预设」/「添加预设」按钮 + 内嵌覆盖层骨架（标题按 index
  分支、名称 LineEdit、空名提示、标头 TextEdit、取消/确定；确定
  空名只置 nameError 不回发）。
- `ui/appwindow.slint`：草稿结构导入 + 五属性四回调透传；
  `headersEditIndex` 默认值写 **-1**（`<=>` 双向绑定下根组件默认值
  生效，否则覆盖 settings_page 的 -1）。
- `src/lib.rs`：`open_headers_preset_edit(window, config, index)` 两态
  直写（None=添加：名称空 + 起点回显 `currentHeaders()` 即当前选中
  预设的标头，镜像 `_onAddClicked`；带值=编辑：回显 `presets[index]`）；
  `SettingsActions` 增 `headers_preset_add_requested` /
  `headers_preset_edit_requested` / `headers_preset_edit_closed` /
  `headers_preset_edit_submit((i64, HeaderPreset))`（确定收
  `draft_to_header_preset` 归一化产物，cfg 写与重推归调用方）；
  smoke 增覆盖层缺省断言 + invoke 四通路 + `open_headers_preset_edit`
  两态独立冒烟。

### 新事实（影响后续步骤）

- **slint 1.18.1 fluent 没有 `TextArea`**：`std-widgets.slint` 只导出
  `TextEdit`（`text`/`placeholder-text`/`read-only`/`edited` 足够）。
  最初写 `TextArea` 导致 `:886: Parse error`（连带尾行三行游离括号
  一并清理）。
- **Slint 无 `trim`**：覆盖层只能判 `name == ""`；空白串由 Rust
  `draft_to_header_preset` 兜「未命名预设」，与 Python `preset()` 的
  `text().strip()` 同口径（Python `validate()` 会先拦，属 UI 层差异，
  已记录）。
- **双向绑定默认值在根组件生效**：`<=>` 下 AppWindow 的
  `headersEditIndex` 缺省 0 会覆盖页面里的 -1，缺省值须写根组件。

### 工具坑增量

- 每文件独立 `let x = actions.clone();` 是既有惯例——`headers_act`
  单变量复用会被前一个 `move` 闭包吃掉。
- `to_logical_lines` 里 `.map(str::trim)` 类型不匹配（要
  `Vec<String>`），改 `.map(|line| py_strip(line).to_string())`。
- clippy `question_mark`：`line.find(':')?` 比 `if let Some` 干净。

### 验证

- `cargo fmt --all --check` 通过。
- `cargo clippy --all-targets -- -D warnings`（全工作区）通过。
- `timeout 1200 cargo test -p gd3-ui`：**43 → 48 passed**，0 failed。
- `timeout 2400 cargo test -p gd3-core -p gd3-net -p gd3-servers`：
  19 个 test binary 全 ok，0 failed。
- Python 全量：`pytest -q --continue-on-collection-errors` →
  **3 failed, 1347 passed, 8 skipped, 35 errors**，失败集合
  （`test_android_config_parity` / `test_github_probe::testProxySiteGetsNoCredentials` /
  `test_runtime_install::test_delete_raises_when_a_path_cannot_be_deleted`）
  全部属既有基线，无新增。本步只动 `gd3-ui`（未接进 Python 侧），
  不涉及影子开关，故无需双模式回归。

### 卡点

无

### 下一步（Phase 7c 续）

1. 第 47 步候选：`IdentityPresetEditDialog` 复用同套覆盖层骨架
   （名称 + hosts 行编辑器 + 「重名拒绝」校验）；或
   `HeadersEditor` 的 cURL 粘贴解析（`parseCurl`）与重复行高亮。
2. 之后：暂停/删除真实语义 → 引擎状态桥接 → Python UI 停用 +
   `AGENT-DONE`。

## 第 47 步：身份预设添加/编辑覆盖层（app/view/dialogs/preset_edit.py）

### 做了什么

- **新建 `rust/crates/gd3-ui/src/pystr.rs`（83 行）**：把 CPython 字符串口径
  抽出共用——`py_isspace`（`\x1c-\x1f` 在 Python 为空白、Rust
  `char::is_whitespace` 不含）、`py_strip`、`split_lines`（`str.splitlines()`
  全量行界：`\r`/`\n`/`\v`/`\f`/`\x1c-\x1e`/`\u{85}`/`\u{2028}`/`\u{2029}`，
  `\r\n` 算一个；`\x1f` **不是**行界）。CPython 实测：`'a\x1fb'.splitlines()`
  → `['a\x1fb']`；`\u{a0}` 两侧都算空白。含 2 单测。
  `headers.rs` 删掉本地三函数副本改 `use crate::pystr::{...}`（第 46 步的
  `parse_header_line`/`to_logical_lines` 语义不变）。
- **`identity.rs`**：`IdentityPreset` derive `Clone`；新增
  - `parse_hosts`：按空白/`,`/`;` 切 → 逐条 `py_strip` → 去空 → 保序去重
    （镜像 `TokenLineEdit` 的 token 口径：回车/空格提交 `text().strip()`
    非空、重复 token 丢弃；覆盖层用多行文本框，故多切 `,`/`;`，差异记录在案）；
  - `hosts_to_text`（`"\n".join`，与 `parse_hosts` 互逆）、`hosts_valid`；
  - `profile_index_of`（`""`→0、`raw`→1、九家族→2+p、未知→`None`）、
    `profile_value_of`（越界兜 `""`）、`profile_label_of`（0 特例「跟随全局
    默认」，其余复用 `profile::label_of` 镜像 `toProfileLabel`）；
  - `draft_to_identity_preset`（镜像 `preset()`：名称 `py_strip` 空兜
    「未命名预设」、UA `py_strip`、hosts 逐 token、isEnabled 由调用方给）；
  - 3 单测（token 规则 / 草稿归一化 / `buildProfileMenu(includeAuto=False)`
    菜单顺序与 0 特例）。
- **`ui/settings_page.slint`**：`IdentityEditDraft` 结构（`index`/`name`/
  `hosts`/`profile`/`userAgent`——字段名用驼峰规避 Slint 连字符字段名
  codegen 风险）+ 11 属性 + 4 回调；身份预设行加「编辑」按钮、列表尾加
  「添加预设」按钮；文件尾新增覆盖层（标题按 `index` 分支、名称
  `LineEdit`、匹配 Host + 「?」帮助开关与逐字 TeachingTip 文案、hosts
  `TextEdit`、错误行「预设需要至少一个 Host 才能匹配请求」、TLS 指纹
  `ComboBox`（labels/values 双模型）、User-Agent `LineEdit`、取消/确定）。
- **`ui/appwindow.slint`**：`IdentityEditDraft` 导入 + 11 属性四回调透传，
  `identityEditIndex` 默认 **-1** 写根组件（`<=>` 下根组件默认值生效）。
- **`src/lib.rs`**：
  - `open_identity_preset_edit(window, config, index)` 两态直写：None=添加
    （名称/hosts/UA 空、profile `""` 即「跟随全局默认」）；带值=编辑回显
    `presets[index]`。profile 模型现推 11 项；**未知字面量（如 `chrome120`）
    追加第 12 项**并原样保留值——Python 侧按钮文本走 `toProfileLabel`、
    菜单里却没有该项，这里补上避免静默丢值。
  - `apply_identity_preset_draft(presets, index, draft) -> bool`：index<0
    追加（`_onAddClicked`）、否则替换并**继承既有 isEnabled**（镜像
    `preset()` 的 `result["isEnabled"] = self._preset.get(...)`），越界返回
    false（镜像 `_onEditClicked` 的越界 return）。
  - `SettingsActions` 增 `identity_preset_add_requested` /
    `identity_preset_edit_requested` / `identity_preset_edit_closed` /
    `identity_preset_edit_submit((i64, IdentityPreset))`。
  - 注册：确定草稿先过 `validate()` 口径——`parse_hosts` 归一化后判空，
    不通过只置 `identityEditHostsError` 并 return（**不收起、不发事件**），
    通过后清错、收起覆盖层、回发 `draft_to_identity_preset` 产物。
  - smoke：覆盖层缺省断言（收起 / 索引 -1 / 模型空）+ invoke 四通路
    （含拒绝路径与通过路径的状态断言）+ `open_identity_preset_edit` 两态
    冒烟（含 `chrome120` 追加项）+ `apply_identity_preset_draft` 追加/替换/
    越界单测。

### 新事实

- **Slint 侧确定按钮不自行收起**（与第 46 步标头覆盖层不一致）：hosts 校验
  必须 `parse_hosts` 归一化后才能判空，精确镜像 `validate()` 只能放 Rust
  侧，收起责任随之移到 Rust。第 46 步的标头覆盖层仍是 Slint 自行收起
  （`== ""` 廉价判定），两处不一致已记录，不回改。
- **profile 双模型（labels + values）**：`identityEditProfile`（磁盘字面量）
  为权威值，`ComboBox` 只负责显示与选择；`selected` 里回写
  `identityEditProfileValues[self.current-index]`。
- **`QT_QPA_PLATFORM=offscreen` 必须来自 `/root/gd3-env.sh`**：漏 source 时
  第一次创建 `qapp` fixture 会 `Fatal Python error: Aborted`（exit 134），
  不是代码问题。

### 验证

- `cargo fmt --all --check` 通过（先 `cargo fmt --all` 修了 6 处换行）。
- `cargo clippy --all-targets -- -D warnings`（全工作区）通过。
- `timeout 1200 cargo test -p gd3-ui`：**53 → 55 passed**，0 failed。
- `timeout 2400 cargo test -p gd3-core -p gd3-net -p gd3-servers`：
  **236 passed, 0 failed**（19 个 test binary）。
- Python 全量：`pytest -q --continue-on-collection-errors` →
  **3 failed, 1347 passed, 8 skipped, 35 errors**，失败集合
  （`test_android_config_parity` / `test_github_probe::testProxySiteGetsNoCredentials` /
  `test_runtime_install::test_delete_raises_when_a_path_cannot_be_deleted`）
  与基线完全相同，无新增。本步只动 `gd3-ui`（未接进 Python 侧），
  不涉及影子开关，故无需双模式回归。

### 卡点

无

### 下一步（Phase 7c 续）

1. `HeadersEditor` 的 cURL 粘贴解析（`parseCurl`）与重复行高亮。
2. `categoryRulesCard` 写路径的 `correct` 过滤。
3. 之后：暂停/删除真实语义 → 引擎状态桥接 → Python UI 停用 + `AGENT-DONE`。

---

## 第 48 步（Phase 7c）：`HeadersEditor` 解析层（cURL 分词 + 归一化 + 重复标头）

### 做了什么

- **新建 `rust/crates/gd3-ui/src/pyshlex.rs`（209 行）**：手写状态机镜像
  CPython 3.13 `shlex.shlex.read_token` 在
  `posix=True, whitespace_split=True, commenters='', punctuation_chars=''`
  下的行为（`shlex.split` 的口径）。`pub fn split(&str) -> Option<Vec<String>>`
  （`None` = Python 的 `ValueError`）。要点：
  - 空白集固定 `' ' '\t' '\r' '\n'`；NBSP / `\x1c` / `\x1f` **不是** shlex
    空白（`char::is_whitespace` 会误判，不能直接用）；
  - `whitespace_split=True` 让 `wordchars` 失效；`commenters=''` 使 `#`
    是普通字符；`punctuation_chars=''` 使 `&&`/`||` 不切开；
  - 单引号内反斜杠是字面量；双引号内只有 `\\` 与 `\"` 吃掉反斜杠
    （`\$`/`` \` ``/`\n` 保留）；引号包裹空串是合法 token；
  - 词外 `\` 的 `escapedstate` 是 `'a'` 而非 `' '`——逐字照抄 CPython
    （可见后果：`"\ x"` → `[" x"]`，空白被带进词里）；
  - EOF 在引号内 / 反斜杠后 → `None`。4 单测。
- **`headers.rs` 新增解析层**：
  - `CURL_HEADER_FLAGS` / `is_command_separator` / `curl_value_flag` /
    `starts_with_curl_command`（逐字符 ASCII 小写比较，等价
    `line[:5].lower() == "curl "`）/ `parse_curl` / `parse_headers`
    （逐逻辑行分派，cURL 行只认第一条）；
  - `to_header_rows`（字面切分、零丢弃，镜像 `toHeaderRows`）；
  - `headers_map`（镜像 `HeadersEditor.headers()`：`strip().lower()`、
    空名/空值丢弃、同名后者覆盖、键序取首次出现位置）；
  - `duplicate_flags`（镜像 `_refreshDuplicates`）/ `duplicate_names` /
    `duplicate_warning`（汇总文案，Slint 专用）。
- **`headers_to_text` 改为镜像 `toHeadersText`**：值空只写名，行间 `\n`，
  **末尾不带换行**（原来是每行带 `\n`）。
- **`draft_to_header_preset` 改走 `headers_map(&parse_headers(pairs))`**：
  Python 在**粘贴时**就归一化 cURL（`HeadersTextEdit.insertFromMimeData`），
  Slint 的 `TextEdit` 拦不到粘贴，只能在确定时做等价归一化。
- **重复标头实时提示接线**：`settings_page.slint` 加
  `headersEditDuplicateWarning` 属性 + `TextEdit` 的 `edited` →
  `headersPresetPairsEdited(string)` 回调 + 警告行；`appwindow.slint`
  属性/回调透传；`lib.rs` 注册回调算 `duplicate_warning`
  （纯呈现层派生，不进 `SettingsActions`），`open_headers_preset_edit`
  也初算。
- **金文件 `rust/crates/gd3-ui/tests/fixtures/headers_parse.jsonl`（203 条）**
  由 `generate_headers_fixture.py` 生成：直接 import 真实 Python 的
  `parseCurl`/`parseHeaders`/`toHeaderRows`/`toHeadersText`/`parseHeaderLine`/
  `toLogicalLines`；`headers()` 与 `_refreshDuplicates()` 用桩替换
  `_currentRows()`/`_rows()` 后跑真方法，保证镜像对象是 Python 本身。
  语料：`SPLIT_CASES`(63) / `CURL_CASES`(43) / `HEADERS_CASES`(27) /
  `ROWS_CASES`(20) / `TEXT_CASES`(10) / `MAP_CASES`(14) / `DUP_CASES`(10) /
  `header_line`(13)。

### 新事实

- **`shlex` crate 不能用**：双引号内 `\$`/`` \` `` 会吃掉反斜杠、空白集缺
  `\r`，与 CPython 有实质分歧。手写状态机 + 金文件逐条对比是唯一能做到
  「逐断言一致」的路径。
- **`toHeadersText` 末尾无换行**：`"\n".join(...)`。第 46 步的实现每行都
  带 `\n`，已改（连带修了两处 smoke 断言）。
- **`draft_to_header_preset` 的口径变化**：从 `parse_header_pairs`（保留空值
  行）改成 `headers_map`（丢弃空值行、名称小写、同名覆盖）。唯一已知差异：
  名称含空白的行——Python 文本视图会保留、表格视图被 `HeaderNameValidator`
  拒绝，取更安全的一侧丢弃，已写进 doc 注释。
- **重复行高亮降级**：Python 是 `row.setDuplicate` 逐行红框，Slint 的
  `TextEdit` 无法逐行染色 → 一行汇总文字「重复标头：x, y（后一条覆盖
  前一条）」，判定口径与 `_refreshDuplicates` 完全相同（`strip().lower()`、
  空名不进 seen）。
- 金文件**可复现**：连跑两次 md5 相同（`6e256b0c…` → 改语料后重算）。

### 验证

- `cargo fmt --all --check` 通过。
- `cargo clippy --all-targets -- -D warnings`（全工作区）通过。
- `timeout 1800 cargo test -p gd3-ui`：**61 passed**，0 failed
  （含 `parse_layer_matches_python_fixture` 逐条对比 203 条金文件，
  以及 8 类语料的下限断言防「金文件被清空仍全绿」）。
- `timeout 2400 cargo test -p gd3-core -p gd3-net -p gd3-servers`：
  **236 passed**，0 failed。
- Python 全量：`pytest -q --continue-on-collection-errors` →
  **3 failed, 1347 passed, 8 skipped, 35 errors**，失败集合
  （`test_android_config_parity` / `test_github_probe::testProxySiteGetsNoCredentials` /
  `test_runtime_install::test_delete_raises_when_a_path_cannot_be_deleted`）
  与基线完全相同，无新增。本步只动 `gd3-ui`（未接进 Python 侧），
  不涉及影子开关，故无需双模式回归。

### 卡点

无

### 下一步（Phase 7c 续）

1. `categoryRulesCard` 写路径的 `correct` 过滤。
2. 之后：暂停/删除真实语义 → 引擎状态桥接 → Python UI 停用 + `AGENT-DONE`。

## 第 49 步（Phase 7c）：分类规则写路径（`rules_to_json` / `correct` / `CategoryService` 五方法）

### 做了什么

- **`gd3-core/src/config.rs`**：`fn json_string` → **`pub fn py_json_string`**，
  并**修掉 `\b`(0x08) / `\f`(0x0c) 的字节偏差**：CPython
  `json.encoder.ESCAPE_DCT` 里这两个走短转义 `\\b` / `\\f`，旧实现给的是
  `\u0008` / `\u000c`。这是 `UserConfig.json` 值串 writer 的真实偏差
  （配置文件里出现退格/换页就写错），顺手修掉；3 处内部调用改为
  `py_json_string(...)`，doc 注释按 `ESCAPE_DCT` 重写（`< 0x20` → `\uXXXX`
  小写四位，`>= 0x20` 原样含 `\x7f` / `\u2028` / `\u2029` / CJK / emoji）。
  提成 `pub` 是因为 `category.rs` 的写路径要复用同一转义口径，不复制第二份。
- **新金文件 `rust/crates/gd3-core/tests/fixtures/json_escape.jsonl`（28 条）**
  + `generate_json_escape_fixture.py`（脚本内 `assert json.loads(expected) == case`
  自检）。语料覆盖 `"` / `\` / `\b` / `\f` / `\n` / `\r` / `\t` / `\x00`-`\x1f`
  全段抽样 / `\x7f` / `\u2028` / `\u2029` / CJK / emoji。
- **`config.rs` 测试 `py_json_string_matches_python_dumps_fixture`**：逐条
  比字节 + `serde_json::from_str::<String>` round-trip + `count == 28`
  防「金文件被清空仍全绿」。
- **新金文件 `rust/crates/gd3-ui/tests/fixtures/category_rules.jsonl`
  （23 条 = 5 `dump` + 11 `correct` + 7 `draft`）**
  + `generate_category_fixture.py`：
  - `dump`：跑真 `CategoryListValidator().correct()` + `json.dumps(ensure_ascii=False)`，
    输入是 `[asdict(Category.fromDict(p)) for p in CATEGORY_PRESETS]` 等 5 组
    （八预设 / 空表 / 转义与 Unicode / 控制字符与行分隔符 / folder 空串形态）。
    不构造 `CategoryService` 实例，免得 `_load` 真读写 cfg。
  - `correct`：11 条 `CategoryListValidator().correct` 输入/输出对
    （非 list、含非 dict 项、`name` 非 str、`icon`/`extensions` 类型错放行等）。
  - `draft`：**直接构造真 `CategoryEditDialog`**（`QT_QPA_PLATFORM=offscreen`
    + `QWidget` parent 且 `resize(800, 600)`——`MessageBoxBase.__init__` 要
    `parent.width()`，`parent=None` 直接 `AttributeError`），设 `nameEdit` /
    `setTokens` / `folderEdit` / `iconCombo.setCurrentIndex(findData(...))`
    后调真 `category()`。extensions 输入是原始文本，脚本按 `,` 切分后
    `setTokens`（与 Slint 逗号分隔口径对齐）。
  - 连跑两次 md5 相同（`6d62d1f76bc069fcbb5e78b436663625`）。
- **`gd3-ui/src/category.rs`**：
  - `CategoryRule` derive 补 `PartialEq, Eq`；`from_dict` 的 `.trim()`、
    `draft_to_rule` 的 `trim()` / `token.trim()` 全部改 **`pystr::py_strip`**
    （修 `\x1c`-`\x1f` 分歧：Python 算空白、Rust `char::is_whitespace` 不算）。
  - 新增 `rules_to_json(&[CategoryRule]) -> String`：键序
    `categoryId/name/icon/extensions/folder`（dataclass 字段序）、分隔符
    `", "` / `": "`、`None` → `null`、`Some("")` → `""`（`asdict` 只把
    `None` 变 `null`）。
  - 新增 `correct_rule_list(&Value) -> Value`：镜像 `CategoryListValidator.correct`
    （非 list / 任一项非 dict / 任一项 `name` 非 str → 整表 `[]`；其余原样）。
  - 新增 `add_category` / `update_category -> bool`（按 id 首个匹配替换，
    未命中静默不写盘）/ `remove_category -> bool`（`retain`，重复 id 一起删）/
    `reset_categories()` / `reorder_categories -> bool`（`HashMap` 建 id→rule、
    后者覆盖前者、长度不等整体放弃）。
  - `draft_to_rule` 的 icon 改为 **只接受 `ICON_CHOICES` 里的值，其余（含空串）
    兜 `DOCUMENT`**——金文件第 7 条 `NOT_AN_ICON` 暴露的偏差：Qt 侧
    `iconCombo.setCurrentIndex(findData(未知))` 返回 -1 → `currentData()` 是
    `None` → `or "DOCUMENT"` 兜底。读侧 `from_dict` **不**过滤未知图标
    （与 Python `fromDict` 一致），两条路径口径不同是 Python 本身如此。
  - 测试新增 `write_path_matches_python_fixture`（`dump` 逐字节 +
    `correct` 恒等 + 往返 `parse_rules`（跳过空表与 `folder == Some("")`）+
    条数断言 `(5, 11, 7)`）与 `service_write_methods_mirror_category_service`。

### 关键决策

- **`rules_to_json` 手写而非 `serde_json::to_string`**：serde_json 分隔符不带
  空格，与 `json.dumps` 默认 `", "` / `": "` 不同；键序按字段序硬编码更直接。
- **`correct` 单独暴露、不塞进 `rules_to_json`**：强类型 `CategoryRule` 恒过
  `validate`（只要求 list/dict/`name` 是 str），塞进去是无效间接层；单独暴露
  + 金文件 11 条断言可以在关口语义被改时报警。
- **不实现 `apply_category_rule(rules, index, rule)`，改镜像 `CategoryService`
  五个真实方法**：Python 编辑路径按 `categoryId` 匹配（`updateCategory` 未命中
  静默不写盘），不是按 index——按 index 会造出不存在的语义。
- **`reorder` 逐字镜像 `byId` 字典语义**（重复 id 后者覆盖、长度不等放弃），
  包括「重复 id 会让同一项出现两次、被挤掉的项丢失」这个 Python 也有的怪行为。
- **`\b`/`\f` 修正是本步的附带收获**：不是为金文件而改，是 `UserConfig.json`
  落盘字节的真实偏差；既然 `category.rs` 要复用同一个 writer，就不能带着已知
  偏差复用。

### 新事实

- CPython `json.dumps(ensure_ascii=False)` 只对 `< 0x20` 做转义，且 `\b`/`\f`
  用短转义；`\x7f`、`\u2028`、`\u2029` 一律原样输出。
- `MessageBoxBase` 子类在无头环境构造必须给 `QWidget` parent 且 parent 有
  非零尺寸，否则 `parent.width()` 直接抛 `AttributeError`。
- 金文件含裸 `\u2028`/`\u2029` → Python 侧核对必须 `split("\n")`，
  `str.splitlines()` 会多切；Rust `str::lines()` 只切 `\n`，安全。
- `Category.fromDict` 对**非字符串**扩展名是 `str(ext).strip()`，会留下
  `"123"` / `"none"`；Rust 侧按空串丢弃（已在 doc 注释记录，磁盘格式由本侧
  写入时恒为字符串，只有手改配置文件才会碰到）。

### 验证

- `cargo fmt --all --check` 通过。
- `cargo clippy --all-targets -- -D warnings`（全工作区）通过。
- `cargo test -p gd3-ui`：**63 passed**（61 → 63）。
- `cargo test -p gd3-core -p gd3-net -p gd3-servers`：**237 passed**
  （236 → 237）。
- 两份金文件连跑两次生成脚本 md5 相同。
- Python 全量：`pytest -q --continue-on-collection-errors` →
  **3 failed, 1347 passed, 8 skipped, 35 errors**，失败集合与基线完全相同。
  本步只动 Rust（未接进 Python 侧），不涉及影子开关，故无需双模式回归。

### 卡点

无

### 下一步（Phase 7c 续）

1. `lib.rs` 把 `rules_to_json` 接到 cfg 写路径（`GD3_RUST_CONFIG` 式影子开关）。
2. 之后：暂停/删除真实语义 → 引擎状态桥接 → Python UI 停用 + `AGENT-DONE`。

## 2026-10-17 12:01 第 50 步

### 做了什么

镜像 `app/view/cards/task_cards.py` 的**卡片派生层**，新建
`rust/crates/gd3-ui/src/card.rs`（~750 行，含 8 个测试）+ 金文件
`tests/fixtures/card.jsonl`（44 条）+ 生成脚本
`tests/fixtures/generate_card_fixture.py`。

Python 侧卡片是「数据驱动的表」：`FieldSpec.formats` / `ButtonSpec.states`
两张 `{status: callable}` 表 + `refresh()` 里
`formats.get(task.status) or formats.get(None)` 的分发。本步把这两张表与分发
**摊平成函数**，逐条对齐：

- `to_eta_text` ← `toEtaText`（`fileSize>0 and speed>0` → `int((总量-已收)/速度)`，
  `int()` 向零截断故用 `as i64`；已收超总量时得负数，单测钉了 `-3s`）；
- `info_fields` ← `TaskCard.infoFields` 三段（`speed`/`eta` 只在 RUNNING 出现，
  `size` 走 `{None: toSizeText}` 全状态出现）；`size` 复用 `row::size_text`，
  为此把 `row::to_size_text(record, received)` 抽成
  `row::size_text(status, file_size, received)` + 薄包装（既有调用点不动）；
- `error_text` ← `toLocalizedError` 的**无译文层**：`translate(message) or message`
  取 message，再 `format_map(params)` 单遍替换 `{key}`；缺参数保留字面
  （Python 抛 `KeyError`，我方构造器恒带全参，走不到）；
- `status_line` ← `_refreshForStatus`（RUNNING 隐藏；COMPLETED 三分支
  「文件不存在」（告警色）/「完成于 {text}」/「任务已经完成」；FAILED 走
  `error_text` 兜底「下载过程中发生错误，请稍后重试」；PAUSED/WAITING 固定文案）
  + `MultiFileTaskCard` 覆盖（`files>1` 且非「文件不存在」且状态 ∈
  {WAITING, COMPLETED} → 改写成「{已选}/{总数} 个文件」）；
- `progress_state` ← `_createProgressBar`（`fileSize ∈ {0, -1}` → 不确定进度条）
  + `_refreshForStatus` 的 `show`/`hide`/`setError`/`pause`；
- `toggle_button` / `select_files_button` / `checksum_button` /
  `open_file_button` ← 四个 `ButtonSpec.states`。

分层边界（与「IO/译文/本地时区留在调用方」的既有约定一致）：`output_exists`
（`isExisting`）、`completed_at_text`（`datetime.fromtimestamp(...).strftime`
**本地时区**，`gd3-ui` 无 chrono）、`can_pause`/`can_seed`/`is_seeding`/
`has_output_file`（`Task` 运行时/类属性，`tasks.jsonl` 里没有）全由调用方给。

### 两处口径修正（本步踩到并修掉）

1. **进度条可见性是三态不是布尔**：Python `_refreshForStatus` 只在 RUNNING 里
   `show()`、COMPLETED 里 `hide()`，FAILED 与 PAUSED/WAITING 分支**根本不碰**
   可见性（沿用上一次的值）。故金文件记动作 `show`/`hide`/`keep`，Rust 侧用
   `ProgressVisibility` 枚举 + `visible()`（`!= Hide`）兜底无状态重渲染。
   生成脚本里 `_last()` 对无参调用返回空元组 `()`（falsy），一度把所有 case
   都判成 `keep`——改用 `_visibility()` 判 `is not None` 后
   RUNNING→show / COMPLETED→hide / 其余→keep 全部对上。
2. **`ButtonState.icon` 是「生效图标」不是「覆盖值」**：Python 是
   `setIcon(state.icon if state.icon is not None else spec.icon)`，永远调
   `setIcon`。故 Rust 侧改成 `&'static str`（不再 `Option`），并导出四个 spec
   图标常量 `TOGGLE_ICON="PLAY"` / `SELECT_FILES_ICON="LIBRARY"` /
   `CHECKSUM_ICON="FINGERPRINT"` / `OPEN_FILE_ICON="LINK"`（取自
   `ButtonSpec(...)` 实参），`ButtonState::DEFAULT` 换成
   `ButtonState::default_with(icon)`。

### 金文件做法

与第 48/49 步一致：**桩替换 widget，跑真方法**。

- `_FakeMixin + TaskCard` / `_FakeMixin + MultiFileTaskCard` 两个桩类，
  **不调用 `__init__`**（跳过 QWidget 初始化），类属性 `infoFields` /
  `buttons` / `nameFormats` 仍是真的；
- 所有 widget 换成 `Stub`（`__getattr__` 记录 `(method, args)`），事后回放；
- `_createProgressBar` **跑真方法**：monkeypatch 模块级
  `IndeterminateProgressBar` / `ProgressBar` 为带 `kind` 标记的桩，
  `fileSize ∈ {UNKNOWN, NOT_SUPPORTED}` 这个分支由真代码判定，脚本不抄；
- `Task` / `TaskFile` / `TaskStep` / `TaskError` 全是真对象，于是 `task.canPause`
  （遍历 steps 找 RUNNING）、`task.lastError`（倒序两轮）、
  `task.currentSnapshot()` 都由真代码算出；
- `_NoPauseStep(TaskStep)` 子类把 `canPause` 置 False（无注解 → 不是 dataclass
  字段），`isExisting` 逐例 monkeypatch 固定文件系统事实。

44 条用例覆盖：五状态基本盘 / RUNNING 的总量未知与不支持与速度为零与不可暂停
与进度截断（42.9→42、99.99→99）/ PAUSED / FAILED 五种错误文案与 lastError
两轮倒序 / COMPLETED 十种（无完成时间、有完成时间、文件不存在、无输出文件、
做种中、可做种未做种、可做种但文件不存在、总量为零、总量不支持、进度 100）/
多文件八种（2 选 1、3 选 2、运行中、暂停、失败、完成全选、完成文件不存在、
单元素列表）/ 文件列表为空与 None / 字段无影响七种。

### 验证

- 金文件 `card.jsonl` 44 条，连跑两次生成脚本 md5 相同
  （`846d8df4ccb97f570f6233dc4a3aff05`）。
- `cargo fmt --all --check` 通过。
- `cargo clippy --all-targets -- -D warnings`（全工作区）通过。
- `cargo test -p gd3-ui`：**71 passed**（63 → 71）。
- `cargo test -p gd3-core -p gd3-net -p gd3-servers`：**237 passed**（不变）。
- Python 全量：`pytest -q --continue-on-collection-errors` →
  **3 failed, 1347 passed, 8 skipped, 35 errors**，与基线完全相同。
  本步只动 Rust（未接进 Python 侧），不涉及影子开关，故无需双模式回归。

### 卡点

无

### 下一步（Phase 7c 续）

1. 把派生层接到 `TaskRow`：`ui/task_page.slint` 的 `TaskRow` 补
   `speedText`/`etaText`/`sizeText`/`statusLine`/`statusWarn`/`progress*`/
   `toggleIcon`/`toggleEnabled`/`openFileEnabled`/`selectFilesVisible`/
   `checksum*`，`appwindow.slint` 加 `taskOpenFile`/`taskOpenFolder` 回调，
   `row::to_task_row` 改为 `(record, &CardTask)` 两参数。
2. 之后：任务抽屉 `task_drawer.py` 派生层 → 暂停/删除真实语义 →
   引擎状态桥接 → Python UI 停用 + `AGENT-DONE`。

## 2026-10-17 12:15 第 51 步（Phase 7c）：把卡片派生层接到 Slint `TaskRow`

### 做了什么

- **`ui/task_page.slint` 的 `TaskRow` 换成卡片字段集**（22 字段）：
  `taskId` / `name` / `status` / `speedText`+`speedVisible` / `etaText`+
  `etaVisible` / `sizeText`+`sizeVisible` / `statusLine`+`statusLineVisible`+
  `statusWarn` / `progressValue`(int)+`progressIndeterminate`+`progressVisible`+
  `progressError` / `toggleIcon`+`toggleEnabled` / `selectFilesVisible` /
  `checksumVisible`+`checksumEnabled` / `openFileEnabled`。
  **删掉**旧的 `statusText` / `progressText` / `progress: float`：
  - `statusText` 是抽屉口径（`task_drawer.py::_toStatusText`），列表行在 Qt
    侧是**卡片**，状态行走 `_refreshForStatus`（= `statusLine`），口径不同；
  - `progressText` 由 `sizeText` 取代（信息行的 SIZE_FIELD）；
  - `progress: float` → `progressValue: int`，镜像 `setValue(int(progress))`
    的截断（99.99 → 99）。
- 新增 `component CardButton`（`PrimaryToolButton`/`TransparentToolButton` 最小
  同构：`label` + `enabled` + `clicked`）；行内 `ProgressIndicator` 绑
  `progressValue / 100` + `indeterminate` + `visible`；状态色点按 `status`
  字面量分派；状态行颜色 `statusWarn ? #C8A050 : #666666`。
- 新增 5 个回调：`taskToggle` / `taskSelectFiles` / `taskChecksum` /
  `taskOpenFile` / `taskOpenFolder`（`appwindow.slint` 声明 + 从 `TaskPage`
  转发）。
- **`card.rs` 新增 `CardRuntime` + `CardTask::from_record`**：`status`/
  `fileSize`/快照/文件计数从记录里取，调用方只补记录里没有的运行时事实
  （`completed_at_text` / `can_pause` / `can_seed` / `is_seeding` /
  `has_output_file` / `output_exists` / `last_error`），`Default` = 可暂停、
  不可做种、未做种、有输出文件、文件在、无完成时间、无错误。
- **`row.rs`：`to_task_row(record, &CardRuntime)` 改为纯装配**，文案全部取自
  `card::info_fields` / `status_line` / `progress_state` / `toggle_button` /
  `select_files_button` / `checksum_button` / `open_file_button`；删
  `to_task_row_with`。`size_text`（被 `card::info_fields` 复用）与
  `to_status_text`（留给后续任务抽屉）保留。
- **`lib.rs`**：`push_task_records` / `refresh_task_rows` 的
  `is_seeding: impl Fn(&TaskRecord) -> bool` 换成
  `runtime: impl Fn(&TaskRecord) -> CardRuntime`；`TaskActions` 补
  `toggle`/`select_files`/`checksum`/`open_file`/`open_folder` 五字段 +
  `install_task_actions` 注册 `on_taskToggle` 等五个生成名。

### 口径决策

- `progressPaused`（Python `progressBar.pause()`）**不进 `TaskRow`**：
  Slint 的 `ProgressIndicator` 没有「暂停动画」的对应属性，`paused` 仍留在
  `card::ProgressState` 里供主题对齐步消费。
- 「打开文件夹」「删除」两个按钮在 `TaskRow` 里**没有可见性字段**：Python
  侧它们的 `spec.states is None`，`_refreshButtons` 直接 `continue`，恒可见
  恒可用，Slint 侧写死即可。
- 冒烟测试里 `TaskRow` 字面量**逐字段写全**（虽然 slint 生成的 struct 有
  `derive(Default)`）：这条测试的职责就是「字段清单能进 model 再读回来」，
  用 `..Default::default()` 会让新增字段悄悄漏测。`CardRuntime` 则用
  `CardRuntime { ..Default::default() }`（`clippy::field_reassign_with_default`
  也会拦「先 default 再赋值」）。

### 验证

- `cargo fmt --all --check` 通过。
- `cargo clippy --all-targets -- -D warnings`（全工作区）通过。
- `cargo test -p gd3-ui`：**73 passed**（71 → 73；`row.rs` 的整行装配测试
  拆成 RUNNING / COMPLETED / 多文件三条）。
- `cargo test -p gd3-core -p gd3-net -p gd3-servers`：**237 passed**（不变）。
- Python 全量：`pytest -q --continue-on-collection-errors` →
  **3 failed, 1347 passed, 8 skipped, 35 errors**，与基线完全相同
  （本步只动 Rust UI 层，Python 侧没有任何 `gd3_ui` 引用，不涉及影子开关）。

### 卡点

无

### 下一步（Phase 7c 续）

1. 任务抽屉 `task_drawer.py::DetailCard` 派生层（`_toStatusText` 已有
   `row::to_status_text`，还差「速度/剩余时间/分享链接/校验和」那几段）。
2. 暂停/删除/继续的真实语义（`_onToggleClicked` 的状态分派 + 队列推进）。
3. 引擎状态桥接（`TaskService` 信号 → `push_task_records` 的 `CardRuntime`
   供给方：`isExisting` 走文件系统、`completedAt` 走本地时区）。
4. 之后：Python UI 停用 + `AGENT-DONE`。

## 2026-10-17 12:33 第 52 步
- 做了什么：
  - 新建 `rust/crates/gd3-ui/src/drawer.rs`（957 行，11 个测试），把
    `app/view/dialogs/task_drawer.py` 里五张卡片的 `refresh()` 摊平成纯函数：
    `header_card` / `error_bar` / `info_card` / `checksum_card` / `file_list_card`
    + `file_status_text` / `file_list_needs_refresh`，配套五个结果 struct。
  - 常量 `DRAWER_WIDTH=592` / `SLIDE_DURATION=300` / `CAPTION_WIDTH=88` /
    `ROW_BUTTON_SIZE=28` / `MATCH_COLORS`。
  - 原语：`to_time_text(timestamp, utc_offset_seconds)`（手写 Howard Hinnant
    `civil_from_days`，`gd3-ui` 没有 chrono）、`parse_checksum`、`match_checksum`、
    私有 `py_split_whitespace`（按 `pystr::py_isspace` 切，对齐 CPython 无参
    `str.split()`）。
  - `DrawerRuntime { card: CardRuntime, output_path, utc_offset_seconds,
    category_enabled, category_name, expected_checksum }`。
  - 金文件 `tests/fixtures/drawer.jsonl`（**48 条**，md5
    `77ceb08b5c0c2d16a195c108bf2ea2a4`，连跑两次相同）+ 生成脚本
    `generate_drawer_fixture.py`（708 行）；`drawer_layer_matches_python_fixture`
    逐条对比并断言总条数。
  - `lib.rs` 加 `pub mod drawer;`（按字母序插在 `conflict` 与 `filter` 之间）。
- 口径决策：
  - `DrawerRuntime` 内嵌 `CardRuntime` 而不是重列字段：抽屉的 `isSeeding` /
    `lastError` / `outputExists` 与卡片同源，`header_card` 直接
    `CardTask::from_record(record, &runtime.card)`。
  - `output_path` 由调用方给 `String`，不在 Rust 拼 `Path`：`str(Path / name)`
    的分隔符跨平台不一致，且调用方本就要算它来判 `outputExists`。
  - `to_time_text` 自己实现 `strftime("%Y-%m-%d %H:%M:%S")`，偏移由调用方给
    （DST 由调用方按当时生效偏移给）；金文件用
    `datetime.fromtimestamp(t, timezone(timedelta(seconds=off)))` 对齐，并断言
    offset=0 时与真 `toTimeText` 逐条相同。已知偏差：年份 < 1000 时 Rust 补到
    4 位、Python/glibc 不补（用例不覆盖）。
  - `checksum_card` 的行排序按**原始键**（`entries.sort_by` 后才 `to_uppercase()`）
    ——Python 是 `sorted(task.checksums.items())` 再 `algorithm.upper()`；
    已用 `checksum_rows_sort_by_raw_key_not_uppercased` 钉住（`"Sha256"` vs
    `"sha1"` 会得出相反顺序）。
  - `match_checksum` 走 dict 插入顺序（`serde_json::Map` 开 `preserve_order`），
    与行显示顺序（排序后）**不同**：两处分别对齐 Python 的 `next(...)` 与
    `sorted(...)`。
  - `ChecksumCard.refresh` 的 `task.checksums == self._shownChecksums` 缓存短路
    归调用方；`FileListCard.refresh` 的重算条件单独暴露为
    `file_list_needs_refresh(last, current) = !(last == Some(current) && current != RUNNING)`。
  - **金文件抓出的真实偏差**：`len(files) <= 1` 时 Python 在 `setVisible` 之后
    直接 `return`，`setRows` **从不调用**，表格里没有行；初版 `file_list_card`
    无条件生成行 → 被「文件-单文件隐藏」用例判死。已改为该分支返回空 `rows`，
    并把两条守卫（提前 return / 状态缓存）在 doc 里分开写清。
  - 金文件钉死 `outputFolder=Path("/downloads")`：本机
    `cfg.downloadFolder.value == "//Downloads"`，不钉死金文件无法跨机器复现。
  - 生成脚本用「真方法绑普通类 + 属性链桩」：`QWidget.isVisible()` 在父窗口
    未 show 时恒 False，`setVisible(True)` 读回仍 False，只能靠记录调用序列判
    可见性；`Stub.__getattr__` 返回可调用子 Stub 且共享日志，才能支持
    `bar.textLayout.setStretchFactor(...)` 这类链式调用。
  - `ErrorBar.refresh` 的复制按钮父是 InfoBar（桩），真
    `TransparentToolButton` / `ToolTipFilter` 构造会炸 → 两者一并换桩。
  - `_refreshMatch` 在 `refresh()` 之后显式再调一次：桩不跑 `_bind`，空
    `checksums` 时 Python 的缓存短路会让 `_refreshMatch` 不被调用；显式补调
    等价于「用户输入了期望值」，两侧口径才可比。
  - clippy 收口：`DrawerRuntime` 改 `derive(Default)`（`derivable_impls`）、
    `civil_from_days` 去掉多余的 `as i64`（`unnecessary_cast`）。
- 验证：
  - `cargo fmt --all --check` 通过。
  - `cargo clippy --all-targets -- -D warnings`（全工作区）通过。
  - `cargo test -p gd3-ui`：**84 passed**（73 → 84，新增 11 个 drawer 测试）。
  - `cargo test -p gd3-core -p gd3-net -p gd3-servers`：**237 passed**（不变）。
  - Python 全量：`pytest -q --continue-on-collection-errors` →
    **3 failed, 1347 passed, 8 skipped, 35 errors**，与基线完全相同
    （本步只动 Rust UI 层，不涉及影子开关）。
- 下一步：
  1. 第 53 步：把 `drawer.rs` 接到 Slint——新建 `ui/task_drawer.slint`
     （HeaderCard / ErrorBar / InfoCard / ChecksumCard / FileListCard 五个
     component + 滑入动画），`appwindow.slint` 加抽屉开关回调，`lib.rs` 加
     `open_task_drawer(record, &DrawerRuntime)` 装配函数 + 无头冒烟测试。
  2. 暂停/删除/继续的真实语义（`_onToggleClicked` 状态分派 + 队列推进）。
  3. 引擎状态桥接（`TaskService` 信号 → `CardRuntime`/`DrawerRuntime` 供给方）。
  4. 之后：Python UI 停用 + `AGENT-DONE`。
- 卡点：无

## 2026-10-17 12:51 第 53 步（Phase 7c）：把任务抽屉接到 Slint `TaskDrawer`
- 做了什么：
  - 新建 `rust/crates/gd3-ui/ui/task_drawer.slint`（483 行）：导出结构体
    `DrawerInfoRow` / `DrawerChecksumRow` / `DrawerFileRow`，私有组件
    `DrawerCaption`（宽 88px = `CAPTION_WIDTH`）/ `DrawerSectionTitle` /
    `DrawerRowButton`（28px = `ROW_BUTTON_SIZE`），导出 `TaskDrawer`：
    HeaderCard 5 属性 + ErrorBar 3 属性 + `infoRows` + ChecksumCard 7 属性
    （含 `in-out expectedChecksum`）+ FileListCard 3 属性，6 个回调。
    遮罩 `rgba(0,0,0,0.4)`（= `QColor(0,0,0,102)`）+ 面板 `x: opened ?
    width - self.width : width`，`animate x { duration: 300ms; ease-out }`。
  - `ui/appwindow.slint`（337 → 425 行）：import 抽屉三结构体 + `TaskDrawer`；
    19 个 `in-out drawer*` 属性；6 个回调（`taskOpenDrawer` +
    `drawerCloseRequested` / `drawerExpectedChecksumEdited` /
    `drawerPasteChecksumRequested` / `drawerCopyTextRequested` /
    `drawerRevealInFolderRequested`）；窗口体外层套
    `Rectangle { width: 100%; height: 100%; clip: true; }` 让抽屉与主内容同级
    叠放；末尾 `TaskDrawer` 实例（属性单向装配 + `expectedChecksum <=>` 双链
    + 5 路回调转发）。
  - `ui/task_page.slint`（293 → 304 行）：加 `callback taskOpenDrawer(string)`；
    行内新增「详情」`CardButton`（镜像卡片右键「任务详情」/点信息区
    `_openDrawer`）；**修进度条整除 bug**：`row.progressValue / 100` →
    `row.progressValue * 1.0 / 100.0`（Slint 里 int/int 是整除，恒得 0/1）。
  - `lib.rs`（2013 → 2201 行）：
    - `pub fn open_task_drawer(window, record, &DrawerRuntime)`：调五个
      `*_card` → 写 19 个 `drawer*` 属性（三个 model 走
      `ModelRc::new(VecModel::from(...))`）→ **末尾**才 `set_drawerVisible(true)`
      （镜像 Qt 先刷状态再 `show()`，避免滑入闪一帧空卡片）。
    - `pub fn close_task_drawer(window)`：置不可见 + `taskId` 归位（幂等；
      Slint 侧关闭按钮/遮罩已自行收起，镜像 Qt `reject()`）。
    - `pub type DrawerTextAction = Box<dyn Fn(&str, &str)>` + `pub struct
      DrawerActions { close, expected_edited, paste_checksum, copy_text,
      reveal_in_folder }` + `install_drawer_actions`（五路注册）。
    - `TaskActions` 补 `open_drawer` 字段 + `on_taskOpenDrawer` 注册。
    - 三个新冒烟测试：`open_task_drawer_assembles_states_headless`（六条信息行
      逐字段 + 校验命中 + 文件列表不可见）、
      `close_task_drawer_hides_and_clears_task_id_headless`、
      `install_drawer_actions_registers_handlers_headless`（五路 `invoke_*`）。
- 决策与理由：
  - **Slint 结构体字段避开保留名**：`shown` 代替 `visible`、`key` 代替 `name`。
  - **`Window` 只能有一个子元素自动铺满** → 整个窗口体外层套撑满的
    `Rectangle`（不参与布局），`TaskDrawer` 与之同级叠放。
  - **`ProgressIndicator` 无 error 态** → 抽屉头部进度条改自绘（底槽 + 填充
    `Rectangle`），把「值 + 错误色」一并表达。
  - **`LineEdit` 改不了边框色** → `checksumMatched` 用一枚 8px 色点表达
    `setCustomFocusedBorderColor(*MATCH_COLORS)`；`setToolTip` 降级为一行
    提示文字（Slint 无 hover tooltip）。
  - **「打开抽屉」用显式「详情」按钮**：Qt 侧是右键菜单 + 点信息区，Slint
    侧没有右键菜单。
  - **抽屉关闭「自行收起 + 通知」**（与第 47 步身份覆盖层「不自行收起」的
    差异：这里没有校验需要保持打开）。
  - **未镜像项记录在 `task_drawer.slint` 头注释**：`MaskDialogBase` 模态、
    `QFileIconProvider` 48×48 文件图标、`featureService.detailCards()` 插件卡、
    `toBezierCurve`（降级 `ease-out`）、滑出动画（`opened=false` 即隐藏）。
  - 首次编译报三处 `vertical-alignment: start` 不存在（Text 的是
    `top`/`center`/`bottom`），改 `top` 后一次通过；`min(592px, root.width)`
    与 `errorBox.preferred-height` 循环引用均未报错。
  - clippy：裸 `Box<dyn Fn(&str, &str)>` 被 `type_complexity` 拦 → 抽
    `DrawerTextAction` 别名。
- 验证：
  - `cargo fmt --all --check` 通过。
  - `cargo clippy --all-targets -- -D warnings`（全工作区）通过。
  - `cargo test -p gd3-ui`：**87 passed**（84 → 87）。
  - `cargo test -p gd3-core -p gd3-net -p gd3-servers`：**237 passed**（不变）。
  - Python 全量：**3 failed, 1347 passed, 8 skipped, 35 errors**，与基线完全相同。
- 下一步：
  1. 暂停/删除/继续的真实语义（`_onToggleClicked` 状态分派 + 队列推进）。
  2. 引擎状态桥接（`TaskService` 信号 → `CardRuntime`/`DrawerRuntime` 供给方：
     `isExisting` 走文件系统、`completedAt` 走本地时区）。
  3. 按 `VPS-MIGRATION.md` 的 P0→P6 表继续。
- 卡点：无

## 第 54 步（commit `1c6d21f`）
- 做了什么：
  - 新 `gd3-core/src/queue.rs`（282 行，7 个单测）：镜像
    `app/services/task_service.py` 的 `TaskQueue`。
    - `wait`（去重追加）/ `cancel`（waiting 移除首个 + running retain）/
      `run`（已有键**就地改 workId 保持插入位**）/ `done` / `work_id_of` /
      `is_running` / `is_waiting` / `waiting_order` / `move_to_front` /
      `running_count` / `running_ids` / `next_waiting`。
    - `move_to_front` 的目标顺序取**等待队列自身顺序**（不是参数顺序），
      全删后 `splice(0..0, targets)`。
    - `overflow_ids(max)` = `running[max..]`（`max >= len` 给空）。
    - `next_to_dispatch(&mut self, max)`：**会从 `waiting` 头部弹出**，
      `while running.len() + popped.len() < max`（`run()` 由调用方在拿到 id
      之后才调，所以要用 popped 长度补占位）。
    - `rebalance(&mut self, max, can_pause)`：遍历 `overflow_ids` 快照 →
      `cancel` + `wait`（追加尾部）→ 返回 moved。**不自己 pump**（`workId`
      只有调用方拿得到）。
  - `gd3-core/src/lib.rs`：`pub mod queue;`（字母序在 `m3u8` 与 `pair_token`
    之间）+ `pub use queue::TaskQueue;`。
  - 金文件 `gd3-core/tests/fixtures/task_queue.jsonl`（**18 条**）+
    `generate_task_queue_fixture.py`（跑真 `TaskQueue`，14 种 op；`pump` /
    `rebalance` 用本地函数照抄 `_pump` / `_rebalance` 循环体 + 假派发器
    `queue.run(tid, f"w{n}")`；每 case 记 `results` + `final{waiting, running}`）。
  - `gd3-core/tests/queue.rs`：`Replay { queue, serial }` 逐 op 回放 +
    `snapshot()` 末态比对 + `EXPECTED_CASES = 18`；另加
    `next_to_dispatch_pops_from_head_within_limit`。
  - `gd3-ui/src/card.rs`：`pub enum ToggleAction { StopSeeding, StartSeeding,
    Pause, Start }` + `pub fn toggle_action(status, is_seeding)`
    （`isSeeding` 优先于 `COMPLETED` 的短路链，镜像 `_onToggleClicked`）。
  - 金文件 `gd3-ui/tests/fixtures/toggle.jsonl`（**10 条** = 5 状态 ×
    2 `isSeeding`）+ `generate_toggle_fixture.py`（把真 `_onToggleClicked` 绑到
    `FakeCard` 上跑，`FakeService` 记录被调方法名，`refresh()` 计数）。
  - 金文件实测：`WAITING/PAUSED/FAILED × false → start`；`RUNNING × false →
    pause`；`COMPLETED × false → startSeeding`；**所有 `isSeeding=true` →
    stopSeeding**。
- 决策与理由：
  - **`next_to_dispatch` 必须「弹出」而非「只读」**——初版写成纯读
    （`waiting.iter().take(room)`），金文件 `pump-under-limit` 立刻暴露：
    Python `_pump` 每轮 `nextWaiting()` 会真把 id 从 `_waiting` 摘掉。这是
    金文件抓出的**第二处真实语义偏差**。
  - **`rebalance` 只做「退回等待队列」不做随后的 `_pump`**——`_pump` 需要
    `workId`，只有调用方拿得到，所以 `rebalance` 返回 moved、调用方自己接着
    `next_to_dispatch`。
  - **`toggle_action` 放在 `card.rs`**——它是 `_onToggleClicked` 的分派逻辑，
    与 `toggle_button` 同源；不接服务层，只返回枚举。
  - **单测里构造「运行中」必须走 `next_waiting` + `run` 两步**——直接 `run()`
    不会把 id 从 `waiting` 里摘掉（Python 亦然），第一版单测因此把
    `next_to_dispatch` 的期望算错。
- 验证：
  - `cargo fmt --all --check` 通过。
  - `cargo clippy --all-targets -- -D warnings`（全工作区）通过。
  - `cargo test -p gd3-core -p gd3-net -p gd3-servers`：**246 passed**（237 → 246）。
  - `cargo test -p gd3-ui`：**88 passed**（87 → 88）。
  - Python 全量：**3 failed, 1347 passed, 8 skipped, 35 errors**，= 基线。
- 下一步：
  1. 引擎状态桥接：`CardRuntime` / `DrawerRuntime` 的供给方（`isExisting` 走
     文件系统、`completedAt` 走本地时区、`canPause` 走 steps 遍历）。
  2. 暂停/删除/继续的真实语义（`toggle_action` → 服务层调用）。
  3. 按 `VPS-MIGRATION.md` 的 P0→P6 表继续。
- 卡点：无

## 第 55 步（commit 待填）
- 做了什么：
  - 新 `gd3-core/src/path.rs`（126 行，4 个单测）：镜像 `pathlib.PurePosixPath`
    的两条口径。
    - `posix_join(folder, name)`：镜像 `str(Path(folder) / name)`——空组件与
      `.` 组件丢弃、重复斜杠压成一个、**恰好两个**前导斜杠保留（`//Downloads/n`）、
      `..` 不消解。`std::path::Path::join` 一条都不管，所以手写。
    - `posix_stem(name)`：镜像 `PurePosixPath(name).stem`（`a.tar.gz` → `a.tar`）。
      跟本项目的 `filename::split_stem_ext`（`("a", ".tar.gz")`）不是一回事。
    - **仅覆盖 POSIX 口径**——迁移目标是 Linux，`tasks.jsonl` 里的 `outputFolder`
      也是 Python `PosixPath` 写出来的。
  - `gd3-core/src/lib.rs`：`pub mod path;` + `pub use path::posix_join;`。
  - `gd3-core/src/task.rs`：
    - 新 `pub fn short_type_name(type_name: &str) -> &str`（全名取最后一段，
      裸类名原样返回——旧 `tasks.jsonl` 只存裸类名）。
    - `impl TaskRecord` 增 `is_output_folder()` / `output_path()` / `short_type_name()`。
      `output_path()` 镜像 `Task.outputPath = str(outputFolder / name)`，唯一被
      覆盖的是 `BilibiliTask`（`isOutputFolder` 时取 `outputFolder / Path(name).stem`）。
  - 新 `gd3-ui/src/facts.rs`（523 行，11 个单测）：任务运行时事实的供给方。
    - 类属性登记表：`TaskKind{can_seed, has_output_file}`、
      `SEEDING_TASKS = ["BTTask","ED2kTask"]`、
      `StepPauseRule{Always,Never,RangeRequests,NotLive}`、
      `NEVER_PAUSE_STEPS`（6）、`RANGE_PAUSE_STEPS`（10）。
    - `task_kind` / `step_pause_rule` / `task_is_live` / `step_can_pause` /
      `can_pause`（取**第一个 RUNNING 步骤**，无 RUNNING 则 `True`）。
    - `is_existing(path)`（`fs::metadata().is_ok()`）/ `completed_at_text`
      （`completed_at == 0` → `None`，镜像 `elif task.completedAt:` 的假值判断）。
    - `LiveFacts{is_seeding, last_error}` / `DrawerEnv{utc_offset_seconds,
      category_enabled, category_name, expected_checksum}` /
      `card_runtime` / `drawer_runtime`。
  - 金文件（`gd3-ui/tests/fixtures/`，脚本 `generate_facts_fixture.py` 531 行）：
    - `task_registry.jsonl`：**29 行**（9 个 Task 含基类 + 20 个 TaskStep 含基类），
      md5 `590ad13d0ffa1f31e73d0cca15427866`。
    - `task_facts.jsonl`：**60 条**，md5 `ab1adc51679548022dc94dfb10e197ce`。
  - `facts.rs` 的两个金文件比对测试：
    - `registry_matches_python_fixture`：逐条比对全名 + 裸类名两种 tag；再反向断言
      `SEEDING_TASKS` / `NEVER_PAUSE_STEPS` / `RANGE_PAUSE_STEPS` 里的名字在
      Python 登记表里仍存在（Python 删类 → 报红）；断言 tasks==9 / steps==20。
    - `facts_match_python_fixture`：60 条，比对 `can_pause`/`can_seed`/
      `has_output_file`/`completed_at_text`/`record.output_path()`，外加
      `drawer_runtime` 的三个同源字段；断言 cases==60。
  - `lib.rs` 新增端到端冒烟 `facts_runtime_drives_task_rows_headless`：
    `facts::card_runtime` → `push_task_records` → 断言三行内容
    （做种包 COMPLETED 输出存在 → PLAY + 三按钮可点 + 「完成于 …」；
     `HttpTaskStep` 缺 `canUseRangeRequests` → PAUSE 禁用；
     输出不存在 → 「文件不存在」+ warn + 按钮禁用）。临时文件落在
    `std::env::temp_dir()/gd3-facts-e2e`。
- 决策与理由：
  - **`posix_join` / `posix_stem` 手写，不用 `std::path::Path`**——实测 CPython：
    `Path("")/"n"` → `n`、`Path(".")/"n"` → `n`、`Path("a//b")/"n"` → `a/b/n`、
    `Path("//Downloads")/"n"` → `//Downloads/n`、`Path("///x")/"n"` → `/x/n`、
    `Path("/a/../b")/"n"` → `/a/../b/n`。`Path::join` 一条都不管。
  - **`posix_stem` 的入参是最后一段组件**，不是整条路径：`Path("a/b.txt").stem`
    里 `name == "b.txt"`；`.` 与 `..` 在 `name` 阶段已被 `pathlib` 消掉
    （`Path(".").name == ""`）。实测补了 `..a` → `.`、`a.b.` → `a.b.`、
    `.a.b` → `.a`、`...` → `...` 四个反直觉用例（初版把 `..a` 的期望写错，
    被单测抓出来）。
  - **类属性登记表用手抄 + 金文件钉死**：`canPause` 有 4 种形态（`True` 类属性 /
    `False` 类属性 / 读 `canUseRangeRequests` 的 property / 读 `not task.isLive`
    的 property）。生成脚本用 `inspect.getattr_static(cls, "canPause")` 拿原样，
    property 再读 `fget.__code__.co_names` 判断读的是哪个字段；**都不匹配就让脚本
    炸掉**，不静默算成 `always`。
  - **`has_output_file` 恒为 `true`**——`Task._registry` 全量导出显示无任何子类
    覆盖 `hasOutputFile`；留字段是为了跟 Python 形状对齐。
  - **`last_error` / `is_seeding` 只能由调用方给**——`step.error` 是 `repr=False`，
    `fromDict` 之后恒为 `None`（不落盘）；`isSeeding` 是 `init=False` 的进程内状态。
  - **`task_facts.jsonl` 的 record 由真对象 `toDict()` 出来、expected 由真属性算出**
    ——脚本不抄算法；`base_record()` 内部走一遍 `fromDict` + `toDict`，把
    `updateStatus()` 对 `status`/`completedAt`/`fileSize` 的改写一并固化。
  - **`BTTask` / `FtpTask` 不构造实例**——前者 `__post_init__` 要求 `files` 非 None
    并求和（实测 `TypeError: 'NoneType' object is not iterable`），后者要求
    `connectionInfo` 且会 `addStep`。改用 `phantom_task(models, record)`：
    `object.__new__(cls)` 只填 `name`/`outputFolder`/`completedAt`/`status`/
    `isLive`/`files`/`steps`，绕开 `__init__`/`__post_init__`，读的仍是**真属性**。
  - **`M3U8TaskStep` 的 live 用例必须先 `fromDict`（步骤 WAITING）再手工置 RUNNING**
    ——`M3U8Task.__post_init__` 会把「live + RUNNING」归一成 COMPLETED。
  - **不出「所有步骤 COMPLETED」的用例**——`updateStatus()` 会往 `completedAt` 写
    `int(time())`，金文件不稳定。
  - **`bili-output` 用例的名字必须带扩展名**（`"My Show.mp4"` / `"a.tar.gz"`）——
    否则多文件 `Path(name).stem` 与单文件 `outputFolder / name` 两条分支结果相同。
  - **`isExisting` 不进金文件**——取决于跑测试的机器，由 `is_existing_reports_the_
    filesystem` 单测（`"/"` / 不存在路径 / `""`）覆盖。
  - **`utcOffset` 作为用例字段显式给出**——生成脚本 `os.environ["TZ"]="UTC"` +
    `time.tzset()` 让默认偏移恒 0，非零偏移用例（`3600` / `-18000`）用
    `timezone(timedelta(seconds=off))` 显式算。
- 验证：
  - `cargo fmt --all --check` 通过。
  - `cargo clippy --all-targets -- -D warnings`（全工作区）通过。
  - `cargo test -p gd3-core -p gd3-net -p gd3-servers`：**250 passed**（246 → 250，
    +4 个 `path.rs` 单测）。
  - `cargo test -p gd3-ui`：**100 passed**（88 → 100：`facts.rs` 11 个单测 +
    1 个端到端冒烟）。
  - Python 全量：**3 failed, 1347 passed, 8 skipped, 35 errors**，= 基线。
- 下一步：
  1. 暂停/删除/继续的真实语义（`toggle_action` → 服务层调用）。
  2. 按 `VPS-MIGRATION.md` 的 P0→P6 表继续（P1 设置页剩余卡片）。
- 卡点：无

## 2026-10-17 14:25 第 56 步
- 做了什么：
  - 新建 `rust/crates/gd3-ui/src/window.rs`（**592 行，12 个单测**），
    `lib.rs` 加 `pub mod window;`（字母序在 `speed` 之后）。这是主窗口
    `main_window.py` 的**派生层**：只搬「不依赖 Qt 部件」的判定与文案。
    - 导航表：`PAGE_TASK`/`PAGE_SETTINGS`/`ROUTE_ADD_TASK` 三个路由键、
      `NavPosition`、`NavItem`、`NAV_ITEMS: [NavItem; 3]`（`下载任务`/`新建任务`/
      `设置`，图标名 `DOWNLOAD`/`ADD`/`SETTING`，`addTaskButton` 不可选中）。
    - 搜索框：`search_target(route_key)` → `SearchTarget { placeholder, accepts_text }`
      （`TaskPage`→`搜索任务`、`SettingPage`→`搜索设置`、其余 → `None`/`false`）；
      `search_edit_geometry(tb_w, tb_h, edit_h)` → `SearchGeometry { width, x, y }`。
    - 拖放：`DropInput`/`DropDecision`/`drop_decision(input, matches)`（镜像
      `dragEnterEvent` 四级短路）/ `drop_urls(input, matches)`（镜像 `dropEvent`
      的取址：有 URL 取 URL，否则取能 match 的文本行）。
    - 关闭：`PLATFORM_*`、`MODE_ASK`/`MODE_BACKGROUND`/`MODE_QUIT`、
      `CloseAction`（`RejectModal`/`ShowNormalThenClose`/`HideToTray`/`Ask`/`Quit`/
      `Close`）、`close_action(platform, mode, modal_visible, fullscreen, quitting)`、
      `AskOutcome` + `resolve_ask(quit, remember)`。
    - 文案：`pair_request_text`（`PAIR_REQUEST_TITLE`/`ALLOW`/`DENY` 三个按钮 +
      两处 `or` 兜底）、`LISTEN_FAILED_*`（浏览器/aria2 两条 InfoBar）、
      `UPDATE_AVAILABLE_*` + `update_available_content`、`EXCEPTION_*` +
      `exception_text`、`CLOSE_ASK_*`（含「记住我的选择」）。
    - 更新提示：`PROGRESS_TOAST_STATES`（`DOWNLOADING`/`READY`/`FAILED`）+
      `UPDATE_TARGET_APP` + `wants_progress_toast(target_id, state)`。
  - 金文件 `rust/crates/gd3-ui/tests/fixtures/window.jsonl`：**119 条**，
    md5 `2fe3b123b6edde6dccc588ed53db06c7`。分布：`drop` 48 / `close` 26 /
    `update-changed` 12 / `pair` 10 / `geometry` 9 / `search` 4 / `exception` 4 /
    `update-available` 3 / `listen-failed` 2 / `nav` 1。
  - 生成脚本 `tests/fixtures/generate_window_fixture.py`（~960 行）：把真方法绑到
    桩宿主上跑（`NavHost` 绑 `_addPage`、`SearchHost` 绑 `_updateSearchTarget`、
    `GeometryHost` 绑 `_refreshSearchEditGeometry`、`DragHost` 绑
    `dragEnterEvent`/`dropEvent`、`CloseHost` 绑 `_onCloseClicked`、`NotifyHost` 绑
    `_onListenFailed`），Qt 部件（`MessageBox`/`CheckBox`/`InfoBar`/`PushButton`/
    `TransparentToolButton`/`ToolTipFilter`/`ProgressToast`/`QTimer`/`QApplication`/
    `QHBoxLayout`）全换成记录型替身。
  - `window.rs` 的 `mod tests` 新增 `matches_python_main_window_fixture`：按 `kind`
    逐条重算并比对，末尾断言 `cases.len() == 119` 与各 kind 计数
    `(nav, search, geometry) == (1, 4, 9)`、`(drop, close, pair) == (48, 26, 10)`、
    `(listen, update, exception, changed) == (2, 3, 4, 12)`。
- 决策与理由：
  - **只搬「不依赖 Qt 部件」的判定与文案**——`DropOverlay` 自绘、Mica/Acrylic、
    主题色、窗口几何持久化的落盘、`setupPacks` 都在头注释里记为未镜像，
    等 Slint 侧真做窗口外壳时再定。
  - **`search_edit_geometry` 用 `div_euclid(2)` 而不是 `/`**——Python `//` 向下取整，
    Rust `/` 向零截断，负差时不同（`(-5)//2 == -3` vs `-2`）。金文件里
    `(800, 21, 32)` → `y = -6` 就是这条的钉子。
  - **`close_action` 保持 Python 短路链顺序**（可见模态框 → macOS 全屏 →
    macOS 非退出模式 → ASK → QUIT/Background），ASK 拆成「返回 `Ask`」+
    调用方拿 `resolve_ask(quit, remember)` 两段，因为 `dialog.exec()` 是阻塞调用。
  - **`MODE_*` 常量直接引用 `close_mode::OPTIONS[i]`**，并加单测
    `mode_literals_share_close_mode_options` 断言两者同源，避免字面量漂移。
  - **`pair_request_text` 的兜底用 `filter(|v| !v.is_empty())`**——Python
    `x or fallback` 是假值判断，`None` 与空串都兜底。
  - **金文件的 `search` 用例里 `PlaceholderOnly` 探针不做 routeKey 比对**——Python
    靠 `hasattr` 判定、Rust 靠 routeKey 查表，pack 页面在 Rust 侧一律按「没有」
    处理；测试里显式 `continue` 并注释说明，只钉住「两个 flag 在 Python 里互相
    独立」这一事实，**不静默容忍分歧**。
  - **`close` 用例的 `cfg.set` 键名由生成脚本归一**——`FakeCfg.set` 把
    `cfg.closeMode`（SimpleNamespace）当键记进 JSON 会 `TypeError`，改为记键名
    `"closeMode"`/`"geometry"`；Rust 侧只比对「geometry 写了几次 / closeMode 写了
    几次 + 值」，几何内容归调用方。
  - **生成脚本踩的 6 个坑**（都修了）：`FakeLayout` 缺 `addWidget`；`edit` 工具的
    `PUT a.=b:` 把 `FakeMsgBox.__init__` 首两行吃掉（用 Python 脚本还原）；
    `FakeCfg` 缺 `geometry` 属性；`NotifyHost._onListenFailed = None` 占位把真方法
    盖掉（改在用例里 `types.MethodType(MainWindow._onListenFailed, host)`）；
    `dialog.titleLabel`/`logButton` 不是 QWidget → `titleRow.addWidget` 抛
    `TypeError`（改为补丁 `main_window.QHBoxLayout`）。
  - **clippy `manual_clamp`**：`(tb_w - 300).min(360).max(200)` 被拦，改
    `(tb_w - 300).clamp(200, 360)`（`200 <= 360` 故等价）。
- 验证：
  - `cargo fmt --all --check` 通过。
  - `cargo clippy --all-targets -- -D warnings`（全工作区）通过。
  - `cargo test -p gd3-core -p gd3-net -p gd3-servers`：**250 passed**。
  - `cargo test -p gd3-ui`：**100 → 112 passed**（12 个 `window::tests::*`，
    含 `matches_python_main_window_fixture ... ok`）。
  - Python 全量：**3 failed, 1347 passed, 8 skipped, 35 errors**，= 基线。
- 下一步：
  1. P0 主链路收口：把 `toggle_action` 接到服务层（暂停/继续/删除的真调用）。
  2. `VPS-MIGRATION.md` 的 P1：设置页剩余卡片（网络/下载/其他三个分区）。
- 卡点：无

## 2026-10-17 15:41 第 57 步
- 做了什么：主窗口外壳装配（导航栏 / 标题栏搜索框 / 通知条 / 消息框 / 拖放）
  接到 Slint（接上一轮未提交的半成品并收口）。
  - 新增 `rust/crates/gd3-ui/ui/app_shell.slint`（244 行）：`NavItemData` /
    `BarButtonData` 两个结构体 + `ShellButton`（`PushButton`/`PrimaryPushButton`
    两态）/ `NavButton`（选中态 = 左侧 3px 指示条 + 底色，`selectable=false`
    永不选中）/ `ShellInfoBar`（标题 + 正文 + 按钮组 + 固定关闭按钮，
    镜像 `isClosable=True`）/ `ShellMessageDialog`（模态遮罩 + 面板 +
    `checkboxLabel == ""` 表示没有复选框）。
  - `appwindow.slint` 重排为 `_initLayout` 的形状：左导航栏 190px
    （`navTop` → 弹性空隙 → `navBottom`）+ 右侧 VerticalLayout
    （标题栏 48px 含搜索框 + 页面栈）；覆盖层按声明顺序决定 z 序
    （主内容 → 通知条 → 抽屉 → 消息框 → 拖放区）。
  - `window.rs` 补「形状层」（+190 行）：`nav_items_at(NavPosition)`、
    `BarButton`/`InfoBarSpec`/`DialogSpec`、`DIALOG_OK`/`DIALOG_CANCEL`
    （「确定」/「取消」，实证 `MessageBoxBase.__init__` 用 `tr('OK')`/`tr('Cancel')`
    且本仓库装了中文 translator）、四个构造函数
    `listen_failed_info_bar` / `update_available_info_bar` /
    `close_ask_dialog` / `exception_dialog` / `pair_request_dialog`。
  - `lib.rs` 补装配层（+155 行）：`nav_model` / `set_nav_items` /
    `switch_page` / `bar_buttons` / `show_info_bar` / `hide_info_bar` /
    `show_dialog` / `hide_dialog` / `dialog_button_text` / `DropPayload`
    （`from_transfer` + `drop_input`）/ `ShellActions` + `install_shell_actions`。
- 决策与理由：
  - **`switch_page` 一定清空 `searchText`**——镜像 `_updateSearchTarget`
    无条件先 `self.searchEdit.clear()`。
  - **`DropPayload.has_urls` 按 `has_file_paths()` 判定**：Slint 1.18 的
    `data-transfer` 只暴露本地文件路径与纯文本（`i-slint-compiler-1.18.1/
    builtin_elements.rs` 里 URL 列表未导出），与 Qt 的 `mimeData().urls()`
    有差异，已写进结构体文档注释，等 slint 暴露再补。
  - **按钮分派只回传序号**（`BarButtonData.index` = Python `addWidget(...)`
    的顺序），按钮身份 → 行为的映射留在调用方，派生层不持有行为。
  - **`dialogCheckboxLabel == ""` 当「没有复选框」**——Slint 没有 Option
    属性，用空串承载 `textLayout.addWidget(checkbox)` 的有无。
  - **`nav_model` 的 `selected` 只对 `selectable` 项可能为真**——Qt 侧
    `_addPage` 只对真页面调 `setSelected`，「新建任务」从未进入选中态。
- 验证：
  - `cargo fmt --all --check` 通过；`cargo clippy --all-targets -- -D warnings`
    0 error。
  - `cargo test -p gd3-ui`：**112 → 115 passed**（新增 3 个无头冒烟：
    `shell_nav_and_search_follow_route_headless` /
    `shell_info_bar_and_dialog_assembly_headless` /
    `drop_payload_maps_to_drop_input`）。
  - `cargo test -p gd3-core -p gd3-net -p gd3-servers`：250 passed（未受影响，
    本步只动 gd3-ui）。
- 下一步：
  1. P0 收口：核对 `cards/task_cards.py` 与 `card.rs` 的覆盖差（按钮态 /
     右键菜单 / 卡片高度）。
  2. 之后按 P1 推进设置页剩余卡片。
- 卡点：无

## 2026-10-17 16:03 第 58 步（commit `6a3f81f`）
- 做了什么：任务卡片的**交互层** `rust/crates/gd3-ui/src/card_menu.rs`（720 行，
  含 `mod tests`）——把 `task_cards.py` 里「只在鼠标与菜单里发生」的判定摊平。
  - `CardClick`（7 变体 `OpenDrawer`/`CopyUrl`/`EditTask`/`MoveToFront`/
    `Redownload`/`SetCategory(id)`/`StartChecksum(alg)`）+ `dispatch()`
    返回 `(sink, call)`，与金文件的 `invoke.calls[0]` 同义；
  - `CardMenuAction`/`CardMenuSubmenu`/`CardMenuEntry`（动作 / 分隔符 / 子菜单）；
  - `ContextMenuInput` + `context_menu()`（镜像 `createContextMenu()` 的
    `addAction` 顺序：任务详情 → 分隔符 → 复制下载链接 →
    [`canEdit and status != COMPLETED`] 编辑任务参数... → 移到最前 → 重新下载 →
    [`cfg.isCategoryEnabled.value`] 子菜单「移动到分类」= 未分类 → 分隔符 → 每分类）；
  - `ChecksumMenuInput` + `checksum_menu()`（`COMMON_ALGORITHMS` 进顶层、其余进
    「更多」，`checked = algorithm in task.checksums`，文案 `algorithm.upper()`，
    **无图标**，`更多` 无条件 `addMenu`）；
  - `ChecksumOutcome` + `checksum_click(in_progress, is_folder, output_exists, menu)`：
    三条短路（校验中 → `cancelChecksum`；目录或文件不存在 →
    `_setStatus("文件不存在，无法校验")`；否则弹菜单），菜单用 `impl FnOnce()`
    惰性构造（对应 Python 的局部变量只在最后一条分支里建）；
  - `can_drag()` = `COMPLETED and hasOutputFile and isExisting(outputPath)`；
  - `ReleaseGesture`/`ReleaseInput`/`release_gesture()`（名字区 → 打开文件、
    信息区 → 抽屉、其余 → 选择，`checked = extend or not selection_mode or not is_checked`）；
  - `double_click_opens_file()`、`category_menu_icon()`（`getattr(FluentIcon, name,
    FluentIcon.TAG)` 的 TAG 兜底）、常量 `MENU_*`/`ICON_*`/`COMMON_ALGORITHMS`。
  - 金文件 `tests/fixtures/card_menu.jsonl`：**133 条**，md5
    `feaadabeec516549f134aa33eb7b5f9b`（两次跑 `cmp` 字节相同，确定性已验证）。
    分布：`context-menu` 16 / `checksum-click` 5 / `can-drag` 16 / `release` 96。
  - 另出 `tests/fixtures/fluent_icons.json`（**174** 个 `FluentIcon` 成员名）。
  - 生成脚本 `tests/fixtures/generate_card_menu_fixture.py`（579 行）：不实例化
    `TaskCard.__init__`，改用 `task_cards.CardWidget.__init__(self)` 建真 QObject，
    `Action`/`RoundMenu` 换记录型替身并**真跑 `triggered` 回调**。
- 决策与理由：
  - **`Action` 是 `singledispatchmethod`，按第一个位置参数类型分派**：
    `Action(text: str, parent)` 与 `Action(icon, text, parent)` 两种重载，
    `task_cards.py` 两种都用。按固定三参写桩会把算法名当图标、把卡片当文本，
    金文件里出现 `<_Card>`；改成 `__init__(self, *args)` 按
    `isinstance(first, str)` 分流。
  - **「点一项菜单会造成什么」是调用序列，不是单个调用**：`_onEditClicked` =
    `featureService.createEditDialog` → `refresh()` → `taskService.checksumProgress`。
    替身改成**一个按发生顺序排列的共享日志** `_CALL_LOG`（每条带 `sink` 标签
    `task`/`feature`/`clipboard`/`drawer`），而不是每个服务各记一份。
  - **`_fake_card` 把 `self.window()` 桩成返回字符串 `"window"`**：真 Qt 里无父
    窗口时返回卡片自身，会让金文件出现 `<_Card>` 这种实现细节。
  - **`_plain()` 不用 `repr`**（带内存地址，金文件不稳定），改 `<ClassName>`，
    并加 `os.PathLike → str` 前置分支。
  - **`ALGORITHMS = sorted(hashlib.algorithms_available)` 是平台相关的**
    （OpenSSL 版本决定），算法表由金文件带出、Rust 侧当输入收
    （`checksum_menu(algorithms, …)`），不硬编码；只有
    `COMMON_ALGORITHMS = ("md5","sha1","sha256","sha512")` 是仓库常量。
  - **`FluentIcon` 成员表单独输出成 `fluent_icons.json`**，Rust 侧镜像
    `getattr(FluentIcon, name, FluentIcon.TAG)` 的 TAG 兜底。
  - **Rust 侧 `CardClick` 只描述意图**，真服务调用留装配层——与第 57 步
    `BarButtonData.index` 同一原则。
- 验证：
  - `cargo fmt --all --check` 通过；`cargo clippy -p gd3-ui --all-targets -- -D warnings`
    0 error。
  - `cargo test -p gd3-ui`：**115 → 128 passed**（新增 4 个 `card_menu::tests::*`，
    其中 `matches_python_card_menu_fixture` 逐条重算 133 条：项数 / 类型 / 文案 /
    图标 / `checked` / `invoke.calls[0]` 的 sink+call+args，`assert_menu` 递归比对）。
- 下一步：Slint 侧装配（`menu_view.rs` + `ui/card_menu.slint` + 无头冒烟）。
- 卡点：无

## 2026-10-17 17:39 第 59 步：卡片右键菜单的 Slint 装配（摊平视图 + 覆盖层）
- 做了什么：
  - 新增 `rust/crates/gd3-ui/src/menu_view.rs`（444 行，含 `mod tests`）：
    `MenuRowKind`（`code()` = Slint 侧 0 动作 / 1 分隔符 / 2 子菜单）、
    `MenuRow`（kind/depth/text/icon/checked/checkable/expanded/path）、
    `CardMenuView`（`entries` + `expanded: Vec<Vec<usize>>`）→ `rows()` 摊平、
    `toggle(row)`、`click(row) -> Option<CardClick>`、`entry_at(path)` 逐级解析；
    行高常量 `PANEL_ROW_ACTION_HEIGHT=30` / `PANEL_ROW_SEPARATOR_HEIGHT=9` /
    `PANEL_PADDING=4` + `panel_height(rows)`。
  - 新增 `rust/crates/gd3-ui/ui/card_menu.slint`（160 行）：`CardMenuRowData`
    struct + `CardMenuPopup`（`opened`/`anchorX`/`anchorY`/`panelHeight`/`rows` +
    `activated(int)`/`dismissed`）；外层 TouchArea 点空白关闭、面板 TouchArea
    吞点击；动作行 30px、分隔符 1px 线、勾选框 `checked` 打勾、子菜单 `›`/`⌄`、
    `depth * 16px` 缩进。
  - `ui/appwindow.slint`：加 `import { CardMenuPopup, CardMenuRowData }`、
    5 个属性 + 2 个回调，覆盖层插在消息框之后、拖放区之前（z 序：菜单在对话框
    之上，拖放区仍在最顶）。
  - `lib.rs`：`pub mod menu_view;` + `CardMenuActions`（`activated(&CardClick)` /
    `dismissed`）+ `card_menu_rows()` / `render_card_menu()` /
    `show_card_menu(window, view, x, y, actions)` / `hide_card_menu()`；
    点击分派：子菜单行 → `toggle` + 重画且**不关**，动作行 → 收起 + 回调，
    分隔符/越界 → 什么都不做（菜单也不关）。
  - 新增无头冒烟 `smoke::card_menu_overlay_assembly_headless`：装配 → 断言行字段
    映射（`icon: None` → `""`、`kind`/`depth`/`checkable`）与 `panelHeight` →
    点分隔符无反应 → 点子菜单行展开（7 → 11 行，`depth=1`，非法图标已兜底
    `TAG`）→ 点分类行收起并回传 `SetCategory("cat_video")` → 再弹一次展开态不残留
    → `dismissed` 收起且不产生动作 → 校验菜单 3 行（MD5 打勾、无图标、`更多` 是
    子菜单）→ 点算法行回传 `StartChecksum("sha256")`。
- 决策与理由：
  - **子菜单从「弹出窗口」改成「缩进 + 点击展开」，是刻意偏离**：Slint 没有多
    弹出窗口，Qt 的 `addMenu` 子菜单悬停即展开。语义上「能点到的动作集合」完全
    一致（折叠只影响可见性），展开态记在 `CardMenuView.expanded`。
  - **面板高度由 Rust 算**：Slint 布局无法把「子项高度之和」回传给父元素，
    所以 `panel_height()` 按行高常量求和后推给 `CardMenuPopup.panelHeight`；
    slint 侧行高写死同像素值，两处互相注释引用，`panel_height_matches_row_kinds`
    测试钉住总和口径。
  - **锚点/高度用 `<int>` 像素而不是 `<length>`**：值全部来自 Rust 侧像素算术，
    走 int 边界避免 `f32` 换算，Slint 侧 `* 1px`。
  - **分隔符行不回调**：Qt 的 `addSeparator()` 不可点。Slint 侧 `if r.kind != 1`
    才发 `activated`，Rust 侧再兜一层（`click()` 返回 `None` → 不关菜单）。
  - **每次 `show_card_menu` 都新建视图与回调**：Qt 侧 `createContextMenu()` 每次
    造新 `RoundMenu`，子菜单展开态不跨次残留。
  - **`expanded` 用路径而不是摊平下标**：展开会改变行号，路径稳定。
- 验证：
  - `cargo fmt --all --check` 通过；`cargo clippy --all-targets -- -D warnings`
    0 error（全工作区）。
  - `cargo test -p gd3-ui`：**128 passed**（第 58 步的 4 个 `card_menu` 测试 +
    本步 8 个 `menu_view` 测试 + 1 个无头冒烟）。
- 运维提示（本轮踩到）：**磁盘满了**。`cargo test -p gd3-core -p gd3-net
  -p gd3-servers -p gd3-ui` 链接 `gd3-net` 的 `hf_head` 测试二进制时
  `ld terminated with signal 7 [Bus error]` + `No space left on device`，
  `/` 100%（40G）。删掉 `rust/target/debug/incremental`（7.9G）后回到 85%，
  但被写坏的 `i-slint-compiler`/`slint` 产物导致重建，全量测试要重跑。
  后续建议：每次开工前看一眼 `df -h /`，`target/debug/incremental` 是最大的
  可安全回收项。
- 下一步：
  1. 等重建完成后补跑 `cargo test -p gd3-core -p gd3-net -p gd3-servers -p gd3-ui`
     与 Python 全量，确认无新增失败。
  2. P0 收口：`toggle_action` 接服务层（暂停/继续/删除的真调用）。
  3. 之后按 P1 推进设置页剩余卡片（`setting_cards.py` 949 / `setting_card_group.py`
     290 / `card_groups.py` 191）。
- 卡点：磁盘（已回收 8G，重建中）

## 2026-10-17 19:42 第 61 步：设置页分组容器语义层（折叠/搜索/排序）
- 做了什么：
  - **磁盘回收**：`find rust/target/debug/deps -maxdepth 1 -type f
    ! -newermt "2026-10-17 18:10" ! -name libgd3_py.so -delete` 删掉 5024 个
    profile 切换前的 debug=2 残留（16.18 GB），`/` 从 94%（2.3G 可用）回到
    **52%（19G 可用）**。删后 `cargo test -p gd3-ui --no-run` **2.59s 直接
    Finished**，证明 cargo 未引用任何被删产物，无需重建。
  - 新增 `rust/crates/gd3-ui/src/settings_group.rs`（429 行，含 11 个单测）：
    镜像 `setting_card_group.py` `CollapsibleSettingCardGroup` +
    `setting_page.py` 的 `setSearchText` / `_restoreOrder` / `_saveOrder` /
    `updateArrows`。
    - 常量 `GROUP_*`（8 个 key：general/category/association/browser/aria2rpc/
      personalization/software/about）+ `PAGE_GROUPS` 创建顺序 + `group_title`
      （`.tr` 字面量，未知 key 返回空串不编造）。
    - `is_collapsed` / `toggle_expand`（展开 append、折叠 remove，保持顺序）/
      `restore_order`（stored 有效 key 在前 → 其余按创建顺序 → about 强制最后）/
      `save_order`（`current + 过期 key` 追加保留）/ `update_arrows`。
    - `CardText` / `GroupCards` / `GroupSearch` + `normalize_search`
      （`strip().lower()`）/ `matches_card` / `search_group` / `search_all`。
- 实证结论（写进模块头）：
  - 折叠态存 `UI/ExpandedSettingGroups`（cfg.py:322，StringList 缺省 `[]`），
    **列表里没有 key 就是折叠**。
  - **搜索只改视觉展开态，不写 cfg**：`expand = (text and hasMatch) or key in
    expanded`，清空搜索后回到 cfg 口径。页面空态判据是「任一分组可见」。
  - 可折叠卡（Proxy/Identity/HeadersPreset）的搜索文本取**头部**标题/副标题
    （`isSearchMatch` 把 `CollapsibleSettingCard` 换成 `card.card`）。
  - 拖拽排序写回**不丢弃过期 key**（追加尾部），下次启动 `_restoreOrder` 才过滤。
- 验证：
  - `cargo fmt --all --check` 通过；`cargo clippy --all-targets -- -D warnings`
    0 warning（8m37s）。
  - `cargo test -p gd3-ui`：**134 → 145 passed**。
  - 其中 2 个测试首轮写错了期望（`ghost downloader` 只命中 software 组第一张卡；
    `剪贴板` 会命中 software 组导致断言前提失效），改测试不改实现——实现与
    Python 源码逐条对应。
- 下一步：
  1. `settings_spec.rs`：把 `setting_page.py::_initCards` 的**全部卡片清单**
     （分组归属 + 顺序 + 控件类型 + 标题/副标题 + 配置键 + 默认值/范围/候选文案）
     做成声明式表，补上 Slint 侧尚缺的卡：应用主题、语言、数据存储模式、
     关于组四卡（了解作者/提供反馈+查看日志/功能包/关于+版本行）、
     关联组的功能包文件类型与协议卡。
  2. `ui/setting_card_row.slint` + `ui/settings_group.slint`：数据驱动渲染
     （替代现在 settings_page.slint 里手写的一排 HorizontalBox），接
     `settings_group.rs` 的折叠/搜索/排序，出无头冒烟。
  3. 之后按 P0 收口 → P2。
- 卡点：无（磁盘已回收 16 GB）

## 2026-10-17 22:53 第 62 步：设置页卡片清单声明式表（settings_spec.rs，47 张卡）
- 做了什么：
  - 新增 `rust/crates/gd3-ui/src/settings_spec.rs`（1293 行，含 12 个单测），
    把 `setting_page.py::_initCards`（:83-388）的**全部卡片**抽成一张可断言的表：
    `ConfigKey{section,key}` / `Control`（Switch / Range{min,max,step,unit,scale} /
    Combo{literals,labels} / Button / Folder / Custom）/ `CardSpec{id,group,title,
    content,control,config,default_config,platforms}` / `Platforms` 位标志。
  - 分组常量复用 `settings_group`（general / category / association / browser /
    aria2rpc / personalization / software / about），顺序与 `_initLayout` 一致。
  - 派生函数：`card_by_id` / `cards_in_group(group, platform)` / `visible_cards` /
    `search_groups(platform)`（喂 `settings_group::search_all`）/
    `combo_index`（未命中回 0）/ `combo_literal`（越界回首项）/
    `Scale::{to_display,to_config}`（One / BytesPerKb / FractionToPercent）/
    `about_content` / `author_content` / `pack_association_card`。
  - `lib.rs` 加 `pub mod settings_spec;`。
- 实证结论（写进模块头注释）：
  - `RangeConfigItem` 的范围是**配置口径**；`SpinBoxSettingCard` 用 `division`
    换算成显示口径（`setRange(int(r[0]*division), int(r[1]*division))`），
    `PercentSpinBoxSettingCard` 固定 ×100、`decimals=0`。表里存**显示口径**。
  - **`setSingleStep()` 不吸附网格**：`setValue()` 直接写值（setting_cards.py:63-70），
    所以 `speedLimitation` 缺省 4194304 字节 → 4096 KB/s 在 min=1/step=512 下
    不是网格点。原先的「缺省值必须在 step 网格上」断言是**测试写错**，已删并注明理由。
  - 下拉卡缺省：`themeMode` Auto=索引 2（候选 `[Light, Dark, Auto]`）、
    `language` Auto=索引 8（9 项，`LANGUAGE_TEXTS` 无 AUTO 项 → 标签回退
    「使用系统设置」）、`nameConflict` `keepBoth`=0、`closeMode` `Ask`=0、
    `backgroundEffect` win32 `[Acrylic,Mica,MicaAlt,Aero,None]` 缺省 Mica、
    darwin `[Acrylic,None]` 缺省 Acrylic、**Linux/Android 上该键不存在**。
  - 平台分支：`association` 组整体只在非 darwin 创建（setting_page.py:64-65）；
    `closeMode` / `storage_mode` 仅非 Android；Dock/菜单栏速度三卡仅 darwin。
  - 搜索口径 = 标题 + 副标题**子串**（`isSearchMatch`），所以搜「代理」在 general
    组命中**两张**：`use_system_dns`（副标题「兼容 TUN、VPN 和代理等网络环境」）
    与 `proxy`。原测试期望只有 `[10]` 是错的，改成 `[9, 10]`。
- 验证：
  - `cargo fmt --all --check` 通过；`cargo clippy --all-targets -- -D warnings`
    0 error（全工作区）。
  - `cargo test -p gd3-ui --lib`：**145 → 159 passed**（12 个 `settings_spec` 测试）。
  - 运维踩坑：一次 `cargo test` 在 `Compiling gd3-ui` 卡满 900s（EXIT=124），
    检查无 rustc 残留进程、内存 267M/939M 空闲 → 判定为上一次超时杀掉 rustc 后
    incremental 产物半写导致的退化；重跑 3 分钟完成。**结论：超时后先重跑一次，
    不要立刻怀疑测试代码。**
- 下一步：
  1. 补齐表里还缺的卡：aria2 组四卡（启用/端口/令牌/模拟指纹）、浏览器组三张
     动作卡（配对令牌/商店安装/Chromium 一键安装）。
  2. `ui/setting_card_row.slint` + `ui/settings_group.slint` 数据驱动渲染，
     接 `settings_group` 的折叠/搜索/排序，出无头冒烟。
- 卡点：无

## 第 63 步：任务草稿状态机 draft.rs（P2 起点）（2026-10-17 23:10）
- 做了什么：新增 `rust/crates/gd3-ui/src/draft.rs`（994 行，15 个单测），
  镜像 `app/services/task_draft.py::TaskDraft` 的信号顺序 + `app/view/dialogs/
  task_draft.py` 的纯函数：
  - `DraftStore`：`set_urls` / `confirm` / `clear` / `refresh` / `add_parsed_tasks`
    / `set_base_options` / `set_url_category` / `update` / `on_parsed` /
    `on_parse_failed`；用「返回 `Vec<DraftEvent>`」替代 Python 的 `Signal`。
  - 纯函数：`parse_url_lines`、`append_unique_urls`、`clipboard_urls`、
    `draft_stats`（镜像 `card_groups.py::updateStats`）、`error_url_display`
    （>48 字符截 45+"..."）、`import_name_filters`（`;;` 连接）、`file_uri`
    （`Path.as_uri()` 口径）、`draft_group_height`。
  - `DraftParser` trait 注入假解析器（Rust 侧没有 coroutineRunner），测试可控
    提交/取消/迟到回调。
- 实证结论：
  - **信号顺序**：`setUrls` = busy → 逐条 failed → items；`refresh` = busy →
    failed → items；`clear` = **itemsCleared 先于 busy**；`confirm` = 逐条
    taskConfirmed → 取消未记选项的项 → **busy 先于 itemsCleared**（与 clear
    相反）。原模块头把 confirm 写成「itemsCleared + busy」，已改正。
  - `confirm` 会 `self._items.clear()`，但 Python 靠回调闭包捕获的 item 对象
    继续持有 `confirmedOptions`。Rust 侧没有对象引用，若只按 parseId 在
    `items` 里找，「确认后解析回来再建任务」会**静默丢任务**（测试当场抓到：
    `on_parsed` 返回空）。修法：新增 `pending_confirm: parseId ->
    (options, autoStart)`，`on_parsed`/`on_parse_failed` 先查这张表。
  - `addParsedTasks` 即使一项都没处理（全是已解析的重复项），末尾仍无条件发
    `parsingBusyChanged` + `itemsChanged`（:154-156）；只有 `tasks` 为空才
    提前 return。原测试断言「空事件」是**测试写错**，已改。
  - `setUrls` 的 `difflib.SequenceMatcher` 等价性：输入已按首次出现去重
    （:100-101），两侧元素唯一 → 匹配块恰好是交集，因此「按新顺序取旧项、
    缺失则新建」观测等价（只在取消调用次序上有差异）。
- 验证：
  - `cargo fmt --all --check` 通过；`cargo clippy --all-targets -- -D warnings`
    0 error（全工作区）。
  - `cargo test -p gd3-ui --lib`：**159 → 177 passed**（18 个 draft 测试）。
- 下一步：第 64 步 `plan_task.rs`（计划任务摘要，100 行）+ `batch_url.rs`
  （批量 URL 解析，184 行）语义层。
- 卡点：无

## 第 64 步：计划任务 + 批量 URL 语义层（P2）（2026-10-17 23:35）
- 做了什么：两个新模块（`lib.rs` 加 `pub mod plan;` / `pub mod batch_url;`）：
  - `plan.rs`（409 行，12 测试）：`PlanAction`（IntEnum 0..3）、`Plan`
    （`set`/`clear`/`trigger`）、`PlanCommand`（交给外壳层执行，UI 不做副作用）、
    `PlanDialogState`（缺省勾选关机、路径行可见性、`validate`）、
    `plan_button_clicked`（镜像 `task_page.py:481-494` 的勾选/接受/拒绝/取消
    四条路径，返回 `PlanButtonOutcome`）。
  - `batch_url.rs`（约 500 行，15 测试）：`expand_template` / `expand_range`
    完整镜像模板语法（`[1-100]`、`[001-050]` 补零、`[1-10:2]` 步长、`[a-z]`
    字母、`{mp4,mkv}` 枚举、笛卡尔积顺序）、`preview`（前 10 条 + `⋯` + 最后
    一条、`共 N 个链接`、`yesButton` 可用性、空模板全清）。
- 实证结论：
  - **Python `re.split` 带捕获组会返回 token，Rust `regex::split` 不返回捕获组**。
    第一版直接照抄 `TOKEN_PATTERN.split` 导致 token 全被丢掉（`http://e/img/.jpg`），
    测试当场抓到。改成手写 `split_tokens`（逐 match 取前后文本 + token），与
    Python 输出逐条对齐。
  - `Plan.trigger()` **先 `clear()` 再执行动作**（`plan.py:38-52`）；`clear()`
    只在注册过 `onCleared` 时回调。`trigger` 未满足条件时**保留**状态。
  - `validate()` 只拦「打开文件 + strip 后空路径」；`selectedFilePath()` 返回
    strip 后文本。
  - `_expandRange` 的补零宽度取**起点**长度（`01-100` → `01..99, 100`，
    不截断）；`[x-y]` 是**合法字母范围**（原测试把它当字面量是写错的），
    真正当字面量的是 `[1-x]` 这类数字/字母混用。
  - 交叉验证：把 `batch_url.py` 的纯函数段（`RANGE_PATTERN` 到 class 之前）
    抠出来用 Python 跑同一批 12 个模板 + 17 个 range 用例，输出与 Rust 侧
    **逐条一致**（含 `None` 分支）。
  - 保守偏离（已写进模块头）：非 ASCII 数字（`١-٣`）不当数字处理；单字符非
    ASCII 字母（`é`）Python 会 `ValueError`，Rust 返回 `None` 不 panic。
- 验证：
  - `cargo fmt --all --check` 通过；`cargo clippy --all-targets -- -D warnings`
    0 error（全工作区）。
  - `cargo test -p gd3-ui --lib`：**177 → 204 passed**（27 个新测试）。
- 下一步：第 65 步 `file_select.rs`（252 行）+ `name_conflict.rs`（82 行）
  语义层（文件选择对话框 / 重名提示队列）。
- 卡点：无（`PySide6` 未装在 venv 里，`app/view/dialogs/*` 无法直接 import，
  纯函数段靠「抠源码 exec」交叉验证）

## 第 65 步：文件选择 + 重名提示语义层（P2 收尾）（2026-10-18 00:32）
- 做了什么：两个新模块（`lib.rs` 加 `pub mod file_select;` / `pub mod name_conflict;`）：
  - `file_select.rs`（856 行，9 测试）：arena 勾选树（`nodes` + `parent/children`
    + `roots`）替代 `QStandardItemModel`；`rows()` 先序摊平成
    `TreeRow{path,name,depth,is_folder,size_text,check,file_index}` 给 Slint
    `ListView`；`set_check`/`set_selected_indexes`/`set_all`/`invert_selection`/
    `select_category`/`summary_text`/`validate` 逐条镜像
    `_onItemChanged`/`_setSelectedIndexes`/`_updateBranchCheckState`/
    `_updateAncestorCheckStates`；`type_menu` + `match_by_name` +
    `path_suffixes`（CPython 3.13 `Path.suffixes` 口径）。
  - `name_conflict.rs`（约 880 行，12 测试）：`NameConflictChoice`（StrEnum
    keepBoth/overwrite/ask）、`NameConflict`、`ProbeTarget`、`title_text`/
    `content_text`/`rest_checkbox_text`/`rest_checkbox_visible`、
    `NameConflictDialogState`（缺省 KeepBoth + 未勾选 + 隐藏标题栏）、
    `DialogOutcome`（`_onFinished` 两条路径）、`NameConflictQueue` +
    `ConflictActions` trait + `ConflictEvent`、`NameConflictPrompt` +
    `DialogRequest`。
- 实证结论：
  - **两个测试自身写错**（实现是对的，已改测试）：
    1. `video/a.mp4(selected=True)` + `video/b.mp4(selected=True)` 建树后
       `video` 是 **Checked** 不是 PartiallyChecked（`_updateBranchCheckState`
       全 Checked → Checked）；
    2. `select_category("cat_video")` 同时命中 `a.mp4` 与 `b.mkv`
       （分类 extensions 里两者都在）→ `[0,1]`，且 `v` 文件夹变 Checked。
  - `_updateAncestorCheckStates` **只看直接子节点**、不递归；空子列表按
    `all([])==True` 落 Checked（不可达但照抄）。
  - `selectedIndexes()` 只统计叶子（`_fileItems`），文件夹行不计入
    `已选择 {n}/{total}` 的 n。
  - `modifiedAt = int(st_mtime)` 是**向零截断**：`ProbeTarget.modified_at`
    保留 `f64`，截断放在队列里（`as i64` 与 `int()` 同口径），测试锁定
    `1.9 → 1`。
  - `setChoice`/`cancel` 的 taskId 校验失败时**连 refresh 都不做**；
    `add` 只在「无 conflict 且未 post」时 post 一次；`_refresh` 末尾
    **无条件** emit（可能为 None）；`isAppliedToRest` 切的是
    `tasks[:restCount]`（restCount 是探测那一刻的剩余长度）。
- 验证：
  - `cargo fmt --all --check` 通过；`cargo clippy --all-targets -- -D warnings`
    0 error（全工作区）。
  - `cargo test -p gd3-ui --lib`：**204 → 226 passed**（file_select 9 +
    name_conflict 12，另 1 个是原有）。
  - 正文四组字面量与 venv 实跑逐字节一致（TZ=UTC、`utc_offset_seconds=0`）。
- 下一步：P3 其它对话框（`edit_task.py` 127 / `pack_info.py` 106 /
  `subtitle_select.py` 88 / `extension_install.py` 133 / `release_info.py` 123）。
- 卡点：无

## 第 66 步：编辑任务对话框语义层（P3 起）（2026-10-18 01:05）
- 做了什么：新模块 `edit_task.rs`（457 行，12 测试，`lib.rs` 插
  `pub mod edit_task;` 于 `drawer` 与 `facts` 之间）：
  - 公共骨架常量：`TITLE=编辑任务参数`、`BUTTON_APPLY=应用`、
    `BUTTON_CANCEL=取消`、`MIN_WIDTH=680`、`CONFIRM_TITLE=确认更换链接`、
    `CONFIRM_TEMPLATE`（`新链接与原链接的内容不一致，将清除已下载的 {0} 数据，
    是否继续？`）、`PARSE_FAILED_TITLE=链接解析失败`、`INFO_BAR_DURATION_MS=4000`。
  - `take_url(&mut Options) -> String`：`options.remove("url")` + `trim()`
    —— **会删键**，这是 `pop` 的语义，也是「url 没变时 edit 收到的 options
    里没有 url」的根源。
  - `DraftEditState` + `DraftAccept{set_options, accepted}`。
  - `LiveEditState{task_url,pending_parse_id,interactive,pending_options}` +
    `set_interactive`/`progress_visible`/`yes_enabled`/`accept(options,&mut submit)`
    /`cancel_pending_parse`/`on_reparsed(can_reuse,received_bytes)`
    /`confirm_replacement(bool)`/`on_parse_failed(localized)`；
    `LiveAccept{Edit,Parsing}`、`ReparseOutcome{EditAndClose,Confirm,Restored,
    ParseFailed{..}}`。
- 验证：
  - `cargo fmt --all --check` 通过；`cargo clippy --all-targets -- -D warnings`
    0 error（全工作区）。
  - `cargo test -p gd3-ui --lib`：**226 → 237 passed**。
- 下一步：P3 剩余四个对话框 `pack_info.rs` / `subtitle_select.rs` /
  `extension_install.rs` / `release_info.rs`（写前需重读源码，上次输出被截断）。
- 卡点：无（`LiveEditState.pending_options` 显式承担 Python 闭包捕获
  `options` 的角色；`_onReparseFailed` 只收已本地化文本，UI 层不引 i18n 表）

## 第 67 步：包信息 + 字幕选择对话框语义层（P3）（2026-10-18 01:28）
- 做了什么：收口上一轮留下的两个未提交模块（`lib.rs` 两行注册一并入库）：
  - `pack_info.rs`（418 行，9 测试）：`UpdateState`（IntEnum 1..6 显式数值，
    供跨语言互认）+ `PackRowState`（`display_name`/`manifest_name`/`version_text`
    = `v{version}`/`status_glyph`/`status_colors`/`retry_visible`）+
    `PackInfoState`（`packs` → 行表，**`manifest is None` 直接跳过**，
    `_rows[manifest.name]` 重名后者覆盖前者）+ `apply_update(target_id, state)`
    （四状态表：AVAILABLE `↑`/DOWNLOADING `↓`/READY `✓`/FAILED `✗` + retry 显示，
    IDLE/CHECKING **无分支**、不改任何东西）+ `retry_target()`。
  - `subtitle_select.rs`（281 行，9 测试）：`SubtitleChoice`/`SubtitleRow` +
    `SubtitleSelectState`（`checked: Vec<bool>` 与 `choices` 等长——**同一 langCode
    出现两次时两行都勾上**，用位向量才能照抄 `set(selected)` 的成员测试）+
    `set_checked`/`set_all`/`checked_count`/`summary_text`（`{0}/{1} 种语言`）/
    `selected_languages`（**按模型顺序**，不是点击顺序）。
- 实证结论（写进两个模块头）：
  - 状态色四组都是 `(浅色, 深色)` 二元组，逐字进常量；
  - `PackRow` 初值 `✓` + success 色，`retryButton` **初始隐藏**；
  - `_rows` 的键是 `manifest.name` 而非类名 → 重名覆盖（测试锁「只有最后一个
    行收到更新，但所有行都进布局」）；
  - `_onUpdateChanged` 拿不到 target 就静默忽略。
- 验证：
  - `cargo fmt --all --check` 通过；`cargo clippy -p gd3-ui --all-targets -- -D warnings`
    0 error（8m34s）。
  - `cargo test -p gd3-ui --lib`：**237 → 255 passed**（pack_info 9 + subtitle_select 9）。
- 下一步：P3 剩余两个对话框 `extension_install.py`(133) / `release_info.py`(123)。
- 卡点：无

## 第 68 步：扩展安装 + 发行说明对话框语义层，P3 收官（2026-10-18 01:45）
- 做了什么：收口上一轮已写盘未提交的 `extension_install.rs`（331 行，7 测试），
  新写 `release_info.rs`（约 790 行，17 测试），`lib.rs` 两行注册
  （`extension_install` 于 `edit_task`/`facts` 之间，`release_info` 于
  `pystr`/`row` 之间）：
  - `extension_install.rs`：`MIN_WIDTH=768`、`MAX_HEIGHT=790`、
    `chip()` 的 `&nbsp;` 与 `font-family: 'Cascadia Code', Consolas, monospace;`
    逐字节；`ExtensionInstallState::new(path, dark, gif_available)` 把
    `QMovie.isValid()` 摊成入参，`install_action(opened)` 返回
    `InstallOutcome{reveal_path, chromium_url, opened, fallback}`。
  - `release_info.rs`：`Release`/`ReleaseAsset` + `from_response`
    （`tag_name`→`name` 取首个 strip 非空、`published_at` 空退 `created_at`、
    `version` 空兜 `Unknown`）；`published_date()` 用 `chars().take(10)`
    （**码点切片**，不是字节）；`description_markdown()` 走 `body or "暂无更新说明"`；
    `Platform{Windows{legacy},MacOs,Android,Linux}` + `arch_keywords` +
    `asset_score`（平台关键词 `40 - i*10` 取最大、架构子串硬门槛、按平台扩展名
    加分）+ `best_asset_index`（`score > bestScore` 初值 -1 → **同分取先出现者**，
    最终 `<0` 返回 None）；`ReleaseInfoState` 建行（`toReadableSize(size)` +
    `str(download_count)`）、按 `bestAsset` 预选、`validate()` = 有选中、
    `min_width(available, needed)` 复刻两段式
    `min(580, w-48)` → 有资产再 `max(旧, needed+68)`。
- 实证结论（写进两个模块头）：
  - `cancelButton` 在 `release_info.py` 里**没有** `setVisible(False)`，
    保持 `MessageBoxBase` 默认「取消」（与第 62 步实测一致）；
  - `dateLabel` 空串就空串，不补占位符；
  - `assetScore` 的 `windows7` 关键词只在 `isLessThanWin10()` 时参与加权，
    且 `setup` 是 `.exe` 加 100 的硬条件（裸 `.exe` 只有基础分）；
  - macOS 的 `mac` 是**第三**关键词 → 只给 20 分（`App-mac-x64.zip` 实测 70，
    不是 90）；
  - 初始选中靠 Qt 的**对象同一性**（`item.data(UserRole) is best`），
    行序与 `assets` 序一致，故等价于 `best_asset_index`。
- 验证：
  - `cargo fmt --all --check` 通过；`cargo clippy -p gd3-ui --all-targets -- -D warnings`
    0 error（11m04s）。
  - `cargo test -p gd3-ui --lib`：**255 → 279 passed**（extension_install 7 +
    release_info 17）。
- 下一步：P4 组件库（`app/view/components/` 12 文件 ~1900 行：`range_slider`、
  `band_selector`、`bootstrap_list`、`markdown_viewer`、`token_line_edit`、
  `track_bar`、`tree_view`、`scroll_area`、`banners`、`labels`、`option_cards`、
  `install_button`、`progress_toast`、`category_settings`、`editors`）。
- 卡点：无（`arch_keywords` 最初用 `Option<[&str;3]>` 装不下 2 元素组，
  编译期就报错，改成 `Option<&[&str]>`；`sizeHintForColumn` 的实测列宽
  由 Slint 侧测量后作为 `needed` 入参）

## 第 70 步：标签族 + 自适应编辑器（2026-10-18 03:14）
- 做了什么：`labels.rs`（约 490 行，11 测试）+ `editors.rs`（约 300 行，
  9 测试），`lib.rs` 两行注册（`editors` 于 `edit_task` 前、`labels` 于
  `menu_view` 前）：
  - `labels.rs`：`elide_right(text, width, advance)` 复刻 Qt
    `QFontMetrics::elidedText(ElideRight)`——空串/`width<=0`/放得下三短路、
    否则在 `width - advance("…")` 预算内二分最长前缀；`icon_minimum_height`
    = `max(size, fmHeight)`、`icon_indent` = `iconSize+4`（**无图标才 0**）、
    `icon_y_offset` 用 `div_euclid`（Python `//` 负数向下取整）；
    `IconCache`（`_iconCache` 的 `id(icon)` 键，同键只渲染一次）；
    `IconStrongBodyLabel`（`contentsWidth - indent` 省略 + 全文 tooltip）、
    `ElidedLabel`、`EditableLabel`（150ms `OutCubic`、`enter/leave/hide`、
    左键双击、下划线 y/长度/笔色）。
  - `editors.rs`：`py_round` 半值取偶（`round(2.5)==2`）；`EditMetrics::new`
    五段拼 `padding`（contents 上下 + viewport 上下 + `frame*2` +
    `round(documentMargin*2)`）；`AutoSizingEdit` 的
    `minimum_size_hint`（`min(minVisible, blockCount)`）与 `size_hint`
    （先按 `maxVisible` 截断再逐维 `expandedTo` 最小）；`combo_size_hint`
    = `advance + 60`；`FolderPicker` 的 `folder_path`（strip）、
    `folder_commit`（空不发信号）、`save_history`（删首个命中 + 插首 +
    截 20）、`refresh_history`（只跳空串，纯空白保留）、`browse_selected`。
- 实证结论（写进两个模块头）：
  - Qt 的 `elidedText` 在 `width <= 0` 时返回**原文**，不是空串；预算连
    省略号都放不下时只留 `…`；
  - `IconStrongBodyLabel._elide` 的宽度是 `contentsRect().width() - indent()`，
    而 `EditableLabel._elide` **不减 indent**（无图标）；
  - `EditableLabel` 下划线长度用 `super().text()`（已省略的显示文本），
    且 `_underlineProgress <= 0` 直接 return；
  - `AutoSizingEdit.sizeHint` 的 `expandedTo` 逐维取大 → `maxVisible` 小于
    `minVisible` 时高度会被抬回最小；
  - `refreshHistory` 的 `if folder:` 只过滤**空串**，`" "` 是真值。
- 验证：
  - `cargo fmt --all --check` 通过；`cargo clippy -p gd3-ui --all-targets --
    -D warnings` 0 error（11m49s）。
  - `cargo test -p gd3-ui --lib`：**297 → 317 passed**（labels 11 + editors 9）。
- 下一步：`track_bar.rs` + `band_selector.rs` + `progress_toast.rs` +
  `category_settings.rs`（P4 第二批），随后 `option_cards` / `token_line_edit`
  / `markdown_viewer` / `bootstrap_list` / `range_slider`。
- 卡点：无（clippy 单轮 11~12 分钟，故按「多模块攒一批再跑」推进）

## 第 71 步：P4 第二批四模块（track_bar / band_selector / progress_toast / category_settings）（2026-10-18 04:10）
- 做了什么：`lib.rs` 注册四模块（`band_selector` 于 `banners` 前、
  `category_settings` 于 `category` 后、`progress_toast` 于 `profile` 后、
  `track_bar` 于 `tree_view` 前），四模块合计 46 单测：
  - `track_bar.rs`（562 行，13 测试）：`TrackKind` 四轨道（`tooltip`
    视频/音频/字幕/封面、`icon` VIDEO/HEADPHONE/LANGUAGE/PHOTO、
    `is_primary` 只含 video/audio/cover、`has_menu` 只含 video/audio）、
    `TrackButtonState`（`icon_fill` 未勾选才 `#808080`）、
    `TrackBar::{set_checked→bool, set_track_enabled, prevent_all_off,
    set_options, menu_items, on_track_deselected, on_option_picked, click,
    layout_order}`。
  - `band_selector.rs`（603 行，13 测试）：`BandSignal` 三信号用
    `Vec` + `take_signals()` 缓冲；`mouse_press/move/release`、
    `on_scroll_changed`、`on_auto_scroll`、`cancel_drag`、`window_deactivate`、
    `update_band`（`first/last` 与自动滚动两侧边距）。
  - `progress_toast.rs`（521 行，11 测试）：`bar_color` 未知状态兜
    DOWNLOADING、`ToastIcon` 三态、`ProgressToastState::{apply, fill_tick,
    user_close, bar_width, bar_top}`（`{:.0}` 半值取偶）。
  - `category_settings.rs`（333 行，9 测试）：`to_category_name`（无译文层
    恒等）、`icon_or_tag`（`ICON_CHOICES ∪ {TAG}` 白名单）、
    `CategoryRow::from_rule`、`CategoryRulesCard::{new, reload, rows, content}`
    （`"已配置 {n} 个分类"`）、`CategoryRequest` 四值 +
    `edit_request`（`categoryById` 查空即 `None`）。
- 实证结论（写进四个模块头）：
  - `TrackButton.nextCheckState` 被 override 成 `pass` → 点击**永不**翻转
    勾选；`setTrackEnabled(False)` 因 `sender.isEnabled()` 为假而**可以真熄灯**；
    `_preventAllOff` 的 primary 不含字幕，但三盏全灭时字幕同样被拉回；
  - `setOptions` 的非法 `selected` 在**未勾选**时被完全忽略；`_onTrackDeselected`
    先 `setChecked(False)`，被拉回才保留 `_selectedValue`；
  - `band_selector._updateBand(lastX, pos.y())` 收的是**控件坐标**（不是视口
    坐标）——`top = min(startY, y)`、`bottom = max(startY, y)`，所以向上拖到
    负数时 `bottom` 仍等于 `startY`；`eventFilter` 首行 `if not _isEnabled:
    return False`（禁用时连 pending 都不清）；
  - `progress_toast.COLORS` 只有三态，`COLORS.get(state, COLORS[DOWNLOADING])`
    兜底；FAILED 把 `_displayProgress` 直接置 100 并停表；`paintEvent` 在
    `_displayProgress <= 0` 直接 return；
  - `category_settings` 的 `_toSummary` 与 `category.rs::summary_of` 同口径
    （超过 4 项时括号里的数字是**总项数**，video 预设实测 20 项）。
- 修掉的**测试算错**（实现均与 Python 一致，是断言写错）：
  - `band_selector::band_maps_rows_by_stride`：向上拖到 `-30` 时
    `bottom = max(10, -30) = 10` → `(0, 0)`，不是 `(-1, -1)`；
  - `band_selector::press_offsets_viewport_by_scroll_value`：`_updateBand` 用
    控件坐标 → `top = 200 → 5`，断言应为 `BandChanged(5, 6)`；
  - `track_bar::menu_items_mirror_open_menu`：`TrackBar` 初始就勾着 video+audio，
    测「未勾选」前必须先 `set_checked(Video, false)`；后半段测「勾选 + v2」前
    也要先补 `set_checked(Video, true)`；
  - `track_bar::option_pick_and_deselect`：「最后一盏 primary 熄不掉」的场景里
    video 必须已熄（原测试把它又点亮了，于是 audio 合法熄灯）；
  - `category_settings::new_preserves_category_order_and_ids`：video 预设是
    20 个扩展名，不是 8。
- 修掉的两处 clippy（`-D warnings`）：`progress_toast.rs` 的
  `doc_lazy_continuation`（收尾段升成独立 bullet）、`track_bar.rs` 的
  `collapsible_if`（改 let-chain，edition 2024）。
- 验证：
  - `cargo fmt --all --check` 通过；
  - `cargo clippy -p gd3-ui --all-targets -- -D warnings` 0 error（12m04s）；
  - `cargo test -p gd3-ui --lib`：**317 → 363 passed**（track_bar 13 +
    band_selector 13 + progress_toast 11 + category_settings 9）。
  - commit `5d215fb`。
- 下一步：P4 剩余组件——`option_cards.rs`(303)、`token_line_edit.rs`(273)、
  `bootstrap_list.rs`(266)、`markdown_viewer.rs`(215)、`range_slider.rs`(331)，
  然后 P1 设置页收尾（`setting_cards.rs` 949、`setting_card_group.rs` 290、
  `card_groups.rs` 191）。
- 卡点：无（clippy 单轮 12 分钟，仍按「多模块攒一批再跑」推进）

## 第 72 步：P4 收官五模块（range_slider / token_line_edit / option_cards / bootstrap_list / markdown_viewer）（2026-10-18 04:35）
- 做了什么：
  - `range_slider.rs`（1074 行，19 测试）：`HANDLE_SIZE 22` / `HANDLE_RADIUS 11` /
    `INNER_RADIUS 5` / `GROOVE_HEIGHT 4` / `LABEL_HEIGHT 24` / `LABEL_GAP 4` /
    `LABEL_PADDING_H 8` / `LABEL_PADDING_V 6` / `PREVIEW_W 160` / `PREVIEW_H 90` /
    `PREVIEW_BORDER 2` / `PREVIEW_MARGIN 6`；`RangeSliderState` 的 `set_range` /
    `set_values` / `to_pixel_x` / `to_value`（`editors::py_round` 半值取偶）/
    `label_rects`（重叠以 mid 轴对称分开、分开后**仍夹**到 `[0, width-w]`）/
    `mouse_press`（仅左键；`y < LABEL_HEIGHT` 只命中气泡且 end 优先）/
    `update_drag`（值变才发信号）/ `start_inline_edit`（56×24，`max(0, min(w-56, hx-28))`）/
    `finish_inline_edit`（**无条件**发信号）/ `update_preview`（`PreviewAction::Fetch`）。
  - `token_line_edit.rs`（604 行，12 测试）：15 个几何/尺寸常量 + `token_background` /
    `token_elide_width`（`width - 34`，可为负）/ `token_label_text` /
    `TokenWidgetState::{elide, label_text}` / `TokenKey::submits` /
    `TokenLineEdit::{key_press, focus_in, focus_out, underline, set_tokens, add_token,
    remove_index, remove_last, input_visible}`；`underline` 的可见带 = Python
    `barPath(0,h-10,w,10, r=5)` 减 `rectPath(0,h-10,w,8)` → `y = h-2`、`height = 2`、
    `radius = min(5, height/2) = 1`。
  - `option_cards.rs`（909 行，11 测试）：四张卡 `OutputFolderCard` /
    `SubworkerCountCard` / `ClientProfileCard` / `UrlEditCard` / `HeadersEditCard`；
    `build_profile_menu(include_auto)` 条目数**恒为 12**（1 首项 + raw + 分隔 + 9 家族），
    `include_auto=false` 只把首项的 value 换成空串；每家族 = 「最新」+ 分隔 + 版本。
  - `bootstrap_list.rs`（737 行，12 测试）：`to_ago_text`（60/3600/86400 三门槛，
    `(s/60).floor()`）/ `to_status_text`（有缓存不看 7 天门槛）/ `card_content`
    （`newest` 只取有缓存的、`hasError` 看全部）/ `is_subscription_url`（CPython
    `urlsplit`：只 lstrip C0+空格、删 `\t\r\n`、scheme 需 `i>0` 且首字符 ASCII 字母、
    netloc 到最早 `/?#`）/ `BootstrapListDialogState`（`validate()` 全通过才 `Commit`）。
  - `markdown_viewer.rs`（568 行，12 测试）：`AlertKind` 五值 + light/dark 色 +
    octicons path；`match_alert`（**大写 only**，输入是 Qt 已去 `>` 的块文本）；
    `insert_alert_separators`（`(?m)^[ \t]*>[ \t]*\[!KIND\].*(?:\n[ \t]*>.*)*`，
    每块后插 `"\n\n<!-- -->"`）；`alert_icon_svg` / `alert_heading_html` /
    `alert_body_html`；`size_for_lines` / `content_height` / `resolve_height`
    （**先 max 后 min**）/ `MarkdownViewerState`。
  - `profile.rs` 追加 `FAMILY_VERSIONS`（9 家族 131 项，wreq `Emulation` 成员快照）+
    `versions_of`；`identity.rs` 的 `IdentityPreset` 补 `Debug`（`ClientProfileCard`
    要 derive）；`lib.rs` 按字母序注册五模块（`pub mod` 47 + `mod format` = 48）。
- 实证结论（写进模块头）：
  - `ALERT_MARKER` 的 `^` **没有** `MULTILINE`，而 `_refreshAlertStyle` 传给
    `ALERT_MARKER.sub("", body, 1)` 的 body 是 Qt `toHtml()` 片段（以 `<p style=...>`
    开头）→ **实际运行时 marker 根本不会被去掉**；Rust 侧忠实保留这个锚定语义，
    并用 `body_html_only_strips_marker_at_body_start` 把两条分支都钉住。
  - `token_line_edit.paintEvent` 的下划线不是「y=h-10 高 10」，而是
    `barPath - rectPath` 剩下的 2px 带（`y = h-2`），`radius` 被高度夹成 1。
  - `_updateDrag` 在值没变时**不**发 `rangeChanged`，`_onInlineEditFinished` 却
    **无条件**发——两者口径不同，分别用 `update_drag -> Option<Range>` 与
    `finish_inline_edit -> Range` 表达。
  - `buildProfileMenu` 的 `includeAuto` 只影响首项，不影响条目数。
- 修掉的**测试算错**（实现均与 Python 一致，是断言写错）：
  - `markdown_viewer::viewer_state_clamps_size_hint`：`size_hint(95.0)` 内容本就高于
    最小 10，结果是 95 不是 100；
  - `option_cards::profile_menu_mirrors_build_profile_menu`：`include_auto=false`
    的条目数仍是 12；
  - `range_slider::label_rects_clamp_to_widget_bounds`：`endW` 是 `advance("100")+16
    = 40`（不是 24），夹取后 `end.x = 300 - 40 = 260`；
  - `range_slider::update_drag_clamps_and_only_reports_changes`：start 已被拖到 80
    后，End 分支再夹回 80 是**无变化**（`None`）；另起一个 slider 才测得到 End 发信号；
  - `token_line_edit::underline_only_when_focused`：`y` 是 `h-2 = 31`（不是 25）。
- 修掉的两处 clippy（`-D warnings`）：`markdown_viewer.rs` 的
  `doc_lazy_continuation`（bullet 里以 `+` 开头的续行被当成新列表项）、
  `token_line_edit.rs` 的 `collapsible_if`（改 let-chain）。
- 验证：
  - `cargo fmt --all --check` 通过；
  - `cargo test -p gd3-ui --lib`：**363 → 439 passed**；
  - `cargo clippy -p gd3-ui --all-targets -- -D warnings` 0 error（2m07s，增量）；
  - `cargo test -p gd3-core -p gd3-net -p gd3-servers -p gd3-ui` 全绿；
  - commit `8d67541`。
- 下一步：P1 设置页收尾——`setting_cards.rs`(949)、`setting_card_group.rs`(290)、
  `card_groups.rs`(191)；然后 P5 OOBE（`oobe_window.py` 971 行）→ P6 平台外壳。
- 卡点：无

## 第 73 步：P1 设置页收尾三模块（setting_cards / setting_card_group / card_groups）（2026-10-18 04:58）
- 做了什么：
  - `card_groups.rs`（551 行，14 测试）：`card_groups.py`(191) 的
    `DraftCardGroup` / `OptionCardGroup` 容器状态。
    `TITLE_PREFIX "    "` / `HEADER_HEIGHT 30` / `GROUP_RADIUS 5.0` /
    `GROUP_INSET (1,1,-1,-1)` / `STATS_FONT_SIZE 13` / `STATS_HEIGHT 30` /
    `HEADER_ROW_SPACING 8` / `RIGHT_PAD_TEXT "  "` / `TITLE_DRAFT "解析结果"` /
    `TITLE_OPTIONS "下载设置"` / `DRAFT_CARD_HEIGHT 35` / `DRAFT_MIN_VISIBLE 5`；
    `group_background(dark)` 深 `(255,255,255,13)` / 浅 `(255,255,255,200)`、
    `group_border` 96/24、`ratio_color` 深 `(252,169,3)` / 浅 `(196,53,1)`、
    `inset_rect(w,h)` = `(w-2, h-2)`、`update_stats(success, fail, total_size) ->
    DraftStats`；`DraftCardGroupState::{add_card, set_urls, clear, size_hint,
    minimum_size_hint}`（同 url 先删后**追加尾部**；`set_urls` 先 retain 再按
    urls 顺序重排）；`OptionCardGroupState::{add_card, insert_card, options,
    reset}`（`insert_card` 手写 `list.insert` 的负索引语义；`options()` 用 Vec
    保插入序，镜像 Python dict）。
  - `setting_card_group.rs`（461 行，16 测试）：`CollapsibleSettingCard` /
    `SettingCardGroup` 的折叠与省略号逻辑。`QWIDGETSIZE_MAX (1<<24)-1` /
    `COLLAPSE_DURATION_MS 200` / `COLLAPSE_EASING "OutCubic"` /
    `HEADER_MARGINS (16,10,8,10)` / `HEADER_SPACING 4` / `TITLE_HEIGHT 26` /
    `MOVE_BUTTON_SIZE 30` / `MOVE_ICON_SIZE 10` / `EXPAND_ANGLE_COLLAPSED 0` /
    `EXPAND_ANGLE_EXPANDED 180` / `CARD_STRETCH_FACTOR 1<<16` /
    `LABEL_WORD_WRAP false`；`group_background(dark)` 深 13 / 浅 **170**（与
    `card_groups` 的 200 不同，两文件本来就不同）、`separator_color` 96/24、
    `separator_rect(w, container_y)` = `(1, y-1, w-2, 1)`、`expand_angle(collapsed)`、
    `elide_lines(text, width, advance)`（`pystr::split_lines` + `labels::elide_right`
    + `"\n"` 拼回，空文本 → `""`）、`container_max_height(collapsed)`、
    `CollapseAnim` / `expand_anim` / `collapse_anim`、
    `CollapsibleSettingCardState`（`set_expand` 幂等、`finish_anim` **仅展开**
    时放 `QWIDGETSIZE_MAX`、`needs_group_separator(view_count)`）、
    `GroupCollapseState`（`header_hit(y, container_top) = y < container_top`）。
  - `setting_cards.rs`（1245 行，26 测试）：`setting_cards.py`(949) 的九类卡片
    语义层。
    - 校验正则：`host_pattern_ok`（IPv4 或域名，与 Python `HOST_PATTERN` 同源）、
      `port_pattern_ok`（`^\d{1,5}$`，**与 `proxy::port_valid` 的 1..=65535
      是两套口径**，前者只给红下划线）；
    - `ErrorUnderline{x,y,width,height,radius}` + `underline_visible(has_focus,
      is_error)` + `underline(width, height, ml, mr)`：Python 画的是
      `addRoundedRect(ml, h-10, w, 10, 5, 5) - addRect(ml, h-10, w, 8)` 的可见带
      → `y = h - 2`、`height = 2`、`radius = 1`；`ERROR_LINE_DEPTH 10` /
      `ERROR_LINE_CUT 8` / `ERROR_LINE_RADIUS 5`；
    - 数值卡：`spin_range` / `spin_display` / `spin_commit`（`int(v/division)`
      向零截断）、`percent_range` / `percent_display` / `percent_commit`
      （`/100` 不截断）、`WHEEL_ADJUSTS_VALUE false`（`eventFilter` 吞 Wheel）、
      `PERCENT_SUFFIX " %"` / `PERCENT_DECIMALS 0` / `PERCENT_SINGLE_STEP 25.0`；
    - `line_edit_commit`（`text()` 原样，不 strip）、`radio_label(choice)`；
    - 代理卡：`PackCompat{pack_id, proxy_schemes}`、`incompatible_packs`
      （`packId.upper()`，`proxySchemes is None` 视为全兼容）、
      `compat_banner_text`（`"{0} 不支持当前代理协议，将直连；使用 SOCKS5
      可兼容全部下载方式"`，`PACK_NAME_SEPARATOR "、"`）、
      `url_scheme(url)`（CPython `urlsplit` 口径：lstrip C0+空格、删
      `\t\r\n`、`:` 前不得含 `/?#`、首字符 ASCII 字母、其余
      `[A-Za-z0-9+.-]`、转小写）；
      `ProxyCardState::{load, radio_clicked, show_proxy_url, url, custom_enabled,
      refresh_banner, field_changed, leave_commit}`；
    - 身份预设行：`DescIcon{Globe,Fingerprint,People}`、`DescSegment`、
      `desc_segments(preset)`（hosts 前 3 项 `", "` 连接 → `profile::label_of`
      → `clip_user_agent`）、`row_enabled`、`UNNAMED_PRESET "未命名预设"`、
      `preset_name`、`toggle_identity` / `remove_identity` / `add_identity`
      （越界返回 `None`，Python 是静默 no-op）；
    - 标头卡：`headers_count_label "{n} 条标头"`、`headers_row_state(index,
      current, total) -> (is_current, total > 1)`、`HeadersRemoval{presets,
      current_value, current_index, can_remove, choice_name}`、
      `remove_headers_preset`；
    - 路径卡：`folder_content` / `file_initial_content(value, hint)` /
      `file_content(path, hint)`；
    - 运行时卡：`parse_version` / `is_newer`（镜像 `app/update.py` 的
      `lstrip("vV")` + 累积数字 + 去尾零）、`RuntimeStatus`、
      `RuntimeCardView{content, install_text: Option, install_visible,
      install_installing: Option, install_progress: Option, install_tooltip:
      Option, delete_visible, refresh_enabled}`、`runtime_view(status,
      can_install, is_managed)`（`Option<T>` 表达「Python 该分支没触碰该属性」）。
  - `identity::clip_user_agent` 改 `pub`；`identity::IdentityPreset` 补
    `PartialEq, Eq`；`headers::HeaderPreset` 补 `Debug, PartialEq, Eq`；
    `lib.rs` 在 `setting_card_group` 与 `settings_group` 之间注册
    `pub mod setting_cards;`。
- 实证结论（写进模块头）：
  - `_refreshCompatBanner` 每次都重新调 `proxy()`，**不缓存**系统代理 URL
    → Rust 侧把 `detected: Option<&str>` 作为参数传给 `refresh_banner` /
    `field_changed`，而不是在结构体里存一份 `detected_scheme`；
  - `_onRadioClicked` 的 custom 分支先 `_onProxyFieldChanged()`（内部已刷新
    banner）再 `_refreshCompatBanner()`，即**连刷两次**，幂等，Rust 照抄；
  - `_loadProxy` 的三分支：`"Auto"` / `"Off"` 直接走 `_onRadioClicked`；
    其余值先 `_showProxyUrl(value)` 再走 custom 分支 —— 此时 ip 已是主机名
    （非占位串），所以不会触发清空；
  - `_showProxyUrl(None)` 里 `protocolCombo.setCurrentText("")` 在不可编辑
    下拉中落到 index -1（文本为空），不是保留 socks4；未知 scheme 走
    `setCurrentText` 未命中 → 保留初值 `PROTOCOLS[0]`；
  - `SpinBoxSettingCard` 的 `int(v / division)` 是**向零截断**（`-1.5 → -1`），
    而 `PercentSpinBoxSettingCard` 的 `/100` 不截断，两者口径不同；
  - `LineEditSettingCard.editingFinished` 直接 `cfg.set(..., text())`，
    **不 strip**。
- 修掉的**测试算错**（实现均与 Python 一致，是断言写错）：
  - `card_groups::set_urls_with_duplicate_moves_twice_like_qt`：retain 在重排
    **之前**，`urls = ["a","a"]` 会先把 b 删掉；
  - `card_groups::option_group_insert_and_reset_follow_python_list_semantics`：
    `dict.update` 是**后者覆盖**，同名键只看最后一张卡；
  - 新增 `card_groups::option_group_reset_restores_each_card_initial_value`：
    `SubworkerCountCard::reset()` 回 `default_value`(8)，不是构造时的
    `initial`(4)。
- 修掉的编译/clippy 问题：`ProxyFields` 未用导入；`manual_contains` ×2
  （`urls.contains(&card.url)` / `schemes.contains(&scheme)`）；
  `assertions_on_constants` ×2（改 `const { assert!(...) };`）。
- 验证：
  - `cargo fmt --all --check` 通过；
  - `cargo test -p gd3-ui --lib`：**439 → 495 passed**（card_groups 14 +
    setting_card_group 16 + setting_cards 26）；
  - `cargo clippy -p gd3-ui --all-targets -- -D warnings` 0 error；
  - `cargo test -p gd3-core -p gd3-net -p gd3-servers -p gd3-ui` 全绿；
  - commit `0a69a73`。
- 下一步：P5 OOBE（`windows/oobe_window.py` 971 行）→ P6 平台外壳
  （`shell/tray.py` 205 / `dock.py` 141 / `mac_status_item.py` 112）→ 更新
  `rust/README.md` + `AGENT-DONE`。
- 卡点：无

## 第 74 步：P5 OOBE 向导语义层（2026-10-18 05:18）
- 做了什么：
  - 新增 `gd3-ui/src/oobe.rs`（**1895 行 / 38 测试**），镜像
    `app/view/windows/oobe_window.py`(971)；`lib.rs` 在 `name_conflict` 与
    `option_cards` 之间注册 `pub mod oobe;`。
  - 覆盖面：
    - 窗口几何：`WINDOW_WIDTH 960` / `WINDOW_HEIGHT 600` / `PREVIEW_WIDTH 592`
      / `PREVIEW_HEIGHT 333` / `STORE_COLUMN_WIDTH 280` / `PAGE_COUNT 6` /
      `LATEST_EXTENSION_VERSION "2.2.0"` / `CHROMIUM_EXTENSIONS_URL`；
      `LAYOUT_MARGIN_H 36` / `LAYOUT_MARGIN_BOTTOM 16` / `LAYOUT_TITLE_BAR_GAP 8`
      / `NAV_MARGIN_TOP 14` / `PRIMARY_BUTTON_WIDTH 200` /
      `SYSTEM_TITLE_BAR_X/Y/WIDTH = 0/10/75`；
    - `IconChip`：`ICON_CHIP_RADIUS 8.0`、`icon_chip_margin(w) = py_round(w*0.25)`、
      `NEUTRAL_CHIP_COLORS` / `SUCCESS_CHIP_COLORS`、三档尺寸 30/34/80；
    - 主题卡：`ThemeMode{Light,Dark,Auto}`（`value()` = `Light/Dark/Auto`、
      `preview_mode()` = `light/dark/auto`、`label()` = 浅色/深色/跟随系统）、
      `selected_theme_index`、`selection_ring(w,h) = (1,1,w-2,h-2)`、
      `CARD_BORDER_RADIUS 5.0` / `SELECTION_PEN_WIDTH 2.0`；
      `ThemePreview` 的 `mini_layout`（标题条 14、padding 8、三条
      `y+22 h9` / `y+37 h9*0.55` / `y+h-20 h11*0.34`、圆角 3）与
      `preview_panes`（Auto 左浅右深各半）、三组预览色；
    - 卡片规格：`ActionCardSpec{title,hint,icon,recommended,url}` +
      `has_hint()` / `fixed_height()` / `minimum_height()`（有 hint →
      `minimum_height=Some(80)` 且 `fixed_height=None`，无 hint → `Some(48)`/`None`）、
      `manual_install_card()`、`store_cards()`（三张 `GLOBE` 卡，URL 复用
      `settings_spec` 的三个商店常量）；`OptionCardSpec` + 七个 `OPTION_*`；
    - 文案常量：欢迎/基本设置/扩展页/推荐组件/更多选项/完成页/底部导航全部逐字，
      含 `LANGUAGE_FALLBACK_LABEL "使用系统设置"`；
    - 语言下拉：`language_items()` 直接复用 `settings_spec::LANGUAGE_LABELS`
      （末项就是兜底文案）、`language_index(value)` = `combo_index`、
      `language_pick(index, current)`（越界/同值 → `None`）；
    - 扩展页：`ListenStatus{Off,Failed,Listening}`、`BannerIcon{WARNING,INFO,SUCCESS}`、
      `Banner`、`BANNER_WIDTH/CLOSABLE/DURATION_MS/SHOWS_CONTENT = 592/false/-1/false`、
      `ListenInfo`、`listen_banner`、`connected_banner`、
      `ExtensionPageState{is_paired,banner:Option<Banner>}` +
      `refresh_listen_state` / `set_connected_banner` / `on_connection_changed` /
      `on_pair_requested` / `on_listen_state_changed(visible,...)` /
      `on_protocol_mismatched`；`ManualInstallToast` / `TOAST_DURATION_MS 5000` /
      `extension_extracted(open_ok)`；
    - 推荐组件：`RuntimeEntry`（`installed` 镜像 `bool(runtime.path())`）、
      `RuntimeRow{entry,has_checkbox,checked}`、`mount_rows`（过滤
      `canInstall && title`，`sort_by_key(|e| !e.is_recommended)` 稳定排序，
      已安装 → 无复选框）、`selected_runtimes`、`install_targets`、
      `RuntimeInstallState{mounted,rows}` + `mount/is_mounted/rows/set_checked/
      selected/install_targets`；
    - 更多选项：`AssocKind{FileTypes,UriSchemes}`（`key()` = `associateFileTypes` /
      `associateUriSchemes`）、`PackAssoc{file_types:Option<bool>,
      uri_schemes:Option<bool>}`、`AdvancedOptions` + `cards()` / `card_titles()` /
      `save(current_run_at_login, packs) -> SavePlan`、`PackAssocWrite`、
      `UrlSchemePlan{register,registered}`、
      `SavePlan{set_run_at_login:Option<bool>,clipboard_listener,category_enabled,
      pack_assoc,url_scheme:Option,aria2_rpc}`；
    - 窗口状态机：`Page` 六值 + `ALL` / `index()` / `from_index()`、
      `OobeEffect` 八值、`Navigation` + `navigation(index)`、
      `OobeState` + `go_next()` / `back()` / `finish()` / `close()` /
      `on_language_changed()`。
- 实证结论（写进模块头，本轮再次核对）：
  - `_onNextClicked` 在 `i == 3` 时**先** `_installSelectedRuntimes()` **再**自增，
    自增后才判断 `i == 2` 刷新监听、`i == 3` 挂载组件 —— 所以 2→3 走挂载、
    3→4 走安装，两者不会同时触发；
  - `_onBackClicked` **不**刷新监听也**不**挂载（只 `setCurrentIndex(..., isBack=True)`）；
  - `_finish` 与 `closeEvent` 共用 `_isFinished` 闸门，区别是前者额外
    `advancedOptionsPage.save()` 且显式 `close()`；
  - `refreshListenState` 在 `_isPaired` 时直接 return（横幅不再被监听状态覆盖），
    `_onListenStateChanged` 只在 `isVisible()` 时刷新；
  - `_onConnectionChanged` 用 `installType or version` 判配对，断开时回落
    `refreshListenState()`；
  - `save()` 的 `runAtLogin` 分支是**值变才写**（`!=` 才 `setRunAtLogin` + `cfg.set`），
    其余六个开关**无条件写**；`urlSchemeCard` 先注册/反注册再写配置；
    macOS（`fileAssocCard is None`）整段跳过关联与协议；
  - `mount()` 过滤条件是 `rt.canInstall and rt.title`（**title 空**才丢，不是 name）；
    排序键是 `not rt.isRecommended`（推荐在前，稳定排序）；
  - `_onExtensionExtracted` 只在 `openChromiumUrl` 失败时才复制剪贴板 + 弹 5s 提示；
  - `LanguageChanged` 整块重建 content 后恢复**原页码**，语言切换不跳页。
- 修掉的问题：
  - 测试数据写错：`RuntimeEntry::new("", "无标题", ...)` 把空串给了 **name**，
    而 Python 过滤的是 **title** → 改成 `("no-title", "", ...)`（实现未动）；
  - `const { assert!(OPTION_CARD_MARGINS == (18,8,18,8)) }` 编译失败
    （元组 `PartialEq` 还不是 const trait）→ 拆成四个字段比较；
  - clippy：`doc_lazy_continuation` ×2（模块头续行以 `（` 开头 → 并成一行）、
    `should_implement_trait`（`OobeState::next` 撞 `Iterator::next` → 改名
    `go_next`，语义仍对应 `_onNextClicked`）、`bool_comparison` ×2
    （`BANNER_CLOSABLE == false` → `!BANNER_CLOSABLE`）。
- 验证：
  - `cargo fmt --all --check` 通过；
  - `cargo test -p gd3-ui --lib`：**495 → 533 passed**（oobe 38 项）；
  - `cargo clippy -p gd3-ui --all-targets -- -D warnings` 0 error（增量 2m21s）；
  - `cargo test -p gd3-core -p gd3-net -p gd3-servers` 全绿；
  - commit `55f2345`。
- 下一步：P6 平台外壳（`shell/tray.py` 205 / `dock.py` 141 /
  `mac_status_item.py` 112）→ 更新 `rust/README.md` + `AGENT-DONE`。
- 卡点：无

## 第 75 步：P6 平台外壳语义层（`shell.rs`，tray/dock/mac_status_item）（2026-10-18 05:29）
- 做了什么：
  - 新增 `rust/crates/gd3-ui/src/shell.rs`（**848 行 / 21 测试**），镜像
    `app/view/shell/tray.py`(205) + `dock.py`(141) + `mac_status_item.py`(112)；
    `lib.rs` 在 `setting_cards` 与 `settings_group` 之间注册 `pub mod shell;`（line 62）。
  - 托盘：`TRAY_NAME "Ghost Downloader"`、四个动作文案、`TrayActionKind` 五值
    （含 `Separator`）、`TrayMenuEntry{kind,icon,text}`、`tray_menu()`（5 项，
    图标 `GHOST/PLAY/PAUSE/None/CLOSE`）、`TraySignal{ActivationRequested,StartAll,
    PauseAll,Quit}`、`tray_action`、`ActivationReason` 五值 + `from_qt`/`qt_value`、
    `tray_activated`（**只认 Trigger**）、`tray_tooltip(speed)`（`speed > 0` 才带
    `"↓ {toReadableSize}/s"`）。
  - Windows 菜单三态：`TrayMenuFlavor{RoundMenu,TrayRoundMenu,AcrylicMenu}` +
    `tray_menu_flavor(platform, win11)`；常量 `TRAY_MENU_WIDTH_PAD 5`、
    `ACRYLIC_MENU_Y_OFFSET 45`、`ACRYLIC_MENU_ITEM_HEIGHT 28`、
    `ACRYLIC_MENU_ICON_SIZE 14`、`ACRYLIC_MENU_HIDE_DELAY_MS 400`、
    `ACRYLIC_MENU_PAINT_COLOR (0,0,0,1)`、`ACRYLIC_TINT_DARK "00000030"` /
    `ACRYLIC_TINT_LIGHT "FFFFFF30"`；`acrylic_position`、`round_menu_position`、
    `acrylic_tint`、`acrylic_is_dark`。
  - Dock：`DOCK_ICON_SIZE 256`、`DOCK_FONT_RATIO 0.14`、`DOCK_FONT_MIN_PIXEL 6`、
    `DOCK_TEXT_MAX_WIDTH_RATIO 0.85`、`DOCK_TEXT_Y_RATIO 0.82`、
    `DOCK_TEXT_STROKE_WIDTH 12` / `COLOR (0,0,0,180)` / `FILL (255,255,255)`、
    `DOCK_BAR_HEIGHT_RATIO 0.08` / `STYLE 0` / `MIN 0.0` / `MAX 100.0` /
    `INDETERMINATE false`；`to_dock_speed`（**一位小数** + `B/s,K/s,M/s,G/s`，
    `<1024` 输出整数）、`DockTextLayout{pixel_size,text_x,text_y,advance}` +
    `dock_text_layout(size,text,&advance)`（字体测宽作闭包入参）、`dock_bar_height`、
    `dock_bar_visible`、`DockEffect{SetActivationPolicy,UpdateTile,ClearTile}`、
    `DockState` + `on_speed_changed` / `on_show_speed_changed` /
    `on_show_icon_changed` / `clear`。
  - 状态项：`STATUS_ICON_SIZE 16.0`、`STATUS_START_ALL_INDEX 1`、
    `STATUS_PAUSE_ALL_INDEX 2`、`StatusMenuItem{title,selector,key,symbol}`、
    `status_menu()`（`showDashboard:/startAll:/pauseAll:/quitApp:`，**仅退出**
    有快捷键 `"q"`）、`status_speed_title(show,bps)`（带**前导空格**）、
    `status_menu_enabled(&[TaskStatus]) -> (bool,bool)`、`status_icon_size(w,h)`。
- 实证结论（写进模块头，本轮实读源码）：
  - 托盘菜单顺序固定五项，分隔线在「全部暂停」与「退出程序」之间；
  - `_onActivated` 只认 `Trigger`（单击），双击/中键/上下文菜单一律忽略；
    `messageClicked` 直连 `activationRequested`；
  - `toReadableSize`（两位小数 + `B/KB/MB/GB/TB`）与 `toDockSpeed`（一位小数 +
    `B/s,K/s,M/s,G/s`）是**两套口径**，不能互相复用；
  - 平台分支只看 `win32` + `isGreaterEqualWin11()`：Win11 → `AcrylicMenu`，
    Win10 → `TrayRoundMenu`，其它 → `RoundMenu`；
  - `AcrylicMenu.adjustPosition` 的宽度是 `sizeHint.width + 5`，`x` 用
    `min(self.x() - m.left(), rect.right() - w)` 钳右边界，`y` 硬减 45；
  - `TrayRoundMenu.showEvent` 只钳右下（`min(x, right-width)` / `min(y, bottom-height)`）；
  - Dock 字号 `int(size * 0.14)`，超宽时**逐像素**降到 `> 6` 为止，`textY` 固定
    `size * 0.82`，进度条高 `tileHeight * 0.08`、`progress >= 0` 才显示；
  - `dock.clear` 在未建 container 时**无副作用**（用 `has_container: bool`
    忠实 Python 闭包的 `nonlocal container`）；
  - 状态项「全部开始」在存在 `PAUSED/FAILED/WAITING` 时启用、「全部暂停」在存在
    `RUNNING` 时启用；`setSpeed` 标题带前导空格（与托盘无空格相反）。
- 修掉的问题：
  - clippy `manual_contains`：`statuses.iter().any(|s| *s == TaskStatus::Running)`
    → `statuses.contains(&TaskStatus::Running)`；
  - 上一轮已删 `status_speed_title_after_toggle`（两分支同返回空串，
    clippy `if_same_then_else`），注释里写明「把开关新值传给 `status_speed_title` 即可」。
- 验证：
  - `cargo test -p gd3-ui --lib`：**533 → 554 passed, 0 failed**（shell 21 项）；
  - `cargo test -p gd3-ui --lib shell::`：21 passed；
  - `cargo clippy -p gd3-ui --all-targets -- -D warnings` 0 error（增量 2m27s）；
  - `cargo fmt --all --check` 通过；
  - `cargo test -p gd3-core -p gd3-net -p gd3-servers` 全绿；
  - commit `f6ca6d7`。
- 下一步：更新 `rust/README.md` 完成状态 → 创建 `AGENT-DONE` → 停止推进等验收。
- 卡点：无

## 第 76 步：Phase 7 收尾（README 完成状态 + AGENT-DONE + 全量门禁）（2026-10-18 05:42）
- 做了什么：
  - `rust/README.md`：顶部「已完成」补 Phase 5–6 与 Phase 7；新增
    **「## Phase 7：UI 层（`gd3-ui`）」**（定位、判定标准、P0–P6 阶段表、规模数字、
    6 个 Slint 文件、无头冒烟、运行命令、已知偏差四条、尚未做的部分）；
    原「## 下一步的顺序（已定）」改写为 **「## 剩余工作（Phase 8）」**
    （cfg 读写必须与 Slint 成为唯一 UI 同一步落地）。
  - 新建 `/root/Ghost-Downloader-3/AGENT-DONE`：做完什么（Phase 1–7 + 各阶段模块清单）、
    验证数字、还剩什么没做（5 条）、已知差异与风险（语义偏差 5 条 / 技术风险 5 条 / 边界）。
- 验证（本轮实跑）：
  - `cargo test -p gd3-core` **89 passed**、`-p gd3-net` **132 passed**、
    `-p gd3-servers` **29 passed**、`-p gd3-ui` **554 passed**，合计 **804 passed, 0 failed**；
  - `cargo clippy --workspace --all-targets -- -D warnings` **0 error**（增量 2m23s）；
  - `cargo fmt --all --check` 通过；
  - Python 全量 `pytest -q --continue-on-collection-errors` →
    **3 failed, 1347 passed, 8 skipped, 35 errors**，与基线逐条相同（本步只动文档）。
  - 注：`pytest -q`（不带 `--continue-on-collection-errors`）会在
    `tests/test_android_pack_import.py` 处收集中断，必须带该参数才是有效全量；
    另外本机 `uv` 不在 PATH（在 `/root/.local/bin`），直接用 `.venv/bin/python -m pytest`。
  - commit：`79fbf39`（README）。
- 下一步：Phase 7 已收口，按 `AGENT-DONE` 停在这里等人工验收。
  若要继续，Phase 8 的顺序建议是：Slint 入口 + 事件循环 → cfg 读写（`UserConfig.json`
  两侧互读测试）→ `TaskService` 绑定 → 托盘/Dock 平台层。
- 卡点：无

## 第 77 步：补 Phase 7 覆盖缺口（`clipboard.rs` + `draft_card.rs`）（2026-10-18 06:04）
- 为什么做这一步：第 76 步 `AGENT-DONE` 里写「Python 侧剩余：无」并不准确。
  对 `app/view`(11 211 行) 逐文件对照后发现两个真实缺口：
  - `shell/clipboard_listener.py`(67)：`draft.rs::clipboard_urls` 只镜像了
    `task_draft.py::_clipboardUrls`（逐行过滤 + matcher），**没有** `ClipboardListener`
    的状态机（自复制闩锁 / Wayland 去重 / 启停只连一次）；
  - `cards/draft_cards.py`(195)：`card_groups.rs` 只管分组容器（卡片高度、统计行），
    草稿卡片自己的改名 / 切分类 / 多文件摘要没有落点。
  - `qfw_patch.py`(75) 判定 **N/A**：纯 Qt monkeypatch（改 `qfluentwidgets` 内部行为），
    无业务语义可镜像，不写 Rust 对应物。
- 做了什么：
  - 新增 `rust/crates/gd3-ui/src/clipboard.rs`（**507 行 / 12 测试**）：
    `ClipboardListener{enabled,self_copied,last_urls,wayland}` +
    `new`/`is_enabled`/`self_copied`/`last_urls`/`set_enabled -> Option<bool>`/
    `set_urls -> String`/`on_data_changed -> Option<Vec<String>>`；
    `downloadable_urls`；`url_roundtrip_ok` + 私有 `split_url`/`geturl`/
    `uses_netloc`/`is_scheme_char`/`is_bracket_char`。
  - 新增 `rust/crates/gd3-ui/src/draft_card.rs`（**650 行 / 18 测试**）：
    常量（`DRAFT_CARD_HEIGHT 35`、`CARD_MARGINS (10,2,10,2)`、`CARD_SPACING 12`、
    `ICON_SIZE 16`、`BUTTON_SIZE 28`、`EDIT_TOOLTIP`/`CONFLICT_TOOLTIP`/
    `SELECT_FILES_TOOLTIP`/`UNCATEGORIZED_LABEL`/`UNCATEGORIZED_ID`/`FALLBACK_NAME`/
    `MAX_NAME_LENGTH`）；`separator_color`/`size_label_text`/`name_edit_selection`/
    `menu_anchor`/`conflict_visible`/`category_button`/`category_menu`；
    `CategoryButton`/`DraftMenuEntry`/`DraftCardSignal`/`DraftCardState`（改名编辑态）/
    `DraftFile`/`MultiFileDraftCardState`（摘要 + `apply_selection`）。
  - `lib.rs` 注册 `pub mod clipboard;`（`category_settings` 与 `close_mode` 之间）、
    `pub mod draft_card;`（`draft` 与 `drawer` 之间）。
- 实证结论（本轮实读源码 + `.venv/bin/python` 核 `urlparse`）：
  - `urlparse` 的判据是 **`parsed.geturl() == 原串`**，不是「URL 合法」：
    `HTTP://x/` → scheme 被小写 → 丢弃；`http:/x` → `geturl` 为 `http:///x` → 丢弃；
    `a:/b` → `a:/b` → **保留**（`http` 在 `uses_netloc` 里、`a` 不在，同形状结果相反）；
    `http://a/1?` / `http://a/1#` → 空 query/fragment 被吃掉 → 丢弃；
    `http://a b/1`、`http://A/1`、`http://a/./b/../c`、`mailto:a@b`、`a:b` → 保留；
    `http://[::1` / `http://::1]` / `http://[zz]/` → `ValueError`；
    `http://a\t/1` → `urlsplit` 删掉 `\t` → 丢弃。
  - `setUrls` 只在 `_enabled` 时置 `_selfCopied`；`setEnabled` 只在翻转时连/断；
    `_onDataChanged` **先消费闩锁再取链接**（无链接也消费）、空则返回、
    只有 Wayland 才比 `_lastUrls` 快照（非 Wayland 同一段文本两次都发）。
  - 草稿卡片改名走 **`task.setName()` → `toSafeFilename(name, fallback=self.name or "download")`**，
    标签/编辑框显示的是**净化后**的名字；进入编辑时选中 `splitStemExt` 的词干（按字符数）。
  - `MultiFileDraftCard._refreshSummary` 的 `<=1` 分支用 `toReadableSize(fileSize)`，
    **没有**基类那条 `"--"` 兜底；`setSelection` 之后 `fileSize = Σ selected.size`。
- 修掉的问题：
  - clippy `if_same_then_else`：`geturl` 里「path 以 `//` 开头」与「`uses_netloc`」两条
    分支体完全相同，合并成一个 `||` 条件；
  - clippy `doc_lazy_continuation`：`draft_card` 模块头里 `——锚点…` 那行补 2 空格缩进。
- 验证：
  - `cargo test -p gd3-ui --lib clipboard::` 12 passed；
  - `cargo test -p gd3-ui --lib draft_card::` 18 passed；
  - `cargo test -p gd3-ui --lib` **554 → 584 passed, 0 failed**；
  - `cargo clippy -p gd3-ui --all-targets -- -D warnings` **0 error**（2m28s）；
  - `cargo fmt --all --check` 通过；
  - `cargo test -p gd3-core -p gd3-net -p gd3-servers` 全绿；
  - commit `dc78e5c`。
  - `gd3-ui` 现状：56 个 `.rs`、**33 411 行 `src/`**、**584 单测**。
- 下一步：修正 `AGENT-DONE` 与 `rust/README.md` 里「Python 侧剩余：无」的表述，
  然后按 Phase 8 顺序推进（Slint 入口 + 事件循环 → cfg 读写 → `TaskService` 绑定 →
  托盘/Dock 平台层）。
- 卡点：无
