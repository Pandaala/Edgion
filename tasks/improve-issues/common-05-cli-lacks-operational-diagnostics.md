# common-05: `edgion-cli` is CRUD-only — no sync-status, config-diff, describe or diagnostics commands that istioctl / deck operators rely on

- **Category**: best-practice
- **Severity**: P3 (direction/nice)
- **Scope**: large-direction
- **Location**: `edgion-cli/src/cli/mod.rs:54-115` (`Commands`), `edgion-cli/src/cli/client.rs:238-270` (endpoints used), `edgion-cli/src/cli/output.rs:106-160` (table columns); controller endpoints in `edgion-controller/src/api/mod.rs:395-510`

## Summary
The CLI exposes `apply`/`get`/`delete` against the Controller, raw `get` against the Controller's
`/configserver` cache or one Gateway's `/configclient`, plus offline tools (`gen-certs`, `gen-token`,
~1.7k lines of `wasm`/`waf` inspection and Envoy-Wasm conversion). It has no command to answer the
first questions an operator asks during an incident: *is every Gateway in sync? why is this route not
Accepted? what does the data plane actually hold vs the controller? what would change if I apply this?*
Several of these answers already exist as Controller endpoints the CLI never calls.

## Current behavior (evidence)
- `Commands` (`mod.rs:54-115`): `Apply`, `Get`, `Delete`, `GenCerts`, `Login`, `GenToken`, `Wasm`, `Waf`.
- Controller endpoints with no CLI surface: `/api/v1/server-info`, `/api/v1/validate/{kind}`,
  `/api/v1/diagnostics/conf-conflicts`, `/api/v1/diagnostics/unknown-fields`,
  `/api/v1/diagnostics/nested_ext_ref_usage`, `/api/v1/region-routes/effective`,
  `/api/v1/debug/store-stats`, `/api/v1/count/...`, cluster-region failover preview.
- `get` table/wide output (`output.rs:124-160`) shows only name/namespace/kind/age (+labels,
  resourceVersion); status conditions (`Accepted`, `ResolvedRefs`, `Programmed`) are not shown, so
  "why is my route ignored" requires `-o yaml` and manual reading.
- `--target client` talks to one Gateway admin port at a time (`DEFAULT_PORT_CLIENT = 12001`), and
  there is no command that compares controller `sv` with each Gateway's applied `sv`.
- `apply` is per-document create-or-PUT with no diff, no prune, no ordering (`apply.rs`).
- `TargetType::Center` / `--target center` still names the federation Center that was extracted to the
  EdgionCenter repo; it actually means "Controller CRUD API" (`client.rs:35-37`).

## Industry comparison
- `istioctl proxy-status` (per-proxy sync state vs istiod, by xDS type and nonce),
  `istioctl proxy-config route|cluster|listener` (what the proxy actually holds),
  `istioctl x describe pod/svc` (which routes/policies apply and why), `istioctl analyze`.
- Kong `deck gateway diff` / `sync` / `dump` (declarative, idempotent, prune-aware).
- `kubectl get httproute` with printer columns showing conditions; Envoy Gateway's
  `egctl config envoy-proxy` and `egctl x status`.

## Impact
Operational debugging of the Controller↔Gateway split relies on ad-hoc curl against undocumented admin
ports. The sv/rv/key_name correlation that design rule B mandates is not reachable from the tool.
Meanwhile CLI effort went into offline Wasm/WAF tooling, which is valuable for migration but not for
day-2 operations.

## Suggested direction
Prioritized, smallest-first:
1. `edgion-cli status` → `/api/v1/server-info` + conditions summary per kind (printer columns
   `ACCEPTED`/`PROGRAMMED`/`REASON` in `get` table output).
2. `edgion-cli validate` / server-side dry-run (see common-04) and `edgion-cli diagnostics
   {conflicts,unknown-fields}`.
3. `edgion-cli sync-status`: requires a small Controller endpoint listing connected Gateways with their
   last acknowledged `sv` per kind (the conf_sync server already tracks watchers); this is the
   `proxy-status` equivalent and the highest-value addition.
4. `edgion-cli diff -f <dir>` (client-side compare with `/api/v1/namespaced/..`) before considering
   `sync --prune`.
5. Rename `--target center|server|client` to `controller|controller-cache|gateway`.

## Notes
No review page or task covers CLI scope (searched `skills/04-review` and `tasks/` for `edgion-cli`,
`proxy-status`, `diff`). Pre-release, so renaming the target enum carries no compatibility burden.
Kept as P3 because it is product usefulness, not correctness; common-04 is the correctness part.

## Verification (independent)

- **Verdict**: CONFIRMED
- **Severity**: P3
- `edgion-cli/src/cli/mod.rs:76-167`: commands are Apply/Get/Delete/GenCerts/Login/GenToken/Wasm/Waf only.
- `edgion-controller/src/api/mod.rs:463,491-500`: `/api/v1/server-info` and `/api/v1/diagnostics/{nested_ext_ref_usage,conf-conflicts,unknown-fields}` exist with no CLI surface.
- `edgion-cli/src/cli/client.rs:35-37`: `Center` target port comment says it is the Controller CRUD API (stale federation naming).
- Kept at P3: operability, not correctness; the sync-status (proxy-status equivalent) item is the one worth prioritizing.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P3
- `Commands` in `edgion-cli/src/cli/mod.rs` is unchanged; `output.rs` only switched `serde_yaml` to `yaml_serde` and still shows no status conditions.
- No new sync-status/diagnostics CLI commands were added.
