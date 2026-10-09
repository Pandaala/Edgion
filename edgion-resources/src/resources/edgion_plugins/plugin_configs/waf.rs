//! Native WAF logical-plugin configuration.

use crate::resources::refs::waf_fields::validated_bundle_target;
use edgion_waf::matcher::RegexProgramError;
use edgion_waf::seclang::{
    compile_rule_bundles_with_external_budget, compile_rule_bundles_with_selection, compile_with_includes,
    inspect_compatibility, CompatibilityLimits, CompatibilityReport, CompiledGeneration, CompilerError,
    DirectiveLocation, FoldRequiredKind, FrontendError, FrontendLimits, IncludeFailure, IncludeRequest, IncludeSink,
    IncludeSource, JsonInspectionLimits, JsonInspectionUnsupported, JsonResultLimits, JsonStreamLimits,
    JsonStructureLimits, Phase, PhaseDefaults, RuleBundleMember, RuleBundleSource, SemanticError, SemanticLocation,
    SemanticValidationContext, SemanticValidationError, SourceError, SourcePosition, SyntaxError,
    UnavailableCapability, WafSelectionCompileError, XmlInspectionUnsupported, DEFAULT_REQUEST_BODY_PREFIX_BYTES,
    MAX_REQUEST_BODY_PREFIX_BYTES,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::edgion_config_data::{
    EdgionConfigDataRef, PreparedWafPolicySelections, WafPolicyData, WafRuleBundleData, WafSelectionSummary,
};
pub use crate::resources::edgion_config_data::{WafBundleRef, WafProfile};
use crate::resources::edgion_plugins::PluginValidation;

/// Controller-resolved, uncompiled bundle transported to the Gateway.
#[derive(Clone, Serialize, Deserialize)]
pub struct ResolvedWafBundle {
    pub identity: String,
    pub digest: String,
    pub data: WafRuleBundleData,
}

/// Shallow source view used for Controller admission trials.
#[derive(Clone, Copy)]
pub struct WafResolvedBundleRef<'a> {
    pub identity: &'a str,
    pub digest: &'a str,
    pub data: &'a WafRuleBundleData,
}

pub struct WafCompilation {
    pub generation: CompiledGeneration,
    pub selection_summary: Option<WafSelectionSummary>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum WafBundleResolutionKind {
    Missing,
    WrongType,
    Disabled,
    Denied,
    Invalid,
    DigestMismatch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WafBundleResolutionError {
    pub profile: String,
    pub index: usize,
    pub namespace: String,
    pub name: String,
    pub kind: WafBundleResolutionKind,
}

impl std::fmt::Debug for ResolvedWafBundle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResolvedWafBundle")
            .field("identity", &self.identity)
            .field("digest", &self.digest)
            .field("data", &"[redacted]")
            .finish()
    }
}

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

/// Explicit independent JSON lexical budgets. Zero remains restrictive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WafJsonLexicalLimits {
    pub max_source_bytes: usize,
    pub max_raw_token_bytes: usize,
    pub max_decoded_bytes: usize,
    pub max_events: usize,
    pub max_depth: usize,
    pub max_retained_bytes: usize,
    pub max_work: usize,
}

/// Explicit independent JSON structure budgets. Zero remains restrictive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WafJsonStructureLimits {
    pub max_retained_bytes: usize,
    pub max_key_bytes: usize,
    pub max_members: usize,
    pub max_path_bytes: usize,
    pub max_depth: usize,
    pub max_work: usize,
}

/// Explicit independent JSON result budgets. Zero remains restrictive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WafJsonResultLimits {
    pub max_candidates: usize,
    /// Allocated candidate/profile/change/outcome/capture slots, including spare and free slots.
    pub max_facts: usize,
    pub max_retained_bytes: usize,
    pub max_work: usize,
    pub max_deferred_entries: usize,
    pub max_deferred_bytes: usize,
}

/// Full-inspection overrides; omission selects the deterministic engine defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WafJsonConfig {
    pub lexical: WafJsonLexicalLimits,
    pub structure: WafJsonStructureLimits,
    pub results: WafJsonResultLimits,
}
impl WafJsonConfig {
    /// Copy explicit budgets without normalizing restrictive zero values.
    pub fn inspection_limits(&self) -> JsonInspectionLimits {
        JsonInspectionLimits {
            lexical: JsonStreamLimits {
                max_source_bytes: self.lexical.max_source_bytes,
                max_raw_token_bytes: self.lexical.max_raw_token_bytes,
                max_decoded_bytes: self.lexical.max_decoded_bytes,
                max_events: self.lexical.max_events,
                max_depth: self.lexical.max_depth,
                max_retained_bytes: self.lexical.max_retained_bytes,
                max_work: self.lexical.max_work,
            },
            structure: JsonStructureLimits {
                max_retained_bytes: self.structure.max_retained_bytes,
                max_key_bytes: self.structure.max_key_bytes,
                max_members: self.structure.max_members,
                max_path_bytes: self.structure.max_path_bytes,
                max_depth: self.structure.max_depth,
                max_work: self.structure.max_work,
            },
            results: JsonResultLimits {
                candidates: self.results.max_candidates,
                records: self.results.max_facts,
                retained_bytes: self.results.max_retained_bytes,
                work: self.results.max_work,
                deferred_entries: self.results.max_deferred_entries,
                deferred_bytes: self.results.max_deferred_bytes,
            },
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WafRequestBodyConfig {
    #[serde(default)]
    pub inspection: WafRequestBodyInspection,
    /// Prefix window, default 64 KiB, at most 1 MiB. Only valid in prefix mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix_size: Option<String>,
    /// JSON checker limit overrides; full mode uses engine defaults when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json: Option<WafJsonConfig>,
}

impl WafRequestBodyConfig {
    /// Resolve once during configuration validation and plugin construction.
    pub fn prefix_size_bytes(&self) -> Result<usize, String> {
        if self.json.is_some() && self.inspection != WafRequestBodyInspection::Full {
            return Err("requestBody.json is only valid with full inspection".to_string());
        }
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
    /// Local inspection source only; excluded from operator and transport data.
    #[serde(skip)]
    #[schemars(skip)]
    pub rules: String,

    /// Fixed reference to the shared WafPolicy EdgionConfigData.
    pub policy_ref: EdgionConfigDataRef,

    /// Optional fixed profile name selected by this plugin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_profile: Option<String>,

    /// Per-plugin selector for one of the shared policy's named profiles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_profile_ref: Option<EdgionConfigDataRef>,

    /// Controller-captured policy whose selected inputs are transported below.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_policy: Option<WafPolicyData>,

    /// Numeric result of the final accepted Controller trial, not an admission certificate.
    #[serde(skip)]
    #[schemars(skip)]
    pub controller_selection_summary: Option<WafSelectionSummary>,

    /// Controller-selected profile for the current publication.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub selected_profile: Option<String>,

    /// Source indices aligned with resolved_bundles, preserving declaration order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(skip)]
    pub resolved_ref_indices: Vec<usize>,

    /// Complete uncompiled bundle contents, resolved by the Controller.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_bundles: Option<Vec<ResolvedWafBundle>>,

    /// Controller-derived reference errors; never supplied by operators.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(skip)]
    pub resolution_errors: Vec<WafBundleResolutionError>,

    /// Safe status-only reasons for skipped optional bundles or profiles.
    #[serde(skip)]
    #[schemars(skip)]
    pub resolution_warnings: Vec<String>,

    /// Policy availability captured with the source snapshot for Controller status.
    #[serde(skip)]
    #[schemars(skip)]
    pub policy_resolved: Option<bool>,

    /// Selector availability captured with the bundles for Controller status.
    #[serde(skip)]
    #[schemars(skip)]
    pub selector_resolved: Option<bool>,

    /// Policy namespace used by the Controller to resolve implicit bundle namespaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub owner_namespace: Option<String>,

    /// Whether disruptive actions are enforced.
    #[serde(default)]
    pub mode: WafMode,

    /// Prefix inspection uses a bounded memory window; full uses the shared body cache.
    #[serde(default)]
    pub request_body: WafRequestBodyConfig,

    /// Request-wide winner priority when multiple scopes attach a WAF.
    /// Route wins over Gateway, which wins over Global. Higher values win
    /// only among candidates at the same scope.
    #[serde(default)]
    pub priority: u16,
}

impl std::fmt::Debug for WafConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WafConfig")
            .field("rules", &"[redacted]")
            .field("policy_ref", &self.policy_ref)
            .field("active_profile", &self.active_profile)
            .field("active_profile_ref", &self.active_profile_ref)
            .field("resolved_policy", &self.resolved_policy)
            .field("selected_profile", &self.selected_profile)
            .field("resolved_ref_indices", &self.resolved_ref_indices)
            .field("resolved_bundles", &self.resolved_bundles.as_ref().map(Vec::len))
            .field("resolution_errors", &self.resolution_errors)
            .field("resolution_warnings", &self.resolution_warnings)
            .field("policy_resolved", &self.policy_resolved)
            .field("selector_resolved", &self.selector_resolved)
            .field("owner_namespace", &self.owner_namespace)
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
    Json(JsonInspectionUnsupported),
    Xml(XmlInspectionUnsupported),
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
        FrontendError::RuleBundle(_) => {
            WafSupportFailure::new("invalid_bundle", "Correct the selected WAF rule bundle")
        }
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
    /// Explicit local metadata surface, separate from value-free support inspection.
    pub fn inspect_local_bundles<'a>(
        resources: &[crate::resources::edgion_config_data::EdgionConfigData],
        selection: &'a crate::resources::edgion_config_data::WafRuleSelection,
        query: crate::resources::edgion_config_data::WafTagQuery<'_>,
        offset: usize,
    ) -> Result<crate::resources::edgion_config_data::WafTagReport<'a>, String> {
        Ok(crate::resources::edgion_config_data::inspect_local_waf_tags(
            resources,
            selection,
            query,
            offset,
            Self::validation_context().map_err(Self::compile_failure_message)?,
        ))
    }
    /// Validate the operator-authored shape before any bundle reference is resolved.
    pub fn validate_shape(&self) -> Result<(), String> {
        self.request_body.prefix_size_bytes()?;
        if self.policy_ref.name.is_empty() || self.policy_ref.name.contains('/') {
            return Err("policyRef requires a valid name".to_string());
        }
        if self
            .active_profile_ref
            .as_ref()
            .is_some_and(|reference| reference.name.is_empty() || reference.name.contains('/'))
        {
            return Err("activeProfileRef requires a valid name".to_string());
        }
        if let Some(name) = self.active_profile.as_deref() {
            if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
                return Err("activeProfile requires a valid profile name".to_string());
            }
            if self.active_profile_ref.is_some() {
                return Err("activeProfile and activeProfileRef are mutually exclusive".to_string());
            }
        }
        Ok(())
    }

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

    fn validate_body_generation(&self, generation: &CompiledGeneration) -> Result<(), WafCompileFailure> {
        if self.request_body.inspection == WafRequestBodyInspection::Prefix {
            generation
                .validate_request_body_prefix()
                .map_err(WafCompileFailure::Prefix)?;
        } else if let Some(finding) = generation.json_inspection_unsupported().first() {
            return Err(WafCompileFailure::Json(*finding));
        } else if let Some(finding) = generation.xml_inspection_unsupported().first() {
            return Err(WafCompileFailure::Xml(*finding));
        }
        Ok(())
    }

    fn compile_generation_detailed(&self) -> Result<CompiledGeneration, WafCompileFailure> {
        // Local source inspection and in-process test fixtures use this path.
        // `rules` is skipped by serde and is unavailable to operator resources.
        if self.resolved_policy.is_none() && !self.rules.trim().is_empty() {
            self.request_body
                .prefix_size_bytes()
                .map_err(WafCompileFailure::Config)?;
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
            self.validate_body_generation(&generation)?;
            return Ok(generation);
        }
        self.validate_shape().map_err(WafCompileFailure::Config)?;
        if self.policy_ref.ref_denied.is_some() {
            return Err(WafCompileFailure::Config("policyRef is not permitted".to_string()));
        }
        let policy = self
            .resolved_policy
            .as_ref()
            .ok_or_else(|| WafCompileFailure::Config("policyRef has not been resolved".to_string()))?;
        let selected = self
            .selected_profile
            .as_deref()
            .ok_or_else(|| WafCompileFailure::Config("selected WAF profile has not been resolved".to_string()))?;
        let namespace = self
            .owner_namespace
            .as_deref()
            .ok_or_else(|| WafCompileFailure::Config("bundleRefs owner namespace is missing".to_string()))?;
        let resolved = self
            .resolved_bundles
            .as_ref()
            .ok_or_else(|| WafCompileFailure::Config("bundleRefs have not been resolved".to_string()))?;
        let prepared = policy.prepare_selections().map_err(WafCompileFailure::Config)?;
        let bundles: Vec<_> = resolved
            .iter()
            .map(|bundle| WafResolvedBundleRef {
                identity: &bundle.identity,
                digest: &bundle.digest,
                data: &bundle.data,
            })
            .collect();
        self.compile_resolved_candidate_detailed(
            policy,
            selected,
            namespace,
            &self.resolved_ref_indices,
            &bundles,
            &prepared,
        )
        .map(|result| result.generation)
    }

    /// Admit a borrowed aggregate through the validation also used by Gateway compilation.
    #[allow(clippy::too_many_arguments)]
    pub fn compile_resolved_candidate(
        &self,
        policy: &WafPolicyData,
        selected: &str,
        owner_namespace: &str,
        indices: &[usize],
        resolved: &[WafResolvedBundleRef<'_>],
        selections: &PreparedWafPolicySelections,
    ) -> Result<WafCompilation, String> {
        self.compile_resolved_candidate_detailed(policy, selected, owner_namespace, indices, resolved, selections)
            .map_err(Self::compile_failure_message)
    }

    #[allow(clippy::too_many_arguments)]
    fn compile_resolved_candidate_detailed(
        &self,
        policy: &WafPolicyData,
        selected: &str,
        owner_namespace: &str,
        indices: &[usize],
        resolved: &[WafResolvedBundleRef<'_>],
        selections: &PreparedWafPolicySelections,
    ) -> Result<WafCompilation, WafCompileFailure> {
        self.validate_shape().map_err(WafCompileFailure::Config)?;
        if self.policy_ref.ref_denied.is_some() {
            return Err(WafCompileFailure::Config("policyRef is not permitted".to_string()));
        }
        policy.validate_shape().map_err(WafCompileFailure::Config)?;
        selections.validate_binding(policy).map_err(WafCompileFailure::Config)?;
        if self
            .active_profile
            .as_ref()
            .is_some_and(|name| !policy.profiles.contains_key(name))
        {
            return Err(WafCompileFailure::Config(
                "activeProfile is not a configured WAF profile".to_string(),
            ));
        }
        if !self.resolution_errors.is_empty() {
            return Err(WafCompileFailure::Config(
                "required bundleRefs resolution failed".to_string(),
            ));
        }
        let profile = policy
            .profiles
            .get(selected)
            .ok_or_else(|| WafCompileFailure::Config("selected WAF profile is unknown".to_string()))?;
        if resolved.len() != indices.len() {
            return Err(WafCompileFailure::Config(
                "bundleRefs resolution is incomplete".to_string(),
            ));
        }
        let mut seen = std::collections::HashSet::new();
        let mut previous_index = None;
        for (index, bundle) in indices.iter().copied().zip(resolved) {
            if !seen.insert(index) || previous_index.is_some_and(|previous| index <= previous) {
                return Err(WafCompileFailure::Config(
                    "bundleRefs contains a duplicate index".to_string(),
                ));
            }
            previous_index = Some(index);
            let reference = profile
                .bundle_refs
                .get(index)
                .ok_or_else(|| WafCompileFailure::Config("bundleRefs contains an invalid index".to_string()))?;
            let target = validated_bundle_target(reference, owner_namespace)
                .map_err(|_| WafCompileFailure::Config("bundleRefs contain an invalid target".to_string()))?;
            let namespace = target.scope().namespace().expect("WAF bundles are namespaced");
            let expected_identity = format!("{namespace}/{}", target.name());
            if reference.target.ref_denied.is_some()
                || bundle.identity != expected_identity
                || reference
                    .expected_digest
                    .as_deref()
                    .is_some_and(|digest| digest != bundle.digest)
                || bundle.digest != bundle.data.compute_digest()
                || bundle.data.validate().is_err()
            {
                return Err(WafCompileFailure::Config(
                    "bundleRefs contain an invalid bundle".to_string(),
                ));
            }
        }
        if profile
            .bundle_refs
            .iter()
            .enumerate()
            .any(|(index, reference)| !reference.optional && !seen.contains(&index))
        {
            return Err(WafCompileFailure::Config("required bundleRefs are missing".to_string()));
        }
        let prepared: Vec<_> = resolved
            .iter()
            .map(|bundle| {
                (
                    bundle.data.roots.iter().map(String::as_str).collect::<Vec<_>>(),
                    bundle
                        .data
                        .rules
                        .iter()
                        .map(|member| RuleBundleMember {
                            path: &member.name,
                            bytes: member.content.as_bytes(),
                        })
                        .collect::<Vec<_>>(),
                    bundle
                        .data
                        .phrase_assets
                        .iter()
                        .map(|member| RuleBundleMember {
                            path: &member.name,
                            bytes: member.content.as_bytes(),
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        let sources = resolved
            .iter()
            .zip(&prepared)
            .map(|(bundle, (entrypoints, rules, phrase_assets))| RuleBundleSource {
                identity: bundle.identity,
                entrypoints,
                rules,
                phrase_assets,
            })
            .collect::<Vec<_>>();

        let adapter_bytes = prepared.capacity()
            * std::mem::size_of::<(Vec<&str>, Vec<RuleBundleMember<'_>>, Vec<RuleBundleMember<'_>>)>()
            + prepared
                .iter()
                .map(|(entries, rules, assets)| {
                    entries.capacity() * std::mem::size_of::<&str>()
                        + (rules.capacity() + assets.capacity()) * std::mem::size_of::<RuleBundleMember<'_>>()
                })
                .sum::<usize>()
            + sources.capacity() * std::mem::size_of::<RuleBundleSource<'_>>();
        // Both internal callers retain one exact-size borrowed-view array through compilation.
        let adapter_bytes = resolved
            .len()
            .checked_mul(std::mem::size_of::<WafResolvedBundleRef<'_>>())
            .and_then(|views| adapter_bytes.checked_add(views))
            .ok_or_else(|| WafCompileFailure::Config("Waf adapter memory accounting overflow".into()))?;
        let (generation, summary) = if let Some(selection) = selections
            .selection(selected, profile)
            .map_err(WafCompileFailure::Config)?
        {
            let live_bytes = selections.live_bytes() - selection.owned_bytes() + adapter_bytes;
            let (bundle, summary) = compile_rule_bundles_with_selection(
                &sources,
                "",
                Self::validation_context()?,
                FrontendLimits::default(),
                selection,
                live_bytes,
                |generation| self.validate_body_generation(generation),
                None,
            )
            .map_err(|error| match error {
                WafSelectionCompileError::Frontend(error) => WafCompileFailure::Frontend(error),
                WafSelectionCompileError::Admission(error) => error,
            })?;
            (
                bundle.into_parts().0,
                (!profile.rule_selection.is_empty()).then_some(summary),
            )
        } else {
            let generation = compile_rule_bundles_with_external_budget(
                &sources,
                "",
                Self::validation_context()?,
                FrontendLimits::default(),
                if selections.live_bytes() == 0 {
                    0
                } else {
                    selections.live_bytes() + adapter_bytes
                },
            )
            .map_err(WafCompileFailure::Frontend)?
            .into_parts()
            .0;
            self.validate_body_generation(&generation)?;
            (generation, None)
        };
        Ok(WafCompilation {
            generation,
            selection_summary: summary,
        })
    }

    /// Inspect the exact inline WAF configuration without publishing it.
    /// This report contains no source text, operand, file identity, or raw error string.
    pub fn inspect_inline_source(source: &str) -> WafSupportReport {
        Self {
            rules: source.to_string(),
            ..Self::default()
        }
        .inspect_support()
    }

    /// Inspect a local source fixture without publishing it.
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
        if !self.policy_ref.name.is_empty() || self.resolved_policy.is_some() {
            report.failure = Some(WafSupportFailure::new(
                "unresolved_reference",
                "Bundle references require Controller resolution before full support inspection",
            ));
            return report;
        }
        if self.request_body.prefix_size_bytes().is_err() {
            report.failure = Some(WafSupportFailure::new(
                "invalid_config",
                "Correct requestBody inspection, prefixSize and JSON budgets",
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
            Err(WafCompileFailure::Json(_)) => {
                report.failure = Some(WafSupportFailure::new(
                    "unsupported_json_dependency",
                    "Use bounded named JSON operands or supported static predicates",
                ));
            }
            Err(WafCompileFailure::Xml(_)) => {
                report.failure = Some(WafSupportFailure::new(
                    "unsupported_xml_dependency",
                    "Use supported XML selectors and static XML value predicates",
                ));
            }
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
        self.compile_generation_detailed()
            .map_err(Self::compile_failure_message)
    }

    fn compile_failure_message(error: WafCompileFailure) -> String {
        match error {
            WafCompileFailure::Config(message) => message,
            WafCompileFailure::Frontend(error) => format!("WAF rules are not executable: {error}"),
            WafCompileFailure::Prefix(message) => message.to_string(),
            WafCompileFailure::Xml(finding) => format!(
                "WAF XML dependency is unsupported: rule {}, member {}, reason {:?}",
                finding.rule_id, finding.member, finding.reason
            ),
            WafCompileFailure::Json(finding) => format!(
                "WAF JSON dependency is unsupported: rule {}, member {}, reason {:?}",
                finding.rule_id, finding.member, finding.reason
            ),
        }
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
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/waf_test.rs"]
mod tests;
