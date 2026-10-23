# Evaluate lazy SSL failure snapshot construction

Status: deferred user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `915d43d4a`. No runtime implementation selected.

## Evidence

The mTLS verify callback claims its terminal audit in emit_mtls_failure before
calling log_mtls_failure. With a non-null SSL pointer and available ex_data,
peek_tls_conn_meta clones the stored TlsConnMeta, then the caller copies the
failure reason into the snapshot. Only afterward does log_ssl consult SSL_LOGGER.
A disabled/uninitialized logger therefore still incurs snapshot/reason preparation.
The snapshot's populated strings and LogBuffer are owned copies; do not assume
all possible metadata fields are populated during certificate verification.

SslLogger::log already calls try_reserve before JSON serialization. Full/closed
queues avoid serialization but not the earlier snapshot. Successful admission
serializes the snapshot synchronously into an owned String, with the existing
{} fallback, and sends it using the reserved permit. This proposal is not a new
reservation-before-serialization optimization; that part is already implemented.

## Decision and ownership boundaries

- Retain the current small failure-path interface if measured cost is negligible.
- Evaluate a narrow lazy-producer interface inside the canonical SSL logger that
  avoids preparation when no logger exists. If it also avoids preparation on
  saturation, preserve context validation and dropped-counter semantics.
- Evaluate a synchronous borrowed snapshot with an error-field override only if
  it can reuse the canonical serialization contract without parallel schemas or
  unsafe references escaping the SSL owner. Do not mutate the original ex_data
  metadata merely to avoid a copy.

Missing SSL, an uninitialized ex_data index, or missing SslCtx currently yields
no record and no dropped increment. A full/closed channel with valid metadata
increments ssl_log_dropped. Reserving/counting before establishing metadata
presence would conflate these paths. Preserve the audit claim before emission:
disabled logging or failed admission must not reopen the claim and create a
second handshake writer. Capture logger identity coherently; avoid a separate
availability test followed by a second global lookup, and explicitly review
startup initialization timing if lookup moves earlier.

All native pointer access and borrowed serialization must finish synchronously
inside the valid SSL callback lifetime. Only the final owned String may cross the
channel. Preserve the redacted panic boundary, failure/rejection behavior, existing
sensitive formatting and serde field omissions. Public SNI and diagnostic behavior
must not be accidentally remasked, and client_cert_info must remain omitted.
No blocking send, extra tracing, new metrics or logging fallback is authorized.

A producer API does not by itself remove formatting that occurs earlier in
verify_callback_impl or allowlist failure construction. Measure snapshot cloning,
reason copying and JSON separately; do not claim all failure-path allocations
are eliminated. This is distinct from the disabled listener TLS logging gate and
its separate direct-serialization task.

The existing decision in
`skills/04-review/observability/intentional-design/protocol-logger-oncelock-pattern-not-extracted.md`
rejects a generic ProtocolLogger/macro framework. This candidate must stay within
the SSL logger's sync callback/queue contract and does not reopen that decision.

## Acceptance before implementation

Use isolated logger state and an in-memory bounded channel to compare disabled,
enabled, full and closed cases, with present/missing context, duplicate terminal
audit claims and failure/panic paths. Assert preparation invocation counts,
serialized bytes/omissions, sensitive modes, exact drop increments, unchanged
original metadata, no duplicate emission and released permits on producer failure.
Test initialization timing assumptions explicitly where the selected design relies
on them. Preserve null/index-missing behavior without dereferencing invalid pointers.

Existing mTLS callback and tls_pingora tests cover verification/audit and ex_data
lifecycle; TLS sensitive-log tests also exercise TlsConnMeta serialization. They
do not directly prove SSL queue admission parity. Add focused tests for the selected
interface and measure preparation cost under realistic failure rates before making
performance claims. Freeze an exact IMPL scope and obtain independent reviews.
