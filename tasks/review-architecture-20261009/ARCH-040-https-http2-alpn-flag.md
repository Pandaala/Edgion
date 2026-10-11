---
id: ARCH-040
status: candidate
kind: behavior-defect
priority: P2
partition: PART-377
analyst: /root/analyze_outbound
independent_verification: pending
---

# Pass the resolved HTTPS HTTP/2 flag to the TLS settings owner

## Current ownership and trigger

GatewayBase::parse_enable_http2_annotation resolves absence to enabled and
false/0/no/off, case-insensitively, to disabled. configure_listeners passes that
boolean through ListenerContext; HTTPS dispatch reaches add_http_listener with
context.enable_http2. EdgionHttpProxy retains it and plaintext h2c honors it.

The HTTPS arm nevertheless passes literal true to
TlsCallback::new_tls_settings_with_callback at listener_builder.rs:277-282.
That constructor immediately enables h2 at tls_pingora.rs:470-485. Its later
conditional enable_h2 call cannot undo the first enablement. The locked supported
BoringSSL Pingora settings default h2 off and enable it by installing ALPN H2H1.

A single ordinary HTTPS Gateway with edgion.io/enable-http2: "false", usable
serving TLS identity and a client offering h2 receives settings preferring h2.
No shared port or reload is required. pg_request_filter:184-190 concurrently uses
EdgionHttpProxy.enable_http2=false to reject native gRPC with Http2Required.
Transport advertisement and the request policy therefore disagree. The annotation
reference explicitly says the flag controls HTTPS ALPN. This is static evidence;
no handshake or production incident is asserted.

## Bounded improvement and alternative

Pass the resolved enable_http2 to the existing TLS settings constructor, giving
that owner the HTTPS ALPN decision. Remove the redundant later enable_h2 call if
it contributes no behavior. An explicit override after constructing enabled TLS
settings is possible but retains two owners for the same decision.

Keep HTTP h2c's distinct server options, TLS-to-TCP's explicit false, process-wide
downstream H2 limits, handshake offload admission, certificate selection/mTLS and
canonical SSL audit ownership. No configuration, factory, mutable session state,
lock, allocation or observability mechanism is needed. BoringSSL remains the sole
supported backend; no rustls support gap is part of this finding.

## Invariants and future verification

Verify actual supported BoringSSL ALPN negotiation for disabled/enabled/default
ordinary HTTPS, including falsy annotation parsing. Preserve h2c enable/disable
and TLS-to-TCP without HTTP ALPN. HTTP/1.1 gRPC-Web remains available when frontend
HTTP/2 is disabled; module registration must not be gated on this flag. Retain
native-gRPC failure semantics. Keep startup-only listener transport policy and
captured dynamic plugin/route ownership unchanged.

Inspected existing tests cover H2 windows/stream bounds, offload and request/module
classification; they do not establish disabled HTTPS ALPN negotiation. No tests,
builds, network or measurements were run. Relevant originals include annotations,
Gateway schema, grpc-web-independent-of-frontend-http2 and downstream handshake
budget/cancellation. Existing ARCH findings do not own this constant argument.
Independent verification is pending.
