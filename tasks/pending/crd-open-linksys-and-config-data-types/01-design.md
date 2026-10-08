# Open LinkSys and ConfigData extension discriminators

## Requirement and assessment

Open only `LinkSys.spec.type` and `EdgionConfigData.spec.data.type` so adding a
supported extension does not require a CRD whitelist update. Unknown software
extensions must decode, report `Accepted=False/Invalid`, and replace stale state.
Known malformed configurations and credential substructures retain their validation.

Use schema-hidden unit `Unsupported` sentinels, following the existing adjacent-tag
plugin implementation. Discard unknown config with `IgnoredAny`: no opaque payload
or attacker-controlled type text is retained, logged, or transported. Wire output
normalizes to the sentinel tag plus an empty `config: {}` required by the CRD;
it remains invalid after conf_sync deserialization. Only these resource sentinels
use explicit exhaustive serializers with a shared borrowed envelope and empty
object for unsupported config; existing plugin serialization stays unchanged.
This avoids the schemars 1.2.1 variant-local hook/skip conflict while retaining
`JsonSchema` derive and schema-hidden sentinels.
This intentionally does not provide transparent forwarding to newer software.

Removing only CRD enums leaves typed lists vulnerable to unknown variants; retaining
opaque configs creates unnecessary secret exposure. Neither alternative is selected.
No compatibility shims, watcher rewrite, new failure policy, or new telemetry.

## Data flow and changes

1. **Resources and CRDs:** add sentinels to `SystemConfig` and `ConfigEntry`, a
   bounded helper `SystemType::Unsupported`, and validation errors. Generalize the
   existing discard helper name if shared by plugins and resource extensions.
   Remove only the two extension discriminator enums. Keep config preservation,
   required fields, nested enums, auth strictness, and schema-hidden secrets.
2. **Controller:** extend LinkSys resolver exhaustive matching with a no-op branch.
   Existing parse must still clear Secret/cross-namespace edges and remove obsolete
   credential-source desired state/results. ConfigData parse must replace the raw
   target, clear outgoing WAF references, and requeue dependents. Status remains
   advisory to transport; unsupported objects must reach consumers to invalidate.
3. **Gateway:** map unsupported LinkSys to no provider so existing wrong-kind removal
   stops the previous runtime on full and partial updates. HTTPDNS uses current raw
   LinkSys type guards. ConfigData removes old typed indexes/proofs on transition;
   overlays retain fail-to-base, IP references retain their established semantics,
   and strict trust/WAF consumers reject the wrong type.
4. **Guards:** replace the obsolete ConfigEntry whitelist-parity script with an
   open-extension contract guard for both resources, update all callers, and add
   Rust schema/decoding tests. Update old LinkSys enum membership tests.
5. **Documentation:** update resource architecture, LinkSys extension guide,
   contributor/user references and CI documentation that mandate enum updates.

## Planned file ownership

- Implementation: the two resource modules, shared discard helper and plugin helper
  references, LinkSys Controller handler, Gateway LinkSys runtime/store and
  ConfigData store exhaustive matches; directly related unit-test files.
  Supporting exhaustive matches include common `link_sys_core/credential.rs`
  and Controller `handlers/san_guard.rs`; unsupported variants resolve no TLS.
- Guard: both CRDs, `cicd/checks/check_edgion_config_data_variant_parity.py` successor,
  its workflow/callers, resource CRD contract tests and affected enum assertions.
- Dynamic verification: existing LinkSys provider-replacement and ConfigData overlay
  scenarios/fixtures, or one focused shared-topology scenario if clearer.
  Final choice: extend `LinkSysProviderReplacement` and `RegionRouteValidation`.
- Coordinator: this task directory, original task status, associated documentation.

## Verification and review

- Decode mixed known/unknown typed Kubernetes lists and watch-shaped objects from
  JSON/YAML, with strict ingest on/off; test config-before-type, nested secret-like
  content discard, conf_sync round-trip and schema hiding. Known invalid configs,
  credential fields and nested enums must still reject. Unknown fields remain a
  separate advisory mechanism for supported types.
- Controller regressions: valid -> unknown -> valid, status generations,
  dependency cleanup/requeue and credential-source invalidation.
- Gateway regressions: full and partial valid -> unknown -> valid, retirement and
  recovery, raw map plus typed-index cleanup, overlay fallback and strict consumers.
- Add dynamic coverage using the shared topology and `wait_phase`; verify status,
  conf_sync and unchanged consumer behavior on unknown and recovery generations.
- Run format, workspace check/clippy, repository guards, focused crate tests, and
  relevant dynamic scenarios when the local topology can be prepared.
- Independent design review before implementation; at least two code review rounds
  after implementation, with all actionable findings resolved and rechecked.

## Verification questions resolved

Full/partial transition tests confirmed existing invalidation paths, credential-source
result cleanup and System fallback for unsupported HTTPDNS targets. Ingest tests
confirmed unknown payloads do not become advisory field findings. Unit tests use
local data; the two dynamic scenarios verified external behavior with test services.
The Admin regression and dynamic runs also verified normalized CRD envelopes.

## Scope and limitations

No commit or push is authorized. Existing unrelated worktree changes are excluded.
Live Kubernetes admission may be unavailable locally; record static schema guard
results separately from live API-server verification and do not conflate them.
