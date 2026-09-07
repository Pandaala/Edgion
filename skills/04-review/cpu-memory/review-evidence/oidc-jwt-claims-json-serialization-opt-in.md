---
name: oidc-jwt-claims-json-serialization-opt-in
description: Use when reviewing findings that criticize OIDC or JWT claims serde_json::to_string per authenticated request as hot-path overhead; all switches default to false.
knowledge_type: review-evidence
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# OIDC / JWT Claims JSON Serialization is the Opt-In Feature Itself

**False-positive scenario**: criticizing `openid_connect/userinfo.rs:60-74` and `jwt_auth/verify.rs:172-176` for doing `serde_json::to_string(claims)` per authenticated request; suggesting "lazy serialization" or "documentation warning that it is enabled by default".

**Reality**:

The three relevant switches all default to `false`:
- `set_userinfo_header` → `openid_connect.rs:340` = `false`
- OIDC `store_claims_in_ctx` → `openid_connect.rs:343` = `false`
- JWT `store_claims_in_ctx` → `jwt_auth.rs:187` = `false`

"Users very easily turn it on by default" does not hold. Once a switch is on, serialization is the feature itself:

- The `X-Userinfo` header must be written before forwarding to upstream; downstream has no trigger point for "lazification".
- The `set_ctx_var(name, value)` signature is `&str`; the ctx-var model stores owned String. To do "serialize on read", the ctx var must be changed to a typed `Value`, affecting the entire plugin runtime ctx interface.
- Claims are typically < 1KB; `serde_json::to_string` is microsecond-level; not in the same order of magnitude as network/route-matching cost on the request path.

The only visible micro-optimization: when `set_userinfo_header && userinfo_json.is_none() && store_claims_in_ctx` are simultaneously true, lines 60 and 71 of `userinfo.rs` would serialize twice — could reuse the same String. But the trigger condition is narrow; saves one microsecond-level alloc; the change benefit does not outweigh review/regression cost.

**Verdict**: inherent cost of the opt-in feature; not a performance issue. Any "OIDC/JWT hot-path claims JSON serialization" finding is closed per this entry, unless it is proved that the default is changed to `true` or claims size is ≥ tens of KB.
