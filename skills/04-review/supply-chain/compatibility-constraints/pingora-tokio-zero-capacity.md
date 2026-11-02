---
name: pingora-tokio-zero-capacity
description: Use when updating Tokio or the Pingora fork's unbuffered stream constructors.
knowledge_type: compatibility-constraints
---

# Pingora unbuffered streams require Tokio before 1.54

## Conclusion and scope

The workspace pins Tokio to 1.53.3 while the Edgion Pingora fork resolves to
1aaa45cc70430bc782990bc20ddee3fef36d6b99. Its TCP, Virtual and Unix Stream
constructors in pingora-core/src/protocols/l4/stream.rs use
BufStream::with_capacity(0, 0, ...). Tokio 1.54 rejects zero read capacity and
panics during connection construction. This affects live proxy connections,
even when compilation and non-proxy transport tests pass.

## Rationale and evidence

Tokio's [upstream change](https://github.com/tokio-rs/tokio/pull/8494) and 1.54
changelog explicitly introduce this panic. The 1.54.1 BufReader constructor
asserts capacity > 0. A real OTLP integration run reached Gateway readiness,
then its HTTP service panicked on the first proxy connection before any
Collector export. The new edgion-gateway/tests/pingora_stream.rs exercises the
actual Pingora TCP conversion and bounded bidirectional transfer over loopback.

## Alternatives and limits

Keep other compatible dependency updates. An exact Tokio constraint prevents a
routine lock refresh from silently restoring the incompatible version. This is
an availability compatibility hold, not a claim that older Tokio fixes the
upstream zero-capacity AsyncBufRead behavior. Do not modify Cargo cache sources,
vendor a replacement, or add per-connection buffers as an upgrade workaround.

## Verification and re-evaluation

Run the focused pingora_stream transport target and actual proxy integration
tests when changing either dependency. The TCP test does not cover Virtual or
Unix sockets. Remove the pin only after the selected fork revision addresses
all three constructors and buffering transitions, and real TCP/TLS/proxy tests
pass with the proposed Tokio version. Fresh results belong in the active
dependency-maintenance task; compilation alone is insufficient evidence.
