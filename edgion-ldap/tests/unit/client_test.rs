use super::*;
use tokio::io::{duplex, DuplexStream};

fn bind_response(id: u8, rc: u8) -> Vec<u8> {
    vec![
        0x30, 0x0c, 0x02, 0x01, id, 0x61, 0x07, 0x0a, 0x01, rc, 0x04, 0x00, 0x04, 0x00,
    ]
}

/// Read one request frame from the peer side, returning the whole frame.
async fn read_request(peer: &mut DuplexStream) -> Vec<u8> {
    let mut header = [0u8; 2];
    peer.read_exact(&mut header).await.unwrap();
    let extra = length_octets(header[0], header[1]).unwrap();
    let mut octets = vec![0u8; extra];
    peer.read_exact(&mut octets).await.unwrap();
    let len = frame_content_len(header[1], &octets).unwrap();
    let mut body = vec![0u8; len];
    peer.read_exact(&mut body).await.unwrap();
    let mut frame = header.to_vec();
    frame.extend_from_slice(&octets);
    frame.extend_from_slice(&body);
    frame
}

/// Spawn a peer that answers one request with `reply`.
fn answer_once(mut peer: DuplexStream, reply: Vec<u8>) -> tokio::task::JoinHandle<(DuplexStream, Vec<u8>)> {
    tokio::spawn(async move {
        let request = read_request(&mut peer).await;
        peer.write_all(&reply).await.unwrap();
        (peer, request)
    })
}

#[tokio::test]
async fn simple_bind_encodes_rfc4511_bind_request() {
    let (client, peer) = duplex(4096);
    let task = answer_once(peer, bind_response(1, 0));
    let mut session = BindSession::new(client);
    assert_eq!(session.simple_bind("cn=a", b"pw").await.unwrap().result_code, 0);
    let (_, request) = task.await.unwrap();
    assert_eq!(
        request,
        [
            0x30, 0x12, 0x02, 0x01, 0x01, 0x60, 0x0d, 0x02, 0x01, 0x03, 0x04, 0x04, b'c', b'n', b'=', b'a', 0x80, 0x02,
            b'p', b'w'
        ]
    );
}

#[tokio::test]
async fn bind_result_codes() {
    for rc in [0u8, 49, 53] {
        let (client, peer) = duplex(4096);
        let task = answer_once(peer, bind_response(1, rc));
        let mut session = BindSession::new(client);
        assert_eq!(
            session.simple_bind("cn=a", b"pw").await.unwrap().result_code,
            u32::from(rc)
        );
        task.await.unwrap();
    }
}

#[tokio::test]
async fn unbind_bytes() {
    let (client, peer) = duplex(4096);
    let task = answer_once(peer, bind_response(1, 0));
    let mut session = BindSession::new(client);
    session.simple_bind("cn=a", b"pw").await.unwrap();
    let (mut peer, _) = task.await.unwrap();
    session.unbind().await.unwrap();
    let mut unbind = Vec::new();
    // The write side is shut down after the request, so the peer sees EOF.
    peer.read_to_end(&mut unbind).await.unwrap();
    assert_eq!(unbind, [0x30, 0x05, 0x02, 0x01, 0x02, 0x42, 0x00]);
}

fn extended_response(id: u8, rc: u8) -> Vec<u8> {
    vec![
        0x30, 0x0c, 0x02, 0x01, id, 0x78, 0x07, 0x0a, 0x01, rc, 0x04, 0x00, 0x04, 0x00,
    ]
}

#[tokio::test]
async fn start_tls_request_bytes_and_success_resumes_ids() {
    let (client, peer) = duplex(4096);
    let task = answer_once(peer, extended_response(1, 0));
    let (stream, next_id) = BindSession::new(client).start_tls().await.unwrap();
    assert_eq!(next_id, MessageId(2));
    let (peer, request) = task.await.unwrap();
    let mut expected = vec![0x30, 0x1d, 0x02, 0x01, 0x01, 0x77, 0x18, 0x80, 0x16];
    expected.extend_from_slice(b"1.3.6.1.4.1.1466.20037");
    assert_eq!(request, expected);

    let task = answer_once(peer, bind_response(2, 0));
    let mut session = BindSession::resume(stream, next_id);
    assert_eq!(session.simple_bind("cn=a", b"pw").await.unwrap().result_code, 0);
    let (_, request) = task.await.unwrap();
    assert_eq!(&request[2..5], &[0x02, 0x01, 0x02]);
}

#[tokio::test]
async fn start_tls_rejected_rc() {
    let (client, peer) = duplex(4096);
    let task = answer_once(peer, extended_response(1, 2));
    assert!(matches!(
        BindSession::new(client).start_tls().await,
        Err(Error::StartTlsRejected(2))
    ));
    task.await.unwrap();
}

#[tokio::test]
async fn unexpected_message_ids_are_protocol_errors() {
    for id in [0u8, 7] {
        let (client, peer) = duplex(4096);
        let task = answer_once(peer, bind_response(id, 0));
        let mut session = BindSession::new(client);
        assert!(matches!(
            session.simple_bind("cn=a", b"pw").await,
            Err(Error::UnexpectedMessageId { expected: 1, actual: a }) if a == u32::from(id)
        ));
        task.await.unwrap();
    }
}

#[tokio::test]
async fn referral_controls_and_ad_quirk_tails_skipped() {
    let reply = [
        0x30, 0x18, 0x02, 0x01, 0x01, 0x61, 0x0f, 0x0a, 0x01, 0x0a, 0x04, 0x00, 0x04, 0x00, 0xa3, 0x06, 0x04, 0x04,
        b'l', b'd', b'a', b'p', 0xa0, 0x00, 0x8a, 0x00,
    ];
    let (client, peer) = duplex(4096);
    let task = answer_once(peer, reply.to_vec());
    let mut session = BindSession::new(client);
    assert_eq!(session.simple_bind("cn=a", b"pw").await.unwrap().result_code, 10);
    task.await.unwrap();
}

#[tokio::test]
async fn oversized_declared_length_leaves_body_unread() {
    let body = [0xaau8; 8];
    let mut reply = vec![0x30, 0x83, 0x01, 0x00, 0x01];
    reply.extend_from_slice(&body);
    let (client, mut peer) = duplex(4096);
    // The peer hangs up after replying, so a client that reads the body hits
    // EOF and fails instead of hanging.
    let task = tokio::spawn(async move {
        read_request(&mut peer).await;
        peer.write_all(&reply).await.unwrap();
    });
    let mut session = BindSession::new(client);
    assert!(matches!(
        session.simple_bind("cn=a", b"pw").await,
        Err(Error::FrameTooLarge {
            declared: 65_537,
            max: 65_536
        })
    ));
    task.await.unwrap();
    let mut unread = [0u8; 8];
    session.stream.read_exact(&mut unread).await.unwrap();
    assert_eq!(unread, body);
}

#[tokio::test]
async fn peer_close_is_closed() {
    let (client, mut peer) = duplex(4096);
    let task = tokio::spawn(async move {
        read_request(&mut peer).await;
        drop(peer);
    });
    let mut session = BindSession::new(client);
    assert!(matches!(session.simple_bind("cn=a", b"pw").await, Err(Error::Closed)));
    task.await.unwrap();
}

#[tokio::test]
async fn write_to_closed_peer_is_io_error() {
    let (client, peer) = duplex(4096);
    drop(peer);
    let mut session = BindSession::new(client);
    assert!(matches!(session.simple_bind("cn=a", b"pw").await, Err(Error::Io(_))));
}

#[tokio::test]
async fn start_tls_does_not_read_past_frame() {
    let mut reply = extended_response(1, 0);
    reply.extend_from_slice(&[0x16, 0x03, 0x01]);
    let (client, peer) = duplex(4096);
    let task = answer_once(peer, reply);
    let (mut stream, _) = BindSession::new(client).start_tls().await.unwrap();
    // Drop the peer so an over-read hits EOF and fails instead of hanging.
    drop(task.await.unwrap());
    let mut next = [0u8; 3];
    stream.read_exact(&mut next).await.unwrap();
    assert_eq!(next, [0x16, 0x03, 0x01]);
}

#[tokio::test]
async fn reply_split_into_single_bytes_decodes() {
    let (client, mut peer) = duplex(4096);
    let task = tokio::spawn(async move {
        read_request(&mut peer).await;
        for byte in bind_response(1, 0) {
            peer.write_all(&[byte]).await.unwrap();
            peer.flush().await.unwrap();
            tokio::task::yield_now().await;
        }
        peer
    });
    let mut session = BindSession::new(client);
    assert_eq!(session.simple_bind("cn=a", b"pw").await.unwrap().result_code, 0);
    task.await.unwrap();
}

#[tokio::test]
async fn long_form_non_minimal_outer_length_accepted() {
    let reply = vec![
        0x30, 0x84, 0x00, 0x00, 0x00, 0x0c, 0x02, 0x01, 0x01, 0x61, 0x07, 0x0a, 0x01, 0x00, 0x04, 0x00, 0x04, 0x00,
    ];
    let (client, peer) = duplex(4096);
    let task = answer_once(peer, reply);
    let mut session = BindSession::new(client);
    assert_eq!(session.simple_bind("cn=a", b"pw").await.unwrap().result_code, 0);
    task.await.unwrap();
}
