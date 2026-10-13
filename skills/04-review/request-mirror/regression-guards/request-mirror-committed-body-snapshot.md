---
name: request-mirror-committed-body-snapshot
description: Use when reviewing RequestMirror body admission at rule or backend scope, committed snapshots, cursor isolation, framing, and discarded mirror responses.
status: fixed
finding_id: BODY-014
closed: 2026-07-27
knowledge_type: regression-guards
---

# RequestMirror body admission and committed snapshots

## Conclusion

When an earlier request plugin has created a canonical `BodyStorageView`,
RequestMirror captures the current immutable committed snapshot using the
canonical snapshot reader. Before detaching, it reserves process body budget
and isolates just the snapshot's memory segments into owned send data; disk
segments remain streamed without whole-body materialization. The mirror never
consumes Pingora's upstream replay cursor. With no body
view, RequestMirror retains its original streaming tee behavior.

Native HTTPRoute `backendRefs[].filters` mirrors may install that read-only tee
during the first backend request dispatch. This does not reopen request-body
capture or mutation and does not change the primary relay plan. The permission
is local to `PingoraSessionAdapter`, never a persistent flag on `RequestInfo`.

The mirror response is semantically irrelevant and is dropped after response
headers arrive; it is never aggregated.

## Core Rationale

**1. The mirror is a later logical body owner**

Request plugins run serially. A mutation committed before RequestMirror must be
visible to both RequestMirror and every upstream attempt. Reading the current
view produces that version without retaining a mutable plugin owner.

**2. Snapshot and replay cursors remain independent**

The detached mirror task owns an independent snapshot reader. It never calls replay
`rewind`, `peek`, or `consume`, so mirror progress, timeout, and failure cannot
move or block the main upstream cursor. Pingora retries continue to rewind and
replay their own owner.

**3. Each outbound hop owns its framing**

The snapshot path removes inherited `Content-Length` and `Transfer-Encoding`.
`reqwest::Body::wrap_stream` derives streaming framing instead of forwarding
metadata that may describe the downstream V0 body. The former full `Bytes`
materialization was replaced as part of D0066; the committed-version contract
remains unchanged.

**4. The no-view path stays streaming**

RequestMirror alone does not opt a request into full-body caching. When no
canonical body view exists, the existing bounded-channel tee remains in use.
The tee additionally enforces per-mirror and process byte admission. Its owned
chunks cannot pin arbitrarily large backing allocations through small slices.
See [mirror resource budgets](mirror-resource-budgets.md) for ownership and limits.

The backend-stage admission correction addresses
`backend-request-mirror-streaming-body-stage`. Previously the backend filter ran
after `RequestInfo.in_request_body_stage` became false, so without a committed
snapshot its `set_mirror_state` call failed with `mirror body stage frozen`.
Explicit prebuffering avoided the error but did not fix streaming admission.

`pg_upstream_peer::select_http_backend` now uses the backend-specific request
dispatcher. Its adapter grants `backend_mirror_allowed` only when Pingora's
canonical `request_attempt_id()` is 1. In the locked fork revision `518f727`,
`HttpProxy::proxy_to_upstream` calls `upstream_peer` before connecting and before
any H1/H2 body relay callback. Thus the selected backend's first dispatch sees
the entire upcoming body sequence. All other adapter construction sites deny
this additional permission, and cancellation drops it with the adapter.

The `RequestBodyPluginState` replay predicate only includes handler and WAF
sources. Mirror observation does not alter framing, the frozen relay source,
or replayability. Do not move the freeze or preselect a backend merely to
install a mirror. Capture, mutation, observer/handler installation and WAF
streaming still require the original request stage. Backend ExtensionRef
request entries use the same adapter and inherit only this mirror permission.

The selected backend is retained across ordinary retries, so its request chain
does not rerun. A mirror that received `Complete` or `Abandoned` retires before
replayed body callbacks; it is not restarted. A connection failure before body
delivery may leave the original mirror waiting for a later successful attempt.
Existing cancellation and budget rules still govern incomplete work. This
correction does not expand the single active streaming-mirror slot into support
for multiple simultaneous tees.

**5. Mirror responses have zero aggregation budget**

The mirrored response never affects the client or gateway policy. Dropping it
after headers avoids unbounded response-body memory and prevents an infinite
response from retaining a mirror concurrency permit.

## Fix Suggestions Not Accepted

- Share or rewind Pingora's upstream replay cursor from the detached mirror
  task.
- Force every RequestMirror request through `BodyStorage` when no earlier body
  owner exists.
- Preserve downstream `Content-Length` or `Transfer-Encoding` on the snapshot
  hop.
- Aggregate the discarded mirror response without a hard bound.

## Re-evaluation Triggers

Re-open this decision only if:

- RequestMirror gains response-dependent behavior.
- Request-body versions can change after the request plugin chain.
- The mirror transport stops accepting an independently owned immutable body.
- Backend request dispatch moves after body delivery, starts running again on
  retries, or Pingora changes its attempt identity or relay-callback ordering.

## Verification of backend admission

Verified on 2026-09-10:

- `cargo test -p edgion-gateway --lib mirror`: 44 passed, including first-attempt
  admission, adapter-local permission, frozen non-mirror capabilities, and
  terminal retirement under replayed body events.
- The same freshly built unit executable, filtered to
  `plugins::runtime::session_adapter::record_outcome_tests`: 61 passed.
- `cargo fmt --all`, workspace check/clippy with `--all-targets`, agent-doc,
  SSA-force, metrics inventory, Pingora source policy and gateway no-tracing
  checks passed. Clippy reported existing warnings. Agent-doc validation first
  found an existing missing UDP documentation task; restoring that pending
  follow-up repaired the reference without changing UDP runtime behavior.
- Canonical `prepare.sh` completed. The local runner with
  `--no-prepare -r EdgionPlugins -i RequestMirror --keep-alive --skip-residue-check`
  passed; the same live topology then passed `test_client -g --dynamic-scenario
  RequestMirrorReferenceGrant`. Fixed-length and chunked unbuffered backend
  POSTs, the prebuffered control, primary/mirror body equality, and grant
  creation/revocation delivery boundaries were exercised.
- Residue validation ran after the dynamic scenario with `cache_residue --mode
  fail`: passed (`wipe-all: total=39 errors=0`). The runner's earlier residue
  step was deliberately deferred until both suites finished. Test services
  were stopped by the canonical cleanup script.
- Independent read-only review returned LGTM for code, knowledge, and the
  restored documentation follow-up.

Run evidence: `tmp/integration_testing/testing_20260910_205312/report.log`;
`/tmp/edgion-backend-mirror-{unit,adapter,check,clippy,dynamic,residue}.log`
(ephemeral). The standalone dynamic invocation initially lacked the runner's
capability environment, then lost the tool-owned service process tree between
commands; the successful run kept startup, dynamic traffic, residue validation
and cleanup in one tool session.

The retry regression drives body events directly; the dynamic scenario does
not induce a network retry. Retry dispatch ordering was also checked against
the locked Pingora source. No fresh Kubernetes or HTTP/2 wire run, or full
workspace test run, is claimed. Historical BODY-014 and D0066 checks below are
separate evidence.

## Reference Cases

- Origin finding:
  `tasks/review-body-buffering-runtime/body-014-request-mirror-after-mutation.md`
- Runtime:
  `edgion-gateway/src/plugins/http/request_mirror/plugin.rs`
- Integration coverage:
  `edgion-tests/src/client/suites/edgion_plugins/body_plugins.rs`
- Ordinary streaming regression coverage:
  `edgion-tests/src/client/suites/edgion_plugins/request_mirror/`

## Resolution references

- `BODY-014` — 2026-07-27.
- `D0066` / `bound-request-mirror-concurrency-and-buffering` — replaces full
  snapshot materialization with budgeted snapshot streaming. Verification is
  recorded in the resource-budget rule, not inferred from BODY-014's history.
- `backend-request-mirror-streaming-body-stage` — fixed and verified on
  2026-09-10; uses a first-attempt adapter permission for a read-only tee.

## Ordinary capture identity

The ordinary POST body and path probes select `find_mirror_capture` before
reading evidence (ARCH-393). Primary and mirror deliveries share the capture
endpoint and trace; arrival order is not mirror identity. Polling for any marked
record does not make `captures[0]` the mirror. The private selector requires the
existing `x-mirror: true` marker; missing marked captures fail explicitly.

The pure capture-identity regression checks both arrival orders and rejects
borrowing a valid primary's method/body when the marked mirror is wrong. It
verifies assertion input selection, not live delivery or production body copying.
Selecting a mirror does not by itself establish query preservation. The normal
and slow test-server capture handlers record the raw query before consuming the
body, independently of the existing path field (ARCH-394). Both query-preservation
cases require the selected mirror's complete path and authored query in original
order; absent, null, malformed or changed values fail. Network-free handler tests
verify the serialized record from both real handlers, while pure client checks
challenge missing, altered and reordered queries. These checks do not establish
live Gateway or Kubernetes query forwarding. Production mirror transport and
body budgets remain unchanged.
