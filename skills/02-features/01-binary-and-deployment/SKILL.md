---
name: binary-and-deployment
description: Startup, CLI arguments, deployment patterns, and Feature Flags for the in-repo binaries (edgion-controller/gateway/cli).
---

# 01 Binary and Deployment

> Edgion is a Cargo workspace. This section describes the startup methods, CLI arguments, and common deployment patterns for the in-repo binaries. Federation Center is maintained in the separate EdgionCenter repository.

## File List

| File | Topic |
|------|-------|
| [00-binaries.md](00-binaries.md) | Entry points of the in-repo binaries, full CLI argument reference, work_dir priority |
| [01-deployment-patterns.md](01-deployment-patterns.md) | Common deployment methods: FileSystem development mode, K8s production mode, HA deployment |
| [02-feature-flags.md](02-feature-flags.md) | Cargo Feature Flags matrix: allocator / TLS backend / testing options |
