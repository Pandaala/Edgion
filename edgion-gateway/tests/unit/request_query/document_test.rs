use super::*;

#[test]
fn token_ranges_preserve_raw_bytes_flags_and_empty_gaps() {
    let document = QueryDocument::new(Some("&&a=1&&flag&empty=&v=a=b&".into()));
    let parsed = document.parsed().unwrap();
    let pairs: Vec<_> = parsed
        .iter(document.raw().unwrap())
        .map(|pair| document.raw_pair(pair))
        .collect();
    assert_eq!(pairs, ["a=1", "flag", "empty=", "v=a=b"]);
    assert!(parsed.pair(1).equals.is_none());
    assert!(parsed.pair(2).equals.is_some());
    assert_eq!(document.raw_value(parsed.pair(3)), "a=b");
    assert_eq!(document.raw(), Some("&&a=1&&flag&empty=&v=a=b&"));
}

#[test]
fn entry_lookup_matches_canonical_parser_without_decoding_unrequested_values() {
    for query in [
        "",
        "&&",
        "a=1&a=2&%61=3",
        "a+key=one&a%20key=two",
        "flag&empty=&=value",
        "bad%ZZ=one&api%5Fkey=two&%+A=three",
        "caf%E9=value&x=%C3%A9%FF",
        "a=%2f&b=+&c=foo%26bar",
    ] {
        let document = QueryDocument::new(Some(query.into()));
        let expected = edgion_resources::uri_utils::parse_query_string(query);
        for (name, value) in expected {
            assert_eq!(
                document.lookup(&name, None),
                Some(value.as_str()),
                "{query:?}: {name:?}"
            );
        }
        assert_eq!(document.lookup("missing", None), None);
    }
    let document = QueryDocument::new(Some("first=plain&other=long%20secret".into()));
    assert_eq!(document.lookup("first", None), Some("plain"));
    assert!(document.parsed().unwrap().pair(1).value.get().is_none());
}

#[test]
fn lb_retains_raw_equality_first_equals_and_empty_value_rules() {
    for (query, name, expected) in [
        ("key&key=&key=late", "key", Some("")),
        ("key=first&key=last", "key", Some("first")),
        ("%6bey=raw%2f&key=last", "key", Some("raw%2f")),
        ("api%5Fkey=literal", "api%5Fkey", Some("literal")),
        ("api%5Fkey=literal", "api_key", Some("literal")),
        ("flag&other=1", "flag", None),
    ] {
        let document = QueryDocument::new(Some(query.into()));
        assert_eq!(document.hash_arg(name, None), expected);
    }
    let document = QueryDocument::new(Some("api%5Fkey=literal".into()));
    assert_eq!(document.hash_arg("api%5Fkey", None), Some("literal"));
    let pair = document.parsed().unwrap().pair(0);
    assert!(pair.name.get().is_none(), "raw-name equality must bypass decoding");
    assert!(pair.value.get().is_none(), "LB must not decode values");
    let raw_value = document.raw_value(pair);
    assert_eq!(document.hash_arg("api_key", None).unwrap().as_ptr(), raw_value.as_ptr());
    let decoded = pair.name.get().unwrap().as_ptr();
    assert_eq!(document.hash_arg("api_key", None), Some("literal"));
    assert_eq!(pair.name.get().unwrap().as_ptr(), decoded);
}

#[test]
fn raw_and_empty_documents_do_not_allocate_structured_storage() {
    let document = QueryDocument::new(Some("a=1".into()));
    assert_eq!(document.raw(), Some("a=1"));
    assert!(document.parsed.get().is_none());
    for raw in [None, Some(""), Some("&&")] {
        let document = QueryDocument::new(raw.map(str::to_owned));
        let (_, stats) = crate::test_allocation_counter::count_allocation_stats(|| {
            assert!(document.parsed().is_none());
        });
        assert_eq!(stats.calls, 0);
    }
}

#[test]
fn sorted_index_selects_last_active_duplicate_without_changing_raw_order() {
    let document = QueryDocument::new(Some("a=1&b=2&%61=3&a=4".into()));
    assert_eq!(document.lookup("a", Some(&[0, 1, 2])), Some("3"));
    assert_eq!(document.lookup("a", Some(&[0, 1])), Some("1"));
    assert_eq!(document.lookup("a", Some(&[1])), None);
    assert_eq!(document.hash_arg("a", Some(&[0, 2, 3])), Some("1"));
}

#[test]
fn first_hit_leaves_long_tail_unparsed_and_reuses_stable_pair() {
    let raw = format!("first=one&{}", "tail=value&".repeat(256));
    let document = QueryDocument::new(Some(raw));
    assert_eq!(document.hash_arg("first", None), Some("one"));
    let parsed = document.parsed().unwrap();
    let first = parsed.pair(0) as *const QueryPair;
    let Slot::Pair(second) = &parsed.first[1] else {
        panic!("pair slot")
    };
    assert!(second.get().is_none());
    let Slot::Tail(tail) = &parsed.first[4] else {
        panic!("tail slot")
    };
    assert!(tail.get().is_none());
    assert_eq!(document.pairs().count(), 257);
    assert_eq!(parsed.pair(0) as *const QueryPair, first);
    assert_eq!(document.hash_arg("first", None), Some("one"));
}

#[test]
fn block_boundaries_cache_end_without_allocating_an_empty_tail() {
    for count in [4, 5, 12, 13, 28, 29] {
        let raw = (0..count)
            .map(|id| format!("key{id}={id}"))
            .collect::<Vec<_>>()
            .join("&")
            + "&&&";
        let document = QueryDocument::new(Some(raw));
        let pairs: Vec<_> = document.pairs().collect();
        assert_eq!(pairs.len(), count);
        let parsed = document.parsed().unwrap();
        for (id, pair) in pairs {
            assert!(std::ptr::eq(pair, parsed.pair(id)));
            assert_eq!(document.raw_value(pair), id.to_string());
        }
        let (_, allocation) = crate::test_allocation_counter::count_allocation_stats(|| {
            assert_eq!(document.pairs().count(), count);
            assert_eq!(document.hash_arg("missing", None), None);
        });
        assert_eq!(allocation.calls, 0);
        if [4, 12, 28].contains(&count) {
            let mut block = parsed.first.as_ref();
            loop {
                let Slot::Tail(cell) = block.last().unwrap() else {
                    panic!("tail slot")
                };
                match cell.get().expect("visited end") {
                    Some(tail) => block = &tail.slots,
                    None => break,
                }
            }
        }
    }
}
