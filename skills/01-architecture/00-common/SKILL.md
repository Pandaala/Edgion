---
name: common-conventions
description: Conventions shared by the in-repo binaries (edgion-controller, edgion-gateway, edgion-cli) — project overview, command line / directory / config-path conventions, Core layering conventions, resource system.
---

# 00 Common conventions

> This directory describes the foundational conventions shared by the in-repo binaries. It is recommended to read this first before reading the architecture documentation for any specific binary.

## File index

| File | Topic | Recommended reading scenario |
|------|------|-------------|
| [00-project-overview.md](00-project-overview.md) | Project overview: high-level architecture diagram, Crate structure, key dependencies | First contact with the project, when a global view is needed |
| [01-cli-and-startup.md](01-cli-and-startup.md) | Unified command-line conventions, working directory, config-file paths | Modifying startup arguments, understanding config loading |
| [02-core-layout.md](02-core-layout.md) | Core module layering conventions, placement rules | Adding new modules, avoiding regression to old directory structures |
| [03-resource-system.md](03-resource-system.md) | Resource system: `define_resources!` macro, ResourceMeta, ResourceKind, Preparse | Adding a new resource type, understanding the resource abstraction |
| [04-grouped-list-pattern.md](04-grouped-list-pattern.md) | Design contract for list-shaped fields with named groups (IP / key / regex / issuer / origin lists): when to group, atomic schema replacement, CRD rules, first-wins + warn, group-in-ok-log iron rule | Adding or extending any `*_groups` field; designing a new list resource |
| [05-unification-points.md](05-unification-points.md) | Canonical registry of cross-cutting mechanisms (outbound dialing, TLS, SSRF guards, DNS, Redis, dye headers, KeyGet, IP matching, real IP, templates, durations, error replies): the canon, the rule, the exemptions | Before implementing any cross-cutting mechanism in a new plugin/module; when a mechanism "feels like it should already exist" |
| [06-resource-reference-contract.md](06-resource-reference-contract.md) | Shared `refs` module: owning field adapters, omitted/empty/literal `core` group semantics, default profiles, finite custom convenience manifest, fixed identities, Grant boundaries | Adding or changing any field that names another resource; reviewing group/kind/namespace defaults |

> The Secret → Material resolution layer (full Resolver table, Controller/Gateway responsibility split, Zeroize scope) has been merged into [../05-resources/10-secret.md](../05-resources/10-secret.md).
