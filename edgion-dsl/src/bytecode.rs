//! Bytecode instruction set for EdgionDSL VM
//!
//! Stack-based VM design: simpler than register-based, compact instructions,
//! natural match with Pratt parser output.

use serde::{Deserialize, Serialize};

/// Current serialized bytecode schema version.
pub const BYTECODE_VERSION: u16 = 1;
/// Legacy version for scripts produced before versioning was introduced.
const LEGACY_BYTECODE_VERSION: u16 = 0;
/// Hard limit for decoded bytecode payload to avoid unbounded allocation.
const MAX_DECODED_BYTECODE_LEN: usize = 256 * 1024;

/// Single VM instruction
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum OpCode {
    // ===== Stack Operations =====
    /// Push constant from pool by index
    LoadConst(u16),
    /// Push Nil
    LoadNil,
    /// Push true
    LoadTrue,
    /// Push false
    LoadFalse,
    /// Discard top of stack
    Pop,

    // ===== Local Variables =====
    /// Push local variable by slot index
    GetLocal(u16),
    /// Pop and store to local slot
    SetLocal(u16),

    // ===== Arithmetic =====
    /// Str+Str = concat, Int+Int = add, Str+other = to_string concat
    Add,
    /// Int only
    Sub,
    /// Int only
    Mul,
    /// Int only, div by zero → RuntimeError
    Div,
    /// Negate top (Int: -n)
    Neg,

    // ===== Comparison (pop 2, push Bool) =====
    Equal,
    NotEqual,
    Less,
    Greater,
    LessEqual,
    GreaterEqual,

    // ===== Logical =====
    /// Pop 1, push !is_truthy()
    Not,

    // ===== String Methods =====
    /// Pop (str, prefix), push Bool
    StartsWith,
    /// Pop (str, suffix), push Bool
    EndsWith,
    /// Pop (str, substr), push Bool
    Contains,
    /// Pop str, match regex at constant pool index, push Bool
    Matches(u16),
    /// regex_find(s, literal_pattern) — pattern pre-compiled in regex_cache[idx]
    /// Stack: [s] → [match_str | Nil]
    RegexFind(u16),
    /// regex_replace(s, literal_pattern, replacement) — pattern pre-compiled
    /// Stack: [s, replacement] → [result_str | Nil]  (Nil when s is not a Str)
    RegexReplace(u16),

    // ===== Control Flow =====
    /// Unconditional jump (relative offset from NEXT instruction)
    Jump(i32),
    /// Pop, jump if falsy
    JumpIfFalse(i32),
    /// Jump backwards, increment loop counter
    LoopBack(i32),
    /// Initialize loop counter for current nesting level
    LoopInit,
    /// Clean up loop counter at end of loop
    LoopEnd,

    // ===== Index Operations (for-in iteration + expr[key] access) =====
    /// Pop (container, index), push container[index]; List (Int key) or Map (Str key)
    IndexGet,
    /// Pop list, push len
    ListLen,

    // ===== Map Operations =====
    /// Pop 2n values (interleaved key:Str, val:Value pairs deepest-first), push Value::Map
    MakeMap(u16),

    // ===== Built-in Calls =====
    /// Call builtin function with arg count
    CallBuiltin(BuiltinId, u8),

    // ===== Termination =====
    /// return next() → GoodNext
    ReturnNext,
    /// Pop (body, status) → ErrResponse
    ReturnDeny,
    /// return terminate() → ErrTerminateRequest
    ReturnTerminate,
    /// Pop (message, code) → gRPC error response (HTTP 200 + grpc-status trailer)
    ReturnGrpcError,
}

/// Built-in function identifier.
/// Each maps to a PluginSession method or utility function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BuiltinId {
    // req.* read
    ReqHeader,
    ReqMethod,
    ReqPath,
    ReqQuery,
    ReqQueryString,
    ReqCookie,
    ReqClientIp,
    ReqRemoteIp,
    ReqPathParam,
    ReqHeaderNames,
    ReqScheme,
    ReqHost,
    ReqUri,
    ReqRequestPath,
    ReqRequestUri,
    ReqContentType,
    ReqHasHeader,

    // req.* mutation
    ReqSetHeader,
    ReqAppendHeader,
    ReqRemoveHeader,
    ReqSetUri,
    ReqSetHost,
    ReqSetMethod,

    // resp.* mutation
    RespSetHeader,
    RespAppendHeader,
    RespRemoveHeader,

    // resp.* read
    RespStatus,    // resp.status → int|nil
    RespHeader,    // resp.header(name) → str|nil
    RespHasHeader, // resp.has_header(name) → bool

    // req.* metadata
    ReqProtocol,
    ReqGrpcService,
    ReqGrpcMethod,
    ReqContentLength,
    ReqClientCertCn,
    ReqClientCertFingerprint,

    // resp.* read (additional)
    RespHeaderNames,
    RespContentType,

    // resp.* set status
    RespSetStatus,

    // req/resp proxy body (buffered) — read the body flowing through the proxy.
    /// `req.body` → str (lossy UTF-8 of the buffered request body)
    ReqBody,
    /// `resp.body` → str (lossy UTF-8 of the buffered response body)
    RespBody,

    // ctx.*
    CtxGet,
    CtxSet,
    CtxRemove,

    // Utilities
    Log,
    Len,
    Substr,
    ToInt,
    ToStr,
    ToUpper,
    ToLower,
    Base64Encode,
    Base64Decode,
    UrlEncode,
    UrlDecode,
    Sha256,
    Md5,
    TimeNow,
    Range,
    Trim,
    Split,

    // Additional utilities
    HmacSha256,
    JsonGet,
    Uuid,
    HexEncode,
    HexDecode,
    TimeFormat,
    IpInCidr,
    Join,
    LogWarn,
    LogError,
    /// `log_fmt(template_literal, sensitive_arg1, sensitive_arg2, ...)` — variadic.
    /// Argument count is carried by the `CallBuiltin(_, argc)` opcode itself.
    LogFmt,

    // External capabilities
    /// `http.call(method, url, body [, timeout_ms [, headers_map]])` → Map
    HttpCall,

    // Redis builtins — backed by LinkSys Redis connection pool
    /// `redis.get(link, key)` → Map { ok: Bool, value: Str|Nil, error: Str|Nil }
    RedisGet,
    /// `redis.set(link, key, value)` or `redis.set(link, key, value, ttl_ms)` → Map { ok: Bool, value: Bool, error: Str|Nil }
    RedisSet,
    /// `redis.del(link, key)` → Map { ok: Bool, value: Bool, error: Str|Nil }
    RedisDel,
    /// `redis.exists(link, key)` → Map { ok: Bool, value: Bool, error: Str|Nil }
    RedisExists,
    /// `redis.ttl(link, key)` → Map { ok: Bool, value: Int, error: Str|Nil }. Returns -1 if key has no expiry, -2 if key does not exist.
    RedisTtl,
    /// `redis.expire(link, key, seconds)` → Map { ok: Bool, value: Bool, error: Str|Nil }
    RedisExpire,
    /// `redis.incr(link, key)` → Map { ok: Bool, value: Int, error: Str|Nil }
    RedisIncr,
    /// `redis.incr_by(link, key, delta)` → Map { ok: Bool, value: Int, error: Str|Nil }
    RedisIncrBy,
    /// `redis.hget(link, key, field)` → Map { ok: Bool, value: Str|Nil, error: Str|Nil }
    RedisHGet,
    /// `redis.hset(link, key, field, value)` → Map { ok: Bool, value: Bool, error: Str|Nil }
    RedisHSet,
    /// `redis.hdel(link, key, field)` → Map { ok: Bool, value: Bool, error: Str|Nil }
    RedisHDel,
    /// `redis.hgetall(link, key)` → Map { ok: Bool, value: Map, error: Str|Nil }
    RedisHGetAll,
    /// `redis.hexists(link, key, field)` → Map { ok: Bool, value: Bool, error: Str|Nil }
    RedisHExists,
}

impl BuiltinId {
    /// Returns true for all `redis.*` builtin IDs that require async dispatch
    /// through the LinkSys Redis connection pool.
    ///
    /// Single authoritative source for the Redis-variant set: the VM dispatch
    /// loop and the plugin-level link-name check both call this method, so a new
    /// `Redis*` variant only has to be added here.
    #[inline]
    pub fn is_redis_builtin(self) -> bool {
        matches!(
            self,
            BuiltinId::RedisGet
                | BuiltinId::RedisSet
                | BuiltinId::RedisDel
                | BuiltinId::RedisExists
                | BuiltinId::RedisTtl
                | BuiltinId::RedisExpire
                | BuiltinId::RedisIncr
                | BuiltinId::RedisIncrBy
                | BuiltinId::RedisHGet
                | BuiltinId::RedisHSet
                | BuiltinId::RedisHDel
                | BuiltinId::RedisHGetAll
                | BuiltinId::RedisHExists
        )
    }

    /// Returns true for the proxy-body builtins that require async dispatch
    /// through `PluginSession::read_*_body`. Single authoritative source, used by
    /// the VM dispatch loop — mirrors [`BuiltinId::is_redis_builtin`].
    #[inline]
    pub fn is_body_builtin(self) -> bool {
        matches!(self, BuiltinId::ReqBody | BuiltinId::RespBody)
    }
}

/// Constant pool entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Constant {
    Str(String),
    Int(i64),
    /// Pre-compiled regex (stored as source pattern, re-compiled on deserialization)
    Regex(String),
}

/// A compiled DSL script — ready for VM execution or serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompiledScript {
    #[serde(default = "default_bytecode_version")]
    pub version: u16,
    pub code: Vec<OpCode>,
    pub constants: Vec<Constant>,
    pub local_count: u16,
    pub max_loop_depth: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Run-length encoded mapping from instruction PC to source line.
    /// Each entry `(count, line)` means the next `count` opcodes originate from `line`.
    /// May be absent; when absent, `line_of()` returns `None` and callers degrade to no line info.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_map: Option<Vec<(u16, u32)>>,
}

fn default_bytecode_version() -> u16 {
    LEGACY_BYTECODE_VERSION
}

impl CompiledScript {
    /// Serialize to base64-encoded JSON (compact, reliable cross-version)
    pub fn serialize_base64(&self) -> Result<String, String> {
        use base64::Engine;
        let json = serde_json::to_vec(self).map_err(|e| format!("serialize error: {}", e))?;
        Ok(base64::engine::general_purpose::STANDARD.encode(&json))
    }

    /// Deserialize from base64-encoded JSON
    pub fn deserialize_base64(encoded: &str) -> Result<Self, String> {
        use base64::Engine;
        // Base64 expands data to roughly 4/3. Reject obviously too-large payloads early.
        let max_encoded_len = (MAX_DECODED_BYTECODE_LEN * 4 / 3) + 8;
        if encoded.len() > max_encoded_len {
            return Err(format!(
                "bytecode payload too large: {} bytes encoded (max {})",
                encoded.len(),
                max_encoded_len
            ));
        }

        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|e| format!("base64 decode error: {}", e))?;
        if bytes.len() > MAX_DECODED_BYTECODE_LEN {
            return Err(format!(
                "bytecode payload too large: {} bytes decoded (max {})",
                bytes.len(),
                MAX_DECODED_BYTECODE_LEN
            ));
        }

        let script: Self = serde_json::from_slice(&bytes).map_err(|e| format!("deserialize error: {}", e))?;
        if script.version != LEGACY_BYTECODE_VERSION && script.version != BYTECODE_VERSION {
            return Err(format!(
                "unsupported bytecode version: {} (supported: {} and legacy {})",
                script.version, BYTECODE_VERSION, LEGACY_BYTECODE_VERSION
            ));
        }
        Ok(script)
    }

    /// Look up the source line that produced the instruction at `pc`.
    /// Returns `None` when no `source_map` is attached (legacy payloads) or `pc` is past the end.
    ///
    /// Not `#[cfg(test)]`-gated: `edgion-gateway`'s property tests call this
    /// on `edgion_dsl::bytecode::CompiledScript` compiled as a normal
    /// (non-test) dependency, so it must be reachable outside `edgion-dsl`'s
    /// own test build.
    pub fn line_of(&self, pc: usize) -> Option<u32> {
        let runs = self.source_map.as_ref()?;
        let mut covered: usize = 0;
        for (count, line) in runs {
            covered += *count as usize;
            if pc < covered {
                return Some(*line);
            }
        }
        None
    }

    /// True if the compiled bytecode calls the given builtin at least once.
    ///
    /// Scans `code` for `CallBuiltin(id, _)`. `BuiltinId` appears nowhere else in
    /// the bytecode, so this is exhaustive (mirrors the `check_redis_link_names`
    /// scan). Used by `EdgionPlugin::body_requirement` to gate a DSL plugin only
    /// when its script actually reads a body builtin.
    pub fn references_builtin(&self, id: BuiltinId) -> bool {
        self.code
            .iter()
            .any(|op| matches!(op, OpCode::CallBuiltin(b, _) if *b == id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_script() -> CompiledScript {
        CompiledScript {
            version: BYTECODE_VERSION,
            code: vec![OpCode::ReturnNext],
            constants: vec![],
            local_count: 0,
            max_loop_depth: 0,
            source: None,
            source_map: None,
        }
    }

    #[test]
    fn test_bytecode_roundtrip_with_version() {
        // Roundtrip with source_map absent.
        let script = minimal_script();
        let b64 = script.serialize_base64().expect("serialize should succeed");
        let decoded = CompiledScript::deserialize_base64(&b64).expect("deserialize should succeed");
        assert_eq!(decoded.version, BYTECODE_VERSION);
        assert_eq!(decoded.code, vec![OpCode::ReturnNext]);
        assert!(decoded.source_map.is_none());

        // Absent source_map must be omitted from the JSON payload (skip_serializing_if).
        let json = serde_json::to_string(&script).expect("json encode");
        assert!(!json.contains("source_map"));

        // Roundtrip with source_map present.
        let with_map = CompiledScript {
            source_map: Some(vec![(2, 1), (1, 2)]),
            ..minimal_script()
        };
        let b64 = with_map.serialize_base64().expect("serialize should succeed");
        let decoded = CompiledScript::deserialize_base64(&b64).expect("deserialize should succeed");
        assert_eq!(decoded.source_map.as_deref(), Some(&[(2u16, 1u32), (1, 2)][..]));
    }

    #[test]
    fn test_legacy_payload_decodes_without_source_map() {
        // A v=1 JSON predating source_map must decode to `Some(_)` source_map == None.
        use base64::Engine;
        let legacy = serde_json::json!({
            "version": BYTECODE_VERSION,
            "code": ["ReturnNext"],
            "constants": [],
            "local_count": 0,
            "max_loop_depth": 0
        });
        let bytes = serde_json::to_vec(&legacy).expect("json encode");
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        let decoded = CompiledScript::deserialize_base64(&encoded).expect("legacy payload should decode");
        assert!(decoded.source_map.is_none());
        assert_eq!(decoded.line_of(0), None);
    }

    #[test]
    fn test_line_of_walks_rle_runs() {
        let script = CompiledScript {
            source_map: Some(vec![(3, 10), (2, 11), (1, 12)]),
            ..minimal_script()
        };
        // PCs 0..3 → line 10
        assert_eq!(script.line_of(0), Some(10));
        assert_eq!(script.line_of(2), Some(10));
        // PCs 3..5 → line 11
        assert_eq!(script.line_of(3), Some(11));
        assert_eq!(script.line_of(4), Some(11));
        // PC 5 → line 12
        assert_eq!(script.line_of(5), Some(12));
        // Past the end → None
        assert_eq!(script.line_of(6), None);
    }

    #[test]
    fn test_reject_oversized_decoded_payload() {
        use base64::Engine;
        let huge = vec![b'a'; MAX_DECODED_BYTECODE_LEN + 1];
        let encoded = base64::engine::general_purpose::STANDARD.encode(huge);
        let err = CompiledScript::deserialize_base64(&encoded).expect_err("should reject oversized payload");
        assert!(err.contains("payload too large"));
    }

    #[test]
    fn test_reject_unknown_version() {
        use base64::Engine;
        let invalid = serde_json::json!({
            "version": 999,
            "code": ["ReturnNext"],
            "constants": [],
            "local_count": 0,
            "max_loop_depth": 0
        });
        let bytes = serde_json::to_vec(&invalid).expect("json encode should succeed");
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        let err = CompiledScript::deserialize_base64(&encoded).expect_err("should reject unsupported version");
        assert!(err.contains("unsupported bytecode version"));
    }

    #[test]
    fn is_body_builtin_only_true_for_body_ids() {
        assert!(BuiltinId::ReqBody.is_body_builtin());
        assert!(BuiltinId::RespBody.is_body_builtin());
        assert!(!BuiltinId::ReqHeader.is_body_builtin());
        assert!(!BuiltinId::RedisGet.is_body_builtin());
    }

    #[test]
    fn references_builtin_detects_body_use_per_direction() {
        let limits = crate::validator::ValidationLimits::default();
        // req.body only
        let req_only = crate::resolve_script(Some("let b = req.body"), None, &limits).unwrap();
        assert!(req_only.references_builtin(BuiltinId::ReqBody));
        assert!(!req_only.references_builtin(BuiltinId::RespBody));
        // resp.body only
        let resp_only = crate::resolve_script(Some("let b = resp.body"), None, &limits).unwrap();
        assert!(resp_only.references_builtin(BuiltinId::RespBody));
        assert!(!resp_only.references_builtin(BuiltinId::ReqBody));
        // neither
        let header_only = crate::resolve_script(Some("let h = req.header(\"X\")"), None, &limits).unwrap();
        assert!(!header_only.references_builtin(BuiltinId::ReqBody));
        assert!(!header_only.references_builtin(BuiltinId::RespBody));
    }
}
