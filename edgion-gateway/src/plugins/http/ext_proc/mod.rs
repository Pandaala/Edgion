mod attached;
mod client;
mod mutation;
mod plugin;
mod response_streamed;
mod response_trailer;
mod streamed;

pub(crate) use attached::AttachedExtProc;
pub use plugin::ExtProc;

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

#[cfg(test)]
pub(crate) const TEST_RESPONSE_WINDOW_CAP: usize = response_streamed::RESPONSE_WINDOW_CAP;
