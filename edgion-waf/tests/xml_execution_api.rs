//! Public owned/scoped XML semantics and independent logical limits.
use bytes::Bytes;
use edgion_waf::seclang::*;
struct NoIncludes;
impl IncludeSource for NoIncludes {
    fn load(&mut self, _: IncludeRequest<'_>, _: &mut IncludeSink<'_>) -> Result<(), IncludeFailure> {
        Err(IncludeFailure::NotFound)
    }
}
fn compile(rules: &str) -> CompiledGeneration {
    let phases = [
        PhaseDefaults::pass(Phase::RequestHeaders, true, true),
        PhaseDefaults::pass(Phase::RequestBody, true, true),
    ];
    compile_with_includes(
        "xml-api",
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
fn tx(g: &CompiledGeneration) -> Transaction<'_> {
    let headers = [RequestHeader::new(b"content-type", b"text/xml")];
    let head = RequestHead::new(
        b"POST / HTTP/1.1",
        b"POST",
        b"/",
        b"/",
        b"HTTP/1.1",
        b"192.0.2.1",
        b"xml-api",
        &headers,
    );
    let mut tx = Transaction::new(
        g,
        head,
        RuntimeLimits {
            max_collection_entries: 1 << 20,
            ..RuntimeLimits::default()
        },
    )
    .unwrap();
    tx.execute_request_headers().unwrap();
    tx
}
#[test]
fn all_two_part_sources_have_owned_capture_chain_and_count_outcomes() {
    let g=compile(concat!(
        "SecRule XML:/*|XML://@* \"@rx (attack)\" \"id:1,phase:2,pass,capture,t:lowercase,setvar:tx.capture=%{TX.1},setvar:tx.name=%{MATCHED_VAR_NAME},chain\"\n",
        "SecRule TX:capture \"@streq attack\" \"setvar:tx.done=yes\"\n",
        "SecRule &XML:/* \"@eq 2\" \"id:2,phase:2,pass,setvar:tx.count=yes\"\n"));
    let xml = b"<r mode='&#65;TTACK'>safe<![CDATA[ATTACK]]></r>";
    let mut owned = tx(&g);
    owned
        .publish_request_body(RequestBody::new(
            Bytes::copy_from_slice(xml),
            BodyProcessorSelection::Auto,
        ))
        .unwrap();
    let expected = owned.execute_request_body().unwrap();
    assert_eq!(owned.tx_value(b"name"), Some(b"XML".as_slice()));
    for cut in 0..=xml.len() {
        let mut scoped = tx(&g);
        let mut session = scoped
            .start_xml_inspection(XmlInspectionLimits::default(), None)
            .unwrap();
        session.feed(&xml[..cut]).unwrap();
        session.feed(b"").unwrap();
        session.feed(&xml[cut..]).unwrap();
        session.complete(xml.len()).unwrap();
        session.finish(None).unwrap();
        assert_eq!(scoped.execute_request_body().unwrap(), expected);
        for key in [b"capture".as_slice(), b"name", b"done", b"count"] {
            assert_eq!(scoped.tx_value(key), owned.tx_value(key));
        }
    }
}
#[test]
fn all_values_boundary_includes_three_metadata_entries_even_without_demands() {
    let g = compile("SecAction \"id:1,phase:2,pass\"");
    for values in [DEFAULT_MAX_BODY_ENTRIES - 3, DEFAULT_MAX_BODY_ENTRIES - 2] {
        let bytes = Bytes::from(format!("<r>{}</r>", "<![CDATA[]]>".repeat(values)));
        let mut transaction = tx(&g);
        let result = transaction.publish_request_body(RequestBody::new(bytes, BodyProcessorSelection::Xml));
        if values == DEFAULT_MAX_BODY_ENTRIES - 3 {
            result.unwrap();
            transaction.execute_request_body().unwrap();
        } else {
            assert!(matches!(
                result,
                Err(RuntimeError::Body {
                    kind: BodyError::EntryLimit { .. }
                })
            ));
        }
    }
}
#[test]
fn malformed_hits_and_source_parity_never_publish_phase_two_effects() {
    let g = compile("SecRule XML \"@contains attack\" \"id:1,phase:2,deny,setvar:tx.hit=yes\"");
    let mut transaction = tx(&g);
    let mut session = transaction
        .start_xml_inspection(XmlInspectionLimits::default(), None)
        .unwrap();
    session.feed(b"<r>attack</r>").unwrap();
    assert_eq!(session.complete(12), Err(RuntimeError::XmlInspectionState));
    drop(session);
    assert_eq!(transaction.tx_value(b"hit"), None);
    assert!(transaction.execute_request_body().is_err());
    let mut transaction = tx(&g);
    assert!(transaction
        .publish_request_body(RequestBody::new(
            Bytes::from_static(b"<r>attack</bad>"),
            BodyProcessorSelection::Xml
        ))
        .is_err());
    assert_eq!(transaction.tx_value(b"hit"), None);
}
