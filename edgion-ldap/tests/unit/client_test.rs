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

/// Bound a test step so a session that never terminates fails the test
/// instead of hanging the suite.
async fn within<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(std::time::Duration::from_secs(5), future)
        .await
        .expect("test step did not finish within 5 s")
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
    assert_eq!(within(session.simple_bind("cn=a", b"pw")).await.unwrap().result_code, 0);
    let (_, request) = within(task).await.unwrap();
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
            within(session.simple_bind("cn=a", b"pw")).await.unwrap().result_code,
            u32::from(rc)
        );
        within(task).await.unwrap();
    }
}

#[tokio::test]
async fn unbind_bytes() {
    let (client, peer) = duplex(4096);
    let task = answer_once(peer, bind_response(1, 0));
    let mut session = BindSession::new(client);
    within(session.simple_bind("cn=a", b"pw")).await.unwrap();
    let (mut peer, _) = within(task).await.unwrap();
    within(session.unbind()).await.unwrap();
    let mut unbind = Vec::new();
    // `unbind` consumes the session, so the peer sees EOF after the request
    // (shutdown itself is asserted with `RecordingStream`).
    within(peer.read_to_end(&mut unbind)).await.unwrap();
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
    let (stream, next_id) = within(BindSession::new(client).start_tls()).await.unwrap();
    assert_eq!(next_id, MessageId(2));
    let (peer, request) = within(task).await.unwrap();
    let mut expected = vec![0x30, 0x1d, 0x02, 0x01, 0x01, 0x77, 0x18, 0x80, 0x16];
    expected.extend_from_slice(b"1.3.6.1.4.1.1466.20037");
    assert_eq!(request, expected);

    let task = answer_once(peer, bind_response(2, 0));
    let mut session = BindSession::resume(stream, next_id);
    assert_eq!(within(session.simple_bind("cn=a", b"pw")).await.unwrap().result_code, 0);
    let (_, request) = within(task).await.unwrap();
    assert_eq!(&request[2..5], &[0x02, 0x01, 0x02]);
}

#[tokio::test]
async fn start_tls_rejected_rc() {
    let (client, peer) = duplex(4096);
    let task = answer_once(peer, extended_response(1, 2));
    assert!(matches!(
        within(BindSession::new(client).start_tls()).await,
        Err(Error::StartTlsRejected(2))
    ));
    within(task).await.unwrap();
}

#[tokio::test]
async fn unexpected_message_ids_are_protocol_errors() {
    for id in [0u8, 7] {
        let (client, peer) = duplex(4096);
        let task = answer_once(peer, bind_response(id, 0));
        let mut session = BindSession::new(client);
        assert!(matches!(
            within(session.simple_bind("cn=a", b"pw")).await,
            Err(Error::UnexpectedMessageId { expected: 1, actual: a }) if a == u32::from(id)
        ));
        within(task).await.unwrap();
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
    assert_eq!(
        within(session.simple_bind("cn=a", b"pw")).await.unwrap().result_code,
        10
    );
    within(task).await.unwrap();
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
        within(session.simple_bind("cn=a", b"pw")).await,
        Err(Error::FrameTooLarge {
            declared: 65_537,
            max: 65_536
        })
    ));
    within(task).await.unwrap();
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
    assert!(matches!(
        within(session.simple_bind("cn=a", b"pw")).await,
        Err(Error::Closed)
    ));
    within(task).await.unwrap();
}

#[tokio::test]
async fn write_to_closed_peer_is_io_error() {
    let (client, peer) = duplex(4096);
    drop(peer);
    let mut session = BindSession::new(client);
    assert!(matches!(
        within(session.simple_bind("cn=a", b"pw")).await,
        Err(Error::Io(_))
    ));
}

#[tokio::test]
async fn start_tls_does_not_read_past_frame() {
    let mut reply = extended_response(1, 0);
    reply.extend_from_slice(&[0x16, 0x03, 0x01]);
    let (client, peer) = duplex(4096);
    let task = answer_once(peer, reply);
    let (mut stream, _) = within(BindSession::new(client).start_tls()).await.unwrap();
    // Drop the peer so an over-read hits EOF and fails instead of hanging.
    drop(within(task).await.unwrap());
    let mut next = [0u8; 3];
    within(stream.read_exact(&mut next)).await.unwrap();
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
    assert_eq!(within(session.simple_bind("cn=a", b"pw")).await.unwrap().result_code, 0);
    within(task).await.unwrap();
}

#[tokio::test]
async fn long_form_non_minimal_outer_length_accepted() {
    let reply = vec![
        0x30, 0x84, 0x00, 0x00, 0x00, 0x0c, 0x02, 0x01, 0x01, 0x61, 0x07, 0x0a, 0x01, 0x00, 0x04, 0x00, 0x04, 0x00,
    ];
    let (client, peer) = duplex(4096);
    let task = answer_once(peer, reply);
    let mut session = BindSession::new(client);
    assert_eq!(within(session.simple_bind("cn=a", b"pw")).await.unwrap().result_code, 0);
    within(task).await.unwrap();
}

// ── Search mode ─────────────────────────────────────────────────────────

fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    assert!(content.len() < 0x80);
    let mut out = vec![tag, content.len() as u8];
    out.extend_from_slice(content);
    out
}

fn entry(id: u8, dn: &[u8], uid: Option<&[u8]>) -> Vec<u8> {
    let attributes = uid
        .map(|value| tlv(0x30, &[tlv(0x04, b"uid"), tlv(0x31, &tlv(0x04, value))].concat()))
        .unwrap_or_default();
    let body = [tlv(0x04, dn), tlv(0x30, &attributes)].concat();
    tlv(0x30, &[tlv(0x02, &[id]), tlv(0x64, &body)].concat())
}

fn reference(id: u8) -> Vec<u8> {
    tlv(0x30, &[tlv(0x02, &[id]), tlv(0x73, &tlv(0x04, b"ldap://x/"))].concat())
}

fn done(id: u8, rc: u8) -> Vec<u8> {
    tlv(
        0x30,
        &[tlv(0x02, &[id]), tlv(0x65, &[0x0a, 0x01, rc, 0x04, 0x00, 0x04, 0x00])].concat(),
    )
}

async fn search(reply: Vec<u8>) -> (Result<Option<FoundEntry>, Error>, Vec<u8>) {
    let (client, peer) = duplex(64 * 1024);
    let task = answer_once(peer, reply);
    let mut session = BindSession::new(client);
    let filter = SearchFilter::compile("(uid={username})").unwrap();
    let result = within(session.search_unique("dc=x", Scope::Subtree, &filter, b"bob", "uid", 5)).await;
    let (_, request) = within(task).await.unwrap();
    (result, request)
}

#[tokio::test]
async fn search_one_entry_is_found() {
    let reply = [entry(1, b"uid=bob,dc=x", Some(b"bob")), done(1, 0)].concat();
    let (result, request) = search(reply).await;
    assert_eq!(
        result.unwrap(),
        Some(FoundEntry {
            dn: "uid=bob,dc=x".to_string(),
            identity: Some("bob".to_string())
        })
    );
    assert_eq!(request[0], 0x30);
    assert!(request.windows(3).any(|window| window == b"bob"));
}

#[tokio::test]
async fn search_none_cases() {
    for reply in [
        done(1, 0),
        [entry(1, b"uid=a,dc=x", Some(b"a")), entry(1, b"uid=b,dc=x", Some(b"b"))].concat(),
        [entry(1, b"uid=a,dc=x", Some(b"a")), done(1, 4)].concat(),
        done(1, 4),
    ] {
        let (result, _) = search(reply).await;
        assert_eq!(result.unwrap(), None);
    }
}

#[tokio::test]
async fn entry_with_references_is_found() {
    let reply = [
        reference(1),
        entry(1, b"uid=bob,dc=x", Some(b"bob")),
        reference(1),
        done(1, 0),
    ]
    .concat();
    let (result, _) = search(reply).await;
    assert_eq!(result.unwrap().map(|found| found.dn), Some("uid=bob,dc=x".to_string()));

    // Exactly the cap is still accepted.
    let mut reply: Vec<u8> = (0..MAX_SEARCH_REFERENCES).flat_map(|_| reference(1)).collect();
    reply.extend(entry(1, b"uid=bob,dc=x", Some(b"bob")));
    reply.extend(done(1, 0));
    let (result, _) = search(reply).await;
    assert!(result.unwrap().is_some());
}

#[tokio::test]
async fn search_errors() {
    let too_many: Vec<u8> = (0..=MAX_SEARCH_REFERENCES).flat_map(|_| reference(1)).collect();
    let (result, _) = search(too_many).await;
    assert!(matches!(result, Err(Error::TooManyReferences)));

    // SearchFailed and EmptyDn are asserted in `failed_exchanges_break_the_session`.
    let (result, _) = search(done(2, 0)).await;
    assert!(matches!(
        result,
        Err(Error::UnexpectedMessageId { expected: 1, actual: 2 })
    ));
}

#[tokio::test]
async fn search_mode_message_ids_in_one_session() {
    let (client, mut peer) = duplex(64 * 1024);
    let task = tokio::spawn(async move {
        let mut ids = Vec::new();
        for reply in [
            bind_response(1, 0),
            [entry(2, b"uid=bob,dc=x", Some(b"bob")), done(2, 0)].concat(),
            bind_response(3, 0),
        ] {
            let request = read_request(&mut peer).await;
            ids.push(request[4]);
            peer.write_all(&reply).await.unwrap();
        }
        let request = read_request(&mut peer).await;
        ids.push(request[4]);
        ids
    });
    let mut session = BindSession::new(client);
    let filter = SearchFilter::compile("(uid={username})").unwrap();
    assert_eq!(
        within(session.simple_bind("cn=svc,dc=x", b"svc"))
            .await
            .unwrap()
            .result_code,
        0
    );
    let found = within(session.search_unique("dc=x", Scope::Subtree, &filter, b"bob", "uid", 5))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        within(session.simple_bind(&found.dn, b"pw")).await.unwrap().result_code,
        0
    );
    within(session.unbind()).await.unwrap();
    assert_eq!(within(task).await.unwrap(), vec![1, 2, 3, 4]);
}

async fn group_check(reply: Vec<u8>) -> (Result<Option<FoundEntry>, Error>, Vec<u8>) {
    let (client, peer) = duplex(64 * 1024);
    let task = answer_once(peer, reply);
    let mut session = BindSession::new(client);
    let filter = SearchFilter::group_requirement("memberOf", &["cn=g,dc=x"], &[], false).unwrap();
    let result = within(session.search_unique("uid=bob,dc=x", Scope::Base, &filter, b"", "1.1", 5)).await;
    let (_, request) = within(task).await.unwrap();
    (result, request)
}

#[tokio::test]
async fn base_scope_group_check_match_and_no_match() {
    // `group_check` passes an empty username: a slotless filter accepts it.
    let (result, request) = group_check([entry(1, b"uid=bob,dc=x", None), done(1, 0)].concat()).await;
    assert_eq!(result.unwrap().map(|found| found.dn), Some("uid=bob,dc=x".to_string()));
    // baseObject scope (ENUMERATED 0) and sizeLimit 1 follow the base DN.
    let base = tlv(0x04, b"uid=bob,dc=x");
    let at = request.windows(base.len()).position(|w| w == base.as_slice()).unwrap() + base.len();
    assert_eq!(
        &request[at..at + 9],
        &[0x0a, 0x01, 0x00, 0x0a, 0x01, 0x00, 0x02, 0x01, 0x01]
    );

    let (result, _) = group_check(done(1, 0)).await;
    assert_eq!(result.unwrap(), None);
    let (result, _) = group_check(done(1, 32)).await;
    assert!(matches!(result, Err(Error::SearchFailed(32))));
}

// ── Preconditions and the broken-session invariant ──────────────────────

/// Search with `(uid={username})` over `session`.
async fn search_on(session: &mut BindSession<DuplexStream>, username: &[u8]) -> Result<Option<FoundEntry>, Error> {
    let filter = SearchFilter::compile("(uid={username})").unwrap();
    within(session.search_unique("dc=x", Scope::Subtree, &filter, username, "uid", 5)).await
}

#[tokio::test]
async fn unauthenticated_binds_and_empty_usernames_are_refused_before_io() {
    let (client, peer) = duplex(4096);
    let task = answer_once(peer, bind_response(1, 0));
    let mut session = BindSession::new(client);
    for (dn, password) in [("cn=a", &b""[..]), ("", &b"pw"[..]), ("", &b""[..]), (" ", &b"pw"[..])] {
        assert!(matches!(
            within(session.simple_bind(dn, password)).await,
            Err(Error::UnauthenticatedBind)
        ));
    }
    assert!(matches!(search_on(&mut session, b"").await, Err(Error::EmptyUsername)));
    // A substring slot is refused too (it would encode an empty component).
    let substring = SearchFilter::compile("(cn={username}*)").unwrap();
    assert!(matches!(
        within(session.search_unique("dc=x", Scope::Subtree, &substring, b"", "uid", 5)).await,
        Err(Error::EmptyUsername)
    ));
    // Nothing was written and the session is still usable: the first frame on
    // the wire is exactly the later bind with message ID 1.
    assert_eq!(within(session.simple_bind("cn=a", b"pw")).await.unwrap().result_code, 0);
    let (_, request) = within(task).await.unwrap();
    assert_eq!(
        request,
        [
            0x30, 0x12, 0x02, 0x01, 0x01, 0x60, 0x0d, 0x02, 0x01, 0x03, 0x04, 0x04, b'c', b'n', b'=', b'a', 0x80, 0x02,
            b'p', b'w'
        ]
    );
}

#[tokio::test]
async fn rejected_bind_keeps_the_session_usable() {
    let (client, mut peer) = duplex(4096);
    let task = tokio::spawn(async move {
        let mut ids = Vec::new();
        for reply in [bind_response(1, 49), bind_response(2, 0)] {
            ids.push(read_request(&mut peer).await[4]);
            peer.write_all(&reply).await.unwrap();
        }
        ids
    });
    let mut session = BindSession::new(client);
    assert_eq!(
        within(session.simple_bind("cn=a", b"bad")).await.unwrap().result_code,
        49
    );
    assert_eq!(within(session.simple_bind("cn=a", b"pw")).await.unwrap().result_code, 0);
    assert_eq!(within(task).await.unwrap(), vec![1, 2]);
}

/// Run one exchange answered with `reply`, check its outcome (`Debug` text),
/// then check that the session is broken: further exchanges fail without I/O
/// and `unbind` sends nothing.
async fn assert_broken_after(reply: Vec<u8>, exchange_is_search: bool, expected: &str) {
    let (client, mut peer) = duplex(64 * 1024);
    let task = tokio::spawn(async move {
        read_request(&mut peer).await;
        peer.write_all(&reply).await.unwrap();
        // Whatever arrives after the first request (only EOF is expected).
        let mut rest = Vec::new();
        peer.read_to_end(&mut rest).await.unwrap();
        rest
    });
    let mut session = BindSession::new(client);
    let outcome = if exchange_is_search {
        format!("{:?}", search_on(&mut session, b"bob").await)
    } else {
        format!("{:?}", within(session.simple_bind("cn=a", b"pw")).await)
    };
    assert_eq!(outcome, expected);
    assert!(matches!(
        within(session.simple_bind("cn=a", b"pw")).await,
        Err(Error::SessionBroken)
    ));
    assert!(matches!(
        search_on(&mut session, b"bob").await,
        Err(Error::SessionBroken)
    ));
    within(session.unbind()).await.unwrap();
    assert!(
        within(task).await.unwrap().is_empty(),
        "a broken session must not send an UnbindRequest"
    );
}

#[tokio::test]
async fn failed_exchanges_break_the_session() {
    // Wrong message ID on a bind.
    assert_broken_after(
        bind_response(7, 0),
        false,
        "Err(UnexpectedMessageId { expected: 1, actual: 7 })",
    )
    .await;
    // A fully read search that failed.
    assert_broken_after(done(1, 32), true, "Err(SearchFailed(32))").await;
    // An entry with an empty DN.
    assert_broken_after([entry(1, b"", Some(b"bob")), done(1, 0)].concat(), true, "Err(EmptyDn)").await;
    // A second entry: the rest of the response is unread.
    assert_broken_after(
        [entry(1, b"uid=a,dc=x", Some(b"a")), entry(1, b"uid=b,dc=x", Some(b"b"))].concat(),
        true,
        "Ok(None)",
    )
    .await;
}

#[tokio::test]
async fn cancelled_exchange_breaks_the_session() {
    let (client, mut peer) = duplex(4096);
    let task = tokio::spawn(async move {
        read_request(&mut peer).await;
        let mut rest = Vec::new();
        peer.read_to_end(&mut rest).await.unwrap();
        rest
    });
    let mut session = BindSession::new(client);
    // The peer never answers, so the bind is dropped while reading.
    let cancelled =
        tokio::time::timeout(std::time::Duration::from_millis(50), session.simple_bind("cn=a", b"pw")).await;
    assert!(cancelled.is_err());
    assert!(matches!(
        within(session.simple_bind("cn=a", b"pw")).await,
        Err(Error::SessionBroken)
    ));
    within(session.unbind()).await.unwrap();
    assert!(within(task).await.unwrap().is_empty());
}

#[tokio::test]
async fn second_entry_wins_over_an_empty_dn() {
    let reply = [entry(1, b"uid=a,dc=x", Some(b"a")), entry(1, b"", Some(b"b"))].concat();
    let (result, _) = search(reply).await;
    assert_eq!(result.unwrap(), None);
}

/// What a [`RecordingStream`] saw.
#[derive(Default)]
struct Recorded {
    written: std::sync::atomic::AtomicUsize,
    shut_down: std::sync::atomic::AtomicBool,
}

impl Recorded {
    fn written(&self) -> usize {
        self.written.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn shut_down(&self) -> bool {
        self.shut_down.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// A stream that records how many bytes were written and whether it was shut
/// down; writes and shutdown can be made to fail. Reads return EOF.
struct RecordingStream {
    recorded: std::sync::Arc<Recorded>,
    fail_writes: bool,
    fail_shutdown: bool,
}

impl RecordingStream {
    fn new(fail_writes: bool, fail_shutdown: bool) -> (Self, std::sync::Arc<Recorded>) {
        let recorded = std::sync::Arc::new(Recorded::default());
        let stream = Self {
            recorded: recorded.clone(),
            fail_writes,
            fail_shutdown,
        };
        (stream, recorded)
    }
}

impl tokio::io::AsyncRead for RecordingStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        _: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

impl tokio::io::AsyncWrite for RecordingStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        if self.fail_writes {
            return std::task::Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
        }
        self.recorded
            .written
            .fetch_add(buf.len(), std::sync::atomic::Ordering::SeqCst);
        std::task::Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.recorded.shut_down.store(true, std::sync::atomic::Ordering::SeqCst);
        if self.fail_shutdown {
            return std::task::Poll::Ready(Err(std::io::ErrorKind::ConnectionReset.into()));
        }
        std::task::Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn healthy_unbind_sends_the_request_then_shuts_down() {
    let (stream, recorded) = RecordingStream::new(false, false);
    within(BindSession::new(stream).unbind()).await.unwrap();
    // The 7-byte UnbindRequest (bytes asserted in `unbind_bytes`), then shutdown.
    assert_eq!(recorded.written(), 7);
    assert!(recorded.shut_down());
}

#[tokio::test]
async fn broken_unbind_sends_nothing_but_shuts_down() {
    // IDs past maxInt break the session on its first exchange, before any write.
    let (stream, recorded) = RecordingStream::new(false, false);
    let mut session = BindSession::resume(stream, MessageId(MAX_MESSAGE_ID + 1));
    assert!(matches!(
        within(session.simple_bind("cn=a", b"pw")).await,
        Err(Error::MessageIdExhausted)
    ));
    within(session.unbind()).await.unwrap();
    assert_eq!(recorded.written(), 0);
    assert!(recorded.shut_down());
}

#[tokio::test]
async fn unbind_shuts_down_even_when_the_request_write_fails() {
    let (stream, recorded) = RecordingStream::new(true, false);
    assert!(matches!(
        within(BindSession::new(stream).unbind()).await,
        Err(Error::Io(std::io::ErrorKind::BrokenPipe))
    ));
    assert!(recorded.shut_down());
}

#[tokio::test]
async fn unbind_returns_the_first_error() {
    let (stream, recorded) = RecordingStream::new(true, true);
    assert!(matches!(
        within(BindSession::new(stream).unbind()).await,
        Err(Error::Io(std::io::ErrorKind::BrokenPipe))
    ));
    assert!(recorded.shut_down());
    let (stream, _) = RecordingStream::new(false, true);
    assert!(matches!(
        within(BindSession::new(stream).unbind()).await,
        Err(Error::Io(std::io::ErrorKind::ConnectionReset))
    ));
}

#[tokio::test]
async fn message_ids_stop_at_max_int() {
    let (client, peer) = duplex(4096);
    // BindResponse for message ID 0x7fffffff.
    let reply = vec![
        0x30, 0x0f, 0x02, 0x04, 0x7f, 0xff, 0xff, 0xff, 0x61, 0x07, 0x0a, 0x01, 0x00, 0x04, 0x00, 0x04, 0x00,
    ];
    let task = answer_once(peer, reply);
    let mut session = BindSession::resume(client, MessageId(MAX_MESSAGE_ID));
    assert_eq!(within(session.simple_bind("cn=a", b"pw")).await.unwrap().result_code, 0);
    let (mut peer, request) = within(task).await.unwrap();
    assert_eq!(&request[2..8], &[0x02, 0x04, 0x7f, 0xff, 0xff, 0xff]);
    assert!(matches!(
        within(session.simple_bind("cn=a", b"pw")).await,
        Err(Error::MessageIdExhausted)
    ));
    assert!(matches!(
        within(session.simple_bind("cn=a", b"pw")).await,
        Err(Error::SessionBroken)
    ));
    within(session.unbind()).await.unwrap();
    let mut rest = Vec::new();
    within(peer.read_to_end(&mut rest)).await.unwrap();
    assert!(rest.is_empty(), "no request may carry an ID above maxInt");
}

#[test]
fn debug_output_carries_no_peer_text() {
    let found = FoundEntry {
        dn: "uid=secret-user,dc=x".to_string(),
        identity: Some("secret-user".to_string()),
    };
    let rendered = format!("{found:?}");
    assert!(!rendered.contains("secret"), "{rendered}");
    assert!(rendered.contains("[redacted]"));
    let anonymous = FoundEntry {
        dn: "uid=secret-user,dc=x".to_string(),
        identity: None,
    };
    let rendered = format!("{anonymous:?}");
    assert!(
        !rendered.contains("secret") && rendered.contains("identity: None"),
        "{rendered}"
    );
    let io = io_error(std::io::Error::other("peer said secret-detail"));
    assert!(!format!("{io:?} {io}").contains("secret"));
    assert!(matches!(io, Error::Io(std::io::ErrorKind::Other)));
}
