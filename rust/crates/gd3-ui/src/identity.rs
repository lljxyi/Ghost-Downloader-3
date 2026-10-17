//! 身份预设列表镜像层（cfg.py Network/IdentityPresets；第 43 步）。
//!
//! 实证：
//! - `identityPresets = ConfigItem("Network", "IdentityPresets",
//!   [百度网盘客户端预设], IdentityPresetListValidator(),
//!   JsonConfigSerializer(list, list))`（cfg.py:334-340）；
//! - 读盘路径（qfluentwidgets QConfig.load → deserializeFrom）只走
//!   serializer 反序列化，**不经过 validator.correct**——合法过滤
//!   仅在 qconfig.set 写路径生效；JSON 解析失败 fallback `[]`；
//! - `REQUIRED_KEYS = {name, clientProfile, userAgent, hosts}`（cfg.py:
//!   174-195）；isEnabled 非必需（行内 `preset.get("isEnabled", True)`），
//!   列表内部是 JSON 原生 bool（非顶层 str 串）；
//! - 缺键（新装）→ ConfigItem 缺省 = 百度网盘客户端预设
//!   （validator.correct(default) 后原样保留）。
//!
//! 行结构镜像 PresetRowWidget（setting_cards.py:372-431）：名称 +
//! 描述三段（hosts 前 3 逗号连接 / profile 显示名 / UA 截断 24） +
//! 开关（isEnabled 默认 T）。编辑对话框（PresetEditDialog）第 47 步已
//! 镜像（hosts/profile/UA 归一化）；删除确认框（MessageBox）归调用方。

use crate::pystr::py_strip;

#[derive(Clone)]
pub struct IdentityPreset {
    pub name: String,
    pub client_profile: String,
    pub user_agent: String,
    pub hosts: Vec<String>,
    pub is_enabled: bool,
}

/// cfg.py:335-340 缺省百度网盘预设逐字镜像。
fn default_presets() -> Vec<IdentityPreset> {
    vec![IdentityPreset {
        name: "百度网盘客户端".into(),
        client_profile: "raw".into(),
        user_agent: "pan.baidu.com".into(),
        hosts: vec!["*.pcs.baidu.com".into()],
        is_enabled: true,
    }]
}

fn str_field(item: &serde_json::Value, key: &str) -> String {
    item.get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// 镜像读盘口径：缺键 → 缺省列表；JSON 解析失败/非数组 → `[]`
/// （JsonConfigSerializer fallback `list()`，correct 不参与）；
/// 逐项容错取字段（Python 端 `preset.get(...)` 语义，无 validator）。
pub fn parse_presets(raw: Option<&str>) -> Vec<IdentityPreset> {
    let Some(raw) = raw else {
        return default_presets();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Vec::new();
    };
    let Some(items) = value.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .map(|item| IdentityPreset {
            name: str_field(item, "name"),
            client_profile: str_field(item, "clientProfile"),
            user_agent: str_field(item, "userAgent"),
            hosts: item
                .get("hosts")
                .and_then(serde_json::Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            is_enabled: item
                .get("isEnabled")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
        })
        .collect()
}

/// 行描述串镜像 PresetRowWidget.descLayout 三段拼接（Python 端为三枚
/// IconBodyLabel 平铺，仅非空段添加；Slint 单 Text 以「 · 」间隔，
/// 对齐留主题布局步）：hosts 前 3 个 ", " 连接 / profile 显示名
/// （toProfileLabel）+ UA 截断（>24 取前 21 补 "..."）。
pub fn row_desc(preset: &IdentityPreset) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !preset.hosts.is_empty() {
        parts.push(
            preset
                .hosts
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    if !preset.client_profile.is_empty() {
        parts.push(crate::profile::label_of(&preset.client_profile));
    }
    if !preset.user_agent.is_empty() {
        parts.push(clip_user_agent(&preset.user_agent));
    }
    parts.join(" · ")
}

fn clip_user_agent(ua: &str) -> String {
    if ua.chars().count() <= 24 {
        ua.to_string()
    } else {
        let head: String = ua.chars().take(21).collect();
        format!("{head}...")
    }
}

/// 镜像 `TokenLineEdit` 的 token 口径（token_line_edit.py:87-94：回车/
/// 空格提交 `text().strip()` 非空 token；172-178 重复 token 丢弃）。
/// 覆盖层简版用多行文本框，故按空白与 `,`/`;` 切、逐条 py_strip、去空、
/// 保序去重（预置里已有的重复 host 保存时会被合并——与「手输重复被拒」
/// 同口径，差异记录在案）。
pub fn parse_hosts(text: &str) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    for piece in text.split(|c: char| c == ',' || c == ';' || crate::pystr::py_isspace(c)) {
        let host = py_strip(piece);
        if host.is_empty() || hosts.iter().any(|seen| seen == host) {
            continue;
        }
        hosts.push(host.to_string());
    }
    hosts
}

/// 覆盖层回显文本（每行一个 host）——与 parse_hosts 互逆。
pub fn hosts_to_text(hosts: &[String]) -> String {
    hosts.join("\n")
}

/// 镜像 PresetEditDialog.validate()（preset_edit.py:103-111）：至少一个
/// 匹配 Host 才能匹配请求。
pub fn hosts_valid(hosts: &[String]) -> bool {
    !hosts.is_empty()
}

/// profile 选择项（`buildProfileMenu(includeAuto=False)` 的扁平化：菜单首项
/// 传空串 = 跟随全局默认，再是 raw 与九家族「最新」）。索引 0 即磁盘里的
/// 空串字面量——Python 侧 `_refreshProfileLabel` 对它显示「跟随全局默认」。
pub fn profile_index_of(value: &str) -> Option<i64> {
    match value {
        "" => Some(0),
        "raw" => Some(1),
        other => crate::profile::FAMILY_LABELS
            .iter()
            .position(|(name, _)| *name == other)
            .map(|p| (p + 2) as i64),
    }
}

/// 索引 → profile 字面量（越界兜空串 = 跟随全局默认，不复刻 UI 崩溃面）。
pub fn profile_value_of(index: i64) -> &'static str {
    match index {
        0 => "",
        1 => "raw",
        i => crate::profile::FAMILY_LABELS
            .get((i - 2) as usize)
            .map_or("", |(name, _)| *name),
    }
}

/// 索引 → 覆盖层 ComboBox 显示串（0 特例「跟随全局默认」，其余复用
/// `toProfileLabel` 镜像）。
pub fn profile_label_of(index: i64) -> String {
    if index == 0 {
        "跟随全局默认".to_string()
    } else {
        crate::profile::label_of(profile_value_of(index))
    }
}

/// 镜像 PresetEditDialog.preset()（preset_edit.py:113-123）：名称 py_strip
/// 空兜「未命名预设」、UA py_strip、hosts 逐 token、isEnabled 由调用方
/// 按既有预设继承（添加时 true）。
pub fn draft_to_identity_preset(
    name: &str,
    hosts_text: &str,
    profile: &str,
    user_agent: &str,
    is_enabled: bool,
) -> IdentityPreset {
    let name = py_strip(name);
    IdentityPreset {
        name: if name.is_empty() {
            "未命名预设".to_string()
        } else {
            name.to_string()
        },
        client_profile: profile.to_string(),
        user_agent: py_strip(user_agent).to_string(),
        hosts: parse_hosts(hosts_text),
        is_enabled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_key_falls_back_to_baidu_preset() {
        let presets = parse_presets(None);
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].name, "百度网盘客户端");
        assert_eq!(presets[0].client_profile, "raw");
        assert_eq!(presets[0].user_agent, "pan.baidu.com");
        assert_eq!(presets[0].hosts, vec!["*.pcs.baidu.com"]);
        assert!(presets[0].is_enabled);
        assert_eq!(
            row_desc(&presets[0]),
            "*.pcs.baidu.com · 不模拟（原样发送） · pan.baidu.com"
        );
    }

    #[test]
    fn disk_json_loads_without_validator_filter() {
        // 读盘路径不过 correct：结构缺失/字段非法也不剔除（get 兜缺省）。
        let raw = r#"[
            {"name": "A", "clientProfile": "chrome", "userAgent": "",
             "hosts": ["h1", "h2", "h3", "h4"], "isEnabled": false},
            {"name": "B"}
        ]"#;
        let presets = parse_presets(Some(raw));
        assert_eq!(presets.len(), 2, "读盘不做 valid 过滤（仅写路径 correct）");
        assert_eq!(presets[0].hosts, vec!["h1", "h2", "h3", "h4"]);
        assert!(!presets[0].is_enabled);
        // 描述段：hosts 只取前 3、profile 家族「最新」、UA 空不出现。
        assert_eq!(row_desc(&presets[0]), "h1, h2, h3 · Chrome（最新）");
        assert_eq!(row_desc(&presets[1]), "", "B 无内容段，desc 为空");
    }

    #[test]
    fn broken_json_is_empty_and_faults_default_off() {
        assert!(parse_presets(Some("not json")).is_empty());
        assert!(parse_presets(Some("{}")).is_empty());
        let raw = r#"[{"name": "X", "isEnabled": "yes"}]"#;
        let presets = parse_presets(Some(raw));
        // isEnabled 非真 bool → get 兜 True（Python .get 同口径）。
        assert!(presets[0].is_enabled);
    }

    #[test]
    fn user_agent_clip_mirrors_24_limit() {
        let ua = "Mozilla/5.0 (X11; Linux x86_64) Chrome";
        let presets = [IdentityPreset {
            name: "t".into(),
            client_profile: String::new(),
            user_agent: ua.into(),
            hosts: Vec::new(),
            is_enabled: true,
        }];
        assert_eq!(row_desc(&presets[0]).len(), 24, "21 字 + 3 位省略号");
        assert!(row_desc(&presets[0]).ends_with("..."));
    }

    #[test]
    fn parse_hosts_mirrors_token_rules() {
        // 空白/逗号/分号都算分隔；逐条 strip、去空、保序去重。
        assert_eq!(
            parse_hosts(" a.com, b.com\nc.com  a.com ; \t"),
            vec!["a.com", "b.com", "c.com"]
        );
        assert!(parse_hosts("   ").is_empty());
        // 通配符原样保留（不 strip 星号）。
        assert_eq!(parse_hosts("*.pcs.baidu.com"), vec!["*.pcs.baidu.com"]);
        // 回显与解析互逆。
        let hosts = vec!["h1".to_string(), "*.h2".to_string()];
        assert_eq!(parse_hosts(&hosts_to_text(&hosts)), hosts);
        assert!(hosts_valid(&hosts));
        assert!(!hosts_valid(&[]));
    }

    #[test]
    fn draft_to_identity_preset_mirrors_dialog_preset() {
        let preset = draft_to_identity_preset("  ", "a.com b.com", "raw", "  UA  ", true);
        assert_eq!(preset.name, "未命名预设");
        assert_eq!(preset.client_profile, "raw");
        assert_eq!(preset.user_agent, "UA");
        assert_eq!(preset.hosts, vec!["a.com", "b.com"]);
        assert!(preset.is_enabled);
        // 编辑时 isEnabled 由调用方继承既有值（Python result 覆盖分支）。
        let named = draft_to_identity_preset(" X ", "a.com", "", "", false);
        assert_eq!(named.name, "X");
        assert_eq!(named.client_profile, "");
        assert!(!named.is_enabled);
    }

    #[test]
    fn profile_choices_mirror_menu_without_auto() {
        assert_eq!(profile_index_of(""), Some(0));
        assert_eq!(profile_index_of("raw"), Some(1));
        assert_eq!(profile_index_of("chrome"), Some(2));
        assert_eq!(profile_index_of("okhttp"), Some(10));
        // 特定版本不在 includeAuto=False 菜单里——调用方追加一项。
        assert_eq!(profile_index_of("chrome120"), None);
        assert_eq!(profile_value_of(0), "");
        assert_eq!(profile_value_of(1), "raw");
        assert_eq!(profile_value_of(2), "chrome");
        assert_eq!(profile_value_of(10), "okhttp");
        assert_eq!(profile_value_of(99), "", "越界兜跟随全局默认");
        assert_eq!(profile_label_of(0), "跟随全局默认");
        assert_eq!(profile_label_of(1), "不模拟（原样发送）");
        assert_eq!(profile_label_of(2), "Chrome（最新）");
        for i in 0..11 {
            assert_eq!(profile_index_of(profile_value_of(i)), Some(i));
        }
    }
}
