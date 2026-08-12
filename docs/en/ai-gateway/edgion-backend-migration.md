# EdgionBackend Stop-the-World Migration

The legacy AI backend resource and `EdgionBackend` are wire-incompatible kind-23 resources. A rolling upgrade,
partial binary rollback, CRD-only rollback, or mixed old/new object graph is unsupported.

## Cutover

1. Export the old CRD, all old backend objects, referencing HTTPRoutes, ReferenceGrants, backend
   traffic policies, relevant Secrets, RBAC, and deployment manifests. Keep resource versions and
   status only as evidence; prepare clean whole-object restore manifests.
2. Generate and validate the complete replacement graph offline: the new CRD, nested
   `EdgionBackend.spec.ai` objects, updated route and policy kinds, RBAC, and coordinated Controller
   and Gateway images.
3. Stop every old Controller and Gateway. Verify that no old process can reconcile, watch, or serve
   configuration sync before changing APIs.
4. Install the new CRD and RBAC, apply the complete converted backend/route/policy graph, then remove
   the old objects and CRD. Do not patch only `spec` in place.
5. Start only the coordinated new Controller and Gateway binaries.
6. Verify that discovery exposes only `edgionbackends.edgion.io`; RBAC permits the new plural and
   status but not the old plural; strict server dry-run succeeds; Admin/config-client paths use
   `edgionbackend`; all resources reach the expected conditions; and representative AI traffic and
   Secret rotation succeed.

## Rollback

1. Stop every new Controller and Gateway before restoring any old API or object.
2. Remove the new resource graph and new CRD/RBAC as one coordinated change.
3. Restore the saved old CRD/RBAC and clean old-shape backend, route, policy, ReferenceGrant, and
   deployment objects as whole objects. Never send a new nested kind-23 payload to an old binary.
4. Start only the previous coordinated binaries, then verify old discovery, status, configuration
   sync, and representative traffic.
