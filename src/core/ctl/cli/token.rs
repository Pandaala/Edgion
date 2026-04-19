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

/// Delete the saved token file. Silently ignores errors (e.g., file doesn't exist).
pub fn delete_token() {
    if let Some(path) = token_file_path() {
        let _ = fs::remove_file(path);
    }
}
