use super::*;

fn fingerprint(query: &str) -> [u8; 32] {
    ParsedGraphqlRequest::parse_registered(query, None)
        .unwrap()
        .document_fingerprint()
        .unwrap()
}

#[test]
fn canonical_tokens_ignore_trivia_without_merging_boundaries() {
    assert_eq!(fingerprint("{ a bc }"), fingerprint("# comment\n{a,bc}"));
    assert_ne!(fingerprint("{ a bc }"), fingerprint("{ ab c }"));
    assert_ne!(
        fingerprint("{ f(v:\"hello world\") }"),
        fingerprint("{ f(v:\"hello  world\") }")
    );
    assert_ne!(
        fingerprint("{ f(v:\"hello\") }"),
        fingerprint("{ f(v:\"\\u0068ello\") }")
    );
    assert_ne!(
        fingerprint("{ f(v:\"\"\"hello\"\"\") }"),
        fingerprint("{ f(v:\"\"\" hello\"\"\") }")
    );
    assert_ne!(
        fingerprint("{ ...F } fragment F on Query { ping }"),
        fingerprint("fragment F on Query { ping } { ...F }")
    );
}

#[test]
fn fingerprint_charges_carried_work_including_discarded_tokens() {
    let mut parsed = ParsedGraphqlRequest::parse_registered("# comment\n{ ping }", None).unwrap();
    let before = parsed.work.0;
    parsed.document_fingerprint().unwrap();
    assert!(parsed.work.0 > before);
    parsed.work = super::super::Work(super::super::MAX_WORK - 1);
    assert_eq!(parsed.document_fingerprint(), Err(GraphqlError::Invalid));
}
