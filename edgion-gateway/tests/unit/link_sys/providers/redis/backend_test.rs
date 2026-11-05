use super::*;

fn slots(host: Value, port: i64) -> Value {
    Value::Array(vec![Value::Array(vec![
        Value::Int(0),
        Value::Int(16383),
        Value::Array(vec![host, Value::Int(port)]),
    ])])
}
#[test]
fn cluster_missing_endpoint_uses_the_actual_responder() {
    for host in [
        Value::Nil,
        Value::BulkString(Vec::new()),
        Value::BulkString(b"0.0.0.0".to_vec()),
        Value::BulkString(b"::".to_vec()),
    ] {
        assert_eq!(
            parse_primaries(slots(host, 6379), "seed.example").unwrap(),
            vec![("seed.example".into(), 6379)]
        );
    }
}
#[test]
fn cluster_rejects_unknown_or_invalid_addresses_and_overlapping_ranges() {
    for (host, port) in [("?", 6379), ("redis://bad", 6379), ("valid", 0), ("valid", 65536)] {
        assert!(parse_primaries(slots(Value::BulkString(host.as_bytes().to_vec()), port), "seed.example").is_err());
    }
    let Value::Array(mut ranges) = slots(Value::BulkString(b"node".to_vec()), 6379) else {
        unreachable!()
    };
    ranges.push(ranges[0].clone());
    assert!(parse_primaries(Value::Array(ranges), "seed.example").is_err());
}

fn slot_range(start: i64, end: i64, nodes: &[(&str, i64)]) -> Value {
    let mut fields = vec![Value::Int(start), Value::Int(end)];
    for (host, port) in nodes {
        fields.push(Value::Array(vec![
            Value::BulkString(host.as_bytes().to_vec()),
            Value::Int(*port),
        ]));
    }
    Value::Array(fields)
}

#[test]
fn cluster_unknown_hostname_skips_that_node_without_failing_the_map() {
    let mixed = Value::Array(vec![
        slot_range(0, 100, &[("?", 6379)]),
        slot_range(101, 200, &[("real.example", 6380)]),
    ]);
    assert_eq!(
        parse_primaries(mixed, "seed.example").unwrap(),
        vec![("real.example".into(), 6380)]
    );
    let promoted = Value::Array(vec![slot_range(0, 16383, &[("?", 6379), ("replica.example", 6381)])]);
    assert_eq!(
        parse_primaries(promoted, "seed.example").unwrap(),
        vec![("replica.example".into(), 6381)]
    );
    // A skipped unknown range does not occupy slots, so a later real range may cover them.
    let overlap = Value::Array(vec![
        slot_range(0, 100, &[("?", 6379)]),
        slot_range(0, 100, &[("real.example", 6380)]),
    ]);
    assert_eq!(
        parse_primaries(overlap, "seed.example").unwrap(),
        vec![("real.example".into(), 6380)]
    );
}
#[test]
fn cluster_server_error_becomes_an_error_and_moved_stays_a_redirect() {
    let noscript = redis::parse_redis_value(b"-NOSCRIPT No matching script. Please use EVAL.\r\n").unwrap();
    let error = normalize_command_value(Ok(noscript)).unwrap_err();
    assert_eq!(error.code(), Some("NOSCRIPT"));
    let ok = redis::parse_redis_value(b"+OK\r\n").unwrap();
    assert!(normalize_command_value(Ok(ok)).is_ok());
    let moved = redis::parse_redis_value(b"-MOVED 3999 127.0.0.1:6380\r\n").unwrap();
    let normalized = normalize_command_value(Ok(moved));
    assert_eq!(moved_target(&normalized).unwrap(), Some(("127.0.0.1".into(), 6380)));
    let ask = redis::parse_redis_value(b"-ASK 3999 127.0.0.1:6381\r\n").unwrap();
    assert_eq!(moved_target(&normalize_command_value(Ok(ask))).unwrap(), None);
}

#[test]
fn cluster_ping_accepts_only_pong() {
    let pong = redis::parse_redis_value(b"+PONG\r\n").unwrap();
    assert!(cluster_ping_succeeded(Ok(Ok(pong))));
    let denied = redis::parse_redis_value(b"-NOPERM this user has no permissions\r\n").unwrap();
    assert!(!cluster_ping_succeeded(Ok(Ok(denied))));
    assert!(!cluster_ping_succeeded(Ok(Err(unavailable()))));
}

#[test]
fn replaced_snapshot_command_failure_does_not_request_recovery() {
    let io = redis::RedisError::from(std::io::Error::new(
        std::io::ErrorKind::ConnectionReset,
        "connection reset",
    ));
    assert!(!command_failure_requests_recovery(false, &io));
    assert!(command_failure_requests_recovery(true, &io));
    let readonly = normalize_command_value(Ok(redis::parse_redis_value(
        b"-READONLY You can't write against a read only replica.\r\n",
    )
    .unwrap()))
    .unwrap_err();
    assert!(command_failure_requests_recovery(true, &readonly));
    assert!(!command_failure_requests_recovery(false, &readonly));
}

#[test]
fn sentinel_duplicate_configured_endpoints_cannot_break_a_tie() {
    use edgion_resources::resources::link_sys::redis::RedisClientConfig;

    let config: RedisClientConfig = serde_json::from_value(serde_json::json!({
        "topology": {"mode": "sentinel", "sentinel": {
            "masterName": "primary",
            "sentinels": ["a.example", "redis://a.example:26379/", "b.example:26379"]
        }}
    }))
    .unwrap();
    let configured: Vec<_> = config
        .parsed_active_endpoints()
        .unwrap()
        .into_iter()
        .map(|endpoint| (endpoint.host, endpoint.port))
        .collect();
    let candidates = sentinel_discovery_candidates(&configured, &[]);
    assert_eq!(
        candidates,
        vec![("a.example".into(), 26379), ("b.example".into(), 26379)]
    );
    let votes: Vec<_> = candidates
        .iter()
        .map(|(host, _)| {
            (
                if host == "a.example" {
                    "old-primary"
                } else {
                    "new-primary"
                }
                .into(),
                6379,
            )
        })
        .collect();
    assert_eq!(choose_sentinel_primary(&votes), None);
}

#[test]
fn sentinel_discovery_keeps_distinct_configured_peers_before_unique_learned_peers() {
    let a = ("a.example".into(), 6379);
    let b = ("b.example".into(), 6379);
    let c = ("a.example".into(), 6380);
    assert_eq!(
        sentinel_discovery_candidates(&[a.clone(), a.clone(), b.clone()], &[b.clone(), c.clone(), c.clone()]),
        vec![a, b, c]
    );
}

#[test]
fn sentinel_discovery_dedups_normalized_spellings() {
    let candidates = sentinel_discovery_candidates(
        &[
            ("A.Example".into(), 6379),
            ("a.example".into(), 6379),
            ("a.example.".into(), 6379),
            ("::1".into(), 26379),
            ("::ffff:10.0.0.1".into(), 26379),
            ("b.example".into(), 6379),
        ],
        &[
            ("a.example".into(), 6379),
            ("0:0:0:0:0:0:0:1".into(), 26379),
            ("10.0.0.1".into(), 26379),
        ],
    );
    // A trailing root dot stays distinct: with search domains it can name another host.
    assert_eq!(
        candidates,
        vec![
            ("A.Example".into(), 6379),
            ("a.example.".into(), 6379),
            ("::1".into(), 26379),
            ("::ffff:10.0.0.1".into(), 26379),
            ("b.example".into(), 6379)
        ]
    );
}

fn vote(id: &str, primary: &str, learned: &[&str]) -> SentinelVote {
    SentinelVote {
        sentinel_id: id.into(),
        primary: (primary.into(), 6379),
        learned: learned.iter().map(|peer| ((*peer).into(), 26379)).collect(),
    }
}

/// The configured DNS seed and the learned IP of the same Sentinel are two
/// candidates but one run ID, so 2:2 stays a refused tie instead of 4:2.
#[test]
fn sentinel_reached_by_name_and_ip_casts_one_vote() {
    let votes = vec![
        vote("run-a", "old-primary", &[]),
        vote("run-b", "old-primary", &[]),
        vote("run-a", "old-primary", &[]),
        vote("run-b", "old-primary", &[]),
        vote("run-c", "new-primary", &[]),
        vote("run-d", "new-primary", &[]),
    ];
    assert!(tally_sentinel_votes(votes).is_none());
    let votes = vec![
        vote("run-a", "old-primary", &[]),
        vote("run-a", "old-primary", &[]),
        vote("run-b", "new-primary", &[]),
        vote("run-c", "new-primary", &[]),
    ];
    assert_eq!(tally_sentinel_votes(votes).unwrap().0, ("new-primary".into(), 6379));
}

/// Three spellings of the new primary are three votes for it, not three
/// singletons that let two identical old-primary votes win 2:1.
#[test]
fn sentinel_primary_votes_count_by_normalized_address() {
    let votes = vec![
        vote("run-a", "New-Primary", &[]),
        vote("run-b", "new-primary", &[]),
        vote("run-c", "NEW-PRIMARY", &[]),
        vote("run-d", "old-primary", &[]),
        vote("run-e", "old-primary", &[]),
    ];
    assert_eq!(tally_sentinel_votes(votes).unwrap().0, ("New-Primary".into(), 6379));
    assert_eq!(
        choose_sentinel_primary(&[("::ffff:10.0.0.9".into(), 6379), ("10.0.0.9".into(), 6379)]),
        Some(("::ffff:10.0.0.9".into(), 6379))
    );
}

#[test]
fn sentinel_learned_peers_come_from_winning_votes_deduped_and_capped() {
    let spellings: Vec<String> = (0..20).map(|n| format!("PEER-{n}")).collect();
    let mut learned: Vec<&str> = vec!["peer-0", "Peer-0", "PEER-0"];
    learned.extend(spellings.iter().map(String::as_str));
    let votes = vec![
        vote("run-a", "primary", &learned),
        vote("run-b", "primary", &["peer-1"]),
        vote("run-c", "loser", &["loser-peer"]),
    ];
    let (_, peers) = tally_sentinel_votes(votes).unwrap();
    assert_eq!(peers.len(), 16);
    assert_eq!(peers[0], ("peer-0".into(), 26379));
    assert_eq!(peers[1], ("PEER-1".into(), 26379));
    assert!(!peers.iter().any(|(host, _)| host == "loser-peer"));
}

#[test]
fn sentinel_primary_is_the_unique_plurality() {
    let primary = ("primary.example".into(), 6379);
    let other = ("other.example".into(), 6379);
    assert_eq!(
        choose_sentinel_primary(&[primary.clone(), primary.clone(), other.clone()]),
        Some(primary.clone())
    );
    assert_eq!(choose_sentinel_primary(&[primary.clone(), other]), None);
    assert_eq!(choose_sentinel_primary(&[primary.clone()]), Some(primary));
    assert_eq!(choose_sentinel_primary(&[]), None);
}

#[test]
fn sentinel_role_timeout_keeps_ping_coverage() {
    let master = Value::Array(vec![Value::BulkString(b"master".to_vec())]);
    let replica = Value::Array(vec![Value::BulkString(b"slave".to_vec())]);
    assert_eq!(
        sentinel_probe_coverage(3, 8, classify_role(RoleReply::Ready(Ok(master)))),
        (3, 8)
    );
    assert_eq!(
        sentinel_probe_coverage(3, 8, classify_role(RoleReply::Ready(Ok(replica)))),
        (0, 8)
    );
    assert_eq!(
        sentinel_probe_coverage(3, 8, classify_role(RoleReply::Ready(Err(unavailable())))),
        (3, 8)
    );
    assert_eq!(
        sentinel_probe_coverage(3, 8, classify_role(RoleReply::TimedOut)),
        (3, 8)
    );
}

#[test]
fn moved_redirect_address_is_host_and_port_not_the_slot() {
    assert_eq!(moved_endpoint("127.0.0.1:6379").unwrap(), ("127.0.0.1".into(), 6379));
    assert_eq!(
        moved_endpoint("[2001:db8::1]:6379").unwrap(),
        ("2001:db8::1".into(), 6379)
    );
    assert!(moved_endpoint("127.0.0.1").is_err());
    assert!(moved_endpoint("127.0.0.1:0").is_err());
}

#[test]
fn cluster_publication_stays_usable_until_its_own_probe() {
    assert_eq!(cluster_published_coverage(3), (1, 3));
    assert_eq!(cluster_published_coverage(0), (0, 0));
}

#[test]
fn superseded_probe_does_not_mark_the_new_generation_unhealthy_or_recovering() {
    let record = record_probe(false, false, 1, (0, 3), true, ProbeRecovery::UnreachableOrTopology);
    assert!(record.healthy);
    assert!(record.coverage.is_none());
    assert!(!record.schedule_recovery);
}

#[test]
fn current_probe_records_partial_coverage_and_a_changed_topology() {
    let pool = record_probe(false, true, 1, (2, 3), false, ProbeRecovery::PartialMembers);
    assert!(pool.healthy);
    assert_eq!(pool.coverage, Some((2, 3)));
    assert!(pool.schedule_recovery);
    let cluster_partial = record_probe(false, true, 1, (2, 3), false, ProbeRecovery::UnreachableOrTopology);
    assert!(cluster_partial.healthy);
    assert_eq!(cluster_partial.coverage, Some((2, 3)));
    assert!(!cluster_partial.schedule_recovery);
    let cluster_down = record_probe(false, true, 1, (0, 3), false, ProbeRecovery::UnreachableOrTopology);
    assert!(!cluster_down.healthy);
    assert!(cluster_down.schedule_recovery);
    let changed = record_probe(false, true, 3, (3, 3), true, ProbeRecovery::UnreachableOrTopology);
    assert!(changed.schedule_recovery);
    let complete = record_probe(false, true, 1, (3, 3), false, ProbeRecovery::UnreachableOrTopology);
    assert!(!complete.schedule_recovery);
    assert!(!record_probe(true, true, 1, (3, 3), true, ProbeRecovery::PartialMembers).healthy);
    assert!(!record_probe(true, true, 1, (3, 3), true, ProbeRecovery::PartialMembers).schedule_recovery);
    assert!(matches!(probe_recovery_for(false), ProbeRecovery::PartialMembers));
    assert!(matches!(probe_recovery_for(true), ProbeRecovery::UnreachableOrTopology));
}

#[test]
fn cluster_discovery_keeps_known_primaries_ahead_of_configured_seeds() {
    let configured = vec![("seed.example".into(), 6379)];
    let known = vec![("primary.example".into(), 6380), ("seed.example".into(), 6379)];
    assert_eq!(
        cluster_discovery_candidates(&configured, &known),
        vec![("primary.example".into(), 6380), ("seed.example".into(), 6379)]
    );
}

#[test]
fn cluster_discovery_bounds_known_primaries_and_keeps_every_configured_seed() {
    let configured = vec![("seed.example".into(), 1), ("other-seed.example".into(), 2)];
    let known: Vec<_> = (0..MAX_KNOWN_CLUSTER_PRIMARIES as u16 + 1)
        .map(|port| (format!("primary-{port}.example"), port.saturating_add(10)))
        .collect();
    let candidates = cluster_discovery_candidates(&configured, &known);
    assert_eq!(candidates.len(), MAX_KNOWN_CLUSTER_PRIMARIES + configured.len());
    assert_eq!(candidates[0], known[0]);
    assert!(!candidates.contains(&known[MAX_KNOWN_CLUSTER_PRIMARIES]));
    assert!(candidates.ends_with(&configured));
}

#[test]
fn multikey_mutation_has_a_single_destination_before_submission() {
    let mut cmd = redis::cmd("DEL");
    cmd.arg("{first}:key").arg("{second}:key");
    assert!(matches!(
        command_route(&cmd).unwrap(),
        RoutingInfo::SingleNode(SingleNodeRoutingInfo::SpecificNode(_))
    ));
    let mut script = redis::cmd("EVAL");
    script.arg("return 1").arg(1).arg("{first}:key");
    assert_eq!(command_route(&cmd).unwrap(), command_route(&script).unwrap());
}
