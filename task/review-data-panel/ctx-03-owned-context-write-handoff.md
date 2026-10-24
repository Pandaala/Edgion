# Owned context-write handoff

Status: deferred for user decision. No runtime change.
Source baseline: `88d7b2c5f`.

## Evidence and scope

PluginSession::set_ctx_var takes key/value slices. The production
PingoraSessionAdapter copies both into Strings before calling the canonical
EdgionHttpContext::set_ctx_var, which inserts them into request-owned ctx_map.
These inspected callers already own final values before that copy:

| Caller | Owned input and current handoff |
| --- | --- |
| CtxSet::run_request | process_rule returns String after source/extract/transform/mapping; each rule publishes before the next runs. |
| JwtAuth::run_request | Optional serde_json::to_string claims result is borrowed for the setter, then discarded. |
| OpenidConnect::apply_upstream_headers | Optional serialized claims are borrowed for oidc_claims, then discarded. |
| JweDecrypt::run_request | After successful sensitive header forwarding and optional claim projection, plaintext is borrowed for jwe_payload. It originated from String::from_utf8 consuming the decoded Vec after detached in-place decryption; there is no truncation step here. |
| DSL BuiltinId::CtxSet | runtime_string returns owned String for both popped value and key, then the builtin borrows both for the setter. This does not eliminate the VM's preceding conversions. |

The current KeySet::Ctx Some branch already moves its value into the canonical
store, while cloning the configured name. RegionRoute computed-hash writeback
is its current production caller. This bounded handoff is not blanket approval
to retain arbitrarily sized buffers from other producers. Constructing a temporary
KeySet::Ctx for every write would itself allocate a name which the adapter then
clones; merely rerouting calls does not establish an overall saving.

## Decision

Retain the existing compact copy, design one canonical owned-aware context setter
with an exact consumer list, or evaluate prepared use of existing KeySet where it
fits. Do not introduce per-plugin context stores or bypass the session contract.
The value of moving a String is avoiding the redundant copy of its live bytes;
it can also carry its existing spare capacity until replacement/removal or request
cleanup. Serialization, transforms and decoded buffers require separate capacity
and lifetime measurements. The current copy overlaps source and destination, so
retained capacity alone proves neither a higher peak nor an overall saving.

This task concerns final publication. [ctx-01](ctx-01-regex-replacement-storage.md)
and [ctx-02](ctx-02-extract-trim-projection.md) concern earlier temporary ownership
and explicitly exclude setter changes. No regex-buffer transfer, template rewrite,
new value limit, schema field, cache, pooling or runtime implementation is approved
here. Preserve borrowed setters for consumers that do not own transferable values
if the selected interface still needs them; do not retain redundant interfaces
solely for backward compatibility.

## Acceptance before implementation

- Freeze an exact allow-list after selecting representation and consumers; use
  independent IMPL plan and final review. Keep one canonical request store.
- Preserve declaration-order writes and cross-rule reads, replacement, empty
  versus absent values, removal, request/response continuity and request isolation.
  Do not hold session loans across mutation or batch writes until stage completion.
- Preserve setter failure handling: CtxSet emits its existing error; the inspected
  claims writers ignore setter results; DSL maps failure to its existing ApiError.
  Preserve auth checks, forwarding/claim projection/hide-credential ordering,
  template policy and current limits. Ownership must not change acceptance.
- Keep jwt_claims, oidc_claims and jwe_payload subject to existing access-log
  declaration/blocklist rules. No new logging, metrics or sensitive-data erasure
  claim. The separately accepted alias-exposure limitation remains unchanged.
- Compare borrowed/static and owned values, missing/default paths, large spare
  capacity, shrinking transforms, repeated overwrite/removal and long-lived
  requests. Measure copied bytes, allocation calls and simultaneous live storage
  through request cleanup, not just setter time or output length. Account for
  dynamic versus prepared key ownership separately.
- Exercise real adapter interoperability and lifecycle, selected plugin writes
  and exact failure outcomes; mocks alone do not verify production ownership.
  Retain existing behavior tests and run project checks. No throughput benefit
  has been measured for a proposed implementation.

## Sources and current verification

Start from `skills/SKILL.md`, then the ctx-set design decisions and claims
access-log guard, DSL request-response context continuity, and the two linked
CtxSet tasks. Source owners are plugins/runtime/{traits/session,session_adapter},
ctx.rs, plugins/http/ctx_set/plugin.rs, jwt_auth/plugin.rs,
openid_connect/userinfo.rs, jwe_decrypt/plugin.rs and dsl/lang/{builtins,vm}.rs.

Fresh unchanged-code selections passed 15 CtxSet-related, 180 adapter and seven
DSL context-builtin cases (199 distinct tests; three DSL cases overlap).
Formatting, workspace all-target check/Clippy and agent-doc
validation passed with existing warnings. These tests cover existing behavior,
not an owned-setter candidate or all five plugins' authentication paths. No new
runtime test, full suite, live network exchange or performance benchmark ran.
