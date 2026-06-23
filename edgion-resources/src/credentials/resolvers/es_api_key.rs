use k8s_openapi::ByteString;
use std::collections::BTreeMap;

use super::super::{
    materials::EsApiKeyMaterial,
    resolver::{ResolveError, SecretMaterialResolver},
};

/// Elasticsearch apiKey auth.
///
/// Secret keys (camelCase, spec §5):
/// - `apiKeyId` — required
/// - `apiKeySecret` — required
///
/// All missing keys are reported in a single error (multi-key contract).
#[derive(Debug, Default, Clone, Copy)]
pub struct EsApiKeyResolver;

impl SecretMaterialResolver for EsApiKeyResolver {
    type Material = EsApiKeyMaterial;

    fn required_keys(&self) -> &'static [&'static str] {
        &["apiKeyId", "apiKeySecret"]
    }

    fn parse(&self, data: &BTreeMap<String, ByteString>) -> Result<Self::Material, ResolveError> {
        let mut missing = Vec::new();
        if !data.contains_key("apiKeyId") {
            missing.push("apiKeyId");
        }
        if !data.contains_key("apiKeySecret") {
            missing.push("apiKeySecret");
        }
        if !missing.is_empty() {
            return Err(ResolveError::MissingKeys(missing));
        }

        let api_key_id = decode_utf8(data, "apiKeyId")?;
        let api_key_secret = decode_utf8(data, "apiKeySecret")?;
        Ok(EsApiKeyMaterial {
            api_key_id,
            api_key_secret,
        })
    }
}

fn decode_utf8(data: &BTreeMap<String, ByteString>, key: &'static str) -> Result<String, ResolveError> {
    let raw = data.get(key).expect("presence already checked");
    std::str::from_utf8(&raw.0)
        .map(|s| s.to_string())
        .map_err(|e| ResolveError::DecodeError {
            key,
            source: anyhow::anyhow!(e),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::{ResolveError, SecretMaterialResolver};

    fn mk(data: &[(&str, &[u8])]) -> BTreeMap<String, ByteString> {
        data.iter()
            .map(|(k, v)| (k.to_string(), ByteString(v.to_vec())))
            .collect()
    }

    #[test]
    fn parse_happy_path() {
        let data = mk(&[("apiKeyId", b"kid"), ("apiKeySecret", b"ksec")]);
        let m = EsApiKeyResolver.parse(&data).unwrap();
        assert_eq!(m.api_key_id, "kid");
        assert_eq!(m.api_key_secret, "ksec");
    }

    #[test]
    fn parse_missing_both_reports_both() {
        let data = BTreeMap::new();
        match EsApiKeyResolver.parse(&data) {
            Err(ResolveError::MissingKeys(k)) => {
                assert!(k.contains(&"apiKeyId"), "missing apiKeyId not reported");
                assert!(k.contains(&"apiKeySecret"), "missing apiKeySecret not reported");
                assert_eq!(k.len(), 2, "exactly two missing keys, got {k:?}");
            }
            other => panic!("expected MissingKeys, got {other:?}"),
        }
    }

    #[test]
    fn parse_missing_one_reports_that_one() {
        let data = mk(&[("apiKeyId", b"kid")]);
        match EsApiKeyResolver.parse(&data) {
            Err(ResolveError::MissingKeys(k)) => {
                assert_eq!(k, vec!["apiKeySecret"]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parse_invalid_utf8_reports_key() {
        let data = mk(&[("apiKeyId", &[0xff, 0xfe][..]), ("apiKeySecret", b"ksec")]);
        match EsApiKeyResolver.parse(&data) {
            Err(ResolveError::DecodeError { key: "apiKeyId", .. }) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parse_extra_keys_ignored() {
        let data = mk(&[("apiKeyId", b"kid"), ("apiKeySecret", b"ksec"), ("unrelated", b"x")]);
        assert!(EsApiKeyResolver.parse(&data).is_ok());
    }
}
