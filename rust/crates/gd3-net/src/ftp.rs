//! ftp 任务 parse 的 URL → 连接信息纯变换层（`GD3_RUST_FTP_URL`）。
//!
//! 镜像 `features/ftp_pack/pack.py::parseFtpSource` 的 Python 原生分支：
//! `urlparse` + `unquote`（errors='replace'）+ `PurePosixPath` 归一化 +
//! `port or defaultPort`。userinfo 的空串**原样保留**（`anonymous`/
//! `anon@` 兜底由 pack.py sink 端 `or` 完成）。所有失败都映射为
//! Python `ValueError`（文案逐字返回，桥侧直接抛出）——原生侧信号
//! 全部来自 urllib/ipaddress 的精度点。
//!
//! 逐字语义来源（CPython 3.13 源码 + 实测探针，见 parity 测试）：
//! - `urlsplit`：**仅左端**去 C0+空格（尾部空格保留）、去 `\t\r\n`、
//!   scheme_chars、`_splitnetloc`（`/?#` 取最早）、`_checknetloc`（NFKC）、
//!   括号失衡 → `Invalid IPv6 URL`（`_check_bracketed_netloc` 只在
//!   `[` 和 `]` 同时入 netloc 时调用——普通主机名不落 ipaddress）；
//! - `_userinfo`：`rpartition('@')` + **首次** `partition(':')`；
//!   `_hostinfo`：bracketed 走 `partition(']')` + `port.partition(':')`，
//!   否则首次 `partition(':')`；username/hostname **不 unquote**；
//! - `port`：`isdigit() and isascii()` → int，超出 0-65535 报 range，
//!   其余 cast（`{port!r}` 用 Python repr 体例）；上游 `port or default`
//!   使显式 0 也落到 defaultPort（hasPort 仍为 True）；
//! - `hostname`：首次 `partition('%')`，头段 `lower()`、zone 原样
//!   （`fe80::1%25eTH0` → `fe80::1%25eTH0`）；
//! - `unquote(str)`：按 `_asciire` 切 ASCII 段，**每段独立**
//!   `_unquote_impl`（逐 `%` 十六进制对，非法落字面 `'%'`）+ utf-8
//!   replace 解码；非 ASCII 段原样穿过（序列无法跨段合并）；
//! - `PurePosixPath`：双前导斜杠特例、段过滤空与 `.`、空结果 `'.'`。
//!
//! 已知偏差（斗胆写明）：希腊 final sigma 的 `str.lower()` 语境规则
//! 未复刻（`Σ` 永远 → `σ`）；非 ASCII 主机名实际不会出现在 ftp 流量里。

use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, PartialEq)]
pub struct FtpSource {
    pub scheme: String,
    pub host: String,
    pub port: i64,
    pub username: String,
    pub password: String,
    pub source_path: String,
    pub has_port: bool,
}

pub fn parse_ftp_source(url: &str, default_port: i64) -> Result<FtpSource, String> {
    // urlsplit 内部只做 lstrip（`_WHATWG_C0_CONTROL_OR_SPACE`，0x00-0x20），
    // 尾部空白保留；调用方的 `options.url.strip()` 由 pack.parse 先行完成。
    let url: String = url
        .trim_start_matches(|c: char| (c as u32) <= 0x20)
        .replace(['\t', '\r', '\n'], "");
    let (scheme, netloc, rest) = urlsplit(&url)?;

    // `parsed.port or defaultPort`：cast/范围错误走 ValueError 文案；
    // 显式 0 也会回退到 defaultPort（与 `or` 的假值语义一致）。
    let hostinfo = host_info(&netloc);
    let host = if hostinfo.hostname.is_empty() {
        String::new()
    } else {
        lower_with_zone(&hostinfo.hostname)
    };
    let port = match hostinfo.port_text {
        None => default_port,
        Some(ref text) => match port_from(text) {
            Ok(0) => default_port,
            Ok(value) => value,
            Err(message) => return Err(message),
        },
    };
    let has_port = hostinfo.port_text.is_some();

    let source_path = pure_posix_str(&unquote(if rest.is_empty() { "/" } else { &rest }));

    Ok(FtpSource {
        scheme,
        host,
        port,
        // urlparse 的 userinfo 中 partition 结果：空串保留 ''，`anonymous`
        // /`anon@` 兜底在 pack.py sink 端用 `or` 完成（行为一致：真值原样）。
        username: unquote(&hostinfo.username),
        password: unquote(&hostinfo.password),
        source_path,
        has_port,
    })
}

// ---------------------------------------------------------------------------
// urlsplit 子集
// ---------------------------------------------------------------------------

fn is_scheme_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')
}

/// `(scheme(小写), netloc, path rest)`；fragment/query 已按 Python 顺序剥掉
/// （先 `#` 后 `?`），只保留 path。所有 ValueError 文案从 `Err` 返回。
fn urlsplit(url: &str) -> Result<(String, String, String), String> {
    let mut rest = url;
    let mut scheme = String::new();
    if let Some(i) = url.find(':') {
        let first_ok = url.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
        if i > 0 && first_ok && url[..i].chars().all(is_scheme_char) {
            scheme = url[..i].to_lowercase();
            rest = &url[i + 1..];
        }
    }

    let mut netloc = String::new();
    if rest.starts_with("//") {
        let after = &rest[2..];
        let delim = after.find(['/', '?', '#']).unwrap_or(after.len());
        netloc = after[..delim].to_string();
        rest = &after[delim..];
        let open = netloc.contains('[');
        let close = netloc.contains(']');
        if open != close {
            return Err("Invalid IPv6 URL".to_string());
        }
        // `_check_bracketed_netloc` 只在两端同时存在时调用
        if open && close {
            check_bracketed_netloc(&netloc)?;
        }
    }
    if let Some(hash) = rest.find('#') {
        rest = &rest[..hash];
    }
    if let Some(q) = rest.find('?') {
        rest = &rest[..q];
    }
    check_netloc(&netloc)?;
    Ok((scheme, netloc, rest.to_string()))
}

fn check_netloc(netloc: &str) -> Result<(), String> {
    if netloc.is_empty() || netloc.is_ascii() {
        return Ok(());
    }
    // 忽略已在 netloc 里的分隔符再归一化（Python：replace 掉 @ : # ?）
    let n: String = netloc
        .chars()
        .filter(|c| !matches!(c, '@' | ':' | '#' | '?'))
        .collect();
    let nfkc: String = n.nfkc().collect();
    if n == nfkc {
        return Ok(());
    }
    // /?#@: 任一出现在归一化结果里 → netloc 已被破坏
    if nfkc.contains(['/', '?', '#', '@', ':']) {
        return Err(format!(
            "netloc '{netloc}' contains invalid characters under NFKC normalization"
        ));
    }
    Ok(())
}

fn check_bracketed_netloc(netloc: &str) -> Result<(), String> {
    // 必须与 _hostinfo 的切法一致：先右分 '@'，再 partition('[')
    let hostinfo = match netloc.rsplit_once('@') {
        Some((_, h)) => h,
        None => netloc,
    };
    let (before_bracket, have_open_br, bracketed) = match hostinfo.split_once('[') {
        Some((b, r)) => (b, true, r),
        None => (hostinfo, false, hostinfo),
    };
    let hostname = if have_open_br {
        // 括号前不允许有数据
        if !before_bracket.is_empty() {
            return Err("Invalid IPv6 URL".to_string());
        }
        let (hostname, _found, port) = punch(bracketed, ']');
        // 括号后、端口分隔前不允许有别的字符
        if !port.is_empty() && !port.starts_with(':') {
            return Err("Invalid IPv6 URL".to_string());
        }
        hostname
    } else {
        before_bracket
            .split_once(':')
            .map_or(before_bracket, |(h, _)| h)
    };
    check_bracketed_host(hostname)
}

/// `(before, found, after)` 三段 partition——after 是「首次命中后」的剩余。
fn punch(s: &str, sep: char) -> (&str, bool, &str) {
    match s.split_once(sep) {
        Some((b, a)) => (b, true, a),
        None => (s, false, ""),
    }
}

fn check_bracketed_host(hostname: &str) -> Result<(), String> {
    if hostname.starts_with('v') {
        // \Av[a-fA-F0-9]+\..+\Z
        let valid = matchhostname(hostname);
        if !valid {
            return Err("IPvFuture address is invalid".to_string());
        }
        Ok(())
    } else if valid_ipv4(hostname) {
        Err("An IPv4 address cannot be in brackets".to_string())
    } else if !valid_ipv6(hostname) {
        Err(format!(
            "'{}' does not appear to be an IPv4 or IPv6 address",
            hostname
        ))
    } else {
        Ok(())
    }
}

/// 正则 `\Av[a-fA-F0-9]+\..+\Z` 的直译（`v` 后 ≥1 个 hex digits，一个点，
/// 点后任意 ≥1 字符，到串尾）。
fn matchhostname(hostname: &str) -> bool {
    let body = &hostname[1..];
    match body.split_once('.') {
        Some((hex_part, tail)) => {
            !hex_part.is_empty()
                && !tail.is_empty()
                && hex_part.chars().all(|c| c.is_ascii_hexdigit())
        }
        None => false,
    }
}

fn valid_ipv4(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.chars().all(|c| c.is_ascii_digit())
                && (p.len() == 1 || !p.starts_with('0'))
                && p.parse::<u32>().is_ok_and(|v| v <= 255)
        })
}

fn is_ipv6_group(s: &str) -> bool {
    !s.is_empty() && s.len() <= 4 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// 镜像 ipaddress.IPv6Address 的字符串校验：`_split_scope_id` 的 zone 语义
/// + `_ip_int_from_string` 的全部结构检查（报错文案由调用方统一起）。
fn valid_ipv6(s: &str) -> bool {
    // zone：首次 partition('%')；空或含第二个 '%' → 非法；内容不再校验
    let ip_str = match s.split_once('%') {
        Some((m, z)) => {
            if z.is_empty() || z.contains('%') {
                return false;
            }
            m
        }
        None => s,
    };
    if ip_str.is_empty() {
        return false;
    }
    // 45 字符上限（zone 已剥掉）
    if ip_str.chars().count() > 45 {
        return false;
    }
    // split(':', maxsplit=9)：冒号 ≥9 个时 len(parts)=10 → 超过 _max_parts
    if ip_str.matches(':').count() >= 9 {
        return false;
    }
    let mut parts: Vec<&str> = ip_str.split(':').collect();
    // 末段含 '.' → 嵌入 IPv4，占两段
    if parts.last_mut().map(|t| t.contains('.')) == Some(true) {
        let tail = parts.pop().unwrap();
        if !valid_ipv4(tail) {
            return false;
        }
        parts.push("0000");
        parts.push("0000");
    }
    // 冒号组数上限
    if parts.len() > 9 {
        return false;
    }

    // 端点外的空段即被压缩的 '::'；至多一个
    let mut skip_index: Option<usize> = None;
    for (i, part) in parts
        .iter()
        .enumerate()
        .skip(1)
        .take(parts.len().saturating_sub(2))
    {
        if part.is_empty() {
            if skip_index.is_some() {
                return false;
            }
            skip_index = Some(i);
        }
    }

    let (parts_hi, parts_lo, parts_skipped) = match skip_index {
        None => {
            if parts.len() != 8 {
                return false;
            }
            if parts[0].is_empty() || parts[parts.len() - 1].is_empty() {
                return false;
            }
            (parts.len(), 0usize, 0usize)
        }
        Some(skip) => {
            let mut parts_hi = skip;
            let mut parts_lo = parts.len() - skip - 1;
            if parts[0].is_empty() {
                parts_hi -= 1;
                if parts_hi != 0 {
                    return false; //'^:' requires '::'
                }
            }
            if parts[parts.len() - 1].is_empty() {
                parts_lo -= 1;
                if parts_lo != 0 {
                    return false; //':$' requires '::'
                }
            }
            match 8usize.checked_sub(parts_hi + parts_lo) {
                Some(skipped) if skipped >= 1 => (parts_hi, parts_lo, skipped),
                _ => return false,
            }
        }
    };
    let _ = parts_skipped;
    let lo_range = parts.len() - parts_lo;
    for part in parts
        .iter()
        .take(parts_hi)
        .chain(parts.iter().skip(lo_range))
    {
        if !is_ipv6_group(part) {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// hostinfo：username / password / hostname / (port 文本)
// ---------------------------------------------------------------------------

struct HostInfo {
    username: String,
    password: String,
    hostname: String,
    port_text: Option<String>,
}

/// 镜像 `_userinfo` + `_hostinfo`（统计原样，解码发生在调用点）。
fn host_info(netloc: &str) -> HostInfo {
    let (userinfo, have_info, hostinfo) = match netloc.rsplit_once('@') {
        Some((u, h)) => (Some(u), true, h),
        None => (None, false, netloc),
    };
    let (username, password) = match userinfo {
        None => (String::new(), String::new()),
        Some(u) => match u.split_once(':') {
            Some((name, pw)) => (name.to_string(), pw.to_string()),
            None => (u.to_string(), String::new()),
        },
    };
    let (hostname, port_text) = match hostinfo.split_once('[') {
        Some((_, bracketed)) => {
            let (name, found, port) = punch(bracketed, ']');
            let text = if found {
                match port.split_once(':') {
                    Some((_, p)) if !p.is_empty() => Some(p.to_string()),
                    _ => None,
                }
            } else {
                None
            };
            (name.to_string(), text)
        }
        None => match hostinfo.split_once(':') {
            Some((name, p)) => {
                let text = if p.is_empty() {
                    None
                } else {
                    Some(p.to_string())
                };
                (name.to_string(), text)
            }
            None => (hostinfo.to_string(), None),
        },
    };
    let _ = have_info;
    HostInfo {
        username,
        password,
        hostname,
        port_text,
    }
}

/// `hostname` 属性：首次 `partition('%')` 后头段小写、zone 原样。
fn lower_with_zone(hostname: &str) -> String {
    match hostname.split_once('%') {
        Some((head, zone)) => format!("{}%{}", head.to_lowercase(), zone),
        None => hostname.to_lowercase(),
    }
}

/// `port` 属性：ascii 数字 → int；范围 0..=65535 之外；其余 cast 文案（带 repr）。
fn port_from(text: &str) -> Result<i64, String> {
    if text.is_ascii() && text.chars().all(|c| c.is_ascii_digit()) {
        let value: i64 = text.parse().expect("纯数字");
        if (0..=65535).contains(&value) {
            Ok(value)
        } else {
            Err("Port out of range 0-65535".to_string())
        }
    } else {
        Err(format!(
            "Port could not be cast to integer value as {}",
            python_repr(text)
        ))
    }
}

/// Python `repr(str)` 的常用体例：单引号包裹、可打印字符原样（非 ASCII
/// 可打印原样）、控制字符/反斜杠/引号转义。
fn python_repr(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        match c {
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ if c < ' ' || c == '\u{7f}' => {
                let hex = format!("{:02x}", c as u32);
                out.push_str("\\x");
                out.push_str(&hex);
            }
            _ => out.push(c),
        }
    }
    out.push('\'');
    out
}

// ---------------------------------------------------------------------------
// unquote（errors='replace' 默认）与 PurePosixPath 归一化
// ---------------------------------------------------------------------------

/// 字节层镜像 `_unquote_impl`：逐 `%` 分段、十六进制对（两路大小写都收）、
/// 非法落字面 `'%'`；结果按 utf-8 replace 解码（`from_utf8_lossy`）。
fn unquote_ascii_run(run: &str) -> String {
    let bytes = run.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut pieces = bytes.split(|b| *b == b'%');
    out.extend_from_slice(pieces.next().unwrap());
    for item in pieces {
        if item.len() >= 2 && item[..2].iter().all(|b| b.is_ascii_hexdigit()) {
            let hex = std::str::from_utf8(&item[..2]).unwrap().to_lowercase();
            let value = u8::from_str_radix(&hex, 16).unwrap();
            out.push(value);
            out.extend_from_slice(&item[2..]);
        } else {
            out.push(b'%');
            out.extend_from_slice(item);
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 镜像 `urllib.parse.unquote`（str 版）：按 `_asciire` 切 ASCII 段（0x00-0x7f），
/// **每段独立**字节解码；非 ASCII 段原样穿过——UTF-8 序列不能跨界合并。
fn unquote(s: &str) -> String {
    if !s.as_bytes().contains(&b'%') {
        return s.to_string();
    }
    let mut out = String::new();
    let mut gap = String::new();
    let mut run = String::new();
    for c in s.chars() {
        if (c as u32) <= 0x7f {
            if !gap.is_empty() && run.is_empty() {
                // 段前的非 ASCII 间隙先产出
                out.push_str(&gap);
                gap.clear();
            }
            run.push(c);
        } else {
            if !run.is_empty() {
                out.push_str(&unquote_ascii_run(&run));
                run.clear();
            }
            gap.push(c);
        }
    }
    if !run.is_empty() {
        out.push_str(&unquote_ascii_run(&run));
        out.push_str(&gap);
    } else {
        out.push_str(&gap);
    }
    out
}

/// `str(PurePosixPath(p))`。
fn pure_posix_str(p: &str) -> String {
    if p.is_empty() {
        return ".".to_string();
    }
    // posixpath.splitroot：`//` 且第 3 个字符非 `/` → UNC 式双斜杠 drive
    let (root, rel) = if !p.starts_with('/') {
        ("", p)
    } else if !p[1..].starts_with('/') || p.get(2..3) == Some("/") {
        ("/", &p[1..])
    } else {
        ("//", &p[2..])
    };
    let tail: Vec<&str> = rel
        .split('/')
        .filter(|x| !x.is_empty() && *x != ".")
        .collect();
    if root.is_empty() && tail.is_empty() {
        ".".to_string()
    } else {
        format!("{root}{}", tail.join("/"))
    }
}

#[cfg(test)]
mod ftp_tests {
    use super::*;

    fn of(url: &str) -> FtpSource {
        parse_ftp_source(url, 21).unwrap()
    }

    fn err(url: &str) -> String {
        parse_ftp_source(url, 21).unwrap_err()
    }

    #[test]
    fn basic_fields() {
        let s = of("ftp://user:pw@host.com:8021/dir/file.txt");
        assert_eq!(s.scheme, "ftp");
        assert_eq!(s.host, "host.com");
        assert_eq!(s.port, 8021);
        assert_eq!(s.username, "user");
        assert_eq!(s.password, "pw");
        assert_eq!(s.source_path, "/dir/file.txt");
        assert!(s.has_port);
        // userinfo 空串原样保留（anonymous/anon@ 兜底在 pack.py sink 端）
        let s = of("ftp://host");
        assert_eq!(s.username, "");
        assert_eq!(s.password, "");
        assert_eq!(s.port, 21);
        assert!(!s.has_port);
        assert_eq!(s.source_path, "/");
        // user@host 空密码
        let s = of("ftp://user@host/");
        assert_eq!(s.username, "user");
        assert_eq!(s.password, "");
        // port or defaultPort：显式 0 也回退，hasPort 仍 True
        let s = of("ftp://host:0/x");
        assert_eq!(s.port, 21);
        assert!(s.has_port);
        // ':' 空 → None → 默认端口、hasPort False
        let s = of("ftp://host:/x");
        assert_eq!(s.port, 21);
        assert!(!s.has_port);
        // 前导零照 int 收
        assert_eq!(of("ftp://host:09/x").port, 9);
        // 前导零 09 → 9；65535 合法
        assert_eq!(of("ftp://host:65535/x").port, 65535);
    }

    #[test]
    fn userinfo_variants() {
        // ':' 首切产生空 username，原样保留
        assert_eq!(of("ftp://:pw@h/x").username, "");
        assert_eq!(of("ftp://:pw@h/x").password, "pw");
        assert_eq!(of("ftp://user:@h/x").password, "");
        assert_eq!(of("ftp://@h/x").username, "");
        assert_eq!(of("ftp://h@/x").host, "");
        assert_eq!(of("ftp://h@/x").username, "h");
        // password 取首个 ':' 之后（含残余 ':'）
        assert_eq!(of("ftp://user:p:w@host/x").password, "p:w");
        // '@' 只取最右一个
        assert_eq!(of("ftp://u@h/x?y#z@w").host, "h");
    }

    #[test]
    fn unquote_and_paths() {
        let s = of("ftp://u%40ser:p%40w@h%6Fst/pa%2Fth");
        assert_eq!(s.username, "u@ser");
        assert_eq!(s.password, "p@w");
        // hostname 不 unquote
        assert_eq!(s.host, "h%6Fst");
        // PurePosixPath 归一化
        assert_eq!(of("ftp://h/a//b/./c/../d/").source_path, "/a/b/c/../d");
        assert_eq!(of("ftp://h/..").source_path, "/..");
        // unquote 错误字节 → U+FFFD
        assert_eq!(of("ftp://h/pa%ffth").source_path, "/pa\u{fffd}th");
        // 非法 hex 落字面 '%'
        assert_eq!(of("ftp://h/pa%zzth").source_path, "/pa%zzth");
        // netloc 由首个 '/' 截断，'%C3%A4' 属 host
        let s = of("ftp://%C3%A4/%ee%");
        assert_eq!(s.host, "%C3%A4");
        assert_eq!(s.source_path, "/\u{fffd}%");
        // 段内非 ASCII：UTF-8 序列不能跨界合并
        assert_eq!(unquote("%C3\u{00e4}%A4"), "\u{fffd}\u{00e4}\u{fffd}");
        assert_eq!(unquote("a%C3A"), "a\u{fffd}A");
        // 多字节首字符的 scheme 检测不 panic
        assert_eq!(
            of("ftp\u{00e4}://host/x").source_path,
            "ftp\u{00e4}:/host/x"
        );
        assert_eq!(of("ftp%3a://host/x").source_path, "ftp::/host/x");
    }

    #[test]
    fn host_lower_and_zone() {
        assert_eq!(of("ftp://USER:PW@HOST/PATH").host, "host");
        assert_eq!(of("ftp://u@[2001:DB8::1]:21/p").host, "2001:db8::1");
        assert_eq!(of("ftp://[fe80::822a%eTh0]:21/k").host, "fe80::822a%eTh0");
        // '%25' 与 '%'-zone 都是字面穿过：头段小写、zone 原样
        assert_eq!(of("ftp://[fe80::1%25eTH0]:21/k").host, "fe80::1%25eTH0");
        assert_eq!(of("ftp://[v1.2.3%zone]/x").host, "v1.2.3%zone");
    }

    #[test]
    fn port_semantics() {
        // cast / range 错误文案逐字
        assert_eq!(
            err("ftp://host:abc/x"),
            "Port could not be cast to integer value as 'abc'"
        );
        assert_eq!(err("ftp://host:99999/x"), "Port out of range 0-65535");
        assert_eq!(
            err("ftp://host:-1/x"),
            "Port could not be cast to integer value as '-1'"
        );
        assert_eq!(
            err("ftp://h::21/x"),
            "Port could not be cast to integer value as ':21'"
        );
        // ' 21'：冒号后首个段（可能含空格）
        assert_eq!(
            err("ftp://host: 21/x"),
            "Port could not be cast to integer value as ' 21'"
        );
    }

    #[test]
    fn url_errors() {
        assert_eq!(
            err("ftp://[zzz]/x"),
            "'zzz' does not appear to be an IPv4 or IPv6 address"
        );
        assert_eq!(
            err("ftp://[1.2.3.4]/x"),
            "An IPv4 address cannot be in brackets"
        );
        assert_eq!(err("ftp://h[o]st/x"), "Invalid IPv6 URL");
        assert_eq!(err("ftp://[host/x"), "Invalid IPv6 URL");
        // userinfo 里出现 '[' → 括号前有数据
        assert_eq!(err("ftp://u:[2001:DB8::1]:21/p"), "Invalid IPv6 URL");
        assert_eq!(of("ftp://[v1.2.3.4.5.6.7]/x").host, "v1.2.3.4.5.6.7");
        assert_eq!(err("ftp://[vz.2]/x"), "IPvFuture address is invalid");
        assert_eq!(
            err("ftp://[ ]/x"),
            "' ' does not appear to be an IPv4 or IPv6 address"
        );
    }

    #[test]
    fn ipv6_semantics() {
        for good in [
            "::1",
            "::",
            "::ffff:1.2.3.4",
            "fe80::1%eth0",
            "2001:db8::8:800:200c:417a",
            "1:2:3:4:5:6:7:8",
            "aaaa:bbbb:cccc:dddd:eeee:ffff:0000:1111",
        ] {
            assert!(valid_ipv6(good), "{good} 应合法");
        }
        for bad in [
            ":",
            ":::",
            ":::1",
            ":::202",
            "1:2:3:4:5:6:7:1.2.3.4",
            "0:0:0:0:0:0:0:1.2.3.4",
            "::ffff:1.2.3.4.5",
            "::ffff:0.0.256.4",
            "1:2:3:4:5:6:7:8::",
            "1:2:3:4:5:6:7:8:9",
            "fe80::1%a%b",
            "fe80::1%",
            "1.2:1",
            "",
            "%zd",
            " ",
            "::2::1",
            "::ffff:1.2.3.04",
        ] {
            assert!(!valid_ipv6(bad), "{bad} 应非法");
        }
    }

    #[test]
    fn nfkc_netloc() {
        assert_eq!(
            err("ftp://\u{2100}host/x"),
            "netloc '\u{2100}host' contains invalid characters under NFKC normalization"
        );
        assert_eq!(
            err("ftp://\u{2100}host@host/x"),
            "netloc '\u{2100}host@host' contains invalid characters under NFKC normalization"
        );
        assert_eq!(
            err("ftp://host:21\u{2100}/x"),
            "netloc 'host:21\u{2100}' contains invalid characters under NFKC normalization"
        );
        // 非 NFKC 等价的非 ASCII（路径里）不触发
        assert_eq!(of("ftpä://host/x").scheme, "");
    }

    #[test]
    fn relative_and_no_netloc() {
        let s = of("ftp:host/path");
        assert!(s.host.is_empty());
        assert_eq!(s.source_path, "host/path");
        assert_eq!(s.username, "");
        assert_eq!(of("host/x").scheme, "");
        assert_eq!(of("host/x").source_path, "host/x");
        assert_eq!(of("ftp:/x").source_path, "/x");
        assert_eq!(of("ftp:/").source_path, "/");
        assert_eq!(of("ftp://").source_path, "/");
        assert_eq!(of("ftp://user@").username, "user");
    }

    #[test]
    fn spaces_fragments_queries() {
        // 尾部空格保留（urlsplit 只 lstrip）
        assert_eq!(of("ftp://host/x  ").source_path, "/x  ");
        assert_eq!(of("ftp://host/x\u{00a0}").source_path, "/x\u{00a0}");
        // 左端 C0+空格剥掉（NBSP 不算）
        assert_eq!(of(" ftp://host/x").source_path, "/x");
        assert_eq!(of("\tftp://host/x").source_path, "/x");
        // \t\r\n 全局剔除发生在 lstrip 之后
        let s = of("ftp\t://ho\nst@host:21/x");
        assert_eq!(s.host, "host");
        assert_eq!(s.username, "host");
        assert_eq!(s.port, 21);
        assert!(s.has_port);
        // 先剥 '#' 后剥 '?'
        assert_eq!(of("ftp://host/x?q#f").source_path, "/x");
        assert_eq!(of("ftp://host/x#f?q").source_path, "/x");
    }
}
