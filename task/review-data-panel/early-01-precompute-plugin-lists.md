# Consider resolving global and Gateway plugin lists at publication

Status: deferred for user decision. Priority: P2. Type: optimization proposal.

## Evidence

- `pg_early_request_filter.rs::initialize_scoped_request_ids` resolves global
  and Gateway keys for each request before installing runtime arrays.
- `pg_request_filter.rs` also resolves these lists when early initialization
  has not selected a Gateway (notably shared listeners).
- `global_plugins_resolve.rs::resolve_gateway_plugins` allocates a Vec with
  key-count capacity, looks up every key, and clones runtime handles.
- `ctx.rs::install_resolved_global_plugins` and its Gateway counterpart convert
  nonempty Vecs into request-owned Arc slices.
- `publication/views.rs::{PluginScope, GatewayScope}` already own immutable
  key lists and compiled maps. `publisher.rs::compose_listeners` reuses a
  GatewayScope only when the class scope Arc and Gateway keys agree.

## Proposed benefit

Build ordered runtime Arc slices once in the corresponding publication owners,
then clone the slice handle per request. This could remove repeated map lookups,
Vec construction, Vec-to-Arc conversion and per-runtime Arc traffic. Empty and
missing references must retain their current behavior.

## Why deferred

This changes publication ownership and retained memory, spanning publisher,
context installation and early/matched-owner dispatch. It needs an explicit
decision about additional per-scope storage and a measured benefit; it is not
just a local clone-to-borrow edit. Do not share mutable per-request execution
state or bypass lazy admission gates.

## Acceptance before implementation

1. Preserve one captured publication, shared-listener owner selection, scope
   availability, reference order/duplicates, dangling-reference filtering and
   early RequestId/preflight ordering.
2. Preserve denied/ambiguous plugin behavior, LKG ownership, hot reload and
   in-flight generation lifetime; old and new requests must remain isolated.
3. Prove no owner cycle or unintended retention; account for publication
   inventory and empty/no-plugin paths.
4. Compare allocation counts and dispatch cost for empty, single and multiple
   references, alongside cold publication cost and retained memory.
5. Independently review the design and existing publication/early-request tests
   before adopting it.

Relevant contract:
`skills/04-review/plugin-runtime/regression-guards/scoped-policy-runtime-publication.md`.
