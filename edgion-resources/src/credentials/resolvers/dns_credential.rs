//! DNS-01 challenge credential resolver (M8).
//!
//! Validates the Secret referenced by `EdgionAcme.spec.challenge.dns01.credentialRef`
//! so that key-presence / decode failures surface as `ResolvedRefs=False` on
//! the CRD status — rather than slipping through to `AcmeService::issue()`
//! and only blowing up during challenge execution.
//!
//! Per-provider required keys (mirrors `dns_provider::create_dns_provider`):
//! - `cloudflare` → `api-token`
//! - `alidns` → `access-key-id`, `access-key-secret`
//! - `pebble` → *(none — `api-url` is optional with a default)*
//!
//! This resolver uses **enum dispatch** (`DnsCredentialResolver::Cloudflare`,
//! `::AliDns`, `::Pebble`) rather than a `Box<dyn SecretMaterialResolver>`
//! factory: `SecretMaterialResolver::required_keys` takes `&self`, so an
//! enum variant match compiles to the same zero-allocation static slice
//! lookup the sibling resolvers use.
//!
//! # Material
//!
//! `DnsCredentialMaterial` is intentionally minimal — it carries no parsed
//! data. The Controller-side consumer (`AcmeService::fetch_dns_credentials`)
//! re-fetches the Secret from Kubernetes at issuance time and reads the raw
//! `Secret.data` directly; the pipeline calls
//! [`SecretMaterialResolver::parse`] purely for validation
//! (`ResolvedRefs` condition + `SecretRefManager` registration). If ACME
//! ever migrates fully onto the credential pipeline, extend this struct with
//! the parsed `HashMap<&'static str, Zeroizing<String>>`; for now the
//! `()` shape keeps the diff minimal.

use k8s_openapi::ByteString;
use std::collections::BTreeMap;

use super::super::resolver::{ResolveError, SecretMaterialResolver};

/// Placeholder material. See module docs for why this is empty.
#[derive(Clone, Debug, Default)]
pub struct DnsCredentialMaterial;

/// DNS-01 credential resolver, parameterised by provider.
///
/// Construct with [`DnsCredentialResolver::from_provider`] from the string
/// value of `EdgionAcme.spec.challenge.dns01.provider`. Unknown providers
/// are rejected by the handler's `validate` pass — `from_provider` returns
/// `None` and the DNS resolve step is skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsCredentialResolver {
    /// Cloudflare API token — key `api-token`.
    Cloudflare,
    /// Alibaba Cloud DNS (AliDNS) — keys `access-key-id` + `access-key-secret`.
    AliDns,
    /// Pebble `challtestsrv` (integration testing only). No required keys;
    /// `api-url` is optional with default `http://localhost:8055`.
    Pebble,
}

impl DnsCredentialResolver {
    /// Map the provider string from CRD spec to a resolver variant.
    /// Returns `None` for unknown providers (handler's `validate()` already
    /// surfaces those as `Accepted=False`).
    pub fn from_provider(provider: &str) -> Option<Self> {
        match provider {
            "cloudflare" => Some(Self::Cloudflare),
            "alidns" => Some(Self::AliDns),
            "pebble" => Some(Self::Pebble),
            _ => None,
        }
    }
}

impl SecretMaterialResolver for DnsCredentialResolver {
    type Material = DnsCredentialMaterial;

    fn required_keys(&self) -> &'static [&'static str] {
        match self {
            Self::Cloudflare => &["api-token"],
            Self::AliDns => &["access-key-id", "access-key-secret"],
            Self::Pebble => &[],
        }
    }

    fn parse(&self, data: &BTreeMap<String, ByteString>) -> Result<Self::Material, ResolveError> {
        // Collect *all* missing keys at once so operators see a complete
        // diagnostic (matches `BasicAuthResolver` / `TlsCertsResolver`
        // behaviour). Don't short-circuit on the first miss.
        let required = self.required_keys();
        let missing: Vec<&'static str> = required.iter().copied().filter(|k| !data.contains_key(*k)).collect();
        if !missing.is_empty() {
            return Err(ResolveError::MissingKeys(missing));
        }

        // DNS credentials are all UTF-8 strings (API tokens, access key IDs,
        // base64-encoded secrets). A binary payload signals a malformed
        // Secret — surface as `DecodeError` so the condition reason is
        // `DecodeError` rather than a vague runtime "API failed" downstream.
        for &key in required {
            if let Some(val) = data.get(key) {
                std::str::from_utf8(&val.0).map_err(|e| ResolveError::DecodeError {
                    key,
                    source: anyhow::anyhow!(e),
                })?;
            }
        }

        Ok(DnsCredentialMaterial)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(pairs: &[(&str, &[u8])]) -> BTreeMap<String, ByteString> {
        let mut m = BTreeMap::new();
        for (k, v) in pairs {
            m.insert(k.to_string(), ByteString(v.to_vec()));
        }
        m
    }

    #[test]
    fn from_provider_known() {
        assert_eq!(
            DnsCredentialResolver::from_provider("cloudflare"),
            Some(DnsCredentialResolver::Cloudflare)
        );
        assert_eq!(
            DnsCredentialResolver::from_provider("alidns"),
            Some(DnsCredentialResolver::AliDns)
        );
        assert_eq!(
            DnsCredentialResolver::from_provider("pebble"),
            Some(DnsCredentialResolver::Pebble)
        );
    }

    #[test]
    fn from_provider_unknown_returns_none() {
        assert_eq!(DnsCredentialResolver::from_provider("route53"), None);
        assert_eq!(DnsCredentialResolver::from_provider(""), None);
    }

    #[test]
    fn cloudflare_required_keys() {
        assert_eq!(DnsCredentialResolver::Cloudflare.required_keys(), &["api-token"]);
    }

    #[test]
    fn alidns_required_keys() {
        assert_eq!(
            DnsCredentialResolver::AliDns.required_keys(),
            &["access-key-id", "access-key-secret"]
        );
    }

    #[test]
    fn pebble_has_no_required_keys() {
        assert!(DnsCredentialResolver::Pebble.required_keys().is_empty());
    }

    #[test]
    fn cloudflare_happy_path() {
        let data = mk(&[("api-token", b"cf-abcdef")]);
        assert!(DnsCredentialResolver::Cloudflare.parse(&data).is_ok());
    }

    #[test]
    fn cloudflare_missing_api_token() {
        let data = mk(&[("wrong-key", b"x")]);
        match DnsCredentialResolver::Cloudflare.parse(&data) {
            Err(ResolveError::MissingKeys(keys)) => {
                assert_eq!(keys, vec!["api-token"]);
            }
            other => panic!("expected MissingKeys, got {other:?}"),
        }
    }

    #[test]
    fn alidns_happy_path() {
        let data = mk(&[("access-key-id", b"LTAI..."), ("access-key-secret", b"secret")]);
        assert!(DnsCredentialResolver::AliDns.parse(&data).is_ok());
    }

    #[test]
    fn alidns_missing_both_keys_reports_both() {
        let data = mk(&[]);
        match DnsCredentialResolver::AliDns.parse(&data) {
            Err(ResolveError::MissingKeys(keys)) => {
                assert!(keys.contains(&"access-key-id"), "got {keys:?}");
                assert!(keys.contains(&"access-key-secret"), "got {keys:?}");
                assert_eq!(keys.len(), 2);
            }
            other => panic!("expected MissingKeys, got {other:?}"),
        }
    }

    #[test]
    fn alidns_missing_only_secret_reports_only_it() {
        let data = mk(&[("access-key-id", b"LTAI...")]);
        match DnsCredentialResolver::AliDns.parse(&data) {
            Err(ResolveError::MissingKeys(keys)) => {
                assert_eq!(keys, vec!["access-key-secret"]);
            }
            other => panic!("expected MissingKeys, got {other:?}"),
        }
    }

    #[test]
    fn pebble_empty_data_ok() {
        let data = mk(&[]);
        assert!(DnsCredentialResolver::Pebble.parse(&data).is_ok());
    }

    #[test]
    fn cloudflare_rejects_non_utf8_token() {
        let data = mk(&[("api-token", &[0xff, 0xfe][..])]);
        match DnsCredentialResolver::Cloudflare.parse(&data) {
            Err(ResolveError::DecodeError { key, .. }) => assert_eq!(key, "api-token"),
            other => panic!("expected DecodeError, got {other:?}"),
        }
    }

    #[test]
    fn extra_keys_ignored() {
        let data = mk(&[("api-token", b"cf"), ("unrelated", b"x")]);
        assert!(DnsCredentialResolver::Cloudflare.parse(&data).is_ok());
    }

    #[test]
    fn default_allowed_secret_types_is_opaque() {
        // DNS credentials are stored in Opaque secrets by convention; don't
        // override the trait default, so that the pipeline accepts Opaque
        // and unset-type secrets equally.
        assert_eq!(
            DnsCredentialResolver::Cloudflare.allowed_secret_types(),
            &["Opaque", ""]
        );
    }
}
