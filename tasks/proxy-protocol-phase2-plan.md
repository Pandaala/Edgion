# Inbound PROXY protocol phase 2 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let one listener serve both load-balanced and direct traffic without opening a spoofing hole, keep the load balancer's own address observable, and prove end-to-end that a client address obtained from a PROXY header survives the next gateway hop.

**Architecture:** Phase 1 shipped mandatory inbound PROXY protocol that overwrites `SocketDigest.peer_addr`. That overwrite is what makes chaining work: every propagation site (the X-Forwarded-For append, the outbound PPv2 builder) reads that one canonical value. Phase 2 keeps `peer_addr` as the single source of truth and adds a source-conditional mode plus a lazily-computed `raw_peer_addr`.

**Tech Stack:** Rust; pingora fork at `/Users/caohao/ws2/pingora` (branch `edgion`, consumed by path `[patch]`); Edgion workspace at `/Users/caohao/ws2/Edgion` (branch `feature-06-24`); docker + HAProxy for integration tests.

## Scope changes from the reviewed draft

Three independent reviews (feasibility, threat model, completeness) found the first draft not executable. Every correction below is folded in; the notable scope decisions:

- **TCPRoute outbound PPv2 is dropped.** It was never in the original request and only matters for plain-TCP chains, which are out of scope for now. HTTPRoute chains propagate via X-Forwarded-For and TLSRoute chains already have outbound PPv2 today.
- **The "TCPRoute laundering" finding becomes documentation, not code.** A transparent L4 relay in front of a PROXY-accepting listener lets a client author the header. This is NOT introduced by conditional mode — it applies equally to Phase 1's mandatory mode, and to HAProxy/nginx. It is a restatement of the existing trust model, so it is documented rather than engineered around.
- **A prerequisite bug was found and already fixed** in `Stream::try_peek` (see Task 0), which the whole conditional mode is built on.

## Global Constraints

- All source-file content in English. Conversations may be Chinese.
- `peer_addr` semantics MUST NOT change: it is "the best known client address" and every propagation site reads it. New information is added alongside, never by redefining it.
- Untrusted peers must never have their bytes interpreted as a PROXY header. This is the single security invariant of the feature, and Task 2 exists to test it.
- Behaviour must be unchanged when the new options are absent: `proxy_protocol: true` with no matcher stays mandatory mode, and all existing pingora PP tests must pass unmodified.
- `TcpSocketOptions` is `#[non_exhaustive]`; cross-crate construction must mutate a `Default`.
- pingora-core must not gain a CIDR-parsing dependency; Edgion injects its matcher through a trait object, mirroring `ConnectionFilter`.
- Edgion gateway feature: `boringssl` (default). Build and test with `--offline`.

---

## File Structure

**pingora fork:**
- `pingora-core/src/protocols/l4/proxy_protocol.rs` — `ProxyProtocolTrust` trait, `peek_is_proxy_header`, `apply` dispatcher.
- `pingora-core/src/protocols/digest.rs` — `raw_peer_addr` on `SocketDigest`.
- `pingora-core/src/listeners/l4.rs` — `proxy_protocol_trusted_sources` on `TcpSocketOptions`.
- `pingora-core/src/listeners/mod.rs` — thread it through; listener-level security tests.

**Edgion:**
- `edgion-resources/src/constants/annotations.rs` — `PROXY_PROTOCOL_TRUSTED_CIDRS`.
- `edgion-gateway/src/runtime/server/listener_builder.rs` — parse, normalise, build the matcher.
- `edgion-gateway/src/routes/tls/proxy.rs`, `routes/tcp/edgion_tcp.rs`, `observe/logs/tcp_log.rs` — transport-peer logging.
- `edgion-tests/integration/conf/ProxyProtocolChain/`, `.../scripts/integration/test_proxy_protocol_chain.sh` — chain test.

---

### Task 0: Land Phase 1 and the prerequisite fixes

Phase 1's Edgion side is uncommitted, so any Phase 2 commit touching the same two files would fuse both phases into one non-revertible change. Three real defects were also found in existing code and are already fixed in the working tree; they belong to Phase 1, not Phase 2.

**Files (already modified, need committing):**
- `/Users/caohao/ws2/pingora/pingora-core/src/protocols/l4/stream.rs` — `try_peek` rewinds on every exit; test module no longer gated to Linux; new `test_failed_peek_preserves_the_bytes_it_read`.
- `/Users/caohao/ws2/Edgion/edgion-resources/src/constants/annotations.rs`, `edgion-gateway/src/runtime/server/listener_builder.rs`, `docs/en/dev-guide/annotations-guide.md`, `skills/02-features/05-annotations/references/annotations-metadata.md`
- `/Users/caohao/ws2/Edgion/edgion-tests/integration/scripts/integration/test_proxy_protocol_inbound.sh` — `$HAPROXY_CT` → `$LB_CT`
- `/Users/caohao/ws2/Edgion/edgion-tests/integration/scripts/utils/start_all_with_conf.sh` — `ProxyProtocolInbound` added to the conf-dir exclusion list
- `/Users/caohao/ws2/Edgion/edgion-tests/integration/conf/ProxyProtocolInbound/`, `tasks/proxy-protocol-inbound.md` (untracked)

**Why the two test-harness fixes matter:** `start_all_with_conf.sh:743` auto-loads every dir under `conf/`. `ProxyProtocolInbound/GatewayClass__public-gateway.yaml` has no `parametersRef`, so loading it overwrites `base/GatewayClass.yaml` and strips the shared `EdgionGatewayConfig` — silently breaking RealIP/XFF, Security, TLS fallback_sni, TCP idle timeout and GlobalConnectionIpRestriction. Its listeners also demand a header on every connection, making those ports unusable by other suites. The `$HAPROXY_CT` reference is a leftover from the HAProxy unification; under `set -u` it turns an assertion failure into an unbound-variable abort.

- [ ] **Step 1: Verify the pingora fix**

Run: `cd /Users/caohao/ws2/pingora && cargo test -p pingora-core --offline --lib`
Expected: 442 passed, including `protocols::l4::stream::tests::test_failed_peek_preserves_the_bytes_it_read`

- [ ] **Step 2: Commit the pingora fix, recording why**

```bash
cd /Users/caohao/ws2/pingora
git add pingora-core/src/protocols/l4/stream.rs
git commit -m "fix(stream): rewind try_peek on every exit path

read_exact drained the bytes it managed to read into the caller's buffer
and then returned, discarding them. Callers treat a failed peek as 'not
the protocol I was sniffing for' and keep serving the connection, so a
short read silently ate the data. Also affects the h2c preface probe in
apps/mod.rs, which peeks 24 bytes.

The test module was gated to Linux even though only the rx-timestamp
cases need it, so the portable rewind/peek tests never ran elsewhere."
git rev-parse HEAD   # record this SHA for the Edgion commit message
```

- [ ] **Step 3: Verify Edgion against the committed fork**

Run: `cd /Users/caohao/ws2/Edgion && cargo check --workspace --all-targets --offline`
Expected: no errors

- [ ] **Step 4: Run the Phase 1 integration test**

Run: `cd /Users/caohao/ws2/Edgion && ./edgion-tests/integration/scripts/integration/test_proxy_protocol_inbound.sh`
Expected: all phases pass, exit 0

- [ ] **Step 5: Commit Phase 1 with explicit paths**

Do NOT `git add` whole directories — the tree carries unrelated staged work under `docs/`, `skills/` and `edgion-gateway/src/ai/`.

```bash
cd /Users/caohao/ws2/Edgion
git add edgion-resources/src/constants/annotations.rs \
        edgion-gateway/src/runtime/server/listener_builder.rs \
        edgion-gateway/src/listeners/connection_filter.rs 2>/dev/null || true
git add docs/en/dev-guide/annotations-guide.md \
        skills/02-features/05-annotations/references/annotations-metadata.md \
        tasks/proxy-protocol-inbound.md \
        edgion-tests/integration/conf/ProxyProtocolInbound \
        edgion-tests/integration/scripts/integration/test_proxy_protocol_inbound.sh \
        edgion-tests/integration/scripts/utils/start_all_with_conf.sh
git commit -m "feat(gateway): inbound PROXY protocol v1/v2 on Gateway listeners

Requires pingora-core at <SHA from Step 2>."
```

- [ ] **Step 6: Register the standalone test so it actually runs**

`test_proxy_protocol_inbound.sh` is currently invoked by nothing. Add it beside the `test_plugin_policy.sh` invocation in `edgion-tests/integration/scripts/integration/run_integration.sh` (around line 1893), guarded so a machine without docker skips rather than fails:

```bash
if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
    run_standalone_script "test_proxy_protocol_inbound.sh"
else
    log_warn "docker unavailable; skipping inbound PROXY protocol suite"
fi
```

Match the surrounding invocation style — read the `test_plugin_policy.sh` call site first and mirror it exactly.

- [ ] **Step 7: Commit**

```bash
git add edgion-tests/integration/scripts/integration/run_integration.sh
git commit -m "test: register the inbound PROXY protocol suite"
```

---

### Task 1: Trust trait and header probe

**Files:**
- Modify: `/Users/caohao/ws2/pingora/pingora-core/src/protocols/l4/proxy_protocol.rs`

**Interfaces:**
- Produces: `pub trait ProxyProtocolTrust: std::fmt::Debug + Send + Sync { fn is_trusted(&self, addr: &StdSockAddr) -> bool; }`
- Produces: `pub(crate) async fn peek_is_proxy_header(stream: &mut Stream) -> Result<bool>`
- Consumes: `V2_SIGNATURE`, `crate::protocols::Peek` (fixed in Task 0)

The probe takes `&mut Stream` concretely rather than a generic `S: Peek`. `Stream` is the only thing the listener path ever hands it, and a concrete signature avoids needing a `Peek` impl (and an `async_trait` import) on the test reader.

- [ ] **Step 1: Write the failing tests**

Append to the `tests` module in `proxy_protocol.rs`. These drive a real `Stream` over loopback, because the peek/rewind interaction with `BufStream` is the part that can actually break:

```rust
    async fn stream_serving(bytes: &'static [u8]) -> crate::protocols::l4::stream::Stream {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            s.write_all(bytes).await.unwrap();
            drop(s);
        });
        tokio::net::TcpStream::connect(addr).await.unwrap().into()
    }

    #[tokio::test]
    async fn test_peek_detects_v2_and_leaves_the_header_readable() {
        let mut header = V2_SIGNATURE.to_vec();
        header.extend_from_slice(&[0x21, 0x11, 0x00, 0x0C]);
        header.extend_from_slice(&INET_ADDR);
        let leaked: &'static [u8] = Box::leak(header.into_boxed_slice());

        let mut stream = stream_serving(leaked).await;
        assert!(peek_is_proxy_header(&mut stream).await.unwrap());
        // The probe must not consume anything: the parser still sees a full header.
        let parsed = read_proxy_header(&mut stream).await.unwrap();
        assert_eq!(parsed.source.unwrap().to_string(), "192.168.0.1:56324");
    }

    #[tokio::test]
    async fn test_peek_detects_v1() {
        let mut stream = stream_serving(b"PROXY TCP4 1.2.3.4 5.6.7.8 1 2\r\n").await;
        assert!(peek_is_proxy_header(&mut stream).await.unwrap());
    }

    #[tokio::test]
    async fn test_peek_rejects_plain_request_without_eating_it() {
        let mut stream = stream_serving(b"GET / HTTP/1.1\r\n\r\n").await;
        assert!(!peek_is_proxy_header(&mut stream).await.unwrap());
        // Critical: the request must survive so the connection can still be served.
        use tokio::io::AsyncReadExt;
        let mut rest = Vec::new();
        stream.read_to_end(&mut rest).await.unwrap();
        assert_eq!(rest, b"GET / HTTP/1.1\r\n\r\n");
    }

    #[tokio::test]
    async fn test_peek_on_short_stream_is_not_an_error() {
        let mut stream = stream_serving(b"hi").await;
        assert!(!peek_is_proxy_header(&mut stream).await.unwrap());
        use tokio::io::AsyncReadExt;
        let mut rest = Vec::new();
        stream.read_to_end(&mut rest).await.unwrap();
        assert_eq!(rest, b"hi");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd /Users/caohao/ws2/pingora && cargo test -p pingora-core --offline --lib proxy_protocol`
Expected: FAIL — `cannot find function peek_is_proxy_header`

- [ ] **Step 3: Implement**

Add `use crate::protocols::Peek;` to the imports, then after the `ProxyHeader` struct:

```rust
/// Decides which peers may speak PROXY protocol to a listener.
///
/// A PROXY header is unauthenticated, so it may only be honoured from peers the
/// deployment already trusts — normally the load balancer. Implementations come
/// from the embedding application (pingora carries no CIDR parser), mirroring
/// [`crate::listeners::ConnectionFilter`].
pub trait ProxyProtocolTrust: std::fmt::Debug + Send + Sync {
    /// Whether `addr`, the transport-level peer, may send a PROXY header.
    fn is_trusted(&self, addr: &StdSockAddr) -> bool;
}

/// Whether the stream begins with something that looks like a PROXY header,
/// without consuming it.
///
/// Only ever call this for peers that passed [`ProxyProtocolTrust`]. Sniffing an
/// untrusted peer would let it choose whether to be trusted, which is exactly
/// what mandatory mode exists to prevent.
///
/// A stream that ends before 12 bytes arrive is reported as "no header" rather
/// than an error, and the bytes it did send remain readable.
pub(crate) async fn peek_is_proxy_header(stream: &mut Stream) -> Result<bool> {
    let mut sig = [0u8; 12];
    match stream.try_peek(&mut sig).await {
        Ok(true) => Ok(sig == V2_SIGNATURE || sig.starts_with(b"PROXY ")),
        Ok(false) => Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(e).or_err(HandshakeError, "failed to probe for a proxy protocol header"),
    }
}
```

- [ ] **Step 4: Run to verify they pass**

Run: `cd /Users/caohao/ws2/pingora && cargo test -p pingora-core --offline --lib proxy_protocol`
Expected: PASS, 26 tests

- [ ] **Step 5: Commit**

```bash
cd /Users/caohao/ws2/pingora
git add pingora-core/src/protocols/l4/proxy_protocol.rs
git commit -m "feat(proxy-protocol): trust trait and non-consuming header probe"
```

---

### Task 2: The dispatcher, and a test for the security invariant

The dispatcher is where the trust decision lives, so this task carries the tests that would fail if the decision were inverted, dropped, or ordered wrongly.

**Files:**
- Modify: `/Users/caohao/ws2/pingora/pingora-core/src/protocols/l4/proxy_protocol.rs`
- Modify: `/Users/caohao/ws2/pingora/pingora-core/src/listeners/l4.rs` (`TcpSocketOptions`)
- Modify: `/Users/caohao/ws2/pingora/pingora-core/src/listeners/mod.rs`

**Interfaces:**
- Produces: `pub(crate) async fn apply(stream: &mut Stream, enabled: bool, trusted: Option<&Arc<dyn ProxyProtocolTrust>>) -> Result<()>`
- Produces: `TcpSocketOptions.proxy_protocol_trusted_sources: Option<Arc<dyn ProxyProtocolTrust>>`

- [ ] **Step 1: Write the failing listener tests**

In `listeners/mod.rs`'s test module, beside `test_listen_tcp_proxy_protocol_v2`:

```rust
    #[derive(Debug)]
    struct TrustNobody;

    impl pingora_core_trust::ProxyProtocolTrust for TrustNobody {
        fn is_trusted(&self, _addr: &std::net::SocketAddr) -> bool {
            false
        }
    }

    #[derive(Debug)]
    struct TrustLoopback;

    impl pingora_core_trust::ProxyProtocolTrust for TrustLoopback {
        fn is_trusted(&self, addr: &std::net::SocketAddr) -> bool {
            addr.ip().is_loopback()
        }
    }

    /// THE security invariant: an untrusted peer's header must be ignored
    /// entirely — not consumed, not honoured — and the connection served as an
    /// ordinary one.
    #[tokio::test]
    async fn test_untrusted_peer_forged_header_is_not_honoured() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let addr = "127.0.0.1:7112";
        let mut listeners = Listeners::new();
        let mut sock_opt = TcpSocketOptions::default();
        sock_opt.proxy_protocol = true;
        sock_opt.proxy_protocol_trusted_sources = Some(Arc::new(TrustNobody));
        listeners.add_tcp_with_settings(addr, sock_opt);
        let listener = listeners.build(#[cfg(unix)] None).await.unwrap().pop().unwrap();

        let server = tokio::spawn(async move {
            let stream = listener.accept().await.unwrap();
            let mut stream = stream.handshake().await.unwrap();
            // The spoofed address must NOT have been adopted.
            let digest = stream.get_socket_digest().unwrap();
            let peer = digest.peer_addr().unwrap().as_inet().unwrap();
            assert_eq!(peer.ip().to_string(), "127.0.0.1");
            assert_ne!(peer.to_string(), "192.168.0.1:56324");
            // And the header bytes must reach the application verbatim: an
            // implementation that consumes-but-ignores is also wrong.
            let mut buf = [0u8; 16];
            stream.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf[..12], &[
                0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A,
            ]);
        });
        sleep(Duration::from_millis(10)).await;

        let mut client = TcpStream::connect(addr).await.unwrap();
        let mut header: Vec<u8> = vec![
            0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A,
            0x21, 0x11, 0x00, 0x0C,
        ];
        header.extend_from_slice(&[192, 168, 0, 1, 10, 0, 0, 2, 0xdc, 0x04, 0x01, 0xbb]);
        client.write_all(&header).await.unwrap();
        server.await.unwrap();
    }

    /// The mixed-mode case: a trusted peer that sends no header is served as an
    /// ordinary connection with its own address.
    #[tokio::test]
    async fn test_trusted_peer_without_header_is_served_directly() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let addr = "127.0.0.1:7113";
        let mut listeners = Listeners::new();
        let mut sock_opt = TcpSocketOptions::default();
        sock_opt.proxy_protocol = true;
        sock_opt.proxy_protocol_trusted_sources = Some(Arc::new(TrustLoopback));
        listeners.add_tcp_with_settings(addr, sock_opt);
        let listener = listeners.build(#[cfg(unix)] None).await.unwrap().pop().unwrap();

        let server = tokio::spawn(async move {
            let stream = listener.accept().await.unwrap();
            let mut stream = stream.handshake().await.unwrap();
            let digest = stream.get_socket_digest().unwrap();
            assert_eq!(
                digest.peer_addr().unwrap().as_inet().unwrap().ip().to_string(),
                "127.0.0.1"
            );
            let mut buf = [0u8; 5];
            stream.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"plain");
        });
        sleep(Duration::from_millis(10)).await;

        let mut client = TcpStream::connect(addr).await.unwrap();
        client.write_all(b"plain").await.unwrap();
        server.await.unwrap();
    }

    /// A trusted peer that does send one is honoured, as in mandatory mode.
    #[tokio::test]
    async fn test_trusted_peer_with_header_is_honoured() {
        use tokio::io::AsyncWriteExt;

        let addr = "127.0.0.1:7114";
        let mut listeners = Listeners::new();
        let mut sock_opt = TcpSocketOptions::default();
        sock_opt.proxy_protocol = true;
        sock_opt.proxy_protocol_trusted_sources = Some(Arc::new(TrustLoopback));
        listeners.add_tcp_with_settings(addr, sock_opt);
        let listener = listeners.build(#[cfg(unix)] None).await.unwrap().pop().unwrap();

        let server = tokio::spawn(async move {
            let stream = listener.accept().await.unwrap();
            let stream = stream.handshake().await.unwrap();
            let digest = stream.get_socket_digest().unwrap();
            assert_eq!(digest.peer_addr().unwrap().to_string(), "192.168.0.1:56324");
        });
        sleep(Duration::from_millis(10)).await;

        let mut client = TcpStream::connect(addr).await.unwrap();
        let mut header: Vec<u8> = vec![
            0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A,
            0x21, 0x11, 0x00, 0x0C,
        ];
        header.extend_from_slice(&[192, 168, 0, 1, 10, 0, 0, 2, 0xdc, 0x04, 0x01, 0xbb]);
        client.write_all(&header).await.unwrap();
        server.await.unwrap();
    }
```

Replace `pingora_core_trust::ProxyProtocolTrust` with the real path once Step 3 lands: `crate::protocols::l4::proxy_protocol::ProxyProtocolTrust`. Add `use crate::protocols::l4::proxy_protocol::ProxyProtocolTrust;` to the test module and drop the placeholder prefix.

- [ ] **Step 2: Run to verify they fail**

Run: `cd /Users/caohao/ws2/pingora && cargo test -p pingora-core --offline --lib listeners::test`
Expected: FAIL — no field `proxy_protocol_trusted_sources`

- [ ] **Step 3: Add the dispatcher**

In `proxy_protocol.rs`, above `apply_proxy_protocol`:

```rust
/// Apply inbound PROXY protocol to a freshly accepted stream.
///
/// * `enabled == false` — returns immediately, connection untouched.
/// * `trusted == None` — mandatory mode: every connection must begin with a
///   valid header (HAProxy `accept-proxy`).
/// * `trusted == Some(_)` — source-conditional mode: only a trusted peer is
///   examined, and then only if it actually sent a header. Untrusted peers are
///   served as ordinary direct connections and their bytes are never parsed, so
///   a forged header from one can never be honoured.
pub(crate) async fn apply(
    stream: &mut Stream,
    enabled: bool,
    trusted: Option<&Arc<dyn ProxyProtocolTrust>>,
) -> Result<()> {
    if !enabled {
        return Ok(());
    }
    if let Some(trust) = trusted {
        // The transport peer, before any override: this is the load balancer.
        let peer = stream
            .get_socket_digest()
            .and_then(|d| d.peer_addr().cloned())
            .and_then(|a| a.as_inet().copied());
        let Some(peer) = peer else {
            // Non-inet (UDS) has no load balancer in front of it.
            return Ok(());
        };
        if !trust.is_trusted(&peer) {
            return Ok(());
        }
        if !peek_is_proxy_header(stream).await? {
            return Ok(());
        }
    }
    apply_proxy_protocol(stream).await
}
```

- [ ] **Step 4: Thread it through the listener stack**

`listeners/l4.rs`, in `TcpSocketOptions` after `proxy_protocol`:

```rust
    /// Restrict [`Self::proxy_protocol`] to connections from these peers.
    ///
    /// `None` (default) keeps mandatory mode: every connection must carry a
    /// header. `Some(_)` switches to source-conditional mode, where untrusted
    /// peers are served as ordinary direct connections — this is how one
    /// listener serves both load-balanced and direct traffic without letting a
    /// direct client forge a source address.
    ///
    /// Note that in this mode a trusted peer which sends nothing is held until
    /// the downstream handshake timeout while the probe waits for 12 bytes, so
    /// it does not suit listeners fronting server-speaks-first protocols.
    pub proxy_protocol_trusted_sources:
        Option<std::sync::Arc<dyn crate::protocols::l4::proxy_protocol::ProxyProtocolTrust>>,
```

In `listeners/mod.rs`: add `proxy_protocol_trusted: Option<Arc<dyn ProxyProtocolTrust>>` to both `TransportStack` and `UninitializedStream` (use this exact name in both), populate it in `TransportStackBuilder::build` from `tcp_sock_opts()` alongside the existing `proxy_protocol` bool, and replace the handshake hook:

```rust
        self.l4.set_buffer();
        proxy_protocol::apply(
            &mut self.l4,
            self.proxy_protocol,
            self.proxy_protocol_trusted.as_ref(),
        )
        .await?;
```

- [ ] **Step 5: Run to verify they pass**

Run: `cd /Users/caohao/ws2/pingora && cargo test -p pingora-core --offline --lib`
Expected: PASS, 445+ tests, including the three new listener tests

- [ ] **Step 6: Commit**

```bash
cd /Users/caohao/ws2/pingora
git add pingora-core/src/protocols/l4/proxy_protocol.rs pingora-core/src/listeners/l4.rs pingora-core/src/listeners/mod.rs
git commit -m "feat(proxy-protocol): source-conditional mode

Untrusted peers are served as direct connections and their bytes are
never parsed as a header, so one listener can serve both load-balanced
and direct traffic without allowing a forged source address."
```

---

### Task 3: Preserve the load balancer's own address

**Files:**
- Modify: `/Users/caohao/ws2/pingora/pingora-core/src/protocols/digest.rs`
- Modify: `/Users/caohao/ws2/pingora/pingora-core/src/listeners/mod.rs` (assertion only)

**Interfaces:**
- Produces: `SocketDigest::raw_peer_addr(&self) -> Option<&SocketAddr>`

Verified: `SocketDigest` is struct-literal-constructed at exactly two places, both inside `digest.rs` (`from_raw_fd`, `from_raw_socket`); everything else goes through those constructors, and no Edgion site constructs one. `digest.rs` has no existing test module.

- [ ] **Step 1: Write the failing assertion**

In `listeners/mod.rs`, inside `test_listen_tcp_proxy_protocol_v2`'s server closure, after the existing `peer_addr` assertion:

```rust
            // The LB's own address stays recoverable behind the override.
            let raw = digest.raw_peer_addr().unwrap().as_inet().unwrap();
            assert_eq!(raw.ip().to_string(), "127.0.0.1");
            assert_ne!(raw.to_string(), "192.168.0.1:56324");
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd /Users/caohao/ws2/pingora && cargo test -p pingora-core --offline --lib test_listen_tcp_proxy_protocol_v2`
Expected: FAIL — no method named `raw_peer_addr`

- [ ] **Step 3: Implement**

In the `SocketDigest` struct:

```rust
    /// Transport-level peer address, before any PROXY protocol override.
    ///
    /// Equal to [`Self::peer_addr`] on an ordinary connection. On a listener
    /// with inbound PROXY protocol this stays the load balancer while
    /// `peer_addr` becomes the real client, which is what makes it possible to
    /// tell which hop produced an address when debugging a gateway chain.
    pub raw_peer_addr: OnceCell<Option<SocketAddr>>,
```

Add `raw_peer_addr: OnceCell::new(),` to both constructors, then:

```rust
    #[cfg(unix)]
    pub fn raw_peer_addr(&self) -> Option<&SocketAddr> {
        self.raw_peer_addr
            .get_or_init(|| SocketAddr::from_raw_fd(self.raw_fd, true))
            .as_ref()
    }

    #[cfg(windows)]
    pub fn raw_peer_addr(&self) -> Option<&SocketAddr> {
        self.raw_peer_addr
            .get_or_init(|| SocketAddr::from_raw_socket(self.raw_sock, true))
            .as_ref()
    }
```

- [ ] **Step 4: Run and commit**

```bash
cd /Users/caohao/ws2/pingora
cargo test -p pingora-core --offline --lib
git add pingora-core/src/protocols/digest.rs pingora-core/src/listeners/mod.rs
git commit -m "feat(proxy-protocol): keep the transport peer reachable as raw_peer_addr"
```

---

### Task 4: Wire source-conditional mode into Edgion

**Files:**
- Modify: `/Users/caohao/ws2/Edgion/edgion-resources/src/constants/annotations.rs`
- Modify: `/Users/caohao/ws2/Edgion/edgion-gateway/src/runtime/server/listener_builder.rs`

**Interfaces:**
- Consumes: `ProxyProtocolTrust`, `TcpSocketOptions.proxy_protocol_trusted_sources` (Tasks 1-2)
- Produces: annotation `edgion.io/proxy-protocol-trusted-cidrs`; `fn build_trusted_sources(raw: &str, gateway_key: &str, listener_name: &str) -> Option<Arc<dyn ProxyProtocolTrust>>`

The helper takes plain strings rather than a `ListenerContext` so it is testable: `ListenerContext` cannot be constructed in a unit test (`Listener`, `AccessLogger` and the CRD `EdgionGatewayConfig` have no `Default`, and building one is pure ceremony).

- [ ] **Step 1: Add the annotation constant**

In `annotations.rs`, after `ACCEPT_PROXY_PROTOCOL`:

```rust
    /// Restrict [`ACCEPT_PROXY_PROTOCOL`] to connections from these sources.
    ///
    /// Comma- or space-separated IP/CIDR list on `Gateway.metadata.annotations`,
    /// e.g. `"10.0.0.0/8, 192.168.1.5"` (a bare address is treated as /32 or
    /// /128). When present, a connection from any other address is served as an
    /// ordinary direct connection and its bytes are never parsed as a PROXY
    /// header — that is what lets one listener serve both load-balanced and
    /// direct traffic safely. When absent, every connection must carry a header.
    ///
    /// This list is about who may SPEAK proxy protocol (the load balancer) and
    /// is unrelated to `EdgionGatewayConfig.spec.realIp.trustedIps`, which is
    /// about which addresses count as proxy hops when walking X-Forwarded-For.
    /// The two normally hold different addresses; do not conflate them.
    ///
    /// Only the Gateway-wide key is read; there is no per-listener suffix, so
    /// all PROXY-enabled listeners of a Gateway share one trust list.
    pub const PROXY_PROTOCOL_TRUSTED_CIDRS: &str = "edgion.io/proxy-protocol-trusted-cidrs";
```

- [ ] **Step 2: Write the failing tests**

Add to `listener_builder.rs` beside the existing `#[cfg(test)] mod tests`:

```rust
#[cfg(test)]
mod proxy_protocol_trust_tests {
    use super::*;

    fn build(raw: &str) -> Option<Arc<dyn ProxyProtocolTrust>> {
        build_trusted_sources(raw, "edgion-test/gw", "pp")
    }

    #[test]
    fn cidr_and_bare_addresses_both_work() {
        let trust = build("10.0.0.0/8, 192.168.1.5").expect("matcher built");
        assert!(trust.is_trusted(&"10.1.2.3:1".parse().unwrap()));
        assert!(trust.is_trusted(&"192.168.1.5:1".parse().unwrap()));
        assert!(!trust.is_trusted(&"172.16.0.1:1".parse().unwrap()));
    }

    #[test]
    fn invalid_entry_fails_closed_to_mandatory_mode() {
        // A typo must not silently widen trust, and must not build a partial
        // matcher that would mis-attribute the ranges it dropped.
        assert!(build("10.0.0.0/8, not-an-ip").is_none());
    }

    #[test]
    fn empty_value_falls_back_to_mandatory_mode() {
        assert!(build("").is_none());
        assert!(build("  , ,").is_none());
    }

    #[test]
    fn match_all_prefixes_are_refused() {
        // `0.0.0.0/0` would trust every peer, reinstating the sniff-everything
        // mode the design rejects. Refusing keeps mandatory mode instead.
        assert!(build("0.0.0.0/0").is_none());
        assert!(build("::/0").is_none());
    }
}
```

- [ ] **Step 3: Run to verify they fail**

Run: `cd /Users/caohao/ws2/Edgion && cargo test -p edgion-gateway --offline --lib proxy_protocol_trust_tests`
Expected: FAIL — `cannot find function build_trusted_sources`

- [ ] **Step 4: Implement**

Add imports to `listener_builder.rs`:

```rust
use edgion_resources::matcher::ip_cidr_helpers::to_cidr_format;
use edgion_resources::matcher::ip_radix_tree::IpRadixMatcher;
use pingora_core::protocols::l4::proxy_protocol::ProxyProtocolTrust;
```

Then, after `accept_proxy_protocol`:

```rust
/// CIDR-backed [`ProxyProtocolTrust`], reusing Edgion's radix matcher so
/// pingora needs no CIDR parser of its own.
#[derive(Debug)]
struct CidrProxyProtocolTrust(IpRadixMatcher);

impl ProxyProtocolTrust for CidrProxyProtocolTrust {
    fn is_trusted(&self, addr: &std::net::SocketAddr) -> bool {
        self.0.contains(&addr.ip())
    }
}

/// Build the trusted-source matcher from a raw annotation value.
///
/// Returns `None` — meaning mandatory mode, a header required on every
/// connection — for an empty value, an unparseable entry, or a match-all
/// prefix. Falling back rather than building a partial matcher is deliberate:
/// a partial list would silently treat the ranges it dropped as untrusted and
/// mis-attribute their client addresses, which is far harder to notice than a
/// listener that plainly requires a header.
fn build_trusted_sources(
    raw: &str,
    gateway_key: &str,
    listener_name: &str,
) -> Option<Arc<dyn ProxyProtocolTrust>> {
    let mut builder = IpRadixMatcher::builder();
    for entry in raw.split([',', ' ']).map(str::trim).filter(|s| !s.is_empty()) {
        // A bare address needs a /32 or /128 before the matcher will take it.
        let normalized = match to_cidr_format(entry) {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(
                    gateway=%gateway_key, listener=%listener_name, entry=%entry, error=%e,
                    "invalid proxy-protocol-trusted-cidrs entry; requiring a header on every connection"
                );
                return None;
            }
        };
        if normalized.ends_with("/0") {
            tracing::warn!(
                gateway=%gateway_key, listener=%listener_name, entry=%entry,
                "proxy-protocol-trusted-cidrs must not match every address; requiring a header on every connection"
            );
            return None;
        }
        if let Err(e) = builder.insert(&normalized, "proxy-protocol-trusted") {
            tracing::warn!(
                gateway=%gateway_key, listener=%listener_name, entry=%entry, error=%e,
                "could not index proxy-protocol-trusted-cidrs entry; requiring a header on every connection"
            );
            return None;
        }
    }
    match builder.build() {
        Ok(matcher) => Some(Arc::new(CidrProxyProtocolTrust(matcher))),
        // An all-empty list yields EmptyTree; mandatory mode is the right answer.
        Err(_) => None,
    }
}

/// Peers allowed to speak PROXY protocol to this listener, or `None` for
/// mandatory mode.
fn proxy_protocol_trusted_sources(context: &ListenerContext) -> Option<Arc<dyn ProxyProtocolTrust>> {
    let raw = context
        .gateway_annotations
        .get(edgion::PROXY_PROTOCOL_TRUSTED_CIDRS)?;
    build_trusted_sources(raw, &context.gateway_key, &context.listener.name)
}
```

Extend `tcp_sock_opts`:

```rust
        let mut opts = TcpSocketOptions::default();
        opts.proxy_protocol = true;
        opts.proxy_protocol_trusted_sources = proxy_protocol_trusted_sources(context);
        opts
```

And extend the enablement log in `add_listener` — keep the existing message text so the Phase 1 integration test's grep for `"Inbound PROXY protocol enabled"` still matches, and keep the `EDGION_STREAM_PLUGINS` warning block that follows it:

```rust
        let trusted = context
            .gateway_annotations
            .get(edgion::PROXY_PROTOCOL_TRUSTED_CIDRS)
            .map(String::as_str)
            .unwrap_or("<none: header required from every peer>");
        tracing::info!(
            gateway=%context.gateway_key,
            listener=%context.listener.name,
            port=%context.listener.port,
            trusted_sources=%trusted,
            "Inbound PROXY protocol enabled; connections not starting with a valid header are rejected"
        );
```

- [ ] **Step 5: Run to verify they pass**

Run: `cd /Users/caohao/ws2/Edgion && cargo test -p edgion-gateway --offline --lib proxy_protocol_trust_tests && cargo clippy -p edgion-gateway --offline 2>&1 | grep listener_builder`
Expected: 4 tests pass, no clippy output for the file

- [ ] **Step 6: Commit**

```bash
cd /Users/caohao/ws2/Edgion
git add edgion-resources/src/constants/annotations.rs edgion-gateway/src/runtime/server/listener_builder.rs
git commit -m "feat(gateway): source-conditional inbound proxy protocol via trusted CIDRs

Requires pingora-core at <SHA from Task 2>."
```

---

### Task 5: Log the transport peer on the L4 paths

Separate from docs because it is a real multi-file change with its own build gate. In a chain this is what lets you tell which hop produced an address.

**Files:**
- Modify: `/Users/caohao/ws2/Edgion/edgion-gateway/src/routes/tls/proxy.rs` (`TlsContext` + its `new` + the log JSON at ~:456)
- Modify: `/Users/caohao/ws2/Edgion/edgion-gateway/src/routes/tcp/edgion_tcp.rs` (`TcpContext` + capture site ~:97)
- Modify: `/Users/caohao/ws2/Edgion/edgion-gateway/src/observe/logs/tcp_log.rs` (`TcpLogEntry` + `from_context`)

- [ ] **Step 1: Capture it beside the client address**

Wherever each path reads the digest for `client_addr` (`tls/proxy.rs:131-135`, `edgion_tcp.rs:97-101`), also read:

```rust
        let transport_peer = downstream
            .get_socket_digest()
            .and_then(|d| d.raw_peer_addr().cloned())
            .and_then(|a| a.as_inet().map(|i| i.ip().to_string()))
            .unwrap_or_else(|| "unknown".to_string());
```

Add `transport_peer: String` to `TlsContext` and `TcpContext`, thread it through `TlsContext::new` (currently 3 args, `pub(super)`), add it to `TcpLogEntry` and its `from_context`, and emit it as `"transport_peer"` next to `"client_addr"` in both log payloads.

- [ ] **Step 2: Verify the build and the log shape**

Run: `cd /Users/caohao/ws2/Edgion && cargo check -p edgion-gateway --offline --all-targets && cargo test -p edgion-gateway --offline --lib observe::logs::tcp_log`
Expected: clean build; existing tcp_log tests pass

- [ ] **Step 3: Commit**

```bash
cd /Users/caohao/ws2/Edgion
git add edgion-gateway/src/routes/tls/proxy.rs edgion-gateway/src/routes/tcp/edgion_tcp.rs edgion-gateway/src/observe/logs/tcp_log.rs
git commit -m "feat(observe): log the transport peer alongside the client address"
```

---

### Task 6: Documentation

Four passages in the existing docs become false once Tasks 2-4 land, and the trust model needs one addition the reviews surfaced.

**Files:**
- Modify: `/Users/caohao/ws2/Edgion/tasks/proxy-protocol-inbound.md`
- Modify: `/Users/caohao/ws2/Edgion/docs/en/dev-guide/annotations-guide.md`
- Modify: `/Users/caohao/ws2/Edgion/skills/02-features/05-annotations/references/annotations-metadata.md`

- [ ] **Step 1: Amend the statements that are now wrong**

In `tasks/proxy-protocol-inbound.md`: the trust section calls a CIDR allowlist "a natural later hardening step" (it now exists); the non-goals list "recovering the balancer's own address" (now `raw_peer_addr`); "never enable this on a listener that also serves probes" (no longer true under conditional mode — that is the feature's main use); and the listener-merging note now has to say the trust list, not just the on/off flag, is decided by whichever listener binds first.

In both annotation docs, "Connections without a valid header are rejected" must become conditional on whether trusted CIDRs are set.

- [ ] **Step 2: Add the resolution order**

Add to `tasks/proxy-protocol-inbound.md`:

```markdown
## Client address resolution

1. `raw_peer` — the transport peer. Always known; the load balancer on a
   proxy-protocol listener.
2. PROXY protocol (L4). If enabled, the peer is trusted and a header is present,
   `peer_addr` becomes the header's source. Otherwise `peer_addr` stays
   `raw_peer`.
3. X-Forwarded-For (L7), i.e. `realIp`. If `peer_addr` is in
   `realIp.trustedIps`, the XFF chain is walked right-to-left from it under
   `recursive` / `maxTrustedHops`. Otherwise `peer_addr` is the answer.

XFF wins when it is trustworthy, the PROXY address is the fallback, the
transport peer is the last resort. Every tier is gated, so a client cannot
promote itself: a forged XFF is only consulted when the peer is already a
trusted proxy, and a forged PROXY header is only parsed when the peer is in the
listener's trusted sources.

Note tier 3 is inactive in the plain load-balancer deployment. Once a header is
applied the anchor is the real client, who is not a trusted proxy, so the XFF
walk stops immediately. It matters in a gateway chain, where the PROXY source is
a peer gateway the operator has listed in `realIp.trustedIps`.

Two trust lists, never to be conflated:
- `edgion.io/proxy-protocol-trusted-cidrs` (L4) — who may SPEAK proxy protocol.
  Holds the load balancer.
- `EdgionGatewayConfig.spec.realIp.trustedIps` (L7) — which addresses count as
  proxy hops when walking XFF. Holds CDN egress ranges and peer gateways.

Forcing modes need no extra configuration: force-PROXY by keeping the PROXY
address out of `realIp.trustedIps`; force-XFF by not enabling proxy protocol on
that listener.
```

- [ ] **Step 3: Document the transparent-relay hazard**

Add to the trust section:

```markdown
### Only reachable through something that writes the header

A PROXY-accepting listener must be reachable only via a component that writes
the header **itself**. A transparent L4 relay does not qualify: it forwards the
client's first bytes verbatim, so a client can author the header and the relay
launders it into a trusted position. This applies to mandatory mode as much as
to conditional mode, and equally to HAProxy and nginx relays.

Concretely, do not put a gateway whose TCPRoutes are plain relays in front of a
PROXY-accepting listener, and do not list such a gateway in
`proxy-protocol-trusted-cidrs`. HTTPRoute hops are safe (they reparse the
request) and TLSRoute hops are safe (they parse the ClientHello and can emit
their own header via `edgion.io/proxy-protocol: v2`).
```

- [ ] **Step 4: Document the new annotation and the server-first caveat**

Add `edgion.io/proxy-protocol-trusted-cidrs` to the Gateway tables in both annotation docs, including: Gateway-wide only (no per-listener suffix); invalid entries and `/0` fall back to mandatory mode; and that conditional mode holds a silent trusted peer until the handshake timeout, so it does not suit listeners fronting server-speaks-first protocols.

- [ ] **Step 5: Commit**

```bash
cd /Users/caohao/ws2/Edgion
git add tasks/proxy-protocol-inbound.md docs/en/dev-guide/annotations-guide.md skills/02-features/05-annotations/references/annotations-metadata.md
git commit -m "docs(proxy-protocol): conditional mode, resolution order, relay hazard"
```

---

### Task 7: Two-hop chain integration test

Proves the motivating scenario: a client address obtained from a PROXY header at hop 1 is still the resolved client address at hop 2.

The chain is HTTPRoute → HTTPRoute, so the inter-hop mechanism is X-Forwarded-For. Hop 2 therefore does NOT get `accept-proxy-protocol` — nothing sends it a header, and an inert annotation would make the test pass for the wrong reason.

**Files:**
- Create: `/Users/caohao/ws2/Edgion/edgion-tests/integration/conf/ProxyProtocolChain/`
- Create: `/Users/caohao/ws2/Edgion/edgion-tests/integration/scripts/integration/test_proxy_protocol_chain.sh`
- Modify: `edgion-tests/integration/scripts/utils/start_all_with_conf.sh` (exclusion list)
- Modify: `edgion-tests/integration/scripts/integration/run_integration.sh` (registration)

- [ ] **Step 1: Determine the address the gateway actually sees**

HAProxy reaches the gateway through `host.docker.internal`, so the gateway observes a Docker NAT address, not `172.28.0.10`. A hardcoded `172.28.0.0/16` in the trusted list would classify HAProxy as untrusted and the test would fail for an unrelated reason.

Resolve it at setup: start the stack, send one probe through HAProxy, read the observed peer from the gateway log line, and template it into the fixture before starting the real phases. Simpler alternative, acceptable because this test asserts mechanism rather than policy: give hop 1 mandatory mode (no trusted-CIDR annotation) — conditional mode is already covered by Task 2's unit tests.

Take the simpler option unless the chain phases need conditional mode.

- [ ] **Step 2: Write the fixtures**

`ProxyProtocolChain/` with one resource per file, named `<Kind>_<namespace>_<name>.yaml`:
- `GatewayClass__public-gateway.yaml` — **with** `parametersRef` to `chain-egc`, unlike the Phase 1 fixtures. Without it `real_ip_extractor` is `None` and hop 2 silently reports hop 1's address.
- `EdgionGatewayConfig__chain-egc.yaml` — `spec.realIp.trustedIps` containing `127.0.0.0/8` (hop 1 as seen by hop 2), `recursive: true`.
- `Gateway_edgion-test_chain-gw1.yaml` — HTTP listener on 58530, `edgion.io/accept-proxy-protocol: "true"`.
- `Gateway_edgion-test_chain-gw2.yaml` — HTTP listener on 58531, no PROXY annotation.
- `HTTPRoute_edgion-test_chain-r1.yaml` → Service `chain-hop2`; `Service_edgion-test_chain-hop2.yaml` + `EndpointSlice_edgion-test_chain-hop2.yaml` → `127.0.0.1:58531`.
- `HTTPRoute_edgion-test_chain-r2.yaml` → Service `chain-backend`; `Service_edgion-test_chain-backend.yaml` + `EndpointSlice_edgion-test_chain-backend.yaml` → `127.0.0.1:30002`.

Neither HTTPRoute sets `hostnames` (the BusyBox probe sends `Host: <ip>:<port>`).

One gateway process serves both Gateways: `configure_listeners` iterates every Gateway and dedups only by `(addr, protocol)`, and loopback backends are permitted under `--integration-testing-mode`.

- [ ] **Step 3: Write the script**

Copy `test_proxy_protocol_inbound.sh` and change:
- `FIXTURE_DIR` → `ProxyProtocolChain`.
- Ports: controller 58520/58521/58522, gateway admin/probe/metrics 58523/58524/58525, hop 1 58530, hop 2 58531, HAProxy v2 58540, and `TS_HTTP_PORT=30002` (30001 is the Phase 1 suite's, so sharing it prevents concurrent runs).
- Delete the inherited `phase_per_listener_override` (no `plain` listener here) and `phase_missing_header` (hop 1 is mandatory-mode so it would still pass, but it duplicates Phase 1 coverage; drop it to keep the suite focused).
- Keep `phase_wire_versions` for the v2 leg only.
- Replace the proxied phases with:

```bash
phase_chain() {
    log_section "Phase: two-hop chain"
    local body xff xri
    body=$(probe_via "${LB_IP}:${LB_V2_PORT}")
    if [ -z "$body" ]; then
        fail "no response through the chain"
        docker logs "$LB_CT" 2>&1 | tail -10 || true
        tail -20 "$GW_LOG" || true
        return
    fi
    xff=$(header_value "$body" x-forwarded-for)
    xri=$(header_value "$body" x-real-ip)
    log_info "chain x-forwarded-for='${xff}' x-real-ip='${xri}'"

    # The load-bearing assertion: hop 2 RESOLVED the client, not merely listed it.
    case "$xri" in
        "$CLIENT_IP") log_ok "hop 2 resolved the real client ${CLIENT_IP}" ;;
        *) fail "hop 2 X-Real-IP is '${xri}', expected ${CLIENT_IP} — the chain lost the client" ;;
    esac
    case "$xff" in
        *"$CLIENT_IP"*) log_ok "X-Forwarded-For carries the client" ;;
        *) fail "X-Forwarded-For '${xff}' lost the client" ;;
    esac
    # Two hops must both appear; a single entry means hop 1 never forwarded.
    case "$xff" in
        *,*) log_ok "X-Forwarded-For shows both hops: ${xff}" ;;
        *) fail "X-Forwarded-For '${xff}' has one entry — hop 1 did not forward the chain" ;;
    esac
}
```

Also assert at startup that **hop 1** logged the enablement line, so a dropped annotation fails the test.

- [ ] **Step 4: Exclude the fixtures from the shared stack**

Add `ProxyProtocolChain` to the exclusion condition in `start_all_with_conf.sh` alongside `ProxyProtocolInbound`, for the same reason: a parametersRef-bearing GatewayClass of its own plus PROXY-requiring listeners.

- [ ] **Step 5: Run it**

Run: `cd /Users/caohao/ws2/Edgion && ./edgion-tests/integration/scripts/integration/test_proxy_protocol_chain.sh`
Expected: all phases pass, exit 0

- [ ] **Step 6: Prove the assertion can fail**

Temporarily remove `realIp` from the EGC fixture and re-run. Expected: the `x-real-ip` assertion FAILS with hop 1's address. Restore the fixture. This confirms the test detects the exact degradation the reviews found in the first draft.

- [ ] **Step 7: Register and commit**

```bash
cd /Users/caohao/ws2/Edgion
# add the docker-guarded invocation next to the Phase 1 one in run_integration.sh
git add edgion-tests/integration/conf/ProxyProtocolChain \
        edgion-tests/integration/scripts/integration/test_proxy_protocol_chain.sh \
        edgion-tests/integration/scripts/integration/run_integration.sh \
        edgion-tests/integration/scripts/utils/start_all_with_conf.sh
git commit -m "test: two-hop gateway chain resolves the client address at hop 2"
```

---

## Self-review notes

- **Coverage.** Conditional mode → Tasks 1, 2, 4. Security invariant → Task 2 Step 1 (the untrusted-forged-header test, which the first draft lacked entirely). Transport peer → Tasks 3, 5. Resolution order → Task 6 (documentation; verified the existing extractor already implements it in every branch). Chain → Task 7.
- **Ordering.** Task 1 alone does not break Edgion: `TcpSocketOptions` is `Default` + field assignment at Edgion's single construction site, and the new field defaults to `None`. But the `[patch]` is by path, not revision, so Tasks 4/5's commit messages must record the pingora SHA.
- **Dropped from the reviewed draft.** TCPRoute outbound PPv2, and with it the plan's only use for a three-hop test.
- **Deliberate non-goals.** No `real_ip_header: proxy_protocol` value (both forcing modes are already expressible). No v2 TLV parsing. No per-listener trusted-CIDR suffix. No preservation of the original destination across hops — each hop rewrites `local_addr` to its own upstream address, and no consumer reads downstream `local_addr` today.
- **Known limitation, documented not fixed.** Conditional mode holds a silent trusted peer until the 60s handshake timeout, so it does not suit listeners fronting server-speaks-first protocols. Unlike mandatory mode, where a peer that sends nothing is supposed to be rejected, this is a legitimate connection being penalised.
- **Known limitation, documented not fixed.** Listeners dedup by `(ip:port, protocol)` first-wins, so two Gateways colliding on a port decide each other's trust list. A controller-side consistency check is the fix if this bites.
