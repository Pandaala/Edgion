#[cfg(test)]
pub(crate) fn test_response_processor(
    owner: std::sync::Arc<AttachedExtProc>,
    failure_mode_allow: bool,
) -> Box<dyn crate::plugins::runtime::traits::StreamingResponseProcessor> {
    Box::new(response_streamed::ExtProcResponseProcessor::new(
        owner,
        failure_mode_allow,
        false,
        false,
    ))
}
