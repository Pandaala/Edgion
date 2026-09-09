# Reconcile Kubernetes Integration Suite Inventory

## Meta

- Created: 2026-09-09
- Status: pending
- Type: bugfix
- Priority: P2
- Issue: N/A

## Evidence

External commit `bfc2691e` references this follow-up but omitted its record.
Independent diagnosis reran `bash edgion-tests/k8s/scripts/common/validate_suite_parity.sh`
on 2026-09-09: exit 1. Static canonical/Kubernetes counts are 158/170 with
TCPRoute, TLSRoute and UDPRoute `DeniedBackendRef` missing; dynamic counts 33/29
miss AllEndpointStatusTls, DirectEndpointPortResolution, RegionRouteValidation,
and ServiceDegradeThreshold; isolated counts 11/8 miss ldap_auth_egress,
oidc_egress, and proxy_protocol_policy_lifecycle. Final gates are 4/4. Fifteen
Kubernetes-only static entries are reported but are not the failure condition.

Relevant runner/inventory/validator inputs were unchanged by that external commit;
this gap is independent of its new HTTP/1-only gRPC-Web coverage and B0203.
See `skills/04-review/h2-grpc/regression-guards/grpc-web-independent-of-frontend-http2.md`.

## Acceptance Criteria

- Trace missing cases to fixtures, dispatch, cleanup and real assertions;
  distinguish unimplemented coverage from missing registration.
- Add executable coverage or obtain and document a deliberate scope exception.
  Do not add markers alone, delete canonical cases or weaken the validator.
- Preserve shared topology, justified isolation, fail-closed execution, dynamic
  cleanup and final gates.
- Run the parity validator and
  `bash edgion-tests/k8s/scripts/common/tests/validate_suite_parity_test.sh`.
- Verify newly wired cases in Kubernetes; report unavailable live checks honestly.
- Existing integration files remain frozen during the optimization campaign;
  implementation requires separately authorized test changes.
