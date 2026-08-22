# Request Access Policy

> **Edgion extension**
>
> `RequestAccessPolicy` is an L7 request plugin in the `EdgionPlugins` CRD. Attach it to a Gateway through the Gateway plugin reference, or to an individual HTTPRoute through an `ExtensionRef` filter.

## Model

A configuration contains named profiles and a required `defaultProfile`. An optional `activeProfileRef` points to a Selector `EdgionConfigData` and switches profiles without changing the plugin resource. An unavailable, denied, wrong-type, or unknown selector result denies the request.

Each profile evaluates two distinct IP identities:

- `directPeerIp`: the immediate TCP peer. It is never rewritten and is the preferred security boundary.
- `remoteIp`: the client address resolved by RealIp from trusted proxy headers. Use it only when the trusted proxy configuration is correct.

`sourceMatch.operator` is `any` by default. With `all`, both IP sources must be configured and both branches must match.

Each source has two OR-ed rule lists:

- `ipRules`: match the source IP first, then evaluate optional auxiliary conditions.
- `urlRules`: match exact host and path first, then evaluate optional auxiliary conditions. Exact and prefix paths use a radix tree and support `:parameter` segments.

An omitted `anyOf` means the primary IP or URL match is sufficient. When present, `anyOf` is OR and each `allOf` is AND. Empty groups are rejected.

## Conditions

The supported `allOf` condition types are:

- `keyExist`: a local unified `KeyGet` value must exist.
- `keyMatch`: a local `KeyGet` value must match `value`, `values`, or `regex`.
- `secretMatch`: a local `KeyGet` value must exactly match one value from a referenced Secret.
- `ipMatch`: the current branch's IP must match an inline or ConfigData-backed IP set.
- `hmacAuth`: the request must pass the standard HMAC verifier. It performs no header mutation when embedded in this policy.

Webhook-backed keys are rejected because access decisions must not add a runtime network dependency. Secret values and resolved HMAC credentials are redacted from the admin configuration output.

## Example

This policy allows internal callers by direct peer IP, or Cloudflare callers that come from the configured CDN range and present the Secret-backed token. It also permits a vendor only on one exact host and route-shaped path.

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
        message: Access denied
        activeProfileRef:
          name: dev-access-selector
        profiles:
          lockdown:
            defaultAction: deny
            sourceMatch:
              operator: any
              directPeerIp:
                ipRules:
                  - name: internal
                    ips:
                      refs:
                        - name: internal-networks
                  - name: cloudflare
                    ips:
                      refs:
                        - name: cloudflare-networks
                    anyOf:
                      - allOf:
                          - type: secretMatch
                            key:
                              type: header
                              name: X-Origin-Token
                            secretRef:
                              name: cloudflare-origin-token
                urlRules:
                  - name: vendor-callback
                    urls:
                      - hosts: ["callbacks.example.com"]
                        paths:
                          - type: PathPrefix
                            value: /callbacks/:vendor
                    anyOf:
                      - allOf:
                          - type: ipMatch
                            refs:
                              - name: vendor-networks
```

An IP list reference points to `EdgionConfigData` with `type: IpList`:

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: internal-networks
  namespace: edgion-system
spec:
  data:
    type: IpList
    config:
      items:
        - name: private
          cidrs: ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16"]
```

## Path and host behavior

Hosts are exact, case-insensitive matches after removing a port and trailing DNS dot. Wildcard hosts are rejected. Every URL entry must contain at least one host and path.

Path types are:

- `Exact`: the whole normalized request path must match.
- `PathPrefix`: the path must match at a segment boundary. `/api` does not match `/apiv2`.
- `RegularExpression`: the configured Rust regex must match the normalized request path.

Static and parameterized Exact/PathPrefix entries are indexed by the shared path radix tree.

## Failure behavior

The plugin fails closed. Invalid runtime initialization, an unavailable selected profile, an unresolved selector, an unresolved IP allow-list reference, a missing Secret value, or failed HMAC verification cannot authorize a request. A profile's `defaultAction` is applied only after normal rule evaluation; it does not turn configuration failures into authorization.

The plugin records `allow:<source>:<rule>`, `allow:default`, or `deny:no-match` in its access-log entry.
