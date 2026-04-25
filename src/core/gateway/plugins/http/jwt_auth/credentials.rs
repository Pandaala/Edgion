//! Credential loading from K8s Secret + DecodingKey construction.

use base64::Engine;
use jsonwebtoken::DecodingKey;
use std::collections::HashMap;

use super::{is_symmetric, JwtAuth, JwtAuthResult};
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::get_secret;

/// Credential loaded from Secret.
#[derive(Debug, Clone)]
pub(super) enum Credential {
    /// Symmetric key (HS256/384/512)
    Symmetric(Vec<u8>),
    /// Asymmetric public key PEM (RS256/384/512, ES256/384/512)
    Asymmetric(String),
}

/// Credentials storage.
#[derive(Debug, Clone)]
pub(super) enum Credentials {
    /// Single credential (from secret_ref)
    Single(Credential),
    /// Multiple credentials keyed by claim value (from secret_refs)
    Multi(HashMap<String, Credential>),
    /// No credentials configured
    None,
}

impl JwtAuth {
    /// Load credentials from K8s Secret (lazily, on first request).
    pub(super) fn load_credentials(&self) -> JwtAuthResult<Credentials> {
        {
            let guard = self.credentials.read();
            if let Some(ref creds) = *guard {
                return Ok(creds.clone());
            }
        }

        let creds = self.load_credentials_from_secret()?;

        {
            let mut guard = self.credentials.write();
            *guard = Some(creds.clone());
        }

        Ok(creds)
    }

    /// Decode secret bytes if base64_secret is enabled.
    fn decode_secret_if_needed(&self, secret_bytes: Vec<u8>) -> JwtAuthResult<Vec<u8>> {
        if self.config.base64_secret {
            let secret_str =
                String::from_utf8(secret_bytes).map_err(|e| format!("Invalid UTF-8 in base64 secret: {}", e))?;
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(secret_str.trim())
                .map_err(|e| format!("Failed to decode base64 secret: {}", e))?;
            Ok(decoded)
        } else {
            Ok(secret_bytes)
        }
    }

    /// Load credentials from pre-resolved data (populated by controller).
    /// Falls back to get_secret for backward compatibility.
    fn load_credentials_from_secret(&self) -> JwtAuthResult<Credentials> {
        let symmetric = is_symmetric(self.config.algorithm);

        // Try pre-resolved single credential first
        if let Some(ref resolved) = self.config.resolved_credential {
            let cred = if symmetric {
                let secret_b64 = resolved.secret.as_ref().ok_or("Resolved credential missing 'secret'")?;
                let secret_bytes = base64::engine::general_purpose::STANDARD
                    .decode(secret_b64)
                    .map_err(|e| format!("Invalid base64 secret: {}", e))?;
                let final_secret = self.decode_secret_if_needed(secret_bytes)?;
                Credential::Symmetric(final_secret)
            } else {
                let pem = resolved
                    .public_key
                    .as_ref()
                    .ok_or("Resolved credential missing 'publicKey'")?;
                Credential::Asymmetric(pem.clone())
            };
            return Ok(Credentials::Single(cred));
        }

        // Try pre-resolved multiple credentials
        if let Some(ref resolved_map) = self.config.resolved_credentials {
            let mut map = HashMap::new();
            for (key, resolved) in resolved_map {
                let cred = if symmetric {
                    let secret_b64 = resolved
                        .secret
                        .as_ref()
                        .ok_or_else(|| format!("Resolved credential '{}' missing 'secret'", key))?;
                    let secret_bytes = base64::engine::general_purpose::STANDARD
                        .decode(secret_b64)
                        .map_err(|e| format!("Invalid base64 secret for '{}': {}", key, e))?;
                    let final_secret = self.decode_secret_if_needed(secret_bytes)?;
                    Credential::Symmetric(final_secret)
                } else {
                    let pem = resolved
                        .public_key
                        .as_ref()
                        .ok_or_else(|| format!("Resolved credential '{}' missing 'publicKey'", key))?;
                    Credential::Asymmetric(pem.clone())
                };
                map.insert(key.clone(), cred);
            }
            if !map.is_empty() {
                return Ok(Credentials::Multi(map));
            }
        }

        // Fallback to get_secret (for controller-side or backward compatibility)
        if let Some(ref secret_ref) = self.config.secret_ref {
            let ns = secret_ref.namespace.as_deref().unwrap_or(&self.plugin_namespace);
            let secret = get_secret(Some(ns), &secret_ref.name)
                .ok_or_else(|| format!("Secret {}/{} not found", ns, secret_ref.name))?;

            let data = secret
                .data
                .as_ref()
                .ok_or_else(|| format!("Secret {}/{} has no data", ns, secret_ref.name))?;

            let cred = if symmetric {
                let secret_bytes = data
                    .get("secret")
                    .ok_or_else(|| format!("Secret {}/{} missing 'secret' key", ns, secret_ref.name))?;
                let final_secret = self.decode_secret_if_needed(secret_bytes.0.clone())?;
                Credential::Symmetric(final_secret)
            } else {
                let public_key_bytes = data
                    .get("publicKey")
                    .ok_or_else(|| format!("Secret {}/{} missing 'publicKey' key", ns, secret_ref.name))?;
                let pem = String::from_utf8(public_key_bytes.0.clone())
                    .map_err(|e| format!("Invalid publicKey encoding: {}", e))?;
                Credential::Asymmetric(pem)
            };

            return Ok(Credentials::Single(cred));
        }

        if let Some(ref secret_refs) = self.config.secret_refs {
            let mut map = HashMap::new();

            for secret_ref in secret_refs {
                let ns = secret_ref.namespace.as_deref().unwrap_or(&self.plugin_namespace);
                let secret = get_secret(Some(ns), &secret_ref.name)
                    .ok_or_else(|| format!("Secret {}/{} not found", ns, secret_ref.name))?;

                let data = secret
                    .data
                    .as_ref()
                    .ok_or_else(|| format!("Secret {}/{} has no data", ns, secret_ref.name))?;

                let key_bytes = data
                    .get("key")
                    .ok_or_else(|| format!("Secret {}/{} missing 'key' field", ns, secret_ref.name))?;
                let key_str =
                    String::from_utf8(key_bytes.0.clone()).map_err(|e| format!("Invalid key encoding: {}", e))?;

                let cred = if symmetric {
                    let secret_bytes = data
                        .get("secret")
                        .ok_or_else(|| format!("Secret {}/{} missing 'secret' key", ns, secret_ref.name))?;
                    let final_secret = self.decode_secret_if_needed(secret_bytes.0.clone())?;
                    Credential::Symmetric(final_secret)
                } else {
                    let public_key_bytes = data
                        .get("publicKey")
                        .ok_or_else(|| format!("Secret {}/{} missing 'publicKey' key", ns, secret_ref.name))?;
                    let pem = String::from_utf8(public_key_bytes.0.clone())
                        .map_err(|e| format!("Invalid publicKey encoding: {}", e))?;
                    Credential::Asymmetric(pem)
                };

                map.insert(key_str, cred);
            }

            return Ok(Credentials::Multi(map));
        }

        Ok(Credentials::None)
    }

    /// Build DecodingKey from credential.
    pub(super) fn build_decoding_key(&self, cred: &Credential) -> JwtAuthResult<DecodingKey> {
        match cred {
            Credential::Symmetric(secret) => Ok(DecodingKey::from_secret(secret)),
            Credential::Asymmetric(pem) => {
                if let Ok(key) = DecodingKey::from_rsa_pem(pem.as_bytes()) {
                    return Ok(key);
                }
                if let Ok(key) = DecodingKey::from_ec_pem(pem.as_bytes()) {
                    return Ok(key);
                }
                Err("Invalid public key PEM (not RSA or EC)".into())
            }
        }
    }
}
