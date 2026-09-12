# Refresh the LinkSys Documentation Set

## Meta
- Status: pending
- Severity: medium
- Category: documentation

## Evidence
The following documents still describe removed Webhook modules or direct-upsert lifecycle paths, call live Kafka support future or placeholder work, list incorrect access-log backends, or publish a nonexistent `failedCache` configuration:

- `skills/01-architecture/02-gateway/09-link-sys.md`
- `skills/01-architecture/02-gateway/14-link-sys-dev.md`
- `skills/01-architecture/03-controller-gateway-link/webhook.md`
- `skills/01-architecture/05-resources/18-link-sys.md`
- `skills/02-features/03-resources/18-link-sys.md`
- `skills/04-review/link-sys/intentional-design/link-sys-dispatch-extract-function.md`
- `skills/05-testing/03-link-sys-testing.md`
- `docs/en/dev-guide/logging-system.md`
- `docs/zh-CN/dev-guide/logging-system.md`
- `docs/en/ops-guide/infrastructure/connectors.md`
- `docs/zh-CN/ops-guide/infrastructure/connectors.md`

## Why separate
The correction is a coordinated architecture, feature, testing, and bilingual user-documentation sweep. The stale comment in `edgion-common/src/link_sys_core/lifecycle.rs` is separate focused work and is intentionally excluded.

## Candidate next step
Revalidate every listed statement against typed resources and current provider, logging, backfill, and Webhook publication paths; update the eleven documents consistently, preserve the operator-facing schema reference, and run the agent-document consistency checks.
