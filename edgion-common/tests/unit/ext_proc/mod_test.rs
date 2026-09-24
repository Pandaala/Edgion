use prost::Message;

use super::proto::{processing_mode, GrpcStatus, ImmediateResponse, ProcessingMode, ProcessingResponse};

#[test]
fn immediate_grpc_status_uses_official_field_numbers() {
    let encoded = ImmediateResponse {
        grpc_status: Some(GrpcStatus { status: 7 }),
        ..Default::default()
    }
    .encode_to_vec();

    assert_eq!(encoded, [0x22, 0x02, 0x08, 0x07]);
    let decoded = ImmediateResponse::decode(encoded.as_slice()).unwrap();
    assert_eq!(decoded.grpc_status.unwrap().status, 7);
}

#[test]
fn mode_override_uses_official_field_and_enum_numbers() {
    let encoded = ProcessingResponse {
        mode_override: Some(ProcessingMode {
            request_header_mode: processing_mode::HeaderSendMode::Send as i32,
            response_header_mode: processing_mode::HeaderSendMode::Skip as i32,
            request_body_mode: processing_mode::BodySendMode::Buffered as i32,
            response_body_mode: processing_mode::BodySendMode::Streamed as i32,
            request_trailer_mode: processing_mode::HeaderSendMode::Default as i32,
            response_trailer_mode: processing_mode::HeaderSendMode::Skip as i32,
        }),
        ..Default::default()
    }
    .encode_to_vec();

    assert_eq!(
        encoded,
        [0x4a, 0x0a, 0x08, 0x01, 0x10, 0x02, 0x18, 0x02, 0x20, 0x01, 0x30, 0x02]
    );
    let decoded = ProcessingResponse::decode(encoded.as_slice()).unwrap();
    assert_eq!(
        decoded.mode_override.unwrap().request_body_mode,
        processing_mode::BodySendMode::Buffered as i32
    );
    assert_eq!(processing_mode::BodySendMode::Grpc as i32, 5);
    assert_eq!(
        ProcessingMode {
            request_body_mode: processing_mode::BodySendMode::Grpc as i32,
            ..Default::default()
        }
        .encode_to_vec(),
        [0x18, 0x05]
    );
}
