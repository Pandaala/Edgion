//! Strict OTLP/gRPC connection and Secret-reference contract.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, Ipv6Addr};

use k8s_openapi::ByteString;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::credentials::materials::BearerMaterial;
use crate::credentials::resolvers::bearer::BEARER_TOKEN_KEY;
use crate::credentials::resolvers::BearerResolver;
use crate::credentials::{InvalidValueClass, ResolveError, SecretMaterialResolver};
use crate::resources::common::{SecretObjectReference, SecretSlot, TlsConfig};

pub const MAX_OTLP_BEARER_TOKEN_BYTES: usize = 8192;

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OtlpClientConfig {
    pub endpoint: String,
    #[serde(default = "default_timeout_ms")]
    #[schemars(range(min = 1, max = 300000))]
    pub timeout_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<OtlpAuth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<TlsConfig>,
}

crate::redact::impl_redacted_debug!(OtlpClientConfig);

fn default_timeout_ms() -> u64 {
    10_000
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OtlpAuth {
    #[serde(flatten)]
    pub slot: SecretSlot,
}

/// Provider-specific metadata size bound; token syntax remains canonical.
#[derive(Debug, Clone, Copy)]
pub struct OtlpBearerResolver;

impl SecretMaterialResolver for OtlpBearerResolver {
    type Material = BearerMaterial;

    fn required_keys(&self) -> &'static [&'static str] {
        BearerResolver.required_keys()
    }

    fn parse(&self, data: &BTreeMap<String, ByteString>) -> Result<Self::Material, ResolveError> {
        if data
            .get(BEARER_TOKEN_KEY)
            .is_some_and(|token| token.0.len() > MAX_OTLP_BEARER_TOKEN_BYTES)
        {
            return Err(ResolveError::InvalidValue {
                key: BEARER_TOKEN_KEY,
                class: InvalidValueClass::BearerToken,
            });
        }
        BearerResolver.parse(data)
    }
}

/// Validated destination. Never use this transport state as diagnostic identity.
#[derive(Clone)]
pub struct OtlpEndpoint {
    pub uri: http::Uri,
    pub host: String,
    pub port: u16,
    pub tls: bool,
}

crate::redact::impl_redacted_debug!(OtlpEndpoint);

/// Parse an origin without URL normalization or input-bearing errors.
pub fn parse_otlp_endpoint(value: &str) -> Result<OtlpEndpoint, &'static str> {
    const INVALID: &str = "invalid OTLP endpoint";
    if value.len() > 512
        || value.bytes().any(|b| {
            !b.is_ascii()
                || b.is_ascii_control()
                || b.is_ascii_whitespace()
                || matches!(b, b'\\' | b'%' | b'@' | b'?' | b'#')
        })
    {
        return Err(INVALID);
    }
    let (tls, authority) = if let Some(authority) = value.strip_prefix("https://") {
        (true, authority)
    } else if let Some(authority) = value.strip_prefix("http://") {
        (false, authority)
    } else {
        return Err(INVALID);
    };
    let authority = authority.strip_suffix('/').unwrap_or(authority);
    if authority.is_empty() || authority.contains('/') {
        return Err(INVALID);
    }
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, tail) = bracketed.split_once(']').ok_or(INVALID)?;
        host.parse::<Ipv6Addr>().map_err(|_| INVALID)?;
        let port = if tail.is_empty() {
            None
        } else {
            Some(tail.strip_prefix(':').ok_or(INVALID)?)
        };
        (host, port)
    } else {
        let (host, port) = authority
            .split_once(':')
            .map_or((authority, None), |(host, port)| (host, Some(port)));
        if host.parse::<Ipv4Addr>().is_err() {
            let dns = host.strip_suffix('.').unwrap_or(host);
            if dns.is_empty()
                || dns.len() > 253
                || !dns.split('.').all(|label| {
                    !label.is_empty()
                        && label.len() <= 63
                        && label.as_bytes()[0].is_ascii_alphanumeric()
                        && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                        && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                })
                || dns.bytes().all(|b| b.is_ascii_digit() || b == b'.')
            {
                return Err(INVALID);
            }
        }
        (host, port)
    };
    let port = match port {
        Some(port) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => {
            let port = port.parse::<u16>().map_err(|_| INVALID)?;
            if port == 0 {
                return Err(INVALID);
            }
            port
        }
        Some(_) => return Err(INVALID),
        None => {
            if tls {
                443
            } else {
                80
            }
        }
    };
    let uri = value.parse::<http::Uri>().map_err(|_| INVALID)?;
    Ok(OtlpEndpoint {
        uri,
        host: host.to_owned(),
        port,
        tls,
    })
}

/// Shared identity gate for admission and Controller resolution. Invalid targets
/// must never be interpreted as a same-name core Secret by the generic resolver.
pub fn validate_otlp_secret_ref(reference: &SecretObjectReference) -> Result<(), &'static str> {
    use crate::resources::common::kubernetes_name::{is_dns_label, is_dns_subdomain};
    if reference
        .group
        .as_deref()
        .is_some_and(|group| !crate::resources::common::is_core_api_group(group))
        || reference.kind.as_deref().is_some_and(|kind| kind != "Secret")
        || !is_dns_subdomain(&reference.name, 253)
        || reference
            .namespace
            .as_deref()
            .is_some_and(|namespace| !is_dns_label(namespace, 63))
    {
        return Err("invalid OTLP Secret reference");
    }
    Ok(())
}

impl OtlpClientConfig {
    pub fn validate(&self) -> Result<OtlpEndpoint, &'static str> {
        let endpoint = parse_otlp_endpoint(&self.endpoint)?;
        if !(1..=300_000).contains(&self.timeout_ms) {
            return Err("invalid OTLP timeout");
        }
        if let Some(auth) = &self.auth {
            validate_otlp_secret_ref(&auth.slot.secret_ref)?;
        }
        if let Some(tls) = self.tls.as_ref().filter(|tls| tls.enabled) {
            if let Some(reference) = &tls.outbound.client_certificate_ref {
                validate_otlp_secret_ref(reference)?;
            }
            if !endpoint.tls {
                return Err("OTLP HTTP endpoint cannot enable TLS policy");
            }
            if tls.outbound.verify == Some(false) {
                return Err("OTLP TLS verification cannot be disabled");
            }
            if let Some(validation) = &tls.outbound.validation {
                if validation.hostname.is_some()
                    || validation
                        .subject_alt_names
                        .as_ref()
                        .is_some_and(|names| !names.is_empty())
                {
                    return Err("OTLP TLS hostname and subjectAltNames overrides are unsupported");
                }
                if let Some(refs) = &validation.ca_certificate_refs {
                    for reference in refs {
                        validate_otlp_secret_ref(&SecretObjectReference {
                            group: Some(reference.group.clone()),
                            kind: Some(reference.kind.clone()),
                            name: reference.name.clone(),
                            namespace: reference.namespace.clone(),
                        })?;
                    }
                }
            }
        }
        Ok(endpoint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redact::AdminRedactGuard;
    use crate::resources::link_sys::{LinkSys, SystemType};
    use k8s_openapi::api::core::v1::Secret;

    #[test]
    fn endpoint_grammar_and_ports() {
        for (raw, host, port, tls) in [
            ("http://collector", "collector", 80, false),
            ("https://Collector.example./", "Collector.example.", 443, true),
            ("http://192.0.2.1:4317", "192.0.2.1", 4317, false),
            ("https://[2001:db8::1]:4317/", "2001:db8::1", 4317, true),
            ("http://[::ffff:127.0.0.1]", "::ffff:127.0.0.1", 80, false),
        ] {
            let endpoint = parse_otlp_endpoint(raw).unwrap();
            assert_eq!((&*endpoint.host, endpoint.port, endpoint.tls), (host, port, tls));
            assert!(!format!("{endpoint:?}").contains(host));
        }
    }

    #[test]
    fn endpoint_rejects_unsupported_raw_shapes_with_fixed_errors() {
        for raw in [
            "",
            "collector:4317",
            "ftp://collector",
            "http://",
            "http:///",
            "http://collector:",
            "http://collector:0",
            "http://collector:65536",
            "http://collector:+80",
            "http://user:password@collector",
            "http://collector?",
            "http://collector#",
            "http://collector/path/..",
            "http://collector//",
            "http://collector\\path",
            "http://collector/%2e",
            "http://collector\n",
            "http://col lector",
            "http://é.example",
            "http://[fe80::1%eth0]",
            "http://::1",
            "http://[::1]tail",
            "http://[bad]",
            "http://bad..name",
            "http://-bad",
            "http://bad-",
            "http://bad_name",
            "http://127.1",
            "http://2130706433",
            "http://999.0.0.1",
            "http://01.2.3.4",
        ] {
            assert_eq!(
                parse_otlp_endpoint(raw).unwrap_err(),
                "invalid OTLP endpoint",
                "{raw:?}"
            );
        }
    }

    fn config() -> OtlpClientConfig {
        serde_json::from_value(serde_json::json!({"endpoint":"https://collector"})).unwrap()
    }

    #[test]
    fn validates_timeout_and_inert_tls() {
        let mut config = config();
        assert_eq!(config.timeout_ms, 10_000);
        for timeout in [0, 300_001, u64::MAX] {
            config.timeout_ms = timeout;
            assert_eq!(config.validate().unwrap_err(), "invalid OTLP timeout");
        }
        config.timeout_ms = 1;
        config.tls = Some(
            serde_json::from_value(
                serde_json::json!({"enabled":false,"verify":false,"validation":{"hostname":"ignored"}}),
            )
            .unwrap(),
        );
        assert!(config.validate().is_ok());
        config.tls.as_mut().unwrap().enabled = true;
        assert!(config.validate().is_err());
        config.tls.as_mut().unwrap().outbound.verify = None;
        assert!(config.validate().is_err());
        config.tls.as_mut().unwrap().outbound.validation = None;
        assert!(config.validate().is_ok());
        config.endpoint = "http://collector".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn secret_references_are_strict_and_disabled_tls_stays_inert() {
        for reference in [
            serde_json::json!({"name":"auth","kind":"ConfigMap"}),
            serde_json::json!({"name":"auth","group":"external.example"}),
            serde_json::json!({"name":"invalid/name"}),
            serde_json::json!({"name":"auth","namespace":"Invalid"}),
        ] {
            let config: OtlpClientConfig = serde_json::from_value(
                serde_json::json!({"endpoint":"https://collector","auth":{"secretRef":reference}}),
            )
            .unwrap();
            assert_eq!(config.validate().unwrap_err(), "invalid OTLP Secret reference");
        }
        let mut config: OtlpClientConfig = serde_json::from_value(serde_json::json!({"endpoint":"https://collector","tls":{"enabled":true,"validation":{"caCertificateRefs":[{"kind":"ConfigMap","name":"ca"}]}}})).unwrap();
        assert_eq!(config.validate().unwrap_err(), "invalid OTLP Secret reference");
        config.tls.as_mut().unwrap().enabled = false;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn bounded_bearer_adapter_preserves_canonical_validation() {
        for length in [1, MAX_OTLP_BEARER_TOKEN_BYTES] {
            let data = BTreeMap::from([(BEARER_TOKEN_KEY.into(), ByteString(vec![b'a'; length]))]);
            assert_eq!(OtlpBearerResolver.parse(&data).unwrap().token.len(), length);
        }
        for value in [
            vec![b'a'; MAX_OTLP_BEARER_TOKEN_BYTES + 1],
            b"private value\r\n".to_vec(),
            vec![0xff],
        ] {
            let data = BTreeMap::from([(BEARER_TOKEN_KEY.into(), ByteString(value))]);
            let error = OtlpBearerResolver.parse(&data).unwrap_err();
            assert!(!format!("{error:?}").contains("private value"));
        }
        assert!(OtlpBearerResolver.parse(&BTreeMap::new()).is_err());
    }

    #[test]
    fn schema_secret_transport_and_redaction() {
        let mut config = config();
        config.auth = Some(serde_json::from_value(serde_json::json!({"secretRef":{"name":"auth"}})).unwrap());
        let secret = Secret {
            data: Some(BTreeMap::from([(
                "bearerToken".into(),
                ByteString(b"private-token".to_vec()),
            )])),
            ..Default::default()
        };
        config.auth.as_mut().unwrap().slot.secret = Some(secret.clone());
        config.tls = Some(TlsConfig {
            enabled: true,
            resolved_ca_certificates: Some(vec![secret.clone()]),
            resolved_client_certificate: Some(secret),
            ..Default::default()
        });
        let transport = serde_json::to_value(&config).unwrap();
        assert!(transport["auth"]["secret"].is_object());
        let restored: OtlpClientConfig = serde_json::from_value(transport).unwrap();
        assert!(restored.auth.unwrap().slot.secret.is_some());
        let redacted = {
            let _guard = AdminRedactGuard::new();
            serde_json::to_value(&config).unwrap()
        };
        assert_eq!(redacted["auth"]["secret"], "[redacted]");
        assert_eq!(redacted["tls"]["resolvedCaCertificates"], "[redacted]");
        assert_eq!(redacted["tls"]["resolvedClientCertificate"], "[redacted]");
        let schema = serde_json::to_string(&schemars::schema_for!(OtlpClientConfig)).unwrap();
        assert!(!schema.contains("resolvedCaCertificates"));
        assert!(!schema.contains("resolvedClientCertificate"));
        assert!(!schema.contains("\"secret\""));
        assert!(!format!("{config:?}").contains("collector"));
    }

    #[test]
    fn rejects_inline_fields_and_validates_resource_identity() {
        for field in ["protocol", "headers", "password", "token"] {
            let mut value = serde_json::json!({"endpoint":"http://collector"});
            value[field] = serde_json::json!("private");
            assert!(serde_json::from_value::<OtlpClientConfig>(value).is_err());
        }
        assert!(
            serde_json::from_value::<OtlpAuth>(serde_json::json!({"secretRef":{"name":"auth"},"token":"private"}))
                .is_err()
        );
        let mut resource: LinkSys = serde_json::from_value(serde_json::json!({"apiVersion":"edgion.io/v1","kind":"LinkSys","metadata":{"namespace":"default","name":"otlp"},"spec":{"type":"otlp","config":{"endpoint":"http://collector"}}})).unwrap();
        assert_eq!(resource.spec.config.system_type(), SystemType::Otlp);
        assert!(resource.validate_config().is_empty());
        resource.metadata.namespace = Some("private/invalid".into());
        assert_eq!(resource.validate_config(), ["invalid LinkSys reference"]);
        let crd: serde_yaml::Value =
            serde_yaml::from_str(include_str!("../../../../config/crd/edgion-crd/link_sys_crd.yaml")).unwrap();
        assert!(
            crd["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]["spec"]["properties"]["type"]["enum"]
                .as_sequence()
                .unwrap()
                .iter()
                .any(|value| value.as_str() == Some("otlp"))
        );
    }
}
