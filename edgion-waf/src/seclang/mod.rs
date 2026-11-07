//! Bounded SecLang source parsing and compilation primitives.

mod action;
mod body;
mod compiler;
mod directive;
mod include;
mod json_checker;
mod json_number;
mod json_scratch;
mod json_stream;
mod operator;
mod semantic;
mod source;
mod syntax;
mod tag_selection;
mod target;
mod tokenizer;
mod transform;
mod xml_stream;

fn decimal_bytes(mut value: usize) -> ([u8; 40], usize) {
    let mut bytes = [0_u8; 40];
    let mut start = bytes.len();
    loop {
        start -= 1;
        bytes[start] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            return (bytes, start);
        }
    }
}

pub(crate) use semantic::validate_executable_profile;
pub use tag_selection::{
    rule_unit_tags, PreparedWafRuleSelection, WafRuleIdRange, WafRuleSelection, WafRuleSelectionExpr,
    WafSelectionSummary, MAX_POLICY_SELECTION_BYTES,
};
pub(crate) use xml_stream::{XmlSemanticSummary, XmlStreamInput, XmlStreamSink, XmlStreamSinkError, XmlStreamUsage};
pub use xml_stream::{XmlStream, XmlStreamError, XmlStreamLimit, XmlStreamLimits};

pub use json_checker::{JsonChecker, JsonCheckerError, JsonStructureLimit, JsonStructureLimits};
pub(crate) use json_checker::{JsonCheckerSink, JsonCheckerSinkError, JsonScalarKind, JsonSemanticSummary};

pub(crate) use json_number::JsonNumber;

pub(crate) use json_stream::JsonStream;
pub use json_stream::{
    JsonDiscardedStringSummary, JsonStreamError, JsonStreamInput, JsonStreamLimit, JsonStreamLimits,
    JsonStreamSelective, JsonStreamSelectiveLimits, JsonStreamSelectiveSink, JsonStreamSinkError, JsonStreamToken,
    JsonStreamUsage,
};

pub use directive::{parse_directives, Directive, DirectiveKind, DirectiveLimits, DirectiveParseError};
pub(crate) use directive::{DirectiveDocument, DirectiveView};

pub use action::ActionSyntaxError;
#[cfg(test)]
pub(crate) use action::ActionValueSpelling;
pub(crate) use action::{ActionEntryKind, ActionText};
pub use body::{BodyError, BodyLimits, BodyProcessorSelection, RequestBody, DEFAULT_MAX_BODY_ENTRIES};
pub(crate) use body::{JsonPublicationSummary, XmlPublicationSummary};
pub(crate) use compiler::{compile_validated, CompilerInvariant, CompilerLimits};
pub use compiler::{
    CompiledGeneration, CompiledScheduleEntryKind, CompilerError, EvaluationMode, JsonInspectionLimits,
    JsonInspectionSession, JsonInspectionUnsupported, JsonInspectionUsage, JsonResultLimit, JsonResultLimits,
    PhaseOutcome, PhraseAssetFailure, PhraseAssetRequest, PhraseAssetSink, PhraseAssetSource,
    RequestBodyPrefixCompletion, RequestHead, RequestHeader, ResponseHead, ResponseHeader, ResponseWindow,
    RuleCheckPolicy, RuleEngineMode, RuntimeError, RuntimeLimits, Transaction, XmlInspectionLimits,
    XmlInspectionSession, XmlInspectionUnsupported, XmlInspectionUsage, DEFAULT_REQUEST_BODY_PREFIX_BYTES,
    MAX_REQUEST_BODY_PREFIX_BYTES,
};
#[cfg(test)]
pub(crate) use compiler::{XmlInspectionUnsupportedReason, DEFAULT_MAX_RUNTIME_OPERATOR_WORK_BYTES};
pub use include::{
    compile_executable_profile_with_includes, compile_rule_bundles, compile_rule_bundles_with_external_budget,
    compile_rule_bundles_with_selection, compile_with_includes, inspect_compatibility, CompatibilityLimits,
    CompatibilityReport, FrontendError, FrontendLimits, IncludeError, IncludeFailure, IncludeRequest, IncludeSink,
    IncludeSource, RuleBundleMember, RuleBundleSource, WafRuleMetadataBudget, WafRuleSelectionObserver,
    WafRuleSelectionPreview, WafSelectionCompileError,
};
pub use operator::OperatorSyntaxError;
pub(crate) use operator::{DecodedTextView, OperatorNameView, OperatorView};
pub use semantic::{
    analyze_one_semantic_draft, validate_semantic_draft, ActionEnvelopeKind, ActionKind, ActionOperandView,
    CollectionKind, CtlView, DirectiveLocation, FoldRequiredKind, MacroExpansionView, OperandTemplateView,
    OperatorOperandView, Phase, PhaseDefaults, SemanticError, SemanticLimits, SemanticLocation,
    SemanticValidationContext, SemanticValidationError, Severity, TemplateSegmentKind, TemplateSliceView,
    TransformationKind, UnavailableCapability, ValidatedSemanticDocument, ValidationLimits,
};
pub(crate) use semantic::{
    CompileRequirement, ExternalAssetView, FinalRuleRole, LocalActionValue, MacroSource, OnOff, OperatorKind,
    RequestBodyProcessor, ResolvedDisruptive, SemanticActionView, SemanticDraft, SemanticNodeKind,
    SemanticOperatorView, SetVarOperation, ValidatedRuleNodeView, ValidatedRuleUnitView,
};
pub use source::{scan_source, SourceError, SourceLimits, SourcePosition};
pub(crate) use source::{LogicalLine, SourceDocument, SourceSpan};
pub use syntax::{parse_syntax, SyntaxError, SyntaxLimits};
pub(crate) use syntax::{SyntaxDirectiveView, SyntaxDocument};
pub(crate) use target::{DirectiveTargetView, IdSelector, IdSelectorKind, RegexStyle, SyntaxText};
pub use target::{SelectorKind, TargetModifier, TargetSyntaxError};
pub use tokenizer::{tokenize_document, TokenizeError, TokenizerLimits};
pub(crate) use tokenizer::{QuoteStyle, Token, TokenizedDocument};

#[cfg(test)]
#[path = "../../tests/unit/seclang/mod_test/measurements.rs"]
pub(crate) mod measurements;
