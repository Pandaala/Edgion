#[cfg(test)]
fn common_response(response: &ProcessingResponse) -> Option<&edgion_common::ext_proc::proto::CommonResponse> {
    use processing_response::Response;

    match response.response.as_ref() {
        Some(Response::RequestHeaders(response)) | Some(Response::ResponseHeaders(response)) => {
            response.response.as_ref()
        }
        Some(Response::RequestBody(response)) | Some(Response::ResponseBody(response)) => response.response.as_ref(),
        _ => None,
    }
}
