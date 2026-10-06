//! Owned XML engine harness shared with the frozen pre-streaming snapshot.
//! Compilation and input construction are excluded. Setup, headers, publication,
//! phase 2 and transaction destruction are included. This is not Gateway IO/RSS.
use bytes::Bytes;
use edgion_waf::seclang::*;
use sha2::{Digest, Sha256};
use std::time::Duration;
struct NoIncludes;
impl IncludeSource for NoIncludes {
    fn load(&mut self, _: IncludeRequest<'_>, _: &mut IncludeSink<'_>) -> Result<(), IncludeFailure> {
        Err(IncludeFailure::NotFound)
    }
}
pub(crate) fn compile(subscribers: usize, raw: bool, controls: bool, captures: bool) -> CompiledGeneration {
    let mut rules = String::new();
    if raw {
        rules.push_str("SecAction \"id:8000,phase:1,pass,ctl:forceRequestBodyVariable=On\"\n");
    }
    if controls {
        rules.push_str("SecAction \"id:901181,phase:2,pass,ctl:ruleRemoveTargetByTag=xml-check;XML://@*\"\n");
    }
    for id in 1..=subscribers {
        if captures {
            rules.push_str(&format!("SecRule XML:/*|XML://@* \"@rx (hit)\" \"id:{id},phase:2,pass,t:lowercase,capture,tag:xml-check,setvar:tx.hit=%{{TX.1}},chain\"\nSecRule TX:hit \"@streq hit\" \"setvar:tx.done=yes\"\n"));
        } else {
            rules.push_str(&format!(
                "SecRule XML:/*|XML://@* \"@pm hit\" \"id:{id},phase:2,pass,t:lowercase,tag:xml-check\"\n"
            ));
        }
    }
    if raw {
        rules.push_str("SecRule REQUEST_BODY \"@contains HIT\" \"id:8001,phase:2,pass,setvar:tx.raw=yes\"\n");
    }
    let phases = [
        PhaseDefaults::pass(Phase::RequestHeaders, true, true),
        PhaseDefaults::pass(Phase::RequestBody, true, true),
    ];
    compile_with_includes(
        "xml-benchmark",
        rules.as_bytes(),
        &mut NoIncludes,
        None,
        SemanticValidationContext::try_new(Some(Phase::RequestBody), &phases).unwrap(),
        FrontendLimits::default(),
    )
    .unwrap()
    .into_parts()
    .0
}
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub(crate) outcome: Result<PhaseOutcome, RuntimeError>,
    pub(crate) next: Option<Phase>,
    pub(crate) error: Option<BodyError>,
    pub(crate) logical: usize,
    hit: Option<Vec<u8>>,
    done: Option<Vec<u8>>,
    raw: Option<Vec<u8>>,
}
pub(crate) fn execute(
    g: &CompiledGeneration,
    body: &Bytes,
    mode: RuleEngineMode,
    evaluation: EvaluationMode,
) -> Snapshot {
    let headers = [RequestHeader::new(b"content-type", b"application/xml")];
    let head = RequestHead::new(
        b"POST /?q=miss HTTP/1.1",
        b"POST",
        b"/?q=miss",
        b"/?q=miss",
        b"HTTP/1.1",
        b"192.0.2.1",
        b"benchmark",
        &headers,
    );
    let mut tx = Transaction::new_with_modes(g, head, RuntimeLimits::default(), evaluation, mode).unwrap();
    tx.execute_request_headers().unwrap();
    let outcome = tx
        .publish_request_body(RequestBody::new(body.clone(), BodyProcessorSelection::Auto))
        .and_then(|()| tx.execute_request_body());
    snapshot(&tx, outcome)
}
impl Snapshot {
    pub(crate) fn semantics(&self) -> String {
        format!(
            "{:?}",
            (&self.outcome, self.next, self.error, &self.hit, &self.done, &self.raw)
        )
    }
    pub(crate) fn assert_case(&self, name: &str, mode: RuleEngineMode) {
        if name == "malformed_tail" && mode == RuleEngineMode::On {
            assert!(
                matches!(
                    self.outcome,
                    Err(RuntimeError::Body {
                        kind: BodyError::Malformed {
                            processor: BodyProcessorSelection::Xml
                        }
                    })
                ),
                "{name}: {:?}",
                self.outcome
            );
        } else {
            assert!(self.outcome.is_ok(), "{name}: {:?}", self.outcome);
            assert_eq!(self.next, Some(Phase::ResponseHeaders));
            assert_eq!(self.error.is_some(), name == "malformed_tail");
        }
    }
    pub(crate) fn digest(&self) -> String {
        format!("{:x}", Sha256::digest(self.semantics().as_bytes()))
    }
}
pub(crate) fn cases() -> Vec<(&'static str, Bytes)> {
    let text = "x".repeat(253) + "HIT";
    let wide_text = format!(
        "<r>{}</r>",
        (0..256).map(|_| format!("<v>{text}</v>")).collect::<String>()
    );
    let wide_attrs = format!(
        "<r {}/>",
        (0..256)
            .map(|i| format!("a{i:04}='{text}'"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let many_miss = format!(
        "<r {}/>",
        (0..10_000)
            .rev()
            .map(|i| format!("a{i:05}='miss'"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let mut malformed = wide_text.clone();
    malformed.pop();
    [
        ("small_plain", "<r a='HIT'><v>safe</v><v>HIT</v></r>".to_string()),
        (
            "small_entities",
            "<r a='&#72;&#73;&#84;'><v>safe&amp;fine</v><v>&#x48;IT</v></r>".to_string(),
        ),
        ("wide_text", wide_text),
        ("wide_attrs", wide_attrs),
        ("10000_attrs_miss", many_miss),
        (
            "mixed_cdata",
            "<r a='safe'>HIT<![CDATA[HIT]]><!-- safe --><?check safe?><v>miss</v></r>".to_string(),
        ),
        ("10000_text_miss", format!("<r>{}</r>", "<v>miss</v>".repeat(10_000))),
        ("early_hit", format!("<r><v>HIT</v>{}</r>", "<v>miss</v>".repeat(256))),
        ("late_hit", format!("<r>{}<v>HIT</v></r>", "<v>miss</v>".repeat(256))),
        ("deep", format!("{}HIT{}", "<r>".repeat(16), "</r>".repeat(16))),
        ("large_token", format!("<r>{}HIT</r>", "x".repeat(256 << 10))),
        ("malformed_tail", malformed),
    ]
    .into_iter()
    .map(|(name, body)| (name, Bytes::from(body)))
    .collect()
}
pub(crate) fn percentile(samples: &[Duration], p: usize) -> u128 {
    samples[(samples.len() * p).div_ceil(100) - 1].as_nanos()
}

pub(crate) fn snapshot(tx: &Transaction<'_>, outcome: Result<PhaseOutcome, RuntimeError>) -> Snapshot {
    Snapshot {
        outcome,
        next: tx.next_phase(),
        error: tx.request_body_error(),
        logical: tx.retained_bytes(),
        hit: tx.tx_value(b"hit").map(<[u8]>::to_vec),
        done: tx.tx_value(b"done").map(<[u8]>::to_vec),
        raw: tx.tx_value(b"raw").map(<[u8]>::to_vec),
    }
}
