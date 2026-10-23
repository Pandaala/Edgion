# JSON selection capture transfer

Status: deferred for user decision. No runtime change.
Source baseline: 65065d528.

## Evidence and scope

`JsonSelectPlan` in `edgion-gateway/src/plugins/http/dsl/lang/body_scanners.rs`
builds a shared-prefix trie once. Identical definite paths reuse one capture
index, while `outputs` preserves each caller-specified name and its capture.
The `JsonSelect` opcode in `lang/vm.rs` receives an owned capture vector and
clones each selected Value while constructing the ordered output Map.

The scanner produces scalar values or serialized compound JSON as `Value::Str`.
String clones share Arc storage; this is not a recursive Map or JSON payload
copy. A straightforward take/replace at each output would break later aliases
of the same capture. The final original capture owner is currently discarded
after output construction, so a terminal-use transfer could avoid one Arc
clone/drop for a successfully emitted string capture. Other aliases still need
their ownership. No measured allocation or throughput improvement follows.

## Decision requested

Retain the current simple projection, or investigate bounded last-use metadata
prepared with the immutable plan in `Vm::new`. Earlier aliases would clone and
only the terminal use would consume the capture. Compare the saved reference
count work against metadata size, construction work and request-path branching.
Do not allocate a usage table or HashSet on every execution merely to save these
Arc operations. Reversing output traversal is not a drop-in substitute because
the Map preserves caller order.

The production plan admits 1–64 entries, possibly fewer under `max_map_size`,
with unique names and independently validated definite paths. Any added metadata
must remain bounded by this plan and derived after defensive validation. Keep
the existing worker-owned plan/reader lifetime; borrowing from a waiting VM
cannot replace ownership needed by the scanning thread.

This concerns completed capture projection, separately from DSL 01's string
input ownership, parser token allocation and JSON worker scheduling. No new
cache, body copy, full DOM or response aggregation is proposed.

## Acceptance before implementation

Use IMPL with an exact source allow-list. Measure unique versus heavily aliased
plans, scalar versus string/compound captures, and small versus maximum plans.
Include cold metadata and retained capture/output ownership, not just reference
count operations. Keep the final output-name String ownership and VM Map
admission unless separately reviewed.

Verify repeated string aliases all receive the same value in caller order;
include root/descendant sharing, missing paths, null, invalid documents, duplicate
parents replacing a complete subtree, and later duplicates replacing an oversized
capture. Preserve online aggregate-overflow behavior rather than replaying a
previously rejected capture after another value shrinks.

Keep Missing/out-of-range capture fallback as Nil and TooLong as the existing
`StringTooLong` lower bound of `max_string_len + 1` with saturating arithmetic.
Preserve ordered projection/error handling, individual and aggregate limits,
snapshot generation, one-scan accounting, cancellation cleanup and storage/error
policy. Do not eagerly reorder failure detection merely to simplify transfer.
Exercise memory and spill-backed requests and defensive plan construction.

At this baseline, all 504 Gateway DSL tests passed. Existing scanner tests cover
shared paths (including integer aliases), root/descendant selection, duplicate
replacement, aggregate limits and malformed-document discard; a VM test covers
multi-field selection with one spilled scan. These do not prove a new terminal-use
scheme, string-alias projection lifetime, or measured performance. Original
contracts are in `skills/01-architecture/02-gateway/dsl-engine-security-model.md`
and `skills/01-architecture/02-gateway/dsl-engine.md`.
