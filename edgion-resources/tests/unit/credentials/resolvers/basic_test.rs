use super::BasicAuthResolver;
use crate::credentials::{ResolveError, SecretMaterialResolver};
use k8s_openapi::ByteString;
use std::collections::BTreeMap;

fn mk(data: &[(&str, &[u8])]) -> BTreeMap<String, ByteString> {
    data.iter()
        .map(|(k, v)| (k.to_string(), ByteString(v.to_vec())))
        .collect()
}

#[test]
fn parse_full() {
    let data = mk(&[("username", b"alice"), ("password", b"secret")]);
    let m = BasicAuthResolver.parse(&data).unwrap();
    assert_eq!(m.username.as_deref(), Some("alice"));
    assert_eq!(m.password, "secret");
}

#[test]
fn parse_password_only() {
    let data = mk(&[("password", b"secret")]);
    let m = BasicAuthResolver.parse(&data).unwrap();
    assert_eq!(m.username, None);
    assert_eq!(m.password, "secret");
}

#[test]
fn parse_missing_password() {
    let data = mk(&[("username", b"alice")]);
    match BasicAuthResolver.parse(&data) {
        Err(ResolveError::MissingKeys(k)) => assert_eq!(k, vec!["password"]),
        other => panic!("{other:?}"),
    }
}

#[test]
fn parse_extra_keys_ignored() {
    let data = mk(&[("password", b"p"), ("unrelated", b"x")]);
    let m = BasicAuthResolver.parse(&data).unwrap();
    assert_eq!(m.password, "p");
}

#[test]
fn parse_invalid_utf8_password() {
    let data = mk(&[("password", &[0xff, 0xfe][..])]);
    match BasicAuthResolver.parse(&data) {
        Err(ResolveError::DecodeError { key: "password", .. }) => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn required_keys_order() {
    assert_eq!(BasicAuthResolver.required_keys(), &["password"]);
}

#[test]
fn allows_basic_auth_and_opaque_types() {
    let allowed = BasicAuthResolver.allowed_secret_types();
    assert!(allowed.contains(&"kubernetes.io/basic-auth"));
    assert!(allowed.contains(&"Opaque"));
    assert!(allowed.contains(&""));
}
