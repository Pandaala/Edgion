---
name: gateway-tls-candidate-ownership
description: Use when reviewing Gateway TLS collision candidate retention and proposing winner-only snapshot publication.
knowledge_type: intentional-design
---

# Gateway TLS snapshots retain configured collision candidates

## Conclusion and scope

Keep the current `GatewayTlsMatcher` candidate vectors. During the 2026-09-10
single-issue discussion, the user accepted the configuration-resident memory
cost and declined optimization. This closes the investigation without a runtime
change; it does not disprove the extra ownership or establish measured savings.

The decision covers the hostname and catch-all maps in `TlsMatcherData`, whose
slots own `Vec<Arc<GatewayTlsEntry>>`. A `GatewayTlsEntry` represents one logical
Gateway listener and holds compiled serving certificates/private keys through
`compiled_serving_certificates`, a client CA store through
`compiled_frontend_ca_store`, and its `frontend_mtls` decision. Only the sorted
first candidate is used for a matching slot.

## Rationale and evidence

- `edgion-gateway/src/runtime/handler.rs`: `GatewayHandler` keeps the canonical
  Gateway resources in `GatewayStore` and rebuilds TLS matching from that input.
- `edgion-gateway/src/runtime/matching/tls.rs`: `rebuild_from_gateways` compiles
  each eligible listener's serving identity and frontend policy, sorts colliding
  candidates with `tls_entry_order`, and atomically publishes both complete maps.
  `match_sni` clones only `entries.first()`; it does not try a losing candidate
  when the winner rejects a handshake. The 64-participant warning cap limits
  diagnostics, not retained candidates.
- Each rebuild constructs replacement maps. The publication source does not
  append previous generations to a history. Old readers can still own an old
  snapshot, and a selected entry can outlive its lookup through its returned Arc.
  These lifetimes are distinct from the canonical configuration's ownership.

This is configuration-sized retained state, not a candidate allocation on every
request. The lookup does not need the losing compiled objects, but no measured
memory pressure, representative collision frequency, or rebuild regression was
established. The acceptance is a value judgment, not a proof that the cost is
universally negligible. Required selection semantics remain in
[the collision precedence rule](../regression-guards/cross-gateway-sni-collision-deterministic.md).

## Alternatives and limits

Publishing only the winner could release losing compiled objects from the new
index. It would not remove source certificate material from canonical resources,
eliminate temporary compilation work by itself, or release objects still owned
by old readers. That optimization and a shared `Rfc6125HostnameMatcher` API change are declined
for this investigation: there is no demonstrated benefit to justify changing the
builder and validating its ownership transitions. The benefit of the current
choice is avoiding that implementation and maintenance work.

The accepted cost is the extra candidate vectors and losing compiled TLS
objects for configured collisions. There is no global 64-candidate ownership
bound. This acceptance does not cover incorrect winner selection, an mTLS
bypass, secret disclosure, or stale generations retained after all owners exit.
TLSRoute's winner-only publication and EdgionTls's incremental indexes have
different ownership contracts; neither determines this matcher's representation.

## Verification and re-evaluation

The build, publication, lookup, and configuration-owner paths were inspected
for this decision. No ownership probe, memory benchmark, or wire test was run;
the decision does not claim quantified memory savings or a measured upper bound.
No runtime, configuration schema, user documentation, or integration test changes
are required for this acceptance.

Closure checks on 2026-09-10: `cargo fmt --all`,
`cargo check --workspace --all-targets`, and
`cargo clippy --workspace --all-targets` exited successfully. Clippy reported
warnings in unchanged Rust files; this was not a warning-free run. The Python
guards `validate_agent_docs.py`, `check_ssa_force.py`,
`check_metrics_inventory.py`, and `check_pingora_source_policy.py` under
`cicd/checks/` also passed. These checks do not measure retention costs.

Reconsider with representative evidence of memory pressure from collisions,
material rebuild cost, unexpected retention after readers release their Arcs,
or a change to winner selection, failure policy, or snapshot ownership. Separate
canonical resources, temporary compilation, published maps, and old readers in
any measurement; preserve deterministic precedence and `StrictReject` ownership.

## Resolution references

- `gateway-tls-collision-snapshot-retention` / `D0125` — accepted configuration
  memory cost, optimization declined on 2026-09-10; no runtime fix or false-positive
  determination. Originally tracked by
  `tasks/todo/investigate-gateway-tls-collision-snapshot-retention.md` after B0222.
