//! Offline supported-CRS inventory and requested-allocation experiment.
//! This executable changes no production policy, dependencies or runtime limits.
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use bytes::Bytes;
use edgion_waf::seclang::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

type Fallible<T> = Result<T, Box<dyn Error>>;
const PACKAGE: &str = "config/waf/crs-4.25.1-supported-v1";
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static CALLS: AtomicUsize = AtomicUsize::new(0);
static REQUESTED: AtomicUsize = AtomicUsize::new(0);
struct CountingSystem;
#[global_allocator]
static ALLOCATOR: CountingSystem = CountingSystem;
fn allocated(size: usize) {
    CALLS.fetch_add(1, Ordering::Relaxed);
    REQUESTED.fetch_add(size, Ordering::Relaxed);
    let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(live, Ordering::Relaxed);
}
// SAFETY: all pointer/layout contracts are forwarded to System unchanged;
// accounting uses only allocation-free atomics and successful operations.
unsafe impl GlobalAlloc for CountingSystem {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller supplies the GlobalAlloc layout contract.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller supplies the GlobalAlloc layout contract.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: pointer and layout originate from this forwarding allocator.
        unsafe { System.dealloc(pointer, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: the caller supplies the original allocation and new size.
        let replacement = unsafe { System.realloc(pointer, layout, size) };
        if !replacement.is_null() {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            allocated(size);
        }
        replacement
    }
}
#[derive(Clone, Copy, Serialize)]
struct Allocation {
    calls: usize,
    requested_bytes: usize,
    peak_extra_bytes: usize,
    live_delta_bytes: i128,
}
struct Stage {
    live: usize,
    calls: usize,
    requested: usize,
}
impl Stage {
    fn start() -> Self {
        let live = LIVE.load(Ordering::Relaxed);
        PEAK.store(live, Ordering::Relaxed);
        Self {
            live,
            calls: CALLS.load(Ordering::Relaxed),
            requested: REQUESTED.load(Ordering::Relaxed),
        }
    }
    fn finish(self) -> Allocation {
        Allocation {
            calls: CALLS.load(Ordering::Relaxed) - self.calls,
            requested_bytes: REQUESTED.load(Ordering::Relaxed) - self.requested,
            peak_extra_bytes: PEAK.load(Ordering::Relaxed).saturating_sub(self.live),
            live_delta_bytes: LIVE.load(Ordering::Relaxed) as i128 - self.live as i128,
        }
    }
}
#[derive(Clone, Deserialize)]
struct Member {
    name: String,
    content: String,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Bundle {
    digest: String,
    roots: Vec<String>,
    rules: Vec<Member>,
    phrase_assets: Vec<Member>,
}
fn load(profile: &str) -> Fallible<Bundle> {
    let value: serde_yaml::Value = serde_yaml::from_slice(&fs::read(format!(
        "{PACKAGE}/generated/EdgionConfigData_crs-supported-{profile}.yaml"
    ))?)?;
    Ok(serde_yaml::from_value(value["spec"]["data"]["config"].clone())?)
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn context() -> Fallible<SemanticValidationContext> {
    let defaults = [
        Phase::RequestHeaders,
        Phase::RequestBody,
        Phase::ResponseHeaders,
        Phase::ResponseBody,
        Phase::Logging,
    ]
    .map(|phase| PhaseDefaults::pass(phase, true, true));
    Ok(SemanticValidationContext::try_new(Some(Phase::RequestBody), &defaults)?)
}
fn compile(tuning: &Bundle, core: &Bundle) -> Fallible<CompiledGeneration> {
    let bundles = [tuning, core];
    let roots = bundles.map(|bundle| bundle.roots.iter().map(String::as_str).collect::<Vec<_>>());
    let rules = bundles.map(|bundle| {
        bundle
            .rules
            .iter()
            .map(|member| RuleBundleMember {
                path: &member.name,
                bytes: member.content.as_bytes(),
            })
            .collect::<Vec<_>>()
    });
    let assets = bundles.map(|bundle| {
        bundle
            .phrase_assets
            .iter()
            .map(|member| RuleBundleMember {
                path: &member.name,
                bytes: member.content.as_bytes(),
            })
            .collect::<Vec<_>>()
    });
    let sources = [0, 1].map(|i| RuleBundleSource {
        identity: &bundles[i].digest,
        entrypoints: &roots[i],
        rules: &rules[i],
        phrase_assets: &assets[i],
    });
    Ok(
        compile_rule_bundles(&sources, "", context()?, FrontendLimits::default())?
            .into_parts()
            .0,
    )
}
fn flatten(tuning: &Bundle, core: &Bundle) -> Fallible<String> {
    let mut text = String::new();
    for bundle in [tuning, core] {
        for root in &bundle.roots {
            let member = bundle
                .rules
                .iter()
                .find(|member| &member.name == root)
                .ok_or("missing root")?;
            if member
                .content
                .lines()
                .any(|line| line.trim_start().starts_with("Include "))
            {
                return Err("nested include requires an explicit inventory expansion".into());
            }
            text.push_str(&member.content);
            text.push('\n');
        }
    }
    Ok(text)
}
fn verify_provenance() -> Fallible<Value> {
    let bytes = fs::read(format!("{PACKAGE}/provenance.json"))?;
    if sha(&bytes) != "ca158245e3a70bd3b1a4029026a09a106dab3715d4562c17520657f49f4e08d7" {
        return Err("unexpected provenance revision".into());
    }
    let manifest: Value = serde_json::from_slice(&bytes)?;
    let files = manifest["files"].as_object().ok_or("missing manifest files")?;
    assert_eq!(files.len(), 50);
    for (name, digest) in files {
        assert_eq!(
            sha(&fs::read(format!("{PACKAGE}/upstream/{name}"))?),
            digest.as_str().ok_or("invalid digest")?
        );
    }
    let units = manifest["units"].as_array().ok_or("missing manifest units")?;
    assert_eq!(units.len(), 627);
    for unit in units {
        let file = fs::read(format!(
            "{PACKAGE}/upstream/rules/{}",
            unit["source"].as_str().ok_or("missing source")?
        ))?;
        let start = unit["start"].as_u64().ok_or("missing start")? as usize;
        let end = unit["end"].as_u64().ok_or("missing end")? as usize;
        assert_eq!(
            sha(file.get(start..end).ok_or("invalid unit span")?),
            unit["sha256"].as_str().ok_or("missing unit digest")?
        );
    }
    Ok(manifest)
}
fn projected(core: &Bundle, manifest: &Value) -> Fallible<Bundle> {
    let mut result = core.clone();
    for id in [921170, 921180] {
        let unit = manifest["units"]
            .as_array()
            .ok_or("missing units")?
            .iter()
            .find(|unit| unit["id"] == id)
            .ok_or("missing HPP unit")?;
        let name = unit["source"].as_str().ok_or("missing source")?;
        let source = fs::read_to_string(format!("{PACKAGE}/upstream/rules/{name}"))?;
        let raw = &source[unit["start"].as_u64().ok_or("missing start")? as usize
            ..unit["end"].as_u64().ok_or("missing end")? as usize];
        let member = result
            .rules
            .iter_mut()
            .find(|member| member.name == name)
            .ok_or("missing projected member")?;
        assert_eq!(member.content.matches(raw).count(), 1);
        member.content = member.content.replacen(raw, "", 1);
    }
    // Experimental identity binds every source byte and never impersonates the product digest.
    let binding = serde_json::to_vec(&json!({"roots":result.roots,
        "rules":result.rules.iter().map(|member| (&member.name, &member.content)).collect::<Vec<_>>(),
        "assets":result.phrase_assets.iter().map(|member| (&member.name, &member.content)).collect::<Vec<_>>()}))?;
    result.digest = format!("experiment-no-hpp-{}", sha(&binding));
    Ok(result)
}
fn expansion(value: MacroExpansionView<'_, '_>, site: &str) -> Value {
    json!({"site":site,"source":format!("{:?}",value.source()),"key":value.key().map(|key| key.text()),"body":value.body().text()})
}
fn template(value: OperandTemplateView<'_, '_>, site: &str) -> Vec<Value> {
    value
        .segments()
        .filter_map(|segment| match segment.kind() {
            TemplateSegmentKind::Expansion(value) => Some(expansion(value, site)),
            _ => None,
        })
        .collect()
}
fn slice_template(value: TemplateSliceView<'_, '_>, site: &str) -> Vec<Value> {
    value
        .segments()
        .filter_map(|segment| match segment.kind() {
            TemplateSegmentKind::Expansion(value) => Some(expansion(value, site)),
            _ => None,
        })
        .collect()
}
fn operator_operand(value: OperatorOperandView<'_, '_>) -> Value {
    match value {
        OperatorOperandView::NoArgument => Value::Null,
        OperatorOperandView::Regex(value) => json!({"regex":value.text()}),
        OperatorOperandView::StringTemplate(value) => json!({"template":value.text()}),
        OperatorOperandView::IntegerTemplate { template, constant } => {
            json!({"template":template.text(),"constant":constant})
        }
        OperatorOperandView::PhraseList(value) => {
            json!({"phrases":value.phrases().map(|value|value.text()).collect::<Vec<_>>()})
        }
        OperatorOperandView::ExternalAssets(value) => {
            json!({"assets":value.assets().map(|value|value.handle()).collect::<Vec<_>>()})
        }
        OperatorOperandView::IpNetworks(value) => {
            json!({"networks":value.networks().map(|value|(value.address().to_string(),value.prefix())).collect::<Vec<_>>()})
        }
        OperatorOperandView::ByteSet(value) => {
            json!({"bytes":(0..=255u8).filter(|byte|value.contains(*byte)).collect::<Vec<_>>()})
        }
    }
}
fn control(value: CtlView<'_, '_>) -> Value {
    match value {
        CtlView::AuditEngine(value) => json!({"audit_engine":format!("{value:?}")}),
        CtlView::ForceRequestBodyVariable(value) => json!({"force_request_body_variable":format!("{value:?}")}),
        CtlView::RequestBodyProcessor(value) => json!({"request_body_processor":format!("{value:?}")}),
        CtlView::RuleRemoveByTag(value) => json!({"remove_tag":value.text()}),
        CtlView::RuleRemoveTargetByTag { tag, target } => json!({"remove_target_tag":tag.text(),
            "collection":target.collection().as_str(),"selector_kind":format!("{:?}",target.selector_kind()),
            "selector":target.selector().map(|value|value.text()),"regex":target.regex_body().map(|value|value.text())}),
    }
}
fn supplements(core: &Bundle) -> Fallible<Vec<Value>> {
    let mut rows = Vec::new();
    for member in &core.rules {
        let source = scan_source(&member.name, member.content.as_bytes(), SourceLimits::default())?;
        let tokens = tokenize_document(&source, TokenizerLimits::default())?;
        let directives = parse_directives(&tokens, DirectiveLimits::default())?;
        for view in directives.iter() {
            let details = match view.directive() {
                Directive::SecRule { .. } | Directive::SecAction { .. } => continue,
                Directive::SecMarker { marker } => json!({"kind":"SecMarker","name":marker.value()}),
                Directive::SecComponentSignature { signature } => {
                    json!({"kind":"SecComponentSignature","signature":signature.value()})
                }
                Directive::SecRuleUpdateTargetById { selectors, targets } => json!({"kind":"SecRuleUpdateTargetById",
                    "selectors":selectors.iter().map(|value|value.value()).collect::<Vec<_>>(),"targets":targets.value()}),
                Directive::SecRuleRemoveById { .. } => return Err("unexpected supplemental removal".into()),
            };
            rows.push(json!({"source":member.name,"line":view.span().start.line,"details":details}));
        }
    }
    assert_eq!(rows.len(), 86);
    Ok(rows)
}
fn inventory(tuning: &Bundle, core: &Bundle, manifest: &Value) -> Fallible<Value> {
    let source = flatten(tuning, core)?;
    let scanned = scan_source("memory-cost-inventory", source.as_bytes(), SourceLimits::default())?;
    let syntax = parse_syntax(&scanned, SyntaxLimits::default())?;
    let draft = analyze_one_semantic_draft(syntax, SemanticLimits::default())?;
    let validated = validate_semantic_draft(draft, context()?, ValidationLimits::default())?;
    let compiled = compile(tuning, core)?;
    let semantic = validated.active_rule_units().collect::<Vec<_>>();
    let runtime = compiled.rule_units().collect::<Vec<_>>();
    assert_eq!(semantic.len(), runtime.len());
    let mut rows = Vec::new();
    for (unit, executable) in semantic.into_iter().zip(runtime) {
        assert_eq!(unit.id(), executable.id());
        assert_eq!(unit.members().count(), executable.members().len());
        let mut members = Vec::new();
        let mut flags = BTreeMap::<String, bool>::new();
        for (index, (member, plan)) in unit.members().zip(executable.members()).enumerate() {
            let targets = member.final_targets().map(|target| json!({
                "collection":target.collection().as_str(),"modifier":format!("{:?}",target.modifier()),
                "selector_kind":format!("{:?}",target.selector_kind()),"selector":target.selector().map(|value|value.text()),
                "regex":target.regex_body().map(|value|value.text()),
            })).collect::<Vec<_>>();
            let compiled_targets =
                plan.target_plan()
                    .map(|plan| {
                        plan.records().map(|target| json!({
                "collection":target.collection().as_str(),"modifier":format!("{:?}",target.modifier()),
                "selector_kind":format!("{:?}",target.selector_kind()),
                "selector":target.selector_bytes().map(String::from_utf8_lossy),
                "regex_program_id":target.selector_regex_program_id(),
            })).collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
            assert_eq!(targets.len(), compiled_targets.len());
            for (left, right) in targets.iter().zip(&compiled_targets) {
                assert_eq!(left["collection"], right["collection"]);
                assert_eq!(left["modifier"], right["modifier"]);
                if left["regex"].is_null() {
                    assert_eq!(left["selector_kind"], right["selector_kind"]);
                    assert_eq!(left["selector"], right["selector"]);
                } else {
                    assert_eq!(right["selector_kind"], "Regex");
                    assert!(!right["regex_program_id"].is_null());
                }
            }
            let mut macros = member
                .operator()
                .and_then(|op| op.template())
                .map(|value| template(value, "operator"))
                .unwrap_or_default();
            let mut actions = Vec::new();
            for action in member.actions() {
                let name = action.name().text();
                let operand = match action.operand() {
                    ActionOperandView::Template(value) => {
                        macros.extend(template(value, name));
                        json!({"text":value.text()})
                    }
                    ActionOperandView::SetVar(value) => {
                        macros.extend(slice_template(value.key(), "setvar_key"));
                        if let Some(value) = value.value() {
                            macros.extend(slice_template(value, "setvar_value"));
                        }
                        if value.key().expansion_count() != 0 {
                            flags.insert("dynamic_tx_key".into(), true);
                        }
                        json!({"collection":value.collection().as_str(),"operation":format!("{:?}",value.operation()),
                            "key":value.key().text(),"value":value.value().map(|value|value.text())})
                    }
                    ActionOperandView::NoArgument => Value::Null,
                    ActionOperandView::Local(value) => json!({"local":format!("{value:?}")}),
                    ActionOperandView::Transformation(value) => json!({"transformation":value.as_str()}),
                    ActionOperandView::Literal(value) => json!({"literal":value.text()}),
                    ActionOperandView::Ctl(value) => control(value),
                    ActionOperandView::InitCol(value) => {
                        json!({"collection":format!("{:?}",value.collection()),"key":value.key().text()})
                    }
                };
                if action.kind() == ActionEnvelopeKind::Action(ActionKind::Capture) {
                    flags.insert("capture".into(), true);
                }
                if action.kind() == ActionEnvelopeKind::Action(ActionKind::MultiMatch) {
                    flags.insert("multi_match".into(), true);
                }
                actions.push(json!({"name":name,"operand":operand}));
            }
            let transforms = member
                .effective_transformations()
                .filter_map(|action| action.transformation())
                .collect::<Vec<_>>();
            assert_eq!(transforms.as_slice(), plan.transformations().unwrap_or_default());
            assert_eq!(member.operator().map(|op| op.kind()), plan.operator_kind());
            assert_eq!(member.operator().map(|op| op.negated()), plan.operator_negated());
            if let Some(marker) = member.skip_after_marker() {
                let sentinel = compiled
                    .phase_schedule(executable.phase())
                    .nth(plan.skip_after_schedule_index().ok_or("missing skip destination")?)
                    .ok_or("invalid skip destination")?;
                assert_eq!(sentinel.kind(), CompiledScheduleEntryKind::MarkerSentinel);
                assert_eq!(sentinel.ordinal(), marker.ordinal());
            } else {
                assert!(plan.skip_after_schedule_index().is_none());
            }
            if transforms.iter().any(|value| {
                !matches!(
                    value,
                    TransformationKind::None | TransformationKind::Lowercase | TransformationKind::Length
                )
            }) {
                flags.insert("nontrivial_transform".into(), true);
            }
            for target in member.final_targets() {
                if target.modifier() == TargetModifier::Exclude {
                    continue;
                }
                let flag = match target.collection() {
                    CollectionKind::Args => "args_values",
                    CollectionKind::ArgsNames => "args_names",
                    CollectionKind::Xml => "xml_targets",
                    CollectionKind::RequestBody => "raw_body_targets",
                    CollectionKind::MatchedVars => "matched_vars",
                    _ => continue,
                };
                flags.insert(flag.into(), true);
                if matches!(target.collection(), CollectionKind::Args | CollectionKind::ArgsNames)
                    && target.selector_kind() == SelectorKind::None
                    && target.modifier() != TargetModifier::Count
                {
                    flags.insert(format!("broad_{flag}"), true);
                }
            }
            if member
                .operator()
                .and_then(|op| op.template())
                .is_some_and(|value| value.expansion_count() != 0)
            {
                flags.insert("dynamic_operator".into(), true);
            }
            if macros.iter().any(|value| value["source"] == "Args") {
                flags.insert("args_macro".into(), true);
            }
            if macros
                .iter()
                .any(|value| matches!(value["source"].as_str(), Some("MatchedVar" | "MatchedVarName")))
            {
                flags.insert("matched_macro".into(), true);
            }
            members.push(json!({"index":index,"targets":targets,"compiled_targets":compiled_targets,
                "transforms":transforms.iter().map(|value|value.as_str()).collect::<Vec<_>>(),"actions":actions,"macros":macros,
                "operator":member.operator().map(|op|json!({"kind":op.kind().as_str(),"negated":op.negated(),"operand":operator_operand(op.operand())})),
                "skip_after_marker_ordinal":member.skip_after_marker().map(|marker|marker.ordinal()),"skip_after_schedule_index":plan.skip_after_schedule_index(),"target_plan_id":plan.target_plan_id(),"transformation_plan_id":plan.transformation_plan_id()}));
        }
        assert_eq!(members.len(), executable.members().len());
        let original = manifest["units"]
            .as_array()
            .ok_or("missing manifest units")?
            .iter()
            .find(|value| value["id"] == unit.id())
            .cloned();
        rows.push(json!({"id":unit.id(),"phase":executable.phase() as usize,"schedule_index":executable.schedule_index(),
            "execution_class":format!("{:?}",executable.request_body_execution_class()),"source":original,
            "extra_source":if unit.id()==900990 {Some("crs-setup.conf.example:946")} else if unit.id()==9900301 {Some("tuning.conf")} else {None},
            "demand_flags":flags,"members":members}));
    }
    let updates = validated
        .applied_target_updates()
        .map(|update| update.affected_units().map(|unit| unit.id()).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    assert_eq!(updates.len(), 55);
    assert!(updates.iter().flatten().all(|id| ![921170, 921180].contains(id)));
    Ok(
        json!({"units":rows,"markers":compiled.marker_count(),"matcher_groups":compiled.matcher_group_count(),
        "supplemental_directives":supplements(core)?,"applied_target_updates":updates,
        "core_identity":core.digest,"tuning_identity":tuning.digest}),
    )
}

struct Fixture {
    name: String,
    xml: bool,
    body: Bytes,
    length: String,
    uri: String,
    chunk: usize,
    detection: bool,
}
impl Fixture {
    fn new(name: &str, xml: bool, body: String) -> Self {
        Self {
            name: name.into(),
            xml,
            length: body.len().to_string(),
            body: Bytes::from(body),
            uri: "/".into(),
            chunk: usize::MAX,
            detection: false,
        }
    }
}
fn fixtures() -> Vec<Fixture> {
    let mut cases = vec![Fixture::new("json-small", false, r#"{"probe":"safe"}"#.into())];
    for width in [64, 256, 10_000] {
        cases.push(Fixture::new(
            &format!("json-width-{width}"),
            false,
            format!(
                "{{{}}}",
                (0..width)
                    .map(|i| format!("\"k{i:04}\":\"safe\""))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        ));
    }
    for depth in [8, 32, 65] {
        cases.push(Fixture::new(
            &format!("json-depth-{depth}"),
            false,
            format!("{}\"safe\"{}", "{\"a\":".repeat(depth), "}".repeat(depth)),
        ));
    }
    for length in [32, 256] {
        cases.push(Fixture::new(
            &format!("json-key-{length}"),
            false,
            format!("{{\"{}\":\"safe\"}}", "k".repeat(length)),
        ));
    }
    for length in [1024, 16 * 1024] {
        cases.push(Fixture::new(
            &format!("json-value-{length}"),
            false,
            format!("{{\"probe\":\"{}\"}}", "x".repeat(length)),
        ));
        cases.push(Fixture::new(
            &format!("json-escaped-{length}"),
            false,
            format!("{{\"probe\":\"{}\"}}", "\\u0078".repeat(length)),
        ));
    }
    cases.extend([
        Fixture::new(
            "json-siblings",
            false,
            format!(
                "[{}]",
                (0..16)
                    .map(|_| "{\"a\":\"safe\",\"b\":\"safe\"}")
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        ),
        Fixture::new("json-hit", false, r#"{"probe":"phar://payload"}"#.into()),
        Fixture::new(
            "json-duplicate-replacement",
            false,
            r#"{"a":"phar://payload","\u0061":"safe"}"#.into(),
        ),
        Fixture::new(
            "json-subtree-replacement",
            false,
            r#"{"a":{"b":"phar://payload"},"a":"safe"}"#.into(),
        ),
        Fixture::new(
            "json-dotted-collision",
            false,
            r#"{"a":{"b":"safe"},"a.b":"safe"}"#.into(),
        ),
        Fixture::new(
            "json-named-macros",
            false,
            r#"{"_method":"POST","_charset_":"utf-8"}"#.into(),
        ),
        Fixture::new("json-malformed", false, r#"{"a":"\uD800"}"#.into()),
        Fixture::new("xml-small", true, "<r a='safe'>safe</r>".into()),
        Fixture::new(
            "xml-attributes-64",
            true,
            format!(
                "<r {}/>",
                (0..64).map(|i| format!("k{i}='safe'")).collect::<Vec<_>>().join(" ")
            ),
        ),
        Fixture::new(
            "xml-namespaces",
            true,
            "<r xmlns:a='urn:example:a' xmlns:b='urn:example:b'><a:n a:k='safe'/><b:n b:k='safe'/></r>".into(),
        ),
        Fixture::new("xml-duplicate-attribute", true, "<r a='1' a='2'/>".into()),
        Fixture::new(
            "xml-expanded-duplicate",
            true,
            "<r xmlns:a='urn:example' xmlns:b='urn:example' a:x='1' b:x='2'/>".into(),
        ),
        Fixture::new("xml-value-16384", true, format!("<r>{}</r>", "x".repeat(16 * 1024))),
    ]);
    let mut hpp = Fixture::new("query-hpp", false, "{}".into());
    hpp.uri = "/?a=1&a=2".into();
    hpp.detection = true;
    cases.push(hpp);
    let mut split = Fixture::new(
        "json-incremental",
        false,
        r#"{"a":"safe","probe":"phar://payload"}"#.into(),
    );
    split.chunk = 7;
    cases.push(split);
    let mut split = Fixture::new("xml-incremental", true, "<r a='safe'><a>phar://payload</a></r>".into());
    split.chunk = 7;
    cases.push(split);
    let mut detection = Fixture::new("json-malformed-detection", false, "{\"a\":".into());
    detection.detection = true;
    cases.push(detection);
    cases
}
#[derive(Default, Serialize)]
struct InspectionObservation {
    current_heap_at_eof: Option<usize>,
    result_heap_at_eof: usize,
    result_work: usize,
    raw_source_required: bool,
}
struct Run {
    allocation: Allocation,
    phases: [Option<PhaseOutcome>; 5],
    error: Option<RuntimeError>,
    body_error: Option<BodyError>,
    logical_retained: usize,
    inspection: InspectionObservation,
    tx_values: [[u8; 32]; 4],
    tx_lengths: [usize; 4],
}
fn execute(generation: &CompiledGeneration, fixture: &Fixture) -> Run {
    let headers = [
        RequestHeader::new(b"host", b"example.test"),
        RequestHeader::new(
            b"content-type",
            if fixture.xml {
                b"application/xml"
            } else {
                b"application/json"
            },
        ),
        RequestHeader::new(b"content-length", fixture.length.as_bytes()),
        RequestHeader::new(b"user-agent", b"WAF memory experiment"),
        RequestHeader::new(b"accept", b"*/*"),
    ];
    let head = RequestHead::new(
        b"POST / HTTP/1.1",
        b"POST",
        fixture.uri.as_bytes(),
        fixture.uri.as_bytes(),
        b"HTTP/1.1",
        b"192.0.2.1",
        b"memory-experiment",
        &headers,
    );
    let mut phases: [Option<PhaseOutcome>; 5] = std::array::from_fn(|_| None);
    let mut inspection = InspectionObservation::default();
    let mut body_error = None;
    let mut logical_retained = 0;
    let mut tx_values = [[0; 32]; 4];
    let mut tx_lengths = [0; 4];
    let stage = Stage::start();
    let execution = (|| -> Result<(), RuntimeError> {
        let mut tx = Transaction::new_with_modes(
            generation,
            head,
            RuntimeLimits::default(),
            EvaluationMode::Accelerated,
            if fixture.detection {
                RuleEngineMode::DetectionOnly
            } else {
                RuleEngineMode::On
            },
        )?;
        let result = (|| -> Result<(), RuntimeError> {
            phases[0] = Some(tx.execute_request_headers()?);
            if tx.next_phase() != Some(Phase::RequestBody) {
                return Ok(());
            }
            if fixture.xml {
                let mut session = tx.start_xml_inspection(XmlInspectionLimits::default(), Some(fixture.body.len()))?;
                for chunk in fixture.body.chunks(fixture.chunk) {
                    session.feed(chunk)?;
                }
                session.complete(fixture.body.len())?;
                let usage = session.usage();
                inspection.current_heap_at_eof = usage.current_heap_bytes();
                inspection.result_heap_at_eof = usage.result_heap_bytes;
                inspection.result_work = usage.result_work;
                inspection.raw_source_required = session.requires_raw_source();
                session.finish(inspection.raw_source_required.then(|| fixture.body.clone()))?;
            } else {
                let mut session =
                    tx.start_json_inspection(JsonInspectionLimits::default(), Some(fixture.body.len()))?;
                for chunk in fixture.body.chunks(fixture.chunk) {
                    session.feed(chunk)?;
                }
                session.complete(fixture.body.len())?;
                let usage = session.usage();
                inspection.current_heap_at_eof = usage.current_heap_bytes();
                inspection.result_heap_at_eof = usage.result_heap_bytes;
                inspection.result_work = usage.result_work;
                inspection.raw_source_required = session.requires_raw_source();
                session.finish(inspection.raw_source_required.then(|| fixture.body.clone()))?;
            }
            phases[1] = Some(tx.execute_request_body()?);
            if tx.next_phase() == Some(Phase::ResponseHeaders) {
                tx.publish_response_head(ResponseHead::new(200, &[]))?;
                phases[2] = Some(tx.execute_response_headers()?);
            }
            if tx.next_phase() == Some(Phase::ResponseBody) {
                phases[3] = Some(tx.execute_response_window(ResponseWindow::new(&[], true))?);
            }
            if tx.next_phase() == Some(Phase::Logging) {
                phases[4] = Some(tx.execute_logging()?);
            }
            Ok(())
        })();
        body_error = tx.request_body_error();
        logical_retained = tx.retained_bytes();
        for (i, key) in [
            b"paramcounter_ARGS_NAMES:a".as_slice(),
            b"inbound_anomaly_score_pl3",
            b"blocking_inbound_anomaly_score",
            b"http_violation_score",
        ]
        .iter()
        .enumerate()
        {
            if let Some(value) = tx.tx_value(key) {
                assert!(value.len() <= 32);
                tx_values[i][..value.len()].copy_from_slice(value);
                tx_lengths[i] = value.len();
            }
        }
        result
        // Transaction drops before closing the continuous allocation span.
    })();
    let allocation = stage.finish();
    Run {
        allocation,
        phases,
        error: execution.err(),
        body_error,
        logical_retained,
        inspection,
        tx_values,
        tx_lengths,
    }
}
fn snapshot(run: &Run) -> Value {
    json!({"phases":run.phases.iter().map(|phase|phase.as_ref().map(|phase| json!({
        "findings":phase.findings().iter().map(|finding|finding.rule_id()).collect::<Vec<_>>(),
        "intervention":phase.intervention().map(|value|(value.rule_id(),value.status())),
        "evaluated_units":phase.evaluated_units(),"omitted_findings":phase.omitted_findings()
    }))).collect::<Vec<_>>(),"error":run.error.map(|error|format!("{error:?}")),"body_error":run.body_error.map(|error|format!("{error:?}")),
        "tx_keys":["paramcounter_ARGS_NAMES:a","inbound_anomaly_score_pl3","blocking_inbound_anomaly_score","http_violation_score"],"tx":run.tx_values.iter().zip(run.tx_lengths).map(|(value,length)|String::from_utf8_lossy(&value[..length])).collect::<Vec<_>>()})
}
fn measure(
    output: &Path,
    standard: &Bundle,
    strict: &Bundle,
    core: &Bundle,
    projection: &Bundle,
    reverse: bool,
) -> Fallible<()> {
    let mut configurations = vec![
        ("standard", standard, core),
        ("standard-no-hpp", standard, projection),
        ("strict", strict, core),
        ("strict-no-hpp", strict, projection),
    ];
    let mut cases = fixtures();
    if reverse {
        configurations.reverse();
        cases.reverse();
    }
    let mut rows = Vec::new();
    for (name, tuning, source) in configurations {
        let stage = Stage::start();
        let generation = compile(tuning, source)?;
        let allocation = stage.finish();
        rows.push(json!({"kind":"compile","profile":name,"allocation":allocation,"logical_retained_bytes":generation.retained_bytes(),
            "logical_compile_peak_bytes":generation.compilation_peak_bytes(),"units":generation.rule_unit_count(),"markers":generation.marker_count()}));
        for fixture in &cases {
            let first = execute(&generation, fixture);
            let expected = snapshot(&first);
            let mut samples = Vec::new();
            if first.error.is_none() {
                for _ in 0..3 {
                    assert_eq!(snapshot(&execute(&generation, fixture)), expected);
                }
                for _ in 0..31 {
                    let run = execute(&generation, fixture);
                    assert_eq!(snapshot(&run), expected);
                    samples.push(run.allocation);
                }
            } else {
                samples.push(first.allocation);
            }
            rows.push(
                json!({"kind":"request","profile":name,"fixture":fixture.name,"body_bytes":fixture.body.len(),
                "chunk_bytes":fixture.chunk,"mode":if fixture.detection {"DetectionOnly"} else {"On"},"samples":samples,
                "outcome":expected,"inspection":first.inspection,"logical_retained_bytes":first.logical_retained,
                "first_probe_allocation":first.allocation,"post_drop_live_bytes":first.allocation.live_delta_bytes}),
            );
        }
    }
    fs::write(output, serde_json::to_vec_pretty(&rows)?)?;
    Ok(())
}

fn main() -> Fallible<()> {
    let mut args = std::env::args().skip(1);
    let mode = args
        .next()
        .ok_or("usage: waf_rule_memory_costs inventory|measure output-path [reverse]")?;
    let output = args.next().ok_or("missing output path")?;
    let manifest = verify_provenance()?;
    let core = load("core")?;
    let standard = load("standard")?;
    let strict = load("strict")?;
    let projection = projected(&core, &manifest)?;
    if mode == "inventory" {
        let full = inventory(&standard, &core, &manifest)?;
        let reduced = inventory(&standard, &projection, &manifest)?;
        let units = full["units"].as_array().ok_or("missing inventory units")?;
        assert_eq!(units.len(), 567);
        assert_eq!(
            units
                .iter()
                .map(|unit| unit["members"].as_array().map_or(0, Vec::len))
                .sum::<usize>(),
            634
        );
        let retained = reduced["units"].as_array().ok_or("missing reduced units")?;
        assert_eq!(retained.len(), 565);
        let mut left = units.iter().filter(|unit| unit["id"] != 921170 && unit["id"] != 921180);
        for unit in retained {
            let expected = left.next().ok_or("unexpected projected unit")?;
            assert_eq!(expected["id"], unit["id"]);
            for (a, b) in expected["members"]
                .as_array()
                .ok_or("missing members")?
                .iter()
                .zip(unit["members"].as_array().ok_or("missing members")?)
            {
                for key in [
                    "targets",
                    "transforms",
                    "actions",
                    "macros",
                    "operator",
                    "skip_after_marker_ordinal",
                ] {
                    assert_eq!(a[key], b[key], "retained member changed");
                }
            }
        }
        assert!(left.next().is_none());
        assert_eq!(full["markers"], reduced["markers"]);
        for (left, right) in full["supplemental_directives"]
            .as_array()
            .ok_or("missing supplements")?
            .iter()
            .zip(
                reduced["supplemental_directives"]
                    .as_array()
                    .ok_or("missing supplements")?,
            )
        {
            assert_eq!(left["source"], right["source"]);
            assert_eq!(left["details"], right["details"]);
        }
        assert_eq!(full["applied_target_updates"], reduced["applied_target_updates"]);
        fs::write(
            output,
            serde_json::to_vec_pretty(&json!({"full":full,"projection":reduced,
            "capability_exclusions":manifest["units"].as_array().ok_or("missing units")?.iter().filter(|unit|unit["excluded_capabilities"].as_array().is_some_and(|v|!v.is_empty())).collect::<Vec<_>>(),
            "projection_binding":projection.digest}))?,
        )?;
    } else if mode == "measure" {
        measure(
            Path::new(&output),
            &standard,
            &strict,
            &core,
            &projection,
            args.next().as_deref() == Some("reverse"),
        )?;
    } else {
        return Err("unknown mode".into());
    }
    Ok(())
}
