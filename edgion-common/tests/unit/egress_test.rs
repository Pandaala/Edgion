use super::*;
use std::str::FromStr;

impl EgressResolver {
    fn cached(&self, host: &str, now: Instant) -> Option<Vec<IpAddr>> {
        self.inner.cached(host, now)
    }

    fn store(&self, host: &str, addrs: Vec<IpAddr>, now: Instant) {
        self.inner.store(host, addrs, now)
    }

    fn cache_len(&self) -> usize {
        self.inner.cache.read().len()
    }
}

fn ip(value: &str) -> IpAddr {
    value.parse().unwrap()
}

fn resolver(block_private: bool) -> EgressResolver {
    EgressResolver::new(
        ResolverSpec::System,
        EgressPolicy { block_private },
        Duration::from_secs(5),
    )
}

#[test]
fn classify_all_segments_and_ipv4_mapped_ipv6() {
    let cases = [
        ("127.0.0.1", IpClass::Loopback),
        ("::1", IpClass::Loopback),
        ("169.254.169.254", IpClass::LinkLocal),
        ("febf::1", IpClass::LinkLocal),
        ("10.0.0.1", IpClass::Private),
        ("172.31.255.255", IpClass::Private),
        ("192.168.1.1", IpClass::Private),
        ("fd00::1", IpClass::UniqueLocal),
        ("0.0.0.0", IpClass::Unspecified),
        ("::", IpClass::Unspecified),
        ("8.8.8.8", IpClass::Global),
        ("2001:db8::1", IpClass::Global),
        ("::ffff:127.0.0.1", IpClass::Loopback),
        ("::ffff:169.254.169.254", IpClass::LinkLocal),
        ("::ffff:192.168.1.1", IpClass::Private),
        ("::ffff:0.0.0.0", IpClass::Unspecified),
        ("::ffff:8.8.8.8", IpClass::Global),
    ];
    for (value, expected) in cases {
        assert_eq!(classify_ip(ip(value)), expected, "classify_ip({value})");
    }
}

#[test]
fn filter_enforces_link_local_unspecified_and_private_policy() {
    let input = vec![
        ip("169.254.169.254"),
        ip("0.0.0.0"),
        ip("10.1.2.3"),
        ip("fd00::1"),
        ip("93.184.216.34"),
    ];
    assert_eq!(
        filter_resolved_addrs(input.clone(), EgressPolicy::default(), false),
        vec![ip("10.1.2.3"), ip("fd00::1"), ip("93.184.216.34")]
    );
    assert_eq!(
        filter_resolved_addrs(input, EgressPolicy { block_private: true }, false),
        vec![ip("93.184.216.34")]
    );
}

#[test]
fn cache_hit_negative_expiry_and_bound() {
    let resolver = resolver(true);
    let now = Instant::now();
    resolver.store("api.example.com", vec![ip("93.184.216.34")], now);
    resolver.store("evil.example.com", vec![], now);
    assert_eq!(resolver.cached("api.example.com", now), Some(vec![ip("93.184.216.34")]));
    assert_eq!(resolver.cached("evil.example.com", now), Some(vec![]));

    let short = EgressResolver::new(ResolverSpec::System, EgressPolicy::default(), Duration::from_millis(1));
    short.store("expired.example.com", vec![ip("93.184.216.34")], now);
    assert_eq!(
        short.cached("expired.example.com", now + Duration::from_millis(50)),
        None
    );

    for index in 0..(EGRESS_CACHE_MAX_ENTRIES * 3) {
        resolver.store(&format!("host-{index}.example.com"), vec![ip("93.184.216.34")], now);
    }
    assert!(resolver.cache_len() <= EGRESS_CACHE_MAX_ENTRIES);
}

#[test]
fn cache_freshness_is_strict_at_ttl_boundary_and_pruning_matches() {
    let resolver = EgressResolver::new(ResolverSpec::System, EgressPolicy::default(), Duration::from_millis(10));
    let inserted_at = Instant::now();
    let address = ip("93.184.216.34");
    resolver.store("old.example.com", vec![address], inserted_at);

    assert_eq!(
        resolver.cached("old.example.com", inserted_at + Duration::from_millis(9)),
        Some(vec![address])
    );
    let boundary = inserted_at + Duration::from_millis(10);
    assert_eq!(resolver.cached("old.example.com", boundary), None);

    resolver.store("new.example.com", vec![address], boundary);
    assert_eq!(resolver.cache_len(), 1);
    assert_eq!(resolver.cached("old.example.com", boundary), None);
    assert_eq!(resolver.cached("new.example.com", boundary), Some(vec![address]));
}

#[test]
fn zero_ttl_expires_immediately_in_lookup_and_pruning() {
    let resolver = EgressResolver::new(ResolverSpec::System, EgressPolicy::default(), Duration::ZERO);
    let now = Instant::now();
    let address = ip("93.184.216.34");
    resolver.store("old.example.com", vec![address], now);
    assert_eq!(resolver.cached("old.example.com", now), None);

    resolver.store("new.example.com", vec![address], now);
    assert_eq!(resolver.cache_len(), 1);
    assert_eq!(resolver.cached("old.example.com", now), None);
    assert_eq!(resolver.cached("new.example.com", now), None);
}

#[test]
fn cache_freshness_handles_reversed_samples_and_huge_ttl() {
    let address = ip("93.184.216.34");
    let earlier = Instant::now();
    let later = earlier + Duration::from_millis(10);
    let resolver = EgressResolver::new(ResolverSpec::System, EgressPolicy::default(), Duration::from_secs(1));
    resolver.store("future.example.com", vec![address], later);
    assert_eq!(resolver.cached("future.example.com", earlier), Some(vec![address]));
    resolver.store("current.example.com", vec![address], earlier);
    assert_eq!(resolver.cache_len(), 2);

    let huge = EgressResolver::new(ResolverSpec::System, EgressPolicy::default(), Duration::MAX);
    huge.store("huge.example.com", vec![address], earlier);
    assert_eq!(
        huge.cached("huge.example.com", earlier + Duration::from_secs(1)),
        Some(vec![address])
    );
}

#[tokio::test]
async fn reqwest_resolve_uses_cached_port_zero_and_rejects_empty() {
    let resolver = resolver(true);
    let now = Instant::now();
    resolver.store("api.example.com", vec![ip("93.184.216.34")], now);
    let name = Name::from_str("api.example.com").unwrap();
    let addrs: Vec<_> = Resolve::resolve(&resolver, name).await.unwrap().collect();
    assert_eq!(addrs, vec![SocketAddr::new(ip("93.184.216.34"), 0)]);

    resolver.store("evil.example.com", vec![], now);
    let name = Name::from_str("evil.example.com").unwrap();
    let result = Resolve::resolve(&resolver, name).await;
    let Err(error) = result else {
        panic!("all-blocked cache entry must surface as an error");
    };
    assert!(error.to_string().contains("evil.example.com"));
}

#[tokio::test]
async fn cache_hit_does_not_touch_network() {
    let resolver = EgressResolver::new(
        ResolverSpec::StandardDns {
            servers: vec!["203.0.113.1:53".to_string()],
        },
        EgressPolicy::default(),
        Duration::from_secs(5),
    );
    resolver.store("cached.example.com", vec![ip("93.184.216.34")], Instant::now());
    let name = Name::from_str("cached.example.com").unwrap();
    let addrs: Vec<_> = tokio::time::timeout(Duration::from_secs(2), Resolve::resolve(&resolver, name))
        .await
        .expect("cache hit must not touch DNS")
        .unwrap()
        .collect();
    assert_eq!(addrs, vec![SocketAddr::new(ip("93.184.216.34"), 0)]);
}
