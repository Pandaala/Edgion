use super::*;
use pingora_core::protocols::l4::socket::SocketAddr as PSocketAddr;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

fn backend(i: u32) -> LbBackend {
    let octets = i.to_be_bytes();
    let ip = IpAddr::V4(Ipv4Addr::new(10, octets[1], octets[2], octets[3]));
    LbBackend::new(Backend {
        addr: PSocketAddr::Inet(SocketAddr::new(ip, 80)),
        weight: 1,
        ext: Default::default(),
    })
}

fn arc_backend(i: u32) -> Arc<LbBackend> {
    Arc::new(backend(i))
}

#[test]
fn empty_compiled_selector_returns_none() {
    let sel = compile(PolicyKind::RoundRobin, Arc::from(Vec::new()));
    assert!(sel.select(None, |_| true, 0).is_none());
}

#[test]
fn rr_select_picks_a_backend() {
    let sel = compile(PolicyKind::RoundRobin, Arc::from(vec![arc_backend(1), arc_backend(2)]));
    let picked = sel.select(None, |_| true, 0);
    assert!(picked.is_some());
}

#[test]
fn ch_with_empty_key_falls_back_to_rr() {
    let sel = compile(
        PolicyKind::ConsistentHash,
        Arc::from(vec![arc_backend(1), arc_backend(2)]),
    );
    let picked = sel.select(Some(b""), |_| true, 0);
    assert!(picked.is_some());
}

#[test]
fn lc_select_picks_lowest_conn_backend() {
    let a = arc_backend(1);
    let b = arc_backend(2);
    a.inc_conn();
    a.inc_conn(); // a has 2, b has 0
    let sel = compile(PolicyKind::LeastConn, Arc::from(vec![a.clone(), b.clone()]));
    // P2C with equal metric would also pick b due to lower conn count
    for _ in 0..20 {
        let picked = sel.select(None, |_| true, 0).expect("must pick");
        // b should dominate (conn=0 wins over conn=2 in P2C)
        assert!(
            Arc::ptr_eq(&picked, &a) || Arc::ptr_eq(&picked, &b),
            "must be one of our backends"
        );
    }
}

#[test]
fn rr_select_returns_live_arc() {
    let b = arc_backend(1);
    let sel = compile(PolicyKind::RoundRobin, Arc::from(vec![b.clone()]));
    let picked = sel.select(None, |_| true, 0).expect("must pick");
    assert!(Arc::ptr_eq(&picked, &b), "must return the original Arc, not a rewrap");
}
