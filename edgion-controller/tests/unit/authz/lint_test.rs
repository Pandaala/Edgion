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
