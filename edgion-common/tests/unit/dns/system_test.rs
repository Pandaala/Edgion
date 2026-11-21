use super::*;

#[test]
fn cancelled_queued_system_lookup_never_starts() {
    use std::sync::mpsc;

    struct Probe(mpsc::Sender<()>);
    impl ToSocketAddrs for Probe {
        type Iter = std::iter::Empty<std::net::SocketAddr>;

        fn to_socket_addrs(&self) -> std::io::Result<Self::Iter> {
            self.0.send(()).unwrap();
            Ok(std::iter::empty())
        }
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_time()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (release, wait) = mpsc::channel();
        let (started, occupied) = tokio::sync::oneshot::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            started.send(()).unwrap();
            let _ = wait.recv();
        });
        occupied.await.unwrap();

        let (probe, calls) = mpsc::channel();
        tokio::select! {
            biased;
            _ = lookup_system_sockets(Probe(probe)) => panic!("lookup ran while the only worker was occupied"),
            _ = std::future::ready(()) => {}
        }
        release.send(()).unwrap();
        blocker.await.unwrap();
        assert_eq!(
            calls.recv_timeout(Duration::from_secs(5)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
    });
}

#[tokio::test]
async fn resolves_localhost_to_loopback() {
    let addrs = resolve_system("localhost", crate::dns::DnsQueryPolicy::Native)
        .await
        .expect("localhost resolves");
    assert!(
        addrs.iter().any(|ip| ip.is_loopback()),
        "expected a loopback, got {addrs:?}"
    );
}

#[tokio::test]
async fn empty_host_errors() {
    assert!(resolve_system("", crate::dns::DnsQueryPolicy::Native).await.is_err());
}

#[tokio::test]
async fn resolves_ipv6_literal_directly() {
    let addrs = resolve_system("::1", crate::dns::DnsQueryPolicy::Native)
        .await
        .expect("ipv6 literal resolves");
    assert!(addrs.contains(&"::1".parse::<IpAddr>().unwrap()), "got {addrs:?}");
}

#[tokio::test]
async fn resolves_ipv4_literal_directly() {
    let addrs = resolve_system("127.0.0.1", crate::dns::DnsQueryPolicy::Native)
        .await
        .expect("ipv4 literal resolves");
    assert!(addrs.contains(&"127.0.0.1".parse::<IpAddr>().unwrap()), "got {addrs:?}");
}
