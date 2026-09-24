use super::is_interim_informational;
use http::StatusCode;

/// Both response-stage entry points forward these untouched and let the
/// final header do the work. On the async side the guard is load-bearing for
/// more than tidiness: a plugin denial here would arm the response-stage
/// abort and kill an exchange whose real response has not arrived.
#[test]
fn interim_informational_covers_100_and_103_but_not_101() {
    assert!(is_interim_informational(StatusCode::CONTINUE));
    assert!(is_interim_informational(StatusCode::PROCESSING));
    assert!(is_interim_informational(StatusCode::from_u16(103).unwrap()));

    // 101 is terminal: the h1 client stops reading response headers after
    // it, so there is no final header to defer to.
    assert!(!is_interim_informational(StatusCode::SWITCHING_PROTOCOLS));
}

#[test]
fn final_statuses_are_never_interim() {
    for code in [200u16, 204, 304, 403, 500, 502] {
        assert!(!is_interim_informational(StatusCode::from_u16(code).unwrap()), "{code}");
    }
}
