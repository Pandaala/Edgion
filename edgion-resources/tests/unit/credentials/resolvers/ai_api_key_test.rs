use super::*;

#[test]
fn parses_fixed_api_key() {
    let data = BTreeMap::from([(API_KEY.to_string(), ByteString(b"provider-key".to_vec()))]);
    let material = AiApiKeyResolver.parse(&data).unwrap();
    assert_eq!(material.api_key, "provider-key");
}

#[test]
fn rejects_missing_or_non_utf8_api_key() {
    assert!(matches!(
        AiApiKeyResolver.parse(&BTreeMap::new()),
        Err(ResolveError::MissingKeys(keys)) if keys == vec![API_KEY]
    ));
    let data = BTreeMap::from([(API_KEY.to_string(), ByteString(vec![0xff]))]);
    assert!(matches!(
        AiApiKeyResolver.parse(&data),
        Err(ResolveError::DecodeError { key: API_KEY, .. })
    ));
}
