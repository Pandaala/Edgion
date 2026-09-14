---
name: backend-generation-arc-slice-handle-layout
description: Use when reviewing the request-path size of HTTP or gRPC backend handles that pin an immutable compiled generation.
knowledge_type: intentional-design
---

# Backend generation handles retain the immutable slice layout

## Conclusion and scope

`LogicalBackendHandle` and `GrpcBackendHandle` deliberately retain their compiled
generations as `Arc<[CompiledBackend]>` and `Arc<[CompiledGrpcBackend]>`. On the
current 64-bit target, each handle is 24 bytes: the slice `Arc` is a two-word fat
pointer and the slot index is one word. The corresponding selected Service state is
40 bytes after adding `BackendTlsPolicyLookup`; its `Option` remains the same size.

This applies only to the HTTPRoute and GRPCRoute backend-selection state stored in a
request context. It is a size budget, not a stable public ABI.

## Rationale and evidence

The slice generation encodes an immutable, exact-length compiled collection and lets
the handle index directly into the generation that it pins. Commit `ddd20c1a8`
changed both implementations from `Arc<Vec<T>>` to `Arc<[T]>`, but left the prior
16/32-byte assertions unchanged. The stale assertions then failed even though the
production layout changed as intended and selection remained allocation-free.

The paired budget tests in `edgion-gateway/src/backends/runtime.rs` and
`edgion-gateway/src/routes/grpc/match_unit.rs` now record the actual symmetric
24/40-byte layout. Their adjacent allocation-counter tests independently guard that
constructing and projecting selected backend state does not allocate per request.

## Alternatives and limits

`Arc<Vec<T>>` would make each handle one word smaller because it points to a sized
`Vec` header, but it reintroduces a separate collection representation and an extra
indirection between the `Arc` and its elements. A custom thin generation wrapper
could also reduce handle size, at the cost of another representation and added
complexity. Neither tradeoff is justified by current profiling evidence.

The accepted cost is one additional word in each request-stable handle. This decision
does not permit further growth without review and does not establish that 40 bytes is
an architecture-independent value.

## Verification and re-evaluation

Fresh focused tests on 2026-09-14 verified both size budgets and the unchanged
allocation-free selection tests. Reconsider the representation if request-context
memory profiling identifies these handles as material, if another field increases
either selected state beyond 40 bytes on the supported 64-bit build, or if the
generation representation changes again. Keep the HTTP and gRPC budgets symmetric
unless their stored semantics intentionally diverge.

## Resolution references

- `Astra-size-01` — stale hot-path size assertions discovered during the independent
  GRPCRoute correction review; accepted `Arc<[T]>` layout and corrected on 2026-09-14.
