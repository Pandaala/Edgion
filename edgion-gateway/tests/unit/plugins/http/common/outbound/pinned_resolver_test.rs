use super::*;

#[tokio::test]
async fn single_endpoint_pin_preserves_socket_and_rejects_unsafe_addresses() {
    for address in ["10.0.0.1:8443", "[2001:db8::1]:9443"] {
        let address: SocketAddr = address.parse().unwrap();
        let resolver = EndpointPinnedResolver::new(address, false);
        let actual: Vec<_> = resolver
            .resolve("unrelated.example".parse().unwrap())
            .await
            .unwrap()
            .collect();
        assert_eq!(actual, [address]);
    }
    let resolver = EndpointPinnedResolver::new("169.254.169.254:80".parse().unwrap(), false);
    assert!(resolver.resolve("backend.example".parse().unwrap()).await.is_err());
}

/// With no EndpointSlice data the resolver yields nothing; the caller then
/// fails the call rather than falling back to a real DNS lookup. (Renamed
/// from the old `rotates_endpoints_so_the_pool_still_round_robins`, which
/// asserted only this empty case and never exercised rotation at all —
/// see `seeded_endpoints_are_pinned_and_rotate_regardless_of_queried_name`
/// below for the actual pin + rotation coverage.)
#[test]
fn no_endpoints_yields_no_addresses_never_a_dns_fallback() {
    let r = AnchorPinnedResolver::new("ns/svc".to_string(), Some(8080));
    assert!(
        r.addrs_for_test().is_empty(),
        "no endpoints => no addresses, never a DNS fallback"
    );
}

/// The pin: a resolver seeded with a Service's ready endpoints returns
/// those endpoints regardless of the (attacker-influenced) queried name —
/// the queried `Name` is never consulted, by design. The round-robin: two
/// successive calls rotate the starting offset, spreading NEW connection
/// establishments across the pool.
#[tokio::test]
async fn seeded_endpoints_are_pinned_and_rotate_regardless_of_queried_name() {
    use crate::backends::discovery::endpoint_slice::get_roundrobin_store;
    use edgion_resources::constants::labels::k8s::SERVICE_NAME;
    use k8s_openapi::api::discovery::v1::{Endpoint, EndpointConditions, EndpointPort, EndpointSlice};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use std::collections::{BTreeMap, HashMap, HashSet};
    use std::net::SocketAddr;
    use std::str::FromStr;

    // Unique namespace/service so a leaked seed cannot collide with any
    // other test running in this process (the EndpointSlice store is a
    // process-global singleton with no reset/clear function).
    let ns = "pinned-resolver-rotation-test";
    let svc = "anchor-svc";
    let service_key = format!("{ns}/{svc}");

    let mut labels = BTreeMap::new();
    labels.insert(SERVICE_NAME.to_string(), svc.to_string());
    let slice = EndpointSlice {
        metadata: ObjectMeta {
            name: Some("anchor-svc-eps1".to_string()),
            namespace: Some(ns.to_string()),
            labels: Some(labels),
            ..Default::default()
        },
        address_type: "IPv4".to_string(),
        ports: Some(vec![EndpointPort {
            port: Some(8080),
            ..Default::default()
        }]),
        endpoints: vec![
            Endpoint {
                addresses: vec!["10.1.0.1".to_string()],
                conditions: Some(EndpointConditions {
                    ready: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Endpoint {
                addresses: vec!["10.1.0.2".to_string()],
                conditions: Some(EndpointConditions {
                    ready: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Endpoint {
                addresses: vec!["10.1.0.3".to_string()],
                conditions: Some(EndpointConditions {
                    ready: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Endpoint {
                addresses: vec!["169.254.169.254".to_string()],
                conditions: Some(EndpointConditions {
                    ready: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ],
    };
    let mut add = HashMap::new();
    add.insert(format!("{ns}/anchor-svc-eps1"), slice);
    get_roundrobin_store().update_data_only(add, HashMap::new(), &HashSet::new());

    let independent_resolver = AnchorPinnedResolver::new(service_key.clone(), Some(8080));
    let resolver = AnchorPinnedResolver::new(service_key, Some(8080));

    let expected: HashSet<SocketAddr> = ["10.1.0.1:8080", "10.1.0.2:8080", "10.1.0.3:8080"]
        .iter()
        .map(|s| s.parse().unwrap())
        .collect();

    // The pin: an arbitrary, unrelated queried name still resolves to the
    // anchor Service's endpoints — never to whatever that name would
    // actually resolve to in real DNS.
    let queried = Name::from_str("attacker-controlled-label.sandbox.internal").unwrap();
    let first: Vec<SocketAddr> = resolver.resolve(queried).await.unwrap().collect();
    assert_eq!(first.len(), 3, "all three seeded endpoints must be returned");
    assert_eq!(
        first.iter().copied().collect::<HashSet<_>>(),
        expected,
        "returned addresses must be exactly the anchor Service's endpoints, \
             regardless of the queried name"
    );

    // The round-robin: a second call (a different, still-unrelated queried
    // name — the pin ignores it either way) rotates the starting offset.
    let queried2 = Name::from_str("some-other-unrelated-name.example").unwrap();
    let second: Vec<SocketAddr> = resolver.resolve(queried2).await.unwrap().collect();
    assert_eq!(second.len(), 3);
    assert_ne!(
        first, second,
        "successive calls must rotate the returned order, not repeat it"
    );
    let mut rotated_first = first.clone();
    rotated_first.rotate_left(1);
    assert_eq!(second, rotated_first, "rotation must advance by one position per call");
    assert_eq!(resolver.cursor.load(Ordering::Relaxed), 2);
    let independent: Vec<SocketAddr> = independent_resolver
        .resolve(Name::from_str("independent-name.example").unwrap())
        .await
        .unwrap()
        .collect();
    assert_eq!(independent, first, "each resolver must own an independent cursor");
    assert_eq!(independent_resolver.cursor.load(Ordering::Relaxed), 1);
    assert_eq!(resolver.cursor.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn all_unsafe_endpoints_yield_no_addresses_without_advancing_rotation() {
    use crate::backends::discovery::endpoint_slice::get_roundrobin_store;
    use edgion_resources::constants::labels::k8s::SERVICE_NAME;
    use k8s_openapi::api::discovery::v1::{Endpoint, EndpointConditions, EndpointPort, EndpointSlice};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use std::collections::{BTreeMap, HashMap, HashSet};
    use std::str::FromStr;

    let ns = "pinned-resolver-unsafe-test";
    let svc = "unsafe-anchor";
    let service_key = format!("{ns}/{svc}");

    let mut labels = BTreeMap::new();
    labels.insert(SERVICE_NAME.to_string(), svc.to_string());
    let slice = EndpointSlice {
        metadata: ObjectMeta {
            name: Some("unsafe-anchor-eps1".to_string()),
            namespace: Some(ns.to_string()),
            labels: Some(labels),
            ..Default::default()
        },
        address_type: "IPv4".to_string(),
        ports: Some(vec![EndpointPort {
            port: Some(8080),
            ..Default::default()
        }]),
        endpoints: vec![
            Endpoint {
                addresses: vec!["169.254.1.1".to_string()],
                conditions: Some(EndpointConditions {
                    ready: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Endpoint {
                addresses: vec!["169.254.169.254".to_string()],
                conditions: Some(EndpointConditions {
                    ready: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ],
    };
    let mut add = HashMap::new();
    add.insert(format!("{ns}/unsafe-anchor-eps1"), slice);
    get_roundrobin_store().update_data_only(add, HashMap::new(), &HashSet::new());

    let resolver = AnchorPinnedResolver::new(service_key, Some(8080));
    let queried = Name::from_str("must-not-fall-back.example").unwrap();
    let actual: Vec<SocketAddr> = resolver.resolve(queried).await.unwrap().collect();

    assert!(actual.is_empty(), "an all-unsafe pool must expose no dial candidates");
    assert_eq!(
        resolver.cursor.load(Ordering::Relaxed),
        0,
        "an empty safe pool must be detected before cursor modulo and rotation"
    );
}
