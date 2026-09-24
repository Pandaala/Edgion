//! Bytecode instruction set for EdgionDSL VM
//!
//! Stack-based VM design: simpler than register-based, compact instructions,
//! natural match with Pratt parser output.

use serde::{Deserialize, Serialize};

/// Current serialized bytecode schema version.
///
/// Version 5 removes whole-body text reads and caps explicit read lengths. Older
/// payloads are intentionally rejected so they can never execute with
/// different Body or stack semantics.
pub const BYTECODE_VERSION: u16 = 5;
/// Maximum argument count represented exactly by `CallBuiltin`.
pub const MAX_BUILTIN_ARGS: usize = u8::MAX as usize;
/// Hard ceiling for one static JSON multi-select plan.
pub const JSON_SELECT_MAX_ENTRIES: usize = 64;
/// Maximum UTF-8 byte length of one JSON multi-select output name.
pub const JSON_SELECT_MAX_NAME_BYTES: usize = 256;
/// Missing versions deserialize to zero and are rejected with other old forms.
const UNVERSIONED_BYTECODE_VERSION: u16 = 0;
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
    /// Pop Str|Body, push Unicode scalar length.
    TextLen,
    /// Pop Str|Body, push whether its logical text is empty.
    TextIsEmpty,

    // ===== Request Body Operations =====
    /// Pop Body, push raw captured byte length.
    BodyByteLen,
    /// Pop (Body, offset, max_bytes), push lossy-decoded Str.
    BodyRead,
    /// Pop Body, push lowercase hexadecimal SHA-256.
    BodySha256,
    /// Pop Body, push lowercase hexadecimal MD5.
    BodyMd5,
    /// Pop (Body, key Str), push lowercase hexadecimal HMAC-SHA-256.
    BodyHmacSha256,
    /// Pop Str|Body, select a definite JSON path from the constant pool.
    JsonGet(u16),
    /// Pop Body, select a statically compiled set of definite JSON paths, push Map.
    JsonSelect(u16),
    /// Pop Body, replace it with an empty body, push Nil.
    BodyClear,
    /// Pop (Body, Str), append bytes, push Nil.
    BodyAppend,
    /// Pop (Body, Str), prepend bytes, push Nil.
    BodyPrepend,
    /// Pop (Body, start, end), remove a half-open byte range, push Nil.
    BodyRemove,
    /// Pop (Body, from, to), replace literal bytes, push Nil.
    BodyReplace,

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
    /// return terminate() → explicit HTTP 502 local response with no body
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
    ReqDirectPeerIp,
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

    // req/resp proxy body (buffered) — access the body flowing through the proxy.
    /// `req.body` → opaque request-scoped Body after the host availability check.
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
    /// `redis.rate_limit(link, policy, key, cost)` -> Map { ok, value, error }
    RedisRateLimit,
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
                | BuiltinId::RedisRateLimit
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

impl OpCode {
    /// Whether this opcode can publish a new request-body snapshot.
    #[inline]
    pub fn is_request_body_mutation(self) -> bool {
        matches!(
            self,
            OpCode::BodyClear | OpCode::BodyAppend | OpCode::BodyPrepend | OpCode::BodyRemove | OpCode::BodyReplace
        )
    }
}

/// One output name and definite JSONPath in a static multi-select plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonSelectEntry {
    pub name: String,
    pub path: String,
}

/// Constant pool entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Constant {
    Str(String),
    Int(i64),
    /// Regex source pattern, compiled during validation and VM construction.
    Regex(String),
    /// Boolean `.matches()` regex accepted by the incremental DFA engine.
    StreamingRegex(String),
    /// Definite JSONPath source, parsed again when untrusted bytecode is validated.
    JsonPath(String),
    /// Static request-body JSON selection plan. This is not a loadable DSL value.
    JsonSelect(Vec<JsonSelectEntry>),
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
    UNVERSIONED_BYTECODE_VERSION
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
        if script.version != BYTECODE_VERSION {
            return Err(format!(
                "unsupported bytecode version: {} (supported: {})",
                script.version, BYTECODE_VERSION
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

    /// True when the script evaluates the opaque request-body value.
    pub fn references_request_body(&self) -> bool {
        self.references_builtin(BuiltinId::ReqBody)
    }

    /// True when the script contains an opcode that can mutate the request body.
    ///
    /// Stage validation uses this separately from [`Self::references_request_body`]:
    /// response-stage scripts may inspect a body captured earlier, but may not
    /// publish request-body mutations.
    pub fn references_request_body_mutation(&self) -> bool {
        self.code.iter().copied().any(OpCode::is_request_body_mutation)
    }
}

#[cfg(test)]
#[path = "../tests/unit/bytecode_test.rs"]
mod tests;
