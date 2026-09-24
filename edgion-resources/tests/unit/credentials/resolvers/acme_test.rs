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
