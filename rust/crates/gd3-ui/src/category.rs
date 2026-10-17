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
//! 行编辑/添加对话框（CategoryEditDialog）、拖拽排序（reorder）、
//! 匹配与输出目录解析（matchByName/categoryOf/outputFolderOf）随
//! Phase 8 任务造/引擎桥接并轨；「恢复默认」「删除」动作收 (),
//! id 字面量，cfg 写与 categoriesChanged 重推归调用方。

#[derive(Clone)]
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
fn from_dict(item: &serde_json::Value) -> CategoryRule {
    let extensions = item
        .get("extensions")
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            let mut list: Vec<String> = Vec::new();
            for ext in arr {
                let mut normalized = ext.as_str().unwrap_or("").trim().to_string();
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
/// 逐 token，简版改为逗号分隔）；folder strip 空兜 None；icon 兜
/// DOCUMENT（与 fromDict 同口径）。
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
    let name = draft_name.trim();
    let folder = draft_folder.trim();
    CategoryRule {
        category_id: draft_id.to_string(),
        name: if name.is_empty() {
            "未命名分类".to_string()
        } else {
            name.to_string()
        },
        icon: if draft_icon.is_empty() {
            "DOCUMENT".to_string()
        } else {
            draft_icon.to_string()
        },
        extensions: draft_extensions
            .split(',')
            .filter_map(|token| {
                let ext = token.trim().trim_start_matches('.').to_lowercase();
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
        assert_eq!(filled.icon, "DOCUMENT", "icon 空/非法兜 DOCUMENT");
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
}
