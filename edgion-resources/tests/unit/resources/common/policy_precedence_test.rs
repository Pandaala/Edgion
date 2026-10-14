use super::*;
use crate::{BackendTLSPolicy, EdgionBackendTrafficPolicy};
use serde_json::json;

#[test]
fn policy_precedence_preserves_complete_instants_and_missing_last() {
    let parse = |value: &str| Some(value.parse::<DateTime<Utc>>().unwrap());
    assert!(
        policy_precedence_key(parse("2026-01-01T00:00:00.100000001Z"), "z")
            < policy_precedence_key(parse("2026-01-01T00:00:00.100000002Z"), "a")
    );
    assert_eq!(
        policy_precedence_key(parse("2026-01-01T01:00:00.123+01:00"), "a"),
        policy_precedence_key(parse("2026-01-01T00:00:00.123Z"), "a")
    );
    assert!(policy_precedence_key(Some(DateTime::<Utc>::MAX_UTC), "z") < policy_precedence_key(None, "a"));
    assert!(policy_precedence_key(None, "a") < policy_precedence_key(None, "b"));
}

#[test]
fn policy_precedence_wire_roundtrips_the_accepted_chrono_domain() {
    let instants = [
        DateTime::<Utc>::MIN_UTC,
        DateTime::<Utc>::MAX_UTC,
        "-0001-01-01T00:00:00.123456789Z".parse().unwrap(),
        "+10000-01-01T00:00:00.123456789Z".parse().unwrap(),
        "1969-12-31T23:59:59.999999999Z".parse().unwrap(),
        "2016-12-31T23:59:60.123456789Z".parse().unwrap(),
        "2026-01-01T01:00:00.123456789+01:00".parse().unwrap(),
    ];
    for instant in instants {
        let prepared = PreparedPolicyPrecedence {
            creation_timestamp: Some(instant),
        };
        let wire = serde_json::to_value(prepared).unwrap();
        assert_eq!(
            serde_json::from_value::<PreparedPolicyPrecedence>(wire.clone()).unwrap(),
            prepared
        );
        // Both schemas accept the same creation instant, independently of the
        // Kubernetes Time serializer that intentionally rounds metadata.
        let source = json!({"metadata":{"creationTimestamp":wire["creationTimestamp"]},
            "spec":{"targetRefs":[{"group":"","kind":"Service","name":"backend"}],
                "validation":{"hostname":"backend.example","wellKnownCACertificates":"System"}}});
        let tls: BackendTLSPolicy = serde_json::from_value(source.clone()).unwrap();
        let traffic: EdgionBackendTrafficPolicy = serde_json::from_value(source).unwrap();
        assert_eq!(tls.metadata.creation_timestamp.unwrap().0, instant);
        assert_eq!(traffic.metadata.creation_timestamp.unwrap().0, instant);
    }
    let missing = PreparedPolicyPrecedence {
        creation_timestamp: None,
    };
    assert_eq!(serde_json::to_value(Some(missing)).unwrap(), json!({}));
    assert_eq!(
        serde_json::to_value(None::<PreparedPolicyPrecedence>).unwrap(),
        json!(null)
    );
}

#[test]
fn policy_precedence_is_transported_but_not_operator_schema() {
    let source = json!({"metadata":{"name":"policy"},"spec":{
        "preparedPrecedence":{},"targetRefs":[{"group":"","kind":"Service","name":"backend"}],
        "validation":{"hostname":"backend.example","wellKnownCACertificates":"System"}}});
    let tls: BackendTLSPolicy = serde_json::from_value(source.clone()).unwrap();
    let traffic: EdgionBackendTrafficPolicy = serde_json::from_value(source).unwrap();
    for wire in [
        serde_json::to_value(tls).unwrap(),
        serde_json::to_value(traffic).unwrap(),
    ] {
        assert_eq!(wire["spec"]["preparedPrecedence"], json!({}));
    }
    assert!(!serde_json::to_string(&schemars::schema_for!(
        crate::resources::backend_tls_policy::BackendTLSPolicySpec
    ))
    .unwrap()
    .contains("preparedPrecedence"));
    assert!(!serde_json::to_string(&schemars::schema_for!(
        crate::resources::edgion_backend_traffic_policy::EdgionBackendTrafficPolicySpec
    ))
    .unwrap()
    .contains("preparedPrecedence"));
}
