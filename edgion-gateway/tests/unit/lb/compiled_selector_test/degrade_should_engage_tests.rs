use super::*;
use pingora_core::protocols::l4::socket::SocketAddr as PSocketAddr;
use pingora_load_balancing::Backend;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

fn backends(n: u8) -> Vec<Arc<LbBackend>> {
    (0..n)
        .map(|i| {
            Arc::new(LbBackend::new(Backend {
                addr: PSocketAddr::Inet(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, i)), 80)),
                weight: 1,
                ext: Default::default(),
            }))
        })
        .collect()
}

fn rr_selector(n: u8) -> CompiledSelector {
    CompiledSelector::RoundRobin(RrCore::build(Arc::from(backends(n))))
}

#[test]
fn engages_when_survivors_below_floor() {
    let sel = rr_selector(10);
    // floor for 50% of 10 = 5; only 4 healthy → degrade mode.
    // Cell<u32> allows interior mutation through an Fn closure.
    let healthy = std::cell::Cell::new(0u32);
    let engaged = sel.degrade_should_engage(5, |_| {
        let h = healthy.get() + 1;
        healthy.set(h);
        h <= 4
    });
    assert!(engaged);
}

#[test]
fn does_not_engage_when_survivors_reach_floor() {
    let sel = rr_selector(10);
    // 6 healthy >= floor 5 → no degrade; early-exit after the 5th survivor.
    let engaged = sel.degrade_should_engage(5, |_| true);
    assert!(!engaged);
}

#[test]
fn floor_zero_never_engages() {
    let sel = rr_selector(3);
    assert!(!sel.degrade_should_engage(0, |_| false));
}
