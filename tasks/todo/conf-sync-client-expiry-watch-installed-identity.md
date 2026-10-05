# Observe the installed conf_sync client identity expiry

## Meta

- Status: pending
- Type: investigation
- Priority: P2
- Severity: medium
- Baseline: `f0ec920dcaf97f3c21c3f5c5ae5e7be5a6d06b09`

## Evidence and trigger

The Common cert_expiry_watch module describes the certificate actually in service, but Gateway startup (`edgion-gateway/src/cli/mod.rs`) selects a file source independent of installed Channel ownership. That source parses certificate validity only. Gateway cert_watch read_and_validate validates certificate/key/CA together; grpc_client reload_client_tls prepares before publishing and retains the old Channel after rejection.

With expiring installed certificate A, replace the disk certificate by long-lived B while keeping A's mismatching key. Reload rejects B, but the expiry watcher can export B's later expiry and clear the alert for still-installed A. A stopped reload watcher can cause the same discrepancy even with valid replacement files. This is observability correctness, not an authentication bypass.

The rationale in `skills/03-coding/observability/01-metrics.md` explicitly calls this gauge an independent cross-check on stalled reload machinery. Other reload-failure signals do not establish the expiry gauge's advertised meaning.

## Required investigation

Design the expiry source around installed-channel identity ownership while preserving cold-start visibility, atomic generation publication, last-good values, and the accepted mtime/length watcher signature. Add a regression for rejected and stalled reloads; align the owning metrics description. Read current original conf-sync and observability rules before implementation.

## Boundary

This spans Gateway startup, Channel ownership and observability semantics. It must not be folded into Common test isolation. Existing lifecycle logging remains cold, and no new parallel metrics mechanism is assumed.
