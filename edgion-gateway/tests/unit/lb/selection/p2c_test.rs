use super::*;
use pingora_core::protocols::l4::socket::SocketAddr as PSocketAddr;
use pingora_load_balancing::Backend;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

fn backend(i: u32) -> Arc<LbBackend> {
    let octets = i.to_be_bytes();
    let ip = IpAddr::V4(Ipv4Addr::new(10, octets[1], octets[2], octets[3]));
    Arc::new(LbBackend::new(Backend {
        addr: PSocketAddr::Inet(SocketAddr::new(ip, 80)),
        weight: 1,
        ext: Default::default(),
    }))
}

#[test]
fn empty_list_returns_none() {
    let result = select_p2c::<_, _>(&[], |_| 0u64, |_| true, 4);
    assert!(result.is_none());
}

#[test]
fn single_backend_returns_it_when_healthy() {
    let bs = vec![backend(1)];
    let picked = select_p2c(&bs, |_| 0u64, |_| true, 4);
    assert!(picked.is_some());
}

#[test]
fn single_backend_returns_none_when_unhealthy() {
    let bs = vec![backend(1)];
    let picked = select_p2c(&bs, |_| 0u64, |_| false, 4);
    assert!(picked.is_none());
}

#[test]
fn fallback_finds_lone_healthy_backend_among_unhealthy() {
    let bs: Vec<Arc<LbBackend>> = (0..16).map(backend).collect();
    let healthy_addr = bs[7].inner().addr.clone();
    let picked = select_p2c(&bs, |_| 0u64, |b| b.inner().addr == healthy_addr, 4);
    assert!(picked.is_some());
    assert_eq!(picked.unwrap().inner().addr, healthy_addr);
}

#[test]
fn skip_mapping_covers_every_non_excluded_index_exactly_once() {
    for n in [3usize, 4, 7, 32, 257] {
        for excluded in 0..n {
            let mapped: Vec<_> = (0..n - 1).map(|raw| skip_excluded_index(raw, excluded)).collect();
            let expected: Vec<_> = (0..n).filter(|&index| index != excluded).collect();

            assert!(mapped.iter().all(|&index| index < n));
            assert!(mapped.iter().all(|&index| index != excluded));
            assert_eq!(mapped, expected);
        }
    }
}

/// P2C distribution sanity check: with uniform metrics, no backend should
/// be picked dramatically more or less than its fair share. This is a
/// regression check for the "two-different-indices" sampling, not a
/// statistical guarantee.
#[test]
fn p2c_distribution_within_tolerance_under_uniform_metric() {
    let n = 32usize;
    let bs: Vec<Arc<LbBackend>> = (0..n as u32).map(backend).collect();
    let trials = 10_000usize;
    let mut counts: HashMap<std::net::SocketAddr, usize> = HashMap::new();
    for _ in 0..trials {
        let picked = select_p2c(&bs, |_| 0u64, |_| true, 4).expect("has healthy backend");
        if let Some(addr) = picked.inner().addr.as_inet() {
            *counts.entry(*addr).or_default() += 1;
        }
    }
    let expected = trials / n;
    let lower = expected * 60 / 100; // 60% of fair share
    let upper = expected * 160 / 100; // 160% of fair share
    assert_eq!(counts.len(), n, "every backend must receive at least one pick");
    for (addr, count) in &counts {
        assert!(
            *count >= lower && *count <= upper,
            "backend {} got {} picks, expected {}±~40% (range {}..={})",
            addr,
            count,
            expected,
            lower,
            upper
        );
    }
}
