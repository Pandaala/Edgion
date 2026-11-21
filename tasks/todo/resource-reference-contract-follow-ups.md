# Resource reference contract follow-ups

## Meta

| Key | Value |
| --- | --- |
| Created | 2026-11-12 |
| Status | pending |
| Type | refactor |
| Priority | P3 |
| Issue | N/A |

## AI guide summary

The shared resource reference contract closed on 2026-11-12. Its canonical
knowledge is
[06-resource-reference-contract.md](../../skills/01-architecture/00-common/06-resource-reference-contract.md).
Three items from its specification remain unmet: user documentation (spec §8),
permanent dynamic scenarios (spec §7.3), and consolidation of the inline custom
`core` alias checks. The original specification remains in git history:

```bash
git show d4fb7f4ad:tasks/pending/resource-reference-contract/01-spec.md
```

Each item below can be done independently. Revalidate the cited sources before
starting.

## (a) User documentation for reference defaults (spec §8)

### Evidence

- Spec §8 required "a concise reference-default table and custom convenience
  field manifest in relevant English/Chinese user docs", clarifying `core` as a
  group concept versus the literal string `core`, and explaining why
  ExtensionRef requires `edgion.io/EdgionPlugins` and why ReferenceGrant targets
  use the empty group.
- Current coverage is piecemeal: `docs/en/user-guide/http-route/backends/service-ref.md`
  (native backendRef rejects literal `core`),
  `docs/en/user-guide/http-route/filters/edgion-plugins/credential-injector.md`
  (custom field accepts literal `core`), and
  `docs/en/ops-guide/infrastructure/reference-grant.md` (around line 161: Grant
  targets use `""`, not `core`). The Chinese counterparts mirror these pages.
  No user doc explains why ExtensionRef requires the `edgion.io` group.
- `docs/en/ops-guide/infrastructure/connectors.md` (LinkSys `secretRef` table,
  around line 162) says `group` is "Reserved for future non-Secret sources;
  leave unset", although the code accepts literal `core` on that field (see the
  canonical page's finite custom convenience manifest).

### Acceptance

- [ ] One concise reference-default table plus the custom-convenience field
      manifest exists in `docs/en` and `docs/zh-CN` (for example in
      `ops-guide/infrastructure/reference-grant.md`), linked from the piecemeal
      pages instead of duplicated there.
- [ ] It explains empty group as Kubernetes core, literal `core` as a finite
      Edgion custom convenience, and native fields rejecting literal `core`.
- [ ] It explains why ExtensionRef requires exact `edgion.io/EdgionPlugins` and
      why ReferenceGrant targets for core objects use the empty group.
- [ ] The LinkSys `auth.group` row in `connectors.md` (both languages) matches
      the implemented behavior.

## (b) Permanent dynamic reference scenarios (spec §7.3)

### Evidence

- Spec §7.3 required representative dynamic phases: valid omitted, then
  invalid explicit native `core`, then omitted recovery; Grant delete/recreate
  (allowed, revoked with no new target use, allowed recovery) for Gateway
  serving, client and CA material, backendRefs and native mirrors; and Grant
  from/to updates that revalidate dependents of both the old and new terms.
- Existing permanent scenarios in `edgion-tests/src/client/dynamic/scenarios/`
  cover CredentialInjector, RequestMirror ReferenceGrant
  (`RequestMirrorReferenceGrant`, which covers native mirrors), BTLSP and plugin
  reference flaps, but not the native `core` or Gateway material phases.
- backendRef Grant delete/restore has status-only checks in
  `edgion-tests/src/client/suites/status/ref_grant/status_test.rs`
  (`delete_reference_grant`); "no new target use" traffic was checked only in
  the manual Kubernetes run on `5e359f300`. Grant from/to updates have dispatch
  unit tests in
  `edgion-controller/tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/reference_grant_test.rs`.
- The 2026-11-12 source-mode parity run and the frontend-CA / backend
  client-certificate matrix were manual one-off drivers. The matrix alone was
  limited to FS mode, Secret CA and default frontend validation.

### Acceptance

- [ ] A dynamic scenario covers native omitted -> literal `core` -> omitted
      recovery on the shared topology, using event-driven waits.
- [ ] A dynamic scenario covers Grant delete/recreate for Gateway serving
      certificates, the backend client certificate and frontend CA material,
      including a ConfigMap CA and a `perPort` validation entry, and asserts
      both traffic and Gateway/listener `ResolvedRefs` status.
- [ ] backendRef Grant delete/recreate asserts traffic (no new target use while
      revoked, recovery afterwards), not only status.
- [ ] A Grant from/to update case asserts that dependents of both the old and
      new terms are revalidated, or a recorded decision keeps that coverage at
      the unit level.
- [ ] Kubernetes-suite coverage or a recorded scope exception exists for each
      new scenario.

## (c) Consolidate inline custom `core` alias checks

### Evidence

- `is_core_api_group` in `edgion-resources/src/resources/common/mod.rs` is the
  documented helper for the finite custom `core` alias.
- The same alias is applied inline elsewhere:
  - `OwnedTarget::prepare_in_namespace` in `edgion-resources/src/resources/refs/owned.rs`
    (`group == "core"` for custom Secret/ConfigMap fields);
  - `PolicyTargetRef::canonical_group` in `edgion-resources/src/resources/refs/targets.rs`;
  - `secret_ref_validation_errors` in `edgion-resources/src/resources/edgion_backend.rs`.
- Spec §1 named the duplication as a problem ("several consumers duplicate that
  predicate") and set "one canonical interpretation" as the goal. The reuse-first
  rules in `skills/00-design-rules.md` §E apply.

### Acceptance

- [ ] Every custom-field `core` alias decision goes through one canonical
      helper; native fields still never use it.
- [ ] Existing finite-manifest behavior is unchanged, verified by focused unit
      tests for each call site (accepted on custom fields, rejected on native
      fields and strict ACME/EdgionTls fields).
- [ ] The canonical knowledge page no longer lists inline exceptions.
