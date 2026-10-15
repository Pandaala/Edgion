//! Runtime-independent Proxy-Wasm compatibility analysis.
//!
//! This crate intentionally has no Edgion workspace dependencies. It owns the
//! stable compatibility report and the exact host ABI profile shared by
//! tooling and the gateway's Wasmtime linker.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use wasmparser::{BinaryReader, Encoding, ExternalKind, FuncType, Parser, Payload, TypeRef, ValType, Validator};

/// Compatibility report schema version.
pub const REPORT_VERSION: u32 = 2;
/// Named HTTP host contract; this is not an SDK certification.
pub const PROFILE: &str = "edgion-proxy-wasm-http-v1";
/// Static analysis never executes or instantiates the guest.
pub const ASSESSMENT: &str = "static_load_compatibility";
/// Proxy-Wasm ABI marker prefix.
pub const ABI_MARKER_PREFIX: &str = "proxy_abi_version_";
/// Supported Proxy-Wasm ABI 0.2.0 marker.
pub const ABI_0_2_0: &str = "proxy_abi_version_0_2_0";
/// Supported Proxy-Wasm ABI 0.2.1 marker.
pub const ABI_0_2_1: &str = "proxy_abi_version_0_2_1";
/// Supported ABI marker exports.
pub const SUPPORTED_ABI_MARKERS: &[&str] = &[ABI_0_2_0, ABI_0_2_1];
/// Maximum combined entries across core sections that build validator indexes.
pub const MAX_ANALYSIS_ENTRIES: u64 = 100_000;
/// Maximum combined import and export inventory items reported.
pub const MAX_INVENTORY_ITEMS: u64 = 50_000;
/// Maximum combined bytes in import module/name and export name strings.
pub const MAX_INVENTORY_NAME_BYTES: u64 = 4 * 1024 * 1024;
/// Maximum total parameter/result cells declared across function types.
pub const MAX_TYPE_SIGNATURE_CELLS: u64 = 100_000;
/// Maximum parameter/result cells materialized across report signatures.
pub const MAX_REPORT_SIGNATURE_CELLS: u64 = 100_000;
/// Maximum expanded locals declared by one function body.
pub const MAX_FUNCTION_LOCALS: u64 = 100_000;
/// Maximum expanded locals declared across all function bodies.
pub const MAX_TOTAL_LOCALS: u64 = 1_000_000;
/// Maximum encoded size of one function body before validation.
pub const MAX_FUNCTION_BODY_BYTES: u64 = 256 * 1024;

/// Names and signatures implemented by the Edgion Proxy-Wasm host.
pub mod host {
    use super::{HostFunction, HostSignature, WasmValueType};

    pub const MODULE: &str = "env";
    pub const PROXY_LOG: &str = "proxy_log";
    pub const PROXY_GET_CURRENT_TIME_NANOSECONDS: &str = "proxy_get_current_time_nanoseconds";
    pub const PROXY_GET_HEADER_MAP_PAIRS: &str = "proxy_get_header_map_pairs";
    pub const PROXY_GET_HEADER_MAP_VALUE: &str = "proxy_get_header_map_value";
    pub const PROXY_REPLACE_HEADER_MAP_VALUE: &str = "proxy_replace_header_map_value";
    pub const PROXY_ADD_HEADER_MAP_VALUE: &str = "proxy_add_header_map_value";
    pub const PROXY_REMOVE_HEADER_MAP_VALUE: &str = "proxy_remove_header_map_value";
    pub const PROXY_SET_HEADER_MAP_PAIRS: &str = "proxy_set_header_map_pairs";
    pub const PROXY_GET_BUFFER_BYTES: &str = "proxy_get_buffer_bytes";
    pub const PROXY_SET_BUFFER_BYTES: &str = "proxy_set_buffer_bytes";
    pub const PROXY_SEND_LOCAL_RESPONSE: &str = "proxy_send_local_response";
    pub const PROXY_GET_PROPERTY: &str = "proxy_get_property";
    pub const PROXY_SET_PROPERTY: &str = "proxy_set_property";
    pub const PROXY_GET_CONFIGURATION: &str = "proxy_get_configuration";
    pub const PROXY_HTTP_CALL: &str = "proxy_http_call";
    pub const PROXY_DISPATCH_HTTP_CALL: &str = "proxy_dispatch_http_call";
    pub const PROXY_GET_SHARED_DATA: &str = "proxy_get_shared_data";
    pub const PROXY_SET_SHARED_DATA: &str = "proxy_set_shared_data";
    pub const PROXY_SET_EFFECTIVE_CONTEXT: &str = "proxy_set_effective_context";
    pub const PROXY_SET_TICK_PERIOD_MILLISECONDS: &str = "proxy_set_tick_period_milliseconds";
    pub const PROXY_DONE: &str = "proxy_done";
    pub const PROXY_CONTINUE_STREAM: &str = "proxy_continue_stream";
    pub const PROXY_CONTINUE_REQUEST: &str = "proxy_continue_request";
    pub const PROXY_CONTINUE_RESPONSE: &str = "proxy_continue_response";
    pub const PROXY_CLOSE_STREAM: &str = "proxy_close_stream";
    pub const PROXY_GET_STATUS: &str = "proxy_get_status";
    pub const PROXY_GRPC_CANCEL: &str = "proxy_grpc_cancel";
    pub const PROXY_GRPC_STREAM: &str = "proxy_grpc_stream";
    pub const PROXY_GRPC_CLOSE: &str = "proxy_grpc_close";
    pub const PROXY_GRPC_CALL: &str = "proxy_grpc_call";
    pub const PROXY_DEQUEUE_SHARED_QUEUE: &str = "proxy_dequeue_shared_queue";
    pub const PROXY_ENQUEUE_SHARED_QUEUE: &str = "proxy_enqueue_shared_queue";
    pub const PROXY_RESOLVE_SHARED_QUEUE: &str = "proxy_resolve_shared_queue";
    pub const PROXY_CALL_FOREIGN_FUNCTION: &str = "proxy_call_foreign_function";
    pub const PROXY_REGISTER_SHARED_QUEUE: &str = "proxy_register_shared_queue";
    pub const PROXY_GRPC_SEND: &str = "proxy_grpc_send";
    pub const PROXY_GET_LOG_LEVEL: &str = "proxy_get_log_level";
    pub const PROXY_DEFINE_METRIC: &str = "proxy_define_metric";
    pub const PROXY_GET_METRIC: &str = "proxy_get_metric";
    pub const PROXY_RECORD_METRIC: &str = "proxy_record_metric";
    pub const PROXY_INCREMENT_METRIC: &str = "proxy_increment_metric";

    const I32: WasmValueType = WasmValueType::I32;
    const RESULT_I32: &[WasmValueType] = &[I32];

    const fn function(name: &'static str, params: &'static [WasmValueType]) -> HostFunction {
        HostFunction {
            module: MODULE,
            name,
            signature: HostSignature {
                params,
                results: RESULT_I32,
            },
            behavior: super::HostBehavior::Implemented,
        }
    }

    const fn trap(name: &'static str, params: &'static [WasmValueType]) -> HostFunction {
        HostFunction {
            module: MODULE,
            name,
            signature: HostSignature {
                params,
                results: RESULT_I32,
            },
            behavior: super::HostBehavior::Trap,
        }
    }

    const fn noop(name: &'static str, params: &'static [WasmValueType]) -> HostFunction {
        HostFunction {
            behavior: super::HostBehavior::AcceptedNoop,
            ..function(name, params)
        }
    }

    /// Exact Edgion host import profile. Unknown imports are incompatible.
    pub const FUNCTIONS: &[HostFunction] = &[
        function(PROXY_LOG, &[I32, I32, I32]),
        function(PROXY_GET_CURRENT_TIME_NANOSECONDS, &[I32]),
        function(PROXY_GET_HEADER_MAP_PAIRS, &[I32, I32, I32]),
        function(PROXY_GET_HEADER_MAP_VALUE, &[I32, I32, I32, I32, I32]),
        function(PROXY_REPLACE_HEADER_MAP_VALUE, &[I32, I32, I32, I32, I32]),
        function(PROXY_ADD_HEADER_MAP_VALUE, &[I32, I32, I32, I32, I32]),
        function(PROXY_REMOVE_HEADER_MAP_VALUE, &[I32, I32, I32]),
        function(PROXY_SET_HEADER_MAP_PAIRS, &[I32, I32, I32]),
        function(PROXY_GET_BUFFER_BYTES, &[I32, I32, I32, I32, I32]),
        function(PROXY_SET_BUFFER_BYTES, &[I32, I32, I32, I32, I32]),
        function(PROXY_SEND_LOCAL_RESPONSE, &[I32, I32, I32, I32, I32, I32, I32, I32]),
        function(PROXY_GET_PROPERTY, &[I32, I32, I32, I32]),
        function(PROXY_SET_PROPERTY, &[I32, I32, I32, I32]),
        function(PROXY_GET_CONFIGURATION, &[I32, I32]),
        function(PROXY_HTTP_CALL, &[I32, I32, I32, I32, I32, I32, I32, I32, I32, I32]),
        function(
            PROXY_DISPATCH_HTTP_CALL,
            &[I32, I32, I32, I32, I32, I32, I32, I32, I32, I32],
        ),
        function(PROXY_GET_SHARED_DATA, &[I32, I32, I32, I32, I32]),
        function(PROXY_SET_SHARED_DATA, &[I32, I32, I32, I32, I32]),
        noop(PROXY_SET_EFFECTIVE_CONTEXT, &[I32]),
        function(PROXY_SET_TICK_PERIOD_MILLISECONDS, &[I32]),
        noop(PROXY_DONE, &[]),
        function(PROXY_CONTINUE_STREAM, &[I32]),
        function(PROXY_CONTINUE_REQUEST, &[]),
        noop(PROXY_CONTINUE_RESPONSE, &[]),
        // The official proxy-wasm Rust SDK declares these imports even when a
        // guest does not use them. They are recognized with exact signatures,
        // but deliberately trap if invoked because Edgion does not implement
        // the corresponding feature.
        trap(PROXY_CLOSE_STREAM, &[I32]),
        trap(PROXY_GET_STATUS, &[I32, I32, I32]),
        trap(PROXY_GRPC_CANCEL, &[I32]),
        trap(PROXY_GRPC_STREAM, &[I32, I32, I32, I32, I32, I32, I32, I32, I32]),
        trap(PROXY_GRPC_CLOSE, &[I32]),
        trap(
            PROXY_GRPC_CALL,
            &[I32, I32, I32, I32, I32, I32, I32, I32, I32, I32, I32, I32],
        ),
        trap(PROXY_DEQUEUE_SHARED_QUEUE, &[I32, I32, I32]),
        trap(PROXY_ENQUEUE_SHARED_QUEUE, &[I32, I32, I32]),
        trap(PROXY_RESOLVE_SHARED_QUEUE, &[I32, I32, I32, I32, I32]),
        trap(PROXY_CALL_FOREIGN_FUNCTION, &[I32, I32, I32, I32, I32, I32]),
        trap(PROXY_REGISTER_SHARED_QUEUE, &[I32, I32, I32]),
        trap(PROXY_GRPC_SEND, &[I32, I32, I32, I32]),
        trap(PROXY_GET_LOG_LEVEL, &[I32]),
        trap(PROXY_DEFINE_METRIC, &[I32, I32, I32, I32]),
        trap(PROXY_GET_METRIC, &[I32, I32]),
        trap(PROXY_RECORD_METRIC, &[I32, WasmValueType::I64]),
        trap(PROXY_INCREMENT_METRIC, &[I32, WasmValueType::I64]),
    ];
}

/// Value types used by the fixed host ABI profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WasmValueType {
    I32,
    I64,
    F32,
    F64,
    V128,
}

/// A borrowed function signature in the host ABI profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostSignature {
    pub params: &'static [WasmValueType],
    pub results: &'static [WasmValueType],
}

/// One function exposed by the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostFunction {
    pub module: &'static str,
    pub name: &'static str,
    pub signature: HostSignature,
    pub behavior: HostBehavior,
}

/// Runtime behavior associated with a recognized host import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostBehavior {
    Implemented,
    AcceptedNoop,
    Trap,
}

/// A serializable core-Wasm function signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionSignature {
    pub params: Vec<String>,
    pub results: Vec<String>,
}

/// One module import and its resolved function signature, when applicable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportInfo {
    pub module: String,
    pub name: String,
    pub kind: String,
    pub support: ImportSupport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<FunctionSignature>,
}

/// How Edgion handles an imported item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportSupport {
    /// The host implements the function.
    Implemented,
    /// Accepted with STATUS_OK but without the operation's semantics.
    AcceptedNoop,
    /// The import is declared by supported SDKs but traps if invoked.
    KnownUnsupportedTrap,
    /// The import is outside the exact host profile and prevents loading.
    Unknown,
}

/// One module export and its resolved function signature, when applicable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportInfo {
    pub name: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<FunctionSignature>,
}

/// A recognized Proxy-Wasm hook export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookInfo {
    pub name: String,
    pub support: HookSupport,
    pub signature: FunctionSignature,
}

/// Whether Edgion invokes a recognized Proxy-Wasm hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookSupport {
    /// The gateway invokes this hook in its current HTTP runtime.
    Implemented,
    /// The hook is part of the supported SDK ABI but Edgion does not invoke it.
    KnownNotInvoked,
}

/// Stable machine-readable compatibility finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompatibilityIssue {
    pub code: CompatibilityCode,
    pub message: String,
}

/// Stable finding codes. Messages are explanatory and not an API discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Error)]
#[serde(rename_all = "snake_case")]
pub enum CompatibilityCode {
    #[error("malformed WebAssembly module")]
    Malformed,
    #[error("WebAssembly components are not supported")]
    ComponentUnsupported,
    #[error("WebAssembly module exceeds compatibility analysis limits")]
    AnalysisLimitExceeded,
    #[error("proxy-wasm ABI marker is missing")]
    MissingAbiMarker,
    #[error("proxy-wasm ABI markers are mixed")]
    MixedAbiMarkers,
    #[error("proxy-wasm ABI marker is unsupported")]
    UnsupportedAbiMarker,
    #[error("proxy-wasm ABI marker must export a function")]
    InvalidAbiMarker,
    #[error("host import is unknown")]
    UnknownImport,
    #[error("host import uses the wrong module")]
    WrongImportModule,
    #[error("host import must be a function")]
    NonFunctionImport,
    #[error("host import signature does not match")]
    ImportSignatureMismatch,
    #[error("proxy-wasm hook must export a function")]
    NonFunctionHook,
    #[error("proxy-wasm hook signature does not match")]
    HookSignatureMismatch,
    #[error("proxy-wasm hook is unknown")]
    UnknownHook,
    #[error("known proxy-wasm hook is not invoked")]
    KnownNotInvokedHook,
    #[error("host import is accepted as a no-op")]
    AcceptedNoopImport,
    #[error("hook requires explicit response companion selection")]
    ConditionalHook,
    #[error("known unsupported host import traps if invoked")]
    KnownUnsupportedImport,
}

/// Stable report returned for every input, including malformed bytes.
///
/// `compatible` means the module passes this static host-profile analysis.
/// Instantiation limits, configuration and execution can still reject it.
/// It does not mean every imported feature is implemented: imports
/// marked `known_unsupported_trap` are load-compatible but trap if invoked,
/// and hooks marked `known_not_invoked` are accepted but never called.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompatibilityReport {
    pub report_version: u32,
    pub profile: String,
    pub assessment: String,
    pub compatible: bool,
    pub sha256: String,
    pub abi_markers: Vec<String>,
    pub hooks: Vec<HookInfo>,
    pub imports: Vec<ImportInfo>,
    pub exports: Vec<ExportInfo>,
    pub warnings: Vec<CompatibilityIssue>,
    pub errors: Vec<CompatibilityIssue>,
}

impl CompatibilityReport {
    fn new(bytes: &[u8]) -> Self {
        Self {
            report_version: REPORT_VERSION,
            profile: PROFILE.into(),
            assessment: ASSESSMENT.into(),
            compatible: true,
            sha256: sha256_hex(bytes),
            abi_markers: Vec::new(),
            hooks: Vec::new(),
            imports: Vec::new(),
            exports: Vec::new(),
            warnings: Vec::new(),
            errors: Vec::new(),
        }
    }

    fn error(&mut self, code: CompatibilityCode, message: impl Into<String>) {
        self.compatible = false;
        self.errors.push(CompatibilityIssue {
            code,
            message: message.into(),
        });
    }
}

#[derive(Clone, Copy)]
struct HookProfile {
    name: &'static str,
    support: HookSupport,
    params: &'static [WasmValueType],
    results: &'static [WasmValueType],
}

const NO_RESULTS: &[WasmValueType] = &[];
const I32_RESULT: &[WasmValueType] = &[WasmValueType::I32];
const HOOKS: &[HookProfile] = &[
    HookProfile {
        name: "proxy_on_memory_allocate",
        support: HookSupport::Implemented,
        params: &[WasmValueType::I32],
        results: I32_RESULT,
    },
    HookProfile {
        name: "proxy_on_context_create",
        support: HookSupport::Implemented,
        params: &[WasmValueType::I32, WasmValueType::I32],
        results: NO_RESULTS,
    },
    HookProfile {
        name: "proxy_on_done",
        support: HookSupport::Implemented,
        params: &[WasmValueType::I32],
        results: I32_RESULT,
    },
    HookProfile {
        name: "proxy_on_log",
        support: HookSupport::KnownNotInvoked,
        params: &[WasmValueType::I32],
        results: NO_RESULTS,
    },
    HookProfile {
        name: "proxy_on_delete",
        support: HookSupport::Implemented,
        params: &[WasmValueType::I32],
        results: NO_RESULTS,
    },
    HookProfile {
        name: "proxy_on_vm_start",
        support: HookSupport::Implemented,
        params: &[WasmValueType::I32, WasmValueType::I32],
        results: I32_RESULT,
    },
    HookProfile {
        name: "proxy_on_configure",
        support: HookSupport::Implemented,
        params: &[WasmValueType::I32, WasmValueType::I32],
        results: I32_RESULT,
    },
    HookProfile {
        name: "proxy_on_tick",
        support: HookSupport::Implemented,
        params: &[WasmValueType::I32],
        results: NO_RESULTS,
    },
    HookProfile {
        name: "proxy_on_queue_ready",
        support: HookSupport::KnownNotInvoked,
        params: &[WasmValueType::I32, WasmValueType::I32],
        results: NO_RESULTS,
    },
    HookProfile {
        name: "proxy_on_new_connection",
        support: HookSupport::KnownNotInvoked,
        params: &[WasmValueType::I32],
        results: I32_RESULT,
    },
    HookProfile {
        name: "proxy_on_downstream_data",
        support: HookSupport::KnownNotInvoked,
        params: &[WasmValueType::I32, WasmValueType::I32, WasmValueType::I32],
        results: I32_RESULT,
    },
    HookProfile {
        name: "proxy_on_downstream_connection_close",
        support: HookSupport::KnownNotInvoked,
        params: &[WasmValueType::I32, WasmValueType::I32],
        results: NO_RESULTS,
    },
    HookProfile {
        name: "proxy_on_upstream_data",
        support: HookSupport::KnownNotInvoked,
        params: &[WasmValueType::I32, WasmValueType::I32, WasmValueType::I32],
        results: I32_RESULT,
    },
    HookProfile {
        name: "proxy_on_upstream_connection_close",
        support: HookSupport::KnownNotInvoked,
        params: &[WasmValueType::I32, WasmValueType::I32],
        results: NO_RESULTS,
    },
    HookProfile {
        name: "proxy_on_request_headers",
        support: HookSupport::Implemented,
        params: &[WasmValueType::I32, WasmValueType::I32, WasmValueType::I32],
        results: I32_RESULT,
    },
    HookProfile {
        name: "proxy_on_request_body",
        support: HookSupport::Implemented,
        params: &[WasmValueType::I32, WasmValueType::I32, WasmValueType::I32],
        results: I32_RESULT,
    },
    HookProfile {
        name: "proxy_on_request_trailers",
        support: HookSupport::KnownNotInvoked,
        params: &[WasmValueType::I32, WasmValueType::I32],
        results: I32_RESULT,
    },
    HookProfile {
        name: "proxy_on_response_headers",
        support: HookSupport::Implemented,
        params: &[WasmValueType::I32, WasmValueType::I32, WasmValueType::I32],
        results: I32_RESULT,
    },
    HookProfile {
        name: "proxy_on_response_body",
        support: HookSupport::Implemented,
        params: &[WasmValueType::I32, WasmValueType::I32, WasmValueType::I32],
        results: I32_RESULT,
    },
    HookProfile {
        name: "proxy_on_response_trailers",
        support: HookSupport::Implemented,
        params: &[WasmValueType::I32, WasmValueType::I32],
        results: I32_RESULT,
    },
    HookProfile {
        name: "proxy_on_http_call_response",
        support: HookSupport::Implemented,
        params: &[
            WasmValueType::I32,
            WasmValueType::I32,
            WasmValueType::I32,
            WasmValueType::I32,
            WasmValueType::I32,
        ],
        results: NO_RESULTS,
    },
    HookProfile {
        name: "proxy_on_grpc_receive_initial_metadata",
        support: HookSupport::KnownNotInvoked,
        params: &[WasmValueType::I32, WasmValueType::I32, WasmValueType::I32],
        results: NO_RESULTS,
    },
    HookProfile {
        name: "proxy_on_grpc_receive",
        support: HookSupport::KnownNotInvoked,
        params: &[WasmValueType::I32, WasmValueType::I32, WasmValueType::I32],
        results: NO_RESULTS,
    },
    HookProfile {
        name: "proxy_on_grpc_receive_trailing_metadata",
        support: HookSupport::KnownNotInvoked,
        params: &[WasmValueType::I32, WasmValueType::I32, WasmValueType::I32],
        results: NO_RESULTS,
    },
    HookProfile {
        name: "proxy_on_grpc_close",
        support: HookSupport::KnownNotInvoked,
        params: &[WasmValueType::I32, WasmValueType::I32, WasmValueType::I32],
        results: NO_RESULTS,
    },
    HookProfile {
        name: "proxy_on_foreign_function",
        support: HookSupport::KnownNotInvoked,
        params: &[WasmValueType::I32, WasmValueType::I32, WasmValueType::I32],
        results: NO_RESULTS,
    },
];

#[derive(Debug, Clone, Copy)]
struct AnalysisLimits {
    entries: u64,
    inventory_items: u64,
    inventory_name_bytes: u64,
    type_signature_cells: u64,
    report_signature_cells: u64,
    function_locals: u64,
    total_locals: u64,
    function_body_bytes: u64,
}

const DEFAULT_ANALYSIS_LIMITS: AnalysisLimits = AnalysisLimits {
    entries: MAX_ANALYSIS_ENTRIES,
    inventory_items: MAX_INVENTORY_ITEMS,
    inventory_name_bytes: MAX_INVENTORY_NAME_BYTES,
    type_signature_cells: MAX_TYPE_SIGNATURE_CELLS,
    report_signature_cells: MAX_REPORT_SIGNATURE_CELLS,
    function_locals: MAX_FUNCTION_LOCALS,
    total_locals: MAX_TOTAL_LOCALS,
    function_body_bytes: MAX_FUNCTION_BODY_BYTES,
};

#[derive(Debug, Default, Clone, Copy)]
struct AnalysisComplexity {
    entries: u64,
    inventory_items: u64,
    inventory_name_bytes: u64,
    type_signature_cells: u64,
    report_signature_cells: u64,
    total_locals: u64,
    max_function_body_bytes: u64,
}

impl AnalysisComplexity {
    fn record_entry(&mut self, limits: AnalysisLimits) -> Result<(), AnalysisPreflightError> {
        self.record_entries(1, limits)
    }

    fn record_entries(&mut self, count: u64, limits: AnalysisLimits) -> Result<(), AnalysisPreflightError> {
        self.entries = self
            .entries
            .checked_add(count)
            .ok_or(AnalysisPreflightError::LimitExceeded)?;
        self.check(limits)
    }

    fn record_type(&mut self, signature_cells: usize, limits: AnalysisLimits) -> Result<(), AnalysisPreflightError> {
        self.record_entry(limits)?;
        self.type_signature_cells = self
            .type_signature_cells
            .checked_add(u64::try_from(signature_cells).unwrap_or(u64::MAX))
            .ok_or(AnalysisPreflightError::LimitExceeded)?;
        self.check(limits)
    }

    fn record_inventory(&mut self, name_bytes: usize, limits: AnalysisLimits) -> Result<(), AnalysisPreflightError> {
        self.record_entry(limits)?;
        self.inventory_items = self
            .inventory_items
            .checked_add(1)
            .ok_or(AnalysisPreflightError::LimitExceeded)?;
        self.inventory_name_bytes = self
            .inventory_name_bytes
            .checked_add(u64::try_from(name_bytes).unwrap_or(u64::MAX))
            .ok_or(AnalysisPreflightError::LimitExceeded)?;
        self.check(limits)
    }

    fn record_report_signature(
        &mut self,
        signature_cells: u64,
        limits: AnalysisLimits,
    ) -> Result<(), AnalysisPreflightError> {
        self.report_signature_cells = self
            .report_signature_cells
            .checked_add(signature_cells)
            .ok_or(AnalysisPreflightError::LimitExceeded)?;
        self.check(limits)
    }

    fn record_function_locals(
        &mut self,
        function_locals: u64,
        limits: AnalysisLimits,
    ) -> Result<(), AnalysisPreflightError> {
        if function_locals > limits.function_locals {
            return Err(AnalysisPreflightError::LimitExceeded);
        }
        self.total_locals = self
            .total_locals
            .checked_add(function_locals)
            .ok_or(AnalysisPreflightError::LimitExceeded)?;
        self.check(limits)
    }

    fn record_function_body(
        &mut self,
        body_bytes: usize,
        limits: AnalysisLimits,
    ) -> Result<(), AnalysisPreflightError> {
        let body_bytes = u64::try_from(body_bytes).unwrap_or(u64::MAX);
        if body_bytes > limits.function_body_bytes {
            return Err(AnalysisPreflightError::LimitExceeded);
        }
        self.max_function_body_bytes = self.max_function_body_bytes.max(body_bytes);
        Ok(())
    }

    fn check(&self, limits: AnalysisLimits) -> Result<(), AnalysisPreflightError> {
        if self.entries > limits.entries
            || self.inventory_items > limits.inventory_items
            || self.inventory_name_bytes > limits.inventory_name_bytes
            || self.type_signature_cells > limits.type_signature_cells
            || self.report_signature_cells > limits.report_signature_cells
            || self.total_locals > limits.total_locals
        {
            return Err(AnalysisPreflightError::LimitExceeded);
        }
        Ok(())
    }
}

#[derive(Debug)]
enum AnalysisPreflightError {
    Malformed(String),
    LimitExceeded,
}

/// Analyze core Wasm bytes against Edgion's exact Proxy-Wasm host profile.
pub fn analyze(bytes: &[u8]) -> CompatibilityReport {
    analyze_with_limits(bytes, DEFAULT_ANALYSIS_LIMITS)
}

fn analyze_with_limits(bytes: &[u8], limits: AnalysisLimits) -> CompatibilityReport {
    let mut report = CompatibilityReport::new(bytes);
    let encoding = match Parser::new(0).parse_all(bytes).next() {
        Some(Ok(Payload::Version { encoding, .. })) => encoding,
        Some(Ok(_)) | None => {
            report.error(CompatibilityCode::Malformed, "input has no WebAssembly header");
            return report;
        }
        Some(Err(error)) => {
            report.error(CompatibilityCode::Malformed, bounded_parse_error(&error));
            return report;
        }
    };
    if encoding == Encoding::Component {
        report.error(
            CompatibilityCode::ComponentUnsupported,
            "WebAssembly components are not supported; provide a core module",
        );
        return report;
    }
    if let Err(error) = preflight_complexity(bytes, limits) {
        record_preflight_error(&mut report, error);
        return report;
    }
    if let Err(error) = Validator::new().validate_all(bytes) {
        report.error(CompatibilityCode::Malformed, bounded_parse_error(&error));
        return report;
    }
    analyze_valid_module(bytes, &mut report, limits);
    report
}

fn scan_function_type_cells(
    bytes: &[u8],
    range: std::ops::Range<usize>,
    count: u32,
    max_cells: u64,
    mut on_type: impl FnMut(u64) -> Result<(), AnalysisPreflightError>,
) -> Result<u64, AnalysisPreflightError> {
    let data = bytes
        .get(range.clone())
        .ok_or_else(|| AnalysisPreflightError::Malformed("type section range is outside the module".to_string()))?;
    let mut reader = BinaryReader::new(data, range.start);
    let encoded_count = reader
        .read_var_u32()
        .map_err(|error| AnalysisPreflightError::Malformed(bounded_parse_error(&error)))?;
    if encoded_count != count {
        return Err(AnalysisPreflightError::Malformed(
            "type section count changed while re-reading".to_string(),
        ));
    }
    let mut total_cells = 0u64;
    for _ in 0..count {
        let mut form = reader
            .read_u8()
            .map_err(|error| AnalysisPreflightError::Malformed(bounded_parse_error(&error)))?;
        if form == 0x4e {
            let group_count = reader
                .read_var_u32()
                .map_err(|error| AnalysisPreflightError::Malformed(bounded_parse_error(&error)))?;
            if group_count != 1 {
                return Err(AnalysisPreflightError::Malformed(
                    "multi-type recursive groups are not supported".to_string(),
                ));
            }
            form = reader
                .read_u8()
                .map_err(|error| AnalysisPreflightError::Malformed(bounded_parse_error(&error)))?;
        }
        match form {
            0x4f => {
                let supertypes = reader
                    .read_var_u32()
                    .map_err(|error| AnalysisPreflightError::Malformed(bounded_parse_error(&error)))?;
                if supertypes != 0 {
                    return Err(AnalysisPreflightError::Malformed(
                        "function type supertypes are not supported".to_string(),
                    ));
                }
                form = reader
                    .read_u8()
                    .map_err(|error| AnalysisPreflightError::Malformed(bounded_parse_error(&error)))?;
            }
            0x50 => {
                return Err(AnalysisPreflightError::Malformed(
                    "non-final function types are not supported".to_string(),
                ));
            }
            _ => {}
        }
        if form == 0x65 {
            form = reader
                .read_u8()
                .map_err(|error| AnalysisPreflightError::Malformed(bounded_parse_error(&error)))?;
        }
        if form == 0x4c {
            reader
                .read_var_u32()
                .map_err(|error| AnalysisPreflightError::Malformed(bounded_parse_error(&error)))?;
            form = reader
                .read_u8()
                .map_err(|error| AnalysisPreflightError::Malformed(bounded_parse_error(&error)))?;
        }
        if form == 0x4d {
            reader
                .read_var_u32()
                .map_err(|error| AnalysisPreflightError::Malformed(bounded_parse_error(&error)))?;
            form = reader
                .read_u8()
                .map_err(|error| AnalysisPreflightError::Malformed(bounded_parse_error(&error)))?;
        }
        if form != 0x60 {
            return Err(AnalysisPreflightError::Malformed(
                "only function types are supported".to_string(),
            ));
        }
        let params = u64::from(
            reader
                .read_var_u32()
                .map_err(|error| AnalysisPreflightError::Malformed(bounded_parse_error(&error)))?,
        );
        if total_cells.checked_add(params).is_none_or(|cells| cells > max_cells) {
            return Err(AnalysisPreflightError::LimitExceeded);
        }
        for _ in 0..params {
            reader
                .read::<ValType>()
                .map_err(|error| AnalysisPreflightError::Malformed(bounded_parse_error(&error)))?;
        }
        let results = reader
            .read_var_u32()
            .map_err(|error| AnalysisPreflightError::Malformed(bounded_parse_error(&error)))?;
        let type_cells = params
            .checked_add(u64::from(results))
            .ok_or(AnalysisPreflightError::LimitExceeded)?;
        total_cells = total_cells
            .checked_add(type_cells)
            .ok_or(AnalysisPreflightError::LimitExceeded)?;
        if total_cells > max_cells {
            return Err(AnalysisPreflightError::LimitExceeded);
        }
        for _ in 0..results {
            reader
                .read::<ValType>()
                .map_err(|error| AnalysisPreflightError::Malformed(bounded_parse_error(&error)))?;
        }
        on_type(type_cells)?;
    }
    if !reader.eof() {
        return Err(AnalysisPreflightError::Malformed(
            "type section has trailing bytes".to_string(),
        ));
    }
    Ok(total_cells)
}

fn preflight_complexity(bytes: &[u8], limits: AnalysisLimits) -> Result<AnalysisComplexity, AnalysisPreflightError> {
    let mut complexity = AnalysisComplexity::default();
    let mut function_declarations = 0u64;
    let mut code_declarations = 0u64;
    let mut code_bodies = 0u64;
    let mut data_segments = 0u64;
    let mut declared_data_count = None;
    for payload in Parser::new(0).parse_all(bytes) {
        let payload = payload.map_err(|error| AnalysisPreflightError::Malformed(bounded_parse_error(&error)))?;
        match payload {
            Payload::TypeSection(reader) => {
                let remaining = limits
                    .type_signature_cells
                    .saturating_sub(complexity.type_signature_cells);
                scan_function_type_cells(bytes, reader.range(), reader.count(), remaining, |cells| {
                    complexity.record_type(usize::try_from(cells).unwrap_or(usize::MAX), limits)
                })?;
            }
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import =
                        import.map_err(|error| AnalysisPreflightError::Malformed(bounded_parse_error(&error)))?;
                    complexity.record_inventory(import.module.len().saturating_add(import.name.len()), limits)?;
                }
            }
            Payload::FunctionSection(reader) => {
                let count = u64::from(reader.count());
                function_declarations = function_declarations
                    .checked_add(count)
                    .ok_or(AnalysisPreflightError::LimitExceeded)?;
                complexity.record_entries(count, limits)?;
            }
            Payload::TableSection(reader) => {
                complexity.record_entries(u64::from(reader.count()), limits)?;
            }
            Payload::MemorySection(reader) => {
                complexity.record_entries(u64::from(reader.count()), limits)?;
            }
            Payload::TagSection(reader) => {
                complexity.record_entries(u64::from(reader.count()), limits)?;
            }
            Payload::GlobalSection(reader) => {
                complexity.record_entries(u64::from(reader.count()), limits)?;
            }
            Payload::ExportSection(reader) => {
                for export in reader {
                    let export =
                        export.map_err(|error| AnalysisPreflightError::Malformed(bounded_parse_error(&error)))?;
                    complexity.record_inventory(export.name.len(), limits)?;
                }
            }
            Payload::ElementSection(reader) => {
                complexity.record_entries(u64::from(reader.count()), limits)?;
            }
            Payload::DataCountSection { count, .. } => {
                if declared_data_count.replace(u64::from(count)).is_some() {
                    return Err(AnalysisPreflightError::Malformed(
                        "module contains more than one data count section".to_string(),
                    ));
                }
                if u64::from(count) > limits.entries {
                    return Err(AnalysisPreflightError::LimitExceeded);
                }
            }
            Payload::DataSection(reader) => {
                let count = u64::from(reader.count());
                data_segments = data_segments
                    .checked_add(count)
                    .ok_or(AnalysisPreflightError::LimitExceeded)?;
                complexity.record_entries(count, limits)?;
            }
            Payload::CodeSectionStart { count, .. } => {
                let count = u64::from(count);
                if count > limits.entries {
                    return Err(AnalysisPreflightError::LimitExceeded);
                }
                code_declarations = code_declarations
                    .checked_add(count)
                    .ok_or(AnalysisPreflightError::LimitExceeded)?;
            }
            Payload::CodeSectionEntry(body) => {
                code_bodies = code_bodies
                    .checked_add(1)
                    .ok_or(AnalysisPreflightError::LimitExceeded)?;
                complexity.record_function_body(body.as_bytes().len(), limits)?;
                let locals = body
                    .get_locals_reader()
                    .map_err(|error| AnalysisPreflightError::Malformed(bounded_parse_error(&error)))?;
                let mut function_locals = 0u64;
                for local in locals {
                    let (count, _) =
                        local.map_err(|error| AnalysisPreflightError::Malformed(bounded_parse_error(&error)))?;
                    function_locals = function_locals
                        .checked_add(u64::from(count))
                        .ok_or(AnalysisPreflightError::LimitExceeded)?;
                    if function_locals > limits.function_locals {
                        return Err(AnalysisPreflightError::LimitExceeded);
                    }
                }
                complexity.record_function_locals(function_locals, limits)?;
            }
            _ => {}
        }
    }
    if code_declarations != function_declarations || code_bodies != code_declarations {
        return Err(AnalysisPreflightError::Malformed(
            "function and code section counts do not match".to_string(),
        ));
    }
    if declared_data_count.is_some_and(|count| count != data_segments) {
        return Err(AnalysisPreflightError::Malformed(
            "data count and data section counts do not match".to_string(),
        ));
    }
    preflight_report_signature_cells(bytes, limits, &mut complexity)?;
    Ok(complexity)
}

fn preflight_report_signature_cells(
    bytes: &[u8],
    limits: AnalysisLimits,
    complexity: &mut AnalysisComplexity,
) -> Result<(), AnalysisPreflightError> {
    // The streaming shape pass above has already bounded the combined entry
    // count and all type signature cells. These compact u32/u64 indexes are
    // therefore bounded to well under 1 MiB and avoid constructing any report
    // String or per-signature Vec during preflight.
    let mut type_cells = Vec::new();
    let mut function_type_indices = Vec::new();
    let mut scanned_type_cells = 0u64;
    for payload in Parser::new(0).parse_all(bytes) {
        let payload = payload.map_err(|error| AnalysisPreflightError::Malformed(bounded_parse_error(&error)))?;
        match payload {
            Payload::TypeSection(reader) => {
                let remaining = limits.type_signature_cells.saturating_sub(scanned_type_cells);
                let section_cells =
                    scan_function_type_cells(bytes, reader.range(), reader.count(), remaining, |cells| {
                        type_cells.push(cells);
                        Ok(())
                    })?;
                scanned_type_cells = scanned_type_cells
                    .checked_add(section_cells)
                    .ok_or(AnalysisPreflightError::LimitExceeded)?;
            }
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import =
                        import.map_err(|error| AnalysisPreflightError::Malformed(bounded_parse_error(&error)))?;
                    if let TypeRef::Func(index) | TypeRef::FuncExact(index) = import.ty {
                        function_type_indices.push(index);
                        if let Some(cells) = type_cells.get(index as usize) {
                            complexity.record_report_signature(*cells, limits)?;
                        }
                    }
                }
            }
            Payload::FunctionSection(reader) => {
                for index in reader {
                    function_type_indices
                        .push(index.map_err(|error| AnalysisPreflightError::Malformed(bounded_parse_error(&error)))?);
                }
            }
            Payload::ExportSection(reader) => {
                for export in reader {
                    let export =
                        export.map_err(|error| AnalysisPreflightError::Malformed(bounded_parse_error(&error)))?;
                    if is_function_export_kind(export.kind) {
                        let cells = function_type_indices
                            .get(export.index as usize)
                            .and_then(|index| type_cells.get(*index as usize));
                        if let Some(cells) = cells {
                            complexity.record_report_signature(*cells, limits)?;
                            if HOOKS.iter().any(|hook| hook.name == export.name) {
                                complexity.record_report_signature(*cells, limits)?;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn record_preflight_error(report: &mut CompatibilityReport, error: AnalysisPreflightError) {
    match error {
        AnalysisPreflightError::Malformed(message) => report.error(CompatibilityCode::Malformed, message),
        AnalysisPreflightError::LimitExceeded => report.error(
            CompatibilityCode::AnalysisLimitExceeded,
            "module exceeds compatibility analysis limits",
        ),
    }
}

fn analyze_valid_module(bytes: &[u8], report: &mut CompatibilityReport, limits: AnalysisLimits) {
    let mut types: Vec<FuncType> = Vec::new();
    let mut function_type_indices: Vec<u32> = Vec::new();
    let mut pending_exports = Vec::new();
    let mut complexity = AnalysisComplexity::default();

    for payload in Parser::new(0).parse_all(bytes) {
        let Ok(payload) = payload else {
            report.error(CompatibilityCode::Malformed, "validated module could not be re-read");
            return;
        };
        match payload {
            Payload::TypeSection(reader) => {
                for ty in reader.into_iter_err_on_gc_types() {
                    match ty {
                        Ok(ty) => {
                            if let Err(error) =
                                complexity.record_type(ty.params().len().saturating_add(ty.results().len()), limits)
                            {
                                record_preflight_error(report, error);
                                return;
                            }
                            types.push(ty);
                        }
                        Err(error) => {
                            report.error(CompatibilityCode::Malformed, bounded_parse_error(&error));
                            return;
                        }
                    }
                }
            }
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let Ok(import) = import else {
                        report.error(
                            CompatibilityCode::Malformed,
                            "validated import section could not be re-read",
                        );
                        return;
                    };
                    if let Err(error) =
                        complexity.record_inventory(import.module.len().saturating_add(import.name.len()), limits)
                    {
                        record_preflight_error(report, error);
                        return;
                    }
                    let signature_type = match import.ty {
                        TypeRef::Func(index) | TypeRef::FuncExact(index) => {
                            function_type_indices.push(index);
                            types.get(index as usize)
                        }
                        _ => None,
                    };
                    if let Some(ty) = signature_type {
                        let cells =
                            u64::try_from(ty.params().len().saturating_add(ty.results().len())).unwrap_or(u64::MAX);
                        if let Err(error) = complexity.record_report_signature(cells, limits) {
                            record_preflight_error(report, error);
                            return;
                        }
                    }
                    let signature = signature_type.map(signature);
                    let support = import_support(import.module, import.name, import.ty, &types);
                    report.imports.push(ImportInfo {
                        module: import.module.to_string(),
                        name: import.name.to_string(),
                        kind: import_kind(import.ty).to_string(),
                        support,
                        signature,
                    });
                    validate_import(import.module, import.name, import.ty, &types, report);
                    if support == ImportSupport::AcceptedNoop {
                        report.warnings.push(CompatibilityIssue {
                            code: CompatibilityCode::AcceptedNoopImport,
                            message: format!(
                                "host import `{}::{}` returns OK without implementing the operation",
                                import.module, import.name
                            ),
                        });
                    }
                    if support == ImportSupport::KnownUnsupportedTrap {
                        report.warnings.push(CompatibilityIssue {
                            code: CompatibilityCode::KnownUnsupportedImport,
                            message: format!(
                                "known host import `{}::{}` will trap if invoked",
                                import.module, import.name
                            ),
                        });
                    }
                }
            }
            Payload::FunctionSection(reader) => {
                for index in reader {
                    match index {
                        Ok(index) => {
                            if let Err(error) = complexity.record_entry(limits) {
                                record_preflight_error(report, error);
                                return;
                            }
                            function_type_indices.push(index);
                        }
                        Err(error) => {
                            report.error(CompatibilityCode::Malformed, bounded_parse_error(&error));
                            return;
                        }
                    }
                }
            }
            Payload::ExportSection(reader) => {
                for export in reader {
                    match export {
                        Ok(export) => {
                            if let Err(error) = complexity.record_inventory(export.name.len(), limits) {
                                record_preflight_error(report, error);
                                return;
                            }
                            if is_function_export_kind(export.kind) {
                                let cells = function_type_indices
                                    .get(export.index as usize)
                                    .and_then(|index| types.get(*index as usize))
                                    .map(|ty| {
                                        u64::try_from(ty.params().len().saturating_add(ty.results().len()))
                                            .unwrap_or(u64::MAX)
                                    });
                                if let Some(cells) = cells {
                                    if let Err(error) = complexity.record_report_signature(cells, limits) {
                                        record_preflight_error(report, error);
                                        return;
                                    }
                                    if HOOKS.iter().any(|hook| hook.name == export.name) {
                                        if let Err(error) = complexity.record_report_signature(cells, limits) {
                                            record_preflight_error(report, error);
                                            return;
                                        }
                                    }
                                }
                            }
                            pending_exports.push(export);
                        }
                        Err(error) => {
                            report.error(CompatibilityCode::Malformed, bounded_parse_error(&error));
                            return;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    for export in pending_exports {
        let signature = if is_function_export_kind(export.kind) {
            function_type_indices
                .get(export.index as usize)
                .and_then(|index| types.get(*index as usize))
                .map(signature)
        } else {
            None
        };
        report.exports.push(ExportInfo {
            name: export.name.to_string(),
            kind: export_kind(export.kind).to_string(),
            signature: signature.clone(),
        });
        if export.name.starts_with(ABI_MARKER_PREFIX) {
            report.abi_markers.push(export.name.to_string());
            if !is_function_export_kind(export.kind) {
                report.error(
                    CompatibilityCode::InvalidAbiMarker,
                    format!("ABI marker `{}` is not a function export", export.name),
                );
            } else if let Some(actual) = signature.as_ref() {
                if !actual.params.is_empty() || !actual.results.is_empty() {
                    report.error(
                        CompatibilityCode::InvalidAbiMarker,
                        format!("ABI marker `{}` must have signature () -> ()", export.name),
                    );
                }
            }
        }
        if let Some(profile) = HOOKS.iter().find(|hook| hook.name == export.name) {
            match signature {
                Some(actual) => {
                    if matches_profile(&actual, profile.params, profile.results) {
                        report.hooks.push(HookInfo {
                            name: export.name.to_string(),
                            support: profile.support,
                            signature: actual,
                        });
                        if export.name == "proxy_on_response_trailers" {
                            report.warnings.push(CompatibilityIssue {
                                code: CompatibilityCode::ConditionalHook,
                                message: "response trailers are invoked only for request entries with response.trailers enabled; observation is read-only".into(),
                            });
                        }
                        if profile.support == HookSupport::KnownNotInvoked {
                            report.warnings.push(CompatibilityIssue {
                                code: CompatibilityCode::KnownNotInvokedHook,
                                message: format!("hook `{}` is recognized but is not invoked by Edgion", export.name),
                            });
                        }
                    } else {
                        report.error(
                            CompatibilityCode::HookSignatureMismatch,
                            format!("hook `{}` has an incompatible function signature", export.name),
                        );
                    }
                }
                None => report.error(
                    CompatibilityCode::NonFunctionHook,
                    format!("hook `{}` is not a function export", export.name),
                ),
            }
        } else if export.name.starts_with("proxy_on_") {
            report.error(
                CompatibilityCode::UnknownHook,
                format!("unknown proxy-wasm hook export `{}`", export.name),
            );
        }
    }
    validate_markers(report);
}

fn validate_import(module: &str, name: &str, ty: TypeRef, types: &[FuncType], report: &mut CompatibilityReport) {
    let known = host::FUNCTIONS.iter().find(|function| function.name == name);
    let Some(function) = known else {
        report.error(
            CompatibilityCode::UnknownImport,
            format!("unknown import `{module}::{name}`"),
        );
        return;
    };
    if module != function.module {
        report.error(
            CompatibilityCode::WrongImportModule,
            format!("host import `{name}` must use module `{}`", function.module),
        );
        return;
    }
    let (TypeRef::Func(type_index) | TypeRef::FuncExact(type_index)) = ty else {
        report.error(
            CompatibilityCode::NonFunctionImport,
            format!("host import `{module}::{name}` is not a function"),
        );
        return;
    };
    let Some(actual) = types.get(type_index as usize).map(signature) else {
        report.error(
            CompatibilityCode::ImportSignatureMismatch,
            format!("host import `{module}::{name}` has no resolvable function signature"),
        );
        return;
    };
    if !matches_profile(&actual, function.signature.params, function.signature.results) {
        report.error(
            CompatibilityCode::ImportSignatureMismatch,
            format!("host import `{module}::{name}` has an incompatible function signature"),
        );
    }
}

fn import_support(module: &str, name: &str, ty: TypeRef, types: &[FuncType]) -> ImportSupport {
    let Some(function) = host::FUNCTIONS
        .iter()
        .find(|function| function.module == module && function.name == name)
    else {
        return ImportSupport::Unknown;
    };
    let (TypeRef::Func(type_index) | TypeRef::FuncExact(type_index)) = ty else {
        return ImportSupport::Unknown;
    };
    let Some(actual) = types.get(type_index as usize).map(signature) else {
        return ImportSupport::Unknown;
    };
    if !matches_profile(&actual, function.signature.params, function.signature.results) {
        return ImportSupport::Unknown;
    }
    match function.behavior {
        HostBehavior::Implemented => ImportSupport::Implemented,
        HostBehavior::Trap => ImportSupport::KnownUnsupportedTrap,
        HostBehavior::AcceptedNoop => ImportSupport::AcceptedNoop,
    }
}

fn validate_markers(report: &mut CompatibilityReport) {
    if report.abi_markers.is_empty() {
        report.warnings.push(CompatibilityIssue {
            code: CompatibilityCode::MissingAbiMarker,
            message: "module exports no proxy-wasm ABI marker; compatibility is inferred from imports and hooks"
                .to_string(),
        });
        return;
    }
    let supported = report
        .abi_markers
        .iter()
        .filter(|marker| SUPPORTED_ABI_MARKERS.contains(&marker.as_str()))
        .count();
    let unsupported = report.abi_markers.len().saturating_sub(supported);
    if report.abi_markers.len() > 1 {
        report.error(
            CompatibilityCode::MixedAbiMarkers,
            "module exports multiple proxy-wasm ABI markers",
        );
    } else if unsupported > 0 {
        report.error(
            CompatibilityCode::UnsupportedAbiMarker,
            "module targets an unsupported proxy-wasm ABI version",
        );
    }
}

fn signature(ty: &FuncType) -> FunctionSignature {
    FunctionSignature {
        params: ty.params().iter().map(value_type_name).collect(),
        results: ty.results().iter().map(value_type_name).collect(),
    }
}

fn value_type_name(ty: &ValType) -> String {
    ty.to_string()
}

fn matches_profile(actual: &FunctionSignature, params: &[WasmValueType], results: &[WasmValueType]) -> bool {
    actual.params.len() == params.len()
        && actual.results.len() == results.len()
        && actual
            .params
            .iter()
            .zip(params)
            .all(|(actual, expected)| actual == value_type_profile_name(*expected))
        && actual
            .results
            .iter()
            .zip(results)
            .all(|(actual, expected)| actual == value_type_profile_name(*expected))
}

fn value_type_profile_name(ty: WasmValueType) -> &'static str {
    match ty {
        WasmValueType::I32 => "i32",
        WasmValueType::I64 => "i64",
        WasmValueType::F32 => "f32",
        WasmValueType::F64 => "f64",
        WasmValueType::V128 => "v128",
    }
}

fn import_kind(ty: TypeRef) -> &'static str {
    match ty {
        TypeRef::Func(_) | TypeRef::FuncExact(_) => "function",
        TypeRef::Table(_) => "table",
        TypeRef::Memory(_) => "memory",
        TypeRef::Global(_) => "global",
        TypeRef::Tag(_) => "tag",
    }
}

fn export_kind(kind: ExternalKind) -> &'static str {
    match kind {
        ExternalKind::Func | ExternalKind::FuncExact => "function",
        ExternalKind::Table => "table",
        ExternalKind::Memory => "memory",
        ExternalKind::Global => "global",
        ExternalKind::Tag => "tag",
    }
}

fn is_function_export_kind(kind: ExternalKind) -> bool {
    matches!(kind, ExternalKind::Func | ExternalKind::FuncExact)
}

fn bounded_parse_error(error: &wasmparser::BinaryReaderError) -> String {
    const MAX_ERROR_BYTES: usize = 240;
    let message = error.to_string();
    if message.len() <= MAX_ERROR_BYTES {
        message
    } else {
        let end = message
            .char_indices()
            .take_while(|(index, _)| *index <= MAX_ERROR_BYTES)
            .map(|(index, _)| index)
            .last()
            .unwrap_or(0);
        format!("{}…", &message[..end])
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;

    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut output, "{byte:02x}").expect("writing to a String cannot fail");
    }
    output
}

#[cfg(test)]
#[path = "../tests/unit/lib_test.rs"]
mod tests;
