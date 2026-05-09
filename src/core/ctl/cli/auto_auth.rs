use serde::Deserialize;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

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
    "config/edgion-controller.yaml",                   // dev: relative to CWD
    "/usr/local/edgion/config/edgion-controller.yaml", // pod: standard install path
    "/etc/edgion/edgion-controller.yaml",              // pod: alternative path
];

/// Try to find and read local_auth credentials from controller config.
///
/// Search order:
/// 1. Explicit path from --controller-config flag
/// 2. EDGION_CONTROLLER_CONFIG env var
/// 3. Default paths (CWD/config/, /usr/local/edgion/config/, /etc/edgion/)
///
/// Returns (username, password) if local_auth is configured and enabled. The password
/// is wrapped in `Zeroizing` so its buffer is wiped on drop.
///
/// Mirrors the controller's logic: when neither `[auth]` (OIDC) nor `[local_auth]`
/// is present, local_auth is auto-enabled with credentials auto-generated and
/// persisted to a K8s Secret (`edgion-admin-credentials` in `edgion-system`).
pub fn detect_credentials(explicit_path: Option<&str>) -> Option<(String, Zeroizing<String>)> {
    let config_path = resolve_config_path(explicit_path)?;

    let content = std::fs::read_to_string(&config_path).ok()?;
    let config: ControllerConfigAuth = serde_yaml::from_str(&content).ok()?;

    // If explicit [local_auth] section exists, use it (if it has a real password)
    if let Some(local_auth) = config.local_auth {
        if !local_auth.enabled {
            return None;
        }
        if !local_auth.password.is_empty() {
            return Some((local_auth.username, Zeroizing::new(local_auth.password)));
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

fn read_generated_credentials(config_content: &str, config_path: &Path) -> Option<(String, Zeroizing<String>)> {
    let partial: PartialConfig = serde_yaml::from_str(config_content).ok()?;

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

/// Minimal K8s Secret shape for credential extraction.
///
/// Mirrors `parse_secret_to_config` in `core::common::local_auth::secret` so
/// the CLI and controller agree on how to read the admin Secret.
#[derive(Deserialize, Default)]
#[serde(default)]
struct SecretDoc {
    #[serde(rename = "stringData")]
    string_data: Option<std::collections::BTreeMap<String, String>>,
    data: Option<std::collections::BTreeMap<String, String>>,
}

/// Extract username and password from a K8s Secret YAML.
///
/// Tries `stringData` (plaintext) first, then falls back to `data` (base64).
/// Returns `None` if the YAML is malformed, the relevant fields are missing,
/// or the password is empty.
fn parse_secret_credentials(yaml: &str) -> Option<(String, Zeroizing<String>)> {
    let doc: SecretDoc = serde_yaml::from_str(yaml).ok()?;

    // Mirror `parse_secret_to_config` in `core::common::local_auth::secret`:
    // when `stringData` is present we commit to that branch — we do NOT fall
    // back to `data` if the password is missing/empty inside `stringData`.
    if let Some(string_data) = doc.string_data.as_ref() {
        let username = string_data.get("username")?.clone();
        let password = string_data.get("password")?.clone();
        if password.is_empty() {
            return None;
        }
        return Some((username, Zeroizing::new(password)));
    }

    let data = doc.data.as_ref()?;
    let username = decode_base64_field(data.get("username")?)?;
    let password = decode_base64_field(data.get("password")?)?;
    if password.is_empty() {
        return None;
    }
    Some((username, Zeroizing::new(password)))
}

fn decode_base64_field(encoded: &str) -> Option<String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).ok()?;
    String::from_utf8(bytes).ok()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn yaml_with_string_data(extra: &str) -> String {
        format!(
            r#"apiVersion: v1
kind: Secret
metadata:
  name: edgion-admin-credentials
  namespace: edgion-system
type: Opaque
stringData:
  username: "admin"
  password: "p@ssw0rd"
  jwt_secret: "jwt"{}"#,
            extra
        )
    }

    #[test]
    fn parses_normal_secret() {
        let (u, p) = parse_secret_credentials(&yaml_with_string_data("")).unwrap();
        assert_eq!(u, "admin");
        assert_eq!(p.as_str(), "p@ssw0rd");
    }

    #[test]
    fn tolerates_comment_inside_string_data() {
        let yaml = yaml_with_string_data("\n  # operator-added comment");
        let (u, p) = parse_secret_credentials(&yaml).unwrap();
        assert_eq!(u, "admin");
        assert_eq!(p.as_str(), "p@ssw0rd");
    }

    #[test]
    fn tolerates_extra_field_inside_string_data() {
        // The old line scanner would `break` on any unknown non-comment line in
        // stringData and silently drop credentials that came after.
        let yaml = format!(
            r#"apiVersion: v1
kind: Secret
metadata:
  name: edgion-admin-credentials
  namespace: edgion-system
type: Opaque
stringData:
  extra_key: "ignored"
  username: "admin"
  password: "p@ssw0rd"
  jwt_secret: "jwt"
"#
        );
        let (u, p) = parse_secret_credentials(&yaml).unwrap();
        assert_eq!(u, "admin");
        assert_eq!(p.as_str(), "p@ssw0rd");
    }

    #[test]
    fn preserves_password_starting_with_quote() {
        // The old parser called `trim_matches('"')` which would strip a real
        // leading/trailing quote character from the password.
        let yaml = r#"apiVersion: v1
kind: Secret
type: Opaque
stringData:
  username: "admin"
  password: '"quoted"value'
  jwt_secret: "jwt"
"#;
        let (_, p) = parse_secret_credentials(yaml).unwrap();
        assert_eq!(p.as_str(), "\"quoted\"value");
    }

    #[test]
    fn preserves_password_with_hash() {
        let yaml = r#"apiVersion: v1
kind: Secret
type: Opaque
stringData:
  username: "admin"
  password: "abc#def"
  jwt_secret: "jwt"
"#;
        let (_, p) = parse_secret_credentials(yaml).unwrap();
        assert_eq!(p.as_str(), "abc#def");
    }

    #[test]
    fn falls_back_to_base64_data() {
        // base64("admin") = YWRtaW4=, base64("s3cret") = czNjcmV0
        let yaml = r#"apiVersion: v1
kind: Secret
type: Opaque
data:
  username: YWRtaW4=
  password: czNjcmV0
  jwt_secret: and0
"#;
        let (u, p) = parse_secret_credentials(yaml).unwrap();
        assert_eq!(u, "admin");
        assert_eq!(p.as_str(), "s3cret");
    }

    #[test]
    fn returns_none_when_password_missing() {
        let yaml = r#"apiVersion: v1
kind: Secret
type: Opaque
stringData:
  username: "admin"
  jwt_secret: "jwt"
"#;
        assert!(parse_secret_credentials(yaml).is_none());
    }

    #[test]
    fn returns_none_when_password_empty() {
        let yaml = r#"apiVersion: v1
kind: Secret
type: Opaque
stringData:
  username: "admin"
  password: ""
  jwt_secret: "jwt"
"#;
        assert!(parse_secret_credentials(yaml).is_none());
    }

    #[test]
    fn returns_none_on_malformed_yaml() {
        assert!(parse_secret_credentials("::: not yaml :::").is_none());
    }

    #[test]
    fn does_not_fall_back_when_string_data_present_without_password() {
        // Locks in parity with `secret.rs::parse_secret_to_config`: once
        // `stringData` is present we commit to that branch and never fall back
        // to `data`, even if the password lives there.
        let yaml = r#"apiVersion: v1
kind: Secret
type: Opaque
stringData:
  username: "admin"
data:
  password: czNjcmV0
"#;
        assert!(parse_secret_credentials(yaml).is_none());
    }

    #[test]
    fn returns_none_on_malformed_base64_in_data() {
        let yaml = r#"apiVersion: v1
kind: Secret
type: Opaque
data:
  username: YWRtaW4=
  password: "!!!not-base64!!!"
"#;
        assert!(parse_secret_credentials(yaml).is_none());
    }
}
