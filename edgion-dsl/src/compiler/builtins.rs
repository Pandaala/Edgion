//! Namespace method resolution and builtin argument arity table

use super::super::bytecode::{BuiltinId, MAX_BUILTIN_ARGS};
use super::super::error::CompileError;
use std::fmt;

/// Check capacity before compiling operands, so no call can truncate its count.
pub(super) fn checked_builtin_argc(actual: usize) -> Result<u8, CompileError> {
    u8::try_from(actual).map_err(|_| CompileError::new("builtin call exceeds maximum argument count (255)"))
}

/// Canonical argument-count rule shared by source compilation and bytecode validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuiltinArity {
    Exact(usize),
    RangeInclusive { min: usize, max: usize },
}

impl BuiltinArity {
    pub(crate) fn accepts(self, actual: usize) -> bool {
        match self {
            Self::Exact(expected) => actual == expected,
            Self::RangeInclusive { min, max } => (min..=max).contains(&actual),
        }
    }
}

impl fmt::Display for BuiltinArity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exact(expected) => write!(formatter, "{expected}"),
            Self::RangeInclusive { min, max } => write!(formatter, "{min} to {max}"),
        }
    }
}

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
        ("req", "direct_peer_ip") => Some(BuiltinId::ReqDirectPeerIp),
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

        // xml.*
        ("xml", "parse") => Some(BuiltinId::XmlParse),
        ("xml", "kind") => Some(BuiltinId::XmlKind),
        ("xml", "name") => Some(BuiltinId::XmlName),
        ("xml", "namespace") => Some(BuiltinId::XmlNamespace),
        ("xml", "value") => Some(BuiltinId::XmlValue),
        ("xml", "child_count") => Some(BuiltinId::XmlChildCount),
        ("xml", "child") => Some(BuiltinId::XmlChild),
        ("xml", "find") => Some(BuiltinId::XmlFind),
        ("xml", "attribute_count") => Some(BuiltinId::XmlAttributeCount),
        ("xml", "attribute") => Some(BuiltinId::XmlAttribute),
        ("xml", "attr_value") => Some(BuiltinId::XmlAttrValue),
        ("xml", "text") => Some(BuiltinId::XmlText),
        ("xml", "attr") => Some(BuiltinId::XmlAttr),
        ("xml", "element") => Some(BuiltinId::XmlElement),
        ("xml", "stringify") => Some(BuiltinId::XmlStringify),
        ("xml", "set_body") => Some(BuiltinId::XmlSetBody),

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
        ("redis", "rate_limit") => Some(BuiltinId::RedisRateLimit),

        _ => None,
    }
}

/// Argument-count rule for each builtin.
pub(crate) fn builtin_arity(id: &BuiltinId) -> BuiltinArity {
    use BuiltinArity::{Exact, RangeInclusive};

    match id {
        BuiltinId::XmlParse
        | BuiltinId::XmlKind
        | BuiltinId::XmlName
        | BuiltinId::XmlNamespace
        | BuiltinId::XmlValue
        | BuiltinId::XmlChildCount
        | BuiltinId::XmlAttributeCount
        | BuiltinId::XmlText
        | BuiltinId::XmlStringify
        | BuiltinId::XmlSetBody => Exact(1),
        BuiltinId::XmlChild | BuiltinId::XmlAttribute => Exact(2),
        BuiltinId::XmlAttrValue | BuiltinId::XmlAttr => Exact(3),
        BuiltinId::XmlFind | BuiltinId::XmlElement => Exact(4),
        // req.* read — 0-arg (field access)
        BuiltinId::ReqMethod
        | BuiltinId::ReqPath
        | BuiltinId::ReqQueryString
        | BuiltinId::ReqDirectPeerIp
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
        | BuiltinId::ReqBody => Exact(0),

        // req.* read — 1-arg
        BuiltinId::ReqHeader
        | BuiltinId::ReqQuery
        | BuiltinId::ReqCookie
        | BuiltinId::ReqPathParam
        | BuiltinId::ReqHasHeader
        | BuiltinId::ReqRemoveHeader
        | BuiltinId::ReqSetUri
        | BuiltinId::ReqSetHost
        | BuiltinId::ReqSetMethod => Exact(1),

        // req.* mutation — 2-arg
        BuiltinId::ReqSetHeader | BuiltinId::ReqAppendHeader => Exact(2),

        // resp.* read
        BuiltinId::RespStatus | BuiltinId::RespHeaderNames | BuiltinId::RespContentType | BuiltinId::RespBody => {
            Exact(0)
        }
        BuiltinId::RespHeader | BuiltinId::RespHasHeader => Exact(1),
        // resp.* mutation — 1-arg
        BuiltinId::RespRemoveHeader | BuiltinId::RespSetStatus => Exact(1),
        // resp.* mutation — 2-arg
        BuiltinId::RespSetHeader | BuiltinId::RespAppendHeader => Exact(2),

        // ctx.*
        BuiltinId::CtxGet | BuiltinId::CtxRemove => Exact(1),
        BuiltinId::CtxSet => Exact(2),

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
        | BuiltinId::LogError => Exact(1),
        BuiltinId::Substr => Exact(3),
        BuiltinId::TimeNow | BuiltinId::Uuid => Exact(0),
        BuiltinId::Split | BuiltinId::HmacSha256 | BuiltinId::TimeFormat | BuiltinId::IpInCidr | BuiltinId::Join => {
            Exact(2)
        }
        BuiltinId::Range => Exact(2),

        BuiltinId::LogFmt => RangeInclusive {
            min: 1,
            max: MAX_BUILTIN_ARGS,
        },

        BuiltinId::HttpCall => RangeInclusive { min: 3, max: 5 },

        // redis.*: fixed-arity per function (each takes link alias as first argument)
        BuiltinId::RedisGet => Exact(2),
        BuiltinId::RedisDel => Exact(2),
        BuiltinId::RedisExists => Exact(2),
        BuiltinId::RedisTtl => Exact(2),
        BuiltinId::RedisIncr => Exact(2),
        BuiltinId::RedisHGetAll => Exact(2),
        BuiltinId::RedisExpire => Exact(3),
        BuiltinId::RedisIncrBy => Exact(3),
        BuiltinId::RedisHGet => Exact(3),
        BuiltinId::RedisHDel => Exact(3),
        BuiltinId::RedisHExists => Exact(3),
        BuiltinId::RedisHSet => Exact(4),
        BuiltinId::RedisRateLimit => Exact(4),
        BuiltinId::RedisSet => RangeInclusive { min: 3, max: 4 },
    }
}
