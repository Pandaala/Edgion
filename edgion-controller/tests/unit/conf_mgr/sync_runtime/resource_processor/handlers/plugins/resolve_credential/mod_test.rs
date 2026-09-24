use super::read_secret_utf8;
use k8s_openapi::{api::core::v1::Secret, ByteString};
use std::collections::BTreeMap;

#[test]
fn secret_utf8_borrows_exact_first_usable_value() {
    let secret = Secret {
        data: Some(BTreeMap::from([
            ("invalid".into(), ByteString(vec![0xff])),
            ("blank".into(), ByteString(b" \t ".to_vec())),
            ("usable".into(), ByteString(b"  exact-secret \t".to_vec())),
            ("later".into(), ByteString(b"other-secret".to_vec())),
        ])),
        ..Default::default()
    };
    let value = read_secret_utf8(&secret, &["missing", "invalid", "blank", "usable", "later"]).unwrap();
    assert_eq!(value, "  exact-secret \t");
    assert_eq!(value.as_ptr(), secret.data.as_ref().unwrap()["usable"].0.as_ptr());
    assert!(read_secret_utf8(&secret, &["missing", "invalid", "blank"]).is_none());
}
