use super::*;

fn make_region(name: &str, low: u16, high: u16, endpoint: &str) -> RegionDef {
    RegionDef {
        name: name.to_string(),
        hash_range: [low, high],
        backend_endpoint: endpoint.to_string(),
        tls: false,
        failover_to: None,
    }
}

#[test]
fn table_bounds_are_inclusive_disjoint_and_consumer_specific() {
    let first = make_region("first", 0, 10, "first:80");
    let overlap = make_region("second", 10, 20, "second:80");
    assert!(validate_regions(&[first.clone(), overlap])
        .unwrap_err()
        .contains("overlap"));
    let gap = make_region("second", 12, 20, "second:80");
    let proof = RegionTableValidation::new(&[gap, first]).unwrap();
    assert!(!proof.accepts_modulo(20));
    assert!(proof.accepts_modulo(21));
    assert!(proof.validate_modulo(20).unwrap_err().contains("modulo 20"));
    assert!(!proof.accepts_modulo(0));
    assert!(!proof.accepts_modulo(1));
    assert!(validate_regions(&[make_region("last", 65534, 65534, "last")]).is_ok());
    assert!(validate_regions(&[make_region("last", 65535, 65535, "last")]).is_err());
}

#[test]
fn failover_graph_rejects_dangling_self_and_cycles_but_allows_chains() {
    let mut regions = vec![
        make_region("a", 0, 9, "a"),
        make_region("b", 10, 19, "b"),
        make_region("c", 20, 29, "c"),
    ];
    regions[0].failover_to = Some("a".to_string());
    assert!(validate_regions(&regions).unwrap_err().contains("itself"));
    regions[0].failover_to = Some("missing".to_string());
    assert!(validate_regions(&regions).unwrap_err().contains("does not exist"));
    regions[0].failover_to = Some("b".to_string());
    regions[1].failover_to = Some("c".to_string());
    assert!(validate_regions(&regions).is_ok());
    regions[2].failover_to = Some("a".to_string());
    assert!(validate_regions(&regions).unwrap_err().contains("cycle"));
    regions[2].failover_to = Some("b".to_string());
    assert!(validate_regions(&regions).unwrap_err().contains("cycle"));
    regions[2].failover_to = None;
    regions[0].failover_to = Some("c".to_string());
    assert!(validate_regions(&regions).is_ok(), "converging walks are not cycles");
}

#[test]
fn long_failover_walk_is_iterative() {
    let mut regions: Vec<_> = (0..4000)
        .map(|slot| make_region(&format!("r{slot}"), slot, slot, "backend"))
        .collect();
    for (index, region) in regions.iter_mut().enumerate().take(3999) {
        region.failover_to = Some(format!("r{}", index + 1));
    }
    assert!(validate_regions(&regions).is_ok());
    regions[3999].failover_to = Some("r0".to_string());
    assert!(validate_regions(&regions).unwrap_err().contains("cycle"));
}

#[test]
fn setting_requires_local_membership_and_a_complete_table() {
    let mut setting = ClusterRegionSetting {
        cluster_name: "prod".into(),
        region_name: "east".into(),
        regions: Vec::new(),
    };
    assert!(setting.validate_and_init().is_err());
    setting.regions = vec![make_region("west", 0, 999, "west")];
    assert!(setting.validate_and_init().unwrap_err().contains("regionName"));
    setting.region_name = "west".into();
    assert!(setting.validate_and_init().is_ok());
    setting.cluster_name.clear();
    assert!(setting.validate_and_init().unwrap_err().contains("clusterName"));
}

#[test]
fn validate_regions_empty_is_err() {
    assert!(validate_regions(&[]).is_err());
}

#[test]
fn validate_regions_valid_passes() {
    let regions = vec![
        make_region("cn", 0, 499, "cn-backend:443"),
        make_region("us", 500, 999, "us-backend:443"),
    ];
    assert!(validate_regions(&regions).is_ok());
}

#[test]
fn validate_regions_duplicate_name_is_err() {
    let regions = vec![
        make_region("cn", 0, 499, "ep-a:443"),
        make_region("cn", 500, 999, "ep-b:443"), // duplicate
    ];
    let err = validate_regions(&regions).unwrap_err();
    assert!(err.contains("duplicate") && err.contains("cn"), "{}", err);
}

#[test]
fn validate_regions_reversed_hash_range_is_err() {
    let regions = vec![make_region("bad", 500, 0, "ep:443")]; // low > high
    let err = validate_regions(&regions).unwrap_err();
    assert!(err.contains("invalid") && err.contains("bad"), "{}", err);
}

#[test]
fn validate_regions_empty_endpoint_is_err() {
    let regions = vec![make_region("r1", 0, 999, "")]; // empty backendEndpoint
    let err = validate_regions(&regions).unwrap_err();
    assert!(err.contains("backendEndpoint") && err.contains("r1"), "{}", err);
}

#[test]
fn validate_regions_empty_name_is_err() {
    let regions = vec![make_region("", 0, 999, "ep:443")]; // empty name
    let err = validate_regions(&regions).unwrap_err();
    assert!(err.contains("empty name"), "{}", err);
}

#[test]
fn override_validate_and_init_delegates_to_validate_regions() {
    // Regression: ClusterRegionSetting::validate_and_init must catch the SAME
    // rules as validate_regions (controller/gateway symmetry).
    let ov = ClusterRegionSetting {
        cluster_name: "prod".into(),
        region_name: "cn".into(),
        regions: vec![
            make_region("cn", 0, 499, "ep-a:443"),
            make_region("cn", 500, 999, "ep-b:443"), // duplicate name
        ],
    };
    let err = ov.validate_and_init().unwrap_err();
    assert!(err.contains("duplicate") && err.contains("cn"), "{}", err);
}
