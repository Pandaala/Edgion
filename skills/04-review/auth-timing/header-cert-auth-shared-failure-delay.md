---
name: header-cert-auth-shared-failure-delay
description: Use when reviewing HeaderCertAuth authentication-failure delay handling; records that it must call the shared auth_common helper directly.
status: fixed
finding_id: gw-plugin-auth-b-02
closed: 2026-08-03
---

# HeaderCertAuth Failure Delay Uses the Shared Helper — FIXED

## Conclusion

`HeaderCertAuth::reject` calls `auth_common::apply_auth_failure_delay` directly. The former private copy of the delay logic was removed.

Fix suggestions that retain a private sleep implementation are **not accepted**.

## Core Rationale

**1. One implementation owns auth-failure delay behavior**

The shared helper is the canonical implementation for HTTP authentication plugins. Direct delegation prevents `HeaderCertAuth` from silently missing later changes to the shared delay policy.

**2. The change is behavior-preserving**

The removed method and the shared helper both skipped zero and otherwise awaited `tokio::time::sleep(Duration::from_millis(delay_ms))` before writing the rejection response.

## Fix Suggestions Not Accepted

- Keep the private method because it is currently equivalent — equivalence today does not prevent future drift.
- Add a one-line receiver wrapper — `HeaderCertAuth` has one call site, so the wrapper adds no lifecycle or readability value.

## Re-evaluation Triggers

Re-open this decision only if HeaderCertAuth gains a distinct, documented failure-delay policy that cannot use the common HTTP authentication behavior.

## Reference Cases

- `plugins-http-F021`, which consolidated the same duplicate implementation in other HTTP authentication plugins.
- `skills/04-review/auth-timing/shared-apply-auth-failure-delay.md`.
