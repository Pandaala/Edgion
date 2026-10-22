use super::*;

fn limits(nodes: u64) -> TranscodingLimits {
    TranscodingLimits {
        request: 1024,
        response: 1024,
        json: 1024,
        depth: 32,
        nodes,
    }
}

fn scalar(kind: &Kind, mut bytes: &[u8], limits: TranscodingLimits, nodes: &mut u64) -> Result<(), TranscodingError> {
    while !bytes.is_empty() {
        count(nodes, limits)?;
        match kind {
            Kind::Double | Kind::Fixed64 | Kind::Sfixed64 => {
                take(&mut bytes, 8)?;
            }
            Kind::Float | Kind::Fixed32 | Kind::Sfixed32 => {
                take(&mut bytes, 4)?;
            }
            _ => {
                varint(&mut bytes)?;
            }
        }
    }
    Ok(())
}

#[test]
fn packed_admission_matches_scalar_results_and_partial_node_charges() {
    for kind in [
        Kind::Double,
        Kind::Fixed64,
        Kind::Sfixed64,
        Kind::Float,
        Kind::Fixed32,
        Kind::Sfixed32,
        Kind::Int32,
        Kind::Uint64,
        Kind::Bool,
    ] {
        for len in (0..=65).chain([1024, 65536]) {
            for fill in [0, 0x80, 0xff] {
                let bytes = vec![fill; len];
                for start in [0, 2, 10, u64::MAX - 2, u64::MAX] {
                    for budget in [0, 1, 2, 3, 4, 9, 10, 11, 128, 16383, 16384, 16385, u64::MAX] {
                        let mut expected_nodes = start;
                        let expected = scalar(&kind, &bytes, limits(budget), &mut expected_nodes);
                        let mut actual_nodes = start;
                        let actual = packed(&kind, &bytes, limits(budget), &mut actual_nodes);
                        assert_eq!(
                            (actual, actual_nodes),
                            (expected, expected_nodes),
                            "{kind:?}, len={len}, fill={fill}, start={start}, budget={budget}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn fixed_packed_fields_keep_both_walker_modes_and_node_bounds() {
    use prost_types::{DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet};
    let pool = prost_reflect::DescriptorPool::from_file_descriptor_set(FileDescriptorSet {
        file: vec![FileDescriptorProto {
            name: Some("fixed.proto".into()),
            syntax: Some("proto3".into()),
            message_type: vec![DescriptorProto {
                name: Some("Fixed".into()),
                field: vec![FieldDescriptorProto {
                    name: Some("values".into()),
                    number: Some(1),
                    label: Some(prost_types::field_descriptor_proto::Label::Repeated as i32),
                    r#type: Some(prost_types::field_descriptor_proto::Type::Fixed32 as i32),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    })
    .unwrap();
    let desc = pool.get_message_by_name("Fixed").unwrap();
    for len in 0..=17 {
        let mut bytes = vec![10, len as u8];
        bytes.resize(len + 2, 0xff);
        for budget in 0..=8 {
            let expected = if len % 4 == 0 && budget >= 2 + (len / 4) as u64 {
                Ok(())
            } else {
                Err(TranscodingError::Invalid)
            };
            assert_eq!(preflight(&desc, &bytes, limits(budget)), expected);
            assert_eq!(preflight_descriptor(&desc, &bytes, limits(budget)), expected);
        }
    }
}
