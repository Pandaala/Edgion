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

/// Read auto-generated credentials from the Secret YAML file in conf_dir.
/// The conf_dir is parsed from the same controller config file.
#[derive(Deserialize, Default)]
#[serde(default)]
struct ConfCenterSection {
    #[serde(rename = "type")]
    center_type: Option<String>,
    conf_dir: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PartialConfig {
    conf_center: Option<ConfCenterSection>,
}

fn read_generated_credentials(config_content: &str, config_path: &Path) -> Option<(String, String)> {
    let partial: PartialConfig = toml::from_str(config_content).ok()?;

    // Resolve conf_dir with same priority as the controller:
    //   1. EDGION_WORK_DIR env + "/config" (--conf-dir override used by test scripts)
    //   2. conf_center.conf_dir from config file
    let conf_dir = if let Ok(env_wd) = std::env::var("EDGION_WORK_DIR") {
        // Test scripts set EDGION_WORK_DIR and --conf-dir $WORK_DIR/config
        let candidate = PathBuf::from(&env_wd).join("config");
        if candidate.exists() {
            candidate
        } else {
            // Fallback to conf_center.conf_dir
            resolve_conf_dir_from_config(&partial, config_path)?
        }
    } else {
        resolve_conf_dir_from_config(&partial, config_path)?
    };

    // Resolve namespace (same as controller: POD_NAMESPACE or "edgion-system")
    let namespace = std::env::var("POD_NAMESPACE").unwrap_or_else(|_| "edgion-system".to_string());

    // Secret file path: Secret_{namespace}_{name}.yaml
    let secret_file = conf_dir.join(format!(
        "Secret_{}_{}.yaml",
        namespace,
        crate::core::common::local_auth::secret::ADMIN_SECRET_NAME
    ));

    if !secret_file.exists() {
        return None;
    }

    // Parse the Secret YAML to extract username and password
    let content = std::fs::read_to_string(&secret_file).ok()?;
    parse_secret_credentials(&content)
}

/// Extract username and password from a K8s Secret YAML (stringData format).
fn parse_secret_credentials(yaml: &str) -> Option<(String, String)> {
    // Simple line-based parsing for stringData fields
    // Format:
    // stringData:
    //   username: "admin"
    //   password: "randomstring"
    let mut username = None;
    let mut password = None;
    let mut in_string_data = false;

    for line in yaml.lines() {
        let trimmed = line.trim();
        if trimmed == "stringData:" {
            in_string_data = true;
            continue;
        }
        if in_string_data {
            if !trimmed.starts_with("username:")
                && !trimmed.starts_with("password:")
                && !trimmed.starts_with("jwt_secret:")
            {
                if !trimmed.is_empty() && !trimmed.starts_with('#') {
                    // We've left the stringData block
                    break;
                }
                continue;
            }
            if let Some(val) = trimmed.strip_prefix("username:") {
                username = Some(val.trim().trim_matches('"').to_string());
            } else if let Some(val) = trimmed.strip_prefix("password:") {
                password = Some(val.trim().trim_matches('"').to_string());
            }
        }
    }

    match (username, password) {
        (Some(u), Some(p)) if !p.is_empty() => Some((u, p)),
        _ => None,
    }
}

/// Resolve conf_dir from the parsed config file.
fn resolve_conf_dir_from_config(partial: &PartialConfig, config_path: &Path) -> Option<PathBuf> {
    let conf_center = partial.conf_center.as_ref()?;
    let conf_dir_str = conf_center.conf_dir.as_ref()?;
    let conf_dir = PathBuf::from(conf_dir_str);
    if conf_dir.is_absolute() {
        Some(conf_dir)
    } else if conf_dir.exists() {
        Some(conf_dir)
    } else {
        Some(config_path.parent()?.join(conf_dir_str))
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
