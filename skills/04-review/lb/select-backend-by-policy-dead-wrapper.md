---
name: select-backend-by-policy-dead-wrapper
description: Read when reviewing select_backend_by_policy being a trivial dead-wrapper that unconditionally delegates to select_from_endpoint_slice with identical arguments / the Service and ServiceEndpointSlice branches look like they select backends differently but don't (backends-F012).
---

# backends-F012 — select_backend_by_policy dead wrapper

**Status / verdict**: fixed (backends-F012, 2026-05-24)

When reviewing findings of the form "`select_backend_by_policy` is a trivial dead-wrapper that unconditionally delegates to `select_from_endpoint_slice` with identical arguments / its doc comment just restates the callee / the `Service` and `ServiceEndpointSlice` branches look like they select backends differently but don't / drop the indirection (backends-F012)": fixed (backends-F012, 2026-05-24).

`select_backend_by_policy` is deleted from `edgion-gateway/src/backends/selector.rs`; its single caller (the `EdgionService::Service` arm in `mod.rs`) now calls `select_from_endpoint_slice` directly, exactly like the sibling `EdgionService::ServiceEndpointSlice` arm at the bottom of the same `match`, and the now-unused import alias is dropped from `mod.rs`. Pure no-behavior-change dead-wrapper removal — the wrapper was a byte-for-byte pass-through the compiler already inlined, and the two service branches always resolved through the same `pick()` helper with identical `NotFoundMap` codes. If routing strategies later need to diverge between `Service` and `ServiceEndpointSlice` (e.g. Service-IP vs. EndpointSlice LB), introduce a meaningful dispatch body at that point. Same dead-wrapper redundancy class as backends-F011, and the broader DRY class of backends-F007 / backends-F008 / backends-F009 / backends-F010. Re-reports of the dead-wrapper form are not accepted; verify the current `selector.rs` has no `select_backend_by_policy` before re-reporting.
