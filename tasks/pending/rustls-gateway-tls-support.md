# Gateway Rustls Backend Support

## Meta

| Key | Value |
|-----|-------|
| Status | blocked |
| Type | product boundary / architecture |
| Priority | deferred |

## Prerequisite Gate

Do not start implementation until Pingora provides complete rustls support for
the downstream and upstream TLS capabilities Edgion requires, the exact Pingora
revision in use has been verified, and the user explicitly authorizes this task.
A compiling feature or a partial API is insufficient.

## Required Design Scope

- Define downstream certificate loading, dynamic SNI selection, EdgionTls,
  TLSRoute termination and custom handshake callback parity.
- Define upstream BackendTLSPolicy custom trust, client identity and additional
  SAN enforcement parity.
- Define failure policy, supported plugin behavior, diagnostics and security
  invariants without weakening the BoringSSL product path.
- Define clean-build, CI, packaging, Docker, deployment documentation and
  integration-test contracts for the complete backend.
- Keep backend-independent rustls clients outside this Gateway backend project.

## Current Decision

BoringSSL remains the only supported Gateway TLS backend. Rustls-only compile,
runtime, plugin, packaging and test gaps are not ordinary cleanup findings and
must not be fixed piecemeal outside this task.
