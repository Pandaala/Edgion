---
name: binary-and-deployment
description: Startup, CLI arguments, deployment patterns, and Feature Flags for the four binaries (edgion-controller/gateway/cli/center).
---

# 01 Binary and Deployment

> Edgion is a single-crate, four-binary project. This section describes each binary's startup methods, CLI arguments, and common deployment patterns.

## File List

| File | Topic |
|------|-------|
| [00-binaries.md](00-binaries.md) | Entry points of the four binaries, full CLI argument reference, work_dir priority |
| [01-deployment-patterns.md](01-deployment-patterns.md) | Common deployment methods: FileSystem development mode, K8s production mode, HA deployment |
| [02-feature-flags.md](02-feature-flags.md) | Cargo Feature Flags matrix: allocator / TLS backend / testing options |
