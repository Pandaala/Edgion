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
                skip_paths: crate::core::common::auth::public_paths::PUBLIC_PROBE_PATHS
                    .iter()
                    .map(|p| (*p).to_string())
                    .collect(),
                cookie_secure: true,
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
        skip_paths: crate::core::common::auth::public_paths::PUBLIC_PROBE_PATHS
            .iter()
            .map(|p| (*p).to_string())
            .collect(),
        cookie_secure: true,
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
    let doc = serde_json::json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": { "name": ADMIN_SECRET_NAME, "namespace": namespace },
        "type": "Opaque",
        "stringData": {
            "username": username,
            "password": password,
            "jwt_secret": jwt_secret,
        },
    });
    serde_yaml::to_string(&doc).expect("admin secret YAML serialize")
}

/// Generate a random alphanumeric string using a CSPRNG.
pub(crate) fn generate_random_string(len: usize) -> String {
    use rand::distr::{Alphanumeric, SampleString};
    Alphanumeric.sample_string(&mut rand::rng(), len)
}
