# D0064 — Validate RealIp Header Names Canonically

## Meta
- Status: needs-implementation
- Severity: medium
- Discovered in: B0098, RealIp analysis
- Category: configuration-validation
- Canonical task: [`tasks/todo/validate-real-ip-header-names.md`](../../../todo/validate-real-ip-header-names.md)

## Evidence
`RealIpConfig` rejects only an empty `real_ip_header`. An invalid HTTP field name is accepted by resources and preparse, then behaves as a silent header miss at runtime. The global and plugin RealIp paths share the same extractor semantics, so local runtime filtering would leave admission inconsistent.

## Why deferred
The fix belongs at the resources boundary and must preserve Controller/Gateway validation parity plus global/plugin behavior. It should reuse any canonical HTTP field-name helper extracted by D0063 instead of adding another parser, and it requires matching English and Chinese schema documentation.

## Integration-test impact
No new integration issue is needed. Resource validation, preparse parity, and colocated extractor/plugin tests can cover the contract while existing trusted-proxy integration tests remain frozen.
