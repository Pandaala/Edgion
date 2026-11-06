# ctrl-svc-01: Controller Admin API proxies unredacted cluster-wide Secret read/write with the controller ServiceAccount

- **Category**: design-error
- **Severity**: P2 (downgraded from P1 by independent verification)
- **Scope**: large-direction
- **Location**:
  - `edgion-controller/src/api/mod.rs:392-526` (resource routes, no kind exclusion)
  - `edgion-controller/src/api/namespaced_handlers.rs:52-85` (list/get from source), `:305-310`, `:679-684` (Secret create/update)
  - `edgion-controller/src/api/common.rs:284` (`parse_kind` accepts every `ResourceKind`, including `Secret`)
  - `edgion-controller/src/api/types.rs:150-190` (`center_get_source`: no redaction, unlike `cache_list_resources` at `:196-210`)
  - `edgion-controller/src/conf_mgr/conf_center/kubernetes/source_query.rs:76-137` (direct apiserver GET/LIST as the controller SA)
  - `edgion-controller/src/authz/mod.rs:83-118` (policy = verbs x kinds, no namespace dimension)
  - `edgion-controller/src/api/authz_middleware.rs:124-170` (only denials are logged with token identity)
  - `cicd/deploy/kubernetes/controller/rbac.yaml:31-46` (cluster-wide Secret and EndpointSlice create/update/patch/delete)

## Summary
In Kubernetes mode the Admin API is a generic proxy onto the apiserver that runs with the
controller's ServiceAccount. Because `Secret` is an ordinary `ResourceKind`, a bearer token holder
can list/get every Secret in every watched namespace (all namespaces by default) **unredacted**,
including `edgion-conf-sync-ca` (the internal CA private key) and `edgion-admin-credentials`, and can
create/overwrite/delete Secrets cluster-wide. The shipped ClusterRole grants write only on Secrets
and EndpointSlices (Edgion CRDs and Gateway API routes are read-only), so in the canonical deployment
the admin write surface that actually works is precisely the most sensitive one.

## Current behavior (evidence)
- `GET /api/v1/namespaced/{kind}` / `{kind}/{namespace}` / `{kind}/{namespace}/{name}` call
  `center_list_resources*` / `center_get_resource` → `ConfCenter::get_source_one` →
  `KubernetesStorage::source_one`, which issues a raw `GET` against the apiserver
  (`source_query.rs:85-93`) and returns the JSON verbatim. `namespace_allowed` only enforces
  `watch_namespaces`; with the default (empty) list every namespace is allowed (`:119-131`).
- Redaction exists only on the conf_sync-cache path (`types.rs:200-206`,
  `AdminRedactGuard`); the source path used by the resource routes has none, and a core `Secret`
  object has no `redact_*` adapters anyway — `.data` is returned base64 as-is.
- `POST/PUT/DELETE /api/v1/namespaced/Secret/...` parse a `k8s_openapi` `Secret` and call
  `writer.create_one/update_one/delete_one` (`namespaced_handlers.rs:305-310`, `:679-684`), which
  in K8s mode becomes an apiserver write as the controller SA. `rbac.yaml:31-37` grants
  `create/update/patch/delete` on Secrets cluster-wide (needed for the CA and credential manager),
  so these succeed; `HTTPRoute`/`EdgionPlugins` writes would be 403 from the apiserver because the
  ClusterRole only grants `get/list/watch` there (`rbac.yaml:47-55`).
- Authorization is `verbs x kinds` only (`authz/mod.rs:83-118`); a scoped `cli_tokens` entry
  cannot be limited to a namespace. Allowed requests are not logged with the token name (only the
  deny branch logs `token_name`, `authz_middleware.rs:151-168`), and in the K8s audit log every
  write is attributed to `system:serviceaccount:edgion-system:edgion-controller`.
- The same router is mounted on the federation reverse tunnel (`cli/mod.rs:600-609`), so a Center
  policy with `kinds: ["*"]` inherits the same Secret access.
- Transport is plaintext HTTP (`api/mod.rs:595-603`); the bearer token travels in cleartext unless
  the operator adds a NetworkPolicy/mesh.

## Industry comparison
- **Istio istiod** debug endpoints (`/debug/configz`, `/debug/syncz`) are read-only and istiod never
  writes user config; config changes go through the apiserver so K8s RBAC and audit apply to the
  real user. Secrets are never dumped (SDS material is redacted in `config_dump`).
- **Envoy admin** `/config_dump` redacts secrets (`include_eds`, `SecretsConfigDump` shows only
  names/versions unless explicitly asked, and it is loopback-bound by default).
- **Kong Ingress Controller / DB-less**: the Admin API is read-only; all writes go through CRDs.
- **Envoy Gateway / Traefik CRD provider**: no write admin API in Kubernetes mode at all.

## Impact
- A single leaked admin (or broad scoped) token yields cluster-wide Secret exfiltration and
  tampering, including the conf_sync CA key (forge gateway or controller identities, receive every
  synced Secret) — a privilege escalation from "gateway admin" to "cluster Secret admin".
- K8s RBAC and audit are bypassed: the actual actor is invisible to the cluster audit trail and
  Edgion logs allowed writes without the token identity.
- The admin write surface is inconsistent across modes (FS/etcd: everything writable; K8s:
  only Secrets/EndpointSlices effectively writable), so tooling built against it behaves
  differently by backend.

## Suggested direction
Options:
1. (Recommended) In Kubernetes mode make the Admin API resource routes **read-only** and exclude
   `Secret` from source reads, or return Secrets with `.data`/`.stringData` replaced by key names
   only. Writes in K8s mode should be done via `kubectl`/GitOps so RBAC and audit apply to the real
   actor. Keep writes for FS/etcd backends where the controller is the source of truth.
2. If K8s-mode writes must remain, deny `Secret` (and `EndpointSlice`) kinds at the classifier,
   add a namespace dimension to `CenterPolicy` rules, and emit an allow-path audit record
   (`token_name`, verb, kind, namespace/name) for every mutating request.
3. Independently, narrow the RBAC: Secret write could be a namespaced Role in the controller
   namespace plus the namespaces that actually host credential-manager consumers.

## Notes
- Checked `skills/04-review/admin-api/` (gateway `/configclient` redaction is mandatory-design for
  the **gateway**; nothing covers controller source reads of Secrets),
  `admin-api-write-precondition-opt-in.md` (concurrency only), `security/mandatory-design/iron-rules.md`.
- The K8s-mode `create/update` support for Secrets is not documented as an intentional design
  anywhere I found; no `tasks/` item tracks it.

## Verification (independent)

- **Verdict**: DOWNGRADED
- **Severity**: P2 (was P1)
- `edgion-controller/src/api/common.rs:284-286`: `parse_kind` accepts every `ResourceKind` incl. `Secret`; `namespaced_handlers.rs:305,679` handle Secret create/update.
- `edgion-controller/src/api/types.rs:169-191`: `center_get_source` returns the source document with no redaction.
- `cicd/deploy/kubernetes/controller/rbac.yaml:31-37`: cluster-wide Secret create/update/patch/delete (needed by CA/credential manager), so K8s-mode Admin writes succeed as the controller SA.
- `edgion-controller/src/authz/lint.rs:70-95`: the project already warns that granting read on `Secret`/`*` exposes credential data — Secret exposure through the Admin API is a known, intentional surface for the allow-all admin token.
- Downgraded to P2: exploitation requires possessing the root Admin token (or a scoped token deliberately granted Secret/*). The actionable gaps are: no namespace dimension in policy, K8s audit attribution to the controller SA, and K8s-mode write paths that bypass cluster RBAC.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P2 (unchanged)
- `edgion-controller/src/api/`: only `common.rs` (serde_yaml→yaml_serde rename) and `validate_handlers.rs` (1 line) changed; `parse_kind`, Secret create/update in `namespaced_handlers.rs`, and unredacted `center_get_source` in `types.rs` unchanged.
- `kubernetes/source_query.rs` changed only the YAML crate; `authz/` and `cicd/deploy/kubernetes/controller/rbac.yaml` unchanged (cluster-wide Secret write, no namespace dimension in policy).
