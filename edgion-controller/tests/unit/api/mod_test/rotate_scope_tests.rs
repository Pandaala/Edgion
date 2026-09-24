use super::{ApiRotateScope, RotateBody};

#[test]
fn rotate_scope_parses_with_leaf_default() {
    let s: RotateBody = serde_json::from_str("{}").unwrap();
    assert!(matches!(s.scope, ApiRotateScope::Leaf));
    let s: RotateBody = serde_json::from_str(r#"{"scope":"ca"}"#).unwrap();
    assert!(matches!(s.scope, ApiRotateScope::Ca));
    assert!(serde_json::from_str::<RotateBody>(r#"{"scope":"root"}"#).is_err());
}
