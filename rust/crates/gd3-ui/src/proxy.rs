//! 代理设置镜像（第 35 步）：`ProxySettingCard`（setting_cards.py:142-322）
//! + `ProxyValidator`（cfg.py:71-91）。
//!
//! 分层：配置口径永远是 `GeneralDownload/ProxyServer` 的字面量
//! （"Auto"/"Off" 或拼装 URL，缺省 "Auto"=cfg.py:251）；UI 发选择
//! 索引与字段串，拼装/校验/写入决策在 install 层（镜像 Python 端
//! `_buildProxyUrl` + `validator.validate` + leaveEvent 才落盘）。

use regex::Regex;
use std::sync::OnceLock;

/// 协议下拉（setting_cards.py:162 `addItems` 序；UI 与配置共用同序）。
pub const PROTOCOLS: [&str; 5] = ["socks4", "socks5", "socks5h", "http", "https"];

pub const VALUE_OFF: &str = "Off";
pub const VALUE_AUTO: &str = "Auto";
/// cfg.py:251 缺省 "Auto"。
pub const VALUE_DEFAULT: &str = VALUE_AUTO;

/// 选择索引：镜像 Off/Auto/Custom 三选单选（fluent 风格 std 无
/// RadioButton 导出，用 ComboBox 三选镜像互斥语义）。
pub const CHOICE_OFF: i32 = 0;
pub const CHOICE_AUTO: i32 = 1;
pub const CHOICE_CUSTOM: i32 = 2;

/// 自定义代理四字段（protocolIndex + host/port/user/pass 串）。
#[derive(Default)]
pub struct ProxyFields {
    pub protocol_index: i32,
    pub host: String,
    pub port: String,
    pub user: String,
    pub pass: String,
}

/// 镜像 `_loadProxy` 的分支：Auto/Off→对应选择，否则自定义。
pub fn choice_of_value(value: &str) -> i32 {
    match value {
        VALUE_AUTO => CHOICE_AUTO,
        VALUE_OFF => CHOICE_OFF,
        _ => CHOICE_CUSTOM,
    }
}

/// 镜像 `_showProxyUrl`/`urlsplit`：按 `://` 切出 scheme，可选
/// `user:pass@` 凭据（urlsplit 只支持一个 @，取第一个），再按最后
/// 一个 `:` 切 host:port。scheme 不在 PROTOCOLS 时 ComboBox 索引落
/// 0（Qt `setCurrentText` 未命中不改值，初始集为缺省 0 的镜像）。
pub fn parse_fields(url: &str) -> ProxyFields {
    let mut fields = ProxyFields::default();
    let (scheme, rest) = match url.split_once("://") {
        Some((scheme, rest)) => (scheme, rest),
        None => ("", url),
    };
    if !scheme.is_empty()
        && let Some(index) = PROTOCOLS
            .iter()
            .position(|p| p.eq_ignore_ascii_case(scheme))
    {
        fields.protocol_index = index as i32;
    }
    let (cred, hostport) = match rest.split_once('@') {
        Some((cred, hostport)) => (cred, hostport),
        None => ("", rest),
    };
    if let Some((user, pass)) = cred.split_once(':') {
        fields.user = user.to_string();
        fields.pass = pass.to_string();
    } else if !cred.is_empty() {
        fields.user = cred.to_string();
    }
    if let Some((host, port)) = hostport.rsplit_once(':') {
        fields.host = host.to_string();
        fields.port = port.to_string();
    } else {
        fields.host = hostport.to_string();
    }
    fields
}

/// 镜像 `_buildProxyUrl`：凭据仅在用户名或密码非空时拼
/// `user:pass@`。
pub fn build_url(protocol: &str, host: &str, port: &str, user: &str, password: &str) -> String {
    let cred = if user.is_empty() && password.is_empty() {
        String::new()
    } else {
        format!("{user}:{password}@")
    };
    format!("{protocol}://{cred}{host}:{port}")
}

fn url_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // 逐字符镜像 cfg.py:74-81 ProxyValidator.PATTERN（^...$、可选
    // 凭据、IPv4 点分八组或域名、1-5 位端口）。
    RE.get_or_init(|| {
        Regex::new(
            r"^(?:(http|https|socks4|socks5|socks5h)://)(?:(\w+):([\w!@#$%^&*()]+)@)?((?:(?:25[0-5]|2[0-4]\d|[01]?\d\d?)\.){3}(?:25[0-5]|2[0-4]\d|[01]?\d\d?)|(?:[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?\.)+[a-zA-Z]{2,6}):(\d{1,5})$",
        )
        .expect("proxy url 正则编译失败")
    })
}

fn host_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"^(?:(?:25[0-5]|2[0-4]\d|[01]?\d\d?)\.){3}(?:25[0-5]|2[0-4]\d|[01]?\d\d?)$|^(?:[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?\.)+[a-zA-Z]{2,6}$",
        )
        .expect("host 正则编译失败")
    })
}

/// 镜像 `ProxyValidator.validate`：Auto/Off 恒有效；URL 走正则且
/// 端口 1-65535（cfg.py:85-88）。
pub fn validate(value: &str) -> bool {
    if value == VALUE_OFF || value == VALUE_AUTO {
        return true;
    }
    url_regex()
        .captures(value)
        .and_then(|caps| caps.get(5))
        .map(|port| {
            port.as_str()
                .parse::<u32>()
                .is_ok_and(|n| (1..=65535).contains(&n))
        })
        .unwrap_or(false)
}

/// 镜像 setting_cards.py:28-30 的行内错误显示口径（HOST/PORT
/// 正则，非空才显示错误——`_onProxyFieldChanged` 运算在调用方）。
pub fn host_valid(host: &str) -> bool {
    host_regex().is_match(host)
}

pub fn port_valid(port: &str) -> bool {
    port.trim()
        .parse::<u32>()
        .is_ok_and(|n| (1..=65535).contains(&n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_value_choice_and_url_round_trip() {
        // 缺省 Auto → 选择 1。
        assert_eq!(choice_of_value(VALUE_DEFAULT), CHOICE_AUTO);
        assert_eq!(choice_of_value("Off"), CHOICE_OFF);
        // 未知字面量 → 自定义（Python 端Validator会把无效值校正
        // 回 "Auto"=cfg correct；这里镜像卡内 _loadProxy 的 else 分支）。
        assert_eq!(choice_of_value("garbage://x"), CHOICE_CUSTOM);

        // 凭据 URL round-trip：parse → build 还原配置字面量。
        let fields = parse_fields("socks5://u:p@1.2.3.4:1080");
        assert_eq!(fields.protocol_index, 1);
        assert_eq!(fields.host, "1.2.3.4");
        assert_eq!(fields.port, "1080");
        assert_eq!(fields.user, "u");
        assert_eq!(fields.pass, "p");
        assert_eq!(
            build_url("socks5", "1.2.3.4", "1080", "u", "p"),
            "socks5://u:p@1.2.3.4:1080"
        );
        assert!(validate("socks5://u:p@1.2.3.4:1080"));
        assert!(validate("http://example.com:80"));
        assert!(!validate("http://1.2.3.4:0"));
        assert!(!validate("http://1.2.3.4:70000"));
        assert!(!validate("http://bad host:80"));
        // Auto/Off 恒有效（cfg.py:85-86）。
        assert!(validate(VALUE_AUTO));
        assert!(validate(VALUE_OFF));
        // host/port 行内错误正则。
        assert!(host_valid("example.com"));
        assert!(!host_valid("bad host"));
        assert!(port_valid("8080"));
        assert!(!port_valid("70000"));
        assert!(!port_valid("abc"));
    }
}
