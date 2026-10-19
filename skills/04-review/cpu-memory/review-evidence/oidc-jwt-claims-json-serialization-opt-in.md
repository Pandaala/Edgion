---
name: oidc-jwt-claims-json-serialization-opt-in
description: Distinguish intentional opt-in OIDC/JWT claims JSON publication from the deferred duplicate OIDC serialization optimization.
knowledge_type: review-evidence
---

# OIDC / JWT claims JSON publication is opt-in

## Current scope

OIDC set_userinfo_header and store_claims_in_ctx and JWT store_claims_in_ctx
all default to false in their resource configuration Default implementations.
When enabled, publishing JSON is the requested feature. An upstream header must
exist before forwarding, and PluginSession::set_ctx_var accepts text; the real
PingoraSessionAdapter copies that text into request-owned context. Deferring all
serialization until an unspecified future read would change these interfaces
and semantics. Default-off does not prove the enabled path is efficient.

Current sources are `edgion-resources/src/resources/edgion_plugins/plugin_configs/`
(openid_connect.rs and jwt_auth.rs), Gateway openid_connect/userinfo.rs,
jwt_auth/plugin.rs and plugins/runtime/session_adapter.rs.

## Narrow optimization and historical assessment

OpenidConnect::apply_upstream_headers serializes the same claims twice when
set_userinfo_header and store_claims_in_ctx are enabled and userinfo_json is None.
The first result supplies fallback X-Userinfo; the second supplies oidc_claims.
Supplied userinfo is a separate document and cannot substitute for claims.

The earlier assessment rejected sharing that result because it expected little
benefit relative to review cost. Its typical-size and microsecond-cost statements
had no recorded benchmark or workload evidence. Preserve this as a historical
value judgment, not a measured result or a blanket reason to close new findings.
A reuse design must preserve independent output behavior and assess retention
across the intervening Authorization write. Final header/context copies remain.

The user's decision is tracked in
[shared OIDC claims serialization](../../../../task/review-data-panel/oidc-01-shared-claims-serialization.md).
No runtime change or performance measurement accompanies that deferral.

## Re-evaluation

Recheck defaults, actual call paths, ownership and serialization count when the
configuration, context API or publication sequence changes. Review large claims
and frequently enabled output combinations using measured evidence. Opt-in JSON
publication remains intended behavior; redundant producer work is a separate
optimization question. Do not use this page to suppress a demonstrated defect.
