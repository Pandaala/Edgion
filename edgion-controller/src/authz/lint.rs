//! Controller-side policy-lint pass for cli-token / Center RBAC config.
//!
//! These checks are advisory WARNINGs (never fatal): the enforcement path is
//! already fail-closed (an unmatched verb/kind simply grants nothing). The goal
//! is to surface operator typos that would otherwise become invisible
//! zero-permission rules. Shared by the startup loader and the reload path.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::authz::{CenterPolicy, CliTokenEntry, Verb};
use crate::config::{CliTokenConfig, EdgionControllerConfig, FedRbacConfig};
use edgion_resources::resource::kind::ResourceKind;

/// Synthetic classifier kinds that are valid in a policy but are NOT `ResourceKind`s.
///
/// - `RegionRoute`: the aggregate region-route read/failover endpoints.
/// - `NonResource`: non-resource operational endpoints (server-info, diagnostics,
///   reload, ...) — see `api::authz_classifier::NON_RESOURCE_KIND`. Having a named
///   kind lets policies scope these without resorting to `kinds: ["*"]`.
const SYNTHETIC_KINDS: &[&str] = &["RegionRoute", "NonResource"];

/// Lint one policy's verbs/kinds and emit advisory WARNs.
///
/// `identity` is the cli-token name or "center".
/// `context` is "startup" or "reload" (for log correlation).
pub fn lint_rbac_policy(rbac: &FedRbacConfig, context: &str, identity: &str) {
    for rule in &rbac.allow {
        for k in &rule.kinds {
            if k != "*" && !SYNTHETIC_KINDS.contains(&k.as_str()) {
                match ResourceKind::from_kind_name(k) {
                    None => {
                        tracing::warn!(
                            component = "rbac_lint",
                            context = context,
                            identity = identity,
                            kind = %k,
                            "rbac rule names an unknown kind; it will match nothing (dead rule) \
                             — check spelling against the canonical ResourceKind"
                        );
                    }
                    Some(rk) if rk.as_str() != k.as_str() => {
                        // The kind is recognizable but not in canonical PascalCase.
                        // The policy stores kinds verbatim and matches against canonical
                        // kind strings at request time, so non-canonical casing produces
                        // a dead rule.
                        tracing::warn!(
                            component = "rbac_lint",
                            context = context,
                            identity = identity,
                            kind = %k,
                            canonical = rk.as_str(),
                            "rbac rule names a non-canonical kind; it will match nothing (dead rule) \
                             — use the canonical spelling shown above"
                        );
                    }
                    Some(_) => {} // canonical and known — OK
                }
            }
        }
        for v in &rule.verbs {
            if v != "*" && Verb::from_str(&v.to_ascii_lowercase()).is_none() {
                tracing::warn!(
                    component = "rbac_lint",
                    context = context,
                    identity = identity,
                    verb = %v,
                    "rbac rule names an unknown verb; it will match nothing (dead rule)"
                );
            }
        }
        // Dangerous-grant WARNs — folded here so both startup AND reload emit them.
        // `watch` is included alongside the point-read verbs: it streams the same
        // credential data and goes through the identical `authorize()` gate, so a
        // `watch Secret` / `watch *` grant is just as dangerous as a `get`/`list`.
        let has_read_verb = rule
            .verbs
            .iter()
            .any(|v| matches!(v.to_ascii_lowercase().as_str(), "get" | "list" | "list-keys" | "watch"));
        if has_read_verb && rule.kinds.iter().any(|k| k == "Secret") {
            tracing::warn!(
                component = "rbac_lint",
                context = context,
                identity = identity,
                "rbac rule grants read/watch access to Secret; this exposes credential data \
                 — review the policy"
            );
        }
        if has_read_verb && rule.kinds.iter().any(|k| k == "*") {
            tracing::warn!(
                component = "rbac_lint",
                context = context,
                identity = identity,
                "rbac rule uses kinds:[\"*\"] with a read/watch verb; this may expose Secret data \
                 via wildcard — review the policy"
            );
        }
    }
}

/// Build the cli-token lookup map (key = `token_sha256`) from config, applying
/// per-entry shape validation, GLOBAL dedup (no silent last-wins), and the lint
/// pass. Invalid / duplicate entries are dropped with a WARN (non-fatal).
pub fn build_cli_token_map(cli_tokens: &[CliTokenConfig], context: &str) -> HashMap<String, CliTokenEntry> {
    let mut map = HashMap::new();
    let mut seen_names: HashSet<&str> = HashSet::new();
    let mut seen_hashes: HashSet<&str> = HashSet::new();

    for entry in cli_tokens {
        // Per-entry shape validation: charset / 64-lowercase-hex / rbac shape.
        // Wrap in a single-element config so we reuse the shared validator rather
        // than re-implementing the checks inline.
        let single = EdgionControllerConfig {
            cli_tokens: vec![entry.clone()],
            ..Default::default()
        };
        if let Err(e) = single.validate_cli_tokens() {
            tracing::warn!(
                component = "cli_tokens",
                context = context,
                name = %entry.name,
                error = %e,
                "dropping invalid cli_token entry (not fatal)"
            );
            continue;
        }

        // GLOBAL dedup: skip (do not overwrite) on duplicate name.
        if !seen_names.insert(entry.name.as_str()) {
            tracing::warn!(
                component = "cli_tokens",
                context = context,
                name = %entry.name,
                "duplicate cli_token name; skipping this entry (first occurrence kept)"
            );
            continue;
        }

        // GLOBAL dedup: skip (do not overwrite) on duplicate hash.
        if !seen_hashes.insert(entry.token_sha256.as_str()) {
            tracing::warn!(
                component = "cli_tokens",
                context = context,
                name = %entry.name,
                "duplicate cli_token token_sha256; skipping this entry \
                 (first occurrence kept, no overwrite)"
            );
            continue;
        }

        lint_rbac_policy(&entry.rbac, context, &entry.name);

        let policy = CenterPolicy::from_config(&entry.rbac);
        map.insert(
            entry.token_sha256.clone(),
            CliTokenEntry {
                name: entry.name.clone(),
                policy: Arc::new(policy),
            },
        );
    }

    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FedAllowRule, FedRbacConfig};

    fn valid_rbac() -> FedRbacConfig {
        FedRbacConfig {
            allow: vec![FedAllowRule {
                verbs: vec!["get".to_string()],
                kinds: vec!["EdgionConfigData".to_string()],
            }],
        }
    }

    fn make_entry(name: &str, hash: &str) -> CliTokenConfig {
        CliTokenConfig {
            name: name.to_string(),
            token_sha256: hash.to_string(),
            rbac: valid_rbac(),
        }
    }

    #[test]
    fn dup_hash_skipped_not_overwritten() {
        // Two entries with the same token_sha256 but different names and different
        // policies. The map must contain only the FIRST entry (no last-wins overwrite).
        let hash = "a".repeat(64);
        let entry1 = CliTokenConfig {
            name: "first".to_string(),
            token_sha256: hash.clone(),
            rbac: FedRbacConfig {
                allow: vec![FedAllowRule {
                    verbs: vec!["get".to_string()],
                    kinds: vec!["EdgionConfigData".to_string()],
                }],
            },
        };
        let entry2 = CliTokenConfig {
            name: "second".to_string(),
            token_sha256: hash.clone(),
            rbac: FedRbacConfig {
                allow: vec![FedAllowRule {
                    verbs: vec!["delete".to_string()],
                    kinds: vec!["Secret".to_string()],
                }],
            },
        };

        let map = build_cli_token_map(&[entry1, entry2], "test");

        assert_eq!(map.len(), 1, "only the first entry should be retained");
        let retained = map.get(&hash).expect("first entry must be present");
        assert_eq!(
            retained.name, "first",
            "the FIRST entry must be kept (no last-wins overwrite)"
        );

        // The retained policy must be the first one (get/EdgionConfigData).
        let az = crate::authz::Authorizer::new(CenterPolicy::deny_all());
        let role = crate::authz::Role::Restricted(retained.policy.clone());
        assert!(
            az.authorize(&role, Verb::Get, "EdgionConfigData").allowed(),
            "first entry policy (get EdgionConfigData) must be active"
        );
        assert!(
            !az.authorize(&role, Verb::Delete, "Secret").allowed(),
            "second entry policy (delete Secret) must NOT be active"
        );
    }

    #[test]
    fn dup_name_skipped() {
        // Two entries with the same name but different valid hashes.
        // Only the first should be kept.
        let entry1 = make_entry("bot", &"a".repeat(64));
        let entry2 = make_entry("bot", &"b".repeat(64));

        let map = build_cli_token_map(&[entry1, entry2], "test");

        assert_eq!(map.len(), 1, "only the first named entry should be retained");
        assert!(
            map.contains_key(&"a".repeat(64)),
            "the first entry (hash=aaa...) must be present"
        );
        assert!(
            !map.contains_key(&"b".repeat(64)),
            "the second entry (hash=bbb...) must be absent"
        );
    }

    #[test]
    fn invalid_entry_dropped() {
        // One entry with a bad token_sha256 (not 64 hex chars) is dropped.
        // A valid sibling must survive.
        let bad = CliTokenConfig {
            name: "bad-bot".to_string(),
            token_sha256: "notahexhash".to_string(),
            rbac: valid_rbac(),
        };
        let good = make_entry("good-bot", &"c".repeat(64));

        let map = build_cli_token_map(&[bad, good], "test");

        assert_eq!(map.len(), 1, "only valid entry should be in the map");
        assert!(map.contains_key(&"c".repeat(64)), "valid entry must be present");
    }

    #[test]
    fn lint_runs_without_panic_on_noncanonical_kind_and_unknown_verb() {
        // Exercises the WARN paths for a non-canonical kind ("httproute" instead of
        // "HTTPRoute") and an unknown verb ("geet"). Neither must panic; the WARNs
        // are side effects only (no assertion on log output here).
        let rbac = FedRbacConfig {
            allow: vec![FedAllowRule {
                verbs: vec!["geet".to_string()],
                kinds: vec!["httproute".to_string()],
            }],
        };
        // Must not panic.
        lint_rbac_policy(&rbac, "test", "test-identity");
    }

    /// In-memory `MakeWriter` so a test can capture the advisory WARN output of
    /// `lint_rbac_policy` and assert that the dangerous-grant warning fired.
    #[derive(Clone, Default)]
    struct BufWriter(std::sync::Arc<parking_lot::Mutex<Vec<u8>>>);

    impl std::io::Write for BufWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for BufWriter {
        type Writer = BufWriter;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Run `lint_rbac_policy` against `rbac` with a thread-local WARN-capturing
    /// subscriber and return everything it logged as a single string.
    fn capture_lint_warns(rbac: &FedRbacConfig) -> String {
        let buf = BufWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buf.clone())
            .with_max_level(tracing::Level::WARN)
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            lint_rbac_policy(rbac, "test", "test-identity");
        });
        let bytes = buf.0.lock().clone();
        String::from_utf8(bytes).expect("captured log is valid UTF-8")
    }

    #[test]
    fn watch_secret_emits_dangerous_grant_warn() {
        // Regression for m1-01: a `watch Secret` grant must trip the same
        // dangerous-grant advisory as `get`/`list`/`list-keys`.
        let rbac = FedRbacConfig {
            allow: vec![FedAllowRule {
                verbs: vec!["watch".to_string()],
                kinds: vec!["Secret".to_string()],
            }],
        };
        let captured = capture_lint_warns(&rbac);
        assert!(
            captured.contains("read/watch access to Secret"),
            "watch Secret must emit the Secret dangerous-grant warn; captured: {captured}"
        );
    }

    #[test]
    fn watch_wildcard_emits_dangerous_grant_warn() {
        // Regression for m1-01: a `watch *` grant must trip the wildcard
        // dangerous-grant advisory, not just point-read verbs.
        let rbac = FedRbacConfig {
            allow: vec![FedAllowRule {
                verbs: vec!["watch".to_string()],
                kinds: vec!["*".to_string()],
            }],
        };
        let captured = capture_lint_warns(&rbac);
        assert!(
            captured.contains("kinds:[\"*\"] with a read/watch verb"),
            "watch * must emit the wildcard dangerous-grant warn; captured: {captured}"
        );
    }
}
