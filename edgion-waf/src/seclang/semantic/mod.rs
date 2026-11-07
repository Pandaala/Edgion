use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::iter::FusedIterator;
use std::mem::size_of;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::ops::Range;

use thiserror::Error;

use super::compiler::CanonicalWork;
use super::target::parse_single_target;
use super::{
    ActionEntryKind, ActionText, CompilerError, CompilerInvariant, CompilerLimits, DecodedTextView, Directive,
    DirectiveTargetView, IdSelector, IdSelectorKind, LogicalLine, OperatorNameView, OperatorView, SelectorKind,
    SourcePosition, SourceSpan, SyntaxDirectiveView, SyntaxDocument, SyntaxText, TargetModifier, TargetSyntaxError,
};

mod actions;
mod draft;
mod fold;
mod graph;
mod model;
mod operand;
mod scan;
mod template;
mod validate;

use actions::*;
use draft::*;
use fold::*;
use graph::*;
use model::*;
use operand::*;
use scan::*;
use template::*;
use validate::*;

pub const DEFAULT_MAX_SEMANTIC_DOCUMENTS: usize = 1_024;
pub const DEFAULT_MAX_SEMANTIC_DIRECTIVES: usize = 200_000;
pub const DEFAULT_MAX_SEMANTIC_RULES: usize = 100_000;
pub const DEFAULT_MAX_SEMANTIC_NODES: usize = 500_000;
pub const DEFAULT_MAX_SEMANTIC_OWNED_BYTES: usize = 64 * 1024 * 1024;
pub const DEFAULT_MAX_SEMANTIC_RETAINED_BYTES: usize = 256 * 1024 * 1024;
pub const DEFAULT_MAX_FOLDED_RULES: usize = 200_000;
pub const DEFAULT_MAX_EFFECTIVE_TRANSFORMATIONS: usize = 200_000;
pub const MARKER_WORK_ENTRY_CHARGE: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphLimits {
    pub max_chains: usize,
    pub max_chain_members: usize,
    pub max_members_per_chain: usize,
    pub max_markers: usize,
    pub max_marker_name_bytes: usize,
    pub max_marker_name_bytes_total: usize,
    pub max_skip_after_refs: usize,
    pub max_registry_work_bytes: usize,
}

impl Default for GraphLimits {
    fn default() -> Self {
        Self {
            max_chains: 100_000,
            max_chain_members: 100_000,
            max_members_per_chain: 1_024,
            max_markers: 100_000,
            max_marker_name_bytes: 4 * 1024,
            max_marker_name_bytes_total: 16 * 1024 * 1024,
            max_skip_after_refs: 200_000,
            max_registry_work_bytes: 32 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoldLimits {
    pub max_folded_rules: usize,
    pub max_effective_transformations: usize,
}

impl Default for FoldLimits {
    fn default() -> Self {
        Self {
            max_folded_rules: DEFAULT_MAX_FOLDED_RULES,
            max_effective_transformations: DEFAULT_MAX_EFFECTIVE_TRANSFORMATIONS,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperandLimits {
    pub max_operand_bytes: usize,
    pub max_scanned_bytes: usize,
    pub max_templates: usize,
    pub max_segments: usize,
    pub max_expansions: usize,
    pub max_expansions_per_template: usize,
    pub max_decoded_bytes: usize,
    pub max_runtime_expansion_bytes: usize,
    pub max_rule_expansion_bytes: usize,
    pub max_phrases_per_operator: usize,
    pub max_phrases: usize,
    pub max_phrase_bytes: usize,
    pub max_assets_per_operator: usize,
    pub max_assets: usize,
    pub max_asset_bytes: usize,
    pub max_networks_per_operator: usize,
    pub max_networks: usize,
    pub max_byte_range_members: usize,
    pub max_structured_actions: usize,
    pub max_ctl_targets: usize,
    pub max_setvar_key_bytes: usize,
    pub max_ctl_value_bytes: usize,
}

impl Default for OperandLimits {
    fn default() -> Self {
        Self {
            max_operand_bytes: 1024 * 1024,
            max_scanned_bytes: 64 * 1024 * 1024,
            max_templates: 500_000,
            max_segments: 1_000_000,
            max_expansions: 500_000,
            max_expansions_per_template: 1_024,
            max_decoded_bytes: 16 * 1024 * 1024,
            max_runtime_expansion_bytes: 64 * 1024,
            max_rule_expansion_bytes: 1024 * 1024,
            max_phrases_per_operator: 65_536,
            max_phrases: 500_000,
            max_phrase_bytes: 16 * 1024 * 1024,
            max_assets_per_operator: 64,
            max_assets: 65_536,
            max_asset_bytes: 4 * 1024,
            max_networks_per_operator: 4_096,
            max_networks: 65_536,
            max_byte_range_members: 1_024,
            max_structured_actions: 200_000,
            max_ctl_targets: 200_000,
            max_setvar_key_bytes: 4 * 1024,
            max_ctl_value_bytes: 64 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemanticLimits {
    pub max_documents: usize,
    pub max_directives: usize,
    pub max_rules: usize,
    pub max_nodes: usize,
    pub max_owned_bytes: usize,
    pub max_retained_bytes: usize,
    pub operands: OperandLimits,
    pub fold: FoldLimits,
    pub graph: GraphLimits,
}

impl Default for SemanticLimits {
    fn default() -> Self {
        Self {
            max_documents: DEFAULT_MAX_SEMANTIC_DOCUMENTS,
            max_directives: DEFAULT_MAX_SEMANTIC_DIRECTIVES,
            max_rules: DEFAULT_MAX_SEMANTIC_RULES,
            max_nodes: DEFAULT_MAX_SEMANTIC_NODES,
            max_owned_bytes: DEFAULT_MAX_SEMANTIC_OWNED_BYTES,
            max_retained_bytes: DEFAULT_MAX_SEMANTIC_RETAINED_BYTES,
            operands: OperandLimits::default(),
            fold: FoldLimits::default(),
            graph: GraphLimits::default(),
        }
    }
}

macro_rules! registry_enum {
    ($name:ident { $($variant:ident => $spelling:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum $name { $($variant),+ }

        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $spelling),+ }
            }

            fn lookup_ascii_insensitive(value: &[u8]) -> Option<Self> {
                $(if value.eq_ignore_ascii_case($spelling.as_bytes()) { return Some(Self::$variant); })+
                None
            }

        }
    };
}

registry_enum!(CollectionKind {
    Args => "ARGS", ArgsCombinedSize => "ARGS_COMBINED_SIZE", ArgsGet => "ARGS_GET",
    ArgsGetNames => "ARGS_GET_NAMES", ArgsNames => "ARGS_NAMES", Files => "FILES",
    FilesCombinedSize => "FILES_COMBINED_SIZE", FilesNames => "FILES_NAMES", MatchedVar => "MATCHED_VAR",
    MatchedVars => "MATCHED_VARS", MultipartPartHeaders => "MULTIPART_PART_HEADERS", QueryString => "QUERY_STRING",
    RemoteAddr => "REMOTE_ADDR", ReqbodyProcessor => "REQBODY_PROCESSOR", RequestBasename => "REQUEST_BASENAME",
    RequestBody => "REQUEST_BODY", RequestBodyLength => "REQUEST_BODY_LENGTH", RequestCookies => "REQUEST_COOKIES",
    RequestCookiesNames => "REQUEST_COOKIES_NAMES", RequestFilename => "REQUEST_FILENAME",
    RequestHeaders => "REQUEST_HEADERS", RequestHeadersNames => "REQUEST_HEADERS_NAMES", RequestLine => "REQUEST_LINE",
    RequestMethod => "REQUEST_METHOD", RequestProtocol => "REQUEST_PROTOCOL", RequestUri => "REQUEST_URI",
    RequestUriRaw => "REQUEST_URI_RAW", ResponseBody => "RESPONSE_BODY", ResponseHeaders => "RESPONSE_HEADERS",
    ResponseStatus => "RESPONSE_STATUS", Tx => "TX", UniqueId => "UNIQUE_ID", Xml => "XML",
});

registry_enum!(OperatorKind {
    BeginsWith => "beginsWith", Contains => "contains", DetectSqli => "detectSQLi", DetectXss => "detectXSS",
    EndsWith => "endsWith", Eq => "eq", Ge => "ge", Gt => "gt", IpMatch => "ipMatch", Lt => "lt", Pm => "pm",
    PmFromFile => "pmFromFile", Rx => "rx", Streq => "streq", UnconditionalMatch => "unconditionalMatch",
    ValidateByteRange => "validateByteRange", ValidateUrlEncoding => "validateUrlEncoding",
    ValidateUtf8Encoding => "validateUtf8Encoding", Within => "within",
});

registry_enum!(ActionKind {
    AuditLog => "auditlog", Block => "block", Capture => "capture", Chain => "chain", Ctl => "ctl", Deny => "deny",
    Id => "id", InitCol => "initcol", Log => "log", LogData => "logdata", Msg => "msg", MultiMatch => "multiMatch",
    NoAuditLog => "noauditlog", NoLog => "nolog", Pass => "pass", Phase => "phase", SetVar => "setvar",
    Severity => "severity", Skip => "skip", SkipAfter => "skipAfter", Status => "status", Tag => "tag", Ver => "ver",
});

registry_enum!(TransformationKind {
    Base64Decode => "base64Decode", CmdLine => "cmdLine", CompressWhitespace => "compressWhitespace",
    CssDecode => "cssDecode", EscapeSeqDecode => "escapeSeqDecode", HexEncode => "hexEncode",
    HtmlEntityDecode => "htmlEntityDecode", JsDecode => "jsDecode", Length => "length", Lowercase => "lowercase",
    None => "none", NormalizePath => "normalizePath", NormalizePathWin => "normalizePathWin",
    RemoveCommentsChar => "removeCommentsChar", RemoveNulls => "removeNulls", RemoveWhitespace => "removeWhitespace",
    ReplaceComments => "replaceComments", Sha1 => "sha1", UrlDecodeUni => "urlDecodeUni",
    Utf8ToUnicode => "utf8toUnicode",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionEnvelopeKind {
    Action(ActionKind),
    Transformation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    RequestHeaders = 1,
    RequestBody = 2,
    ResponseHeaders = 3,
    ResponseBody = 4,
    Logging = 5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Emergency = 0,
    Alert = 1,
    Critical = 2,
    Error = 3,
    Warning = 4,
    Notice = 5,
    Info = 6,
    Debug = 7,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalActionValue {
    None,
    Id(u64),
    Phase(Phase),
    Severity(Severity),
    Status(u16),
    Skip(u32),
    Unresolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetVarOperation {
    Assign,
    Add,
    Subtract,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitColCollection {
    Global,
    Ip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditEngineMode {
    On,
    Off,
    RelevantOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnOff {
    On,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestBodyProcessor {
    UrlEncoded,
    Multipart,
    Json,
    Xml,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompileRequirement {
    RegexCompilation,
    TargetSelectorRegexCompilation,
    PhraseMatcherCompilation,
    ExternalPhraseAsset,
    RuntimeMacro,
    BlockDefaultDisruptive,
    TransactionSetVar,
    DefaultPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnavailableCapability {
    FullResponseBody,
    Ctl,
    InitCol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultDisruptive {
    Pass,
    Deny { status: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhaseDefaults {
    phase: Phase,
    disruptive: DefaultDisruptive,
    log: bool,
    audit_log: bool,
}

impl PhaseDefaults {
    pub const fn pass(phase: Phase, log: bool, audit_log: bool) -> Self {
        Self {
            phase,
            disruptive: DefaultDisruptive::Pass,
            log,
            audit_log,
        }
    }

    pub fn try_deny(phase: Phase, status: u16, log: bool, audit_log: bool) -> Result<Self, DefaultsContextError> {
        if !(200..=599).contains(&status) {
            return Err(DefaultsContextError::InvalidStatus { phase });
        }
        Ok(Self {
            phase,
            disruptive: DefaultDisruptive::Deny { status },
            log,
            audit_log,
        })
    }

    pub const fn phase(self) -> Phase {
        self.phase
    }
    pub const fn disruptive(self) -> DefaultDisruptive {
        self.disruptive
    }
    pub const fn log(self) -> bool {
        self.log
    }
    pub const fn audit_log(self) -> bool {
        self.audit_log
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum DefaultsContextError {
    #[error("phase defaults contain duplicate {phase:?} entries")]
    DuplicatePhase { phase: Phase },
    #[error("phase defaults contain an invalid status for {phase:?}")]
    InvalidStatus { phase: Phase },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemanticValidationContext {
    implicit_rule_phase: Option<Phase>,
    defaults: [Option<PhaseDefaults>; 5],
}

impl SemanticValidationContext {
    pub fn try_new(
        implicit_rule_phase: Option<Phase>,
        defaults: &[PhaseDefaults],
    ) -> Result<Self, DefaultsContextError> {
        let mut result = Self {
            implicit_rule_phase,
            defaults: [None; 5],
        };
        for value in defaults.iter().copied() {
            let slot = &mut result.defaults[phase_index(value.phase())];
            if slot.is_some() {
                return Err(DefaultsContextError::DuplicatePhase { phase: value.phase() });
            }
            if let DefaultDisruptive::Deny { status } = value.disruptive() {
                if !(200..=599).contains(&status) {
                    return Err(DefaultsContextError::InvalidStatus { phase: value.phase() });
                }
            }
            *slot = Some(value);
        }
        Ok(result)
    }

    pub const fn implicit_rule_phase(self) -> Option<Phase> {
        self.implicit_rule_phase
    }

    pub const fn phase_defaults(self, phase: Phase) -> Option<PhaseDefaults> {
        self.defaults[phase_index(phase)]
    }
}

const fn phase_index(phase: Phase) -> usize {
    phase as usize - 1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidationLimits {
    pub max_rule_units: usize,
    pub max_id_selectors: usize,
    pub max_raw_matches_per_directive: usize,
    pub max_distinct_matches_per_directive: usize,
    pub max_raw_matches: usize,
    pub max_distinct_matches: usize,
    pub max_selector_work: usize,
    pub max_final_targets_per_rule: usize,
    pub max_final_target_references: usize,
    pub max_applied_exceptions: usize,
    pub max_target_comparisons: usize,
    pub max_target_comparison_bytes: usize,
    pub max_owned_bytes: usize,
    pub max_working_bytes: usize,
    pub max_retained_bytes: usize,
}

impl Default for ValidationLimits {
    fn default() -> Self {
        Self {
            max_rule_units: 100_000,
            max_id_selectors: 200_000,
            max_raw_matches_per_directive: 100_000,
            max_distinct_matches_per_directive: 100_000,
            max_raw_matches: 500_000,
            max_distinct_matches: 500_000,
            max_selector_work: 2_000_000,
            max_final_targets_per_rule: 1_024,
            max_final_target_references: 1_000_000,
            max_applied_exceptions: 200_000,
            max_target_comparisons: 10_000_000,
            max_target_comparison_bytes: 256 * 1024 * 1024,
            max_owned_bytes: 64 * 1024 * 1024,
            max_working_bytes: 32 * 1024 * 1024,
            max_retained_bytes: DEFAULT_MAX_SEMANTIC_RETAINED_BYTES,
        }
    }
}

pub(crate) use draft::SemanticDraft;
pub use draft::{analyze_one_semantic_draft, FoldRequiredKind, SemanticError};
pub use graph::{DirectiveLocation, SemanticValidationError, ValidatedSemanticDocument};
pub(crate) use graph::{FinalRuleRole, ResolvedDisruptive, ValidatedRuleNodeView, ValidatedRuleUnitView};
pub use model::{
    ActionOperandView, CtlView, MacroExpansionView, OperandTemplateView, OperatorOperandView, SemanticLocation,
    TemplateSegmentKind, TemplateSliceView,
};
pub(crate) use model::{ExternalAssetView, MacroSource, SemanticActionView, SemanticNodeKind, SemanticOperatorView};
pub(crate) use validate::validate_executable_profile;
pub use validate::validate_semantic_draft;

#[cfg(test)]
#[path = "../../../tests/unit/seclang/semantic/mod_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../tests/support/seclang/semantic/mod.rs"]
mod test_support;
#[cfg(test)]
use test_support::VALIDATION_ALLOCATION_FAILURE;
