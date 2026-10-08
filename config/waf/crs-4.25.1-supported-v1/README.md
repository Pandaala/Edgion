# Supported CRS 4.25.1 profile

An explicit Edgion-supported projection, not full CRS compatibility. Sources
are pinned to upstream revision `3b89d5a05322f448b4b74b9cadc5fb05ac6915ad`
and copied unchanged in `upstream/` with the Apache-2.0 LICENSE. The manifest
records all 627 core rule units and exact byte spans/hashes; the Rust generator
checks its pin, file hashes and the native compiler's complete capability report.
Only 62 declared whole units are removed: 59 require response-body inspection,
901320 initializes optional persistent collections unused by core CRS, and
905100/905110 exempt Apache loopback probes and suppress audits. Those actions
remain unsupported and strict admission rejects them. Supported response-header,
status and TX rules remain active.

`generated/` holds complete typed resources for namespace `edgion-default`:
three required rule bundles, a two-profile policy and a standard On plugin.
Import all five YAML files using kubectl or the Controller admin/CLI import.
Attach `EdgionPlugins/crs-supported` through a route ExtensionRef. Change
`activeProfile` to `strict` or `mode` to `detectionOnly` explicitly; profile names
have no automatic version-update semantics. Required content digest pins are
part of the policy; editing a bundle requires an intentional pin update.

| Profile | Blocking/detection PL | UTF-8 validation | XML attributes |
|---|---|---|---|
| standard (default) | 1 / 1 | enabled | inspected |
| strict | 4 / 4 | enabled | inspected |

Higher paranoia can increase false positives and work. Start with DetectionOnly
and tune against your application's valid traffic. Original 901 initialization
supplies severity scores, thresholds 5/4 and early_blocking=0. There is no FTW
setup or implicit ARG_LENGTH/MAX_NUM_ARGS tuning. Each selected generation has
565 original core units, original setup action 900990 and one product tuning
unit 9900301 (567 total). The original setup's five SecDefaultAction declarations
are supplied by Edgion's existing phase 1–5 pass/log/audit defaults; no unsupported
setup directives are imported. Gateway request/JSON/XML budgets stay unchanged;
a Full profile may still hit a runtime budget. DetectionOnly suppresses rule
interventions, not fatal runtime errors.

Rebuild/check from repository files only:

```bash
cargo run -p edgion-resources --example generate_supported_crs
cargo run -p edgion-resources --example generate_supported_crs -- --check
cargo test -p edgion-resources --test waf_supported_crs
cargo test -p edgion-tests --test waf_supported_fixture
```

The same generator emits local/Kubernetes WafDynamic fixtures with the exact
product core/tuning pins and one separately declared test-owned guard. That
568-unit test generation must not be described as 568 original CRS rules.
`generated/acceptance.json` records pins, output hashes and exact exclusions;
current runtime contracts are recorded in the
[WAF review knowledge](../../../skills/04-review/waf/SKILL.md). Remaining
performance and Kubernetes verification have standalone tasks linked there.
