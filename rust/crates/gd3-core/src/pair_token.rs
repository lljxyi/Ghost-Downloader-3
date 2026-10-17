//! 浏览器扩展配对令牌（第 40 步）。
//!
//! 镜像 `app/services/browser_service.py` 的 `token_urlsafe(16)`：
//! 16 字节随机数经 base64url（无填充）编码，产出 22 字符令牌。
//! 随机源用 `uuid v4`（同为 16 密码学安全随机字节；与 Python
//! `secrets.token_urlsafe` 的随机强度、字母表、长度对齐），base64url
//! 编码与 `gd3-servers` 同用 `base64 0.22`。

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use uuid::Uuid;

/// 生成新的配对令牌（Python `token_urlsafe(16)` 口径）。
pub fn generate() -> String {
    URL_SAFE_NO_PAD.encode(Uuid::new_v4().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_matches_token_urlsafe16_shape() {
        let token = generate();
        assert_eq!(token.len(), 22);
        assert!(
            token
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }

    #[test]
    fn tokens_are_unpredictable_pairs() {
        let a = generate();
        let b = generate();
        assert_ne!(a, b);
        assert_eq!(b.len(), 22);
    }
}
