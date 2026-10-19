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

## 第 78 步：按真机磁盘格式重写 `gd3-core::config`（2026-10-18 06:25）
- 为什么做这一步：第 49 步的 `config.rs` 建立在「每个配置项落盘都是字符串」的假设上，
  **该假设是错的**。本步读 qfluentwidgets 1.18 源码 + 跑真机 `UserConfig.json` 推翻它。
- 实证结论（本轮实读 `qfluentwidgets/common/config.py` 与 `app/config/cfg.py`）：
  - `ConfigItem.serialize()` 只有一句 `self.serializer.serialize(self.value)`，
    默认 serializer 是恒等 —— **没显式传 serializer 的项，落盘就是裸 JSON 值**：
    `"EmulateFingerprint": false`、`"MaxTaskNum": 3`、`"HistoryDownloadFolder": []`、
    `"DpiScale": 0`、`"CurrentHeadersPreset": 0`。
  - 只有三类项被转成字符串：`JsonConfigSerializer`（`CategoryRules` /
    `HeadersPresets` / `IdentityPresets` → `json.dumps(..., ensure_ascii=False)` 紧凑串）、
    `EnumSerializer`（`Language`/`ThemeMode`/`CloseMode`/`ClientProfile` → **enum 的 value**，
    `CloseMode.QUIT.value` 是 `"Quit"` 不是 `"QUIT"`）、
    `GeometrySerializer`（元组 → `"x,y,w,h"`）。
  - **旧实现会毁掉 Python 配置**（本轮实测）：
    `BoolValidator.correct("False")` → 不在 `[True, False]` → `options[0]` → **变成 True**；
    `RangeValidator.correct("7")` → `max(1, "7")` → `TypeError` →
    `@exceptionHandler()` 兜住 → **整个 `QConfig.load` 中断**，后续项全部保持缺省
    （实测 `{"MaxTaskNum": "7", "PreBlockNum": 16}` 读入后仍是 3 / 8）。
  - 落盘顺序：`toDict()` 遍历 `dir(self._cfg.__class__)`（属性名**字母序**），
    组在首次出现时创建 → 组序 = 各组首项的出现次序，组内键序 = 属性名字母序。
    `serde_json` 已开 `preserve_order` → Rust 侧可逐字节同构。
  - `QFluentWidgets` 组（`ThemeColor`/`FontFamilies`，继承自 `QConfig`）也在
    `toDict()` 里，必须原样保留；`qconfig.load()` 文件不存在时**不写盘**；
    `cfg.save()` 是 `json.dump(..., ensure_ascii=False, indent=4)`，**无尾随换行**。
- 做了什么：
  - 新增 `tests/fixtures/generate_user_config_fixture.py`（183 行），在 venv 里跑出
    4 个金文件（**金文件由 Python 生成，不手写** —— 第 49 步手写 fixture 就是错误源头）：
    `user_config_default_linux.json`(2166B)、`user_config_mutated_expected.json`(2456B)、
    `py_json_writer.jsonl`(20 条)、`py_float_repr.jsonl`(20 条)。
  - 重写 `src/config.rs`（+602/−215，707 行）：`GroupMap = Map<String, Value>`、
    `UserConfig{groups}`；`load`/`from_json`/`save`/`to_json_text`/`entries`/`get`；
    typed 读 `get_str`/`get_py_str`/`get_bool`/`get_i64`/`get_f64`；
    typed 写 `set`/`set_str`/`set_bool`/`set_i64`/`set_f64`/`set_py_json`；
    writer `py_json_string`/`py_float_repr`/`py_json_dumps`/`write_compact`/
    `write_indented`/`push_indent`/`py_json_scalar`/`py_number_text`。
  - 新增 `get_py_str`（python `str()` 口径）：解决「磁盘存 int、调用方要文本」的
    `Network.CurrentHeadersPreset`；新增 `set_py_json`：`JsonConfigSerializer` 三项必须存串。
  - `py_float_repr` 手写 python `repr` 口径：`|x| >= 1e16` 或 `< 1e-4` 切指数
    （Rust `{:e}` → `1e16`/`1.5e-7`，python → `1e+16`/`1.5e-07`，指数补两位 + 带符号）、
    整值补 `.0`、`-0.0` 保符号。
  - 10 个测试（`config::tests`）：缺省/改动金文件逐字节回写、**Rust 侧镜像
    `MUTATIONS` 后与 python `qconfig.set`+`save` 逐字节相同**、typed 取值口径、
    遗留字符串档容错、`py_json_dumps`+`write_indented` 对 20 条金文件、
    `py_float_repr` 20 条、`py_json_string` 28 条、存读往返、非对象组跳过。
  - `gd3-ui/src/lib.rs` 3 处：`push_settings` 与 `open_headers_preset_edit` 的
    `CurrentHeadersPreset` 改走 `get_py_str(...).as_deref()`（`get_str` 读不到 int），
    两处测试的 `set_str(..., "1")` → `set_i64(..., 1)` 对齐磁盘形态。
  - 删除废弃金文件 `user_config_sample.json`、`user_config_expected_save.json`。
- 已知差异（写进模块头）：
  - **容错策略不同**：Python 在首个非法值处中断整个 load，Rust 逐键跳过、保留其余项；
  - `get_*` 接受「数字/布尔的字符串形式」这一遗留形态（Python 会拒收），
    写回按 set 的入参类型落盘，**不做批量归一**；
  - `get_py_str` 对数组/对象返回 `None`（python `str()` 会给出 `"[...]"`，无调用方需要）。
- 验证：
  - `cargo test -p gd3-core`：**95 passed**（lib 60，其中 `config::` 10）；
  - `cargo test -p gd3-ui --lib`：**584 passed, 0 failed**（无新增失败）；
  - `cargo test --workspace`：**840 passed, 0 failed**（第 76 步 834 + 新增 6）；
  - `cargo clippy --workspace --all-targets -- -D warnings` **0 warning**（2m32s）；
  - `cargo fmt --all --check` 通过；
  - commit `9e7dbb4`。
- 下一步：第 79 步做 typed config 语义层（`settings.rs`：41 项 schema + 缺省值 +
  validator 边界 + `to_proxy_url`/`current_headers`），随后进 Phase 8 主链路。
- 卡点：无

## 第 79 步：`gd3-core::settings` typed 语义层（2026-10-18 06:39）

第 78 步的 `config::UserConfig` 只懂「组 → 键 → 裸 JSON 值」，不懂类型/缺省/约束。
这一步把 `app/config/cfg.py` 的 40 个 `ConfigItem` + `QConfig` 自带的 2 个
（`ThemeColor`/`FontFamilies`）搬成一张 42 项的静态表，并把每个 validator /
serializer 逐个镜像过来。

- 做了什么（`rust/crates/gd3-core/src/settings.rs`，新增 1665 行）：
  - `static SPECS: &[Spec]`（42 项，`attr`/`group`/`key`/`kind`），**顺序 = `dir(Config)`
    的属性名字母序** —— 这是落盘顺序的协议，不是排版偏好。
  - `enum Kind`：`Bool`/`Int`/`Float`/`Text`/`Options`/`Proxy`/`Profile`/`Folder`/
    `FolderList`/`StringList`/`StrList`/`JsonList`/`Geometry`/`Color`，每个变体自带
    `raw_default()`（源码字面量）/`correct()`（validator 镜像）/`deserialize()`/
    `serialize()`（落盘形态）。
  - `default_config()` / `default_config_with_download_folder()`：**纯 Rust 造出与
    Python 逐字节相同的 `UserConfig.json`**（缺省值先过 `correct()` 再 `serialize()`）。
  - `read`/`write`（按 python 属性名，通用）+ `read_bool`/`read_i64`/`read_f64`/`read_text`。
  - `to_proxy_url`、`current_headers_preset_index`、`current_headers`、`base_headers`、
    `default_headers_preset`、`proxy_valid`、`folder_stored`、`color_hex_argb`。
- 实证结论（在 venv 里逐条跑 `validator.correct()` 验证，**不是读源码猜的**）：
  - `BoolValidator` 继承 `OptionsValidator([True, False])` → 非法值取 `options[0]`：
    磁盘写字符串 `"False"` 读回来是 **`true`**；而 `0`/`1` 因为 `0 == False` 命中白名单，
    **原样保留数字形态**。
  - `OptionsValidator(Language)` 的 options 是 **enum 成员**，`options[0]` 是
    `Language.CHINESE_SIMPLIFIED` → 非法 `Language` 落回 `"zh_CN"`（**不是 `"Auto"`**）；
    `Theme` 同理落回 `"Light"`，`CloseMode` 落回 `"Ask"`。我第一版测试按「落回 Auto」
    写，被实测推翻。
  - `RangeValidator.correct(0.0)` 返回 **int `0`**（`max(0, 0.0)` 相等时返回第一个参数，
    而边界是 int 字面量），`correct(2.0)` 仍是 float `2.0`。这就是 `DpiScale` 缺省落盘
    是 `0` 而不是 `0.0` 的原因；`Kind::Float` 的缺省回落也走 `bound_value()` 保 int 形态。
  - `RangeValidator.correct("7")` → `TypeError`（`max(1, "7")`）→ `@exceptionHandler()`
    中断整个 `load`，该项保持缺省 → Rust 侧 `clamp_number` 返回 `None` 后退回缺省。
  - `Path(...).absolute()` 只做 `PurePath` 归一（丢空段与 `.`、保留**恰好两个**前导斜杠、
    `///a` → `/a`），**不解析 `..`**（`a/../b` 原样保留），也不检查存在性。第一版我按
    `posixpath.normpath` 写了会解析 `..` 的版本，实测 `Path("Downloads/./x/../y/").absolute()`
    → `.../Downloads/x/../y` 后改掉。
  - `ProxyValidator` 的密码字符集 `[\w!@#$%^&*()]` **含 `@`** → `socks5h://user:p@ss@1.2.3.4:1080`
    合法；用「最后一个 `@`」近似 CPython 正则回溯，15 个非法样例 + 11 个合法样例与
    Python 逐条对照，**两侧零分歧**。
  - `fontFamilies` 是 `ConfigItem` **没有 validator**（不是 `StringListValidator`）→
    拆成 `Kind::StrList { default }`（恒等）与 `Kind::StringList`（过滤非字符串）两个变体，
    否则缺省会落成 `[]`。
- 测试（`settings::tests`，26 个全绿）：
  - `defaults_are_byte_identical_to_python`：`default_config().to_json_text()` 与
    `tests/fixtures/user_config_default_linux.json` **逐字节相同** —— 一条测试同时钉住
    42 项缺省值 + 每个 validator 的 `correct()` + 序列化方式 + 落盘顺序。
  - `specs_are_in_python_dir_order`（42 项 attr 顺序与 `dir(Config)` 实测一致，第一版
    `dpiScale`/`downloadFolder` 顺序写反，被这条抓住）、
    `specs_cover_every_group_and_key_of_the_fixture`（组序 + 组内键序）。
  - range clamp / float 形态 / 非数字回落、bool 回落、options 回落、proxy 26 例、
    headers 过滤与回落、identity 无回落、category 全有全无、geometry 往返、
    color 归一、str list、folder 绝对化、folder list 只留存在路径、`to_proxy_url`、
    `current_headers`、存读往返、`spec()` 查表。
- 已知差异（写进模块头）：
  - **不建目录**（`FolderValidator.correct` 会 `mkdir`）、**不抛异常**（Python 在首个非法
    值处中断整个 `load`，Rust 逐键跳过）、`Color` 不认 Qt 颜色名、`get_*`/`read` 对
    遗留字符串形态的容错与 Python 不完全一致（已在模块头逐条列出）。
- 验证：
  - `cargo test -p gd3-core`：**121 passed**（lib 86，其中 `settings::` 26；第 78 步 lib 60）；
  - `cargo test --workspace`：**866 passed, 0 failed**（第 78 步 840 + 新增 26）；
  - `cargo clippy --workspace --all-targets -- -D warnings` **0 warning**（2m38s）；
  - `cargo fmt --all --check` 通过；
  - commit `abd23c6`。
- 下一步：第 80 步起进 Phase 8 主链路 —— `gd3-ui` 目前**没有二进制**（`Cargo.toml` 只有 lib），
  先加 `main.rs` + Slint 事件循环 + `TaskService` 绑定，再接托盘/Dock。
- 卡点：无

## 第 80 步：`gd3-ui` 二进制入口 + 启动序列（2026-10-18 06:58）

Phase 7 收口时 `gd3-ui` 只有 lib（58 个 `.rs`、无 `main`、无事件循环）。
这一步把 `Ghost-Downloader-3.py`（268 行，仓库根的真正入口）的启动序列搬成 Rust，
让 Slint 侧第一次「真的能跑起来」——本机无显示器，所以跑的是无头软件渲染路径。

- 做了什么：
  - 新增 `crates/gd3-ui/src/boot.rs`（685 行）：
    `Paths`（镜像 `app/config/paths.py` 58 行）、`Cli` + `parse_args`、
    `is_launch_uri`/`is_wake_uri`（`url_scheme.py`）、`uris_from_argv`
    （`application.py::urisFromArgv`）、`load_config`（`qconfig.load` 语义）、
    `has_display`、`run`（路径 → 配置 → 窗口 + `push_settings` → 事件循环）、
    `main_entry`（`--help`/`--version`/错误码 2）。
  - 新增 `crates/gd3-ui/src/main.rs`（11 行）：只有 `boot::main_entry(env::args().skip(1))`。
  - `lib.rs` 加一行 `pub mod boot;`（放在 `bootstrap_list` 前，保持字母序）。
  - 不需要 `[[bin]]`：`src/main.rs` 与 `src/lib.rs` 并存时 cargo 自动生成同名 bin `gd3-ui`。
- 实证结论：
  - **本机 `HOME` 是空的**（`HOME=`），Qt 因此给 `HomeLocation='/'`、
    `GenericDataLocation='//.local/share'`、`DownloadLocation='//Downloads'`。
    双斜杠不是笔误：Qt 用**字符串拼接**（home + "/" + leaf），不做路径归一。
    所以 `home_path_join` 也照抄拼接，`Path::join` 会归一掉（有单测钉住）。
  - Python 的 `qconfig.load` 在**文件不存在时不写盘**；Rust 侧同样不写
    （`--headless --frames 3 --data-dir <空目录>` 跑完目录仍为空）。
  - 配置文件存在但损坏时 Python 的 `@exceptionHandler()` 中断 load、保持缺省；
    Rust 退回缺省并把 `broken` 标出来（首版只区分「缺失/存在」，坏文件被打印成
    `config=loaded`，本轮改成 `invalid->default`）。
  - `urisFromArgv` 的判据是 `arg.find(':') > 1`：所以 `"a:b"` 与 `"C:/x"` **不算 URL**
    （冒号在 index 1），会被当路径再判 `is_file()`；`ghostdownloader://...` 直接跳过。
  - `isWakeUri` 是 `uri.rstrip("/") == "ghostdownloader://wake"`：大小写敏感，
    `ghostdownloader://wake?x=1` 不算；命中即 `isSilent`。
  - `app/config/constants.py::VERSION` 是 **`4.4.0`**（`pyproject.toml` 的
    `version = 4.3.5` 是另一个数），二进制 `--version` 用前者。
- 测试（`boot::tests`，15 个全绿）：CLI 全形态 / `=` 形式 / 未知参数 / 缺值与非法帧数 /
  wake URI 静默（含尾斜杠、大小写、query）/ `uris_from_argv`（URL、含空格文件 →
  `file://...%20...`、`a:b` 不算、`-z` 跳过）/ `home_path_join` 双斜杠 /
  便携与非便携路径 / 配置缺失·存在·损坏三条 / 无头启动冒烟（含 `--frames 0`）。
- 二进制实跑（`target/debug/gd3-ui`，本机无 `DISPLAY`）：
  - `--version` → `Ghost Downloader v4.4.0`；`--help` → 帮助；`--nope` → 退出码 2；
  - `--headless --frames 3 --data-dir <空目录>` →
    `launched (headless, 3 frame(s), config=default, data=/tmp/tmp.sYqayzLgho)`，目录仍为空；
  - 无参数（自动无头）→ `data=//.local/share/GhostDownloader`，与 Qt 实测一致；
  - 把 `tests/fixtures/user_config_default_linux.json` 当配置 → `config=loaded`，
    跑完 **md5 不变**（2166B，未被回写）；
  - 坏配置（`{`）→ `config=invalid->default`，进程正常退出。
- 验证：
  - `cargo test -p gd3-ui --lib boot::`：**15 passed**；
  - `cargo test --workspace`：**881 passed, 0 failed**（第 79 步 866 + 15）；
  - `cargo clippy --workspace --all-targets -- -D warnings` **0 warning**（2m31s）；
  - `cargo fmt --all --check` 通过；
  - commit `0490908`（含 `rust/README.md`：新增「Phase 8：启动入口」一节，
    规模改为 56 模块 / 34 113 行 / 599 单测）。
- 未迁移（写进 `boot.rs` 模块头）：单实例锁（QSharedMemory）、D-Bus / URL scheme 接收、
  `app/engine.py` 全部服务、托盘 / keepalive / 通知、功能包加载、更新检查、
  OOBE 分支、翻译器。
- 下一步：第 81 步做 `TaskService` 语义层（`app/services/task_service.py`）——
  任务的增删改、`runningCount`/`tasksAllCompleted`、运行时事实（`CardRuntime`）的维护，
  把 `boot::run` 里 `push_settings` 之后的「推任务行」接上。
- 卡点：无

## 第 81 步：`gd3-ui::task_service`（`TaskService` 引擎无关半边）（2026-10-18 07:20）

`app/services/task_service.py`（705 行）把「存储/队列记账 + 引擎交互 + 19 个信号」揉在
一个类里。这一步只搬**引擎无关**的那一半：任务列表、队列记账、`pump`/`allCompleted`
的判定、`runningProgress`、落盘防抖。引擎半边（协程、速度表、文件监听、做种、校验）
等 `app/engine.py` 迁过来再接。

- 做了什么（`rust/crates/gd3-ui/src/task_service.rs`，新增 675 行，21 个测试）：
  - `TaskService { records: Vec<TaskRecord>, queue: TaskQueue, loaded, pending_flush,
    delete_denied_notified }`；`records` 用 `Vec` 保**插入序**（Python `dict` 迭代序）。
  - `new`/`from_records`/`load`（`loadSaved`）/`records`/`task`/`task_mut`/`len`/
    `is_empty`/`is_loaded`/`mark_dirty`/`pending_flush`。
  - `add`/`add_with_free_space`、`remove`、`start`、`pause`、`cancel_work`、
    `on_run_started`、`on_run_finished`、`pump`、`running_count`、`waiting_order`、
    `move_to_front`、`running_progress`、`note_delete_denied`、`flush`、`refresh`。
  - `AddOutcome::{Duplicate, DiskSpaceInsufficient{free}, Added{queued}}`；
    `RunOutcome { started, all_completed }`。
- 实证结论（逐条对着源码核对，**不是照抄我自己上一版的假设**）：
  - **`add()` 的入队条件与 `fileSize` 无关**。Python 的 `if task.fileSize > 0:` 只包住
    「磁盘空间检查」，`self._schedule(task)` 在 `if` **块外**无条件调用。所以
    `autoStart=True` + `fileSize=0` 也会 `queue.wait`。我上一版写的
    `queued = auto_start && file_size > 0` 是错的，被这条源码阅读推翻（有单测钉住）。
  - `diskSpaceInsufficient` 是 `return True`（记录**已经在列表里**，只是不入队），
    所以 `DiskSpaceInsufficient` 之后 `len()` 仍然 +1。
  - `_onRunDone` 里 `if runningCount() == 0: tasksAllCompleted.emit()` 在 `_pump()`
    **之后**：`pump` 会 `queue.run()` 把 `runningCount` 顶上去，所以「刚做完一个、
    正好接上下一个等待任务」**不发** `tasksAllCompleted`。这就是 `on_run_finished`
    返回 `RunOutcome { started, all_completed }` 而不是一个裸 bool 的原因（第一版是裸
    bool，会漏掉 pump 的影响）。
  - `_pump` 的 `while runningCount() < maxTaskNum` 依赖 `_dispatch` 里立刻 `queue.run`；
    本模块没有引擎，所以 `pump()` 用 **空 workId** 占位 `queue.run(id, "")`，
    引擎起来后由 `on_run_started(id, work_id)` 回填真实 workId。
  - `_pump` 里 `task is None` 的 id 会被 `nextWaiting()` 弹出但**不派发**（有单测
    `pump_skips_waiting_ids_without_a_record` 钉住「弹出但不跑」）。
  - `pause(task)` 除 `_cancelWork` 外只有 `setStatus(PAUSED)` + `_flushSoon`，全部引擎无关
    → 一并搬了。
  - `TaskStore.loadSaved` 在文件不存在时也是 `_loaded = True`（之后 `flush()` 会真写盘），
    `TaskService::load` 保留这个语义（`store::load_records` 缺失即空）。
- 已知差异（写进模块头）：不做重名冲突解决（`FileExistsError` 重试 + `deleteRecoverably`
  + `nameConflicted`/`overwriteFailed`）、不建输出目录与占位文件、不算磁盘剩余空间
  （由调用方传 `free_space`，`None` = `OSError`）、不做 `_rebalance`（依赖
  `Task.canPause` 运行时事实）、不做做种/校验/文件监听/速度表。
- 验证：
  - `cargo test -p gd3-ui --lib task_service::`：**21 passed**；
  - `cargo test --workspace`：**902 passed, 0 failed**（第 80 步 881 + 21）；
  - `cargo clippy --workspace --all-targets -- -D warnings` **0 warning**（2m25s）；
  - `cargo fmt --all --check` 通过；
  - commit `38ca14a`（含 `rust/README.md`：新增「任务记账（`task_service.rs`）」小节，
    规模改为 57 模块 / 34 790 行 / 620 单测）。
- 下一步：第 82 步把 `TaskService` 接进 `boot::run`（启动时 `TaskService::load(
  APP_DATA_DIR/tasks.jsonl)` → `refresh(window)`），并给无头模式加 `--tasks-file`
  之类的可测入口。
- 卡点：无

## 第 82 步：`TaskService` 接进 `boot::run`（2026-10-18 07:33）

第 81 步的 `task_service.rs` 只有库，启动序列还没用它。这一步把「启动时装载任务列表
→ 重新入队 → 推成 UI 行」接进 `boot::run`，Slint 侧第一次带着真实任务列表起来。

- 做了什么：
  - `boot.rs`：`Paths::tasks_path()`（`APP_DATA_DIR / "tasks.jsonl"`，与
    `TaskStore._path` 同路径）+ 常量 `TASKS_FILE`；`Cli` 新增 `--tasks-file <PATH>`
    （Rust 侧可测性开关，两种写法都支持，缺值报错）；`RunSummary` 新增
    `tasks` / `task_load_failures` / `resumed`；`run()` 在配置之后、窗口之前
    `TaskService::load(&tasks_path)` → `resume_saved()`，两个分支都在
    `push_settings` 之后 `service.refresh(&window)`；无头摘要行追加
    `tasks={}, resumed={}`。
  - `task_service.rs`：新增 `resume_saved() -> Vec<String>`。
- 实证结论：
  - `resumeSaved()` 的真实语义（`task_service.py:520-532`）：已完成的任务走
    `_watchFile`/`_seed`，`WAITING`/`RUNNING` 的**先把状态改成 `WAITING`** 再
    `_schedule`（= `queue.wait` + `_pump`）。所以启动后「正在下」的任务会变成「等待中」。
  - 没有引擎时「派发」是假的：`_pump` 之后 `runningCount()` 会 >0、但没有任何协程在跑，
    而且 `_dispatch` 里 `task.setStatus(RUNNING)` 这一步也没人做（记录状态与队列状态会
    不一致）。所以 `resume_saved()` **只入队不 pump**，把 `pump(maxTaskNum)` 留给引擎层。
  - `setStatus` 不落盘：Python 的 `resumeSaved` 改完状态也不 `_flushSoon`，Rust 侧同样
    不置 `pending_flush`（有单测钉住）。启动因此**不会回写** `tasks.jsonl`。
- 测试：
  - `task_service::tests::resume_saved_requeues_waiting_and_running_only`（只重排
    WAITING/RUNNING、保插入序、不 pump、不置脏）；
  - `boot::tests`：`parses_tasks_file_flag_in_both_forms`、`tasks_path_defaults_to_app_data_dir`、
    `headless_run_loads_tasks_from_app_data_dir`（3 有效行 + 1 坏行 → `tasks=3`、
    `failures=1`、`resumed=2`，且跑完文件字节不变）、
    `tasks_file_override_wins_over_app_data_dir`。
- 二进制实跑（`target/debug/gd3-ui`，无 `DISPLAY`）：
  ```text
  $ ./target/debug/gd3-ui --headless --frames 1 --data-dir /tmp/tmp.P98YEjL99E
  Ghost Downloader v4.4.0 launched (headless, 1 frame(s), config=default,
    data=/tmp/tmp.P98YEjL99E, tasks=2, resumed=2)
  ```
  跑完 `tasks.jsonl` 的 md5 不变（`a3bdd49b…`），目录里也没有多出 `UserConfig.json`。
- 验证：
  - `cargo test -p gd3-ui --lib boot::`：**19 passed**；
  - `cargo test --workspace`：**907 passed, 0 failed**（第 81 步 902 + 5）；
  - `cargo clippy --workspace --all-targets -- -D warnings` **0 warning**（2m24s）；
  - `cargo fmt --all --check` 通过；
  - commit `f317c23`（含 `rust/README.md`：启动流程图加 `TaskService` 一行、CLI 清单
    加 `--tasks-file`、规模改为 34 964 行 / 625 单测）。
- 下一步：第 83 步做 `app/engine.py`（只有 74 行，是纯装配门面）的 Rust 对应物 ——
  把「哪个服务依赖哪个服务」的构造顺序固定下来，即使引擎内部还是桩，也能先钉住装配契约。
- 卡点：无

## 第 83 步：`gd3-ui::engine`（镜像 `app/engine.py` 的装配门面）（2026-10-18 07:56）

`app/engine.py` 只有 74 行，零业务逻辑：按固定顺序拼服务 + 唯一一条信号连线，再按固定
顺序 `start`/`stop`。这一步把这份**装配契约**搬成 Rust，服务内部仍是桩，但「谁在谁之后、
谁连谁、停机什么顺序」从此有测试钉住。

- 做了什么：
  - `engine.rs`（1189 行，17 单测）：`EngineConfig::from_config`（六项，每项过
    `Spec::read` 的 `correct()` 口径）、`SpeedMeter`（整类）、`CategoryService`（只搬
    `_load`）、`RuntimeStatusService`/`FeatureService`/`BrowserService`/`Aria2RpcService`
    （桩）、`Listener` + `ListenerBackend` + `ListenerSettings` + 两个工厂、
    `Engine`（13 字段 + 全套访问器）、`StartOutcome`、`TaskConflictActions`（`ConflictActions`
    的引擎侧实现）；
  - `task_service.rs`：新增 `load_into(&Path)`（`*self = load(...)` 的就地版）与
    `stop()`（`RUNNING`/`WAITING` → `PAUSED` + 置 `pending_flush`）；
  - `boot::run`：`Engine::new(&loaded.config, tasks_path)` → `Engine::start()` →
    `engine.tasks().refresh(&window)` → 事件循环 → `Engine::stop()`；
  - `rust/README.md`：规模 58 模块 / 36 201 行 / 642 单测，新增「引擎装配（`engine.rs`）」。
- 实证结论（源码级）：
  - `TaskService.stop()`（`task_service.py:536-540`）只 `setStatus(PAUSED)`：**不动队列**、
    不 `_flushSoon()`；而 `TaskStore.flush()`（:179-201）**没有脏标记**、每次都整文件重写。
    所以 Python 停机时 PAUSED 是被 `Engine.stop()` 里那次 `flush()` 写下去的。Rust 的
    `flush()` 有 `pending_flush` 短路 → `TaskService::stop()` 必须显式置脏，否则停机状态
    永远不落盘（单测 `stop_flush_persists_paused_status` 钉住）。
  - `Engine.stop()` 里 `flush()` 出现两次（`engine.py:44` 与 `:49`），第二次是给
    `featureService.deactivate()` 可能产生的改动兜底；Rust 保留「两次都在」的顺序事实，
    第二次通常因脏标记已清而 `Ok(false)`（字节等价，少了那次空写）。
  - `PortListener` 存的是 `ConfigItem` 引用、`start()` 时才读值并连 `valueChanged`；
    `rust_browser.isEnabled()` 精确等于 `os.environ.get("GD3_RUST_BROWSER") == "1"`
    （`rust_aria2` 同构）→ `flag_is_on` 只认 `"1"`（`"true"`/`"01"`/`"1 "` 全 false）。
  - `nameConflicted.emit` 发生在 `_store.add` **之后**，所以队列回调 `TaskService.add`
    第一行 `if task.taskId in self._store.tasks: return True` 必然命中 → Rust 侧
    `TaskConflictActions::add` 走 `AddOutcome::Duplicate` 分支。
  - `TaskStore.flush()` 无条件写 → Python 侧「启动完立刻退出」也会留下 `tasks.jsonl`。
    所以 `boot::run` 的**无头模式也走 `Engine::stop()`**；第 82 步那条「跑完文件字节不变」
    的断言随之改成「停机后 `RUNNING`/`WAITING` 变 `PAUSED`、`COMPLETED` 不动」。
- 已知差异（写进模块头）：
  - 配置是快照：`nameConflict`/`maxTaskNum` 每次用时现读（与 `lambda` / `_pump` 同构），
    监听器端口在构造时取快照，改配置不会自动重开端口；
  - `probeConflict` 恒 `None`（占位文件未迁移）→ 冲突队列不会弹窗、按 `keepBoth` 放行；
  - `TaskService::stop()` 不动队列（Python 也不动）→ 停机后 `running_count()` 仍可能 >0；
  - `_pump` 不自动跑（`resume_saved` 只入队），由调用方显式 `Engine::pump()`。
- 测试（`engine::tests`，17 个）：装配顺序 16 条 trace 逐条比对、`start` 5 条、
  `stop` 7 条 + 监听器状态、装载/坏行/resume、缺文件不算错、停机只改 RUNNING/WAITING、
  `pump` 按 `maxTaskNum` 派发、`nameConflict` 现读、监听器配置四项、后端开关、
  `flag_is_on`、速度表 tick/stop、冲突入队→`refresh`→`nameConflictQueue.add(a, keepBoth)`、
  无冲突时对话框结论是 no-op、`categoryRules` 装载。
- 验证：
  - `cargo test -p gd3-ui --lib engine::`：**17 passed**；`boot::`：**19 passed**；
  - `cargo test --workspace`：**924 passed, 0 failed**（第 82 步 907 + 17）；
  - `cargo clippy --workspace --all-targets -- -D warnings` **0 warning**（2m35s）；
  - `cargo fmt --all --check` 通过；
  - 二进制实跑（无 `DISPLAY`，2 条 WAITING/RUNNING）：
    ```text
    $ ./target/debug/gd3-ui --headless --frames 1 --data-dir /tmp/tmp.PfGIRAz12d
    Ghost Downloader v4.4.0 launched (headless, 1 frame(s), config=default,
      data=/tmp/tmp.PfGIRAz12d, tasks=2, resumed=2)
    ```
    跑完 `tasks.jsonl` 两条都变 `"status": "PAUSED"`（md5 变了），目录里没有 `UserConfig.json`。
  - commit `46f52b7`。
- 下一步：Phase 8 按 VPS-MIGRATION 第 12 节优先级 —— 单实例锁（`QSharedMemory` /
  `desktop_keepalive.py`）、托盘（`shell/tray.py`）、D-Bus / URL scheme 接收、
  更新检查。这几项都是「Rust 侧可以独立立起来、且不碰 `UserConfig.json` 写权」的，
  顺序上单实例锁最靠前（它决定「第二个实例该把 URL 交给谁」）。
- 卡点：无

## 第 84 步：`gd3-ui::single_instance`（单实例锁 + URI 交接）（2026-10-18 08:18）

Phase 8 第一项：`app/platform/application.py::SingletonApplication` 的 Rust 对应物。
Python 是**两套机制叠加**——锁用 `QSharedMemory(DESKTOP_ID)`（`attach()` 成功 = 已有实例
→ `sys.exit(-1)`），通道用 Windows `WM_COPYDATA` / Linux `org.freedesktop.Application`
的 `Open`/`Activate`。Rust 侧换成一个 **Unix domain socket**（`APP_DATA_DIR/instance.sock`）：
`bind()` 成功即抢到锁、`connect()` 成功即「已有实例」，同一条连接顺带把参数送过去。
一个机制同时给出锁与通道，不依赖 D-Bus、不依赖显示器。

- 做了什么：
  - `single_instance.rs`（521 行，9 单测）：`Handoff{Activate, OpenUris}` + `to_payload`/
    `parse_payload`、`plan_from_argv`（要不要发 / 发什么）、`Acquire{Primary(Instance),
    Secondary(Secondary)}`、`acquire`/`acquire_with_handoff`、`Instance`（后台线程收消息 →
    `take_handoffs`，`release`/`Drop` 停线程 + 删 socket）、陈旧 socket 回收、常量
    `LOCK_FILE`/`PROTOCOL`/`SECOND_INSTANCE_EXIT = 255`；
  - `boot.rs`：`Paths::lock_path()`、`Cli::positional`（原始位置参数，wake 判定要用）、
    `RunSummary::handoffs`、新枚举 `RunOutcome{Ran(RunSummary), AlreadyRunning(Secondary)}`；
    `run()` 在 `load_config` 之后上锁（Python 的顺序：`setupEnvironment` → `SingletonApplication`），
    事件循环结束后 `take_handoffs()` + `release()`，再走 `Engine::stop()`；
    `main_entry` 对 `AlreadyRunning` 返回 255；
  - 测试：`ran(&cli)` helper 替掉 4 处 `run(&cli).expect(...)`；新增
    `lock_path_lives_in_app_data_dir`、`second_instance_in_the_same_data_dir_hands_off`
    （同数据目录第二个 `run()` → `AlreadyRunning` + 主实例收到 `OpenUris`）；
  - `rust/README.md`：规模 59 模块 / 36 838 行 / 653 单测；启动流程图加锁与放锁两步；
    新增「单实例锁（`single_instance.rs`）」一节。
- 实证结论（源码级）：
  - 第二个实例的分支（`application.py:114-124`）：`attach()` 成功后**先**判断
    `any(isWakeUri(arg) for arg in sys.argv[1:])`，命中就**一个字节都不发**直接退出；
    否则有 URI 发 `Open(uris)`、没 URI 发 `Activate`。两种都 `sys.exit(-1)`；
  - `urisFromArgv` 会跳过 `ghostdownloader://...`，所以「带启动 URI 但没别的 URL」= 空 uris
    = `Activate`；这与 D-Bus `Open` slot 里「含 `isLaunchUri` → `activationRequested`」
    是同一条语义，所以 `parse_payload` 直接镜像了 `_DesktopBusReceiver.Open` 的过滤：
    空行/ wake URI 丢掉 → 空则丢弃 → 含启动 URI 则升级成 `Activate`；
  - `desktop_keepalive.py`（113 行）是**睡眠抑制**（`SetThreadExecutionState` /
    `IOPMAssertionCreateWithName`），与单实例锁无关，别混；
  - `_unlockSingleInstance` 在 `exec()` 的 `finally` 里，即**停机序列之前**——Rust 侧
    `release()` 也放在 `Engine::stop()` 之前（有注释钉住顺序）；
  - 陈旧 socket：Python 的 `QSharedMemory` 用「先 `attach`/`detach` 清一遍」处理残留；
    socket 版本靠 `bind` 失败后 `connect` 探活，`ConnectionRefused`/`NotFound` → 删文件重试一次。
- 线协议（Rust 侧自定，Python 的两套通道都被替换掉了）：
  ```text
  gd3-ipc/1 activate\n              ← Activate / WM_USER_WAKE
  gd3-ipc/1 open\n<uri>\n<uri>\n    ← Open / COPYDATA_OPEN_FILES
  ```
  首行是协议名、第二行是动作名；空载荷（`release()` 的唤醒连接、半截连接）不算消息。
- 已知差异（写进模块头）：锁在 `APP_DATA_DIR` 下（换 `--data-dir` 就换锁，Python 的
  `QSharedMemory` key 是全局的）、Windows 后端没实现、**不注册 D-Bus 服务名**
  （桌面双击链接仍走「第二个实例 → socket 交接」，结果等价）、`SIGINT` 处理没搬、
  交接消息要等事件循环结束才取（消费方还没迁过来）。
- 验证：
  - `cargo test -p gd3-ui --lib single_instance::`：**9 passed**；`boot::`：**21 passed**；
  - `cargo test --workspace`：**935 passed, 0 failed**（第 83 步 924 + 11）；
  - `cargo clippy --workspace --all-targets -- -D warnings` **0 warning**（2m24s）；
  - `cargo fmt --all --check` 通过；
  - 二进制双进程实测（同一个 `--data-dir`）：
    ```text
    $ ./target/debug/gd3-ui --headless --frames 6000 --data-dir /tmp/tmp.1ZIIJgzjZ4 &
    $ ls -a /tmp/tmp.1ZIIJgzjZ4      → instance.sock
    $ ./target/debug/gd3-ui --headless --data-dir /tmp/tmp.1ZIIJgzjZ4 https://example.com/x
    → exit 255
    $ ./target/debug/gd3-ui --headless --data-dir /tmp/tmp.1ZIIJgzjZ4 ghostdownloader://wake
    → exit 255（wake 分支同样退出，但一个字节都不发）
    A: launched (headless, 6000 frame(s), …), exit 0；退出后 instance.sock 已删
    ```
  - commit `69dafe2`。
- 下一步：Phase 8 继续按优先级 —— 托盘（`shell/tray.py`，205 行，纯 Slint 语义层 + 单测）、
  D-Bus 服务名注册（`org.freedesktop.Application` 的 `Open`/`Activate`，可复用
  `single_instance::Handoff` 的过滤规则）、更新检查、keepalive（`desktop_keepalive.py`
  睡眠抑制）。
- 卡点：无

## 第 85 步：`gd3-core::filesystem`（2026-10-18 08:41）

- 做了什么：
  - 新增 `rust/crates/gd3-core/src/filesystem.rs`（**636 行 / 18 单测**），镜像
    `app/platform/filesystem.py` 里除 `toSafeFilename` / `splitStemExt`（已在
    `gd3-core::filename`）之外的**六个**函数：
    `is_existing`、`is_folder`、`to_posix_path`、`local_file_path`、`find_executable`、
    `delete_path`；
  - `path.rs` 加 `posix_normpath`（镜像 `str(PurePosixPath(p))`，**135 → 210 行**）+ 1 单测；
  - `gd3-core` 在 `[target.'cfg(unix)'.dependencies]` 下引入 `libc`，只为
    `shutil._access_check` 用的 `access(2)`；
  - `lib.rs`：`pub mod filesystem;` + 导出六个函数与 `posix_normpath`；
  - `rust/README.md`：新增「平台辅助（`gd3-core::filesystem`）」一节（对照表 + 5 条
    CPython 细节 + 为什么不加影子开关）。`gd3-ui` 的规模数字（59 模块 / 36 838 行 /
    653 单测）不受影响，没动。
- 实证结论（源码级 + CPython 3.13.5 实跑）：
  - `urlsplit` **只 `lstrip`**（`"  file:///tmp/x  "` 的 `path` 是 `"/tmp/x  "`，尾部
    空格保留），并删掉 URL 里的 `\t\r\n`；scheme 必须首字符是 ASCII 字母、其余属于
    `[A-Za-z0-9+-.]`，`":file:..."`（`i == 0`）与 `"fi le:..."` 都**不**认；
    `file` 不在 `uses_params` 里，所以 path 不按 `;` 拆；
  - `url2pathname` 在 POSIX 上就是 `unquote`：按 `%` 切段，**每段只解前两个十六进制
    字符、剩下的原样拼回去** —— 所以 `unquote("%4142") == "A42"`（不是 `"AB"`），
    `unquote("%%41") == "%A"`、`unquote("%2") == "%2"`；`+` **不**解成空格；最后按
    UTF-8 `errors="replace"` 解（`%FF` → `�`、`%C3%28` → `�(`）；
  - `str(PurePosixPath(p))`：`""`→`"."`、`"//"`→`"//"`、`"///a"`→`"/a"`、
    `"//a//b/./c"`→`"//a/b/c"`、`"a/.."`→`"a/.."`（**不**消解 `..`）、
    `"a\\b"`→`"a\\b"`（POSIX 上反斜杠是普通字符）；
  - `toPosixPath` 的顺序是「先 `Path()` 归一，再 `.replace("\\", "/")`」——反过来的话
    `a\b/c` 会变成 `a/b/c`，正确结果是 `a/b/c`… 两种写法在这条上碰巧同结果，但
    `Path("a\\b//c")` 归一成 `a\b/c` 才替换，所以顺序必须照抄（有单测钉住）；
  - `shutil.which` 的 POSIX 分支（`/usr/lib/python3.13/shutil.py:1340-1410`）：
    `os.path.split(cmd)` 有 `dirname` 时 `path = [dirname]`、只查这一条；否则
    `path = PATH.split(':')`（`PATH=""` 直接 `return None`，空段 = 当前目录），
    `files = [cmd]`（**POSIX 上不看 `PATHEXT`**，那是 win32 分支），判据
    `_access_check = exists && os.access(F_OK|X_OK) && not isdir`；按
    `os.path.normcase(dir)` 去重（POSIX 恒等）；
  - `os.access(..., X_OK)` 在 root 下是「任一个 x 位为真」、非 root 下按 euid/egid 判，
    **std 没有对应物**（自己看 `mode & 0o111` 在非 root 下会把「只有 group-x 且我不在
    那个组」判成真），所以加了 `libc`。这是 `gd3-core` 唯一的平台相关依赖；
  - `deletePath` 用 `path.is_dir() and not path.is_symlink()` 分流：`is_dir()` 跟随
    链接、`is_symlink()` 不跟，所以**指向目录的符号链接走 `unlink` 分支**（只删链接，
    目标还在）——Rust 侧照抄（有单测）；
  - `localFilePath` 的 `path.is_file()` 对含 NUL 的路径抛 `ValueError` 而 Python 只捕
    了构造 `Path` 时的 `OSError` → **Python 会崩、Rust 返回 `None`**（写进模块头差异表）；
  - `Path.suffix.lower()` 里 `0 < i < len-1` 才算有后缀（`".bashrc"`/`"a."` 都无后缀），
    集合里的项按原样比（只把路径后缀转小写）。
- 验证：
  - `cargo test -p gd3-core`：**105 passed**（原 87 + 18 filesystem + 1 normpath）；
  - `cargo test --workspace`：**954 passed, 0 failed**（第 84 步 935 + 19）；
  - `cargo clippy --workspace --all-targets -- -D warnings` **0 warning**（10m52s）；
  - `cargo fmt --all --check` 通过；
  - Python 侧**没改一行**，不需要跑 Python 全量；
  - commit `e01309c`（代码）。
- 勘误：commit message 写「636 行 / 19 单测」，`filesystem.rs` 实际是 **18 单测**，
  第 19 个在 `path.rs`（`posix_normpath`）。
- 下一步（Phase 8，按 `VPS-MIGRATION.md` 第 12 节的优先级）：
  1. D-Bus 名字注册（`org.freedesktop.Application` 的 `Open`/`Activate`，过滤规则直接
     复用 `single_instance::Handoff`；本机没有 session bus 时要有降级路径，别让启动挂住）；
  2. 更新检查（`app/services/update_service.py` 335 行）；
  3. keepalive（`app/platform/desktop_keepalive.py` 113 行，睡眠抑制）。
  另外 `app/models/pack.py`（310 行，`BinaryRuntime` / `FeaturePack`）现在只差
  `find_executable` 这一个依赖了，可以考虑排进 Phase 8。
- 卡点：无

## 第 86 步：`gd3-core::pack` + `gd3-core::install`（2026-10-18 10:28）

Phase 8 的「插件运行时」地基。`app/models/pack.py` 310 行里**能搬的只有不依赖 Qt 的那半截**
（`FileType` / `UriScheme` / `VersionInfo` / `PackManifest.fromDir` / `BinaryRuntime`）：
`TaskParser` / `PackConfig` / `FeaturePack` / `PackPage` / `PackServices` 是「插件由 Python
模块实现」这层协议（`PackConfig.__init_subclass__` 把子类的 `ConfigItem` 注册进 `cfg`、
`FeaturePack` 的方法返回 Qt 控件与信号），Rust 侧没有对应概念，插件加载本身留在 Python。
顺带把 `app/install.py` 的 `deleteInstalled` / `installFile` 也搬了——它们是
`BinaryRuntime.delete()` 的唯一依赖。

- 做了什么：
  - 新增 `rust/crates/gd3-core/src/pack.rs`（**1146 行 / 37 单测**）：
    `FileType`、`UriScheme`、`VersionInfo`、`PackManifest`（`from_dir` / `from_dir_detailed`）、
    `ManifestError`（七种失败原因，`Display` 逐字照抄 Python 的日志格式串）、
    `BinaryRuntime` trait（`name` / `binary_name` / `can_install` / `title` / `description` /
    `icon` / `is_recommended` 是带默认实现的类属性；`install_folder` / `installed_paths`
    是抽象方法；`runtime_id` / `path` / `is_app_managed` / `probe_version` / `delete`
    给默认实现）；私有 `str_field` / `read_toml` / `py_isspace` / `py_strip` /
    `split_lines` / `decode_ignore`；
  - 新增 `rust/crates/gd3-core/src/install.rs`（**286 行 / 12 单测**）：
    `delete_installed`（改名 `<name>.<8hex>.old` + 按 `<name>.*.old` 扫残留）、
    `install_file`（`deleteInstalled` + `os.replace`）、私有 `is_leftover` / `parent_dir`；
    `installArchive` 标注未迁移；
  - `Cargo.toml`：`gd3-core` 加 `toml = "1.1.8"`（**离线零下载**，见下）；
  - `lib.rs`：`pub mod install;` / `pub mod pack;` + 六个类型的 re-export；
  - `rust/README.md`：新增「插件清单与运行时（`gd3-core::pack` / `gd3-core::install`）」一节
    （对照表 + 七条校验 + 五个易错点 + 规模）。
- 调研结论（推翻了先前的候选）：
  - **「新建 `gd3-core::update`」作废**：`app/update.py` 的纯逻辑已经迁完 ——
    `parseVersion` / `isNewer` 在 `gd3-ui::setting_cards`（:611，有单测
    `parse_version_mirrors_update_py`），`Release` / `ReleaseAsset` / `assetScore` /
    `bestAsset` 在 `gd3-ui::release_info`。剩下的只有网络请求；
  - **`toml` crate 离线可用**：`Cargo.lock` 里本来就有 `toml 1.1.8`（别的 crate 拉的），
    `~/.cargo/registry/cache` 里有 `toml-1.1.8+spec-1.1.0`、`zbus-5.19.0`、`md-5`、
    `digest`，`cargo add toml --offline -p gd3-core` 成功、零下载
    （`static.crates.io` 根路径返回 403，所以别试在线）；
  - **`D-Bus 名字注册` 继续往后排**：它的净效果已被第 84 步的 `single_instance` socket
    覆盖（桌面唤起 = 起新进程 → 交接），手写线协议数百行高风险，`zbus` 编译代价又大；
    `pack` / `install` 是运行时状态卡、`loader`、`update_service` 的共同依赖，先做这个。
- 实证结论（源码级 + CPython 3.13 实测，全部有单测钉住）：
  - `fromDir` 七条校验**先到先错**：缺 `manifest.toml` → 读/UTF-8/TOML 解析失败 →
    无 `[pack]` 表 → `entry` 非 str 或 `strip()` 空（**默认 `"pack.py"`**）→ 入口文件不存在
    （只有 `.py` 才退到同名 `.pyc`，**报错路径仍是原始的 `packDir/entry`**）→ `class`
    非 str 或 strip 空 → `dependencies` 非 list 或含非 str/空串；`version` / `gdMinVersion`
    非 str 时**静默**变 `""`；`name = packDir.name`；
  - `strip()` **只用于判空**：拼路径用未 strip 的 `entry`，所以 `entry = " pack.py "`
    会去找带空格的文件名；`class` 也原样保留（含前后空格）；
  - `BinaryRuntime.path()` = `findExecutable(installFolder(), binaryName or name)`；
    `isAppManaged()` = `bool(p) and Path(p).is_relative_to(installFolder())` ——
    CPython 实测 `Path("ffmpeg").is_relative_to(Path("/opt/x"))` 是 `False`（不抛）、
    `/opt/xy/...` 相对 `/opt/x` 是 `False`，所以 Rust 侧 `Path::starts_with` 直接等价；
  - `probeVersion()`：`<path> --version`、stdin/stderr `DEVNULL`、非 0 退出码 →
    `VersionInfo("")`、`stdout.decode(errors="ignore").splitlines()[0].strip()`；
    `errors="ignore"` 是**丢字节**不是插 `U+FFFD`，所以 `decode_ignore` 手写
    （`from_utf8_lossy` 会给 `�`）；
  - `deleteInstalled` 用 `path.exists() or path.is_symlink()` 判存在 —— 等价于
    `symlink_metadata().is_ok()`（断链符号链接只后者为真）；
  - `Path.glob(glob.escape(name) + ".*.old")` 的 `glob.escape` 只把 `[` / `]` / `*` / `?`
    包成字符类（`a[1]*?.old` → `a[[]1][*][?].old`），真通配符只剩那一个 `*`，
    而目录项不含 `/` → 「前缀 + 后缀 + 长度」判定与 `fnmatch` 等价；
  - `Path(".").with_name("x")` / `Path("").with_name(...)` / `Path("/").with_name(...)`
    在 CPython 全抛 `ValueError`（Python 侧整个函数中断），`Path("..").name == ".."`
    且 `with_name("x") == Path("x")`；
  - `os.replace` **不建父目录**（父目录缺失 = `FileNotFoundError`）——Rust 的 `fs::rename`
    同构，第 86 步有一条单测专门钉这个（原先那条「目标不存在也能装」的断言是错的，
    跑全量时暴露出来，改成「父目录存在 → 成功」+「父目录缺失 → `NotFound`」两条）。
- 已知差异（写进模块头）：
  - `tomllib.loads` 与 `toml` crate 对**非法文档**的接受面不完全一致（例如 `tomllib`
    拒绝 BOM 而 `toml` crate 接受）；合法 manifest 两侧结果一致；
  - `read_text` 失败时 Python 抛 `UnicodeDecodeError`、`repr(e)` 的文本无法逐字复刻
    （`reason` 用 Rust 的 `Display`），归到同一个「无法读取」分支；
  - `packDir.name` 对非 UTF-8 目录名：Python 走 `surrogateescape` 原样保留，
    Rust 走 `to_string_lossy()`；
  - `runtimeId` 两边都是「类型全名」但**字符串不同**（`gd3_ui::runtime::FfmpegRuntime`
    vs `xxx.FfmpegRuntime`），不能混用作持久化键；
  - `probeVersion` 在 Python 是 `async`，Rust 是同步 `Command`，调用方自己丢线程；
  - `delete_installed` 对空 / `/` / `.` 路径直接 `Ok(())`（Python 抛 `ValueError` 中断）；
  - `pack.rs` 里另有一份私有的 `py_isspace` / `py_strip` / `split_lines` / `decode_ignore`
    ——`gd3-core::hls` 与 `gd3-ui::pystr` 已各有一份，抽公共模块会牵动既有调用点，留作后续。
- 验证：
  - `cargo test -p gd3-core --lib`：**154 passed**（第 85 步 105 + 37 pack + 12 install）；
  - `cargo test --workspace`：**1003 passed, 0 failed**（第 85 步 954 + 49）；
  - `cargo clippy --workspace --all-targets -- -D warnings` **0 warning**（1m23s）；
  - `cargo fmt --all --check` 通过；
  - Python 侧**没改一行**，不需要跑 Python 全量；
  - commit `a2efbc3`。
- 环境记录（重要）：本机只有 **939MB 内存 + 4GB swap**，`cargo test --workspace`
  默认并行会把 `gd3-ui` 的 lib 与 lib-test 两个 rustc 同时拉起来（各 300~630MB），
  然后**双双掉进 swap 抖动**（CPU 3~4%、13 分钟没编完，最后 1300s 超时被杀）。
  改成 `CARGO_BUILD_JOBS=1` 后单进程能跑到 60% CPU，全量编译约 10 分钟；
  `cargo clippy` 元数据缓存热的时候只要 1m23s。**以后 Rust 全量一律带
  `CARGO_BUILD_JOBS=1`**，并且用 `nohup ... &` + 轮询日志，别让工具调用阻塞。
- 下一步（Phase 8）：
  1. `gd3-core::loader`（`app/loader.py` 140 行）：`orderedByDependency`（拓扑排序 + 环检测）、
     `loadPacks` 的目录扫描 + `gdMinVersion` 门禁、`seedPacks`（`rmtree` + `copytree`）——
     纯逻辑 + 文件系统，依赖刚做完的 `PackManifest`；`loadManifest`（importlib）N/A；
  2. `gd3-core::checksum`（`app/checksum.py` 33 行）：`parseChecksum` / `matchChecksum` 纯逻辑；
     `toChecksum` 需要 md5/sha1/sha256/sha512/shake（`md-5` / `digest` 已缓存，
     `sha1` / `sha2` 未见缓存 → 需先验证离线可用）；
  3. `app/models/serialization.py`（94 行）：`TaskRecord` ↔ JSON 的字段映射；
  4. D-Bus / keepalive 仍排在最后（`zbus` 缓存可用但编译代价大）。
- 卡点：无

## 第 87 步：gd3-core::loader + gd3-core::version（2026-10-18 10:30）

- 做了什么：
  - 新增 `rust/crates/gd3-core/src/version.rs`（110 行 / 4 单测）：
    `parse_version` / `is_newer` **从 `gd3-ui::setting_cards` 搬进 core**，
    `setting_cards.rs` 那 34 行实现换成
    `pub use gd3_core::version::{is_newer, parse_version};`
    （`setting_cards` 里 `is_newer` 还有 `runtime_card` 的调用点，`pub use` 后照常可用；
    它自己那两个测试也留着，等于跨 crate 复验一遍）。
    搬家的原因：`gd3-core::loader` 的 `gdMinVersion` 门禁要用 `parse_version`，
    而 `gd3-core` 不能反向依赖 `gd3-ui`。
  - 新增 `rust/crates/gd3-core/src/loader.rs`（1029 行 / 37 单测）：
    `VersionRejection` / `ScanReport` / `DependencyError` / `DependencySkip` /
    `OrderReport` / `LoadReport` / `SeededPack` / `SeedReport` +
    `scan_packs` / `load_packs` / `ordered_by_dependency` / `seed_packs` +
    私有 `VisitState` / `visit` / `sorted_subdirs` / `copy_tree` / `resolve_path` /
    `lexical_normalize`。
  - `lib.rs` 加 `pub mod loader;` / `pub mod version;` + 12 个类型/函数 re-export。
  - `rust/README.md` 新增「插件加载与版本门禁」一节（对照表 + 精确口径 + 四条已知差异 + 规模）。
- 关键决策：
  - `ordered_by_dependency` 返回 `OrderReport { ordered, skipped }` 而**不是** `Result`：
    Python 的失败只影响顶层那个 manifest（`skipped.add(m.name)` + `visiting.clear()`
    然后继续下一个），整体排序不中断，`Result` 表达不了这个语义。
  - `load_packs` 只搬到「排序好的 manifest 列表」：`loadManifest`（`importlib` +
    `getattr(module, className)` + `PackClass(services)`）的产物是 Python 类实例
    （返回 Qt 控件与信号），Rust 侧没有对应概念，实例化留在 Python。
  - `app_version` 作为参数传进 `scan_packs` / `load_packs`，不在 core 里硬编码
    `VERSION = "4.4.0"`（`app/config/constants.py` 的常量归 `gd3-ui` 入口）。
  - `seed_packs` 返回 `io::Result<SeedReport>`：Python 那边 `rmtree` / `copytree`
    没有 `try/except`，文件系统错误原样冒出去；返回的只有「同步了哪些」，日志由调用方打。
- 实证结论（源码级 + CPython 3.13 实测，全部有单测钉住）：
  - `orderedByDependency` 是 DFS 三色：命中 `visited` 直接返回、命中 `skipped` 报
    `{name} 依赖的 FeaturePack 已被跳过`、命中 `visiting` 报
    `循环依赖: a -> b -> a`（环从 `visiting` 里该名字**首次**出现处开始、末尾补一次自己）、
    依赖不在 `byName` 里报 `{name} 依赖未找到的 FeaturePack: {dep}`（`{name}` 是
    **当前正在访问的节点**，不是顶层那个）；
  - 两节点环 `a -> b -> a` 的**两条** skip 记录原因不同：第一轮顶层 `a` 报 `Cycle`，
    第二轮顶层 `b` 命中 `skipped` 报 `SkippedDependency`（`visiting` 已在上一轮被
    `clear()`）——这条有专门单测；
  - `byName = {m.name: m}` 是**同名后者胜**，`ordered` 里存的是 `byName[name]`
    而不是循环变量，所以同名 manifest 只入列一次、入列的是最后一个；
  - `seedPacks` 在 `mkdir` **之前**就 `return`（`seedDir` 不存在或 `resolve()` 相等），
    所以目标目录不会被创建；用户侧版本判定是 `parseVersion(user) >= parseVersion(seed)`
    → **`1.0` 与 `1.0.0` 去尾零后相等，不覆盖**（有单测）；
  - `copytree` 默认 `symlinks=False`：符号链接按目标内容拷，所以 `copy_tree` 用
    `Path::is_dir()`（跟随链接）+ `fs::copy`，且 `dst` 必须不存在（先 `os.mkdir`）。
- 已知差异（写进模块头 + README）：
  - `Path.resolve()` 解析符号链接；`resolve_path` 先 `fs::canonicalize`（目标存在时等价），
    失败退回 `std::path::absolute` + 词法归一。只有「目标目录尚不存在**且**路径带链接」
    才有区别；
  - `copytree` 会 `copystat` 目录 mtime/权限，`copy_tree` 只保留文件权限位；
  - 目录项排序：Python 按 `PurePath._parts_normcase`（POSIX 上等价按码点），
    Rust 按 `to_string_lossy()` 的 `String`（等价按 UTF-8 字节），非 UTF-8 名字会退化；
  - Python 的 `isdigit()` / `int()` 接受 Unicode 十进制数字（`parseVersion("１.２")`
    得 `(1, 2)`），Rust 的 `char::to_digit(10)` 只认 ASCII → `()`。
- 验证：
  - `cargo test -p gd3-core --lib`：**195 passed**（第 86 步 154 + 37 loader + 4 version）；
  - `cargo test --workspace`：**1044 passed, 0 failed**（第 86 步 1003 + 41）；
  - `cargo clippy --workspace --all-targets -- -D warnings` **0 warning**（1m18s）；
  - `cargo fmt --all --check` 通过；
  - Python 侧没改一行，不需要跑 Python 全量；
  - commit `f860401`。
- 下一步：
  1. `gd3-core::checksum`（`app/checksum.py` 33 行）：`parse_checksum`
     （`text.split()[0].lower()`）/ `match_checksum` 是纯逻辑，`to_checksum` 要
     md5/sha1/sha256/sha512/shake（`md-5` / `digest` 已缓存，`sha1` / `sha2` 未验证
     离线可用 → 先试 `cargo add --offline`，不可用就只搬前两个 + `CHUNK_SIZE = 4MiB`）；
  2. `app/models/serialization.py`（94 行）：`TaskRecord` ↔ JSON 的字段映射；
  3. `app/services/runtime_status.py`（195 行）接线：现在有 `pack::BinaryRuntime` 可依赖；
  4. D-Bus / keepalive 仍排在最后（`zbus` 缓存可用但编译代价大）。
- 卡点：无

## 第 88 步：gd3-core::checksum（2026-10-18 10:51）

- 做了什么：
  - 新增 `rust/crates/gd3-core/src/checksum.rs`（**914 行 / 10 单测**）：
    `CHUNK_SIZE` / `COMMON_ALGORITHMS` / `SUPPORTED_ALGORITHMS` / `SHAKE_LENGTHS` /
    `ChecksumError`（两变体，`Display` 照抄 `ValueError: unsupported hash type xxx`）/
    `parse_checksum` / `match_checksum` / `to_checksum` +
    私有 `read_chunk` / `py_isspace` / `to_hex` / `Hasher` /
    私有 `mod sha256`（FIPS 180-4，64 项 K 表）/ `mod sha512`（80 项 K 表）。
  - `Cargo.toml`：`cargo add --offline` 加了 `md-5 = "0.11"` / `sha1 = "0.11"`
    （**零下载**，本来就在 `Cargo.lock` 里）；`lib.rs` 加 `pub mod checksum;` + 8 个 re-export。
  - `rust/README.md` 新增「文件校验（`gd3-core::checksum`）」一节。
- 关键决策：
  - **手写 SHA-256 / SHA-512，而不是放弃这两个算法**：`sha2` 不在离线缓存里，
    但 `sha256` 是下载校验最常用的算法，缺位等于功能回归。K 表 / 初始状态**由 Python
    按定义现算生成**（素数立方根小数部分），避免手抄常量；向量对着 `hashlib` 钉住。
    模块头写明「`sha2` 可用后应整体替换这两个私有模块」。
  - `shake_128` / `shake_256` **不实现**：Keccak 代码量与风险远高于 SHA-2，
    `to_checksum` 返回 `ChecksumError::Unsupported`；`SHAKE_LENGTHS` 常量保留，
    好让调用方能识别这两个名字而不是当成未知算法。
  - `ALGORITHMS`（`sorted(hashlib.algorithms_available)`，本机 40 项、**环境相关**）
    不迁移，改用 `SUPPORTED_ALGORITHMS`（= `COMMON_ALGORITHMS` 四项）→ 记为已知差异
    （任务卡的算法菜单将只有主菜单那四项，没有「更多」子菜单）。
  - `to_checksum` **同步阻塞**（Python 是 `async` + 每块 `asyncio.to_thread`），
    取消靠调用方丢弃任务。
  - `match_checksum` 收 `(&算法, &值)` 的迭代器：Python 入参是保序 `dict[str, str]`，
    泛型迭代器让 `serde_json::Map` / `BTreeMap` / 数组都能直接传。
- 实证结论（源码级 + CPython 3.13 实测，全部有单测钉住）：
  - `parseChecksum` = `(text.split() or [""])[0].lower()`：无参 `split()` 按「一段连续
    空白」切、丢空段 → 前导空白不影响；空白口径是 `str.isspace()`，**包含** `\x1c`-`\x1f`
    （Rust 的 `char::is_whitespace` 不含）→ 私有 `py_isspace`；只取第一段。
  - `f.read(CHUNK_SIZE)` 走 `BufferedReader`，**除非 EOF 否则返回满块**；
    直接用 `File::read` 的短读会让进度回调的次数与取值和 Python 不一致
    → `read_chunk` 循环填满 buffer。
  - `size = path.stat().st_size` 在**开文件之前**取；空文件一块都读不到，
    `done * 100 // size` 根本不执行（Python 侧也就不存在 `ZeroDivisionError`）。
  - 向量全部用 CPython 的 `hashlib` 重算核对过一遍（11 组 × 4 算法 + 4MiB+7 大文件）：
    空串、`abc`、`hello world\n`、55/56/63/64/111/112/127/128 字节
    （SHA-256 的 55/56 与 SHA-512 的 111/112 是两条填充边界），**逐字一致**。
- 修掉的两处自己写的问题：
  - 测试里 `matches!(error, Unsupported { ref algorithm } if algorithm == algorithm.as_str())`
    是**恒真**的（模式变量遮蔽了外层 `algorithm`）→ 改成 `match &error` + `assert_eq!`；
  - clippy `useless_conversion`：`self.state.iter_mut().zip([a,..,h].into_iter())`
    两处 `.into_iter()` 是多余的（`zip` 收 `IntoIterator`）→ 删掉。
- 验证：
  - `cargo test -p gd3-core --lib`：**205 passed**（第 87 步 195 + 10）；
  - `cargo test --workspace`：**1054 passed, 0 failed**（第 87 步 1044 + 10）；
  - `cargo clippy --workspace --all-targets -- -D warnings` **0 warning**（1m17s）；
  - `cargo fmt --all --check` 通过；
  - Python 侧没改一行，不需要跑 Python 全量；
  - commit `888154d`。
- 环境记录：`nohup cmd &` 起的长任务会被 Bash 工具在命令结束时**整组杀掉**
  （日志 0 字节、进程消失）；改用 `setsid env ... > log 2>&1 < /dev/null & disown`
  才能真正常驻，然后轮询日志。本轮 `cargo test --workspace`（`CARGO_BUILD_JOBS=1`）
  约 6 分钟，clippy 1m17s。
- 下一步：
  1. `app/platform/run_at_login.py`（79 行）+ `app/platform/file_association.py`（148 行）
     的 Linux 分支：`.desktop` 自动启动项、`loadMimeTypes` / `saveMimeTypes`
     （`~/.local/share/applications` + `dbus-1/services` 两份文件）、
     `registerUrlScheme` / `unregisterUrlScheme`；`home` / `executable_path` / `desktop_id`
     / `version` 全部作参数传入（照第 87 步 `app_version` 的做法）；
     Windows 注册表 / macOS plist 的**文本**可以做纯函数，注册表 API 判 N/A；
  2. `app/platform/sysio.py`（131 行）**判 N/A**：POSIX 分支就是
     `from os import pwrite, ftruncate`，Rust 侧对应 `FileExt::write_at` / `File::set_len`，
     没有可搬的逻辑；Windows 分支是 `ctypes` 调 `WriteFile` / `FSCTL_SET_SPARSE`；
  3. `app/services/runtime_status.py`（195 行）暂时做不了：`install` /
     `fetchLatestVersion` 在 Python 是 `async` 且 `pack::BinaryRuntime` 里**还没搬**
     （它们要网络），需要先把这两个抽象方法定成同步签名；
  4. `app/services/speed_meter.py`（56 行）/ `plan.py`（53 行）都太小，
     可以顺手并进相邻步骤。
- 卡点：无

## 第 89 步：gd3-core::file_association + gd3-core::run_at_login（2026-10-18 11:19）

- 做了什么：
  - 新增 `rust/crates/gd3-core/src/file_association.rs`（**548 行 / 14 单测**）：
    `MIME_PREFIX` / `desktop_file_path` / `service_file_path` / `load_mime_types` /
    `desktop_entry_text` / `service_entry_text` / `save_mime_types` /
    `register_file_types` / `register_url_scheme` / `unregister_url_scheme` +
    私有（现 `pub(crate)`）`remove_if_exists`。
  - 新增 `rust/crates/gd3-core/src/run_at_login.rs`（**415 行 / 14 单测**）：
    `LAUNCH_AGENT_LABEL` / `WINDOWS_RUN_KEY` / `WINDOWS_VALUE_NAME` /
    `autostart_file_path` / `autostart_entry_text` / `set_autostart` /
    `launch_agent_path` / `launch_agent_text` / `set_launch_agent` /
    `windows_run_value` / `set_run_at_login`。
  - `lib.rs`：加 `pub mod file_association;` / `pub mod run_at_login;` + 21 个 re-export；
    `pack.rs` 的 `split_lines` 从 `fn` 改成 `pub(crate) fn`（`file_association` 复用，
    不再复制第 4 份）；
  - `rust/README.md` 新增「Linux 桌面集成」一节。
- 关键决策：
  - **`home` / `desktop_id` / `executable_path` / `version` 全部作参数传入**（照第 87 步
    `app_version`）：core 不硬编码 `gd3-ui` 的 `DESKTOP_ID` / `VERSION`，更不猜
    `EXECUTABLE_PATH`（Python 侧是 `$APPIMAGE` → `sys.argv[0]`（frozen）→
    `sys.executable` 的运行时探测结果）。macOS 的 plist 路径在 Python 里写死成
    `/Users/{getpwuid(...).pw_name}/...`，这里换成 `home`（macOS 上等价，且可测）。
  - **`save_mime_types` 把 `subprocess.run` 拆成返回值 `Vec<Vec<String>>`**：
    `update-desktop-database <applications>` 总排第一（即使 mimes 为空、文件已被删），
    之后每个 mime 一条 `xdg-mime default <desktop_id>.desktop <mime>`；Python 在
    `FileNotFoundError` 时 `break`，调用方按「`NotFound` 就停」复刻。
  - **mime 集合用 `BTreeSet` 而不是 `set`**：`xdg-mime` 的调用顺序变确定，落盘内容不受影响
    （`';'.join(sorted(mimes))` 本来就排序）。
  - **`register_file_types` 保留「丢掉非 `x-scheme-handler/*` 的已有 MIME」**：
    `saveMimeTypes(schemes | {ft.mimeType for ft in fileTypes})` 原样如此，写进 doc + 单测钉住。
  - **`remove_if_exists` 只吞 `NotFound`**（镜像 `unlink(missing_ok=True)`），
    改成 `pub(crate)` 供 `run_at_login` 的禁用分支复用。
  - **Windows 注册表判 N/A，但 `set_run_at_login` 在 Windows 上返回 `Unsupported` 而非
    静默成功**：Python 用 `winreg` 写 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`，
    本机离线拿不到 `windows-sys` / `winreg` crate。静默 `Ok(())` 会让用户以为设置生效；
    `windows_run_value` 保留数据格式（`"<EXE>" --silence`）+ 单测，等 API 可用时接上。
- 实证结论（CPython 3.13 实测 + 源码级，全部有单测钉住）：
  - Linux `.desktop` 正文 **217 字节**、macOS plist **398 字节**（`executable =
    /opt/ghost/GhostDownloader`、`VERSION = 4.4.0`）：用
    `QT_QPA_PLATFORM=offscreen HOME=<tmp> setRunAtLogin(True)` 真跑一遍生成文件后
    `repr()` 取值，两条断言逐字比较（含 `Exec="..." --silence` 的引号）；
  - `loadMimeTypes` 在循环里**直接 `return`**：只有第一个 `MimeType=` 行有效
    （第二行 `MimeType=z;` 被忽略，有单测）；空段 `a;b;;c;` 被丢掉；
  - 空集合时只 `unlink` 两份文件、**不 `mkdir`、不删目录**，但
    `update-desktop-database` 照样跑（单测断言命令数 = 1、`.local/share` 不存在）；
  - `run_at_login` 禁用时同样只 `unlink`，`~/.config/autostart` / `LaunchAgents` 留着；
  - 两个 Python 文件里 `file_association.py` / `run_at_login.py` **都没有 `try/except`**
    （外层 `try/except Exception` + log 在 `register` / `registerUrlScheme` 那三个入口），
    所以 Rust 侧一律返回 `io::Result`，日志由调用方打；
  - Python 的 `sys.platform` 三分支没有 `else`（未知平台静默不做）→ Rust 侧同样 `Ok(())`。
- 验证：
  - `cargo test -p gd3-core --lib`：**233 passed**（第 88 步 205 + 28）；
  - `cargo test --workspace`：**1082 passed, 0 failed**（第 88 步 1054 + 28，10m47s 编译）；
  - `cargo clippy --workspace --all-targets -- -D warnings` **0 warning**（3m08s）；
  - `cargo fmt --all --check` 通过；
  - Python 侧没改一行，不需要跑 Python 全量；
  - commit `85c0e6e`。
- 下一步（按价值/风险排序）：
  1. `app/services/runtime_status.py`（195 行）需要先给 `pack::BinaryRuntime` 补
     `install` / `fetch_latest_version` 的**同步签名**（Python 是 `async` 且要网络）；
  2. `app/platform/hidden_subprocess.py`（99 行）：Linux 分支是 `subprocess` +
     `start_new_session`，Rust 侧 `CommandExt::pre_exec`；
  3. `app/platform/desktop.py`（405 行）：shutdown / restart / sleep / openFile，
     Linux 走 `systemctl` / `xdg-open`；
  4. `app/services/speed_meter.py`（56 行）/ `plan.py`（53 行）/
     `name_conflict_queue.py`（73 行）都太小，并进相邻步骤；
  5. D-Bus / keepalive 仍排最后（`zbus` 缓存可用但编译代价大）。
- 卡点：无

## 第 90 步：gd3-core::desktop + filesystem::which（2026-10-18 11:31）

- 做了什么：
  - 新增 `rust/crates/gd3-core/src/desktop.rs`（**604 行 / 14 单测**）：
    `LINUX_BROWSERS`（7 项）/ `MACOS_BROWSERS`（4 项）/ `browser_url` /
    `shutdown_command_on` / `restart_command_on` / `sleep_command_on` /
    `open_file_command_on` / `open_folder_command_on`（共用私有 `open_target_command_on`）/
    `reveal_in_folder_on`（+ 私有 `parent_display`）/ `open_chromium_url_on` /
    `open_chromium_url` / 六个当前平台包装 / `spawn_detached`（`setsid(2)` + 三路 `Stdio::null()`）。
  - `filesystem.rs` 新增 `pub fn which(name)`（`shutil.which` 的纯 `PATH` 版，1 单测）；
    `lib.rs` 加 `pub mod desktop;` + 18 个 re-export，`filesystem` 的 re-export 补 `which`；
  - `rust/README.md` 新增「桌面动作（`gd3-core::desktop`）」一节。
- 关键决策：
  - **`*_on(os, ...)` 纯函数 + 不带 `_on` 的包装**（包装只做 `std::env::consts::OS` 分发），
    这样三条平台的命令表都能在本机（Linux）被单测钉住，而不是靠 `cfg!` 分支。
  - **`os` 判定照抄 `match sys.platform`：只认 `win32` / `darwin`，其余全部落进 `case _`**。
    上一轮凭印象写的版本对未知平台返回 `None`，这是错的——Python 里
    `"freebsd13"` / `"openbsd7"` / 空串拿到的都是 POSIX 分支的
    `shutdown -h now` / `systemctl suspend` / `xdg-open` / Linux 浏览器表。
    已改成 `shutdown_command_on` 等直接返回 `Vec<String>`，`Option` 只用于表示
    「Windows 未迁移」；单测 `unknown_platform_falls_into_posix_branch` 钉住。
  - **`openChromiumUrl` 的 `which` 用闭包注入**：单测传假 `which`，不依赖本机装了什么浏览器。
    `open_chromium_url` 改用新加的 `filesystem::which` 而不是
    `find_executable(Path::new(""), name)`——后者的第一条分支是 `""/<name>` 的
    `is_file()`，也就是**相对当前目录**，`shutil.which` 没有这一步。
  - **`buildFontFamilies` / `deleteRecoverably` 仍标「未迁移」**：前者入参是 `QLocale` 的
    `script()` / `territory()`（要自己造 `Script`/`Territory` 枚举才能搬，且 Slint 侧没有
    对应概念）；后者要 freedesktop 回收站实现（下一步候选）。
- 实证结论（回读 `app/platform/desktop.py:120-234` 逐条核对，全部有单测钉住）：
  - `shutdown`：win32 `["shutdown","/s","/t","0"]`、darwin `["osascript","-e","tell app \"System Events\" to shut down"]`、`case _` `["shutdown","-h","now"]`；
  - `restart`：`/r` / `... to restart` / `["shutdown","-r","now"]`；
  - `sleep`：`["rundll32.exe","powrprof.dll,SetSuspendState","0","1","0"]` / `["pmset","sleepnow"]` / `["systemctl","suspend"]`；
  - `revealInFolder` 的三分支：`isExisting(path)` → win32 `SHOpenFolderAndSelectItems`（N/A）/ darwin `open -R <path>` / `case _` `xdg-open <str(path.parent)>`；
    `elif isExisting(path.parent)` → `openFolder(path.parent)`；否则什么都不做；
  - Linux 浏览器候选表字面顺序：`google-chrome`、`google-chrome-stable`、
    `microsoft-edge-stable`、`microsoft-edge`、`brave-browser`、`chromium-browser`、`chromium`；
    macOS：`Google Chrome`、`Microsoft Edge`、`Brave Browser`、`Chromium`；
  - `toBrowserUrl`：`url.startswith("chrome://")` → `f"{scheme}://{url[9:]}"`，
    只剥一次、大小写敏感、`"chrome://"` 本身换成 `"{scheme}://"`；
  - `str(Path("foo").parent)` == `"."`，而 Rust 的 `Path::new("foo").parent()` 是空路径
    （`display()` 是空串）；`Path::new("/").parent()` 是 `None`，Python 的
    `Path("/").parent` 还是 `Path("/")` → 私有 `parent_display` 把两种边界都对齐。
- 验证：
  - `cargo test -p gd3-core --lib`：**248 passed**（第 89 步 233 + 15）；
  - `cargo test --workspace`：**1097 passed, 0 failed**（第 89 步 1082 + 15，约 10 分钟编译）；
  - `cargo clippy --workspace --all-targets -- -D warnings` **0 warning**（1m24s）；
  - `cargo fmt --all --check` 通过；
  - Python 侧没改一行，不需要跑 Python 全量；
  - commit `93be2db`。
- 下一步（按价值/风险排序）：
  1. **`gd3-core::trash`**：`desktop.py:112-114` 的 `deleteRecoverably`
     （`QFile.moveToTrash`）→ freedesktop 回收站规范：`$XDG_DATA_HOME/Trash`、
     每个挂载点的 `.Trash/$uid` 或 `.Trash-$uid`、`files/` + `info/*.trashinfo`
     （`[Trash Info]` 的 `Path=` 百分号编码 + `DeletionDate=`）；
     `deleteRecoverably` 被 `engine.py:17,22` / `startup.py:36,50` 注入，是真实缺口；
  2. `app/platform/hidden_subprocess.py`（99 行）：Linux 分支 `subprocess` +
     `start_new_session`，可复用本步的 `spawn_detached`；
  3. `buildFontFamilies`：造 `Script` / `Territory` 枚举后搬（纯函数）；
  4. `app/services/runtime_status.py`（195 行）仍需先给 `pack::BinaryRuntime` 补
     `install` / `fetch_latest_version` 的同步签名（Python 是 `async` 且要网络）；
  5. D-Bus / keepalive 仍排最后（`zbus` 缓存可用但编译代价大）。
- 卡点：无

## 第 91 步：gd3-core::trash（2026-10-18 11:56）

- 做了什么：
  - 新增 `rust/crates/gd3-core/src/trash.rs`（**899 行 / 23 单测**），镜像 Qt 的
    `QFile::moveToTrash`（= `app/platform/desktop.py:112-114` 的 `deleteRecoverably`）：
    `home_trash_dir` / `generic_data_dir` / `topdir_trash_dir` / `choose_trash_dir`（纯路径选择）/
    `TrashLocation{dir, topdir}` / `resolve_trash_dir` / `mount_point_of` / `shared_trash_ok` /
    `encode_path` / `trash_info_text` / `trash_entry_name` / `current_uid` /
    `deletion_date_at` / `deletion_date_now` / `move_to_trash` / `delete_recoverably`。
  - `lib.rs`：`pub mod trash;`（:29）+ 16 个 re-export。
  - `rust/README.md` 新增「可恢复删除（`gd3-core::trash`）」一节（插在「剩余工作（Phase 8）」之前）；
    `desktop.rs` 文档表里 `deleteRecoverably` 那行从「未迁移」改成「见 `crate::trash`」。
- 关键决策：
  - **复刻的是 Qt，不是 freedesktop 规范**：Python 两行代码把整件事委托给 `QFile.moveToTrash`，
    所以全部口径都用 PySide6 + `QT_QPA_PLATFORM=offscreen` 真跑实测（第 91 步调研的探针目录
    `/root/gd3probe2..10`，本轮又补了 `gd3probe-py*` / `gd3probe-rust*` 两组交叉验证）。
  - **`TrashLocation { dir, topdir }` 而不是裸 `PathBuf`**：`Path=` 的口径两种回收站不一样，
    必须由同一个判定结果驱动，否则会重算一遍设备/sticky 出现两处不一致。
  - **时间戳作参数传入**（`deletion_date: &str`），只有 `delete_recoverably` 去取
    `deletion_date_now()`。`localtime_r` 走 `libc`（`chrono` 不在离线缓存里），
    格式 `%Y-%m-%dT%H:%M:%S`。
  - **`move_to_trash` 返回 `io::Result<PathBuf>`，`delete_recoverably` 返回 `bool`**：
    前者给调用方日志用（`Err` = Python 的 `False`），后者保留 `deleteRecoverably` 的原始口径。
  - **不做跨设备 `copy` 兜底**：回收站目录本来就按「文件所在设备」选，`rename` 不会跨设备；
    Qt 在这里也只是失败回滚（`rename` 失败把刚写的 `.trashinfo` 删掉），Rust 同样。
- 实证结论（**本轮抓到并修掉一个真实偏差**）：
  - **`Path=` 在顶层回收站里是相对 `$topdir` 的**，不是绝对路径：
    Qt 把 `/tmp/gd3probe-py2/sub/deep/x y 中.txt` 写成
    `Path=gd3probe-py2/sub/deep/x%20y%20%E4%B8%AD.txt`（不带前导 `/`）。
    初稿写的是绝对路径，交叉验证时发现不一致，改成 `TrashLocation::original_path`
    （home → 原样；顶层 → `strip_prefix(topdir)`）。
  - home 回收站写**调用方传进来的原样**：`/root/gd3probe-py2/sub/y.txt` 原样、
    相对路径 `rel.txt` 也原样（`Path=rel.txt`）；
  - 顶层回收站目录选择与 Qt 完全一致：`/tmp` 上实测两侧都落到 `/tmp/.Trash-0`
    （`/tmp` 是 tmpfs、`/root` 在 `/` 上，正好跨设备）；
  - `$XDG_DATA_HOME` 不存在 → `False` 且什么都不建；源文件不存在 → `False` 但
    `Trash/{files,info}` 已建好；`Trash` / `files` / `info` 三个目录都是 `0700`；
  - 冲突命名 `dup.txt`、`dup.txt-271242`、`dup.txt-271243`……（第 i 次用 `inode + i - 1`）；
  - 百分号编码保留 `A-Za-z0-9-._~/`（**斜杠不编码**），其余按 UTF-8 逐字节大写 `%XX`。
- 已知差异（都写进模块文档与 README）：
  1. **Qt 在跨设备分支里会顺手建一个空的 `$XDG_DATA_HOME/Trash`**（内部流程先试 home
     回收站），Rust 侧不建——空目录对用户不可见，规范也没要求；
  2. `Path=` 里的非 UTF-8 字节：Python 走 `surrogateescape` 原样保留，Rust 走
     `to_string_lossy()`；
  3. Qt 在无 sticky 的 `.Trash` 上打 `qWarning`，库侧没有 logger，分支选择改由
     `shared_trash_ok` 的返回值暴露；
  4. 冲突重试上限都是 100 次，但 Qt 之后**静默**返回 `False`，这里返回 `AlreadyExists`。
- 验证：
  - `cargo test -p gd3-core --lib`：**271 passed**（第 90 步 248 + 23）；
  - `cargo test --workspace`：**1120 passed, 0 failed**（第 90 步 1097 + 23，约 9 分钟编译）；
  - `cargo clippy --workspace --all-targets -- -D warnings` **0 warning**（5m10s，
    中途抓到一处 `needless_as_bytes`：`text.as_bytes().len()` → `text.len()`）；
  - `cargo fmt --all --check` 通过；
  - 交叉验证：临时集成测试 `tests/trash_probe_tmp.rs` 跑完即删（工作树里没有残留），
    Rust 与 Qt 对同一文件写出的 `Path=` 逐字节一致；
  - Python 侧没改一行，不需要跑 Python 全量；
  - commit `da4a890`。
- 下一步（按价值/风险排序）：
  1. **`gd3-ui::task_service` 的 `OVERWRITE` 分支**：`TaskStore.add` 遇到
     `NameConflictChoice::OVERWRITE` 时先 `move_to_trash` 再重建占位符，失败则
     `isOverwriteFailed = True` + 退化成 `KEEP_BOTH`（`app/services/task_service.py:69-90`）
     ——`trash` 落地后的自然下游，也是唯一真实调用点；
  2. `app/platform/hidden_subprocess.py`（99 行）判 N/A（非 win32 直接 return），
     若要搬可复用 `desktop::spawn_detached`；
  3. `buildFontFamilies`（`desktop.py:15-45`）：造 `Script` / `Territory` 枚举后可搬（纯函数）；
  4. `app/services/runtime_status.py`（195 行）仍需先给 `pack::BinaryRuntime` 补
     `install` / `fetch_latest_version` 的同步签名（Python 是 `async` 且要网络）；
  5. D-Bus / keepalive 仍排最后（`zbus` 缓存可用但编译代价大）。
- 卡点：无

## 第 92 步：gd3-core::placeholder（2026-10-18 12:13）

- 做了什么：把 `TaskStore.add` 的「占位符 + 重名冲突决策」整条链路搬进
  `gd3-core::placeholder`（**915 行 / 31 单测**）。
  - `NameConflictChoice { KeepBoth, Overwrite, Ask }` + `as_str()`（`"keepBoth"` /
    `"overwrite"` / `"ask"`）+ `parse()`；`Default = KeepBoth`。
  - `placeholder_paths(output_folder, name) -> Vec<PathBuf>`：恒 `[folder/name]`。
  - `sidecar_path(path)`：`Path(f"{path}.ghd")` 的等价物（`OsString::push`）。
  - `delete_placeholder(path) -> io::Result<()>`：空目录 → `remove_dir`，
    0 字节文件 → `remove_file`，其余一律不动。
  - `create_placeholders(paths, existing, is_output_folder)
    -> Result<Vec<PathBuf>, CreateError>`：`is_output_folder && index == 0` →
    `create_dir`，否则 `create_new`；失败回滚本次已建成的；成功后 `delete_path`
    清掉每个 `<path>.ghd`。
  - `CreateError::{Taken(TakenPath), Io{path, error}}` + `taken_path()`。
  - `resolve_conflict(name, choice, create, is_unfinished, delete_recoverably)
    -> Result<Option<ConflictOutcome>, CreateError>`。
  - `PlaceholderRegistry`（`by_task: BTreeMap<String, Placeholders>` +
    `by_path: BTreeMap<PathBuf, String>`）：`set`/`remove`/`get`/`owner`/`len`/
    `is_empty`/`is_unfinished(path, status_of)`/`replaced_tasks(paths)`。
  - `lib.rs`：`pub mod placeholder;`（:23）+ 12 个 re-export。
  - `rust/README.md` 新增「占位符与重名冲突（`gd3-core::placeholder`）」一节
    （插在「剩余工作（Phase 8）」之前，README 现 759 行）。
- 关键决策：
  - **冲突决策做成「纯循环 + 闭包注入」。** `resolve_conflict` 不认识 `Task`，
    只通过 `create` / `is_unfinished` / `delete_recoverably` 三个闭包与外界交互。
    五个决策分支（建成 / `Ask` 放弃 / `Overwrite` 成功同名重试 / `Overwrite` 失败
    退化 `KeepBoth` / 未完成占位符强制改名）全部在 core 层被单测钉住；
    `TaskService` 的接线（含 `tasks.jsonl` 落盘与 `replacedTasks` 移除）留第 93 步。
  - **`CreateError` 区分 `Taken` 与 `Io`。** Python 只有 `FileExistsError` 会被
    `except` 抓住继续决策，其余 `OSError`（父目录没了、ENOSPC、EACCES）直接冒泡
    把 `add` 打断。用同一个 `TakenPath` 表达两者会让「权限不足」被误当成「重名」，
    在 `Overwrite` 分支里反复重试。
  - **`PlaceholderRegistry` 存 `taskId` 而不是任务对象。** Python 的
    `_taskByPlaceholder` 存 `Task`、`_isUnfinishedPlaceholder` 直接读 `task.status`；
    core 层不认识 `TaskRecord` 的生命周期，所以 `is_unfinished` 收一个
    `status_of: impl Fn(&str) -> Option<TaskStatus>`。
  - **改名循环加了 `MAX_RENAME_ATTEMPTS = 10_000` 上限。** Python 的
    `while True` 没有上限：文件名长到触发 `toSafeFilename` 的 200 字符截断时，
    `name(1).ext` 与 `name(2).ext` 截断后可能完全相同 → 死循环。到达上限后返回
    最后一次的 `Taken`（而不是静默给出错误名字）。
- 复核过的 Python 口径（第 92 步调研已读完，本轮再确认一遍）：
  - `add`（`task_service.py:62-90`）：`mkdir(parents=True, exist_ok=True)` →
    `stem, ext = splitStemExt(task.name)`（取自**原始**名字）→ `while True`；
    `Overwrite` 成功时 `continue`（同名重试，不改名），失败时
    `isOverwriteFailed = True` + `choice = KEEP_BOTH`；
    未完成占位符既不 `Ask` 放弃也不覆盖，直接 `index += 1` 改名。
  - `setName` = `toSafeFilename(name, fallback=self.name or "download")` ——
    每次改名的 fallback 都是**上一个** `current`，所以 `resolve_conflict` 里
    `to_safe_filename(&format!("{stem}({index}){ext}"), &current, 200)`。
  - `_createPlaceholders`（:135-155）：跳过已在 `existingPaths` 的；
    `isOutputFolder and path == Path(task.outputPath)` → `mkdir`，否则
    `touch(exist_ok=False)`；`FileExistsError` 时把本次 `created` 逐个
    `deletePlaceholder` 再抛；成功后对每个 created `deletePath(f"{path}.ghd")`。
  - `_isUnfinishedPlaceholder`（:130-133）：`task is not None and
    task.status != COMPLETED`。
  - `_setPlaceholders`（:156-162）：先 `_removePlaceholders(taskId)` 再写两张表。
  - `_removePlaceholders`（:163-171）：pop 之后**只删 `by_path[path].taskId ==
    taskId` 的项**（这个守卫有单测钉住：t2 抢走路径后移除 t1 不能清掉 t2 的登记）。
  - `deletePlaceholder`（`task.py:20-25`）：`is_dir() and not any(iterdir())` →
    `rmdir`；`elif is_file() and st_size == 0` → `unlink`。
  - `Task.placeholderPaths`（`task.py:241-251`）：`[Path(outputPath)]` + step 循环；
    step 循环因 `TaskStep.outputPath` 基类返回 `""` 且全仓无子类覆盖而永不追加。
- 已知差异（都写进模块文档与 README）：
  1. **`placeholder_paths` 只返回 `[outputFolder/name]`**，不遍历 `steps`；
  2. **改名有 10_000 次上限**（Python 死循环）；
  3. `delete_placeholder` 返回 `io::Result` 而不是静默 —— Python 的
     `deletePlaceholder` 遇到 `OSError` 会抛，回滚循环也跟着中断。
- 验证：
  - `cargo test -p gd3-core --lib`：**302 passed**（第 91 步 271 + 31）；
  - `cargo test --workspace`：**1151 passed, 0 failed**（28 个 test result 段，
    第 91 步 1120 + 31，约 11 分钟编译）；
  - `cargo clippy --workspace --all-targets -- -D warnings` **0 warning**（1m15s）；
  - `cargo fmt --all --check` 通过；
  - Python 侧没改一行，不需要跑 Python 全量；
  - commit `c8dfd5e`。
- 下一步：
  1. **第 93 步：把 `placeholder` 接进 `gd3-ui::task_service`。** 目标形态
     `TaskService::add_with_conflict(record, choice, auto_start) ->
     Option<AddOutcome>`：`output_folder.mkdir(parents=True)` → 用
     `create_placeholders` + `PlaceholderRegistry` 驱动 `resolve_conflict` →
     `Ok(None)` 映射成「返回 `None`」→ 按 `replaced_tasks` 移除被替换任务 →
     插入 → `registry.set` → 复用现有磁盘检查 + 入队逻辑；`delete_recoverably`
     直接接 `trash::delete_recoverably`。`AddOutcome` 要补
     `replaced_tasks` / `overwrite_failed` 两个字段（对应 `AddResult`）。
  2. 之后候选（按价值/风险）：`buildFontFamilies`（`desktop.py:15-45`，纯函数）→
     `TaskStore.updatePlaceholders`（改名/移动占位符）→
     `app/services/runtime_status.py`（195 行，需先给 `pack::BinaryRuntime` 补
     `install`/`fetch_latest_version` 的同步签名）→ D-Bus / keepalive 仍排最后。
- 卡点：无

## 第 93 步：把 placeholder 接进 gd3-ui::task_service（2026-10-18 12:39）

**目标**：第 92 步把「建占位符 + 冲突决策」做成了与 `Task` 无关的纯函数，这一步把它接回
`TaskService`，补齐 Python `TaskService.add`（`app/services/task_service.py:358-383`）里
`TaskStore.add`（`:62-90`）那一段。

### 改动清单

`gd3-core` 侧两处前置改动（都是为了让「按候选名字重算路径」成为可能）：

| 文件 | 改动 |
|---|---|
| `task.rs` | 抽出 `pub fn output_path_for(&self, name: &str) -> String`；`output_path()` 改为委托 |
| `placeholder.rs` | 新增 `pub fn placeholder_paths_of(output_path: &str) -> Vec<PathBuf>` |
| `lib.rs` | re-export 补 `placeholder_paths_of` |

`gd3-ui/src/task_service.rs`（909 → 1271 行，21 → 32 单测）：

- imports：`std::fs`、`std::path::{Path, PathBuf}`、`gd3_core::filename::{DEFAULT_MAX_FILENAME_LENGTH,
  to_safe_filename}`、`gd3_core::placeholder::{CreateError, NameConflictChoice, PlaceholderRegistry,
  create_placeholders, placeholder_paths_of, resolve_conflict}`；
- `pub struct AddReport { name, replaced: Vec<String>, overwrite_failed: bool,
  outcome: Option<AddOutcome> }`；
- `TaskService` 新增字段 `placeholders: PlaceholderRegistry`，配 `placeholders()` 只读访问器；
- `add_with_free_space` 的尾段（磁盘检查 + `queue.wait`）抽成私有
  `fn schedule_add(&mut self, task_id, file_size, auto_start, free_space) -> AddOutcome`，
  两条 add 路径共用；
- 新增 `pub fn add_with_conflict<D: FnMut(&Path) -> bool>(record, choice, auto_start,
  free_space, delete_recoverably) -> Result<AddReport, CreateError>`；
- `remove()` 补 `self.placeholders.remove(task_id)`（Python 的 `delete` 也会
  `_removePlaceholders`）；
- 模块 doc 的「已知差异」重写：`add()`/`add_with_free_space()` 不解决冲突、
  `add_with_conflict()` 不调 `CategoryService.outputFolderOf()`。

### `add_with_conflict` 的执行顺序（逐条对齐 Python）

```text
1. 已存在同 taskId            -> Ok(AddReport { outcome: Some(Duplicate) })，不进冲突循环
2. name = toSafeFilename(name, "download", 200)   # Task.__post_init__ 的不变量
3. shouldSeed = True
4. fs::create_dir_all(outputFolder)               # mkdir(parents=True, exist_ok=True)
5. resolve_conflict(name, choice,
       create = |candidate| create_placeholders(placeholder_paths_of(
                    record.output_path_for(candidate)), &[], is_output_folder),
       is_unfinished = |path| registry.is_unfinished(path, |id| records[id].status),
       delete_recoverably)
6. Ok(None)  -> Ok(AddReport { outcome: None })    # nameConflicted + return False
7. replaced = registry.replaced_tasks(paths)       # 按占位符顺序去重
8. for id in replaced: self.remove(id)             # taskRemoved
9. records.push(record); registry.set(taskId, outputFolder, name, paths)
10. pending_flush = true; schedule_add(...)        # 磁盘检查 + 入队
```

### 关键决策

- **`AddReport.outcome: Option<AddOutcome>` 而不是往 `AddOutcome` 里加变体。**
  `None` 精确表达 Python 的 `nameConflicted` + `return False`——任务**没**加进
  `_store._tasks`，调用方要拿同一个 `Task` 重新问用户选什么。塞进 `AddOutcome` 会让
  「已加入/未加入」这个二分变得不可穷举。
- **`output_path_for(name)` 抽成独立方法。** `BilibiliTask` 多文件时 `outputPath` 取
  `outputFolder / Path(name).stem`，改名重试的每个候选名字都要**整条重算**，不能只换路径
  最后一段（否则 `a(1).bin` 会算成 `a.bin(1)`）。
- **`add_with_conflict` 入口补一次 `to_safe_filename(..., "download", 200)`。** 复刻
  `Task.__post_init__` 的不变量，防止调用方塞进带 `/` 的名字把占位符建到输出目录之外；
  该函数幂等，对已安全的名字无副作用。
- **`delete_recoverably` 由调用方注入**，单测传 `|_| false`（`never_trash`）保持
  hermetic；生产传 `gd3_core::trash::delete_recoverably`，`Err` 按 `false` 处理即可
  （与 Python 的 `try/except` 一致）。
- **错误类型是 `CreateError` 而不是 `io::Error`。** `create_dir_all` 的失败手动映射成
  `CreateError::Io { path, error }`：Python 里 `mkdir` 的 `OSError` 与建占位符的
  `OSError` 是同一种冒泡，而 `CreateError::Taken` 是改名循环用尽 10_000 次的上限信号。
  （首版写 `io::Result`，`?` 直接编译失败——`From<CreateError> for io::Error` 不存在。）
- **`schedule_add` 抽出来而不是复制。** 两条 add 路径的尾段（`fileSize > 0 && free <
  fileSize` → `DiskSpaceInsufficient` 不入队；否则 `queue.wait`）必须逐字一致，
  第 81 步那版「`fileSize > 0` 才入队」的 bug 就出在这段。

### 单测（10 个，全部 hermetic：`TempDir` + `AtomicU32` + `Drop`）

| 测试 | 钉住的行为 |
|---|---|
| `..._creates_placeholder_and_registers_it` | `mkdir -p` 输出目录 + 0 字节占位符 + `owner == taskId` |
| `..._renames_when_placeholder_is_taken` | `KeepBoth` → `a(1).bin`，两个任务都在 |
| `..._ask_leaves_the_task_out` | 无主占位符 + `Ask` → `outcome == None`、`len == 0`、不置 `pending_flush`、原文件不动 |
| `..._overwrite_reuses_the_name` | 回收站移入成功 → **同名**重试，`trashed == [path]` |
| `..._overwrite_failure_degrades_to_keep_both` | 移入失败 → `overwrite_failed == true` + `a(1).bin` + 原文件保留 |
| `..._reports_replaced_task` | `replaced == ["a"]`、旧任务被 `remove`、登记转给 `b` |
| `..._short_circuits_on_duplicate` | 同 taskId → `Duplicate`，`delete_recoverably` 一次都不调 |
| `..._never_overwrites_unfinished_task` | `Running` 占位符 → 只改名，`calls == 0` |
| `..._creates_a_directory_for_multi_file_tasks` | `files.len() == 2` → 占位符是目录 |
| `..._uses_the_same_tail_as_plain_add` | `DiskSpaceInsufficient` 仍留在列表、不入队；`autoStart=false` → `queued: false` |

### 验证

- `cargo check -p gd3-ui --lib --tests`：44s 通过（首版 1 个 E0277，见上「错误类型」）；
- `cargo test -p gd3-ui --lib`：**663 passed**（+10）；
- `cargo test --workspace`：**1161 passed / 0 failed**（28 个 test result 段，
  第 92 步 1151 + 10，编译约 11 分钟）；
- `cargo clippy --workspace --all-targets -- -D warnings`：**0 warning**（1m15s）；
- `cargo fmt --all --check` 通过；
- Python 侧没改一行，不跑 Python 全量；
- 代码 commit `40fb910`。

### 下一步（第 94 步候选，按价值/风险）

1. **`TaskStore.updatePlaceholders`**（`task_service.py:91-121`，改名/移动占位符）：
   `PlaceholderRegistry` 已就位，剩下 `zip(oldPaths, newPaths)` + `move` + `.ghd` 搬运，
   纯文件操作，风险低；
2. **`buildFontFamilies`**（`app/platform/desktop.py:15-45`）：造 `Script`/`Territory`
   枚举后可搬，纯函数；
3. **`app/services/runtime_status.py`（195 行）**：需先给 `pack::BinaryRuntime` 补
   `install`/`fetch_latest_version` 的同步签名（Python 是 `async` 且要网络）；
4. D-Bus / keepalive：仍排最后（`zbus` 在离线缓存里，但编译代价大）。

### 卡点

无。

## 第 94 步：TaskService::update_placeholders（改名/移动占位符，2026-10-18 13:03）

**目标**：补齐 Python `TaskStore.updatePlaceholders`（`app/services/task_service.py:91-121`）——
第 93 步只做了「新建占位符 + 重名决策」，改名/移动这条路径还缺。它是编辑任务
（`edit_task`）、改输出目录、以及 `add` 之后用户改名的唯一落地点。

### 改动清单

| 文件 | 改动 |
|---|---|
| `gd3-core/src/task.rs` | 新增 `output_path_in(output_folder, name)`；`output_path_for(name)` 改为委托；新增 `placeholder_paths()` 与 `placeholder_paths_at(folder, name)` |
| `gd3-core/src/filesystem.rs` | 新增 `pub fn move_path(from, to) -> io::Result<()>` + 私有 `copy_dir_all`；文档表补 `shutil.move` 一行；3 单测 |
| `gd3-core/src/lib.rs` | `filesystem` re-export 补 `move_path` |
| `gd3-ui/src/task_service.rs` | 新增 `UpdateReport` + `pub fn update_placeholders`；`add_with_conflict` 改用 `placeholder_paths_at` / `placeholder_paths`；6 单测（1410 → 1622 行，32 → 38 单测） |
| `rust/README.md` | 「任务记账」补 `update_placeholders`/`UpdateReport` 两条 bullet；filesystem 表补 `move_path`；规模行更新 |

### `update_placeholders` 的执行顺序（逐条对齐 Python）

```text
1. 无登记项                    -> Ok(None)            # Python 是 KeyError
2. current == 登记 paths       -> Ok(Some(空 report)) # 直接 return，不置 pending_flush
3. fs::create_dir_all(outputFolder)                   # mkdir(parents=True, exist_ok=True)
4. resolve_conflict(name, KeepBoth,
       create = |候选| create_placeholders(placeholder_paths_at(folder, 候选), &旧 paths, isOutputFolder),
       is_unfinished = |_| false, delete_recoverably = |_| false)
5. (outputFolder, name) != (登记值) 时：
     oldPaths = placeholder_paths_at(旧目录, 旧名字)
     for (oldPath, newPath) in zip(oldPaths, newPaths):
         if oldPath not in 登记 paths: continue
         for (old, new) in [(oldPath, newPath), (oldPath.ghd, newPath.ghd)]:
             if old.exists(): delete_path(new); move_path(old, new)
6. 登记 paths 里不在 newPaths 的 -> delete_placeholder
7. placeholders.set(...) + 写回 record + pending_flush = true
```

### 关键决策

- **`output_path_in` 而不是「临时改字段再取 property」。** Python 的
  `Task.placeholderPathsAt(outputFolder, name)` 是 `with` 式临时改字段；Rust 侧
  `TaskRecord` 是纯数据，直接传参更省事，也顺带让 `add_with_conflict` 复用同一条路径。
- **复用 `resolve_conflict(KeepBoth, …)` 而不是另写一个改名循环。** Python 这里只有
  `except FileExistsError: index += 1; setName(f"{stem}({index}){ext}")`——既不查
  `_isUnfinishedPlaceholder`，也不进回收站，恰好等于 `KeepBoth` 分支的全部语义
  （`stem`/`ext` 都取自**原始**名字，index 单调递增，不会 `a(1)(1).bin`）。
- **`create` 闭包传 `&existing = 旧登记 paths`。** Python 的 `_createPlaceholders`
  会跳过 `placeholders.paths` 里已登记的路径（`if path in existingPaths: continue`），
  否则「只改目录不改名字」时会把已存在的占位符当成 `FileExistsError` 白白改名。
- **`UpdateReport` 暴露 `created` / `moved` / `removed`。** Python 只改对象、不发信号，
  调用方（卡片重绘 / 日志）无从知道「到底动没动、动了哪些」。三个 `Vec` 让 `Ok(None)`
  与「无变化」这两种情形也能被区分（前者是没登记项，后者是三个 Vec 全空）。
- **`move_path` 抽进 `gd3-core::filesystem`。** `shutil.move` 跨设备会 copy+delete，
  直接 `fs::rename` 在 `/tmp` → `/root`（不同 tmpfs/overlay 层）会 `EXDEV`。回退只在
  `ErrorKind::CrossesDevices` 触发，其余错误原样冒泡（见 doc 里的三条差异：不做
  `except OSError` 全量回退、不保 mtime、符号链接按内容复制）。
- **先 `delete_path(新)` 再 `move_path`。** Python 是 `deletePath(new); move(old, new)`：
  第 4 步刚 `create_new` 出来的 0 字节占位符必须先删掉，否则 `rename` 到已存在文件在
  某些平台会失败；目录型占位符同理。
- **置 `pending_flush = true`。** Python 由调用方 `edit()` / `updateSelection()` 触发的
  `_flushSoon()` 落盘；Rust 侧改名结果必须能被持久化，不能靠调用方记得。
- **`old_paths` 用「当前 record + 旧目录/旧名字」算。** Python 的
  `placeholderPathsAt` 也是拿**当前** task 的其他字段（`isOutputFolder`、`files`）算的，
  只有 `outputFolder` / `name` 被替换——不能拿登记时快照里的旧 record 重算。

### 单测

`gd3-core::filesystem`（3 个）：

| 测试 | 钉住的行为 |
|---|---|
| `move_path_renames_within_one_device` | 同设备 `rename`，源消失、内容不变 |
| `move_path_moves_a_whole_directory` | 目录整体搬走（含子文件） |
| `move_path_reports_missing_source` | 源不存在 → `Err(NotFound)`，不是静默 |

`gd3-ui::task_service`（6 个，全部 hermetic：`TempDir` + `AtomicU32` + `Drop`）：

| 测试 | 钉住的行为 |
|---|---|
| `update_placeholders_returns_none_for_unknown_task` | 空服务 + 有任务但没登记项，都 `Ok(None)` |
| `update_placeholders_is_a_noop_when_paths_match` | 路径没变 → 三个 Vec 全空、**不**置 `pending_flush` |
| `update_placeholders_renames_and_moves_the_file` | `a.bin` → `b.bin`：`created == [b.bin]`、`moved == [(a.bin, b.bin)]`、内容跟着走、登记转给新路径 |
| `update_placeholders_moves_the_ghd_sidecar` | `.ghd` 分片一起搬，`moved` 里两对 |
| `update_placeholders_moves_across_folders` | 只改 `outputFolder`（名字不变）→ 文件搬进新目录、旧目录不留 |
| `update_placeholders_renames_again_on_collision` | 目标名字被别人占着 → `b(1).bin`，占位文件原样不动、不进回收站 |

### 验证

- `cargo fmt --all` + `--check`：通过；
- `cargo check -p gd3-ui --lib --tests`：50s 通过（无 warning）；
- `cargo test -p gd3-ui --lib`：**669 passed**（663 + 6）；
- `cargo test --workspace`：**1170 passed / 0 failed**（28 个 test result 段，
  第 93 步 1161 + 6 + 3）；
- `cargo clippy --workspace --all-targets -- -D warnings`：**0 warning**（1m18s）；
- Python 侧没改一行，不跑 Python 全量；
- 代码 commit `3286fff`。

### 下一步（第 95 步候选，按价值/风险）

1. **`buildFontFamilies`**（`app/platform/desktop.py:15-45`）：`QLocale.Script` × 平台
   字体链，造 `Script` 枚举后可搬，纯函数、无 I/O；
2. **`app/services/runtime_status.py`（195 行）**：需先给 `pack::BinaryRuntime` 补
   `install` / `fetch_latest_version` 的**同步签名**（Python 是 `async` 且要网络）；
3. **`NameConflictQueue` 与 UI 的接线**：`name_conflict.rs` + `add_with_conflict` 都已
   就位，但 `Engine` 还没把 `nameConflicted` 事件接到卡片上；
4. D-Bus / keepalive：仍排最后（`zbus` 在离线缓存里，但编译代价大）。

### 卡点

无。

## 第 95 步：分类匹配与输出目录推导（CategoryService 搬进 category.rs，2026-10-18 13:56）

**目标**：补齐 `app/services/category_service.py`（223 行）里**还没迁**的那一半——
`matchByName` / `categoryOf` / `folderOf` / `outputFolderOf`，以及
`cfg.isCategoryEnabled` / `cfg.downloadFolder` 两个键的读取。第 83 步只搬了 `_load`
（规则表 CRUD），`TaskService.add` 的第一步 `outputFolderOf` 一直是空的。

### 改动清单

| 文件 | 改动 |
|---|---|
| `gd3-ui/src/category.rs` | 650 → **1180 行 / 14 单测**；新增 `path_suffixes` / `normalized_suffixes` / `match_by_name_in` / `match_by_name`、`CategoryOutputSettings`、`OutputFolder`、`category_by_id` / `category_of` / `folder_of` / `output_folder_of`；`CategoryService` 整类从 `engine.rs` 搬来（`from_config` / `new` / `rules` / `set_rules` / `settings` / `set_settings` / `match_by_name` / `category_of` / `folder_of` / `output_folder_of`）+ 7 单测 |
| `gd3-ui/src/file_select.rs` | 814 行 / 9 单测；`path_suffixes` / `normalized_suffixes` 改成 `pub use crate::category::{…}`；`match_by_name` 变薄包装（把 `CategoryInfo` 投影成 `(&str, &[String])` 委托 `match_by_name_in`）；`path_suffixes_matches_cpython_313` 单测搬去 `category.rs` |
| `gd3-ui/src/task_service.rs` | 1622 → **1776 行 / 42 单测**；字段 `categories: CategoryService`（`Default` = 空规则 + 关闭）+ `categories()` / `set_categories()`；`add_with_conflict` 在 duplicate 检查后、`to_safe_filename` 前插 `output_folder_of` 写回；模块 doc「已知差异」补「分类服务是克隆」；4 新单测 + helper `categories(folder)` |
| `gd3-ui/src/engine.rs` | 1189 → **1176 行**；删掉 `CategoryService` 桩，改 `pub use crate::category::CategoryService`；`Engine::new` 里 `tasks.set_categories(category.clone())`；新增 `Engine::push_categories()` |
| `rust/README.md` | 799 → 837 行；新增 `### 分类匹配与输出目录（category.rs）`（映射表 + `OutputFolder` 语义 + 2 条差异 + 规模行）；「任务记账」的已知差异改成「第一步是 `outputFolderOf`」；规模行 `task_service.rs` **1776 行 / 42 单测** |

### `outputFolderOf` 的三条分支（`category_service.py:184-192`）

```text
if not cfg.isCategoryEnabled.value:      # 分支 1：原样返回
    return task.outputFolder, task.category          # 注意 category 也不猜
if categoryId is None:                   # 分支 2：按名字猜
    categoryId = categoryOf(task)
if categoryId and Path(folder) == Path(cfg.downloadFolder.value):   # 分支 3
    folder = Path(folderOf(categoryId) or folder)    # folderOf 为 None 时保持原目录
```

`categoryOf`（:173-176）：`task.files is not None and len(task.files) > 1` → `""`
（多文件任务按目录走，不按名字猜）；否则 `matchByName(task.name)`。

`matchByName`（:157-171）：候选 = `[".".join(suffixes[-2:]), suffixes[-1]]`，
**逐候选遍历全部分类**（分类内按 `extensions` 顺序），命中即返回。所以
`a.tar.gz` 先试 `tar.gz` 再试 `gz`，`a.mp3.mp4` 先试 `mp3.mp4` 再试 `mp4`——
长候选优先是既有行为。

`folderOf`（:178-182）：未知分类 / `folder` 缺失或空串 → `None`；否则
`category.folder.replace("{default}", cfg.downloadFolder.value)`（**全替换**，
不是只换第一个）。

### 关键决策

- **`CategoryService` 从 `engine.rs` 搬进 `category.rs`，`engine` 只 `pub use`。**
  `TaskService` 需要直接持有它；留在 `engine` 会造 `task_service → engine` 的反向依赖。
- **`match_by_name_in<'a, I: IntoIterator<Item = (&'a str, &'a [String])>>` 作共享内核。**
  `category::match_by_name`（规则表）与 `file_select::match_by_name`（`CategoryInfo`
  投影）必须用**同一套候选顺序**，否则两处会漂移（文件选择对话框的分类过滤与
  下载落地的分类判断本来就必须一致）。`file_select` 的 `path_suffixes` /
  `normalized_suffixes` 因此改成 `pub use`，不再各存一份。
- **`OutputFolder { category: Option<String>, folder: PathBuf }` 保留 `None` / `Some("")` 之别。**
  Python 用 `if categoryId is None` 才猜分类、用 `if categoryId` 才换目录，两种空值
  语义不同；压成一个 `Option` 会丢分支（`""` 来自 `categoryOf` 没命中，必须「不猜也不换」）。
- **默认目录判定用 `Path` 相等（逐组件）而非字符串相等。**
  与 Python `Path(...) == Path(...)` 同口径：`/d/`、`/d/.`、`/d//` 都等价；
  **替换出来的目录显式过 `posix_normpath`**（对齐 `Path(str)`），
  **未替换时逐字节保留入参**（避免把 `""` 归一成 `"."` 这类副作用）。
- **`CategoryOutputSettings::from_config` 走 `settings::read_bool` / `read_text`。**
  按 `SPECS` 缺省兜底，与 Python `ConfigItem.value` 的「缺键即缺省」一致；
  不用 `config.get(...)`（那会把缺键当 `None` 再各自写缺省，两处容易漂）。
- **`TaskService` 持有克隆 + `set_categories()` / `Engine::push_categories()`。**
  Python 是同一个实例（改规则立刻可见），Rust 侧必须显式重推；`Default` =
  空规则 + 关闭 → 没接线的调用方行为与第 94 步完全一致（38 个旧单测零改动）。
- **`add_with_conflict` 里先写 `output_folder` 再写 `category`。**
  `folder_string(&self)` 借用 `resolved`，先移动 `category` 会
  `E0382: borrow of partially moved value`。

### 与 Python 的差异

1. **分类服务是克隆**（见上），改规则后要显式 `set_categories`；
2. **前导 `//` 不镜像**：Python `Path("//a/b")` 保留双斜杠（POSIX 特例），
   Rust `posix_normpath` 压成单根——与第 85 步 `path.rs` 记录的差异同源；
3. **`posix_normpath` 只作用于「替换出来的」目录**，未替换时逐字节透传入参。

### 单测（7 个新增，`category.rs` 共 14）

| 测试 | 钉住的行为 |
|---|---|
| `path_suffixes_matches_cpython_313` | 12 条金值（从 `file_select.rs` 搬来），含 `a.` → `[]`、`.bashrc`、`a..b` |
| `match_by_name_prefers_the_longer_suffix_chain` | `a.tar.gz` → archive（先试 `tar.gz`）、`a.mp3.mp4` → video、`a.mp3.gz` → archive |
| `category_of_skips_multi_file_tasks` | `files` 长度 > 1 → `""`；长度 1 → 照猜 |
| `folder_of_replaces_every_default_placeholder` | `{default}` 多次出现全替换；`folder == None` → `None` |
| `output_folder_of_only_rewrites_the_default_folder` | 8 条分支：关闭 / 显式 `""` / 未知 id / 自定义目录 / 尾斜杠 / `/.` / `//` / 多文件 |
| `output_folder_of_normalizes_like_pathlib` | 替换结果过 `posix_normpath`，未替换逐字节透传 |
| `service_reads_both_config_keys` | `from_config` 读 `isCategoryEnabled` / `downloadFolder`，缺键按 `SPECS` 缺省 |

`task_service.rs` 4 个新增（`categories(folder)` helper 造服务）：

| 测试 | 钉住的行为 |
|---|---|
| `add_with_conflict_moves_the_task_into_the_category_folder` | `movie.mkv` + 默认目录 → `cat_video` + `…/Video` |
| `add_with_conflict_keeps_a_custom_folder_but_sets_the_category` | 自定义目录 → 只定分类、目录不动 |
| `add_with_conflict_respects_an_explicit_category` | 显式 `Some("")` → 不猜也不换 |
| `add_with_conflict_leaves_the_folder_alone_when_categories_are_disabled` | 关闭 → `(None, 目录不变)` |

金值全部由 venv 里的 Python 实测（`CategoryService.__new__` + 手填 `_categories`，
模块级 `cfg` 换 `SimpleNamespace`，避免真读配置）。

### 验证

- `cargo check -p gd3-ui --lib --tests`：46s 通过（中途修 `E0425` / `E0382` / `E0063`）；
- `cargo test -p gd3-ui --lib`：**679 passed**；
- `cargo test --workspace`：**1180 passed / 0 failed**（28 个 test result 段，
  第 94 步 1170 + 7 + 4 - 1 搬走的）；
- `cargo clippy --workspace --all-targets -- -D warnings`：**0 warning**（4m04s，
  抓到 1 处 `manual_contains` → 改 `extensions.contains(&candidate)`）；
- `cargo fmt --all` + `--check`：通过；
- Python 侧没改一行，不跑 Python 全量；
- 代码 commit `ac1facd`。

### 踩坑记录

- **第三次踩 `Edit` 的坑**：往 scratch 脚本里「插入注释」时，`PUT n.=n:` 实际
  **覆盖**了下一行 `url: String::new(),`，编译报 `E0063: missing field url`。
  改文件一律「`sed -n` 核对 + Python 字符串替换（带 `assert count == 1`）」。
- **`cargo fmt && … && setsid … &` 会把整条 `&&` 链后台化**（含 `fmt`），
  之后 `--check` 读到的还是未格式化的源码。`fmt` 必须单独前台跑。

### 下一步（第 96 步候选，按价值/风险）

1. **`TaskService.probe_conflict` 接线**（`task_service.py:388-390`）：`name_conflict.rs`
   + `add_with_conflict` 都已就位，`PlaceholderRegistry::is_unfinished` 可查
   「未完成」，缺的是 `Engine::on_name_conflicted` 到卡片的这一段；
2. **`buildFontFamilies`**（`app/platform/desktop.py:15-45`）：造 `Script` / `Territory`
   枚举后可搬，纯函数、无 I/O；
3. **`app/services/runtime_status.py`（195 行）**：需先给 `pack::BinaryRuntime` 补
   `install` / `fetch_latest_version` 的同步签名（Python 是 `async` 且要网络）；
4. D-Bus / keepalive：仍排最后（`zbus` 在离线缓存里，但编译代价大）。

### 卡点

无。

## 第 96 步：probeConflict 接线（冲突队列真的能探测了，2026-10-18 14:31）

**目标**：补上 `TaskStore.probeConflict`（`app/services/task_service.py:126-128`）与
`TaskService.probeConflict`（`:388-390`）。第 65 步就把 `NameConflictQueue` 搬完了、
第 92/93 步占位符也落地了，但 `Engine` 的 `ConflictActions::probe` 一直恒 `None`
——队列因此永远认为「没有冲突」，按 `keepBoth` 放行、不弹窗。这条线是重名冲突
对话框（`name_conflict.rs` + `draft_card.rs` 的冲突图标）唯一的触发源。

### 改动清单

| 文件 | 改动 |
|---|---|
| `gd3-core/src/path.rs` | 210 → **285 行**；新增 `posix_relative_to(path, base) -> Option<String>` + 私有 `split_root`；`relative_to_matches_pathlib` 单测（15 条金值） |
| `gd3-ui/src/task_service.rs` | 1776 → **1945 行 / 47 单测**；新增 `probe_conflict` / `probe_conflict_in` / `probe_conflict_by_id` + 私有 `modified_seconds`；模块 doc「已知差异」补一条；5 个新单测 |
| `gd3-ui/src/engine.rs` | 1176 → 1178 行；`TaskConflictActions::probe` 从恒 `None` 改为 `self.tasks.probe_conflict_by_id(&task.task_id)`；模块 doc / `on_name_conflicted` / `apply_dialog_outcome` 三处文档同步 |
| `rust/README.md` | 「任务记账」补 `probe_conflict` 一条；「引擎装配」的「`probeConflict` 恒 `None`」改写为「已接线」；规模行 `task_service.rs` **1945 行 / 47 单测** |

### 逐条对齐 Python

```python
# TaskStore.probeConflict (task_service.py:126-128)
paths = sorted(folder / path.relative_to(task.outputFolder) for path in task.placeholderPaths)
return next((path for path in paths if isExisting(path) and not self._isUnfinishedPlaceholder(path)), None)

# TaskService.probeConflict (:388-390)
_, folder = self._categoryService.outputFolderOf(task)
return self._store.probeConflict(task, folder)
```

```text
1. folder = categories.output_folder_of(record).folder      # outputFolderOf
2. paths = [ folder / rel | rel = p.relative_to(record.output_folder), p in placeholder_paths ]
3. paths.sort()                                             # Path 逐组件比较
4. 取第一个 fs::metadata 成功、且 placeholders.is_unfinished(p) 为假的路径
5. 返回 ProbeTarget { path, is_folder, existing_size, modified_at }
```

`isExisting` = `Path(path).stat()` 不抛异常（`app/platform/filesystem.py:24`）；
`_isUnfinishedPlaceholder` = 登记表里该路径的**所属任务**存在且 `status != COMPLETED`
（`_taskByPlaceholder`，由 `_setPlaceholders` 逐路径登记）。

### 关键决策

- **`posix_relative_to` 放进 `gd3-core::path`。** 它是 `PurePosixPath.relative_to` 的
  镜像，与 `posix_join` / `posix_normpath` 同一族；`probeConflict` 只是第一个用户
  （`file_select` / 未来的路径展示都会用）。
- **`split_root` 单独抽出来，返回 `("/" | "//" | "", parts)`。** Python 的根是第一段：
  `Path("//a").parts == ('//', 'a')`，与 `/a` 的 `'/'` **不相等**，所以
  `Path("//a/b").relative_to("/a")` 抛 `ValueError`、`relative_to("//a")` 得 `b`。
  把「压斜杠」和「比根」混在一起就会把这两种情况判成同一件事（`posix_normpath`
  压成单根是另一回事，不能复用）。
- **`Option<String>` 而不是 `Result`。** Python 的 `ValueError` 在 Rust 里没有调用方
  会处理：占位符路径都是 `posix_join(output_folder, name)` 拼出来的，`None` 只可能
  出现在「调用方手写了不一致的 record」这种病态输入上——`filter_map` 直接跳过比
  引入错误路径更省事（doc 里写明）。
- **`relative == "."` 单独处理。** `pathlib` 里 `Path(folder) / "."` 就是
  `Path(folder)`，而 `posix_join(folder, ".")` 会拼出 `folder/.`（`metadata` 能读，
  但返回的 `path` 字符串与 Python 不同）。`posix_join` 的入参约定是「任务名」，
  所以在这个调用点补分支，不动 `posix_join`。
- **`paths.sort()` 用 `PathBuf` 的 `Ord`（逐组件）。** CPython 的 `PurePath.__lt__`
  比的是 `_cparts`，实测 `Path("/a/b") < Path("/a.b")` 为 `True`——字符串比较会
  给出相反结论（`'.'` < `'/'`）。虽然当前 `placeholder_paths()` 只返回一个路径
  （第 92 步的已知差异：不遍历 `steps`），语义仍按 Python 写。
- **四列 `stat()` 用同一次 `fs::metadata`。** Python 是 `isExisting` 一次、队列
  `takenPath.stat()` 再一次，中间文件被删会 `OSError`；Rust 侧一次拿全，顺带
  消掉这个竞态（记在模块 doc 的「已知差异」）。
- **`probe_conflict_by_id` 单独开一个方法。** Python 的队列持有 `Task` 对象，
  Rust 的 `ConflictTask` 只有 `task_id` / `file_size`——「同一实例」差异的又一例
  （同第 95 步的分类服务）。查不到任务 → `None`（Python 那边 `Task` 一定在
  `_tasks` 里，不存在这条路径）。
- **`modified_seconds` 退回 `0.0`。** `metadata` 已经成功时 `modified()` 几乎不可能
  失败；Python 那边是 `AttributeError`，不值得为它加错误类型。

### 单测

`gd3-core::path`（1 个）：

| 测试 | 钉住的行为 |
|---|---|
| `relative_to_matches_pathlib` | 15 条金值：`/a/b`→`b`、自反→`.`、`///a/b`→`b`、`//a/b`/`//a`→`b`，以及 5 种 `ValueError`（`/a`←`/a/b`、`//a/b`←`/a`、`/a/b`←`//a`、`a`←`b`、`/a/b`←``） |

`gd3-ui::task_service`（5 个，全部 hermetic）：

| 测试 | 钉住的行为 |
|---|---|
| `probe_conflict_returns_none_when_nothing_exists` | 文件不存在 → `None`；任务不在列表里 → `probe_conflict_by_id` 也是 `None` |
| `probe_conflict_reports_the_existing_file` | 5 字节文件 → `path` 精确、`!is_folder`、`existing_size == 5`、`modified_at > 0` |
| `probe_conflict_reports_a_folder_placeholder` | 目录型占位符（多文件任务）→ `is_folder == true` |
| `probe_conflict_skips_unfinished_placeholders` | `add_with_conflict` 建出的 0 字节占位符：`RUNNING` → `None`；改成 `COMPLETED` → `Some(existing_size == 0)` |
| `probe_conflict_looks_inside_the_category_folder` | 分类开启 + `movie.mkv` → 探测的是 `…/Video/movie.mkv`；显式 `probe_conflict_in(dir)` 在默认目录下 → `None` |

Python 侧实测（`TaskStore(lambda p: False)` + `_setPlaceholders`，不碰真配置）：

```text
placeholderPaths: ['/tmp/…/a.bin']
1 文件不存在                        -> None
2 文件存在 + status WAITING          -> None      # 未完成 → 自己人
3 文件存在 + status COMPLETED        -> /tmp/…/a.bin  size=5
4 换一个目录（folder=…/sub）         -> /tmp/…/sub/a.bin
5 目录型占位符 + 已登记且未完成       -> None
```

第 5 条第一次跑出 `None` 是因为探针把任务登记进了 `_taskByPlaceholder`（默认
`WAITING`）——Rust 单测里那个服务没有登记项，对应「登记表为空」的情形，两边不矛盾。

### 验证

- `cargo fmt --all` + `--check`：通过；
- `cargo check -p gd3-ui --lib --tests`：55s 通过；
- `cargo test -p gd3-core --lib path::`：6 passed；
- `cargo test -p gd3-ui --lib`：**684 passed**（679 + 5）；
- `cargo test --workspace`：**1186 passed / 0 failed**（28 段，第 95 步 1180 + 5 + 1）；
- `cargo clippy --workspace --all-targets -- -D warnings`：**0 warning**（2m32s）；
- Python 侧没改一行，不跑 Python 全量；
- 代码 commit `4438b6c`。

### 下一步（第 97 步候选，按价值/风险）

1. **`TaskService` 的 `startSeeding` / `_seed` 记账半边**（`task_service.py:395-420`）：
   做种开关 + `isSeeding` 状态 + `seedingStateChanged`，libtorrent 本体仍是桩；
2. **`buildFontFamilies`**（`app/platform/desktop.py:15-45`）：造 `Script` / `Territory`
   枚举后可搬，纯函数、无 I/O；
3. **`app/services/runtime_status.py`（195 行）**：需先给 `pack::BinaryRuntime` 补
   `install` / `fetch_latest_version` 的同步签名（Python 是 `async` 且要网络）；
4. D-Bus / keepalive：仍排最后（`zbus` 在离线缓存里，但编译代价大）。

### 卡点

无。

## 第 97 步：TaskService 做种记账半边（2026-10-18 15:30）

**目标**：补上 `TaskService` 的做种（seeding）记账半边——`startSeeding` / `stopSeeding` /
`_seed` / `_onSeedingDone` / `_onSeedingEnded` / `_cancelWork` 的 workId 兜底 /
`_refreshSpeedMeter` 的判定 / `_onRunDone` 尾部的自动做种 / `resumeSaved` 的做种分支
（`app/services/task_service.py:395-420`、`:585-604`、`:629-639`、`:659-673`、`:529-531`）。
`task.runSeeding()` 本体（libtorrent）与 `_watchFile` 仍归引擎侧——本步只做**记账**：
谁在做种、workId 是谁、`shouldSeed` 落不落盘、速度表该不该跑。

### 改动清单

| 文件 | 改动 |
|---|---|
| `gd3-ui/src/task_service.rs` | 1945 → **2356 行 / 47 → 56 单测**；新增 `SeedingEvent` / `CancelWork` 两个类型、`seeding: BTreeMap<String, String>` 字段、11 个方法（`start_seeding` / `stop_seeding` / `seed` / `on_seeding_done` / `on_seeding_ended` / `is_seeding` / `seeding_work_id` / `seeding_count` / `speed_meter_should_run` / `should_start_seeding` / `resume_seeding_candidates`）；`cancel_work` 返回值从 `()` 改成 `CancelWork`；`refresh()` 不再一律 `CardRuntime::default()`（填真实 `isSeeding` / `canSeed` / `hasOutputFile`）；9 个新单测 |
| `rust/README.md` | 837 → **865 行**；「任务记账」补「做种记账（第 97 步）」一节（含 `or` 短路缺陷说明）；规模行改 **2356 行 / 56 单测** |

### 逐条对齐 Python

```python
# startSeeding (task_service.py:397-400)
task.shouldSeed = True; self._flushSoon(); self._seed(task, isManual=True)

# stopSeeding (:402-406)
task.shouldSeed = False; self._flushSoon(); self._cancelWork(task); self._refreshSpeedMeter()

# _seed (:585-593)
task.isSeeding = True
self._seeding[task.taskId] = self._coroutineRunner.submit(task.runSeeding(isManual),
                                                          done=_onSeedingDone, failed=_onSeedingEnded)
self.seedingStarted.emit(task); self._refreshSpeedMeter()

# _onSeedingDone (:595-597) -> _onSeedingEnded (:599-604)
task.shouldSeed = False
self._seeding.pop(task.taskId, None); task.isSeeding = False
self._flushSoon(); self.seedingStopped.emit(task); self._refreshSpeedMeter()

# _cancelWork (:629-639)
workId = self._queue.workIdOf(task.taskId) or self._seeding.pop(task.taskId, None)
self._queue.cancel(task.taskId)
if task.isSeeding: task.isSeeding = False; self.seedingStopped.emit(task)
if workId is not None: self._coroutineRunner.cancel(workId, finished=finished)

# _refreshSpeedMeter (:659-663)
if self._queue.runningCount() or self._seeding: start() else: stop()

# _onRunDone 尾部 (:672-673)
if task.status == COMPLETED and task.canSeed and task.shouldSeed: self._seed(task)

# resumeSaved (:529-531)
if task.status == COMPLETED and task.hasOutputFile and isExisting(task.outputPath):
    self._watchFile(task)
    if task.canSeed and task.shouldSeed: self._seed(task)
```

### 关键决策

- **`seeding` 一个 map 顶两个 Python 字段。** `_seeding`（dict）与 `Task.isSeeding`
  （bool）在 Python 里是同生同死的：`_seed` 同时置两者，`_onSeedingEnded` 同时清两者。
  唯一的例外是 `_cancelWork` 的 `or` 短路（见下），那里两者会分叉——Rust 侧用
  「`_seeding` 里有这个 id」当 `isSeeding`，分叉的那一半靠 `CancelWork::stopped_seeding`
  在入口处快照，行为与 Python 一致。
- **`_cancelWork` 的 `or` 短路缺陷照抄，不修。** 队列里有 workId 时
  `self._seeding.pop(...)` 根本不会执行：`isSeeding` 被清成 `False`，但 `_seeding`
  里那条留下 → `_refreshSpeedMeter` 永远为真（速度表一直跑）。Python 实测见下，
  Rust 单测 `cancel_work_prefers_the_queue_work_id` 钉住。**改行为会与 Python 漂移**，
  这是迁移不是修 bug。
- **`should_seed` 是持久化字段，`is_seeding` 不是。** 所以 `start_seeding` /
  `stop_seeding` / `on_seeding_ended` 置 `pending_flush`，`seed` 不置——对应
  Python 的 `_flushSoon()` 出现位置（`_seed` 里没有）。
- **`_flushSoon()` 的 200ms 防抖仍是 `pending_flush` 标记**（第 81 步的既定退化）。
- **`_seed` 不校验 `canSeed` / `shouldSeed`**：Python 也不校验，判定在调用点
  （`should_start_seeding` / `resume_seeding_candidates`）。
- **`on_seeding_ended` 返回 `SeedingEvent` 而不是 `Option`**：Python 无条件 `emit`
  （`pop(taskId, None)` 带缺省），哪怕本来没在做种。
- **`resume_seeding_candidates()` 返回 id 列表而不是就地 `_seed`**：workId 只有引擎
  能给；`_watchFile` 也归引擎。`hasOutputFile` 恒真（`facts::task_kind`），保留是为了
  与 Python 的形状对齐。
- **`refresh()` 顺带接上三个运行时事实**：`isSeeding` 来自本步的记账，`canSeed` /
  `hasOutputFile` 来自 `facts::task_kind`（按 `type` 反查类属性）。`canPause`（要读
  步骤状态）、`completedAtText`（要时区）、`outputExists`（要 stat）、`lastError`
  仍留给调用方——`push_task_records` 的注入点就是为它们留的。

### Python 侧实测（`TaskService.__new__` + 假 runner/meter/watcher，不碰真配置）

```text
1 startSeeding : shouldSeed=True  isSeeding=True  seeding={'t1': 'w2'} events=[started]        meter=True  flush=w1
2 stopSeeding  : shouldSeed=False isSeeding=False seeding={}          events=[started,stopped] meter=False
3 _cancelWork（队列有 workId）
               : isSeeding=False  seeding={'t1': 'w1'}（残留！）        events=[started,stopped] meter=True  cancelled=['run-1']
3b _cancelWork（队列没有 workId）
               : isSeeding=False  seeding={}                          events=[started,stopped] cancelled=['w1']
4 _onSeedingDone: shouldSeed=False isSeeding=False seeding={}          events=[started,stopped] meter=False
5 _onRunDone COMPLETED + shouldSeed=True  -> isSeeding=True  seeding={'t1':'w2'} events=[completed,started,allCompleted]
5 _onRunDone COMPLETED + shouldSeed=False -> isSeeding=False seeding={}            events=[completed,allCompleted]
5 _onRunDone FAILED    + shouldSeed=True  -> isSeeding=False seeding={}            events=[completed,allCompleted]
6 meter: 空闲 False -> 队列有任务 True -> done 后 False -> 只做种 True
```

第 3 条是 `or` 短路的直接证据（`seeding` 没被摘、`meter` 还在跑）；第 6 条对应
`speed_meter_should_run`。

### 单测（9 个，全部 hermetic）

| 测试 | 钉住的行为 |
|---|---|
| `start_seeding_marks_should_seed_and_registers_the_work` | `shouldSeed=True` + 置脏 + `_seeding` 登记 + `seedingStarted` + 速度表该跑；任务不在 → `None` 且不登记 |
| `seed_requires_the_task_to_exist` | `seed` 只登记、不看 `canSeed`/`shouldSeed`；任务不在 → `None` |
| `stop_seeding_clears_should_seed_and_returns_the_work` | `shouldSeed=False` + 置脏 + `work_id` 从 `_seeding` 兜底 + `stopped_seeding` + 速度表停 |
| `on_seeding_ended_flushes_and_clears` | `_onSeedingEnded` 摘 `_seeding` + 置脏 + 幂等；`_onSeedingDone` 先清 `shouldSeed` |
| `cancel_work_prefers_the_queue_work_id` | 队列 workId 优先 + `stopped_seeding` 取入口值 + **`_seeding` 残留、速度表仍跑**（照抄缺陷） |
| `speed_meter_runs_while_seeding` | `runningCount()>0` 或 `_seeding` 非空 → 跑；两者都空 → 停 |
| `should_start_seeding_requires_completed_and_seedable_type` | 三种否决：`shouldSeed` 假、状态不是 `COMPLETED`、`type_name` 不可做种 |
| `resume_seeding_candidates_needs_an_existing_output_file` | 四种记录里只挑「已完成 + 可做种 + `shouldSeed` + 文件在」 |
| `refresh_reflects_the_seeding_state` | 做种前 `toggleIcon == "PLAY"`、`toggleEnabled` 为真（`BTTask.canSeed`）；`seed` 后 `"PAUSE"` |

`record()` 之外加了一个 `seedable(task_id, status)` 帮手（`type_name =
"features.bittorrent_pack.task.BTTask"`）；用 `from_records` 而不是 `add`，因为
Python 的 `add` 会强制 `shouldSeed = True`，那样就测不出 `startSeeding` 自己那一句。

### 验证

- `cargo fmt --all` + `--check`：通过；
- `cargo check -p gd3-ui --lib --tests`：50s 通过；
- `cargo test -p gd3-ui --lib`：**693 passed / 0 failed**（684 + 9）；
- `cargo test --workspace`：**1195 passed / 0 failed**（28 段，第 96 步 1186 + 9）；
- `cargo clippy --workspace --all-targets -- -D warnings`：**0 warning**（5m19s）；
- Python 侧没改一行，不跑 Python 全量（探针脚本在 scratch，未入库）。

### 下一步（第 98 步候选，按价值/风险）

1. **`actions::TaskService` 的接线**：`start_seeding`/`stop_seeding` 已经有真实实现，
   但 `engine.rs` 还没 `impl actions::TaskService`——现在整条卡片动作链（pause/start/
   delete/redownload/…）都还没落到 `TaskService` 上，这是「引擎装配」的下一块硬骨头；
2. **`buildFontFamilies`**（`app/platform/desktop.py:15-45`）：造 `Script`/`Territory`
   枚举后可搬，纯函数；
3. **`startChecksum` 记账半边**：`_checksums` + `checksumProgress` + 三个信号；
4. `app/services/runtime_status.py`（195 行）：需先给 `pack::BinaryRuntime` 补
   `install`/`fetch_latest_version` 的同步签名；
5. D-Bus / keepalive：仍排最后（`zbus` 在离线缓存里，但编译代价大）。

### 卡点

无。

## 第 98 步：TaskService 校验记账半边 + _rebalance（2026-10-18 16:13）

**目标**：补上 `TaskService` 的校验（checksum）记账半边与 `_rebalance`——
`startChecksum` / `cancelChecksum` / `checksumProgress` / `_onChecksumDone` /
`_onChecksumFailed`（`app/services/task_service.py:408-427`、`:617-626`），以及
`cfg.maxTaskNum.valueChanged` 直连的 `_rebalance`（`:639-647`）。`toChecksum` 本体
（手写 SHA-256/512）已在 `gd3-core::checksum`（第 88 步），协程提交归引擎侧——本步只做
**记账**：谁在算、workId 是谁、进度多少、算完了往哪写、超限时该摘谁。

### 改动清单

| 文件 | 改动 |
|---|---|
| `gd3-ui/src/task_service.rs` | 2356 → **2717 行 / 56 → 65 单测**；新增 `ChecksumWork` / `ChecksumEvent` / `RebalanceReport` 三个类型、`checksums: BTreeMap<String, ChecksumWork>` 字段、9 个方法（`checksum_progress` / `checksum_work` / `checksum_count` / `start_checksum` / `set_checksum_progress` / `cancel_checksum` / `on_checksum_done` / `on_checksum_failed` / `rebalance`）；模块 doc 新增「校验也只做记账」一条；imports 加 `TaskError` / `serde_json::Value`；9 个新单测 |
| `rust/README.md` | 865 → **888 行**；「任务记账」补「校验记账 + `_rebalance`（第 98 步）」一节（含 `del` → `None`、孤儿进度两条差异）；规模行改 **2717 行 / 65 单测** |

### 逐条对齐 Python

```python
# checksumProgress (task_service.py:408-410)
work = self._checksums.get(task.taskId); return work.progress if work else None

# startChecksum (:412-420)
work = ChecksumWork()                      # workId="" progress=0
self._checksums[task.taskId] = work
work.workId = self._coroutineRunner.submit(
    toChecksum(Path(task.outputPath), algorithm, lambda p: setattr(work, "progress", p)),
    done=lambda c: self._onChecksumDone(task, algorithm, c),
    failed=lambda e: self._onChecksumFailed(task, e))
self.checksumStarted.emit(task)

# cancelChecksum (:422-427)
work = self._checksums.pop(task.taskId, None)
if work is None: return
self._coroutineRunner.cancel(work.workId)
self.checksumStopped.emit(task, None)

# _onChecksumDone (:617-621)
del self._checksums[task.taskId]           # 缺项 -> KeyError
task.checksums[algorithm] = checksum
self._flushSoon()
self.checksumCompleted.emit(task, algorithm)

# _onChecksumFailed (:623-626)
del self._checksums[task.taskId]
self.checksumStopped.emit(task, TaskError("无法读取文件：{detail}", detail=str(error)))

# _rebalance (:639-647)
for taskId in self._queue.runningIds()[cfg.maxTaskNum.value:]:
    task = self._store.taskById(taskId)
    if task is not None and task.canPause:
        self._cancelWork(task)
        task.setStatus(TaskStatus.WAITING)
        self._queue.wait(taskId)
self._pump()
```

Rust 侧的对应：

| Python | Rust | 差异 |
|---|---|---|
| `_checksums[taskId] = work` + `checksumStarted.emit` | `start_checksum(id, work_id) -> Option<ChecksumEvent>` | 任务不在列表 → `None`（Python 拿的一定是活对象） |
| `setattr(work, "progress", p)` | `set_checksum_progress(id, p) -> bool` | 只写登记表里的那条；Python 写的是闭包捕获的孤儿对象 |
| `_checksums.pop(..., None)` + `cancel` + `emit` | `cancel_checksum(id) -> Option<String>` | `Some(workId)` = 调用方还要发 `checksumStopped(task, None)` |
| `del _checksums[taskId]` + 写 `checksums` + `_flushSoon` + `emit` | `on_checksum_done(id, algorithm, checksum) -> Option<ChecksumEvent::Completed>` | 缺项退化成 `None` 而非 `KeyError` |
| `del` + `emit(TaskError(...))` | `on_checksum_failed(id, detail) -> ChecksumEvent::Stopped { error }` | 返回值不是 `Option`（Python 无条件 `emit`） |
| `runningIds()[max:]` 循环 | `rebalance(max_task_num) -> RebalanceReport` | 无 |

`TaskError::new("无法读取文件：{detail}", [("detail", detail)])` 与
`app/i18n.py:18` 的 `TaskErrors` 文案逐字一致（占位符留给渲染层）。

### 关键决策

- **`start_checksum` 不接收 `algorithm`**：`ChecksumWork` 只有 `workId` / `progress`
  两个字段（Python 同），算法只用于引擎构造 `toChecksum` 与 `_onChecksumDone` 的写回，
  落账无意义。workId 由引擎提交协程后回填——Rust 侧改成入参，因为「提交协程」在引擎。
- **`on_checksum_done` 用 `self.checksums.remove(task_id)?` 作守卫**：Python 的 `del`
  在缺项时抛 `KeyError`，Rust 退化成 `None`（不写 `checksums`、不置脏、不发信号）。
  首跑 `on_checksum_done_is_idempotent_and_needs_the_task` 红灯正是漏了这一步——
  当时只判任务存在、没判登记项存在，第二次调用会把摘要覆盖掉。
- **`on_checksum_failed` 返回 `ChecksumEvent` 而不是 `Option`**：Python 的 `del` 之后
  无条件 `emit`，任务已从 store 消失也照发（它拿的是活对象，`del` 只碰 `_checksums`）。
- **`set_checksum_progress` 返回 `bool`**：Python 的 `onProgress` 闭包持有 `work`
  对象，`cancelChecksum` 之后迟到的进度会写进孤儿对象（界面上看不见，也不报错）；
  Rust 只写登记表里的那条，返回是否真写了——语义等价（都对界面不可见），但不会
  制造无主对象。记入模块 doc「已知差异」。
- **`cancel_checksum` 返回 `Option<String>`（workId）而不是 `CancelWork`**：这里没有
  「做种」那半边，只有协程要取消；`Some` 同时编码了「要发 `checksumStopped(task, None)`」。
- **`rebalance` 不置 `pending_flush`**：Python 的 `task.setStatus(WAITING)` 不落盘，
  要等下一次真实改动才写。`resume_saved` 同样处理（第 82 步的既有约定）。
- **`canPause` 走 `facts::can_pause`**：不可暂停的任务原样留在运行中（Python 同样
  `continue`），`running_count()` 可能仍大于上限——这是照抄，不是缺陷。
- **`RebalanceReport.cancelled` 用 `Vec<(String, CancelWork)>`**：取消与「放回等待」
  是一对，拆成并列两个 vec 会丢失对应关系；`requeued` 保留一份纯 id 列表给调用方
  批量发信号，`started` 单独放 `_pump()` 的结果（对应 `runStarted`）。
- **`rebalance` 的溢出切片先 `collect()`**：`self.queue.running_ids()` 借 `self.queue`，
  循环体里要 `&mut self` 调 `cancel_work` / `task_mut` / `queue.wait`。

### Python 实测（`TaskService.__new__` + 假 runner/store，脚本在 scratch `probe98.py`）

```text
1 startChecksum: workId=w1 progress=0 events=[('started', 't1')] path=/tmp/gd3-probe/out.bin alg=sha256
  checksumProgress=0  absent=None
2 progress    : progress=42 checksumProgress=42
3 onDone      : checksums={'sha256': 'abc123'} work={} events=[('started','t1'),('completed','t1','sha256')] flush=w2
4 onDone twice: KeyError('t1')
5 cancel      : work={} events=[('started','t1'),('stopped','t1',None,None)] cancelled=['w1']
5b cancel twice: events=[('started','t1'),('stopped','t1',None,None)] cancelled=['w1']
6 onFailed    : work={} events=[('started','t1'),('stopped','t1','无法读取文件：boom',{'detail':'boom'})]
7 rebalance   : status=[COMPLETED, COMPLETED, WAITING] waiting=['c'] running=['a','b'] cancelled=['wc'] events=[]
8 rebalance   : status=[COMPLETED, COMPLETED] waiting=[] running=['a','b'] cancelled=[] events=[]
9 rebalance   : waiting=[] running=['a','b'] events=[('runStarted','b')]
```

要点：第 3 条证明 `startChecksum` 自己**不置脏**（`flush` 仍是 `startChecksum` 之前
留下的 `w2`），脏标记来自 `_onChecksumDone` 里的 `_flushSoon()`；第 4 条是 `del` 的
`KeyError` 现场；第 5b 条证明第二次 `cancelChecksum` 直接 `return`（不再取消协程、
不再发信号）；第 7/8 条证明 `canPause` 为假时原样跳过、上限够时不摘；第 9 条证明
`_rebalance` 末尾的 `_pump()` 会把等待队列里的任务派发出去。

### 单测（9 个，全部 hermetic）

| 测试 | 钉住的行为 |
|---|---|
| `start_checksum_registers_the_work` | `workId` + `progress=0` 登记 + `Started`；任务不在 → `None` 且不登记 |
| `set_checksum_progress_updates_the_registered_work` | 进度写进登记项；`cancel_checksum` 之后迟到的进度返回 `false` |
| `cancel_checksum_returns_the_work_id_once` | 首次返回 `Some(workId)` + 摘项；第二次 `None`（不取消协程、不发信号） |
| `on_checksum_done_records_the_digest_and_flushes` | `startChecksum` 不置脏 → `onDone` 写 `checksums` + 置脏 + 摘项 |
| `on_checksum_done_is_idempotent_and_needs_the_task` | 第二次 `None`（Python `KeyError`）；任务不在 → `None` |
| `on_checksum_failed_reports_the_task_error` | `TaskError` 文案 + 不置脏 + 不写 `checksums`；任务不在也发信号 |
| `rebalance_cancels_tasks_beyond_the_limit` | 超限的运行中任务被摘（`CancelWork` 带 workId）+ 状态回 `WAITING` + 进等待队列；到上限不派发 |
| `rebalance_skips_tasks_that_cannot_pause` | 塞一个 `FFmpegStep` + `Running` 的步骤 → `canPause` 假 → 不动 |
| `rebalance_pumps_after_the_limit_grows` | 上限从 1 涨到 2 → 等待队列里的任务被派发 |

### 验证

- `cargo fmt --all` + `--check`：通过；
- `cargo test -p gd3-ui --lib`：**702 passed / 0 failed**（693 + 9）；
- `cargo test --workspace`：**1204 passed / 0 failed**（28 段，第 97 步 1195 + 9）；
- `cargo clippy --workspace --all-targets -- -D warnings`：**0 warning**；
- Python 侧没改一行，不跑 Python 全量（探针脚本在 scratch，未入库）。

### 下一步（第 99 步候选，按价值/风险）

1. **`actions::TaskService` 的接线**：`engine.rs` 还没 `impl actions::TaskService`
   （`pause` / `start` / `start_seeding` / `stop_seeding` / `delete` / `redownload` /
   `move_to_front` / `set_category` / `start_checksum` / `cancel_checksum`）——
   整条卡片动作链都还没落到 `TaskService` 上，是「引擎装配」的下一块硬骨头。
   trait 收 `&self`、`TaskService` 要 `&mut self`，需要
   `impl actions::TaskService for RefCell<TaskService>` 之类的适配器；
2. `delete` / `redownload` / `edit` / `set_category` / `update_selection` /
   `start_all` / `pause_all` 的记账半边；
3. **`buildFontFamilies`**（`app/platform/desktop.py:15-45`）：造 `Script`/`Territory`
   枚举后可搬，纯函数；
4. `app/services/runtime_status.py`（195 行）：需先给 `pack::BinaryRuntime` 补
   `install`/`fetch_latest_version` 的同步签名；
5. D-Bus / keepalive：仍排最后（`zbus` 在离线缓存里，但编译代价大）。

### 卡点

无。

## 第 99 步：TaskService 删除/重下/分类/批量记账 + pause 补 _pump()（2026-10-18 17:09）

**目标**：把 `TaskService` 剩下五个「卡片动作」的记账半边补齐——`pause`（补尾部
`_pump()`）、`delete`、`redownload`、`setCategory`、`startAll` / `pauseAll`
（`app/services/task_service.py:429-480`、`:530-539`、`:606-615`）。文件删除
（`Task.deleteFiles`）、文件监听器（`_unwatchFile`）、协程取消仍归引擎层——本步只做
**记账 + 顺序**：谁被摘掉、谁的校验协程要取消、能不能删、删完之后让出的空位派发给谁。

### 改动清单

| 文件 | 改动 |
|---|---|
| `gd3-ui/src/task_service.rs` | 2717 → **3122 行 / 65 → 72 单测**；新增 `PauseReport` / `DeleteReport` / `RedownloadReport` 三个类型、模块级私有 `can_delete_in`（`#[cfg]` 双实现）、6 个方法（`pause` 改签名 + `delete` / `redownload` / `set_category` / `start_all` / `pause_all`）；模块 doc 改写「为什么还没做 `actions::TaskService` 的 `impl`」一段 + 「删除/重下也只做记账」一条；7 个新单测 + 1 个旧单测改断言 |
| `rust/README.md` | 888 → **909 行**；「任务记账」补「删除 / 重下 / 分类 / 批量（第 99 步）」一节；规模行改 **3122 行 / 72 单测** |

### 逐条对齐 Python

```python
# pause (task_service.py:429-436)
self._cancelWork(task); task.setStatus(TaskStatus.PAUSED); self._flushSoon()
self.taskPaused.emit(task); self._pump()          # ← 尾部 pump 不能省

# delete (:438-452)
self.cancelChecksum(task); self._unwatchFile(task)
canDelete = shouldDeleteFiles and self._canDeleteIn(task.outputFolder)
if shouldDeleteFiles and not canDelete and not self._hasNotifiedDeleteDenied:
    self._hasNotifiedDeleteDenied = True; self.fileDeleteDenied.emit()
def onStopped():
    if canDelete: task.deleteFiles()
    task.deletePlaceholders()
self._cancelWork(task, finished=onStopped)
self._store.remove(task.taskId); self._flushSoon()
self.taskRemoved.emit(task.taskId); self._pump()

# redownload (:454-463)
self.cancelChecksum(task); self._unwatchFile(task)
def onStopped():
    task.deleteFiles(); task.reset(); self._flushSoon(); self._schedule(task)
self._cancelWork(task, finished=onStopped)

# setCategory (:478-480)
task.category = categoryId; self._flushSoon()     # 不重算 outputFolder、不动占位符

# startAll (:530-534) / _schedule (:569-571)
for task in tasks:
    if status in {PAUSED, WAITING, FAILED}: self._schedule(task)   # wait + pump
# pauseAll (:536-539)
for task in list(tasks.values()):
    if _queue.isRunning(id) or _queue.isWaiting(id): self.pause(task)

# _canDeleteIn (:606-615)
if sys.platform != "darwin": return True
try: next(folder.iterdir(), None); return True
except PermissionError: return False
except OSError: return True
```

Rust 侧的对应：

| Python | Rust | 差异 |
|---|---|---|
| `pause` + 尾部 `_pump()` | `pause(id, max_task_num) -> Option<PauseReport>` | 签名从 `-> bool` 改了；`None` = 任务不在（Python `AttributeError`） |
| `delete` 的 `onStopped` / `_store.remove` / `taskRemoved` | `delete(id, should_delete_files, max_task_num) -> Option<DeleteReport>` | 返回值带 `removed`（`taskRemoved` 的载荷）、`delete_files`、`delete_denied`、`checksum_work`、`started` |
| `_canDeleteIn(folder)` | 模块级 `can_delete_in(&Path)` | 非 macOS 恒 `true`（`#[cfg]` 两条实现），macOS 才 `read_dir` 判 `PermissionDenied` |
| `redownload` 的 `onStopped` | `redownload(id, max_task_num) -> Option<RedownloadReport>` | `reset()` + `queue.wait` + `pump` 同步做完（见决策） |
| `setCategory` | `set_category(id, &str) -> bool` | `""` 落成 `Some("")` 而不是 `None` |
| `startAll` 逐任务 `_schedule` | `start_all(max_task_num) -> Vec<String>` | 同序聚合 `started`；**不改状态** |
| `pauseAll` | `pause_all(max_task_num) -> Vec<String>` | 先快照 id 再遍历；`started` 聚合各次 `pause` 的结果 |

### 关键决策

- **`pause` 的返回类型从 `bool` 改成 `Option<PauseReport>`**：尾部 `_pump()` 是
  Python 语义的一部分（暂停让出空位要立刻补位），漏了就得让每个调用方自己 pump；
  只有 1 个旧单测受影响，改动成本低。
- **`delete` 就地删占位符文件、已下载内容留给调用方**：`delete_placeholder` 只删
  「空目录 / 零字节文件」，不会碰真数据；`Task.deleteFiles`（截断/清空）在 Python 里
  放在 `finished` 回调里等协程真的停下，Rust 侧对应 `DeleteReport::delete_files` 标志。
- **`can_delete_in` 用 `#[cfg]` 双实现**：非 macOS 恒 `true`（Python
  `sys.platform != "darwin"` 直接返回 `True`），macOS 才 `fs::read_dir` 判
  `PermissionDenied`（`next(iterdir())` 只在打开目录时可能抛）。本机 Linux 上
  `delete_denied` 永远为 `false`，无法单测，只在文档里说明。
- **`redownload` 同步做完 `reset()` + `queue.wait` + `pump`**：Python 把这段放在
  `finished` 回调里，但 `_pump()` 只**返回**该派发的 id，真正派发归调用方；
  调用方拿到 `started` 后、提交协程前删旧文件即可，顺序等价。
- **`start_all` 不改状态**：`_dispatch` 才 `setStatus(RUNNING)`；Rust 的 `pump` 本来就
  不碰状态（第 81 步的约定），排队期间保持 `PAUSED`/`FAILED`。
- **`set_category("")` 落成 `Some("")` 而非 `None`**：`outputFolderOf` 用
  `categoryId is None` 判「要不要按名字猜分类」，两种空值语义不同。
- **`pause_all` 先快照 id 再遍历**：`pause` 里的 `_pump` 会改动队列，边迭代边改会
  漏/重；返回的 `started` 会包含「被 pump 派发又立刻被 pause」的中间任务（Python 同，
  见实测第 8 条）。
- **`DeleteReport` 不 `derive(PartialEq)`**：`removed` 是 `TaskRecord`，`gd3-core`
  侧没给它 `PartialEq`。首跑 `E0369`/`E0277` 就是这个（`#[derive(Debug, Clone)]`）。
- **`Task.deletePlaceholders` 的第一段仍镜像不了**：未完成步骤的 `step.outputPath`
  在基类恒为 `""`，真实路径来自 `HttpStep`/`yt_dlp`/`bittorrent` 等**步骤子类**，
  Rust 的 `TaskStep` 只是数据行。与第 93 步 `placeholder_paths()` 只返回
  `[outputPath]` 是同一个缺口，记入模块 doc 与 README。

### Python 实测（`TaskService.__new__` + 假 store/queue/runner，脚本在 scratch `probe99.py`）

```text
1 pause       : status=[PAUSED, RUNNING] events=[('paused','a'), ('runStarted','b')] waiting=[] running=['b']
2 delete(True) : log=['deleteFiles','deletePlaceholders'] events=[('removed','a'), ('runStarted','b')] store=['b'] running=['b']
3 delete(False): log=['deletePlaceholders'] events=[('removed','a')] store=[]
4 denied twice: log(a)=['deletePlaceholders'] log(b)=['deletePlaceholders'] events=[('deleteDenied',), ('removed','a'), ('removed','b')] notified=True
5 redownload  : log=['deleteFiles','reset'] status=RUNNING completedAt=0 shouldSeed=True checksums={} events=[('runStarted','a')]
6 setCategory : category='' outputFolder=/tmp/gd3-probe outputPath=/tmp/gd3-probe/a.bin
6b setCategory: category='video'
7 startAll    : status=[RUNNING,WAITING,FAILED,COMPLETED] waiting=['w','f'] events=[('runStarted','p')]
8 pauseAll    : status=[PAUSED,PAUSED,COMPLETED] events=[('paused','a'), ('runStarted','b'), ('paused','b')] running=[]
9 canDeleteIn : platform=linux result=True
```

要点：第 1 条证明 `pause` 尾部的 `_pump()` 会把 `b` 派发出去；第 2/3 条证明
`deleteFiles` 只在 `canDelete` 时调用、`deletePlaceholders` **无条件**调用；第 4 条是
`fileDeleteDenied` 只提示一次的现场（`deleteFiles` 两次都没调）；第 5 条证明
`redownload` 走 `deleteFiles → reset → _schedule`（`completedAt=0`、`shouldSeed=True`、
`checksums` 清空）；第 6 条证明 `setCategory("")` 保留空串、不动输出目录；第 7 条证明
`startAll` 只排三种状态、派发的那个被 `_dispatch` 改成 `RUNNING`；第 8 条证明
`pauseAll` 的中间态（`b` 先被 pump 派发再被自己暂停）；第 9 条证明 Linux 上恒可删。

### 单测（7 个新增 + 1 个改断言）

| 测试 | 钉住的行为 |
|---|---|
| `pause_cancels_queue_and_marks_record_paused`（改） | `PauseReport.cancel.work_id` + `PAUSED` + 置脏；任务不在 → `None` |
| `pause_pumps_the_freed_slot` | 上限 1、`a` 运行中、`b` 等待 → `pause(a)` 的 `started == ["b"]` |
| `delete_reports_checksum_cancel_and_pumps` | `checksum_work` / `cancel.work_id` / `removed` / `delete_files=false` / `delete_denied=false` / `started == ["b"]` / 校验登记项被摘 |
| `delete_removes_placeholders_but_keeps_content` | 零字节占位符 + `.ghd` 被删；**有内容的文件保留**（`delete_placeholder` 口径） |
| `redownload_resets_and_requeues` | `reset()` 后 `completed_at=0` / `should_seed=true` / `checksums` 空 / step 回 `WAITING` / 重新入队并被派发 |
| `set_category_writes_the_raw_string_and_flushes` | `""` → `Some("")`、`"video"` → `Some("video")`、置脏；先 `flush` 清脏以证明「是它置的」；任务不在 → `false` |
| `start_all_queues_paused_waiting_and_failed` | 只排 `PAUSED`/`WAITING`/`FAILED`、保持记录顺序、排队期间不改状态、`COMPLETED` 不动 |
| `pause_all_pauses_running_and_waiting` | 快照遍历：`started == ["b"]`（`pause(a)` 的 pump 先派发 `b`）、两者都 `PAUSED`、`COMPLETED` 不动 |

### 验证

- `cargo fmt --all` + `--check`：通过；
- `cargo test -p gd3-ui --lib`：**709 passed / 0 failed**（702 + 7）；
- `cargo test --workspace`：**1211 passed / 0 failed**（28 段，第 98 步 1204 + 7）；
- `cargo clippy --workspace --all-targets -- -D warnings`：**0 warning**（5m57s）；
- Python 侧没改一行，不跑 Python 全量（探针脚本在 scratch，未入库）。

### 下一步（第 100 步候选，按价值/风险）

1. **`actions::TaskService` 接线**：`engine.rs` 仍无 `impl actions::TaskService`
   （11 个方法）。trait 收 `&self`、`TaskService` 要 `&mut self` →
   `impl actions::TaskService for RefCell<TaskService>` 适配器；第 97–99 步已把记账
   半边补齐，接引擎只剩协程提交 + 文件监听 + `deleteFiles` 三块；
2. `edit`（`_cancelWork(finished=onStopped)` + `replaceWith` + `setOptions` +
   `updatePlaceholders`）与 `update_selection` 的记账半边；
3. **`buildFontFamilies`**（`app/platform/desktop.py:15-45`）：造 `Script`/`Territory`
   枚举后可搬，纯函数；
4. `app/services/runtime_status.py`（195 行）：需先给 `pack::BinaryRuntime` 补
   `install`/`fetch_latest_version` 的同步签名；
5. D-Bus / keepalive：仍排最后（`zbus` 在离线缓存里，但编译代价大）。

### 卡点

无。

## 第 100 步：`actions::TaskService` 接线（`gd3-ui::task_actions`）（2026-10-18 18:13）

**目标**：把 `actions::TaskService` 的 11 个方法真正落到 `TaskService` 上。trait 收
`&self`、`TaskService` 要 `&mut self`；每个动作的第一步又是「取消协程 / 停做种 / 删文件」，
没有协程运行时只能假装——这一层把两个缺口一起补掉，判定标准是
`app/services/task_service.py` 的**调用序列**与**信号发射顺序**。

### 改动清单

| 文件 | 改动 |
|---|---|
| `gd3-ui/src/task_actions.rs` | **新文件 916 行 / 15 单测**；模块 doc（Python 调用序列表 + 两个缺口 + 顺序约定）、`TaskAction`（12 变体）、`TaskStore` 端口（3 个 `impl`）、`TaskActions<S>`、`impl actions::TaskService`、`start_all` / `pause_all` / `pump` / `rebalance` / `on_run_finished` / `status_of` |
| `gd3-ui/src/task_service.rs` | 3170 → **3180 行 / 72 单测**（无新单测）；新增 `schedule` / `dispatch` 两个记账原语 + `is_running` / `is_waiting` 两个只读访问器；`pause` / `stop` / `resume_saved` / `rebalance` 四处 `record.status = …` 改成 `record.set_status(…)`；模块 doc 改写「为什么还没做 impl」一段 + 新增 `set_status` 口径一条 |
| `gd3-ui/src/lib.rs` | `pub mod task_actions;`（字母序，`task_service` 之前） |
| `gd3-ui/src/engine.rs` | `use std::cell::RefCell;` + `Engine::task_actions() -> TaskActions<RefCell<&mut TaskService>>` |
| `rust/README.md` | 909 → **969 行**；新增「动作层接线（`task_actions.rs`）」一节；规模行更新 |

### 逐条对齐 Python

```python
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
```

Rust 侧的对应：

| Python | Rust | 差异 |
|---|---|---|
| `coroutineRunner.submit/cancel` | `TaskAction::{Started,CancelWork,SeedingStarted,SeedingStopped,ChecksumStarted,ChecksumStopped}` | 效果队列，宿主按序消费 |
| `taskPaused` / `taskRemoved` / `fileDeleteDenied` / `queueChanged` | `TaskAction::{Paused,Removed,DeleteDenied,QueueChanged}` | 同上 |
| `Task.deleteFiles()` | `TaskAction::DeleteFiles` | 排在 `CancelWork` 之后（宿主先取消再删） |
| `_unwatchFile(task)` | `TaskAction::UnwatchFile` | 引擎层摘 `QFileSystemWatcher` |
| `_schedule(task)` | `TaskService::schedule(id, max) -> Vec<String>` | `queue.wait` + `pump` |
| `_dispatch(task)` | `TaskService::dispatch(id, workId) -> bool` | 置 `RUNNING` + `queue.run`，不置脏 |
| `startAll` / `pauseAll` | `TaskActions::start_all` / `pause_all` | 后者按 `isRunning or isWaiting` 过滤 |
| `_pump()` | `TaskActions::pump` | 派发时发号并登记 `Started` |
| `_rebalance` / `_onRunDone` | `TaskActions::rebalance` / `on_run_finished` | 同序聚合效果 |

### 关键决策

- **适配层单独一个模块，不塞进 `task_service.rs`**：`task_service.rs` 保持「引擎无关记账
  半边」的定位（3180 行已经很大），`task_actions.rs` 只做「报告 → 效果」的翻译。
- **用 `TaskStore` 端口 trait 抽象「怎么拿到 `&mut TaskService`」**
  （`with_tasks<R>(&self, f: impl FnOnce(&mut TaskService) -> R) -> R`），三个 `impl`：
  `RefCell<TaskService>` / `RefCell<&mut TaskService>` / `Rc<RefCell<TaskService>>`。
  `Engine.tasks` 仍是值类型（不做 `Rc` 大改），`Engine::task_actions()` 用第二种临时借出。
- **副作用摊成 `TaskAction` 效果队列（12 变体）**：宿主（引擎 / 外壳）按序消费；
  `take_effects()` 取走、`effects()` 只读拷贝。
- **workId 由 `TaskActions` 自己发号**（`run-1` / `seed-2` / `checksum-3`，一个全局
  计数器 + 前缀）：宿主把它当不透明句柄原样交回 `CancelWork`，不解析。
- **`TaskAction::CancelWork` 必须排在 `DeleteFiles` 之前**：Python 把 `onStopped` 放在
  `_cancelWork(finished=onStopped)` 的回调里，Rust 没有回调，只能用**顺序**表达依赖，
  与第 99 步 `DeleteReport::delete_files` 的约定一致（写进模块 doc 的「顺序」一节）。
- **`max_task_num` 用 `MaxTaskNum = Rc<dyn Fn() -> usize>`**：Python 每次 `_pump` 现读
  `cfg.maxTaskNum.value`；引擎临时借出时用 `with_max(store, n)`（一次动作内配置不变，
  等价）。
- **`pause_all` 走 `is_running || is_waiting` 过滤，不用 `TaskService::pause_all`**：
  后者只返回聚合的 `started`，拿不到每个任务的 `CancelWork` / `taskPaused`；
  为此给 `TaskService` 补了 `is_running` / `is_waiting` 两个只读访问器。
- **`set_status` 口径统一**（`pause` / `stop` / `resume_saved` / `rebalance` / `dispatch`）：
  `Task.setStatus` 会连带改未完成 step 的状态、再由 `updateStatus()` 反推任务状态；直接赋
  `record.status` 会漏掉 step 半边，`canPause`（读第一个 `RUNNING` 步骤）永远看到旧值。
- **`dispatch` 不置 `pending_flush`**：Python 的 `_dispatch` 不 `_flushSoon`。
- **`actions::UiHooks` 不在本模块**：剪贴板 / 抽屉 / 编辑对话框 / 打开文件落在
  `app.platform.desktop`，要等 Slint 窗口 + `gd3-core::desktop` 的 GUI 调用方。

### 单测（15 个）

| 测试 | 钉住的行为 |
|---|---|
| `pause_emits_cancel_then_paused_then_dispatches_the_freed_slot` | 效果序 `CancelWork → Paused → Started(b)`；`a` 暂停、`b` 被派发、等待队列空 |
| `start_short_circuits_for_running_and_queued_tasks` | 在跑 → 无效果（连 `_pump` 都不走）；暂停后 → 入队 + 派发 |
| `start_seeding_marks_the_record_and_emits_one_effect` | `shouldSeed=true`、`is_seeding`、`SeedingStarted{seed-1,is_manual:true}`；未知 id → 无效果 |
| `stop_seeding_emits_seeding_stopped_before_the_cancel` | `SeedingStopped` 在 `CancelWork` 之前；`shouldSeed` 置假、登记项摘除 |
| `delete_emits_unwatch_cancel_removed_then_dispatches` | `UnwatchFile → CancelWork → Removed → Started(b)`；无 `DeleteFiles` |
| `delete_puts_delete_files_after_the_cancel_and_checksum_stopped_first` | `ChecksumStopped` 最先、`DeleteFiles` 在 `CancelWork` 之后、`Removed` 之前 |
| `redownload_deletes_files_after_the_cancel_and_requeues` | `UnwatchFile → CancelWork → DeleteFiles → Started`；`completedAt=0` / `shouldSeed` / `checksums` 清空 |
| `move_to_front_emits_queue_changed_only_when_the_queue_takes_it` | 命中即发（哪怕已在队首）；不在等待队列 → 无效果 |
| `set_category_writes_the_field_without_any_effect` | 无效果、`Some("video")`、置脏 |
| `checksum_start_and_cancel_round_trip` | `ChecksumStarted{checksum-1}` → `ChecksumStopped{checksum-1}` → 再取消无效果 |
| `start_all_then_pause_all_walks_the_queue_in_order` | `startAll` 只排 `PAUSED`/`WAITING`；`pauseAll` 的中间态（`w` 先被 pump 派发又被暂停） |
| `work_ids_are_unique_across_kinds` | 三类 workId 互不相同（做种 / 校验 / 下载） |
| `refcell_ref_mut_variant_borrows_the_engine_copy` | `RefCell<&mut TaskService>` 变体：句柄释放后引擎那份能看到改动 |
| `on_run_finished_dispatches_the_next_waiting_task` | `queue.done` 后立刻派发下一个；`all_completed` 在派发之后算 |
| `rebalance_cancels_the_overflow_and_keeps_the_head` | 上限 2→1：`runningIds()[1:]` 的 `b` 被取消并放回等待队列，`a` 不动 |

### 验证

- `cargo fmt --all` + `--check`：通过；
- `cargo test -p gd3-ui --lib task_actions`：**16 passed**（15 个新单测 + 1 个名字撞上的
  旧 smoke `install_task_actions_registers_handlers_headless`）；
- `cargo test --workspace`：**1226 passed / 0 failed**（28 段，第 99 步 1211 + 15）；
- `cargo clippy --workspace --all-targets -- -D warnings`：**0 warning**（7m34s）；
- Python 侧没改一行，不跑 Python 全量。

**已知缺口**：`Engine::task_actions()` 只有编译期验证（借用形态
`RefCell<&mut TaskService>` 能通过类型检查），没有运行期冒烟测试——它的函数体是
「读一次 `max_task_num()` + `RefCell::new(&mut self.tasks)`」，首次 `with_tasks`
必然是单次独占借用，没有可踩的 `RefCell` 双借；下次动 `engine.rs` 时顺手补。

### 下一步（第 101 步候选）

1. `edit`（`_cancelWork(finished=onStopped)` + `replaceWith` + `setOptions` +
   `updatePlaceholders`）与 `update_selection` 的记账半边；
2. **`buildFontFamilies`**（`app/platform/desktop.py:15-45`）：纯函数，造
   `Script`/`Territory` 枚举后可搬；
3. `app/services/runtime_status.py`（195 行）：需先给 `pack::BinaryRuntime` 补
   `install` / `fetch_latest_version` 的同步签名；
4. `actions::UiHooks` 实现：等 Slint 主窗口；
5. D-Bus / keepalive：仍排最后。
## 第 101 步：`edit` / `updateSelection` 接线（第 100 步的续）（2026-10-18 19:05）

**目标**：第 100 步把 `actions::TaskService` 的 10 个方法接到了真 `TaskService` 上；
剩下两个**不走卡片分派**的服务方法——`edit(task, options, newTask)`（编辑对话框）与
`updateSelection(task, selectedIndexes)`（抽屉文件列表）——还停在「只算意图」。
判定标准同前：`app/services/task_service.py` 的**调用序列**与**信号发射顺序**。

### 改动清单

| 文件 | 改动 |
|---|---|
| `gd3-core/src/task.rs` | 420 → **493 行**：新增 `TaskOptions`（`output_folder` / `category`）+ `TaskRecord::{can_reuse_progress, set_options, replace_with, set_selection}` |
| `gd3-ui/src/task_service.rs` | 3180 → **3611 行 / 77 单测**：新增 `EditReport` / `SelectionReport` + `TaskService::{edit, update_selection}` + 5 单测 |
| `gd3-ui/src/task_actions.rs` | 916 → **1116 行 / 18 单测**：`TaskAction::DeleteFilesAt` 变体 + `TaskActions::{edit, update_selection}` + 3 单测 |
| `rust/README.md` | 969 → **987 行**：调用序列表补两行、变体数 12 → 13、新增 `edit` / `updateSelection` 三段 |

### 逐条对齐 Python

```python
edit(task, options, newTask)                       # task_service.py:464-476
    cancelChecksum(task)
    needsDelete = newTask is not None and not task.canReuseProgress(newTask)
    def onStopped():
        if needsDelete: task.deleteFiles()
        if newTask is not None: task.replaceWith(newTask)
        task.setOptions(options)
        self._store.updatePlaceholders(task)
        self._flushSoon()
        self._schedule(task)
    self._cancelWork(task, finished=onStopped)

updateSelection(task, selectedIndexes)             # task_service.py:482-528
    wasCompleted = task.status == COMPLETED        # 早于 setSelection
    def apply():
        task.setSelection(selectedSet)
        self._store.updatePlaceholders(task)
        if wasCompleted and task.files and any(f.selected and not f.completed for f in task.files):
            task.completedAt = 0
            self._unwatchFile(task)
            self._cancelWork(task, finished=lambda: self._schedule(task))
        self._flushSoon()
    isRunningDeselected = 队列在跑 and 第一个 RUNNING step 的 fileIndex 不在选中里
    if isRunningDeselected: _cancelWork(task, finished=onStopped{apply(); _schedule()})
    else: apply()
```

Rust 侧的对应：

| Python | Rust | 差异 |
|---|---|---|
| `task.setOptions(options)` | `TaskRecord::set_options(&TaskOptions)` | `TaskStep.setOptions` 是 `pass`（全仓无覆盖），不搬 |
| `task.replaceWith(newTask)` | `TaskRecord::replace_with(&TaskRecord)` | `_bindTask` 只写回指 → Rust 侧空操作，随后同 `updateStatus()` |
| `task.setSelection(set)` | `TaskRecord::set_selection(&BTreeSet<i64>)` | `set(selectedIndexes)` 用 `BTreeSet` 表达 |
| `task.canReuseProgress(newTask)` | `TaskRecord::can_reuse_progress` | 基类恒 `False`，**全仓无子类覆盖** |
| `task.deleteFiles()`（`edit` 里） | `TaskAction::DeleteFilesAt { paths }` | 路径是**改名前的快照**（见下） |
| `_store.updatePlaceholders(task)` | `TaskService::update_placeholders`（第 94 步） | 返回 `Result<_, CreateError>`，`edit` 透传 |
| `_unwatchFile` | `TaskAction::UnwatchFile` | 已有变体 |

### 关键决策

- **`edit` 的 `deleteFiles` 必须用**快照**路径，新增 `TaskAction::DeleteFilesAt`**：
  Python 的 `onStopped` 先 `deleteFiles()` 再 `replaceWith(newTask)`；`replaceWith` 换掉
  `name` 之后 `placeholderPaths` 就跟着变。Rust 侧记录**就地**改过，宿主事后按
  `taskId` 现算会删到新路径（`delete` / `redownload` 的路径不变，仍用
  `DeleteFiles { task_id }`）。快照由 `TaskService::edit` 在 `replaceWith` 之前取，
  放在 `EditReport::delete_paths`。
- **`edit` / `update_selection` 是 `TaskActions` 的固有方法，不进 `actions::TaskService`
  trait**：trait 是「卡片动作触达的服务面」，编辑对话框走
  `featureService.createEditDialog`、选择走抽屉文件列表，都不经过 `card::` 分派。
- **`edit` 不 `_unwatchFile`**（`delete` / `redownload` 会）——照抄 Python；文件监听由
  引擎在协程收尾时摘。
- **`TaskOptions.category` 用 `Option<Option<String>>`**：Python 用
  `if "category" in options` 判断键存在，`{"category": None}` 会把分类清成 `None`；
  单层 `Option` 表达不了「键不存在」与「键存在、值 None」之别。
- **`TaskOptions.output_folder` 只收 `String`**：Python 的
  `isinstance(newFolder, (str, Path))` 允许 `Path`，Rust 侧调用方自己转字符串
  （记录里存的就是 `String`）。
- **`update_selection` 的 `wasCompleted` 在 `set_selection` 之前快照**：`setSelection`
  末尾 `updateStatus()`，已完成任务可能被重算成别的状态。
- **「有选中未完成文件」看新选中集**：`apply()` 里 `setSelection` 已经改过
  `files[].selected`，判断必须用它之后的值。
- **`isRunningDeselected` 只看第一个 `RUNNING` step**（Python `break`）：
  `file_index is None` 判 `False`。
- **`restarted` 与 `interrupted` 互斥**（在跑的任务不会是 `COMPLETED`），所以
  「尾部 `_schedule`」两种分支各一次、不会叠加；实现里用
  `if restarted || interrupted` 一次表达。
- **`edit` 出错时记录已被改过**：`replaceWith` / `setOptions` 先跑、
  `update_placeholders` 才可能 `Err`，与 Python 抛异常时的中间态一致；本层此时
  **不登记任何效果**（`?` 在 emit 之前）。
- **`SelectionReport` 用 `Option<CancelWork>` 而不是「空 `CancelWork`」**：外层取消
  「没发生」与「发生了但没工作可取消」是两件事，前者不该发任何效果。

### 单测（8 个）

| 测试 | 钉住的行为 |
|---|---|
| `edit_without_new_task_writes_options_and_requeues`（service） | 没 `newTask` → `needs_delete=false`；`delete_paths` 仍是旧路径；`outputFolder` / `category` 落上；占位符跟着搬；`started=[a]`；**`report.status` 仍是 `Paused`**（`pump` 只排队） |
| `edit_with_new_task_snapshots_the_old_delete_paths`（service） | `needs_delete=true`；`delete_paths` 是**改名前的**路径；`replaceWith` 换了 name / url、不换 outputFolder |
| `update_selection_marks_files_and_recomputes_the_size`（service） | `files[].selected` 按选中集置位；`fileSize` = 选中项之和；`restarted=false`、无效果 |
| `update_selection_restarts_a_completed_task_with_selected_incomplete_files`（service） | `restarted=true`；`completedAt=0`；`restart_cancel` 一定 `Some`（哪怕没工作可取消）；`started=[a]` |
| `update_selection_cancels_a_running_task_when_the_running_file_is_deselected`（service） | 第一个 `RUNNING` step 的 `fileIndex` 被取消勾选 → 外层 `cancel.work_id = work-1`，随后重新派发 |
| `edit_emits_delete_files_at_the_snapshot_paths_then_dispatches`（actions） | 效果序 `DeleteFilesAt(旧路径) → Started(run-1)`；`status` 由动作层的 `_dispatch` 置 `Running` |
| `update_selection_restart_emits_unwatch_then_dispatches`（actions） | `UnwatchFile → Started`（没有工作可取消 → 不发 `CancelWork`） |
| `update_selection_of_a_running_task_cancels_before_it_requeues`（actions） | `CancelWork(run-1) → Started(run-2)`：外层取消在 `apply()` 之前 |

### 验证

- `cargo fmt --all` + `--check`：通过；
- `cargo test -p gd3-ui --lib`：**739 passed / 0 failed**；
- `cargo test --workspace`：**1234 passed / 0 failed**（28 段，第 100 步 1226 + 8）；
- `cargo clippy --workspace --all-targets -- -D warnings`：**0 warning**（9m49s）；
- Python 侧没改一行，不跑 Python 全量。

### 下一步（第 102 步候选）

1. **`buildFontFamilies`**（`app/platform/desktop.py:15-45`）：纯函数，造
   `Script`/`Territory` 枚举后可搬；
2. `app/services/runtime_status.py`（195 行）：需先给 `pack::BinaryRuntime` 补
   `install` / `fetch_latest_version` 的同步签名；
3. `actions::UiHooks` 实现：等 Slint 主窗口；
4. D-Bus / keepalive：仍排最后。

## 第 102 步：运行时状态服务（`gd3-ui::runtime_status`）（2026-10-18 20:06）

**目标**：`app/services/runtime_status.py`（195 行）的移植。它是设置页
`RuntimeCard` 的唯一数据源——「某个运行时装没装 / 什么版本 / 有没有新版 / 正在装到几成」。
第 95 步搬 `setting_cards.rs` 时只搬了**渲染**（`runtime_view(status, can_install,
is_managed)`，输入是快照），产生快照的那个服务一直缺着。

### 改动清单

| 文件 | 改动 |
|---|---|
| `gd3-core/src/pack.rs` | 1165 → **1180 行**：`BinaryRuntime` 新增 `fetch_latest_version`（缺省 `Ok("")`）与 `install`（缺省 `Unsupported`）两个同步默认方法；`runtime_id` 去掉 `where Self: Sized` 进 vtable；+1 单测 |
| `gd3-core/src/task_error.rs` | 新增 `TaskError::install_not_found(name)`（`安装后未找到 {name}`） |
| `gd3-ui/src/runtime_status.rs` | **新文件 932 行 / 22 单测**：`RuntimeStatus` / `RuntimeWorkKind` / `RuntimeEffect` / `RuntimeStatusService` |
| `gd3-ui/src/lib.rs` | 注册 `pub mod runtime_status;` |
| `rust/README.md` | 987 → **1049 行**：新增「运行时状态」一节 + 修上一节过期的规模行 |

### 逐条对齐 Python

| Python（`runtime_status.py`） | Rust | 备注 |
|---|---|---|
| `status(runtime)` :38-42 | `status(&dyn BinaryRuntime)` | 没登记过就现造一个只带 id/name/path 的 |
| `refreshStatus` :44-51 | `refresh_status` | 已知状态干净且路径没变 → **无效果** |
| `invalidate` :53-60 | `invalidate` | 先 `CancelWork` 再 `probe` |
| `install` :62-88 | `install` | 已在装 → 无效果；版本号取当前状态的 `latestVersion` |
| `cancelInstall` :90-101 | `cancel_install` | 没在装 → 无效果 |
| `_runInstall` 的 `reportProgress` :106-113 | `report_progress` | 只在「正在装 + 整数进度变了」时 `Emit` |
| `_runInstall` 尾巴 :115-117 | `check_installed`（关联函数） | 失败返回 `TaskError::install_not_found(name)` |
| `_onInstallFinished` :119-124 | `on_install_finished` | 先摘 `workId` 再 `invalidate` → **不吐** `CancelWork` |
| `_onInstallFailed` :126-137 | `on_install_failed` | 不在装 → 忽略（迟到的失败回调） |
| `_probe` :139-154 | `probe`（私有） | 置「检测中」→ `Emit` → 派 `Probe` |
| `_onProbeFinished` :156-172 | `on_probe_finished` | `name`/`path` 是探测时快照；`isAppManaged()` 才 `_fetchLatest` |
| `_onProbeFailed` :174-180 | `on_probe_failed` | 记 `error`、丢掉持有对象 |
| `_fetchLatest` :182-189 | `fetch_latest`（私有） | 只派活，不动状态 |
| `_onFetchFinished` :191-199 | `on_fetch_finished` | 版本为空 → 不改状态；状态不存在 → 什么都不做 |
| `_onFetchFailed` :201-203 | `on_fetch_failed` | **只清账**，不吐效果（Python 也只有两行 `pop`，`error` 直接丢） |

### 关键决策

- **副作用摊成 `RuntimeEffect` 队列**（`Emit` / `CancelWork` / `Run`）：沿用第 100/101 步
  的形态。Python 的 `coroutineRunner.submit(coro, done=…, failed=…)` 在 Rust 里没有对应
  物，宿主就是运行器——它拿到 `Run` 去跑 `probe_version` / `fetch_latest_version` /
  `install`，跑完回调 `on_*`。
- **`workId` 由服务自己发号**（`runtime-1` / `runtime-2` …，**一个全局计数器**）：Python
  拿的是 `submit` 的返回值；服务替宿主发号才能记账（`_workIds[runtimeId]`），宿主把它当
  **不透明句柄**原样交回 `CancelWork`（同第 100 步 `task_actions`）。
- **`BinaryRuntime::runtime_id` 进 vtable**：服务按 `runtimeId` 记账，手上是
  `Rc<dyn BinaryRuntime>`（Python 是 `dict[str, BinaryRuntime]`）；带
  `where Self: Sized` 时 `dyn` 调不到。去掉后 trait 仍 object-safe（`&'static str` 返回值）。
  `grep` 过全仓：`runtime_id` 只在 `pack.rs` 内用过，改动面为零。
- **`_runtimes` 用 `Rc<dyn BinaryRuntime>`**：Python 在异步间隙里攥着对象引用，
  `_onProbeFinished` 要问它 `isAppManaged()`、`_onInstallFinished` 要再 `invalidate` 一次。
  `Rc` 是唯一能表达「服务与调用方共享同一份对象」的写法（`Box` 做不到，`dyn` 不能 clone）。
- **`BinaryRuntime` 新增两个同步默认方法**：`fetch_latest_version`（缺省 `Ok("")`，对齐基类
  `return ""`）、`install`（缺省 `Unsupported`，对齐 `raise NotImplementedError`）。两者在
  Python 都是 `async`，Rust 同步阻塞——与 `probe_version` 同一口径（`pack.rs` 模块 doc 第 5 条）。
- **`report_progress` 的 `percent as i32`**：Python 是 `int(percent)`（向零截断），对 `NaN`
  抛 `ValueError`；`as i32` 饱和到 `0`。实际无影响：进度只由本仓自己的 `install` 实现回报。
- **`install` 的「安装中」状态 `path` 为空**：照抄 Python
  `RuntimeStatus(runtimeId, runtime.name, isInstalling=True)`——`path` 不传，走缺省 `""`。
  所以装的过程中卡片显示「正在下载… N%」而不是版本号。
- **`cancel_install` 的状态 `path` 现读 `runtime.path()`**：Python 是
  `RuntimeStatus(runtimeId, runtime.name, path=runtime.path())`，与 `install` 不同。
- **`to_card_status` / `card_view` 把本地化函数当参数**：`setting_cards::RuntimeStatus.error`
  是**已本地化文本**（Python `toLocalizedError(status.error)`），而 `gd3-ui` 只有错误键与
  参数、没有翻译表。所以翻译由调用方注入（`|e| e.format()` 是当前唯一可用的近似）。

### 单测（22 个）

| 测试 | 钉住的行为 |
|---|---|
| `status_falls_back_to_a_bare_snapshot` | 没探测过 → `status()` 现造 id/name/path，`is_busy=false`，不持有对象 |
| `refresh_status_emits_busy_then_runs_a_probe` | 效果序 `Emit(busy) → Run{Probe, runtime-1}`；`work_id` / `held_runtimes` 同步 |
| `refresh_status_is_a_no_op_when_the_path_is_unchanged` | 干净状态 + 路径没变 → 空效果 |
| `refresh_status_probes_again_after_the_path_moves` | 路径变了 → 重新探测，且 `Emit` 里带新路径 |
| `refresh_status_probes_again_after_a_failed_probe` | 上次带 `error` → 重新探测 |
| `on_probe_finished_fetches_the_latest_version_for_managed_runtimes` | 托管运行时 → `Emit(version/detail) → Run{FetchLatest, runtime-2}` |
| `on_probe_finished_drops_unmanaged_runtimes` | 非托管 → 只有 `Emit`，持有对象清空、`workId` 摘掉 |
| `on_probe_failed_records_the_error_and_drops_the_runtime` | `error` 落上、`path` 用快照值、对象清空 |
| `install_emits_installing_then_runs_with_the_latest_version` | `Emit(is_installing, path="") → Run{Install{version: "2.0"}}` |
| `install_is_ignored_while_already_installing` | 重复 `install` 无效果 |
| `report_progress_only_emits_when_the_integer_progress_changes` | 0.5 → 无（0 == 0）；12.7 → 12；12.9 → 无；未知 id → 无 |
| `report_progress_ignores_statuses_that_are_not_installing` | 非安装中 → 无效果 |
| `cancel_install_cancels_the_work_then_restores_the_idle_status` | `CancelWork → Emit(空状态)`；再取消无效果 |
| `on_install_finished_invalidates_and_probes_again` | **不吐 `CancelWork`**（先摘 `workId`），`Emit(busy) → Run{Probe, runtime-2}` |
| `on_install_finished_of_an_unknown_runtime_does_nothing` | 没有持有对象 → 空效果 |
| `on_install_failed_marks_the_error_and_clears_the_flag` | `is_installing=false` + `error`；迟到回调被忽略 |
| `on_fetch_finished_skips_the_emit_for_an_empty_version` | `""` → 无效果；`"3.0"` → `Emit`；未知 id → 无 |
| `on_fetch_failed_only_clears_the_bookkeeping` | 不吐效果、状态不动（仍是 `is_busy`） |
| `check_installed_reports_the_runtime_name` | 非托管 → `Err(安装后未找到 ffmpeg)`；托管 → `Ok(())` |
| `card_status_bridges_to_the_setting_card_snapshot` | 本地化函数被调用一次；`content` = 错误文本；按钮态由 `can_install` 决定 |
| `the_host_can_drive_install_and_fold_progress_back` | 宿主循环：`runtime.install(version, report)` → `report_progress` 折回 `Emit(progress=40)` |
| `work_ids_are_unique_across_kinds` | 探测 / 拉版本 / 安装三类 workId 互不相同 |

### 验证

- `cargo fmt --all` + `--check`：通过（`FMT_OK` / `FMT_CHECK_OK`）；
- `cargo check -p gd3-ui --tests`：干净（无 warning）；
- `cargo test -p gd3-ui --lib`：**754 passed / 0 failed**（732 + 22）；
  首跑 1 个失败：`install_emits_installing_then_runs_with_the_latest_version` 断言
  `workId == runtime-3`，实际拿到 `runtime-2`。原因是 `fake()` 默认**非托管**，
  `on_probe_finished` 不会去拉最新版本，只发了 1 个号。修法不是改断言而是改夹具：
  该测试把 `managed` 置真，走完整的「探测 → 拉版本 → 安装」三步，`runtime-3` 才成立；
  这也让 `on_fetch_finished` 的调用有了真实来源。
- `cargo test --workspace`：**1257 passed / 0 failed**（28 段，第 101 步 1234 + 22 + 1）；
- `cargo clippy --workspace --all-targets -- -D warnings`：**0 warning**（6m51s）；
- Python 侧没改一行，不跑 Python 全量。

### 下一步（第 103 步）

**选 `app/services/task_draft.py`（241 行）+ `difflib.SequenceMatcher` 的最小移植**
（`gd3-ui::task_draft` / `gd3-ui::sequence_matcher`）。理由：

- 它是 P2「建任务链路」的状态机核心，`setUrls` 的**增量 diff** 直接决定哪些草稿被
  取消、哪些被提交解析；不搬它，主窗口的批量 URL 面板无法接线；
- 不碰网络：`featureService.parse(options)` 是端口，宿主注入；
- 唯一的硬骨头是 `SequenceMatcher(a=旧 urls, b=新 urls, autojunk=False)`——`isjunk=None`
  + `autojunk=False` 时 `__chain_b` 退化成「元素 → 下标列表」的普通 map，`find_longest_match`
  的两条 junk 扩展循环也整体消失，只剩「最长公共块 + 队列切分 + 相邻块合并」三步，
  可以逐行照抄（`/usr/lib/python3.13/difflib.py`）；
- 预期测试向量已用 Python 现算（含 `[]`/单边空/全等/交换/重复元素等 13 组 opcodes）。

**暂缓 `update_service.py`（335 行）**：它依赖 `app/sources.py`（295 行，github/gitcode
双源 fallback 的 async HTTP）与 `zipfile` / `tarfile(r:xz)` / `hdiutil` 三种解包。
本机离线缓存里只有 `flate2` / `tar` / `zstd`，**没有 `zip` 与 `xz2`**，硬做要么手写
ZIP 中央目录解析、要么退化成 `Unsupported`——两条路都会把这一步拖成两步。等
`gd3-net` 侧的 sources 端口成形后再搬。

**其余候选（按可行性排）**：`app/services/feature_service.py`（234 行，工厂，等主窗口）；
`app/platform/desktop.py:15-45` `buildFontFamilies`（纯函数，但要先造 `QLocale.Script` /
`Country` 的映射表）；`actions::UiHooks`（等主窗口）；D-Bus / keepalive（仍排最后）。

## 第 103 步：任务草稿（`gd3-ui::task_draft` / `gd3-ui::sequence_matcher`）（2026-10-18 20:44）

**目标**：`app/services/task_draft.py`（241 行）的移植。它是 P2「建任务链路」的状态机核心：
批量 URL 面板往里面塞 URL，它做**增量 diff**（谁留下、谁取消、谁新提交解析），
解析回调回来时把 `Task` 挂到草稿项上，确认时把整批草稿交给建任务链路。

唯一的外部依赖是 `difflib.SequenceMatcher(a=previousUrls, b=urls, autojunk=False)`
——`isjunk=None` + `autojunk=False` 是**全仓唯一用法**，所以只移植这一种配置。

### 改动清单

| 文件 | 改动 |
|---|---|
| `gd3-ui/src/sequence_matcher.rs` | **新文件 387 行 / 11 单测**：`OpTag` / `Opcode` / `Match` / `opcodes()` / `matching_blocks()` / `find_longest_match()` |
| `gd3-ui/src/task_draft.rs` | **新文件 1073 行 / 32 单测**：`DraftItem` / `DraftEffect`（8 变体）/ `AddParsedReport` / `TaskDraft` |
| `gd3-core/src/task.rs` | 493 → **513 行**：新增 `impl TaskOptions { pub fn from_options(&Map<String, Value>) -> Self }`；顺手修掉第 101 步遗留的错位 doc 注释 |
| `gd3-ui/src/lib.rs` | 注册 `pub mod sequence_matcher;` / `pub mod task_draft;` |
| `rust/README.md` | 1049 → **1131 行**：新增「任务草稿」一节 |

### 逐条对齐 Python

| Python（`task_draft.py`） | Rust | 备注 |
|---|---|---|
| `urls` :42 | `urls()` | 按 `items` 顺序现算 |
| `items` / `itemByUrl` / `taskByUrl` :45-56 | `items()` / `item_by_url` / `task_by_url` | 后者只看 `task.is_some()` |
| `failCount` :58 | `fail_count()` | |
| `canConfirm` :61 | `can_confirm()` | 「有任一项在解析或有 task」且「不是全部失败」 |
| `update` :64 | `update(url, mutate)` | 只对**已解析**的项生效，改完发 `Changed` |
| `setBaseOptions` :71 | `set_base_options(&Map)` | 已解析的项同步重算 options |
| `setUrlCategory` :78 | `set_url_category(url, Option<&str>)` | 同时改 `category_override` 与已解析 task |
| `setUrls` :87-116 | `set_urls(&[String])` | 见下方 diff 表 |
| `refresh` :118-126 | `refresh(url)` | 只重提**失败**的项 |
| `_submit` :128-141 | `submit_at`（私有） | 先清 `error`，再发 `Parse`；无「提交异常」路径 |
| `addParsedTasks` :143-172 | `add_parsed_tasks(&[TaskRecord])` | 见下方两遍循环 |
| `confirm` :174-190 | `confirm(auto_start)` | 见下方两遍循环 |
| `clear` :192-199 | `clear()` | 取消所有待解析项；**不动 `detached`** |
| `_buildOptions` :201-205 | `build_options(base, item)`（自由函数） | `{**base, "url": url, "category": override}` |
| `_isParsing` :207 | `is_parsing()` | 只看 `items`，不看 `detached` |
| `_onParsed` :210-225 | `on_parsed(parse_id, task)` | 按 `parseId` 反查；反查不到就丢 |
| `_onParseFailed` :227-241 | `on_parse_failed(parse_id, error)` | 同上；`detached` 里的项直接丢 |

### `setUrls` 的 diff 口径

`SequenceMatcher(a=旧 urls, b=新 urls, autojunk=False).get_opcodes()` 的每个 opcode：

| tag | 动作 |
|---|---|
| `equal` | 旧项原样搬进 `next`（**不重提交**，`parseId` 保留） |
| 其它 | 先对 `previous[oldStart:oldEnd]` 里 `parseId` 非空的项发 `CancelWork` 并清空 `parseId`；再对 `urls[newStart:newEnd]` 逐个 `DraftItem` + `submit_at` |

收尾顺序固定：`Busy(is_parsing())` → 各 `Failed`（`submitErrors`）→ `Changed`。
**Rust 没有 `submitErrors`**：`Parse` 只是入队、同步不会失败，所以退化成 `Busy → Changed`。

### 关键决策

- **选 `task_draft.py` + `SequenceMatcher` 而不是 `update_service.py`**：`update_service.py`（335 行）
  依赖 `app/sources.py`（295 行，github/gitcode 双源 fallback 的 async HTTP）与
  `zipfile` / `tarfile(r:xz)` / `hdiutil`；本机离线缓存里**没有 `zip` 与 `xz2`**。
  `task_draft` 不碰网络、不碰解包，是纯状态机。
- **`sequence_matcher` 只覆盖 `isjunk=None` + `autojunk=False`**：此时 `__chain_b` 退化成
  「元素 → 下标列表」的普通 map，`find_longest_match` 的两条 junk 扩展循环是死代码，整体省略。
  `get_opcodes()` 不缓存（Python 有 `_opcodes` 缓存，但这里每次都是新对象）。
- **`matching_blocks` 的哨兵 `Match { a: a.len(), b: b.len(), size: 0 }` 必须保留**：
  `get_opcodes` 靠它把「最后一个公共块之后的部分」也吐出来。丢掉哨兵会漏掉尾部 `delete`/`insert`。
- **`Match` 的 `Ord` 派生顺序即 Python 元组排序口径**：字段声明顺序 `(a, b, size)`。
- **`DraftEffect::Parse` 携带原始 options 字典（含 `"url"` 键），不在服务里转 `TaskOptions`**：
  本仓 `TaskOptions` 只建模 `outputFolder` / `category`，提前转换会丢掉 `headers` /
  `subworkerCount` 等尚未建模的键。转换放在宿主（真正建任务的那一侧）。
- **`confirm()` 之后 `items` 清空，仍在解析的项移进独立的 `detached: Vec<DraftItem>`**：
  Python 是 `self._items = []`，但协程仍持有 `item` 引用，回调照样会进来。Rust 没有「被协程持有」
  这回事，必须自己留一份，否则迟到的回调无处可去。
  `on_parsed` / `on_parse_failed` 先查 `detached` 再查 `items`（按 `parse_id` 定位）。
- **`detached` 里的项必然 `confirmed_options.is_some()`**：`confirm()` 第二遍会取消
  「`parse_id` 非空但 `confirmed_options` 为 None」的项，只有冻结过 options 的才进 `detached`。
- **回调按 `parseId` 反查而不是闭包捕获 item**：取消时 `parse_id` 被清空，于是「取消后迟到的回调」
  自然查不到，与 Python 的 `if not item.parseId: return` 等价。
- **`build_options` 写成自由函数 `fn build_options(base: &Map, item: &DraftItem) -> Map`**：
  避免「`&mut self.items` 与 `&self.base_options` 同时借」的借用冲突。
- **`submitErrors` 判 N/A**：见上。
- **`category_override: Option<String>`**（`None` = 不覆盖）：Python 是
  `dict.get("category")`，同样表达不了「把 category 显式置 None」，口径一致。
- **`DraftEffect::Confirmed` 携带 `TaskRecord` 的克隆**：Python 传活对象；效果队列的消费者本来就取所有权。
- **`TaskDraft` 不持有 `coroutineRunner` / `featureService`**：两者分别退化成效果队列与宿主注入。
- **`clear()` 不动 `detached`**：那里是已确认的任务，清草稿不该把它们的解析掐掉。
- **`is_parsing()` 只看 `items` 不看 `detached`**：照抄 Python，`confirm()` 之后
  `parsingBusyChanged` 仍发 `false`。
- **`add_parsed_tasks` 的 `tasks` 非空但全被跳过时仍发 `Busy` + `Changed`**：Python 的两个 emit
  在循环之外，不能因为「什么都没干」就跳过。
- **`on_parsed` 必须把 `build_options` 应用到 task 上再入队 `Succeeded`**：首跑漏了这一句，
  被 `unused variable: options` 警告抓出来（见下）。
- **`work_id` 形如 `draft-N`，`N` 由一个 `u64` 计数器发号，永不复用**。

### 单测（11 + 32 个）

`sequence_matcher.rs`（11）：

| 测试 | 钉住的行为 |
|---|---|
| `empty_sequences_produce_no_opcodes` | `([], [])` → 空 |
| `identical_sequences_produce_one_equal_opcode` | 全等 → 单个 `Equal{0,n,0,n}` |
| `a_single_substitution_is_a_replace_between_two_equals` | `abc` vs `axc` → `Equal/Replace/Equal` |
| `reversed_sequences_anchor_on_the_longest_block` | 反转 → 最长公共块优先，两侧各一 `Delete`+`Insert` |
| `a_contiguous_slice_in_the_middle_is_one_delete_either_side` | 中段整体删除 → 两段 `Equal` 夹一个 `Delete` |
| `shifting_a_window_produces_delete_equal_insert` | 窗口平移 → `Delete`/`Equal`/`Insert` |
| `repeated_elements_keep_the_leftmost_longest_block` | 重复元素 → 取最左的最长块（`find_longest_match` 的 `besti` 只更新严格更长） |
| `alternating_keeps_and_inserts_are_all_equal_length` | 交替序列 → 块长与 Python 一致 |
| `adjacent_blocks_are_merged_into_one_equal_opcode` | 相邻块合并成一个 `Equal` |
| `opcodes_are_contiguous_by_construction` | `i1<=i2`、`j1<=j2`、相邻 opcode 首尾相接、覆盖到末尾 |
| `works_on_owned_strings_too` | `T = String` 也能用（泛型 `Eq + Hash`） |

`task_draft.rs`（32）：

| 测试 | 钉住的行为 |
|---|---|
| `set_urls_submits_each_url_once_and_reports_busy` | 空 → 3 项：`Busy(true)` → 3×`Parse` → `Changed` |
| `set_urls_dedupes_while_keeping_the_first_occurrence` | 重复 URL 只留第一次出现 |
| `set_urls_keeps_equal_items_without_resubmitting` | 同列表 → 只有 `Changed`，`parse_id` 不变 |
| `set_urls_with_an_identical_list_only_emits` | 同上（不同夹具） |
| `set_urls_replaces_the_middle_span` | 中段替换 → 只取消被替换的那几项 |
| `set_urls_cancels_the_tail` | 尾部删除 → `CancelWork` 只发给被删项 |
| `set_urls_does_not_cancel_items_that_already_finished` | `parse_id` 已空的项**不发** `CancelWork` |
| `set_urls_to_an_empty_list_cancels_everything` | 清空 → 每项一条 `CancelWork` |
| `refresh_resubmits_only_the_failed_item` | 只重提 `error.is_some()` 的项 |
| `refresh_ignores_healthy_and_unknown_items` | 健康项 / 未知 URL → 空效果 |
| `add_parsed_tasks_appends_unknown_urls` | 新 URL 追加为新项并挂上 task |
| `add_parsed_tasks_skips_urls_that_already_have_a_task` | 已有 task 的 URL 跳过 |
| `add_parsed_tasks_cancels_a_pending_parse_and_takes_over` | 还在解析的项 → 先 `CancelWork` 再接管 |
| `add_parsed_tasks_with_no_tasks_is_a_no_op` | `tasks` 为空 → 空效果（**连 `Changed` 都不发**） |
| `on_parsed_stores_the_task_and_reports_success` | `Succeeded{url, task}` + `Busy(false)` + `Changed` |
| `on_parsed_applies_the_base_options` | task 的 `output_folder` / `category` 来自 `base_options` |
| `on_parsed_of_a_cancelled_parse_is_ignored` | `parse_id` 已清 → 空效果 |
| `on_parse_failed_records_the_error` | `error` 落上 + `Failed` |
| `confirm_hands_over_parsed_items_and_detaches_the_pending_ones` | 已解析的进 `Confirmed`，未解析的进 `detached` |
| `confirm_carries_the_auto_start_flag` | `Confirmed{auto_start}` 透传 |
| `confirm_freezes_the_options_of_a_pending_item` | 待解析项在 `confirm` 时冻结 options |
| `on_parse_failed_of_a_detached_item_drops_it_silently` | `detached` 项失败 → 无效果（不再发 `Failed`） |
| `clear_cancels_every_pending_parse` | 每项一条 `CancelWork` |
| `clear_leaves_detached_items_alone` | `detached` 不受影响 |
| `set_base_options_rewrites_parsed_tasks` | 改基准 options → 已解析 task 跟着改 |
| `parse_carries_the_base_options_and_the_url` | `Parse{work_id, options}` 里含 `url` 与基准键 |
| `a_category_override_wins_over_the_base_options` | 单项覆盖优先于基准 |
| `set_url_category_updates_the_parsed_task_too` | 已解析 task 的 `category` 同步 |
| `update_mutates_only_parsed_items` | 未解析项改不到 |
| `can_confirm_is_true_while_anything_is_parsing_or_parsed` | 有在解析或有 task → `true` |
| `can_confirm_is_false_when_every_item_failed` | 全失败 → `false` |
| `work_ids_are_never_reused` | 取消/清空之后新发的号仍递增 |

### 三次失败定位（第一次跑出 2 个红灯，第二次 1 个，第三次全绿）

1. **首跑 794 passed / 2 failed**，其中一个暴露**真 bug**：
   `on_parsed` 里算了 `build_options(...)` 却忘了 `task.set_options(...)`，
   编译器给了 `unused variable: options` 警告——**这一条是 Rust 编译器替我发现的**，
   否则会变成「草稿项显示的分类与最终任务不一致」的隐性缺陷。
   另两条是**测试断言写错**（实现是对的）：
   - `set_urls_does_not_cancel_items_that_already_finished`：夹具里 `b` 早已解析完
     （`parse_id` 已空），本来就不该有 `Cancel`，断言却要求有一条；
   - `add_parsed_tasks_skips_urls_that_already_have_a_task`：全跳过时仍会发
     `Busy` + `Changed`（Python 的两个 emit 在循环之外），断言漏了这两条。
2. **二跑 796 passed / 1 failed**：还是
   `add_parsed_tasks_skips_urls_that_already_have_a_task`，断言写成 `busy == true`，
   实际 `busy == false`——被跳过项的 `parse_id` 是空的，`is_parsing()` 自然是 `false`。
   改断言。
3. **三跑 797 passed / 0 failed**（`/tmp/ui_lib3.log`，2.58s）。

**教训**：效果队列类测试的断言要把「emit 的时机在循环内还是循环外」写清楚；
Python 里两个 emit 挨着写、看着像一次，但语义是「循环之后无条件发」。

### clippy `-D warnings` 的 4 个红灯（首跑）

| 位置 | lint | 修法 |
|---|---|---|
| `sequence_matcher.rs:75` | `needless_range_loop` | `for i in alo..ahi { a[i] }` → `for (i, av) in a.iter().enumerate().take(ahi).skip(alo)`，体内改读 `av` |
| `task_draft.rs:211` | `unnecessary_unwrap` | `if item.task.is_some() { … item.task.as_mut().expect(…) }` → 保留外层判空、内层改 `if let Some(task) = item.task.as_mut()` |
| `task_draft.rs:252` | `collapsible_if` | 嵌套 `if let Some(mut item) = slot.take() { if item.is_parsing() {…} }` → `if let Some(mut item) = slot.take() && item.is_parsing() {…}`（edition 2024 let-chain） |
| `task_draft.rs:357` | `unnecessary_unwrap` | 同 211，且把 `effects.push(Confirmed{…})` 一起挪进 `if let` |

**为什么不用 `#[allow]`**：这四处都是「写法可以更直接」，不是「工具误报」。
`needless_range_loop` 那条尤其值得改——`av` 是 `&T`，比 `a[i]` 少一次边界检查，
而且顺手消掉了「`i` 同时充当索引与位置」的双重身份。

### 验证

- `cargo fmt --all` + `--check`：通过（`FMT_CHECK_EXIT=0`）；
- `cargo test -p gd3-ui --lib`：**797 passed / 0 failed**（754 + 11 + 32）；
- `cargo test --workspace`：**1300 passed / 0 failed**（28 段；第 102 步 1257 + 43）；
- `cargo clippy --workspace --all-targets -- -D warnings`：**0 warning**（4m29s）；
- 修完 clippy 的 4 处之后**重跑** `--workspace`：仍是 **1300 passed / 0 failed**（`/tmp/ws103b.log`）；
- Python 侧没改一行，不跑 Python 全量。

### 下一步（第 104 步）

**选 `app/services/speed_meter.py`（56 行）+ `app/services/name_conflict_queue.py`（73 行）**：

- 两者都是**引擎级小服务**，且都是当前 Rust 侧的真实缺口：
  `SpeedMeter` 在 `engine.py:20` 构造、注入 `TaskService`（`:320`），`TaskService` 在
  `:578` 把 `addSpeed` / `waitForSpeedLimit` 交给 `task.run(...)`、在 `_refreshSpeedMeter`
  里 `start()` / `stop()`。Rust 侧第 81 步把这一整套**退化成了
  `TaskService::speed_meter_should_run()`**（只回答「该不该跑」，不真的跑），
  搬完 `speed_meter` 才能把引擎那一侧接上；
- `NameConflictQueue` 是**重名冲突的串行化队列**：一次只弹一个冲突给 UI，
  `setChoice(…, isAppliedToRest)` 把同一个选择套用到后面 `restCount` 个任务。
  Rust 侧第 93 步只做到 `add_with_conflict`（单个任务），没有队列；
  `probe_conflict_by_id`（第 96 步）与 `add_with_conflict(…, choice)`（第 93 步）都已就位，
  端口齐了；
- 两者都不碰网络、不碰解包，`coroutineRunner.post` 退化成「`refresh_posted` 标记 + 返回效果」，
  与第 103 步同一形态。

## 第 104 步：速度表（`gd3-ui::speed_meter`）

> 时间：2026-10-18 21:19（`date '+%Y-%m-%d %H:%M'` 实取）

### 选型：为什么是 `speed_meter.py`

`app/services/speed_meter.py` 只有 56 行，但它是**引擎级真实缺口**：第 81 步搬
`task_service.rs` 时把整个速度表退化成了一个谓词
`TaskService::speed_meter_should_run()`（只回答「该不该跑」），`engine.py:20` 那句
`self._speedMeter = SpeedMeter(coroutineRunner)` 一直没有对应物。任务侧的两个注入点
（`task_service.py:578` 的 `task.run(self._speedMeter.addSpeed,
self._speedMeter.waitForSpeedLimit)`）也因此缺一半。

原计划同批搬 `app/services/name_conflict_queue.py`，**实际做的时候发现重复，已放弃**，
原因见下一节。

### 放弃 `name_conflict_queue.py`：它已经搬过了

第 65 步 b（`gd3-ui::name_conflict`，1180 行里的一节）就把
`name_conflict_queue.py` 的队列搬完了，第 96 步把 `probeConflict` 接上、
`engine.rs` 里 `on_name_conflicted` / `refresh_conflicts` / `apply_dialog_outcome`
三个入口全在跑：

| Python | Rust（既有） |
|---|---|
| `add(task)` | `name_conflict::NameConflictQueue::add` |
| `setChoice(...)` | `::set_choice` |
| `cancel(taskId)` | `::cancel` |
| `_refresh()` | `::refresh` |
| `_post(self._refresh)` | `ConflictEvent::RefreshPosted` |
| `conflictChanged.emit(...)` | `ConflictEvent::ConflictChanged(Option<NameConflict>)` |
| `probeConflict` / `add` 注入 | `trait ConflictActions` |

本轮我一度新写了 `gd3-ui::name_conflict_queue`（556 行 / 13 单测，`TaskRecord` 载荷 +
`Box<dyn Fn>` 闭包注入），编译前对照 `name_conflict.rs:341` 才发现是**同一状态机的第二份
实现**——而且 `engine.rs` 已经用 `TaskConflictActions` 把 `ConflictTask` 载荷的缺口补掉了
（`add` 里按 `task_id` 反查 `TaskRecord` 再 `tasks.add(record, true)`）。留着就是两份
`NameConflictQueue` 同名并存、13 条单测重复覆盖同一段逻辑。**删掉**，不留 `#[allow]`
式的将就。

> 教训记在这里：搬一个新模块之前，先 `grep -n 'pub struct <名字>' crates/*/src/*.rs`，
> 别只看进度笔记里的「Rust 侧还没有」——第 65 步 b 的笔记标题写的是「重名冲突提示」，
> 不翻内容看不出来它把服务层队列一起搬了。

### 逐条对齐（`speed_meter.py` 56 行 → `speed_meter.rs`）

| Python | Rust | 备注 |
|---|---|---|
| `__init__(coroutineRunner)`（:11-15） | `new()` / `Default` | 运行器退化成效果队列，无可存之物 |
| `currentSpeed` 属性（:17-19） | `current_speed()` | `&self` |
| `_bytes`（无公开读法） | `pending_bytes()` | 测试与诊断用 |
| `_tickWorkId is not None` | `is_running()` | |
| `start()`（:21-25） | `start() -> Vec<SpeedEffect>` | 已在跑 → 空效果；否则发 `StartTicking{work_id}` |
| `stop()`（:27-34） | `stop() -> Vec<SpeedEffect>` | `[Cancel, Emit(0)]`，**无条件**发 0 |
| `addSpeed(n)`（:36-37） | `add_speed(n)` | 不看「在不在跑」，照抄 |
| `waitForSpeedLimit()`（:39-41） | `should_wait(enabled, limit)` | 谓词化，见下 |
| `_tickLoop()`（:43-46） | `StartTicking` 效果 | 1 秒周期由宿主起 |
| `_tick()`（:48-53） | `tick() -> Vec<SpeedEffect>` | 先发布再清零；循环已停 → 空 |
| `_onTickFailed(err)`（:55-56） | `on_tick_failed()` | 只置空 workId，不清 `bytes` / `current_speed` |

### 关键决策

1. **字段全用 `Cell<T>`，方法分 `&self` / `&mut self` 两档**。`current_speed` /
   `pending_bytes` / `is_running` / `should_wait` 只要 `&self`——宿主在
   `while meter.should_wait(..)` 里持有的是不可变借用，改状态的四个方法
   （`start` / `stop` / `add_speed` / `tick` / `on_tick_failed`）才要 `&mut self`。
   `add_speed` 也走 `&mut self`：它虽然只碰一个 `Cell`，但语义上是写操作，
   保留 `&mut` 让「谁在累加」这件事在类型上可见。

2. **`_tickLoop` 摊成 `StartTicking{work_id}`，模块内不留计时器**。宿主拿到效果后起一个
   1 秒周期，每周期调 `tick()`；周期本身失败就调 `on_tick_failed()`。这与第 100~103 步
   「副作用摊成效果队列」的口径一致，也让单测不必等真实时间。

3. **`waitForSpeedLimit` 退化成谓词 `should_wait(enabled, limit_bytes)`**。
   Python 每轮重读 `cfg.isSpeedLimitEnabled.value` / `cfg.speedLimitation.value`；
   Rust 侧由调用方每轮读 `settings::read_bool(cfg, "isSpeedLimitEnabled")` /
   `settings::read_i64(cfg, "speedLimitation")` 再传进来——**等价于每轮重读，且模块不持有
   配置快照**。宿主的口径是 `while meter.should_wait(e, l) { sleep(0.1) }`。

4. **`should_wait` 是严格大于**（`bytes > limit`，等于上限不算超），照抄 `:40`。

5. **`stop()` 无条件 `Emit(0)`**：即使 `current_speed` 本来就是 0 也发——界面靠这一发把
   数字抹掉（单测 `stop_emits_zero_even_when_the_speed_is_already_zero`）。

6. **workId 由服务发号**（`speed-1` / `speed-2` …，一个只增不减的 `issued`）。
   与第 100 步 `TaskActions`（`run-1` / `seed-2`）、第 102 步 `RuntimeStatus`
   （`runtime-1`）同一形态：宿主把 workId 当不透明值，`Cancel` 时原样交回。

7. **`on_tick_failed` 不清 `bytes` / `current_speed`**（照抄 Python）：下一轮 `start()` 的
   第一次 `tick` 会把残留量当成速度发出去——这是 Python 的既有行为，照抄不修。

### 13 条单测

| 测试 | 钉住的行为 |
|---|---|
| `start_runs_the_tick_loop_once` | `start` → 恰好一条 `StartTicking{speed-1}` |
| `start_twice_is_a_no_op` | 第二次 `start` → 空（`:22` 判重） |
| `stop_cancels_then_resets_and_emits_zero` | 效果顺序 `[Cancel, Emit(0)]` + 双清零 |
| `stop_without_start_does_nothing` | 没在跑 → 空（`:28` 判空） |
| `stop_emits_zero_even_when_the_speed_is_already_zero` | 0 也要发 |
| `tick_publishes_the_accumulated_bytes_and_clears_them` | 10+5 → 发 15；下一轮从 0 重新累加（无滑动平均） |
| `tick_without_a_running_loop_is_ignored` | 迟到的 tick 不发，且**不清** `bytes` |
| `a_failed_tick_loop_lets_start_run_again` | `on_tick_failed` 后可重新 `start`，workId 递增 |
| `add_speed_accumulates_while_stopped` | 没在跑时累加的量会留给下一轮 |
| `should_wait_compares_strictly_greater_than_the_limit` | 等于上限不算超；关掉永不等待 |
| `should_wait_reads_the_pending_bytes_not_the_published_speed` | 闸门看本轮累加量，不看上一轮发布值 |
| `current_speed_is_zero_until_the_first_tick` | `addSpeed` 不发布 |
| `work_ids_are_never_reused` | 三轮 `start`/`stop` → `speed-1..3` |

### 验证

- `cargo fmt --all` + `--check`：通过（`FMT_CHECK_EXIT=0`）；
- `cargo test -p gd3-ui --lib`：**810 passed / 0 failed**（第 103 步 797 + 13；12m21s 编译 + 2.88s 跑完）；
- `cargo test --workspace`：**1313 passed / 0 failed**（28 段；第 103 步 1300 + 13）；
- `cargo clippy --workspace --all-targets -- -D warnings`：**0 warning**（4m44s，首跑即干净，`speed_meter.rs` 的 `Cell` 用法没触 lint）；
- Python 侧没改一行，不跑 Python 全量；
- `rust/README.md` 1131 → **1176 行**：新增「速度表（`gd3-ui::speed_meter`）」一节，
  含「为什么不搬 `name_conflict_queue.py`」小节。

### 下一步（第 105 步）

`SpeedMeter` 目前只是**可用的模块**，还没接回引擎。下一步把它接上：

- `Engine` 加 `speed_meter: SpeedMeter` 字段（`engine.py:20` 的构造顺序）；
- `_refreshSpeedMeter()`（`task_service.py:659-663`）→ `Engine::refresh_speed_meter()`：
  `if tasks.speed_meter_should_run() { meter.start() } else { meter.stop() }`，
  返回 `Vec<SpeedEffect>`。Python 侧只有**四个**调用点（实读 `grep -n`）：
  `stopSeeding`（:406）、`_seed`（:593）、`_onSeedingEnded`（:604）、`_pump`（:657）
  ——注意 `pause` / `resume` 里**没有**，它们只改状态；
- 引擎轨迹（`Trace`）里补 `construct:SpeedMeter`，与既有 `construct:NameConflictQueue` 同形。

## 第 105 步：把速度表接回 `TaskService`

> 时间：2026-10-18 22:05（`date '+%Y-%m-%d %H:%M'` 实取）

### 做了什么

第 104 步的 `SpeedMeter` 当时只是个能用的模块，没有任何调用方。这一步把它接进
`TaskService`，对齐 Python 的 `_refreshSpeedMeter()` 与四个调用点。

| Python | Rust |
|---|---|
| `TaskService.__init__(…, speedMeter, …)`（:314-320） | `TaskService` 新字段 `speed_meter: SpeedMeter` |
| `self._speedMeter.addSpeed`（:578 注入任务） | `TaskService::add_speed(n)` |
| `self._speedMeter.waitForSpeedLimit`（:578） | `TaskService::speed_meter_should_wait(enabled, limit)` |
| `_refreshSpeedMeter()`（:659-663） | 私有 `refresh_speed_meter()`：`should_run ? start() : stop()` |
| `stopSeeding` 末尾（:406） | `stop_seeding` 末尾 |
| `_seed` 末尾（:593） | `seed` 末尾（`start_seeding` 经 `seed` 覆盖） |
| `_onSeedingEnded` 末尾（:604） | `on_seeding_ended` 末尾 |
| `_pump` 末尾（:657） | `pump` 末尾 |
| `speedChanged.emit(...)` / `coroutineRunner.cancel(...)` | 攒进 `speed_effects`，宿主 `take_speed_effects()` 取走 |
| `_tick`（宿主每秒调） | `tick_speed_meter()` |
| `_onTickFailed` | `on_speed_tick_failed()` |
| `self._speedMeter.currentSpeed` | `speed_meter().current_speed()` |

**`pause` / `resume` 里没有 `_refreshSpeedMeter()`**（实读 `grep -n`：只有 :406 / :593 /
:604 / :657 四处）。这是容易搞错的地方——想当然会以为暂停就该停表，但 Python 的
`pause` 走 `_cancelWork` + `setStatus` + `_pump`，`_pump` 末尾那一发自然会算。

### 关键决策：效果进缓冲，而不是改四个返回类型

Python 的 `_refreshSpeedMeter()` 是四个方法的**尾语句**，而那四个方法在 Rust 侧都已经有
返回值：

| 方法 | 现有返回 |
|---|---|
| `stop_seeding` | `CancelWork` |
| `seed` | `Option<SeedingEvent>` |
| `on_seeding_ended` | `SeedingEvent` |
| `pump` | `Vec<String>` |

要把 `Vec<SpeedEffect>` 一起吐出来，就得给四个返回类型各包一层报告结构，再改
`engine.rs` 的四个包装（`Engine::pump` 的返回值还被引擎轨迹测试断言）以及
`task_actions` 里的调用点——改动面横跨三个文件、几十条既有断言，而语义收益为零。

所以改成**服务内缓冲**：`speed_effects: Vec<SpeedEffect>`，`refresh_speed_meter()` 把
`start()` / `stop()` 的效果 `extend` 进去，宿主在**本轮动作结束后**调
`take_speed_effects()` 一次性取走。这与 `pending_flush`（第 81 步把
`_flushSoon()` 的 200ms 防抖退化成标记）是同一手法：**保留调用点、把「什么时候取」交给
宿主**。

四个调用点因此与 Python **一字不差**，不需要调用方记得「pump 之后还要再调一次」。

### 9 条单测

| 测试 | 钉住的行为 |
|---|---|
| `pump_starts_the_speed_meter` | `pump` 派发了任务 → `[StartTicking{speed-1}]` |
| `pump_with_nothing_to_dispatch_leaves_the_meter_alone` | 没派发 → 无效果（`stop()` 在没跑时不吐） |
| `finishing_the_last_run_stops_the_meter` | `on_run_finished` → `pump` → 队列空 → `[Cancel, Emit(0)]` |
| `seeding_starts_and_stops_the_meter` | `seed` → start；`on_seeding_ended` → stop |
| `stop_seeding_refreshes_the_meter` | `stopSeeding` 是第三个调用点 |
| `add_speed_and_tick_flow_through_the_service` | `add_speed` 累加、`tick_speed_meter` 发布并清零 |
| `speed_meter_should_wait_delegates_to_the_meter` | 闸门谓词透传（等于上限不算超） |
| `take_speed_effects_drains_the_buffer` | 取一次就空 |
| `on_speed_tick_failed_lets_the_meter_start_again` | 失败后下一次 `pump` 发新 workId（`speed-2`） |

### 已知差异（照抄，不修）

- **`speed_meter_should_run` 的残留项**：`cancel_work` 的 `or` 短路缺陷（第 97 步记录）
  会让 `_seeding` 里留一条，于是速度表一直跑。Python 同病，Rust 照抄；
- **`stop()` 无条件 `Emit(0)`**：`pump` 里「队列本来就空」这一支不吐效果（因为 `stop()`
  在没跑时直接返回），与 Python 一致；
- **缓冲不清空语义**：宿主不取就一直攒。这与 `pending_flush` 一样，是「宿主必须消费」的
  契约，写在方法文档里。

### 验证

- `cargo fmt --all` + `--check`：通过；
- `cargo test -p gd3-ui --lib`：**819 passed / 0 failed**（第 104 步 810 + 9；8m27s 编译）；
- `cargo test --workspace`：**1322 passed / 0 failed**（28 段；第 104 步 1313 + 9）；
- `cargo clippy --workspace --all-targets -- -D warnings`：**0 warning**（4m23s，首跑即干净）；
- Python 侧没改一行。

### 下一步（第 106 步）

`take_speed_effects()` 目前没有真正的消费者：`Engine` 只暴露 `tasks_mut()`，宿主（未来的
Slint 主窗口 / `boot::run`）得自己记得取。下一步二选一：

1. **给 `Engine` 补 `drain_speed_effects()`**：把效果同时记进 `Trace`（`speed:start` /
   `speed:cancel` / `speed:emit:N`），并在 `Engine::pump` / 做种三个入口之后自动 drain——
   这样引擎轨迹测试就能钉住「谁在什么时候起表」；
2. 或者直接做 `app/services/browser_service.py`（597 行）——它需要 `gd3-net` 侧的
   sources 端口，是 Phase 8 里剩下最大的一块。

倾向 1：小、且能让第 104/105 步的成果在引擎层可见；`browser_service` 留到网络端口就位后。
## 第 106 步：`Engine` 侧收敛 + 速度表效果接线

> 时间：2026-10-18 22:56（`date '+%Y-%m-%d %H:%M'` 实取）

### 做了什么

第 105 步让 `TaskService` 持有速度表并攒效果，但 `Engine` 里还躺着第 83 步的一份**弱化版
`SpeedMeter`**（原 `engine.rs:151-214`：`start() -> ()`、`stop() -> Option<i64>`、没有
workId、没有 `should_wait`），而 `Engine::stop()` 调的正是它，效果直接丢弃。结果是**两份
状态机各说各话**：`Engine` 停了自己那份，`TaskService` 那份照跑。

这一步做四件事：

1. 删掉 `engine.rs` 里那份旧 `SpeedMeter`（-64 行），改
   `use crate::speed_meter::{SpeedEffect, SpeedMeter}`；
2. 删掉 `Engine.speed_meter` 字段 / 构造 / 初始化项；`Engine::speed_meter()` 改成委托
   `self.tasks.speed_meter()`；`Engine::speed_meter_mut()` **整个删掉**；
3. `TaskService` 补 `stop_speed_meter()`（`engine.py:44` 的 `self.speedMeter.stop()`）；
4. 新增 `Engine::drain_speed_effects()`，`Engine::pump()` / `Engine::stop()` 各调一次。

### 逐条对照

| Python | Rust |
|---|---|
| `self.speedMeter = SpeedMeter(coroutineRunner)`（`engine.py:20`） | 速度表归 `TaskService` 持有（第 105 步）；`construct:SpeedMeter` 轨迹标签保留 |
| `TaskService(…, self.speedMeter, …)`（:21-22） | `TaskService` 自己 `SpeedMeter::default()`；装配顺序不变 |
| `engine.speedMeter` | `Engine::speed_meter()` → `TaskService::speed_meter()` |
| `self.speedMeter.stop()`（:44） | `TaskService::stop_speed_meter()`（效果进缓冲） |
| `speedChanged` 的订阅者（`BrowserService` 等） | `Engine::drain_speed_effects()` |
| `_pump()` 末尾的 `_refreshSpeedMeter()` | `TaskService::pump` 里已有（第 105 步）；`Engine::pump` 末尾 drain |
| `speedChanged.emit(0)` 在停机时的落点 | `drain_speed_effects()` 记 `speed:cancel:<id>` + `speed:emit:0` |

### 关键决策

- **速度表只留一份，归 `TaskService` 所有**。Python 是 `Engine` 构造后注入、两个对象拿同一
  引用；Rust 侧只有 `TaskService` 会改它，所以「谁用谁持有」，`Engine` 只转出口。
- **删 `speed_meter_mut()` 是刻意的**：绕过 `add_speed` / `tick_speed_meter` /
  `stop_speed_meter` 直接改状态机，等于把 `speedChanged` 丢了。唯一使用者是
  `stop_order_mirrors_python_and_flushes_twice`，改成用 `pump()` 让表**真的**跑起来——
  比手工 `start()` 更接近真实路径。
- **drain 只挂在 `Engine::pump` / `Engine::stop`**。另外三个调用点（`stopSeeding` /
  `_seed` / `_onSeedingEnded`）走 `TaskActions`，宿主在那里 `take_speed_effects()`
  （第 100 步的效果队列口径）。没人取就攒着，下次 pump / stop 一并吐出，顺序不变。
- **`construct:SpeedMeter` 轨迹标签保留**：它钉的是「装配顺序」而不是「谁持有」；删掉会让
  `construction_order_mirrors_python_engine` 的期望与 `engine.py` 的构造顺序脱节。
- **删掉两条旧单测**（`speed_meter_tick_and_stop` /
  `speed_meter_stop_without_start_emits_nothing`）：它们测的是已删掉的弱化实现，同样的行为
  在 `speed_meter.rs` 的 13 条里都有，且更严（含 workId 与效果队列）。

### 2 条新单测

| 测试 | 钉住的行为 |
|---|---|
| `pump_records_the_speed_meter_start_in_the_trace` | `pump` 派发 → 轨迹 `speed:start:speed-1`；表在跑；缓冲随即为空 |
| `draining_an_idle_speed_meter_yields_nothing` | 空闲 drain 返回空且不记标签 |

`stop_order_mirrors_python_and_flushes_twice` 的期望轨迹插进
`speed:cancel:speed-1` / `speed:emit:0`，位置紧跟 `stop:speedMeter.stop`（与
`engine.py:44` 的调用位置一致，在 `browserListener.stop()` 之前）。

### 验证

- `cargo fmt --all` + `--check`：通过；
- `cargo test -p gd3-ui --lib`：**819 passed / 0 failed**（第 105 步 819 − 2 + 2；首跑即绿）；
- `cargo test --workspace`：**1322 passed / 0 failed**（28 段）；
- `cargo clippy --workspace --all-targets -- -D warnings`：**0 warning**；
- Python 侧没改一行。

### 遗留：`engine.rs` 还有一处同类重复

`engine.rs:161` 还留着 `pub struct RuntimeStatusService;` 的空壳桩，而第 102 步已经把整类
搬进 `gd3-ui::runtime_status`（`runtime_status.rs:145`，932 行 / 22 单测）。与速度表是同一个
坑：**只看进度笔记标题发现不了**，必须
`grep -n 'pub struct <名字>' crates/*/src/*.rs` 才能看到两份定义。

### 下一步（第 107 步）

二选一：

1. **`Engine.runtime_status` 换成 `crate::runtime_status::RuntimeStatusService`**——同这一步的
   收敛手法，但要改 `FeatureService` 桩里对它的调用（桩的方法签名与真实现不同）；
2. 回到 Phase 7 的 P0 主线（主窗口 + 任务卡片 + 抽屉）。

倾向 1：小、且与这一步是同一类债务，趁热清掉；否则后面接 `boot::run` 时会同时踩两个重复
实现。

## 第 107 步：`RuntimeStatusService` 收敛 + `matchIdentityPreset`

> 时间：2026-10-18 23:21（`date '+%Y-%m-%d %H:%M'` 实取）

### 做了什么

两件同一类的事：清掉第 106 步记下的重复定义；补上 `FeatureService.parse()` 的前置函数。

1. **`engine.rs` 删掉 `RuntimeStatusService` 空壳桩**（`pub struct RuntimeStatusService;`，
   第 83 步的产物），改成 `pub use crate::runtime_status::RuntimeStatusService;`。第 102 步
   已经把整类搬进 `runtime_status.rs`（932 行 / 22 单测），但 `Engine` 一直在用自己那份
   空壳——`runtime_status()` 返回的是零大小结构体，`stored_status()` / `install()` /
   `refresh_status()` 一个都调不到。补 `runtime_status_mut()` 作写入口
   （`setting_cards.py:804` 的 `RuntimeCard` 与 OOBE 向导是它的真消费者）。
2. **`identity.rs` 补 `match_identity_preset`**（`app/client.py:83-94`）：
   `FeatureService.parse()` 在 `options.clientProfile` 为空时按 host 找预设，把
   `clientProfile` / `userAgent` 补进 options。这是 `parse` 端口的前半段，纯函数、无依赖。

### 逐条对照

| Python | Rust |
|---|---|
| `matchIdentityPreset(host)`（`client.py:83`） | `identity::match_identity_preset(host, presets)` |
| `preset.get("isEnabled", True)` | `IdentityPreset.is_enabled`（第 43 步已按读盘口径建模） |
| `pattern.startswith("*.")` → `host == suffix or host.endswith("." + suffix)` | `host_matches` 的 `Some(suffix)` 分支 |
| `elif host == pattern` | `None` 分支 |
| `return None` | `Option<&IdentityPreset>` |

### 关键决策

- **返回 `&IdentityPreset` 而不是克隆**：Python 返回的是 `cfg.identityPresets` 里那个 dict，
  调用方只读两个字段（`clientProfile` / `userAgent`）；生命周期绑在 `presets` 上，要留就
  clone。
- **`presets` 作参数**：`cfg` 是模块级全局，Rust 侧没有响应式配置层，由调用方每轮现读再
  传入（同 `speed_meter::should_wait`、`settings::read_*`）。
- **不做 URL 解析**：`urlparse(options.url).hostname` 的取 host 留给调用方
  （`FeatureService.parse` 的移植里做）。`gd3-core` 里已有几处私有的「从 URL 抠 host」
  （`http_pack.rs` / `github.rs`），等搬 `parse` 时统一抽，不先造半吊子。
- **空模式串照抄**：`pattern == ""` 时 Python 走 `elif host == pattern`，于是只有空 host
  命中。不特判、不修正。

### 5 条新单测（`identity.rs`）

| 测试 | 钉住的行为 |
|---|---|
| `match_identity_preset_skips_disabled_entries` | `isEnabled=false` 整条跳过，命中后面那条 |
| `wildcard_matches_bare_suffix_and_subdomains_only` | `*.pcs.baidu.com` 命中裸后缀与多级子域，不命中 `xpcs.baidu.com` |
| `plain_patterns_are_exact_matches` | 非通配是精确匹配，不做后缀 |
| `first_hit_wins_not_the_most_specific` | 顺序优先，不是「最具体」 |
| `default_presets_match_baidu_pcs_hosts` | 缺省预设（第 43 步）与命中函数串起来 |

### 把「桩的边界」写清了

`featureService` / `browserService` / `aria2RpcService` 仍是桩，但这次把**为什么**写进
README 与模块头：三者共用一条 `parse` 端口（`featureService.parse` 被
`browser_service.py:496` 与 `aria2_rpc.py:218` 各调一次），默认实现是 **Python 插件**
（`features/` 下 `FeaturePack` 子类的 `pack.parsers` / `pack.pages()` / `pack.config` /
`taskCardClass`），Rust 无法加载；`gd3-core::loader`（第 87 步）只搬了「按依赖排序
manifest 列表」。这不是「还没做」，是**结构性不可移植**——除非将来定义一套 Rust 侧插件
ABI。这条边界值得写下来，否则后续每轮都会有人（包括我）重新怀疑这里漏了活。

### 验证

- `cargo fmt --all` + `--check`：通过；
- `cargo test -p gd3-ui --lib`：**824 passed / 0 failed**（第 106 步 819 + 5；首跑即绿）；
- `cargo test --workspace`：**1327 passed / 0 failed**（28 段）；
- `cargo clippy --workspace --all-targets -- -D warnings`：**本步单独跑的那次没跑完**。
  第一次跑到 `Checking gd3-ui`（`--test` target）时已耗时 9 分钟、CPU 只有 8%
  （机器 939 MB 内存 + 1.1 GB swap，编译在抖动），随后被停掉。**第 108 步的 clippy
  是整 workspace 一起跑的，覆盖了本步的改动**（本步只删了一个空壳桩 + 加了一个纯函数），
  结果记在第 108 步的笔记里。这是为省掉一次 10 分钟的重复编译而做的合并，不是漏检；
- Python 侧没改一行。

### 下一步（第 108 步）

`feature_service` 的**可移植内核**：

- `TaskParser` trait：`priority`（默认 100，`pack.py:42`）/ `matches` /
  `matches_passive`（默认转 `matches`，`pack.py:49-50`）/ `parse`；
- `ParserChain`：`register` 后按 `priority` **稳定**排序（Python 的 `list.sort` 稳定）、
  `parse` 先跑 `match_identity_preset` 前置再逐条 `matches`、无匹配 →
  `TaskError`（`raise ValueError(f"No parser matched: {options.url}")`）；
- pack / UI 部分（`pack.pages()`、`taskCardClass`、`pack.config.createRuntimeCard`）
  明确标 N/A，理由同上面那条边界。

它是 `browser_service.py:496` 与 `aria2_rpc.py:218` 的共同依赖，也是 Phase 8 剩下最大
两块（浏览器扩展服务 / aria2 RPC）的前置。

## 第 108 步：`gd3-core::url` 抽取 + `gd3-ui::feature_service` 内核

> 时间：2026-10-18 23:50（`date '+%Y-%m-%d %H:%M'` 实取）

### 做了什么

两件互为前提的事：把「从 URL 抠 host」抽成公共模块；用它把 `FeatureService.parse` 的
可移植部分搬过来（第 107 步笔记里许的那笔债）。

1. **新建 `gd3-core/src/url.rs`（179 行 / 4 单测）**：`pub fn host_of(url) -> String`
   是唯一 `pub` 项（`urlparse(url).hostname or ""` 的口径）；`ParsedUrl` /
   `parse_url` 保持 `pub(crate)`。`github.rs`（246 → 157 行）与 `http_pack.rs` 改成
   `use crate::url::parse_url;`，删掉各自那份 92 行私有实现。
2. **`hostname()` 按 CPython `_hostinfo` 重写**：旧实现用 `rfind(':')` + `is_ascii_digit`
   判断端口，`https://host:notaport/x` 会得到 `host:notaport`；CPython 是**第一个** `:`
   处切、**不校验**端口是不是数字。这是一处真 bug，不是风格差异。
3. **新建 `gd3-ui/src/feature_service.rs`（597 行 / 15 单测）**：`TaskParser` trait、
   `ParserChain`（稳定序 `register` + `parse` / `matches` / `matches_passive`）、
   `apply_identity_preset`、`FeatureService`。
4. **`engine.rs` 的 `FeatureService` 空壳桩删掉**（第 83 步产物），改
   `pub use crate::feature_service::FeatureService;`；`Engine::start()` / `stop()` 在
   原来的轨迹标签之后**真调** `activate()` / `deactivate()`（无 pack 时空转，但调用点
   落在 `engine.py:23` / `:44` 的位置上）；补 `features_mut()` 作注册解析器的写入口。

### 逐条对照

| Python | Rust |
|---|---|
| `urlparse(url).hostname or ""`（`feature_service.py:79`、`client.py:83`） | `gd3_core::url::host_of(url)` |
| `TaskParser.priority = 100`（`pack.py:42`） | `TaskParser::priority()` 默认返回 `100` |
| `matchPassive` 默认转 `match`（`pack.py:49-50`） | `matches_passive` 默认 `self.matches(options)` |
| `self._parsers.sort(key=lambda p: p.priority)`（`feature_service.py:61`） | `ParserChain::register` 插到第一个更大 priority 之前 |
| `parse` 的预设前置（:77-89） | `apply_identity_preset(options, presets)` |
| `raise ValueError(f"No parser matched: {options.url}")`（:88） | `ParseError::NoParserMatched { url }` → `TaskError::io(...)` |
| `match(url)` / `matchPassive(url)`（:91-99） | `FeatureService::matches` / `matches_passive` |
| `activate()` / `deactivate()`（:43-48 / :225-234） | 空转（没有 pack 就没有协程） |

### 关键决策

- **`host_of` 是唯一 `pub` 项**，`ParsedUrl` / `parse_url` 保持 `pub(crate)`：它们带着
  「path 被 lowercase」这个降级（Python 只 lowercase scheme 与 host），不该升级成公共契约。
  两个消费者（`http_pack` / `github`）都只做小写比较，降级写在文档注释里。
- **`hostname` 照 CPython 抄，不「修正」**：`https://host:notaport/x` → `host`；
  `ed2k://|file|x|1|h|/` → `|file|x|1|h|`（`_hostinfo` 不挑食）；IPv6 去 `[]`；
  userinfo 在**最后一个** `@` 切。`host_of_mirrors_urlparse_hostname` 的 20 组期望值是
  CPython 实取，不是手推。
- **`ParserChain::register` 用「插到第一个更大 priority 之前」**而不是 `sort_by_key`：
  语义与 Python 稳定 `list.sort` 一致，但每次 register 是 O(n) 而不是 O(n log n)。
- **`TaskParser::parse` 同步返回 `Result<TaskRecord, TaskError>`**（Python 是 `async`）：
  需要异步的实现在内部阻塞，同 `gd3-core::checksum` 的口径。
- **`ParseError::NoParserMatched` 经 `to_task_error()` 落成 `TaskError::io(...)`**：
  `TaskError::io(impl Display)` 会包成 `发生了意外错误：{detail}`，正是 Python
  `toTaskError(ValueError(...))` 的落点。
- **`apply_identity_preset` 就地改 `&mut Map`**：Python 的 `replace(options, **kwargs)`
  造新对象、原对象不变；Rust 侧调用方（`DraftEffect::Parse`）交出来的本来就是可变副本，
  语义等价，差异写进文档注释。
- **`matches` / `matches_passive` 只塞 `{"url": url}`**：现有八个解析器的 `match` 只读
  `options.url`（`features/*/pack.py` 实测），读配置留给调用方。
- **`is_truthy` 完整镜像 Python 真值判定**：预设里的 `clientProfile` 既可能是 `""` 也可能
  缺键，`0` / `false` / 空容器同样为假。
- **`activate()` 也补上空转方法**（不只是 `deactivate()`）：`Engine::start()` 里那个调用点
  在 Python 里是真实调用（:23），补一个空实现比留「只有轨迹标签」更接近原型；将来若有
  Rust 侧插件 ABI，落点已经在。

### 15 条新单测（`feature_service.rs`）

| 测试 | 钉住的行为 |
|---|---|
| `register_sorts_by_priority_and_keeps_registration_order` | 稳定序：同 priority 保持注册先后 |
| `parse_picks_the_lowest_priority_match` | 排序之后第一个 `matches` 的胜出 |
| `parse_skips_parsers_that_do_not_match` | 不匹配的解析器不参与 |
| `parse_without_a_match_reports_no_parser_matched` | 错误带 URL |
| `parser_errors_pass_through_untouched` | 解析器自己的 `TaskError` 不被包一层 |
| `matches_passive_falls_back_to_matches` | 默认实现转发 |
| `matches_and_matches_passive_are_independent` | 覆写后两者互不影响 |
| `match_only_gets_the_url_key` | options 里只有 `url` |
| `identity_preset_fills_an_empty_client_profile` | 空 `clientProfile` 被预设填上 |
| `identity_preset_leaves_an_explicit_profile_alone` | 已有值不覆盖 |
| `identity_preset_treats_null_as_empty` | `null` 与缺键同义 |
| `identity_preset_skips_unknown_hosts` | 未命中不动 options |
| `identity_preset_fills_nothing_when_the_preset_is_empty` | 预设两字段都空时不改动、返回 false |
| `parse_applies_the_preset_before_matching` | 前置真的发生在匹配之前（解析器看到的是补过的 options） |
| `a_feature_service_without_packs_has_an_empty_chain` | 没有 pack → 空链、`parse` 报 NoParserMatched |

### 4 条新单测（`url.rs`）

| 测试 | 钉住的行为 |
|---|---|
| `host_of_mirrors_urlparse_hostname` | 20 组 CPython 实取期望（含 `host:notaport`、IPv6、userinfo、`ed2k`） |
| `parse_url_splits_scheme_host_path` | scheme / host / path 三段，query 与 fragment 不进 path |
| `parse_url_lowercases_path_as_documented` | 文档化的降级 |
| `parse_url_rejects_illegal_schemes` | 非法 scheme → 整条当 path，host 为空 |

### 搬不了的部分（结构性 N/A）

`load()` / `loadPacks()`（要 `importlib` 加载 `features/` 下的 Python 子类）、
- `cargo fmt --all` + `--check`：通过（写完后先 fmt 再 check；`github.rs` 少了一行
  空行被 fmt 补掉）；
- `cargo test -p gd3-core --lib`：**311 passed / 0 failed**（第 107 步 307 + 4；
  `github` / `http_pack` 原有单测在 `hostname` 语义改动后仍全绿）；
- `cargo test -p gd3-ui --lib`：**839 passed / 0 failed**（824 + 15）；
- `cargo test --workspace`：**1346 passed / 0 failed**（28 段；1327 + 19）；
- `cargo clippy --workspace --all-targets -- -D warnings`：**0 warning**（6m15s）。
  这一次同时覆盖第 107 步的改动（第 107 步那次 clippy 没跑完，见其笔记的「验证」）；
- Python 侧没改一行。

编译耗时记录：本轮 `gd3-ui` 单 crate 编译 **约 13 分钟**（比前几轮慢 50%），
clippy 6m15s。机器 939 MB 内存 + swap 抖动是主因，不是代码规模问题。
卡片）、`register` 里的属性注入、`_registerAssociations`（Qt 信号）。
理由与第 107 步那条一样：**结构性不可移植，不是「还没做」**。`browserService` /
`aria2RpcService` 仍共用 `featureService.parse` 这条端口，它们能接上的前提是先有 pack。

### 验证

VERIFY_PLACEHOLDER

### 下一步（第 109 步）

`browser_service.py`（597 行）与 `aria2_rpc.py`（约 400 行）都卡在「需要 pack 才能
`parse`」上，但两者**协议层**（配对握手 / WebSocket 帧 / JSON-RPC 方法表）并不依赖 pack：

1. **`aria2_rpc.py`**：JSON-RPC 2.0 方法表（`addUri` / `pause` / `unpause` /
   `remove` / `tellStatus` / `getGlobalOption` / `changeGlobalOption`）+ 任务草稿请求，
   纯请求/响应，`gd3-net` 侧已有 HTTP 客户端；`parse` 端口可以注入成闭包，测试里给假的；
2. **`browser_service.py`**：配对码、token、帧格式（`PortListener` 那侧已在
   `gd3-servers`），但 597 行里有相当一部分是 Qt 托盘/对话框，得先分清哪半是协议。

倾向 1：接口面清楚、可测性高，且 `aria2` 开关（`GD3_RUST_ARIA2`）已经在引擎里留好了。
