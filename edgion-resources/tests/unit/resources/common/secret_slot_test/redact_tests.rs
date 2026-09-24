use super::*;
use crate::redact::AdminRedactGuard;
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;
use std::collections::BTreeMap;

fn slot_with_secret() -> SecretSlot {
    let mut data = BTreeMap::new();
    data.insert("password".to_string(), ByteString(b"SLOT-PRIVATE-KEY".to_vec()));
    SecretSlot {
        secret_ref: SecretObjectReference {
            group: None,
            kind: None,
            name: "s".to_string(),
            namespace: None,
        },
        secret: Some(Secret {
            data: Some(data),
            ..Default::default()
        }),
    }
}

#[test]
fn secret_slot_redacted_under_guard() {
    let json = {
        let _g = AdminRedactGuard::new();
        serde_json::to_value(slot_with_secret()).unwrap()
    };
    assert_eq!(json["secret"], "[redacted]");
}

#[test]
fn secret_slot_real_without_guard_for_transport() {
    // Transport regression: conf_sync (controller side, no guard) must emit the
    // real Secret object so the gateway can rebuild it.
    let json = serde_json::to_value(slot_with_secret()).unwrap();
    assert!(json["secret"].is_object(), "real Secret must serialize without guard");
    assert!(json["secret"]["data"].get("password").is_some());
}
