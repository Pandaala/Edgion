use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use prost::Message;
use prost_types::{
    field_descriptor_proto::{Label, Type},
    DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet, MethodDescriptorProto,
    ServiceDescriptorProto,
};
#[test]
fn exact_decimal_scan_retains_integer_bits_and_bounds_exponents() {
    for (text, expected) in [
        ("9.223372036854775807e18", i64::MAX as i128),
        ("-9223372036854775808.0", i64::MIN as i128),
        ("184467440737095516150e-1", u64::MAX as i128),
        ("0e9999999999999999999999999", 0),
        ("-0e-9999999999999999999999999", 0),
        ("+1e+2", 100),
    ] {
        assert_eq!(exact_integer(text), Ok(expected));
    }
    for text in [
        "1.1",
        "1e-99999999999999999999999999",
        "1e99999999999999999999999999",
        "1e",
        "1e+",
        "1e2e3",
        "1.",
        "NaN",
    ] {
        assert!(exact_integer(text).is_err(), "{text}");
    }
}

#[test]
fn wkt_depth_and_many_field_mask_paths_fail_before_reflection() {
    let pool = prost_reflect::DescriptorPool::decode(
        include_bytes!("../../../../edgion-tests/proto/grpc_json_transcoding/test.pb").as_slice(),
    )
    .unwrap();
    let limits = TranscodingLimits {
        request: 1024 * 1024,
        response: 512 * 1024,
        json: 1024 * 1024,
        depth: 2,
        nodes: 16384,
    };
    let mut value = serde_json::json!({"a":1});
    assert_eq!(
        normalize(
            &pool.get_message_by_name("google.protobuf.Struct").unwrap(),
            &mut value,
            limits
        ),
        Err(Error::Invalid)
    );
    let mut value = serde_json::Value::String("a,".repeat(64));
    assert_eq!(
        normalize(
            &pool.get_message_by_name("google.protobuf.FieldMask").unwrap(),
            &mut value,
            TranscodingLimits { nodes: 16, ..limits }
        ),
        Err(Error::Invalid)
    );
}

#[test]
fn ordinary_value_suffix_cannot_bypass_admission_or_field_normalization() {
    let field = |name: &str, number, kind: Type, type_name: Option<&str>| FieldDescriptorProto {
        name: Some(name.into()),
        number: Some(number),
        label: Some(Label::Optional as i32),
        r#type: Some(kind as i32),
        type_name: type_name.map(str::to_owned),
        ..Default::default()
    };
    let mut descriptor = FileDescriptorSet::decode(
        include_bytes!("../../../../edgion-tests/proto/grpc_json_transcoding/test.pb").as_slice(),
    )
    .unwrap();
    descriptor.file.push(FileDescriptorProto {
        name: Some("ordinary_value.proto".into()),
        package: Some("google.protobuf".into()),
        syntax: Some("proto2".into()),
        dependency: vec!["google/protobuf/field_mask.proto".into()],
        message_type: vec![
            DescriptorProto {
                name: Some("CustomValue".into()),
                field: vec![
                    field("value", 1, Type::String, None),
                    field("mask", 2, Type::Message, Some(".google.protobuf.FieldMask")),
                    field("signed", 3, Type::Int64, None),
                    field("optional_count", 4, Type::Int32, None),
                ],
                ..Default::default()
            },
            DescriptorProto {
                name: Some("ValueContainer".into()),
                field: vec![field(
                    "custom_value",
                    1,
                    Type::Message,
                    Some(".google.protobuf.CustomValue"),
                )],
                ..Default::default()
            },
        ],
        service: vec![ServiceDescriptorProto {
            name: Some("AdmissionCodec".into()),
            method: vec![
                MethodDescriptorProto {
                    name: Some("Echo".into()),
                    input_type: Some(".google.protobuf.CustomValue".into()),
                    output_type: Some(".google.protobuf.CustomValue".into()),
                    ..Default::default()
                },
                MethodDescriptorProto {
                    name: Some("Nested".into()),
                    input_type: Some(".google.protobuf.ValueContainer".into()),
                    output_type: Some(".google.protobuf.ValueContainer".into()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }],
        ..Default::default()
    });
    let config: super::super::GrpcJsonTranscoderConfig = serde_json::from_value(serde_json::json!({
        "descriptor":{"type":"inline","base64":STANDARD.encode(descriptor.encode_to_vec())},
        "services":["google.protobuf.AdmissionCodec"], "autoMapping":true,
    }))
    .unwrap();
    let compiled = config.compile().unwrap();
    let root = compiled
        .bind("POST", "/google.protobuf.AdmissionCodec/Echo", None)
        .unwrap()
        .binding;
    for nodes in [2, 4] {
        let mut value = serde_json::json!({"mask":"a,b,c"});
        // Calling the admission pass directly proves rejection precedes reflection.
        assert_eq!(
            normalize(
                &root.method.input(),
                &mut value,
                TranscodingLimits {
                    nodes,
                    ..compiled.limits
                }
            ),
            Err(Error::Invalid)
        );
    }
    for (method, body, tight_nodes) in [
        ("Echo", serde_json::json!({"mask":"a,b,c"}), 4),
        ("Nested", serde_json::json!({"customValue":{"mask":"a,b,c"}}), 6),
    ] {
        let path = format!("/google.protobuf.AdmissionCodec/{method}");
        let body = serde_json::to_vec(&body).unwrap();
        assert!(compiled
            .bind("POST", &path, None)
            .unwrap()
            .encode(
                &body,
                TranscodingLimits {
                    nodes: tight_nodes,
                    ..compiled.limits
                }
            )
            .is_err());
        assert!(compiled
            .bind("POST", &path, None)
            .unwrap()
            .encode(&body, compiled.limits)
            .is_ok());
    }
    for (method, body) in [
        ("Echo", serde_json::json!({"signed":"1e2","optionalCount":null})),
        (
            "Nested",
            serde_json::json!({"custom_value":{"signed":"1e2","optional_count":null}}),
        ),
    ] {
        let path = format!("/google.protobuf.AdmissionCodec/{method}");
        let (binding, frame) = compiled
            .bind("POST", &path, None)
            .unwrap()
            .encode(&serde_json::to_vec(&body).unwrap(), compiled.limits)
            .unwrap();
        let expected: &[u8] = if method == "Echo" {
            &[24, 100]
        } else {
            &[10, 2, 24, 100]
        };
        assert_eq!(&frame[5..], expected);
        assert!(binding.decode(&frame[5..], compiled.limits, None).is_ok());
    }
    for body in [
        serde_json::json!({"signed":"9223372036854775808"}),
        serde_json::json!({"unknown":null}),
        serde_json::json!({"optional_count":null,"optionalCount":null}),
    ] {
        assert!(compiled
            .bind("POST", &root.rpc_path, None)
            .unwrap()
            .encode(&serde_json::to_vec(&body).unwrap(), compiled.limits)
            .is_err());
    }
}
