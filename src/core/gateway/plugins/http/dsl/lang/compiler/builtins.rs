//! Namespace method resolution and builtin argument arity table

use super::super::bytecode::BuiltinId;

/// Resolve namespace.method to BuiltinId
pub(super) fn resolve_namespace_method(namespace: &str, method: &str) -> Option<BuiltinId> {
    match (namespace, method) {
        // req.* read
        ("req", "header") => Some(BuiltinId::ReqHeader),
        ("req", "method") => Some(BuiltinId::ReqMethod),
        ("req", "path") => Some(BuiltinId::ReqPath),
        ("req", "query") => Some(BuiltinId::ReqQuery),
        ("req", "query_string") => Some(BuiltinId::ReqQueryString),
        ("req", "cookie") => Some(BuiltinId::ReqCookie),
        ("req", "client_ip") => Some(BuiltinId::ReqClientIp),
        ("req", "remote_ip") => Some(BuiltinId::ReqRemoteIp),
        ("req", "path_param") => Some(BuiltinId::ReqPathParam),
        ("req", "header_names") => Some(BuiltinId::ReqHeaderNames),
        ("req", "scheme") => Some(BuiltinId::ReqScheme),
        ("req", "host") => Some(BuiltinId::ReqHost),
        ("req", "uri") => Some(BuiltinId::ReqUri),
        ("req", "content_type") => Some(BuiltinId::ReqContentType),
        ("req", "has_header") => Some(BuiltinId::ReqHasHeader),

        // req.* metadata (P1)
        ("req", "protocol") => Some(BuiltinId::ReqProtocol),
        ("req", "grpc_service") => Some(BuiltinId::ReqGrpcService),
        ("req", "grpc_method") => Some(BuiltinId::ReqGrpcMethod),
        ("req", "content_length") => Some(BuiltinId::ReqContentLength),
        ("req", "client_cert_cn") => Some(BuiltinId::ReqClientCertCn),
        ("req", "client_cert_fingerprint") => Some(BuiltinId::ReqClientCertFingerprint),

        // req.* mutation
        ("req", "set_header") => Some(BuiltinId::ReqSetHeader),
        ("req", "append_header") => Some(BuiltinId::ReqAppendHeader),
        ("req", "remove_header") => Some(BuiltinId::ReqRemoveHeader),
        ("req", "set_uri") => Some(BuiltinId::ReqSetUri),
        ("req", "set_host") => Some(BuiltinId::ReqSetHost),
        ("req", "set_method") => Some(BuiltinId::ReqSetMethod),

        // resp.* mutation
        ("resp", "set_header") => Some(BuiltinId::RespSetHeader),
        ("resp", "append_header") => Some(BuiltinId::RespAppendHeader),
        ("resp", "remove_header") => Some(BuiltinId::RespRemoveHeader),

        // resp.* read
        ("resp", "status") => Some(BuiltinId::RespStatus),
        ("resp", "header") => Some(BuiltinId::RespHeader),
        ("resp", "has_header") => Some(BuiltinId::RespHasHeader),

        // resp.* read (P2)
        ("resp", "header_names") => Some(BuiltinId::RespHeaderNames),
        ("resp", "content_type") => Some(BuiltinId::RespContentType),

        // resp.* set status
        ("resp", "set_status") => Some(BuiltinId::RespSetStatus),

        // ctx.*
        ("ctx", "get") => Some(BuiltinId::CtxGet),
        ("ctx", "set") => Some(BuiltinId::CtxSet),
        ("ctx", "remove") => Some(BuiltinId::CtxRemove),

        _ => None,
    }
}

/// Expected argument count for each builtin — used for compile-time validation.
pub(crate) fn builtin_expected_argc(id: &BuiltinId) -> usize {
    match id {
        // req.* read — 0-arg (field access)
        BuiltinId::ReqMethod
        | BuiltinId::ReqPath
        | BuiltinId::ReqQueryString
        | BuiltinId::ReqClientIp
        | BuiltinId::ReqRemoteIp
        | BuiltinId::ReqHeaderNames
        | BuiltinId::ReqScheme
        | BuiltinId::ReqHost
        | BuiltinId::ReqUri
        | BuiltinId::ReqContentType
        | BuiltinId::ReqProtocol
        | BuiltinId::ReqGrpcService
        | BuiltinId::ReqGrpcMethod
        | BuiltinId::ReqContentLength
        | BuiltinId::ReqClientCertCn
        | BuiltinId::ReqClientCertFingerprint => 0,

        // req.* read — 1-arg
        BuiltinId::ReqHeader
        | BuiltinId::ReqQuery
        | BuiltinId::ReqCookie
        | BuiltinId::ReqPathParam
        | BuiltinId::ReqHasHeader
        | BuiltinId::ReqRemoveHeader
        | BuiltinId::ReqSetUri
        | BuiltinId::ReqSetHost
        | BuiltinId::ReqSetMethod => 1,

        // req.* mutation — 2-arg
        BuiltinId::ReqSetHeader | BuiltinId::ReqAppendHeader => 2,

        // resp.* read
        BuiltinId::RespStatus | BuiltinId::RespHeaderNames | BuiltinId::RespContentType => 0,
        BuiltinId::RespHeader | BuiltinId::RespHasHeader => 1,
        // resp.* mutation — 1-arg
        BuiltinId::RespRemoveHeader | BuiltinId::RespSetStatus => 1,
        // resp.* mutation — 2-arg
        BuiltinId::RespSetHeader | BuiltinId::RespAppendHeader => 2,

        // ctx.*
        BuiltinId::CtxGet | BuiltinId::CtxRemove => 1,
        BuiltinId::CtxSet => 2,

        // Utilities
        BuiltinId::Log
        | BuiltinId::Len
        | BuiltinId::ToStr
        | BuiltinId::ToInt
        | BuiltinId::ToUpper
        | BuiltinId::ToLower
        | BuiltinId::Base64Encode
        | BuiltinId::Base64Decode
        | BuiltinId::UrlEncode
        | BuiltinId::UrlDecode
        | BuiltinId::Sha256
        | BuiltinId::Md5
        | BuiltinId::Trim
        | BuiltinId::HexEncode
        | BuiltinId::HexDecode
        | BuiltinId::LogWarn
        | BuiltinId::LogError => 1,
        BuiltinId::Substr | BuiltinId::RegexReplace => 3,
        BuiltinId::TimeNow | BuiltinId::Uuid => 0,
        BuiltinId::RegexFind
        | BuiltinId::Split
        | BuiltinId::HmacSha256
        | BuiltinId::JsonGet
        | BuiltinId::TimeFormat
        | BuiltinId::IpInCidr
        | BuiltinId::Join => 2,
        BuiltinId::Range => 2,
    }
}
