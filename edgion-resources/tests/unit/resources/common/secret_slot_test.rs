use super::SecretSlot;

#[test]
fn deserialize_secret_ref_only() {
    let yaml = "secretRef:\n  name: creds";
    let slot: SecretSlot = yaml_serde::from_str(yaml).unwrap();
    assert_eq!(slot.secret_ref.name, "creds");
    assert!(slot.secret.is_none());
}

#[test]
fn rejects_unknown_field() {
    let yaml = "secretRef:\n  name: creds\nbogus: 1";
    assert!(yaml_serde::from_str::<SecretSlot>(yaml).is_err());
}

#[test]
fn secret_ref_required() {
    let yaml = "{}";
    assert!(yaml_serde::from_str::<SecretSlot>(yaml).is_err());
}
