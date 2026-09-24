use super::*;

#[test]
fn extract_account_credentials() {
    let mut data = BTreeMap::new();
    data.insert(
        ACME_ACCOUNT_KEY.to_string(),
        ByteString(br#"{"preferred":true}"#.to_vec()),
    );
    let z = extract_account_json_string(&data).unwrap().expect("Some");
    assert!(z.contains("preferred"));
}

#[test]
fn extract_legacy_key_is_missing() {
    let mut data = BTreeMap::new();
    data.insert("credentials".to_string(), ByteString(br#"{"only":"legacy"}"#.to_vec()));
    assert!(extract_account_json_string(&data).unwrap().is_none());
}

#[test]
fn extract_none_when_no_keys() {
    let data = BTreeMap::new();
    assert!(extract_account_json_string(&data).unwrap().is_none());
}
