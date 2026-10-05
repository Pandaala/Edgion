use super::*;

#[test]
fn diagnostic_codes_serialize_as_stable_strings() {
    let info_codes = [
        (EdgionInfoCode::AcmeChallengeServed, "AcmeChallengeServed"),
        (EdgionInfoCode::PluginResponseReplaced, "PluginResponseReplaced"),
    ];
    let error_codes = [
        (EdgionErrorCode::Unknown, "Unknown"),
        (EdgionErrorCode::AccessLogSerializeFailed, "AccessLogSerializeFailed"),
        (
            EdgionErrorCode::AiResponseTranslationFailed,
            "AiResponseTranslationFailed",
        ),
        (
            EdgionErrorCode::AiQuotaReconciliationFailed,
            "AiQuotaReconciliationFailed",
        ),
        (
            EdgionErrorCode::AiQuotaOverLimitTerminated,
            "AiQuotaOverLimitTerminated",
        ),
        (EdgionErrorCode::AiQuotaErrorEventDropped, "AiQuotaErrorEventDropped"),
        (EdgionErrorCode::AiSseDecodeDegraded, "AiSseDecodeDegraded"),
        (
            EdgionErrorCode::AiSseDegradedQuotaTerminated,
            "AiSseDegradedQuotaTerminated",
        ),
        (EdgionErrorCode::AiSseDecodeMalformed, "AiSseDecodeMalformed"),
        (
            EdgionErrorCode::AiSseMalformedQuotaTerminated,
            "AiSseMalformedQuotaTerminated",
        ),
        (EdgionErrorCode::HostMissing, "HostMissing"),
        (EdgionErrorCode::XffHeaderTooLong, "XffHeaderTooLong"),
        (EdgionErrorCode::PathEscapedSlashRejected, "PathEscapedSlashRejected"),
        (
            EdgionErrorCode::PathInvalidPercentEncoding,
            "PathInvalidPercentEncoding",
        ),
        (EdgionErrorCode::RouteNotFound, "RouteNotFound"),
        (EdgionErrorCode::SniHostMismatch, "SniHostMismatch"),
        (EdgionErrorCode::ClientClosed, "ClientClosed"),
        (EdgionErrorCode::UpstreamNotRouteMatched, "UpstreamNotRouteMatched"),
        (EdgionErrorCode::UpstreamNotBackendRefs, "UpstreamNotBackendRefs"),
        (
            EdgionErrorCode::UpstreamInconsistentWeight,
            "UpstreamInconsistentWeight",
        ),
        (
            EdgionErrorCode::GrpcUpstreamNotRouteMatched,
            "GrpcUpstreamNotRouteMatched",
        ),
        (
            EdgionErrorCode::BackendEndpointSliceNotFoundByConsistent,
            "BackendEndpointSliceNotFoundByConsistent",
        ),
        (
            EdgionErrorCode::BackendEndpointSliceNotFoundByLeastConn,
            "BackendEndpointSliceNotFoundByLeastConn",
        ),
        (
            EdgionErrorCode::BackendEndpointSliceNotFoundByRoundRobin,
            "BackendEndpointSliceNotFoundByRoundRobin",
        ),
        (
            EdgionErrorCode::BackendEndpointSliceNotFoundByRoundRobinDefault,
            "BackendEndpointSliceNotFoundByRoundRobinDefault",
        ),
        (
            EdgionErrorCode::BackendEndpointSliceNotFoundByEwma,
            "BackendEndpointSliceNotFoundByEwma",
        ),
        (EdgionErrorCode::RetryBudgetExhausted, "RetryBudgetExhausted"),
        (
            EdgionErrorCode::UpstreamTransportRetryCeiling,
            "UpstreamTransportRetryCeiling",
        ),
        (EdgionErrorCode::BackendServiceNotFound, "BackendServiceNotFound"),
        (EdgionErrorCode::BackendClusterIpNotFound, "BackendClusterIpNotFound"),
        (
            EdgionErrorCode::BackendExternalNameNotFound,
            "BackendExternalNameNotFound",
        ),
        (
            EdgionErrorCode::BackendPortResolutionFailed,
            "BackendPortResolutionFailed",
        ),
        (
            EdgionErrorCode::BackendAddressParsingFailed,
            "BackendAddressParsingFailed",
        ),
        (
            EdgionErrorCode::BackendServiceImportNotImplemented,
            "BackendServiceImportNotImplemented",
        ),
        (
            EdgionErrorCode::BackendLocalhostNotAllowed,
            "BackendLocalhostNotAllowed",
        ),
        (EdgionErrorCode::Http2Required, "Http2Required"),
        (
            EdgionErrorCode::GrpcUpstreamNotBackendRefs,
            "GrpcUpstreamNotBackendRefs",
        ),
        (EdgionErrorCode::RefDenied, "RefDenied"),
        (
            EdgionErrorCode::BackendAddrLinkLocalNotAllowed,
            "BackendAddrLinkLocalNotAllowed",
        ),
        (
            EdgionErrorCode::BackendDnsResolutionFailed,
            "BackendDnsResolutionFailed",
        ),
        (EdgionErrorCode::BackendUnsupportedKind, "BackendUnsupportedKind"),
        (EdgionErrorCode::StickyBackendUnhealthy, "StickyBackendUnhealthy"),
        (
            EdgionErrorCode::BackendTlsSanValidationFailed,
            "BackendTlsSanValidationFailed",
        ),
        (EdgionErrorCode::BackendTlsCaInvalid, "BackendTlsCaInvalid"),
        (EdgionErrorCode::BackendTlsPolicyInvalid, "BackendTlsPolicyInvalid"),
        (EdgionErrorCode::UpstreamAuthorityRejected, "UpstreamAuthorityRejected"),
        (
            EdgionErrorCode::PluginTerminateWithoutResponse,
            "PluginTerminateWithoutResponse",
        ),
        (EdgionErrorCode::CredentialExpired, "CredentialExpired"),
        (
            EdgionErrorCode::CredentialConnectionConflict,
            "CredentialConnectionConflict",
        ),
        (
            EdgionErrorCode::ForwardAuthServiceUnreachable,
            "ForwardAuthServiceUnreachable",
        ),
        (EdgionErrorCode::RequestTimeoutExceeded, "RequestTimeoutExceeded"),
        (EdgionErrorCode::BodyRefusedWebsocket, "BodyRefusedWebsocket"),
        (EdgionErrorCode::BodyRefusedGrpc, "BodyRefusedGrpc"),
        (EdgionErrorCode::BodyRefusedStage, "BodyRefusedStage"),
        (EdgionErrorCode::BodyReadAborted, "BodyReadAborted"),
        (EdgionErrorCode::BodyReadTimeout, "BodyReadTimeout"),
        (EdgionErrorCode::BodyReadTooLarge, "BodyReadTooLarge"),
        (EdgionErrorCode::BodyReadUnimplemented, "BodyReadUnimplemented"),
        (EdgionErrorCode::BodyRefusedNoSnapshot, "BodyRefusedNoSnapshot"),
        (EdgionErrorCode::BodyReplayFailed, "BodyReplayFailed"),
        (EdgionErrorCode::BodyCaptureStorageFailed, "BodyCaptureStorageFailed"),
        (EdgionErrorCode::BodyMutationConflict, "BodyMutationConflict"),
        (
            EdgionErrorCode::GrpcClientDeadlineExceeded,
            "GrpcClientDeadlineExceeded",
        ),
        (
            EdgionErrorCode::BackendRequestTimeoutRetry,
            "BackendRequestTimeoutRetry",
        ),
        (
            EdgionErrorCode::PluginGrpcMessageBodyNotUtf8,
            "PluginGrpcMessageBodyNotUtf8",
        ),
        (
            EdgionErrorCode::PluginResponseReplaceUnsupportedGrpc,
            "PluginResponseReplaceUnsupportedGrpc",
        ),
        (EdgionErrorCode::PluginResponseDenyAborted, "PluginResponseDenyAborted"),
        (
            EdgionErrorCode::OutboundTlsVerifyUnsupported,
            "OutboundTlsVerifyUnsupported",
        ),
        (EdgionErrorCode::OutboundTlsCaMissing, "OutboundTlsCaMissing"),
        (
            EdgionErrorCode::OutboundTlsClientCertInvalid,
            "OutboundTlsClientCertInvalid",
        ),
        (EdgionErrorCode::OutboundTlsSanMismatch, "OutboundTlsSanMismatch"),
    ];

    for (code, expected) in info_codes {
        assert_eq!(serde_json::to_value(code).unwrap(), expected);
    }
    for (code, expected) in error_codes {
        assert_eq!(serde_json::to_value(code).unwrap(), expected);
    }
}

#[test]
fn code_serialization_has_no_numeric_or_http_field() {
    let json = serde_json::to_value(EdgionErrorCode::HostMissing).unwrap();
    assert_eq!(json, serde_json::Value::String("HostMissing".to_string()));
}
