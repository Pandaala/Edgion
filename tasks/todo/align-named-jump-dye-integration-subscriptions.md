# Align Named-jump Dye Integration Subscriptions

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-09 |
| Status | pending |
| Type | test |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Align the historical named-jump integration fixtures with stage- and policy-aware dye validation while preserving their existing routing, denial, fallback, and anti-spoof security assertions.

## Requirements

- Audit the Canary and DynamicInternalUpstream named-jump fixtures and make each response dye rule subscribe only to outcomes reachable from its effective policy and execution path.
- Keep any fixture that can reach both reject and fallback subscribed to both outcomes.
- Preserve the existing denial, fallback, success, backend-contact, route-selection, access-log, and spoofed-header stripping assertions.
- Do not weaken stage-aware validation, add compatibility behavior, expose a new dye capability, or change production runtime semantics.
- Existing integration files remain frozen until their exact paths are separately authorized.

## Scope

### In scope

- The invalid named-jump integration fixtures and their directly corresponding scenario expectations.
- Focused validation and integration verification for those fixtures.

### Out of scope

- Production dye capability, routing, plugin execution, or response rendering changes.
- Unrelated integration fixtures or broad integration-suite cleanup.

## Acceptance criteria

- Every named-jump fixture passes standalone resource and Gateway compilation validation.
- The currently blocked cases execute their original outcome and anti-spoof assertions successfully.
- Legal denial and fallback paths retain their no-unintended-backend-contact guarantees.
- Focused named-jump integration checks and repository documentation guards pass.
