# Request Access Policy

`RequestAccessPolicy` is an L7 request plugin. It can be attached to a Gateway or to one HTTPRoute through an `ExtensionRef`.

## Evaluation model

A configuration contains named profiles and a required `defaultProfile`. An optional `activeProfileRef` selects a profile through Selector `EdgionConfigData`. Selector failures are hard failures and deny the request.

Each profile contains `requiredRuleGroups`. Every group must pass (AND). Within one group, any complete rule in `anyOfRules` may pass (OR). The complete rule types are:

- `directPeerIp`: matches the immediate TCP peer, then every `allOf` condition.
- `remoteIp`: matches the RealIp-resolved client, then every `allOf` condition.
- `url`: matches a path first, then an exact host and every `allOf` condition.

One group may mix all three rule types. This directly models alternatives such as an internal client IP or an authenticated vendor callback.

Inline IPs and all usable `ipRefs` in one complete IP rule are expanded by the Controller and compiled into one radix matcher. `configRefs` import typed `RequestAccessUrlAllowList` data. Missing, disabled, wrong-type, or unpermitted IP/URL data refs are ignored individually and reported through `PartiallyInvalid`; valid siblings continue to work. ConfigData and Secret changes dynamically requeue the owning `EdgionPlugins` resource.

`allOf` is the same `Condition` enum as plugin `run` / `skip` (including `timeRange` and `probability`). `keyMatch` uses the shared `MatchCore` (`values` / `regex` / `caseSensitive`, no singular `value`). See [Plugin Conditions](./conditions.md). Secret and HMAC material is resolved by the Controller and redacted from admin configuration output. `hmacAuth.validateRequestBody` is accepted only here (and later RequestRestriction), not on entry-level conditions.

### HMAC condition

The `hmacAuth` condition reuses the same credential loading and signature verification as the standalone `HmacAuth` plugin, but it is match-only. It accepts `secretGroups`, `algorithms`, `clockSkew`, `enforceHeaders`, `validateRequestBody`, `minSecretLength`, `secretField`, and `usernameField`. Standalone behavior fields such as `anonymous`, `realm`, `authFailureDelayMs`, `hideCredentials`, and `upstreamHeaderFields` are rejected.

```yaml
allOf:
  - type: hmacAuth
    secretGroups:
      - name: vendors
        secretRefs:
          - name: vendor-hmac-credentials
    algorithms: [hmac-sha256]
    enforceHeaders: [date, "@request-target"]
    validateRequestBody: true
```

A missing or invalid signature, an unknown credential, or a body-digest mismatch makes this condition not match. Another rule in the same `anyOfRules` group may still authorize the request. Body capture or storage failures that may leave no forwardable request body terminate policy evaluation through the gateway's canonical body-error response; they never fall through to a sibling rule. Credential metadata headers are ignored because a policy condition must not mutate the request.

## Example

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: dev-access
  namespace: edgion-system
spec:
  requestPlugins:
    - type: RequestAccessPolicy
      config:
        defaultProfile: lockdown
        status: 403
        profiles:
          lockdown:
            requiredRuleGroups:
              - name: trusted-previous-hop
                anyOfRules:
                  - name: internal-peer
                    type: directPeerIp
                    config:
                      ipRefs:
                        - name: internal-networks
                  - name: authenticated-cdn
                    type: directPeerIp
                    config:
                      ipRefs:
                        - name: cloudflare-networks
                      allOf:
                        - type: secretMatch
                          key: { type: header, name: X-Origin-Token }
                          secretRef: { name: cloudflare-origin-token }
                          secretKey: token
              - name: allowed-client
                anyOfRules:
                  - name: internal-client
                    type: remoteIp
                    config:
                      ipRefs:
                        - name: internal-networks
                  - name: vendor-callback
                    type: url
                    config:
                      hosts: [callbacks.example.com]
                      paths:
                        - type: PathPrefix
                          value: /callbacks/:vendor
                      allOf:
                        - type: remoteIpMatch
                          ipRefs:
                            - name: vendor-networks
```

`Exact` and `PathPrefix` entries support route-style `:parameter` segments and share one profile-wide radix tree. `PathPrefix` matches only at segment boundaries. `RegularExpression` uses a compiled Rust regex. Hosts are exact, case-insensitive matches after port and trailing-dot normalization; wildcard hosts are rejected.

The plugin always fails closed. Every required group must find a successful complete rule; configuration or profile-selection failures deny the request.
