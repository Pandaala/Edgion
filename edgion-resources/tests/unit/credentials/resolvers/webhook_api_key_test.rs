use super::*;
use crate::credentials::{ResolveError, SecretMaterialResolver};

#[test]
fn parse_happy_path() {
    let mut data = BTreeMap::new();
    data.insert("apiKey".to_string(), ByteString(b"my-secret-key".to_vec()));
    let m = WebhookApiKeyResolver.parse(&data).unwrap();
    assert_eq!(m.api_key, "my-secret-key");
}

#[test]
fn parse_missing() {
    let data = BTreeMap::new();
    match WebhookApiKeyResolver.parse(&data) {
        Err(ResolveError::MissingKeys(k)) => assert_eq!(k, vec!["apiKey"]),
        other => panic!("{other:?}"),
    }
}

#[test]
fn parse_invalid_utf8() {
    let mut data = BTreeMap::new();
    data.insert("apiKey".to_string(), ByteString(vec![0xff, 0xfe]));
    match WebhookApiKeyResolver.parse(&data) {
        Err(ResolveError::DecodeError { key: "apiKey", .. }) => {}
        other => panic!("{other:?}"),
    }
}
