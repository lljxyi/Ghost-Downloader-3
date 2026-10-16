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
