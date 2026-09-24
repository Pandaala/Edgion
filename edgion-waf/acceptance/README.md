# Production WAF acceptance profile v1

`production-v1.json` is a repository-tracked acceptance corpus for the three
`EdgionPlugins/Waf/DynamicTest` configurations submitted to the Gateway. It is
an intentionally narrow selected profile, not a CRS compatibility claim. The
YAML paths and SHA-256 values pin the exact production configuration; the
acceptance executable pins the complete manifest digest and baseline case
expectations. No external WAF oracle is used.

Run the dedicated gate from the repository root:

```bash
bash cicd/checks/run_waf_production_acceptance.sh
```

The command writes `target/waf-production-acceptance/coverage.json`, runs the
Gateway generation pinning test, then runs the `WafDynamic` scenario in the
existing shared Gateway/Controller integration topology. The scenario checks blocking before
backend exposure, forwarding and body digests across HTTP/1 and HTTP/2,
prefix/full behavior and limits, response-stage denial, detection-only mode,
invalid-update retention, recovery, and deletion. Its raw report is copied to
`target/waf-production-acceptance/integration-report.log`. Use `--offline-only`
to run strict compilation and transaction assertions without the integration
sweep; it still runs the generation pinning test.
After a complete passing run, `acceptance.json` combines the case coverage and
the `WafDynamic` integration outcome into one machine-readable result.

The JSON report counts cases in this selected profile: `total = supported +
excluded`, `executed = passed + failed`, and `false_positives` and
`false_negatives` count incorrect blocking decisions. Rule IDs and expected
HTTP statuses are asserted independently. The initial profile has three
configurations, nine cases, and no exclusions. The gate refuses fewer than
nine frozen baseline cases, missing or changed YAML, changed rule IDs, duplicate case IDs,
unexpected exclusions, and incorrect outcomes. Extend the profile by reviewing
new cases and updating the reviewed manifest digest and recorded YAML digests.
Any exclusion needs a case ID,
rationale, and revisit condition; this version fails on every exclusion.

The `WafDynamic` Gateway scenario also applies `to_injection` and `to_control`
generations after its frozen v1 phases. The first checks transformed SQLi/XSS
blocking and a clean request. The second checks all four selected request
controls: conditional tag and response target exclusions, JSON processor
selection, raw-body visibility, and successful/failed chains with capture and
macro expansion. The scenario then restores the prior generation. These
extensions do not alter the nine
frozen v1 cases or their configuration digests. The strict
`WafConfig` compile test for the injection generation is
`cargo test -p edgion-resources --test waf_injection_config`.

The supported request control subset is `forceRequestBodyVariable`,
`requestBodyProcessor`, `ruleRemoveByTag`, and `ruleRemoveTargetByTag`. Direct
WAF transaction tests and the Gateway scenario cover all four, including
publication, hot reload, and request isolation. This is not full `ctl` or CRS
compatibility. The frozen `production-v1.json` corpus remains the nine-case
baseline; the two post-baseline reloads are checked by the integration gate.

The FTW executable-subset inventory remains a separate exploratory tool. It
does not define the production acceptance denominator. When a larger selected
CRS profile can strictly compile with the product's supported rule semantics,
add its exact configuration and cases here, then expand the Gateway assertions.
