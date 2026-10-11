---
id: ARCH-044
status: candidate
kind: behavior-defect
priority: P2
primary_partition: PART-259
independent_verification: confirmed
---

# Preserve the declared Pingora process CLI overrides during configuration merge

## Actual ownership and witness

`cli/config.rs::ServerConfig` declares six ordinary process CLI fields: `threads`,
`work_stealing`, `grace_period_seconds`, `graceful_shutdown_timeout_seconds`,
`upstream_keepalive_pool_size`, and `error_log` (lines 576-630). The Gateway CLI
flattens this structure. `EdgionGatewayCli::run` passes the parsed configuration
to `EdgionGatewayConfig::load`; that function always starts from the YAML file
or a fresh default and invokes `merge` (1152-1176).

`merge` (1178-1237) copies the three H2 and two TLS offload fields but none of
these six process options. `cli/pingora.rs::create_server_conf` correctly consumes
the merged `ServerConfig`; it does not read the original CLI. The original CLI
remaining in `run_after_logging` is used for the separately gated integration
flag, not as another source of Pingora process settings.

For example, a valid Gateway configuration with `server.threads: 4` and an
explicit `--threads 1` still constructs `ServerConf.threads=4`. With an omitted
YAML threads field, the same CLI flag is lost and platform parallelism wins.
The other five declared options suffer the same omission. This is source-proven
wiring loss, not a freshly executed CLI or server experiment.

## Bounded correction and architectural value

Complete the existing typed merge for these six `Option` fields: a present CLI
value wins, and absence retains the YAML value. Keep strings owned only at this
cold configuration boundary. A private ServerConfig merge helper could own the
entire server field list if useful; a generic reflection/registration framework,
second configuration schema or remote bootstrap policy is unnecessary.

The configuration declaration, precedence and actual ServerConf consumer should
be reviewed together so adding a process flag cannot create another silently
inert option. Removing advertised CLI flags is an alternative product decision;
current code explicitly provides and documents them, and no retrieved decision
accepts dropping their values. Existing local-YAML ownership means restart-bound
process policy, not permission to ignore declared CLI overrides.

## Required invariants and future verification

Preserve YAML-only class-independent process ownership; do not add these fields
to the synced EGC CRD. Keep H2 merged validation and TLS offload pair/aggregate
bounds, zero/omitted thread platform fallback, retry-count translation and existing
ServerConf defaults. Preserve FIPS require OR semantics and tri-state precedence,
unknown-field advisory/strict subtree behavior, secure conf-sync requirements,
work-directory resolution and binary-owned startup diagnostics. No hot-path
allocation, new logging/metrics or compatibility layer is required.

Future tests must drive real Clap parsing plus production load/merge, then the
actual ServerConf constructor, for every affected flag with a conflicting YAML
value and when omitted. Cover an explicit false `work_stealing`, legal zero values
according to existing constructor semantics, and the String path. Retain the
existing H2/offload, FIPS and unknown-field tests. Constructor-only tests such as
`positive_thread_count_is_preserved` set the final struct directly and do not
exercise this broken boundary.

## Evidence and limitations

All three CLI primary implementations were personally read in full. Selected
actual load/merge tests, full ServerConf tests, startup guard/diagnostic tests,
original startup architecture, process-YAML ownership and advisory-unknown-field
decisions were inspected. Focused searches across original review knowledge and
pending tasks found no decision accepting these six dropped overrides. Broader
startup-guide hydration wording is a documentation follow-up, separate from this
cause. Existing ARCH040/041 concern physical listener policy, not CLI merge.

Static review only. No tests, builds, CLI execution, network calls, production
changes, Git mutations or measurements were performed. Independent verification
is pending.
