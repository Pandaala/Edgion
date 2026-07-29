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
        ("req", "request_path") => Some(BuiltinId::ReqRequestPath),
        ("req", "request_uri") => Some(BuiltinId::ReqRequestUri),
        ("req", "content_type") => Some(BuiltinId::ReqContentType),
        ("req", "has_header") => Some(BuiltinId::ReqHasHeader),
        ("req", "body") => Some(BuiltinId::ReqBody),

        // req.* metadata
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

        // resp.* read (additional)
        ("resp", "header_names") => Some(BuiltinId::RespHeaderNames),
        ("resp", "content_type") => Some(BuiltinId::RespContentType),
        ("resp", "body") => Some(BuiltinId::RespBody),

        // resp.* set status
        ("resp", "set_status") => Some(BuiltinId::RespSetStatus),

        // ctx.*
        ("ctx", "get") => Some(BuiltinId::CtxGet),
        ("ctx", "set") => Some(BuiltinId::CtxSet),
        ("ctx", "remove") => Some(BuiltinId::CtxRemove),

        // http.*
        ("http", "call") => Some(BuiltinId::HttpCall),

        // redis.*
        ("redis", "get") => Some(BuiltinId::RedisGet),
        ("redis", "set") => Some(BuiltinId::RedisSet),
        ("redis", "del") => Some(BuiltinId::RedisDel),
        ("redis", "exists") => Some(BuiltinId::RedisExists),
        ("redis", "ttl") => Some(BuiltinId::RedisTtl),
        ("redis", "expire") => Some(BuiltinId::RedisExpire),
        ("redis", "incr") => Some(BuiltinId::RedisIncr),
        ("redis", "incr_by") => Some(BuiltinId::RedisIncrBy),
        ("redis", "hget") => Some(BuiltinId::RedisHGet),
        ("redis", "hset") => Some(BuiltinId::RedisHSet),
        ("redis", "hdel") => Some(BuiltinId::RedisHDel),
        ("redis", "hgetall") => Some(BuiltinId::RedisHGetAll),
        ("redis", "hexists") => Some(BuiltinId::RedisHExists),

        _ => None,
    }
}

/// Expected argument count for each builtin — used for compile-time validation.
/// Returns `Some(n)` for fixed-arity builtins, `None` for variadic builtins
/// (where the runtime reads argc from the `CallBuiltin(_, argc)` opcode).
pub(crate) fn builtin_expected_argc(id: &BuiltinId) -> Option<usize> {
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
        | BuiltinId::ReqRequestPath
        | BuiltinId::ReqRequestUri
        | BuiltinId::ReqContentType
        | BuiltinId::ReqProtocol
        | BuiltinId::ReqGrpcService
        | BuiltinId::ReqGrpcMethod
        | BuiltinId::ReqContentLength
        | BuiltinId::ReqClientCertCn
        | BuiltinId::ReqClientCertFingerprint
        | BuiltinId::ReqBody => Some(0),

        // req.* read — 1-arg
        BuiltinId::ReqHeader
        | BuiltinId::ReqQuery
        | BuiltinId::ReqCookie
        | BuiltinId::ReqPathParam
        | BuiltinId::ReqHasHeader
        | BuiltinId::ReqRemoveHeader
        | BuiltinId::ReqSetUri
        | BuiltinId::ReqSetHost
        | BuiltinId::ReqSetMethod => Some(1),

        // req.* mutation — 2-arg
        BuiltinId::ReqSetHeader | BuiltinId::ReqAppendHeader => Some(2),

        // resp.* read
        BuiltinId::RespStatus | BuiltinId::RespHeaderNames | BuiltinId::RespContentType | BuiltinId::RespBody => {
            Some(0)
        }
        BuiltinId::RespHeader | BuiltinId::RespHasHeader => Some(1),
        // resp.* mutation — 1-arg
        BuiltinId::RespRemoveHeader | BuiltinId::RespSetStatus => Some(1),
        // resp.* mutation — 2-arg
        BuiltinId::RespSetHeader | BuiltinId::RespAppendHeader => Some(2),

        // ctx.*
        BuiltinId::CtxGet | BuiltinId::CtxRemove => Some(1),
        BuiltinId::CtxSet => Some(2),

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
        | BuiltinId::LogError => Some(1),
        BuiltinId::Substr => Some(3),
        BuiltinId::TimeNow | BuiltinId::Uuid => Some(0),
        BuiltinId::Split | BuiltinId::HmacSha256 | BuiltinId::TimeFormat | BuiltinId::IpInCidr | BuiltinId::Join => {
            Some(2)
        }
        BuiltinId::Range => Some(2),

        // Variadic: argc carried by the opcode itself.
        BuiltinId::LogFmt => None,

        // http.call: 3-5 args — variadic, runtime validates
        BuiltinId::HttpCall => None,

        // redis.*: fixed-arity per function (each takes link alias as first argument)
        BuiltinId::RedisGet => Some(2),
        BuiltinId::RedisDel => Some(2),
        BuiltinId::RedisExists => Some(2),
        BuiltinId::RedisTtl => Some(2),
        BuiltinId::RedisIncr => Some(2),
        BuiltinId::RedisHGetAll => Some(2),
        BuiltinId::RedisExpire => Some(3),
        BuiltinId::RedisIncrBy => Some(3),
        BuiltinId::RedisHGet => Some(3),
        BuiltinId::RedisHDel => Some(3),
        BuiltinId::RedisHExists => Some(3),
        BuiltinId::RedisHSet => Some(4),
        // redis.set: 3 or 4 args (link, key, value [, ttl_ms]) — variadic, runtime validates
        BuiltinId::RedisSet => None,
    }
}
