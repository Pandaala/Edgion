use super::*;
use crate::credentials::{ResolveError, SecretMaterialResolver};

#[test]
fn parse_happy_path() {
    let mut data = BTreeMap::new();
    data.insert("hmacKey".to_string(), ByteString(b"k".to_vec()));
    assert_eq!(HmacResolver.parse(&data).unwrap().key, "k");
}

#[test]
fn parse_missing() {
    let data = BTreeMap::new();
    match HmacResolver.parse(&data) {
        Err(ResolveError::MissingKeys(k)) => assert_eq!(k, vec!["hmacKey"]),
        other => panic!("{other:?}"),
    }
}

#[test]
fn parse_invalid_utf8() {
    let mut data = BTreeMap::new();
    data.insert("hmacKey".to_string(), ByteString(vec![0xff, 0xfe]));
    match HmacResolver.parse(&data) {
        Err(ResolveError::DecodeError { key: "hmacKey", .. }) => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn parse_extra_keys_ignored() {
    let mut data = BTreeMap::new();
    data.insert("hmacKey".to_string(), ByteString(b"k".to_vec()));
    data.insert("unrelated".to_string(), ByteString(b"x".to_vec()));
    assert!(HmacResolver.parse(&data).is_ok());
}
