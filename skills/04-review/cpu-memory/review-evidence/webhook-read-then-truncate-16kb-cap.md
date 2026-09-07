---
name: webhook-read-then-truncate-16kb-cap
description: Use when reviewing findings that claim webhook read_body_limited causes 1GB full buffering; the actual hard cap is 16KB enforced by WEBHOOK_GLOBAL_MAX_RESPONSE_BYTES.
knowledge_type: review-evidence
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# Webhook Read-Then-Truncate Has a 16KB Hard Cap

**False-positive scenario**: `read_body_limited` calls `resp.bytes().await` to read fully into memory then truncate; described as "1GB full buffering".

**Reality**:
```rust
// Call site
let max_bytes = runtime.config.max_response_bytes.min(WEBHOOK_GLOBAL_MAX_RESPONSE_BYTES);
pub const WEBHOOK_GLOBAL_MAX_RESPONSE_BYTES: usize = 16384;  // 16KB hard cap
```
Two layers of `.min()` guard; worst case extra memory read and discarded = 16KB, not the 1GB described. The code comment already acknowledges this limit and notes "sufficient since max_bytes is capped at 16KB".

**Verdict**: impact is minimal (extra 16KB); no fix needed.
