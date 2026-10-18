//! `app/checksum.py` 的 Rust 对应物。
//!
//! | Python | Rust |
//! |---|---|
//! | `parseChecksum`（:27-28） | [`parse_checksum`] |
//! | `matchChecksum`（:31-33） | [`match_checksum`] |
//! | `toChecksum`（:14-24） | [`to_checksum`] |
//! | `COMMON_ALGORITHMS`（:9） | [`COMMON_ALGORITHMS`] |
//! | `CHUNK_SIZE`（:11） | [`CHUNK_SIZE`] |
//! | `SHAKE_LENGTHS`（:10） | [`SHAKE_LENGTHS`]（**未实现**，见下） |
//! | `ALGORITHMS`（:8，`sorted(hashlib.algorithms_available)`） | **未迁移** |
//!
//! ## `parseChecksum` 的精确口径
//!
//! `(text.split() or [""])[0].lower()`：
//!
//! - `str.split()` 无参 = 按「一段连续空白」切、丢掉空段，所以前导空白不影响结果；
//! - 空白口径是 `str.isspace()`，**包含** `\x1c`-`\x1f`（Rust 的 `char::is_whitespace`
//!   不包含，故用私有的 `py_isspace`）；
//! - 全部是空白 → `[]` → 空串；
//! - 只取**第一段**：`"ABCDEF  sha256:..."` → `"abcdef"`。
//!
//! ## `toChecksum` 的精确口径
//!
//! - `size = path.stat().st_size` 在**开文件之前**取；进度是
//!   `done * 100 // size`（整数下取整）；
//! - `f.read(CHUNK_SIZE)` 返回的块**除非 EOF 否则一定是满的**（`BufferedReader`
//!   会补齐），所以 Rust 侧用 [`read_chunk`] 循环填满，否则进度回调的次数与取值
//!   会与 Python 不同；
//! - 空文件不产生任何块 → 不会触发 `done * 100 // size`（Python 那边也就不存在
//!   `ZeroDivisionError`）；
//! - `hexdigest()` 是小写十六进制；`shake_*` 走 `hexdigest(length)`。
//!
//! ## 为什么手写 SHA-256 / SHA-512
//!
//! 本机离线（`static.crates.io` 403），`~/.cargo/registry/cache` 里有 `md-5` 与
//! `sha1`（且已在 `Cargo.lock` 里），但**没有 `sha2`**。为了不让「计算校验值」这个
//! 功能在 Rust 侧对最常用的两个算法缺位，这里按 FIPS 180-4 实现了 SHA-256 / SHA-512，
//! 全部常量由 Python 按定义（素数立方根 / 平方根的小数部分）现算生成，并对着
//! `hashlib` 的向量钉住（空串、`"abc"`、55/56/63/64/111/112/127/128 字节边界、
//! 4MiB+7 的大文件）。**`sha2` crate 可用后应整体替换掉这两个私有模块**。
//!
//! `shake_128` / `shake_256` 是 Keccak，代码量与风险都远高于 SHA-2，本轮不实现：
//! [`to_checksum`] 对它们返回 [`ChecksumError::Unsupported`]。Python 侧它们是能算的
//! （`SHAKE_LENGTHS` 就是给它们用的），这是已知差异。
//!
//! ## 与 Python 的已知差异
//!
//! 1. `ALGORITHMS = sorted(hashlib.algorithms_available)` 是**环境相关**的（本机 40 项，
//!    取决于 OpenSSL 版本），无法跨平台复刻；Rust 侧改用 [`SUPPORTED_ALGORITHMS`]
//!    （= 能算的四种）。任务卡的算法菜单在 Python 里是 `COMMON_ALGORITHMS` 四项进主菜单、
//!    其余进「更多」，Rust 侧将只有主菜单那四项。
//! 2. `hashlib.new(algorithm)` 对未知算法抛 `ValueError: unsupported hash type xxx`；
//!    [`ChecksumError::Unsupported`] 的 `Display` 照抄这句话。
//! 3. `toChecksum` 在 Python 是 `async`（每块 `asyncio.to_thread`），Rust 是同步阻塞；
//!    取消靠调用方丢弃任务，不在本函数里做。

use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::Path;

use md5::{Digest as _, Md5};
use sha1::Sha1;

/// `4 * 1024 * 1024`：`toChecksum` 的读块大小，也是进度回调的粒度。
pub const CHUNK_SIZE: usize = 4 * 1024 * 1024;

/// `COMMON_ALGORITHMS`：任务卡主菜单里的四项（**保持 Python 的元组顺序**，不是排序后的）。
pub const COMMON_ALGORITHMS: [&str; 4] = ["md5", "sha1", "sha256", "sha512"];

/// Rust 侧能算的算法（见模块头「已知差异」第 1 条）。
pub const SUPPORTED_ALGORITHMS: [&str; 4] = COMMON_ALGORITHMS;

/// `SHAKE_LENGTHS`：`hexdigest(length)` 的长度。当前**未实现**，留着是为了让调用方
/// 能识别这两个名字并给出「Rust 侧暂不支持」的提示，而不是当成未知算法。
pub const SHAKE_LENGTHS: [(&str, usize); 2] = [("shake_128", 32), ("shake_256", 64)];

/// [`to_checksum`] 的失败原因。
#[derive(Debug)]
pub enum ChecksumError {
    /// 算法不在 [`SUPPORTED_ALGORITHMS`] 里（含 Python 支持、这里未实现的 `shake_*`）。
    Unsupported { algorithm: String },
    /// `stat` / `open` / `read` 失败（Python 那边这些异常原样冒出去）。
    Io(io::Error),
}

impl fmt::Display for ChecksumError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // CPython：`ValueError: unsupported hash type xxx`。
            Self::Unsupported { algorithm } => write!(f, "unsupported hash type {algorithm}"),
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ChecksumError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unsupported { .. } => None,
            Self::Io(error) => Some(error),
        }
    }
}

impl From<io::Error> for ChecksumError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// 镜像 `parseChecksum`：取第一段并转小写。
pub fn parse_checksum(text: &str) -> String {
    let mut first = String::new();
    for ch in text.chars() {
        if py_isspace(ch) {
            if !first.is_empty() {
                break;
            }
            continue;
        }
        first.push(ch);
    }
    first.to_lowercase()
}

/// 镜像 `matchChecksum`：按**插入顺序**找第一个值等于 `expected` 的算法。
///
/// Python 的入参是 `dict[str, str]`（保序），所以这里收 `(&算法, &值)` 的迭代器，
/// 调用方从 `serde_json::Map` 或 `BTreeMap` 现转。
pub fn match_checksum<'a, I>(checksums: I, expected: &str) -> Option<&'a str>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let expected = parse_checksum(expected);
    checksums
        .into_iter()
        .find(|(_, value)| *value == expected)
        .map(|(algorithm, _)| algorithm)
}

/// 镜像 `toChecksum`：边读边算，每读完一块回调一次进度百分比。
///
/// 返回小写十六进制摘要。`on_progress` 的入参是 `done * 100 / size`（0..=100）。
pub fn to_checksum(
    path: &Path,
    algorithm: &str,
    mut on_progress: impl FnMut(u64),
) -> Result<String, ChecksumError> {
    let mut hasher = Hasher::new(algorithm)?;
    // Python 在 `open` 之前 `stat`：文件不存在时先报 `FileNotFoundError`。
    let size = fs::metadata(path)?.len();
    let mut file = fs::File::open(path)?;
    let mut buffer = vec![0u8; CHUNK_SIZE];
    let mut done: u64 = 0;
    loop {
        let read = read_chunk(&mut file, &mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        done += read as u64;
        // `read > 0` 蕴含 `size > 0`，所以不会除零。
        on_progress(done.saturating_mul(100) / size);
    }
    Ok(hasher.hex_digest())
}

/// `f.read(n)` 的语义：除非 EOF，否则**填满** `buffer`。
///
/// `File::read` 允许短读，直接用会让进度回调的粒度与 Python 不一致。
fn read_chunk(file: &mut fs::File, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}

/// CPython `str.isspace()` 口径（`\x1c`-`\x1f` 也算空白）。
///
/// `gd3-core::pack` / `gd3-core::hls` / `gd3-ui::pystr` 各有一份同样的小工具；
/// 抽公共模块会牵动既有调用点，先按「谁用谁留」处理。
fn py_isspace(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

/// 小写十六进制（`hashlib.hexdigest()` 的输出）。
fn to_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[usize::from(byte >> 4)] as char);
        out.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    out
}

/// `hashlib.new(algorithm)` 的替身：把算法名映射到一个具体的摘要实现。
enum Hasher {
    Md5(Md5),
    Sha1(Sha1),
    Sha256(sha256::Sha256),
    Sha512(sha512::Sha512),
}

impl Hasher {
    fn new(algorithm: &str) -> Result<Self, ChecksumError> {
        // `hashlib.new` 的算法名大小写不敏感（走 OpenSSL 的 `EVP_get_digestbyname`），
        // 但 Python 侧调用点传进来的都是 `ALGORITHMS` 里的小写名。
        match algorithm {
            "md5" => Ok(Self::Md5(Md5::new())),
            "sha1" => Ok(Self::Sha1(Sha1::new())),
            "sha256" => Ok(Self::Sha256(sha256::Sha256::new())),
            "sha512" => Ok(Self::Sha512(sha512::Sha512::new())),
            other => Err(ChecksumError::Unsupported {
                algorithm: other.to_string(),
            }),
        }
    }

    fn update(&mut self, data: &[u8]) {
        match self {
            Self::Md5(hasher) => hasher.update(data),
            Self::Sha1(hasher) => hasher.update(data),
            Self::Sha256(hasher) => hasher.update(data),
            Self::Sha512(hasher) => hasher.update(data),
        }
    }

    fn hex_digest(self) -> String {
        match self {
            Self::Md5(hasher) => to_hex(&hasher.finalize()),
            Self::Sha1(hasher) => to_hex(&hasher.finalize()),
            Self::Sha256(hasher) => to_hex(&hasher.finalize()),
            Self::Sha512(hasher) => to_hex(&hasher.finalize()),
        }
    }
}

/// FIPS 180-4 的 SHA-256（`sha2` crate 离线不可用，见模块头）。
mod sha256 {
    const INIT: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    /// 前 64 个素数的立方根小数部分的前 32 位。
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    pub struct Sha256 {
        state: [u32; 8],
        buffer: [u8; 64],
        buffered: usize,
        /// 已喂入的**字节**数（填充用 `* 8` 换成位数）。
        length: u64,
    }

    impl Sha256 {
        pub fn new() -> Self {
            Self {
                state: INIT,
                buffer: [0u8; 64],
                buffered: 0,
                length: 0,
            }
        }

        pub fn update(&mut self, data: &[u8]) {
            self.length = self.length.wrapping_add(data.len() as u64);
            self.absorb(data);
        }

        pub fn finalize(mut self) -> [u8; 32] {
            let bits = self.length.wrapping_mul(8);
            // `1` + 若干 `0` + 8 字节大端长度，总长补齐到 64 的倍数。
            let zeros = (64 - (self.buffered + 9) % 64) % 64;
            let total = 1 + zeros + 8;
            let mut padding = [0u8; 72];
            padding[0] = 0x80;
            padding[total - 8..total].copy_from_slice(&bits.to_be_bytes());
            self.absorb(&padding[..total]);

            let mut out = [0u8; 32];
            for (index, word) in self.state.iter().enumerate() {
                out[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
            }
            out
        }

        /// 只更新状态与缓冲，**不**动 `length`（`finalize` 的填充走这里）。
        fn absorb(&mut self, mut data: &[u8]) {
            if self.buffered > 0 {
                let take = (64 - self.buffered).min(data.len());
                self.buffer[self.buffered..self.buffered + take].copy_from_slice(&data[..take]);
                self.buffered += take;
                data = &data[take..];
                if self.buffered == 64 {
                    let block = self.buffer;
                    self.compress(&block);
                    self.buffered = 0;
                }
            }
            while data.len() >= 64 {
                let mut block = [0u8; 64];
                block.copy_from_slice(&data[..64]);
                self.compress(&block);
                data = &data[64..];
            }
            if !data.is_empty() {
                self.buffer[..data.len()].copy_from_slice(data);
                self.buffered = data.len();
            }
        }

        fn compress(&mut self, block: &[u8; 64]) {
            let mut w = [0u32; 64];
            for (index, word) in w.iter_mut().take(16).enumerate() {
                *word = u32::from_be_bytes([
                    block[index * 4],
                    block[index * 4 + 1],
                    block[index * 4 + 2],
                    block[index * 4 + 3],
                ]);
            }
            for index in 16..64 {
                let s0 = w[index - 15].rotate_right(7)
                    ^ w[index - 15].rotate_right(18)
                    ^ (w[index - 15] >> 3);
                let s1 = w[index - 2].rotate_right(17)
                    ^ w[index - 2].rotate_right(19)
                    ^ (w[index - 2] >> 10);
                w[index] = w[index - 16]
                    .wrapping_add(s0)
                    .wrapping_add(w[index - 7])
                    .wrapping_add(s1);
            }

            let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
            for index in 0..64 {
                let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
                let ch = (e & f) ^ (!e & g);
                let temp1 = h
                    .wrapping_add(s1)
                    .wrapping_add(ch)
                    .wrapping_add(K[index])
                    .wrapping_add(w[index]);
                let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
                let maj = (a & b) ^ (a & c) ^ (b & c);
                let temp2 = s0.wrapping_add(maj);
                h = g;
                g = f;
                f = e;
                e = d.wrapping_add(temp1);
                d = c;
                c = b;
                b = a;
                a = temp1.wrapping_add(temp2);
            }

            for (slot, value) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
                *slot = slot.wrapping_add(value);
            }
        }
    }
}

/// FIPS 180-4 的 SHA-512（同上，`sha2` crate 离线不可用）。
mod sha512 {
    const INIT: [u64; 8] = [
        0x6a09e667f3bcc908,
        0xbb67ae8584caa73b,
        0x3c6ef372fe94f82b,
        0xa54ff53a5f1d36f1,
        0x510e527fade682d1,
        0x9b05688c2b3e6c1f,
        0x1f83d9abfb41bd6b,
        0x5be0cd19137e2179,
    ];

    /// 前 80 个素数的立方根小数部分的前 64 位。
    const K: [u64; 80] = [
        0x428a2f98d728ae22,
        0x7137449123ef65cd,
        0xb5c0fbcfec4d3b2f,
        0xe9b5dba58189dbbc,
        0x3956c25bf348b538,
        0x59f111f1b605d019,
        0x923f82a4af194f9b,
        0xab1c5ed5da6d8118,
        0xd807aa98a3030242,
        0x12835b0145706fbe,
        0x243185be4ee4b28c,
        0x550c7dc3d5ffb4e2,
        0x72be5d74f27b896f,
        0x80deb1fe3b1696b1,
        0x9bdc06a725c71235,
        0xc19bf174cf692694,
        0xe49b69c19ef14ad2,
        0xefbe4786384f25e3,
        0x0fc19dc68b8cd5b5,
        0x240ca1cc77ac9c65,
        0x2de92c6f592b0275,
        0x4a7484aa6ea6e483,
        0x5cb0a9dcbd41fbd4,
        0x76f988da831153b5,
        0x983e5152ee66dfab,
        0xa831c66d2db43210,
        0xb00327c898fb213f,
        0xbf597fc7beef0ee4,
        0xc6e00bf33da88fc2,
        0xd5a79147930aa725,
        0x06ca6351e003826f,
        0x142929670a0e6e70,
        0x27b70a8546d22ffc,
        0x2e1b21385c26c926,
        0x4d2c6dfc5ac42aed,
        0x53380d139d95b3df,
        0x650a73548baf63de,
        0x766a0abb3c77b2a8,
        0x81c2c92e47edaee6,
        0x92722c851482353b,
        0xa2bfe8a14cf10364,
        0xa81a664bbc423001,
        0xc24b8b70d0f89791,
        0xc76c51a30654be30,
        0xd192e819d6ef5218,
        0xd69906245565a910,
        0xf40e35855771202a,
        0x106aa07032bbd1b8,
        0x19a4c116b8d2d0c8,
        0x1e376c085141ab53,
        0x2748774cdf8eeb99,
        0x34b0bcb5e19b48a8,
        0x391c0cb3c5c95a63,
        0x4ed8aa4ae3418acb,
        0x5b9cca4f7763e373,
        0x682e6ff3d6b2b8a3,
        0x748f82ee5defb2fc,
        0x78a5636f43172f60,
        0x84c87814a1f0ab72,
        0x8cc702081a6439ec,
        0x90befffa23631e28,
        0xa4506cebde82bde9,
        0xbef9a3f7b2c67915,
        0xc67178f2e372532b,
        0xca273eceea26619c,
        0xd186b8c721c0c207,
        0xeada7dd6cde0eb1e,
        0xf57d4f7fee6ed178,
        0x06f067aa72176fba,
        0x0a637dc5a2c898a6,
        0x113f9804bef90dae,
        0x1b710b35131c471b,
        0x28db77f523047d84,
        0x32caab7b40c72493,
        0x3c9ebe0a15c9bebc,
        0x431d67c49c100d4c,
        0x4cc5d4becb3e42b6,
        0x597f299cfc657e2a,
        0x5fcb6fab3ad6faec,
        0x6c44198c4a475817,
    ];

    pub struct Sha512 {
        state: [u64; 8],
        buffer: [u8; 128],
        buffered: usize,
        /// 已喂入的**字节**数（`* 8` 换成位数，放进 16 字节大端长度）。
        length: u128,
    }

    impl Sha512 {
        pub fn new() -> Self {
            Self {
                state: INIT,
                buffer: [0u8; 128],
                buffered: 0,
                length: 0,
            }
        }

        pub fn update(&mut self, data: &[u8]) {
            self.length = self.length.wrapping_add(data.len() as u128);
            self.absorb(data);
        }

        pub fn finalize(mut self) -> [u8; 64] {
            let bits = self.length.wrapping_mul(8);
            let zeros = (128 - (self.buffered + 17) % 128) % 128;
            let total = 1 + zeros + 16;
            let mut padding = [0u8; 144];
            padding[0] = 0x80;
            padding[total - 16..total].copy_from_slice(&bits.to_be_bytes());
            self.absorb(&padding[..total]);

            let mut out = [0u8; 64];
            for (index, word) in self.state.iter().enumerate() {
                out[index * 8..index * 8 + 8].copy_from_slice(&word.to_be_bytes());
            }
            out
        }

        fn absorb(&mut self, mut data: &[u8]) {
            if self.buffered > 0 {
                let take = (128 - self.buffered).min(data.len());
                self.buffer[self.buffered..self.buffered + take].copy_from_slice(&data[..take]);
                self.buffered += take;
                data = &data[take..];
                if self.buffered == 128 {
                    let block = self.buffer;
                    self.compress(&block);
                    self.buffered = 0;
                }
            }
            while data.len() >= 128 {
                let mut block = [0u8; 128];
                block.copy_from_slice(&data[..128]);
                self.compress(&block);
                data = &data[128..];
            }
            if !data.is_empty() {
                self.buffer[..data.len()].copy_from_slice(data);
                self.buffered = data.len();
            }
        }

        fn compress(&mut self, block: &[u8; 128]) {
            let mut w = [0u64; 80];
            for (index, word) in w.iter_mut().take(16).enumerate() {
                let mut bytes = [0u8; 8];
                bytes.copy_from_slice(&block[index * 8..index * 8 + 8]);
                *word = u64::from_be_bytes(bytes);
            }
            for index in 16..80 {
                let s0 = w[index - 15].rotate_right(1)
                    ^ w[index - 15].rotate_right(8)
                    ^ (w[index - 15] >> 7);
                let s1 = w[index - 2].rotate_right(19)
                    ^ w[index - 2].rotate_right(61)
                    ^ (w[index - 2] >> 6);
                w[index] = w[index - 16]
                    .wrapping_add(s0)
                    .wrapping_add(w[index - 7])
                    .wrapping_add(s1);
            }

            let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
            for index in 0..80 {
                let s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
                let ch = (e & f) ^ (!e & g);
                let temp1 = h
                    .wrapping_add(s1)
                    .wrapping_add(ch)
                    .wrapping_add(K[index])
                    .wrapping_add(w[index]);
                let s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
                let maj = (a & b) ^ (a & c) ^ (b & c);
                let temp2 = s0.wrapping_add(maj);
                h = g;
                g = f;
                f = e;
                e = d.wrapping_add(temp1);
                d = c;
                c = b;
                b = a;
                a = temp1.wrapping_add(temp2);
            }

            for (slot, value) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
                *slot = slot.wrapping_add(value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = env::temp_dir().join(format!("gd3cs-{}-{tag}-{id}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// 内容 = `0..len` 的字节序列（`len <= 256` 时无重复）。
    fn counting_bytes(len: usize) -> Vec<u8> {
        (0..len).map(|index| (index % 256) as u8).collect()
    }

    /// `(内容, [md5, sha1, sha256, sha512])`，摘要全部由 CPython 3.13 的 `hashlib` 生成。
    ///
    /// 覆盖填充边界：55/56（SHA-256 的分块边界）、63/64、111/112/127/128（SHA-512）。
    fn hash_vectors() -> Vec<(Vec<u8>, [&'static str; 4])> {
        vec![
            (
                b"".to_vec(),
                [
                    "d41d8cd98f00b204e9800998ecf8427e",
                    "da39a3ee5e6b4b0d3255bfef95601890afd80709",
                    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                    "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e",
                ],
            ),
            (
                b"abc".to_vec(),
                [
                    "900150983cd24fb0d6963f7d28e17f72",
                    "a9993e364706816aba3e25717850c26c9cd0d89d",
                    "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
                    "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
                ],
            ),
            (
                b"hello world\n".to_vec(),
                [
                    "6f5902ac237024bdd0c176cb93063dc4",
                    "22596363b3de40b06f981fb85d82312e8c0ed511",
                    "a948904f2f0f479b8f8197694b30184b0d2ed1c1cd2a1ec0fb85d299a192a447",
                    "db3974a97f2407b7cae1ae637c0030687a11913274d578492558e39c16c017de84eacdc8c62fe34ee4e12b4b1428817f09b6a2760c3f8a664ceae94d2434a593",
                ],
            ),
            (
                counting_bytes(55),
                [
                    "6912ee65fff2d9f9ce2508cddf8bcda0",
                    "8ae2d46729cfe68ff927af5eec9c7d1b66d65ac2",
                    "463eb28e72f82e0a96c0a4cc53690c571281131f672aa229e0d45ae59b598b59",
                    "6856647f269c2ee3d8128f0b25427659d880641ef343300dd3cd4679168f58d6527fda70b4ebc854e2065e172b7d58c1536992c0810599259ba84a2b40c65414",
                ],
            ),
            (
                counting_bytes(56),
                [
                    "51fdd1acda72405dfdfa03fcb85896d7",
                    "636e2ec698dac903498e648bd2f3af641d3c88cb",
                    "da2ae4d6b36748f2a318f23e7ab1dfdf45acdc9d049bd80e59de82a60895f562",
                    "8b12b2f6fe400a51d29656e2b8c42a1bbfe6fcf3e425da430db05d1a2dda14790dee20fa8b22d8762afffe4988a5c98a4430d22a17e41e23d90fa61ab75671a9",
                ],
            ),
            (
                counting_bytes(63),
                [
                    "48a6295221902e8e0938f773a7185e72",
                    "6d942da0c4392b123528f2905c713a3ce28364bd",
                    "29af2686fd53374a36b0846694cc342177e428d1647515f078784d69cdb9e488",
                    "9dc9c5598e55dc42955695320839788e353f1d7f6ba74df74c80a8a52f463c0697f57f68835d1418f4ce9b6530cd79bd0f4c6f7e13c93feb1218c0b65c2c0561",
                ],
            ),
            (
                counting_bytes(64),
                [
                    "b2d3f56bc197fd985d5965079b5e7148",
                    "c6138d514ffa2135bfce0ed0b8fac65669917ec7",
                    "fdeab9acf3710362bd2658cdc9a29e8f9c757fcf9811603a8c447cd1d9151108",
                    "ee4320ebaf3fdb4f2c832b137200c08e235e0fa7bbd0eb1740c7063ba8a0d151da77e003398e1714a955d475b05e3e950b639503b452ec185de4229bc4873949",
                ],
            ),
            (
                counting_bytes(111),
                [
                    "4fad3ab7d8546851ec1bb63ea7e6f5a8",
                    "bc544e24573d592290fdaff8ecf3f7f2b00cd483",
                    "60780e9451bdc43cf4530ffc95cbb0c4eb24dae2c39f55f334d679e076c08065",
                    "a1a111449b198d9b1f538bad7f3fc1022b3a5b1a5e90a0bc860de8512746cbc31599e6c834de3a3235327af0b51ff57bf7acf1974a73014d9c3953812edc7c8d",
                ],
            ),
            (
                counting_bytes(112),
                [
                    "d1fec2ac3715e791ca5f489f300381b3",
                    "e4ce142d09a84a8645338dd6535cbfaaf800d320",
                    "09373f127d34e61dbbaa8bc4499c87074f2ddb10e1b465f506d7d70a15011979",
                    "c5fbd731d19d2ae1180f001be72c2c1aaba1d7b094b3748880e24593b8e117a750e11c1bd867cc2f96dace8c8b74abd2d5c4f236be444e77d30d1916174070b9",
                ],
            ),
            (
                counting_bytes(127),
                [
                    "8402b21e7bc7906493bae0dac017f1f9",
                    "89d7312a903f65cd2b3e34a975e55dbea9033353",
                    "92ca0fa6651ee2f97b884b7246a562fa71250fedefe5ebf270d31c546bfea976",
                    "eab89674feaa34e27aebeeff3c0a4d70070bb872d5e9f186cf1dbbdee517b6e35724d629ff025a5b07185e911ada7e3c8acf830aa0e4f71777bd2d44f504f7f0",
                ],
            ),
            (
                counting_bytes(128),
                [
                    "37eff01866ba3f538421b30b7cbefcac",
                    "e6434bc401f98603d7eda504790c98c67385d535",
                    "471fb943aa23c511f6f72f8d1652d9c880cfa392ad80503120547703e56a2be5",
                    "1dffd5e3adb71d45d2245939665521ae001a317a03720a45732ba1900ca3b8351fc5c9b4ca513eba6f80bc7b1d1fdad4abd13491cb824d61b08d8c0e1561b3f7",
                ],
            ),
        ]
    }

    #[test]
    fn to_checksum_matches_hashlib_vectors() {
        let dir = TempDir::new("vectors");
        for (index, (body, expected)) in hash_vectors().into_iter().enumerate() {
            let path = dir.path().join(format!("v{index}.bin"));
            fs::write(&path, &body).expect("write");
            for (slot, algorithm) in COMMON_ALGORITHMS.iter().enumerate() {
                let digest = to_checksum(&path, algorithm, |_| {}).expect("checksum");
                assert_eq!(
                    digest,
                    expected[slot],
                    "{algorithm} of {} bytes",
                    body.len()
                );
            }
        }
    }

    #[test]
    fn to_checksum_handles_a_multi_chunk_file() {
        // 4MiB + 7：跨过 `CHUNK_SIZE` 边界，覆盖「第二块不满」的分支。
        let body: Vec<u8> = (0..4 * 1024 * 1024 + 7)
            .map(|index| ((index * 37 + 11) % 251) as u8)
            .collect();
        let dir = TempDir::new("big");
        let path = dir.path().join("big.bin");
        fs::write(&path, &body).expect("write");

        let expected = [
            "2865d3832b3172d1b0918a1e7de5629e",
            "cc63ea831e803dd815c3fe7b8fdbbe0730c65001",
            "10337a63d3537bf9def3a12b3efa64f90ba80389b062256d56065eb88ea8c94c",
            "838114e16d0c1863c6fdffc6efd484c77590d376962e7ae1d40fa9cf9e3b06b1520517af805791936bbf0cba3b8aa049ba17ce6b04fa8f3c1da97722a90b4f9d",
        ];
        for (slot, algorithm) in COMMON_ALGORITHMS.iter().enumerate() {
            let digest = to_checksum(&path, algorithm, |_| {}).expect("checksum");
            assert_eq!(digest, expected[slot], "{algorithm}");
        }
    }

    #[test]
    fn to_checksum_reports_progress_per_chunk() {
        let dir = TempDir::new("progress");

        // 空文件：一块都读不到，回调不触发（Python 那边也就不会除零）。
        let empty = dir.path().join("empty.bin");
        fs::write(&empty, b"").expect("write");
        let mut seen = Vec::new();
        to_checksum(&empty, "sha256", |percent| seen.push(percent)).expect("checksum");
        assert!(seen.is_empty());

        // 3 字节：一次读满 → 100。
        let small = dir.path().join("small.bin");
        fs::write(&small, b"abc").expect("write");
        let mut seen = Vec::new();
        to_checksum(&small, "sha256", |percent| seen.push(percent)).expect("checksum");
        assert_eq!(seen, vec![100]);

        // 4MiB + 7：两块 → 99、100（`4194304 * 100 / 4194311` 下取整）。
        let big = dir.path().join("big.bin");
        fs::write(&big, vec![0u8; 4 * 1024 * 1024 + 7]).expect("write");
        let mut seen = Vec::new();
        to_checksum(&big, "md5", |percent| seen.push(percent)).expect("checksum");
        assert_eq!(seen, vec![99, 100]);
    }

    #[test]
    fn to_checksum_rejects_unsupported_algorithms() {
        let dir = TempDir::new("unsupported");
        let path = dir.path().join("a.bin");
        fs::write(&path, b"abc").expect("write");

        for algorithm in ["sha3_256", "shake_128", "shake_256", ""] {
            let error = to_checksum(&path, algorithm, |_| {}).expect_err("unsupported");
            match &error {
                ChecksumError::Unsupported { algorithm: name } => assert_eq!(name, algorithm),
                other => panic!("expected unsupported, got {other:?}"),
            }
            assert_eq!(
                error.to_string(),
                format!("unsupported hash type {algorithm}")
            );
        }
    }

    #[test]
    fn to_checksum_reports_io_errors() {
        let dir = TempDir::new("io");
        let error =
            to_checksum(&dir.path().join("absent.bin"), "md5", |_| {}).expect_err("missing file");
        match error {
            ChecksumError::Io(io_error) => {
                assert_eq!(io_error.kind(), io::ErrorKind::NotFound);
            }
            other => panic!("expected io error, got {other:?}"),
        }
    }

    #[test]
    fn parse_checksum_mirrors_python_split() {
        assert_eq!(parse_checksum(""), "");
        assert_eq!(parse_checksum("   \t\n"), "");
        assert_eq!(parse_checksum("ABCDEF"), "abcdef");
        assert_eq!(parse_checksum("  abcdef  "), "abcdef");
        // 只取第一段：`"ABCDEF sha256:..."` → `"abcdef"`。
        assert_eq!(parse_checksum("ABCDEF sha256:0000"), "abcdef");
        assert_eq!(parse_checksum("a\tb"), "a");
        // `\x1c`-`\x1f` 在 Python 里也算空白（Rust 的 `char::is_whitespace` 不算）。
        assert_eq!(parse_checksum("\u{1c}abc\u{1c}def"), "abc");
        // 全角空格是 Unicode White_Space，两侧都算空白。
        assert_eq!(parse_checksum("\u{3000}abc"), "abc");
    }

    #[test]
    fn match_checksum_returns_the_first_algorithm_in_insertion_order() {
        let checksums = [("md5", "aaaa"), ("sha256", "bbbb"), ("sha512", "bbbb")];
        assert_eq!(
            match_checksum(checksums.iter().copied(), "bbbb"),
            Some("sha256")
        );
        // `expected` 先过 `parseChecksum`：大小写与多余段落都不影响。
        assert_eq!(
            match_checksum(checksums.iter().copied(), "BBBB sha256:bbbb"),
            Some("sha256")
        );
        assert_eq!(match_checksum(checksums.iter().copied(), "cccc"), None);
        assert_eq!(match_checksum(checksums.iter().copied(), ""), None);
    }

    #[test]
    fn sha256_incremental_matches_one_shot() {
        for len in 0..200 {
            let body = counting_bytes(len);
            let mut one_shot = sha256::Sha256::new();
            one_shot.update(&body);
            let expected = to_hex(&one_shot.finalize());

            let mut byte_by_byte = sha256::Sha256::new();
            for byte in &body {
                byte_by_byte.update(std::slice::from_ref(byte));
            }
            assert_eq!(to_hex(&byte_by_byte.finalize()), expected, "len={len}");

            let mut chunked = sha256::Sha256::new();
            for chunk in body.chunks(7) {
                chunked.update(chunk);
            }
            assert_eq!(to_hex(&chunked.finalize()), expected, "len={len}");
        }
    }

    #[test]
    fn sha512_incremental_matches_one_shot() {
        for len in 0..300 {
            let body = counting_bytes(len);
            let mut one_shot = sha512::Sha512::new();
            one_shot.update(&body);
            let expected = to_hex(&one_shot.finalize());

            let mut byte_by_byte = sha512::Sha512::new();
            for byte in &body {
                byte_by_byte.update(std::slice::from_ref(byte));
            }
            assert_eq!(to_hex(&byte_by_byte.finalize()), expected, "len={len}");

            let mut chunked = sha512::Sha512::new();
            for chunk in body.chunks(13) {
                chunked.update(chunk);
            }
            assert_eq!(to_hex(&chunked.finalize()), expected, "len={len}");
        }
    }

    #[test]
    fn to_hex_is_lowercase_pairs() {
        assert_eq!(to_hex(&[0x00, 0x0f, 0xa0, 0xff]), "000fa0ff");
        assert_eq!(to_hex(&[]), "");
    }
}
