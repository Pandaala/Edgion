use super::*;

#[test]
fn unescaped_inputs_are_borrowed_and_escaped_inputs_remain_strict() {
    for preserve_slash in [false, true] {
        for value in ["", "plain", "café", "a+b", "count=3"] {
            assert!(matches!(decode_path(value, preserve_slash), Ok(Cow::Borrowed(actual)) if actual == value));
        }
        for (value, expected) in [("hello%20world", "hello world"), ("%C3%A9", "é"), ("a%26b+c", "a&b+c")] {
            assert!(matches!(decode_path(value, preserve_slash), Ok(Cow::Owned(actual)) if actual == expected));
        }
        for value in ["%", "%2", "%XY", "%FF", "%C3"] {
            assert!(
                matches!(decode_path(value, preserve_slash), Err(TranscodingError::Invalid)),
                "{value}"
            );
        }
    }
    assert_eq!(decode_path("a%2Fb%2fc", true).unwrap(), "a%2Fb%2fc");
    assert_eq!(decode_path("a%2Fb%2fc", false).unwrap(), "a/b/c");
}

#[test]
fn field_resolution_preserves_aliases_depth_and_descriptor_ownership() {
    let resolved = {
        let pool = prost_reflect::DescriptorPool::decode(
            include_bytes!("../../../../edgion-tests/proto/grpc_json_transcoding/test.pb").as_slice(),
        )
        .unwrap();
        let input = pool.get_message_by_name("test.Complex").unwrap();
        for (proto, json) in [
            ("display_name", "displayName"),
            ("nested.display_name", "nested.displayName"),
        ] {
            assert_eq!(
                resolve_field(&input, proto).unwrap(),
                resolve_field(&input, json).unwrap()
            );
        }
        for terminal in ["numbers", "labels", "nested"] {
            assert_eq!(resolve_field(&input, terminal).unwrap().len(), 1);
        }
        for invalid in [
            "",
            ".",
            "nested.",
            ".id",
            "nested..id",
            "missing",
            "Nested.id",
            "numbers.id",
            "labels.id",
            "id.name",
        ] {
            assert_eq!(
                resolve_field(&input, invalid).unwrap_err(),
                TranscodingError::Invalid,
                "{invalid}"
            );
        }
        let valid = format!("{}displayName", "nested.".repeat(31));
        assert_eq!(resolve_field(&input, &valid).unwrap().len(), 32);
        let too_deep = format!("nested.{valid}");
        assert_eq!(resolve_field(&input, &too_deep).unwrap_err(), TranscodingError::Invalid);
        assert_eq!(
            resolve_field(&input, &"x".repeat(4097)).unwrap_err(),
            TranscodingError::Invalid
        );
        resolve_field(&input, "nested.displayName").unwrap()
    };
    assert_eq!(resolved.iter().map(FieldDescriptor::number).collect::<Vec<_>>(), [5, 6]);
    assert_eq!(resolved[1].full_name(), "test.Complex.display_name");
}
