# Validate RealIp Header Names Canonically

## Meta
- Status: pending
- Severity: medium
- Category: configuration-validation

## Evidence
`RealIpConfig` rejects only an empty `real_ip_header`. An invalid HTTP field name is accepted by resources and preparse, then behaves as a silent header miss at runtime. The global and plugin RealIp paths share the same extractor semantics, so local runtime filtering would leave admission inconsistent.

## Why separate
The fix belongs at the resources boundary and must preserve Controller/Gateway validation parity plus global/plugin behavior. Reuse an existing canonical HTTP field-name helper when available instead of adding another parser, and update the matching English and Chinese schema documentation.

## Integration-test impact
No new integration issue is needed. Resource validation, preparse parity, and colocated extractor/plugin tests can cover the contract while existing trusted-proxy integration tests remain frozen.
