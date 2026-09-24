use super::*;

#[test]
fn request_context_allows_exactly_one_json_schema_validator() {
    let mut ctx = EdgionHttpContext::new();
    assert!(ctx.claim_json_schema_validation());
    assert!(!ctx.claim_json_schema_validation());
}
