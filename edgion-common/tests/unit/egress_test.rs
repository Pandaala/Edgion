use super::*;
use crate::dns::{DnsAnswer, StandardDnsServers};
use std::str::FromStr;

/// A private answer cache so tests never share state with the process-wide one.
fn private_cache() -> &'static DnsAnswerCache {
    Box::leak(Box::new(DnsAnswerCache::new(64, 1)))
}

/// Seed `cache` with a raw resolver answer for `host` without any network I/O.
async fn seed(cache: &DnsAnswerCache, spec: &ResolverSpec, host: &str, addrs: &[&str]) {
    let addrs: Vec<IpAddr> = addrs.iter().map(|value| ip(value)).collect();
    cache
        .lookup_with(
            spec,
            host,
            DnsQueryPolicy::Native,
            &DnsCachePolicy::new(Duration::from_secs(60)),
            |_, _, _| std::future::ready(Ok(DnsAnswer::without_ttl(addrs))),
        )
        .await
        .unwrap();
}

fn resolver_with(
    cache: &'static DnsAnswerCache,
    spec: ResolverSpec,
    block_private: bool,
    observer: Option<EgressBlockObserver>,
) -> EgressResolver {
    EgressResolver::with_cache(
        spec,
        EgressPolicy { block_private },
        Duration::from_secs(5),
        false,
        observer,
        cache,
    )
}

fn ip(value: &str) -> IpAddr {
    value.parse().unwrap()
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
        ("100.63.255.255", IpClass::Global),
        ("100.64.0.0", IpClass::Private),
        ("100.127.255.255", IpClass::Private),
        ("::ffff:100.64.0.1", IpClass::Private),
        ("100.128.0.0", IpClass::Global),
        ("0.0.0.1", IpClass::NonDialable),
        ("0.255.255.255", IpClass::NonDialable),
        ("1.0.0.0", IpClass::Global),
        ("223.255.255.255", IpClass::Global),
        ("224.0.0.0", IpClass::NonDialable),
        ("239.255.255.255", IpClass::NonDialable),
        ("240.0.0.0", IpClass::NonDialable),
        ("255.255.255.255", IpClass::NonDialable),
        ("::ffff:224.0.0.1", IpClass::NonDialable),
    ];
    for (value, expected) in cases {
        assert_eq!(classify_ip(ip(value)), expected, "classify_ip({value})");
    }
}

#[test]
fn transition_prefixes_require_global_unicast_ipv4() {
    for prefix in ["64:ff9b::", "2002::"] {
        for (v4, expected) in [
            ("0.0.0.0", IpClass::NonDialable),
            ("10.0.0.1", IpClass::NonDialable),
            ("100.64.0.1", IpClass::NonDialable),
            ("127.0.0.1", IpClass::NonDialable),
            ("169.254.1.1", IpClass::NonDialable),
            ("192.0.0.8", IpClass::NonDialable),
            ("192.0.0.9", IpClass::Global),
            ("192.0.0.10", IpClass::Global),
            ("192.0.2.1", IpClass::NonDialable),
            ("192.88.99.2", IpClass::NonDialable),
            ("198.19.255.255", IpClass::NonDialable),
            ("198.51.100.1", IpClass::NonDialable),
            ("203.0.113.1", IpClass::NonDialable),
            ("224.0.0.1", IpClass::NonDialable),
            ("255.255.255.255", IpClass::NonDialable),
            ("8.8.8.8", IpClass::Global),
        ] {
            let [a, b, c, d] = v4.parse::<Ipv4Addr>().unwrap().octets();
            let mut bytes = prefix.parse::<Ipv6Addr>().unwrap().octets();
            let offset = if prefix == "2002::" { 2 } else { 12 };
            bytes[offset..offset + 4].copy_from_slice(&[a, b, c, d]);
            assert_eq!(classify_ip(Ipv6Addr::from(bytes).into()), expected, "{prefix} {v4}");
        }
    }
    // Network-specific translation prefixes and neighboring ranges are native IPv6.
    for value in ["64:ff9b:1::a9fe:1", "64:ff9b:0:0:0:1:a9fe:1", "2003:7f00:1::"] {
        assert_eq!(classify_ip(ip(value)), IpClass::Global);
    }
}

fn blocked_matcher(cidrs: &[&str]) -> IpRadixMatcher {
    compile_blocked_cidrs(cidrs).unwrap().unwrap()
}

#[test]
fn blocked_cidr_list_only_adds_blocks_and_keeps_builtin_classes() {
    let matcher = blocked_matcher(&[
        "100.100.100.200/32",
        "fd00:ec2::254/128",
        "10.9.0.0/16",
        "8.8.8.0/24",
        "64:ff9b::909:900/120",
        "2002:909:900::/40",
        "0.0.0.0/8",
        "127.0.0.0/8",
        "169.254.0.0/16",
        "fe80::/10",
    ]);
    let cases = [
        ("100.100.100.200", IpClass::BlockedCidr),
        ("::ffff:100.100.100.200", IpClass::BlockedCidr),
        ("100.100.100.201", IpClass::Private),
        ("fd00:ec2::254", IpClass::BlockedCidr),
        ("fd00:ec2::253", IpClass::UniqueLocal),
        ("10.9.1.1", IpClass::BlockedCidr),
        ("10.8.1.1", IpClass::Private),
        ("8.8.8.8", IpClass::BlockedCidr),
        ("64:ff9b::808:808", IpClass::BlockedCidr),
        ("2002:808:808:a:b:c:d:e", IpClass::BlockedCidr),
        ("64:ff9b::909:909", IpClass::BlockedCidr),
        ("2002:909:909::1", IpClass::BlockedCidr),
        ("64:ff9b::6400:1", IpClass::Global),
        ("64:ff9b::6464:64c8", IpClass::BlockedCidr),
        ("0.0.0.0", IpClass::BlockedCidr),
        // Loopback and link-local keep their built-in classes (and so their
        // reasons and exceptions) even when the list also covers them.
        ("127.0.0.1", IpClass::Loopback),
        ("::ffff:127.0.0.1", IpClass::Loopback),
        ("169.254.169.254", IpClass::LinkLocal),
        ("fe80::1", IpClass::LinkLocal),
    ];
    for (value, expected) in cases {
        assert_eq!(classify_ip_with(ip(value), Some(&matcher)), expected, "{value}");
    }
    // Without a list every address keeps its built-in class.
    assert_eq!(classify_ip_with(ip("100.100.100.200"), None), IpClass::Private);
    assert_eq!(classify_ip_with(ip("fd00:ec2::254"), None), IpClass::UniqueLocal);
}

#[test]
fn relaxed_resolver_keeps_local_and_global_dials_and_rejects_non_dialable() {
    let allowed = [
        "127.0.0.1",
        "169.254.1.1",
        "0.0.0.0",
        "100.64.0.1",
        "64:ff9b::808:808",
        "2002:808:808::1",
    ]
    .map(ip);
    let rejected = ["64:ff9b::a9fe:1", "2002:7f00:1::", "239.1.1.1"].map(ip);
    let result = filter_resolved_addrs_with_observer(
        allowed.into_iter().chain(rejected).collect(),
        ResolverAddressPolicy::AllowPrivateAndLocal,
        false,
        None,
    );
    assert_eq!(result, allowed);
    assert_eq!(EgressBlockReason::NonDialable.as_str(), "non_dialable");
}

#[test]
fn uncached_system_answers_keep_scoped_sockets_and_filter_once_per_distinct_socket() {
    thread_local! {
        static DROPS: std::cell::RefCell<Vec<EgressBlockReason>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    fn observe(reason: EgressBlockReason) {
        DROPS.with(|drops| drops.borrow_mut().push(reason));
    }
    let resolver = UncachedLocalSystemResolver::new(Some(observe));
    let scoped = |scope| SocketAddr::V6(std::net::SocketAddrV6::new("fe80::1".parse().unwrap(), 0, 0, scope));
    let rejected = SocketAddr::new(ip("224.0.0.1"), 0);
    let global = SocketAddr::new(ip("8.8.8.8"), 0);
    let sockets = [scoped(5), rejected, scoped(3), rejected, scoped(5), global];
    assert_eq!(
        resolver.allowed_sockets(sockets.into_iter()),
        [scoped(5), scoped(3), global]
    );
    DROPS.with(|drops| assert_eq!(*drops.borrow(), [EgressBlockReason::NonDialable]));

    let distinct = (1..).map(scoped);
    let allowed = resolver.allowed_sockets(distinct);
    assert_eq!(allowed.len(), crate::dns::MAX_ANSWER_ADDRS);
    assert_eq!(allowed.last(), Some(&scoped(crate::dns::MAX_ANSWER_ADDRS as u32)));
}

#[test]
fn blocked_cidr_is_rejected_under_every_private_and_loopback_policy() {
    for block_private in [false, true] {
        for allow_loopback in [false, true] {
            assert_eq!(
                egress_block_reason(IpClass::BlockedCidr, EgressPolicy { block_private }, allow_loopback),
                Some(EgressBlockReason::BlockedCidr)
            );
        }
    }
    assert_eq!(EgressBlockReason::BlockedCidr.as_str(), "blocked_cidr");
}

#[test]
fn compile_blocked_cidrs_accepts_empty_and_rejects_invalid_entries() {
    assert!(compile_blocked_cidrs::<&str>(&[]).unwrap().is_none());
    for invalid in [
        "100.100.100.200",
        "100.100.100.200/33",
        "fd00:ec2::254/129",
        "not-an-ip/8",
        "",
    ] {
        let error = compile_blocked_cidrs(&[invalid]).unwrap_err();
        assert!(error.contains("invalid blocked CIDR"), "{invalid:?}: {error}");
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
fn observed_filter_preserves_address_order_and_drop_reasons() {
    thread_local! {
        static DROPS: std::cell::RefCell<Vec<EgressBlockReason>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    fn observe(reason: EgressBlockReason) {
        DROPS.with(|drops| drops.borrow_mut().push(reason));
    }
    let cases = [
        ("127.0.0.1", None),
        ("::1", None),
        ("::ffff:127.0.0.1", None),
        ("169.254.169.254", Some(EgressBlockReason::LinkLocal)),
        ("fe80::1", Some(EgressBlockReason::LinkLocal)),
        ("::ffff:169.254.169.254", Some(EgressBlockReason::LinkLocal)),
        ("0.0.0.0", Some(EgressBlockReason::Unspecified)),
        ("::", Some(EgressBlockReason::Unspecified)),
        ("::ffff:0.0.0.0", Some(EgressBlockReason::Unspecified)),
        ("10.1.2.3", Some(EgressBlockReason::Private)),
        ("100.80.0.1", Some(EgressBlockReason::Private)),
        ("64:ff9b::6440:1", Some(EgressBlockReason::NonDialable)),
        ("2002:7f00:1::1", Some(EgressBlockReason::NonDialable)),
        ("240.0.0.1", Some(EgressBlockReason::NonDialable)),
        ("fd00::1", Some(EgressBlockReason::Private)),
        ("::ffff:192.168.1.1", Some(EgressBlockReason::Private)),
        ("93.184.216.34", None),
        ("2001:db8::1", None),
        ("169.254.169.254", Some(EgressBlockReason::LinkLocal)),
        ("93.184.216.34", None),
    ];
    for block_private in [false, true] {
        for allow_loopback in [false, true] {
            let policy = EgressPolicy { block_private };
            let mut allowed = Vec::new();
            let mut reasons = Vec::new();
            for (address, reason) in cases {
                let reason = reason.filter(|reason| *reason != EgressBlockReason::Private || block_private);
                DROPS.with(|drops| drops.borrow_mut().clear());
                assert_eq!(
                    is_ip_allowed_with_observer(ip(address), policy, allow_loopback, Some(observe)),
                    reason.is_none(),
                    "{address}"
                );
                DROPS.with(|drops| assert_eq!(*drops.borrow(), reason.into_iter().collect::<Vec<_>>()));
                if let Some(reason) = reason {
                    reasons.push(reason);
                } else {
                    allowed.push(ip(address));
                }
            }
            DROPS.with(|drops| drops.borrow_mut().clear());
            let actual = filter_resolved_addrs_with_observer(
                cases.iter().map(|(address, _)| ip(address)).collect(),
                policy,
                allow_loopback,
                Some(observe),
            );
            assert_eq!(actual, allowed);
            DROPS.with(|drops| assert_eq!(*drops.borrow(), reasons));
        }
    }
}

#[tokio::test]
async fn cached_raw_answer_is_classified_by_each_resolver_on_every_use() {
    thread_local! {
        static DROPS: std::cell::RefCell<Vec<EgressBlockReason>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    fn observe(reason: EgressBlockReason) {
        DROPS.with(|drops| drops.borrow_mut().push(reason));
    }
    let cache = private_cache();
    seed(
        cache,
        &ResolverSpec::System,
        "mixed.example.com",
        &["93.184.216.34", "10.0.0.1"],
    )
    .await;
    let strict = resolver_with(cache, ResolverSpec::System, true, Some(observe));
    let lenient = resolver_with(cache, ResolverSpec::System, false, None);
    for _ in 0..2 {
        assert_eq!(
            strict.resolve_socket_addrs("mixed.example.com", 443).await.unwrap(),
            vec![SocketAddr::new(ip("93.184.216.34"), 443)]
        );
    }
    assert_eq!(
        lenient.resolve_socket_addrs("mixed.example.com", 443).await.unwrap(),
        vec![
            SocketAddr::new(ip("93.184.216.34"), 443),
            SocketAddr::new(ip("10.0.0.1"), 443)
        ],
        "the cache stores the raw answer, not one consumer's filtered view"
    );
    DROPS.with(|drops| assert_eq!(*drops.borrow(), vec![EgressBlockReason::Private; 2], "observed per use"));
    assert_eq!(cache.stats().entries, 1);
}

#[tokio::test]
async fn reqwest_resolve_uses_cached_port_zero_and_rejects_empty() {
    let cache = private_cache();
    let resolver = resolver_with(cache, ResolverSpec::System, true, None);
    seed(cache, &ResolverSpec::System, "api.example.com", &["93.184.216.34"]).await;
    let name = Name::from_str("api.example.com").unwrap();
    let addrs: Vec<_> = Resolve::resolve(&resolver, name).await.unwrap().collect();
    assert_eq!(addrs, vec![SocketAddr::new(ip("93.184.216.34"), 0)]);

    seed(cache, &ResolverSpec::System, "evil.example.com", &["169.254.169.254"]).await;
    let name = Name::from_str("evil.example.com").unwrap();
    let result = Resolve::resolve(&resolver, name).await;
    let Err(error) = result else {
        panic!("an answer whose every address is blocked must surface as an error");
    };
    assert_eq!(
        error.to_string(),
        "egress policy blocked all resolved addresses for host 'evil.example.com'"
    );
    assert_eq!(
        resolver
            .resolve_socket_addrs("evil.example.com", 443)
            .await
            .unwrap_err(),
        "egress policy blocked all resolved addresses"
    );
}

#[tokio::test]
async fn cache_hit_does_not_touch_network() {
    let cache = private_cache();
    let spec = ResolverSpec::StandardDns(StandardDnsServers::new(vec!["203.0.113.1:53".to_string()]));
    seed(cache, &spec, "cached.example.com", &["93.184.216.34"]).await;
    let resolver = resolver_with(cache, spec, false, None);
    let name = Name::from_str("cached.example.com").unwrap();
    let addrs: Vec<_> = tokio::time::timeout(Duration::from_secs(2), Resolve::resolve(&resolver, name))
        .await
        .expect("cache hit must not touch DNS")
        .unwrap()
        .collect();
    assert_eq!(addrs, vec![SocketAddr::new(ip("93.184.216.34"), 0)]);
}

#[tokio::test]
async fn zero_ttl_skips_retention_but_preserves_policy_and_observation() {
    thread_local! {
        static DROPS: std::cell::RefCell<Vec<EgressBlockReason>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    fn observe(reason: EgressBlockReason) {
        DROPS.with(|drops| drops.borrow_mut().push(reason));
    }
    // Numeric System inputs are parsed locally without an OS lookup.
    for ttl in [Duration::ZERO, Duration::MAX] {
        let cache = private_cache();
        let resolver = EgressResolver::with_cache(
            ResolverSpec::System,
            EgressPolicy { block_private: true },
            ttl,
            false,
            Some(observe),
            cache,
        );
        DROPS.with(|drops| drops.borrow_mut().clear());
        for _ in 0..2 {
            assert_eq!(
                resolver.resolve_socket_addrs("93.184.216.34", 443).await.unwrap(),
                vec![SocketAddr::new(ip("93.184.216.34"), 443)]
            );
            assert_eq!(
                resolver.resolve_socket_addrs("169.254.169.254", 443).await.unwrap_err(),
                "egress policy blocked all resolved addresses"
            );
        }
        // Classification and observation run on every use; IP literals never
        // enter the answer cache.
        DROPS.with(|drops| {
            assert_eq!(*drops.borrow(), vec![EgressBlockReason::LinkLocal; 2]);
        });
        assert_eq!(cache.stats().entries, 0);
    }
}

#[tokio::test]
async fn hostname_answers_are_retained_unless_the_ttl_is_zero() {
    // `localhost` resolves through the OS resolver without network access.
    for (ttl, retained) in [(Duration::ZERO, 0), (Duration::from_secs(5), 1), (Duration::MAX, 1)] {
        let cache = private_cache();
        let resolver = EgressResolver::with_cache(
            ResolverSpec::System,
            EgressPolicy { block_private: false },
            ttl,
            true,
            None,
            cache,
        );
        resolver.resolve_socket_addrs("localhost", 443).await.unwrap();
        assert_eq!(cache.stats().entries, retained, "ttl {ttl:?}");
    }
}
