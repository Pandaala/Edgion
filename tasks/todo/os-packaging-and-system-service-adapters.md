# Add OS Packaging and System-Service Adapters

## Meta

- Status: pending
- Type: feature
- Priority: P2
- Issue: N/A

## AI guide summary

Define and implement an explicit standalone-distribution contract for supported
operating systems. The work must package Edgion binaries and configuration
assets, provide service-manager integration for the Controller and Gateway, and
make installation, upgrade, rollback, and removal behavior testable. The main
difficulty is keeping OS-specific ownership and lifecycle policy outside the
Rust binaries while preserving operator control over host tuning.

## Problem

The repository currently provides standalone deployment examples and optional
Linux tuning templates, but it does not provide a complete package installer or
an authoritative system-service adapter. Existing systemd examples assume that
an operator has already installed binaries, created paths, selected an account,
and supplied service units. The Linux tuning assets likewise assume an existing
service and are deliberately inert.

Without a defined packaging boundary, downstream installers can disagree about
filesystem layout, service dependencies, user and file ownership, configuration
preservation, upgrade ordering, and uninstall behavior.

## Requirements

- Select and document the initially supported operating systems, package
  formats, architectures, and service managers before implementing adapters.
- Define one canonical installation layout for the three binaries, process
  configuration, mutable work data, logs, and packaged documentation.
- Install `edgion-controller` and `edgion-gateway` as independently manageable
  services with explicit startup ordering, restart policy, shutdown behavior,
  environment handling, privileges, and resource limits. `edgion-cli` remains a
  client binary and must not receive a daemon service.
- Define package install, upgrade, downgrade or rollback, disable, and uninstall
  semantics. Preserve operator-owned configuration and mutable data unless an
  explicit purge operation is selected.
- Reconcile the authoritative service definitions with the standalone guide so
  examples cannot silently drift from packaged behavior.
- Integrate optional host-tuning assets only through an explicit operator choice.
  Packaging and service startup must not automatically mutate sysctls, PAM
  limits, container-runtime limits, or unrelated host policy.
- Keep `net.ipv4.tcp_tw_reuse` unchanged. Preserve the reviewed tuning baseline:
  `net.core.somaxconn=65535` and `LimitNOFILE=1048576` are opt-in template
  values, not unconditional runtime defaults or capacity guarantees.
- Provide repeatable package and service lifecycle verification in disposable
  Linux environments for every supported adapter.

## Scope

### In scope

- Package metadata, build/release integration, installation scripts, and
  service-manager assets for the explicitly selected targets.
- Dedicated service identity, directory creation, permissions, configuration
  placement, process arguments, dependency ordering, and clean shutdown.
- Conflict-safe handling of existing files and clear ownership boundaries
  between package-managed assets and operator-managed configuration.
- Upgrade, rollback, uninstall, and purge tests, including service enablement and
  preservation of operator data.
- Documentation for installation, verification, troubleshooting, and recovery.

### Out of scope

- Changes to Controller, Gateway, or CLI product behavior solely to accommodate
  a package manager.
- Kubernetes manifests, Helm charts, operators, or privileged tuning DaemonSets.
- Automatic host tuning at process startup or package installation.
- Claims that file-descriptor or backlog allowances establish a supported
  connection capacity.
- Adapters for an OS, package format, or service manager not selected by the
  task's approved design.

## Acceptance criteria

- The supported target matrix and package/service ownership contract are
  documented and reviewed before implementation.
- A clean target can install the package artifacts, start the Controller and
  Gateway, verify their service state, and use `edgion-cli` without manual binary
  placement.
- Upgrade and rollback exercises preserve operator configuration and mutable
  data, and service ordering remains correct across restart and reboot.
- Uninstall removes package-owned services and immutable assets without deleting
  operator data; an explicitly documented purge path handles any broader
  cleanup.
- Installing, starting, stopping, upgrading, and removing the packages leaves
  unrelated sysctls and host limits unchanged. Optional tuning requires a
  separate, visible operator action and has documented verification and rollback.
- CI or an equivalent repeatable harness validates package contents, permissions,
  service definitions, lifecycle transitions, and failure recovery on every
  supported target.

## Starting references

- `cicd/deploy/standalone/README.md`
- `cicd/deploy/linux-tuning/README.md`
- `docs/en/ops-guide/kernel-tuning.md`
- `skills/04-review/listeners/intentional-design/linux-host-tuning-baseline.md`
