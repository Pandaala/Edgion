# IpRestriction

`IpRestriction` allows or denies requests using ordered references to `EdgionConfigData` resources of type `IpList`. Author every IP/CIDR collection in that list's named `IpGroup` items. The Gateway compiles each list centrally and reads its current matcher; plugin configuration never contains or expands CIDRs.

## Example

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: office-networks
  namespace: edgion-default
spec:
  data:
    type: IpList
    config:
      items:
        - name: office
          cidrs: ["192.0.2.0/24", "2001:db8::/32"]
---
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: blocked-networks
  namespace: edgion-default
spec:
  data:
    type: IpList
    config:
      items:
        - name: blocked
          cidrs: ["192.0.2.100/32"]
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: office-access
  namespace: edgion-default
spec:
  requestPlugins:
    - type: IpRestriction
      config:
        ipSource: RemoteIp
        allowRefs:
          - name: office-networks
        denyRefs:
          - name: blocked-networks
        defaultAction: deny
        status: 403
```

Attach the `EdgionPlugins` resource through an HTTPRoute `ExtensionRef` filter. The same IpList can contain several named groups or be referenced by several policies. To manage independent collections, reference several lists instead of copying their CIDRs into each consumer.

## Configuration

| Field | Type | Default | Meaning |
|---|---|---|---|
| `allowRefs` | Array of EdgionConfigDataRef objects | Absent | Non-empty authored references enable whitelist mode. |
| `denyRefs` | Array of EdgionConfigDataRef objects | Absent | Ordered blacklist sources; matching deny takes precedence. |
| `ipSource` | `RemoteIp` or `DirectPeerIp` | `RemoteIp` | Canonical resolved client address or physical TCP peer. |
| `defaultAction` | `allow` or `deny` | `allow` | Decision when whitelist mode is absent and no deny matches. |
| `message` | String | Standard denial message | Message in the prebuilt JSON denial response. |
| `status` | Integer, 200–599 | `403` | Final HTTP status for a policy denial. |

At least one of `allowRefs` or `denyRefs` must be non-empty. A reference has a required non-empty `name` and an optional `namespace`; an omitted namespace means the consuming resource's namespace. Cross-namespace access follows the normal ReferenceGrant/visibility rules. Do not author Controller read proofs or denial markers.

The removed `allow` and `deny` collection fields are invalid, including `null`, empty arrays, and mixed old/new configurations. Their rejected values are discarded; bounded invalidity survives Controller-to-Gateway transport. Other unknown configuration keys retain the advisory warning contract.

## Decision order and list availability

1. Visit available deny lists in reference order. The first matching source denies the request.
2. With non-empty authored `allowRefs`, visit available allow lists in reference order. A match allows; otherwise deny as a whitelist miss.
3. Without whitelist mode, use `defaultAction` when no deny matches.

Within one IpList, the most specific matching prefix determines the named group. Reference source order takes precedence over a narrower prefix in a later list. Deny takes precedence even when the same address matches an allow source.

Unavailable references—missing, disabled, wrong type, unauthorized, or inconsistent resource identity—are omitted. Good siblings still contribute. An unavailable allow union keeps whitelist mode and denies all misses. An unavailable deny-only union preserves its configured default, which can allow traffic. Choose the default policy with this behavior in mind; ACL references do not become an all-required dependency gate.

Malformed owning policy/proofs fail construction and seal that runtime generation. Ordinary shared IP Conditions and Geo CIDR dependencies have their own required-reference contract: an unavailable required dependency fails the HTTP runtime's preflight with its configuration/dependency response, rather than becoming an ACL whitelist miss.

## Client address

`RemoteIp` uses the canonical RealIp result, falling back to the effective transport peer. Configure trusted proxy lists through RealIp's `trustedIpRefs` before relying on forwarded headers. `DirectPeerIp` always uses the physical peer and is independent of forwarded headers and PROXY protocol's advertised source.

IPv4, IPv6 and IPv4-mapped IPv6 membership use the shared canonical matcher. Bare addresses in an IpList become host prefixes. Partial-good collection admission also belongs to IpList: valid CIDRs survive invalid siblings, and list-local `PartiallyInvalid` status reports rejected entries. A completely empty available allow matcher still produces whitelist misses.

## Hot reload and publication

Changing an IpList's content, enable flag, type or visibility affects subsequent decisions without rebuilding the referring plugin. One dispatch uses one immutable ConfigData snapshot for both shared Conditions and ACL membership, including nested ExtensionRef execution. A held snapshot keeps its original generation; the next dispatch can observe deletion and restoration.

A published typed invalid policy generation replaces the previous executable policy with a sealed configuration outcome; publishing a clean generation restores it. This includes captured removed-field presence. An update rejected by Kubernetes admission or typed decoding never reaches publication and leaves the previously admitted generation in place.

## Denial response and logs

A normal policy denial returns the configured final status and a JSON object with a `message` member. Default status is 403. The body is prepared during configuration construction.

ACL access logs intentionally retain the matched group: `deny:group=blocked` or `allow:group=office`. Whitelist misses use `deny:not-in-allow`; default decisions use `allow:default` or `deny:default`. This is the documented ACL diagnostic exception; credential authentication failures must not expose credential-group identity.

## Stream policies

Connection-phase `IpRestriction`, TLS-route `IpRestriction`, and each `GlobalConnectionIpRestriction` profile also use `allowRefs`/`denyRefs`. L4 connection policies use the physical peer; TLS-route policies support their existing address selector. GCIR owns named scalar policies plus optional Selector-based active-profile selection; its IP collections remain live referenced IpLists. See [Stream plugins](../../../tcp-route/stream-plugins.md).
