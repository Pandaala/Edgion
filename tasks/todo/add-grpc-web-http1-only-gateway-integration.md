# Add gRPC-Web Coverage for an HTTP/1-Only Gateway

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-09 |
| Status | pending |
| Type | bugfix |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Add an integration scenario proving that disabling frontend HTTP/2 does not disable HTTP/1.1 gRPC-Web translation to an H2 gRPC backend.

## Requirements

- Configure an HTTP Gateway listener with `edgion.io/enable-http2: "false"`.
- Send an HTTP/1.1 gRPC-Web request through an HTTPRoute to the established h2c backend.
- Assert successful protobuf data and the body-encoded gRPC trailer frame.
- Preserve bounded waits and deterministic cleanup.

## Scope

### In scope

- A new or extended gRPC-Web integration scenario after the campaign freeze.
- Reuse of the existing h2c backend and gRPC-Web assertions.

### Out of scope

- Editing an existing `edgion-tests/**` file during the quality campaign.
- Changing frontend HTTP/2 enablement or backend protocol selection semantics.

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-09 | Defer from B0183 | The existing frozen suite covers only the default H2-enabled Gateway and cannot prove the independent HTTP/1-only bridge boundary. |
