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
    let raw = Zeroizing::new(fs::read_to_string(&path).ok()?);
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

#[cfg(test)]
#[path = "../../tests/unit/cli/token_test/gen_token_tests.rs"]
mod gen_token_tests;
