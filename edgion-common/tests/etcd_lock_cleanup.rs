//! Native SDK wire coverage for lock completion and cancellation (ARCH-079).
//! The local fixture implements only the unary RPCs needed by this test.
use bytes::Bytes;
use edgion_common::link_sys_core::providers::etcd::{EtcdBootstrapConfig, EtcdLinkClient};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

const UNLOCK: &str = "/v3lockpb.Lock/Unlock";
const REVOKE: &str = "/etcdserverpb.Lease/LeaseRevoke";
const KEY: &[u8] = b"/locks/token";

struct Call {
    path: String,
    body: Vec<u8>,
    reply: oneshot::Sender<bool>,
}

fn frame(payload: &[u8]) -> Vec<u8> {
    let mut body = vec![0];
    body.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    body.extend_from_slice(payload);
    body
}

async fn respond(
    request: http::Request<h2::RecvStream>,
    mut response: h2::server::SendResponse<Bytes>,
    calls: mpsc::UnboundedSender<Call>,
) {
    let path = request.uri().path().to_owned();
    let mut incoming = request.into_body();
    let mut body = Vec::new();
    while let Some(chunk) = incoming.data().await {
        let chunk = chunk.unwrap();
        assert!(body.len() + chunk.len() <= 1024);
        incoming.flow_control().release_capacity(chunk.len()).unwrap();
        body.extend_from_slice(&chunk);
    }
    let (success, payload) = match path.as_str() {
        "/etcdserverpb.Lease/LeaseGrant" => {
            assert_eq!(body, frame(&[0x08, 30]));
            (true, vec![0x10, 42, 0x18, 30])
        }
        "/v3lockpb.Lock/Lock" => {
            assert_eq!(body, frame(b"\x0a\x08testlock\x10\x2a"));
            let mut payload = vec![0x12, KEY.len() as u8];
            payload.extend_from_slice(KEY);
            (true, payload)
        }
        "/etcdserverpb.Maintenance/Status" => (true, vec![]),
        UNLOCK | REVOKE => {
            let (reply, result) = oneshot::channel();
            calls.send(Call { path, body, reply }).unwrap();
            let Ok(success) = result.await else { return };
            (success, vec![])
        }
        _ => panic!("unexpected RPC: {path}"),
    };
    let head = http::Response::builder()
        .header("content-type", "application/grpc")
        .body(())
        .unwrap();
    let Ok(mut stream) = response.send_response(head, false) else {
        return;
    };
    if success && stream.send_data(Bytes::from(frame(&payload)), false).is_err() {
        return;
    }
    let mut trailers = http::HeaderMap::new();
    trailers.insert("grpc-status", if success { "0" } else { "13" }.parse().unwrap());
    let _ = stream.send_trailers(trailers);
}

async fn next_call(calls: &mut mpsc::UnboundedReceiver<Call>, expected: &str) -> Call {
    let call = tokio::time::timeout(Duration::from_secs(3), calls.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.path, expected);
    let payload = if expected == UNLOCK {
        let mut payload = vec![0x0a, KEY.len() as u8];
        payload.extend_from_slice(KEY);
        payload
    } else {
        vec![0x08, 42]
    };
    assert_eq!(
        call.body,
        frame(&payload),
        "cleanup must retain the acquired key and lease"
    );
    call
}

#[derive(Clone, Copy, Debug)]
enum Completion {
    Success,
    Drop,
    ErrorAt(usize),
    CancelAt(usize),
}

async fn check_completion(completion: Completion) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (calls_tx, mut calls) = mpsc::unbounded_channel();
    let server = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let calls = calls_tx.clone();
            connections.spawn(async move {
                let mut connection = h2::server::handshake(socket).await.unwrap();
                let mut requests = tokio::task::JoinSet::new();
                while let Some(request) = connection.accept().await {
                    let Ok((request, response)) = request else { break };
                    requests.spawn(respond(request, response, calls.clone()));
                }
            });
        }
    });
    let client = EtcdLinkClient::from_bootstrap(
        "lock-completion-test",
        EtcdBootstrapConfig {
            endpoints: vec![format!("http://{address}")],
            auth: None,
            tls: None,
            timeout: None,
            keep_alive: None,
            namespace: Some("test".into()),
        },
    )
    .unwrap();
    client.init().await.unwrap();
    let guard = client
        .try_lock("lock", 30, Duration::from_secs(3))
        .await
        .unwrap()
        .unwrap();
    if matches!(completion, Completion::Drop) {
        drop(guard);
    } else {
        let task = tokio::spawn(guard.unlock());
        for (index, path) in [UNLOCK, REVOKE].into_iter().enumerate() {
            let call = next_call(&mut calls, path).await;
            if matches!(completion, Completion::CancelAt(step) if step == index) {
                task.abort();
                assert!(task.await.unwrap_err().is_cancelled());
                drop(call.reply);
                break;
            }
            let success = !matches!(completion, Completion::ErrorAt(step) if step == index);
            call.reply.send(success).unwrap();
            if !success {
                let error = task.await.unwrap().unwrap_err().to_string();
                assert!(error.starts_with(if index == 0 {
                    "Etcd UNLOCK:"
                } else {
                    "Etcd LEASE REVOKE (unlock):"
                }));
                break;
            }
            if index == 1 {
                task.await.unwrap().unwrap();
                break;
            }
        }
    }
    if !matches!(completion, Completion::Success) {
        next_call(&mut calls, UNLOCK).await.reply.send(true).unwrap();
        next_call(&mut calls, REVOKE).await.reply.send(true).unwrap();
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(100), calls.recv())
            .await
            .is_err(),
        "unexpected additional cleanup RPC for {completion:?}"
    );
    client.shutdown().await.unwrap();
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn successful_explicit_release_disarms_drop_cleanup() {
    check_completion(Completion::Success).await;
}

#[tokio::test]
async fn ordinary_drop_preserves_cleanup() {
    check_completion(Completion::Drop).await;
}

#[tokio::test]
async fn explicit_release_errors_preserve_cleanup() {
    for step in 0..2 {
        check_completion(Completion::ErrorAt(step)).await;
    }
}

#[tokio::test]
async fn explicit_release_cancellation_preserves_cleanup() {
    for step in 0..2 {
        check_completion(Completion::CancelAt(step)).await;
    }
}
