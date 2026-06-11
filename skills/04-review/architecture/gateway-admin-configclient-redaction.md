# Gateway admin `/configclient` secret redaction

**Decision:** The gateway admin API `/configclient/{kind}` endpoints stay always-mounted
(loopback by default), but every resolved/inline secret they serialize is mandatorily
redacted to `"[redacted]"`.

**Mechanism:** `edgion-resources/src/redact.rs` provides a gateway-only thread-local RAII
`AdminRedactGuard` and two generic serde `serialize_with` adapters (`redact_opt`,
`redact_required`). Secret-bearing fields on the synced resource structs carry the
adapter; the guard is constructed inside the synchronous `list_client_resources`
(`src/core/gateway/api/mod.rs`), covering both the list and get-by-name paths. Default
(no guard) emits real values, so the controller's conf_sync transport serialization
— a different process that never constructs the guard — is unaffected.

**Why independent from access-log masking:** access-log masking (`edgion-resources/src/sensitive.rs`,
`SENSITIVE_MODE`) is operator-configurable (Off/Replace/Wrap). Admin redaction is a
mandatory security invariant with no config knob; reusing `SENSITIVE_MODE` would let an
operator disable security redaction by turning off log masking. They share the serde-adapter
idiom but use separate ambient state.

**Plan B (keep `/configclient` mounted):** gating `/configclient` behind integration-testing
mode would 404 the shipped `edgion-cli get <kind> --target client` debug path. Since redaction
makes the endpoint leak-free, it stays mounted; only the test-only `/api/v1/server-info` moved
into the testing-gated router.

**Invariants (silent-failure guardrails):** keep the `/configclient` serialization path
single-threaded (the guard is a thread-local — no rayon/spawn_blocking), and never add an
overriding `impl Serialize` / `#[serde(into)]` on a type above a secret field.

Spec: `docs/superpowers/specs/2026-06-07-gateway-admin-api-harden-design.md`.
Sibling note: `gateway-admin-loopback-default.md`.
