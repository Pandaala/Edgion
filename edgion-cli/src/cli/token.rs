use anyhow::Result;
use std::fs;
use std::path::PathBuf;
use zeroize::Zeroizing;

/// Get the path to the token file (~/.edgion/token)
pub fn token_file_path() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".edgion").join("token"))
}

/// Read saved token from file. Returns None if file doesn't exist or is empty.
///
/// Token is wrapped in `Zeroizing` so the buffer is wiped when dropped, reducing the
/// window a core dump / swap page could expose the bearer credential.
pub fn read_token() -> Option<Zeroizing<String>> {
    let path = token_file_path()?;
    let raw = fs::read_to_string(&path).ok()?;
    let trimmed = raw.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(Zeroizing::new(trimmed))
    }
}

/// Save token to file. Creates ~/.edgion/ directory if needed.
///
/// On Unix, parent directory is set to 0700 and the token file is created with mode
/// 0600 so other local accounts on multi-user hosts cannot read the bearer credential.
pub fn save_token(token: &str) -> Result<()> {
    let path = token_file_path().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)?;
        file.write_all(token.as_bytes())?;
    }
    #[cfg(not(unix))]
    {
        fs::write(&path, token)?;
    }
    Ok(())
}

/// Delete the saved token file. Silently ignores NotFound; prints a warning for other errors.
pub fn delete_token() {
    if let Some(path) = token_file_path() {
        if let Err(e) = fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                eprintln!("warning: failed to delete token file: {e}");
            }
        }
    }
}

/// Generate a new CLI token and print it with its SHA-256 digest.
///
/// The token is 32 CSPRNG bytes encoded as URL-safe base64 (no padding) with an
/// `edg_` prefix.  The SHA-256 is computed over the **full `edg_`-prefixed token**,
/// which is the exact `Authorization: Bearer <token>` string the CLI sends.  Paste
/// the SHA-256 value into the controller `cli_tokens[].token_sha256`.
///
/// Does not contact the controller.
pub fn gen_token() -> Result<()> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine as _;
    use rand::RngCore as _;
    use sha2::{Digest, Sha256};

    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let b64 = URL_SAFE_NO_PAD.encode(bytes);
    let token = format!("edg_{b64}");

    let mut h = Sha256::new();
    h.update(token.as_bytes());
    let sha256 = hex::encode(h.finalize());

    println!("Token (use with --token or EDGION_TOKEN):");
    println!("  {token}");
    println!();
    println!("SHA-256 (paste into controller cli_tokens[].token_sha256):");
    println!("  {sha256}");

    Ok(())
}

/// Compute the lowercase hex-encoded SHA-256 digest of `token`.
///
/// Mirrors `controller::authz::sha256_hex` — used in tests to verify that the
/// full `edg_`-prefixed token is what gets hashed.
pub fn sha256_hex_of(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hex::encode(h.finalize())
}

#[cfg(test)]
mod gen_token_tests {
    use super::*;

    /// `gen_token` produces a token whose sha256 matches `sha256_hex_of(full_token)`.
    /// We verify the invariant by re-implementing the generation inline.
    #[test]
    fn sha256_covers_full_edg_prefixed_token() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use base64::Engine as _;
        use rand::RngCore as _;

        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        let b64 = URL_SAFE_NO_PAD.encode(bytes);
        let token = format!("edg_{b64}");

        let hash = sha256_hex_of(&token);

        // Token must start with "edg_"
        assert!(token.starts_with("edg_"), "token must start with edg_: {token}");

        // SHA-256 must be 64 lowercase hex chars (digits 0-9 or letters a-f, never A-F)
        assert_eq!(hash.len(), 64, "sha256 must be 64 hex chars: {hash}");
        assert!(
            hash.chars().all(|c| c.is_ascii_digit() || matches!(c, 'a'..='f')),
            "sha256 must be lowercase hex only (no uppercase): {hash}"
        );

        // Re-hashing the same token must produce the same digest
        assert_eq!(sha256_hex_of(&token), hash);

        // Hashing just the suffix (without edg_) must differ — the full prefix is included
        let without_prefix = &token["edg_".len()..];
        assert_ne!(
            sha256_hex_of(without_prefix),
            hash,
            "hash must cover the full edg_-prefixed token, not just the suffix"
        );
    }

    /// Token length: 32 bytes base64url → 43 chars + "edg_" = 47 chars
    #[test]
    fn token_has_expected_length() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use base64::Engine as _;
        use rand::RngCore as _;

        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        let token = format!("edg_{}", URL_SAFE_NO_PAD.encode(bytes));
        // 32 bytes base64url no-pad = ceil(32*4/3) = 43 chars; prefix "edg_" = 4; total = 47
        assert_eq!(token.len(), 47, "token length: {}", token.len());
    }
}
