# XML namespace prefix lookup reuse

Status: deferred for user decision. No runtime design selected.
Source baseline: `957513b28`.

## Evidence and boundary

Active full WAF XML inspection uses `XmlStream`. In `xml_stream.rs`,
`close_start` resolves the element namespace before lexical attribute sorting and
duplicate rejection. It then splits ordinary attribute QNames and resolves each
nonempty prefix with `prefix_uri`. That helper searches active bindings in reverse
order, charging every prefix comparison through `equal_spans`.

Several attributes can repeat one prefix while the binding set remains unchanged
throughout this resolution loop. Each currently repeats the reverse search.
This is a bounded CPU candidate, not evidence of a missing parser limit. Existing
name, attribute, namespace, structural, retained and work limits remain required.
The earlier QName split reuse already avoids a second delimiter scan; this task
concerns repeated binding lookup and does not reopen that completed optimization.

## Decision alternatives

- Retain the scalar lookup if typical binding sets are small or bookkeeping and
  preserved accounting outweigh avoided comparisons.
- Evaluate bounded reuse local to the start-tag resolution interval, returning
  the canonical Uri representation. Single-entry and wider representations have
  different hit rates and storage costs; neither is selected.

Do not introduce a global cache, another namespace resolver, a new body owner or
unbounded per-tag map. Keep copied names and URIs in their existing arena: their
scope must survive synchronous value callbacks. A lookup optimization is not
permission to borrow transient incoming bytes for persistent bindings.

## Semantic and accounting gates

Preserve element lookup before lexical duplicate validation, ordinary attribute
resolution after that validation, and expanded duplicate checks afterward.
Do not eagerly resolve later attributes: an unbound-prefix Syntax error must
retain precedence over later work failures. Attributes continue to be observed
under their lexical QName in source order before start-tag completion.

Reverse lookup implements nearest-scope shadowing. Late declarations in the same
tag remain visible during resolution. Default namespaces apply to unprefixed
elements but not ordinary unprefixed attributes; reserved xml/xmlns cases retain
their distinct behavior. Cache identity must respect decoded prefix bytes and
lookup context, not merely an arena offset, QName suffix or URI equality.

Every canonical binding comparison retains its work charge and failure point.
`charge_work` assigns a checked sum before testing its limit: a failed limit
charge remains in usage, while arithmetic overflow does not assign. A cached
successful total alone cannot reproduce intermediate failure checkpoints.
Distinguish optional reuse-bookkeeping cost from these preserved logical charges,
and justify any physical bound for bookkeeping rather than silently changing
budget acceptance or scanning unbounded data before admission.

All reused spans must expire before start-tag arena compaction, empty-tag
reclamation or closing-tag scope restoration. Preserve failure cleanup and
partial parser state; no shared mutable cache may outlive the owning call.
Account for any retained capacity, inline layout or temporary allocation explicitly.
Keep observability unchanged, with no request tracing or profiling metrics.

Before implementation, freeze an exact allow-list and obtain independent IMPL
plan/final review. Compare against scalar behavior across repeated/distinct
prefixes, aliases, nested shadowing, late declarations, empty/default namespaces,
reserved and unbound prefixes, lexical/expanded duplicates, empty tags, chunk
splits and scope reclamation. Include exact/one-less work boundaries, arithmetic
edges, usage after failure, emitted values and terminal cleanup.

Measure short and wide tags with small/large active binding sets, repeated and
alternating prefixes, and early errors. Include bookkeeping and whole-parser cost.
No throughput, allocation or RSS improvement has been demonstrated.

## Sources and unchanged verification

- `edgion-waf/src/seclang/xml_stream.rs`: attribute completion, prefix resolution,
  sorting, compaction, scope restoration and work charging.
- [Native XML contract](../../skills/04-review/waf/regression-guards/native-xml-inspection.md).

Fresh checks passed 21 XmlStream and 52 Gateway WAF tests (73 total), formatting
and workspace all-target check/Clippy with existing warnings. These establish
current behavior, not an accepted reuse design. No prototype, benchmark, full
suite or live integration ran.
