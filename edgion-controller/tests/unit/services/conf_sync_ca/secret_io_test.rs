use super::*;
use std::collections::BTreeMap;

/// Regression (found in cluster verification): an unforced apply 409s against
/// the POST-created CA Secret, wedging CA renewal forever. Both Secrets this
/// module writes must be force-applied.
#[test]
fn apply_params_force_and_field_manager() {
    let pp = apply_params();
    assert!(
        pp.force,
        "SSA apply must be forced or CA renewal 409s (see apply_params docs)"
    );
    assert_eq!(pp.field_manager.as_deref(), Some(FIELD_MANAGER));
}

#[test]
fn ca_secret_data_preserves_key_across_renewal() {
    use crate::services::conf_sync_ca::Ca;
    // A CA renewal re-signs the cert with a fresh window but the SAME key.
    // The persisted Secret map MUST carry the new cert AND the original key.
    let ca = Ca::generate_root(::time::Duration::days(3650)).unwrap();
    let original_key = ca.ca_key_pem().to_string();
    let renewed = ca.renew_root_same_key(::time::Duration::days(3650)).unwrap();
    let data = ca_secret_data(&renewed);
    // Both fields present.
    assert!(data.contains_key("ca.crt"), "ca.crt must be present");
    assert!(data.contains_key("ca.key"), "ca.key must be present");
    // ca.crt is the renewed cert PEM.
    assert_eq!(
        data.get("ca.crt").unwrap(),
        &renewed.ca_cert_pem().as_bytes().to_vec(),
        "ca.crt must be the renewed certificate"
    );
    // ca.key is the ORIGINAL key bytes (renew keeps the key).
    assert_eq!(
        data.get("ca.key").unwrap(),
        &original_key.as_bytes().to_vec(),
        "ca.key must be preserved as the original root key bytes"
    );
    // And the cert actually changed (sanity: this was a real renewal write).
    assert_ne!(
        data.get("ca.crt").unwrap(),
        &ca.ca_cert_pem().as_bytes().to_vec(),
        "renewed cert should differ from the original cert"
    );
}

#[test]
fn build_secret_sets_name_ns_label_and_data() {
    let mut data = BTreeMap::new();
    data.insert("ca.crt".to_string(), b"X".to_vec());
    let s = build_secret("edgion-conf-sync-ca", "edgion-system", data);
    assert_eq!(s.metadata.name.as_deref(), Some("edgion-conf-sync-ca"));
    assert_eq!(s.metadata.namespace.as_deref(), Some("edgion-system"));
    assert!(s.metadata.labels.as_ref().unwrap().contains_key("edgion.io/managed-by"));
    assert!(s.data.as_ref().unwrap().contains_key("ca.crt"));
    assert_eq!(s.type_.as_deref(), Some("Opaque"));
}
