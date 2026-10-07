//! Explicit malformed native-gRPC fixtures for the REST transcoder suite.
use bytes::Bytes;
use h2::server::SendResponse;

pub(super) fn respond(respond: &mut SendResponse<Bytes>, mode: &str, streaming: bool) {
    if mode.starts_with("header-only-") {
        let mut response = http::Response::builder()
            .status(200)
            .header("content-type", "application/grpc+proto");
        if mode != "header-only-missing" {
            response = response.header("grpc-status", if mode == "header-only-error" { "7" } else { "invalid" });
        }
        let _ = respond.send_response(response.body(()).unwrap(), true);
        return;
    }
    let response = http::Response::builder()
        .status(200)
        .header("content-type", "application/grpc+proto")
        .body(())
        .unwrap();
    let Ok(mut send) = respond.send_response(response, false) else {
        return;
    };
    let payload: &[u8] = if streaming { &[8, 1] } else { &[10, 2, b'o', b'k'] };
    let mut frame = vec![u8::from(mode == "compressed")];
    let len = if mode == "truncated" {
        payload.len() + 1
    } else {
        payload.len()
    };
    frame.extend_from_slice(&(len as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    let eos = matches!(mode, "missing-status" | "truncated");
    if send.send_data(Bytes::from(frame), eos).is_err() || eos {
        return;
    }
    let mut trailers = http::HeaderMap::new();
    let status = match mode {
        "bad-status" => "invalid",
        "error-status" => "7",
        _ => "0",
    };
    trailers.insert("grpc-status", status.parse().unwrap());
    trailers.insert("grpc-message", "private fixture detail".parse().unwrap());
    let _ = send.send_trailers(trailers);
}
