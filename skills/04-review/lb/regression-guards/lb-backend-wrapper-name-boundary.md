---
name: lb-backend-wrapper-name-boundary
description: Use when reviewing findings about the gateway LB backend wrapper name or about residual `EdgionBackend` identifiers inside `edgion-gateway/src/lb/`; covers why the wrapper is `LbBackend` and which `EdgionBackend` occurrences deliberately remain.
status: fixed
finding_id: task-rename-lb-edgion-backend-name-collision
closed: 2026-09-05
knowledge_type: regression-guards
---

# LB Backend Wrapper Name Boundary — FIXED

## Conclusion

The gateway load-balancer backend wrapper is `edgion_gateway::lb::backend::LbBackend`.
The Kubernetes resource `edgion_resources::resources::edgion_backend::EdgionBackend` keeps
its name unchanged. Both names are final.

Fix suggestions of the form "rename the `EdgionBackend` CRD / `ResourceKind::EdgionBackend`
for symmetry", "rename `LbBackend` to `BackendHandle` or `TrackedBackend`", or "the remaining
`EdgionBackend` identifiers under `lb/` are leftovers from the rename" are **not accepted**.

## Core Rationale

**1. Only one of the two names is free to change**

`EdgionBackend` is a Kubernetes API contract: a CRD kind, `ResourceKind` discriminant 23, an
`HTTPBackendRefKind::EdgionBackend` route target, a `PolicyTargetKind::EdgionBackend` policy
target, and the `edgion.io/EdgionBackend/<ns>/<name>` scope-key wire format. Renaming it would
break stored objects and persisted session-persistence tokens. The LB wrapper is a
process-local type with no external contract, so the collision is resolved on that side.

**2. `LbBackend` was chosen deliberately over the alternatives**

`BackendHandle` was rejected: the `*BackendHandle` family (`LogicalBackendHandle`,
`GrpcBackendHandle`) is `#[derive(Clone)]` `{generation, index}` route-slot pointers — the
opposite of a `!Clone` owner of per-backend mutable state — and `backends/mod.rs` already
names both. `TrackedBackend` describes the state but not the owning subsystem. `LbBackend`
matches the existing `LbCommitGuard` prefix convention in `lb/attempt.rs`.

**3. `EdgionBackend` identifiers still present under `lb/` are correct, not residue**

The rename was driven by the compiler: the definition was renamed first, then substitutions
were applied only at the `file:line` locations the compiler reported as unresolved, so CRD
references — which continued to resolve — could not be touched. The surviving occurrences in
LB-adjacent code all mean the resource:

- `lb/runtime_state/mod.rs` and `routes/http/session_persistence.rs` scope-key fixtures of the
  form `"edgion.io/EdgionBackend/<ns>/<name>\0core/Service/<ns>/<name>"` — this is the
  `<apiGroup>/<Kind>/<namespace>/<name>` wire format, not a type name.
- `backends/mod.rs` `compute_routed_external_resolver` doc ("routed external EdgionBackend"),
  reached only from `resolve_ai_endpoint`.
- `ctx.rs` `BackendSource::AiBackend` doc ("Typed EdgionBackend selected by an AiProxy
  attempt plan").
- `CompiledBackendTarget::EdgionBackend`, `PolicyTargetKind::EdgionBackend`,
  `HTTPBackendRefKind::EdgionBackend`, `ResourceKind::EdgionBackend`,
  `Rsc::new("EdgionBackend", ..)`, `kind: Some("EdgionBackend")`.

## Fix Suggestions Not Accepted

- Rename the CRD kind, `ResourceKind` discriminant, or the route/policy target kinds — breaks
  the Kubernetes API contract and the persisted scope-key format.
- Rewrite the `edgion.io/EdgionBackend/...` scope-key literals — they are wire format; changing
  them invalidates existing session-persistence tokens.
- Add a `pub use lb::backend::LbBackend as EdgionBackend` compatibility alias — pre-release
  project rule forbids compatibility shims.
- Rename `LbBackend` again to `BackendHandle` / `TrackedBackend` — see rationale 2.

## Re-evaluation Triggers

Re-open only if:

- The `EdgionBackend` CRD kind is itself renamed as part of a deliberate API break, making the
  collision moot from the other side.
- A third `*Backend` owner type appears under `lb/` and makes the `Lb` prefix ambiguous.

## Reference Cases

- `tasks/todo/rename-lb-edgion-backend-name-collision.md` (closed 2026-09-05).
- `edgion-gateway/src/lb/backend.rs` — the wrapper definition.
- `skills/01-architecture/02-gateway/06-load-balancing.md` — the `lb/` module map.

## Resolution references

- `task-rename-lb-edgion-backend-name-collision` — 2026-09-05.
