use super::*;
use prost::Message;
const DESCRIPTOR: &[u8] = include_bytes!("../../../../edgion-tests/proto/grpc_json_transcoding/test.pb");
fn config() -> GrpcJsonTranscoderConfig {
    serde_json::from_value(serde_json::json!({
        "descriptor": {"type":"inline","base64":STANDARD.encode(DESCRIPTOR)},
        "services":["test.TestService", "test.CodecTest"], "autoMapping":true
    }))
    .unwrap()
}
#[test]
fn http_annotations_additional_bindings_and_current_parameters() {
    let c = config().compile().unwrap();
    let (binding, frame) = c
        .bind("GET", "/v1/hello/hello%20world", None)
        .unwrap()
        .encode(b"", c.limits)
        .unwrap();
    assert_eq!(binding.rpc_path, "/test.TestService/SayHello");
    assert_eq!(frame[0], 0);
    assert_eq!(
        u32::from_be_bytes(frame[1..5].try_into().unwrap()) as usize,
        frame.len() - 5
    );
    let message = prost_reflect::DynamicMessage::decode(binding.method.input(), &frame[5..]).unwrap();
    assert_eq!(message.get_field_by_name("name").unwrap().as_str(), Some("hello world"));
    assert!(c.bind("GET", "/v1/hello/name", Some("name=override")).is_err());
    assert!(c
        .bind("POST", "/v1/hello", None)
        .unwrap()
        .encode(br#"{"name":"body"}"#, c.limits)
        .is_ok());
    let (binding, frame) = c
        .bind("GET", "/v1/numbers", Some("count=3"))
        .unwrap()
        .encode(b"", c.limits)
        .unwrap();
    assert!(binding.method.is_server_streaming());
    assert_eq!(frame[5..], [8, 3]);
}
#[test]
fn canonical_protojson_round_trip_and_timestamp_root() {
    let c = config().compile().unwrap();
    let body = br#"{"id":"9223372036854775807","data":"YWJj","numbers":[1,2],"labels":{"a":3},"nested":{"displayName":"nested"}}"#;
    let (binding, frame) = c
        .bind("POST", "/test.CodecTest/Echo", None)
        .unwrap()
        .encode(body, c.limits)
        .unwrap();
    let output = binding.decode(&frame[5..], c.limits, None).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["id"], "9223372036854775807");
    assert_eq!(value["data"], "YWJj");
    assert_eq!(value["numbers"], serde_json::json!([1, 2]));
    assert_eq!(value["labels"]["a"], 3);
    let (binding, frame) = c
        .bind("POST", "/test.CodecTest/Time", None)
        .unwrap()
        .encode(br#""2026-10-07T00:00:00Z""#, c.limits)
        .unwrap();
    assert_eq!(
        binding.decode(&frame[5..], c.limits, None).unwrap(),
        br#""2026-10-07T00:00:00Z""#
    );
}
#[test]
fn input_rejects_duplicates_unknown_fields_and_complexity_before_reflection() {
    let c = config().compile().unwrap();
    for body in [
        br#"{"displayName":"","display_name":"later"}"#.as_slice(),
        br#"{"nested":{"displayName":null,"display_name":"later"}}"#,
    ] {
        assert!(c
            .bind("POST", "/test.CodecTest/Echo", None)
            .unwrap()
            .encode(body, c.limits)
            .is_err());
    }
    for body in [br#"{"name":"a","name":"b"}"#.as_slice(), br#"{"unknown":true}"#, b"{"] {
        assert!(c
            .bind("POST", "/v1/hello", None)
            .unwrap()
            .encode(body, c.limits)
            .is_err());
    }
    assert!(c.bind("GET", "/v1/numbers", Some("unknown=1")).is_err());
    assert!(c.bind("GET", "/v1/numbers", Some("count=1&count=2")).is_err());
    let mut limits = c.limits;
    limits.nodes = 2;
    assert!(c
        .bind("POST", "/test.CodecTest/Echo", None)
        .unwrap()
        .encode(br#"{"numbers":[1,2]}"#, limits)
        .is_err());
    limits = c.limits;
    limits.depth = 2;
    assert!(c
        .bind("POST", "/test.CodecTest/Echo", None)
        .unwrap()
        .encode(br#"{"nested":{"nested":{}}}"#, limits)
        .is_err());
}
#[test]
fn packed_and_recursive_wire_admission_and_capped_json() {
    let c = config().compile().unwrap();
    let binding = c.bind("POST", "/test.CodecTest/Echo", None).unwrap().binding;
    let mut limits = c.limits;
    limits.nodes = 4;
    assert!(binding.decode(&[26, 3, 1, 2, 3], limits, None).is_err());
    limits = c.limits;
    limits.depth = 2;
    assert!(binding.decode(&[42, 2, 42, 0], limits, None).is_err());
    for bytes in [&[10, 255][..], &[51, 52], &[0], &[8, 128], &[18, 2, 1]] {
        assert!(binding.decode(bytes, c.limits, None).is_err());
    }
    limits = c.limits;
    limits.json = 5;
    assert!(binding.decode(&[8, 1], limits, None).is_err());
    let output = binding
        .decode(&[8, 1], c.limits, Some(GrpcJsonStreamFormat::Ndjson))
        .unwrap();
    assert!(output.ends_with(b"}\n"));
    assert_eq!(output.capacity(), output.len());
}
#[test]
fn descriptor_validation_and_config_transport_visibility() {
    let mut c = config();
    c.services.clear();
    assert!(c.compile().is_err());
    c = config();
    c.max_response_message_size = "1m".into();
    assert!(c.compile().is_err());
    c = config();
    c.max_message_nodes = 16385;
    assert!(c.compile().is_err());
    c = config();
    c.descriptor = GrpcDescriptorSource::ConfigMapRef {
        name: "descriptor".into(),
        key: "test.pb".into(),
    };
    assert!(c.validation_error().is_none());
    assert!(c.compile().is_err());
    c.resolved_descriptor = Some(DESCRIPTOR.to_vec());
    assert!(c.compile().is_ok());
    let json = serde_json::to_value(&c).unwrap();
    assert!(json.get("resolvedDescriptor").is_some());
    let schema = serde_json::to_value(schemars::schema_for!(GrpcJsonTranscoderConfig)).unwrap();
    assert!(!schema["properties"]
        .as_object()
        .unwrap()
        .contains_key("resolvedDescriptor"));
    c.descriptor_error = Some("missing descriptor".into());
    assert!(c.compile().is_err());
}

#[test]
fn annotated_multi_segment_templates_preserve_encoded_slashes_and_empty_matches() {
    // Exercise the same compiler used for google.api.http annotated bindings.
    let compiled = config().compile().unwrap();
    let method = compiled
        .bind("GET", "/v1/hello/name", None)
        .unwrap()
        .binding
        .method
        .clone();
    for (template, path, expected) in [
        ("/v1/{name=**}", "/v1", ""),
        ("/v1/{name=**}", "/v1/", ""),
        ("/v1/{name=**}", "/v1/a/b%2Fc%2fd", "a/b%2Fc%2fd"),
        ("/v1/{name=foo/**}:watch", "/v1/foo:watch", "foo"),
        ("/v1/{name=foo/**}:watch", "/v1/foo/a%2Fb:watch", "foo/a%2Fb"),
        ("/v1/{name}:watch", "/v1/a%2Fb:watch", "a/b"),
    ] {
        let binding = Arc::new(CompiledBinding::compile(method.clone(), "GET", template, "").unwrap());
        let (_, frame) = binding
            .bind(path, None, false)
            .unwrap()
            .encode(b"", compiled.limits)
            .unwrap();
        let message = prost_reflect::DynamicMessage::decode(method.input(), &frame[5..]).unwrap();
        assert_eq!(
            message.get_field_by_name("name").unwrap().as_str(),
            Some(expected),
            "{template} {path}"
        );
    }
    for (template, path) in [
        ("/v1/**:watch", "/v1:watch"),
        ("/v1/**:watch", "/v1/a/b:watch"),
        ("/v1/*:watch", "/v1/a:watch"),
    ] {
        let binding = Arc::new(CompiledBinding::compile(method.clone(), "GET", template, "").unwrap());
        assert!(binding.bind(path, None, false).is_ok(), "{template} {path}");
    }
    for template in [
        "/v1/{name=**}/later",
        "/v1/**/later",
        "/v1/{name}suffix",
        "/v1/{name=foo/**/later}",
    ] {
        assert!(
            CompiledBinding::compile(method.clone(), "GET", template, "").is_err(),
            "{template}"
        );
    }
}

#[test]
fn hostile_descriptor_defaults_extensions_maps_and_wkt_shapes_are_rejected() {
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet, MessageOptions,
        MethodDescriptorProto, ServiceDescriptorProto,
    };
    fn field(name: &str, number: i32, kind: Type, repeated: bool) -> FieldDescriptorProto {
        FieldDescriptorProto {
            name: Some(name.into()),
            number: Some(number),
            r#type: Some(kind as i32),
            label: Some(if repeated { Label::Repeated } else { Label::Optional } as i32),
            ..Default::default()
        }
    }
    fn descriptor(
        response: DescriptorProto,
        child: Option<DescriptorProto>,
        package: &str,
    ) -> GrpcJsonTranscoderConfig {
        let response_name = response.name.clone().unwrap();
        let mut messages = vec![
            DescriptorProto {
                name: Some("Request".into()),
                ..Default::default()
            },
            response,
        ];
        messages.extend(child);
        let set = FileDescriptorSet {
            file: vec![FileDescriptorProto {
                name: Some("hostile.proto".into()),
                package: Some(package.into()),
                syntax: Some("proto2".into()),
                message_type: messages,
                service: vec![ServiceDescriptorProto {
                    name: Some("Service".into()),
                    method: vec![MethodDescriptorProto {
                        name: Some("Echo".into()),
                        input_type: Some(format!(".{package}.Request")),
                        output_type: Some(format!(".{package}.{response_name}")),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        let mut config = config();
        config.services = vec![format!("{package}.Service")];
        config.descriptor = GrpcDescriptorSource::Inline {
            base64: STANDARD.encode(set.encode_to_vec()),
        };
        config
    }
    let mut value = field("value", 1, Type::String, false);
    value.default_value = Some("x".repeat(128 * 1024));
    let mut children = field("children", 1, Type::Message, true);
    children.type_name = Some(".bad.Child".into());
    let response = DescriptorProto {
        name: Some("Response".into()),
        field: vec![children],
        ..Default::default()
    };
    let child = DescriptorProto {
        name: Some("Child".into()),
        field: vec![value],
        ..Default::default()
    };
    assert!(descriptor(response.clone(), Some(child), "bad")
        .compile()
        .unwrap_err()
        .contains("defaults"));
    let child = DescriptorProto {
        name: Some("Child".into()),
        field: vec![field("value", 2, Type::String, false)],
        options: Some(MessageOptions {
            map_entry: Some(true),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(descriptor(response, Some(child), "bad")
        .compile()
        .unwrap_err()
        .contains("map entry"));
    let response = DescriptorProto {
        name: Some("Response".into()),
        extension_range: vec![prost_types::descriptor_proto::ExtensionRange {
            start: Some(100),
            end: Some(200),
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(descriptor(response, None, "bad")
        .compile()
        .unwrap_err()
        .contains("extensions"));
    let response = DescriptorProto {
        name: Some("Struct".into()),
        field: vec![field("fields", 1, Type::Bytes, false)],
        ..Default::default()
    };
    assert!(descriptor(response, None, "google.protobuf")
        .compile()
        .unwrap_err()
        .contains("well-known"));
}

#[test]
fn descriptor_namespace_expansion_is_admitted_before_reflection() {
    for package in ["a.".repeat(100_000) + "a", "a.".repeat(20) + "a", "a".repeat(129)] {
        let set = prost_types::FileDescriptorSet {
            file: vec![prost_types::FileDescriptorProto {
                name: Some("namespace.proto".into()),
                package: Some(package),
                syntax: Some("proto3".into()),
                ..Default::default()
            }],
        };
        let mut config = config();
        config.descriptor = GrpcDescriptorSource::Inline {
            base64: STANDARD.encode(set.encode_to_vec()),
        };
        let error = config.compile().unwrap_err();
        assert!(
            error.contains("descriptor namespace") || error.contains("descriptor identifier"),
            "{error}"
        );
    }
}

#[test]
fn request_errors_distinguish_mapping_and_protobuf_payload_limits() {
    let c = config().compile().unwrap();
    assert!(matches!(
        c.bind("GET", "/unmapped", None),
        Err(TranscodingError::NotFound)
    ));
    let mut limits = c.limits;
    limits.request = 1;
    assert!(matches!(
        c.bind("POST", "/v1/hello", None)
            .unwrap()
            .encode(br#"{"name":"valid"}"#, limits),
        Err(TranscodingError::TooLarge)
    ));
    assert_eq!(TranscodingError::Invalid.request_status(), 400);
    assert_eq!(TranscodingError::NotFound.request_status(), 404);
    assert_eq!(TranscodingError::TooLarge.request_status(), 413);
}

#[test]
fn descriptor_defaults_are_rejected_before_custom_option_reflection() {
    use prost_types::field_descriptor_proto::Type;
    for kind in [Type::String, Type::Bytes] {
        for extension in [false, true] {
            let field = prost_types::FieldDescriptorProto {
                name: Some("value".into()),
                r#type: Some(kind as i32),
                default_value: Some("x".repeat(128 * 1024)),
                ..Default::default()
            };
            let set = prost_types::FileDescriptorSet {
                file: vec![prost_types::FileDescriptorProto {
                    name: Some("default.proto".into()),
                    message_type: if extension {
                        vec![]
                    } else {
                        vec![prost_types::DescriptorProto {
                            name: Some("OptionValue".into()),
                            field: vec![field.clone()],
                            ..Default::default()
                        }]
                    },
                    extension: if extension { vec![field] } else { vec![] },
                    ..Default::default()
                }],
            };
            assert!(descriptor::admit(&set.encode_to_vec())
                .unwrap_err()
                .contains("defaults"));
        }
    }
}

#[test]
fn descriptor_custom_and_uninterpreted_options_are_rejected_before_reflection() {
    fn varint(mut n: u64) -> Vec<u8> {
        let mut bytes = Vec::new();
        while n >= 128 {
            bytes.push((n as u8 & 127) | 128);
            n >>= 7;
        }
        bytes.push(n as u8);
        bytes
    }
    fn field(number: u64, value: &[u8]) -> Vec<u8> {
        let mut bytes = varint(number << 3 | 2);
        bytes.extend(varint(value.len() as u64));
        bytes.extend(value);
        bytes
    }
    let pool = descriptor::admission_pool();
    let options = pool.get_message_by_name("google.protobuf.MethodOptions").unwrap();
    let limits = config().limits().unwrap();
    // A small physical payload can contain more reflection values than the cold node budget.
    let packed = field(1, &vec![0; 20_000]);
    assert!(wire::preflight_descriptor(&options, &field(1001, &packed), limits).is_err());
    let mut nested = Vec::new();
    for _ in 0..40 {
        nested = field(1, &nested);
    }
    assert!(wire::preflight_descriptor(&options, &field(1001, &nested), limits).is_err());
    let uninterpreted = prost_types::MethodOptions {
        uninterpreted_option: vec![prost_types::UninterpretedOption {
            name: (0..40)
                .map(|_| prost_types::uninterpreted_option::NamePart {
                    name_part: "child".into(),
                    is_extension: true,
                })
                .collect(),
            identifier_value: Some("true".into()),
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(wire::preflight_descriptor(&options, &uninterpreted.encode_to_vec(), limits).is_err());
    // HTTP annotations remain supported and their descendants consume the same budget.
    let rule = field(4, b"/v1/hello");
    let annotation = field(72_295_728, &rule);
    assert!(wire::preflight_descriptor(&options, &annotation, limits).is_ok());
    let mut tight = limits;
    tight.nodes = 2;
    assert!(wire::preflight_descriptor(&options, &annotation, tight).is_err());
    let mut recursive_rule = rule;
    for _ in 0..40 {
        recursive_rule = field(11, &recursive_rule);
    }
    let annotation = field(72_295_728, &recursive_rule);
    assert!(wire::preflight_descriptor(&options, &annotation, limits).is_err());
    let mut deeper = limits;
    deeper.depth = 64;
    assert!(wire::preflight_descriptor(&options, &annotation, deeper).is_ok());
}
#[test]
fn descriptor_reserved_option_shapes_cannot_shadow_admitted_wire() {
    use prost_types::field_descriptor_proto::{Label, Type};
    let mut set = prost_types::FileDescriptorSet::decode(
        include_bytes!("../../../../edgion-tests/proto/grpc_json_transcoding/test.pb").as_slice(),
    )
    .unwrap();
    let annotation = set
        .file
        .iter_mut()
        .find(|file| file.name.as_deref() == Some("google/api/annotations.proto"))
        .unwrap();
    annotation.extension[0].type_name = Some(".google.api.CustomHttpPattern".into());
    assert!(descriptor::admit(&set.encode_to_vec())
        .unwrap_err()
        .contains("HTTP option extension"));
    let mut set = prost_types::FileDescriptorSet::decode(
        include_bytes!("../../../../edgion-tests/proto/grpc_json_transcoding/test.pb").as_slice(),
    )
    .unwrap();
    let http = set
        .file
        .iter_mut()
        .find(|file| file.name.as_deref() == Some("google/api/http.proto"))
        .unwrap();
    let rule = http
        .message_type
        .iter_mut()
        .find(|message| message.name.as_deref() == Some("HttpRule"))
        .unwrap();
    rule.field
        .iter_mut()
        .find(|field| field.name.as_deref() == Some("selector"))
        .unwrap()
        .r#type = Some(Type::Int32 as i32);
    assert!(descriptor::admit(&set.encode_to_vec())
        .unwrap_err()
        .contains("reserved descriptor shape"));
    let relative = prost_types::FileDescriptorSet {
        file: vec![prost_types::FileDescriptorProto {
            name: Some("relative.proto".into()),
            package: Some("google.protobuf".into()),
            extension: vec![prost_types::FieldDescriptorProto {
                name: Some("shadow".into()),
                extendee: Some("FileOptions".into()),
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    assert!(descriptor::admit(&relative.encode_to_vec())
        .unwrap_err()
        .contains("absolute"));
    let extension = prost_types::FieldDescriptorProto {
        name: Some("shadow".into()),
        number: Some(1),
        r#type: Some(Type::Message as i32),
        label: Some(Label::Optional as i32),
        extendee: Some(".google.protobuf.FileOptions".into()),
        type_name: Some(".google.api.HttpRule".into()),
        ..Default::default()
    };
    let set = prost_types::FileDescriptorSet {
        file: vec![prost_types::FileDescriptorProto {
            name: Some("shadow.proto".into()),
            extension: vec![extension],
            ..Default::default()
        }],
    };
    assert!(descriptor::admit(&set.encode_to_vec())
        .unwrap_err()
        .contains("shadows a reserved field"));
}

#[test]
fn configmap_descriptor_rejects_malformed_names_before_valid_material() {
    for name in ["", "bad/name", "Bad_Name", ".schema", "schema.", "a..b"] {
        let mut c = config();
        c.descriptor = GrpcDescriptorSource::ConfigMapRef {
            name: name.into(),
            key: "test.pb".into(),
        };
        c.resolved_descriptor = Some(DESCRIPTOR.to_vec());
        assert_eq!(
            c.validation_error().as_deref(),
            Some("invalid descriptor ConfigMap reference")
        );
        assert_eq!(
            c.descriptor_bytes().unwrap_err(),
            "invalid descriptor ConfigMap reference"
        );
        assert_eq!(
            c.compile().err().as_deref(),
            Some("invalid descriptor ConfigMap reference")
        );
    }
    let mut c = config();
    c.descriptor = GrpcDescriptorSource::ConfigMapRef {
        name: "schema.v1".into(),
        key: "test.pb".into(),
    };
    c.resolved_descriptor = Some(DESCRIPTOR.to_vec());
    assert!(c.compile().is_ok());
    c.max_message_depth = 0;
    assert_eq!(
        c.validation_error().as_deref(),
        Some("message complexity exceeds depth 32 or nodes 16384")
    );
}

#[test]
fn correction_null_presence_and_collection_sentinels() {
    let c = config().compile().unwrap();
    let encode = |body: &[u8]| {
        c.bind("POST", "/test.CodecTest/Scalars", None)
            .unwrap()
            .encode(body, c.limits)
    };
    for body in [
        br#"{"optional_count":null}"#.as_slice(),
        br#"{"values":null,"valueMap":null}"#,
    ] {
        assert_eq!(&encode(body).unwrap().1[5..], b"");
    }
    let (b, f) = encode(br#"{"count":null,"text":"x"}"#).unwrap();
    let m = prost_reflect::DynamicMessage::decode(b.method.input(), &f[5..]).unwrap();
    assert!(!m.has_field_by_name("count"));
    assert!(m.has_field_by_name("text"));
    let (b, f) = encode(br#"{"value":null,"values":[null],"valueMap":{"x":null}}"#).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&b.decode(&f[5..], c.limits, None).unwrap()).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"value":null,"values":[null],"valueMap":{"x":null}})
    );
    for body in [
        br#"{"unknown":null}"#.as_slice(),
        br#"{"optional_count":null,"optionalCount":null}"#,
        br#"{"parentMessage":{"unknown":null}}"#,
    ] {
        assert!(encode(body).is_err());
    }
}

#[test]
fn correction_numeric_ranges_exact_exponents_and_wrappers() {
    let c = config().compile().unwrap();
    for (field, good, bad) in [
        ("signed", "9.223372036854775807e18", "9223372036854775808"),
        ("unsigned", "184467440737095516150e-1", "18446744073709551616"),
        ("int32Number", "2147483647.0", "2147483648"),
        ("uint32Number", "4294967295e0", "4294967296"),
        ("signedZigzag", "-9223372036854775808.0", "-9223372036854775809"),
        ("signedFixed", "1e2", "1.1"),
        ("unsignedFixed", "100e-2", "-1"),
        ("sint32Number", "1.00", "1e-1"),
        ("sfixed32Number", "-2147483648e0", "-2147483649"),
        ("fixed32Number", "1e2", "4294967296"),
    ] {
        for (value, valid) in [(good, true), (bad, false)] {
            let body = serde_json::to_vec(&serde_json::json!({field:value})).unwrap();
            assert_eq!(
                c.bind("POST", "/test.CodecTest/Scalars", None)
                    .unwrap()
                    .encode(&body, c.limits)
                    .is_ok(),
                valid,
                "{field} {value}"
            );
        }
    }
    for body in [
        br#"{"signed":9223372036854775808.0}"#.as_slice(),
        br#"{"unsigned":18446744073709551616.0}"#,
        br#"{"signedList":[9223372036854775808.0]}"#,
        br#"{"unsignedMap":{"x":18446744073709551616.0}}"#,
    ] {
        assert!(c
            .bind("POST", "/test.CodecTest/Scalars", None)
            .unwrap()
            .encode(body, c.limits)
            .is_err());
    }
    for (method, value, valid) in [
        ("Signed", "9223372036854775808.0", false),
        ("Unsigned", "18446744073709551616.0", false),
        ("Signed", "\"1e2\"", true),
        ("Unsigned", "\"18446744073709551615.0\"", true),
    ] {
        assert_eq!(
            c.bind("POST", &format!("/test.CodecTest/{method}"), None)
                .unwrap()
                .encode(value.as_bytes(), c.limits)
                .is_ok(),
            valid
        );
    }
}

#[test]
fn correction_float_overflow_and_nonfinite_output_all_envelopes() {
    let c = config().compile().unwrap();
    for method in ["Double", "Float"] {
        let b = c
            .bind("POST", &format!("/test.CodecTest/{method}"), None)
            .unwrap()
            .binding;
        for special in ["NaN", "Infinity", "-Infinity"] {
            let body = serde_json::to_vec(special).unwrap();
            let (_, f) = c
                .bind("POST", &b.rpc_path, None)
                .unwrap()
                .encode(&body, c.limits)
                .unwrap();
            for format in [
                None,
                Some(GrpcJsonStreamFormat::Ndjson),
                Some(GrpcJsonStreamFormat::Sse),
            ] {
                let out = String::from_utf8(b.decode(&f[5..], c.limits, format).unwrap()).unwrap();
                assert!(out.contains(&format!("\"{special}\"")), "{out}");
            }
        }
        for value in ["1e999", "-1e999"] {
            assert!(c
                .bind("POST", &b.rpc_path, None)
                .unwrap()
                .encode(&serde_json::to_vec(value).unwrap(), c.limits)
                .is_err());
        }
    }
    for body in [
        br#"{"floatNumber":"3.502823e38"}"#.as_slice(),
        br#"{"doubleNumber":"-1e999"}"#,
        br#"{"floatList":["-3.502823e38"]}"#,
        br#"{"doubleMap":{"x":"1e999"}}"#,
        br#"{"floats":["3.502823e38"]}"#,
        br#"{"doubles":{"x":"-1e999"}}"#,
    ] {
        assert!(c
            .bind("POST", "/test.CodecTest/Scalars", None)
            .unwrap()
            .encode(body, c.limits)
            .is_err());
    }
    let body = br#"{"doubleValue":"Infinity","floatList":["NaN","-Infinity"],"doubleMap":{"x":"Infinity"},"parentMessage":{"doubleValue":"-Infinity","floatList":["Infinity"],"doubleMap":{"x":"NaN"}}}"#;
    let (b, f) = c
        .bind("POST", "/test.CodecTest/Scalars", None)
        .unwrap()
        .encode(body, c.limits)
        .unwrap();
    for format in [
        None,
        Some(GrpcJsonStreamFormat::Ndjson),
        Some(GrpcJsonStreamFormat::Sse),
    ] {
        let out = b.decode(&f[5..], c.limits, format).unwrap();
        let text = std::str::from_utf8(&out).unwrap().trim();
        let text = text.strip_prefix("event: rpc_message\ndata: ").unwrap_or(text);
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        let parsed = if format == Some(GrpcJsonStreamFormat::Ndjson) {
            &parsed["data"]
        } else {
            &parsed
        };
        assert_eq!(parsed, &serde_json::from_slice::<serde_json::Value>(body).unwrap());
        let mut tight = c.limits;
        tight.json = 5;
        assert!(b.decode(&f[5..], tight, format).is_err());
    }
    for (method, finite, overflow) in [
        ("Float", "3.4028234663852886e38", "3.502823e38"),
        ("Double", "1.7976931348623157e308", "1.8e308"),
    ] {
        for sign in ["", "-"] {
            for (number, accepted) in [(finite, true), (overflow, false)] {
                let text = format!("{sign}{number}");
                assert_eq!(
                    c.bind("POST", &format!("/test.CodecTest/{method}"), None)
                        .unwrap()
                        .encode(&serde_json::to_vec(&text).unwrap(), c.limits)
                        .is_ok(),
                    accepted
                );
                let value = if method == "Float" {
                    serde_json::json!({"floatNumber":text,"floats":[text],"floatList":[text]})
                } else {
                    serde_json::json!({"doubleNumber":text,"doubles":{"x":text},"doubleValue":text,"doubleMap":{"x":text}})
                };
                assert_eq!(
                    c.bind("POST", "/test.CodecTest/Scalars", None)
                        .unwrap()
                        .encode(&serde_json::to_vec(&value).unwrap(), c.limits)
                        .is_ok(),
                    accepted
                );
            }
        }
    }
}

#[test]
fn correction_wkt_expansion_admitted_before_reflection() {
    let c = config().compile().unwrap();
    for (method, body, nodes) in [
        ("Mask", br#""a,b,c""#.as_slice(), 2),
        ("Object", br#"{"a":1}"#, 3),
        ("List", br#"[1,2]"#, 3),
        ("Value", br#"{"a":[1]}"#, 6),
    ] {
        let mut tight = c.limits;
        tight.nodes = nodes;
        assert!(
            c.bind("POST", &format!("/test.CodecTest/{method}"), None)
                .unwrap()
                .encode(body, tight)
                .is_err(),
            "{method}"
        );
        assert!(c
            .bind("POST", &format!("/test.CodecTest/{method}"), None)
            .unwrap()
            .encode(body, c.limits)
            .is_ok());
    }
}

#[test]
fn correction_nested_binding_accepts_both_parent_spellings() {
    let c = config().compile().unwrap();
    let method = c
        .bind("POST", "/test.CodecTest/Scalars", None)
        .unwrap()
        .binding
        .method
        .clone();
    let b = Arc::new(CompiledBinding::compile(method, "POST", "/parent/{parent_message.signed}", "*").unwrap());
    let mut frames = Vec::new();
    for body in [
        br#"{"parent_message":{"text":"x"}}"#.as_slice(),
        br#"{"parentMessage":{"text":"x"}}"#,
    ] {
        frames.push(
            b.bind("/parent/100", None, false)
                .unwrap()
                .encode(body, c.limits)
                .unwrap()
                .1,
        );
    }
    assert_eq!(frames[0], frames[1]);
    for body in [
        br#"{"parent_message":{"signed":"1"}}"#.as_slice(),
        br#"{"parent_message":{},"parentMessage":{}}"#,
    ] {
        assert!(b
            .bind("/parent/100", None, false)
            .unwrap()
            .encode(body, c.limits)
            .is_err());
    }
}

#[test]
fn correction_publication_rejects_intersecting_http_templates() {
    let mut c = config();
    c.services = vec!["test.OverlapTest".into()];
    assert!(c.compile().unwrap_err().contains("Overlap"));
    assert!(c.validation_error().unwrap().contains("Overlap"));
}

#[test]
fn correction_owner_fixture_plugins_pass_shared_validation() {
    use crate::resources::edgion_plugins::EdgionPlugins;
    use serde::Deserialize;
    let fixture = include_str!(
        "../../../../edgion-tests/integration/conf/EdgionPlugins/GrpcJsonTranscoder/60_correction_owners.yaml"
    );
    for doc in serde_yaml::Deserializer::from_str(fixture) {
        let value = serde_yaml::Value::deserialize(doc).unwrap();
        if value["kind"].as_str() == Some("EdgionPlugins") {
            let resource: EdgionPlugins = serde_yaml::from_value(value).unwrap();
            assert!(
                resource.validate_plugins().is_empty(),
                "{:?}",
                resource.validate_plugins()
            );
        }
    }
}

#[test]
fn correction_scalar_presence_proto2_and_integer_wire_boundaries() {
    use prost_types::{
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet, MethodDescriptorProto,
        ServiceDescriptorProto,
    };
    let set = FileDescriptorSet {
        file: vec![FileDescriptorProto {
            name: Some("presence.proto".into()),
            package: Some("presence".into()),
            syntax: Some("proto2".into()),
            message_type: vec![DescriptorProto {
                name: Some("Request".into()),
                field: vec![FieldDescriptorProto {
                    name: Some("count".into()),
                    number: Some(1),
                    label: Some(1),
                    r#type: Some(5),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            service: vec![ServiceDescriptorProto {
                name: Some("Service".into()),
                method: vec![MethodDescriptorProto {
                    name: Some("Echo".into()),
                    input_type: Some(".presence.Request".into()),
                    output_type: Some(".presence.Request".into()),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    let mut cfg = config();
    cfg.descriptor = GrpcDescriptorSource::Inline {
        base64: STANDARD.encode(set.encode_to_vec()),
    };
    cfg.services = vec!["presence.Service".into()];
    let c = cfg.compile().unwrap();
    let (b, f) = c
        .bind("POST", "/presence.Service/Echo", None)
        .unwrap()
        .encode(br#"{"count":null}"#, c.limits)
        .unwrap();
    assert_eq!(&f[5..], b"");
    assert!(!prost_reflect::DynamicMessage::decode(b.method.input(), &f[5..])
        .unwrap()
        .has_field_by_name("count"));
    let c = config().compile().unwrap();
    for (method, text, expected) in [
        (
            "Signed",
            "\"9.223372036854775807e18\"",
            prost_reflect::Value::I64(i64::MAX),
        ),
        (
            "Unsigned",
            "\"184467440737095516150e-1\"",
            prost_reflect::Value::U64(u64::MAX),
        ),
        (
            "Signed",
            "9223372036854774784.0",
            prost_reflect::Value::I64(9223372036854774784),
        ),
        (
            "Unsigned",
            "18446744073709549568.0",
            prost_reflect::Value::U64(18446744073709549568),
        ),
    ] {
        let (b, f) = c
            .bind("POST", &format!("/test.CodecTest/{method}"), None)
            .unwrap()
            .encode(text.as_bytes(), c.limits)
            .unwrap();
        let message = prost_reflect::DynamicMessage::decode(b.method.input(), &f[5..]).unwrap();
        assert_eq!(*message.get_field_by_name("value").unwrap(), expected);
    }
    let b = c.bind("POST", "/test.CodecTest/Value", None).unwrap().binding;
    let mut wire = vec![17];
    wire.extend_from_slice(&f64::INFINITY.to_le_bytes());
    assert!(b.decode(&wire, c.limits, None).is_err());
}

#[test]
fn bound_values_outlive_plain_and_escaped_mapping_inputs() {
    let compiled = config().compile().unwrap();
    for (name, expected) in [("plain", "plain"), ("hello%20world", "hello world")] {
        let bound = {
            let path = format!("/v1/hello/{name}");
            compiled.bind("GET", &path, None).unwrap()
        };
        let (binding, frame) = bound.encode(b"", compiled.limits).unwrap();
        let message = prost_reflect::DynamicMessage::decode(binding.method.input(), &frame[5..]).unwrap();
        assert_eq!(message.get_field_by_name("name").unwrap().as_str(), Some(expected));
    }
    let method = compiled
        .bind("GET", "/v1/hello/name", None)
        .unwrap()
        .binding
        .method
        .clone();
    let binding = Arc::new(CompiledBinding::compile(method, "GET", "/query", "").unwrap());
    for (query, expected) in [("name=plain", "plain"), ("name=hello+world%26%C3%A9", "hello world&é")] {
        let bound = {
            let query = query.to_owned();
            binding.bind("/query", Some(&query), false).unwrap()
        };
        let (_, frame) = bound.encode(b"", compiled.limits).unwrap();
        let message = prost_reflect::DynamicMessage::decode(binding.method.input(), &frame[5..]).unwrap();
        assert_eq!(message.get_field_by_name("name").unwrap().as_str(), Some(expected));
    }
}

#[test]
fn ignored_unknown_queries_still_require_strict_percent_and_utf8_decoding() {
    let mut config = config();
    config.ignore_unknown_query_parameters = true;
    let compiled = config.compile().unwrap();
    assert!(compiled
        .bind("GET", "/v1/numbers", Some("count=3&unknown=hello+world%26more"))
        .is_ok());
    for query in [
        "count=3&unknown=%",
        "count=3&unknown=%2",
        "count=3&unknown=%XY",
        "count=3&unknown=%FF",
        "%FF=ignored",
    ] {
        assert!(
            matches!(
                compiled.bind("GET", "/v1/numbers", Some(query)),
                Err(TranscodingError::Invalid)
            ),
            "{query}"
        );
    }
}

#[test]
fn selected_binding_outlives_compiled_owner_and_preserves_ambiguity_gate() {
    let (bound, limits) = {
        let compiled = config().compile().unwrap();
        (compiled.bind("GET", "/v1/hello/owned", None).unwrap(), compiled.limits)
    };
    let (binding, frame) = bound.encode(b"", limits).unwrap();
    assert_eq!(binding.rpc_path, "/test.TestService/SayHello");
    let message = prost_reflect::DynamicMessage::decode(binding.method.input(), &frame[5..]).unwrap();
    assert_eq!(message.get_field_by_name("name").unwrap().as_str(), Some("owned"));

    let mut compiled = config().compile().unwrap();
    assert!(matches!(
        compiled.bind("DELETE", "/v1/hello/owned", None),
        Err(TranscodingError::NotFound)
    ));
    assert!(matches!(
        compiled.bind("GET", "/unmapped", None),
        Err(TranscodingError::NotFound)
    ));
    assert!(matches!(
        compiled.bind("GET", &"/".repeat(65537), None),
        Err(TranscodingError::Invalid)
    ));
    // Bypass cold disjointness admission only to exercise the runtime defense.
    let duplicate = compiled
        .bindings
        .iter()
        .find(|binding| binding.http_method == "GET" && binding.regex.is_match("/v1/hello/owned"))
        .unwrap()
        .clone();
    compiled.bindings.push(duplicate);
    assert!(matches!(
        compiled.bind("GET", "/v1/hello/owned", None),
        Err(TranscodingError::Invalid)
    ));
}
