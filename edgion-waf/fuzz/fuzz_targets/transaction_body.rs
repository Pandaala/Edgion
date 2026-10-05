#![no_main]

use std::sync::OnceLock;

use edgion_waf::seclang::{
    compile_with_includes, BodyProcessorSelection, CompiledGeneration, FrontendLimits, IncludeFailure,
    IncludeRequest, IncludeSink, IncludeSource, Phase, PhaseDefaults, RequestBody, RequestHead, RequestHeader,
    RuntimeLimits, SemanticValidationContext, Transaction,
};
use libfuzzer_sys::fuzz_target;

struct NoIncludes;

impl IncludeSource for NoIncludes {
    fn load(&mut self, _request: IncludeRequest<'_>, _sink: &mut IncludeSink<'_>) -> Result<(), IncludeFailure> {
        Err(IncludeFailure::NotFound)
    }
}

fn generation() -> &'static CompiledGeneration {
    static GENERATION: OnceLock<CompiledGeneration> = OnceLock::new();
    GENERATION.get_or_init(|| {
        let defaults = [
            Phase::RequestHeaders,
            Phase::RequestBody,
            Phase::ResponseHeaders,
            Phase::ResponseBody,
            Phase::Logging,
        ]
        .map(|phase| PhaseDefaults::pass(phase, true, true));
        let context = SemanticValidationContext::try_new(Some(Phase::RequestBody), &defaults)
            .expect("fixed phase defaults must be valid");
        compile_with_includes(
            "transaction-body-fuzz",
            b"",
            &mut NoIncludes,
            None,
            context,
            FrontendLimits::default(),
        )
        .expect("fixed empty ruleset must compile")
        .into_parts()
        .0
    })
}

fuzz_target!(|data: &[u8]| {
    let Some((&selector, body)) = data.split_first() else {
        return;
    };
    let processor = match selector % 4 {
        0 => BodyProcessorSelection::UrlEncoded,
        1 => BodyProcessorSelection::Json,
        2 => BodyProcessorSelection::Multipart,
        _ => BodyProcessorSelection::Xml,
    };
    let headers = [RequestHeader::new(
        b"Content-Type",
        b"multipart/form-data; boundary=edge",
    )];
    let request = RequestHead::new(
        b"POST /fuzz HTTP/1.1",
        b"POST",
        b"/fuzz",
        b"/fuzz",
        b"HTTP/1.1",
        b"192.0.2.1",
        b"fuzz",
        &headers,
    );
    let Ok(mut transaction) = Transaction::new(generation(), request, RuntimeLimits::default()) else {
        return;
    };
    if transaction.execute_request_headers().is_err() {
        return;
    }
    if transaction
        .publish_request_body(RequestBody::new(bytes::Bytes::copy_from_slice(body), processor))
        .is_ok()
    {
        let _ = transaction.execute_request_body();
    }
});
