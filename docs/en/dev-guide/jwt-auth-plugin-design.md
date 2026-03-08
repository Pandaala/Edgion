# JWT Auth Plugin Design and Configuration (Feature/Config Preparation)

This document corresponds to the **feature and configuration preparation** phase of the plugin development process: defining the plugin struct and configuration items, comparing implementations from APISIX/Kong/Traefik/Envoy, determining feature logic and configuration for review before implementing runtime logic and tests.

---

## 1. Directory and Code Locations

- **Plugin directory**: `Edgion/src/core/plugins/edgion_plugins/jwt_auth/`
  - `mod.rs`: Exports `JwtAuth`
  - `plugin.rs`: Plugin implementation (currently a stub, runtime logic to be added after review)
- **Configuration types**: `Edgion/src/types/resources/edgion_plugins/plugin_configs/jwt_auth.rs`
  - `JwtAuthConfig`, `JwtAlgorithm`
- **Registration**: `EdgionPlugin::JwtAuth(JwtAuthConfig)` is registered in `edgion_plugin.rs` and `runtime.rs`.

---

## 2. Feature Comparison: Edgion Design vs Kong vs APISIX

The following table compares Edgion's current design with Kong JWT and APISIX jwt-auth by feature dimension, for review and future iteration.

| Feature | APISIX jwt-auth | Kong JWT | Edgion Design | Notes |
|---------|-----------------|----------|---------------|-------|
| **Credential model** | Consumer + Credential (key/secret/public_key, etc.) | Consumer + JWT Credential (key/secret/rsa_public_key) | Route-level `secret_ref` (single key) or `secret_refs` (multi-key, K8s Secret) | Edgion has no independent Consumer resource; credentials come from K8s Secret references |
| **Key storage** | etcd + optional APISIX Secret (env/vault) | DB + optional vault, etc. | K8s Secret only (secret_ref / secret_refs) | Edgion v1 does not support env/vault references |
| **Token source** | header / query / cookie (configurable names) | Authorization / uri_param_names / cookie_names | header / query / cookie (configurable names) | All three are consistent |
| **Token priority** | Header > Query > Cookie | Same | Header > Query > Cookie | Consistent |
| **Algorithms** | HS256/HS512/RS256/ES256 | HS256/HS384/HS512/RS256/RS384/RS512/ES256/ES384/ES512 | HS256/HS384/HS512/RS256/ES256 | Edgion v1 lacks RS384/RS512/ES384/ES512 |
| **Multi-key selection** | payload key_claim_name value selects Consumer credential | Same (key_claim_name) | key_claim_name selects from secret_refs | Same logic, different data source |
| **hide_credentials** | Yes (don't forward token to upstream) | Yes | Yes | Consistent |
| **Anonymous access** | anonymous_consumer (consumer name) | anonymous (consumer id) | anonymous (consumer name) | Consistent |
| **key_claim_name** | Yes (default: key) | Yes | Yes (default: key) | Consistent |
| **exp/nbf validation** | Yes | Yes | Yes (+ optional claims_to_verify) | Consistent |
| **Clock skew** | lifetime_grace_period | - | lifetime_grace_period | Kong docs don't emphasize this; Edgion aligns with APISIX |
| **base64_secret** | Yes (credential-level) | secret_is_base64 (credential-level) | Not in v1, can be added later | Can be addressed in implementation phase |
| **Credential-level exp** | Yes (default token expiry in Consumer credential) | - | No | Edgion does not issue tokens, only validates |
| **Token issuance** | Yes (Admin API creates JWT for Consumer) | No (validation only) | No | Edgion aligns with Kong, validation only |
| **store_in_ctx** | Yes (store payload in ctx for downstream plugins) | - | No | Not in Edgion v1 |
| **Headers forwarded to upstream** | X-Consumer-Username, X-Credential-Identifier, custom Consumer headers | Configurable claim_to_headers, etc. | X-Consumer-Username (aligned with BasicAuth) | Edgion v1 only username/identifier, claim_to_headers extensible later |
| **JWKS / Remote public key** | No (credential is the key) | No | No | All three skip in v1; Edgion design reserves "JWKS extensibility" |
| **iss/aud validation** | No | Optional | No | Not in Edgion v1 |
| **Configuration level** | Consumer (credential) + Route/Service (behavior) | Consumer (credential) + Route/Service (behavior) | Route/Plugin level only (credentials via secret_ref(s)) | Edgion has no independent Consumer; configuration is flatter |

**Summary of differences**

- **Edgion has, aligned with APISIX/Kong**: Token source and priority, multi-key (key_claim_name), hide_credentials, anonymous, exp/nbf, lifetime_grace_period, algorithms (v1 subset).
- **Edgion differs**: Credentials from K8s Secret (secret_ref / secret_refs), no Consumer resource, no Admin API token issuance.
- **Edgion v1 deferred, iteratable later**: base64_secret, store_in_ctx, more algorithms (RS384/512, ES384/512), claim_to_headers, JWKS/iss/aud.

---

## 3. Benchmarking: APISIX / Kong (Brief)

### 3.1 APISIX jwt-auth

- **Consumer/Credential**: Each Consumer can configure jwt-auth credentials: `key` (required), `secret` (HS*), `public_key` (RS256/ES256), `algorithm`, `exp`, `base64_secret`, `lifetime_grace_period`, `key_claim_name`.
- **Route/Service** (route-level): `header` (default: authorization), `query` (default: jwt), `cookie` (default: jwt), `hide_credentials`, `key_claim_name`, `anonymous_consumer`, `store_in_ctx`.
- **Token source**: Header / Query / Cookie, priority generally Header > Query > Cookie.
- **After validation**: Sets `X-Consumer-Username`, `X-Credential-Identifier`, etc. as request headers forwarded to upstream.

### 3.2 Kong JWT

- Similar to APISIX: JWT plugin validates signatures and claims, supports token extraction from Header/Query/Cookie, configurable anonymous consumer, hide credentials, etc.

---

## 4. Benchmarking: Traefik / Envoy / Tyk (Differences and Trade-offs)

| Capability | Traefik | Envoy | Edgion Design |
|-----------|---------|-------|---------------|
| Key source | signingSecret / publicKey / jwksFile / jwksUrl | local_jwks / remote_jwks (with issuer) | v1: secret_ref / secret_refs (K8s Secret), JWKS URL extensible later |
| Multi-issuer | trustedIssuers + JWKS | Multiple Provider + rules | v1: Multiple credentials via secret_refs + key_claim_name |
| Token location | Header / Form / Query | from_headers / from_params / from_cookies | header / query / cookie (aligned with APISIX) |
| Claim validation | claims (custom rules) | exp, aud, iss, etc. | exp, nbf (optional claims_to_verify) + lifetime_grace_period |
| Forward claims | forwardHeaders | claim_to_headers | v1: X-Consumer-Username, etc., aligned with BasicAuth |
| Anonymous access | - | - | anonymous (aligned with BasicAuth) |

**Edgion v1 trade-offs**:

- **Not doing**: JWKS URL, iss/aud validation, custom claim rules (iteratable later).
- **Doing**: Single key (secret_ref) or multi-key (secret_refs + key_claim_name), HS256/384/512 and RS256/384/512, ES256/384/512, Header/Query/Cookie token extraction, hide_credentials, anonymous, exp/nbf with clock skew.
- **No Consumer**: Edgion has no independent Consumer resource; all credentials come from K8s Secrets (secret_ref / secret_refs), configuration is route/plugin-level only.

---

## 5. Feature Goals (Confirmed Logic)

1. **Token extraction** (priority: Header > Query > Cookie)  
   - Header: Default `authorization`, supports `Bearer <token>` or bare token.  
   - Query: Default parameter name `jwt`.  
   - Cookie: Default name `jwt`.

2. **Credential model**  
   - **Single issuer**: `secret_ref` points to one Secret containing `secret` (HS*) or `publicKey` (RS*/ES*), without looking up by key in the payload.  
   - **Multi-key**: `secret_refs` points to multiple Secrets, each containing `key` + `secret` or `key` + `publicKey`; uses the value of `key_claim_name` field in JWT payload to select the credential.

3. **Algorithms**  
   - Symmetric: HS256, HS384, HS512.  
   - Asymmetric RSA: RS256, RS384, RS512.  
   - Asymmetric ECDSA: ES256, ES384.  
   - Note: ES512 (P-521) is not supported due to underlying library limitations.

4. **Claims and time**  
   - Validate `exp`, `nbf` (if present).  
   - `lifetime_grace_period` (seconds) for clock skew.  
   - Optional `claims_to_verify` to explicitly specify claims to validate (e.g., `["exp","nbf"]`).

5. **Anonymous and upstream**  
   - No token or validation failure: If `anonymous` is configured, allow and set anonymous consumer identifier (e.g., X-Consumer-Username); otherwise return 401.  
   - Validation success: Set X-Consumer-Username (can be a claim from payload, e.g., `key` or future extensions), optionally hide credentials (`hide_credentials`).

6. **Error responses**  
   - No token / invalid token / signature or claim validation failure: 401 Unauthorized, can return JSON or plain text (aligned with BasicAuth style).

---

## 6. Configuration Items (Implemented JwtAuthConfig)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| secret_ref | SecretObjectReference? | - | Single key: Secret contains `secret` or `publicKey` |
| secret_refs | []SecretObjectReference? | - | Multi-key: Each Secret contains `key` + `secret` or `key` + `publicKey` |
| algorithm | JwtAlgorithm | HS256 | HS256/HS384/HS512/RS256/ES256 |
| header | string | "authorization" | Header name for token extraction |
| query | string | "jwt" | Query parameter name |
| cookie | string | "jwt" | Cookie name |
| hide_credentials | bool | false | Whether to remove token before forwarding to upstream |
| anonymous | string? | - | Anonymous consumer name, allows unauthenticated access |
| key_claim_name | string | "key" | Claim name in payload used for credential selection |
| lifetime_grace_period | u64 | 0 | exp/nbf clock skew (seconds) |
| claims_to_verify | []string? | - | Optional, e.g., ["exp","nbf"] |

**Constraints** (can be validated in handler or at runtime):  
- At least one of `secret_ref` or `secret_refs` must be configured.  
- When using `secret_refs`, `key_claim_name` must be configured and algorithm must be consistent (HS* uses secret, RS*/ES* uses publicKey).

---

## 7. Secret Data Format (Convention)

- **secret_ref (single key)**  
  - HS*: Secret contains key `secret` (raw or base64 to be determined during implementation).  
  - RS*/ES*: Secret contains key `publicKey` (PEM).

- **secret_refs (multi-key)**  
  - Each Secret:  
    - HS*: `key` (identifier) + `secret`.  
    - RS*/ES*: `key` (identifier) + `publicKey` (PEM).

---

## 8. Consistency with Existing Plugins

- Aligned with **BasicAuth**: `hide_credentials`, `anonymous`, 401 and upstream headers (X-Consumer-Username).  
- Consistent with **EdgionPlugins**: Registered through `RequestFilterEntry` and `PluginRuntime::add_from_request_filters`, supports conditional execution (conditions).

---

## 9. Post-Review TODO (Not Implemented in This Document)

- Implement plugin **runtime logic** (parse JWT, select key, validate signature and exp/nbf, set headers/anonymous/401).  
- **Credential loading**: Parse secret_ref / secret_refs from K8s Secrets (if pulling in conf sync or request path, plan required).  
- **Unit tests**: Mock session, cover with-token/no-token/bad-token/anonymous/hide_credentials.  
- **Integration tests**: Scripts and config directories per "plugin development" docs (`run_integration_test.sh`, `EdgionPlugins/PluginJwtAuth`, client suite).  
- **User documentation**: `Edgion/docs/zh-CN/user-guide/edgion-plugins/jwt-auth.md` (or corresponding path).

---

**Current status**: Directory created, configuration and stub integrated, awaiting review of **feature logic** and **configuration items**; after review approval, proceed to "post-review coding" and integration/user documentation phases.
