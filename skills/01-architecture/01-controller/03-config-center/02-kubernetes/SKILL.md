---
name: kubernetes-config-center
description: KubernetesCenter implementation — K8s Reflector watching, leader election, HA mode, ResourceController lifecycle, status writeback.
---

# Kubernetes ConfigCenter

> KubernetesCenter is the production ConfigCenter implementation; it watches resource changes via the K8s API.

## File List

| File | Topic | Recommended reading scenario |
|------|-------|------------------------------|
| [00-lifecycle.md](00-lifecycle.md) | Startup flow and leader election | Debugging Controller startup, understanding HA behavior |
| [01-ha-mode.md](01-ha-mode.md) | HA mode in detail | Configuring high availability, understanding failover |
| [02-resource-controller.md](02-resource-controller.md) | ResourceController lifecycle | Understanding the per-resource processing flow, status writeback |
