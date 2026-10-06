//! Frame-level backend proofs. These do not replace real Gateway scenarios.

use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const DEADLINE: Duration = Duration::from_secs(5);

struct Frame {
    kind: u8,
    flags: u8,
    stream: u32,
    payload: Vec<u8>,
}

fn frame(out: &mut Vec<u8>, kind: u8, flags: u8, stream: u32, payload: &[u8]) {
    let len = payload.len();
    out.extend_from_slice(&[(len >> 16) as u8, (len >> 8) as u8, len as u8, kind, flags]);
    out.extend_from_slice(&stream.to_be_bytes());
    out.extend_from_slice(payload);
}

fn header(out: &mut Vec<u8>, name: &str, value: &str) {
    assert!(name.len() < 127 && value.len() < 127);
    out.extend_from_slice(&[0, name.len() as u8]);
    out.extend_from_slice(name.as_bytes());
    out.push(value.len() as u8);
    out.extend_from_slice(value.as_bytes());
}

async fn backend() -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let conn = recorder::claim_conn(LISTENER_H2);
        let _ = handle_conn(socket, conn).await;
        recorder::mark_closed(LISTENER_H2, conn);
    });
    (address, task)
}

async fn connect(mode: &str, trace: &str, open: bool) -> (TcpStream, tokio::task::JoinHandle<()>) {
    let (address, task) = backend().await;
    let mut socket = TcpStream::connect(address).await.unwrap();
    let mut block = Vec::new();
    for (name, value) in [
        (":method", "POST"),
        (":scheme", "http"),
        (":authority", "lifecycle.proof"),
        (":path", "/proof"),
        ("x-up-lifecycle", mode),
        ("x-trace-id", trace),
    ] {
        header(&mut block, name, value);
    }
    if mode == "held" {
        header(&mut block, "x-up-release-trace", "lifecycle-proof-release");
    }
    let mut bytes = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n".to_vec();
    frame(&mut bytes, 4, 0, 0, &[]);
    frame(&mut bytes, 1, if open { 4 } else { 5 }, 1, &block);
    socket.write_all(&bytes).await.unwrap();
    (socket, task)
}

async fn read(socket: &mut TcpStream) -> Frame {
    let mut bytes = [0u8; 9];
    socket.read_exact(&mut bytes).await.unwrap();
    let len = ((bytes[0] as usize) << 16) | ((bytes[1] as usize) << 8) | bytes[2] as usize;
    assert!(len <= 65_535, "unexpected unbounded proof frame");
    let mut payload = vec![0; len];
    socket.read_exact(&mut payload).await.unwrap();
    Frame {
        kind: bytes[3],
        flags: bytes[4],
        stream: u32::from_be_bytes(bytes[5..9].try_into().unwrap()),
        payload,
    }
}

async fn acknowledge(socket: &mut TcpStream, received: &Frame) {
    if matches!(received.kind, 4 | 6) && received.flags & 1 == 0 {
        let mut bytes = Vec::new();
        frame(
            &mut bytes,
            received.kind,
            1,
            0,
            if received.kind == 6 { &received.payload } else { &[] },
        );
        socket.write_all(&bytes).await.unwrap();
    }
}

#[tokio::test]
async fn stall_exhausts_default_credit_before_response_and_retains_upload_until_cancel() {
    tokio::time::timeout(DEADLINE, async {
        let trace = "lifecycle-proof-stall";
        let (mut socket, task) = connect("stall", trace, true).await;
        let mut remaining = lifecycle::UPLOAD_WINDOW;
        let mut bytes = Vec::new();
        while remaining > 0 {
            let count = remaining.min(16_384);
            frame(&mut bytes, 0, 0, 1, &vec![b'u'; count]);
            remaining -= count;
        }
        socket.write_all(&bytes).await.unwrap();
        let mut response = Vec::new();
        let mut settings_seen = false;
        loop {
            let received = read(&mut socket).await;
            acknowledge(&mut socket, &received).await;
            if received.kind == 4 && received.flags & 1 == 0 {
                settings_seen = true;
                let window = received
                    .payload
                    .chunks_exact(6)
                    .find_map(|setting| {
                        (setting[..2] == [0, 4]).then(|| u32::from_be_bytes(setting[2..6].try_into().unwrap()))
                    })
                    .unwrap_or(65_535);
                assert_eq!(window, lifecycle::UPLOAD_WINDOW as u32);
            }
            assert_ne!((received.kind, received.stream), (8, 1), "stall returned stream credit");
            assert_ne!(received.kind, 3, "backend reset before completing response");
            if received.kind == 0 && received.stream == 1 {
                response.extend_from_slice(&received.payload);
                if received.flags & 1 != 0 {
                    break;
                }
            }
        }
        assert!(settings_seen);
        assert_eq!(response.len(), 64);
        let events = recorder::query(trace);
        let data: Vec<_> = events["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["kind"] == recorder::KIND_REQ_DATA)
            .collect();
        assert_eq!(
            data.iter().map(|event| event["len"].as_u64().unwrap()).sum::<u64>(),
            65_535
        );
        assert!(data.iter().all(|event| event["end_stream"] == false));
        let mut reset = Vec::new();
        frame(&mut reset, 3, 0, 1, &8u32.to_be_bytes());
        socket.write_all(&reset).await.unwrap();
        loop {
            if recorder::query(trace)["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|event| event["kind"] == recorder::KIND_PEER_RESET && event["code"] == "CANCEL")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        drop(socket);
        task.abort();
        let _ = task.await;
    })
    .await
    .expect("stall lifecycle proof exceeded deadline");
}

#[tokio::test]
async fn complete_goaway_sequences_precede_full_responses_even_for_excluded_streams() {
    // This raw peer deliberately ignores GOAWAY. Sending the excluded reply
    // makes the Gateway's negative assertion depend on its lifecycle policy,
    // rather than on the backend silently withholding all response bytes.
    for (mode, expected) in [
        ("goaway-descending", vec![0x7fff_ffffu32, 1]),
        ("goaway-excluding", vec![0x7fff_ffffu32, 0]),
        ("goaway-raising", vec![0u32, 1]),
    ] {
        tokio::time::timeout(DEADLINE, async {
            let (mut socket, task) = connect(mode, &format!("lifecycle-proof-{mode}"), false).await;
            let mut ceilings = Vec::new();
            let mut response = Vec::new();
            loop {
                let received = read(&mut socket).await;
                acknowledge(&mut socket, &received).await;
                if received.kind == 7 {
                    assert_eq!(received.stream, 0);
                    assert_eq!(received.payload.len(), 8);
                    assert_eq!(&received.payload[4..], &[0; 4]);
                    ceilings.push(u32::from_be_bytes(received.payload[..4].try_into().unwrap()));
                }
                if received.kind == 1 && received.stream == 1 {
                    assert_eq!(ceilings, expected, "response overtook GOAWAY injection");
                }
                if received.kind == 0 && received.stream == 1 {
                    response.extend_from_slice(&received.payload);
                    if received.flags & 1 != 0 {
                        break;
                    }
                }
            }
            assert_eq!(ceilings, expected);
            assert_eq!(response.len(), 64);
            drop(socket);
            task.abort();
            let _ = task.await;
        })
        .await
        .expect("GOAWAY sequence proof exceeded deadline");
    }
}

#[tokio::test]
async fn malformed_nonzero_and_truncated_goaway_are_exact_wire_faults() {
    for mode in ["goaway-malformed", "goaway-nonzero", "goaway-truncated"] {
        tokio::time::timeout(DEADLINE, async {
            let (mut socket, task) = connect(mode, &format!("lifecycle-proof-{mode}"), false).await;
            loop {
                let mut head = [0u8; 9];
                socket.read_exact(&mut head).await.unwrap();
                let len = ((head[0] as usize) << 16) | ((head[1] as usize) << 8) | head[2] as usize;
                assert!(len <= 65_535);
                let mut payload = vec![0; len];
                if head[3] == 7 {
                    if mode == "goaway-truncated" {
                        let mut received = Vec::new();
                        socket.read_to_end(&mut received).await.unwrap();
                        assert_eq!(len, 8);
                        assert_eq!(received, [0, 0, 0, 1, 0, 0]);
                    } else {
                        socket.read_exact(&mut payload).await.unwrap();
                        assert_eq!(len, if mode == "goaway-malformed" { 4 } else { 8 });
                        assert_eq!(
                            u32::from_be_bytes(head[5..9].try_into().unwrap()),
                            if mode == "goaway-nonzero" { 1 } else { 0 }
                        );
                        assert_eq!(&payload[..4], &[0, 0, 0, 1]);
                    }
                    break;
                }
                socket.read_exact(&mut payload).await.unwrap();
                acknowledge(
                    &mut socket,
                    &Frame {
                        kind: head[3],
                        flags: head[4],
                        stream: 0,
                        payload,
                    },
                )
                .await;
            }
            drop(socket);
            task.abort();
            let _ = task.await;
        })
        .await
        .expect("invalid GOAWAY proof exceeded deadline");
    }
}

#[tokio::test]
async fn held_stream_remains_active_until_release_request_is_observed() {
    tokio::time::timeout(DEADLINE, async {
        let trace = "lifecycle-proof-held";
        let (mut socket, task) = connect("held", trace, false).await;
        loop {
            if recorder::query(trace)["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|event| event["detail"] == "lifecycle-held-ready")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(!recorder::query(trace)["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["kind"] == recorder::KIND_RESP_HEAD));
        let mut block = Vec::new();
        for (name, value) in [
            (":method", "GET"),
            (":scheme", "http"),
            (":authority", "lifecycle.proof"),
            (":path", "/release"),
            ("x-trace-id", "lifecycle-proof-release"),
        ] {
            header(&mut block, name, value);
        }
        let mut release = Vec::new();
        frame(&mut release, 1, 5, 3, &block);
        socket.write_all(&release).await.unwrap();
        let mut bodies = [Vec::new(), Vec::new()];
        let mut ended = [false, false];
        while ended != [true, true] {
            let received = read(&mut socket).await;
            acknowledge(&mut socket, &received).await;
            assert_ne!(received.kind, 3);
            if received.kind == 0 && matches!(received.stream, 1 | 3) {
                let index = if received.stream == 1 { 0 } else { 1 };
                bodies[index].extend_from_slice(&received.payload);
                ended[index] = received.flags & 1 != 0;
            }
        }
        assert_eq!(bodies[0].len(), 64);
        assert_eq!(bodies[1], b"edgion-transport");
        drop(socket);
        task.abort();
        let _ = task.await;
    })
    .await
    .expect("held lifecycle proof exceeded deadline");
}

#[tokio::test]
async fn goaway_fault_is_not_repeated_on_a_new_connection_with_the_same_trace() {
    tokio::time::timeout(DEADLINE, async {
        let trace = "lifecycle-proof-goaway-followup";
        let (mut socket, task) = connect("goaway-excluding", trace, false).await;
        let mut response = Vec::new();
        let mut goaways = 0;
        loop {
            let received = read(&mut socket).await;
            acknowledge(&mut socket, &received).await;
            if received.kind == 7 {
                goaways += 1;
            }
            if received.kind == 0 && received.stream == 1 {
                response.extend_from_slice(&received.payload);
                if received.flags & 1 != 0 {
                    break;
                }
            }
        }
        assert_eq!(goaways, 2);
        assert_eq!(response.len(), 64);
        drop(socket);
        task.abort();
        let _ = task.await;

        // Use the public decoder for the eligible attempt: its status must be
        // 201, distinguishing a new response from the excluded first reply.
        let (address, task) = backend().await;
        let socket = TcpStream::connect(address).await.unwrap();
        let (mut client, connection) = h2::client::handshake(socket).await.unwrap();
        let driver = tokio::spawn(connection);
        let request = http::Request::builder()
            .method("POST")
            .uri("http://lifecycle.proof/proof")
            .header("x-up-lifecycle", "goaway-excluding")
            .header("x-trace-id", trace)
            .body(())
            .unwrap();
        let (response, _) = client.send_request(request, true).unwrap();
        let response = response.await.expect("eligible retry received a repeated GOAWAY");
        assert_eq!(response.status(), 201);
        let mut body = response.into_body();
        let mut received = Vec::new();
        while let Some(chunk) = body.data().await {
            let chunk = chunk.unwrap();
            body.flow_control().release_capacity(chunk.len()).unwrap();
            received.extend_from_slice(&chunk);
        }
        assert_eq!(received.len(), 64);
        driver.abort();
        let _ = driver.await;
        task.abort();
        let _ = task.await;

        let query = recorder::query(trace);
        let headers = query["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["kind"] == recorder::KIND_REQ_HEADERS)
            .collect::<Vec<_>>();
        assert_eq!(headers.len(), 2);
        assert_ne!(headers[0]["conn"], headers[1]["conn"]);
        assert!(query["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["detail"] == "lifecycle-goaway-followup"
                && event["trace_request_ordinal"] == 2
                && event["status"] == 201));
    })
    .await
    .expect("GOAWAY followup proof exceeded deadline");
}
