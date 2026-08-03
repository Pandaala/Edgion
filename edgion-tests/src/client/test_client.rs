// Edgion Unified Test Client
// Supports all protocol tests: HTTP/HTTPS, gRPC, WebSocket, TCP, UDP

#![allow(clippy::module_inception)]
#![allow(dead_code)]
#![allow(unused_imports)]

// Shared client framework lives in the `edgion_tests` library crate.
use edgion_tests::framework::{self, TestContext, TestRunner};
use edgion_tests::reporter::{ConsoleReporter, JsonReporter};
use edgion_tests::{dynamic, port_config, suites};

use anyhow::Result;
use clap::Parser;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Once, OnceLock};
use std::time::Instant;

static INIT: Once = Once::new();
static CAPABILITY_REGISTRY: OnceLock<String> = OnceLock::new();
const KNOWN_GATEWAY_CAPABILITIES: &[&str] = &["body-buffering"];

fn capability_entry_applies(kind: &str, entry_id: &str, id: &str) -> bool {
    entry_id == id || (kind == "suite" && id.strip_prefix(entry_id).is_some_and(|suffix| suffix.starts_with('/')))
}

fn validate_capability_registry(contents: &str) -> Result<()> {
    let mut entries = HashSet::new();
    let conf_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("integration/conf");
    for (index, raw_line) in contents.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != 3 {
            anyhow::bail!(
                "capability registry line {} must contain exactly three tab-separated fields",
                index + 1
            );
        }
        let (kind, id, capability) = (fields[0], fields[1], fields[2]);
        if !matches!(kind, "suite" | "dynamic") {
            anyhow::bail!("capability registry line {} has unknown kind `{kind}`", index + 1);
        }
        if id.is_empty() || capability.is_empty() {
            anyhow::bail!("capability registry line {} contains an empty field", index + 1);
        }
        if !KNOWN_GATEWAY_CAPABILITIES.contains(&capability) {
            anyhow::bail!(
                "capability registry line {} has unknown capability `{capability}`",
                index + 1
            );
        }
        if !entries.insert((kind, id)) {
            anyhow::bail!("capability registry contains duplicate {kind} id `{id}`");
        }
        match kind {
            "suite" if !conf_root.join(id).is_dir() => {
                anyhow::bail!(
                    "capability registry suite `{id}` does not exist under {}",
                    conf_root.display()
                );
            }
            "dynamic" if !dynamic::all().iter().any(|scenario| scenario.id == id) => {
                anyhow::bail!("capability registry dynamic scenario `{id}` is not registered");
            }
            _ => {}
        }
    }
    Ok(())
}

fn capability_registry() -> Result<&'static str> {
    if CAPABILITY_REGISTRY.get().is_none() {
        let path = std::env::var_os("EDGION_CAPABILITY_REGISTRY")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("integration/capabilities.tsv"));
        let contents =
            std::fs::read_to_string(&path).map_err(|err| anyhow::anyhow!("read {}: {err}", path.display()))?;
        validate_capability_registry(&contents)?;
        let _ = CAPABILITY_REGISTRY.set(contents);
    }
    Ok(CAPABILITY_REGISTRY.get().expect("capability registry initialized"))
}

fn required_capability(kind: &str, id: &str) -> Result<Option<&'static str>> {
    Ok(capability_registry()?.lines().find_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let mut fields = line.split('\t');
        match (fields.next(), fields.next(), fields.next(), fields.next()) {
            (Some(entry_kind), Some(entry_id), Some(capability), None)
                if entry_kind == kind && capability_entry_applies(kind, entry_id, id) =>
            {
                Some(capability)
            }
            _ => None,
        }
    }))
}

fn gateway_has_capability(capability: &str) -> bool {
    std::env::var("EDGION_TEST_GATEWAY_CAPABILITIES")
        .ok()
        .is_some_and(|value| value.lines().any(|line| line == format!("{capability}=enabled")))
}

fn missing_capability(kind: &str, id: &str) -> Result<Option<&'static str>> {
    Ok(required_capability(kind, id)?.filter(|capability| !gateway_has_capability(capability)))
}

#[cfg(test)]
mod capability_tests {
    use super::{capability_entry_applies, resolve_suite, validate_capability_registry};
    use edgion_tests::dynamic;
    use std::collections::BTreeSet;

    #[test]
    fn empty_registry_is_valid() {
        validate_capability_registry("# kind\tid\trequired-capability\n").unwrap();
    }

    #[test]
    fn rejects_malformed_unknown_and_duplicate_entries() {
        assert!(validate_capability_registry("suite base body-buffering\n").is_err());
        assert!(validate_capability_registry("suite\tbase\tunknown\n").is_err());
        assert!(validate_capability_registry("other\tbase\tbody-buffering\n").is_err());
        assert!(validate_capability_registry("suite\tbase\tbody-buffering\nsuite\tbase\tbody-buffering\n").is_err());
    }

    #[test]
    fn suite_capability_applies_to_nested_config_only() {
        assert!(capability_entry_applies(
            "suite",
            "EdgionPlugins/Body",
            "EdgionPlugins/Body"
        ));
        assert!(capability_entry_applies(
            "suite",
            "EdgionPlugins/Body",
            "EdgionPlugins/Body/initial"
        ));
        assert!(!capability_entry_applies(
            "suite",
            "EdgionPlugins/Body",
            "EdgionPlugins/BodyOther"
        ));
        assert!(!capability_entry_applies(
            "dynamic",
            "body-reload",
            "body-reload/initial"
        ));
    }

    #[test]
    fn body_suites_are_registered_in_resource_and_default_sweeps() {
        let runner = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/scripts/integration/run_integration.sh"
        ));
        assert!(
            runner.contains(r#"log_warn "${name} test skipped""#),
            "capability skips must use the runner's defined warning helper"
        );
        let starter = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/scripts/utils/start_all_with_conf.sh"
        ));
        assert!(
            starter.contains("body-buffering=enabled"),
            "the shared starter must verify the Gateway capability snapshot"
        );
        for suite in ["DslBody", "BodyPlugins", "JsonSchemaValidation"] {
            assert_eq!(
                runner.matches(&format!("EdgionPlugins_{suite}")).count(),
                2,
                "{suite} must be invoked by both the EdgionPlugins resource sweep and the default sweep"
            );
            assert_eq!(
                runner.matches(&format!("EdgionPlugins/{suite}")).count(),
                1,
                "{suite} config must be included in the EdgionPlugins resource-wide load list"
            );
        }
    }

    /// Every registered suite is actually executed by some runner path.
    ///
    /// This is the outer guard. `every_targeted_sweep_suite_also_runs_in_the_default_sweep`
    /// below only compares the runner's two suite lists against each other, so a
    /// suite missing from **both** falls in its blind spot — which is exactly what
    /// happened: 25 registered suites (including `EdgionPlugins/Wasm` and
    /// `EdgionPlugins/TraceContext`, whole plugins with real conf fixtures) were
    /// never executed by any runner, in either sweep, for an unknown length of time.
    ///
    /// Coverage is proven three ways, in order of preference:
    ///   1. the runner invokes it — resolved through the real `resolve_suite`, so
    ///      alias forms (`-g http-security`, `-g h2-mtls-sp`, ...) count;
    ///   2. a dynamic scenario drives it — matched on the suite id's last segment
    ///      against `dynamic::all()` ids, since `run_dynamic_scenarios` enumerates
    ///      those from the registry rather than from a hardcoded list;
    ///   3. an explicit `KNOWN_EXEMPT` entry with a stated reason.
    #[test]
    fn every_registered_suite_is_executed_by_some_runner_path() {
        // Suites that no runner invokes, by decision rather than oversight.
        // Adding an entry requires a reason; an empty list is the goal state.
        const KNOWN_EXEMPT: &[(&str, &str)] = &[
            ("all", "not a suite — the CLI keyword that expands to every suite"),
            (
                "tcp",
                "protocol shorthand handled by the direct-mode block, not a suite id",
            ),
            (
                "udp",
                "protocol shorthand handled by the direct-mode block, not a suite id",
            ),
            (
                "Services/acme",
                "ACME has its own runner (test_acme.sh); it boots a Pebble CA and is not part of \
                 the shared-Gateway sweep",
            ),
            (
                "HTTPRoute/Protocol",
                "parent grouping id; the concrete Protocol/* children are each invoked",
            ),
            (
                "Status/ObservedGeneration",
                "parent grouping id; the HighFrequency child runs as a dynamic scenario",
            ),
            (
                "HTTPRoute/Protocol/HTTPS",
                "alternate id for HttpsTestSuite, which the runner executes as EdgionTls/https",
            ),
            (
                "GRPCRoute/TLS",
                "alternate id for GrpcTlsTestSuite, which the runner executes as EdgionTls/grpctls",
            ),
            (
                "EdgionBackendTrafficPolicy/upstream_authority",
                "snake_case alias of EdgionBackendTrafficPolicy/UpstreamAuthority, which is invoked",
            ),
            ("EdgionTls", "resource-level id; every EdgionTls/* child is invoked"),
            ("Gateway", "resource-level id; every Gateway/* child is invoked"),
            ("GRPCRoute", "resource-level id; every GRPCRoute/* child is invoked"),
            ("TLSRoute", "resource-level id; every TLSRoute/* child is invoked"),
        ];

        let own_src = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/client/test_client.rs"));
        let runner = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/scripts/integration/run_integration.sh"
        ));

        // Registered suite ids = the string patterns of `add_suites_for_suite`'s match arms.
        // Match the definition at column 0, not the first textual occurrence — this
        // test module sits above the function and mentions its name in a literal.
        let arms_start = own_src
            .find("\nfn add_suites_for_suite")
            .expect("add_suites_for_suite must be defined at column 0");
        let arms = &own_src[arms_start..];
        let arms_end = arms.find("\n}\n").expect("add_suites_for_suite must be terminated");
        let registered: BTreeSet<String> = arms[..arms_end]
            .lines()
            .filter_map(|l| {
                let t = l.trim();
                t.strip_prefix('"')
                    .and_then(|r| r.find('"').map(|e| r[..e].to_string()))
                    .filter(|s| !s.is_empty() && s.contains(|c: char| c.is_ascii_uppercase() || c == '/'))
            })
            .collect();
        assert!(
            registered.len() > 100,
            "expected >100 registered suites, parsed {} — the arm-extraction heuristic is stale",
            registered.len()
        );

        // What the runner actually invokes, canonicalized through resolve_suite.
        let mut invoked: BTreeSet<String> = BTreeSet::new();
        for (i, _) in runner.match_indices("${EDGION_TEST_CLIENT_BIN}") {
            let rest = &runner[i + "${EDGION_TEST_CLIENT_BIN}".len()..];
            let Some(end) = rest.find('"') else { continue };
            let args: Vec<&str> = rest[..end].split_whitespace().collect();
            if args.iter().any(|a| a.contains('$')) && !args.iter().any(|a| *a == "-r") {
                continue; // dynamic `${item_safe}` invocation, nothing static to resolve
            }
            let val = |flag: &str| -> Option<String> {
                args.iter()
                    .position(|a| *a == flag)
                    .and_then(|p| args.get(p + 1))
                    .filter(|v| !v.starts_with('-') && !v.contains('$'))
                    .map(|v| v.to_string())
            };
            let (r, it) = (val("-r"), val("-i"));
            // `resolve_suite` short-circuits on `legacy`, so it may only be read
            // when there is no `-r` — otherwise the first bare token is `-r`'s own
            // value and every invocation resolves to the resource name.
            let legacy = if r.is_some() {
                None
            } else {
                args.iter()
                    .find(|a| !a.starts_with('-') && !a.contains('$'))
                    .map(|s| s.to_string())
            };
            if r.is_none() && legacy.is_none() {
                continue;
            }
            invoked.insert(resolve_suite(r.as_deref(), it.as_deref(), legacy.as_deref()));
        }

        // A suite counts as dynamic-covered when a registered scenario drives it.
        // Matched exactly, two ways, rather than by prefix — a prefix rule would
        // let a parent id inherit a child scenario's coverage:
        //   `conf_path == "<suite>/DynamicTest"`  (the usual layout), or
        //   `conf_path == "<suite>Test"`          (Gateway/Dynamic -> Gateway/DynamicTest).
        // The scenario id itself is also accepted when it equals the suite's last
        // segment (EdgionPlugins/BasicAuthDynamic -> "BasicAuthDynamic").
        let scenario_ids: BTreeSet<&str> = dynamic::all().iter().map(|s| s.id).collect();
        let conf_paths: BTreeSet<&str> = dynamic::all().iter().map(|s| s.conf_path).collect();
        let dynamic_covers = |suite: &str| -> bool {
            scenario_ids.contains(suite.rsplit('/').next().unwrap_or(suite))
                || conf_paths.contains(format!("{suite}/DynamicTest").as_str())
                || conf_paths.contains(format!("{suite}Test").as_str())
        };

        let mut missing: Vec<String> = registered
            .iter()
            .filter(|s| !invoked.contains(*s))
            .filter(|s| !dynamic_covers(s))
            .filter(|s| !KNOWN_EXEMPT.iter().any(|(k, _)| k == s))
            .cloned()
            .collect();
        missing.sort();

        assert!(
            missing.is_empty(),
            "registered test suite(s) that NO runner path executes — they compile, they have conf \
             fixtures, and they never run:\n  {missing:?}\n\
             Fix: add a `run_test` line to run_integration.sh (both the `-r <Resource>` dispatch \
             and the default sweep), or add a KNOWN_EXEMPT entry stating why it cannot run."
        );
    }

    /// Every suite reachable via `-r <Resource>` must also run in the default sweep.
    ///
    /// `run_integration.sh` carries the suite list twice: once inside the
    /// `case "$G_RESOURCE" in` block that serves `-r <Resource>`, and once in the
    /// default full run. `cicd/coverage.sh` invokes the script with no arguments,
    /// i.e. the default list — so a suite present only in the targeted block is
    /// **never executed by CI**, and the failure is silent because the report
    /// only counts tests that ran.
    ///
    /// This has already happened twice. Commit `82fcfda3` (2026-06-24) created
    /// the duplication and added suites to the targeted block only; six suites
    /// went unexecuted for up to five weeks (`EdgionPlugins_Cors`,
    /// `HTTPRoute_Protocol_{H2C,GrpcWeb,H2TrailerMalformed}`,
    /// `GRPCRoute_Delegation`, `TCPRoute_GlobalConnectionIpRestriction`). The
    /// hardcoded three-suite check above was the first patch for one instance of
    /// this; this test generalizes it to the whole list.
    ///
    /// A targeted entry may be satisfied by finer-grained default entries — the
    /// targeted block runs bundles like `HTTPRoute_Backend` where the default
    /// sweep runs `HTTPRoute_Backend_WeightedBackend` and friends — so an
    /// `EXACT` or `EXACT_` prefix match both count as covered.
    #[test]
    fn every_targeted_sweep_suite_also_runs_in_the_default_sweep() {
        let runner = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/scripts/integration/run_integration.sh"
        ));

        let case_start = runner
            .find(r#"case "$G_RESOURCE" in"#)
            .expect("run_integration.sh must contain the `case \"$G_RESOURCE\" in` dispatch block");
        let rest = &runner[case_start..];
        let case_len = rest
            .lines()
            .scan(0usize, |off, line| {
                let here = *off;
                *off += line.len() + 1;
                Some((here, line))
            })
            .find(|(off, line)| *off > 0 && line.trim() == "esac")
            .map(|(off, _)| off)
            .expect("the `case \"$G_RESOURCE\"` block must be terminated by an `esac` line");

        let targeted = &rest[..case_len];
        let default_sweep = &rest[case_len..];

        // Collect `run_test "<name>"` names from a region.
        let names = |region: &str| -> Vec<String> {
            region
                .match_indices("run_test \"")
                .filter_map(|(i, _)| {
                    let after = &region[i + "run_test \"".len()..];
                    after.find('"').map(|end| after[..end].to_string())
                })
                .collect()
        };

        // Suites deliberately reachable only via `-r <Resource>`. Each entry is a
        // KNOWN COVERAGE GAP, recorded rather than silently tolerated; adding the
        // suite to the default sweep and deleting the entry is always the fix.
        // Empty is the goal state — keep it that way.
        const KNOWN_TARGETED_ONLY: &[(&str, &str)] = &[];

        let targeted_names = names(targeted);
        assert!(
            targeted_names.len() > 50,
            "expected the targeted block to hold the bulk of the suite list, found {} — \
             the region markers in this test are probably out of date",
            targeted_names.len()
        );

        let mut missing: Vec<String> = targeted_names
            .iter()
            // `run_test "Foo_${item_safe}"` is the dynamic `-i <Item>` path, not a
            // static suite name; there is nothing for the default sweep to match.
            .filter(|n| !n.contains('$'))
            .filter(|n| !KNOWN_TARGETED_ONLY.iter().any(|(known, _)| *known == n.as_str()))
            .filter(|n| {
                !default_sweep.contains(&format!("run_test \"{n}\""))
                    && !default_sweep.contains(&format!("run_test \"{n}_"))
            })
            .cloned()
            .collect();
        missing.sort();
        missing.dedup();

        assert!(
            missing.is_empty(),
            "suite(s) runnable via `-r <Resource>` but absent from the default sweep, so \
             `cicd/coverage.sh` never executes them:\n  {missing:?}\n\
             Fix: add a matching `run_test` line to the default full-run block of \
             run_integration.sh (the section after the `case \"$G_RESOURCE\"` dispatch)."
        );
    }
}

#[derive(Parser, Debug)]
#[command(name = "test-client")]
#[command(about = "Edgion ")]
struct Cli {
    /// Resource type (HTTPRoute, GRPCRoute, TCPRoute, UDPRoute, TLS, Security, Plugins)
    #[arg(short = 'r', long = "resource")]
    resource: Option<String>,

    /// sub-item (Match, Backend, Filters, Protocol )
    #[arg(short = 'i', long = "item")]
    item: Option<String>,

    /// Enable gateway mode (tests target the running Edgion gateway).
    #[arg(short = 'g', long = "gateway")]
    gateway: bool,

    #[arg(long, default_value = "127.0.0.1")]
    target_host: String,

    #[arg(long, default_value = "30001")]
    http_port: u16,

    #[arg(long, default_value = "30021")]
    grpc_port: u16,

    #[arg(long, default_value = "30005")]
    websocket_port: u16,

    #[arg(long, default_value = "30010")]
    tcp_port: u16,

    #[arg(long, default_value = "30011")]
    udp_port: u16,

    #[arg(long, default_value = "10443")]
    https_port: u16,

    #[arg(long, default_value = "18443")]
    grpc_https_port: u16,

    #[arg(long, default_value = "12101")]
    admin_port: u16,

    #[arg(long)]
    json: bool,

    #[arg(long, default_value = "test_report.json")]
    json_output: String,

    #[arg(short, long)]
    verbose: bool,

    /// Test phase for dynamic tests (initial or update)
    #[arg(long)]
    phase: Option<String>,

    /// List all registered dynamic scenario ids (one per line) and exit.
    #[arg(long = "list-dynamic-scenarios")]
    list_dynamic_scenarios: bool,

    /// Run a single dynamic scenario by id.
    #[arg(long = "dynamic-scenario")]
    dynamic_scenario: Option<String>,

    /// Legacy positional command (use -r/-i instead).
    #[arg(value_name = "COMMAND")]
    legacy_command: Option<String>,
}

/// Resolve the test suite name from resource/item flags or a legacy command.
fn resolve_suite(resource: Option<&str>, item: Option<&str>, legacy: Option<&str>) -> String {
    if let Some(cmd) = legacy {
        return match cmd.to_lowercase().as_str() {
            "http" => "HTTPRoute/Basic".to_string(),
            "http-match" | "httpmatch" => "HTTPRoute/Match".to_string(),
            "http-redirect" | "httpredirect" => "HTTPRoute/Filters/Redirect".to_string(),
            "http-security" | "httpsecurity" => "HTTPRoute/Filters/Security".to_string(),
            "https" => "EdgionTls/https".to_string(),
            "websocket" => "HTTPRoute/Protocol/WebSocket".to_string(),
            "h2c" => "HTTPRoute/Protocol/H2C".to_string(),
            "h2-trailer-malformed" | "h2trailermalformed" | "h2tm" => {
                "HTTPRoute/Protocol/H2TrailerMalformed".to_string()
            }
            "lb-rr" | "lbrr" | "lb-roundrobin" => "HTTPRoute/Backend/LBRoundRobin".to_string(),
            "lb-ch" | "lbch" | "lb-consistenthash" => "HTTPRoute/Backend/LBConsistentHash".to_string(),
            "lb-lc" | "lblc" | "lb-leastconn" => "HTTPRoute/Backend/LBLeastConn".to_string(),
            "lb-lc-rs" | "lblcrs" | "lb-leastconn-retry-on-status" => {
                "HTTPRoute/Backend/LBLeastConnRetryOnStatus".to_string()
            }
            "lb-ewma" | "lbewma" | "lb-ewma-test" => "HTTPRoute/Backend/LBEwma".to_string(),
            "lb-ewma-rs" | "lbewmars" | "lb-ewma-retry-on-status" => {
                "HTTPRoute/Backend/LBEwmaRetryOnStatus".to_string()
            }
            "weighted-backend" | "weightedbackend" => "HTTPRoute/Backend/WeightedBackend".to_string(),
            "timeout" => "HTTPRoute/Backend/Timeout".to_string(),
            "retry-on-status" | "retryonstatus" => "HTTPRoute/Backend/RetryOnStatus".to_string(),
            "per-try-timeout" | "pertrytimeout" => "HTTPRoute/Backend/PerTryTimeout".to_string(),
            "circuit-breaker" | "circuitbreaker" => "HTTPRoute/Backend/CircuitBreaker".to_string(),
            "health-check" | "healthcheck" => "HTTPRoute/Backend/HealthCheck".to_string(),
            "grpc-health-check" | "grpchealthcheck" => "HTTPRoute/Backend/GrpcHealthCheck".to_string(),
            "health-check-transition" | "healthcheck-transition" => {
                "HTTPRoute/Backend/HealthCheckTransition".to_string()
            }
            "session-persistence" | "sp" => "HTTPRoute/Backend/SessionPersistence".to_string(),
            "h2-mtls-sp" | "h2_mtls_sp" | "h2mtlssp" => "HTTPRoute/Backend/H2BackendMtlsPersistence".to_string(),
            "backend-tls-sni-host-override" | "backend-tls-sni" | "btls-sni" => {
                "HTTPRoute/Backend/BackendTLSSNIHostOverride".to_string()
            }
            "grpc" => "GRPCRoute/Basic".to_string(),
            "grpc-match" | "grpcmatch" => "GRPCRoute/Match".to_string(),
            "grpc-rate-limit" | "grpcratelimit" => "GRPCRoute/RateLimit".to_string(),
            "grpc-sp" | "grpcsp" => "GRPCRoute/Backend/SessionPersistence".to_string(),
            "grpc-tls" | "grpctls" => "EdgionTls/grpctls".to_string(),
            "tcp" => "TCPRoute/Basic".to_string(),
            "tcp-keepalive-smoke" | "tcpkeepalivesmoke" => "TCPRoute/KeepaliveSmoke".to_string(),
            "udp" => "UDPRoute/Basic".to_string(),
            "mtls" => "EdgionTls/mTLS".to_string(),
            "security" => "Gateway/Security".to_string(),
            "stream-plugins" | "streamplugins" | "connection-filter" => "Gateway/StreamPlugins".to_string(),
            "tcp-stream-plugins" | "tcpstreamplugins" => "TCPRoute/StreamPlugins".to_string(),
            "udp-stream-plugins" | "udpstreamplugins" => "UDPRoute/StreamPlugins".to_string(),
            "tcp-gcir" | "tcpgcir" | "tcp-global-connection-ip-restriction" => {
                "TCPRoute/GlobalConnectionIpRestriction".to_string()
            }
            "tls" | "tls-route" | "tlsroute" => "TLSRoute/Basic".to_string(),
            "tls-pp2" | "tlspp2" | "tls-proxy-protocol" => "TLSRoute/ProxyProtocol".to_string(),
            "tls-stream-plugins" | "tlsstreamplugins" => "TLSRoute/StreamPlugins".to_string(),
            "tls-multi-sni" | "tlsmultisni" | "multi-sni" => "TLSRoute/MultiSNI".to_string(),
            "tls-passthrough" | "tlspassthrough" => "TLSRoute/Passthrough".to_string(),
            "tls-unknown-kind-backend" | "tlsunknownkindbackend" => "TLSRoute/UnknownKindBackend".to_string(),
            "real-ip" | "realip" => "Gateway/RealIP".to_string(),
            "backend-tls" | "backendtls" => "Gateway/TLS/BackendTLS".to_string(),
            "backend-tls-san" | "backendtlssan" => "Gateway/TLS/BackendTLSSAN".to_string(),
            "backend-tls-gateway-cert" | "backendtlsgatewaycert" => "Gateway/TLS/BackendTLSGatewayCert".to_string(),
            "misdirected-request" | "misdirectedrequest" | "421" => "Gateway/TLS/MisdirectedRequest".to_string(),
            "frontend-validation" | "frontendvalidation" | "fv-mtls" => "Gateway/FrontendValidation".to_string(),
            "csrf" => "EdgionPlugins/Csrf".to_string(),
            "plugin-logs" | "pluginlogs" => "EdgionPlugins/DebugAccessLog".to_string(),
            "plugin-condition" | "plugincondition" => "EdgionPlugins/PluginCondition".to_string(),
            "all-conditions" | "allconditions" => "EdgionPlugins/PluginCondition/AllConditions".to_string(),
            "jwt-auth" | "jwtauth" => "EdgionPlugins/JwtAuth".to_string(),
            "jwe-decrypt" | "jwedecrypt" => "EdgionPlugins/JweDecrypt".to_string(),
            "key-auth" | "keyauth" => "EdgionPlugins/KeyAuth".to_string(),
            "hmac-auth" | "hmacauth" => "EdgionPlugins/HmacAuth".to_string(),
            "header-cert-auth" | "headercertauth" => "EdgionPlugins/HeaderCertAuth".to_string(),
            "ldap-auth" | "ldapauth" => "EdgionPlugins/LdapAuth".to_string(),
            "ldap-auth-tls" | "ldapauthtls" => "EdgionPlugins/LdapAuthTls".to_string(),
            "forward-auth" | "forwardauth" => "EdgionPlugins/ForwardAuth".to_string(),
            "ext-proc" | "extproc" => "EdgionPlugins/ExtProc".to_string(),
            "openid-connect" | "openidconnect" | "oidc" => "EdgionPlugins/OpenidConnect".to_string(),
            "webhook-keyget" | "webhookkeyget" => "EdgionPlugins/WebhookKeyGet".to_string(),
            "rate-limit-redis" | "ratelimitredis" | "rlr" => "EdgionPlugins/RateLimitRedis".to_string(),
            "dsl" => "EdgionPlugins/Dsl".to_string(),
            "dsl-body" | "dslbody" => "EdgionPlugins/DslBody".to_string(),
            "body-plugins" | "bodyplugins" => "EdgionPlugins/BodyPlugins".to_string(),
            "json-schema-validation" | "jsonschemavalidation" => "EdgionPlugins/JsonSchemaValidation".to_string(),
            "EdgionConfigDataManaged" | "edgionconfigdatamanaged" | "data-managed" => {
                "EdgionPlugins/EdgionConfigDataManaged".to_string()
            }
            "global-access-control" | "globalaccesscontrol" | "gac" => "EdgionPlugins/GlobalAccessControl".to_string(),
            "global-connection-ip-restriction" | "globalconnectioniprestriction" | "gcir" => {
                "EdgionPlugins/GlobalConnectionIpRestriction".to_string()
            }
            "global-plugins-snapshot" | "globalpluginssnapshot" | "gp-snapshot" => {
                "EdgionPlugins/GlobalPluginsSnapshot".to_string()
            }
            "resolved-refs" | "resolvedrefs" | "ep-resolved-refs" => "EdgionPlugins/ResolvedRefs".to_string(),
            "dangling-extension-ref" | "danglingextensionref" => "HTTPRoute/DanglingExtensionRef".to_string(),
            "linksys-status" | "linksysstatus" | "ls-status" => "LinkSys/Status".to_string(),
            "ref-grant-status" => "Status/RefGrant".to_string(),
            "hostname-intersection-status" => "Status/HostnameIntersection".to_string(),
            "backend-tls-status" | "btls-status" => "Status/BackendTLSPolicy".to_string(),
            "gateway-frontend-validation-status" | "gwfv-status" => "Status/GatewayFrontendValidation".to_string(),
            "grpc-route-status" | "grpcroutestatus" => "Status/GRPCRoute".to_string(),
            "tls-route-status" | "tlsroutestatus" => "Status/TLSRoute".to_string(),
            "observed-generation-status" | "observedgeneration" => "Status/ObservedGeneration".to_string(),
            _ => cmd.to_string(),
        };
    }

    //  -r/-i
    match (resource, item) {
        (Some(r), Some(i)) => format!("{}/{}", r, i),
        (Some(r), None) => r.to_string(),
        (None, Some(i)) => format!("HTTPRoute/{}", i), //  HTTPRoute
        (None, None) => "all".to_string(),
    }
}

/// Return the port-config key for the given suite name.
fn suite_to_port_key(suite: &str) -> &str {
    match suite {
        // Control-plane suite: no data-plane listener of its own; borrow a valid
        // HTTP port key so the per-suite context still resolves a port.
        "EdgionBackendTrafficPolicy" => "HTTPRoute/Backend/CircuitBreaker",
        // Data-plane suite: dynamic upstream authority (Host/SNI dispatch). Has
        // its own Gateway/HTTPRoute fixture and its own ports.json entry, so
        // (unlike the control-plane suite above) it does not borrow another
        // suite's port. Accept both the brief's literal `-i upstream_authority`
        // and the repo's usual CamelCase item spelling.
        "EdgionBackendTrafficPolicy/upstream_authority" | "EdgionBackendTrafficPolicy/UpstreamAuthority" => {
            "EdgionBackendTrafficPolicy/UpstreamAuthority"
        }
        // Data-plane suite: per-backend retry budget (retryConstraint). Own
        // Gateway/HTTPRoute/EBTP fixture and ports.json entry.
        "EdgionBackendTrafficPolicy/RetryBudget" => "EdgionBackendTrafficPolicy/RetryBudget",
        // Data-plane suite: per-backend connect-timeout override. Own
        // Gateway/HTTPRoute/EBTP fixture and ports.json entry.
        "EdgionBackendTrafficPolicy/ConnectTimeout" => "EdgionBackendTrafficPolicy/ConnectTimeout",
        // HTTPRoute
        "HTTPRoute/Basic" | "HTTPRoute" => "HTTPRoute/Basic",
        "HTTPRoute/Delegation" => "HTTPRoute/Delegation",
        "HTTPRoute/Match" => "HTTPRoute/Match",
        "HTTPRoute/Backend" | "HTTPRoute/Backend/LBRoundRobin" => "HTTPRoute/Backend/LBRoundRobin",
        "HTTPRoute/Backend/LBConsistentHash" => "HTTPRoute/Backend/LBConsistentHash",
        "HTTPRoute/Backend/LBLeastConn" => "HTTPRoute/Backend/LBLeastConn",
        "HTTPRoute/Backend/LBLeastConnRetryOnStatus" => "HTTPRoute/Backend/LBLeastConnRetryOnStatus",
        "HTTPRoute/Backend/LBEwma" => "HTTPRoute/Backend/LBEwma",
        "HTTPRoute/Backend/LBEwmaRetryOnStatus" => "HTTPRoute/Backend/LBEwmaRetryOnStatus",
        "HTTPRoute/Backend/WeightedBackend" => "HTTPRoute/Backend/WeightedBackend",
        "HTTPRoute/Backend/Timeout" => "HTTPRoute/Backend/Timeout",
        "HTTPRoute/Backend/RetryOnStatus" => "HTTPRoute/Backend/RetryOnStatus",
        "HTTPRoute/Backend/PerTryTimeout" => "HTTPRoute/Backend/PerTryTimeout",
        "HTTPRoute/Backend/CircuitBreaker" => "HTTPRoute/Backend/CircuitBreaker",
        "HTTPRoute/Backend/HealthCheck" => "HTTPRoute/Backend/HealthCheck",
        "HTTPRoute/Backend/GrpcHealthCheck" => "HTTPRoute/Backend/GrpcHealthCheck",
        "HTTPRoute/Backend/HealthCheckTransition" => "HTTPRoute/Backend/HealthCheckTransition",
        "HTTPRoute/Backend/DnsResolutionFailure" => "HTTPRoute/Backend/DnsResolutionFailure",
        "HTTPRoute/Backend/DnsResolution" => "HTTPRoute/Backend/DnsResolution",
        "HTTPRoute/Backend/PartiallyInvalid" => "HTTPRoute/Backend/PartiallyInvalid",
        "HTTPRoute/Backend/SessionPersistence" => "HTTPRoute/Backend/SessionPersistence",
        "HTTPRoute/Backend/SessionPersistenceBackendChange" => "HTTPRoute/Backend/SessionPersistenceBackendChange",
        "HTTPRoute/Backend/SessionPersistenceWeighted" => "HTTPRoute/Backend/SessionPersistenceWeighted",
        "HTTPRoute/Backend/H2BackendMtlsPersistence" => "HTTPRoute/Backend/H2BackendMtlsPersistence",
        "HTTPRoute/Filters/FilterConflict" => "HTTPRoute/Filters/FilterConflict",
        "HTTPRoute/Filters/PercentMirror" => "HTTPRoute/Filters/PercentMirror",
        "HTTPRoute/Filters" | "HTTPRoute/Filters/Redirect" => "HTTPRoute/Filters/Redirect",
        "HTTPRoute/Filters/Security" => "HTTPRoute/Filters/Security",
        "HTTPRoute/Protocol" => "HTTPRoute/Protocol/WebSocket",
        "HTTPRoute/Protocol/WebSocket" => "HTTPRoute/Protocol/WebSocket",
        "HTTPRoute/Protocol/H2C" => "HTTPRoute/Protocol/H2C",
        "HTTPRoute/Protocol/GrpcWeb" => "HTTPRoute/Protocol/GrpcWeb",
        "HTTPRoute/Protocol/H2TrailerMalformed" => "HTTPRoute/Protocol/H2TrailerMalformed",
        "HTTPRoute/MultiPort" => "HTTPRoute/MultiPort",
        // DanglingExtensionRef is parented to edgion-plugins-gateway (port 31180),
        // shared with EdgionPlugins ExtensionRef-based scenarios.
        "HTTPRoute/DanglingExtensionRef" => "EdgionPlugins",
        // GRPCRoute
        "GRPCRoute/Basic" | "GRPCRoute" => "GRPCRoute/Basic",
        "GRPCRoute/Delegation" => "GRPCRoute/Delegation",
        "GRPCRoute/Match" => "GRPCRoute/Match",
        "GRPCRoute/UnmatchedRoute" => "GRPCRoute/UnmatchedRoute",
        "GRPCRoute/RateLimit" => "GRPCRoute/RateLimit",
        "GRPCRoute/Backend/SessionPersistence" => "GRPCRoute/Backend/SessionPersistence",
        "GRPCRoute/Backend/SessionPersistenceWeighted" => "GRPCRoute/Backend/SessionPersistenceWeighted",
        // TCPRoute
        "TCPRoute/Basic" | "TCPRoute" => "TCPRoute/Basic",
        "TCPRoute/IdleTimeout" => "TCPRoute/Basic",
        "TCPRoute/KeepaliveSmoke" => "TCPRoute/Basic",
        "TCPRoute/StreamPlugins" => "TCPRoute/StreamPlugins",
        "TCPRoute/GlobalConnectionIpRestriction" => "TCPRoute/GlobalConnectionIpRestriction",
        // TLSRoute
        "TLSRoute/Basic" | "TLSRoute" => "TLSRoute/Basic",
        "TLSRoute/ProxyProtocol" => "TLSRoute/ProxyProtocol",
        "TLSRoute/StreamPlugins" => "TLSRoute/StreamPlugins",
        "TLSRoute/MultiSNI" => "TLSRoute/MultiSNI",
        "TLSRoute/BothAbsentParentRef" => "TLSRoute/BothAbsentParentRef",
        "TLSRoute/Passthrough" => "TLSRoute/Passthrough",
        "TLSRoute/UnknownKindBackend" => "TLSRoute/UnknownKindBackend",
        // UDPRoute
        "UDPRoute/Basic" | "UDPRoute" => "UDPRoute/Basic",
        "UDPRoute/StreamPlugins" => "UDPRoute/StreamPlugins",
        // Gateway
        "Gateway/Security" | "Gateway" => "Gateway/Security",
        "Gateway/RealIP" => "Gateway/RealIP",
        "Gateway/TLS/BackendTLS" => "Gateway/TLS/BackendTLS",
        "Gateway/TLS/BackendTLSSAN" => "Gateway/TLS/BackendTLSSAN",
        "Gateway/TLS/BackendTLSGatewayCert" => "Gateway/TLS/BackendTLSGatewayCert",
        "Gateway/TLS/GatewayTLS" => "Gateway/TLS/GatewayTLS",
        "Gateway/TLS/MisdirectedRequest" => "Gateway/TLS/MisdirectedRequest",
        "Gateway/TLS/NoHostnameListener" => "Gateway/TLS/NoHostnameListener",
        "Gateway/FrontendValidation" => "Gateway/FrontendValidation",
        "EdgionPlugins/DebugAccessLog" => "EdgionPlugins",
        "EdgionPlugins/PluginCondition" => "EdgionPlugins",
        "EdgionPlugins/PluginCondition/AllConditions" => "EdgionPlugins",
        "EdgionPlugins/Cors" => "EdgionPlugins",
        "EdgionPlugins/Csrf" => "EdgionPlugins",
        "EdgionPlugins/CtxSet" => "EdgionPlugins",
        "EdgionPlugins/BasicAuth" => "EdgionPlugins",
        "EdgionPlugins/BasicAuthDynamic" => "EdgionPlugins",
        "EdgionPlugins/BasicAuthDynamicGrouped" => "EdgionPlugins",
        "EdgionPlugins/JwtAuth" => "EdgionPlugins",
        "EdgionPlugins/JweDecrypt" => "EdgionPlugins",
        "EdgionPlugins/KeyAuth" => "EdgionPlugins",
        "EdgionPlugins/HmacAuth" => "EdgionPlugins",
        "EdgionPlugins/HeaderCertAuth" => "EdgionPlugins",
        "EdgionPlugins/LdapAuth" => "EdgionPlugins",
        "EdgionPlugins/LdapAuthTls" => "EdgionPlugins",
        "EdgionPlugins/ProxyRewrite" => "EdgionPlugins",
        "EdgionPlugins/RateLimit" => "EdgionPlugins",
        "EdgionPlugins/RateLimitDynamic" => "EdgionPlugins",
        "EdgionPlugins/RateLimitRedis" => "EdgionPlugins",
        "EdgionPlugins/BandwidthLimit" => "EdgionPlugins",
        "EdgionPlugins/RealIp" => "EdgionPlugins",
        "EdgionPlugins/TraceContext" => "EdgionPlugins",
        "EdgionPlugins/Wasm" => "EdgionPlugins",
        "EdgionPlugins/RequestMirror" => "EdgionPlugins",
        "EdgionPlugins/RequestRestriction" => "EdgionPlugins",
        "EdgionPlugins/IpRestriction" => "EdgionPlugins",
        "EdgionPlugins/IpRestrictionRefs" => "EdgionPlugins",
        "EdgionPlugins/IpRestrictionPartialGood" => "EdgionPlugins",
        "EdgionPlugins/ResponseRewrite" => "EdgionPlugins",
        "EdgionPlugins/FaultInjection" => "EdgionPlugins",
        "EdgionPlugins/ForwardAuth" => "EdgionPlugins",
        "EdgionPlugins/ExtProc" => "EdgionPlugins",
        "EdgionPlugins/DirectEndpoint" => "EdgionPlugins",
        "EdgionPlugins/DynamicInternalUpstream" => "EdgionPlugins",
        "EdgionPlugins/DynamicExternalUpstream" => "EdgionPlugins",
        "EdgionPlugins/RegionRoute" => "EdgionPlugins",
        "EdgionPlugins/EdgionConfigDataManaged" => "EdgionPlugins",
        "EdgionPlugins/GlobalAccessControl" => "EdgionPlugins",
        "EdgionPlugins/Canary" => "EdgionPlugins",
        "EdgionPlugins/GlobalConnectionIpRestriction" => "EdgionPlugins",
        "EdgionPlugins/GlobalPluginsSnapshot" => "EdgionPlugins",
        "EdgionPlugins/EdgionConfigDataStatus" => "EdgionPlugins",
        "EdgionPlugins/EdgionConfigDataStatus/RefFlap" => "EdgionPlugins",
        "EdgionPlugins/PluginValidationStatus" => "EdgionPlugins",
        "EdgionPlugins/ResolvedRefs" => "EdgionPlugins",
        "EdgionPlugins/AllEndpointStatus" => "EdgionPlugins",
        "LinkSys/Status" => "EdgionPlugins",
        "EdgionPlugins/OpenidConnect" => "EdgionPlugins",
        "EdgionPlugins/PathNormalization" => "EdgionPlugins",
        "EdgionPlugins/PathNormalization/PreserveParam" => "EdgionPlugins",
        "EdgionPlugins/PathNormalization/ForwardView" => "EdgionPlugins",
        "EdgionPlugins/MatchedRouteMeta" => "EdgionPlugins",
        "EdgionPlugins/AccessLogExtern" => "EdgionPlugins/AccessLogExtern",
        "EdgionPlugins/WebhookKeyGet" => "EdgionPlugins",
        "EdgionPlugins/Dsl" => "EdgionPlugins",
        "EdgionPlugins/DslBody" => "EdgionPlugins",
        "EdgionPlugins/BodyPlugins" => "EdgionPlugins",
        "EdgionPlugins/JsonSchemaValidation" => "EdgionPlugins",
        "EdgionPlugins/BodyDynamic" => "EdgionPlugins",
        "EdgionPlugins/DslDynamic" => "EdgionPlugins",
        "EdgionPlugins/DslGrpcError" => "GRPCRoute/Basic",
        "Gateway/AllowedRoutes/Selector" => "Gateway/AllowedRoutes/Selector",
        "Status/RefGrant/TCPRouteCrossNsSvc" => "Status/RefGrant/TCPRouteCrossNsSvc",
        "Status/RefGrant/TLSRouteCrossNsSvc" => "Status/RefGrant/TLSRouteCrossNsSvc",
        "Status/RefGrant/EdgionConfigData" => "Status/RefGrant/EdgionConfigData",
        "Status/ObservedGeneration/HighFrequency" => "Status/ObservedGeneration/HighFrequency",
        "Status/BackendTLSPolicy/RefFlap" => "Status/BackendTLSPolicy/RefFlap",
        "Gateway/StreamPlugins" => "Gateway/StreamPlugins",
        "Gateway/PortConflict" => "Gateway/PortConflict",
        // EdgionTls
        "EdgionTls" | "EdgionTls/https" => "EdgionTls/https",
        "EdgionTls/grpctls" => "EdgionTls/grpctls",
        "EdgionTls/mTLS" => "EdgionTls/mTLS",
        "EdgionTls/cipher" => "EdgionTls/cipher",
        "EdgionTls/fallback_sni" => "EdgionTls/fallback_sni",
        "EdgionTls/port_only" => "EdgionTls/port_only",
        "EdgionTls/BothAbsentParentRef" => "EdgionTls/BothAbsentParentRef",
        "EdgionTls/wildcard_single_label" => "EdgionTls/wildcard_single_label",
        _ => suite,
    }
}

/// Register test suites matching the given suite name into the runner.
fn add_suites_for_suite(runner: &mut TestRunner, suite: &str, gateway: bool, phase: Option<&str>) {
    match suite {
        // EdgionBackendTrafficPolicy (control-plane: policy sync, LB store
        // mapping, oldest-wins conflict, hot reload, resync survival).
        "EdgionBackendTrafficPolicy" => {
            runner.add_suite(Box::new(suites::EdgionBackendTrafficPolicyTestSuite));
        }
        // Dynamic upstream authority (Task 8 of the dynamic-upstream-authority
        // feature): HTTPS/cleartext per-tenant dispatch, fail-closed, injection,
        // outbound-path degradation, pinning, and health-probe authority.
        "EdgionBackendTrafficPolicy/upstream_authority" | "EdgionBackendTrafficPolicy/UpstreamAuthority" => {
            runner.add_suite(Box::new(suites::UpstreamAuthorityTestSuite));
        }
        // Per-backend retry budget (Task 8 of the retry-budget / retryConstraint
        // feature): floor-permitted trickle at low volume, percentage cap
        // refusing the majority of retries under load with a deterministic 503,
        // and the denial counter advancing.
        "EdgionBackendTrafficPolicy/RetryBudget" => {
            runner.add_suite(Box::new(suites::RetryBudgetTestSuite));
        }
        // Per-backend connect-timeout override: a blackholed backend with the
        // BTP `connection.connectTimeout` override fails fast, while a sibling
        // backend with no override keeps the global default and still works.
        "EdgionBackendTrafficPolicy/ConnectTimeout" => {
            runner.add_suite(Box::new(suites::ConnectTimeoutTestSuite));
        }
        // HTTPRoute
        "HTTPRoute/Basic" | "HTTPRoute" => {
            runner.add_suite(Box::new(suites::HttpTestSuite));
        }
        "HTTPRoute/Delegation" => {
            if !gateway {
                eprintln!("Error: HTTPRoute/Delegation tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::DelegationTestSuite));
        }
        "HTTPRoute/Match" => {
            if !gateway {
                eprintln!("Error: HTTPRoute/Match tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HttpMatchTestSuite));
        }
        "HTTPRoute/Backend" => {
            if !gateway {
                eprintln!("Error: HTTPRoute/Backend tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::LBRoundRobinTestSuite));
            runner.add_suite(Box::new(suites::LBConsistentHashTestSuite));
            runner.add_suite(Box::new(suites::LBLeastConnTestSuite));
            runner.add_suite(Box::new(suites::LBLeastConnRetryOnStatusTestSuite));
            runner.add_suite(Box::new(suites::LBEwmaTestSuite));
            runner.add_suite(Box::new(suites::LBEwmaRetryOnStatusTestSuite));
            runner.add_suite(Box::new(suites::WeightedBackendTestSuite));
            runner.add_suite(Box::new(suites::TimeoutTestSuite));
            runner.add_suite(Box::new(suites::HealthCheckTestSuite));
            runner.add_suite(Box::new(suites::GrpcHealthCheckTestSuite));
            runner.add_suite(Box::new(suites::RetryOnStatusTestSuite));
            runner.add_suite(Box::new(suites::PerTryTimeoutTestSuite));
            runner.add_suite(Box::new(suites::CircuitBreakerTestSuite));
            runner.add_suite(Box::new(suites::PanicThresholdTestSuite));
            runner.add_suite(Box::new(suites::SessionPersistenceTestSuite));
        }
        "HTTPRoute/Backend/LBRoundRobin" => {
            if !gateway {
                eprintln!("Error: LB RoundRobin tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::LBRoundRobinTestSuite));
        }
        "HTTPRoute/Backend/LBConsistentHash" => {
            if !gateway {
                eprintln!("Error: LB ConsistentHash tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::LBConsistentHashTestSuite));
        }
        "HTTPRoute/Backend/LBLeastConn" => {
            if !gateway {
                eprintln!("Error: LB LeastConn tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::LBLeastConnTestSuite));
        }
        "HTTPRoute/Backend/LBLeastConnRetryOnStatus" => {
            if !gateway {
                eprintln!("Error: LB LeastConn RetryOnStatus tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::LBLeastConnRetryOnStatusTestSuite));
        }
        "HTTPRoute/Backend/LBEwma" => {
            if !gateway {
                eprintln!("Error: LB EWMA tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::LBEwmaTestSuite));
        }
        "HTTPRoute/Backend/LBEwmaRetryOnStatus" => {
            if !gateway {
                eprintln!("Error: LB EWMA RetryOnStatus tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::LBEwmaRetryOnStatusTestSuite));
        }
        "HTTPRoute/Backend/WeightedBackend" => {
            if !gateway {
                eprintln!("Error: Weighted backend tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::WeightedBackendTestSuite));
        }
        "HTTPRoute/Backend/Timeout" => {
            if !gateway {
                eprintln!("Error: Timeout tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TimeoutTestSuite));
        }
        "HTTPRoute/Backend/RetryOnStatus" => {
            if !gateway {
                eprintln!("Error: RetryOnStatus tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::RetryOnStatusTestSuite));
        }
        "HTTPRoute/Backend/PerTryTimeout" => {
            if !gateway {
                eprintln!("Error: PerTryTimeout tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::PerTryTimeoutTestSuite));
        }
        "HTTPRoute/Backend/CircuitBreaker" => {
            if !gateway {
                eprintln!("Error: CircuitBreaker tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::CircuitBreakerTestSuite));
        }
        "HTTPRoute/Backend/HealthCheck" => {
            if !gateway {
                eprintln!("Error: HealthCheck tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HealthCheckTestSuite));
        }
        "HTTPRoute/Backend/PanicThreshold" => {
            if !gateway {
                eprintln!("Error: PanicThreshold tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::PanicThresholdTestSuite));
        }
        "HTTPRoute/Backend/GrpcHealthCheck" => {
            if !gateway {
                eprintln!("Error: GrpcHealthCheck tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GrpcHealthCheckTestSuite));
        }
        "HTTPRoute/Backend/HealthCheckTransition" => {
            if !gateway {
                eprintln!("Error: HealthCheckTransition tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HealthCheckTransitionTestSuite));
        }
        "HTTPRoute/Backend/DnsResolutionFailure" => {
            if !gateway {
                eprintln!("Error: DnsResolutionFailure tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::DnsResolutionFailureTestSuite));
        }
        "HTTPRoute/Backend/DnsResolution" => {
            if !gateway {
                eprintln!("Error: DnsResolution tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::DnsResolutionTestSuite));
        }
        "HTTPRoute/Backend/PartiallyInvalid" => {
            if !gateway {
                eprintln!("Error: PartiallyInvalid tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::PartiallyInvalidTestSuite));
        }
        "HTTPRoute/Backend/SessionPersistence" => {
            if !gateway {
                eprintln!("Error: Session Persistence tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::SessionPersistenceTestSuite));
        }
        "HTTPRoute/Backend/SessionPersistenceBackendChange" => {
            if !gateway {
                eprintln!("Error: SessionPersistenceBackendChange tests require --gateway flag");
                std::process::exit(1);
            }
            match phase {
                Some("initial") => {
                    runner.add_suite(Box::new(suites::SpChangeInitialTestSuite));
                }
                Some("final") => {
                    runner.add_suite(Box::new(suites::SpChangeFinalTestSuite));
                }
                _ => {
                    runner.add_suite(Box::new(suites::SpChangeInitialTestSuite));
                    runner.add_suite(Box::new(suites::SpChangeFinalTestSuite));
                }
            }
        }
        "HTTPRoute/Backend/SessionPersistenceWeighted" => {
            if !gateway {
                eprintln!("Error: SessionPersistenceWeighted tests require --gateway flag");
                std::process::exit(1);
            }
            match phase {
                Some("initial") => {
                    runner.add_suite(Box::new(suites::SpWeightedInitialTestSuite));
                }
                Some("zero_weight") => {
                    runner.add_suite(Box::new(suites::SpWeightedZeroWeightTestSuite));
                }
                Some("final") => {
                    runner.add_suite(Box::new(suites::SpWeightedFinalTestSuite));
                }
                _ => {
                    runner.add_suite(Box::new(suites::SpWeightedInitialTestSuite));
                    runner.add_suite(Box::new(suites::SpWeightedZeroWeightTestSuite));
                    runner.add_suite(Box::new(suites::SpWeightedFinalTestSuite));
                }
            }
        }
        "HTTPRoute/Backend/H2BackendMtlsPersistence" => {
            if !gateway {
                eprintln!("Error: H2BackendMtlsPersistence tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::H2MtlsPersistenceTestSuite));
        }
        "HTTPRoute/Backend/BackendTLSSNIHostOverride" => {
            if !gateway {
                eprintln!("Error: BackendTLSSNIHostOverride tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::BackendTlsSniHostOverrideTestSuite));
        }
        "HTTPRoute/Filters" => {
            if !gateway {
                eprintln!("Error: HTTPRoute/Filters tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HttpRedirectTestSuite));
            runner.add_suite(Box::new(suites::HttpSecurityTestSuite));
            runner.add_suite(Box::new(suites::HeaderModifierTestSuite));
            runner.add_suite(Box::new(suites::FilterConflictTestSuite));
            runner.add_suite(Box::new(suites::PercentMirrorTestSuite));
        }
        "HTTPRoute/Filters/PercentMirror" => {
            if !gateway {
                eprintln!("Error: PercentMirror tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::PercentMirrorTestSuite));
        }
        "HTTPRoute/Filters/FilterConflict" => {
            if !gateway {
                eprintln!("Error: FilterConflict tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::FilterConflictTestSuite));
        }
        "HTTPRoute/Filters/Redirect" => {
            if !gateway {
                eprintln!("Error: HTTP Redirect tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HttpRedirectTestSuite));
        }
        "HTTPRoute/Filters/Security" => {
            if !gateway {
                eprintln!("Error: HTTP Security tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HttpSecurityTestSuite));
        }
        "HTTPRoute/Filters/HeaderModifier" => {
            if !gateway {
                eprintln!("Error: HTTP Header Modifier tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HeaderModifierTestSuite));
        }
        "HTTPRoute/DanglingExtensionRef" => {
            if !gateway {
                eprintln!("Error: HTTPRoute/DanglingExtensionRef tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::DanglingExtensionRefTestSuite));
        }
        "HTTPRoute/Protocol" => {
            runner.add_suite(Box::new(suites::WebSocketTestSuite::new(gateway)));
            runner.add_suite(Box::new(suites::H2cTestSuite));
            if gateway {
                runner.add_suite(Box::new(suites::HttpsTestSuite));
            }
        }
        "HTTPRoute/Protocol/WebSocket" => {
            runner.add_suite(Box::new(suites::WebSocketTestSuite::new(gateway)));
        }
        "HTTPRoute/Protocol/H2C" => {
            if !gateway {
                eprintln!("Error: HTTPRoute/Protocol/H2C tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::H2cTestSuite));
        }
        "HTTPRoute/Protocol/GrpcWeb" => {
            if !gateway {
                eprintln!("Error: HTTPRoute/Protocol/GrpcWeb tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GrpcWebTestSuite));
        }
        "HTTPRoute/Protocol/H2TrailerMalformed" => {
            if !gateway {
                eprintln!("Error: HTTPRoute/Protocol/H2TrailerMalformed tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::H2TrailerMalformedTestSuite));
        }
        "HTTPRoute/Protocol/HTTPS" => {
            if !gateway {
                eprintln!("Error: HTTPS tests only support Gateway mode. Use -g flag.");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HttpsTestSuite));
        }
        "HTTPRoute/MultiPort" => {
            if !gateway {
                eprintln!("Error: HTTPRoute/MultiPort tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::MultiPortTestSuite));
        }
        // GRPCRoute
        "GRPCRoute" => {
            runner.add_suite(Box::new(suites::GrpcTestSuite));
            if gateway {
                runner.add_suite(Box::new(suites::GrpcMatchTestSuite));
            }
        }
        "GRPCRoute/Basic" => {
            runner.add_suite(Box::new(suites::GrpcTestSuite));
        }
        "GRPCRoute/Delegation" => {
            if !gateway {
                eprintln!("Error: GRPCRoute/Delegation tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GrpcDelegationTestSuite));
        }
        "GRPCRoute/Match" => {
            if !gateway {
                eprintln!("Error: GRPCRoute/Match tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GrpcMatchTestSuite));
        }
        "GRPCRoute/UnmatchedRoute" => {
            if !gateway {
                eprintln!("Error: GRPCRoute/UnmatchedRoute tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GrpcUnmatchedRouteTestSuite));
        }
        "GRPCRoute/RateLimit" => {
            if !gateway {
                eprintln!("Error: GRPCRoute/RateLimit tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GrpcRateLimitTestSuite));
        }
        "GRPCRoute/Backend/SessionPersistence" => {
            if !gateway {
                eprintln!("Error: GRPCRoute/Backend/SessionPersistence tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GrpcSessionPersistenceTestSuite));
        }
        "GRPCRoute/Backend/SessionPersistenceWeighted" => {
            if !gateway {
                eprintln!("Error: GRPCRoute/Backend/SessionPersistenceWeighted tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GrpcSpWeightedTestSuite));
        }
        "GRPCRoute/TLS" => {
            if !gateway {
                eprintln!("Error: GRPCRoute/TLS tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GrpcTlsTestSuite));
        }
        // TCP/UDP
        "tcp" | "TCPRoute" | "TCPRoute/Basic" => {
            runner.add_suite(Box::new(suites::TcpTestSuite));
        }
        "TCPRoute/StreamPlugins" => {
            if !gateway {
                eprintln!("Error: TCPRoute/StreamPlugins tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TcpStreamPluginsTestSuite));
        }
        "TCPRoute/GlobalConnectionIpRestriction" => {
            if !gateway {
                eprintln!("Error: TCPRoute/GlobalConnectionIpRestriction tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GcirEnforcementTestSuite));
        }
        "TCPRoute/IdleTimeout" => {
            if !gateway {
                eprintln!("Error: TCPRoute/IdleTimeout tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TcpIdleTimeoutTestSuite));
        }
        "TCPRoute/KeepaliveSmoke" => {
            if !gateway {
                eprintln!("Error: TCPRoute/KeepaliveSmoke tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TcpKeepaliveSmokeTestSuite));
        }
        // TLSRoute
        "TLSRoute" | "TLSRoute/Basic" => {
            if !gateway {
                eprintln!("Error: TLSRoute tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TlsRouteTestSuite));
        }
        "TLSRoute/ProxyProtocol" => {
            if !gateway {
                eprintln!("Error: TLSRoute/ProxyProtocol tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TlsProxyProtocolTestSuite));
        }
        "TLSRoute/StreamPlugins" => {
            if !gateway {
                eprintln!("Error: TLSRoute/StreamPlugins tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TlsStreamPluginsTestSuite));
        }
        "TLSRoute/MultiSNI" => {
            if !gateway {
                eprintln!("Error: TLSRoute/MultiSNI tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TlsMultiSniTestSuite));
        }
        "TLSRoute/BothAbsentParentRef" => {
            if !gateway {
                eprintln!("Error: TLSRoute/BothAbsentParentRef tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TlsBothAbsentParentRefTestSuite));
        }
        "TLSRoute/Passthrough" => {
            if !gateway {
                eprintln!("Error: TLSRoute/Passthrough tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TlsPassthroughTestSuite));
        }
        "TLSRoute/UnknownKindBackend" => {
            if !gateway {
                eprintln!("Error: TLSRoute/UnknownKindBackend tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TlsUnknownKindBackendTestSuite));
        }
        "udp" | "UDPRoute" | "UDPRoute/Basic" => {
            runner.add_suite(Box::new(suites::UdpTestSuite));
        }
        "UDPRoute/StreamPlugins" => {
            if !gateway {
                eprintln!("Error: UDPRoute/StreamPlugins tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::UdpStreamPluginsTestSuite));
        }
        // Gateway
        "Gateway" => {
            if !gateway {
                eprintln!("Error: Gateway tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::SecurityTestSuite));
            runner.add_suite(Box::new(suites::RealIpTestSuite));
            runner.add_suite(Box::new(suites::PluginLogsTestSuite));
        }
        "Gateway/Security" => {
            if !gateway {
                eprintln!("Error: Gateway/Security tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::SecurityTestSuite));
        }
        "Gateway/RealIP" => {
            if !gateway {
                eprintln!("Error: Gateway/RealIP tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::RealIpTestSuite));
        }
        "Gateway/TLS/BackendTLS" => {
            if !gateway {
                eprintln!("Error: Gateway/TLS/BackendTLS tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::BackendTlsTestSuite));
        }
        "Gateway/TLS/BackendTLSSAN" => {
            if !gateway {
                eprintln!("Error: Gateway/TLS/BackendTLSSAN tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::BackendTlsSanTestSuite));
        }
        "Gateway/TLS/BackendTLSGatewayCert" => {
            if !gateway {
                eprintln!("Error: Gateway/TLS/BackendTLSGatewayCert tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::BackendTlsGatewayCertTestSuite));
        }
        "Gateway/TLS/GatewayTLS" => {
            if !gateway {
                eprintln!("Error: Gateway/TLS/GatewayTLS tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GatewayTlsTestSuite));
        }
        "Gateway/TLS/NoHostnameListener" => {
            if !gateway {
                eprintln!("Error: Gateway/TLS/NoHostnameListener tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GatewayTlsNoHostnameListenerTestSuite));
        }
        "Gateway/TLS/MisdirectedRequest" => {
            if !gateway {
                eprintln!("Error: Gateway/TLS/MisdirectedRequest tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::MisdirectedRequestTestSuite));
        }
        "Gateway/FrontendValidation" => {
            if !gateway {
                eprintln!("Error: Gateway/FrontendValidation tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::FrontendValidationTestSuite));
        }
        "EdgionPlugins/DebugAccessLog" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/DebugAccessLog tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::PluginLogsTestSuite));
        }
        "EdgionTls/port_only" => {
            if !gateway {
                eprintln!("Error: EdgionTls/port_only tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::PortOnlyEdgionTlsTestSuite));
        }
        "EdgionTls/BothAbsentParentRef" => {
            if !gateway {
                eprintln!("Error: EdgionTls/BothAbsentParentRef tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::EdgionTlsBothAbsentParentRefTestSuite));
        }
        "EdgionPlugins/PluginCondition" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/PluginCondition tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::PluginConditionTestSuite));
            runner.add_suite(Box::new(suites::AllConditionsTestSuite));
        }
        "EdgionPlugins/PluginCondition/AllConditions" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/PluginCondition/AllConditions tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::AllConditionsTestSuite));
        }
        "EdgionPlugins/JwtAuth" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/JwtAuth tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::JwtAuthTestSuite));
        }
        "EdgionPlugins/BasicAuth" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/BasicAuth tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::BasicAuthTestSuite));
        }
        "EdgionPlugins/BasicAuthDynamic" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/BasicAuthDynamic tests require --gateway flag");
                std::process::exit(1);
            }
            match phase {
                Some("initial") => {
                    runner.add_suite(Box::new(suites::BasicAuthDynamicInitialTestSuite));
                }
                Some("update") => {
                    runner.add_suite(Box::new(suites::BasicAuthDynamicUpdateTestSuite));
                }
                _ => {
                    runner.add_suite(Box::new(suites::BasicAuthDynamicInitialTestSuite));
                    runner.add_suite(Box::new(suites::BasicAuthDynamicUpdateTestSuite));
                }
            }
        }
        "EdgionPlugins/BasicAuthDynamicGrouped" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/BasicAuthDynamicGrouped tests require --gateway flag");
                std::process::exit(1);
            }
            match phase {
                Some("initial") => {
                    runner.add_suite(Box::new(suites::BasicAuthDynamicGroupedInitialTestSuite));
                }
                Some("update") => {
                    runner.add_suite(Box::new(suites::BasicAuthDynamicGroupedUpdateTestSuite));
                }
                _ => {
                    runner.add_suite(Box::new(suites::BasicAuthDynamicGroupedInitialTestSuite));
                    runner.add_suite(Box::new(suites::BasicAuthDynamicGroupedUpdateTestSuite));
                }
            }
        }
        "EdgionPlugins/JweDecrypt" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/JweDecrypt tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::JweDecryptTestSuite));
        }
        "EdgionPlugins/KeyAuth" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/KeyAuth tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::KeyAuthTestSuite));
        }
        "EdgionPlugins/HmacAuth" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/HmacAuth tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HmacAuthTestSuite));
        }
        "EdgionPlugins/HeaderCertAuth" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/HeaderCertAuth tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HeaderCertAuthTestSuite));
        }
        "EdgionPlugins/LdapAuth" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/LdapAuth tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::LdapAuthTestSuite));
        }
        "EdgionPlugins/LdapAuthTls" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/LdapAuthTls tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::LdapAuthTlsTestSuite));
        }
        "EdgionPlugins/ProxyRewrite" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/ProxyRewrite tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::ProxyRewriteTestSuite));
        }
        "EdgionPlugins/ResponseRewrite" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/ResponseRewrite tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::ResponseRewriteTestSuite));
        }
        "EdgionPlugins/RateLimit" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/RateLimit tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::RateLimitTestSuite));
        }
        "EdgionPlugins/RateLimitDynamic" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/RateLimitDynamic tests require --gateway flag");
                std::process::exit(1);
            }
            match phase {
                Some("initial") => {
                    runner.add_suite(Box::new(suites::RateLimitDynamicInitialTestSuite));
                }
                Some("update") => {
                    runner.add_suite(Box::new(suites::RateLimitDynamicUpdateTestSuite));
                }
                _ => {
                    runner.add_suite(Box::new(suites::RateLimitDynamicInitialTestSuite));
                    runner.add_suite(Box::new(suites::RateLimitDynamicUpdateTestSuite));
                }
            }
        }
        "EdgionPlugins/RateLimitRedis" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/RateLimitRedis tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::RateLimitRedisTestSuite));
        }
        "EdgionPlugins/RealIp" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/RealIp tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::RealIpPluginTestSuite));
        }
        "EdgionPlugins/TraceContext" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/TraceContext tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TraceContextPluginTestSuite));
        }
        "EdgionPlugins/Wasm" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/Wasm tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::WasmPluginTestSuite));
        }
        "EdgionPlugins/RequestMirror" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/RequestMirror tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::RequestMirrorTestSuite));
        }
        "EdgionPlugins/RequestRestriction" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/RequestRestriction tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::RequestRestrictionTestSuite));
        }
        "EdgionPlugins/GlobalAccessControl" => {
            if !gateway {
                eprintln!("Error: GlobalAccessControl tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GlobalAccessControlTestSuite));
        }
        "EdgionPlugins/Canary" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/Canary tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::CanaryTestSuite));
        }
        "EdgionPlugins/IpRestriction" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/IpRestriction tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::IpRestrictionPluginTestSuite));
        }
        "EdgionPlugins/IpRestrictionRefs" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/IpRestrictionRefs tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::IpRestrictionRefsPluginTestSuite));
        }
        "EdgionPlugins/IpRestrictionPartialGood" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/IpRestrictionPartialGood tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::IpRestrictionPartialGoodPluginTestSuite));
        }
        "EdgionPlugins/FaultInjection" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/FaultInjection tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::FaultInjectionTestSuite));
        }
        "EdgionPlugins/ForwardAuth" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/ForwardAuth tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::ForwardAuthTestSuite));
        }
        "EdgionPlugins/ExtProc" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/ExtProc tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::ExtProcTestSuite));
        }
        "EdgionPlugins/DirectEndpoint" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/DirectEndpoint tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::DirectEndpointTestSuite));
        }
        "EdgionPlugins/DynamicInternalUpstream" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/DynamicInternalUpstream tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::DynamicInternalUpstreamTestSuite));
        }
        "EdgionPlugins/DynamicExternalUpstream" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/DynamicExternalUpstream tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::DynamicExternalUpstreamTestSuite));
        }
        "EdgionPlugins/RegionRoute" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/RegionRoute tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::RegionRouteTestSuite));
        }
        "EdgionPlugins/EdgionConfigDataManaged" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/EdgionConfigDataManaged tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::EdgionConfigDataManagedTestSuite));
        }
        "EdgionPlugins/GlobalConnectionIpRestriction" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/GlobalConnectionIpRestriction tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GlobalConnectionIpRestrictionTestSuite));
        }
        "EdgionPlugins/EdgionConfigDataStatus" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/EdgionConfigDataStatus tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::EdgionConfigDataStatusTestSuite));
        }
        "EdgionPlugins/PluginValidationStatus" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/PluginValidationStatus tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::PluginValidationStatusTestSuite));
        }
        "EdgionPlugins/EdgionConfigDataStatus/RefFlap" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/EdgionConfigDataStatus/RefFlap tests require --gateway flag");
                std::process::exit(1);
            }
            match phase {
                Some("initial") => {
                    runner.add_suite(Box::new(suites::EdgionConfigDataFlapInitialTestSuite));
                }
                Some("invalid") => {
                    runner.add_suite(Box::new(suites::EdgionConfigDataFlapInvalidTestSuite));
                }
                Some("valid") => {
                    runner.add_suite(Box::new(suites::EdgionConfigDataFlapValidTestSuite));
                }
                _ => {
                    runner.add_suite(Box::new(suites::EdgionConfigDataFlapInitialTestSuite));
                    runner.add_suite(Box::new(suites::EdgionConfigDataFlapInvalidTestSuite));
                    runner.add_suite(Box::new(suites::EdgionConfigDataFlapValidTestSuite));
                }
            }
        }
        "EdgionPlugins/ResolvedRefs" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/ResolvedRefs tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::ResolvedRefsTestSuite));
        }
        "LinkSys/Status" => {
            if !gateway {
                eprintln!("Error: LinkSys/Status tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::LinkSysStatusTestSuite));
        }
        "EdgionPlugins/AllEndpointStatus" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/AllEndpointStatus tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::AllEndpointStatusTestSuite));
        }
        "EdgionPlugins/OpenidConnect" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/OpenidConnect tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::OpenidConnectTestSuite));
        }
        "EdgionPlugins/PathNormalization" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/PathNormalization tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::PathNormalizationTestSuite));
        }
        "EdgionPlugins/PathNormalization/PreserveParam" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/PathNormalization/PreserveParam tests require --gateway flag");
                std::process::exit(1);
            }
            // NOTE: This suite requires the gateway to be started with
            // preserve_path_parameters: true in the `path_normalization` YAML config.
            // TC-C is excluded from automation (manual curl verifies preserve-mode bypass
            // behaviour, see fixture); TC-B is mode-agnostic and passes in both strip and
            // preserve modes.
            runner.add_suite(Box::new(suites::PathNormalizationPreserveParamTestSuite));
        }
        "EdgionPlugins/PathNormalization/ForwardView" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/PathNormalization/ForwardView tests require --gateway flag");
                std::process::exit(1);
            }
            // Verifies which path the gateway forwards to upstream based on:
            //   TC-A: default (no annotation) → normalized path forwarded
            //   TC-B: annotation forward-raw-path: "true" → raw path forwarded
            //   TC-C: annotation forward-raw-path: "false" → normalized path forwarded
            // Scenarios that require gateway YAML restart (global forward_normalized_path: false,
            // region_route with no annotation) are documented in the fixture YAMLs and suite
            // header comment for manual verification.
            runner.add_suite(Box::new(suites::PathNormalizationForwardViewTestSuite));
        }
        "EdgionPlugins/BandwidthLimit" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/BandwidthLimit tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::BandwidthLimitTestSuite));
        }
        "EdgionPlugins/Cors" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/Cors tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::CorsPluginTestSuite));
        }
        "EdgionPlugins/GlobalPluginsSnapshot" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/GlobalPluginsSnapshot tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GlobalPluginsSnapshotTestSuite));
        }
        "EdgionPlugins/Csrf" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/Csrf tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::CsrfTestSuite));
        }
        "EdgionPlugins/CtxSet" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/CtxSet tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::CtxSetTestSuite));
        }
        "EdgionPlugins/MatchedRouteMeta" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/MatchedRouteMeta tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::MatchedRouteMetaTestSuite));
        }
        "EdgionPlugins/AccessLogExtern" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/AccessLogExtern tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::AccessLogExternTestSuite));
        }
        "EdgionPlugins/WebhookKeyGet" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/WebhookKeyGet tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::WebhookKeyGetTestSuite));
        }
        "EdgionPlugins/Dsl" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/Dsl tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::DslTestSuite));
        }
        "EdgionPlugins/DslBody" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/DslBody tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::DslBodyTestSuite));
        }
        "EdgionPlugins/BodyPlugins" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/BodyPlugins tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::BodyPluginsTestSuite));
        }
        "EdgionPlugins/JsonSchemaValidation" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/JsonSchemaValidation tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::JsonSchemaValidationTestSuite));
        }
        "EdgionPlugins/BodyDynamic" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/BodyDynamic tests require --gateway flag");
                std::process::exit(1);
            }
            match phase {
                Some("initial") => {
                    runner.add_suite(Box::new(suites::BodyDynamicInitialTestSuite));
                }
                Some("update") => {
                    runner.add_suite(Box::new(suites::BodyDynamicUpdateTestSuite));
                }
                None => {
                    runner.add_suite(Box::new(suites::BodyDynamicInitialTestSuite));
                    runner.add_suite(Box::new(suites::BodyDynamicUpdateTestSuite));
                }
                _ => {
                    eprintln!("Error: Invalid phase '{}'. Use 'initial' or 'update'", phase.unwrap());
                    std::process::exit(1);
                }
            }
        }
        "EdgionPlugins/DslDynamic" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/DslDynamic tests require --gateway flag");
                std::process::exit(1);
            }
            match phase {
                Some("initial") => {
                    runner.add_suite(Box::new(suites::DslDynamicInitialTestSuite));
                }
                Some("update") => {
                    runner.add_suite(Box::new(suites::DslDynamicUpdateTestSuite));
                }
                None => {
                    runner.add_suite(Box::new(suites::DslDynamicInitialTestSuite));
                    runner.add_suite(Box::new(suites::DslDynamicUpdateTestSuite));
                }
                _ => {
                    eprintln!("Error: Invalid phase '{}'. Use 'initial' or 'update'", phase.unwrap());
                    std::process::exit(1);
                }
            }
        }
        "EdgionPlugins/DslGrpcError" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/DslGrpcError tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::DslGrpcErrorTestSuite));
        }
        "Gateway/ListenerHostname" => {
            if !gateway {
                eprintln!("Error: Gateway/ListenerHostname tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::ListenerHostnameTestSuite));
        }
        "Gateway/AllowedRoutes/Same" => {
            if !gateway {
                eprintln!("Error: Gateway/AllowedRoutes/Same tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::AllowedRoutesSameNamespaceTestSuite));
        }
        "Gateway/AllowedRoutes/All" => {
            if !gateway {
                eprintln!("Error: Gateway/AllowedRoutes/All tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::AllowedRoutesAllNamespacesTestSuite));
        }
        "Gateway/AllowedRoutes/Kinds" => {
            if !gateway {
                eprintln!("Error: Gateway/AllowedRoutes/Kinds tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::AllowedRoutesKindsTestSuite));
        }
        "Gateway/AllowedRoutes/Selector" => {
            if !gateway {
                eprintln!("Error: Gateway/AllowedRoutes/Selector tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::AllowedRoutesSelectorNamespaceTestSuite));
        }
        "Gateway/Combined" => {
            if !gateway {
                eprintln!("Error: Gateway/Combined tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::CombinedScenariosTestSuite));
        }
        "Gateway/StreamPlugins" => {
            if !gateway {
                eprintln!("Error: Gateway/StreamPlugins tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::StreamPluginsTestSuite));
        }
        "Gateway/PortConflict" => {
            if !gateway {
                eprintln!("Error: Gateway/PortConflict tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::PortConflictTestSuite));
        }
        "Gateway/Dynamic" => {
            if !gateway {
                eprintln!("Error: Gateway/Dynamic tests require --gateway flag");
                std::process::exit(1);
            }
            match phase {
                Some("initial") => {
                    runner.add_suite(Box::new(suites::InitialPhaseTestSuite));
                }
                Some("update") => {
                    runner.add_suite(Box::new(suites::UpdatePhaseTestSuite));
                }
                None => {
                    runner.add_suite(Box::new(suites::InitialPhaseTestSuite));
                    runner.add_suite(Box::new(suites::UpdatePhaseTestSuite));
                }
                _ => {
                    eprintln!("Error: Invalid phase '{}'. Use 'initial' or 'update'", phase.unwrap());
                    std::process::exit(1);
                }
            }
        }
        // EdgionTls
        "EdgionTls" => {
            if !gateway {
                eprintln!("Error: EdgionTls tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HttpsTestSuite));
            runner.add_suite(Box::new(suites::GrpcTlsTestSuite));
            runner.add_suite(Box::new(suites::MtlsTestSuite));
            runner.add_suite(Box::new(suites::MtlsFailcloseTestSuite));
            runner.add_suite(Box::new(suites::PortOnlyEdgionTlsTestSuite));
            runner.add_suite(Box::new(suites::FallbackSniTestSuite));
            runner.add_suite(Box::new(suites::WildcardSingleLabelTestSuite));
            runner.add_suite(Box::new(suites::SniCollisionTestSuite));
        }
        "EdgionTls/https" => {
            if !gateway {
                eprintln!("Error: EdgionTls/https tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HttpsTestSuite));
        }
        "EdgionTls/grpctls" => {
            if !gateway {
                eprintln!("Error: EdgionTls/grpctls tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GrpcTlsTestSuite));
        }
        "EdgionTls/mTLS" => {
            if !gateway {
                eprintln!("Error: EdgionTls/mTLS tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::MtlsTestSuite));
        }
        "EdgionTls/cipher" => {
            if !gateway {
                eprintln!("Error: EdgionTls/cipher tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::CipherTestSuite));
        }
        "EdgionTls/mtls_failclose" => {
            if !gateway {
                eprintln!("Error: EdgionTls/mtls_failclose tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::MtlsFailcloseTestSuite));
        }
        "EdgionTls/fallback_sni" => {
            if !gateway {
                eprintln!("Error: EdgionTls/fallback_sni tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::FallbackSniTestSuite));
        }
        "EdgionTls/wildcard_single_label" => {
            if !gateway {
                eprintln!("Error: EdgionTls/wildcard_single_label tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::WildcardSingleLabelTestSuite));
        }
        "EdgionTls/sni_collision" => {
            if !gateway {
                eprintln!("Error: EdgionTls/sni_collision tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::SniCollisionTestSuite));
        }
        // Status tests
        "Status/RefGrant" => {
            if !gateway {
                eprintln!("Error: Status/RefGrant tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::RefGrantStatusTestSuite));
        }
        "Status/HostnameIntersection" => {
            if !gateway {
                eprintln!("Error: Status/HostnameIntersection tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HostnameIntersectionStatusTestSuite));
        }
        "Status/BackendTLSPolicy" => {
            if !gateway {
                eprintln!("Error: Status/BackendTLSPolicy tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::BackendTlsStatusTestSuite));
        }
        "Status/EdgionBackendTrafficPolicy" => {
            if !gateway {
                eprintln!("Error: Status/EdgionBackendTrafficPolicy tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::EbtpAncestorStatusTestSuite));
        }
        "Status/BackendTLSPolicy/RefFlap" => {
            if !gateway {
                eprintln!("Error: Status/BackendTLSPolicy/RefFlap tests require --gateway flag");
                std::process::exit(1);
            }
            match phase {
                Some("initial") => {
                    runner.add_suite(Box::new(suites::BtlspFlapInitialTestSuite));
                }
                Some("invalid") => {
                    runner.add_suite(Box::new(suites::BtlspFlapInvalidTestSuite));
                }
                Some("valid") => {
                    runner.add_suite(Box::new(suites::BtlspFlapValidTestSuite));
                }
                _ => {
                    runner.add_suite(Box::new(suites::BtlspFlapInitialTestSuite));
                    runner.add_suite(Box::new(suites::BtlspFlapInvalidTestSuite));
                    runner.add_suite(Box::new(suites::BtlspFlapValidTestSuite));
                }
            }
        }
        "Status/GatewayFrontendValidation" => {
            if !gateway {
                eprintln!("Error: Status/GatewayFrontendValidation tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GatewayFrontendValidationStatusTestSuite));
        }
        "Status/TLSListenerMode" => {
            if !gateway {
                eprintln!("Error: Status/TLSListenerMode tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TlsListenerModeStatusTestSuite));
        }
        "Status/GRPCRoute" => {
            if !gateway {
                eprintln!("Error: Status/GRPCRoute tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GrpcRouteStatusTestSuite));
        }
        "Status/TLSRoute" => {
            if !gateway {
                eprintln!("Error: Status/TLSRoute tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TlsRouteStatusTestSuite));
        }
        "Status/ObservedGeneration" => {
            if !gateway {
                eprintln!("Error: Status/ObservedGeneration tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::ObservedGenerationStatusTestSuite));
        }
        "Status/ObservedGeneration/HighFrequency" => {
            if !gateway {
                eprintln!("Error: Status/ObservedGeneration/HighFrequency tests require --gateway flag");
                std::process::exit(1);
            }
            match phase {
                Some("initial") => {
                    runner.add_suite(Box::new(suites::HighFreqInitialTestSuite));
                }
                Some("final") => {
                    runner.add_suite(Box::new(suites::HighFreqFinalTestSuite));
                }
                _ => {
                    runner.add_suite(Box::new(suites::HighFreqInitialTestSuite));
                    runner.add_suite(Box::new(suites::HighFreqFinalTestSuite));
                }
            }
        }
        "Status/RefGrant/TCPRouteCrossNsSvc" => {
            if !gateway {
                eprintln!("Error: Status/RefGrant/TCPRouteCrossNsSvc tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TcpRouteCrossNsSvcTestSuite));
        }
        "Status/RefGrant/TLSRouteCrossNsSvc" => {
            if !gateway {
                eprintln!("Error: Status/RefGrant/TLSRouteCrossNsSvc tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TlsRouteCrossNsSvcTestSuite));
        }
        "Status/RefGrant/EdgionConfigData" => {
            if !gateway {
                eprintln!("Error: Status/RefGrant/EdgionConfigData tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::EdgionConfigDataRefGrantTestSuite));
        }
        // Services tests (ACME, etc.)
        "Services/acme" => {
            runner.add_suite(Box::new(suites::AcmeTestSuite));
        }
        "all" => {
            runner.add_suite(Box::new(suites::HttpTestSuite));
            runner.add_suite(Box::new(suites::GrpcTestSuite));
            runner.add_suite(Box::new(suites::WebSocketTestSuite::new(gateway)));
            runner.add_suite(Box::new(suites::H2cTestSuite));
            runner.add_suite(Box::new(suites::TcpTestSuite));
            runner.add_suite(Box::new(suites::UdpTestSuite));
            if gateway {
                runner.add_suite(Box::new(suites::TcpIdleTimeoutTestSuite));
                runner.add_suite(Box::new(suites::TcpKeepaliveSmokeTestSuite));
                runner.add_suite(Box::new(suites::HttpMatchTestSuite));
                runner.add_suite(Box::new(suites::HttpsTestSuite));
                runner.add_suite(Box::new(suites::GrpcMatchTestSuite));
                runner.add_suite(Box::new(suites::RealIpTestSuite));
                runner.add_suite(Box::new(suites::SecurityTestSuite));
                runner.add_suite(Box::new(suites::HttpSecurityTestSuite));
                runner.add_suite(Box::new(suites::HttpRedirectTestSuite));
                runner.add_suite(Box::new(suites::PluginLogsTestSuite));
                runner.add_suite(Box::new(suites::LBRoundRobinTestSuite));
                runner.add_suite(Box::new(suites::LBConsistentHashTestSuite));
                runner.add_suite(Box::new(suites::LBLeastConnTestSuite));
                runner.add_suite(Box::new(suites::LBLeastConnRetryOnStatusTestSuite));
                runner.add_suite(Box::new(suites::LBEwmaTestSuite));
                runner.add_suite(Box::new(suites::LBEwmaRetryOnStatusTestSuite));
                runner.add_suite(Box::new(suites::WeightedBackendTestSuite));
                runner.add_suite(Box::new(suites::TimeoutTestSuite));
                runner.add_suite(Box::new(suites::HealthCheckTestSuite));
                runner.add_suite(Box::new(suites::GrpcHealthCheckTestSuite));
                runner.add_suite(Box::new(suites::RetryOnStatusTestSuite));
                runner.add_suite(Box::new(suites::PerTryTimeoutTestSuite));
                runner.add_suite(Box::new(suites::CircuitBreakerTestSuite));
                runner.add_suite(Box::new(suites::PanicThresholdTestSuite));
                runner.add_suite(Box::new(suites::MtlsTestSuite));
            }
        }
        _ => {
            eprintln!("Error: Unknown suite: {}", suite);
            std::process::exit(1);
        }
    }
}

async fn run_dynamic_scenario(cli: &Cli, sid: &str) -> Result<()> {
    use anyhow::Context;

    let scenario =
        dynamic::registry::find(sid).ok_or_else(|| anyhow::anyhow!("Unknown dynamic scenario id: {}", sid))?;

    let project_root = std::env::current_dir().context("Failed to get current directory")?;
    let conf_root = project_root.join("edgion-tests/integration/conf");

    // In Gateway mode, pull the default HTTP port from ports.json; otherwise use the CLI defaults.
    let (http_port, grpc_port, tcp_port, udp_port, https_port, grpc_https_port, http_host, grpc_host) = if cli.gateway {
        match port_config::PortConfig::load() {
            Ok(cfg) => {
                let p = cfg.get_ports("default");
                (
                    p.http.unwrap_or(cli.http_port),
                    p.grpc.unwrap_or(cli.grpc_port),
                    p.tcp.unwrap_or(cli.tcp_port),
                    p.udp.unwrap_or(cli.udp_port),
                    p.https.unwrap_or(cli.https_port),
                    p.grpc_tls.unwrap_or(cli.grpc_https_port),
                    Some("test.example.com".to_string()),
                    Some("grpc.example.com".to_string()),
                )
            }
            Err(_) => (
                cli.http_port,
                cli.grpc_port,
                cli.tcp_port,
                cli.udp_port,
                cli.https_port,
                cli.grpc_https_port,
                Some("test.example.com".to_string()),
                Some("grpc.example.com".to_string()),
            ),
        }
    } else {
        (
            cli.http_port,
            cli.grpc_port,
            cli.tcp_port,
            cli.udp_port,
            cli.https_port,
            cli.grpc_https_port,
            None,
            None,
        )
    };

    let access_log_path =
        std::env::var("EDGION_TEST_ACCESS_LOG_PATH").unwrap_or_else(|_| "logs/edgion_access.log".to_string());

    let test_ctx = framework::TestContext::new(
        cli.target_host.clone(),
        http_port,
        grpc_port,
        cli.websocket_port,
        tcp_port,
        cli.tcp_port + 1, // tcp_filtered_port is by convention tcp+1
        udp_port,
        https_port,
        grpc_https_port,
        cli.admin_port,
        http_host,
        grpc_host,
        cli.gateway,
        cli.verbose,
        PathBuf::from(access_log_path),
    );

    let ctx = dynamic::ScenarioCtx {
        test_ctx,
        conf_root,
        conf_path: scenario.conf_path.to_string(),
        controller_url: format!("http://127.0.0.1:{}", cli.admin_port),
        gateway_url: "http://127.0.0.1:12001".to_string(),
        project_root,
    };

    eprintln!("▶ Running dynamic scenario: {} ({})", scenario.id, scenario.conf_path);
    (scenario.run)(ctx).await?;
    eprintln!("✓ Dynamic scenario {} passed", scenario.id);
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // Install the default rustls crypto provider (once).
    INIT.call_once(|| {
        rustls::crypto::ring::default_provider()
            .install_default()
            .expect("Failed to install rustls crypto provider");
    });

    let cli = Cli::parse();

    if cli.verbose {
        tracing_subscriber::fmt().with_max_level(tracing::Level::DEBUG).init();
    }

    // --list-dynamic-scenarios: print the id list and exit.
    if cli.list_dynamic_scenarios {
        for s in dynamic::all() {
            println!("{}", s.id);
        }
        return Ok(());
    }

    // --dynamic-scenario <id>: run a single scenario and exit.
    if let Some(ref sid) = cli.dynamic_scenario {
        if let Some(capability) = missing_capability("dynamic", sid)? {
            if std::env::var("EDGION_INTEGRATION_EXPLICIT_TARGET").as_deref() != Ok("0") {
                anyhow::bail!("dynamic scenario `{sid}` requires Gateway capability `{capability}`");
            }
            println!("SKIPPED: dynamic scenario `{sid}` requires Gateway capability `{capability}`");
            return Ok(());
        }
        return run_dynamic_scenario(&cli, sid).await;
    }

    // Resolve suite name.
    let suite = resolve_suite(
        cli.resource.as_deref(),
        cli.item.as_deref(),
        cli.legacy_command.as_deref(),
    );

    if let Some(capability) = missing_capability("suite", &suite)? {
        if std::env::var("EDGION_INTEGRATION_EXPLICIT_TARGET").as_deref() != Ok("0") {
            anyhow::bail!("suite `{suite}` requires Gateway capability `{capability}`");
        }
        println!("SKIPPED: suite `{suite}` requires Gateway capability `{capability}`");
        return Ok(());
    }

    // Port config key
    let port_key = suite_to_port_key(&suite);

    // Determine ports and host based on gateway flag
    let (
        http_port,
        grpc_port,
        tcp_port,
        tcp_filtered_port,
        udp_port,
        websocket_port,
        https_port,
        grpc_https_port,
        http_host,
        grpc_host,
    ) = if cli.gateway {
        // Gateway mode: prefer ports from ports.json, but allow CLI ports as fallback/override.
        // This is important for K8s service mode where gateway often listens on 80/443 instead of 31xxx.
        match port_config::PortConfig::load() {
            Ok(config) => {
                let ports = config.get_ports(port_key);
                // Select http_host based on suite
                let http_host = match suite.as_str() {
                    "Gateway/TLS/GatewayTLS" => "gateway-tls.test.com",
                    "Gateway/TLS/NoHostnameListener" => "no-hostname-gateway-tls.test.com",
                    "Gateway/TLS/MisdirectedRequest" => "sni421-exact.test.com",
                    "EdgionTls/port_only" => "port-only.example.com",
                    "EdgionTls/BothAbsentParentRef" => "both-absent-tls.example.com",
                    _ => "test.example.com",
                };
                let http_port = ports.http.unwrap_or(cli.http_port);
                (
                    http_port,
                    ports.grpc.unwrap_or(cli.grpc_port),
                    ports.tcp.unwrap_or(cli.tcp_port),
                    ports.tcp_filtered.unwrap_or(cli.tcp_port + 1),
                    ports.udp.unwrap_or(cli.udp_port),
                    http_port,
                    ports.https.unwrap_or(cli.https_port),
                    ports.grpc_tls.unwrap_or(cli.grpc_https_port),
                    Some(http_host.to_string()),
                    Some("grpc.example.com".to_string()),
                )
            }
            Err(e) => {
                eprintln!("Warning: Failed to load ports.json: {}. Using CLI/default ports.", e);
                let http_host = match suite.as_str() {
                    "Gateway/TLS/GatewayTLS" => "gateway-tls.test.com",
                    "Gateway/TLS/NoHostnameListener" => "no-hostname-gateway-tls.test.com",
                    "Gateway/TLS/MisdirectedRequest" => "sni421-exact.test.com",
                    "EdgionTls/port_only" => "port-only.example.com",
                    "EdgionTls/BothAbsentParentRef" => "both-absent-tls.example.com",
                    _ => "test.example.com",
                };
                (
                    cli.http_port,
                    cli.grpc_port,
                    cli.tcp_port,
                    cli.tcp_port + 1,
                    cli.udp_port,
                    cli.websocket_port,
                    cli.https_port,
                    cli.grpc_https_port,
                    Some(http_host.to_string()),
                    Some("grpc.example.com".to_string()),
                )
            }
        }
    } else {
        // Direct mode: use CLI provided ports
        (
            cli.http_port,
            cli.grpc_port,
            cli.tcp_port,
            cli.tcp_port + 1,
            cli.udp_port,
            cli.websocket_port,
            cli.https_port,
            cli.grpc_https_port,
            None,
            None,
        )
    };

    let mode_name = if cli.gateway { "Gateway" } else { "Direct" };

    println!("\n========================================");
    println!("Edgion ");
    println!("========================================");
    println!("Mode: {}", mode_name);
    println!("Suite: {}", suite);
    println!("Target: {}:{}", cli.target_host, http_port);
    println!("========================================\n");

    // Get access log path from environment variable
    let access_log_path =
        std::env::var("EDGION_TEST_ACCESS_LOG_PATH").unwrap_or_else(|_| "logs/edgion_access.log".to_string());

    let context = TestContext::new(
        cli.target_host.clone(),
        http_port,
        grpc_port,
        websocket_port,
        tcp_port,
        tcp_filtered_port,
        udp_port,
        https_port,
        grpc_https_port,
        cli.admin_port,
        http_host.clone(),
        grpc_host,
        cli.gateway,
        cli.verbose,
        PathBuf::from(access_log_path),
    );

    let mut runner = TestRunner::new(context);

    // Enable per-suite port resolution from ports.json
    if let Ok(port_config) = port_config::PortConfig::load() {
        runner.set_port_resolver(Box::new(move |key: &str| port_config.get_ports(key).http));
    }

    add_suites_for_suite(&mut runner, &suite, cli.gateway, cli.phase.as_deref());

    let start_time = Instant::now();
    let results = runner.run().await;
    let total_duration = start_time.elapsed();

    let console_reporter = ConsoleReporter::new();
    console_reporter.report(&results, total_duration);

    if cli.json {
        let json_reporter = JsonReporter::new();
        json_reporter.save_to_file(&results, total_duration, &cli.json_output)?;
        println!("\n✓ JSON : {}", cli.json_output);
    }

    if results.has_failures() {
        std::process::exit(1);
    }

    Ok(())
}
