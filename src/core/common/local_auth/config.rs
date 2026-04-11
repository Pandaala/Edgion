//! Local authentication configuration.
//!
//! Provides built-in username/password authentication with JWT for the Admin API.
//! When no OIDC (`auth`) section is present and no `[local_auth]` is configured,
//! credentials are auto-generated and persisted to `{work_dir}/runtime/local_auth_generated.toml`.
//!
//! Example TOML:
//! ```toml
//! [local_auth]
//! enabled = true
//! username = "admin"
//! password = "my-secure-password"
//! jwt_secret = "change-me-in-production"
//! jwt_expiry_hours = 24
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

/// File name for auto-generated credentials, stored under `{work_dir}/runtime/`.
pub const GENERATED_CREDENTIALS_FILE: &str = "local_auth_generated.toml";

/// Local authentication configuration.
///
/// When absent from the config file and no OIDC `[auth]` section is configured,
/// local auth is automatically enabled with auto-generated credentials persisted
/// to `{work_dir}/runtime/local_auth_generated.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalAuthConfig {
    /// Enable local authentication. Default: true.
    pub enabled: bool,

    /// Username for local auth login. Default: "admin".
    pub username: String,

    /// Plaintext password (bcrypt hash is computed at startup).
    /// Empty string means credentials need to be generated.
    pub password: String,

    /// JWT signing secret. Empty string means credentials need to be generated.
    pub jwt_secret: String,

    /// JWT token expiry in hours. Default: 24.
    pub jwt_expiry_hours: u64,

    /// Paths that skip authentication (e.g., health/ready probes).
    /// `/api/v1/auth/` prefix paths are always skipped regardless of this list.
    #[serde(default = "default_skip_paths")]
    pub skip_paths: Vec<String>,
}

impl Default for LocalAuthConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            username: "admin".to_string(),
            password: String::new(),   // empty = needs generation
            jwt_secret: String::new(), // empty = needs generation
            jwt_expiry_hours: 24,
            skip_paths: default_skip_paths(),
        }
    }
}

impl LocalAuthConfig {
    /// Check if credentials need to be generated (password or jwt_secret is empty).
    pub fn needs_generation(&self) -> bool {
        self.password.is_empty() || self.jwt_secret.is_empty()
    }

    /// Validate the configuration. Returns None if valid, Some(error) if invalid.
    pub fn validate(&self) -> Option<&str> {
        if !self.enabled {
            return None;
        }
        if self.username.is_empty() {
            return Some("local_auth.username must not be empty");
        }
        if self.password.is_empty() {
            return Some("local_auth.password must not be empty");
        }
        if self.jwt_secret.is_empty() {
            return Some("local_auth.jwt_secret must not be empty");
        }
        if self.jwt_expiry_hours == 0 {
            return Some("local_auth.jwt_expiry_hours must be greater than 0");
        }
        None
    }

    /// Build a HashSet of skip paths for O(1) lookup in middleware.
    pub fn skip_paths_set(&self) -> HashSet<String> {
        self.skip_paths.iter().cloned().collect()
    }
}

/// Generate random credentials or load from existing file.
///
/// - If `{work_dir}/runtime/local_auth_generated.toml` exists, load it.
/// - Otherwise, generate random password + jwt_secret, save to file, and return.
///
/// Returns a complete `LocalAuthConfig` with real credentials.
pub fn generate_or_load(work_dir: &Path) -> anyhow::Result<LocalAuthConfig> {
    let runtime_dir = work_dir.join("runtime");
    let cred_file = runtime_dir.join(GENERATED_CREDENTIALS_FILE);

    // Try to load existing credentials
    if cred_file.exists() {
        let content = std::fs::read_to_string(&cred_file)?;
        let config: LocalAuthConfig = toml::from_str(&content)?;
        if !config.password.is_empty() && !config.jwt_secret.is_empty() {
            tracing::info!(
                component = "local_auth",
                file = %cred_file.display(),
                username = %config.username,
                "Loaded auto-generated credentials from file"
            );
            return Ok(config);
        }
    }

    // Generate new credentials
    let password = generate_random_string(20);
    let jwt_secret = generate_random_string(32);

    let config = LocalAuthConfig {
        enabled: true,
        username: "admin".to_string(),
        password: password.clone(),
        jwt_secret,
        jwt_expiry_hours: 24,
        skip_paths: default_skip_paths(),
    };

    // Save to file
    std::fs::create_dir_all(&runtime_dir)?;
    let content = toml::to_string_pretty(&config)?;
    std::fs::write(&cred_file, &content)?;

    tracing::info!(
        component = "local_auth",
        file = %cred_file.display(),
        "Auto-generated local auth credentials saved (see file for details)"
    );

    Ok(config)
}

/// Generate a random alphanumeric string using /dev/urandom (Unix)
/// or a fallback mixing system time if /dev/urandom is unavailable.
fn generate_random_string(len: usize) -> String {
    use std::io::Read;
    let mut buf = vec![0u8; len];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        if f.read_exact(&mut buf).is_ok() {
            return buf
                .iter()
                .map(|b| {
                    let charset = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
                    charset[(*b as usize) % charset.len()] as char
                })
                .collect();
        }
    }

    // Fallback: use system time mixing (less secure but functional)
    use std::time::SystemTime;
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut state = seed;
    (0..len)
        .map(|i| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407 + i as u128);
            let charset = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
            charset[((state >> 33) as usize) % charset.len()] as char
        })
        .collect()
}

fn default_skip_paths() -> Vec<String> {
    vec!["/health".to_string(), "/ready".to_string()]
}
