//! 浏览器扩展的 CRX 容器：读 `app/assets/chrome_extension.crx`，按 `zipfile.extractall`
//! 的口径解包到扩展目录。
//!
//! 这一步只做**容器**（第 121 步）：`loadCrx()` 读的字节、CRX v3 头部、ZIP 中央目录、
//! deflate 解压、路径净化、落盘顺序。四个平台副作用（剪贴板 / 商店 URL / 解包后弹框 /
//! 导出对话框）留给下一步接线。
//!
//! | Python 源 | 语义 | 这里 |
//! |---|---|---|
//! | `desktop.py:118` `loadCrx()` | `QResource(":/res/chrome_extension.crx")` 的字节 | [`crx_bytes`]（`include_bytes!`） |
//! | `browser_service.py:200` | `struct.unpack_from("<I", crx, 8)` 取头部长度 | [`payload`] |
//! | `browser_service.py:201` | `extensionFolder.mkdir(parents=True, exist_ok=True)` | [`unpack`] 第一句 |
//! | `browser_service.py:202-203` | `zipfile.ZipFile(...).extractall(folder)` | [`entries`] + [`unpack`] |
//! | `setting_page.py:487-495` | `QFile.write(loadCrx())` | [`export`] |
//!
//! ## 实测口径（`.venv/bin/python` 解真实 crx + 实读 CPython `zipfile`）
//!
//! - CRX 头：`Cr24` + 版本 3 + `headerSize`（小端 u32）→ 载荷从 `12 + headerSize` 开始。
//!   本仓的 crx：`headerSize = 1310`，载荷是标准 ZIP。
//! - 载荷 42 项 = **11 个目录 + 31 个文件**，全部 `compress_type = 8`（deflate）。
//!   目录靠**名字以 `/` 结尾**判定（`ZipInfo.is_dir()`），不看 `external_attr`。
//! - `extractall` **不还原权限与 mtime**：落盘就是 `mkdir` 的 `0o755` 与 `open()` 的
//!   `0o644`（umask 决定），mtime 是解包时刻。所以这里也不 `chmod` / `utime`。
//! - 顺序是 `infolist()` 序（= 中央目录序），父目录由 `os.makedirs(upperdirs)` 兜底，
//!   所以「文件先出现、目录项后出现」也能落地。
//! - 路径净化照抄 `_extract_member`（POSIX 分支）：`arcname = filename.replace('/', sep)`
//!   → `splitdrive`（POSIX 无操作）→ **丢掉 `''` / `'.'` / `'..'` 三个分量**再按 `/` 拼回。
//!   注意这是「丢分量」不是「解析」：`a/../b` → `a/b`，`/abs/x` → `abs/x`，
//!   而 `..\\..\\x` 在 POSIX 上**不拆反斜杠**，是单个文件名。
//! - `_extract_member` 在 `os.sep == '\\'` 时才走 `_sanitize_windows_name`；本模块只做
//!   POSIX 分支（与 `gd3-core::desktop` 的 `*_on(os, ..)` 家族同一思路，跨平台差异留待
//!   Windows 移植时补）。
//!
//! ## 刻意的偏离
//!
//! - **不校验 CRC32**：Python 的 `ZipExtFile` 读完会比对 CRC。这里只比对「解压后长度
//!   与中央目录里的 `file_size`」，因为扩展包是打进二进制的自家产物，CRC 校验加不上
//!   额外保障，却要再拖一个 `crc32fast` 依赖（已在锁里但非必需）。
//! - **不支持 ZIP64**：crx 只有 341 KB，Python 的 `allowZip64` 走不到这条路。
//! - **头部越界报错**：Python 的 `crx[12 + headerSize:]` 在越界时静默给空串（后面
//!   `BadZipFile`）；这里直接 [`CrxError::Truncated`]，错误信息更准。
//! - **`unpack` 返回落盘清单**：Python 的 `extractall` 无返回值。清单给下一步的
//!   「解包后弹安装引导」用，也方便单测断言顺序。

use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// 打进二进制的扩展包，镜像 `loadCrx()`（`app/platform/desktop.py:118`）读的
/// `:/res/chrome_extension.crx`（资源表见 `app/assets/resources.qrc`）。
pub const CRX: &[u8] = include_bytes!("../../../../app/assets/chrome_extension.crx");

/// CRX v3 的固定头长度：`Cr24` + 版本 + `headerSize`。
const CRX3_HEADER: usize = 12;

const CRX_MAGIC: [u8; 4] = *b"Cr24";

const EOCD_SIG: u32 = 0x0605_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const LOCAL_SIG: u32 = 0x0403_4b50;

/// 中央目录项固定部分长度。
const CENTRAL_HEADER: usize = 46;
/// 本地文件头固定部分长度。
const LOCAL_HEADER: usize = 30;
/// EOCD 固定部分长度（不含可变注释）。
const EOCD_HEADER: usize = 22;

/// 打进二进制的 crx 字节。
pub fn crx_bytes() -> &'static [u8] {
    CRX
}

/// 解包失败的原因。
#[derive(Debug)]
pub enum CrxError {
    /// 长度不够读某个结构。
    Truncated(&'static str),
    /// 魔数不是 `Cr24`。
    BadMagic([u8; 4]),
    /// CRX 版本不是 3（v2 的头部布局不同，本项目只发 v3）。
    UnsupportedVersion(u32),
    /// ZIP 结构有问题。
    Zip(&'static str),
    /// 压缩方法既不是 stored 也不是 deflate。
    UnsupportedMethod(u16),
    /// 落盘失败。
    Io(io::Error),
}

impl fmt::Display for CrxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CrxError::Truncated(what) => write!(f, "扩展包在 {what} 处被截断"),
            CrxError::BadMagic(magic) => write!(f, "扩展包魔数不是 Cr24：{magic:02x?}"),
            CrxError::UnsupportedVersion(v) => write!(f, "不支持的 CRX 版本：{v}"),
            CrxError::Zip(what) => write!(f, "扩展包结构损坏：{what}"),
            CrxError::UnsupportedMethod(m) => write!(f, "不支持的压缩方法：{m}"),
            CrxError::Io(err) => write!(f, "扩展包落盘失败：{err}"),
        }
    }
}

impl std::error::Error for CrxError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CrxError::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for CrxError {
    fn from(err: io::Error) -> Self {
        CrxError::Io(err)
    }
}

/// ZIP 中央目录里的一项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// 原始条目名（未净化，可能带 `/` 结尾）。
    pub name: String,
    /// 0 = stored，8 = deflate。
    pub method: u16,
    /// 压缩后长度（来自中央目录）。
    pub compressed_size: u64,
    /// 解压后长度（来自中央目录）。
    pub size: u64,
    /// 载荷内数据起点（已跳过本地头与它的名字/扩展字段）。
    pub data_start: usize,
    /// `name` 以 `/` 结尾 —— `ZipInfo.is_dir()` 的判定。
    pub is_dir: bool,
}

/// CRX → ZIP 载荷。镜像 `crx[12 + struct.unpack_from("<I", crx, 8)[0]:]`。
pub fn payload(crx: &[u8]) -> Result<&[u8], CrxError> {
    if crx.len() < CRX3_HEADER {
        return Err(CrxError::Truncated("CRX 头"));
    }
    let magic = [crx[0], crx[1], crx[2], crx[3]];
    if magic != CRX_MAGIC {
        return Err(CrxError::BadMagic(magic));
    }
    let version = read_u32(crx, 4, "CRX 版本")?;
    if version != 3 {
        return Err(CrxError::UnsupportedVersion(version));
    }
    let header = read_u32(crx, 8, "CRX 头长度")? as usize;
    let start = CRX3_HEADER
        .checked_add(header)
        .ok_or(CrxError::Truncated("CRX 头长度"))?;
    if start > crx.len() {
        return Err(CrxError::Truncated("CRX 载荷"));
    }
    Ok(&crx[start..])
}

/// 读 ZIP 载荷的中央目录（`infolist()` 的顺序）。
pub fn entries(zip: &[u8]) -> Result<Vec<Entry>, CrxError> {
    let eocd = find_eocd(zip)?;
    let total = read_u16(zip, eocd + 10, "EOCD 条目数")? as usize;
    let cd_size = read_u32(zip, eocd + 12, "EOCD 中央目录长度")? as usize;
    let cd_offset = read_u32(zip, eocd + 16, "EOCD 中央目录偏移")? as usize;
    if total == 0xffff || cd_size == 0xffff_ffff || cd_offset == 0xffff_ffff {
        return Err(CrxError::Zip("不支持 ZIP64"));
    }
    let cd_end = cd_offset
        .checked_add(cd_size)
        .ok_or(CrxError::Zip("中央目录越界"))?;
    if cd_end > zip.len() {
        return Err(CrxError::Zip("中央目录越界"));
    }

    let mut out = Vec::with_capacity(total);
    let mut at = cd_offset;
    for _ in 0..total {
        let header_end = at
            .checked_add(CENTRAL_HEADER)
            .ok_or(CrxError::Zip("中央目录项越界"))?;
        if header_end > cd_end {
            return Err(CrxError::Zip("中央目录项越界"));
        }
        if read_u32(zip, at, "中央目录项签名")? != CENTRAL_SIG {
            return Err(CrxError::Zip("中央目录项签名"));
        }
        let method = read_u16(zip, at + 10, "压缩方法")?;
        let compressed_size = read_u32(zip, at + 20, "压缩后长度")? as u64;
        let size = read_u32(zip, at + 24, "解压后长度")? as u64;
        let name_len = read_u16(zip, at + 28, "条目名长度")? as usize;
        let extra_len = read_u16(zip, at + 30, "条目扩展字段长度")? as usize;
        let comment_len = read_u16(zip, at + 32, "条目注释长度")? as usize;
        let local_offset = read_u32(zip, at + 42, "本地头偏移")? as usize;

        let name_end = header_end
            .checked_add(name_len)
            .ok_or(CrxError::Zip("条目名越界"))?;
        if name_end > cd_end {
            return Err(CrxError::Zip("条目名越界"));
        }
        let name = String::from_utf8_lossy(&zip[header_end..name_end]).into_owned();
        let data_start = local_data_start(zip, local_offset)?;
        let data_end = data_start
            .checked_add(compressed_size as usize)
            .ok_or(CrxError::Zip("条目数据越界"))?;
        if data_end > zip.len() {
            return Err(CrxError::Zip("条目数据越界"));
        }

        out.push(Entry {
            is_dir: name.ends_with('/'),
            name,
            method,
            compressed_size,
            size,
            data_start,
        });

        at = name_end
            .checked_add(extra_len)
            .and_then(|v| v.checked_add(comment_len))
            .ok_or(CrxError::Zip("中央目录项越界"))?;
    }
    Ok(out)
}

/// 读一项的**解压后**字节。
pub fn read_entry(zip: &[u8], entry: &Entry) -> Result<Vec<u8>, CrxError> {
    let end = entry
        .data_start
        .checked_add(entry.compressed_size as usize)
        .ok_or(CrxError::Zip("条目数据越界"))?;
    if end > zip.len() {
        return Err(CrxError::Zip("条目数据越界"));
    }
    let raw = &zip[entry.data_start..end];
    match entry.method {
        0 => Ok(raw.to_vec()),
        8 => {
            let mut out = Vec::with_capacity(entry.size as usize);
            flate2::read::DeflateDecoder::new(raw).read_to_end(&mut out)?;
            if out.len() as u64 != entry.size {
                return Err(CrxError::Zip("解压后长度与中央目录不符"));
            }
            Ok(out)
        }
        other => Err(CrxError::UnsupportedMethod(other)),
    }
}

/// 镜像 `_extract_member` 的 `arcname` 处理（POSIX 分支）：丢掉 `''` / `'.'` / `'..'`
/// 三个分量，其余按 `/` 拼回。**不拆反斜杠**（POSIX 的 `os.sep` 只有 `/`）。
pub fn sanitize(name: &str) -> PathBuf {
    let mut path = PathBuf::new();
    for part in name.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            continue;
        }
        path.push(part);
    }
    path
}

/// 解包到 `folder`，返回落盘的相对路径（中央目录序）。
///
/// 镜像 `install()` 的 `unpack()`：先 `mkdir(parents=True, exist_ok=True)`，再
/// `extractall`。父目录按需创建，所以「文件先于目录项出现」也能落地。
pub fn unpack(crx: &[u8], folder: &Path) -> Result<Vec<String>, CrxError> {
    let zip = payload(crx)?;
    let entries = entries(zip)?;
    fs::create_dir_all(folder)?;
    let mut written = Vec::with_capacity(entries.len());
    for entry in &entries {
        let relative = sanitize(&entry.name);
        if relative.as_os_str().is_empty() {
            // `arcname` 净化后为空：Python 会拿 `folder` 本身当目标（目录项时
            // `isdir` 直接通过，文件项时写 `folder` 会 `IsADirectoryError`）。
            // 自家扩展包里没有这种条目，静默跳过。
            continue;
        }
        let target = folder.join(&relative);
        if entry.is_dir {
            fs::create_dir_all(&target)?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&target, read_entry(zip, entry)?)?;
        }
        written.push(relative.to_string_lossy().into_owned());
    }
    Ok(written)
}

/// 把 crx 原样写到 `path`，镜像 `_onExportExtensionClicked`（`setting_page.py:487-495`）
/// 的 `f.write(loadCrx())`。
pub fn export(crx: &[u8], path: &Path) -> io::Result<()> {
    fs::write(path, crx)
}

/// 从尾部扫 EOCD（注释最长 65535 字节，`zipfile._EndRecData` 同口径）。
fn find_eocd(zip: &[u8]) -> Result<usize, CrxError> {
    if zip.len() < EOCD_HEADER {
        return Err(CrxError::Truncated("ZIP 尾部"));
    }
    let last = zip.len() - EOCD_HEADER;
    let first = last.saturating_sub(0xffff);
    for at in (first..=last).rev() {
        if read_u32(zip, at, "EOCD 签名")? == EOCD_SIG {
            return Ok(at);
        }
    }
    Err(CrxError::Zip("找不到 EOCD"))
}

/// 本地文件头 → 数据起点。
fn local_data_start(zip: &[u8], local_offset: usize) -> Result<usize, CrxError> {
    let header_end = local_offset
        .checked_add(LOCAL_HEADER)
        .ok_or(CrxError::Zip("本地头越界"))?;
    if header_end > zip.len() {
        return Err(CrxError::Zip("本地头越界"));
    }
    if read_u32(zip, local_offset, "本地头签名")? != LOCAL_SIG {
        return Err(CrxError::Zip("本地头签名"));
    }
    let name_len = read_u16(zip, local_offset + 26, "本地头名字长度")? as usize;
    let extra_len = read_u16(zip, local_offset + 28, "本地头扩展字段长度")? as usize;
    let start = header_end
        .checked_add(name_len)
        .and_then(|v| v.checked_add(extra_len))
        .ok_or(CrxError::Zip("本地头越界"))?;
    if start > zip.len() {
        return Err(CrxError::Zip("本地头越界"));
    }
    Ok(start)
}

fn read_u16(bytes: &[u8], at: usize, what: &'static str) -> Result<u16, CrxError> {
    let slice = bytes.get(at..at + 2).ok_or(CrxError::Truncated(what))?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(bytes: &[u8], at: usize, what: &'static str) -> Result<u32, CrxError> {
    let slice = bytes.get(at..at + 4).ok_or(CrxError::Truncated(what))?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("gd3_crx_{}_{}_{tag}", std::process::id(), n));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    /// 递归收集 `dir` 下的（目录, 文件）相对路径。
    fn tree(dir: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        fn walk(root: &Path, at: &Path, dirs: &mut Vec<PathBuf>, files: &mut Vec<PathBuf>) {
            for entry in fs::read_dir(at).expect("读目录") {
                let path = entry.expect("目录项").path();
                let relative = path.strip_prefix(root).expect("相对路径").to_path_buf();
                if path.is_dir() {
                    dirs.push(relative);
                    walk(root, &path, dirs, files);
                } else {
                    files.push(relative);
                }
            }
        }
        walk(dir, dir, &mut dirs, &mut files);
        dirs.sort();
        files.sort();
        (dirs, files)
    }

    /// 造一个最小 ZIP：只有本地头 + 中央目录 + EOCD，不写 CRC（本模块不校验）。
    struct ZipBuilder {
        local: Vec<u8>,
        central: Vec<u8>,
        count: u16,
    }

    impl ZipBuilder {
        fn new() -> Self {
            Self {
                local: Vec::new(),
                central: Vec::new(),
                count: 0,
            }
        }

        fn dir(self, name: &str) -> Self {
            let name = if name.ends_with('/') {
                name.to_string()
            } else {
                format!("{name}/")
            };
            self.push(&name, 0, &[], 0)
        }

        fn stored(self, name: &str, data: &[u8]) -> Self {
            self.push(name, 0, data, data.len())
        }

        fn deflated(self, name: &str, data: &[u8]) -> Self {
            use std::io::Write;
            let mut encoder =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(data).expect("deflate");
            let packed = encoder.finish().expect("deflate finish");
            self.push(name, 8, &packed, data.len())
        }

        fn raw(self, name: &str, method: u16, packed: &[u8], size: usize) -> Self {
            self.push(name, method, packed, size)
        }

        fn push(mut self, name: &str, method: u16, packed: &[u8], size: usize) -> Self {
            let offset = self.local.len() as u32;
            let name_bytes = name.as_bytes();
            let mut header = Vec::with_capacity(LOCAL_HEADER + name_bytes.len());
            header.extend_from_slice(&LOCAL_SIG.to_le_bytes());
            header.extend_from_slice(&20u16.to_le_bytes()); // version needed
            header.extend_from_slice(&0u16.to_le_bytes()); // flags
            header.extend_from_slice(&method.to_le_bytes());
            header.extend_from_slice(&0u16.to_le_bytes()); // mod time
            header.extend_from_slice(&0u16.to_le_bytes()); // mod date
            header.extend_from_slice(&0u32.to_le_bytes()); // crc（不校验）
            header.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            header.extend_from_slice(&(size as u32).to_le_bytes());
            header.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            header.extend_from_slice(&0u16.to_le_bytes()); // extra 长度
            header.extend_from_slice(name_bytes);
            self.local.extend_from_slice(&header);
            self.local.extend_from_slice(packed);

            self.central.extend_from_slice(&CENTRAL_SIG.to_le_bytes());
            self.central.extend_from_slice(&20u16.to_le_bytes()); // version made by
            self.central.extend_from_slice(&20u16.to_le_bytes()); // version needed
            self.central.extend_from_slice(&0u16.to_le_bytes()); // flags
            self.central.extend_from_slice(&method.to_le_bytes());
            self.central.extend_from_slice(&0u16.to_le_bytes()); // mod time
            self.central.extend_from_slice(&0u16.to_le_bytes()); // mod date
            self.central.extend_from_slice(&0u32.to_le_bytes()); // crc
            self.central
                .extend_from_slice(&(packed.len() as u32).to_le_bytes());
            self.central.extend_from_slice(&(size as u32).to_le_bytes());
            self.central
                .extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            self.central.extend_from_slice(&0u16.to_le_bytes()); // extra
            self.central.extend_from_slice(&0u16.to_le_bytes()); // comment
            self.central.extend_from_slice(&0u16.to_le_bytes()); // disk start
            self.central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            self.central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
            self.central.extend_from_slice(&offset.to_le_bytes());
            self.central.extend_from_slice(name_bytes);

            self.count += 1;
            self
        }

        fn finish(self) -> Vec<u8> {
            let cd_offset = self.local.len() as u32;
            let cd_size = self.central.len() as u32;
            let mut out = self.local;
            out.extend_from_slice(&self.central);
            out.extend_from_slice(&EOCD_SIG.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // 本盘号
            out.extend_from_slice(&0u16.to_le_bytes()); // 中央目录所在盘
            out.extend_from_slice(&self.count.to_le_bytes());
            out.extend_from_slice(&self.count.to_le_bytes());
            out.extend_from_slice(&cd_size.to_le_bytes());
            out.extend_from_slice(&cd_offset.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // 注释长度
            out
        }
    }

    /// 给 ZIP 载荷套一个 CRX v3 头（头部内容本模块不解析）。
    fn wrap(payload: &[u8]) -> Vec<u8> {
        let header = b"protobuf-header".as_slice();
        let mut out = Vec::new();
        out.extend_from_slice(&CRX_MAGIC);
        out.extend_from_slice(&3u32.to_le_bytes());
        out.extend_from_slice(&(header.len() as u32).to_le_bytes());
        out.extend_from_slice(header);
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn the_bundled_crx_is_a_v3_container() {
        let crx = crx_bytes();
        assert_eq!(crx.len(), 341_464);
        assert_eq!(&crx[0..4], b"Cr24");
        assert_eq!(read_u32(crx, 4, "版本").unwrap(), 3);
        // 实测头部长度（Python `struct.unpack_from("<I", crx, 8)` 同一值）。
        assert_eq!(read_u32(crx, 8, "头长度").unwrap(), 1310);
        let payload = payload(crx).unwrap();
        assert_eq!(payload.len(), 341_464 - 12 - 1310);
        assert_eq!(&payload[0..4], &LOCAL_SIG.to_le_bytes());
    }

    #[test]
    fn the_bundled_crx_lists_42_entries() {
        let zip = payload(crx_bytes()).unwrap();
        let entries = entries(zip).unwrap();
        assert_eq!(entries.len(), 42);
        assert_eq!(entries.iter().filter(|e| e.is_dir).count(), 11);
        assert_eq!(entries.iter().filter(|e| !e.is_dir).count(), 31);
        // 全部 deflate（实测 `compress_type = 8`）。
        assert!(entries.iter().all(|e| e.method == 8));
        // 中央目录序的第一项是 `_locales/`，不是字典序。
        assert_eq!(entries[0].name, "_locales/");
        assert!(entries[0].is_dir);
        let manifest = entries
            .iter()
            .find(|e| e.name == "manifest.json")
            .expect("有 manifest.json");
        assert!(!manifest.is_dir);
        assert_eq!(manifest.size, 2084);
    }

    #[test]
    fn the_bundled_crx_unpacks_like_python() {
        let dir = temp_dir("real");
        let folder = dir.join("browser_extension");
        let written = unpack(crx_bytes(), &folder).unwrap();
        assert_eq!(written.len(), 42);
        assert_eq!(written[0], "_locales");

        let (dirs, files) = tree(&folder);
        assert_eq!(dirs.len(), 11);
        assert_eq!(files.len(), 31);
        // 实测字节数（Python `extractall` 之后的 `stat().st_size`）。
        assert_eq!(
            fs::metadata(folder.join("manifest.json")).unwrap().len(),
            2084
        );
        assert_eq!(
            fs::metadata(folder.join("background.js")).unwrap().len(),
            101_558
        );
        let manifest = fs::read(folder.join("manifest.json")).unwrap();
        assert_eq!(manifest[0], b'{');
        // 目录项也真的建了目录。
        assert!(folder.join("_locales/en/messages.json").is_file());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unpacking_twice_leaves_the_same_tree() {
        let dir = temp_dir("twice");
        let folder = dir.join("browser_extension");
        let first = unpack(crx_bytes(), &folder).unwrap();
        let before = tree(&folder);
        let second = unpack(crx_bytes(), &folder).unwrap();
        assert_eq!(first, second);
        assert_eq!(tree(&folder), before);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sanitize_drops_python_invalid_parts() {
        // 表来自 CPython `zipfile.ZipFile._extract_member` 的 POSIX 分支。
        assert_eq!(sanitize("a/b"), PathBuf::from("a/b"));
        assert_eq!(sanitize("a/../b"), PathBuf::from("a/b"));
        assert_eq!(sanitize("/abs/x"), PathBuf::from("abs/x"));
        assert_eq!(sanitize("./x"), PathBuf::from("x"));
        assert_eq!(sanitize("a//b"), PathBuf::from("a/b"));
        assert_eq!(sanitize("a/"), PathBuf::from("a"));
        assert_eq!(sanitize(""), PathBuf::new());
        assert_eq!(sanitize("../.."), PathBuf::new());
        // POSIX 不拆反斜杠：整串是一个文件名。
        assert_eq!(sanitize("..\\..\\x"), PathBuf::from("..\\..\\x"));
    }

    #[test]
    fn a_bad_magic_is_rejected() {
        let mut crx = crx_bytes().to_vec();
        crx[0] = b'X';
        assert!(matches!(payload(&crx), Err(CrxError::BadMagic(_))));
    }

    #[test]
    fn a_short_or_odd_header_is_rejected() {
        assert!(matches!(payload(b"Cr24"), Err(CrxError::Truncated(_))));
        let crx = crx_bytes()[..20].to_vec();
        assert!(matches!(payload(&crx), Err(CrxError::Truncated(_))));
        // 版本 2 的头部布局不同，明确拒绝而不是瞎猜。
        let mut v2 = crx_bytes().to_vec();
        v2[4..8].copy_from_slice(&2u32.to_le_bytes());
        assert!(matches!(payload(&v2), Err(CrxError::UnsupportedVersion(2))));
    }

    #[test]
    fn stored_entries_are_copied_verbatim() {
        let zip = ZipBuilder::new()
            .dir("assets")
            .stored("assets/a.txt", b"hello")
            .finish();
        let dir = temp_dir("stored");
        let written = unpack(&wrap(&zip), &dir).unwrap();
        assert_eq!(
            written,
            vec!["assets".to_string(), "assets/a.txt".to_string()]
        );
        assert_eq!(fs::read(dir.join("assets/a.txt")).unwrap(), b"hello");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn deflated_entries_are_inflated() {
        let text = "幽灵下载器".repeat(200);
        let zip = ZipBuilder::new()
            .deflated("manifest.json", text.as_bytes())
            .finish();
        let crx = wrap(&zip);
        let zip = payload(&crx).unwrap();
        let entries = entries(zip).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].size as usize, text.len());
        // 压缩后确实变小了（不然这条测试等于在测 stored 分支）。
        assert!((entries[0].compressed_size as usize) < text.len());
        assert_eq!(read_entry(zip, &entries[0]).unwrap(), text.as_bytes());
    }

    #[test]
    fn an_unsupported_method_is_rejected() {
        let zip = ZipBuilder::new().raw("a.bin", 12, b"whatever", 8).finish();
        let dir = temp_dir("method");
        assert!(matches!(
            unpack(&wrap(&zip), &dir),
            Err(CrxError::UnsupportedMethod(12))
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn escaping_paths_are_clamped_to_the_folder() {
        let zip = ZipBuilder::new()
            .stored("../evil.txt", b"x")
            .stored("/abs.txt", b"y")
            .stored("a/../../b.txt", b"z")
            .finish();
        let dir = temp_dir("escape");
        let folder = dir.join("ext");
        let written = unpack(&wrap(&zip), &folder).unwrap();
        assert_eq!(
            written,
            vec![
                "evil.txt".to_string(),
                "abs.txt".to_string(),
                "a/b.txt".to_string()
            ]
        );
        assert!(folder.join("evil.txt").is_file());
        assert!(folder.join("abs.txt").is_file());
        assert!(folder.join("a/b.txt").is_file());
        // 上级目录没有被写。
        assert!(!dir.join("evil.txt").exists());
        assert!(!dir.join("abs.txt").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_eocd_is_rejected() {
        let zip = ZipBuilder::new().stored("a.txt", b"x").finish();
        let truncated = &zip[..zip.len() - 10];
        assert!(matches!(entries(truncated), Err(CrxError::Zip(_))));
        assert!(matches!(entries(b"PK"), Err(CrxError::Truncated(_))));
    }

    #[test]
    fn export_writes_the_bundled_bytes() {
        let dir = temp_dir("export");
        let path = dir.join("Extension.crx");
        export(crx_bytes(), &path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), crx_bytes());
        let _ = fs::remove_dir_all(&dir);
    }
}
