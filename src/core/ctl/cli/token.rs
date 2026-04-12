use anyhow::Result;
use std::fs;
use std::path::PathBuf;

/// Get the path to the token file (~/.edgion/token)
pub fn token_file_path() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".edgion").join("token"))
}

/// Read saved token from file. Returns None if file doesn't exist or is empty.
pub fn read_token() -> Option<String> {
    let path = token_file_path()?;
    fs::read_to_string(&path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Save token to file. Creates ~/.edgion/ directory if needed.
pub fn save_token(token: &str) -> Result<()> {
    let path = token_file_path().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, token)?;
    Ok(())
}

/// Delete the saved token file. Silently ignores errors (e.g., file doesn't exist).
pub fn delete_token() {
    if let Some(path) = token_file_path() {
        let _ = fs::remove_file(path);
    }
}
