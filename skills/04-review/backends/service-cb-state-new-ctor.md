---
name: service-cb-state-new-ctor
description: Read when reviewing set_service_config constructing Arc<ServiceCbState> with an identical field list in both the Occupied-changed swap and the Vacant insert / the two insert sites duplicate the struct literal (backends-F010).
---

# backends-F010 — set_service_config duplicate ServiceCbState literal

**Status / verdict**: fixed (backends-F010, 2026-05-24)

When reviewing findings of the form "`set_service_config` constructs `Arc<ServiceCbState>` with an identical field list in both the Occupied-changed swap and the Vacant insert / the two insert sites duplicate the struct literal / a future `ServiceCbState` field could be added to one branch but missed in the other (backends-F010)": fixed (backends-F010, 2026-05-24).

`ServiceCbState::new(config) -> Arc<Self>` (`edgion-gateway/src/backends/outlier_detection/store.rs`) is now the single canonical construction site; both `set_service_config` branches call `occ.insert(ServiceCbState::new(cfg))` / `vac.insert(ServiceCbState::new(cfg))`, so a future field is added in exactly one place. Pure no-behavior-change DRY refactor (byte-for-byte identical construction; private struct, private method). Same class as backends-F007 / backends-F008 / backends-F009. Re-reports of the duplicate-literal form are not accepted; verify `ServiceCbState::new` exists before re-reporting.
