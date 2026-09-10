# Add ForwardAuth Denial Multivalue Header Integration Coverage

## Meta

- Status: pending
- Priority: P2
- Category: deferred integration coverage

## Scope and evidence

ForwardAuth copies authentication-service denial headers selected by
`decision.clientHeaders` to the downstream response. Repeated `WWW-Authenticate`
challenges and separate `Set-Cookie` fields must survive; they must not be reduced
to one value or comma-joined. The current integration assertion reads a single
challenge and does not establish repeated-field fidelity.

Recheck `edgion-gateway/src/plugins/http/forward_auth/plugin.rs`, its inline
denial-response mock tests, and the current header-preservation rules before adding
coverage. The source currently selects only the first auth response value; a
separate code correction is planned, so confirm that correction before asserting
the intended behavior end to end.

## Acceptance

- Use the shared integration topology and a real auth denial returning two distinct
  challenges and two separate cookies, including a cookie update or clearance.
- Assert all repeated response fields and per-name order at the downstream client
  using a multivalue-aware API, not a single-value lookup.
- Verify duplicate or case-equivalent configured client header names do not multiply
  the values and unselected auth headers are not forwarded.
- Preserve denial status/body behavior and success-side upstream identity semantics.
- Keep synchronization deterministic and avoid credential-bearing logs.
- Do not alter production behavior or existing integration fixtures without the
  separately authorized test scope. This task is deferred; no integration addition
  or modification is authorized by the current code-quality work.
