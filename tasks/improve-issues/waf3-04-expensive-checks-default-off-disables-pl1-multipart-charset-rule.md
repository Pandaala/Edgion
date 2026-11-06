# waf3-04: `expensiveChecks` defaults to false and silently disables CRS PL1 rule 922110 (multipart charset evasion) in the "standard" profile

- **Category**: design-error
- **Severity**: P2
- **Scope**: local
- **Location**:
  - `edgion-resources/src/resources/edgion_gateway_config.rs:117-125` (`GatewayWafConfig.expensive_checks`, `#[serde(default)]` → false)
  - `edgion-gateway/src/config/edgion_gateway/conf_handler_impl.rs:147,175` (false when unset / no EGC)
  - `edgion-gateway/src/plugins/http/waf/mod.rs:439-457` (`RuleCheckPolicy { expensive_checks }` pinned per transaction)
  - `edgion-waf/src/seclang/expensive_checks.rs:45-49` (`skips_unit`: 921170/921180, 922140/922110)
  - `edgion-waf/src/seclang/compiler.rs` `publish_with_work` (`member.expensive_aggregate_actions = optional_checks.multipart && unit.id == 922150`)
  - `edgion-waf/src/seclang/transaction.rs:2021,2029-2033,2858-2860` (pruned units are skipped; 922150's setvars are suppressed)
  - `edgion-waf/src/seclang/expensive_check_signatures.rs` (exact 4.25.1 signature of 922110, `paranoia-level/1`)

## Summary
The Gateway-wide `spec.waf.expensiveChecks` knob defaults to `false`. With it off, the engine removes the
CRS multipart Content-Type aggregate family: 922140/922150 stop recording per-part `Content-Type` values
and 922110 — a **paranoia-level 1** rule that blocks multipart parts declaring a charset other than
utf-8/iso-8859-1(5)/windows-1252 — never runs. That rule exists to stop the well-known multipart charset
evasion (e.g. `Content-Type: text/plain; charset=ibm037` or `utf-7`/`utf-16` parts that ASP.NET/Java
decode while the WAF inspects the raw bytes). Operators who import the "standard" CRS profile get PL1
coverage minus this anti-evasion control without any indication in the profile or WAF docs.

## Current behavior (evidence)
- `OptionalChecks::skips_unit` marks 922140/922110 (and HPP 921170/921180) as `expensive_check` units;
  `execute_phase_with_prefix` skips them when `policy_prunes` (`has_expensive_checks &&
  !check_policy.expensive_checks`). 922150 keeps matching but its two `setvar`s are dropped
  (`expensive_aggregate_actions`).
- 922110 in `upstream/rules/REQUEST-922-MULTIPART-ATTACK.conf:91-110` carries `tag:'paranoia-level/1'`,
  severity CRITICAL, `+%{tx.critical_anomaly_score}` to the PL1 inbound score.
- `config/waf/crs-4.25.1-supported-v1/README.md` advertises "565 original core units" per generation and
  does not mention that one PL1 unit is disabled by default; the WAF user guide
  (`docs/en/user-guide/http-route/filters/edgion-plugins/waf.md`) does not mention `expensiveChecks` at
  all. Only `docs/en/ops-guide/configuration/edgion-gateway-config.md` describes the knob, as a
  performance option.
- The measured benefit recorded in `skills/04-review/waf/regression-guards/fused-json-inspection.md`
  ("Completed multi-pattern optimization evidence") is a 2.2–3.8% reduction of requested bytes / live
  heap peak for the standard profile in debug builds — no latency or throughput gain was measured.

## Industry comparison
ModSecurity/Coraza with CRS run 922110 at PL1 by default; CRS's own tuning model expresses cost/false-
positive trade-offs through paranoia levels and rule exclusions that are visible in the rule set, not via
an engine-level switch that removes a specific PL1 unit.

## Impact
Default deployments lose a PL1 protection against a documented WAF-bypass technique in exchange for a
low-single-digit memory saving. Because the knob lives on `EdgionGatewayConfig` (GatewayClass level), a
route owner who attaches the CRS plugin cannot see or change it.

## Suggested direction
1. Default `expensiveChecks` to `true` (or split it: keep the HPP pair — PL3, genuinely aggregate-heavy —
   optional, and always run the multipart Content-Type family, whose per-request state is bounded by
   the multipart part limit).
2. If the knob stays, surface it in the CRS profile README / WAF user guide as a coverage reduction, and
   expose the effective state in the WAF admission status so operators can see which PL1 units are not
   active.

## Notes
- The named-profile regression guard documents the switch and its default as intentional
  ("`expensiveChecks` defaults to false in the Gateway schema"). The new evidence is the security
  impact of 922110 specifically (PL1 anti-evasion, not merely an aggregate counter) weighed against the
  recorded 2–4% memory benefit, neither of which the guard discusses.
- Related: `waf3-05` (over-design) on the exact-text signature mechanism behind this switch.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P2 unchanged.
- `edgion-resources/src/resources/edgion_gateway_config.rs:113-121`: `expensive_checks` is `#[serde(default)]` false.
- `edgion-waf/src/seclang/expensive_checks.rs:45-49`: `skips_unit` covers 922140/922110 for the multipart family.
- 922110 carries `tag:'paranoia-level/1'`, severity CRITICAL (`REQUEST-922-MULTIPART-ATTACK.conf:91-110`).
- The default is documented intent (`named-profile-selection-and-bundle-publication.md:127`). The only recorded benefit is a 2.2-3.8% requested-bytes / heap-peak reduction for the standard profile in debug builds, with no gateway latency measured (`fused-json-inspection.md:637-650`). Neither page weighs the loss of a PL1 anti-evasion rule, which is the new evidence here. The WAF user guide and CRS README do not mention the switch.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P2 unchanged.
- `edgion-resources/src/resources/edgion_gateway_config.rs:117-125`: `GatewayWafConfig.expensive_checks` is still `#[serde(default)]` false (lines shifted +4 by the new `downstream_tls` field).
- `edgion-gateway/src/config/edgion_gateway/conf_handler_impl.rs:147,175` still resolve it to the EGC value / `false` (shifted by the new `downstream_tls` field).
- `edgion-waf/src/seclang/expensive_checks.rs:45-49` unchanged; 922110 still PL1 in the shipped bundle; docs still do not mention the coverage loss.
- `git diff feature-08-30..HEAD` shows no change under `edgion-gateway/src/plugins/http/waf/`, `edgion-gateway/src/ctx.rs`, `edgion-gateway/src/request_body/config.rs`, or `edgion-waf/src/` apart from a `serde_yaml`→`yaml_serde` dependency rename and a `fetch_update`→`try_update` rename in `json_inspection.rs:12`; none touch this finding's code paths.
