# External ExtProc integration fixture

This standalone Go program implements the Envoy
`envoy.service.ext_proc.v3.ExternalProcessor` service with official generated
types from `github.com/envoyproxy/go-control-plane/envoy` v1.37.0. It provides
independent wire-interoperability evidence for Edgion's ExtProc client. It is a
test fixture, not a production processor.

The integration runner builds and starts this program only for qualifying
`--full-test` ExtProc runs. Both listeners must be explicit literal IPv4
loopback addresses:

```bash
./target/debug/ext_proc_external_processor \
  --grpc-listen 127.0.0.1:30044 \
  --admin-listen 127.0.0.1:30045
```

Optional independent TLS listeners are enabled explicitly:

```bash
./target/debug/ext_proc_external_processor \
  --grpc-listen 127.0.0.1:30044 \
  --admin-listen 127.0.0.1:30045 \
  --tls-listen 127.0.0.1:30046 \
  --mtls-listen 127.0.0.1:30047 \
  --tls-material-dir /absolute/workdir/ext-proc-tls
```

All four listen flags accept only a literal `127.0.0.1` address. The material
directory is required when either secure listener is enabled and must be an
absolute path. It is created with mode `0700`; every generated PEM or YAML file
has mode `0600`.

Each startup creates a private server CA, server leaves A/B, a private client
CA, and client leaves A/B. The material directory contains:

- `server-ca.crt`, `client-ca.crt`
- `server-{a,b}.crt`, `server-{a,b}.key`
- `client-{a,b}.crt`, `client-{a,b}.key`
- `ext-proc-external-client-initial.yaml` using client A
- `ext-proc-external-client-updated.yaml` using client B
- `ext-proc-external-server-ca.yaml` containing the public server CA

Both client Secret manifests have namespace `edgion-default`, name
`ext-proc-external-client`, type `kubernetes.io/tls`, and base64-encoded
`tls.crt`/`tls.key`. Certificate material is random per fixture startup.
The server CA manifest has namespace `edgion-default`, name
`ext-proc-external-server-ca`, type `Opaque`, and only base64-encoded
`data.ca.crt`. The server CA private key is never written to disk.

Every processed request must include these headers:

- `x-ext-proc-case-id`: 1–64 ASCII characters matching
  `[A-Za-z0-9][A-Za-z0-9._-]*`.
- `x-ext-proc-test-mode`: exactly `interop`, `immediate`, `grpc-immediate`,
  `disabled-immediate`, `mode-reduce-none`, `mode-forbidden-expand`,
  `trailer-mutate`, `no-trailer`, `long-idle`, `barrier-headers`, or
  `barrier-request-body`.
- `x-ext-proc-barrier-id`: required only for the two barrier modes and subject
  to the same grammar and length limit as the case ID.

`interop` applies the fixed header and body mutations used by the Rust
integration assertions. `immediate` returns a fixed 403 response.
`grpc-immediate` carries the official exact gRPC status field.
`disabled-immediate` returns the same ordinary ImmediateResponse to a route
that disables it. `mode-reduce-none` requests the allow-listed request-body
reduction, while `mode-forbidden-expand` requests an unlisted acquisition
change. `trailer-mutate` returns a fixed mutation for a real response trailer;
`no-trailer` is used to prove clean EOF without a fabricated trailer message.
`long-idle` waits exactly three seconds before the request-header verdict.
`barrier-headers` pauses before the first request-header verdict, while
`barrier-request-body` pauses before the first request-body verdict. A stream
uses its barrier at most once. Barrier waits have a fixed 25-second hard
deadline; request-controlled delay values are intentionally unsupported.

## Admin API

The separate loopback-only HTTP listener exposes:

- `GET /readyz`
- `POST /v1/cases/{id}/reset` with an empty body
- `GET /v1/cases/{id}`
- `GET /v1/barriers/{id}`
- `POST /v1/barriers/{id}/release-one` with an empty body
- `POST /v1/barriers/{id}/release-all` with an empty body
- `POST /v1/barriers/{id}/reset` with an empty body
- `GET /v1/tls`
- `POST /v1/tls/{tls|mtls}/rotate/{a|b}` with an empty body

A barrier snapshot has this stable shape:

```json
{
  "barrier_id": "example",
  "waiting": 1,
  "arrivals": 1,
  "releases": 0,
  "released_all": false,
  "waiter_capacity": 96,
  "fixture_stream_capacity": 96
}
```

`release-one` wakes the oldest current waiter and returns HTTP 409 if none is
waiting. `release-all` is idempotent, creates an open barrier when necessary,
wakes every current waiter, and leaves the barrier open for later arrivals
until reset. This makes failure cleanup safe even when a request has not yet
reached the fixture. Reset is idempotent but returns HTTP 409 while the barrier
still has waiters.

Case snapshots store only bounded phase transitions, message counts, response
status codes, byte counts, SHA-256 digests, EOS counts, fixed response-trailer
presence booleans, and stream lifecycle flags. Each stream includes `request_header_messages`,
`response_header_messages`, and `response_statuses`. The status array records
the numeric `:status` value from every `response_headers` message in arrival
order. Repeated messages in one phase increment the corresponding counters
without duplicating the phase name, so two consecutive response-header
messages may produce one phase transition and two status observations. Each
header/trailer direction is limited to 64 messages or fields; excess messages fail the Process
stream with gRPC `ResourceExhausted`. Each body direction is limited to 8,192
messages and 32 MiB of observed bytes. The fixture never stores request header
values, arbitrary trailer values, or body contents. Active cases are never evicted. Resetting an active
case returns HTTP 409.

The fixture admits at most 96 concurrent gRPC streams and 96 waiters per
barrier. The higher fixture stream cap intentionally leaves headroom for tests
of Edgion's separate 64-session target limit. At most 128 barrier IDs may exist;
tests must reset completed barriers when they no longer need them.
`GET /readyz` also returns `fixture_stream_capacity: 96` so a saturation test
can fail closed if it is accidentally pointed at a fixture that cannot observe
the 65th stream. It reports `tls_capable`, `mtls_capable`, `tls_ready`, and
`mtls_ready`; the readiness booleans are true only for listeners enabled and
successfully bound during this process startup.

## TLS identity and rotation

`GET /v1/tls` returns only public SHA-256 fingerprints, current A/B generation,
listener readiness, active connection counts, cumulative `accepted_connections`
and `rpc_calls`, and the fixture stream capacity. TCP accepts are counted before
the TLS handshake, including failed handshakes; RPCs include calls on reused
connections and do not depend on request headers. Neither counter is reset by
case reset or certificate rotation. The TLS policy recovery scenario uses their
deltas to distinguish no-dial/no-call rejection from handshake or RPC failure.
It never returns PEM or private-key content. Stream observations add:

- `transport`: `plain`, `tls`, or `mtls`
- `tls_generation`: the generation selected by that accepted connection
- `server_leaf_sha256`: the exact server leaf used by its handshake
- `peer_leaf_sha256`: the exact verified client leaf for mTLS, otherwise absent

These values are captured from the accepted TLS connection before gRPC serves
it; they are not inferred from a global current-generation label.

Rotation is synchronous. Changing A to B, or B to A, switches the selected leaf
and closes only active connections from the previous generation, forcing the
next Process stream to handshake. Rotating to the already-current generation is
idempotent and retains its connections. The endpoint accepts no PEM or request
body. TLS and mTLS listeners each advertise 96 HTTP/2 streams, while all plain,
TLS, and mTLS Process streams share one bounded 96-slot application limit.

## Local verification

Go 1.24 or newer is required. Disable automatic toolchain downloads:

```bash
GOTOOLCHAIN=local go mod verify
GOTOOLCHAIN=local go test -mod=readonly ./...
GOTOOLCHAIN=local go build -mod=readonly ./...
```

`config.yaml.tmpl` is not loaded by the normal recursive YAML loader. The
full-test startup path renders `__EXTERNAL_EXT_PROC_TARGET__` into a work
directory and applies that rendered file explicitly.
