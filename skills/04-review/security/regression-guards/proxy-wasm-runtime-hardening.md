---
name: proxy-wasm-runtime-hardening
description: Use when reviewing Proxy-Wasm module loading, callouts, OCI pulls, VM pooling, or test-engine lifecycle; records the 2026-07-30 hardening decisions and regression boundaries.
knowledge_type: regression-guards
---

# Proxy-Wasm Runtime Hardening

The 2026-07-30 review closed a cluster of related availability, isolation,
egress, and lifecycle findings. The fixes intentionally preserve streaming of
the main upstream response and keep request-path work bounded.

## Module publication and refresh

- OCI `Always` uses the last successfully published module while one
  single-flight background refresh runs. The effective pull policy belongs to
  the private configured runtime slot; there is no global mutable runtime cache.
- A refresh candidate remains tick-inactive until publication. Handoff cancels
  the old tick and publishes/activates the new entry. Cancellation takes effect
  at host await points or bounded guest boundaries; brief overlap is possible.
  In-flight request bindings finish on the old entry without rollback choreography.
- Preload and failed-load retry use atomic claims and a negative-cache deadline,
  preventing request-rate task and warning storms.
- URL and OCI retry backoff occurs after releasing the shared download permit;
  a slow retrying source cannot reserve concurrency while sleeping.

Do not replace last-known-good publication with synchronous request-path pulls,
and do not hold the download semaphore across retry backoff.

## Host ABI and callouts

- Pending callout requests charge their complete retained representation before
  ownership is committed. Before guest memory is copied, an allocation profile
  covers raw bytes, lossy UTF-8 expansion, decoded/final pair containers, URL
  normalization, pending state, and path normalization. Map decoding uses two
  allocation-free validation passes and no intermediate size vector. Failure
  leaves no partial pending request.
- Callout, URL, and OCI clients disable proxy-environment discovery. A guarded
  callout client construction failure is fail-close; it never falls back to an
  unguarded shared client.
- One absolute callout deadline covers semaphore acquisition, request sending,
  and response-body streaming.
- A cancelled asynchronous hook does not return a VM to the pool while its guest
  context remains open.

Do not count only callout body bytes, start the timeout after acquiring a
permit, or pool a VM whose callback context has not been closed.

Request-body replacement work is bounded per request at twice the resolved
physical request-body cap. This permits two complete same-size rewrites while
keeping cumulative work proportional to the operator-selected cap. The hot path
uses the already-resolved byte count from the request context; it performs no
size parsing or blocking lookup.

## OCI egress boundary

The operator-configured registry origin remains trusted for built-in address
classes. Gateway-configured blocked CIDRs apply to its literal or resolved IP. Cross-origin bearer-token realms and blob redirects are checked
at dial time: literal IP destinations pass through the shared IP classifier,
and each redirect hop is revalidated. Blob redirects allow at most three
followed hops; a fourth target is rejected before network activity.
Every redirect also rejects HTTPS-to-HTTP downgrade before the next request,
including same-host and same-port targets where the HTTP client could otherwise
retain an `Authorization` header.

Errors exposed to requests remain generic. Destination details belong only in
bounded cold-path diagnostics and low-cardinality metrics.

## Hot-path observability and lifecycle

Callout execution resolves egress without emitting request-path warnings. This
is now unconditional rather than a callsite choice: the opt-in metric-only
constructor selected here was removed once every `EgressResolver` became
tracing-free, reporting blocked addresses only through `record_egress_blocked` —
see [../observability/gw-plugin-outbound-01-egress-resolver-tracing-removed.md](../../observability/regression-guards/gw-plugin-outbound-01-egress-resolver-tracing-removed.md).
URL and OCI module fetching retains its own cold-path diagnostics.

Each Wasmtime engine owns a stoppable epoch ticker and joins it during drop.
Failed or injected engine construction starts no ticker. Tests must retain
engines with ordinary ownership; leaking engines to keep epochs advancing is
not accepted.

## Regression coverage

The guards include:

- single-flight refresh, negative-cache, and semaphore fairness tests;
- zero-network client-build failure and per-hop OCI redirect tests;
- same-host credentialed HTTPS-to-HTTP redirects with a zero-connection target;
- callout queue/send/stalled-body deadline tests;
- invalid-UTF-8 and high-pair-count allocation preflight tests;
- request-body mutation budget scaling and atomic rejection tests;
- real-guest cancellation and VM-pool reuse tests;
- owner cancellation, publication closure, and old/new tick retirement tests;
- repeated manager creation/drop tests that verify epoch ticker shutdown.

The Proxy-Wasm library test module passes both serially and with the default
parallel runner. The integration suite covers local replies, header mutation,
trap recovery, callouts, tick callbacks, shared data, and a performance smoke
test.

## Resolution references

- `WASM-HARD-01..14` — 2026-07-30.
