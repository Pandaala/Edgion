use thiserror::Error;

use super::{
    analyze_one_semantic_draft, compile_validated, parse_syntax, scan_source, validate_executable_profile,
    validate_semantic_draft, CompiledGeneration, CompilerError, CompilerLimits, FinalRuleRole, PhraseAssetSource,
    SemanticError, SemanticLimits, SemanticLocation, SemanticValidationContext, SemanticValidationError, SourceError,
    SourceLimits, SyntaxError, SyntaxLimits, UnavailableCapability, ValidationLimits,
};

#[cfg(test)]
use super::{PhraseAssetFailure, PhraseAssetRequest, PhraseAssetSink};

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
                self.append(physical.as_bytes())?;
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
}

fn expand_includes(
    root_identity: &str,
    root: &[u8],
    resolver: &mut dyn IncludeSource,
    limits: IncludeLimits,
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
        UnavailableCapability::LibInjection => 3,
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
    unavailable_counts: [usize; 4],
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
        let mut unavailable_counts = [0_usize; 4];
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
        self.unavailable_counts[0] == 0
            && self.unavailable_counts[1] == 0
            && self.unavailable_counts[2] == 0
            && self.unavailable_counts[3] == 0
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
    let expanded = expand_includes(root_identity, root, includes, limits.includes)?;
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
    let expanded = expand_includes(root_identity, root, includes, limits.includes)?;
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
    let expanded = expand_includes(root_identity, root, includes, limits.includes)?;
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
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::seclang::{Phase, PhaseDefaults};

    type FixtureSource = (&'static str, &'static [u8]);
    type FixtureEntry = (&'static str, Vec<FixtureSource>);

    struct FixtureResolver {
        entries: Vec<FixtureEntry>,
        calls: usize,
        fail: Option<IncludeFailure>,
        ignore_sink_errors: bool,
    }

    impl IncludeSource for FixtureResolver {
        fn load(&mut self, request: IncludeRequest<'_>, sink: &mut IncludeSink<'_>) -> Result<(), IncludeFailure> {
            self.calls += 1;
            assert!(!format!("{request:?}").contains(request.reference()));
            if let Some(failure) = self.fail {
                return Err(failure);
            }
            let Some((_, sources)) = self
                .entries
                .iter()
                .find(|(reference, _)| *reference == request.reference())
            else {
                return Err(IncludeFailure::NotFound);
            };
            for (identity, bytes) in sources {
                let result = sink.push_source(identity, bytes);
                if !self.ignore_sink_errors {
                    result.map_err(|_| IncludeFailure::Other)?;
                }
            }
            Ok(())
        }
    }

    fn resolver(entries: Vec<FixtureEntry>) -> FixtureResolver {
        FixtureResolver {
            entries,
            calls: 0,
            fail: None,
            ignore_sink_errors: false,
        }
    }

    fn context() -> SemanticValidationContext {
        let defaults = [
            Phase::RequestHeaders,
            Phase::RequestBody,
            Phase::ResponseHeaders,
            Phase::ResponseBody,
            Phase::Logging,
        ]
        .map(|phase| PhaseDefaults::pass(phase, true, true));
        SemanticValidationContext::try_new(Some(Phase::RequestBody), &defaults).unwrap()
    }

    #[test]
    fn include_expansion_preserves_depth_first_multi_source_rule_order() {
        let root = concat!(
            "SecRule ARGS \"@eq 1\" \"id:1,phase:2,pass\"\n",
            "Include 'bundle' # expansion\n",
            "SecRule ARGS \"@eq 5\" \"id:5,phase:2,pass\"\n",
        );
        let mut includes = resolver(vec![
            (
                "bundle",
                vec![
                    (
                        "a.conf",
                        b"SecRule ARGS \"@eq 2\" \"id:2,phase:2,pass\"\nInclude nested\n",
                    ),
                    ("b.conf", b"SecRule ARGS \"@eq 4\" \"id:4,phase:2,pass\"\n"),
                ],
            ),
            (
                "nested",
                vec![("nested.conf", b"SecRule ARGS \"@eq 3\" \"id:3,phase:2,pass\"\n")],
            ),
        ]);
        let bundle = compile_with_includes(
            "root.conf",
            root.as_bytes(),
            &mut includes,
            None,
            context(),
            FrontendLimits::default(),
        )
        .unwrap();
        assert_eq!(
            bundle
                .generation()
                .rule_units()
                .map(|unit| unit.id())
                .collect::<Vec<_>>(),
            [1, 2, 3, 4, 5]
        );
        assert_eq!(includes.calls, 2);
        assert_eq!(
            bundle.compatibility().include_metrics(),
            IncludeMetrics {
                source_count: 3,
                source_bytes: 138,
                expanded_bytes: 205,
                maximum_depth: 2,
            }
        );
        assert!(bundle.compatibility().is_supported());
    }

    #[test]
    fn optional_missing_include_is_empty_but_required_missing_is_an_error() {
        let mut optional = resolver(Vec::new());
        let report = inspect_compatibility(
            "root",
            b"IncludeOptional missing\nSecAction \"id:1,phase:2,pass\"\n",
            &mut optional,
            FrontendLimits::default(),
        )
        .unwrap();
        assert_eq!(report.rule_count(), 1);
        let mut required = resolver(Vec::new());
        assert!(matches!(
            inspect_compatibility("root", b"Include missing\n", &mut required, FrontendLimits::default(),),
            Err(FrontendError::Include(IncludeError::Source {
                failure: IncludeFailure::NotFound,
                ..
            }))
        ));
    }

    #[test]
    fn include_cycles_depth_count_and_identity_work_are_bounded() {
        let mut cycle = resolver(vec![
            ("a", vec![("a.conf", b"Include b\n")]),
            ("b", vec![("b.conf", b"Include a\n")]),
        ]);
        assert!(matches!(
            inspect_compatibility("root", b"Include a\n", &mut cycle, FrontendLimits::default(),),
            Err(FrontendError::Include(IncludeError::Cycle { depth: 3 }))
        ));

        let mut depth = resolver(vec![
            ("a", vec![("a.conf", b"Include b\n")]),
            ("b", vec![("b.conf", b"SecAction \"id:1,phase:2,pass\"\n")]),
        ]);
        assert!(matches!(
            inspect_compatibility(
                "root",
                b"Include a\n",
                &mut depth,
                FrontendLimits {
                    includes: IncludeLimits {
                        max_depth: 1,
                        ..IncludeLimits::default()
                    },
                    ..FrontendLimits::default()
                },
            ),
            Err(FrontendError::Include(IncludeError::DepthLimit { actual: 2, limit: 1 }))
        ));

        let mut count = resolver(vec![("many", vec![("a.conf", b""), ("b.conf", b"")])]);
        assert!(matches!(
            inspect_compatibility(
                "root",
                b"Include many\n",
                &mut count,
                FrontendLimits {
                    includes: IncludeLimits {
                        max_sources: 1,
                        ..IncludeLimits::default()
                    },
                    ..FrontendLimits::default()
                },
            ),
            Err(FrontendError::Include(IncludeError::Sink {
                error: IncludeSinkError::SourceLimit { limit: 1 },
                ..
            }))
        ));

        let mut identity = resolver(vec![("a", vec![("child", b"")])]);
        assert!(matches!(
            inspect_compatibility(
                "root",
                b"Include a\n",
                &mut identity,
                FrontendLimits {
                    includes: IncludeLimits {
                        max_identity_comparison_bytes: 0,
                        ..IncludeLimits::default()
                    },
                    ..FrontendLimits::default()
                },
            ),
            Err(FrontendError::Include(IncludeError::IdentityComparisonLimit {
                limit: 0
            }))
        ));
    }

    #[test]
    fn include_source_and_expanded_byte_limits_are_exact() {
        let child = b"SecAction \"id:1,phase:2,pass\"\n";
        let mut exact = resolver(vec![("child", vec![("child.conf", child)])]);
        inspect_compatibility(
            "root",
            b"Include child\n",
            &mut exact,
            FrontendLimits {
                includes: IncludeLimits {
                    max_source_bytes: child.len(),
                    max_expanded_bytes: child.len(),
                    ..IncludeLimits::default()
                },
                ..FrontendLimits::default()
            },
        )
        .unwrap();
        let mut excess = resolver(vec![("child", vec![("child.conf", child)])]);
        assert!(matches!(
            inspect_compatibility(
                "root",
                b"Include child\n",
                &mut excess,
                FrontendLimits {
                    includes: IncludeLimits {
                        max_source_bytes: child.len() - 1,
                        ..IncludeLimits::default()
                    },
                    ..FrontendLimits::default()
                },
            ),
            Err(FrontendError::Include(IncludeError::Sink {
                error: IncludeSinkError::SourceByteLimit { .. },
                ..
            }))
        ));

        let mut expanded = resolver(vec![("child", vec![("child.conf", child)])]);
        assert!(matches!(
            inspect_compatibility(
                "root",
                b"Include child\n",
                &mut expanded,
                FrontendLimits {
                    includes: IncludeLimits {
                        max_source_bytes: child.len(),
                        max_expanded_bytes: child.len() - 1,
                        ..IncludeLimits::default()
                    },
                    ..FrontendLimits::default()
                },
            ),
            Err(FrontendError::Include(IncludeError::Sink {
                error: IncludeSinkError::AggregateByteLimit { .. },
                ..
            }))
        ));
    }

    #[test]
    fn multi_source_boundaries_insert_one_separator_when_needed() {
        let mut includes = resolver(vec![(
            "bundle",
            vec![
                ("a.conf", b"SecAction \"id:1,phase:2,pass\""),
                ("b.conf", b"SecAction \"id:2,phase:2,pass\""),
            ],
        )]);
        let bundle = compile_with_includes(
            "root",
            b"Include bundle\n",
            &mut includes,
            None,
            context(),
            FrontendLimits::default(),
        )
        .unwrap();
        assert_eq!(
            bundle
                .generation()
                .rule_units()
                .map(|unit| unit.id())
                .collect::<Vec<_>>(),
            [1, 2]
        );
    }

    #[test]
    fn sticky_sink_failure_precedes_provider_failure_and_redacts_names() {
        let mut includes = FixtureResolver {
            entries: vec![(
                "private-reference",
                vec![(
                    "private-identity",
                    b"this included source is deliberately larger than the root source",
                )],
            )],
            calls: 0,
            fail: None,
            ignore_sink_errors: true,
        };
        let error = inspect_compatibility(
            "private-root",
            b"Include private-reference\n",
            &mut includes,
            FrontendLimits {
                includes: IncludeLimits {
                    max_source_bytes: 26,
                    ..IncludeLimits::default()
                },
                ..FrontendLimits::default()
            },
        )
        .unwrap_err();
        let rendered = format!("{error:?} {error}");
        assert!(
            matches!(
                &error,
                FrontendError::Include(IncludeError::Sink {
                    error: IncludeSinkError::SourceByteLimit { .. },
                    ..
                })
            ),
            "{error:?}"
        );
        assert!(!rendered.contains("private"));
    }

    #[test]
    fn compatibility_report_is_bounded_and_compilation_remains_fail_closed() {
        let source = concat!(
            "SecRule RESPONSE_BODY \"@rx private-pattern\" \"id:1,phase:4,pass\"\n",
            "SecRule RESPONSE_BODY \"@rx another-pattern\" \"id:2,phase:4,pass\"\n",
        );
        let mut inspect_resolver = resolver(Vec::new());
        let report = inspect_compatibility(
            "private-root",
            source.as_bytes(),
            &mut inspect_resolver,
            FrontendLimits {
                compatibility: CompatibilityLimits {
                    max_findings: 1,
                    max_rule_summaries: 1,
                },
                ..FrontendLimits::default()
            },
        )
        .unwrap();
        assert!(!report.is_supported());
        assert_eq!(report.unavailable_count(UnavailableCapability::FullResponseBody), 2);
        assert_eq!(report.findings().len(), 1);
        assert_eq!(report.omitted_findings(), 1);
        assert_eq!(report.rule_summaries().len(), 1);
        assert_eq!(report.omitted_rule_summaries(), 1);
        let rendered = format!("{report:?}");
        assert!(!rendered.contains("private-pattern"));
        assert!(!rendered.contains("private-root"));

        let mut compile_resolver = resolver(Vec::new());
        assert!(matches!(
            compile_with_includes(
                "private-root",
                source.as_bytes(),
                &mut compile_resolver,
                None,
                context(),
                FrontendLimits::default(),
            ),
            Err(FrontendError::Validation(
                SemanticValidationError::UnavailableCapability { .. }
            ))
        ));

        let mut profile_resolver = resolver(Vec::new());
        assert!(matches!(
            compile_executable_profile_with_includes(
                "private-root",
                source.as_bytes(),
                &mut profile_resolver,
                None,
                context(),
                FrontendLimits {
                    compatibility: CompatibilityLimits {
                        max_rule_summaries: 1,
                        ..CompatibilityLimits::default()
                    },
                    ..FrontendLimits::default()
                },
            ),
            Err(FrontendError::Compatibility(CompatibilityError::RuleSummaryLimit {
                limit: 1
            }))
        ));
    }

    #[test]
    fn requirement_discharge_is_location_keyed_across_compilation_stages() {
        let source = concat!(
            "SecRule REQUEST_URI \"@rx grouped\" \"id:1,phase:1,pass\"\n",
            "SecRule REQUEST_URI \"@rx (captured)\" \"id:2,phase:1,pass,capture\"\n",
        );
        let mut includes = resolver(Vec::new());
        let bundle = compile_with_includes(
            "requirements",
            source.as_bytes(),
            &mut includes,
            None,
            context(),
            FrontendLimits::default(),
        )
        .unwrap();

        assert_eq!(
            bundle
                .generation()
                .rule_units()
                .map(|unit| unit.id())
                .collect::<Vec<_>>(),
            [1, 2]
        );
    }

    #[test]
    fn executable_profile_is_explicit_attributed_and_cannot_weaken_strict_compilation() {
        let source = concat!(
            "SecRule REQUEST_URI \"@contains allowed\" \"id:1,phase:1,pass\"\n",
            "SecRule RESPONSE_BODY \"@contains private\" \"id:2,phase:4,pass\"\n",
            "SecRule ARGS \"@detectSQLi\" \"id:3,phase:2,pass\"\n",
            "SecAction \"id:4,phase:1,pass,ctl:auditEngine=Off\"\n",
            "SecAction \"id:5,phase:1,pass,initcol:global=global\"\n",
        );
        let mut strict_includes = resolver(Vec::new());
        assert!(matches!(
            compile_with_includes(
                "profile",
                source.as_bytes(),
                &mut strict_includes,
                None,
                context(),
                FrontendLimits::default(),
            ),
            Err(FrontendError::Validation(
                SemanticValidationError::UnavailableCapability { .. }
            ))
        ));

        let mut profile_includes = resolver(Vec::new());
        let bundle = compile_executable_profile_with_includes(
            "profile",
            source.as_bytes(),
            &mut profile_includes,
            None,
            context(),
            FrontendLimits::default(),
        )
        .unwrap();
        assert_eq!(
            bundle
                .generation()
                .rule_units()
                .map(|unit| unit.id())
                .collect::<Vec<_>>(),
            [1]
        );
        assert_eq!(
            (
                bundle.compatibility().rule_unit_count(),
                bundle.compatibility().unavailable_rule_unit_count(),
                bundle.compatibility().locally_executable_rule_unit_count(),
            ),
            (5, 4, 1)
        );
        assert_eq!(
            bundle
                .compatibility()
                .rule_summaries()
                .map(CompatibilityRuleSummary::rule_id)
                .collect::<Vec<_>>(),
            [2, 3, 4, 5]
        );
    }

    #[test]
    fn malformed_include_and_continued_text_do_not_escape_the_parser_contract() {
        let mut malformed = resolver(Vec::new());
        assert!(matches!(
            inspect_compatibility(
                "root",
                b"Include 'unterminated\n",
                &mut malformed,
                FrontendLimits::default(),
            ),
            Err(FrontendError::Include(IncludeError::Syntax { line: 1 }))
        ));

        let mut continued = resolver(Vec::new());
        let error = inspect_compatibility(
            "root",
            b"SecAction \"id:1,phase:2,pass\" \\\nInclude should-not-load\n",
            &mut continued,
            FrontendLimits::default(),
        )
        .unwrap_err();
        assert_eq!(continued.calls, 0);
        assert!(matches!(error, FrontendError::Syntax(_) | FrontendError::Semantic(_)));
    }

    #[test]
    fn pinned_crs_compatibility_report_matches_the_frozen_inventory() {
        let Some(root) = std::env::var_os("EDGION_WAF_CRS_RULES_DIR") else {
            return;
        };
        struct OwnedResolver {
            entries: Vec<(String, Vec<u8>)>,
        }
        impl IncludeSource for OwnedResolver {
            fn load(&mut self, request: IncludeRequest<'_>, sink: &mut IncludeSink<'_>) -> Result<(), IncludeFailure> {
                let Some((identity, bytes)) = self
                    .entries
                    .iter()
                    .find(|(identity, _)| identity == request.reference())
                else {
                    return Err(IncludeFailure::NotFound);
                };
                sink.push_source(identity, bytes).map_err(|_| IncludeFailure::Other)
            }
        }
        struct OwnedAssets {
            root: PathBuf,
        }
        impl PhraseAssetSource for OwnedAssets {
            fn load(
                &mut self,
                request: PhraseAssetRequest<'_>,
                sink: &mut PhraseAssetSink<'_>,
            ) -> Result<(), PhraseAssetFailure> {
                let handle = PathBuf::from(request.handle());
                if handle.components().count() != 1 {
                    return Err(PhraseAssetFailure::Denied);
                }
                let bytes = fs::read(self.root.join(handle)).map_err(|_| PhraseAssetFailure::NotFound)?;
                sink.extend_from_slice(&bytes).map_err(|_| PhraseAssetFailure::Other)
            }
        }

        let root = PathBuf::from(root);
        let mut paths = fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "conf"))
            .collect::<Vec<_>>();
        paths.sort();
        let mut include_root = String::new();
        let mut entries = Vec::new();
        for path in paths {
            let identity = path.strip_prefix(&root).unwrap().to_string_lossy().into_owned();
            include_root.push_str("Include '");
            include_root.push_str(&identity);
            include_root.push_str("'\n");
            entries.push((identity, fs::read(path).unwrap()));
        }
        let mut includes = OwnedResolver {
            entries: entries.clone(),
        };
        let report = inspect_compatibility(
            "crs-rules",
            include_root.as_bytes(),
            &mut includes,
            FrontendLimits::default(),
        )
        .unwrap();
        assert_eq!(
            [
                report.unavailable_count(UnavailableCapability::FullResponseBody),
                report.unavailable_count(UnavailableCapability::Ctl),
                report.unavailable_count(UnavailableCapability::InitCol),
                report.unavailable_count(UnavailableCapability::LibInjection),
            ],
            [59, 18, 2, 4]
        );
        assert_eq!(report.findings().len(), 83);
        assert_eq!(report.omitted_findings(), 0);
        assert_eq!(
            (
                report.rule_unit_count(),
                report.unavailable_rule_unit_count(),
                report.locally_executable_rule_unit_count(),
                report.unattributed_findings(),
            ),
            (627, 72, 555, 0)
        );
        let unavailable_rule_counts = [
            UnavailableCapability::FullResponseBody,
            UnavailableCapability::Ctl,
            UnavailableCapability::InitCol,
            UnavailableCapability::LibInjection,
        ]
        .map(|capability| {
            report
                .findings()
                .filter(|finding| finding.capability() == capability)
                .filter_map(CompatibilityFinding::rule_id)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
        });
        assert_eq!(unavailable_rule_counts, [59, 8, 1, 4]);
        assert_eq!(report.rule_summaries().len(), 72);
        assert_eq!(report.omitted_rule_summaries(), 0);
        for summary in report.rule_summaries() {
            assert!([
                UnavailableCapability::FullResponseBody,
                UnavailableCapability::Ctl,
                UnavailableCapability::InitCol,
                UnavailableCapability::LibInjection,
            ]
            .into_iter()
            .any(|capability| summary.has_capability(capability)));
        }
        assert!(!report.is_supported());

        let mut includes = OwnedResolver { entries };
        let mut assets = OwnedAssets { root: root.clone() };
        let profile = compile_executable_profile_with_includes(
            "crs-rules",
            include_root.as_bytes(),
            &mut includes,
            Some(&mut assets),
            context(),
            FrontendLimits::default(),
        )
        .unwrap();
        assert_eq!(profile.generation().rule_unit_count(), 555);
        assert_eq!(profile.compatibility(), &report);
    }
}
