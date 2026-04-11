use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Minimal struct to deserialize auth-related sections from controller config.
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ControllerConfigAuth {
    auth: Option<OidcAuthSection>,
    local_auth: Option<LocalAuthSection>,
}

/// Minimal OIDC auth section — we only need to know if it exists.
#[derive(Debug, Deserialize)]
struct OidcAuthSection {
    #[serde(default = "default_true")]
    enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct LocalAuthSection {
    enabled: bool,
    username: String,
    password: String,
}

impl Default for LocalAuthSection {
    fn default() -> Self {
        Self {
            enabled: true,
            username: "admin".to_string(),
            password: String::new(),
        }
    }
}

/// Default paths to search for controller config, in order.
const DEFAULT_CONFIG_PATHS: &[&str] = &[
    "config/edgion-controller.toml",                   // dev: relative to CWD
    "/usr/local/edgion/config/edgion-controller.toml", // pod: standard install path
    "/etc/edgion/edgion-controller.toml",              // pod: alternative path
];

/// Try to find and read local_auth credentials from controller config.
///
/// Search order:
/// 1. Explicit path from --controller-config flag
/// 2. EDGION_CONTROLLER_CONFIG env var
/// 3. Default paths (CWD/config/, /usr/local/edgion/config/, /etc/edgion/)
///
/// Returns (username, password) if local_auth is configured and enabled.
///
/// Mirrors the controller's logic: when neither `[auth]` (OIDC) nor `[local_auth]`
/// is present, default local_auth credentials (admin/edgion1234) are used.
pub fn detect_credentials(explicit_path: Option<&str>) -> Option<(String, String)> {
    let config_path = resolve_config_path(explicit_path)?;

    let content = std::fs::read_to_string(&config_path).ok()?;
    let config: ControllerConfigAuth = toml::from_str(&content).ok()?;

    // If explicit [local_auth] section exists, use it (if it has a real password)
    if let Some(local_auth) = config.local_auth {
        if !local_auth.enabled {
            return None;
        }
        if !local_auth.password.is_empty() {
            return Some((local_auth.username, local_auth.password));
        }
        // [local_auth] exists but password is empty — credentials were auto-generated
        return read_generated_credentials(&content, &config_path);
    }

    // No [local_auth] section — check if OIDC [auth] is configured
    let oidc_enabled = config.auth.map_or(false, |a| a.enabled);
    if oidc_enabled {
        // OIDC is handling auth, no local credentials to use
        return None;
    }

    // Neither [auth] nor [local_auth] → controller auto-generates credentials.
    // Try to read the generated file from work_dir.
    if let Some(creds) = read_generated_credentials(&content, &config_path) {
        return Some(creds);
    }

    // Can't find generated credentials — user needs to login manually
    None
}

/// Read auto-generated credentials from {work_dir}/runtime/local_auth_generated.toml.
/// The work_dir is parsed from the same controller config file.
fn read_generated_credentials(config_content: &str, config_path: &Path) -> Option<(String, String)> {
    // Parse work_dir from the config
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct WorkDirConfig {
        work_dir: Option<String>,
    }

    let wd_config: WorkDirConfig = toml::from_str(config_content).ok()?;

    // Resolve work_dir with same priority as the controller:
    //   1. EDGION_WORK_DIR env var (set by test scripts, Docker entrypoint, etc.)
    //   2. work_dir from config file
    //   3. "." (default)
    let work_dir = if let Ok(env_wd) = std::env::var("EDGION_WORK_DIR") {
        PathBuf::from(env_wd)
    } else if let Some(wd) = &wd_config.work_dir {
        let p = PathBuf::from(wd);
        if p.is_absolute() {
            p
        } else {
            config_path
                .parent()
                .unwrap_or(Path::new("."))
                .join(p)
        }
    } else {
        config_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf()
    };

    let cred_file = work_dir
        .join("runtime")
        .join(crate::core::common::local_auth::config::GENERATED_CREDENTIALS_FILE);
    if !cred_file.exists() {
        return None;
    }

    let content = std::fs::read_to_string(&cred_file).ok()?;
    let local_auth: LocalAuthSection = toml::from_str(&content).ok()?;

    if local_auth.enabled && !local_auth.password.is_empty() {
        Some((local_auth.username, local_auth.password))
    } else {
        None
    }
}

/// Resolve controller config path from explicit, env, or default locations.
fn resolve_config_path(explicit: Option<&str>) -> Option<PathBuf> {
    // 1. Explicit flag
    if let Some(path) = explicit {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
        // Don't log warning - user explicitly specified, they'll see the error
        return None;
    }

    // 2. Environment variable
    if let Ok(path) = std::env::var("EDGION_CONTROLLER_CONFIG") {
        let p = PathBuf::from(&path);
        if p.exists() {
            return Some(p);
        }
    }

    // 3. Default paths
    for path in DEFAULT_CONFIG_PATHS {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }

    None
}
