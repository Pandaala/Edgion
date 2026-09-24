use super::{ResolveError, SecretMaterialResolver};
use k8s_openapi::ByteString;
use std::collections::BTreeMap;

struct DummyResolver;
#[derive(Debug)]
struct DummyMaterial(String);

impl SecretMaterialResolver for DummyResolver {
    type Material = DummyMaterial;
    fn required_keys(&self) -> &'static [&'static str] {
        &["alpha", "beta"]
    }
    fn parse(&self, data: &BTreeMap<String, ByteString>) -> Result<Self::Material, ResolveError> {
        let alpha = data
            .get("alpha")
            .ok_or_else(|| ResolveError::MissingKeys(vec!["alpha"]))?;
        let value = std::str::from_utf8(&alpha.0).map_err(|e| ResolveError::DecodeError {
            key: "alpha",
            source: anyhow::anyhow!(e),
        })?;
        Ok(DummyMaterial(value.to_string()))
    }
}

#[test]
fn required_keys_exposed() {
    assert_eq!(DummyResolver.required_keys(), &["alpha", "beta"]);
}

#[test]
fn default_allowed_types_is_opaque() {
    assert_eq!(DummyResolver.allowed_secret_types(), &["Opaque", ""]);
}

#[test]
fn parse_happy_path() {
    let mut data = BTreeMap::new();
    data.insert("alpha".to_string(), ByteString(b"hello".to_vec()));
    data.insert("beta".to_string(), ByteString(b"world".to_vec()));
    let m = DummyResolver.parse(&data).unwrap();
    assert_eq!(m.0, "hello");
}

#[test]
fn parse_missing_key() {
    let data = BTreeMap::new();
    match DummyResolver.parse(&data) {
        Err(ResolveError::MissingKeys(keys)) => assert_eq!(keys, vec!["alpha"]),
        other => panic!("expected MissingKeys, got {other:?}"),
    }
}

#[test]
fn parse_decode_error() {
    let mut data = BTreeMap::new();
    data.insert("alpha".to_string(), ByteString(vec![0xff, 0xfe]));
    match DummyResolver.parse(&data) {
        Err(ResolveError::DecodeError { key, .. }) => assert_eq!(key, "alpha"),
        other => panic!("expected DecodeError, got {other:?}"),
    }
}
