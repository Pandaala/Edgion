//! Repository-owned CRS package projection shared by examples and acceptance tests.
use anyhow::{ensure, Context, Result};
use edgion_resources::resources::edgion_config_data::{
    EdgionConfigDataRef, WafBundleMember, WafBundleRef, WafPolicyData, WafProfile, WafRuleBundleData,
};
use edgion_resources::resources::edgion_plugins::plugin_configs::{
    ResolvedWafBundle, WafConfig, WafRequestBodyInspection,
};
use edgion_waf::seclang::{
    inspect_compatibility, FrontendLimits, IncludeFailure, IncludeRequest, IncludeSink, IncludeSource,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

pub const PROVENANCE_SHA256: &str = "ca158245e3a70bd3b1a4029026a09a106dab3715d4562c17520657f49f4e08d7";
pub const CORE: &str = "crs-supported-core";
pub const POLICY: &str = "crs-supported-policy";
pub const TUNING_ID: u64 = 9900301;
pub const VERSION_ID: u64 = 9900302;
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Unit {
    pub id: u64,
    pub source: String,
    pub phase: u8,
    pub start: usize,
    pub end: usize,
    pub sha256: String,
    pub excluded_capabilities: Vec<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub version: String,
    pub revision: String,
    pub archive_sha256: String,
    pub files: BTreeMap<String, String>,
    pub units: Vec<Unit>,
}
pub struct Package {
    pub manifest: Provenance,
    pub core: WafRuleBundleData,
    pub original: Vec<WafBundleMember>,
    pub setup: String,
}
struct Sources<'a>(&'a [WafBundleMember]);
impl IncludeSource for Sources<'_> {
    fn load(&mut self, request: IncludeRequest<'_>, sink: &mut IncludeSink<'_>) -> Result<(), IncludeFailure> {
        let member = self
            .0
            .iter()
            .find(|m| m.name == request.reference())
            .ok_or(IncludeFailure::NotFound)?;
        sink.push_source(&member.name, member.content.as_bytes())
            .map_err(|_| IncludeFailure::Other)
    }
}
pub fn directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../config/waf/crs-4.25.1-supported-v1")
}
pub fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub fn load() -> Result<Package> {
    let dir = directory();
    let manifest_bytes = fs::read(dir.join("provenance.json"))?;
    ensure!(
        sha(&manifest_bytes) == PROVENANCE_SHA256,
        "provenance manifest changed; review original corpus before updating its pin"
    );
    let manifest: Provenance = serde_json::from_slice(&manifest_bytes)?;
    ensure!(
        manifest.version == "4.25.1" && manifest.revision == "3b89d5a05322f448b4b74b9cadc5fb05ac6915ad",
        "unexpected CRS provenance"
    );
    ensure!(
        manifest.archive_sha256 == "0539e66e7627fe71c160a644d8fb7ab6e450d53c9de208be5f95a35c70e1a154",
        "unexpected archive claim"
    );
    let mut original = Vec::new();
    let mut assets = Vec::new();
    for (name, expected) in &manifest.files {
        ensure!(!name.contains("..") && !name.starts_with('/'), "unsafe provenance path");
        let bytes = fs::read(dir.join("upstream").join(name))?;
        ensure!(sha(&bytes) == *expected, "original file hash mismatch: {name}");
        if let Some(name) = name.strip_prefix("rules/") {
            let m = WafBundleMember {
                name: name.into(),
                content: String::from_utf8(bytes)?,
            };
            if name.ends_with(".conf") {
                original.push(m);
            } else {
                ensure!(name.ends_with(".data"), "unexpected asset");
                assets.push(m);
            }
        }
    }
    ensure!(
        original.len() == 27 && assets.len() == 21 && manifest.files.len() == 50 && manifest.units.len() == 627,
        "corpus denominator changed"
    );
    let mut ids = BTreeSet::new();
    let mut declared = BTreeMap::<u64, BTreeSet<String>>::new();
    for unit in &manifest.units {
        ensure!(ids.insert(unit.id), "duplicate original unit");
        let m = original
            .iter()
            .find(|m| m.name == unit.source)
            .context("missing unit source")?;
        let bytes = m
            .content
            .as_bytes()
            .get(unit.start..unit.end)
            .context("unit span out of range")?;
        ensure!(sha(bytes) == unit.sha256, "original unit hash mismatch: {}", unit.id);
        if !unit.excluded_capabilities.is_empty() {
            declared.insert(unit.id, unit.excluded_capabilities.iter().cloned().collect());
        }
    }
    let root = original
        .iter()
        .map(|m| format!("Include '{}'\n", m.name))
        .collect::<String>();
    let report = inspect_compatibility(
        "original-crs",
        root.as_bytes(),
        &mut Sources(&original),
        FrontendLimits::default(),
    )?;
    ensure!(
        report.rule_unit_count() == 627
            && report.unavailable_rule_unit_count() == 62
            && report.omitted_findings() == 0
            && report.omitted_rule_summaries() == 0,
        "incomplete capability inventory"
    );
    let mut actual = BTreeMap::<u64, BTreeSet<String>>::new();
    for f in report.findings() {
        actual
            .entry(f.rule_id().context("capability without unit")?)
            .or_default()
            .insert(format!("{:?}", f.capability()));
    }
    ensure!(actual == declared, "explicit capability exclusions drifted");
    ensure!(
        declared.get(&901320) == Some(&BTreeSet::from(["InitCol".into()]))
            && declared.get(&905100) == Some(&BTreeSet::from(["Ctl".into()]))
            && declared.get(&905110) == Some(&BTreeSet::from(["Ctl".into()]))
            && declared
                .values()
                .filter(|caps| **caps == BTreeSet::from(["FullResponseBody".into()]))
                .count()
                == 59,
        "unsupported capability decision changed"
    );
    let setup_source = fs::read_to_string(dir.join("upstream/crs-setup.conf.example"))?;
    let start = setup_source
        .find("SecAction \\\n    \"id:900990,")
        .context("original setup action missing")?;
    let end = start
        + setup_source[start..]
            .find("setvar:tx.crs_setup_version=4251\"\n")
            .context("setup action ending missing")?
        + "setvar:tx.crs_setup_version=4251\"\n".len();
    let setup = setup_source[start..end].to_string();
    let mut rules = vec![WafBundleMember {
        name: "setup-version.conf".into(),
        content: setup.clone(),
    }];
    for member in &original {
        let mut ranges = manifest
            .units
            .iter()
            .filter(|u| u.source == member.name)
            .collect::<Vec<_>>();
        ranges.sort_by_key(|u| u.start);
        let mut last = 0;
        let mut content = String::new();
        for u in ranges {
            ensure!(u.start >= last, "overlapping original unit spans");
            content.push_str(&member.content[last..u.start]);
            if u.excluded_capabilities.is_empty() {
                content.push_str(&member.content[u.start..u.end]);
            }
            last = u.end;
        }
        content.push_str(&member.content[last..]);
        rules.push(WafBundleMember {
            name: member.name.clone(),
            content,
        });
    }
    let mut core=WafRuleBundleData{version:"4.25.1-supported-v1".into(),profile:"supported-core".into(),provenance:"OWASP CRS 4.25.1 at 3b89d5a05322f448b4b74b9cadc5fb05ac6915ad; 565 unchanged core units + original setup 900990; 62 explicit exclusions; Apache-2.0".into(),digest:None,roots:rules.iter().map(|m|m.name.clone()).collect(),rules,phrase_assets:assets};
    core.digest = Some(core.compute_digest());
    core.validate().map_err(anyhow::Error::msg)?;
    Ok(Package {
        manifest,
        core,
        original,
        setup,
    })
}
pub fn tuning(profile: &str) -> Result<WafRuleBundleData> {
    let pl = match profile {
        "standard" => 1,
        "strict" => 4,
        _ => anyhow::bail!("unknown supported profile"),
    };
    let mut data=WafRuleBundleData{version:"supported-v1".into(),profile:profile.into(),provenance:"Edgion explicit tuning: PL1/PL4, UTF-8 validation, XML attributes; original 901 score/threshold/early-blocking defaults".into(),digest:None,roots:vec!["tuning.conf".into()],rules:vec![WafBundleMember{name:"tuning.conf".into(),content:format!("SecAction \"id:{TUNING_ID},phase:1,pass,nolog,t:none,setvar:tx.blocking_paranoia_level={pl},setvar:tx.detection_paranoia_level={pl},setvar:tx.crs_validate_utf8_encoding=1,setvar:tx.crs_xml_attr_inspect=1\"\n")}],phrase_assets:Vec::new()};
    data.digest = Some(data.compute_digest());
    Ok(data)
}
pub fn policy(package: &Package) -> Result<WafPolicyData> {
    let profiles = ["standard", "strict"]
        .into_iter()
        .map(|p| {
            Ok((
                p.into(),
                WafProfile {
                    rule_selection: Default::default(),
                    bundle_refs: vec![
                        reference(&format!("crs-supported-{p}"), &tuning(p)?),
                        reference(CORE, &package.core),
                    ],
                },
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    Ok(WafPolicyData {
        default_profile: "standard".into(),
        profiles,
    })
}
fn reference(name: &str, data: &WafRuleBundleData) -> WafBundleRef {
    WafBundleRef {
        target: EdgionConfigDataRef {
            name: name.into(),
            ..Default::default()
        },
        expected_digest: Some(data.compute_digest()),
        optional: false,
    }
}
pub fn config(package: &Package, profile: &str) -> Result<WafConfig> {
    let tuning = tuning(profile)?;
    let mut config = WafConfig {
        policy_ref: EdgionConfigDataRef {
            name: POLICY.into(),
            ..Default::default()
        },
        owner_namespace: Some("edgion-default".into()),
        selected_profile: Some(profile.into()),
        resolved_ref_indices: vec![0, 1],
        resolved_policy: Some(policy(package)?),
        resolved_bundles: Some(vec![
            ResolvedWafBundle {
                identity: format!("edgion-default/crs-supported-{profile}"),
                digest: tuning.compute_digest(),
                data: tuning,
            },
            ResolvedWafBundle {
                identity: format!("edgion-default/{CORE}"),
                digest: package.core.compute_digest(),
                data: package.core.clone(),
            },
        ]),
        ..Default::default()
    };
    config.request_body.inspection = WafRequestBodyInspection::Full;
    Ok(config)
}
pub fn bundle_resource(name: &str, data: &WafRuleBundleData) -> Value {
    json!({"apiVersion":"edgion.io/v1","kind":"EdgionConfigData","metadata":{"name":name,"namespace":"edgion-default"},"spec":{"data":{"type":"WafRuleBundle","config":data}}})
}
// Kubernetes YAML 1.1 treats bare `on` as a boolean, while the CRD requires
// a string. Restrict this rendering adjustment to generated WAF plugin maps.
fn serialize_waf_plugin(plugin: &Value) -> Result<String> {
    ensure!(plugin["kind"] == "EdgionPlugins", "expected a WAF plugin resource");
    let mode = plugin["spec"]["waf"]["mode"].as_str().context("WAF mode string")?;
    ensure!(matches!(mode, "on" | "detectionOnly"), "unexpected WAF mode");
    let mut yaml = yaml_serde::to_string(plugin)?;
    if mode == "on" {
        ensure!(
            yaml.lines().filter(|line| *line == "    mode: on").count() == 1,
            "expected one generated WAF mode line"
        );
        yaml = yaml
            .lines()
            .map(|line| {
                if line == "    mode: on" {
                    "    mode: \"on\""
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
    }
    ensure!(
        yaml_serde::from_str::<Value>(&yaml)? == *plugin,
        "WAF YAML rendering changed resource values"
    );
    Ok(yaml)
}

pub fn outputs(package: &Package) -> Result<BTreeMap<String, String>> {
    let mut files = BTreeMap::new();
    files.insert(
        format!("EdgionConfigData_{CORE}.yaml"),
        yaml_serde::to_string(&bundle_resource(CORE, &package.core))?,
    );
    for p in ["standard", "strict"] {
        files.insert(
            format!("EdgionConfigData_crs-supported-{p}.yaml"),
            yaml_serde::to_string(&bundle_resource(&format!("crs-supported-{p}"), &tuning(p)?))?,
        );
        let g = config(package, p)?.compile_generation().map_err(anyhow::Error::msg)?;
        let expected = [TUNING_ID, 900990]
            .into_iter()
            .chain(
                package
                    .manifest
                    .units
                    .iter()
                    .filter(|u| u.excluded_capabilities.is_empty())
                    .map(|u| u.id),
            )
            .collect::<Vec<_>>();
        ensure!(
            g.rule_units().skip(2).map(|u| u.phase() as u8).collect::<Vec<_>>()
                == package
                    .manifest
                    .units
                    .iter()
                    .filter(|u| u.excluded_capabilities.is_empty())
                    .map(|u| u.phase)
                    .collect::<Vec<_>>(),
            "original executable phase sequence changed"
        );
        ensure!(
            g.rule_units().map(|u| u.id()).collect::<Vec<_>>() == expected,
            "original executable ID sequence changed"
        );
        ensure!(
            g.rule_units().count() == 567
                && g.json_inspection_unsupported().is_empty()
                && g.xml_inspection_unsupported().is_empty(),
            "supported profile not strictly executable"
        );
    }
    files.insert(format!("EdgionConfigData_{POLICY}.yaml"),yaml_serde::to_string(&json!({"apiVersion":"edgion.io/v1","kind":"EdgionConfigData","metadata":{"name":POLICY,"namespace":"edgion-default"},"spec":{"data":{"type":"WafPolicy","config":policy(package)?}}}))?);
    files.insert("EdgionPlugins_crs-supported.yaml".into(),serialize_waf_plugin(&json!({"apiVersion":"edgion.io/v1","kind":"EdgionPlugins","metadata":{"name":"crs-supported","namespace":"edgion-default"},"spec":{"waf":{"mode":"on","policyRef":{"name":POLICY},"activeProfile":"standard","requestBody":{"inspection":"full"}}}}))?);
    let record = json!({"version":"supported-v1","original_core_units":627,"retained_original_units":565,"original_setup_units":1,"product_tuning_units":1,"core_digest":package.core.compute_digest(),"setup_sha256":sha(package.setup.as_bytes()),"profiles":policy(package)?,"outputs_sha256":files.iter().map(|(n,c)|(n.clone(),sha(c.as_bytes()))).collect::<BTreeMap<_,_>>(),"excluded_units":package.manifest.units.iter().filter(|u|!u.excluded_capabilities.is_empty()).collect::<Vec<_>>()});
    files.insert("acceptance.json".into(), serde_json::to_string_pretty(&record)? + "\n");
    Ok(files)
}
fn check_file_set(directory: &Path, expected: BTreeSet<String>) -> Result<()> {
    let actual = fs::read_dir(directory)?
        .map(|entry| entry.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<Result<BTreeSet<_>, _>>()?;
    ensure!(
        actual == expected,
        "unexpected or missing generated files: {}",
        directory.display()
    );
    Ok(())
}
pub fn generate(check: bool) -> Result<()> {
    let package = load()?;
    let outputs = outputs(&package)?;
    if check {
        check_file_set(&directory().join("generated"), outputs.keys().cloned().collect())?;
    }
    for (name, content) in outputs {
        let path = directory().join("generated").join(name);
        if check {
            ensure!(
                fs::read_to_string(&path).with_context(|| format!("missing generated {}", path.display()))? == content,
                "generated output drift: {}",
                path.display()
            );
        } else {
            fs::create_dir_all(path.parent().context("output parent")?)?;
            fs::write(path, content)?;
        }
    }
    Ok(())
}

/// Test-owned guards do not alter product core or tuning bytes/digests.
pub fn wire_outputs(package: &Package) -> Result<BTreeMap<String, String>> {
    let mut files = BTreeMap::new();
    let guard=WafRuleBundleData{version:"wire-v1".into(),profile:"test-owned-version-guard".into(),provenance:"Integration-only publication witness; not an original CRS rule".into(),digest:None,roots:vec!["guard.conf".into()],rules:vec![WafBundleMember{name:"guard.conf".into(),content:format!("SecRule REQUEST_HEADERS:x-waf-version \"!@streq supported-crs\" \"id:{VERSION_ID},phase:1,deny,status:403,log,t:none\"\n")}],phrase_assets:vec![]};
    let mut policy = policy(package)?;
    for p in policy.profiles.values_mut() {
        p.bundle_refs.insert(0, reference("crs-supported-wire-guard", &guard));
    }
    let policy_value = json!({"apiVersion":"edgion.io/v1","kind":"EdgionConfigData","metadata":{"name":"waf-policy","namespace":"edgion-default"},"spec":{"data":{"type":"WafPolicy","config":policy}}});
    for (name, data) in [
        (CORE, package.core.clone()),
        ("crs-supported-standard", tuning("standard")?),
        ("crs-supported-strict", tuning("strict")?),
        ("crs-supported-wire-guard", guard),
    ] {
        files.insert(
            format!("updates/to_supported_on/EdgionConfigData_{name}.yaml"),
            yaml_serde::to_string(&bundle_resource(name, &data))?,
        );
    }
    files.insert(
        "updates/to_supported_on/EdgionConfigData_waf-policy.yaml".into(),
        yaml_serde::to_string(&policy_value)?,
    );
    let route = json!({"apiVersion":"gateway.networking.k8s.io/v1","kind":"HTTPRoute","metadata":{"name":"waf-supported","namespace":"edgion-default"},"spec":{"parentRefs":[{"name":"edgion-plugins-gateway","namespace":"edgion-test"}],"hostnames":["waf-supported.example.com"],"rules":[{"filters":[{"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"waf-dynamic"}}],"backendRefs":[{"name":"test-http","namespace":"edgion-test","port":30001}]}]}});
    files.insert(
        "updates/to_supported_on/HTTPRoute_waf-supported.yaml".into(),
        yaml_serde::to_string(&route)?,
    );
    for (phase, profile, mode) in [
        ("to_supported_on", "strict", "on"),
        ("to_supported_detection", "strict", "detectionOnly"),
        ("to_supported_standard", "standard", "on"),
    ] {
        let plugin = json!({"apiVersion":"edgion.io/v1","kind":"EdgionPlugins","metadata":{"name":"waf-dynamic","namespace":"edgion-default"},"spec":{"waf":{"mode":mode,"policyRef":{"name":"waf-policy"},"activeProfile":profile,"requestBody":{"inspection":"full"}}}});
        files.insert(
            format!("updates/{phase}/EdgionPlugins_waf-dynamic.yaml"),
            serialize_waf_plugin(&plugin)?,
        );
    }
    let mut tampered = package.core.clone();
    tampered.rules[0].content.push('\n');
    files.insert(
        format!("updates/to_supported_invalid/EdgionConfigData_{CORE}.yaml"),
        yaml_serde::to_string(&bundle_resource(CORE, &tampered))?,
    );
    files.insert("delete/supported_resources.txt".into(),format!("HTTPRoute/edgion-default/waf-supported\nEdgionConfigData/edgion-default/{CORE}\nEdgionConfigData/edgion-default/crs-supported-standard\nEdgionConfigData/edgion-default/crs-supported-strict\nEdgionConfigData/edgion-default/crs-supported-wire-guard\n"));
    Ok(files)
}
pub fn generate_wire(check: bool) -> Result<()> {
    let package = load()?;
    let outputs = wire_outputs(&package)?;
    let root = directory().join("../../..");
    for platform in ["integration", "k8s"] {
        let fixture = root
            .join("edgion-tests")
            .join(platform)
            .join("conf/EdgionPlugins/Waf/DynamicTest");
        if check {
            for phase in [
                "to_supported_on",
                "to_supported_detection",
                "to_supported_standard",
                "to_supported_invalid",
            ] {
                let prefix = format!("updates/{phase}/");
                check_file_set(
                    &fixture.join("updates").join(phase),
                    outputs
                        .keys()
                        .filter_map(|p| p.strip_prefix(&prefix).map(str::to_owned))
                        .collect(),
                )?;
            }
        }
        for (name, content) in &outputs {
            let path = fixture.join(name);
            if check {
                ensure!(
                    fs::read_to_string(&path)? == *content,
                    "wire fixture drift: {}",
                    path.display()
                );
            } else {
                fs::create_dir_all(path.parent().context("fixture parent")?)?;
                fs::write(path, content)?;
            }
        }
    }
    Ok(())
}
