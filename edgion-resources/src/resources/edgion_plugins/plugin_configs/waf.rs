//! Native WAF logical-plugin configuration.

use edgion_waf::matcher::RegexProgramError;
use edgion_waf::seclang::{
    compile_with_includes, inspect_compatibility, CompatibilityLimits, CompatibilityReport, CompiledGeneration,
    CompilerError, DirectiveLocation, FoldRequiredKind, FrontendError, FrontendLimits, IncludeFailure, IncludeRequest,
    IncludeSink, IncludeSource, Phase, PhaseDefaults, SemanticError, SemanticLocation, SemanticValidationContext,
    SemanticValidationError, SourceError, SourcePosition, SyntaxError, UnavailableCapability,
    DEFAULT_REQUEST_BODY_PREFIX_BYTES, MAX_REQUEST_BODY_PREFIX_BYTES,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::edgion_plugins::PluginValidation;

/// Disruptive-action behavior for one WAF generation.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum WafMode {
    /// Apply disruptive rule actions.
    #[default]
    On,
    /// Record findings without disrupting traffic.
    DetectionOnly,
}

/// Request payload range inspected before forwarding.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum WafRequestBodyInspection {
    #[default]
    Prefix,
    Full,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WafRequestBodyConfig {
    #[serde(default)]
    pub inspection: WafRequestBodyInspection,
    /// Prefix window, default 64 KiB, at most 1 MiB. Only valid in prefix mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix_size: Option<String>,
}

impl WafRequestBodyConfig {
    /// Resolve once during configuration validation and plugin construction.
    pub fn prefix_size_bytes(&self) -> Result<usize, String> {
        if self.inspection == WafRequestBodyInspection::Full && self.prefix_size.is_some() {
            return Err("requestBody.prefixSize is only valid with prefix inspection".to_string());
        }
        let Some(value) = self.prefix_size.as_deref() else {
            return Ok(DEFAULT_REQUEST_BODY_PREFIX_BYTES);
        };
        let bytes = crate::resources::common::parse_byte_size(value)
            .ok_or("requestBody.prefixSize must be a valid byte size")?;
        if bytes == 0 || bytes > MAX_REQUEST_BODY_PREFIX_BYTES as u64 {
            return Err("requestBody.prefixSize must be between 1 byte and 1 MiB".to_string());
        }
        Ok(bytes as usize)
    }
}

/// One logical WAF configuration spanning all supported transaction phases.
#[derive(Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WafConfig {
    /// Inline SecLang source. Includes and filesystem paths are not available.
    #[serde(serialize_with = "crate::redact::redact_required::serialize")]
    pub rules: String,

    /// Whether disruptive actions are enforced.
    #[serde(default)]
    pub mode: WafMode,

    /// Prefix inspection uses a bounded memory window; full uses the shared body cache.
    #[serde(default)]
    pub request_body: WafRequestBodyConfig,

    /// Request-wide winner priority when multiple scopes attach a WAF.
    /// Higher values win; equal values prefer Route, then Gateway, then Global.
    #[serde(default)]
    pub priority: u16,
}

impl std::fmt::Debug for WafConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WafConfig")
            .field("rules", &"[redacted]")
            .field("mode", &self.mode)
            .field("request_body", &self.request_body)
            .field("priority", &self.priority)
            .finish()
    }
}

struct NoIncludes;

/// Value-free, bounded WAF support inspection for local tools and EdgionCenter.
/// A successful support analysis is not a publication decision.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WafSupportReport {
    pub report_version: u8,
    pub analysis_complete: bool,
    pub strict_validation_passed: bool,
    pub rule_count: Option<usize>,
    pub unavailable_rule_count: Option<usize>,
    pub unattributed_findings: Option<usize>,
    pub findings: Vec<WafSupportFinding>,
    pub omitted_findings: usize,
    pub omitted_rule_summaries: usize,
    pub failure: Option<WafSupportFailure>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WafSupportFinding {
    pub rule_id: Option<u64>,
    pub document_index: usize,
    pub directive_index: usize,
    pub capability: &'static str,
    pub reason: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WafSupportFailure {
    pub category: &'static str,
    pub reason: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directive_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub physical_line: Option<usize>,
}

impl WafSupportFailure {
    const fn new(category: &'static str, reason: &'static str) -> Self {
        Self {
            category,
            reason,
            document_index: None,
            directive_index: None,
            physical_line: None,
        }
    }

    fn at_location(mut self, location: SemanticLocation) -> Self {
        self.document_index = Some(location.document_index());
        self.directive_index = Some(location.directive_index());
        self
    }

    fn at_directive(mut self, location: DirectiveLocation) -> Self {
        self.document_index = Some(location.document_index());
        self.directive_index = Some(location.directive_index());
        self
    }
}

enum WafCompileFailure {
    Config(String),
    Frontend(FrontendError),
    Prefix(&'static str),
}

fn capability_name(capability: UnavailableCapability) -> (&'static str, &'static str) {
    match capability {
        UnavailableCapability::FullResponseBody => (
            "full_response_body",
            "Remove full-response-body rules; upstream responses are streaming",
        ),
        UnavailableCapability::Ctl => ("ctl", "Remove unsupported ctl actions"),
        UnavailableCapability::InitCol => ("initcol", "Remove unsupported persistent collection initialization"),
    }
}

fn target_resource_limit(error: &edgion_waf::seclang::TargetSyntaxError) -> bool {
    use edgion_waf::seclang::TargetSyntaxError;
    matches!(
        error,
        TargetSyntaxError::AtomLimit { .. }
            | TargetSyntaxError::IdSelectorLimit { .. }
            | TargetSyntaxError::NodeLimit { .. }
            | TargetSyntaxError::OwnedByteLimit { .. }
            | TargetSyntaxError::Allocation { .. }
    )
}

fn syntax_resource_limit(error: &SyntaxError) -> bool {
    use edgion_waf::seclang::{ActionSyntaxError, DirectiveParseError, OperatorSyntaxError, TokenizeError};
    match error {
        SyntaxError::RetainedByteLimit { .. } => true,
        SyntaxError::Tokenizer(error) => matches!(
            error,
            TokenizeError::TokenLimit { .. }
                | TokenizeError::LineTokenLimit { .. }
                | TokenizeError::TokenByteLimit { .. }
                | TokenizeError::OwnedByteLimit { .. }
                | TokenizeError::Allocation { .. }
        ),
        SyntaxError::Directive(error) => matches!(
            error,
            DirectiveParseError::DirectiveLimit { .. }
                | DirectiveParseError::UpdateSelectorLimit { .. }
                | DirectiveParseError::OwnedByteLimit { .. }
                | DirectiveParseError::Allocation { .. }
        ),
        SyntaxError::Target(error) => target_resource_limit(error),
        SyntaxError::Operator(error) => matches!(
            error,
            OperatorSyntaxError::MappingOwnedByteLimit { .. }
                | OperatorSyntaxError::OperatorLimit { .. }
                | OperatorSyntaxError::OperatorEscapeLimit { .. }
                | OperatorSyntaxError::DocumentEscapeLimit { .. }
                | OperatorSyntaxError::DecodedByteLimit { .. }
                | OperatorSyntaxError::OwnedByteLimit { .. }
                | OperatorSyntaxError::Allocation { .. }
        ),
        SyntaxError::Action(error) => matches!(
            error,
            ActionSyntaxError::MappingOwnedByteLimit { .. }
                | ActionSyntaxError::ListLimit { .. }
                | ActionSyntaxError::ScannedByteLimit { .. }
                | ActionSyntaxError::ListEntryLimit { .. }
                | ActionSyntaxError::DocumentEntryLimit { .. }
                | ActionSyntaxError::OwnedByteLimit { .. }
                | ActionSyntaxError::Allocation { .. }
        ),
        _ => false,
    }
}

fn semantic_resource_limit(error: &SemanticError) -> bool {
    if let SemanticError::CtlTarget(error) = error {
        return target_resource_limit(error);
    }
    matches!(
        error,
        SemanticError::DocumentLimit { .. }
            | SemanticError::DirectiveLimit { .. }
            | SemanticError::RuleLimit { .. }
            | SemanticError::NodeLimit { .. }
            | SemanticError::FoldLimit { .. }
            | SemanticError::MarkerLimit { .. }
            | SemanticError::SkipAfterLimit { .. }
            | SemanticError::MarkerNameByteLimit { .. }
            | SemanticError::MarkerNameTotalLimit { .. }
            | SemanticError::MarkerWorkLimit { .. }
            | SemanticError::ChainLimit { .. }
            | SemanticError::ChainMemberLimit { .. }
            | SemanticError::ChainLengthLimit { .. }
            | SemanticError::EffectiveTransformationLimit { .. }
            | SemanticError::PreflightOwnedByteLimit { .. }
            | SemanticError::RetainedByteLimit { .. }
            | SemanticError::OwnedByteLimit { .. }
            | SemanticError::OperandByteLimit { .. }
            | SemanticError::ScannedOperandByteLimit { .. }
            | SemanticError::TemplateLimit { .. }
            | SemanticError::TemplateSegmentLimit { .. }
            | SemanticError::ExpansionLimit { .. }
            | SemanticError::TemplateExpansionLimit { .. }
            | SemanticError::DecodedOperandByteLimit { .. }
            | SemanticError::OperatorPhraseLimit { .. }
            | SemanticError::PhraseLimit { .. }
            | SemanticError::PhraseByteLimit { .. }
            | SemanticError::OperatorAssetLimit { .. }
            | SemanticError::AssetLimit { .. }
            | SemanticError::AssetByteLimit { .. }
            | SemanticError::OperatorNetworkLimit { .. }
            | SemanticError::NetworkLimit { .. }
            | SemanticError::ByteRangeMemberLimit { .. }
            | SemanticError::StructuredActionLimit { .. }
            | SemanticError::CtlTargetLimit { .. }
            | SemanticError::SetVarKeyByteLimit { .. }
            | SemanticError::CtlValueByteLimit { .. }
            | SemanticError::Allocation { .. }
    )
}

fn semantic_position(error: &SemanticError) -> Option<SourcePosition> {
    match error {
        SemanticError::CtlTarget(error) => target_position(error),
        SemanticError::MarkerLimit { position, .. }
        | SemanticError::SkipAfterLimit { position, .. }
        | SemanticError::MarkerNameByteLimit { position, .. }
        | SemanticError::MarkerNameTotalLimit { position, .. }
        | SemanticError::MarkerWorkLimit { position, .. }
        | SemanticError::ChainLimit { position, .. }
        | SemanticError::ChainMemberLimit { position, .. }
        | SemanticError::ChainLengthLimit { position, .. }
        | SemanticError::EffectiveTransformationLimit { position, .. }
        | SemanticError::OwnedByteLimit { position, .. }
        | SemanticError::UnknownCollection { position }
        | SemanticError::UnknownOperator { position }
        | SemanticError::UnknownAction { position }
        | SemanticError::UnknownTransformation { position }
        | SemanticError::ValueContract { position, .. }
        | SemanticError::InvalidScalar { position, .. }
        | SemanticError::ScalarOverflow { position, .. }
        | SemanticError::MacroScalar { position, .. }
        | SemanticError::OperandByteLimit { position, .. }
        | SemanticError::ScannedOperandByteLimit { position, .. }
        | SemanticError::TemplateLimit { position, .. }
        | SemanticError::TemplateSegmentLimit { position, .. }
        | SemanticError::ExpansionLimit { position, .. }
        | SemanticError::TemplateExpansionLimit { position, .. }
        | SemanticError::DecodedOperandByteLimit { position, .. }
        | SemanticError::MacroSyntax { position, .. }
        | SemanticError::OperatorOperandPresence { position, .. }
        | SemanticError::IntegerOperand { position, .. }
        | SemanticError::PhraseSyntax { position }
        | SemanticError::OperatorPhraseLimit { position, .. }
        | SemanticError::PhraseLimit { position, .. }
        | SemanticError::PhraseByteLimit { position, .. }
        | SemanticError::OperatorAssetLimit { position, .. }
        | SemanticError::AssetLimit { position, .. }
        | SemanticError::AssetByteLimit { position, .. }
        | SemanticError::UnsafeAsset { position, .. }
        | SemanticError::IpNetworkSyntax { position }
        | SemanticError::OperatorNetworkLimit { position, .. }
        | SemanticError::NetworkLimit { position, .. }
        | SemanticError::ByteSetSyntax { position }
        | SemanticError::ByteRangeMemberLimit { position, .. }
        | SemanticError::SetVarSyntax { position, .. }
        | SemanticError::InitColSyntax { position, .. }
        | SemanticError::CtlSyntax { position, .. }
        | SemanticError::FoldDuplicate { position, .. }
        | SemanticError::SecActionForbidden { position, .. }
        | SemanticError::FoldMissing { position, .. }
        | SemanticError::StatusPass { position }
        | SemanticError::DanglingChain { position }
        | SemanticError::ChildAction { position, .. }
        | SemanticError::MarkerControl { position }
        | SemanticError::DuplicateMarker { position, .. }
        | SemanticError::UnresolvedSkipAfter { position }
        | SemanticError::NonForwardSkipAfter { position, .. }
        | SemanticError::StructuredActionLimit { position, .. }
        | SemanticError::CtlTargetLimit { position, .. }
        | SemanticError::SetVarKeyByteLimit { position, .. }
        | SemanticError::CtlValueByteLimit { position, .. }
        | SemanticError::TemplateSliceInvariant { position } => Some(*position),
        _ => None,
    }
}

fn target_position(error: &edgion_waf::seclang::TargetSyntaxError) -> Option<SourcePosition> {
    use edgion_waf::seclang::TargetSyntaxError;
    match error {
        TargetSyntaxError::AtomLimit { position, .. }
        | TargetSyntaxError::IdSelectorLimit { position, .. }
        | TargetSyntaxError::NodeLimit { position, .. }
        | TargetSyntaxError::EmptyAtom { position }
        | TargetSyntaxError::EmptyCollection { position }
        | TargetSyntaxError::InvalidModifier { position, .. }
        | TargetSyntaxError::UnclosedRegex { position, .. }
        | TargetSyntaxError::UnclosedRegexQuote { position }
        | TargetSyntaxError::RegexSuffix { position }
        | TargetSyntaxError::SingleTargetModifier { position }
        | TargetSyntaxError::SingleTargetDelimiter { position }
        | TargetSyntaxError::InvalidIdSelector { position }
        | TargetSyntaxError::IdSelectorOverflow { position }
        | TargetSyntaxError::OwnedByteLimit { position, .. } => Some(*position),
        _ => None,
    }
}

#[expect(
    clippy::collapsible_match,
    reason = "keep parser stages separate for typed safe-location review"
)]
fn syntax_position(error: &SyntaxError) -> Option<SourcePosition> {
    use edgion_waf::seclang::{ActionSyntaxError, DirectiveParseError, OperatorSyntaxError, TokenizeError};

    match error {
        SyntaxError::Tokenizer(error) => match error {
            TokenizeError::TokenLimit { position, .. }
            | TokenizeError::LineTokenLimit { position, .. }
            | TokenizeError::TokenByteLimit { position, .. }
            | TokenizeError::UnterminatedQuote { position, .. }
            | TokenizeError::DanglingEscape { position }
            | TokenizeError::CharactersAfterClosingQuote { position }
            | TokenizeError::OwnedByteLimit { position, .. } => Some(*position),
            _ => None,
        },
        SyntaxError::Directive(error) => match error {
            DirectiveParseError::DirectiveLimit { position, .. }
            | DirectiveParseError::MissingDirective { position, .. }
            | DirectiveParseError::QuotedDirectiveName { position, .. }
            | DirectiveParseError::UnknownDirective { position }
            | DirectiveParseError::Arity { position, .. }
            | DirectiveParseError::UpdateSelectorLimit { position, .. }
            | DirectiveParseError::QuoteStyle { position, .. }
            | DirectiveParseError::EmptyField { position, .. }
            | DirectiveParseError::OwnedByteLimit { position, .. } => Some(*position),
            _ => None,
        },
        SyntaxError::Target(error) => target_position(error),
        SyntaxError::Operator(error) => match error {
            OperatorSyntaxError::OperatorLimit { position, .. }
            | OperatorSyntaxError::OperatorEscapeLimit { position, .. }
            | OperatorSyntaxError::DocumentEscapeLimit { position, .. }
            | OperatorSyntaxError::DecodedByteLimit { position, .. }
            | OperatorSyntaxError::OwnedByteLimit { position, .. } => Some(*position),
            _ => None,
        },
        SyntaxError::Action(error) => match error {
            ActionSyntaxError::ListLimit { position, .. }
            | ActionSyntaxError::ScannedByteLimit { position, .. }
            | ActionSyntaxError::ListEntryLimit { position, .. }
            | ActionSyntaxError::DocumentEntryLimit { position, .. }
            | ActionSyntaxError::Empty { position, .. }
            | ActionSyntaxError::UnclosedQuote { position }
            | ActionSyntaxError::QuotedValueSuffix { position }
            | ActionSyntaxError::OwnedByteLimit { position, .. } => Some(*position),
            _ => None,
        },
        _ => None,
    }
}

#[expect(
    clippy::collapsible_match,
    reason = "keep frontend stages separate for typed safe-location review"
)]
fn attach_safe_location(error: &FrontendError, failure: &mut WafSupportFailure) {
    if failure.physical_line.is_none() {
        let position = match error {
            FrontendError::Source(error) => match error {
                SourceError::SourceByteLimit { position, .. }
                | SourceError::InvalidUtf8 { position }
                | SourceError::BareCarriageReturn { position }
                | SourceError::PhysicalLineLimit { position, .. }
                | SourceError::PhysicalLineByteLimit { position, .. }
                | SourceError::LogicalLineLimit { position, .. }
                | SourceError::LogicalLineByteLimit { position, .. }
                | SourceError::ProvenanceSegmentLimit { position, .. }
                | SourceError::OwnedByteLimit { position, .. }
                | SourceError::UnfinishedContinuation { position }
                | SourceError::InvalidScannerState { position } => Some(*position),
                _ => None,
            },
            FrontendError::Syntax(error) => syntax_position(error),
            FrontendError::Semantic(error) => semantic_position(error),
            FrontendError::Validation(error) => match error {
                SemanticValidationError::ReversedIdRange { position }
                | SemanticValidationError::UnresolvedIdSelector { position }
                | SemanticValidationError::ForwardIdSelector { position }
                | SemanticValidationError::RawMatchDirectiveLimit { position, .. }
                | SemanticValidationError::DistinctMatchDirectiveLimit { position, .. }
                | SemanticValidationError::RawMatchLimit { position, .. }
                | SemanticValidationError::DistinctMatchLimit { position, .. }
                | SemanticValidationError::SelectorWorkLimit { position, .. } => Some(*position),
                _ => None,
            },
            _ => None,
        };
        failure.physical_line = position.map(|position| position.line);
    }
    if failure.document_index.is_none() {
        let location = match error {
            FrontendError::Validation(error) => match error {
                SemanticValidationError::DuplicateRuleId { second, .. }
                | SemanticValidationError::MissingImplicitPhase { location: second }
                | SemanticValidationError::MissingPhaseDefaults { location: second, .. }
                | SemanticValidationError::InactiveUpdate { location: second }
                | SemanticValidationError::EmptyRemoval { location: second }
                | SemanticValidationError::SecActionTargetUpdate { location: second }
                | SemanticValidationError::FinalTargetLimit { location: second, .. }
                | SemanticValidationError::FinalTargetReferenceLimit { location: second, .. } => Some(*second),
                _ => None,
            },
            _ => None,
        };
        if let Some(location) = location {
            *failure = (*failure).at_directive(location);
        }
    }
    if failure.document_index.is_none() {
        let location = match error {
            FrontendError::Validation(error) => match error {
                SemanticValidationError::BlockStatusPass { location }
                | SemanticValidationError::CollectionPhase { location, .. }
                | SemanticValidationError::SelectorNotAllowed { location, .. }
                | SemanticValidationError::PathSelectorCollection { location }
                | SemanticValidationError::MissingExclusionBase { location }
                | SemanticValidationError::TargetComparisonLimit { location, .. }
                | SemanticValidationError::TargetComparisonByteLimit { location, .. }
                | SemanticValidationError::UnavailableCapability { location, .. } => Some(*location),
                _ => None,
            },
            _ => None,
        };
        if let Some(location) = location {
            *failure = (*failure).at_location(location);
        }
    }
}

fn frontend_failure(error: &FrontendError) -> WafSupportFailure {
    let mut failure = match error {
        FrontendError::Include(edgion_waf::seclang::IncludeError::Syntax { line }) => WafSupportFailure {
            physical_line: Some(*line),
            ..WafSupportFailure::new("syntax", "Correct the include directive syntax")
        },
        FrontendError::Include(
            edgion_waf::seclang::IncludeError::Source { .. }
            | edgion_waf::seclang::IncludeError::EmptyResolution { .. },
        ) => WafSupportFailure::new("missing_asset", "Inline WAF rules cannot load included sources"),
        FrontendError::Include(_) => WafSupportFailure::new("resource_limit", "Reduce or remove include directives"),
        FrontendError::Source(error) => match error {
            SourceError::InvalidUtf8 { position } | SourceError::BareCarriageReturn { position } => WafSupportFailure {
                physical_line: Some(position.line),
                ..WafSupportFailure::new("syntax", "Use UTF-8 source with LF or CRLF line endings")
            },
            SourceError::UnfinishedContinuation { position } => WafSupportFailure {
                physical_line: Some(position.line),
                ..WafSupportFailure::new("syntax", "Complete or remove the trailing continuation")
            },
            _ => WafSupportFailure::new("resource_limit", "Reduce rule source size or complexity"),
        },
        FrontendError::Syntax(error) if syntax_resource_limit(error) => {
            WafSupportFailure::new("resource_limit", "Reduce rule source size or syntax complexity")
        }
        FrontendError::Syntax(error) => match error {
            SyntaxError::Tokenizer(edgion_waf::seclang::TokenizeError::UnterminatedQuote { position, .. }) => {
                WafSupportFailure {
                    physical_line: Some(position.line),
                    ..WafSupportFailure::new("syntax", "Close the quoted token")
                }
            }
            SyntaxError::Tokenizer(edgion_waf::seclang::TokenizeError::DanglingEscape { position }) => {
                WafSupportFailure {
                    physical_line: Some(position.line),
                    ..WafSupportFailure::new("syntax", "Complete or remove the trailing escape")
                }
            }
            SyntaxError::Directive(edgion_waf::seclang::DirectiveParseError::UnknownDirective { position }) => {
                WafSupportFailure {
                    physical_line: Some(position.line),
                    ..WafSupportFailure::new("syntax", "Remove or replace the unsupported directive")
                }
            }
            _ => WafSupportFailure::new("syntax", "Correct SecLang syntax near the rejected directive"),
        },
        FrontendError::Semantic(SemanticError::FoldMissing {
            kind: FoldRequiredKind::Id,
            position,
        }) => WafSupportFailure {
            physical_line: Some(position.line),
            ..WafSupportFailure::new("unsupported_semantics", "Add an ID to the rule")
        },
        FrontendError::Semantic(error) if semantic_resource_limit(error) => {
            WafSupportFailure::new("resource_limit", "Reduce rule count or semantic complexity")
        }
        FrontendError::Semantic(_) => {
            WafSupportFailure::new("unsupported_semantics", "Revise unsupported or invalid rule semantics")
        }
        FrontendError::Compatibility(_) => {
            WafSupportFailure::new("resource_limit", "Reduce the number of compatibility findings")
        }
        FrontendError::Validation(SemanticValidationError::UnavailableCapability { location, .. }) => {
            WafSupportFailure::new(
                "unsupported_semantics",
                "Remove or replace the unsupported rule capability",
            )
            .at_location(*location)
        }
        FrontendError::Validation(
            SemanticValidationError::RuleUnitLimit { .. }
            | SemanticValidationError::IdSelectorLimit { .. }
            | SemanticValidationError::ExceptionLimit { .. }
            | SemanticValidationError::WorkingByteLimit { .. }
            | SemanticValidationError::RawMatchDirectiveLimit { .. }
            | SemanticValidationError::DistinctMatchDirectiveLimit { .. }
            | SemanticValidationError::RawMatchLimit { .. }
            | SemanticValidationError::DistinctMatchLimit { .. }
            | SemanticValidationError::SelectorWorkLimit { .. }
            | SemanticValidationError::FinalTargetLimit { .. }
            | SemanticValidationError::FinalTargetReferenceLimit { .. }
            | SemanticValidationError::TargetComparisonLimit { .. }
            | SemanticValidationError::TargetComparisonByteLimit { .. }
            | SemanticValidationError::OwnedByteLimit { .. }
            | SemanticValidationError::RetainedByteLimit { .. }
            | SemanticValidationError::Allocation { .. },
        ) => WafSupportFailure::new("resource_limit", "Reduce rule count or validation complexity"),
        FrontendError::Validation(_) => {
            WafSupportFailure::new("unsupported_semantics", "Revise rule validation errors")
        }
        FrontendError::Compiler(CompilerError::MissingAssetSource { location, .. }) => {
            WafSupportFailure::new("missing_asset", "Inline WAF rules cannot load external phrase assets")
                .at_location(*location)
        }
        FrontendError::Compiler(CompilerError::AssetSource { location, .. }) => {
            WafSupportFailure::new("missing_asset", "The external phrase asset is unavailable").at_location(*location)
        }
        FrontendError::Compiler(CompilerError::RegexProgram { location, error }) => {
            if matches!(
                error,
                RegexProgramError::Syntax | RegexProgramError::Engine | RegexProgramError::ProjectionInvariant
            ) {
                WafSupportFailure::new("unsupported_semantics", "Correct the rejected regex").at_location(*location)
            } else {
                WafSupportFailure::new("resource_limit", "Reduce regex size or complexity").at_location(*location)
            }
        }
        FrontendError::Compiler(CompilerError::CtlPhase { location }) => WafSupportFailure::new(
            "unsupported_semantics",
            "Move request-body control to the request headers phase",
        )
        .at_location(*location),
        FrontendError::Compiler(CompilerError::CtlXmlSelector { location }) => {
            WafSupportFailure::new("unsupported_semantics", "Use a supported XML control target selector")
                .at_location(*location)
        }
        FrontendError::Compiler(_) => WafSupportFailure::new("resource_limit", "Reduce rule compilation complexity"),
    };
    attach_safe_location(error, &mut failure);
    failure
}

impl IncludeSource for NoIncludes {
    fn load(&mut self, _request: IncludeRequest<'_>, _sink: &mut IncludeSink<'_>) -> Result<(), IncludeFailure> {
        Err(IncludeFailure::NotFound)
    }
}

impl WafConfig {
    fn validation_context() -> Result<SemanticValidationContext, WafCompileFailure> {
        let defaults = [
            Phase::RequestHeaders,
            Phase::RequestBody,
            Phase::ResponseHeaders,
            Phase::ResponseBody,
            Phase::Logging,
        ]
        .map(|phase| PhaseDefaults::pass(phase, true, true));
        SemanticValidationContext::try_new(Some(Phase::RequestBody), &defaults)
            .map_err(|error| WafCompileFailure::Config(format!("WAF defaults are invalid: {error}")))
    }

    fn compile_generation_detailed(&self) -> Result<CompiledGeneration, WafCompileFailure> {
        self.request_body
            .prefix_size_bytes()
            .map_err(WafCompileFailure::Config)?;
        if self.rules.trim().is_empty() {
            return Err(WafCompileFailure::Config("rules must not be empty".to_string()));
        }
        let generation = compile_with_includes(
            "inline-waf-rules",
            self.rules.as_bytes(),
            &mut NoIncludes,
            None,
            Self::validation_context()?,
            FrontendLimits::default(),
        )
        .map(|bundle| bundle.into_parts().0)
        .map_err(WafCompileFailure::Frontend)?;
        if self.request_body.inspection == WafRequestBodyInspection::Prefix {
            generation
                .validate_request_body_prefix()
                .map_err(WafCompileFailure::Prefix)?;
        }
        Ok(generation)
    }

    /// Inspect the exact inline WAF configuration without publishing it.
    /// This report contains no source text, operand, file identity, or raw error string.
    pub fn inspect_support(&self) -> WafSupportReport {
        let mut report = WafSupportReport {
            report_version: 1,
            analysis_complete: false,
            strict_validation_passed: false,
            rule_count: None,
            unavailable_rule_count: None,
            unattributed_findings: None,
            findings: Vec::new(),
            omitted_findings: 0,
            omitted_rule_summaries: 0,
            failure: None,
        };
        if self.request_body.prefix_size_bytes().is_err() {
            report.failure = Some(WafSupportFailure::new(
                "invalid_config",
                "Correct requestBody inspection and prefixSize",
            ));
            return report;
        }
        if self.rules.trim().is_empty() {
            report.failure = Some(WafSupportFailure::new("invalid_config", "Provide nonempty WAF rules"));
            return report;
        }
        let limits = FrontendLimits {
            compatibility: CompatibilityLimits {
                max_findings: 128,
                max_rule_summaries: 128,
            },
            ..FrontendLimits::default()
        };
        match inspect_compatibility("inline-waf-rules", self.rules.as_bytes(), &mut NoIncludes, limits) {
            Ok(compatibility) => {
                report.analysis_complete = true;
                report.rule_count = Some(compatibility.rule_count());
                report.unavailable_rule_count = Some(compatibility.unavailable_rule_unit_count());
                report.unattributed_findings = Some(compatibility.unattributed_findings());
                report.omitted_findings = compatibility.omitted_findings();
                report.omitted_rule_summaries = compatibility.omitted_rule_summaries();
                report.findings = support_findings(&compatibility);
            }
            Err(error) => {
                report.failure = Some(frontend_failure(&error));
                return report;
            }
        }
        match self.compile_generation_detailed() {
            Ok(_) => report.strict_validation_passed = true,
            Err(WafCompileFailure::Config(_)) => {
                report.failure = Some(WafSupportFailure::new("invalid_config", "Correct WAF configuration"))
            }
            Err(WafCompileFailure::Frontend(error)) => report.failure = Some(frontend_failure(&error)),
            Err(WafCompileFailure::Prefix(_)) => {
                report.failure = Some(WafSupportFailure::new(
                    "unsupported_semantics",
                    "Use full request-body inspection for these rules",
                ));
            }
        }
        report
    }

    /// Compile the exact strict generation used by both validation and runtime.
    pub fn compile_generation(&self) -> Result<CompiledGeneration, String> {
        self.compile_generation_detailed().map_err(|error| match error {
            WafCompileFailure::Config(message) => message,
            WafCompileFailure::Frontend(error) => format!("WAF rules are not executable: {error}"),
            WafCompileFailure::Prefix(message) => message.to_string(),
        })
    }
}

fn support_findings(compatibility: &CompatibilityReport) -> Vec<WafSupportFinding> {
    compatibility
        .findings()
        .map(|finding| {
            let (capability, reason) = capability_name(finding.capability());
            WafSupportFinding {
                rule_id: finding.rule_id(),
                document_index: finding.location().document_index(),
                directive_index: finding.location().directive_index(),
                capability,
                reason,
            }
        })
        .collect()
}

impl PluginValidation for WafConfig {
    fn validation_error(&self) -> Option<String> {
        self.compile_generation().err()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_report_separates_inventory_from_strict_validation_and_redacts_operands() {
        let config = WafConfig {
            rules: concat!(
                "SecRule REQUEST_URI \"@contains private-token\" \"id:1,phase:1,pass\"\n",
                "SecRule RESPONSE_BODY \"@rx secret-pattern\" \"id:2,phase:4,pass\"\n",
            )
            .to_string(),
            ..WafConfig::default()
        };
        let report = config.inspect_support();
        assert!(report.analysis_complete, "{report:?}");
        assert!(!report.strict_validation_passed);
        assert_eq!(report.rule_count, Some(2));
        assert_eq!(report.unavailable_rule_count, Some(1));
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].rule_id, Some(2));
        assert_eq!(report.findings[0].capability, "full_response_body");
        let rendered = serde_json::to_string(&report).unwrap();
        assert!(!rendered.contains("private-token"));
        assert!(!rendered.contains("secret-pattern"));
        assert!(!format!("{report:?}").contains("secret-pattern"));
    }

    #[test]
    fn support_report_classifies_incomplete_syntax_and_missing_assets() {
        let syntax = WafConfig {
            rules: "SecRule REQUEST_URI \"@contains secret".to_string(),
            ..WafConfig::default()
        }
        .inspect_support();
        assert!(!syntax.analysis_complete);
        assert!(!syntax.strict_validation_passed);
        assert_eq!(syntax.failure.unwrap().category, "syntax");
        assert_eq!(syntax.failure.unwrap().physical_line, Some(1));

        let asset = WafConfig {
            rules: "SecRule ARGS \"@pmFromFile private.data\" \"id:1,phase:2,pass\"\n".to_string(),
            ..WafConfig::default()
        }
        .inspect_support();
        assert!(asset.analysis_complete);
        assert!(!asset.strict_validation_passed);
        assert_eq!(asset.failure.unwrap().category, "missing_asset");
        assert_eq!(asset.failure.unwrap().directive_index, Some(0));
        assert!(!serde_json::to_string(&asset).unwrap().contains("private.data"));
    }

    #[test]
    fn support_report_bounds_findings_and_marks_omissions() {
        let rules = (1..=130)
            .map(|id| format!("SecRule RESPONSE_BODY \"@contains secret\" \"id:{id},phase:4,pass\"\n"))
            .collect::<String>();
        let report = WafConfig {
            rules,
            ..WafConfig::default()
        }
        .inspect_support();
        assert!(report.analysis_complete);
        assert_eq!(report.findings.len(), 128);
        assert_eq!(report.omitted_findings, 2);
        assert_eq!(report.omitted_rule_summaries, 2);
        assert!(!report.strict_validation_passed);
    }

    #[test]
    fn support_report_locates_missing_rule_id_without_fabricating_one() {
        let report = WafConfig {
            rules: "SecRule RESPONSE_BODY \"@contains secret\" \"phase:4,pass\"\n".to_string(),
            ..WafConfig::default()
        }
        .inspect_support();
        assert!(!report.analysis_complete);
        assert!(report.findings.is_empty());
        assert_eq!(report.failure.unwrap().physical_line, Some(1));
        assert_eq!(report.failure.unwrap().reason, "Add an ID to the rule");
        assert!(!report.strict_validation_passed);
    }

    #[test]
    fn support_report_preserves_available_locations_and_resource_categories() {
        let position = SourcePosition {
            line: 7,
            column: 1,
            byte_offset: 0,
        };
        let chain_limit = frontend_failure(&FrontendError::Semantic(SemanticError::ChainLengthLimit {
            limit: 2,
            position,
        }));
        assert_eq!(chain_limit.category, "resource_limit");
        assert_eq!(chain_limit.physical_line, Some(7));
        let work_limit = frontend_failure(&FrontendError::Validation(SemanticValidationError::WorkingByteLimit {
            limit: 1,
        }));
        assert_eq!(work_limit.category, "resource_limit");
        let ctl_target = frontend_failure(&FrontendError::Semantic(SemanticError::CtlTarget(
            edgion_waf::seclang::TargetSyntaxError::AtomLimit { limit: 1, position },
        )));
        assert_eq!(ctl_target.category, "resource_limit");
        assert_eq!(ctl_target.physical_line, Some(7));

        let actions = std::iter::repeat_n("t:none", 1_025).collect::<Vec<_>>().join(",");
        let action_limit = WafConfig {
            rules: format!("SecAction \"id:1,phase:1,pass,{actions}\""),
            ..WafConfig::default()
        }
        .inspect_support();
        assert_eq!(action_limit.failure.unwrap().category, "resource_limit");
        assert_eq!(action_limit.failure.unwrap().physical_line, Some(1));

        let unknown_operator = WafConfig {
            rules: "SecRule REQUEST_URI \"@notAnOperator secret\" \"id:1,phase:1,pass\"".to_string(),
            ..WafConfig::default()
        }
        .inspect_support();
        assert!(!unknown_operator.analysis_complete);
        assert_eq!(unknown_operator.failure.unwrap().physical_line, Some(1));

        let duplicate_id = WafConfig {
            rules: concat!(
                "SecRule REQUEST_URI \"@contains first\" \"id:1,phase:1,pass\"\n",
                "SecRule REQUEST_URI \"@contains second\" \"id:1,phase:1,pass\"\n",
            )
            .to_string(),
            ..WafConfig::default()
        }
        .inspect_support();
        assert!(duplicate_id.analysis_complete);
        let failure = duplicate_id.failure.unwrap();
        assert_eq!(failure.document_index, Some(0));
        assert_eq!(failure.directive_index, Some(1));

        let missing_fields = WafConfig {
            rules: "SecRule REQUEST_URI".to_string(),
            ..WafConfig::default()
        }
        .inspect_support();
        assert_eq!(missing_fields.failure.unwrap().physical_line, Some(1));
    }

    #[test]
    fn prefix_size_is_bounded_and_round_trips_without_changing_full_defaults() {
        for (size, bytes) in [("1", 1), ("16KiB", 16384), ("100k", 102400), ("1m", 1048576)] {
            let config: WafConfig = serde_json::from_value(serde_json::json!({
                "rules": "SecRule REQUEST_BODY \"@contains forbidden\" \"id:1,phase:2,deny\"",
                "requestBody": {"prefixSize": size}
            }))
            .unwrap();
            assert_eq!(config.request_body.prefix_size_bytes().unwrap(), bytes);
            assert_eq!(config.validation_error(), None);
            let encoded = serde_json::to_value(&config).unwrap();
            assert_eq!(encoded["requestBody"]["prefixSize"], size);
            let restored: WafConfig = serde_json::from_value(encoded).unwrap();
            assert_eq!(restored.request_body.prefix_size_bytes().unwrap(), bytes);
        }
        for size in ["0", "1048577", "2m", "-1", "invalid"] {
            let config = WafConfig {
                rules: "SecAction \"id:1,phase:1,pass\"".to_string(),
                request_body: WafRequestBodyConfig {
                    prefix_size: Some(size.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            };
            assert!(config.validation_error().is_some(), "{size}");
        }
        let full = WafRequestBodyConfig {
            inspection: WafRequestBodyInspection::Full,
            ..Default::default()
        };
        assert!(full.prefix_size_bytes().is_ok());
        assert!(WafRequestBodyConfig {
            prefix_size: Some("1m".to_string()),
            ..full
        }
        .prefix_size_bytes()
        .is_err());
    }

    #[test]
    fn prefix_defaults_and_full_validation_share_the_same_compiler() {
        let mut config: WafConfig = serde_json::from_value(serde_json::json!({
            "rules": "SecRule REQUEST_BODY \"@contains forbidden\" \"id:1,phase:2,deny\""
        }))
        .unwrap();
        assert_eq!(config.request_body.inspection, WafRequestBodyInspection::Prefix);
        assert_eq!(config.validation_error(), None);
        config.request_body.inspection = WafRequestBodyInspection::Full;
        let serialized = serde_json::to_value(&config).unwrap();
        assert_eq!(serialized["requestBody"]["inspection"], "full");
        let restored: WafConfig = serde_json::from_value(serialized).unwrap();
        assert_eq!(restored.request_body.inspection, WafRequestBodyInspection::Full);
        assert_eq!(restored.validation_error(), None);
    }

    #[test]
    fn full_capture_dependency_includes_later_phases_but_not_query_only_headers() {
        for (rules, requires_body) in [
            ("SecRule QUERY_STRING \"@contains query\" \"id:1,phase:1,deny\"", false),
            (
                "SecRule REQUEST_HEADERS:X-Test \"@contains test\" \"id:1,phase:2,deny\"",
                false,
            ),
            ("SecRule REQUEST_BODY \"@contains test\" \"id:1,phase:3,deny\"", true),
            ("SecRule ARGS:item \"@contains test\" \"id:1,phase:5,pass\"", true),
            ("SecRule RESPONSE_STATUS \"@eq 200\" \"id:1,phase:3,pass\"", false),
            (
                "SecRule REQUEST_HEADERS:X-Test \"@streq %{ARGS.item}\" \"id:1,phase:3,pass\"",
                true,
            ),
            ("SecAction \"id:1,phase:3,pass,setvar:tx.item=%{ARGS.item}\"", true),
            (
                "SecRule REQUEST_HEADERS:X-Test \"@streq %{REQUEST_HEADERS.X-Other}\" \"id:1,phase:3,pass\"",
                false,
            ),
        ] {
            let config = WafConfig {
                rules: rules.to_owned(),
                request_body: WafRequestBodyConfig {
                    inspection: WafRequestBodyInspection::Full,
                    ..Default::default()
                },
                ..WafConfig::default()
            };
            let generation = config.compile_generation().unwrap();
            assert_eq!(generation.requires_complete_request_body(), requires_body, "{rules}");
        }
    }

    #[test]
    fn prefix_accepts_body_independent_phase_two_rules() {
        for target in [
            "REQUEST_HEADERS:X-Test",
            "REQUEST_URI",
            "ARGS_GET:item",
            "ARGS_GET_NAMES",
        ] {
            let config = WafConfig {
                rules: format!("SecRule {target} \"!@contains blocked\" \"id:1,phase:2,deny\""),
                ..Default::default()
            };
            let generation = config.compile_generation().unwrap();
            assert!(!generation.requires_complete_request_body(), "{target}");
            assert!(
                !generation.request_body_execution_summary().requires_request_body(),
                "{target}"
            );
            assert_eq!(
                generation.request_body_execution_summary().streaming_raw_rule_units(),
                1
            );
        }
    }

    #[test]
    fn prefix_rejects_whole_body_and_control_dependencies_across_generation() {
        for rules in [
            "SecRule REQUEST_BODY \"!@contains forbidden\" \"id:1,phase:2,deny\"",
            "SecRule REQUEST_BODY \"@endsWith forbidden\" \"id:1,phase:2,deny\"",
            "SecRule ARGS:item \"@contains forbidden\" \"id:1,phase:2,deny\"",
            "SecRule ARGS_NAMES \"@contains forbidden\" \"id:1,phase:2,deny\"",
            "SecRule REQBODY_PROCESSOR \"@streq JSON\" \"id:1,phase:2,deny\"",
            "SecRule REQUEST_BODY_LENGTH \"@gt 10\" \"id:1,phase:2,deny\"",
            "SecRule REQUEST_BODY \"@contains forbidden\" \"id:1,phase:2,deny,t:base64Decode\"",
            "SecRule REQUEST_BODY \"@contains forbidden\" \"id:1,phase:3,deny\"",
            "SecRule ARGS:item \"@contains forbidden\" \"id:1,phase:5,pass\"",
            "SecAction \"id:1,phase:1,pass,setvar:tx.flag=1\"",
            "SecRule REQUEST_URI \"@contains private\" \"id:1,phase:1,pass,skip:1\"\nSecRule REQUEST_BODY \"@contains forbidden\" \"id:2,phase:2,deny\"",
            "SecRule REQUEST_BODY \"@contains forbidden\" \"id:1,phase:2,deny,chain\"\nSecRule REQUEST_BODY \"@contains secret\" \"t:none\"",
        ] {
            let mut config = WafConfig { rules: rules.to_owned(), ..WafConfig::default() };
            let error = config.validation_error().expect("prefix must reject unsupported semantics");
            assert!(error.contains("inspection prefix"), "{error}");
            config.request_body.inspection = WafRequestBodyInspection::Full;
            assert_eq!(config.validation_error(), None, "full fixture must remain executable: {rules}");
        }
    }

    #[test]
    fn blank_rules_are_rejected_by_the_shared_compile_entry() {
        let config = WafConfig {
            rules: " \n\t".to_string(),
            mode: WafMode::On,
            request_body: WafRequestBodyConfig::default(),
            priority: 0,
        };

        assert_eq!(
            config.compile_generation().err().as_deref(),
            Some("rules must not be empty")
        );
        assert_eq!(config.validation_error().as_deref(), Some("rules must not be empty"));
    }

    #[test]
    fn validation_compiles_supported_rules_without_a_separate_prepare_call() {
        let valid = WafConfig {
            rules: "SecRule REQUEST_URI \"@contains private\" \"id:1,phase:1,deny\"\n".to_string(),
            mode: WafMode::On,
            request_body: WafRequestBodyConfig::default(),
            priority: 0,
        };
        assert_eq!(valid.validation_error(), None);

        let invalid = WafConfig {
            rules: "SecRule RESPONSE_BODY \"@contains private\" \"id:2,phase:4,deny\"\n".to_string(),
            mode: WafMode::DetectionOnly,
            request_body: WafRequestBodyConfig::default(),
            priority: 0,
        };
        assert!(invalid.validation_error().is_some());
    }

    #[test]
    fn terminal_waf_status_matches_the_final_reply_contract() {
        for status in [100, 101, 199, 200, 599, 600] {
            let config = WafConfig {
                rules: format!("SecRule REQUEST_URI \"@contains private\" \"id:1,phase:1,deny,status:{status}\"\n"),
                mode: WafMode::On,
                request_body: WafRequestBodyConfig::default(),
                priority: 0,
            };
            assert_eq!(
                config.validation_error().is_none(),
                crate::http_status::is_valid_final_http_status(status),
                "status {status}"
            );
        }
    }

    #[test]
    fn validation_errors_do_not_render_rule_content() {
        let config = WafConfig {
            rules: "SecRule private-(".to_string(),
            mode: WafMode::On,
            request_body: WafRequestBodyConfig::default(),
            priority: 0,
        };
        let error = config.validation_error().expect("invalid source must be rejected");
        assert!(!error.contains("private"));
    }

    #[test]
    fn rules_round_trip_for_transport_but_redact_from_admin_and_debug_output() {
        let config = WafConfig {
            rules: "SecAction \"id:1,phase:1,pass,msg:'private'\"".to_string(),
            mode: WafMode::On,
            request_body: WafRequestBodyConfig::default(),
            priority: 17,
        };

        let transport = serde_json::to_value(&config).expect("transport serialization succeeds");
        assert!(transport["rules"]
            .as_str()
            .is_some_and(|rules| rules.contains("private")));

        let admin = {
            let _guard = crate::redact::AdminRedactGuard::new();
            serde_json::to_value(&config).expect("admin serialization succeeds")
        };
        assert_eq!(admin["rules"], "[redacted]");
        assert_eq!(admin["priority"], 17);
        assert!(!format!("{config:?}").contains("private"));
    }

    #[test]
    fn priority_defaults_to_zero_and_rejects_out_of_range_values() {
        let base = r#"{"rules":"SecAction \"id:1,phase:1,pass\""}"#;
        let parsed: WafConfig = serde_json::from_str(base).expect("missing priority uses its default");
        assert_eq!(parsed.priority, 0);

        let maximum: WafConfig =
            serde_json::from_str(r#"{"rules":"SecAction \"id:1,phase:1,pass\"","priority":65535}"#)
                .expect("u16 maximum is accepted");
        assert_eq!(maximum.priority, u16::MAX);
        assert!(
            serde_json::from_str::<WafConfig>(r#"{"rules":"SecAction \"id:1,phase:1,pass\"","priority":-1}"#).is_err()
        );
        assert!(
            serde_json::from_str::<WafConfig>(r#"{"rules":"SecAction \"id:1,phase:1,pass\"","priority":65536}"#)
                .is_err()
        );
    }

    #[test]
    fn dynamic_integration_generations_compile_with_the_production_profile() {
        let generations = [
            concat!(
                "SecRule REQUEST_HEADERS:X-Waf-Version \"!@streq v1\" ",
                "\"id:1001,phase:1,deny,status:403\"\n",
                "SecRule REQUEST_BODY \"@contains blocked\" ",
                "\"id:1002,phase:2,deny,status:406\"\n"
            ),
            "SecRule REQUEST_HEADERS:X-Waf-Version \"!@streq v2\" \"id:2001,phase:1,deny,status:451\"\n",
            "SecRule REQUEST_HEADERS:X-Waf-Version \"!@streq v3\" \"id:3001,phase:1,deny,status:429\"\n",
        ];

        for rules in generations {
            let config = WafConfig {
                rules: rules.to_string(),
                mode: WafMode::On,
                request_body: WafRequestBodyConfig::default(),
                priority: 0,
            };
            assert_eq!(config.validation_error(), None, "fixture must compile: {rules}");
        }
    }
}
