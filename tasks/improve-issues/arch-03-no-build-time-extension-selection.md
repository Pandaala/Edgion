# arch-03: Every subsystem is always compiled into the Gateway — no build-time extension selection

- **Category**: over-design (packaging) / best-practice
- **Severity**: P3 (downgraded from P2 by independent verification)
- **Scope**: large-direction
- **Location**:
  - `edgion-gateway/Cargo.toml:19-45` (features: only allocator, TLS backend, `fips`)
  - `edgion-gateway/Cargo.toml` `[dependencies]`: `wasmtime` (Cranelift JIT), `edgion-waf` (~63k LOC),
    `ldap3`, `rskafka`, `fred`, `etcd-client`, `maxminddb`, `opentelemetry*`, `tonic` 0.12 **and** `tonic014`
    (0.14, only for OTLP), `reqwest`, `instant-acme`/`rcgen` (no non-test use in `edgion-gateway/src`)
  - `edgion-gateway/src/plugins/http/` (~55 HTTP plugin types), `edgion-gateway/src/ai/`

## Summary

The Gateway binary still unconditionally links every plugin, the native WAF engine, a full Wasm JIT runtime,
the AI gateway, GraphQL/gRPC transcoding, LDAP/Kafka/Redis/OTLP/Elasticsearch clients, and GeoIP. The only
runtime switch is `EdgionGatewayConfig.pluginPolicy` (allow/deny/bypass per plugin name), which disables
*execution* but not the code, dependencies, or their attack/CVE surface. The dependency-hygiene part of this
finding has been fixed on `feature-10-28`; what remains is the lack of a build-time "core vs. optional"
subsystem split.

## Current behavior (evidence)

- Gateway features (`edgion-gateway/Cargo.toml:18-45`): `test-util`, `allocator-*`, `boringssl`/`rustls`,
  `fips` — nothing else.
- Heavy optional subsystems are unconditional dependencies: `edgion-waf` (`:59`), `redis` (`:141`),
  `opentelemetry*` (`:155-157`), `ldap3` (`:172`), `rskafka` (`:173`), `wasmtime` (`:178`, Cranelift JIT;
  now with an explicit feature list in the workspace manifest, but still always compiled), `maxminddb`
  (`:182`).
- `pluginPolicy` (`edgion-resources/src/resources/plugin_policy.rs`) is a runtime execution filter; a denied
  plugin's code and its dependencies are still present.
- Fixed since the original report: `instant-acme` is no longer a gateway dependency, `rcgen` moved to
  `[dev-dependencies]` (`:198`), and the workspace converged on a single `tonic` 0.14 (`tonic014` removed).

## Industry comparison

- **Envoy**: `source/extensions/extensions_build_config.bzl` selects extensions at build time; distributions
  (Envoy Gateway, Istio proxy) ship curated subsets; Wasm runtimes (V8/Wasmtime/WAMR) are build options.
- **Caddy**: modules are compiled in via `xcaddy`; core stays small.
- **Traefik / Kong**: plugin code is loaded on demand (Yaegi/Wasm; Lua `plugins = bundled,...`), the core
  binary does not carry every integration client.

## Impact

- Larger image and longer builds; every CVE in Wasmtime/Cranelift, LDAP, Kafka, etc. is a Gateway CVE even for
  operators that never use those features (security scanners flag the image regardless).
- Harder to offer a minimal, auditable "core API gateway" build or a FIPS build whose crypto inventory is small.

## Suggested direction

1. Introduce cargo features on `edgion-gateway` for coarse optional subsystems: `wasm`, `waf`, `ai`,
   `graphql`, `ldap`, `kafka`, `geoip`, `otlp`; default = all (current behavior), and a documented `core`
   profile. Unknown/disabled plugin types already decode into the fail-closed `Unsupported` sentinel, so a
   compiled-out plugin naturally fails closed with `Accepted=False` once the controller reports gateway
   capability (or keep validation in the controller and fail closed in the gateway).
2. Add a `cargo machete`/`cargo udeps` CI gate so the dependency hygiene just achieved does not regress.

## Notes

- Do not confuse with the explicitly rejected "fifth fail-open shape" or plugin allow-list work; this is
  packaging only. No accepted-design page on build-time feature selection was found in `skills/04-review/packaging/`.

## Verification (independent)

- **Verdict**: DOWNGRADED
- **Severity**: P3 (was P2)
- `edgion-gateway/Cargo.toml:19-45`: features are only test-util, allocator-*, boringssl/rustls, fips.
- `edgion-gateway/Cargo.toml:176`: `instant-acme` is declared but `grep -rl instant_acme edgion-gateway/` finds no use at all; `rcgen` (`:177`) is used only under `edgion-gateway/tests/` (dev-dependency candidate).
- `edgion-gateway/Cargo.toml:92`: `tonic014` (0.14) coexists with workspace tonic 0.12.
- Downgraded to P3: build-time subsystem selection is a packaging/product choice with large cost; the actionable near-term part is dependency hygiene (drop `instant-acme`, move `rcgen` to dev-deps).

## Re-verification (feature-10-28)

- **Verdict**: PARTIALLY-FIXED
- **Severity**: P3 (unchanged)
- `git diff feature-08-30..HEAD -- edgion-gateway/Cargo.toml`: `instant-acme` removed, `rcgen` moved to `[dev-dependencies]` (`:198`), `tonic014` removed; root `Cargo.toml:102` now has one `tonic` 0.14.9.
- `edgion-gateway/Cargo.toml:18-45`: still no subsystem features; `edgion-waf`, `wasmtime`, `ldap3`, `rskafka`, `maxminddb`, `opentelemetry*` remain unconditional.
- No unused-dependency CI gate was added under `cicd/checks/`.
