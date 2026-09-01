# Harden JWE Protected Header JSON Parsing

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | correctness |
| Priority | P2 |
| Issue | N/A |
| Depends on | B0084 |

## AI guide summary

Parse every compact-JWE protected header once as a strict JSON object, rejecting duplicate members and non-object values while preserving the original encoded segment as authenticated AAD.

## Requirements

- Reject duplicate protected-header member names, including `alg`, `enc`, and unknown names.
- Reject arrays, scalars, and `null` as invalid protected-header JSON instead of classifying them as missing required headers.
- Preserve B0084's `zip`/`crit` presence rejection, algorithm checks, allowlist, token cap, failure precedence, status, and static logs.
- Preserve the exact original protected segment as AES-GCM AAD; do not authenticate reserialized JSON.
- Do not modify existing integration tests.

## Acceptance criteria

- Focused tests cover duplicate known and unknown members, every non-object JSON shape, valid unknown non-critical members, and raw-AAD success.
- Gateway format/check/Clippy and repository guards pass.
