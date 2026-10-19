# Evaluate request dye name ownership

Status: deferred for user decision. Priority: P2. Type: performance investigation.
Inspected baseline: `ce29cb6c7`.

## Evidence and alternatives

CompiledDyeRule in plugins/runtime/dye_plan.rs owns a String header name.
session_adapter.rs::strip_request_dye_once_core scans the request's handled
Vec<String> case-insensitively. On the first occurrence it removes the client
header and clones the name into that vector. Templates, conditions and outcome
membership are already prepared; this remaining copy occurs per distinct request
dye name. The vector survives replacement of the current entry's plan.

Evaluate sharing compiled names with Arc<str> and retaining those owners in the
handled vector. This could remove per-request name copies but introduces shared
allocation metadata and reference-count work, potentially contended across
requests. It does not remove the vector allocation or case-insensitive scan.
A typed HeaderName alternative could also prepare parsing, but changes name
representation and may normalize the first authored spelling. Treat that as a
separate scope choice, not an incidental replacement. No measured net benefit
or preferred representation is established.

## Required boundaries

Preserve once-per-name handling across entry and ExtensionRef boundaries,
case-insensitive matching, and first-authored spelling where currently observed.
The initial strip must remain before the active plugin's kill switch and
conditions. A gateway-rendered value from an earlier entry must survive later
entries, including differently cased declarations. Response-only rules must
not strip request headers. Keep disabled-entry semantics, shared validation,
denied names, key collisions and existing Set/Append behavior unchanged.

The current plan can be replaced before later entries execute. Do not borrow
handled names from that short-lived current-plan reference or retain entire
plans merely to keep names alive. Preserve request-local lifetime and cleanup
across runtime generation changes. No new header mutation path, response-body
aggregation, logging, metrics or late post-commit dye capability is authorized.
Canonical behavior: skills/01-architecture/02-gateway/entry-level-dye.md and
skills/04-review/plugin-runtime/regression-guards/dye-stage-producer-parity.md.

## Acceptance for a later implementation

Measure allocation calls/bytes, retained memory and CPU for realistic name
lengths, rule counts, duplicate/mixed-case names, concurrent requests and hot
configuration replacement. Include cold plan construction and request cleanup.
Compare unchanged rendering, validation and error timing if typed names are
selected. Exercise anti-spoof behavior through production adapters and nested
entry dispatch, with conditions/kill switches and an earlier rendered value.
Verify that a request's handled names remain valid after its current plan changes.

Fresh Gateway dye selection passed 24 tests for the current implementation.
No candidate implementation, allocation benchmark or live transport run was
performed. Keep this decision separate from pending response-header queue
ownership and the completed outcome-mask/field-value optimizations.
