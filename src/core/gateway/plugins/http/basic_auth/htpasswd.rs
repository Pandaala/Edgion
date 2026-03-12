//! Self-contained htpasswd password verification.
//!
//! Replaces the `htpasswd-verify` crate to eliminate its vulnerable dependency
//! chain: `rust-crypto` (RUSTSEC-2022-0011), `rustc-serialize` (RUSTSEC-2022-0004),
//! and `time 0.1` (RUSTSEC-2020-0071).
//!
//! Supported hash formats:
//!   - Bcrypt   `$2a$` / `$2b$` / `$2y$`
//!   - SHA-1    `{SHA}<base64>`
//!   - APR1     `$apr1$salt$hash`   (Apache htpasswd MD5)
//!   - MD5-crypt `$1$salt$hash`     (BSD / Linux)

use base64::{engine::general_purpose, Engine as _};

/// Verify a plaintext password against a stored htpasswd hash string.
pub fn verify(password: &str, hash: &str) -> bool {
    if hash.starts_with("$2a$") || hash.starts_with("$2b$") || hash.starts_with("$2y$") {
        bcrypt::verify(password, hash).unwrap_or(false)
    } else if let Some(encoded) = hash.strip_prefix("{SHA}") {
        verify_sha1(password, encoded)
    } else if hash.starts_with("$apr1$") {
        verify_md5_variant(password, hash, b"$apr1$")
    } else if hash.starts_with("$1$") {
        verify_md5_variant(password, hash, b"$1$")
    } else {
        false
    }
}

// ──────────────────────────── SHA-1 ({SHA}) ────────────────────────────

fn verify_sha1(password: &str, encoded_hash: &str) -> bool {
    use sha1::Digest;
    let Ok(expected) = general_purpose::STANDARD.decode(encoded_hash) else {
        return false;
    };
    let actual = sha1::Sha1::digest(password.as_bytes());
    actual.as_slice() == expected.as_slice()
}

// ──────────────────── MD5-crypt / APR1 ($1$ / $apr1$) ──────────────────

const ITOA64: &[u8] = b"./0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

fn to64(buf: &mut Vec<u8>, mut v: u32, n: usize) {
    for _ in 0..n {
        buf.push(ITOA64[(v & 0x3f) as usize]);
        v >>= 6;
    }
}

fn verify_md5_variant(password: &str, hash: &str, magic: &[u8]) -> bool {
    let magic_str = std::str::from_utf8(magic).unwrap_or("");
    let after_magic = &hash[magic_str.len()..];
    let Some(dollar_pos) = after_magic.find('$') else {
        return false;
    };
    let salt = &after_magic[..dollar_pos];
    let computed = compute_md5_crypt(password.as_bytes(), salt.as_bytes(), magic);
    computed == hash
}

/// Implements the MD5-crypt algorithm (Poul-Henning Kamp, FreeBSD).
/// Used by both `$1$` (standard) and `$apr1$` (Apache) variants.
fn compute_md5_crypt(password: &[u8], salt: &[u8], magic: &[u8]) -> String {
    use md5::Digest;

    let salt = &salt[..salt.len().min(8)];

    // Step 1: primary context = password + magic + salt
    let mut ctx = md5::Md5::new();
    ctx.update(password);
    ctx.update(magic);
    ctx.update(salt);

    // Step 2: alternate = password + salt + password
    let mut alt = md5::Md5::new();
    alt.update(password);
    alt.update(salt);
    alt.update(password);
    let alt_result: [u8; 16] = alt.finalize().into();

    // Step 3: fold alternate digest into primary, password-length bytes
    let plen = password.len();
    let mut remaining = plen;
    while remaining > 16 {
        ctx.update(alt_result);
        remaining -= 16;
    }
    ctx.update(&alt_result[..remaining]);

    // Step 4: for each bit in password length (LSB first)
    let mut i = plen;
    while i > 0 {
        if i & 1 != 0 {
            ctx.update([0u8]);
        } else {
            ctx.update(&password[..1]);
        }
        i >>= 1;
    }

    let mut result: [u8; 16] = ctx.finalize().into();

    // Step 5: 1000 stretching rounds
    for round in 0..1000u32 {
        let mut r = md5::Md5::new();
        if round & 1 != 0 {
            r.update(password);
        } else {
            r.update(result);
        }
        if round % 3 != 0 {
            r.update(salt);
        }
        if round % 7 != 0 {
            r.update(password);
        }
        if round & 1 != 0 {
            r.update(result);
        } else {
            r.update(password);
        }
        result = r.finalize().into();
    }

    // Step 6: custom base64 encoding with specific byte interleaving
    let mut out = Vec::with_capacity(magic.len() + salt.len() + 1 + 22);
    out.extend_from_slice(magic);
    out.extend_from_slice(salt);
    out.push(b'$');

    to64(&mut out, ((result[0] as u32) << 16) | ((result[6] as u32) << 8) | (result[12] as u32), 4);
    to64(&mut out, ((result[1] as u32) << 16) | ((result[7] as u32) << 8) | (result[13] as u32), 4);
    to64(&mut out, ((result[2] as u32) << 16) | ((result[8] as u32) << 8) | (result[14] as u32), 4);
    to64(&mut out, ((result[3] as u32) << 16) | ((result[9] as u32) << 8) | (result[15] as u32), 4);
    to64(&mut out, ((result[4] as u32) << 16) | ((result[10] as u32) << 8) | (result[5] as u32), 4);
    to64(&mut out, result[11] as u32, 2);

    String::from_utf8(out).expect("base64 output is always valid UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bcrypt_valid() {
        let hash = bcrypt::hash("hello", 4).unwrap();
        assert!(verify("hello", &hash));
        assert!(!verify("wrong", &hash));
    }

    #[test]
    fn sha1_valid() {
        // htpasswd -nbs user password  →  {SHA}W6ph5Mm5Pz8GgiULbPgzG37mj9g=
        assert!(verify("password", "{SHA}W6ph5Mm5Pz8GgiULbPgzG37mj9g="));
        assert!(!verify("wrong", "{SHA}W6ph5Mm5Pz8GgiULbPgzG37mj9g="));
    }

    #[test]
    fn apr1_valid() {
        // Generated by: htpasswd -nbm user myPassword
        let hash = "$apr1$r31.....$HqJZimcKQFAMYayBlzkrA/";
        let computed = compute_md5_crypt(b"myPassword", b"r31.....", b"$apr1$");
        assert_eq!(computed, hash);
        assert!(verify("myPassword", hash));
        assert!(!verify("wrong", hash));
    }

    #[test]
    fn md5_crypt_roundtrip() {
        let hash = compute_md5_crypt(b"myPassword", b"sXiKzkus", b"$1$");
        assert!(hash.starts_with("$1$sXiKzkus$"));
        assert!(verify("myPassword", &hash));
        assert!(!verify("wrong", &hash));
    }

    #[test]
    fn unknown_format_rejects() {
        assert!(!verify("password", "plaintext_is_not_supported"));
    }
}
