#[cfg(test)]
#[allow(clippy::result_large_err)]
fn response_status(
    response: &ProcessingResponse,
) -> Result<edgion_common::ext_proc::proto::common_response::ResponseStatus, tonic::Status> {
    let Some(common) = common_response(response) else {
        return Ok(edgion_common::ext_proc::proto::common_response::ResponseStatus::Continue);
    };
    edgion_common::ext_proc::proto::common_response::ResponseStatus::try_from(common.status)
        .map_err(|_| tonic::Status::failed_precondition("ext_proc processor returned an invalid response status"))
}
