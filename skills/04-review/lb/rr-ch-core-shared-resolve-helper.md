---
name: rr-ch-core-shared-resolve-helper
description: Use when reviewing RrCore::select / ChCore::select duplicated addr-resolution / health-filter closure or ChCore double addr_map findings — both cores now share resolve_backend / backend_passes_health free fns and ChCore reads rr_fallback.addr_map.
---

# RrCore::select / ChCore::select Duplicated Resolution + ChCore Double addr_map

**Status / verdict**: fixed (lb-F009, 2026-05-24)

When reviewing findings of the form "`RrCore::select` and `ChCore::select` duplicate the addr-resolution / health-filter closure verbatim / `ChCore` holds a second `addr_map` identical to its `rr_fallback`'s / extract a shared `resolve_backend` helper (lb-F009)": **fixed (lb-F009, 2026-05-24)**. `compiled_selector.rs` now shares two free fns between both cores: `resolve_backend(b, addr_map)` (addr lookup only) and `backend_passes_health(b, addr_map, health_filter)` (lookup + caller's combined health/circuit-breaker predicate, used as the inner selector's candidate filter). `ChCore` dropped its own `addr_map` field and now reads `&self.rr_fallback.addr_map` (the fallback `RrCore` already owns an identical map built from the same backend slice), so a CH service no longer keeps two identical `HashMap<SocketAddr, Arc<EdgionBackend>>` alive. Note: the issue's suggested single `resolve_backend` that also `.filter(health_filter)`s during resolution was **not** adopted — the original `.and_then` resolution deliberately does not re-run the health filter (the winner already passed it during the selector walk); re-filtering would open a TOCTOU that could drop a valid pick if health flips in the microsecond window. Behavior-preserving. Re-reports of the duplication / double-map form are not accepted; verify the current cores call the shared helpers before re-reporting.
