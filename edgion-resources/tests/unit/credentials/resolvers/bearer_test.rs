use super::*;
use crate::credentials::{ResolveError, SecretMaterialResolver};

#[test]
fn parse_happy_path() {
    let mut data = BTreeMap::new();
    data.insert("bearerToken".to_string(), ByteString(b"tk".to_vec()));
    let m = BearerResolver.parse(&data).unwrap();
    assert_eq!(m.token, "tk");
}

#[test]
fn parse_missing() {
    let data = BTreeMap::new();
    match BearerResolver.parse(&data) {
        Err(ResolveError::MissingKeys(k)) => assert_eq!(k, vec!["bearerToken"]),
        other => panic!("{other:?}"),
    }
}

#[test]
fn parse_invalid_utf8() {
    let mut data = BTreeMap::new();
    data.insert("bearerToken".to_string(), ByteString(vec![0xff, 0xfe]));
    match BearerResolver.parse(&data) {
        Err(ResolveError::DecodeError { key: "bearerToken", .. }) => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn parse_extra_keys_ignored() {
    let mut data = BTreeMap::new();
    data.insert("bearerToken".to_string(), ByteString(b"tk".to_vec()));
    data.insert("unrelated".to_string(), ByteString(b"x".to_vec()));
    assert!(BearerResolver.parse(&data).is_ok());
}

#[test]
fn parse_accepts_rfc_6750_b64token_shape() {
    for token in ["a", "AZaz09-._~+/", "token=", "token==="] {
        let data = BTreeMap::from([("bearerToken".to_string(), ByteString(token.as_bytes().to_vec()))]);
        assert_eq!(BearerResolver.parse(&data).unwrap().token, token);
    }
}

#[test]
fn parse_rejects_invalid_bearer_values_without_echoing_them() {
    for token in [
        "",
        " ",
        "token value",
        "Bearer token",
        "token\r\n",
        "=token",
        "tok=en",
        "té",
    ] {
        let data = BTreeMap::from([("bearerToken".to_string(), ByteString(token.as_bytes().to_vec()))]);
        let error = BearerResolver.parse(&data).unwrap_err();
        assert!(matches!(
            error,
            ResolveError::InvalidValue {
                key: "bearerToken",
                class: InvalidValueClass::BearerToken,
            }
        ));
    }

    let private = "unique-private value";
    let data = BTreeMap::from([("bearerToken".to_string(), ByteString(private.as_bytes().to_vec()))]);
    let error = BearerResolver.parse(&data).unwrap_err();
    assert!(!format!("{error:?}").contains(private));
    assert!(!error.to_string().contains(private));
}
