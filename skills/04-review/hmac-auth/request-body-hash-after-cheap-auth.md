---
name: request-body-hash-after-cheap-auth
description: Use when reviewing HMAC request-body validation; covers authentication ordering, bounded capture, and non-materializing hashing.
status: fixed
finding_id: BODY-006
closed: 2026-07-27
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
