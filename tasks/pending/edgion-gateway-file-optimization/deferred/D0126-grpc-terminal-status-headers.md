# D0126 — Canonical gRPC Terminal Status Headers

- Status: pending
- Severity: medium
- Discovered in: B0224 error-response analysis
- Canonical task: [Enforce canonical gRPC terminal status headers](../../../todo/enforce-canonical-grpc-terminal-status-headers.md)

Request-stage queued status/message headers can survive adapter terminal writes;
the existing response-stage stripping regression does not cover these paths.
Source reachability was independently confirmed by the B0224 final reviewer,
not freshly reproduced on the wire. No repair is authorized by its HTML optimization.
