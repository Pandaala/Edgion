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
                            group: reference.group.clone(),
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
#[path = "../../../tests/unit/resources/link_sys/otlp_test.rs"]
mod tests;
