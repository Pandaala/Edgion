use serde::Deserialize;
use std::path::PathBuf;
use zeroize::Zeroizing;

/// Minimal struct to deserialize auth-related sections from controller config.
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ControllerConfigAuth {
    auth: Option<OidcAuthSection>,
    local_auth: Option<LocalAuthSection>,
}

/// Minimal OIDC auth section. Retained so `[auth]` still parses cleanly even
/// though credential detection no longer branches on it (env / `[local_auth]`
/// are the only credential sources for the CLI now).
#[derive(Debug, Deserialize)]
struct OidcAuthSection {
    // Parsed for completeness; not currently read by detect_credentials.
    #[allow(dead_code)]
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
    "config/edgion-controller.yaml",                   // dev: relative to CWD
    "/usr/local/edgion/config/edgion-controller.yaml", // pod: standard install path
    "/etc/edgion/edgion-controller.yaml",              // pod: alternative path
];

/// Try to find admin credentials for CLI auto-login.
///
/// Search order:
/// 1. `EDGION_ADMIN_*` environment variables (matches the controller; lets the CLI
///    auto-login from the same Secret-injected env).
/// 2. The `[local_auth]` section in the controller config file (real password only).
///
/// Returns (username, password) if credentials are found. The password is wrapped
/// in `Zeroizing` so its buffer is wiped on drop. Credentials are never
/// auto-generated; when nothing is found we return `None` and the caller falls
/// back to `--token` / interactive login.
pub fn detect_credentials(explicit_path: Option<&str>) -> Option<(String, Zeroizing<String>)> {
    // 1. Env-var precedence: matches the controller's EDGION_ADMIN_* path so the
    //    CLI can auto-login from the same Secret-injected environment.
    // Intentional divergence from the controller: this client-side auto-login is a
    // convenience and does not consult the config-file `local_auth.enabled` flag,
    // so env creds win even when the file sets `enabled = false`. This is fail-safe
    // because the server is the source of truth — it rejects on its own policy and
    // the CLI falls back to `--token`/interactive login.
    if let Ok(pw) = std::env::var("EDGION_ADMIN_PASSWORD") {
        if !pw.is_empty() {
            let user = std::env::var("EDGION_ADMIN_USERNAME")
                .ok()
                .filter(|u| !u.is_empty())
                .unwrap_or_else(|| "admin".to_string());
            return Some((user, Zeroizing::new(pw)));
        }
    }

    // 2. Config-file [local_auth] with a real password.
    let config_path = resolve_config_path(explicit_path)?;

    let content = std::fs::read_to_string(&config_path).ok()?;
    let config: ControllerConfigAuth = serde_yaml::from_str(&content).ok()?;

    if let Some(local_auth) = config.local_auth {
        if !local_auth.enabled {
            return None;
        }
        if !local_auth.password.is_empty() {
            return Some((local_auth.username, Zeroizing::new(local_auth.password)));
        }
        // [local_auth] exists but password is empty — credentials are no longer
        // auto-generated, so there is nothing to read; fall back to manual login.
        return None;
    }

    // No [local_auth] section — OIDC or nothing; no local credentials to use.
    None
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

#[cfg(test)]
mod tests {
    use super::*;

    const ADMIN_ENV_VARS: [&str; 3] = [
        "EDGION_ADMIN_USERNAME",
        "EDGION_ADMIN_PASSWORD",
        "EDGION_ADMIN_JWT_SECRET",
    ];

    // Process-global lock serializing `EDGION_ADMIN_*` env manipulation across
    // these tests. CLI tests run in their own test binary (separate process), so
    // a per-crate lock is equivalent to the controller's — no need to link the
    // server-side auth module just for a mutex.
    //
    // `std::sync::Mutex` (not `parking_lot`) is deliberate: this is a test-only
    // `Mutex<()>` whose guard is recovered on poison via `into_inner()` at the
    // call sites, so poisoning is a non-issue and pulling in `parking_lot` just
    // for it is not worth a new dependency.
    #[allow(clippy::disallowed_types)]
    static ADMIN_ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn clear_admin_env() {
        for k in ADMIN_ENV_VARS {
            std::env::remove_var(k);
        }
        // Ensure no config file is picked up via env during these tests.
        std::env::remove_var("EDGION_CONTROLLER_CONFIG");
    }

    #[test]
    fn detect_credentials_returns_env_creds_when_password_set() {
        let _guard = ADMIN_ENV_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_admin_env();
        std::env::set_var("EDGION_ADMIN_USERNAME", "envuser");
        std::env::set_var("EDGION_ADMIN_PASSWORD", "envpass");

        // Pass a non-existent explicit path: env must take precedence and short-circuit
        // before any config-file lookup.
        let creds = detect_credentials(Some("/nonexistent/controller.yaml"));
        let (u, p) = creds.expect("env credentials should be detected");
        assert_eq!(u, "envuser");
        assert_eq!(p.as_str(), "envpass");

        clear_admin_env();
    }

    #[test]
    fn detect_credentials_env_wins_over_readable_config_file() {
        use tempfile::TempDir;
        let _guard = ADMIN_ENV_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_admin_env();

        // A real, readable config file with valid file-based credentials.
        let tmp = TempDir::new().unwrap();
        let config_path = tmp.path().join("controller.yaml");
        std::fs::write(
            &config_path,
            "local_auth:\n  enabled: true\n  username: fileuser\n  password: filepass\n",
        )
        .unwrap();

        // Env vars are set: they must take precedence over the readable file.
        std::env::set_var("EDGION_ADMIN_USERNAME", "envuser");
        std::env::set_var("EDGION_ADMIN_PASSWORD", "envpass");

        let creds = detect_credentials(Some(config_path.to_str().unwrap()));
        let (u, p) = creds.expect("env credentials should win over the config file");
        // Env wins, NOT the file values.
        assert_eq!(u, "envuser");
        assert_eq!(p.as_str(), "envpass");

        clear_admin_env();
    }

    #[test]
    fn detect_credentials_env_defaults_username_to_admin() {
        let _guard = ADMIN_ENV_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_admin_env();
        // Only the password is set; username defaults to "admin".
        std::env::set_var("EDGION_ADMIN_PASSWORD", "envpass");

        let creds = detect_credentials(Some("/nonexistent/controller.yaml"));
        let (u, p) = creds.expect("env credentials should be detected");
        assert_eq!(u, "admin");
        assert_eq!(p.as_str(), "envpass");

        clear_admin_env();
    }

    #[test]
    fn detect_credentials_reads_local_auth_from_config_file() {
        use tempfile::TempDir;
        let _guard = ADMIN_ENV_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_admin_env();

        let tmp = TempDir::new().unwrap();
        let config_path = tmp.path().join("controller.yaml");
        std::fs::write(
            &config_path,
            "local_auth:\n  enabled: true\n  username: fileuser\n  password: filepass\n",
        )
        .unwrap();

        let creds = detect_credentials(Some(config_path.to_str().unwrap()));
        let (u, p) = creds.expect("config-file local_auth should be detected");
        assert_eq!(u, "fileuser");
        assert_eq!(p.as_str(), "filepass");

        clear_admin_env();
    }

    #[test]
    fn detect_credentials_empty_local_auth_password_returns_none() {
        use tempfile::TempDir;
        let _guard = ADMIN_ENV_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_admin_env();

        let tmp = TempDir::new().unwrap();
        let config_path = tmp.path().join("controller.yaml");
        // Enabled but empty password — credentials are no longer auto-generated.
        std::fs::write(
            &config_path,
            "local_auth:\n  enabled: true\n  username: admin\n  password: \"\"\n",
        )
        .unwrap();

        assert!(detect_credentials(Some(config_path.to_str().unwrap())).is_none());

        clear_admin_env();
    }

    #[test]
    fn detect_credentials_disabled_local_auth_returns_none() {
        use tempfile::TempDir;
        let _guard = ADMIN_ENV_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_admin_env();

        let tmp = TempDir::new().unwrap();
        let config_path = tmp.path().join("controller.yaml");
        std::fs::write(
            &config_path,
            "local_auth:\n  enabled: false\n  username: admin\n  password: secret\n",
        )
        .unwrap();

        assert!(detect_credentials(Some(config_path.to_str().unwrap())).is_none());

        clear_admin_env();
    }
}
