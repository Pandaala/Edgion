use super::*;

#[test]
fn contract_is_strict_and_not_a_lossy_general_directive_parser() {
    for value in ["public, max-age=1", " MAX-AGE=10 , PUBLIC ", "public,max-age=999999"] {
        assert!(public_max_age(value.as_bytes()).is_some(), "{value}");
    }
    for value in [
        "public",
        "public,max-age=0",
        "public,max-age=1,max-age=2",
        "public,max-age=1,s-maxage=0",
        "public,max-age=1,no-store",
        "private,max-age=10",
        "public,max-age=\"10\"",
        "public,max-age=1.5",
        "public,max-age=18446744073709551616",
        "public,public",
        "public,\nmax-age=1",
    ] {
        assert!(public_max_age(value.as_bytes()).is_none(), "{value}");
    }
}

#[test]
fn upstream_age_delay_and_body_residency_do_not_restart_freshness() {
    let now = Instant::now();
    let wall = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000);
    let mut response = crate::response_cache::tests::response();
    response
        .insert_header("date", httpdate::fmt_http_date(wall - Duration::from_secs(15)))
        .unwrap();
    response.insert_header("age", "20").unwrap();
    let freshness = Freshness::from_headers(
        &response,
        now - Duration::from_secs(3),
        now,
        wall,
        Duration::from_secs(60),
    )
    .unwrap();
    assert_eq!(freshness.initial_age, Duration::from_secs(23));
    assert_eq!(
        freshness.remaining(now + Duration::from_secs(10)),
        Some(Duration::from_secs(27))
    );
    assert_eq!(freshness.remaining(now + Duration::from_secs(37)), None);
}

#[test]
fn malformed_repeated_and_private_headers_cannot_admit_a_writer() {
    for (name, value) in [
        ("date", "bad"),
        ("age", "bad"),
        ("set-cookie", "token=test"),
        ("vary", "*"),
        ("pragma", "no-cache"),
    ] {
        let mut response = crate::response_cache::tests::response();
        response.insert_header(name, value).unwrap();
        assert!(Freshness::from_headers(
            &response,
            Instant::now(),
            Instant::now(),
            SystemTime::now(),
            Duration::from_secs(60)
        )
        .is_none());
    }
    for name in ["date", "age", "cache-control"] {
        let mut response = crate::response_cache::tests::response();
        if name == "age" {
            response.append_header(name, "1").unwrap();
        }
        response.append_header(name, "1").unwrap();
        assert!(Freshness::from_headers(
            &response,
            Instant::now(),
            Instant::now(),
            SystemTime::now(),
            Duration::from_secs(60)
        )
        .is_none());
    }
}
