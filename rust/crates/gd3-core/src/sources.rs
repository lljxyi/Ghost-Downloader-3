//! 更新源的纯逻辑层（第 114 步）：镜像 `app/sources.py`（295 行）里**不碰网络**的那半截。
//!
//! ## 为什么要单独一层
//!
//! `app/sources.py` 的结构是「双源（github / gitcode）+ 竞速」：
//! 每个 `fetch*` 先 `buildClient()` 起 HTTP，再在多个镜像之间
//! `asyncio.gather(..., return_exceptions=True)`（全跑完取最优）或
//! `asyncio.staggered.staggered_race(...)`（错开 0.05 秒起跑，第一个成功者胜）。
//! 网络与解包归宿主（第 114 步不动），本模块只搬三件事：
//!
//! 1. **端点表与 URL 拼接**：`SOURCES` / `SOURCE_ORDER` / `Repo.nameOn` /
//!    `buildDownloadUrl` / `buildRawUrl` / `buildPypiUrl` / `pypi_file_candidates`；
//! 2. **两个聚合口径**：`gather` 的「跳过失败项、取 `parseVersion` 最大者」与
//!    `staggered_race` 的「第一个成功者胜」；
//! 3. **六条失败文案**：Python 在 `sources.py` 里直接 `raise TaskError("…")`，
//!    这里摊成构造函数，宿主拿不到任何源时用它。
//!
//! `Release` / `ReleaseAsset` / `Release.fromResponse` 已经在 `gd3-ui::release_info`
//! （第 68 步，镜像 `app/update.py` 的 `assetScore` / `bestAsset` 时一起搬的），
//! 本模块不重复定义——`newest_by_version` 用闭包取版本串，两个 crate 都能用。
//!
//! ## 实证口径（2026-10-19，CPython 3.13 + 源码）
//!
//! - **`SOURCES` 的插入序是 `gitcode` 在前**（`sources.py:38`），而 `SOURCE_ORDER`
//!   是 `("github", "gitcode")`（`:54`）。`buildSources()` 按 `SOURCE_ORDER` 过滤，
//!   所以顺序恒为 github → gitcode，与 `SOURCES` 的书写顺序无关。
//! - **`nameOn("github")` 恒返回 `self.name`**，不看 `mirrors["github"]`；
//!   `mirrors` 里放 `github` 键是死数据（`app/update.py:9` 的 `APP_REPO` 就是这样）。
//! - **`nameOn` 的其它源走 `mirrors.get(source)`**，缺键 → `None` → 该源被
//!   `buildSources()` 跳过。
//! - **`buildPypiUrl` 的未知源回退到 PyPI 官方**（`PYPI_MIRRORS.get(source,
//!   PYPI_MIRRORS["github"])`），而 `buildDownloadUrl` / `buildRawUrl` 的未知源是
//!   `KeyError`（`SOURCES[source]`）——Rust 侧前者回退、后者返回 `None`。
//! - **`staggered_race` 全失败时返回 `(None, None, exceptions)`，不抛**（实测
//!   `[bad, bad]` → `(None, None, [RuntimeError, RuntimeError])`、`[]` →
//!   `(None, None, [])`）；`sources.py` 的 `if index is None: raise TaskError(...)`
//!   就是这条路径。赢家那一格是 `None`，**被取消的输家是 `CancelledError()`**
//!   （实测 `[slow, fast]` → `('fast', 1, [CancelledError(), None])`）。
//! - **`staggered_race` 的胜者不一定是下标最小的**：第一个协程还没失败、但已经
//!   过了 `delay` 秒时下一个也会起跑，谁先完成谁赢。所以 Rust 侧把「完成顺序」
//!   当入参（[`first_success`]），不在纯逻辑层模拟时序。
//! - **`fetchPypiReleases` 的合并方向是反的**：`for result in reversed(succeeded):
//!   releases |= result` —— 先合并**后**到的源，再合并靠前的源，于是
//!   `SOURCE_ORDER` 里靠前的 github 覆盖 gitcode。键序按首次插入位置保留
//!   （`serde_json` 开了 `preserve_order`，`Map` 是 `IndexMap`）。
//!
//! ## 与 Python 的已知差异
//!
//! - **未知源的 `KeyError` 退化成 `None`**：`buildDownloadUrl(repo, …, source="x")`
//!   在 Python 抛 `KeyError("x")`；Rust 返回 `None`。调用方只可能从
//!   `Repo::build_sources()` 取值，两者不可达。
//! - **`mirrors` 用 `Vec` 而不是 `Map`**：Python 的 dict 保插入序、重复键后者覆盖，
//!   [`Repo::name_on`] 用 `rev().find()` 复刻「后者覆盖」。
//! - **`merge_pypi_releases` 的失败值类型是泛型**：Python 的 `return_exceptions=True`
//!   会把任意 `BaseException` 塞进结果表，Rust 侧只关心「是不是 Err」。

use serde_json::{Map, Value};

use crate::task_error::TaskError;
use crate::version::parse_version;

/// 一个镜像源的四个端点（`sources.py:31-35`，逐字）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceEndpoints {
    pub api: &'static str,
    pub download: &'static str,
    pub raw: &'static str,
    pub raw_infix: &'static str,
}

/// `SOURCES`（`sources.py:38-51`）——**按 Python dict 的插入序**：`gitcode` 在前。
pub const SOURCES: [(&str, SourceEndpoints); 2] = [
    (
        "gitcode",
        SourceEndpoints {
            api: "https://api.gitcode.com/api/v5/repos",
            download: "https://gitcode.com",
            raw: "https://cnb.cool",
            raw_infix: "/-/git/raw/",
        },
    ),
    (
        "github",
        SourceEndpoints {
            api: "https://api.github.com/repos",
            download: "https://github.com",
            raw: "https://raw.githubusercontent.com",
            raw_infix: "/",
        },
    ),
];

/// `SOURCE_ORDER`（`sources.py:53`）——尝试顺序，与 [`SOURCES`] 的书写顺序不同。
pub const SOURCE_ORDER: [&str; 2] = ["github", "gitcode"];

/// `STAGGER_DELAY`（`sources.py:54`）：错开起跑的间隔（秒）。
pub const STAGGER_DELAY: f64 = 0.05;

/// `SOURCES[source]`；未知源 → `None`（Python 是 `KeyError`）。
pub fn endpoints(source: &str) -> Option<&'static SourceEndpoints> {
    SOURCES
        .iter()
        .find(|(name, _)| *name == source)
        .map(|(_, endpoints)| endpoints)
}

/// 一个仓库在多个镜像上的坐标（`sources.py:17-27`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repo {
    /// 官方（github）坐标，`owner/name`。
    pub name: String,
    /// 镜像坐标表；重复键**后者覆盖**（同 Python dict）。
    pub mirrors: Vec<(String, String)>,
}

impl Repo {
    pub fn new(name: impl Into<String>, mirrors: Vec<(String, String)>) -> Self {
        Self {
            name: name.into(),
            mirrors,
        }
    }

    /// 镜像上的坐标；`github` 恒为 [`Self::name`]，其它源查 [`Self::mirrors`]。
    pub fn name_on(&self, source: &str) -> Option<&str> {
        if source == "github" {
            return Some(&self.name);
        }
        self.mirrors
            .iter()
            .rev()
            .find(|(key, _)| key == source)
            .map(|(_, value)| value.as_str())
    }

    /// `[s for s in SOURCE_ORDER if nameOn(s) is not None]`（`sources.py:26-27`）。
    pub fn build_sources(&self) -> Vec<&'static str> {
        SOURCE_ORDER
            .into_iter()
            .filter(|source| self.name_on(source).is_some())
            .collect()
    }
}

/// `app/update.py:9` 的 `APP_REPO`——`mirrors` 里那个 `github` 键是死数据（见模块头）。
pub fn app_repo() -> Repo {
    Repo::new(
        "XiaoYouChR/Ghost-Downloader-3",
        vec![(
            "gitcode".to_string(),
            "XiaoYouChR/Ghost-Downloader-3".to_string(),
        )],
    )
}

/// `PYPI_MIRRORS`（`sources.py:226-229`）。
pub const PYPI_MIRRORS: [(&str, &str); 2] = [
    ("gitcode", "https://mirrors.bfsu.edu.cn/pypi/web/json"),
    ("github", "https://pypi.org/pypi"),
];
/// `PYPI_FILES`（`sources.py:230`）。
pub const PYPI_FILES: &str = "https://files.pythonhosted.org/packages/";
/// `PYPI_FILES_MIRROR`（`sources.py:231`）。
pub const PYPI_FILES_MIRROR: &str = "https://mirrors.bfsu.edu.cn/pypi/web/packages/";

/// `buildDownloadUrl`（`sources.py:288-290`）；未知源 → `None`。
pub fn build_download_url(repo: &Repo, tag: &str, asset: &str, source: &str) -> Option<String> {
    let endpoints = endpoints(source)?;
    let name = repo.name_on(source)?;
    Some(format!(
        "{}/{name}/releases/download/{tag}/{asset}",
        endpoints.download
    ))
}

/// `buildRawUrl`（`sources.py:293-295`）；未知源 → `None`。
pub fn build_raw_url(repo: &Repo, branch: &str, path: &str, source: &str) -> Option<String> {
    let endpoints = endpoints(source)?;
    let name = repo.name_on(source)?;
    Some(format!(
        "{}/{name}{}{branch}/{path}",
        endpoints.raw, endpoints.raw_infix
    ))
}

/// `buildPypiUrl`（`sources.py:234-238`）：未知源**回退到 PyPI 官方**；
/// `gitcode` 走镜像的 `/json` 端点（base 本身已带 `json`，所以是 `<base>/<package>`）。
pub fn build_pypi_url(package: &str, source: &str) -> String {
    let base = PYPI_MIRRORS
        .iter()
        .find(|(name, _)| *name == source)
        .map(|(_, base)| *base)
        .unwrap_or("https://pypi.org/pypi");
    if source == "gitcode" {
        format!("{base}/{package}")
    } else {
        format!("{base}/{package}/json")
    }
}

/// `fetchPypiFile` 的候选表（`sources.py:281`）：只有 `pythonhosted` 的地址
/// 才会追加镜像候选，且 `str.replace(..., 1)` 只替换**第一处**。
pub fn pypi_file_candidates(url: &str) -> Vec<String> {
    if url.starts_with(PYPI_FILES) {
        vec![
            url.to_string(),
            url.replacen(PYPI_FILES, PYPI_FILES_MIRROR, 1),
        ]
    } else {
        vec![url.to_string()]
    }
}

// ---------------------------------------------------------------------------
// 六条失败文案（Python 在 `sources.py` 里直接 `raise TaskError(...)`）
// ---------------------------------------------------------------------------

/// `TaskError("无法获取 {name} 的最新 release", name=repo.name)`（`:135`）。
pub fn latest_release_failure(repo_name: &str) -> TaskError {
    TaskError::new(
        "无法获取 {name} 的最新 release",
        vec![("name", repo_name.to_string())],
    )
}

/// `TaskError("无法获取 {name}/{branch}/{path}", …)`（`:160`）。
pub fn json_failure(repo_name: &str, branch: &str, path: &str) -> TaskError {
    TaskError::new(
        "无法获取 {name}/{branch}/{path}",
        vec![
            ("name", repo_name.to_string()),
            ("branch", branch.to_string()),
            ("path", path.to_string()),
        ],
    )
}

/// `TaskError("无法下载 {name}/{branch}/{path}", …)`（`:187`）。
pub fn raw_file_failure(repo_name: &str, branch: &str, path: &str) -> TaskError {
    TaskError::new(
        "无法下载 {name}/{branch}/{path}",
        vec![
            ("name", repo_name.to_string()),
            ("branch", branch.to_string()),
            ("path", path.to_string()),
        ],
    )
}

/// `TaskError("无法下载 {name}/{tag}/{asset}", …)`（`:213`）。
pub fn download_failure(repo_name: &str, tag: &str, asset: &str) -> TaskError {
    TaskError::new(
        "无法下载 {name}/{tag}/{asset}",
        vec![
            ("name", repo_name.to_string()),
            ("tag", tag.to_string()),
            ("asset", asset.to_string()),
        ],
    )
}

/// `TaskError("无法获取 PyPI 包信息: {package}", …)`（`:258`）。
pub fn pypi_releases_failure(package: &str) -> TaskError {
    TaskError::new(
        "无法获取 PyPI 包信息: {package}",
        vec![("package", package.to_string())],
    )
}

/// `TaskError("无法下载 {url}", url=url)`（`:284`）。
pub fn pypi_file_failure(url: &str) -> TaskError {
    TaskError::new("无法下载 {url}", vec![("url", url.to_string())])
}

// ---------------------------------------------------------------------------
// 两个聚合口径
// ---------------------------------------------------------------------------

/// `staggered_race` 的挑选口径：`completions` 按**完成顺序**给出 `(下标, 结果)`，
/// 第一个 `Ok` 胜出；全部失败 → `None`（Python 返回 `(None, None, exceptions)`，
/// `sources.py` 随后 `raise TaskError(...)`）。
///
/// `STAGGER_DELAY` 只影响起跑时刻，不影响这条规则——所以纯逻辑层把它当入参。
pub fn first_success<T, E>(completions: Vec<(usize, Result<T, E>)>) -> Option<(usize, T)> {
    completions
        .into_iter()
        .find_map(|(index, result)| result.ok().map(|value| (index, value)))
}

/// `asyncio.gather(..., return_exceptions=True)` + 「取 `parseVersion` 最大者、
/// 同分取**先出现**者」（`sources.py:126-134`）。
///
/// 失败项直接跳过；全部失败 → `None`（Python 随后 `raise TaskError(...)`）。
pub fn newest_by_version<T, E>(
    results: Vec<Result<T, E>>,
    version_of: impl Fn(&T) -> &str,
) -> Option<T> {
    let mut chosen: Option<T> = None;
    for result in results {
        let Ok(candidate) = result else { continue };
        match &chosen {
            None => chosen = Some(candidate),
            Some(current) => {
                if parse_version(version_of(&candidate)) > parse_version(version_of(current)) {
                    chosen = Some(candidate);
                }
            }
        }
    }
    chosen
}

/// `fetchPypiReleases` 的合并（`sources.py:256-262`）：
/// `for result in reversed(succeeded): releases |= result`。
///
/// 先到的源后合并 → `SOURCE_ORDER` 里靠前的源**覆盖**靠后的源；键序按首次插入
/// 位置保留（`Map` 是 `IndexMap`）。全部失败 → `None`。
pub fn merge_pypi_releases<E>(
    results: Vec<Result<Map<String, Value>, E>>,
) -> Option<Map<String, Value>> {
    let succeeded: Vec<Map<String, Value>> = results.into_iter().filter_map(Result::ok).collect();
    if succeeded.is_empty() {
        return None;
    }
    let mut releases: Map<String, Value> = Map::new();
    for result in succeeded.into_iter().rev() {
        for (key, value) in result {
            releases.insert(key, value);
        }
    }
    Some(releases)
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    fn repo() -> Repo {
        app_repo()
    }

    /// `(版本串, 来源标记)`——用来验证「同分取先出现者」。
    fn versioned(version: &str, tag: &'static str) -> (String, &'static str) {
        (version.to_string(), tag)
    }

    fn version_of<'a>(item: &'a (String, &'static str)) -> &'a str {
        item.0.as_str()
    }

    #[test]
    fn source_order_differs_from_the_dict_order() {
        // `SOURCES` 按 `sources.py:38` 的书写顺序（gitcode 在前），
        // `SOURCE_ORDER` 才是尝试顺序（github 在前）。
        assert_eq!(SOURCES[0].0, "gitcode");
        assert_eq!(SOURCES[1].0, "github");
        assert_eq!(SOURCE_ORDER, ["github", "gitcode"]);
    }

    #[test]
    fn the_endpoint_table_is_mirrored_verbatim() {
        let gitcode = endpoints("gitcode").expect("gitcode 在表里");
        assert_eq!(gitcode.api, "https://api.gitcode.com/api/v5/repos");
        assert_eq!(gitcode.download, "https://gitcode.com");
        assert_eq!(gitcode.raw, "https://cnb.cool");
        assert_eq!(gitcode.raw_infix, "/-/git/raw/");

        let github = endpoints("github").expect("github 在表里");
        assert_eq!(github.api, "https://api.github.com/repos");
        assert_eq!(github.download, "https://github.com");
        assert_eq!(github.raw, "https://raw.githubusercontent.com");
        assert_eq!(github.raw_infix, "/");

        assert_eq!(STAGGER_DELAY, 0.05);
    }

    #[test]
    fn github_ignores_the_mirror_entry() {
        // `nameOn("github")` 恒返回 `self.name`，`mirrors["github"]` 是死数据。
        let repo = Repo::new(
            "owner/name",
            vec![("github".to_string(), "dead/data".to_string())],
        );
        assert_eq!(repo.name_on("github"), Some("owner/name"));
        assert_eq!(repo.build_sources(), ["github"]);
    }

    #[test]
    fn a_missing_mirror_drops_the_source() {
        let repo = Repo::new("owner/name", Vec::new());
        assert_eq!(repo.build_sources(), ["github"]);
        assert_eq!(repo.name_on("gitcode"), None);

        let mirrored = Repo::new(
            "owner/name",
            vec![("gitcode".to_string(), "mirror/name".to_string())],
        );
        assert_eq!(mirrored.build_sources(), ["github", "gitcode"]);
        assert_eq!(mirrored.name_on("gitcode"), Some("mirror/name"));
    }

    #[test]
    fn an_unknown_mirror_key_is_carried_but_never_tried() {
        // `nameOn("other")` 能取到值，但 `SOURCE_ORDER` 里没有它 → 不参与尝试。
        let repo = Repo::new("owner/name", vec![("other".to_string(), "z/z".to_string())]);
        assert_eq!(repo.name_on("other"), Some("z/z"));
        assert_eq!(repo.build_sources(), ["github"]);
    }

    #[test]
    fn a_duplicate_mirror_key_keeps_the_last_value() {
        let repo = Repo::new(
            "owner/name",
            vec![
                ("gitcode".to_string(), "first/name".to_string()),
                ("gitcode".to_string(), "second/name".to_string()),
            ],
        );
        assert_eq!(repo.name_on("gitcode"), Some("second/name"));
    }

    #[test]
    fn download_urls_match_the_python_strings() {
        let repo = repo();
        assert_eq!(
            build_download_url(&repo, "v4.4.0", "app.exe", "github").as_deref(),
            Some(
                "https://github.com/XiaoYouChR/Ghost-Downloader-3/releases/download/v4.4.0/app.exe"
            )
        );
        assert_eq!(
            build_download_url(&repo, "v4.4.0", "app.exe", "gitcode").as_deref(),
            Some(
                "https://gitcode.com/XiaoYouChR/Ghost-Downloader-3/releases/download/v4.4.0/app.exe"
            )
        );
    }

    #[test]
    fn raw_urls_match_the_python_strings() {
        let repo = repo();
        assert_eq!(
            build_raw_url(&repo, "main", "a/b.json", "github").as_deref(),
            Some("https://raw.githubusercontent.com/XiaoYouChR/Ghost-Downloader-3/main/a/b.json")
        );
        assert_eq!(
            build_raw_url(&repo, "main", "a/b.json", "gitcode").as_deref(),
            Some("https://cnb.cool/XiaoYouChR/Ghost-Downloader-3/-/git/raw/main/a/b.json")
        );
    }

    #[test]
    fn an_unknown_source_has_no_endpoints() {
        let repo = repo();
        assert_eq!(endpoints("nope"), None);
        assert_eq!(build_download_url(&repo, "v1", "a", "nope"), None);
        assert_eq!(build_raw_url(&repo, "main", "a", "nope"), None);
    }

    #[test]
    fn pypi_urls_fall_back_to_the_official_index() {
        // `PYPI_MIRRORS.get(source, PYPI_MIRRORS["github"])`：未知源不报错。
        assert_eq!(
            build_pypi_url("foo", "github"),
            "https://pypi.org/pypi/foo/json"
        );
        assert_eq!(
            build_pypi_url("foo", "gitcode"),
            "https://mirrors.bfsu.edu.cn/pypi/web/json/foo"
        );
        assert_eq!(
            build_pypi_url("foo", "nope"),
            "https://pypi.org/pypi/foo/json"
        );
    }

    #[test]
    fn pypi_file_candidates_only_mirror_pythonhosted_urls() {
        let hosted = "https://files.pythonhosted.org/packages/ab/cd/foo-1.0-py3-none-any.whl";
        assert_eq!(
            pypi_file_candidates(hosted),
            vec![
                hosted.to_string(),
                "https://mirrors.bfsu.edu.cn/pypi/web/packages/ab/cd/foo-1.0-py3-none-any.whl"
                    .to_string(),
            ]
        );

        let other = "https://example.invalid/foo.whl";
        assert_eq!(pypi_file_candidates(other), vec![other.to_string()]);
    }

    #[test]
    fn the_app_repo_mirrors_update_py() {
        let repo = repo();
        assert_eq!(repo.name, "XiaoYouChR/Ghost-Downloader-3");
        assert_eq!(repo.mirrors.len(), 1);
        assert_eq!(
            repo.name_on("gitcode"),
            Some("XiaoYouChR/Ghost-Downloader-3")
        );
        assert_eq!(repo.build_sources(), ["github", "gitcode"]);
    }

    #[test]
    fn newest_by_version_skips_failures() {
        let results: Vec<Result<(String, &'static str), &'static str>> = vec![
            Err("github 挂了"),
            Ok(versioned("v4.3.0", "gitcode")),
            Ok(versioned("4.4.0", "third")),
        ];
        let chosen = newest_by_version(results, version_of).expect("至少一个源成功");
        assert_eq!(chosen.1, "third");
    }

    #[test]
    fn newest_by_version_keeps_the_first_on_a_tie() {
        // `parseVersion(result.version) > parseVersion(chosen.version)` 是严格大于。
        let results: Vec<Result<(String, &'static str), &'static str>> = vec![
            Ok(versioned("v4.4.0", "first")),
            Ok(versioned("4.4", "second")),
        ];
        let chosen = newest_by_version(results, version_of).expect("至少一个源成功");
        assert_eq!(chosen.1, "first");
    }

    #[test]
    fn newest_by_version_returns_none_when_every_source_fails() {
        let results: Vec<Result<(String, &'static str), &'static str>> = vec![Err("a"), Err("b")];
        assert!(newest_by_version(results, version_of).is_none());
        assert!(
            newest_by_version(
                Vec::<Result<(String, &'static str), &'static str>>::new(),
                version_of
            )
            .is_none()
        );
    }

    #[test]
    fn first_success_returns_the_first_completion() {
        // 实测 `staggered_race([slow, fast], 0.01)` → `('fast', 1, …)`：
        // 胜者是先完成的下标 1，不是下标 0。
        let completions = vec![(1, Ok("fast")), (0, Err("slow 被取消"))];
        assert_eq!(first_success(completions), Some((1, "fast")));
    }

    #[test]
    fn first_success_returns_none_when_every_source_fails() {
        let completions: Vec<(usize, Result<&str, &str>)> = vec![(0, Err("a")), (1, Err("b"))];
        assert_eq!(first_success(completions), None);
        assert_eq!(
            first_success(Vec::<(usize, Result<&str, &str>)>::new()),
            None
        );
    }

    #[test]
    fn merge_pypi_releases_lets_the_earlier_source_win() {
        // 实测 CPython：`reversed(succeeded)` 逐个 `|=` → 先合并 gitcode 再合并 github，
        // 键序 `['1.0', '2.0', '3.0']`，`2.0` 取 github 的值。
        let github = json!({"2.0": ["gh"], "3.0": ["gh3"]});
        let gitcode = json!({"1.0": ["gc1"], "2.0": ["gc2"]});
        let results: Vec<Result<Map<String, Value>, &'static str>> = vec![
            Ok(github.as_object().expect("对象").clone()),
            Ok(gitcode.as_object().expect("对象").clone()),
        ];
        let merged = merge_pypi_releases(results).expect("至少一个源成功");
        assert_eq!(merged.keys().collect::<Vec<_>>(), ["1.0", "2.0", "3.0"]);
        assert_eq!(merged["2.0"], json!(["gh"]));
    }

    #[test]
    fn merge_pypi_releases_returns_none_when_every_source_fails() {
        let results: Vec<Result<Map<String, Value>, &'static str>> = vec![Err("a"), Err("b")];
        assert!(merge_pypi_releases(results).is_none());
    }

    #[test]
    fn failure_messages_match_the_python_wording() {
        assert_eq!(
            latest_release_failure("owner/name").format(),
            "无法获取 owner/name 的最新 release"
        );
        assert_eq!(
            json_failure("owner/name", "main", "a/b.json").format(),
            "无法获取 owner/name/main/a/b.json"
        );
        assert_eq!(
            raw_file_failure("owner/name", "main", "a/b.json").format(),
            "无法下载 owner/name/main/a/b.json"
        );
        assert_eq!(
            download_failure("owner/name", "v4.4.0", "app.exe").format(),
            "无法下载 owner/name/v4.4.0/app.exe"
        );
        assert_eq!(
            pypi_releases_failure("foo").format(),
            "无法获取 PyPI 包信息: foo"
        );
        assert_eq!(
            pypi_file_failure("https://example.invalid/foo.whl").format(),
            "无法下载 https://example.invalid/foo.whl"
        );
    }
}
