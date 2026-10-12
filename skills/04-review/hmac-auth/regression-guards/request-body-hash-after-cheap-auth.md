---
name: request-body-hash-after-cheap-auth
description: Use when reviewing HMAC request-body validation; covers authentication ordering, bounded capture, and non-materializing hashing.
status: fixed
finding_id: BODY-006
closed: 2026-07-27
knowledge_type: regression-guards
---

# HMAC Request-Body Hashing After Cheap Authentication — FIXED

## Conclusion

HMAC validates authorization metadata, credentials, timestamp, signed-header
requirements, and the signature before capturing the request body. Once those
checks pass, it completes bounded capture and hashes the current immutable body
version without materializing disk-backed storage.

Fix suggestions that move HMAC back behind the generic eager body gate or add a
shared body cursor are **not accepted**.

## Core Rationale

**1. Invalid authentication should not force body IO**

The HMAC plugin owns its resolved fail-close `BodyRequirement`, allowing cheap
header and signature rejection before request-body capture. Other body-aware
plugins retain the generic body-gate behavior.

**2. Digest verification needs bytes, not a contiguous allocation**

`PluginSession::ensure_request_body` completes capture and returns the final
length. The Pingora adapter implements it without calling `read_all`, after
which `BodyStorageView::sha256` scans memory or disk with a private positional
offset. Disk-backed requests therefore remain disk-backed.

**3. Existing safety boundaries remain authoritative**

The gateway physical cap is enforced while capture streams, the HMAC
per-plugin cap is checked against the captured length, and capture failures use
the existing 413/408/500 status mapping. HMAC's intrinsic requirement remains
fail-close even when construction bypasses the normal runtime wiring.

## ARCH-006 — Resolve logical limits during construction

`HmacVerifier::new` resolves `maxBodySize` after `BodyRequirement::resolve` and
retains an immutable numeric `Option<u64>` for its generation. Generic gates use
the same cold `plugin_limit_bytes` resolver. The shared runtime comparator
`over_plugin_limit_len` accepts resolved bytes and exact captured length only;
no byte-size parser or schema requirement is reachable from that check.

This closes repeated configuration parsing and temporary lowercase allocation for
authenticated body requests. It establishes a source-visible boundary, with no
throughput or memory-reduction benchmark claim. A reconstructed verifier resolves
the new generation independently. Direct construction retains the historical
invalid/zero optional-limit fallback (`None`); it does not weaken intrinsic
fail-close or permit operator body conditions to bypass authentication. Normal
resource validation continues to reject invalid limits.

Regression coverage in `hmac_auth/plugin_test.rs` configures requirements before
construction and covers logical rejection before hashing, equality, invalid/zero
fallback with timeout fail-close, and generation reconstruction. Generic gate
boundary/condition tests and the storage memory/disk digest test retain the
physical-cap, error-policy, and non-materializing hashing boundaries. Standalone
HMAC logical failures retain 413; body-aware condition consumers retain their
no-match semantics and `negate` behavior. Fresh command results belong to the
implementation task; the BODY-006 historical date is not a new verification date.

## Fix Suggestions Not Accepted

- Put HMAC behind `BodyGatedRequestFilter` — this restores eager capture before
  cheap authentication and materializes disk-backed bodies through `read_all`.
- Add a shared mutable hashing cursor — body versions are immutable snapshots;
  local positional reads avoid cross-plugin and upstream replay coupling.
- Apply the HMAC ordering change to every body plugin — their protocol and
  failure semantics differ, and BODY-006 requires only a narrow HMAC path.

## Re-evaluation Triggers

Re-open this decision only if:

- HMAC gains a body-signature mode that cannot authenticate headers before
  capture.
- Request-body storage stops providing immutable versioned views or positional
  hashing.
- A new adapter cannot implement capture completion without contiguous
  materialization.

## Reference Cases

- `BODY-006`
- `edgion-gateway/src/plugins/http/hmac_auth/plugin.rs`
- `edgion-gateway/src/plugins/runtime/session_adapter.rs`
- `edgion-gateway/src/request_body/storage.rs`

## Resolution references

- `BODY-006` — 2026-07-27.
- `ARCH-006` — cold logical-limit resolution and numeric runtime comparison.
