use super::*;
#[test]
fn discovery_deduplicates_normalized_client_urls() {
    let urls = normalize_client_urls(
        ["https://ETCD:2379/", "https://etcd:2379", "https://other:2379"],
        true,
        &[],
    )
    .unwrap();
    assert_eq!(urls.len(), 2);
    assert_eq!(urls[0], "https://etcd:2379".parse::<http::Uri>().unwrap());
}
#[test]
fn discovery_rejects_complete_invalid_or_empty_candidate_set() {
    assert!(normalize_client_urls([], false, &[]).is_err());
    assert!(normalize_client_urls(["https://good:2379", "http://bad:2379"], true, &[]).is_err());
    assert!(normalize_client_urls(["http://good:2379", "http://user:password@bad:2379"], false, &[]).is_err());
    let urls: Vec<_> = (0..65).map(|index| format!("http://member-{index}:2379")).collect();
    assert!(normalize_client_urls(urls.iter().map(String::as_str), false, &[]).is_err());
    assert!(normalize_client_urls(urls[..64].iter().map(String::as_str), false, &[]).is_ok());
}

#[test]
fn discovery_accepts_tls_upgrade_but_rejects_mixed_or_downgraded_sets() {
    assert!(normalize_client_urls(["https://secure:2379"], false, &[]).is_ok());
    assert!(normalize_client_urls(["https://secure:2379", "http://plain:2379"], false, &[]).is_err());
    assert!(normalize_client_urls(["http://plain:2379"], true, &[]).is_err());
    assert!(normalize_client_urls(["https://secure:2379"], true, &[]).is_ok());
}

#[test]
fn current_effective_https_set_prevents_downgrade_after_http_bootstrap() {
    let original = vec!["http://seed:2379".parse::<http::Uri>().unwrap()];
    let upgraded = normalize_client_urls(["https://secure:2379"], false, &original).unwrap();
    assert!(normalize_client_urls(["http://seed:2379"], false, &upgraded).is_err());
    assert!(normalize_client_urls(["https://replacement:2379"], false, &upgraded).is_ok());
    // Failed candidate validation leaves the caller's last-good snapshot intact.
    assert!(normalize_client_urls(["https://secure:2379", "http://mixed:2379"], false, &original).is_err());
    assert!(normalize_client_urls(["http://replacement:2379"], false, &original).is_ok());
}
