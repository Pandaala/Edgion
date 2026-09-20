# Optional Linux tuning assets

These examples are inert until an operator installs them. They do not include a
service unit, package installer, or automatic host mutation.

- `sysctl.d/99-edgion.conf`: listen backlog ceiling only; preserves `tcp_tw_reuse`.
- `systemd/edgion-gateway.service.d/90-resource-limits.conf`: FD allowance for an existing service.
- `limits.d/99-edgion.conf`: alternative for a dedicated account using PAM sessions.

Read prerequisites, capacity caveats, verification, and rollback in the
[English guide](../../../docs/en/ops-guide/kernel-tuning.md) or
[Chinese guide](../../../docs/zh-CN/ops-guide/kernel-tuning.md) before installation.
