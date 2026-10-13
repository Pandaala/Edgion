use super::*;
use crate::value::{Value, MAX_VALUE_WORK_UNITS};

fn parsed(source: &str) -> XmlValue {
    parse(source, &mut ValueWorkBudget::default()).unwrap()
}

#[test]
fn namespaces_attributes_order_and_mixed_content_round_trip() {
    let root = parsed("<s:Envelope a:x='one' xmlns:a='urn:a' xmlns:s='urn:soap'>before<s:Body id='7'>x<![CDATA[<&]]></s:Body><s:Body/>after</s:Envelope>");
    assert_eq!(root.name(), Some(("urn:soap", "Envelope")));
    assert_eq!(root.attributes().unwrap().len(), 1);
    let mut budget = ValueWorkBudget::default();
    assert_eq!(root.attr_value("urn:a", "x", &mut budget).unwrap(), Some("one"));
    assert_eq!(root.children().unwrap().len(), 4);
    let first = root.find("urn:soap", "Body", 0, &mut budget).unwrap().unwrap();
    assert_eq!(first.value(128, &mut budget).unwrap(), "x<&");
    assert_eq!(root.value(128, &mut budget).unwrap(), "beforex<&after");
    assert!(root.find("urn:soap", "Body", 1, &mut budget).unwrap().is_some());
    assert!(root.find("urn:soap", "Body", 2, &mut budget).unwrap().is_none());
    let serialized = root.stringify(MAX_XML_BYTES, &mut budget).unwrap();
    let reparsed = parsed(&serialized);
    assert_eq!(reparsed.name(), root.name());
    assert_eq!(reparsed.value(128, &mut budget).unwrap(), "beforex<&after");
    assert_eq!(reparsed.attr_value("urn:a", "x", &mut budget).unwrap(), Some("one"));
}

#[test]
fn malformed_unsupported_and_omitted_content_still_validate() {
    for source in [
        "",
        " ",
        "<!--ok-->",
        "<?ok data?>",
        "<?pi!data?><r/>",
        "<?pi?data?><r/>",
        "<r>",
        "<a><b></a></b>",
        "<a/><b/>",
        "<!DOCTYPE r><r/>",
        "<!DOCTYPE r [<!ENTITY e 'boom'>]><r>&e;</r>",
        "<!DOCTYPE r SYSTEM 'file:///etc/passwd'><r/>",
        "<r>&unknown;</r>",
        "<r>&#0;</r>",
        "<r>&#+65;</r>",
        "<r>&#x+41;</r>",
        "<r a='1' a='2'/>",
        "<p:r/>",
        "<r xmlns:a='urn:x' xmlns:b='urn:x' a:v='1' b:v='2'/>",
        "<r xmlns:xml='urn:wrong'/>",
        "<r xmlns:p='http://www.w3.org/XML/1998/namespace'/>",
        "<r xmlns='http://www.w3.org/XML/1998/namespace'/>",
        "<r xmlns:p=''/>",
        "<r xmlns:xmlns='urn:x'/>",
        "<r xmlns='http://www.w3.org/2000/xmlns/'/>",
        "<xi:include xmlns:xi='http://www.w3.org/2001/XInclude' href='http://example.com'/>",
        "<?xml version='1.1'?><r/>",
        "<?xml\tversion='1.0'?><r/>",
        "<?xml\rversion='1.0'?><r/>",
        "<?xml\nversion='1.0'?><r/>",
        "<?xml version='1.0' encoding='ISO-8859-1'?><r/>",
        "<r><!--\0--></r>",
        "<r><?pi \0?></r>",
        "<r a='&#xD800;'/>",
    ] {
        assert!(
            matches!(parse(source, &mut ValueWorkBudget::default()), Err(XmlError::Invalid)),
            "source {source:?}"
        );
    }
}

#[test]
fn namespace_rebinding_and_xml_whitespace_normalization() {
    for source in ["<?pi?><r/>", "<?pi !data?><r/>", "<r><?pi\t?data?></r>"] {
        assert_eq!(parsed(source).name(), Some(("", "r")));
    }
    let root = parsed("<r xmlns='urn:one' xmlns:p='urn:a' a='x\r\ny\t&#9;&#10;&#13;'>a\r\nb&#13;<p:c xmlns:p='urn:b'/><c xmlns=''/><xml:c xml:lang='en'/></r>");
    let mut budget = ValueWorkBudget::default();
    assert_eq!(root.attr_value("", "a", &mut budget).unwrap(), Some("x y \t\n\r"));
    assert_eq!(root.value(64, &mut budget).unwrap(), "a\nb\r");
    assert_eq!(root.children().unwrap()[1].name(), Some(("urn:b", "c")));
    assert_eq!(root.children().unwrap()[2].name(), Some(("", "c")));
    assert_eq!(root.children().unwrap()[3].name(), Some((XML_URI, "c")));
    let output = root.stringify(4096, &mut budget).unwrap();
    let roundtrip = parsed(&output);
    assert_eq!(roundtrip.attr_value("", "a", &mut budget).unwrap(), Some("x y \t\n\r"));
    assert_eq!(roundtrip.value(64, &mut budget).unwrap(), "a\nb\r");
    assert!(output.contains("xml:lang=\"en\""));
    assert!(!output.contains("xmlns:xml"));
}

#[test]
fn source_depth_nodes_attributes_names_and_decoded_bounds() {
    let mut budget = ValueWorkBudget::default();
    assert!(matches!(
        parse(&"x".repeat(MAX_XML_BYTES + 1), &mut budget),
        Err(XmlError::Limit {
            kind: "source bytes",
            ..
        })
    ));
    let exact = format!("{}{}", "<r>".repeat(MAX_XML_DEPTH), "</r>".repeat(MAX_XML_DEPTH));
    assert!(parse(&exact, &mut ValueWorkBudget::default()).is_ok());
    assert!(matches!(
        parse(&format!("<r>{exact}</r>"), &mut ValueWorkBudget::default()),
        Err(XmlError::Limit { kind: "depth", .. })
    ));
    let exact = format!("<r>{}</r>", "<c/>".repeat(MAX_XML_NODES - 1));
    assert!(parse(&exact, &mut ValueWorkBudget::default()).is_ok());
    assert!(matches!(
        parse(
            &format!("<r>{}</r>", "<c/>".repeat(MAX_XML_NODES)),
            &mut ValueWorkBudget::default()
        ),
        Err(XmlError::Limit { kind: "nodes", .. })
    ));
    let attrs = (0..MAX_XML_TAG_ATTRIBUTES)
        .map(|i| format!(" a{i}='x'"))
        .collect::<String>();
    assert!(parse(&format!("<r{attrs}/>"), &mut ValueWorkBudget::default()).is_ok());
    assert!(matches!(
        parse(&format!("<r{attrs} extra='x'/>"), &mut ValueWorkBudget::default()),
        Err(XmlError::Limit {
            kind: "attributes per element",
            ..
        })
    ));
    assert!(XmlValue::element("", &"n".repeat(MAX_XML_NAME_BYTES), vec![], vec![], &mut budget).is_ok());
    assert!(matches!(
        XmlValue::element("", &"n".repeat(MAX_XML_NAME_BYTES + 1), vec![], vec![], &mut budget),
        Err(XmlError::Limit { kind: "name bytes", .. })
    ));
    assert!(XmlValue::text(&"x".repeat(MAX_XML_BYTES), &mut ValueWorkBudget::default()).is_ok());
    assert!(matches!(
        XmlValue::text(&"x".repeat(MAX_XML_BYTES + 1), &mut budget),
        Err(XmlError::Limit {
            kind: "decoded bytes",
            ..
        })
    ));
}

#[test]
fn constructors_reject_invalid_structure_and_alias_expansion() {
    let mut budget = ValueWorkBudget::default();
    for (ns, name) in [
        ("", "a:b"),
        ("", "1bad"),
        ("", "xmlns"),
        (XMLNS_URI, "r"),
        (XINCLUDE_URI, "include"),
    ] {
        assert!(matches!(
            XmlValue::element(ns, name, vec![], vec![], &mut budget),
            Err(XmlError::Invalid)
        ));
    }
    let attr = XmlValue::attribute("urn:a", "v", "<&\"", &mut budget).unwrap();
    assert!(matches!(
        XmlValue::element("", "r", vec![attr.clone(), attr.clone()], vec![], &mut budget),
        Err(XmlError::Invalid)
    ));
    assert!(matches!(
        XmlValue::element("", "r", vec![], vec![attr], &mut budget),
        Err(XmlError::Invalid)
    ));
    let mut node = XmlValue::element("", "r", vec![], vec![], &mut budget).unwrap();
    for _ in 0..11 {
        node = XmlValue::element("", "r", vec![], vec![node.clone(), node], &mut budget).unwrap();
    }
    assert_eq!(node.0.summary.nodes, 4095);
    assert!(matches!(
        XmlValue::element("", "r", vec![], vec![node.clone(), node], &mut budget),
        Err(XmlError::Limit { kind: "nodes", .. })
    ));
}

#[test]
fn output_and_work_limits_never_truncate_or_reset() {
    let root = parsed("<r>&amp;</r>");
    let mut budget = ValueWorkBudget::default();
    let output = root.stringify(12, &mut budget).unwrap();
    assert_eq!(output, "<r>&amp;</r>");
    assert!(matches!(
        root.stringify(output.len() - 1, &mut budget),
        Err(XmlError::Limit {
            kind: "output bytes",
            ..
        })
    ));
    assert!(matches!(
        root.value(0, &mut budget),
        Err(XmlError::Limit {
            kind: "output bytes",
            ..
        })
    ));
    assert!(matches!(
        parse("<r/>", &mut ValueWorkBudget::exhausted()),
        Err(XmlError::Work(RuntimeError::ValueWorkLimitExceeded { .. }))
    ));
    // Determine exact work from the account, then enforce exact/one-less admission.
    let mut spent = ValueWorkBudget::default();
    parse("<r/>", &mut spent).unwrap();
    let mut remaining = 0;
    while spent.charge(1).is_ok() {
        remaining += 1;
    }
    let cost = MAX_VALUE_WORK_UNITS - remaining;
    let mut exact = ValueWorkBudget::default();
    exact.charge(MAX_VALUE_WORK_UNITS - cost).unwrap();
    assert!(parse("<r/>", &mut exact).is_ok());
    let mut short = ValueWorkBudget::default();
    short.charge(MAX_VALUE_WORK_UNITS - cost + 1).unwrap();
    assert!(matches!(parse("<r/>", &mut short), Err(XmlError::Work(_))));
    let xml = Value::Xml(root.clone());
    assert_eq!(xml.type_name(), "Xml");
    assert!(xml.implicit_string_len().is_err());
    assert!(xml.try_as_str().is_err());
    assert!(xml.try_into_string().is_err());
    assert_eq!(root, root.clone());
    assert_ne!(root, parsed("<r>&amp;</r>"));
    assert_eq!(std::mem::size_of::<Value>(), 24);
}

#[test]
fn retained_tree_producers_cannot_evade_the_cumulative_work_budget() {
    let payload = "x".repeat(64 * 1024);
    let mut budget = ValueWorkBudget::default();
    let mut retained = Vec::new();
    loop {
        match XmlValue::text(&payload, &mut budget) {
            Ok(node) => retained.push(node),
            Err(XmlError::Work(RuntimeError::ValueWorkLimitExceeded { .. })) => break,
            other => panic!("unexpected result {other:?}"),
        }
    }
    assert_eq!(retained.len(), 63);
    assert!(matches!(XmlValue::text(&payload, &mut budget), Err(XmlError::Work(_))));
}

#[test]
fn namespace_and_attribute_limits_are_independent_and_expanded() {
    let namespaces = (0..MAX_XML_TAG_NAMESPACES)
        .map(|i| format!(" xmlns:p{i}='urn:{i}'"))
        .collect::<String>();
    let attrs = (0..MAX_XML_TAG_ATTRIBUTES)
        .map(|i| format!(" a{i}='x'"))
        .collect::<String>();
    assert!(parse(&format!("<r{namespaces}{attrs}/>"), &mut ValueWorkBudget::default()).is_ok());
    assert!(matches!(
        parse(
            &format!("<r{namespaces} xmlns:extra='urn:extra'/>"),
            &mut ValueWorkBudget::default()
        ),
        Err(XmlError::Limit {
            kind: "namespaces per element",
            ..
        })
    ));
    let uri = "u".repeat(MAX_XML_NAMESPACE_BYTES);
    assert!(parse(&format!("<r xmlns='{uri}'/>"), &mut ValueWorkBudget::default()).is_ok());
    assert!(matches!(
        parse(&format!("<r xmlns='{uri}u'/>"), &mut ValueWorkBudget::default()),
        Err(XmlError::Limit { .. })
    ));
    let tag = format!("<c{attrs}/>");
    assert!(parse(&format!("<r>{}</r>", tag.repeat(64)), &mut ValueWorkBudget::default()).is_ok());
    assert!(matches!(
        parse(
            &format!("<r>{}<c a='x'/></r>", tag.repeat(64)),
            &mut ValueWorkBudget::default()
        ),
        Err(XmlError::Limit { kind: "attributes", .. })
    ));
}

#[test]
fn serializer_reuses_bindings_and_bounds_generated_declarations() {
    let attrs = (0..MAX_XML_TAG_ATTRIBUTES)
        .map(|i| format!(" p:a{i}='x'"))
        .collect::<String>();
    for root_uri in ["urn:root", "urn:shared"] {
        let root = parsed(&format!("<r xmlns='{root_uri}' xmlns:p='urn:shared'{attrs}/>"));
        let output = root.stringify(MAX_XML_BYTES, &mut ValueWorkBudget::default()).unwrap();
        assert_eq!(
            output.matches(" xmlns:").count(),
            if root_uri == "urn:shared" { 1 } else { 2 }
        );
        let reparsed = parsed(&output);
        assert_eq!(reparsed.name(), root.name());
        assert_eq!(reparsed.attributes().unwrap().len(), MAX_XML_TAG_ATTRIBUTES);
        for i in 0..MAX_XML_TAG_ATTRIBUTES {
            let attribute = &reparsed.attributes().unwrap()[i];
            assert_eq!(attribute.name(), Some(("urn:shared", format!("a{i}").as_str())));
        }
    }
    let attributes = (0..MAX_XML_TAG_ATTRIBUTES)
        .map(|i| {
            XmlValue::attribute(
                &format!("urn:{i}"),
                &format!("a{i}"),
                "x",
                &mut ValueWorkBudget::default(),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    for root_uri in ["", "urn:0"] {
        let root = XmlValue::element(
            root_uri,
            "r",
            attributes.clone(),
            vec![],
            &mut ValueWorkBudget::default(),
        )
        .unwrap();
        let output = root.stringify(MAX_XML_BYTES, &mut ValueWorkBudget::default()).unwrap();
        assert_eq!(output.matches(" xmlns:").count(), MAX_XML_TAG_NAMESPACES);
        assert_eq!(parsed(&output).attributes().unwrap().len(), MAX_XML_TAG_ATTRIBUTES);
    }
    let root = XmlValue::element("urn:distinct", "r", attributes, vec![], &mut ValueWorkBudget::default()).unwrap();
    assert!(matches!(
        root.stringify(MAX_XML_BYTES, &mut ValueWorkBudget::default()),
        Err(XmlError::Limit {
            kind: "namespaces per element",
            limit: MAX_XML_TAG_NAMESPACES,
        })
    ));
    let declarations = (0..MAX_XML_TAG_ATTRIBUTES)
        .map(|i| format!(" xmlns:p{i}='urn:{i}' p{i}:a{i}='x'"))
        .collect::<String>();
    let inherited = parsed(&format!("<outer xmlns:q='urn:distinct'><q:r{declarations}/></outer>"));
    assert!(matches!(
        inherited.children().unwrap()[0].stringify(MAX_XML_BYTES, &mut ValueWorkBudget::default()),
        Err(XmlError::Limit {
            kind: "namespaces per element",
            limit: MAX_XML_TAG_NAMESPACES,
        })
    ));
}

#[test]
fn decoder_borrows_only_unchanged_tokens() {
    for (raw, attribute, references) in [
        ("", false, true),
        ("plain é text", false, true),
        ("line\nnext\ttab", false, true),
        ("plain attribute", true, true),
        ("<&amp; é", false, false),
    ] {
        let decoded = decode(raw, attribute, references, raw.len(), &mut ValueWorkBudget::default()).unwrap();
        assert!(matches!(decoded, Cow::Borrowed(_)));
        assert_eq!(decoded, raw);
        assert_eq!(decoded.as_ptr(), raw.as_ptr());
    }
    for (raw, attribute, references, expected) in [
        ("&amp;tail", false, true, "&tail"),
        ("prefix é &amp;tail", false, true, "prefix é &tail"),
        ("line\r\nnext\rtail", false, true, "line\nnext\ntail"),
        ("a\r\nb\tc", true, true, "a b c"),
        ("a&#10;&#x9;b", true, true, "a\n\tb"),
        ("&amp;\r\n", false, false, "&amp;\n"),
    ] {
        let decoded = decode(
            raw,
            attribute,
            references,
            expected.len(),
            &mut ValueWorkBudget::default(),
        )
        .unwrap();
        assert!(matches!(decoded, Cow::Owned(_)));
        assert_eq!(decoded, expected);
    }
}

fn decoder_budget(units: u64) -> ValueWorkBudget {
    let mut budget = ValueWorkBudget::default();
    budget.charge(MAX_VALUE_WORK_UNITS - units).unwrap();
    budget
}

fn decoder_remaining(mut budget: ValueWorkBudget) -> u64 {
    let mut remaining = 0;
    while budget.charge(1).is_ok() {
        remaining += 1;
    }
    remaining
}

#[test]
fn decoder_keeps_exact_work_charges_across_late_ownership_transition() {
    for (raw, expected) in [("plain é", "plain é"), ("abc&amp;é", "abc&é"), ("é\r\nx", "é\nx")] {
        let cost = expected.len() as u64 * 4;
        let mut budget = decoder_budget(cost);
        assert_eq!(decode(raw, false, true, expected.len(), &mut budget).unwrap(), expected);
        assert_eq!(decoder_remaining(budget), 0);
        let mut short = decoder_budget(cost - 1);
        assert!(matches!(
            decode(raw, false, true, expected.len(), &mut short),
            Err(XmlError::Work(_))
        ));
        assert_eq!(
            decoder_remaining(short),
            expected.chars().last().unwrap().len_utf8() as u64 * 2 - 1
        );
    }
}

#[test]
fn decoder_preserves_competing_error_order_and_remaining_work() {
    for (raw, limit, units, expected, remaining) in [
        ("a", 0, 1, "work", 1),
        ("a", 0, 2, "limit", 0),
        ("a", 1, 3, "work", 1),
        ("é", 1, 3, "work", 3),
        ("é", 1, 4, "limit", 0),
        ("abc&amp;", 3, 13, "work", 1),
        ("abc&amp;", 3, 14, "limit", 0),
        ("abc&amp;", 4, 15, "work", 1),
        ("a&unknown;", 0, 10, "limit", 8),
        ("&unknown;", 0, 0, "invalid", 0),
        ("a&#0;", 1, 8, "invalid", 4),
    ] {
        let mut budget = decoder_budget(units);
        let error = decode(raw, false, true, limit, &mut budget).unwrap_err();
        let kind = match error {
            XmlError::Work(_) => "work",
            XmlError::Limit { .. } => "limit",
            XmlError::Invalid => "invalid",
            XmlError::Allocation => "allocation",
        };
        assert_eq!(kind, expected, "{raw}");
        assert_eq!(decoder_remaining(budget), remaining, "{raw}");
    }
}

#[test]
fn decoded_nodes_own_text_attributes_and_namespace_after_source_drop() {
    let root = {
        let source = String::from("<r xmlns='urn:owned' a='plain é'>plain &amp; text</r>");
        parsed(&source)
    };
    assert_eq!(root.name(), Some(("urn:owned", "r")));
    assert_eq!(
        root.attr_value("", "a", &mut ValueWorkBudget::default()).unwrap(),
        Some("plain é")
    );
    assert_eq!(root.value(32, &mut ValueWorkBudget::default()).unwrap(), "plain & text");
}
