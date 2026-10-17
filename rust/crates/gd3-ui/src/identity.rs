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
//! 开关（isEnabled 默认 T）。编辑对话框（PresetEditDialog）与
//! 「添加预设」按钮留欠账步；删除确认框（MessageBox）归调用方。

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
}
