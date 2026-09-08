//! Mandatory secret redaction for the gateway admin `/configclient` output.
//!
//! Unlike `crate::sensitive` (operator-configurable access-log masking),
//! this is an always-on security invariant with no config knob. A gateway admin
//! handler constructs an [`AdminRedactGuard`] around its synchronous serialization;
//! while the guard is alive on the current thread, the `redact_opt` / `redact_required`
//! serde adapters emit `"[redacted]"` instead of the real value.
//!
//! Default (no guard) emits the real value, so the controller's conf_sync transport
//! serialization — which runs in a different process and never constructs the guard —
//! is unaffected. See docs/superpowers/specs/2026-06-07-gateway-admin-api-harden-design.md.
//!
//! INVARIANT: the guard is a thread-local, so it only applies on the thread that runs
//! `serde_json::to_value`. Never move the admin serialization off that thread
//! (no rayon/`par_iter`/`spawn_blocking` in the `/configclient` serialize path), and
//! never add an overriding `impl Serialize` / `#[serde(into)]` on a type ABOVE a
//! secret field — either would silently bypass the field adapters.

use std::cell::Cell;

use serde::{Serialize, Serializer};

// Fixed Debug output must not delegate to credential fields or map keys.
// Independent from admin serialization: conf_sync still carries real values.
macro_rules! impl_redacted_debug {
    ($($material:ty),+ $(,)?) => {
        $(impl std::fmt::Debug for $material {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(concat!(stringify!($material), "([redacted])"))
            }
        })+
    };
}

pub(crate) use impl_redacted_debug;

thread_local! {
    static REDACT_ACTIVE: Cell<bool> = const { Cell::new(false) };
}

/// True while an [`AdminRedactGuard`] is alive on the current thread.
#[inline]
pub fn redact_active() -> bool {
    REDACT_ACTIVE.with(|c| c.get())
}

/// RAII guard that turns on mandatory secret redaction for the current thread.
///
/// Construct it immediately before the synchronous serialization of admin output
/// (no `.await` may occur while it is held). Re-entrancy safe: it saves and restores
/// the previous flag value, and `Drop` runs on panic unwinding, so a panicking
/// handler cannot leave redaction stuck on for the next request on the same thread.
pub struct AdminRedactGuard {
    prev: bool,
}

impl AdminRedactGuard {
    // `new()` has the side effect of activating the redact scope, so a `Default`
    // impl would be misleading; suppress the lint here on the impl block.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let prev = REDACT_ACTIVE.with(|c| c.replace(true));
        Self { prev }
    }
}

impl Drop for AdminRedactGuard {
    fn drop(&mut self) {
        REDACT_ACTIVE.with(|c| c.set(self.prev));
    }
}

/// serde adapter for `Option<T>` secret fields.
/// `#[serde(serialize_with = "crate::redact::redact_opt::serialize")]`
///
/// In an admin-redact scope, emits the scalar `"[redacted]"` (never recursing into
/// `T`, so the whole subtree collapses). Otherwise serializes the real value.
pub mod redact_opt {
    use super::{redact_active, Serialize, Serializer};

    pub fn serialize<T, S>(v: &Option<T>, s: S) -> Result<S::Ok, S::Error>
    where
        T: Serialize,
        S: Serializer,
    {
        if redact_active() {
            s.serialize_str("[redacted]")
        } else {
            v.serialize(s)
        }
    }
}

/// serde adapter for bare (non-`Option`) secret fields, e.g. `ActiveHttpChallenge.key_authorization`.
/// `#[serde(serialize_with = "crate::redact::redact_required::serialize")]`
pub mod redact_required {
    use super::{redact_active, Serialize, Serializer};

    pub fn serialize<T, S>(v: &T, s: S) -> Result<S::Ok, S::Error>
    where
        T: Serialize,
        S: Serializer,
    {
        if redact_active() {
            s.serialize_str("[redacted]")
        } else {
            v.serialize(s)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_credential_debug<T>(value: T)
    where
        T: Serialize + serde::de::DeserializeOwned + std::fmt::Debug,
    {
        let wire = serde_json::to_value(&value).unwrap();
        let decoded: T = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(&decoded).unwrap(), wire);
        for debug in [
            format!("{value:?}"),
            format!("{value:#?}"),
            format!("{:?}", Some(&decoded)),
        ] {
            assert!(debug.contains("[redacted]"));
            assert!(!debug.contains("private-"));
            assert!(!debug.contains("[112, 114, 105"));
        }
    }

    #[test]
    fn credential_containers_hide_debug_but_preserve_transport() {
        use crate::resources::common::{OutboundTlsConfig, SecretSlot, TlsConfig};
        use crate::resources::edgion_plugins::plugin_configs::*;
        use serde_json::json;

        let secret = json!({"stringData": {"private-key": "private-material"}});
        macro_rules! check_config {
            ($ty:ty, $extra:expr) => {{
                let mut wire = serde_json::to_value(<$ty>::default()).unwrap();
                wire.as_object_mut()
                    .unwrap()
                    .extend($extra.as_object().unwrap().clone());
                let value: $ty = serde_json::from_value(wire).unwrap();
                check_credential_debug(value);
            }};
        }
        let hmac = json!({"secret": b"private-material", "headers": {"private-header": "private-identity"}, "group": "private-group"});
        check_credential_debug(serde_json::from_value::<HmacCredential>(hmac.clone()).unwrap());
        check_config!(HmacMatchConfig, json!({"resolvedCredentials": {"private-user": hmac}}));
        check_config!(
            BasicAuthConfig,
            json!({"resolvedUsers": {"private-user": {"password": "private-password", "group": "private-group"}}})
        );
        let metadata = json!({"headers": {"private-header": "private-identity"}, "group": "private-group"});
        check_credential_debug(serde_json::from_value::<KeyMetadata>(metadata.clone()).unwrap());
        check_config!(KeyAuthConfig, json!({"resolvedKeys": {"private-api-key": metadata}}));
        let jwt = json!({"key": "private-kid", "secret": "private-key", "publicKey": "private-cert"});
        check_credential_debug(serde_json::from_value::<ResolvedJwtCredential>(jwt.clone()).unwrap());
        let issuer = json!({"group": "private-group", "algorithm": "HS256", "credentials": {"private-kid": jwt}});
        check_credential_debug(serde_json::from_value::<ResolvedJwtIssuerGroup>(issuer.clone()).unwrap());
        check_config!(
            JwtAuthConfig,
            json!({"resolvedGroupsByIss": {"private-issuer": issuer}})
        );
        check_config!(
            OpenidConnectConfig,
            json!({"resolvedOidcClientSecret": "private-client", "resolvedSessionSecret": "private-session", "resolvedClientCertificate": secret})
        );
        check_config!(
            LdapAuthConfig,
            json!({"resolvedCaCertificates": [secret], "resolvedClientCertificate": secret})
        );
        check_config!(HeaderCertAuthConfig, json!({"resolvedCaSecrets": [secret]}));
        check_config!(CsrfConfig, json!({"resolvedKey": "private-key"}));
        check_credential_debug(ResolvedJweCredential {
            secret: Some("private-key".into()),
        });
        check_config!(
            JweDecryptConfig,
            json!({"resolvedCredential": {"secret": "private-key"}})
        );
        check_config!(
            TlsConfig,
            json!({"resolvedCaCertificates": [secret], "resolvedClientCertificate": secret})
        );
        check_config!(
            OutboundTlsConfig,
            json!({"resolvedCaCertificates": [secret], "resolvedClientCertificate": secret})
        );
        check_config!(WasmFetchSpec, json!({"resolvedAuthHeader": "private-token"}));
        check_config!(WasmOciSpec, json!({"resolvedPullSecret": "private-registry-token"}));
        check_credential_debug(
            serde_json::from_value::<SecretSlot>(json!({"secretRef": {"name": "source"}, "secret": secret})).unwrap(),
        );

        macro_rules! check_wire {
            ($ty:ty, $wire:expr) => {
                check_credential_debug(serde_json::from_value::<$ty>($wire).unwrap());
            };
        }
        check_wire!(
            crate::resources::edgion_plugins::conditions::SecretMatchCondition,
            json!({"key": {"type": "header", "name": "x-token"}, "secretKey": "token", "secretRef": {"name": "source"}, "resolvedValues": ["private-token"]})
        );
        check_wire!(
            crate::resources::edgion_tls::ClientAuthConfig,
            json!({"caSecret": secret, "allowedCns": ["private-identity"]})
        );
        check_wire!(
            crate::resources::edgion_tls::EdgionTlsSpec,
            json!({"hosts": ["example.com"], "secretRef": {"name": "source"}, "secret": secret})
        );
        check_wire!(
            crate::resources::backend_tls_policy::BackendTLSPolicySpec,
            json!({"targetRefs": [], "validation": {"hostname": "example.com"}, "resolvedClientCertificate": secret})
        );
        check_wire!(
            crate::resources::gateway::GatewayBackendTLS,
            json!({"resolvedClientCertificate": secret})
        );
        check_wire!(
            crate::resources::gateway::ResolvedServingCertificateReference,
            json!({"index": 0, "source": {"group": "", "kind": "Secret", "namespace": "default", "name": "source"}, "secret": secret})
        );
        check_wire!(
            crate::resources::gateway::ResolvedFrontendCAReference,
            json!({"index": 0, "source": {"kind": "Secret", "namespace": "default", "name": "source"}, "caCertificate": "cHJpdmF0ZS1jZXJ0"})
        );
        check_wire!(
            crate::resources::edgion_acme::ActiveHttpChallenge,
            json!({"domain": "example.com", "token": "private-token", "keyAuthorization": "private-key", "expireAt": 1})
        );

        // Debug must not depend on the thread-local admin guard, and the guard
        // must still redact the real transport fields independently.
        let cfg = HmacMatchConfig {
            resolved_credentials: Some(std::collections::HashMap::from([(
                "private-user".to_string(),
                HmacCredential {
                    secret: b"private-material".to_vec(),
                    headers: Default::default(),
                    group: "private-group".into(),
                },
            )])),
            ..Default::default()
        };
        assert!(serde_json::to_string(&cfg).unwrap().contains("private-user"));
        let _guard = AdminRedactGuard::new();
        assert_eq!(serde_json::to_value(&cfg).unwrap()["resolvedCredentials"], "[redacted]");
    }

    #[derive(Serialize)]
    struct Demo {
        #[serde(skip_serializing_if = "Option::is_none", serialize_with = "redact_opt::serialize")]
        opt_secret: Option<String>,
        #[serde(serialize_with = "redact_required::serialize")]
        bare_secret: String,
        public_field: String,
    }

    fn demo() -> Demo {
        Demo {
            opt_secret: Some("OPT-SECRET".to_string()),
            bare_secret: "BARE-SECRET".to_string(),
            public_field: "visible".to_string(),
        }
    }

    #[test]
    fn redacts_when_guard_active() {
        let json = {
            let _g = AdminRedactGuard::new();
            serde_json::to_value(demo()).unwrap()
        };
        assert_eq!(json["opt_secret"], "[redacted]");
        assert_eq!(json["bare_secret"], "[redacted]");
        assert_eq!(json["public_field"], "visible");
        let s = json.to_string();
        assert!(!s.contains("OPT-SECRET"), "opt secret leaked: {s}");
        assert!(!s.contains("BARE-SECRET"), "bare secret leaked: {s}");
    }

    #[test]
    fn emits_real_value_without_guard() {
        let json = serde_json::to_value(demo()).unwrap();
        assert_eq!(json["opt_secret"], "OPT-SECRET");
        assert_eq!(json["bare_secret"], "BARE-SECRET");
    }

    #[test]
    fn none_option_is_omitted_when_active() {
        let _g = AdminRedactGuard::new();
        let json = serde_json::to_value(Demo {
            opt_secret: None,
            bare_secret: "x".to_string(),
            public_field: "p".to_string(),
        })
        .unwrap();
        assert!(json.get("opt_secret").is_none(), "None field should be omitted");
    }

    #[test]
    fn guard_clears_on_drop() {
        assert!(!redact_active());
        {
            let _g = AdminRedactGuard::new();
            assert!(redact_active());
        }
        assert!(!redact_active());
    }
}
