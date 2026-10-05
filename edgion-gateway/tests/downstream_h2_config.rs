//! In-memory protocol integration: the production policy-to-HttpProxy seam and
//! real Pingora/h2 handshake frames. No Gateway/Controller topology is started.

use async_trait::async_trait;
use edgion_gateway::cli::config::ServerConfig;
use edgion_gateway::runtime::server::downstream_h2::DownstreamH2Policy;
use pingora_core::apps::HttpServerApp;
use pingora_core::protocols::http::v2::server::handshake;
use pingora_core::server::configuration::ServerConf;
use pingora_core::upstreams::peer::HttpPeer;
use pingora_proxy::{ProxyHttp, ProxyServiceBuilder, Session};
use std::collections::BTreeMap;
use std::future::poll_fn;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct HandshakeOnlyProxy;

#[async_trait]
impl ProxyHttp for HandshakeOnlyProxy {
    type CTX = ();

    fn new_ctx(&self) -> Self::CTX {}

    async fn upstream_peer(&self, _session: &mut Session, _ctx: &mut ()) -> pingora_core::Result<Box<HttpPeer>> {
        panic!("handshake-only test must never initiate an upstream request");
    }
}

async fn observe_initial_frames(config: ServerConfig) -> (BTreeMap<u16, u32>, u32) {
    let policy = DownstreamH2Policy::try_from(&config).unwrap();
    let mut service = ProxyServiceBuilder::new(&Arc::new(ServerConf::default()), HandshakeOnlyProxy).build();
    policy.apply_to(&mut service).unwrap();
    let options = service.app_logic().unwrap().h2_options();
    assert!(options.is_some());

    let (mut client, server) = tokio::io::duplex(65_536);
    let (initialized, ready) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let mut connection = handshake(Box::new(server), options).await.unwrap();
        let mut initialized = Some(initialized);
        let _ = poll_fn(|cx| {
            let result = connection.poll_accept(cx);
            if result.is_pending() {
                if let Some(initialized) = initialized.take() {
                    let _ = initialized.send(());
                }
            }
            result
        })
        .await;
    });
    client.write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n").await.unwrap();
    // First drive the server through an idle poll: h2 schedules and flushes
    // the initial connection WINDOW_UPDATE in that poll's Pending branch.
    // The 64 KiB duplex can hold all initial frames without a blocked flush;
    // this is not a general flush guarantee for arbitrary backpressured I/O.
    // A PING sent before this point can be acknowledged before that update.
    client.write_all(&[0, 0, 0, 4, 0, 0, 0, 0, 0]).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), ready)
        .await
        .unwrap()
        .unwrap();
    // The later PING acknowledgement now delimits the initial frame exchange.
    client
        .write_all(&[0, 0, 8, 6, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8])
        .await
        .unwrap();

    let observed = tokio::time::timeout(Duration::from_secs(3), async {
        let mut settings = BTreeMap::new();
        let mut connection_window = 65_535;
        loop {
            let mut header = [0u8; 9];
            client.read_exact(&mut header).await.unwrap();
            let length = (usize::from(header[0]) << 16) | (usize::from(header[1]) << 8) | usize::from(header[2]);
            assert!(length <= 16_384);
            let mut payload = vec![0; length];
            client.read_exact(&mut payload).await.unwrap();
            let stream = u32::from_be_bytes(header[5..9].try_into().unwrap()) & 0x7fff_ffff;
            if header[3] == 4 && header[4] == 0 {
                assert_eq!(stream, 0);
                assert_eq!(length % 6, 0);
                for entry in payload.chunks_exact(6) {
                    settings.insert(
                        u16::from_be_bytes(entry[..2].try_into().unwrap()),
                        u32::from_be_bytes(entry[2..].try_into().unwrap()),
                    );
                }
            } else if header[3] == 8 && stream == 0 {
                assert_eq!(length, 4);
                connection_window += u32::from_be_bytes(payload.try_into().unwrap()) & 0x7fff_ffff;
            } else if header[3] == 6 && header[4] == 1 {
                assert_eq!(payload, [1, 2, 3, 4, 5, 6, 7, 8]);
                break (settings, connection_window);
            }
        }
    })
    .await;
    task.abort();
    let _ = task.await;
    observed.expect("initial H2 settings exchange must complete")
}

#[tokio::test]
async fn omitted_configuration_preserves_pingora_bounded_defaults() {
    let (settings, connection_window) = observe_initial_frames(ServerConfig::default()).await;
    assert_eq!(settings.get(&3), Some(&100));
    assert_eq!(settings.get(&6), Some(&65_536));
    assert_eq!(settings.get(&4).copied().unwrap_or(65_535), 65_535);
    assert_eq!(connection_window, 65_535);
}

#[tokio::test]
async fn configured_windows_and_stream_limit_reach_the_peer_without_losing_header_protection() {
    let (settings, connection_window) = observe_initial_frames(ServerConfig {
        downstream_h2_stream_window_size: Some(262_144),
        downstream_h2_connection_window_size: Some(1_048_576),
        downstream_h2_max_concurrent_streams: Some(64),
        ..Default::default()
    })
    .await;
    assert_eq!(settings.get(&3), Some(&64));
    assert_eq!(settings.get(&4), Some(&262_144));
    assert_eq!(settings.get(&6), Some(&65_536));
    assert_eq!(connection_window, 1_048_576);
}
