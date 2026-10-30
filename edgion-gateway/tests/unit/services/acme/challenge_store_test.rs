use super::*;
use crate::runtime::gateway_info::GatewayInfo;
use crate::runtime::store::listener_gateway_info::ListenerEntry;
use edgion_resources::resources::edgion_acme::ActiveHttpChallenge;
use edgion_resources::resources::edgion_tls::ResolvedListenerAttachment;

fn test_proof() -> GatewayAttachmentProof {
    GatewayAttachmentProof {
        instance: "controller-a".to_string(),
        generation: 1,
        epoch: "epoch-a".to_string(),
        attachment_digest: "a".repeat(64),
    }
}

fn test_listener_key() -> ListenerKey {
    ListenerKey("0.0.0.0:80/HTTP".to_string())
}

fn test_gateway_index(proof: Option<GatewayAttachmentProof>) -> GatewayInfoIndex {
    test_gateway_index_for("gw", "http", None, proof)
}

fn test_gateway_index_for(
    gateway_name: &str,
    listener_name: &str,
    hostname: Option<&str>,
    proof: Option<GatewayAttachmentProof>,
) -> GatewayInfoIndex {
    GatewayInfoIndex::from_entries_for_test(vec![(
        format!("default/{gateway_name}"),
        ListenerEntry {
            port: 80,
            info: Arc::new(GatewayInfo::new(
                Some("default".to_string()),
                gateway_name.to_string(),
                Some(listener_name.to_string()),
                None,
                None,
                None,
                Vec::new(),
                None,
                None,
            )),
            gateway_proof: proof,
            hostname: hostname.map(str::to_string),
            allowed_routes: None,
        },
    )])
}

impl AcmeChallengeStore {
    fn lookup(&self, token: &str, host: &str) -> Option<Bytes> {
        self.lookup_for_listener(
            &test_listener_key(),
            &test_gateway_index(Some(test_proof())),
            token,
            host,
            host,
        )
    }

    fn lookup_at_epoch_seconds(&self, token: &str, host: &str, now: u64) -> Option<Bytes> {
        self.lookup_for_listener_at_epoch_seconds(
            &test_listener_key(),
            &test_gateway_index(Some(test_proof())),
            token,
            host,
            host,
            now,
        )
    }
}

/// Helper: build a minimal EdgionAcme from JSON with optional active challenges.
fn make_acme(challenges: Vec<ActiveHttpChallenge>) -> EdgionAcme {
    let active = serde_json::to_value(&challenges).unwrap();

    crate::test_current_status::from_value(serde_json::json!({
        "apiVersion": "edgion.io/v1",
        "kind": "EdgionAcme",
        "metadata": { "name": "test", "namespace": "default" },
        "spec": {
            "email": "test@example.com",
            "privateKeySecretRef": { "name": "test-acme-account" },
            "domains": ["example.com"],
            "challenge": {
                "type": "http-01",
                "gatewayRef": { "name": "gw" },
            },
            "storage": { "secretName": "test-cert" },
            "resolvedListenerAttachments": [{
                "gatewayNamespace": "default",
                "gatewayName": "gw",
                "listenerName": "http",
                "listenerKey": "0.0.0.0:80/HTTP",
                "gatewayProof": {
                    "instance": "controller-a",
                    "generation": 1,
                    "epoch": "epoch-a",
                    "attachmentDigest": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                }
            }],
        },
        "status": {
            "activeChallenges": active,
        }
    }))
    .unwrap()
}

fn make_acme_for(
    challenges: Vec<ActiveHttpChallenge>,
    gateway_name: &str,
    listener_name: &str,
    listener_key: ListenerKey,
    proof: GatewayAttachmentProof,
) -> EdgionAcme {
    let mut acme = make_acme(challenges);
    acme.spec.resolved_listener_attachments = Some(vec![ResolvedListenerAttachment {
        gateway_namespace: "default".to_string(),
        gateway_name: gateway_name.to_string(),
        listener_name: listener_name.to_string(),
        listener_key,
        gateway_proof: proof,
    }]);
    acme
}

/// Helper: build an ActiveHttpChallenge with the given fields.
fn challenge(domain: &str, token: &str, key_auth: &str, expire_at: u64) -> ActiveHttpChallenge {
    ActiveHttpChallenge {
        domain: domain.to_string(),
        token: token.to_string(),
        key_authorization: key_auth.to_string(),
        expire_at,
    }
}

fn future_time() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() + 300
}

#[test]
fn test_empty_store() {
    let store = AcmeChallengeStore::new();
    assert!(store.is_empty());
    assert!(store.lookup("any-token", "any-host").is_none());
}

#[test]
fn test_lookup_valid_token() {
    let store = AcmeChallengeStore::new();
    let ft = future_time();

    let mut data = HashMap::new();
    data.insert(
        "ns/acme1".to_string(),
        make_acme(vec![challenge(
            "example.com",
            "test-token-123",
            "test-token-123.thumbprint",
            ft,
        )]),
    );
    store.full_set(&data);

    assert!(!store.is_empty());

    // Valid lookup
    let result = store.lookup("test-token-123", "example.com");
    assert_eq!(result, Some(Bytes::from_static(b"test-token-123.thumbprint")));

    // Wrong token
    assert!(store.lookup("wrong-token", "example.com").is_none());

    // Wrong domain
    assert!(store.lookup("test-token-123", "wrong.com").is_none());
}

#[test]
fn test_expired_token() {
    let store = AcmeChallengeStore::new();

    let mut data = HashMap::new();
    data.insert(
        "ns/acme1".to_string(),
        make_acme(vec![challenge(
            "example.com",
            "expired-token",
            "expired-token.thumbprint",
            0,
        )]),
    );
    store.full_set(&data);

    assert!(!store.is_empty());
    assert!(store.lookup("expired-token", "example.com").is_none());
}

#[test]
fn test_expiration_boundary_is_exclusive() {
    let store = AcmeChallengeStore::new();
    let expire_at = 100;

    let mut data = HashMap::new();
    data.insert(
        "ns/acme1".to_string(),
        make_acme(vec![challenge(
            "example.com",
            "boundary-token",
            "boundary-token.thumbprint",
            expire_at,
        )]),
    );
    store.full_set(&data);

    assert_eq!(
        store.lookup_at_epoch_seconds("boundary-token", "example.com", expire_at - 1),
        Some(Bytes::from_static(b"boundary-token.thumbprint"))
    );
    assert!(store
        .lookup_at_epoch_seconds("boundary-token", "example.com", expire_at)
        .is_none());
}

#[test]
fn test_full_set_replaces() {
    let store = AcmeChallengeStore::new();
    let ft = future_time();

    let mut data = HashMap::new();
    data.insert(
        "ns/a".to_string(),
        make_acme(vec![challenge("a.com", "token-a", "auth-a", ft)]),
    );
    store.full_set(&data);
    assert!(store.lookup("token-a", "a.com").is_some());

    // Replace with new set
    let mut data2 = HashMap::new();
    data2.insert(
        "ns/b".to_string(),
        make_acme(vec![challenge("b.com", "token-b", "auth-b", ft)]),
    );
    store.full_set(&data2);

    // Old token gone
    assert!(store.lookup("token-a", "a.com").is_none());
    // New token available
    assert!(store.lookup("token-b", "b.com").is_some());
}

#[test]
fn test_empty_host_no_longer_matches() {
    let store = AcmeChallengeStore::new();
    let ft = future_time();

    let mut data = HashMap::new();
    data.insert(
        "ns/acme1".to_string(),
        make_acme(vec![challenge("example.com", "token-1", "auth-1", ft)]),
    );
    store.full_set(&data);

    // An empty/unparseable Host must NOT match: it can no longer bypass the
    // per-domain binding the domain check enforces. The token still exists
    // and is unexpired, so this exercises the new `empty_host` miss path.
    assert!(store.lookup("token-1", "").is_none());
}

#[test]
fn test_lookup_case_insensitive_domain() {
    let store = AcmeChallengeStore::new();
    let ft = future_time();

    let mut data = HashMap::new();
    data.insert(
        "ns/acme1".to_string(),
        make_acme(vec![challenge("Example.COM", "token-1", "auth-1", ft)]),
    );
    store.full_set(&data);

    // Stored domain is mixed-case; request host in lower-case must hit
    // (RFC 4343: DNS/Host comparisons are case-insensitive).
    assert_eq!(
        store.lookup("token-1", "example.com"),
        Some(Bytes::from_static(b"auth-1"))
    );
    // Request host in a different mixed-case must also hit.
    assert_eq!(
        store.lookup("token-1", "EXAMPLE.com"),
        Some(Bytes::from_static(b"auth-1"))
    );
    // Negative guard: a genuinely different domain still misses
    // (case-insensitivity must not collapse into "match any").
    assert!(store.lookup("token-1", "other.com").is_none());
}

#[test]
fn test_partial_update_add_and_remove() {
    let store = AcmeChallengeStore::new();
    let ft = future_time();

    // Start with two resources
    let mut data = HashMap::new();
    data.insert(
        "ns/a".to_string(),
        make_acme(vec![challenge("a.com", "token-a", "auth-a", ft)]),
    );
    data.insert(
        "ns/b".to_string(),
        make_acme(vec![challenge("b.com", "token-b", "auth-b", ft)]),
    );
    store.full_set(&data);

    assert!(store.lookup("token-a", "a.com").is_some());
    assert!(store.lookup("token-b", "b.com").is_some());

    // Partial update: remove resource "ns/a", add resource "ns/c"
    let mut upsert = HashMap::new();
    upsert.insert(
        "ns/c".to_string(),
        make_acme(vec![challenge("c.com", "token-c", "auth-c", ft)]),
    );
    let mut remove = HashSet::new();
    remove.insert("ns/a".to_string());
    store.partial_update(&upsert, &remove);

    // "a" should be gone, "b" untouched, "c" added
    assert!(store.lookup("token-a", "a.com").is_none());
    assert!(store.lookup("token-b", "b.com").is_some());
    assert!(store.lookup("token-c", "c.com").is_some());
}

#[test]
fn test_partial_update_replaces_resource_tokens() {
    let store = AcmeChallengeStore::new();
    let ft = future_time();

    let mut data = HashMap::new();
    data.insert(
        "ns/a".to_string(),
        make_acme(vec![challenge("a.com", "token-old", "auth-old", ft)]),
    );
    store.full_set(&data);

    assert!(store.lookup("token-old", "a.com").is_some());

    let mut replacement = HashMap::new();
    replacement.insert(
        "ns/a".to_string(),
        make_acme(vec![challenge("a.com", "token-new", "auth-new", ft)]),
    );
    store.partial_update(&replacement, &HashSet::new());
    assert!(store.lookup("token-old", "a.com").is_none());
    assert_eq!(
        store.lookup("token-new", "a.com"),
        Some(Bytes::from_static(b"auth-new"))
    );
}

#[test]
fn partial_update_drops_token_when_attachments_or_challenges_are_revoked() {
    let store = AcmeChallengeStore::new();
    let listener_key = test_listener_key();
    let index = test_gateway_index(Some(test_proof()));
    let ft = future_time();
    let key = "default/acme".to_string();
    let serving = make_acme(vec![challenge(
        "example.com",
        "ownership-token",
        "ownership-token.authorization",
        ft,
    )]);
    store.partial_update(&HashMap::from([(key.clone(), serving.clone())]), &HashSet::new());
    assert_eq!(
        store.lookup_for_listener(&listener_key, &index, "ownership-token", "example.com", "example.com",),
        Some(Bytes::from_static(b"ownership-token.authorization"))
    );

    let mut without_attachments = serving.clone();
    let mut without_current = serving.clone();
    without_current.spec.current_status = None;
    store.partial_update(&HashMap::from([(key.clone(), without_current)]), &HashSet::new());
    assert!(
        store
            .lookup_for_listener(&listener_key, &index, "ownership-token", "example.com", "example.com")
            .is_none(),
        "full status challenges cannot replace a missing current projection"
    );
    store.partial_update(&HashMap::from([(key.clone(), serving.clone())]), &HashSet::new());
    without_attachments.spec.resolved_listener_attachments = None;
    store.partial_update(&HashMap::from([(key.clone(), without_attachments)]), &HashSet::new());
    assert!(store
        .lookup_for_listener(&listener_key, &index, "ownership-token", "example.com", "example.com",)
        .is_none());

    store.partial_update(&HashMap::from([(key.clone(), serving.clone())]), &HashSet::new());
    assert!(store
        .lookup_for_listener(&listener_key, &index, "ownership-token", "example.com", "example.com",)
        .is_some());

    let mut without_challenges = serving;
    without_challenges
        .spec
        .current_status
        .as_mut()
        .expect("make_acme writes status")
        .active_challenges
        .clear();
    store.partial_update(&HashMap::from([(key, without_challenges)]), &HashSet::new());
    assert!(store
        .lookup_for_listener(&listener_key, &index, "ownership-token", "example.com", "example.com",)
        .is_none());
}

#[test]
fn test_partial_update_after_full_set_uses_latest_resource_index() {
    let store = AcmeChallengeStore::new();
    let ft = future_time();

    let mut data_a = HashMap::new();
    data_a.insert(
        "ns/a".to_string(),
        make_acme(vec![challenge("a.com", "token-a", "auth-a", ft)]),
    );
    store.full_set(&data_a);

    let mut data_b = HashMap::new();
    data_b.insert(
        "ns/b".to_string(),
        make_acme(vec![challenge("b.com", "token-b", "auth-b", ft)]),
    );
    store.full_set(&data_b);

    assert!(store.lookup("token-a", "a.com").is_none());
    assert_eq!(store.lookup("token-b", "b.com"), Some(Bytes::from_static(b"auth-b")));

    let mut remove = HashSet::new();
    remove.insert("ns/b".to_string());
    store.partial_update(&HashMap::new(), &remove);

    assert!(store.is_empty());
    assert!(store.lookup("token-a", "a.com").is_none());
    assert!(store.lookup("token-b", "b.com").is_none());
}

#[test]
fn test_concurrent_reads_during_snapshot_updates() {
    use std::sync::mpsc;
    use std::sync::Arc as StdArc;
    use std::thread;
    use std::time::Duration;

    let store = StdArc::new(AcmeChallengeStore::new());
    let ft = future_time();
    let (published_tx, published_rx) = mpsc::channel::<usize>();
    let (ack_tx, ack_rx) = mpsc::channel::<()>();

    let reader_store = StdArc::clone(&store);
    let reader = thread::spawn(move || {
        let mut reads = 0;

        while let Ok(i) = published_rx.recv() {
            assert!(!reader_store.is_empty());
            if i % 2 == 0 {
                assert_eq!(
                    reader_store.lookup("token-a", "a.com"),
                    Some(Bytes::from_static(b"auth-a"))
                );
                assert!(reader_store.lookup("token-b", "b.com").is_none());
            } else {
                assert_eq!(
                    reader_store.lookup("token-b", "b.com"),
                    Some(Bytes::from_static(b"auth-b"))
                );
                assert!(reader_store.lookup("token-a", "a.com").is_none());
            }

            reads += 1;
            ack_tx.send(()).unwrap();
        }

        reads
    });

    for i in 0..500 {
        let mut data = HashMap::new();
        let token = if i % 2 == 0 { "token-a" } else { "token-b" };
        let domain = if i % 2 == 0 { "a.com" } else { "b.com" };
        let auth = if i % 2 == 0 { "auth-a" } else { "auth-b" };
        data.insert(
            format!("ns/{}", token),
            make_acme(vec![challenge(domain, token, auth, ft)]),
        );
        store.full_set(&data);
        published_tx.send(i).unwrap();
        ack_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    }

    drop(published_tx);
    let reads = reader.join().unwrap();

    assert_eq!(reads, 500);
    assert!(store.lookup("token-a", "a.com").is_none());
    assert_eq!(store.lookup("token-b", "b.com"), Some(Bytes::from_static(b"auth-b")));
}

#[test]
fn lookup_requires_physical_logical_proof_and_hostname_ownership() {
    let store = AcmeChallengeStore::new();
    let listener_key = test_listener_key();
    let proof = test_proof();
    let mut data = HashMap::new();
    data.insert(
        "default/acme".to_string(),
        make_acme_for(
            vec![challenge("api.example.com", "owned", "authorization", future_time())],
            "gw",
            "http",
            listener_key.clone(),
            proof.clone(),
        ),
    );
    store.full_set(&data);

    let matching = test_gateway_index_for("gw", "http", Some("*.example.com"), Some(proof.clone()));
    assert_eq!(
        store.lookup_for_listener(&listener_key, &matching, "owned", "API.Example.com", "api.example.com",),
        Some(Bytes::from_static(b"authorization"))
    );
    assert!(store
        .lookup_for_listener(
            &listener_key,
            &matching,
            "owned",
            "api.example.com",
            "other.example.com",
        )
        .is_none());

    let wrong_gateway = test_gateway_index_for("other", "http", None, Some(proof.clone()));
    assert!(store
        .lookup_for_listener(
            &listener_key,
            &wrong_gateway,
            "owned",
            "api.example.com",
            "api.example.com",
        )
        .is_none());
    let wrong_listener = test_gateway_index_for("gw", "other", None, Some(proof.clone()));
    assert!(store
        .lookup_for_listener(
            &listener_key,
            &wrong_listener,
            "owned",
            "api.example.com",
            "api.example.com",
        )
        .is_none());
    let stale = test_gateway_index_for(
        "gw",
        "http",
        None,
        Some(GatewayAttachmentProof {
            generation: 2,
            ..proof.clone()
        }),
    );
    assert!(store
        .lookup_for_listener(&listener_key, &stale, "owned", "api.example.com", "api.example.com",)
        .is_none());
    let wrong_hostname = test_gateway_index_for("gw", "http", Some("other.example.com"), Some(proof));
    assert!(store
        .lookup_for_listener(
            &listener_key,
            &wrong_hostname,
            "owned",
            "api.example.com",
            "api.example.com",
        )
        .is_none());
    let wrong_physical = ListenerKey("127.0.0.1:80/HTTP".to_string());
    assert!(store
        .lookup_for_listener(
            &wrong_physical,
            &matching,
            "owned",
            "api.example.com",
            "api.example.com",
        )
        .is_none());
}

#[test]
fn conflicting_authorizations_fail_closed_but_identical_authorizations_coexist() {
    let store = AcmeChallengeStore::new();
    let listener_key = test_listener_key();
    let index = test_gateway_index(Some(test_proof()));
    let ft = future_time();
    let mut data = HashMap::new();
    for (resource, authorization) in [("a", "same"), ("b", "same")] {
        data.insert(
            format!("default/{resource}"),
            make_acme(vec![challenge("example.com", "collision", authorization, ft)]),
        );
    }
    store.full_set(&data);
    assert_eq!(
        store.lookup_for_listener(&listener_key, &index, "collision", "example.com", "example.com",),
        Some(Bytes::from_static(b"same"))
    );

    data.insert(
        "default/b".to_string(),
        make_acme(vec![challenge("example.com", "collision", "different", ft)]),
    );
    store.full_set(&data);
    assert!(store
        .lookup_for_listener(&listener_key, &index, "collision", "example.com", "example.com",)
        .is_none());
}

#[test]
fn token_candidate_overflow_fails_closed_and_partial_removal_recovers() {
    let store = AcmeChallengeStore::new();
    let listener_key = test_listener_key();
    let index = test_gateway_index(Some(test_proof()));
    let ft = future_time();
    let mut data = HashMap::new();
    for i in 0..MAX_ACME_TOKEN_CANDIDATES {
        data.insert(
            format!("default/acme-{i}"),
            make_acme(vec![challenge("example.com", "shared", "same", ft)]),
        );
    }
    store.full_set(&data);
    assert_eq!(
        store.lookup_for_listener(&listener_key, &index, "shared", "example.com", "example.com",),
        Some(Bytes::from_static(b"same"))
    );

    let overflow_key = format!("default/acme-{}", MAX_ACME_TOKEN_CANDIDATES);
    let mut upsert = HashMap::new();
    upsert.insert(
        overflow_key.clone(),
        make_acme(vec![challenge("example.com", "shared", "same", ft)]),
    );
    store.partial_update(&upsert, &HashSet::new());
    assert!(store
        .lookup_for_listener(&listener_key, &index, "shared", "example.com", "example.com",)
        .is_none());

    let mut remove = HashSet::new();
    remove.insert(overflow_key);
    store.partial_update(&HashMap::new(), &remove);
    assert_eq!(
        store.lookup_for_listener(&listener_key, &index, "shared", "example.com", "example.com",),
        Some(Bytes::from_static(b"same"))
    );
}

#[test]
fn forged_oversized_attachment_vector_is_not_published() {
    let store = AcmeChallengeStore::new();
    let mut resource = make_acme(vec![challenge(
        "example.com",
        "oversized",
        "authorization",
        future_time(),
    )]);
    let attachment = resource.spec.resolved_listener_attachments.as_ref().unwrap()[0].clone();
    resource.spec.resolved_listener_attachments = Some(vec![attachment; MAX_ACME_RESOLVED_ATTACHMENTS + 1]);
    store.full_set(&HashMap::from([("default/oversized".to_string(), resource)]));
    assert!(store.lookup("oversized", "example.com").is_none());
}

#[test]
fn full_and_partial_updates_build_the_same_canonical_buckets() {
    let ft = future_time();
    let data = HashMap::from([
        (
            "default/a".to_string(),
            make_acme(vec![challenge("a.example.com", "token", "same", ft)]),
        ),
        (
            "default/b".to_string(),
            make_acme(vec![challenge("b.example.com", "token", "same", ft)]),
        ),
    ]);
    let full = AcmeChallengeStore::new();
    full.full_set(&data);

    let partial = AcmeChallengeStore::new();
    for (key, acme) in &data {
        partial.partial_update(&HashMap::from([(key.clone(), acme.clone())]), &HashSet::new());
    }

    assert_eq!(**full.tokens.load(), **partial.tokens.load());
}
