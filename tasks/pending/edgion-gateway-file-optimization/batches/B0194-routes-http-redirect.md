# B0194 — HTTP Redirect Service

## Scope

- Analysis baseline: `2f7a26abf0e47de4037f4f7545887ce912d1c59d`; revalidated at `8f1530ab6490a90346fd5eab31887ac3249538f1`
- Target blob: `1f28570aa504d1dfddb149a977c16a614ace5fec`
- Pipeline state: ready to commit
- Active anchor crate: `edgion-gateway`
- Target and exact technical allow-list: `edgion-gateway/src/routes/http/redirect_http.rs`, including inline tests only
- Existing `edgion-tests/**` files and protected user paths remain frozen.

## Handoff

- Analysis: change; standard. The dedicated redirect access log stores the raw `path_and_query` in `path` and a query-bearing `redirect_url`; both serialize without the shared sensitive adapter, exposing query credentials or PII by default.
- Required behavior: preserve the original raw path/query bytes in the wire `Location`, but retain only query-free path and redirect target in the logging context. Apply the canonical sensitive serializer to logged path/URL and the optional sensitive serializer to dynamic error detail.
- Required preservation: duplicate/missing Host 400, IPv4/IPv6 and configured-port authority construction, percent encoding and query on the wire, terminal 301/EOS/connection-close behavior, binary-owned `AccessLogger`, and no added request-path observability.
- Tests: default masking, absence of query tokens from serialized log fields, sensitivity replace/wrap/off behavior, and unchanged wire target construction. Fresh baseline 6/6.
- No new integration issue: same-file serialization tests deterministically cover the security property, while frozen integration coverage already proves wire query and IPv6 preservation.
- Previous batch: `8f1530ab`; never push.
- Resume: HEAD `7ac8a760` includes external OCI and controller ACME work. A read-only diagnosis verified the sole integration fingerprint change belongs to committed `7534a9bf`; only that manifest entry was reconciled under the user's external-change instruction. Two externally added Gateway OCI test files were added to pending coverage.
- Independent review requested correction of process-global masking-mode test isolation and avoidable request path/query ownership; correction is in progress.
- Wire verification corrected case-sensitive header-name assertions. Independent diagnosis also confirmed pre-existing Pingora keepalive rewrites of `Connection: close`; D0108 owns the policy decision. The wire fixture requests closure explicitly and makes no claim of server-forced closure.

## Outcome

- Logged path and redirect target exclude query data and use canonical sensitive serializers; optional error detail is masked. Wire Location retains the original path/query bytes. Borrowed path/query avoids a new temporary allocation.
- Independent correction review: LGTM, all Must Fix and Should Fix items resolved. Global-mode assertions run in exact-test subprocesses and cannot race unrelated test suites.
- Target tests: 9/9. Complete HTTP route module: 390/390 with default parallel execution.
- Verification passed: format, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff and frozen-path guards. Clippy exited zero with warnings.
- Commit: pending; never push.
