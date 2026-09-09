# Correct the EdgionTls allowedSans Structural CRD Schema

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-09 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Make the committed EdgionTls `allowedSans` schema structural and installable without weakening its typed operator contract or runtime validation.

## Problem

The complete CRD installation is rejected because `EdgionTls.spec.clientAuth.allowedSans.items` contains the non-structural `additionalProperties: false` declaration. BackendTLSPolicy verification therefore required a temporary CRD copy with only that declaration removed.

## Requirements

- Make the committed EdgionTls CRD structural and installable without weakening the typed `allowedSans` contract.
- Preserve the declared `type`, `match`, `value`, `ignoreCase`, and `oid` fields and their required, default, enum, and validation constraints.
- Preserve runtime `deny_unknown_fields` validation and the Controller-to-Gateway transport contract.
- Keep the Rust schema and hand-written CRD synchronized, including the two-direction CRD coverage guard.
- Do not modify the pinned upstream Gateway API CRD bundle.
- Add a deterministic structural-schema or API-server validation regression.

## Scope

### In scope

- EdgionTls `allowedSans` Rust/schema alignment, the committed EdgionTls CRD, and focused schema/install validation.

### Out of scope

- BackendTLSPolicy API versions, certificate matching semantics, or unrelated CRDs.

## Acceptance criteria

- The unmodified complete committed CRD set installs successfully on a supported Kubernetes cluster.
- Valid EdgionTls `allowedSans` samples are admitted and malformed samples retain the intended rejection behavior.
- CRD coverage, focused resource tests, agent-doc validation, and repository guards pass.
