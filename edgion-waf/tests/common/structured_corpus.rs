//! Shared test-only original-rule fixtures; no external corpus or task files required.
use std::collections::BTreeMap;

use bytes::Bytes;
use edgion_waf::seclang::{
    compile_with_includes, BodyError, BodyProcessorSelection, CompiledGeneration, EvaluationMode, FrontendLimits,
    IncludeFailure, IncludeRequest, IncludeSink, IncludeSource, JsonInspectionLimits, Phase, PhaseDefaults,
    PhaseOutcome, PhraseAssetFailure, PhraseAssetRequest, PhraseAssetSink, PhraseAssetSource, RequestBody, RequestHead,
    RequestHeader, RuleCheckPolicy, RuleEngineMode, RuntimeError, RuntimeLimits, SemanticValidationContext,
    Transaction, XmlInspectionLimits,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub const UNITS_JSON: &str = include_str!("../fixtures/crs-structured-v4.25.1/units.json");
pub const UNITS_SHA256: &str = "a73ed220e43cd5761842e5a1a587369fad7cd40eab424d5281fb9ecdf4fbe81b";
pub const SETUP: &str = concat!(
    "SecAction \"id:9900001,phase:1,pass,nolog,noauditlog,",
    "setvar:tx.blocking_paranoia_level=4,setvar:tx.detection_paranoia_level=4,",
    "setvar:tx.crs_validate_utf8_encoding=1,setvar:tx.crs_xml_attr_inspect=1,",
    "setvar:tx.arg_name_length=100,setvar:tx.arg_length=400,",
    "setvar:tx.total_arg_length=64000,setvar:tx.max_num_args=255,",
    "setvar:tx.critical_anomaly_score=5,setvar:tx.error_anomaly_score=4,",
    "setvar:tx.warning_anomaly_score=3,setvar:tx.notice_anomaly_score=2,",
    "setvar:tx.allow_method_override_parameter=0,setvar:tx.crs_setup_version=4251\"\n",
    // A separate witness proves mixed REQUEST_BODY targets cannot supply valid structured hits.
    "SecRule &REQUEST_BODY \"@eq 0\" \"id:9900002,phase:2,pass,nolog,noauditlog,",
    "t:none,setvar:tx.supplement_raw_absent=1\"\n",
    "SecRule &ARGS \"@eq 0\" \"id:9900003,phase:2,pass,nolog,noauditlog,",
    "t:none,setvar:tx.supplement_args_absent=1\"\n",
    "SecRule &XML \"@eq 0\" \"id:9900004,phase:2,pass,nolog,noauditlog,",
    "t:none,setvar:tx.supplement_xml_absent=1\"\n",
);

pub const ASSETS: &[(&str, &str)] = &[
    (
        "lfi-os-files.data",
        include_str!("../fixtures/crs-structured-v4.25.1/lfi-os-files.data"),
    ),
    (
        "windows-powershell-commands.data",
        include_str!("../fixtures/crs-structured-v4.25.1/windows-powershell-commands.data"),
    ),
    (
        "unix-shell.data",
        include_str!("../fixtures/crs-structured-v4.25.1/unix-shell.data"),
    ),
    (
        "php-variables.data",
        include_str!("../fixtures/crs-structured-v4.25.1/php-variables.data"),
    ),
    (
        "php-function-names-933150.data",
        include_str!("../fixtures/crs-structured-v4.25.1/php-function-names-933150.data"),
    ),
    (
        "ssrf.data",
        include_str!("../fixtures/crs-structured-v4.25.1/ssrf.data"),
    ),
    (
        "ssrf-no-scheme.data",
        include_str!("../fixtures/crs-structured-v4.25.1/ssrf-no-scheme.data"),
    ),
    (
        "java-classes.data",
        include_str!("../fixtures/crs-structured-v4.25.1/java-classes.data"),
    ),
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Corpus {
    pub schema: String,
    pub upstream: String,
    pub revision: String,
    pub units: Vec<Unit>,
    pub assets: Vec<Asset>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unit {
    pub id: u64,
    pub source: String,
    pub start_line: usize,
    pub end_line: usize,
    pub source_sha256: String,
    pub raw_sha256: String,
    pub raw: String,
    pub xml_target: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    pub name: String,
    pub sha256: String,
}

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn corpus() -> Corpus {
    assert_eq!(
        sha256(UNITS_JSON.as_bytes()),
        UNITS_SHA256,
        "review original fixture edits"
    );
    let corpus: Corpus = serde_json::from_str(UNITS_JSON).expect("pinned original fixture schema");
    assert_eq!(corpus.schema, "edgion-waf-structured-original-units/v1");
    assert_eq!(
        corpus.upstream,
        "https://github.com/coreruleset/coreruleset/tree/v4.25.1"
    );
    assert_eq!(corpus.revision, "3b89d5a05322f448b4b74b9cadc5fb05ac6915ad");
    assert_eq!(corpus.units.len(), 195);
    assert_eq!(corpus.units.iter().filter(|unit| unit.xml_target).count(), 173);
    assert_eq!(corpus.assets.len(), ASSETS.len());
    for unit in &corpus.units {
        assert!(!unit.source.is_empty() && unit.start_line <= unit.end_line);
        assert_eq!(unit.source_sha256.len(), 64);
        assert_eq!(
            sha256(unit.raw.as_bytes()),
            unit.raw_sha256,
            "original unit {}",
            unit.id
        );
    }
    for (asset, (name, content)) in corpus.assets.iter().zip(ASSETS) {
        assert_eq!(asset.name, *name);
        assert_eq!(asset.sha256, sha256(content.as_bytes()));
    }
    corpus
}

struct NoIncludes;
impl IncludeSource for NoIncludes {
    fn load(&mut self, _: IncludeRequest<'_>, _: &mut IncludeSink<'_>) -> Result<(), IncludeFailure> {
        Err(IncludeFailure::NotFound)
    }
}

struct PhraseAssets;
impl PhraseAssetSource for PhraseAssets {
    fn load(
        &mut self,
        request: PhraseAssetRequest<'_>,
        sink: &mut PhraseAssetSink<'_>,
    ) -> Result<(), PhraseAssetFailure> {
        let (_, content) = ASSETS
            .iter()
            .find(|(name, _)| *name == request.handle())
            .ok_or(PhraseAssetFailure::NotFound)?;
        sink.extend_from_slice(content.as_bytes())
            .map_err(|_| PhraseAssetFailure::Other)
    }
}

pub fn compile(raw_units: &str) -> CompiledGeneration {
    let source = format!("{SETUP}{raw_units}");
    let defaults = [
        PhaseDefaults::pass(Phase::RequestHeaders, false, false),
        PhaseDefaults::try_deny(Phase::RequestBody, 403, true, true).expect("valid test deny default"),
    ];
    let context = SemanticValidationContext::try_new(Some(Phase::RequestBody), &defaults).expect("unique test phases");
    let (generation, compatibility) = compile_with_includes(
        "structured-original",
        source.as_bytes(),
        &mut NoIncludes,
        Some(&mut PhraseAssets),
        context,
        FrontendLimits::default(),
    )
    .expect("unchanged original units must strictly compile")
    .into_parts();
    assert!(compatibility.is_supported());
    assert_eq!(compatibility.omitted_findings(), 0);
    assert_eq!(generation.json_inspection_unsupported().len(), 0);
    assert_eq!(generation.xml_inspection_unsupported().len(), 0);
    generation
}

#[derive(Debug, PartialEq, Eq)]
pub struct Observation {
    pub outcome: PhaseOutcome,
    pub tx: BTreeMap<String, Vec<u8>>,
    pub body_error: Option<BodyError>,
}

#[derive(Clone, Copy)]
pub struct Input<'a> {
    pub body: &'a [u8],
    pub xml: bool,
    pub referer: Option<&'a str>,
}

pub fn observe(
    generation: &CompiledGeneration,
    input: Input<'_>,
    mode: RuleEngineMode,
    evaluation: EvaluationMode,
    chunked: bool,
    extra_tx: &[String],
) -> Result<Observation, RuntimeError> {
    let Input { body, xml, referer } = input;
    let content_length = body.len().to_string();
    let mut headers = vec![
        RequestHeader::new(
            b"content-type",
            if xml { b"application/xml" } else { b"application/json" },
        ),
        RequestHeader::new(b"content-length", content_length.as_bytes()),
        RequestHeader::new(b"host", b"structured.test"),
    ];
    if let Some(referer) = referer {
        headers.push(RequestHeader::new(b"referer", referer.as_bytes()));
    }
    let head = RequestHead::new(
        b"POST /body HTTP/1.1",
        b"POST",
        b"/body",
        b"/body",
        b"HTTP/1.1",
        b"192.0.2.1",
        b"structured.test",
        &headers,
    );
    let mut tx = Transaction::new_with_policy(
        generation,
        head,
        RuntimeLimits::default(),
        evaluation,
        mode,
        RuleCheckPolicy::default(),
    )?;
    tx.execute_request_headers()?;
    let processor = if xml {
        BodyProcessorSelection::Xml
    } else {
        BodyProcessorSelection::Json
    };
    assert_eq!(tx.preflight_request_body(BodyProcessorSelection::Auto)?, processor);
    let publication = (|| -> Result<(), RuntimeError> {
        if chunked {
            // One-byte feeds split escapes, UTF-8, delimiters and identifiers; empty chunks preserve parity.
            if xml {
                let mut session = tx.start_xml_inspection(XmlInspectionLimits::default(), Some(body.len()))?;
                for chunk in body.chunks(1) {
                    session.feed(chunk)?;
                }
                session.feed(b"")?;
                session.complete(body.len())?;
                let source = session.requires_raw_source().then(|| Bytes::copy_from_slice(body));
                session.finish(source)?;
            } else {
                let mut session = tx.start_json_inspection(JsonInspectionLimits::default(), Some(body.len()))?;
                for chunk in body.chunks(1) {
                    session.feed(chunk)?;
                }
                session.feed(b"")?;
                session.complete(body.len())?;
                let source = session.requires_raw_source().then(|| Bytes::copy_from_slice(body));
                session.finish(source)?;
            }
        } else {
            tx.publish_request_body(RequestBody::new(
                Bytes::copy_from_slice(body),
                BodyProcessorSelection::Auto,
            ))?;
        }
        Ok(())
    })();
    if let Err(error) = publication {
        assert_eq!(tx.tx_value(b"supplement_raw_absent"), None);
        assert_eq!(tx.tx_value(b"rce_score"), None);
        assert_eq!(tx.tx_value(b"php_injection_score"), None);
        return Err(error);
    }
    let outcome = tx.execute_request_body()?;
    let mut keys = vec![
        "supplement_raw_absent",
        "supplement_args_absent",
        "supplement_xml_absent",
        "rce_score",
        "php_injection_score",
        "inbound_anomaly_score_pl1",
        "inbound_anomaly_score_pl2",
        "inbound_anomaly_score_pl3",
        "inbound_anomaly_score_pl4",
    ];
    keys.extend(extra_tx.iter().map(String::as_str));
    let mut values = BTreeMap::new();
    for key in keys {
        if let Some(value) = tx.tx_value(key.as_bytes()) {
            values.insert(key.to_owned(), value.to_vec());
        }
    }
    Ok(Observation {
        outcome,
        tx: values,
        body_error: tx.request_body_error(),
    })
}
