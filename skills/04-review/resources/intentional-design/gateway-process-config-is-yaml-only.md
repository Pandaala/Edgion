---
name: gateway-process-config-is-yaml-only
description: Historical review evidence for Config / schema; revalidate against current code before applying.
knowledge_type: intentional-design
---

# Gateway process config is yaml only

## Recorded conclusion

Gateway process config is **YAML-only** (`EdgionGatewayConfig::load`, `edgion-gateway/src/cli/config.rs`, parses via `serde_yaml` `singleton_map_recursive`; default path `config/edgion-gateway.yaml`) but was mislabeled "TOML" in many places. Fixed in `config.rs`: the clap `--config-file` help (`(TOML format)`→`(YAML format)`), the `load()` doc, and 8 in-code `///` examples (`access_log_sensitive`, `access_log_options`, `path_normalization`, `metrics` / `metrics.request_duration` / `metrics.backend_requests`, `rate_limit`, `lru_cache`) rewritten TOML→YAML; the 3 `access_log_sensitive` unit tests switched from `toml::from_str` to the loader's `serde_yaml` path via a `parse_yaml_config` helper. The same "Gateway TOML" / TOML-syntax mislabel was corrected repo-wide (both the Controller and Gateway process configs are YAML — both `EdgionControllerConfig::load` and `EdgionGatewayConfig::load` parse via `serde_yaml`; there is no TOML runtime config anywhere): other `src/` doc comments (`types/observe.rs`, `gateway/observe/access_log/entry.rs`, `gateway/path_normalization/config.rs` example block, `gateway/plugins/http/all_endpoint_status/plugin.rs`); user docs (`ops-guide/README.md`, `ops-guide/configuration/{README,gateway-config,edgion-gateway-config}.md` incl. converting `### [section]` headers + `{ Size: N }` flow form, `path-normalization.md`, `user-guide/edgion-plugins/rate-limit-local.md`, `dev-guide/logging-system.md`; en + zh-CN); skills (`SKILL.md`, `02-features/{,02-config/}SKILL.md`, `02-config/{00-controller-config(gateway-side path_normalization header),01-gateway-config,02-edgion-gateway-config}.md`, `02-features/04-observability/{00-logging,01-metrics}.md`, `01-architecture/02-gateway/{00-overview,01-startup-shutdown,uri-normalization/SKILL}.md`, `03-coding/{01-log-safety,observability/00-access-log}.md`, `08-kubernetes/01-gateway-api/01-not-implemented.md`). **Correctness note:** `AccessLogOptions` is `#[serde(rename_all = "camelCase")]`, so the YAML key is `includeRawUri` (not `include_raw_uri`) — every example was corrected. A follow-up pass (2026-06-09) corrected the remaining mislabels that this finding had wrongly left as "Controller TOML": the Controller config docs/schema, the `[local_auth]`/`[auth]`/`[conf_center]` bracket references, the etcd ConfCenter doc comments, and the unused `toml` crate dependency (removed from `Cargo.toml`; its only uses were unit tests, now switched to `serde_yaml`). NOT changed (genuinely TOML/Rust): Cargo `[features]` (`03-ssl-backends.md`), Cargo manifest snippets, `deny.toml`, and `rust-toolchain.toml`. Re-reports that the gateway OR controller config is/should be TOML, that the in-code examples fail to parse, or that `include_raw_uri` is the YAML key, are not accepted — verify `config.rs::load` / `EdgionControllerConfig::load` use `serde_yaml` and `AccessLogOptions` casing before re-reporting.

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

## Resolution references

- `types-misc-02` — 2026-05-25.
