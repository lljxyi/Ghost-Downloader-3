//! 下载分类规则镜像层（cfg.py Category/CategoryRules + CategoryService；
//! 第 44 步）。
//!
//! 实证：
//! - `categoryRules = ConfigItem("Category", "CategoryRules", [],
//!   CategoryListValidator(), JsonConfigSerializer(list, list))`
//!   （cfg.py:262-265）；
//! - `CategoryService._load`（category_service.py:136-144）：raw 为空
//!   （缺键/`[]`/序列化失败 fallback `[]`）→ CATEGORY_PRESETS 八预设
//!   （经 Category.fromDict 归一化）并立即 _save 写回；非空 → 逐项
//!   fromDict（与 identity 读盘同口径：不过 validator.correct）；
//! - `Category.fromDict`（category_service.py:112-131）容错归一化：
//!   extensions 逐项 str→strip→去点→小写→去重；icon 缺省 DOCUMENT；
//!   name 缺省 ""；folder 空串/缺省 → None；categoryId 缺省生成
//!   `cat_{uuid4().hex}`（读侧不生成随机 ID，保留空串由写侧处理，
//!   不复刻随机性）；
//! - 行摘要 `CategoryRowWidget._toSummary`：0 项「无扩展名」；
//!   ≤4 项 ", ".join 全部；>4 项「{前4} 等 {N} 项」。
//!
//! 写路径（第 49 步）：`rules_to_json` 镜像 `_save()` 的落盘串
//! （`json.dumps([asdict(c) …], ensure_ascii=False)`——键序即 dataclass
//! 字段序、分隔符 `", "` / `": "`），`correct_rule_list` 镜像
//! `CategoryListValidator.correct`，`add/update/remove/reorder/reset`
//! 镜像 `CategoryService` 的五个写方法；`draft_to_rule` 由真
//! `CategoryEditDialog.category()` 生成的金文件逐条对比。
//!
//! 匹配与输出目录解析（matchByName/categoryOf/outputFolderOf）随
//! Phase 8 任务造/引擎桥接并轨；cfg 写与 categoriesChanged 重推归调用方。

use crate::pystr::py_strip;
use gd3_core::config::py_json_string;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CategoryRule {
    pub category_id: String,
    pub name: String,
    pub icon: String,
    pub extensions: Vec<String>,
    pub folder: Option<String>,
}

/// category_service.py:16-99 CATEGORY_PRESETS 逐字镜像（fromDict 归一
/// 化后与源一致：预设扩展名已为小写去重点）。
fn default_rules() -> Vec<CategoryRule> {
    vec![
        CategoryRule {
            category_id: "cat_video".into(),
            name: "视频".into(),
            icon: "VIDEO".into(),
            folder: Some("{default}/Video".into()),
            extensions: [
                "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "rmvb", "rm", "mpg",
                "mpeg", "mpe", "mpa", "3gp", "ts", "m2ts", "ogv", "asf", "qt",
            ]
            .map(str::to_string)
            .to_vec(),
        },
        CategoryRule {
            category_id: "cat_audio".into(),
            name: "音频".into(),
            icon: "MUSIC".into(),
            folder: Some("{default}/Audio".into()),
            extensions: [
                "mp3", "flac", "wav", "aac", "ogg", "m4a", "wma", "ape", "opus", "mid", "ra", "aif",
            ]
            .map(str::to_string)
            .to_vec(),
        },
        CategoryRule {
            category_id: "cat_image".into(),
            name: "图片".into(),
            icon: "PHOTO".into(),
            folder: Some("{default}/Images".into()),
            extensions: [
                "jpg", "jpeg", "png", "gif", "bmp", "webp", "avif", "svg", "tif", "tiff", "ico",
                "heic", "heif",
            ]
            .map(str::to_string)
            .to_vec(),
        },
        CategoryRule {
            category_id: "cat_subtitle".into(),
            name: "字幕".into(),
            icon: "CHAT".into(),
            folder: Some("{default}/Subtitles".into()),
            extensions: [
                "srt", "ass", "ssa", "sub", "sup", "idx", "vtt", "lrc", "smi", "psb",
            ]
            .map(str::to_string)
            .to_vec(),
        },
        CategoryRule {
            category_id: "cat_document".into(),
            name: "文档".into(),
            icon: "DOCUMENT".into(),
            folder: Some("{default}/Documents".into()),
            extensions: [
                "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "epub", "mobi", "azw3",
                "rtf", "odt", "ods", "odp", "md", "csv", "nfo", "chm",
            ]
            .map(str::to_string)
            .to_vec(),
        },
        CategoryRule {
            category_id: "cat_archive".into(),
            name: "压缩包".into(),
            icon: "ZIP_FOLDER".into(),
            folder: Some("{default}/Archives".into()),
            extensions: [
                "zip", "rar", "7z", "tar", "gz", "gzip", "bz2", "xz", "tgz", "tbz2", "zst", "ace",
                "arj", "cab", "lzh", "sea", "sit", "sitx", "z", "001", "tar.gz", "tar.bz2",
                "tar.xz", "tar.zst",
            ]
            .map(str::to_string)
            .to_vec(),
        },
        CategoryRule {
            category_id: "cat_program".into(),
            name: "程序".into(),
            icon: "APPLICATION".into(),
            folder: Some("{default}/Programs".into()),
            extensions: [
                "exe", "msi", "msu", "msp", "apk", "apks", "apkm", "dmg", "pkg", "deb", "rpm",
                "appimage", "iso", "img", "esd", "wim", "bin", "jar", "bat", "sh", "com",
            ]
            .map(str::to_string)
            .to_vec(),
        },
        CategoryRule {
            category_id: "cat_other".into(),
            name: "其他".into(),
            icon: "HELP".into(),
            folder: None,
            extensions: Vec::new(),
        },
    ]
}

/// CategoryEditDialog.ICON_CHOICES 逐字（category_edit.py:17-23）。
pub const ICON_CHOICES: [&str; 25] = [
    "DOCUMENT",
    "MUSIC",
    "VIDEO",
    "ZIP_FOLDER",
    "APPLICATION",
    "LIBRARY",
    "ALBUM",
    "PHOTO",
    "MOVIE",
    "MEDIA",
    "GAME",
    "CODE",
    "EDUCATION",
    "LANGUAGE",
    "BRUSH",
    "FOLDER",
    "CHAT",
    "MAIL",
    "PRINT",
    "GLOBE",
    "CAMERA",
    "IMAGE_EXPORT",
    "MUSIC_FOLDER",
    "MARKET",
    "HELP",
];

/// 单项 fromDict 容错归一化镜像（判空与缺省口径见模块注释）。
///
/// 已知偏差：Python 是 `str(ext).strip()`，非字符串项会被 `str()` 化后
/// 留下（`123` → `"123"`、`None` → `"none"`）；Rust 侧按空串丢弃。磁盘
/// 格式由本侧写入时恒为字符串，手改配置文件才会碰到这个岔口。
/// `strip()` 走 CPython 口径（`\x1c`-`\x1f` 算空白），见 `pystr::py_strip`。
fn from_dict(item: &serde_json::Value) -> CategoryRule {
    let extensions = item
        .get("extensions")
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            let mut list: Vec<String> = Vec::new();
            for ext in arr {
                let mut normalized = py_strip(ext.as_str().unwrap_or("")).to_string();
                while normalized.starts_with('.') {
                    normalized.remove(0);
                }
                let normalized = normalized.to_lowercase();
                if !normalized.is_empty() && !list.contains(&normalized) {
                    list.push(normalized);
                }
            }
            list
        })
        .unwrap_or_default();
    let folder = item
        .get("folder")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    CategoryRule {
        category_id: item
            .get("categoryId")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string(),
        name: item
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string(),
        icon: item
            .get("icon")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or("DOCUMENT")
            .to_string(),
        extensions,
        folder,
    }
}

/// 镜像 CategoryService._load：缺键/空串/`[]`/非 JSON → 八预设；
/// 否则逐项 fromDict（文件名 i18n 显示走翻译层，镜像仅名字面量；
/// 行删除/重置的 cfg 回写归调用方）。
pub fn parse_rules(raw: Option<&str>) -> Vec<CategoryRule> {
    let Some(raw) = raw else {
        return default_rules();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return default_rules();
    };
    let Some(items) = value.as_array() else {
        return default_rules();
    };
    if items.is_empty() {
        return default_rules();
    }
    items.iter().map(from_dict).collect()
}

/// 镜像 _toSummary：0 项「无扩展名」；≤4 项全量 append；>4 项
/// 「{前4} 等 {N} 项」（分隔 ", " 同 Python join）。
pub fn summary_of(extensions: &[String]) -> String {
    if extensions.is_empty() {
        return "无扩展名".into();
    }
    let head = extensions
        .iter()
        .take(4)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if extensions.len() > 4 {
        format!("{head} 等 {} 项", extensions.len())
    } else {
        head
    }
}

/// 镜像 CategoryEditDialog.category()（category_edit.py:93-110）：
/// 名称 strip 空兜「未命名分类」；扩展名逐 token strip→去点→小写、
/// 空 token 丢弃、**不去重**（TokenLineEdit tokens 口径：回车/空格
/// 逐 token，简版改为逗号分隔）；folder strip 空兜 None；icon 只接受
/// `ICON_CHOICES` 里的值，其余（含空串）兜 DOCUMENT——Qt 侧
/// `iconCombo.setCurrentIndex(findData(x))` 对未知值返回 -1、
/// `currentData()` 变 None，`or "DOCUMENT"` 兜底，与读侧 `fromDict`
/// 的「空串兜 DOCUMENT」口径不同（读侧不过滤未知图标，与 Python 一致）。
/// strip 全部走 CPython 口径
/// （`\x1c`-`\x1f` 也算空白，Rust `str::trim` 不含这四个码位）。
///
/// 差异记录：Qt 侧「显示名未改动则保留原文名」依赖翻译层（
/// toCategoryName 对照），Rust 无译文层，直接采用所填名称。
pub fn draft_to_rule(
    draft_id: &str,
    draft_name: &str,
    draft_icon: &str,
    draft_extensions: &str,
    draft_folder: &str,
) -> CategoryRule {
    let name = py_strip(draft_name);
    let folder = py_strip(draft_folder);
    CategoryRule {
        category_id: draft_id.to_string(),
        name: if name.is_empty() {
            "未命名分类".to_string()
        } else {
            name.to_string()
        },
        icon: if ICON_CHOICES.contains(&draft_icon) {
            draft_icon.to_string()
        } else {
            "DOCUMENT".to_string()
        },
        extensions: draft_extensions
            .split(',')
            .filter_map(|token| {
                let ext = py_strip(token).trim_start_matches('.').to_lowercase();
                if ext.is_empty() { None } else { Some(ext) }
            })
            .collect(),
        folder: if folder.is_empty() {
            None
        } else {
            Some(folder.to_string())
        },
    }
}

/// 镜像 `CategoryService._save()` 的落盘串：
/// `cfg.set(cfg.categoryRules, [asdict(c) for c in categories])`。
///
/// `QConfig.set` 先过 `CategoryListValidator.correct`，再走
/// `JsonConfigSerializer.serialize` = `json.dumps(value, ensure_ascii=False)`：
/// - 键序即 dataclass 字段序 `categoryId/name/icon/extensions/folder`；
/// - 分隔符用 Python 默认的 `", "` 与 `": "`（serde_json 不带空格）；
/// - `folder: None` → `null`；`Some("")` 原样落 `""`（Python `asdict`
///   只把 `None` 变 `null`，空串是空串）；
/// - 字符串转义见 `gd3_core::config::py_json_string`。
///
/// `correct` 对强类型列表恒等（`validate` 只要求「是 list、每项是 dict、
/// `name` 是 str」），故此处不重复过滤；该关口的语义由
/// [`correct_rule_list`] 单独钉住。
pub fn rules_to_json(rules: &[CategoryRule]) -> String {
    let mut out = String::from("[");
    for (index, rule) in rules.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str("{\"categoryId\": ");
        out.push_str(&py_json_string(&rule.category_id));
        out.push_str(", \"name\": ");
        out.push_str(&py_json_string(&rule.name));
        out.push_str(", \"icon\": ");
        out.push_str(&py_json_string(&rule.icon));
        out.push_str(", \"extensions\": [");
        for (position, extension) in rule.extensions.iter().enumerate() {
            if position > 0 {
                out.push_str(", ");
            }
            out.push_str(&py_json_string(extension));
        }
        out.push_str("], \"folder\": ");
        match &rule.folder {
            Some(folder) => out.push_str(&py_json_string(folder)),
            None => out.push_str("null"),
        }
        out.push('}');
    }
    out.push(']');
    out
}

/// 镜像 `CategoryListValidator.correct`（cfg.py:138-148）：非 list、或任
/// 一项不是 dict、或任一项 `name` 不是字符串 → 整表丢弃成 `[]`；其余原样
/// 返回（**不**逐项修正——`icon`/`extensions` 类型错也放行，与 Python 同）。
///
/// 写侧 [`rules_to_json`] 的输入是强类型 `CategoryRule`，恒过 `validate`，
/// 故这层过滤在 Rust 里不会改变结果；单独暴露是为了 `categoryRules` 的
/// 关口语义被改动时有断言兜住。
pub fn correct_rule_list(value: &serde_json::Value) -> serde_json::Value {
    let valid = value.as_array().is_some_and(|items| {
        items.iter().all(|item| {
            item.as_object()
                .and_then(|map| map.get("name"))
                .is_some_and(serde_json::Value::is_string)
        })
    });
    if valid {
        value.clone()
    } else {
        serde_json::Value::Array(Vec::new())
    }
}

/// 镜像 `CategoryService.addCategory`：无条件追加。新项的 `categoryId` 由
/// 调用方给（Python 侧新项是 `cat_{uuid4().hex}`，随机性不在本层）。
pub fn add_category(rules: &mut Vec<CategoryRule>, rule: CategoryRule) {
    rules.push(rule);
}

/// 镜像 `CategoryService.updateCategory`：按 `categoryId` 找首个匹配项替换；
/// 没有匹配项则什么都不做（Python 静默忽略、不写盘）。返回是否命中。
pub fn update_category(rules: &mut [CategoryRule], rule: CategoryRule) -> bool {
    let Some(slot) = rules
        .iter_mut()
        .find(|existing| existing.category_id == rule.category_id)
    else {
        return false;
    };
    *slot = rule;
    true
}

/// 镜像 `CategoryService.removeCategory`：按 id 全删（Python 用列表推导，
/// 重复 id 会一起删掉）。返回是否真的删了东西（决定要不要写盘）。
pub fn remove_category(rules: &mut Vec<CategoryRule>, category_id: &str) -> bool {
    let before = rules.len();
    rules.retain(|rule| rule.category_id != category_id);
    rules.len() != before
}

/// 镜像 `CategoryService.reset`：回到八预设。
pub fn reset_categories() -> Vec<CategoryRule> {
    default_rules()
}

/// 镜像 `CategoryService.reorder`：按给定 id 顺序重排。Python 先建
/// `{categoryId: category}` 字典（重复 id 后者覆盖前者）再按 id 取，
/// 取出的项数与当前项数不等 → 整体放弃（返回 false，调用方不写盘）；
/// 相等则替换（重复 id 会让同一项出现两次、被挤掉的项丢失，Python 亦然）。
pub fn reorder_categories(rules: &mut Vec<CategoryRule>, category_ids: &[String]) -> bool {
    let by_id: std::collections::HashMap<&str, &CategoryRule> = rules
        .iter()
        .map(|rule| (rule.category_id.as_str(), rule))
        .collect();
    let reordered: Vec<CategoryRule> = category_ids
        .iter()
        .filter_map(|id| by_id.get(id.as_str()).map(|rule| (*rule).clone()))
        .collect();
    if reordered.len() != rules.len() {
        return false;
    }
    *rules = reordered;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_builds_eight_presets() {
        for raw in [None, Some(""), Some("not json"), Some("[]"), Some("{}")] {
            let rules = parse_rules(raw);
            assert_eq!(rules.len(), 8, "输入：{raw:?}");
            assert_eq!(rules[0].category_id, "cat_video");
            assert_eq!(rules[0].extensions.len(), 20);
            assert_eq!(rules[5].extensions.len(), 24);
            assert_eq!(rules[5].extensions[20], "tar.gz", "复合扩展名保留");
            assert_eq!(rules[7].category_id, "cat_other");
            assert!(rules[7].extensions.is_empty());
            assert_eq!(rules[7].folder, None);
        }
    }

    #[test]
    fn disk_values_normalize_like_from_dict() {
        let raw = r#"[
            {"categoryId": "x", "name": "自定义", "icon": "",
             "extensions": [".MP4", "mkv", "mkv"], "folder": ""},
            {"name": "空 id"}
        ]"#;
        let rules = parse_rules(Some(raw));
        assert_eq!(rules.len(), 2);
        // 扩展名：去点/小写/去重；folder 空串 → None；icon 缺省 DOCUMENT。
        assert_eq!(rules[0].extensions, vec!["mp4", "mkv"]);
        assert_eq!(rules[0].folder, None);
        assert_eq!(rules[0].icon, "DOCUMENT");
        // categoryId 缺省：读侧不生成随机 ID（Python 端 fromDict 会
        // 生成 cat_{uuid4}，保留空串由写侧处理——不复刻随机性）。
        assert_eq!(rules[1].category_id, "");
        assert_eq!(rules[1].icon, "DOCUMENT");
    }

    #[test]
    fn summary_mirrors_to_summary() {
        assert_eq!(summary_of(&[]), "无扩展名");
        assert_eq!(summary_of(&["mp4".into(), "mkv".into()]), "mp4, mkv");
        let many: Vec<String> = ["a", "b", "c", "d", "e"].map(str::to_string).to_vec();
        assert_eq!(summary_of(&many), "a, b, c, d 等 5 项");
    }

    #[test]
    fn draft_to_rule_mirrors_dialog_category() {
        // 名称空兜「未命名分类」；扩展名去点/小写/去空、不去重；
        // folder 空串兜 None；id 原样保留。
        let rule = draft_to_rule("", "  自定义  ", "PHOTO", " .MP4 , mkv,,  ", " ");
        assert_eq!(rule.category_id, "");
        assert_eq!(rule.name, "自定义");
        assert_eq!(rule.extensions, vec!["mp4", "mkv"]);
        assert_eq!(rule.folder, None);

        let filled = draft_to_rule("x", "名称", "", "mp4, MP4, tar.gz", "{default}/X");
        assert_eq!(filled.icon, "DOCUMENT", "icon 空兜 DOCUMENT");
        // Qt 侧 `setCurrentIndex(findData(未知))` → -1 → currentData() None。
        let bogus = draft_to_rule("x", "名称", "NOT_AN_ICON", "", "");
        assert_eq!(bogus.icon, "DOCUMENT", "icon 未知值兜 DOCUMENT");
        // 不去重的 Python 口径保留重复项。
        assert_eq!(filled.extensions, vec!["mp4", "mp4", "tar.gz"]);
        assert_eq!(filled.folder.as_deref(), Some("{default}/X"));
        assert_eq!(filled.category_id, "x");
    }

    #[test]
    fn icon_choices_mirror_dialog_list() {
        assert_eq!(ICON_CHOICES.len(), 25);
        assert_eq!(ICON_CHOICES[0], "DOCUMENT");
        assert_eq!(ICON_CHOICES[24], "HELP");
    }

    fn sample_rule(id: &str, name: &str) -> CategoryRule {
        CategoryRule {
            category_id: id.to_string(),
            name: name.to_string(),
            icon: "DOCUMENT".to_string(),
            extensions: Vec::new(),
            folder: None,
        }
    }

    fn ids_of(rules: &[CategoryRule]) -> Vec<&str> {
        rules.iter().map(|rule| rule.category_id.as_str()).collect()
    }

    /// 金文件里的一条 `rules[i]` / `expected` 对象 → 强类型（缺字段按读侧
    /// 缺省口径兜底；`categoryId` 缺失即空串）。
    fn rule_from_json(value: &serde_json::Value) -> CategoryRule {
        CategoryRule {
            category_id: value["categoryId"].as_str().unwrap_or("").to_string(),
            name: value["name"].as_str().unwrap_or("").to_string(),
            icon: value["icon"].as_str().unwrap_or("").to_string(),
            extensions: value["extensions"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .map(|item| item.as_str().unwrap_or("").to_string())
                        .collect()
                })
                .unwrap_or_default(),
            folder: value["folder"].as_str().map(str::to_string),
        }
    }

    /// 写路径与 CPython 逐条对比（`category_rules.jsonl`）：
    /// `rules_to_json` ↔ `json.dumps(correct([asdict(c) …]), ensure_ascii=False)`；
    /// `correct_rule_list` ↔ `CategoryListValidator.correct`；
    /// `draft_to_rule` ↔ 真 `CategoryEditDialog.category()`。
    #[test]
    fn write_path_matches_python_fixture() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/category_rules.jsonl");
        let text = std::fs::read_to_string(&path).expect("category_rules.jsonl 缺失");
        let (mut dumps, mut corrects, mut drafts) = (0usize, 0usize, 0usize);
        // 注意：金文件里含裸 `\u{2028}`/`\u{2029}`（`ensure_ascii=False` 不转义），
        // Python `str.splitlines()` 会把它们当行界——`str::lines()` 只切 `\n`，
        // 所以这里读得到完整行。
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let case: serde_json::Value =
                serde_json::from_str(line).expect("金文件行不是合法 JSON");
            let label = case["label"].as_str().unwrap_or("");
            match case["kind"].as_str().expect("kind 缺失") {
                "dump" => {
                    let rules: Vec<CategoryRule> = case["rules"]
                        .as_array()
                        .expect("rules 必须是数组")
                        .iter()
                        .map(rule_from_json)
                        .collect();
                    let expected = case["text"].as_str().expect("text 必须是字符串");
                    assert_eq!(rules_to_json(&rules), expected, "落盘串不一致：{label}");
                    // Python 的期望串是过完 `correct` 的结果；强类型输入下
                    // `correct` 恒等，两者能对齐即这条断言。
                    let dumped: serde_json::Value =
                        serde_json::from_str(expected).expect("期望串应是合法 JSON");
                    assert_eq!(
                        correct_rule_list(&dumped),
                        dumped,
                        "correct 应恒等：{label}"
                    );
                    // 往返：读侧 `parse_rules` 还原同一批规则。空表会落回八
                    // 预设、空串 folder 被 `fromDict` 归一成 None，故跳过。
                    let round_trip_safe = !rules.is_empty()
                        && rules.iter().all(|rule| rule.folder.as_deref() != Some(""));
                    if round_trip_safe {
                        assert_eq!(parse_rules(Some(expected)), rules, "往返不一致：{label}");
                    }
                    dumps += 1;
                }
                "correct" => {
                    assert_eq!(
                        correct_rule_list(&case["input"]),
                        case["output"],
                        "correct 不一致：{label}"
                    );
                    corrects += 1;
                }
                "draft" => {
                    let rule = draft_to_rule(
                        case["id"].as_str().expect("id 缺失"),
                        case["name"].as_str().expect("name 缺失"),
                        case["icon"].as_str().expect("icon 缺失"),
                        case["extensions"].as_str().expect("extensions 缺失"),
                        case["folder"].as_str().expect("folder 缺失"),
                    );
                    let expected = rule_from_json(&case["expected"]);
                    assert_eq!(
                        rule.category_id,
                        case["id"].as_str().expect("id 缺失"),
                        "categoryId：{label}"
                    );
                    assert_eq!(rule.name, expected.name, "name：{label}");
                    assert_eq!(rule.icon, expected.icon, "icon：{label}");
                    assert_eq!(rule.extensions, expected.extensions, "extensions：{label}");
                    assert_eq!(rule.folder, expected.folder, "folder：{label}");
                    drafts += 1;
                }
                other => panic!("未知 kind：{other}"),
            }
        }
        // 防「金文件被清空/被裁剪仍全绿」。
        assert_eq!(
            (dumps, corrects, drafts),
            (5, 11, 7),
            "金文件条数变化需同步更新断言"
        );
    }

    /// `CategoryService` 五个写方法的语义（`category_service.py:201-236`）。
    #[test]
    fn service_write_methods_mirror_category_service() {
        let mut rules = vec![sample_rule("a", "A"), sample_rule("b", "B")];
        // addCategory：无条件追加。
        add_category(&mut rules, sample_rule("c", "C"));
        assert_eq!(ids_of(&rules), vec!["a", "b", "c"]);
        // updateCategory：按 id 命中替换；未命中静默忽略（不写盘）。
        assert!(update_category(&mut rules, sample_rule("b", "B2")));
        assert_eq!(rules[1].name, "B2");
        assert!(!update_category(&mut rules, sample_rule("zz", "Z")));
        assert_eq!(ids_of(&rules), vec!["a", "b", "c"]);
        // removeCategory：删到才算；重复 id 一起删。
        assert!(remove_category(&mut rules, "a"));
        assert!(!remove_category(&mut rules, "a"));
        assert_eq!(ids_of(&rules), vec!["b", "c"]);
        // reorder：按给定 id 重排；项数不等整体放弃。
        let order = vec!["c".to_string(), "b".to_string()];
        assert!(reorder_categories(&mut rules, &order));
        assert_eq!(ids_of(&rules), vec!["c", "b"]);
        assert!(!reorder_categories(&mut rules, &["b".to_string()]));
        assert_eq!(ids_of(&rules), vec!["c", "b"], "放弃时保持原序");
        assert!(!reorder_categories(
            &mut rules,
            &["b".to_string(), "x".to_string()]
        ));
        assert_eq!(ids_of(&rules), vec!["c", "b"]);
        // reset：回到八预设。
        let reset = reset_categories();
        assert_eq!(reset.len(), 8);
        assert_eq!(reset[0].category_id, "cat_video");
        assert_eq!(reset, default_rules());
    }
}
