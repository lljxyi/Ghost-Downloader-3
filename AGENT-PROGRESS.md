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
