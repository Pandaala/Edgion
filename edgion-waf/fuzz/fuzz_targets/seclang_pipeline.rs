#![no_main]

use edgion_waf::seclang::{
    compile_with_includes, BodyProcessorSelection, EvaluationMode, FrontendLimits, IncludeFailure, IncludeRequest,
    IncludeSink, IncludeSource, Phase, PhaseDefaults, RequestBody, RequestHead, RequestHeader, ResponseHead,
    ResponseHeader, ResponseWindow, RuleEngineMode, RuntimeLimits, SemanticValidationContext, Transaction,
};
use libfuzzer_sys::fuzz_target;

const MAX_SOURCE_BYTES: usize = 4 * 1024;
const MAX_PAYLOAD_BYTES: usize = 4 * 1024;

struct NoIncludes;

impl IncludeSource for NoIncludes {
    fn load(&mut self, _request: IncludeRequest<'_>, _sink: &mut IncludeSink<'_>) -> Result<(), IncludeFailure> {
        Err(IncludeFailure::NotFound)
    }
}

fuzz_target!(|data: &[u8]| {
    let (source, payload) = split_input(data);
    let defaults = [
        Phase::RequestHeaders,
        Phase::RequestBody,
        Phase::ResponseHeaders,
        Phase::ResponseBody,
        Phase::Logging,
    ]
    .map(|phase| PhaseDefaults::pass(phase, true, true));
    let context = SemanticValidationContext::try_new(Some(Phase::RequestBody), &defaults)
        .expect("fixed fuzz defaults must be valid");
    let Ok(bundle) = compile_with_includes(
        "seclang-pipeline-fuzz",
        source,
        &mut NoIncludes,
        None,
        context,
        FrontendLimits::default(),
    ) else {
        return;
    };
    let generation = bundle.into_parts().0;

    let headers = [
        RequestHeader::new(b"Content-Type", b"application/json"),
        RequestHeader::new(b"X-Fuzz", payload),
    ];
    let request = RequestHead::new(
        b"POST /fuzz?value=input HTTP/1.1",
        b"POST",
        b"/fuzz?value=input",
        b"/fuzz?value=input",
        b"HTTP/1.1",
        b"192.0.2.1",
        b"fuzz-request",
        &headers,
    );
    let limits = RuntimeLimits::default();
    let accelerated = Transaction::new_with_modes(
        &generation,
        request,
        limits,
        EvaluationMode::Accelerated,
        RuleEngineMode::On,
    );
    let reference = Transaction::new_with_modes(
        &generation,
        request,
        limits,
        EvaluationMode::Reference,
        RuleEngineMode::On,
    );
    let (mut accelerated, mut reference) = match (accelerated, reference) {
        (Ok(accelerated), Ok(reference)) => (accelerated, reference),
        (Err(left), Err(right)) => {
            assert_eq!(left, right);
            return;
        }
        _ => panic!("matcher modes disagreed while constructing a transaction"),
    };

    assert_eq!(
        accelerated.execute_request_headers(),
        reference.execute_request_headers()
    );
    let body = RequestBody::new(payload, BodyProcessorSelection::Auto);
    let accelerated_body = accelerated.publish_request_body(body);
    let reference_body = reference.publish_request_body(body);
    assert_eq!(accelerated_body, reference_body);
    if accelerated_body.is_err() {
        return;
    }
    assert_eq!(accelerated.execute_request_body(), reference.execute_request_body());

    let response_headers = [ResponseHeader::new(b"X-Fuzz", payload)];
    let response = ResponseHead::new(200, &response_headers);
    assert_eq!(
        accelerated.publish_response_head(response),
        reference.publish_response_head(response)
    );
    assert_eq!(
        accelerated.execute_response_headers(),
        reference.execute_response_headers()
    );
    let response_window = ResponseWindow::new(payload, true);
    assert_eq!(
        accelerated.execute_response_window(response_window),
        reference.execute_response_window(response_window)
    );
    assert_eq!(accelerated.execute_logging(), reference.execute_logging());
});

fn split_input(data: &[u8]) -> (&[u8], &[u8]) {
    let data = &data[..data.len().min(MAX_SOURCE_BYTES + MAX_PAYLOAD_BYTES + 2)];
    let Some(delimiter) = data.windows(2).position(|window| window == b"\n\n") else {
        return (&data[..data.len().min(MAX_SOURCE_BYTES)], &[]);
    };
    let source = &data[..delimiter.min(MAX_SOURCE_BYTES)];
    let payload_start = delimiter.saturating_add(2).min(data.len());
    let payload_end = payload_start.saturating_add(MAX_PAYLOAD_BYTES).min(data.len());
    (source, &data[payload_start..payload_end])
}
