//! Bytecode instruction set for EdgionDSL VM
//!
//! Stack-based VM design: simpler than register-based, compact instructions,
//! natural match with Pratt parser output.

use serde::{Deserialize, Serialize};

/// Single VM instruction
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

    // ===== List Operations (internal, for for-in) =====
    /// Pop (list, index), push list[index]
    ListGet,
    /// Pop list, push len
    ListLen,

    // ===== Built-in Calls =====
    /// Call builtin function with arg count
    CallBuiltin(BuiltinId, u8),

    // ===== Termination =====
    /// return next() → GoodNext
    ReturnNext,
    /// Pop (body, status) → ErrResponse
    ReturnDeny,
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
    ReqContentType,
    ReqHasHeader,

    // req.* mutation
    ReqSetHeader,
    ReqAppendHeader,
    ReqRemoveHeader,
    ReqSetUri,
    ReqSetHost,
    ReqSetMethod,

    // resp.*
    RespSetHeader,
    RespAppendHeader,
    RespRemoveHeader,

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
    RegexFind,
    RegexReplace,
    Range,
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
    pub code: Vec<OpCode>,
    pub constants: Vec<Constant>,
    pub local_count: u16,
    pub max_loop_depth: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
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
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|e| format!("base64 decode error: {}", e))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("deserialize error: {}", e))
    }
}
