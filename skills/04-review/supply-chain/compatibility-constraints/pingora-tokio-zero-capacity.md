---
name: pingora-tokio-zero-capacity
description: Use when updating Tokio or the Pingora fork's L4 stream buffer constructors.
knowledge_type: compatibility-constraints
---

# Tokio 1.54+ requires the Pingora fork's zero-capacity read-buffer fix

## Conclusion and scope

Tokio 1.54 panics when `BufReader` or `BufStream` is created with zero
capacity. Pingora's TCP, Virtual and Unix L4 `Stream` constructors in
`pingora-core/src/protocols/l4/stream.rs` request a zero read capacity, so any
fork revision without upstream commit `572e044` ("Keep zero-capacity L4 read
buffers working on tokio 1.54"; the same change is `5b30d71` on Cloudflare `main`,
seen via the fork checkout's `upstream` remote) panics on the
first proxied connection under Tokio 1.54 or later. Compilation and non-proxy
tests still pass on such a combination.

The locked fork `edgion_v0.9.1@e5f4287be713d01ffd444544251bf75c9d00062a`
contains `572e044`: all three constructors and `set_buffer` go through a private
`BufferedStream` that omits the `BufReader` when the read capacity is zero.
The workspace therefore requires `tokio = "1.54.1"` (a caret floor, not an exact
pin) and resolves 1.54.1.

## Rationale and evidence

Tokio's [upstream change](https://github.com/tokio-rs/tokio/pull/8494) and the
1.54.0 changelog introduce the panic. Before the fork fix, a real OTLP
integration run reached Gateway readiness and then panicked on the first proxy
connection; the workspace held Tokio at exactly 1.53.3 from that discovery
until 2026-11-06 (fork `1aaa45cc` and earlier used
`BufStream::with_capacity(0, 0, ...)`).

Lifting the hold on 2026-11-06 was verified as follows:

- Source audit at `e5f4287`: the three `From` constructors and `set_buffer`
  use `BufferedStream::with_capacity`; `into_inner` drops buffered bytes exactly
  as the former `BufStream` did. No other fork or Edgion production code builds
  a zero-capacity `BufReader`/`BufStream`; third-party crates (hyper, h2, tonic,
  kube and others) were not audited. The other 1.54 zero-value panics
  (`duplex`/`simplex`, `copy_bidirectional_with_sizes`,
  `max_io_events_per_tick`, `File::set_max_buf_size`) are unused or receive
  nonzero constants.
- On Tokio 1.54.1 the gateway transport targets `pingora_stream`,
  `listener_tls_tcp_alpn`, `tls_passthrough_clienthello`,
  `downstream_h2_config`, `reqwest_san_transport`, `ai_unary_hold_transport`
  and `wasm_stream_transport` (with `test-util`) all passed.
- A full default `run_integration.sh` run on Tokio 1.54.1 reported 285 passed,
  10 skipped and 1 failure; the gateway log contained no panic. The single
  failure (`Dyn_RetainedListenerHandles`) was a test sampling race unrelated to
  Tokio, see
  [retained listener manager coverage](../../listeners/regression-guards/retained-listener-manager-clearing-wire-coverage.md).
  The OTLP integration that originally exposed the panic, the `otlp_transport`
  target, and the `--full-test`-only `HTTPRoute/Transport` item were not re-run;
  every proxied connection in the 285 passes still constructs an L4 `Stream`.

## Alternatives and limits

The caret floor keeps routine `cargo update` on the latest 1.x while preventing
a resolution below the verified version (and below the RUSTSEC-2026-0386 fixed
1.53.3). An exact pin is no longer needed. Do not modify Cargo cache sources,
vendor a replacement, or add per-connection buffers as a workaround. A Pingora
fork rebase that drops `572e044` reintroduces the panic.

## Verification and re-evaluation

Re-run the transport targets above and real proxy integration whenever the
Pingora fork's L4 stream code or the Tokio minor version changes. The
`pingora_stream` target covers only TCP; Virtual and Unix constructors are
covered by source inspection. The verification above is macOS-local; Linux CI
results were not separately recorded.
