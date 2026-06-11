use k8s_openapi::ByteString;
use std::collections::BTreeMap;
use zeroize::Zeroizing;

use super::super::{
    materials::AcmeAccountMaterial,
    resolver::{ResolveError, SecretMaterialResolver},
};

/// ACME account credentials resolver.
///
/// Secret key (camelCase, spec §5):
/// - `accountCredentials` — JSON-serialized `instant_acme::AccountCredentials`,
///   holding the account private key PEM. Zeroize-covered in-memory.
///
/// The resolver only performs a JSON well-formedness pre-check; it does not
/// deserialize into the concrete `AccountCredentials` type (that is the
/// responsibility of the ACME service, avoiding `instant_acme` coupling here).
#[derive(Debug, Default, Clone, Copy)]
pub struct AcmeAccountResolver;

/// Canonical Secret key for the ACME account blob. Shared between reader
/// (resolver) and writer (ACME service) so they never drift.
pub const ACME_ACCOUNT_KEY: &str = "accountCredentials";

impl SecretMaterialResolver for AcmeAccountResolver {
    type Material = AcmeAccountMaterial;

    fn required_keys(&self) -> &'static [&'static str] {
        &[ACME_ACCOUNT_KEY]
    }

    // NOTE on Zeroize scope: only the final `Zeroizing::new(json.to_string())`
    // copy held by `AcmeAccountMaterial` is zero-wiped on drop. The
    // intermediate artifacts produced below — the `serde_json::Value` tree
    // allocated by `from_str::<Value>` for well-formedness checking, and the
    // `String` created by `json.to_string()` before being wrapped — live on
    // the heap without zeroization. Callers must treat `Zeroizing` here as
    // defense-in-depth; see docs on `AcmeAccountMaterial`. If a stricter
    // guarantee is ever required, replace the `from_str::<Value>` pre-check
    // with a hand-written JSON well-formedness scanner that never allocates
    // a parsed tree.
    fn parse(&self, data: &BTreeMap<String, ByteString>) -> Result<Self::Material, ResolveError> {
        let raw = data
            .get(ACME_ACCOUNT_KEY)
            .ok_or_else(|| ResolveError::MissingKeys(vec![ACME_ACCOUNT_KEY]))?;

        let json = std::str::from_utf8(&raw.0).map_err(|e| ResolveError::DecodeError {
            key: ACME_ACCOUNT_KEY,
            source: anyhow::anyhow!(e),
        })?;

        serde_json::from_str::<serde_json::Value>(json).map_err(|e| ResolveError::DecodeError {
            key: ACME_ACCOUNT_KEY,
            source: anyhow::anyhow!(e),
        })?;

        Ok(AcmeAccountMaterial {
            credentials: Zeroizing::new(json.to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::{ResolveError, SecretMaterialResolver};

    fn mk(val: &[u8]) -> BTreeMap<String, ByteString> {
        let mut m = BTreeMap::new();
        m.insert(ACME_ACCOUNT_KEY.to_string(), ByteString(val.to_vec()));
        m
    }

    #[test]
    fn parse_happy_path() {
        let data = mk(br#"{"account_url":"https://acme/example","key":"<pem>"}"#);
        let m = AcmeAccountResolver.parse(&data).unwrap();
        assert!(m.credentials.starts_with('{'));
        assert!(m.credentials.contains("account_url"));
    }

    #[test]
    fn parse_missing() {
        let data = BTreeMap::new();
        match AcmeAccountResolver.parse(&data) {
            Err(ResolveError::MissingKeys(k)) => assert_eq!(k, vec![ACME_ACCOUNT_KEY]),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parse_invalid_json() {
        let data = mk(b"not-json-at-all");
        match AcmeAccountResolver.parse(&data) {
            Err(ResolveError::DecodeError { key, .. }) => assert_eq!(key, ACME_ACCOUNT_KEY),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parse_invalid_utf8() {
        let data = mk(&[0xff, 0xfe][..]);
        match AcmeAccountResolver.parse(&data) {
            Err(ResolveError::DecodeError { key, .. }) => assert_eq!(key, ACME_ACCOUNT_KEY),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parse_extra_keys_ignored() {
        let mut data = mk(br#"{"ok":true}"#);
        data.insert("unrelated".to_string(), ByteString(b"x".to_vec()));
        assert!(AcmeAccountResolver.parse(&data).is_ok());
    }

    #[test]
    fn required_keys_is_account_credentials() {
        assert_eq!(AcmeAccountResolver.required_keys(), &[ACME_ACCOUNT_KEY]);
    }

    #[test]
    fn exported_constant_value() {
        assert_eq!(ACME_ACCOUNT_KEY, "accountCredentials");
    }
}
