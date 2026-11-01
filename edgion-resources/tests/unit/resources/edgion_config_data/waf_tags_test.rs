use super::*;
use crate::resources::edgion_config_data::{EdgionConfigDataSpec, WafBundleMember, WafRuleBundleData};
use crate::resources::edgion_plugins::WafConfig;

fn bundle(source: &str) -> EdgionConfigData {
    EdgionConfigData::new(
        "bundle",
        EdgionConfigDataSpec {
            current_status: None,
            enable: true,
            active: None,
            visibility: Default::default(),
            unknown_fields: vec![],
            data: ConfigEntry::WafRuleBundle(WafRuleBundleData {
                version: "1".into(),
                profile: "local".into(),
                provenance: "test/revision".into(),
                digest: None,
                roots: vec!["main.conf".into()],
                rules: vec![WafBundleMember {
                    name: "main.conf".into(),
                    content: source.into(),
                }],
                phrase_assets: vec![],
            }),
        },
    )
}
#[test]
fn local_inventory_reports_root_membership_and_strict_failure_separately() {
    let resource =
        bundle("SecAction \"id:3,phase:1,pass,tag:php,tag:php\"\nSecAction \"id:1,phase:2,pass,tag:java\"\n");
    let selected: WafRuleSelection = yaml_serde::from_str("include: {tag: php}\nexclude: {tag: unknown}\n").unwrap();
    let inputs = [resource];
    let report = WafConfig::inspect_local_bundles(&inputs, &selected, WafTagQuery::Catalog, 0).unwrap();
    assert!(report.inventory_complete);
    assert_eq!(report.selected_compilation_passed, Some(true));
    assert_eq!(report.selected_unit_count, Some(1));
    assert_eq!(report.tags.len(), 2);
    assert_eq!(report.tags[1].unit_count, 1);
    assert_eq!(report.tags[1].root_ids, [3]);
    assert_eq!(report.unmatched_atom_refs, ["/exclude/tag"]);
    let encoded = report.to_json().unwrap();
    assert!(!encoded.contains("SecAction"));
    assert!(!encoded.contains("main.conf"));
    assert!(!report.body_admission_checked);
    let page = WafConfig::inspect_local_bundles(&inputs, &selected, WafTagQuery::Excluded, 0).unwrap();
    assert_eq!(page.root_ids, [1]);
    assert_eq!(page.membership_basis, Some("compiledSelection"));
    let failed = [bundle(
        "SecRule REQUEST_URI \"@rx secret-(\" \"id:7,phase:1,pass,tag:php\"\n",
    )];
    let report = WafConfig::inspect_local_bundles(&failed, &selected, WafTagQuery::Selected, 0).unwrap();
    assert!(report.inventory_complete);
    assert_eq!(report.baseline_compilation_passed, Some(false));
    assert_eq!(report.selected_compilation_passed, None);
    assert_eq!(report.selected_unit_count, None);
    assert_eq!(report.empty_selection, None);
    assert_eq!(report.root_ids, [7]);
    assert_eq!(report.membership_basis, Some("semanticPlan"));
    assert!(!report.to_json().unwrap().contains("secret-("));
}

#[test]
fn pagination_reuses_id_order_and_output_truncation_does_not_change_selection() {
    let source = (1..=260)
        .rev()
        .map(|id| format!("SecAction \"id:{id},phase:1,pass,tag:tag/{id}\"\n"))
        .collect::<String>();
    let inputs = [bundle(&source)];
    let selection = WafRuleSelection::default();
    let report = WafConfig::inspect_local_bundles(&inputs, &selection, WafTagQuery::Catalog, 0).unwrap();
    assert!(report.inventory_complete);
    assert!(!report.report_complete);
    assert!(report.tags.len() <= 128);
    assert_eq!(report.selected_unit_count, Some(260));
    assert!(report.to_json().unwrap().len() <= MAX_WAF_TAG_REPORT_BYTES);
    let mut ids = Vec::new();
    let mut offset = 0;
    loop {
        let page = WafConfig::inspect_local_bundles(&inputs, &selection, WafTagQuery::Selected, offset).unwrap();
        assert!(page.report_complete);
        assert_eq!(page.total_ids, Some(260));
        ids.extend(page.root_ids);
        if let Some(next) = page.next_offset {
            offset = next;
        } else {
            break;
        }
    }
    assert_eq!(ids, (1..=260).collect::<Vec<_>>());
    let empty = WafConfig::inspect_local_bundles(&inputs, &selection, WafTagQuery::Tag("unknown"), 0).unwrap();
    assert_eq!(empty.membership_basis, Some("semanticInventory"));
    assert_eq!(empty.total_ids, Some(0));
    assert!(empty.failure.is_none());
}

#[test]
fn incomplete_inventory_never_reports_zero_matches_as_known() {
    let inputs = [bundle("SecRule private-bad-source")];
    let selection: WafRuleSelection = yaml_serde::from_str("include: {tag: unknown}").unwrap();
    let report = WafConfig::inspect_local_bundles(&inputs, &selection, WafTagQuery::Selected, 0).unwrap();
    assert!(!report.inventory_complete);
    assert_eq!(report.total_ids, None);
    assert_eq!(report.baseline_compilation_passed, None);
    assert_eq!(report.atom_match_counts[0].raw_matches, None);
    assert!(!report.to_json().unwrap().contains("private-bad-source"));
}

#[test]
fn local_identity_defaults_do_not_allow_duplicate_bundle_aliases() {
    let first = bundle("SecAction \"id:1,phase:1,pass\"\n");
    let mut alias = first.clone();
    alias.metadata.namespace = Some("default".into());
    let selection = WafRuleSelection::default();
    let resources = [first, alias];
    let report = WafConfig::inspect_local_bundles(&resources, &selection, WafTagQuery::Catalog, 0).unwrap();
    assert_eq!(report.failure, Some("invalid_input"));
    assert!(!report.inventory_complete);
}

#[test]
fn escaped_selection_metadata_and_deep_atom_paths_stay_within_report_bound() {
    let tags = (0..64)
        .map(|id| serde_json::json!({"tag": format!("{id}{}", "\"\\".repeat(48))}))
        .collect::<Vec<_>>();
    let mut node = serde_json::json!({"anyOf": tags});
    for _ in 0..14 {
        node = serde_json::json!({"not": node});
    }
    let selection: WafRuleSelection = serde_json::from_value(serde_json::json!({"include": node})).unwrap();
    let mut resources = Vec::new();
    for index in 0..16 {
        let source = (0..16)
            .map(|offset| {
                let id = index * 16 + offset + 1;
                format!("SecAction \"id:{id},phase:1,pass,tag:tag-{id}-{}\"\n", "x".repeat(220))
            })
            .collect::<String>();
        let mut resource = bundle(&source);
        resource.metadata.name = Some(format!("bundle-{index}"));
        let ConfigEntry::WafRuleBundle(data) = &mut resource.spec.data else {
            unreachable!()
        };
        data.version = "\"\\".repeat(500);
        data.profile = "\"\\".repeat(500);
        resources.push(resource);
    }
    let report = WafConfig::inspect_local_bundles(&resources, &selection, WafTagQuery::Catalog, 0).unwrap();
    assert!(report.inventory_complete);
    assert_eq!(report.selected_compilation_passed, Some(true));
    assert_eq!(report.selected_unit_count, Some(0));
    assert_eq!(report.atom_match_counts.len(), 64);
    assert_eq!(report.unmatched_atom_refs.len(), 64);
    assert!(report.omitted_bundle_metadata > 0);
    assert!(report.omitted_tag_occurrences > 0);
    assert!(!report.report_complete);
    let encoded = report.to_json().unwrap();
    assert!(encoded.len() <= MAX_WAF_TAG_REPORT_BYTES);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&encoded).unwrap()["selection"],
        serde_json::to_value(&selection).unwrap()
    );
}
