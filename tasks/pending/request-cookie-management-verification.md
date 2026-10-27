# Request Cookie implementation verification

## Outcome and authority

Completed and independently accepted against baseline
`6069963764e2435aaa834aaa8aa56028b0dec67a`. The user authorized implementation,
all related paths and multiple independent review/fix rounds. No commit or push.
Unrelated `impl/` content remains untouched.

The selected auth-01 Cookie finding is closed as **measured retention of borrowed
survivor Vec storage, canonical reads/deletion, no-match no-render, and safe failure
propagation**. It is not closed on a claim that all intermediate storage disappeared.
The original task file is removed after acceptance; the Query portion was already
completed separately. The approved [plan](request-cookie-management-plan.md) and
[manifest](request-cookie-management-manifest.md) record scope and amendments.

## Implemented contract

Pingora's live HeaderMap is authoritative. No production Cookie cache, context
state, deferred Cookie rendering, dependency, schema or configuration is added.
Unused raw Cookie fields pass through without parsing or assembly. Explicit named
reads use the canonical parser; borrowed consumers avoid copied values. Repeated
named reads still scan: this implementation does not promise parse-once behavior.

Nonempty deletion validates all field lines before publication and removes every
matching raw, case-sensitive name. Matched deletion merges surviving trimmed
segments with `; `; unmatched deletion preserves raw fields without output assembly.
An empty removal-name list is a no-op; a list containing an empty name is distinct.
Selected duplicate names, bare names, values with equals signs, quotes, percent
bytes, empty values, multiple fields and whitespace have explicit regressions.

Lookup retains HeaderValue text eligibility and opaque-field ordering. Editing
accepts UTF-8 and fails atomically on non-UTF-8 fields: fixed 400 for unsanitizable
input, fixed 500 for internal publication failure. KeyAuth/JwtAuth and all five
OIDC continuation paths propagate these via the existing runtime dispatcher.
Successful mutations update current headers immediately for plugins, signatures,
webhooks, mirror and main forwarding. No-match removal cancels a prior deferred
Cookie override when current fields exist. Empty lists and absent fields do not.

The all-path audit covered generic setters/appends/removals, protected-header and
dye operations, whole-body header commits, Query projections, AI baseline restore,
provider writes, HMAC, webhook/templates, ExtProc, Wasm, WAF, response-cache
eligibility, access-log parsing, session persistence and LB. LB retains first-field
scope; WAF leaf grammar and response Set-Cookie remain distinct owners.

## Independent reviews and corrections

- Exact API/manifest and subsequent integration/runtime/K8s amendments received
  independent core and transport review before implementation.
- Round 1 blocked acceptance on invalid conversion-test fixtures, missing actual
  auth-callback failure coverage, missing independent old-semantics oracles and
  missing K8s support for the shared suite. All were corrected.
- Round 2 core/transport reviews passed actual planner allocation evidence,
  KeyAuth/JWT errors, OIDC five-branch markers, runtime stop witnesses and
  HTTP/gRPC error mapping, expanded benchmark and current-header export paths.
- Round 3 transport review passed K8s endpoint inventory and per-Pod aggregation.
  A fresh independent final reviewer found no further production issue and
  verified the final runtime logs and closure disposition.

Two pre-existing fixture defects surfaced in expanded coverage. ExtProc expected
`/api` for `/api?`, although baseline Query preserved the explicit delimiter. An AI
provider fixture omitted entry-point Query capture before using the real adapter.
Independent review approved test-only corrections; production Query/AI behavior
and all other assertions remain unchanged.

## Runtime verification

**2,838 unique scoped unit tests passed**, deduplicated across final selections;
16 pre-existing OIDC tests remain ignored. This is not `cargo test --all`.
Core/caller/transport selections include canonical Cookie, auth common, adapter,
runtime dispatcher, KeyAuth, JwtAuth, OIDC, CSRF, DSL, templates, outbound,
session persistence, selector, access log, ExtProc, HMAC and webhook. Expanded
selections include AI, WAF, mirror, response cache and Wasm. The final changed
fixture rerun passed AI 594, runtime 552 and ExtProc 147, among others.

The parallel Wasm run exhausted its existing process-wide preparation capacity;
all 271 passed serially without code changes. No observed unit failure remained
without a subsequent passing execution. Logs:
`/private/tmp/cookie-final-tests/`, `/private/tmp/cookie-accepted-tests/`,
`/private/tmp/cookie-path-tests/`, `/private/tmp/cookie-wasm-serial-tests.log` and
`/private/tmp/cookie-response-cache-tests.log`.

**14 live integration suites, 187 cases passed** in one prepared topology:
WebhookKeyGet 19, KeyAuth 20, JwtAuth 14, OpenidConnect 8, CSRF 9, DSL 56,
ForwardAuth 26, RequestMirror 11, HMAC 8, ResponseCache 1, WAF 5,
SessionPersistence 5, H2C 1 and AI Resilience 4. The new Cookie case proves three
webhook projections plus mirror and main forwarding are sanitized; invalid bytes
produce HTTP 400 and no captures throughout the negative observation window.
Real adapter tests complement the capture endpoint's collapsed header map with
`get_all(COOKIE)` and single-publication assertions.

The final standard runner passed metrics-label, unknown-config-field, requeue-loop
and cache-residue gates (`wipe-all: total=447 errors=0`; all checked caches empty).
Logs: `/private/tmp/cookie-live/` and
`tmp/integration_testing/testing_20261027_110436/test_logs/`.

Local startup initially stopped because macOS lacked loopback aliases 127.0.0.2-7
and passwordless sudo was unavailable. No network settings were changed. The
standard WAF runner supports a 127.0.0.1 topology and started the selected union
after normal preflights; prepared test_client ran the other compatible suites.
The first direct Webhook run omitted the runner's admin-token environment, causing
log-fetch 401s; after restoring the same fixture environment, all 19 passed.

**Limits:** multi-IP LB live testing was not run (selector unit tests passed).
No live K8s cluster run is claimed. K8s manifests, both runner inventory branches
and deduplicated per-Pod capture aggregation were independently reviewed; the
existing batch-selection shell regression and shell syntax checks passed.

## Checks

Passed: workspace all-target check, workspace all-target Clippy, formatting,
agent-doc validation, SSA force, metrics inventory, Pingora source policy, Gateway
API transform contract, unit-test layout, Gateway no-tracing, Cookie ownership,
English-only, types purity, Bash portability and diff whitespace checks.
All new lint warnings were corrected. Clippy retains unrelated existing warnings;
its remaining Gateway warning is the unchanged outbound call-test initializer.
Build and check logs use `/private/tmp/cookie-*`; no Cargo ran during live tests.
The owned integration processes and Redis container were stopped and removed after
verification; unrelated processes and containers were preserved.

## Performance decision and limits

The committed benchmark imports the actual core. An isolated release harness used
http 1.5.0 / bytes 1.12.1, alternating timing batches with dispersion; allocation
counters are compiled out of timing runs. Allocation mode reports requested bytes,
not RSS or allocator overhead. Fixtures are prepared outside measured closures.

The full offset cache failed cold lookup gates: 32 short pairs/first key measured
18.3 ns baseline versus 1057.3 ns candidate. The 256-pair fixture exceeds the 8 KiB
metadata budget; its warm results are uncached fallback, explicitly labeled.
The two-pass planner regressed matched long-pair fixtures approximately 49-55%.
Both candidates were rejected, not adopted conditionally in production.

The accepted single-pass planner preserves borrowed survivor Vec metadata and
exact-size owned HeaderValue conversion. No-match may allocate metadata, but
allocates no output or replacement header. The expanded fixture matrix covers
absent/empty/multiple fields, short/long/dense/UTF-8 values, multiple names and
high deletion ratios. Final logs: `/private/tmp/cookie-final-timing.log` and
`/private/tmp/cookie-final-alloc.log`.

A prepared HeaderMap helper sequence measured 447.5 ns baseline versus 372.7 ns
candidate (-16.7%). Both used 8 allocation calls requesting 451 bytes, 387 peak
extra requested bytes and 43 retained bytes at its measurement boundary. It covers
lookup, deletion/publication, subsequent reads, append and two detached snapshots.
It does **not** measure actual adapter/authentication, webhook construction,
network throughput or complete Gateway peak memory. Independent final review
accepted these limits because no alternative cache/representation was adopted.
No universal speedup, parse-once implementation or eliminated Vec claim is made.
