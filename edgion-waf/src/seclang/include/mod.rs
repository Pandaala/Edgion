use thiserror::Error;

use super::{
    analyze_one_semantic_draft, compile_validated, parse_syntax, scan_source, validate_executable_profile,
    validate_semantic_draft, CompiledGeneration, CompilerError, CompilerLimits, FinalRuleRole, PhraseAssetSource,
    SemanticDraft, SemanticError, SemanticLimits, SemanticLocation, SemanticValidationContext, SemanticValidationError,
    SourceDocument, SourceError, SourceLimits, SyntaxError, SyntaxLimits, UnavailableCapability, ValidationLimits,
};

use super::{PhraseAssetFailure, PhraseAssetRequest, PhraseAssetSink};

mod compatibility;

pub use compatibility::CompatibilityReport;
use compatibility::*;

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

/// Byte-comparison budget for bundle path and identity equality checks, charged
/// against `IncludeLimits::max_identity_comparison_bytes` before each compare.
/// `str` equality compares lengths first, so only an equal-length pair costs
/// byte work and is charged its full length.
struct PathComparisonBudget {
    used: usize,
    limit: usize,
    exceeded: bool,
}

impl PathComparisonBudget {
    const fn new(limits: IncludeLimits) -> Self {
        Self {
            used: 0,
            limit: limits.max_identity_comparison_bytes,
            exceeded: false,
        }
    }

    fn error(&self) -> IncludeError {
        IncludeError::IdentityComparisonLimit { limit: self.limit }
    }

    fn equal(&mut self, left: &str, right: &str) -> Result<bool, IncludeError> {
        if left.len() != right.len() {
            return Ok(false);
        }
        match self.used.checked_add(left.len()) {
            Some(used) if used <= self.limit => self.used = used,
            _ => {
                self.exceeded = true;
                return Err(self.error());
            }
        }
        Ok(left == right)
    }

    /// Charged linear search; `Err` only when the budget is exhausted.
    fn find<'m, 'a>(
        &mut self,
        members: &'m [RuleBundleMember<'a>],
        path: &str,
    ) -> Result<Option<&'m RuleBundleMember<'a>>, IncludeError> {
        for member in members {
            if self.equal(member.path, path)? {
                return Ok(Some(member));
            }
        }
        Ok(None)
    }

    /// Replace a provider-side failure with the budget error once exhausted.
    fn map_exhausted<E: Into<FrontendError>>(&self, error: E) -> FrontendError {
        if self.exceeded {
            self.error().into()
        } else {
            error.into()
        }
    }
}

struct BundleIncludes<'a> {
    bundles: &'a [RuleBundleSource<'a>],
    comparisons: PathComparisonBudget,
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
        let member = self
            .comparisons
            .find(bundle.rules, &path)
            .map_err(|_| IncludeFailure::Other)?
            .ok_or(IncludeFailure::Denied)?;
        let identity = format!("{index}:{path}");
        sink.push_source(&identity, member.bytes)
            .map_err(|_| IncludeFailure::Other)
    }
}

struct BundleAssets<'a> {
    bundles: &'a [RuleBundleSource<'a>],
    provenance: &'a [ExpandedSegment],
    comparisons: PathComparisonBudget,
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
        let member = self
            .comparisons
            .find(bundle.phrase_assets, &path)
            .map_err(|_| PhraseAssetFailure::Other)?
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
    compile_rule_bundles_with_external_budget(bundles, inline_rules, context, limits, 0)
}

/// Single-pass strict compilation charging a coexisting configuration owner, without selection inventory.
pub fn compile_rule_bundles_with_external_budget(
    bundles: &[RuleBundleSource<'_>],
    inline_rules: &str,
    context: SemanticValidationContext,
    limits: FrontendLimits,
    external_live_bytes: usize,
) -> Result<CompiledBundle, FrontendError> {
    if external_live_bytes > limits.compiler.max_compilation_peak_bytes {
        return Err(CompilerError::PeakByteLimit {
            limit: limits.compiler.max_compilation_peak_bytes,
        }
        .into());
    }
    let expanded = prepare_rule_bundles(bundles, inline_rules, limits)?;
    let source = scan_source("bundle-root", &expanded.bytes, limits.source)?;
    let (draft, compatibility) = frontend_draft(&source, &expanded, limits, None)?;
    let validated = validate_semantic_draft(draft, context, limits.validation)?;
    let mut assets = BundleAssets {
        bundles,
        provenance: &expanded.provenance,
        comparisons: PathComparisonBudget::new(limits.includes),
    };
    let generation = if external_live_bytes == 0 {
        compile_validated(validated, Some(&mut assets), limits.compiler)
            .map_err(|error| assets.comparisons.map_exhausted(error))?
    } else {
        let input_bytes = external_live_bytes
            .checked_add(expanded_owned_bytes(&expanded)?)
            .and_then(|bytes| bytes.checked_add(compatibility_owned_bytes(&compatibility).ok()?))
            .ok_or(CompilerError::PreflightArithmetic)?;
        super::compiler::compile_validated_with_budget(
            validated,
            Some(&mut assets),
            limits.compiler,
            input_bytes,
            &mut super::compiler::CanonicalWork::default(),
        )
        .map_err(|error| assets.comparisons.map_exhausted(error))?
    };
    Ok(CompiledBundle {
        generation,
        compatibility,
    })
}

/// Peak-byte accounting between front-end stages; only the selection path supplies one.
struct PeakAccount {
    bytes: usize,
    limit: usize,
}

impl PeakAccount {
    fn error(&self) -> FrontendError {
        FrontendError::Compiler(CompilerError::PeakByteLimit { limit: self.limit })
    }

    /// Add retained bytes without checking the limit; overflow is a peak failure.
    fn charge(&mut self, add: usize) -> Result<(), FrontendError> {
        self.bytes = self.bytes.checked_add(add).ok_or_else(|| self.error())?;
        Ok(())
    }

    /// Fail when the retained bytes plus a transient `extra` exceed the limit.
    fn check(&self, extra: usize) -> Result<(), FrontendError> {
        match self.bytes.checked_add(extra) {
            Some(bytes) if bytes <= self.limit => Ok(()),
            _ => Err(self.error()),
        }
    }
}

/// Shared front-end stages after `scan_source`: syntax, semantic draft, compatibility report.
/// Only a caller that supplies `peak` gets the inter-stage check; other paths keep none.
fn frontend_draft<'s>(
    source: &'s SourceDocument,
    expanded: &ExpandedSource,
    limits: FrontendLimits,
    peak: Option<&mut PeakAccount>,
) -> Result<(SemanticDraft<'s>, CompatibilityReport), FrontendError> {
    let syntax = parse_syntax(source, limits.syntax)?;
    let draft = analyze_one_semantic_draft(syntax, limits.semantic)?;
    if let Some(peak) = peak {
        peak.check(draft.retained_charged_bytes())?;
    }
    let compatibility = CompatibilityReport::from_draft(&draft, expanded.metrics, limits.compatibility)?;
    Ok((draft, compatibility))
}

fn expanded_owned_bytes(expanded: &ExpandedSource) -> Result<usize, CompilerError> {
    let mut bytes = expanded
        .bytes
        .capacity()
        .checked_add(
            expanded
                .provenance
                .capacity()
                .checked_mul(std::mem::size_of::<ExpandedSegment>())
                .ok_or(CompilerError::PreflightArithmetic)?,
        )
        .ok_or(CompilerError::PreflightArithmetic)?;
    for segment in &expanded.provenance {
        bytes = bytes
            .checked_add(segment.identity.capacity())
            .ok_or(CompilerError::PreflightArithmetic)?;
    }
    Ok(bytes)
}
fn compatibility_owned_bytes(report: &CompatibilityReport) -> Result<usize, CompilerError> {
    std::mem::size_of::<CompatibilityReport>()
        .checked_add(
            report
                .rule_summaries
                .capacity()
                .checked_mul(std::mem::size_of::<CompatibilityRuleSummary>())
                .ok_or(CompilerError::PreflightArithmetic)?,
        )
        .and_then(|bytes| {
            bytes.checked_add(
                report
                    .findings
                    .capacity()
                    .checked_mul(std::mem::size_of::<CompatibilityFinding>())?,
            )
        })
        .ok_or(CompilerError::PreflightArithmetic)
}

/// A strict compilation or the consuming plugin's body-admission failure.
#[derive(Debug)]
pub enum WafSelectionCompileError<E> {
    Frontend(FrontendError),
    Admission(E),
}

impl<E> From<FrontendError> for WafSelectionCompileError<E> {
    fn from(error: FrontendError) -> Self {
        Self::Frontend(error)
    }
}

/// Local inspection hook. Implementations must retain only bounded metadata.
/// This hook runs before compilation, so inventory is not an admission certificate.
pub trait WafRuleSelectionObserver {
    fn inventory(
        &mut self,
        document: &super::ValidatedSemanticDocument<'_>,
        preview: WafRuleSelectionPreview<'_>,
        budget: &mut WafRuleMetadataBudget<'_>,
    ) -> Result<(), CompilerError>;
    fn compilation_started(&mut self, _baseline: bool) {}
    fn compilation_passed(&mut self, _baseline: bool) {}
}

/// Metadata inspection shares the compiler's cumulative work budget.
pub struct WafRuleMetadataBudget<'a> {
    work: &'a mut super::compiler::CanonicalWork,
    limits: CompilerLimits,
}
impl WafRuleMetadataBudget<'_> {
    pub fn charge_records(&mut self, count: usize) -> Result<(), CompilerError> {
        self.work.charge_records(count, self.limits)
    }
    pub fn charge_bytes(&mut self, count: usize) -> Result<(), CompilerError> {
        self.work.charge_comparison_bytes(count, self.limits)
    }
}

#[derive(Clone, Copy)]
pub struct WafRuleSelectionPreview<'a> {
    pub selected: &'a [bool],
    pub summary: super::WafSelectionSummary,
    pub raw_atom_matches: &'a [usize],
    pub active_atom_matches: &'a [usize],
}

/// Compile and admit the complete baseline before removing any executable activity.
/// The baseline is released before the selected pass. No source text is rewritten.
// Public API consumed by edgion-resources; sources, limits, selection and callbacks are independent.
#[allow(clippy::too_many_arguments)]
pub fn compile_rule_bundles_with_selection<E>(
    bundles: &[RuleBundleSource<'_>],
    inline_rules: &str,
    context: SemanticValidationContext,
    limits: FrontendLimits,
    selection: &super::PreparedWafRuleSelection,
    selection_live_bytes: usize,
    mut admit: impl FnMut(&CompiledGeneration) -> Result<(), E>,
    mut observer: Option<&mut dyn WafRuleSelectionObserver>,
) -> Result<(CompiledBundle, super::WafSelectionSummary), WafSelectionCompileError<E>> {
    use super::compiler::{compile_validated_with_budget, CanonicalWork};
    let mut peak = PeakAccount {
        bytes: selection_live_bytes,
        limit: limits.compiler.max_compilation_peak_bytes,
    };
    peak.charge(selection.owned_bytes())?;
    peak.check(0)?;
    let expanded = prepare_rule_bundles(bundles, inline_rules, limits)?;
    peak.charge(expanded.bytes.capacity())?;
    peak.charge(
        expanded
            .provenance
            .capacity()
            .checked_mul(std::mem::size_of::<ExpandedSegment>())
            .ok_or_else(|| peak.error())?,
    )?;
    for segment in &expanded.provenance {
        peak.charge(segment.identity.capacity())?;
    }
    peak.check(0)?;
    let source = scan_source("bundle-root", &expanded.bytes, limits.source).map_err(FrontendError::from)?;
    peak.check(source.charged_bytes())?;
    let (draft, compatibility) = frontend_draft(&source, &expanded, limits, Some(&mut peak))?;
    peak.charge(std::mem::size_of::<CompatibilityReport>())?;
    peak.charge(
        compatibility
            .rule_summaries
            .capacity()
            .checked_mul(std::mem::size_of::<CompatibilityRuleSummary>())
            .ok_or_else(|| peak.error())?,
    )?;
    peak.charge(
        compatibility
            .findings
            .capacity()
            .checked_mul(std::mem::size_of::<CompatibilityFinding>())
            .ok_or_else(|| peak.error())?,
    )?;
    let validated = validate_semantic_draft(draft, context, limits.validation).map_err(FrontendError::from)?;
    // Precharge mask and evaluation stack before allocating or traversing metadata.
    peak.charge(
        std::mem::size_of::<super::tag_selection::SelectionDecision>() + super::tag_selection::MAX_SELECTION_NODES,
    )?;
    peak.charge(validated.rule_units().len())?;
    peak.check(validated.retained_charged_bytes())?;
    let mut work = CanonicalWork::default();
    work.charge_records(selection.instruction_count(), limits.compiler)
        .map_err(FrontendError::from)?;
    let decision = selection
        .select(&validated, &mut work, limits.compiler)
        .map_err(FrontendError::from)?;
    if let Some(observer) = observer.as_mut() {
        observer
            .inventory(
                &validated,
                WafRuleSelectionPreview {
                    selected: &decision.mask,
                    summary: decision.summary,
                    raw_atom_matches: &decision.raw_atom_matches[..selection.atom_refs().len()],
                    active_atom_matches: &decision.active_atom_matches[..selection.atom_refs().len()],
                },
                &mut WafRuleMetadataBudget {
                    work: &mut work,
                    limits: limits.compiler,
                },
            )
            .map_err(FrontendError::from)?;
    }
    let mut assets = BundleAssets {
        bundles,
        provenance: &expanded.provenance,
        comparisons: PathComparisonBudget::new(limits.includes),
    };
    if let Some(observer) = observer.as_mut() {
        observer.compilation_started(true);
    }
    let baseline = compile_validated_with_budget(validated, Some(&mut assets), limits.compiler, peak.bytes, &mut work)
        .map_err(|error| assets.comparisons.map_exhausted(error))?;
    admit(&baseline).map_err(WafSelectionCompileError::Admission)?;
    if let Some(observer) = observer.as_mut() {
        observer.compilation_passed(true);
    }
    if decision.summary.excluded_units == 0 {
        if let Some(observer) = observer.as_mut() {
            observer.compilation_passed(false);
        }
        return Ok((
            CompiledBundle {
                generation: baseline,
                compatibility,
            },
            decision.summary,
        ));
    }
    let baseline_peak = baseline.compilation_peak_bytes();
    drop(baseline);
    // Reparse the same immutable input. Revalidate original graph before changing activity.
    let syntax = parse_syntax(&source, limits.syntax).map_err(FrontendError::from)?;
    let draft = analyze_one_semantic_draft(syntax, limits.semantic).map_err(FrontendError::from)?;
    let mut validated = validate_semantic_draft(draft, context, limits.validation).map_err(FrontendError::from)?;
    peak.check(
        validated
            .retained_charged_bytes()
            .checked_add(validated.selection_target_scratch_bytes())
            .ok_or_else(|| peak.error())?,
    )?;
    validated
        .apply_selection_mask(&decision.mask, &mut work, limits.compiler)
        .map_err(FrontendError::from)?;
    if let Some(observer) = observer.as_mut() {
        observer.compilation_started(false);
    }
    let mut generation =
        compile_validated_with_budget(validated, Some(&mut assets), limits.compiler, peak.bytes, &mut work)
            .map_err(|error| assets.comparisons.map_exhausted(error))?;
    generation.include_compilation_peak(baseline_peak);
    admit(&generation).map_err(WafSelectionCompileError::Admission)?;
    if let Some(observer) = observer.as_mut() {
        observer.compilation_passed(false);
    }
    Ok((
        CompiledBundle {
            generation,
            compatibility,
        },
        decision.summary,
    ))
}

fn prepare_rule_bundles(
    bundles: &[RuleBundleSource<'_>],
    inline_rules: &str,
    limits: FrontendLimits,
) -> Result<ExpandedSource, FrontendError> {
    if bundles.len() > limits.includes.max_sources {
        return Err(RuleBundleError::Limit.into());
    }
    // Count every member before any per-member validation or comparison scan.
    let mut members = 0_usize;
    for bundle in bundles {
        members = members
            .checked_add(bundle.rules.len())
            .and_then(|count| count.checked_add(bundle.phrase_assets.len()))
            .ok_or(RuleBundleError::Limit)?;
        if members > limits.includes.max_sources {
            return Err(RuleBundleError::Limit.into());
        }
    }
    // Every identity and path equality below is charged before it compares bytes.
    let mut comparisons = PathComparisonBudget::new(limits.includes);
    let mut root = String::new();
    let mut source_bytes = 0_usize;
    for (index, bundle) in bundles.iter().enumerate() {
        if bundle.identity.is_empty() || bundle.identity.len() > limits.includes.max_identity_bytes {
            return Err(RuleBundleError::Identity.into());
        }
        for previous in &bundles[..index] {
            if comparisons.equal(previous.identity, bundle.identity)? {
                return Err(RuleBundleError::Identity.into());
            }
        }
        if bundle.entrypoints.is_empty() {
            return Err(RuleBundleError::Entrypoints.into());
        }
        for (entry_index, path) in bundle.entrypoints.iter().enumerate() {
            if !valid_bundle_path(path) {
                return Err(RuleBundleError::Path.into());
            }
            for previous in &bundle.entrypoints[..entry_index] {
                if comparisons.equal(previous, path)? {
                    return Err(RuleBundleError::Entrypoints.into());
                }
            }
            if comparisons.find(bundle.rules, path)?.is_none() {
                return Err(RuleBundleError::MissingEntrypoint.into());
            }
            root.push_str(&format!("Include {index}:{path}\n"));
        }
        for (member_index, member) in bundle.rules.iter().chain(bundle.phrase_assets).enumerate() {
            if !valid_bundle_path(member.path) {
                return Err(RuleBundleError::Path.into());
            }
            for previous in bundle.rules.iter().chain(bundle.phrase_assets).take(member_index) {
                if comparisons.equal(previous.path, member.path)? {
                    return Err(RuleBundleError::DuplicateMember.into());
                }
            }
            // Members are unique at this point, so a rule member is exactly one
            // whose position lies in `rules`; no second path scan is needed.
            if member_index < bundle.rules.len() {
                if member.bytes.len() > limits.includes.max_source_bytes {
                    return Err(RuleBundleError::Limit.into());
                }
            } else if member.bytes.len() > limits.compiler.max_asset_bytes {
                return Err(RuleBundleError::Limit.into());
            }
            source_bytes = source_bytes
                .checked_add(member.bytes.len())
                .ok_or(RuleBundleError::Limit)?;
        }
    }
    if source_bytes > limits.includes.max_expanded_bytes {
        return Err(RuleBundleError::Limit.into());
    }
    if root.len() > limits.includes.max_source_bytes {
        return Err(RuleBundleError::Limit.into());
    }
    let mut resolver = BundleIncludes { bundles, comparisons };
    let mut expanded = expand_includes("bundle-root", root.as_bytes(), &mut resolver, limits.includes, true)
        .map_err(|error| resolver.comparisons.map_exhausted(error))?;
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
    Ok(expanded)
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
    Ok(frontend_draft(&source, &expanded, limits, None)?.1)
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
    let (draft, compatibility) = frontend_draft(&source, &expanded, limits, None)?;
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
    let (draft, compatibility) = frontend_draft(&source, &expanded, limits, None)?;
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
#[path = "../../../tests/unit/seclang/include/mod_test.rs"]
mod tests;
