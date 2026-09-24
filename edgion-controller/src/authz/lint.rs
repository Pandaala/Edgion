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
/// - `RegionRoute`: the effective region-route read endpoint and the failover endpoint.
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
#[path = "../../tests/unit/authz/lint_test.rs"]
mod tests;
