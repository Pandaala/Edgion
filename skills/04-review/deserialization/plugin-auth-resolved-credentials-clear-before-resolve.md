# Plugin auth resolved credentials must be rebuilt by the Controller

## Conclusion

`KeyAuthConfig.resolved_keys` and `JwtAuthConfig.resolved_groups_by_iss` are
Controller-resolved transport state. Their resolvers must clear these maps
before inspecting operator-facing Secret references, then populate them only
from successfully resolved Secret data.

JWT configuration validation must not treat `resolved_groups_by_iss` as a
substitute for `issuer_groups`. Operator-facing validity is determined only by
operator-facing fields.

## Core rationale

Plugin configs are accepted below preserve-unknown CRD nodes, and unknown-field
detection is advisory. `#[schemars(skip)]` keeps resolved fields out of the CRD
schema, but it does not prevent serde from accepting them. Without
clear-before-resolve, an operator-authored or stale credential map can survive
an absent, denied, missing, or invalid Secret reference and cross the intended
Secret/RBAC trust boundary into the Gateway.

The canonical boundary is the beginning of each credential resolver, before
all early returns. A successful resolution pass may then rebuild the field.

## Fix suggestions not accepted

- Rely on unknown-field warnings: they are intentionally non-blocking.
- Clear only when Secret resolution reports an error: absent source fields and
  future early returns can still retain injected state.
- Mark the resolved fields `serde(skip)`: the Gateway needs their real values
  over conf_sync.
- Accept a resolved JWT map during validation: it makes Controller-owned state
  an operator-facing credential input.

## Re-evaluation triggers

Re-evaluate only if operator ingress and conf_sync use separate wire types that
make Controller-resolved fields impossible to submit through the CRD path.

## Reference case

- UFD-02 in [`CLOSED-FINDINGS.md`](../CLOSED-FINDINGS.md)
- Regressions:
  `parse_clears_injected_auth_credentials_without_source_refs`,
  `parse_clears_injected_auth_credentials_when_secrets_are_missing`, and
  `test_resolved_groups_cannot_replace_operator_issuer_groups`
