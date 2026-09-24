//! Bounded SecLang source parsing and compilation primitives.

mod action;
mod body;
mod compiler;
mod directive;
mod include;
mod operator;
mod semantic;
mod source;
mod syntax;
mod target;
mod tokenizer;
mod transform;

pub(crate) use semantic::validate_executable_profile;

pub use directive::{
    parse_directives, Directive, DirectiveArity, DirectiveDocument, DirectiveField, DirectiveKind, DirectiveLimits,
    DirectiveParseError, DirectiveView, MarkerText, QuoteRequirement, DEFAULT_MAX_DIRECTIVES,
    DEFAULT_MAX_DIRECTIVE_OWNED_BYTES, DEFAULT_MAX_UPDATE_SELECTORS,
};

pub use action::{
    ActionEntry, ActionEntryKind, ActionListKind, ActionListView, ActionSyntaxError, ActionSyntaxLimits, ActionText,
    ActionValue, ActionValueSpelling, EmptyActionKind,
};
pub use body::{
    BodyError, BodyLimits, BodyProcessorSelection, RequestBody, DEFAULT_MAX_BODY_DECODED_BYTES, DEFAULT_MAX_BODY_DEPTH,
    DEFAULT_MAX_BODY_ENTRIES, DEFAULT_MAX_BODY_NAME_BYTES, DEFAULT_MAX_BODY_WORK_BYTES, DEFAULT_MAX_MULTIPART_HEADERS,
    DEFAULT_MAX_MULTIPART_PARTS, DEFAULT_MAX_REQUEST_BODY_BYTES,
};
pub use compiler::{
    compile_validated, CompiledGeneration, CompiledRuleMemberView, CompiledRuleUnitView, CompiledScheduleEntryKind,
    CompiledScheduleEntryView, CompiledTargetPlanView, CompiledTargetRecordView, CompiledTargetSelectorKind,
    CompilerError, CompilerInvariant, CompilerLimits, EvaluationMode, Intervention, PhaseOutcome, PhraseAssetFailure,
    PhraseAssetRequest, PhraseAssetSink, PhraseAssetSinkError, PhraseAssetSource, PhraseMatcherErrorKind,
    RequestBodyExecutionClass, RequestBodyExecutionSummary, RequestBodyPrefixCompletion, RequestHead, RequestHeader,
    ResponseHead, ResponseHeader, ResponseWindow, RuleEngineMode, RuleFinding, RuntimeError, RuntimeLimits,
    RuntimeMatcherError, RuntimeTransformError, Transaction, DEFAULT_MAX_RUNTIME_CAPTURE_BYTES,
    DEFAULT_MAX_RUNTIME_CAPTURE_ENTRIES, DEFAULT_MAX_RUNTIME_COLLECTION_ENTRIES, DEFAULT_MAX_RUNTIME_COOKIES,
    DEFAULT_MAX_RUNTIME_FINDINGS, DEFAULT_MAX_RUNTIME_HEADERS, DEFAULT_MAX_RUNTIME_HEADER_BYTES,
    DEFAULT_MAX_RUNTIME_INPUT_BYTES, DEFAULT_MAX_RUNTIME_MATCHER_SCRATCH_BYTES,
    DEFAULT_MAX_RUNTIME_OPERATOR_EVALUATIONS, DEFAULT_MAX_RUNTIME_OPERATOR_WORK_BYTES,
    DEFAULT_MAX_RUNTIME_QUERY_ARGUMENTS, DEFAULT_MAX_RUNTIME_RESPONSE_HEADERS,
    DEFAULT_MAX_RUNTIME_RESPONSE_HEADER_BYTES, DEFAULT_MAX_RUNTIME_RESPONSE_WINDOW_BYTES,
    DEFAULT_MAX_RUNTIME_RETAINED_BYTES, DEFAULT_MAX_RUNTIME_SELECTED_VALUES, DEFAULT_MAX_RUNTIME_SETVAR_ACTIONS,
    DEFAULT_MAX_RUNTIME_TRANSFORMATION_OUTPUT_BYTES, DEFAULT_MAX_RUNTIME_TRANSFORMATION_WORK_BYTES,
    DEFAULT_MAX_RUNTIME_TX_BYTES, DEFAULT_MAX_RUNTIME_TX_VARIABLES, DEFAULT_MAX_RUNTIME_VALUE_BYTES,
    DEFAULT_REQUEST_BODY_PREFIX_BYTES, MAX_REQUEST_BODY_PREFIX_BYTES,
};
pub use include::{
    compile_executable_profile_with_includes, compile_rule_bundles, compile_with_includes, inspect_compatibility,
    CompatibilityError, CompatibilityFinding, CompatibilityLimits, CompatibilityReport, CompatibilityRuleSummary,
    CompiledBundle, FrontendError, FrontendLimits, IncludeError, IncludeFailure, IncludeKind, IncludeLimits,
    IncludeMetrics, IncludeRequest, IncludeSink, IncludeSinkError, IncludeSource, RuleBundleError, RuleBundleMember,
    RuleBundleSource, DEFAULT_MAX_COMPATIBILITY_FINDINGS, DEFAULT_MAX_COMPATIBILITY_RULE_SUMMARIES,
    DEFAULT_MAX_EXPANDED_SOURCE_BYTES, DEFAULT_MAX_INCLUDE_DEPTH, DEFAULT_MAX_INCLUDE_IDENTITY_BYTES,
    DEFAULT_MAX_INCLUDE_IDENTITY_COMPARISON_BYTES, DEFAULT_MAX_INCLUDE_SOURCES, DEFAULT_MAX_INCLUDE_SOURCE_BYTES,
};
pub use operator::{
    DecodedBytes, DecodedChunks, DecodedTextView, OperatorNameView, OperatorSpelling, OperatorSyntaxError,
    OperatorSyntaxLimits, OperatorView,
};
pub use semantic::{
    analyze_one_semantic_draft, analyze_semantic_draft, validate_semantic_draft, ActionEnvelopeKind, ActionKind,
    ActionOperandView, ActiveRuleUnitIter, AppliedAffectedUnitIter, AppliedRuleRemovalView, AppliedTargetUpdateView,
    AssetErrorKind, AuditEngineMode, ByteSetView, CapabilityRecord, ChainMemberIter, ChainMemberView, ChainView,
    CollectionKind, CompileRequirement, CtlErrorKind, CtlTargetView, CtlView, DefaultDisruptive, DefaultsContextError,
    DirectiveLocation, DisruptiveKind, EffectiveTransformationIter, ExternalAssetIter, ExternalAssetView,
    ExternalAssetsView, FinalPhaseState, FinalRuleRole, FinalRuleView, FoldActionPresence, FoldLimits,
    FoldRequiredKind, FoldSingletonKind, GraphLimits, InitColCollection, InitColErrorKind, InitColView,
    IntegerOperandErrorKind, IpNetworkIter, IpNetworkView, IpNetworksView, LocalActionValue, LocalRuleEnvelope,
    LocalRuleFoldView, MacroErrorKind, MacroExpansionView, MacroSource, MarkerView, OnOff, OperandLimits,
    OperandTemplateView, OperandTextView, OperatorKind, OperatorOperandContract, OperatorOperandView, Phase,
    PhaseDefaults, PhraseIter, PhraseListView, PhraseView, ProvisionalRuleRole, RequestBodyProcessor,
    RequirementRecord, ResolvedDisruptive, ResolvedRuleStateView, ResolvedSkipAfterView, ScalarKind,
    SemanticActionIter, SemanticActionView, SemanticDirectiveView, SemanticDraft, SemanticError, SemanticLimits,
    SemanticLocation, SemanticNodeKind, SemanticOperatorView, SemanticTarget, SemanticText, SemanticValidationContext,
    SemanticValidationError, SetVarErrorKind, SetVarOperation, SetVarView, Severity, TemplateSegmentKind,
    TemplateSegmentView, TemplateSliceSegmentIter, TemplateSliceSegmentView, TemplateSliceView, TransformationKind,
    UnavailableCapability, ValidatedMarkerView, ValidatedRequirementView, ValidatedRuleNodeIter, ValidatedRuleNodeView,
    ValidatedRuleUnitView, ValidatedSemanticDocument, ValidationLimits, ValueContract,
    DEFAULT_MAX_EFFECTIVE_TRANSFORMATIONS, DEFAULT_MAX_FOLDED_RULES, DEFAULT_MAX_SEMANTIC_DIRECTIVES,
    DEFAULT_MAX_SEMANTIC_DOCUMENTS, DEFAULT_MAX_SEMANTIC_NODES, DEFAULT_MAX_SEMANTIC_OWNED_BYTES,
    DEFAULT_MAX_SEMANTIC_RETAINED_BYTES, DEFAULT_MAX_SEMANTIC_RULES, MARKER_WORK_ENTRY_CHARGE,
};
pub use source::{
    scan_source, LogicalLine, ProvenanceSegment, SourceDocument, SourceError, SourceLimits, SourcePosition, SourceSpan,
};
pub use syntax::{
    parse_syntax, SyntaxDirectiveView, SyntaxDocument, SyntaxError, SyntaxInvariantKind, SyntaxIter, SyntaxLimits,
    DEFAULT_MAX_SYNTAX_RETAINED_BYTES,
};
pub use target::{
    DirectiveTargetView, IdSelector, IdSelectorKind, ModifierErrorKind, RegexStyle, SelectorKind, SyntaxText,
    TargetAtom, TargetModifier, TargetSyntaxError, TargetSyntaxLimits,
};
pub use tokenizer::{
    tokenize_document, QuoteStyle, Token, TokenizeError, TokenizedDocument, TokenizedLine, TokenizerLimits,
    DEFAULT_MAX_TOKENIZER_OWNED_BYTES, DEFAULT_MAX_TOKENS, DEFAULT_MAX_TOKENS_PER_LINE, DEFAULT_MAX_TOKEN_BYTES,
};
