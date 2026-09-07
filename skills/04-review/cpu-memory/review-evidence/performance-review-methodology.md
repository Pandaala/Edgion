---
name: performance-review-methodology
description: Use when evaluating CPU or memory audit findings; provides the general decision methodology (trace call chain, distinguish phases, verify mechanism) to avoid common false positives.
knowledge_type: review-evidence
---

# CPU/Memory Performance Audit Methodology

> Source: post-mortem of the high false-positive rate (49%) in the 2026-04 CPU/Memory review. This document provides **general decision methodology** to avoid common false positives.
> For specific scenarios, see `not-a-performance-issue.md` in the same directory.

---

## Core Principle

**Trace the call chain, distinguish phases, verify the mechanism.**

Many seemingly serious performance problems are not actually triggered along the real execution paths in code.

---

## Critical Phases That Must Be Distinguished

```
┌─────────────────────────────────────────────────────────────────┐
│  Config-load phase (cold path)        Request-handling phase (hot path)  │
│  ─────────────────────                ─────────────────────              │
│  • preparse() / compile()             • run_request() / handle()         │
│  • from_yaml() / deserialize          • match_host() / route()           │
│  • Resource creation / init           • Executed for every request       │
│                                                                          │
│  Clone/Regex compile here → OK        Clone/Regex compile here → issue  │
└─────────────────────────────────────────────────────────────────┘
```

**To verify whether the issue is on the hot path**:
1. Find the call sites of the suspicious code
2. Trace the call chain back to the entry function
3. Determine whether the entry is config-change-triggered or per-request-triggered

---

## Rust Performance Pattern Checklist

### Arc-Wrapped Clone is O(1)

```rust
// ❌ False positive: looks like Clone has overhead
let config = self.plugin_config.clone();  // If Arc<T>, this is just refcount +1

// Verification: look at the type definition
pub plugin_config: Arc<PluginRuntime>  // Arc Clone = O(1)
pub plugin_config: PluginRuntime       // Value Clone = O(n) ← real issue
```

### Work Done in the preparse Phase Will Not Be Repeated on the Hot Path

Edgion's `EdgionPlugins::preparse()` is responsible for precompiling regex, preprocessing conditions, etc. at config load time.

```rust
// Common pattern
impl Plugin {
    fn preparse(&mut self) {
        self.compiled_regex = Some(Regex::new(&self.pattern).unwrap());
    }
    
    fn handle_request(&self) {
        // Use self.compiled_regex; it will not be recompiled
    }
}
```

**Key checks**:
- Search for `preparse`, `compile`, `compile_conditions` methods
- Confirm whether `#[serde(skip)]` runtime fields are set during the init phase

### Fallback Code is Defensive Programming

```rust
// This kind of code is usually not a performance issue
if self.compiled.is_none() {
    // fallback: only triggered if preparse failed
    self.compiled = Some(compile_pattern(&self.pattern)?);
}
```

**Decision method**: check whether `compiled` has been set by `preparse()` in the normal flow.

---

## Memory Growth Analysis Principles

### Distinguish "proportional to scale" vs "unbounded growth"

| Scenario | Verdict |
|------|------|
| HashMap stores K8s resource state | ✅ Normal — proportional to cluster scale |
| Push to Vec on every request | ❌ Leak — unbounded growth |
| Offline node retains historical data | ⚠️ Check whether a cleanup mechanism exists |

### Check Existing Cleanup Mechanisms

**Keywords to search**:
- `remove` / `delete` / `clear` / `gc` / `cleanup`
- `ttl` / `timeout` / `expire`
- `max_entries` / `capacity` / `limit`
- Admin API / DELETE endpoint

```rust
// Example: code comment explains the design decision
// Offline entries are retained in memory and must be removed explicitly via
// the Admin DELETE API (see ticket #20).
```

**If a cleanup mechanism is found** → not a leak; it is a design decision.

---

## Verification Flow

```
Find suspicious code
    ↓
Trace call chain → find entry function
    ↓
Hot path?
    ├─ No → ❌ False positive (cold path)
    └─ Yes → check Arc/Box wrapping
                ├─ Wrapped → Clone O(1)?
                │             ├─ Yes → ❌ False positive (Arc Clone)
                │             └─ No → ✅ Confirmed issue
                └─ Not wrapped → ✅ Confirmed issue
```

---

## Common False-Positive Patterns

| Looks like an issue | Reality | Verification method |
|-------------|----------|----------|
| `config.clone()` in handler | config is `Arc<T>` | Look at type definition |
| `Regex::new()` in match code | `preparse()` does precompilation | Search for preparse calls |
| HashMap grows unboundedly | Stores current state snapshot | Check whether external resources are tracked |
| Offline data not cleaned up | Has Admin DELETE API | Search for remove/delete methods |
| Large enum Clone overhead | Cloned only at config load | Trace Clone call chain |
| Repeated Entry-type structs | Type safety, no runtime impact | Confirm whether compile-time or runtime |

---

## Report Format

For each risk point, the verified report must include:

```markdown
### [Risk point name]

**Original rating**: 🔴 High
**Verification result**: ❌ False positive / ✅ Confirmed / ⚠️ Potential

**Call-chain analysis**:
- Entry function: `xxx`
- Trigger condition: config change / per-request / scheduled task

**Key code**:
[Code snippet]

**Conclusion**: [Reason explanation]
```

---

## Red Flags — Need Re-Verification

If these thoughts appear during a review, stop and re-verify:

- "This Clone must be expensive" → check whether Arc-wrapped
- "Regex is compiled every request" → search for preparse/compile methods
- "This HashMap grows unboundedly" → check whether external resource state is tracked
- "No cleanup code seen" → search for Admin API / gc / ttl
