# JSON value profiling worklist representation

Status: deferred user decision. Source baseline: `862082aef`.

## Evidence and scope

The canonical `profile_json_value` in
`edgion-resources/src/resources/edgion_plugins/plugin_configs/json_schema_validation.rs`
starts with `vec![(value, 1)]`. Scalar roots and empty containers therefore allocate
the traversal worklist even though they have no pending descendants. The recent
number-length change removed temporary numeric Strings but retained this owner.

Reachable request consumers include:

- REST-to-GraphQL `CompiledRestToGraphql::bind`: profile each present input and
  the converted output before type checking; missing optional values exit earlier.
- Gateway ProxyRewrite: profile copy sources, resolved patch values and the
  document after each successful operation; node/text totals feed its work charge.
- Resources gRPC `BoundRequest::encode`: profile the merged input before
  normalization, serialization and protobuf encoding, with additional caller limits.

Cold literal/config validation also calls the same function. These consumers
already impose body, structure, work or exchange-admission bounds. Eager worklist
population alone is not evidence of an unrestricted gateway memory vulnerability.
This record concerns temporary traversal representation, not a new resource policy.

## Ordering and accounting contract

The current walk checks depth before incrementing/checking node count, then updates
maximum depth and text bytes. Limits are 64 levels and 20,000 nodes. All additions
to node/text counters saturate. Numeric bytes use the existing Number Display
counter; strings count their bytes, booleans/null add no text bytes.

Arrays append every child in iteration order and visit them in reverse through
LIFO pops. Objects count all current keys and append all current children before
visiting any descendant, again in reverse iteration order. A wide container may
populate pending references before any child reaches its node/depth check.
The worklist's pending storage therefore depends on tree shape, not just depth.
A depth-first iterator-frame replacement must not silently change visit order,
key accounting timing, depth convention or which error wins when multiple
structural limits are exceeded. No fixed Vec growth factor or capacity is assumed.

The strict JSON parser collects its own accounting while parsing and does not
count numeric text the same way as this already-parsed value profiler. Reusing
its recorded totals or merging the two implementations is a separate semantic
change, not an interchangeable optimization.

## Alternatives for user decision

- Retain the existing walk and measure realistic scalar-heavy REST mappings,
  repeated rewrite operations and wide/deep merged inputs first.
- Hold the root outside the Vec or introduce a carefully scoped scalar/empty-root
  path. This can avoid the initial worklist allocation for those inputs, but a
  naive empty Vec can change subsequent container capacity growth. Avoid duplicate
  scalar accounting and verify mixed-input CPU/allocation costs.
- Evaluate inline worklist storage with heap fallback. This trades allocations
  for a larger stack frame and possibly different capacity/retention behavior;
  choose any inline size from measurements rather than a presumed typical shape.
- Evaluate borrowed iterator frames to limit eagerly retained sibling references.
  Frame representation, reverse traversal and object-key accounting need an exact
  design and independent review. A new early fanout rejection would be a policy
  change and is not authorized by this task.

No alternative is selected. Preserve the single shared profiler rather than
adding a Gateway-only variant, request cache or persistent profile cache.

## Acceptance before implementation

Compare the original and candidate reports and errors for every scalar kind,
empty containers, mixed arrays/objects, broad roots, deep chains and combined
wide/deep shapes. Include exact and exceeded node/depth limits, especially cases
where both limits could fail in different traversal orders. Current independent
boundary tests do not establish the latter precedence coverage. Retain saturating
counts, numeric Display semantics, object key bytes and all caller-specific limits.

Measure allocations, allocated bytes, peak pending storage, stack-frame cost and
CPU for individual profiles and repeated REST/rewrite call patterns. Preserve
fixed client errors, canonical body ownership, publication order, gRPC admission
and cancellation behavior; add no tracing, metrics or new configuration. Keep
request data borrowed only for the synchronous profile and release all temporary
owners before consumer publication or outbound work.

Relevant original sources include the REST adaptation regression guard and the
gRPC transcoder admission/outcome ledger under `skills/04-review`, plus the
Gateway ProxyRewrite implementation. Existing tests and fresh baseline execution
are recorded in INDEX. No replacement algorithm or benchmark accompanies this task.
