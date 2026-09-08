//! Material types produced by each [`super::resolver::SecretMaterialResolver`].
//!
//! Design principles:
//! - Only materials that carry a **raw private key** use `Zeroizing`:
//!   - [`AcmeAccountMaterial::credentials`] — the ACME account PEM JSON.
//!   - [`TlsMaterial::client_key_pem`] — the TLS client private key PEM.
//! - Application-layer credentials remain owned strings. Their `Debug`
//!   implementations redact material independently of any enclosing state.
//!   Debug redaction does not erase the underlying allocation.
//!
//! # Zeroize scope — read before trusting the guarantee
//!
//! The `Zeroizing` wrappers in this module only wipe the **final in-memory
//! copy** held by the Material. Several plaintext copies exist outside of
//! that protection and are NOT zero-wiped:
//!
//! - The controller-side `SecretStore` owns raw `ByteString` plaintext until
//!   replacement, removal, or store destruction; fetched snapshots are independent.
//! - The resolved `Secret` object is embedded on `*Auth.secret` /
//!   `TlsConfig.resolved_ca_certificates`/`resolved_client_certificate` and serialized via `serde_json` / gRPC through
//!   conf_sync to the Gateway; none of those buffers are wiped.
//! - Gateway stores release their references on replacement/removal; outstanding
//!   requests and runtime generations may keep material alive independently.
//! - `serde_json::from_str::<Value>` and other intermediate allocations
//!   (e.g. `String::from_utf8`, `json.to_string()`) produce short-lived
//!   heap copies that are dropped normally, without zeroization.
//!
//! Treat `Zeroizing` here as **defense-in-depth, not a strict end-to-end
//! guarantee**.

use std::fmt;

use zeroize::Zeroizing;

/// Shared by Redis / Etcd / ES basic / Webhook basic auth.
#[derive(Clone)]
pub struct BasicAuthMaterial {
    pub username: Option<String>,
    pub password: String,
}

/// Elasticsearch api_key auth.
#[derive(Clone)]
pub struct EsApiKeyMaterial {
    pub api_key_id: String,
    pub api_key_secret: String,
}

/// Elasticsearch / Webhook bearer auth.
#[derive(Clone)]
pub struct BearerMaterial {
    pub token: String,
}

impl fmt::Debug for BearerMaterial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BearerMaterial")
            .field("token", &"[redacted]")
            .finish()
    }
}

/// Webhook HMAC auth.
#[derive(Clone)]
pub struct HmacMaterial {
    pub key: String,
}

/// Webhook API Key auth.
#[derive(Clone)]
pub struct WebhookApiKeyMaterial {
    pub api_key: String,
}

/// API key used to authenticate an AI provider request.
#[derive(Clone)]
pub struct AiApiKeyMaterial {
    pub api_key: String,
}

/// ACME account credentials: a JSON-serialized `instant_acme::AccountCredentials`
/// that holds the account's private key PEM and therefore must be zeroized.
///
/// **Do not derive `Clone`.** `Zeroizing<String>` itself implements `Clone`, but
/// cloning would produce an additional live copy of the private key whose drop
/// ordering is not guaranteed; this silently weakens the zeroize contract.
/// Share via `&AcmeAccountMaterial` or `Arc` instead.
///
/// # Zeroize scope
///
/// Only the `credentials` `String` held by this struct is zero-wiped on drop.
/// The following plaintext copies exist during resolution / transport and are
/// **NOT** zero-wiped:
///
/// - `SecretStore` retains the raw `ByteString` (plaintext `Vec<u8>`); its
///   lifetime is independent of this struct.
/// - JSON parsing may allocate dependency-owned scratch buffers; syntax checking
///   does not establish erasure of those buffers.
/// - Transient `String` allocations produced during decoding (e.g.
///   `json.to_string()`) live until the allocator reclaims the memory.
/// - The embedded `Secret` object on `*Auth.secret` / `TlsConfig.resolved_ca_certificates`
///   / `TlsConfig.resolved_client_certificate` is serialized via `serde_json` and
///   transported over gRPC to the Gateway — none of those buffers are zero-wiped.
///
/// In other words: `Zeroize` here reduces but does NOT eliminate the window
/// during which the private key is resident in memory. Treat this as
/// defense-in-depth, not a strict guarantee.
pub struct AcmeAccountMaterial {
    pub credentials: Zeroizing<String>,
}

/// TLS certificate material parsed from a `kubernetes.io/tls` or `Opaque` Secret.
///
/// - `ca_pem` — CA certificate used to verify the backend.
/// - `client_cert_pem` — client certificate for mTLS (public).
/// - `client_key_pem` — **client private key**, always wrapped in `Zeroizing`.
///
/// All three are optional; the resolver rejects the secret only if **none** of
/// them is present. Inline `ca_cert` / `client_cert` from the CRD take
/// precedence at the Gateway, then fall back to these fields.
///
/// **Do not derive `Clone`** for the same reason as `AcmeAccountMaterial`.
///
/// # Zeroize scope
///
/// `client_key_pem` (the private key) is held in `Zeroizing<Vec<u8>>` and
/// zero-wiped on drop. `ca_pem` and `client_cert_pem` are plaintext public
/// material and deliberately not wrapped. The same caveats about the
/// `SecretStore` cache, the embedded `Secret` on `TlsConfig.resolved_ca_certificates`
/// / `resolved_client_certificate`, and the
/// conf_sync transport buffers apply here — see [`AcmeAccountMaterial`] for
/// the full list. `Zeroize` is defense-in-depth, not an end-to-end guarantee.
pub struct TlsMaterial {
    pub ca_pem: Option<Vec<u8>>,
    pub client_cert_pem: Option<Vec<u8>>,
    pub client_key_pem: Option<Zeroizing<Vec<u8>>>,
}

// Do not delegate to field Debug implementations: Zeroizing wipes on drop,
// but does not redact Debug, and public certificate fields may contain PII.
crate::redact::impl_redacted_debug!(
    BasicAuthMaterial,
    EsApiKeyMaterial,
    HmacMaterial,
    WebhookApiKeyMaterial,
    AiApiKeyMaterial,
    AcmeAccountMaterial,
    TlsMaterial,
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_material_debug_never_formats_fields() {
        let materials: Vec<Box<dyn fmt::Debug>> = vec![
            Box::new(BasicAuthMaterial {
                username: Some("private-user".into()),
                password: "private-password".into(),
            }),
            Box::new(EsApiKeyMaterial {
                api_key_id: "private-id".into(),
                api_key_secret: "private-key".into(),
            }),
            Box::new(HmacMaterial {
                key: "private-key".into(),
            }),
            Box::new(WebhookApiKeyMaterial {
                api_key: "private-key".into(),
            }),
            Box::new(AiApiKeyMaterial {
                api_key: "private-key".into(),
            }),
            Box::new(AcmeAccountMaterial {
                credentials: Zeroizing::new("private-account".into()),
            }),
            Box::new(TlsMaterial {
                ca_pem: Some(b"private-ca".to_vec()),
                client_cert_pem: Some(b"private-cert".to_vec()),
                client_key_pem: Some(Zeroizing::new(b"private-key".to_vec())),
            }),
        ];
        for material in materials {
            for debug in [format!("{material:?}"), format!("{material:#?}")] {
                let (_, fields) = debug.split_once('(').expect("material type prefix");
                assert_eq!(fields, "[redacted])");
            }
        }
    }

    #[test]
    fn basic_auth_without_username() {
        let m = BasicAuthMaterial {
            username: None,
            password: "p".into(),
        };
        assert!(m.username.is_none());
    }

    #[test]
    fn acme_material_zeroizes_on_drop() {
        let m = AcmeAccountMaterial {
            credentials: zeroize::Zeroizing::new("{\"x\":1}".into()),
        };
        assert!(m.credentials.starts_with('{'));
        drop(m);
    }

    #[test]
    fn types_are_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<BasicAuthMaterial>();
        assert_send_sync::<EsApiKeyMaterial>();
        assert_send_sync::<BearerMaterial>();
        assert_send_sync::<HmacMaterial>();
        assert_send_sync::<WebhookApiKeyMaterial>();
        assert_send_sync::<AiApiKeyMaterial>();
        assert_send_sync::<AcmeAccountMaterial>();
        assert_send_sync::<TlsMaterial>();
    }

    #[test]
    fn bearer_debug_is_redacted() {
        let material = BearerMaterial {
            token: "private-token".to_string(),
        };
        let debug = format!("{material:?}");
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains("private-token"));
    }

    #[test]
    fn tls_material_private_key_zeroizes_on_drop() {
        let m = TlsMaterial {
            ca_pem: Some(b"ca".to_vec()),
            client_cert_pem: None,
            client_key_pem: Some(zeroize::Zeroizing::new(b"KEY".to_vec())),
        };
        assert_eq!(m.client_key_pem.as_ref().unwrap().as_slice(), b"KEY");
        drop(m);
    }
}
