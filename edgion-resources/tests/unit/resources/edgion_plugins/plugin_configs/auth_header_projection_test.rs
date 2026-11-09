use super::*;

#[test]
fn authenticated_projection_rejects_host_without_restricting_generic_projection() {
    for destination in [
        "Host",
        "host",
        "HOST",
        "hOsT",
        "Content-Length",
        "transfer-encoding",
        "Connection",
    ] {
        let mapping = HashMap::from([("tenant".to_string(), destination.to_string())]);
        assert_eq!(
            compile_authenticated_claim_header_projection(Some(&mapping), &[]),
            Err(PROTOCOL_OWNED_CLAIM_DESTINATION)
        );
        assert!(compile_claim_header_projection(Some(&mapping)).is_ok());
    }
    let mapping = HashMap::from([("tenant".to_string(), "X-Tenant".to_string())]);
    let plan = compile_authenticated_claim_header_projection(Some(&mapping), &[]).unwrap();
    assert_eq!(plan.entries()[0].header_name(), "x-tenant");
}

#[test]
fn compilation_canonicalizes_destinations_and_is_stable_across_insertion_order() {
    let first = HashMap::from([
        ("subject".to_string(), "X-Subject".to_string()),
        ("tenant".to_string(), "X-Tenant".to_string()),
    ]);
    let second = HashMap::from([
        ("tenant".to_string(), "X-Tenant".to_string()),
        ("subject".to_string(), "X-Subject".to_string()),
    ]);

    let first = compile_claim_header_projection(Some(&first)).unwrap();
    let second = compile_claim_header_projection(Some(&second)).unwrap();

    assert_eq!(first, second);
    assert_eq!(first.entries()[0].claim_path(), "subject");
    assert_eq!(first.entries()[0].header_name(), "x-subject");
    assert_eq!(first.entries()[1].claim_path(), "tenant");
    assert_eq!(first.entries()[1].header_name(), "x-tenant");
}

#[test]
fn compilation_rejects_case_insensitive_duplicate_destinations() {
    let mapping = HashMap::from([
        ("subject".to_string(), "X-Identity".to_string()),
        ("email".to_string(), "x-identity".to_string()),
    ]);

    assert_eq!(
        compile_claim_header_projection(Some(&mapping)),
        Err(DUPLICATE_HEADER_DESTINATION)
    );
}

#[test]
fn compilation_rejects_destinations_already_owned_by_the_plugin() {
    let mapping = HashMap::from([("subject".to_string(), "X-Identity".to_string())]);

    assert_eq!(
        compile_claim_header_projection_with_owned_destinations(Some(&mapping), &["x-identity"]),
        Err(DUPLICATE_HEADER_DESTINATION)
    );
}

#[test]
fn compilation_rejects_invalid_destination_names() {
    for destination in ["", "bad header", "x-bad\r\nname"] {
        let mapping = HashMap::from([("subject".to_string(), destination.to_string())]);
        assert_eq!(
            compile_claim_header_projection(Some(&mapping)),
            Err(INVALID_HEADER_DESTINATION)
        );
    }
}
