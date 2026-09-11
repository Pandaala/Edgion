# JWT Auth Plugin Design

This document describes the implemented JwtAuth configuration, Controller resolution, and Gateway request behavior. JwtAuth validates JWTs with Kubernetes Secret-backed issuer groups; it does not issue tokens, fetch JWKS, or perform OIDC discovery.

---

## 1. Code Locations

- Gateway runtime: `edgion-gateway/src/plugins/http/jwt_auth/`
- Resource schema: `edgion-resources/src/resources/edgion_plugins/plugin_configs/jwt_auth.rs`
- Controller credential resolver: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/jwt.rs`
- Plugin registration: `EdgionPlugin::JwtAuth(JwtAuthConfig)`

---

## 2. Configuration Model

`issuerGroups` is the operator-facing credential model. Each group contains:

- A unique group name.
- One or more exact `iss` values.
- One or more Kubernetes `secretRefs`.
- Optional group-level `algorithm` and `audiences` overrides.

Plugin-level `algorithm` and `audiences` provide defaults. The Controller snapshot-merges these defaults into each resolved group. A group-level `audiences: None` inherits the plugin value. A group-level `audiences: []` explicitly overrides even a non-empty plugin default and disables validation. Only an effective `None` (both levels unset) or empty list disables validation.

Each referenced Secret represents one credential:

- `key`: identifier selected by the JWT payload claim named by `keyClaimName` (`key` by default).
- `secret`: HMAC material for HS256, HS384, or HS512.
- `publicKey`: PEM public key for RS256, RS384, RS512, ES256, or ES384.

A JOSE header `kid` or `keyid` is not a credential selector. The selector is read only from the configured payload claim.

---

## 3. Configuration Fields

| Field | Type | Default | Runtime meaning |
|-------|------|---------|-----------------|
| `issuerGroups` | Array | none | Named issuer and Secret groups; required unless `anonymous` is set |
| `algorithm` | `JwtAlgorithm` | `HS256` | Plugin default, overridden by a group value |
| `audiences` | Array | none | Plugin default audience allow-list, overridden by a group value |
| `header` | String | `authorization` | First token source; supports `Bearer <token>` or a bare token |
| `query` | String | `jwt` | Query parameter name |
| `allowTokenInQuery` | Boolean | `false` | Enables the query source; disabled by default to avoid URL credential exposure |
| `cookie` | String | `jwt` | Cookie token source |
| `hideCredentials` | Boolean | `false` | Removes the selected token source before proxying |
| `anonymous` | String | none | Allows only requests with no token and writes anonymous headers |
| `keyClaimName` | String | `key` | Payload claim used to select a credential within the routed group |
| `lifetimeGracePeriod` | Integer | `0` | Leeway in seconds for `exp` and present `nbf` claims |
| `base64Secret` | Boolean | `false` | `false` uses the Controller-provided UTF-8 bytes; `true` decodes that string as standard Base64 after Kubernetes has decoded the outer Secret `.data` layer |
| `maximumExpiration` | Integer | `0` | Maximum `exp` distance from the current time; `0` disables this additional cap |
| `authFailureDelayMs` | Integer | `0` | Delay before missing-token and invalid-token 401 responses |
| `realm` | String | `jwt` | Realm used in the Bearer `WWW-Authenticate` challenge |
| `storeClaimsInCtx` | Boolean | `false` | Stores verified claims in `jwt_claims` for downstream plugins |
| `claimsToHeaders` | Object | none | Explicitly maps verified claims to bounded upstream request headers; destinations are case-insensitive and unique per plugin instance |

Kubernetes always removes the outer `.data` Base64 encoding before the Controller reads a Secret value. With `base64Secret: false`, JwtAuth uses the resulting UTF-8 `secret` bytes directly. With `base64Secret: true`, that UTF-8 string must itself be standard Base64 and is decoded again into the actual HMAC key bytes.

---

## 4. Controller Resolution

For each configuration generation, the Controller:

1. Clears any previously resolved transport state.
2. Resolves every `secretRefs` entry through the normal Secret and ReferenceGrant path.
3. Reads `key` and either `secret` or `publicKey` from each usable Secret.
4. Keeps the first credential when a group contains duplicate `key` values.
5. Snapshot-merges group-level or plugin-level algorithm and audience policy.
6. Indexes the resolved group by every configured exact issuer string.

The resolved map is Controller-owned, redacted in configuration output, excluded from the CRD schema, and transported to the Gateway. A Secret change requeues the owning plugin resource and publishes a new plugin generation.

At Gateway construction, JwtAuth consumes this resolved map, prepares immutable decoding keys and validation policies once, and removes the raw resolved map from its retained configuration. Request processing performs only read-only lookups in that generation.

---

## 5. Request Flow

JwtAuth extracts one token with this priority:

1. Header.
2. Query, only when `allowTokenInQuery` is `true`.
3. Cookie.

If no token is present, `anonymous` may allow the request and writes `X-Anonymous-Consumer: true` plus `X-Consumer-Username: <anonymous>`. Without `anonymous`, the plugin applies `authFailureDelayMs` and returns the fixed 401 response.

If a token is present, JwtAuth always verifies it. A malformed, expired, immature, wrongly signed, wrongly scoped, or otherwise invalid token returns the fixed 401 even when `anonymous` is configured.

Verification order is intentionally strict:

1. Parse the JOSE header.
2. Decode untrusted payload selectors without accepting them as authenticated output.
3. Require a non-empty `iss` and route to its exact issuer group.
4. Require `header.alg` to equal the resolved group algorithm.
5. Read the configured payload key claim and look up that key only inside the routed group.
6. Verify the signature and registered claims with the prepared group policy.
7. Apply `maximumExpiration` when configured.

The unsigned selector values never enter a successful result. Claims become usable only after full verification.

---

## 6. Claim Validation

- `exp` is required and validated.
- `nbf` is optional; when present, it is validated.
- `lifetimeGracePeriod` is applied to both `exp` and `nbf` validation.
- A non-empty effective `audiences` list requires `aud` and at least one matching audience.
- Group `None` inherits the plugin value; group `[]` overrides even a non-empty plugin default. Validation is disabled only when the merged effective value is `None` (both levels unset) or empty.
- `maximumExpiration` accepts the same JWT NumericDate integer/floating-point forms as the JWT library and applies its rounding semantics before enforcing the cap.

The exact `iss` route, resolved algorithm, and group-scoped key lookup prevent issuer/key and algorithm-confusion crossover.

---

## 7. Success and Failure Behavior

On success, JwtAuth:

- Records `OK group=<name> iss=<iss> u=<sub>` in PluginLog; `sub` is handled as sensitive data and masked by the configured policy. An absent or non-string `sub` uses `(missing sub)`.
- Writes only headers explicitly configured through `claimsToHeaders`.
- Optionally stores verified claims in `jwt_claims`.
- Optionally removes the credential source selected for this request.

JwtAuth does not write `X-Consumer-Username` from `sub` by default.

All authentication failures use one public shape:

- Status: 401.
- Content type: `text/plain`.
- Body: `401 Unauthorized - Invalid or missing JWT`.
- Missing token: `WWW-Authenticate: Bearer realm="<realm>"`.
- Present invalid token: the same realm plus `error="invalid_token"` and generic `error_description="Unauthorized - Invalid or missing JWT"`.

Failure PluginLog entries are static classifications. They never contain the token, issuer, group, algorithm, key identifier, key material, or claims.

---

## 8. Boundaries

- JwtAuth uses Controller-resolved Kubernetes Secrets; it has no request-time Secret fetch.
- JwtAuth has no JWKS fetch, OIDC discovery, token issuance, or Consumer resource.
- Use the OpenID Connect plugin for OIDC/JWKS provider behavior.
- Query tokens remain opt-in because URLs can expose bearer credentials through logs, browser history, and referrers.
- Header projection is explicit and bounded; adding an implicit identity header is outside this plugin's current contract.
