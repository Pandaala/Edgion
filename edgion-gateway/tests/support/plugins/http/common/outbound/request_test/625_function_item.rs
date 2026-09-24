/// Reach a deterministic build failure after inspecting the header policy,
/// without opening a socket. Shared by consumer wiring regression tests.
#[cfg(test)]
pub(crate) fn test_presence_failure_request(presence: PresencePolicy) -> CompiledRequest {
    use edgion_resources::resources::common::outbound_request::{MethodSpec, OriginField, PathSpec};
    CompiledRequest::from_spec(&OutboundRequestSpec {
        method: Some(MethodSpec { template: "GET".into() }),
        path: Some(PathSpec {
            template: "/verify".into(),
            allow_override: false,
        }),
        headers: ListFields {
            origin: vec![OriginField {
                name: "x-private-metadata".into(),
                presence,
            }],
            ..Default::default()
        },
        cookies: ListFields {
            origin: vec![OriginField {
                name: "private-session".into(),
                presence: PresencePolicy::Required,
            }],
            ..Default::default()
        },
        ..Default::default()
    })
    .unwrap()
}
