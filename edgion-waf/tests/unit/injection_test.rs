use super::*;

#[test]
fn work_cost_enforces_hard_input_and_checked_arithmetic() {
    assert_eq!(work_cost(Kind::Sqli, 0), Some(BASE_WORK));
    assert_eq!(work_cost(Kind::Xss, 1), Some(BASE_WORK + XSS_WORK_PER_BYTE));
    assert!(work_cost(Kind::Sqli, MAX_INPUT_BYTES).is_some());
    assert_eq!(work_cost(Kind::Xss, MAX_INPUT_BYTES + 1), None);
    assert_eq!(work_cost(Kind::Xss, usize::MAX), None);
}

#[test]
fn detection_and_fingerprint_match_pinned_examples() {
    assert_eq!(
        detect_sqli(b"1 OR 1=1").map(|fp| fp.as_bytes().to_vec()),
        Some(b"1&1".to_vec())
    );
    assert_eq!(detect_sqli(b"ordinary text"), None);
    assert!(detect_xss(b"<script>alert(1)</script>"));
    assert!(!detect_xss(b"<p>Hello</p>"));
    assert!(detect_sqli(b"'-(f=q'\xbf").is_some());
    assert_eq!(detect_sqli(b"1 OR q'\xbf\xc2\xbf'").unwrap().as_bytes(), b"1&s");
    assert!(detect_xss(b"<a \xffid=x><script>alert(1)</script>"));
}

#[test]
fn full_size_and_adversarial_inputs_finish_within_default_work_budget() {
    let full = vec![b'a'; MAX_INPUT_BYTES];
    assert!(work_cost(Kind::Sqli, full.len()).unwrap() <= crate::seclang::DEFAULT_MAX_RUNTIME_OPERATOR_WORK_BYTES);
    assert!(work_cost(Kind::Xss, full.len()).unwrap() <= crate::seclang::DEFAULT_MAX_RUNTIME_OPERATOR_WORK_BYTES);
    assert!(detect_sqli(&full).is_none());
    assert!(!detect_xss(&full));

    for pattern in [
        b"'/*".as_slice(),
        b"<!--".as_slice(),
        b"<&".as_slice(),
        b"<a ".as_slice(),
        b"a.a`".as_slice(),
    ] {
        let mut input = Vec::with_capacity(256 * 1024);
        while input.len() < 256 * 1024 {
            input.extend_from_slice(pattern);
        }
        input.truncate(256 * 1024);
        let _ = detect_sqli(&input);
        let _ = detect_xss(&input);
    }
}

/// F-S4-3: libinjection-go v0.3.2 indexes past the end on these CDATA shapes;
/// libinjection C and libinjection-go v0.3.3 report neither SQLi nor XSS.
#[test]
fn cdata_lone_right_brackets_do_not_panic_and_match_c() {
    for input in [
        b"<![CDATA[]]]".as_slice(),
        b"<![CDATA[]x]]".as_slice(),
        b"a<![CDATA[]]]".as_slice(),
    ] {
        assert!(!detect_xss(input), "{input:?}");
        assert_eq!(detect_sqli(input), None, "{input:?}");
    }
}
