use thiserror::Error;

use super::{
    analyze_one_semantic_draft, compile_validated, parse_syntax, scan_source, validate_executable_profile,
    validate_semantic_draft, CompiledGeneration, CompilerError, CompilerLimits, FinalRuleRole, PhraseAssetSource,
    SemanticError, SemanticLimits, SemanticLocation, SemanticValidationContext, SemanticValidationError, SourceError,
    SourceLimits, SyntaxError, SyntaxLimits, UnavailableCapability, ValidationLimits,
};

use super::{PhraseAssetFailure, PhraseAssetRequest, PhraseAssetSink};

/// A named rule or phrase asset within one immutable rule bundle.
#[derive(Clone, Copy)]
pub struct RuleBundleMember<'a> {
    pub path: &'a str,
    pub bytes: &'a [u8],
}

/// One ordered rule bundle. Identity must uniquely name the selected bundle revision.
#[derive(Clone, Copy)]
pub struct RuleBundleSource<'a> {
    pub identity: &'a str,
    pub entrypoints: &'a [&'a str],
    pub rules: &'a [RuleBundleMember<'a>],
    pub phrase_assets: &'a [RuleBundleMember<'a>],
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RuleBundleError {
    #[error("rule bundle identity is empty, duplicated, or exceeds the identity byte limit")]
    Identity,
    #[error("rule bundle has no entrypoints or contains a repeated entrypoint")]
    Entrypoints,
    #[error("rule bundle member or reference path is invalid")]
    Path,
    #[error("rule bundle contains a duplicate member path")]
    DuplicateMember,
    #[error("rule bundle entrypoint does not name a rule member")]
    MissingEntrypoint,
    #[error("rule bundle input exceeds a configured source limit")]
    Limit,
}

pub const DEFAULT_MAX_INCLUDE_DEPTH: usize = 16;
pub const DEFAULT_MAX_INCLUDE_SOURCES: usize = 1_024;
pub const DEFAULT_MAX_INCLUDE_SOURCE_BYTES: usize = 16 * 1024 * 1024;
pub const DEFAULT_MAX_EXPANDED_SOURCE_BYTES: usize = 64 * 1024 * 1024;
pub const DEFAULT_MAX_INCLUDE_IDENTITY_BYTES: usize = 4 * 1024;
pub const DEFAULT_MAX_INCLUDE_IDENTITY_COMPARISON_BYTES: usize = 64 * 1024 * 1024;
pub const DEFAULT_MAX_COMPATIBILITY_FINDINGS: usize = 1_024;
pub const DEFAULT_MAX_COMPATIBILITY_RULE_SUMMARIES: usize = 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IncludeLimits {
    pub max_depth: usize,
    pub max_sources: usize,
    pub max_source_bytes: usize,
    pub max_expanded_bytes: usize,
    pub max_identity_bytes: usize,
    pub max_identity_comparison_bytes: usize,
}

impl Default for IncludeLimits {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_MAX_INCLUDE_DEPTH,
            max_sources: DEFAULT_MAX_INCLUDE_SOURCES,
            max_source_bytes: DEFAULT_MAX_INCLUDE_SOURCE_BYTES,
            max_expanded_bytes: DEFAULT_MAX_EXPANDED_SOURCE_BYTES,
            max_identity_bytes: DEFAULT_MAX_INCLUDE_IDENTITY_BYTES,
            max_identity_comparison_bytes: DEFAULT_MAX_INCLUDE_IDENTITY_COMPARISON_BYTES,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompatibilityLimits {
    pub max_findings: usize,
    pub max_rule_summaries: usize,
}

impl Default for CompatibilityLimits {
    fn default() -> Self {
        Self {
            max_findings: DEFAULT_MAX_COMPATIBILITY_FINDINGS,
            max_rule_summaries: DEFAULT_MAX_COMPATIBILITY_RULE_SUMMARIES,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FrontendLimits {
    pub includes: IncludeLimits,
    pub source: SourceLimits,
    pub syntax: SyntaxLimits,
    pub semantic: SemanticLimits,
    pub validation: ValidationLimits,
    pub compiler: CompilerLimits,
    pub compatibility: CompatibilityLimits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncludeKind {
    Required,
    Optional,
}

#[derive(Clone, Copy)]
pub struct IncludeRequest<'request> {
    parent_identity: &'request str,
    reference: &'request str,
    kind: IncludeKind,
    depth: usize,
}

impl std::fmt::Debug for IncludeRequest<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IncludeRequest")
            .field("kind", &self.kind)
            .field("depth", &self.depth)
            .finish_non_exhaustive()
    }
}

impl<'request> IncludeRequest<'request> {
    pub const fn parent_identity(self) -> &'request str {
        self.parent_identity
    }

    pub const fn reference(self) -> &'request str {
        self.reference
    }

    pub const fn kind(self) -> IncludeKind {
        self.kind
    }

    pub const fn depth(self) -> usize {
        self.depth
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncludeFailure {
    NotFound,
    Denied,
    Unavailable,
    InvalidData,
    Other,
}

impl std::fmt::Display for IncludeFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "include provider failed with {self:?}")
    }
}

impl std::error::Error for IncludeFailure {}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum IncludeSinkError {
    #[error("include source count exceeds the configured limit of {limit}")]
    SourceLimit { limit: usize },
    #[error("include canonical identity has {actual} bytes, exceeding the limit of {limit}")]
    IdentityByteLimit { actual: usize, limit: usize },
    #[error("include canonical identity is empty")]
    EmptyIdentity,
    #[error("included source has {actual} bytes, exceeding the limit of {limit}")]
    SourceByteLimit { actual: usize, limit: usize },
    #[error("expanded include inputs exceed the configured byte limit of {limit}")]
    AggregateByteLimit { limit: usize },
    #[error("include sink arithmetic overflowed")]
    Arithmetic,
    #[error("cannot allocate bounded include source storage")]
    Allocation,
}

#[derive(Debug)]
struct LoadedInclude {
    identity: String,
    bytes: Vec<u8>,
}

pub struct IncludeSink<'sink> {
    documents: &'sink mut Vec<LoadedInclude>,
    sticky: Option<IncludeSinkError>,
    prior_sources: usize,
    prior_bytes: usize,
    pending_bytes: usize,
    limits: IncludeLimits,
}

impl IncludeSink<'_> {
    pub fn push_source(&mut self, canonical_identity: &str, bytes: &[u8]) -> Result<(), IncludeSinkError> {
        if let Some(error) = self.sticky {
            return Err(error);
        }
        let result = self.push_once(canonical_identity, bytes);
        if let Err(error) = result {
            self.sticky = Some(error);
        }
        result
    }

    fn push_once(&mut self, canonical_identity: &str, bytes: &[u8]) -> Result<(), IncludeSinkError> {
        if canonical_identity.is_empty() {
            return Err(IncludeSinkError::EmptyIdentity);
        }
        if canonical_identity.len() > self.limits.max_identity_bytes {
            return Err(IncludeSinkError::IdentityByteLimit {
                actual: canonical_identity.len(),
                limit: self.limits.max_identity_bytes,
            });
        }
        let sources = self
            .prior_sources
            .checked_add(self.documents.len())
            .and_then(|count| count.checked_add(1))
            .ok_or(IncludeSinkError::Arithmetic)?;
        if sources > self.limits.max_sources {
            return Err(IncludeSinkError::SourceLimit {
                limit: self.limits.max_sources,
            });
        }
        if bytes.len() > self.limits.max_source_bytes {
            return Err(IncludeSinkError::SourceByteLimit {
                actual: bytes.len(),
                limit: self.limits.max_source_bytes,
            });
        }
        let aggregate = self
            .prior_bytes
            .checked_add(self.pending_bytes)
            .and_then(|total| total.checked_add(bytes.len()))
            .ok_or(IncludeSinkError::Arithmetic)?;
        if aggregate > self.limits.max_expanded_bytes {
            return Err(IncludeSinkError::AggregateByteLimit {
                limit: self.limits.max_expanded_bytes,
            });
        }
        let mut identity = String::new();
        identity
            .try_reserve_exact(canonical_identity.len())
            .map_err(|_| IncludeSinkError::Allocation)?;
        identity.push_str(canonical_identity);
        let mut owned = Vec::new();
        owned
            .try_reserve_exact(bytes.len())
            .map_err(|_| IncludeSinkError::Allocation)?;
        owned.extend_from_slice(bytes);
        self.documents
            .try_reserve_exact(1)
            .map_err(|_| IncludeSinkError::Allocation)?;
        self.documents.push(LoadedInclude { identity, bytes: owned });
        self.pending_bytes = self
            .pending_bytes
            .checked_add(bytes.len())
            .ok_or(IncludeSinkError::Arithmetic)?;
        Ok(())
    }
}

pub trait IncludeSource {
    fn load(&mut self, request: IncludeRequest<'_>, sink: &mut IncludeSink<'_>) -> Result<(), IncludeFailure>;
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum IncludeError {
    #[error("root source identity is empty")]
    EmptyRootIdentity,
    #[error("root source identity has {actual} bytes, exceeding the limit of {limit}")]
    RootIdentityByteLimit { actual: usize, limit: usize },
    #[error("root source has {actual} bytes, exceeding the limit of {limit}")]
    RootSourceByteLimit { actual: usize, limit: usize },
    #[error("include directive is malformed on physical line {line}")]
    Syntax { line: usize },
    #[error("include nesting depth {actual} exceeds the limit of {limit}")]
    DepthLimit { actual: usize, limit: usize },
    #[error("include canonical identity cycle detected at depth {depth}")]
    Cycle { depth: usize },
    #[error("include identity comparison work exceeds the limit of {limit}")]
    IdentityComparisonLimit { limit: usize },
    #[error("required include resolved no sources at depth {depth}")]
    EmptyResolution { depth: usize },
    #[error("include source failed at depth {depth} with {failure:?}")]
    Source { depth: usize, failure: IncludeFailure },
    #[error("include sink failed at depth {depth}: {error}")]
    Sink { depth: usize, error: IncludeSinkError },
    #[error("expanded source exceeds the configured byte limit of {limit}")]
    ExpandedByteLimit { limit: usize },
    #[error("include expansion arithmetic overflowed")]
    Arithmetic,
    #[error("cannot allocate bounded include expansion storage")]
    Allocation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IncludeMetrics {
    pub source_count: usize,
    pub source_bytes: usize,
    pub expanded_bytes: usize,
    pub maximum_depth: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ParsedInclude<'line> {
    kind: IncludeKind,
    reference: &'line str,
}

struct IncludeExpander<'resolver> {
    resolver: &'resolver mut dyn IncludeSource,
    limits: IncludeLimits,
    expanded: Vec<u8>,
    stack: Vec<String>,
    source_count: usize,
    source_bytes: usize,
    maximum_depth: usize,
    identity_comparison_bytes: usize,
    provenance: Vec<ExpandedSegment>,
    provenance_bytes: usize,
    track_provenance: bool,
}

#[derive(Debug)]
struct ExpandedSegment {
    start: usize,
    end: usize,
    identity: String,
}

impl IncludeExpander<'_> {
    fn append(&mut self, bytes: &[u8]) -> Result<(), IncludeError> {
        let next = self
            .expanded
            .len()
            .checked_add(bytes.len())
            .ok_or(IncludeError::Arithmetic)?;
        if next > self.limits.max_expanded_bytes {
            return Err(IncludeError::ExpandedByteLimit {
                limit: self.limits.max_expanded_bytes,
            });
        }
        self.expanded
            .try_reserve_exact(bytes.len())
            .map_err(|_| IncludeError::Allocation)?;
        self.expanded.extend_from_slice(bytes);
        Ok(())
    }

    fn append_from(&mut self, identity: &str, bytes: &[u8]) -> Result<(), IncludeError> {
        if bytes.is_empty() {
            return Ok(());
        }
        let start = self.expanded.len();
        self.append(bytes)?;
        if !self.track_provenance {
            return Ok(());
        }
        if let Some(last) = self.provenance.last_mut() {
            if last.end == start && last.identity == identity {
                last.end = self.expanded.len();
                return Ok(());
            }
        }
        let provenance_bytes = self
            .provenance_bytes
            .checked_add(identity.len())
            .ok_or(IncludeError::Arithmetic)?;
        if self.provenance.len() >= self.limits.max_sources.saturating_mul(self.limits.max_source_bytes)
            || provenance_bytes > self.limits.max_expanded_bytes
        {
            return Err(IncludeError::ExpandedByteLimit {
                limit: self.limits.max_expanded_bytes,
            });
        }
        self.provenance.try_reserve(1).map_err(|_| IncludeError::Allocation)?;
        self.provenance.push(ExpandedSegment {
            start,
            end: self.expanded.len(),
            identity: identity.to_owned(),
        });
        self.provenance_bytes = provenance_bytes;
        Ok(())
    }

    fn expand_source(&mut self, identity: &str, bytes: &[u8], depth: usize) -> Result<(), IncludeError> {
        if depth > self.limits.max_depth {
            return Err(IncludeError::DepthLimit {
                actual: depth,
                limit: self.limits.max_depth,
            });
        }
        self.maximum_depth = self.maximum_depth.max(depth);
        for active in &self.stack {
            let compared = active.len().min(identity.len());
            self.identity_comparison_bytes = self
                .identity_comparison_bytes
                .checked_add(compared)
                .ok_or(IncludeError::Arithmetic)?;
            if self.identity_comparison_bytes > self.limits.max_identity_comparison_bytes {
                return Err(IncludeError::IdentityComparisonLimit {
                    limit: self.limits.max_identity_comparison_bytes,
                });
            }
            if active == identity {
                return Err(IncludeError::Cycle { depth });
            }
        }
        let mut owned_identity = String::new();
        owned_identity
            .try_reserve_exact(identity.len())
            .map_err(|_| IncludeError::Allocation)?;
        owned_identity.push_str(identity);
        self.stack.try_reserve_exact(1).map_err(|_| IncludeError::Allocation)?;
        self.stack.push(owned_identity);

        let result = self.expand_lines(identity, bytes, depth);
        self.stack.pop();
        result
    }

    fn expand_lines(&mut self, identity: &str, bytes: &[u8], depth: usize) -> Result<(), IncludeError> {
        let text = std::str::from_utf8(bytes).map_err(|_| IncludeError::Syntax { line: 1 })?;
        let mut start = 0_usize;
        let mut line = 1_usize;
        let mut continued = false;
        while start < text.len() {
            let relative_end = text.as_bytes()[start..].iter().position(|byte| *byte == b'\n');
            let end = relative_end.map_or(text.len(), |offset| start + offset + 1);
            let physical = &text[start..end];
            let content = physical.strip_suffix('\n').unwrap_or(physical);
            let content = content.strip_suffix('\r').unwrap_or(content);
            let continues = line_continues(content.as_bytes());
            let parsed = if continued {
                None
            } else {
                parse_include_line(content, line)?
            };
            if let Some(include) = parsed {
                if continues {
                    return Err(IncludeError::Syntax { line });
                }
                self.expand_include(identity, include, depth.checked_add(1).ok_or(IncludeError::Arithmetic)?)?;
                if !self.expanded.is_empty() && !self.expanded.ends_with(b"\n") {
                    self.append(b"\n")?;
                }
            } else {
                self.append_from(identity, physical.as_bytes())?;
            }
            continued = continues;
            start = end;
            line = line.checked_add(1).ok_or(IncludeError::Arithmetic)?;
        }
        Ok(())
    }

    fn expand_include(&mut self, parent: &str, include: ParsedInclude<'_>, depth: usize) -> Result<(), IncludeError> {
        if depth > self.limits.max_depth {
            return Err(IncludeError::DepthLimit {
                actual: depth,
                limit: self.limits.max_depth,
            });
        }
        let mut loaded = Vec::new();
        let mut sink = IncludeSink {
            documents: &mut loaded,
            sticky: None,
            prior_sources: self.source_count,
            prior_bytes: self.source_bytes,
            pending_bytes: 0,
            limits: self.limits,
        };
        let provider = self.resolver.load(
            IncludeRequest {
                parent_identity: parent,
                reference: include.reference,
                kind: include.kind,
                depth,
            },
            &mut sink,
        );
        if let Some(error) = sink.sticky {
            return Err(IncludeError::Sink { depth, error });
        }
        if let Err(failure) = provider {
            if include.kind == IncludeKind::Optional && failure == IncludeFailure::NotFound && loaded.is_empty() {
                return Ok(());
            }
            return Err(IncludeError::Source { depth, failure });
        }
        if loaded.is_empty() {
            return if include.kind == IncludeKind::Optional {
                Ok(())
            } else {
                Err(IncludeError::EmptyResolution { depth })
            };
        }
        for (ordinal, document) in loaded.into_iter().enumerate() {
            if ordinal > 0 && !self.expanded.is_empty() && !self.expanded.ends_with(b"\n") {
                self.append(b"\n")?;
            }
            if self.source_count >= self.limits.max_sources {
                return Err(IncludeError::Sink {
                    depth,
                    error: IncludeSinkError::SourceLimit {
                        limit: self.limits.max_sources,
                    },
                });
            }
            let next_source_bytes = self
                .source_bytes
                .checked_add(document.bytes.len())
                .ok_or(IncludeError::Arithmetic)?;
            if next_source_bytes > self.limits.max_expanded_bytes {
                return Err(IncludeError::Sink {
                    depth,
                    error: IncludeSinkError::AggregateByteLimit {
                        limit: self.limits.max_expanded_bytes,
                    },
                });
            }
            self.source_count = self.source_count.checked_add(1).ok_or(IncludeError::Arithmetic)?;
            self.source_bytes = next_source_bytes;
            self.expand_source(&document.identity, &document.bytes, depth)?;
        }
        Ok(())
    }
}

fn line_continues(bytes: &[u8]) -> bool {
    let backslashes = bytes.iter().rev().take_while(|byte| **byte == b'\\').count();
    backslashes % 2 == 1
}

fn parse_include_line(line: &str, line_number: usize) -> Result<Option<ParsedInclude<'_>>, IncludeError> {
    let trimmed = line.trim_matches([' ', '\t']);
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Ok(None);
    }
    let name_end = trimmed.find([' ', '\t']).unwrap_or(trimmed.len());
    let name = &trimmed[..name_end];
    let kind = if name.eq_ignore_ascii_case("Include") {
        IncludeKind::Required
    } else if name.eq_ignore_ascii_case("IncludeOptional") {
        IncludeKind::Optional
    } else {
        return Ok(None);
    };
    let tail = trimmed[name_end..].trim_start_matches([' ', '\t']);
    if tail.is_empty() {
        return Err(IncludeError::Syntax { line: line_number });
    }
    let (reference, rest) = match tail.as_bytes()[0] {
        b'\'' | b'"' => {
            let quote = tail.as_bytes()[0];
            let Some(end) = tail.as_bytes()[1..].iter().position(|byte| *byte == quote) else {
                return Err(IncludeError::Syntax { line: line_number });
            };
            (&tail[1..end + 1], &tail[end + 2..])
        }
        _ => {
            let end = tail.find([' ', '\t', '#']).unwrap_or(tail.len());
            (&tail[..end], &tail[end..])
        }
    };
    if reference.is_empty() {
        return Err(IncludeError::Syntax { line: line_number });
    }
    let rest = rest.trim_start_matches([' ', '\t']);
    if !rest.is_empty() && !rest.starts_with('#') {
        return Err(IncludeError::Syntax { line: line_number });
    }
    Ok(Some(ParsedInclude { kind, reference }))
}

#[derive(Debug)]
struct ExpandedSource {
    bytes: Vec<u8>,
    metrics: IncludeMetrics,
    provenance: Vec<ExpandedSegment>,
}

fn expand_includes(
    root_identity: &str,
    root: &[u8],
    resolver: &mut dyn IncludeSource,
    limits: IncludeLimits,
    track_provenance: bool,
) -> Result<ExpandedSource, IncludeError> {
    if root_identity.is_empty() {
        return Err(IncludeError::EmptyRootIdentity);
    }
    if root_identity.len() > limits.max_identity_bytes {
        return Err(IncludeError::RootIdentityByteLimit {
            actual: root_identity.len(),
            limit: limits.max_identity_bytes,
        });
    }
    if root.len() > limits.max_source_bytes {
        return Err(IncludeError::RootSourceByteLimit {
            actual: root.len(),
            limit: limits.max_source_bytes,
        });
    }
    let mut expander = IncludeExpander {
        resolver,
        limits,
        expanded: Vec::new(),
        stack: Vec::new(),
        source_count: 0,
        source_bytes: 0,
        maximum_depth: 0,
        identity_comparison_bytes: 0,
        provenance: Vec::new(),
        provenance_bytes: 0,
        track_provenance,
    };
    expander.expand_source(root_identity, root, 0)?;
    Ok(ExpandedSource {
        metrics: IncludeMetrics {
            source_count: expander.source_count,
            source_bytes: expander.source_bytes,
            expanded_bytes: expander.expanded.len(),
            maximum_depth: expander.maximum_depth,
        },
        bytes: expander.expanded,
        provenance: expander.provenance,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompatibilityFinding {
    capability: UnavailableCapability,
    location: SemanticLocation,
    rule_id: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompatibilityRuleSummary {
    rule_id: u64,
    capability_mask: u8,
}

impl CompatibilityRuleSummary {
    pub const fn rule_id(self) -> u64 {
        self.rule_id
    }

    pub const fn has_capability(self, capability: UnavailableCapability) -> bool {
        self.capability_mask & (1_u8 << capability_index(capability)) != 0
    }
}

const fn capability_index(capability: UnavailableCapability) -> usize {
    match capability {
        UnavailableCapability::FullResponseBody => 0,
        UnavailableCapability::Ctl => 1,
        UnavailableCapability::InitCol => 2,
    }
}

impl CompatibilityFinding {
    pub const fn capability(self) -> UnavailableCapability {
        self.capability
    }

    pub const fn location(self) -> SemanticLocation {
        self.location
    }

    pub const fn rule_id(self) -> Option<u64> {
        self.rule_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibilityReport {
    include_metrics: IncludeMetrics,
    directive_count: usize,
    rule_count: usize,
    requirement_count: usize,
    unavailable_counts: [usize; 3],
    rule_unit_count: usize,
    unavailable_rule_unit_count: usize,
    unattributed_findings: usize,
    rule_summaries: Vec<CompatibilityRuleSummary>,
    omitted_rule_summaries: usize,
    findings: Vec<CompatibilityFinding>,
    omitted_findings: usize,
}

impl CompatibilityReport {
    fn from_draft(
        draft: &super::SemanticDraft<'_>,
        include_metrics: IncludeMetrics,
        limits: CompatibilityLimits,
    ) -> Result<Self, CompatibilityError> {
        let mut root_ids = Vec::new();
        root_ids
            .try_reserve_exact(draft.final_rules().len())
            .map_err(|_| CompatibilityError::Allocation)?;
        for rule in draft.final_rules() {
            if matches!(rule.role(), FinalRuleRole::ChainChild { .. }) {
                continue;
            }
            let Some(id) = rule.fold().id() else {
                continue;
            };
            let location = rule.location();
            root_ids.push((location.document_index(), location.directive_index(), id));
        }
        root_ids.sort_unstable_by_key(|(document, directive, _)| (*document, *directive));

        let mut attributions = Vec::new();
        attributions
            .try_reserve_exact(draft.final_rules().len())
            .map_err(|_| CompatibilityError::Allocation)?;
        for rule in draft.final_rules() {
            let unit_location = match rule.role() {
                FinalRuleRole::ChainChild { root, .. } => root,
                _ => rule.location(),
            };
            let Ok(index) = root_ids.binary_search_by_key(
                &(unit_location.document_index(), unit_location.directive_index()),
                |(document, directive, _)| (*document, *directive),
            ) else {
                continue;
            };
            let location = rule.location();
            attributions.push((location.document_index(), location.directive_index(), root_ids[index].2));
        }
        attributions.sort_unstable_by_key(|(document, directive, _)| (*document, *directive));

        let mut findings = Vec::new();
        findings
            .try_reserve_exact(limits.max_findings.min(draft.unavailable_capabilities().len()))
            .map_err(|_| CompatibilityError::Allocation)?;
        let mut unavailable_counts = [0_usize; 3];
        let mut omitted_findings = 0_usize;
        let mut unavailable_rule_capabilities = Vec::new();
        unavailable_rule_capabilities
            .try_reserve_exact(draft.unavailable_capabilities().len())
            .map_err(|_| CompatibilityError::Allocation)?;
        let mut unattributed_findings = 0_usize;
        for record in draft.unavailable_capabilities() {
            let index = capability_index(record.capability());
            unavailable_counts[index] = unavailable_counts[index]
                .checked_add(1)
                .ok_or(CompatibilityError::Arithmetic)?;
            let location = record.location();
            let rule_id = attributions
                .binary_search_by_key(
                    &(location.document_index(), location.directive_index()),
                    |(document, directive, _)| (*document, *directive),
                )
                .ok()
                .map(|index| attributions[index].2);
            if let Some(rule_id) = rule_id {
                unavailable_rule_capabilities.push((rule_id, index));
            } else {
                unattributed_findings = unattributed_findings
                    .checked_add(1)
                    .ok_or(CompatibilityError::Arithmetic)?;
            }
            if findings.len() < limits.max_findings {
                findings.push(CompatibilityFinding {
                    capability: record.capability(),
                    location,
                    rule_id,
                });
            } else {
                omitted_findings = omitted_findings.checked_add(1).ok_or(CompatibilityError::Arithmetic)?;
            }
        }
        unavailable_rule_capabilities.sort_unstable();
        let mut rule_summaries = Vec::new();
        rule_summaries
            .try_reserve_exact(limits.max_rule_summaries.min(unavailable_rule_capabilities.len()))
            .map_err(|_| CompatibilityError::Allocation)?;
        let mut unavailable_rule_unit_count = 0_usize;
        let mut omitted_rule_summaries = 0_usize;
        let mut cursor = 0_usize;
        while cursor < unavailable_rule_capabilities.len() {
            let rule_id = unavailable_rule_capabilities[cursor].0;
            let mut capability_mask = 0_u8;
            while cursor < unavailable_rule_capabilities.len() && unavailable_rule_capabilities[cursor].0 == rule_id {
                capability_mask |= 1_u8 << unavailable_rule_capabilities[cursor].1;
                cursor += 1;
            }
            unavailable_rule_unit_count = unavailable_rule_unit_count
                .checked_add(1)
                .ok_or(CompatibilityError::Arithmetic)?;
            if rule_summaries.len() < limits.max_rule_summaries {
                rule_summaries.push(CompatibilityRuleSummary {
                    rule_id,
                    capability_mask,
                });
            } else {
                omitted_rule_summaries = omitted_rule_summaries
                    .checked_add(1)
                    .ok_or(CompatibilityError::Arithmetic)?;
            }
        }
        Ok(Self {
            include_metrics,
            directive_count: draft.directive_count(),
            rule_count: draft.final_rules().len(),
            requirement_count: draft.requirements().len(),
            unavailable_counts,
            rule_unit_count: root_ids.len(),
            unavailable_rule_unit_count,
            unattributed_findings,
            rule_summaries,
            omitted_rule_summaries,
            findings,
            omitted_findings,
        })
    }

    pub const fn include_metrics(&self) -> IncludeMetrics {
        self.include_metrics
    }

    pub const fn directive_count(&self) -> usize {
        self.directive_count
    }

    pub const fn rule_count(&self) -> usize {
        self.rule_count
    }

    pub const fn requirement_count(&self) -> usize {
        self.requirement_count
    }

    pub const fn is_supported(&self) -> bool {
        self.unavailable_counts[0] == 0 && self.unavailable_counts[1] == 0 && self.unavailable_counts[2] == 0
    }

    pub const fn rule_unit_count(&self) -> usize {
        self.rule_unit_count
    }

    pub const fn unavailable_rule_unit_count(&self) -> usize {
        self.unavailable_rule_unit_count
    }

    pub const fn locally_executable_rule_unit_count(&self) -> usize {
        self.rule_unit_count - self.unavailable_rule_unit_count
    }

    pub const fn unattributed_findings(&self) -> usize {
        self.unattributed_findings
    }

    pub fn rule_summaries(&self) -> impl ExactSizeIterator<Item = CompatibilityRuleSummary> + '_ {
        self.rule_summaries.iter().copied()
    }

    pub const fn omitted_rule_summaries(&self) -> usize {
        self.omitted_rule_summaries
    }

    pub const fn unavailable_count(&self, capability: UnavailableCapability) -> usize {
        self.unavailable_counts[capability_index(capability)]
    }

    pub fn findings(&self) -> impl ExactSizeIterator<Item = CompatibilityFinding> + '_ {
        self.findings.iter().copied()
    }

    pub const fn omitted_findings(&self) -> usize {
        self.omitted_findings
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum CompatibilityError {
    #[error("compatibility report arithmetic overflowed")]
    Arithmetic,
    #[error("cannot allocate bounded compatibility findings")]
    Allocation,
    #[error("compatibility findings could not be attributed to rule units")]
    UnattributedRule,
    #[error("compatibility rule summaries exceed the configured limit of {limit}")]
    RuleSummaryLimit { limit: usize },
}

#[derive(Debug, Error)]
pub enum FrontendError {
    #[error("rule bundle input failed: {0}")]
    RuleBundle(#[from] RuleBundleError),
    #[error("include expansion failed: {0}")]
    Include(#[from] IncludeError),
    #[error("source scanning failed: {0}")]
    Source(#[from] SourceError),
    #[error("syntax parsing failed: {0}")]
    Syntax(#[from] SyntaxError),
    #[error("semantic analysis failed: {0}")]
    Semantic(#[from] SemanticError),
    #[error("compatibility reporting failed: {0}")]
    Compatibility(#[from] CompatibilityError),
    #[error("semantic validation failed: {0}")]
    Validation(#[from] SemanticValidationError),
    #[error("immutable compilation failed: {0}")]
    Compiler(#[from] CompilerError),
}

fn valid_bundle_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= DEFAULT_MAX_INCLUDE_IDENTITY_BYTES
        && path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-'))
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn bundle_path(parent: &str, reference: &str) -> Option<String> {
    if !valid_bundle_path(reference) {
        return None;
    }
    let (_, parent_path) = parent.split_once(':')?;
    let directory = parent_path.rsplit_once('/').map(|(directory, _)| directory);
    let result = directory.map_or_else(|| reference.to_owned(), |directory| format!("{directory}/{reference}"));
    valid_bundle_path(&result).then_some(result)
}

struct BundleIncludes<'a> {
    bundles: &'a [RuleBundleSource<'a>],
}

impl IncludeSource for BundleIncludes<'_> {
    fn load(&mut self, request: IncludeRequest<'_>, sink: &mut IncludeSink<'_>) -> Result<(), IncludeFailure> {
        let (index, path) = if request.parent_identity() == "bundle-root" {
            let (index, path) = request.reference().split_once(':').ok_or(IncludeFailure::Denied)?;
            (
                index.parse::<usize>().map_err(|_| IncludeFailure::Denied)?,
                path.to_owned(),
            )
        } else {
            let (index, _) = request
                .parent_identity()
                .split_once(':')
                .ok_or(IncludeFailure::Denied)?;
            (
                index.parse::<usize>().map_err(|_| IncludeFailure::Denied)?,
                bundle_path(request.parent_identity(), request.reference()).ok_or(IncludeFailure::Denied)?,
            )
        };
        let bundle = self.bundles.get(index).ok_or(IncludeFailure::Denied)?;
        if !valid_bundle_path(&path) {
            return Err(IncludeFailure::Denied);
        }
        let member = bundle
            .rules
            .iter()
            .find(|member| member.path == path)
            .ok_or(IncludeFailure::Denied)?;
        let identity = format!("{index}:{path}");
        sink.push_source(&identity, member.bytes)
            .map_err(|_| IncludeFailure::Other)
    }
}

struct BundleAssets<'a> {
    bundles: &'a [RuleBundleSource<'a>],
    provenance: &'a [ExpandedSegment],
}

struct DenyIncludes;

impl IncludeSource for DenyIncludes {
    fn load(&mut self, _request: IncludeRequest<'_>, _sink: &mut IncludeSink<'_>) -> Result<(), IncludeFailure> {
        Err(IncludeFailure::Denied)
    }
}

impl PhraseAssetSource for BundleAssets<'_> {
    fn load(
        &mut self,
        request: PhraseAssetRequest<'_>,
        sink: &mut PhraseAssetSink<'_>,
    ) -> Result<(), PhraseAssetFailure> {
        let offset = request.span().start.byte_offset;
        let segment_index = self.provenance.partition_point(|segment| segment.end <= offset);
        let segment = self
            .provenance
            .get(segment_index)
            .filter(|segment| segment.start <= offset && offset < segment.end)
            .ok_or(PhraseAssetFailure::Denied)?;
        let (index, _) = segment.identity.split_once(':').ok_or(PhraseAssetFailure::Denied)?;
        let index = index.parse::<usize>().map_err(|_| PhraseAssetFailure::Denied)?;
        let bundle = self.bundles.get(index).ok_or(PhraseAssetFailure::Denied)?;
        let path = bundle_path(&segment.identity, request.handle()).ok_or(PhraseAssetFailure::Denied)?;
        let member = bundle
            .phrase_assets
            .iter()
            .find(|member| member.path == path)
            .ok_or(PhraseAssetFailure::NotFound)?;
        sink.extend_from_slice(member.bytes)
            .map_err(|_| PhraseAssetFailure::Other)
    }
}

/// Compile all selected bundle entrypoints and trailing inline rules as one strict generation.
/// Includes and phrase assets can only resolve inside the rule's own bundle.
pub fn compile_rule_bundles(
    bundles: &[RuleBundleSource<'_>],
    inline_rules: &str,
    context: SemanticValidationContext,
    limits: FrontendLimits,
) -> Result<CompiledBundle, FrontendError> {
    if bundles.len() > limits.includes.max_sources {
        return Err(RuleBundleError::Limit.into());
    }
    let mut root = String::new();
    let mut members = 0_usize;
    let mut source_bytes = 0_usize;
    for (index, bundle) in bundles.iter().enumerate() {
        if bundle.identity.is_empty()
            || bundle.identity.len() > limits.includes.max_identity_bytes
            || bundles[..index]
                .iter()
                .any(|previous| previous.identity == bundle.identity)
        {
            return Err(RuleBundleError::Identity.into());
        }
        if bundle.entrypoints.is_empty() {
            return Err(RuleBundleError::Entrypoints.into());
        }
        for (entry_index, path) in bundle.entrypoints.iter().enumerate() {
            if !valid_bundle_path(path) {
                return Err(RuleBundleError::Path.into());
            }
            if bundle.entrypoints[..entry_index].contains(path) {
                return Err(RuleBundleError::Entrypoints.into());
            }
            if !bundle.rules.iter().any(|member| member.path == *path) {
                return Err(RuleBundleError::MissingEntrypoint.into());
            }
            root.push_str(&format!("Include {index}:{path}\n"));
        }
        for (member_index, member) in bundle.rules.iter().chain(bundle.phrase_assets).enumerate() {
            if !valid_bundle_path(member.path) {
                return Err(RuleBundleError::Path.into());
            }
            if bundle
                .rules
                .iter()
                .chain(bundle.phrase_assets)
                .take(member_index)
                .any(|previous| previous.path == member.path)
            {
                return Err(RuleBundleError::DuplicateMember.into());
            }
            if bundle.rules.iter().any(|rule| rule.path == member.path) {
                if member.bytes.len() > limits.includes.max_source_bytes {
                    return Err(RuleBundleError::Limit.into());
                }
            } else if member.bytes.len() > limits.compiler.max_asset_bytes {
                return Err(RuleBundleError::Limit.into());
            }
            members = members.checked_add(1).ok_or(RuleBundleError::Limit)?;
            source_bytes = source_bytes
                .checked_add(member.bytes.len())
                .ok_or(RuleBundleError::Limit)?;
        }
    }
    if members > limits.includes.max_sources || source_bytes > limits.includes.max_expanded_bytes {
        return Err(RuleBundleError::Limit.into());
    }
    if root.len() > limits.includes.max_source_bytes {
        return Err(RuleBundleError::Limit.into());
    }
    let mut resolver = BundleIncludes { bundles };
    let mut expanded = expand_includes("bundle-root", root.as_bytes(), &mut resolver, limits.includes, true)?;
    if !inline_rules.is_empty() {
        let inline = expand_includes(
            "inline",
            inline_rules.as_bytes(),
            &mut DenyIncludes,
            limits.includes,
            false,
        )?;
        if !expanded.bytes.is_empty() && !expanded.bytes.ends_with(b"\n") {
            expanded.bytes.push(b'\n');
        }
        let total = expanded
            .bytes
            .len()
            .checked_add(inline.bytes.len())
            .ok_or(RuleBundleError::Limit)?;
        if total > limits.includes.max_expanded_bytes {
            return Err(RuleBundleError::Limit.into());
        }
        expanded.bytes.extend_from_slice(&inline.bytes);
        expanded.metrics.expanded_bytes = total;
    }
    let source = scan_source("bundle-root", &expanded.bytes, limits.source)?;
    let syntax = parse_syntax(&source, limits.syntax)?;
    let draft = analyze_one_semantic_draft(syntax, limits.semantic)?;
    let compatibility = CompatibilityReport::from_draft(&draft, expanded.metrics, limits.compatibility)?;
    let validated = validate_semantic_draft(draft, context, limits.validation)?;
    let mut assets = BundleAssets {
        bundles,
        provenance: &expanded.provenance,
    };
    let generation = compile_validated(validated, Some(&mut assets), limits.compiler)?;
    Ok(CompiledBundle {
        generation,
        compatibility,
    })
}

#[derive(Debug)]
pub struct CompiledBundle {
    generation: CompiledGeneration,
    compatibility: CompatibilityReport,
}

impl CompiledBundle {
    pub const fn generation(&self) -> &CompiledGeneration {
        &self.generation
    }

    pub const fn compatibility(&self) -> &CompatibilityReport {
        &self.compatibility
    }

    pub fn into_parts(self) -> (CompiledGeneration, CompatibilityReport) {
        (self.generation, self.compatibility)
    }
}

pub fn inspect_compatibility(
    root_identity: &str,
    root: &[u8],
    includes: &mut dyn IncludeSource,
    limits: FrontendLimits,
) -> Result<CompatibilityReport, FrontendError> {
    let expanded = expand_includes(root_identity, root, includes, limits.includes, false)?;
    let source = scan_source(root_identity, &expanded.bytes, limits.source)?;
    let syntax = parse_syntax(&source, limits.syntax)?;
    let draft = analyze_one_semantic_draft(syntax, limits.semantic)?;
    CompatibilityReport::from_draft(&draft, expanded.metrics, limits.compatibility).map_err(FrontendError::from)
}

pub fn compile_with_includes(
    root_identity: &str,
    root: &[u8],
    includes: &mut dyn IncludeSource,
    phrase_assets: Option<&mut dyn PhraseAssetSource>,
    context: SemanticValidationContext,
    limits: FrontendLimits,
) -> Result<CompiledBundle, FrontendError> {
    let expanded = expand_includes(root_identity, root, includes, limits.includes, false)?;
    let source = scan_source(root_identity, &expanded.bytes, limits.source)?;
    let syntax = parse_syntax(&source, limits.syntax)?;
    let draft = analyze_one_semantic_draft(syntax, limits.semantic)?;
    let compatibility = CompatibilityReport::from_draft(&draft, expanded.metrics, limits.compatibility)?;
    let validated = validate_semantic_draft(draft, context, limits.validation)?;
    let generation = compile_validated(validated, phrase_assets, limits.compiler)?;
    Ok(CompiledBundle {
        generation,
        compatibility,
    })
}

/// Compile only rule units with no locally attributed unavailable capability.
///
/// This entry point exists for conformance measurement. Production publication
/// must use [`compile_with_includes`], which rejects every unavailable
/// capability. The returned compatibility report is the authoritative list of
/// excluded rule IDs and reasons.
pub fn compile_executable_profile_with_includes(
    root_identity: &str,
    root: &[u8],
    includes: &mut dyn IncludeSource,
    phrase_assets: Option<&mut dyn PhraseAssetSource>,
    context: SemanticValidationContext,
    limits: FrontendLimits,
) -> Result<CompiledBundle, FrontendError> {
    let expanded = expand_includes(root_identity, root, includes, limits.includes, false)?;
    let source = scan_source(root_identity, &expanded.bytes, limits.source)?;
    let syntax = parse_syntax(&source, limits.syntax)?;
    let draft = analyze_one_semantic_draft(syntax, limits.semantic)?;
    let compatibility = CompatibilityReport::from_draft(&draft, expanded.metrics, limits.compatibility)?;
    if compatibility.unattributed_findings() != 0 {
        return Err(CompatibilityError::UnattributedRule.into());
    }
    if compatibility.omitted_rule_summaries() != 0 {
        return Err(CompatibilityError::RuleSummaryLimit {
            limit: limits.compatibility.max_rule_summaries,
        }
        .into());
    }
    let validated = validate_executable_profile(draft, context, limits.validation)?;
    let generation = compile_validated(validated, phrase_assets, limits.compiler)?;
    Ok(CompiledBundle {
        generation,
        compatibility,
    })
}

#[cfg(test)]
#[path = "../../tests/unit/seclang/include_test.rs"]
mod tests;
