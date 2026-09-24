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
