---
name: add-new-resource-guide
description: Comprehensive guide for adding new CRD resource types.
---
# Add New Resource Guide

> Comprehensive guide for adding new CRD resource types. Touches 10+ files across `types/`, `core/controller/conf_mgr/`, `core/controller/conf_sync/`, `core/controller/api/`, `core/gateway/conf_sync/`, `core/gateway/api/`, and CRD YAML.
>
> **TODO (2026-02-25): P0, New**
> - [ ] `define_resources!` macro: adding new kind entry in `src/types/resource/defs.rs`
> - [ ] Resource struct definition (`types/resources/`) + `impl_resource_meta!`
> - [ ] `ProcessorHandler` implementation
> - [ ] ConfHandler implementation (gateway-side config sync)
> - [ ] Admin API route registration
> - [ ] CRD YAML schema authoring (`config/crd/edgion-crd/`)
> - [ ] Complete file-level checklist (10+ files to touch)
> - [ ] Note: existing `docs/zh-CN/dev-guide/add-new-resource-guide.md` is outdated — references `ConfigServer`/`ConfigClient` but actual codebase uses `ResourceProcessor` + `ProcessorRegistry`
