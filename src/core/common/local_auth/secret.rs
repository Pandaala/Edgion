//! Admin credentials Secret management.
//!
//! Stores auto-generated admin credentials as a K8s Secret resource,
//! managed via the standard CenterApi (FileSystem or Kubernetes).

use super::config::LocalAuthConfig;
use crate::core::controller::conf_mgr::conf_center::traits::{CenterApi, ConfWriterError};

/// Fixed Secret name for admin credentials.
pub const ADMIN_SECRET_NAME: &str = "edgion-admin-credentials";

/// Default namespace for admin credentials Secret.
pub const ADMIN_SECRET_NAMESPACE: &str = "edgion-system";

/// Ensure admin credentials Secret exists, creating it with random credentials if needed.
///
/// Returns a `LocalAuthConfig` populated from the Secret's data.
pub async fn ensure_admin_credentials<C: CenterApi>(center: &C) -> anyhow::Result<LocalAuthConfig> {
    let ns = resolve_namespace();

    // Try to read existing Secret
    match center.get_one("Secret", Some(&ns), ADMIN_SECRET_NAME).await {
        Ok(yaml) => {
            tracing::info!(
                component = "local_auth",
                secret = ADMIN_SECRET_NAME,
                namespace = %ns,
                "Loaded admin credentials from Secret"
            );
            parse_secret_to_config(&yaml)
        }
        Err(ConfWriterError::NotFound(_)) => {
            // Generate new credentials
            let password = generate_random_string(20);
            let jwt_secret = generate_random_string(32);
            let username = "admin".to_string();

            let yaml = build_secret_yaml(&username, &password, &jwt_secret, &ns);

            center
                .create_one("Secret", Some(&ns), ADMIN_SECRET_NAME, yaml)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to create admin credentials Secret: {}", e))?;

            tracing::info!(
                component = "local_auth",
                secret = ADMIN_SECRET_NAME,
                namespace = %ns,
                "Auto-generated admin credentials Secret created"
            );

            Ok(LocalAuthConfig {
                enabled: true,
                username,
                password,
                jwt_secret,
                jwt_expiry_hours: 24,
                skip_paths: vec!["/health".to_string(), "/ready".to_string()],
            })
        }
        Err(e) => {
            anyhow::bail!("Failed to check admin credentials Secret: {}", e);
        }
    }
}

/// Resolve the namespace for the admin credentials Secret.
/// Uses POD_NAMESPACE env var (K8s), falls back to "edgion-system".
fn resolve_namespace() -> String {
    std::env::var("POD_NAMESPACE").unwrap_or_else(|_| ADMIN_SECRET_NAMESPACE.to_string())
}

/// Parse a K8s Secret YAML into LocalAuthConfig.
fn parse_secret_to_config(yaml: &str) -> anyhow::Result<LocalAuthConfig> {
    let doc: serde_yaml::Value =
        serde_yaml::from_str(yaml).map_err(|e| anyhow::anyhow!("Failed to parse Secret YAML: {}", e))?;

    let string_data = &doc["stringData"];
    // Try stringData first (plaintext), then data (base64)
    let (username, password, jwt_secret) = if string_data.is_mapping() {
        (
            string_data["username"].as_str().unwrap_or("admin").to_string(),
            string_data["password"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Secret missing stringData.password"))?
                .to_string(),
            string_data["jwt_secret"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Secret missing stringData.jwt_secret"))?
                .to_string(),
        )
    } else {
        let data = &doc["data"];
        let decode = |field: &str| -> anyhow::Result<String> {
            let encoded = data[field]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Secret missing data.{}", field))?;
            let bytes =
                base64_decode(encoded).map_err(|e| anyhow::anyhow!("Failed to decode data.{}: {}", field, e))?;
            String::from_utf8(bytes).map_err(|e| anyhow::anyhow!("data.{} is not valid UTF-8: {}", field, e))
        };
        (decode("username")?, decode("password")?, decode("jwt_secret")?)
    };

    Ok(LocalAuthConfig {
        enabled: true,
        username,
        password,
        jwt_secret,
        jwt_expiry_hours: 24,
        skip_paths: vec!["/health".to_string(), "/ready".to_string()],
    })
}

/// Decode a base64-encoded string using the standard alphabet.
fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(input)
        .map_err(|e| e.to_string())
}

/// Build a K8s Secret YAML with stringData (plaintext, K8s base64-encodes on apply).
fn build_secret_yaml(username: &str, password: &str, jwt_secret: &str, namespace: &str) -> String {
    format!(
        r#"apiVersion: v1
kind: Secret
metadata:
  name: {}
  namespace: {}
type: Opaque
stringData:
  username: "{}"
  password: "{}"
  jwt_secret: "{}""#,
        ADMIN_SECRET_NAME, namespace, username, password, jwt_secret
    )
}

/// Generate a random alphanumeric string using /dev/urandom.
pub(crate) fn generate_random_string(len: usize) -> String {
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
    // Fallback using system time
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
