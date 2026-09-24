use super::*;
use crate::lb::backend::LbBackend;
use crate::lb::runtime_state;
use pingora_core::protocols::l4::socket::SocketAddr as PSocketAddr;
use pingora_load_balancing::Backend;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

fn make_backend(i: u8) -> Backend {
    let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, i));
    Backend {
        addr: PSocketAddr::Inet(SocketAddr::new(ip, 8080)),
        weight: 1,
        ext: Default::default(),
    }
}

#[test]
fn cleaner_default_constructs_without_handle() {
    let cleaner = BackendCleaner::default();
    // Default and new() must be equivalent — both must construct without
    // panicking and without spawning a task. We can only assert behavior
    // (no observable side effects, drop is clean).
    drop(cleaner);
}

#[test]
fn cleaner_new_constructs_without_handle() {
    let cleaner = BackendCleaner::new();
    drop(cleaner);
}

#[tokio::test]
async fn cleaner_start_then_stop_is_clean() {
    let mut cleaner = BackendCleaner::new();
    cleaner.start();
    // start must be idempotent: a second call must not panic and must
    // not replace the running handle (covered by the warn-once guard).
    cleaner.start();
    cleaner.stop().await;
}

#[tokio::test]
async fn cleaner_can_restart_after_stop() {
    // After stop() the cancel token is reset, so a follow-up start() must
    // succeed. This guards against accidentally leaving the cancellation
    // token in a permanently-cancelled state.
    let mut cleaner = BackendCleaner::new();
    cleaner.start();
    cleaner.stop().await;
    cleaner.start();
    cleaner.stop().await;
}

#[test]
fn draining_registry_admits_and_removes_backend() {
    // Sanity check for the runtime_state ledger that BackendCleaner polls.
    // A backend with conn=0 is removable; before removal it appears in the
    // draining set under its `service\0addr` key.
    let svc = "test-svc-leastconn-admit";
    let backend = Backend {
        addr: PSocketAddr::Inet(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 251)), 8080)),
        weight: 1,
        ext: Default::default(),
    };
    let addr = backend.addr.clone();
    let arc = Arc::new(LbBackend::new(backend));

    runtime_state::begin_draining(svc, &addr, arc.clone());
    let listed = runtime_state::all_draining_backends();
    assert!(
        listed.iter().any(|(k, _)| k.starts_with(&format!("{}\0", svc))),
        "draining ledger must contain the registered backend"
    );

    runtime_state::remove_backend_exact(svc, &addr, &arc);
    let listed = runtime_state::all_draining_backends();
    assert!(
        !listed.iter().any(|(k, _)| k.starts_with(&format!("{}\0", svc))),
        "remove_backend must drop the entry"
    );
    // Touch the backend so the binding is non-trivial.
    let _ = make_backend(1);
}

#[test]
fn remove_service_clears_all_draining_backends_for_service() {
    let svc = "test-svc-leastconn-bulk";
    for i in 0..3u8 {
        let b = Backend {
            addr: PSocketAddr::Inet(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, i)), 9000)),
            weight: 1,
            ext: Default::default(),
        };
        let addr = b.addr.clone();
        runtime_state::begin_draining(svc, &addr, Arc::new(LbBackend::new(b)));
    }
    let before = runtime_state::all_draining_backends();
    let count_before = before
        .iter()
        .filter(|(k, _)| k.starts_with(&format!("{}\0", svc)))
        .count();
    assert_eq!(count_before, 3);

    runtime_state::remove_service(svc);
    let after = runtime_state::all_draining_backends();
    let count_after = after
        .iter()
        .filter(|(k, _)| k.starts_with(&format!("{}\0", svc)))
        .count();
    assert_eq!(count_after, 0, "remove_service must drain all backends for the service");
}
