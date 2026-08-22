# LinkSys auth resolved Secrets must be cleared before resolution

## Conclusion

`SecretSlot.secret` is Controller-resolved transport state. Every LinkSys auth
resolution attempt must clear it before reading `secretRef`, then populate it
only with the successful resolver result.

## Core rationale

LinkSys provider config is accepted below a preserve-unknown CRD node, while
unknown-field detection is advisory. `#[schemars(skip)]` hides `secret` from a
generated schema but does not prevent serde input. Without clear-before-resolve,
an operator-authored or stale `secret` survives a missing, denied, or invalid
`secretRef` and can be consumed by the Gateway despite `ResolvedRefs=False`.

The canonical clear belongs in `drive_standard`, immediately before
`AuthSpec::resolve`, so Redis, Etcd, Elasticsearch, Kafka, and future standard
auth providers share one boundary.

## Fix suggestions not accepted

- Rely on `UnknownConfigFields`: it is deliberately advisory and cannot enforce
  a credential boundary.
- Clear only in resolver error branches: early returns and future error paths
  can retain state; clear-before-attempt is the invariant.
- Mark the field `serde(skip)`: conf_sync must serialize the real resolved
  Secret to the Gateway.

## Re-evaluation triggers

Re-evaluate only if operator and conf_sync resource shapes are separated into
distinct types so Controller-resolved fields cannot be deserialized at operator
ingress.

## Reference case

- UFD-01 in [`CLOSED-FINDINGS.md`](../CLOSED-FINDINGS.md)
- Regression: `test_secret_not_found_clears_injected_secret_and_returns_pending`
