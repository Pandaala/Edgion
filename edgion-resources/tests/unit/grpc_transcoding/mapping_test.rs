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

#[test]
fn scalar_binding_preserves_owned_and_borrowed_values_and_errors() {
    let pool = prost_reflect::DescriptorPool::decode(
        include_bytes!("../../../../edgion-tests/proto/grpc_json_transcoding/test.pb").as_slice(),
    )
    .unwrap();
    for (message, field, text, expected) in [
        (
            "test.Complex",
            "display_name",
            "hello world&é",
            Some(Json::String("hello world&é".into())),
        ),
        ("test.Complex", "data", "YWJj", Some(Json::String("YWJj".into()))),
        ("test.Complex", "nested", "{}", None),
        (
            "test.NumericFields",
            "null_value",
            "NULL_VALUE",
            Some(Json::String("NULL_VALUE".into())),
        ),
        (
            "test.NumericFields",
            "signed",
            "+0007",
            Some(Json::String("+0007".into())),
        ),
        (
            "test.NumericFields",
            "signed_fixed",
            "-9223372036854775808",
            Some(Json::String("-9223372036854775808".into())),
        ),
        (
            "test.NumericFields",
            "signed_zigzag",
            "9223372036854775807",
            Some(Json::String("9223372036854775807".into())),
        ),
        (
            "test.NumericFields",
            "unsigned",
            "18446744073709551615",
            Some(Json::String("18446744073709551615".into())),
        ),
        (
            "test.NumericFields",
            "unsigned_fixed",
            "+0007",
            Some(Json::String("+0007".into())),
        ),
        (
            "test.NumericFields",
            "int32_number",
            "-2147483648",
            Some(Json::from(i32::MIN)),
        ),
        ("test.NumericFields", "sint32_number", "7", Some(Json::from(7))),
        ("test.NumericFields", "sfixed32_number", "7", Some(Json::from(7))),
        (
            "test.NumericFields",
            "uint32_number",
            "4294967295",
            Some(Json::from(u32::MAX)),
        ),
        ("test.NumericFields", "fixed32_number", "7", Some(Json::from(7))),
        ("test.NumericFields", "float_number", "1.25", Some(Json::from(1.25))),
        ("test.NumericFields", "double_number", "-2.5", Some(Json::from(-2.5))),
        ("google.protobuf.BoolValue", "value", "true", Some(Json::Bool(true))),
        ("google.protobuf.BoolValue", "value", "false", Some(Json::Bool(false))),
        ("google.protobuf.BoolValue", "value", "True", None),
        ("test.NumericFields", "signed", "9223372036854775808", None),
        ("test.NumericFields", "unsigned", "-1", None),
        ("test.NumericFields", "int32_number", "2147483648", None),
        ("test.NumericFields", "uint32_number", "-1", None),
        ("test.NumericFields", "float_number", "NaN", None),
        ("test.NumericFields", "double_number", "Infinity", None),
        ("test.NumericFields", "signed", "1e2", None),
    ] {
        let descriptor = pool
            .get_message_by_name(message)
            .unwrap()
            .get_field_by_name(field)
            .unwrap();
        for input in [Cow::Borrowed(text), Cow::Owned(text.to_owned())] {
            assert_eq!(
                scalar(&descriptor, input),
                expected.clone().ok_or(TranscodingError::Invalid),
                "{message}.{field}: {text}"
            );
        }
    }
}

#[test]
fn owned_scalar_text_moves_decoder_allocation() {
    let pool = prost_reflect::DescriptorPool::decode(
        include_bytes!("../../../../edgion-tests/proto/grpc_json_transcoding/test.pb").as_slice(),
    )
    .unwrap();
    for (message, field, encoded, expected) in [
        ("test.Complex", "display_name", "hello%20world", "hello world"),
        ("test.Complex", "data", "%59WJj", "YWJj"),
        ("test.NumericFields", "signed", "%2B0007", "+0007"),
        ("test.NumericFields", "unsigned", "%31", "1"),
        ("test.NumericFields", "null_value", "%4EULL_VALUE", "NULL_VALUE"),
    ] {
        let descriptor = pool
            .get_message_by_name(message)
            .unwrap()
            .get_field_by_name(field)
            .unwrap();
        let Cow::Owned(input) = decode_path(encoded, false).unwrap() else {
            panic!("escaped input must be owned")
        };
        let pointer = input.as_ptr();
        let capacity = input.capacity();
        let Json::String(output) = scalar(&descriptor, Cow::Owned(input)).unwrap() else {
            panic!("expected string")
        };
        assert_eq!(output, expected);
        assert_eq!(output.as_ptr(), pointer);
        assert_eq!(output.capacity(), capacity);
    }
}
