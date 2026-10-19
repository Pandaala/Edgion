# CtxSet regex replacement result storage

Status: deferred for user decision. No runtime change.
Source baseline: 2304b3bdc.

## Evidence and scope

`CtxSet::apply_transform` in
`edgion-gateway/src/plugins/http/ctx_set/plugin.rs` calls the precompiled regex's
`replace_all(value, replacement).to_string()`. A matching replacement returns an
owned Cow; converting it with `into_owned()` could move that result instead of
copying its bytes. A no-match borrowed Cow would still need an owned copy under
the current return type. Replacement bytes and capture-expansion semantics do
not inherently change with this ownership conversion.

Locked regex 1.13.1 implements both its no-expansion and capture-expansion paths
with `String::with_capacity(haystack.len())` once the first match is found. A
replacement that shrinks the input can therefore leave substantial spare capacity.
A local probe against the existing locked dependency artifact used pattern
`^x+$`, 65,536 ASCII `x` bytes and replacement `a`. It asserted an owned Cow and
identical final text; copied output had length/capacity 1/1, while moved output
had length/capacity 1/65,536. These observed capacities are not a general allocator
guarantee or a throughput, physical heap or peak-memory measurement.

`process_rule` wraps the transformed String and then performs optional mapping.
A mapping result/default can discard it immediately. Without replacement by a
mapping result, the String returns to `run_request`, which calls the canonical
Session setter. The real setter copies the text into independently owned context
storage. There is no await in this rule's extraction/transform/mapping/set path.
Moving the regex buffer therefore changes its temporary release point, not the
capacity of the context value produced by the existing setter. It may remove an
allocation and copy while keeping the larger buffer through more synchronous
work. The current copy also has overlapping original/result storage; no higher
peak-memory claim follows from retained capacity alone.

This candidate is separate from the completed borrowing of static/source inputs
and mapping values. It does not authorize changing setter ownership, extraction,
other transforms or the sequential visibility of context updates.

## Decision requested

Retain the compacting copy, authorize moving the regex result with the temporary
capacity tradeoff, or measure a bounded ownership design before choosing. Any
change to context ownership would require its own reviewed scope and accounting;
this task does not imply that carrying the original regex capacity into long-lived
context storage is acceptable.

## Acceptance before implementation

Use IMPL with a frozen allow-list. Preserve the one-operation validation gate,
defensive transform priority, precompiled regex ownership, replacement/capture
syntax, no-match behavior, missing-regex PluginLog/default fallback, whitespace,
empty output, mapping defaults and ordered cross-rule reads. Do not compile a
regex per request or introduce another transform mechanism.

Cover matching and nonmatching inputs, shrinking and growing replacements, capture
expansion, empty replacements and mapping hit/miss/default paths. Measure copies,
allocation calls, requested bytes, retained capacity and peak lifetime separately,
including the final context setter. Keep existing resource validation and Gateway
CtxSet tests, project checks and independent final review. No operator-facing
limits, logs or schema changes are part of this candidate.
