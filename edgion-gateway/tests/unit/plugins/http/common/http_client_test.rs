use super::*;

#[test]
fn test_hop_by_hop_detection() {
    // Exact lowercase match
    assert!(is_hop_by_hop("connection"));
    assert!(is_hop_by_hop("keep-alive"));
    assert!(is_hop_by_hop("transfer-encoding"));
    assert!(is_hop_by_hop("upgrade"));
    assert!(is_hop_by_hop("te"));
    assert!(is_hop_by_hop("trailer"));
    assert!(!is_hop_by_hop("trailers"));
    assert!(is_hop_by_hop("proxy-authenticate"));
    assert!(is_hop_by_hop("proxy-authorization"));

    // Case-insensitive
    assert!(is_hop_by_hop("Connection"));
    assert!(is_hop_by_hop("Keep-Alive"));
    assert!(is_hop_by_hop("TRANSFER-ENCODING"));
    assert!(is_hop_by_hop("Upgrade"));
    assert!(is_hop_by_hop("TE"));
    assert!(is_hop_by_hop("Proxy-Authenticate"));

    // Non hop-by-hop headers
    assert!(!is_hop_by_hop("content-type"));
    assert!(!is_hop_by_hop("authorization"));
    assert!(!is_hop_by_hop("host"));
    assert!(!is_hop_by_hop("x-forwarded-for"));
    assert!(!is_hop_by_hop("cookie"));
    assert!(!is_hop_by_hop("accept"));
}

#[test]
fn test_connection_listed_hop_by_hop_parses_tokens() {
    // Single token
    assert_eq!(
        connection_listed_hop_by_hop("X-Sensitive"),
        vec!["x-sensitive".to_string()]
    );
    // Multiple tokens, mixed case, surrounding whitespace
    assert_eq!(
        connection_listed_hop_by_hop("X-A, X-B , close"),
        vec!["x-a".to_string(), "x-b".to_string(), "close".to_string()],
    );
    // Empty / whitespace tokens are skipped
    assert_eq!(connection_listed_hop_by_hop(",  ,X-Only,"), vec!["x-only".to_string()],);
    // Duplicates are deduplicated case-insensitively
    assert_eq!(
        connection_listed_hop_by_hop("X-Dup, x-dup, X-DUP"),
        vec!["x-dup".to_string()],
    );
    // Empty input
    assert!(connection_listed_hop_by_hop("").is_empty());
    // ASCII folding leaves non-ASCII bytes intact, including in duplicates.
    assert_eq!(connection_listed_hop_by_hop("X-é, x-é, X-ñ"), vec!["x-é", "x-ñ"]);
}

#[test]
fn test_is_hop_by_hop_or_listed_combines_static_and_dynamic() {
    let listed = connection_listed_hop_by_hop("X-Sensitive, X-Token");
    // Static hop-by-hop matches regardless of dynamic set
    assert!(is_hop_by_hop_or_listed("Connection", &listed));
    assert!(is_hop_by_hop_or_listed("TRANSFER-ENCODING", &[]));
    // Dynamic-only names match when listed
    assert!(is_hop_by_hop_or_listed("x-sensitive", &listed));
    assert!(is_hop_by_hop_or_listed("X-Token", &listed));
    // Same names do not match when dynamic set is empty
    assert!(!is_hop_by_hop_or_listed("X-Sensitive", &[]));
    // Unrelated headers do not match
    assert!(!is_hop_by_hop_or_listed("Authorization", &listed));
    assert!(!is_hop_by_hop_or_listed("Cookie", &listed));
    // The dynamic list contract is lowercase; preserve exact comparison for
    // manually supplied noncanonical lists.
    assert!(!is_hop_by_hop_or_listed("X-Token", &["X-Token".into()]));
}

#[test]
fn hop_by_hop_static_and_empty_dynamic_checks_do_not_allocate() {
    let listed = vec![Cow::Owned("x-token".to_string())];
    let (results, stats) = crate::test_allocation_counter::count_allocation_stats(|| {
        (
            is_hop_by_hop_or_listed(std::hint::black_box("Connection"), &listed),
            is_hop_by_hop_or_listed(std::hint::black_box("X-Token"), &[]),
        )
    });
    assert_eq!(results, (true, false));
    assert_eq!(stats.calls, 0);
    assert_eq!(stats.requested_bytes, 0);
}

#[test]
fn repeated_connection_options_do_not_add_allocations() {
    let (unique, unique_stats) = crate::test_allocation_counter::count_allocation_stats(|| {
        connection_listed_hop_by_hop(std::hint::black_box("X-A, X-B"))
    });
    let (repeated, repeated_stats) = crate::test_allocation_counter::count_allocation_stats(|| {
        connection_listed_hop_by_hop(std::hint::black_box("X-A, x-a, X-B, x-b, X-A"))
    });
    assert_eq!(unique, vec!["x-a", "x-b"]);
    assert_eq!(repeated, unique);
    assert_eq!(repeated_stats, unique_stats);
}

#[test]
fn test_get_http_client_returns_same_instance() {
    let client1 = get_http_client();
    let client2 = get_http_client();
    // Both should point to the same instance
    assert!(std::ptr::eq(client1, client2));
}

#[test]
fn test_get_mirror_http_client_is_process_wide_and_dedicated() {
    let mirror1 = get_mirror_http_client();
    let mirror2 = get_mirror_http_client();

    assert!(std::ptr::eq(mirror1, mirror2));
    assert!(!std::ptr::eq(mirror1, get_http_client()));
}

#[test]
fn lowercase_dynamic_header_checks_do_not_allocate() {
    let listed = connection_listed_hop_by_hop("X-Token, X-é, X-É");
    for (name, expected) in [("x-token", true), ("x-missing", false), ("x-é", true), ("x-É", true)] {
        let (matched, stats) = crate::test_allocation_counter::count_allocation_stats(|| {
            is_hop_by_hop_or_listed(std::hint::black_box(name), &listed)
        });
        assert_eq!(matched, expected);
        assert_eq!(stats.calls, 0);
        assert_eq!(stats.requested_bytes, 0);
    }
    assert!(is_hop_by_hop_or_listed("X-É", &listed));
    assert!(!is_hop_by_hop_or_listed("x-É", &["x-é".into()]));
    assert!(!is_hop_by_hop_or_listed("x-token", &["X-Token".into()]));
}

#[test]
fn lowercase_connection_options_avoid_token_allocations() {
    let (lowercase, lower_stats) = crate::test_allocation_counter::count_allocation_stats(|| {
        connection_listed_hop_by_hop(std::hint::black_box("x-a, x-b"))
    });
    let (uppercase, upper_stats) = crate::test_allocation_counter::count_allocation_stats(|| {
        connection_listed_hop_by_hop(std::hint::black_box("X-A, X-B"))
    });
    assert_eq!(lowercase, uppercase);
    assert_eq!(lowercase, ["x-a", "x-b"]);
    assert_eq!(upper_stats.calls, lower_stats.calls + 2);
}
