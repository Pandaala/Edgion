# TLS `SslCtx` owned handoff and exactly-once audit

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-30 |
| Status | pending |
| Type | refactor |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Replace the clone-based `SslCtx` ex-data handoff with an explicitly owned state transition and centralize store-failure audit ownership. The difficult part is preserving fail-closed TLS behavior, exactly-once `ssl.log`, and exactly-once `Box<SslCtx>` release across select, certificate, completion, and BoringSSL destruction callbacks. Do not combine this with rustls parity work.

## Requirements

- Make `store_ssl_ctx` consume an owned `SslCtx`, returning ownership to the caller on failure instead of cloning the full context.
- Define one owner for the terminal audit record on every store failure; `store_ssl_ctx` and its caller must never both emit for the same connection state.
- Preserve the current callback ordering: select owns SNI/matching/minimum-version decisions, certificate applies the selected policy and certificate, completion publishes the final metadata, and the destructor covers only genuine early aborts.
- Keep every callback panic-safe with `edgion_common::startup::catch_redacted_panic` and never include certificate identity, secret material, or dependency detail in a fail-close diagnostic.
- Preserve the supported BoringSSL-only Gateway boundary; rustls parity is out of scope.

## Confirmed current limitations

- `store_ssl_ctx` clones the complete `SslCtx` at both callback handoffs. A matched Gateway listener can therefore duplicate its resolved Secret vector and certificate/key byte buffers per handshake even though ownership could be moved.
- `store_ssl_ctx` emits an `ssl.log` entry on setter failure. The finalize path then marks the same context failed and its caller emits again, so an injected finalize-store failure can produce two terminal records.
- The pinned BoringSSL implementation normally reuses the already-sized ex-data slot during finalize, making that setter failure exceptional, but the Rust API contract still exposes failure and needs a tested single-owner policy.

## Scope

### In scope

- `edgion-gateway/src/tls/runtime/gateway/tls_pingora.rs`
- The `SslCtx` store, take, peek, finalize, completion, and destructor transitions.
- Focused fault-injection tests for ex-data setter failures and callback ordering.

### Out of scope

- Gateway rustls support or parity.
- Changes to certificate matching, FIPS policy, mTLS validation semantics, or log schema.
- General changes to BoringSSL or Pingora.

## Acceptance criteria

- A setter-failure seam proves that replacing a populated slot never leaves a freed pointer in ex-data.
- Clearing a populated slot is checked before `Box::from_raw`; a simulated clear failure leaves ownership with BoringSSL and cannot double-free.
- Success and every injected failure path release each allocated `Box<SslCtx>` exactly once.
- A state-matrix test covers select rejection, early abort before certificate callback, certificate application failure, finalize-store failure, normal completion, and missing-context fallback.
- Each covered connection path emits exactly one terminal `ssl.log` entry, including finalize-store failure.
- The owned handoff removes the full `SslCtx` clone from both the select-to-certificate and certificate-to-completion transitions.
- Targeted TLS tests, workspace check, Clippy, and repository guards pass.

## Related guidance

- `skills/00-design-rules.md`
- `skills/01-architecture/02-gateway/04-tls/SKILL.md`
- `skills/01-architecture/02-gateway/04-tls/01-downstream-tls.md`
- `skills/03-coding/01-log-safety.md`
- `skills/05-testing/SKILL.md`

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-30 | Defer the owned-handoff and log-ownership refactor from the file audit. | It crosses four callbacks and needs setter-failure injection plus an explicit state matrix; a local optimization without those proofs risks duplicate logs, leaks, or use-after-free. |
