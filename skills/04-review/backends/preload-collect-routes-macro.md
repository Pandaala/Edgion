---
name: preload-collect-routes-macro
description: Read when reviewing LB-preload collect_http_routes / grpc / tcp / udp / tls in preload.rs having five byte-for-byte identical bodies / extract a generic helper or trait to dedup the namespace-fallback + key-format logic (backends-F009).
---

# backends-F009 — preload collect_*_routes five identical bodies

**Status / verdict**: superseded (backends-F009, 2026-09-02)

The preload subsystem was deleted when Service/EndpointSlice reconciliation moved to eager selector
compilation in `edgion-gateway/src/backends/discovery/coordinator.rs`. There is no longer a route
collector or startup hydration path to deduplicate. Re-reports of the five-identical-copies form
are not accepted unless a new preload mechanism is introduced.
