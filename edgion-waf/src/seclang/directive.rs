use std::mem::size_of;

use thiserror::Error;

use super::{QuoteStyle, SourceDocument, SourcePosition, SourceSpan, Token, TokenizedDocument};

/// Default maximum number of directives in one parsed document.
pub const DEFAULT_MAX_DIRECTIVES: usize = 100_000;
/// Default maximum number of selectors in one rule-ID directive.
pub const DEFAULT_MAX_UPDATE_SELECTORS: usize = 1_024;
/// Default maximum charged bytes owned by one directive document.
pub const DEFAULT_MAX_DIRECTIVE_OWNED_BYTES: usize = 32 * 1024 * 1024;

/// Resource limits for parsing one tokenized SecLang document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectiveLimits {
    /// Maximum number of directives in the document.
    pub max_directives: usize,
    /// Maximum selectors in one rule-ID directive.
    pub max_update_selectors: usize,
    /// Maximum charged bytes owned by the returned directive document.
    pub max_owned_bytes: usize,
}

impl Default for DirectiveLimits {
    fn default() -> Self {
        Self {
            max_directives: DEFAULT_MAX_DIRECTIVES,
            max_update_selectors: DEFAULT_MAX_UPDATE_SELECTORS,
            max_owned_bytes: DEFAULT_MAX_DIRECTIVE_OWNED_BYTES,
        }
    }
}

/// A selected top-level SecLang directive kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectiveKind {
    /// `SecRule`.
    SecRule,
    /// `SecAction`.
    SecAction,
    /// `SecMarker`.
    SecMarker,
    /// `SecComponentSignature`.
    SecComponentSignature,
    /// `SecRuleUpdateTargetById`.
    SecRuleUpdateTargetById,
    /// `SecRuleRemoveById`.
    SecRuleRemoveById,
}

/// A directive field used in quote-profile diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectiveField {
    /// Directive name.
    Name,
    /// Rule target expression.
    Targets,
    /// Rule operator expression.
    Operator,
    /// Action list.
    Actions,
    /// Marker text.
    Marker,
    /// Component signature.
    Signature,
    /// Target-update ID selector.
    UpdateSelector,
    /// Target-update target expression.
    UpdateTargets,
}

/// An accepted set of top-level quote styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteRequirement {
    /// The field must be unquoted.
    Unquoted,
    /// The field must be double quoted.
    Double,
    /// The field may be unquoted or double quoted.
    UnquotedOrDouble,
}

/// The required top-level arity for a directive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectiveArity {
    /// Exactly one token count is accepted.
    Exact(usize),
    /// Either of two token counts is accepted.
    Either(usize, usize),
    /// At least this many tokens are required.
    AtLeast(usize),
}

/// Borrowed marker text, including a verbatim multiword normalized tail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkerText<'source> {
    value: &'source str,
    quote_style: QuoteStyle,
    span: SourceSpan,
}

impl<'source> MarkerText<'source> {
    /// Marker value with a single pair of top-level quotes removed.
    pub const fn value(&self) -> &'source str {
        self.value
    }

    /// Quote style of a single-token marker, or unquoted for a multiword tail.
    pub const fn quote_style(&self) -> QuoteStyle {
        self.quote_style
    }

    /// Diagnostic envelope for the marker value.
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Debug, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)] // Flat records avoid one allocation per directive.
enum DirectiveRecordKind<'source> {
    Rule {
        targets: Token<'source>,
        operator: Token<'source>,
        actions: Option<Token<'source>>,
    },
    Action {
        actions: Token<'source>,
    },
    Marker {
        marker: MarkerText<'source>,
    },
    ComponentSignature {
        signature: Token<'source>,
    },
    RuleUpdateTargetById {
        selector_start: usize,
        selector_count: usize,
        targets: Token<'source>,
    },
    RuleRemoveById {
        selector_start: usize,
        selector_count: usize,
    },
}

#[derive(Debug, PartialEq, Eq)]
struct DirectiveRecord<'source> {
    logical_line_index: usize,
    span: SourceSpan,
    kind: DirectiveRecordKind<'source>,
}

/// A document-bound view of one typed directive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectiveView<'view, 'source> {
    logical_line_index: usize,
    span: SourceSpan,
    directive: Directive<'view, 'source>,
}

impl<'view, 'source> DirectiveView<'view, 'source> {
    /// Zero-based logical-line index in the retained source document.
    pub const fn logical_line_index(&self) -> usize {
        self.logical_line_index
    }

    /// Full normalized logical-line diagnostic envelope.
    pub const fn span(&self) -> SourceSpan {
        self.span
    }

    /// Typed directive fields.
    pub const fn directive(&self) -> Directive<'view, 'source> {
        self.directive
    }
}

/// Typed borrowed fields for one selected SecLang directive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names, clippy::large_enum_variant)] // Public names mirror SecLang directives; views stay allocation-free.
pub enum Directive<'view, 'source> {
    /// A `SecRule` envelope.
    SecRule {
        /// Unquoted target expression, not yet split.
        targets: Token<'source>,
        /// Double-quoted operator expression, not yet parsed.
        operator: Token<'source>,
        /// Optional double-quoted action list, not yet parsed.
        actions: Option<Token<'source>>,
    },
    /// A `SecAction` envelope.
    SecAction {
        /// Double-quoted action list, not yet parsed.
        actions: Token<'source>,
    },
    /// A `SecMarker` envelope.
    SecMarker {
        /// Marker text.
        marker: MarkerText<'source>,
    },
    /// A `SecComponentSignature` envelope.
    SecComponentSignature {
        /// Double-quoted component signature.
        signature: Token<'source>,
    },
    /// A `SecRuleUpdateTargetById` envelope.
    SecRuleUpdateTargetById {
        /// Non-empty document-bound slice of unquoted selector tokens.
        selectors: &'view [Token<'source>],
        /// Final quoted or unquoted target expression.
        targets: Token<'source>,
    },
    /// A `SecRuleRemoveById` envelope.
    SecRuleRemoveById {
        /// Non-empty document-bound slice of unquoted selector tokens.
        selectors: &'view [Token<'source>],
    },
}

impl Directive<'_, '_> {
    /// Top-level directive kind.
    pub const fn kind(&self) -> DirectiveKind {
        match self {
            Self::SecRule { .. } => DirectiveKind::SecRule,
            Self::SecAction { .. } => DirectiveKind::SecAction,
            Self::SecMarker { .. } => DirectiveKind::SecMarker,
            Self::SecComponentSignature { .. } => DirectiveKind::SecComponentSignature,
            Self::SecRuleUpdateTargetById { .. } => DirectiveKind::SecRuleUpdateTargetById,
            Self::SecRuleRemoveById { .. } => DirectiveKind::SecRuleRemoveById,
        }
    }
}

/// A bounded typed directive document retaining its scanned source identity.
#[derive(Debug, PartialEq, Eq)]
pub struct DirectiveDocument<'source> {
    source: &'source SourceDocument,
    records: Vec<DirectiveRecord<'source>>,
    update_selectors: Vec<Token<'source>>,
    charged_bytes: usize,
}

impl<'source> DirectiveDocument<'source> {
    /// The scanned source document that owns all borrowed directive text.
    pub const fn source(&self) -> &'source SourceDocument {
        self.source
    }

    /// Number of parsed directives.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the parsed document contains no directives.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Bytes charged to the parser's configured owned-metadata budget.
    pub const fn charged_bytes(&self) -> usize {
        self.charged_bytes
    }

    /// Iterate over document-bound directive views in source order.
    pub fn iter(&self) -> impl Iterator<Item = DirectiveView<'_, 'source>> {
        self.records.iter().map(|record| self.view(record))
    }

    pub(crate) fn get(&self, index: usize) -> Option<DirectiveView<'_, 'source>> {
        self.records.get(index).map(|record| self.view(record))
    }

    fn view<'view>(&'view self, record: &'view DirectiveRecord<'source>) -> DirectiveView<'view, 'source> {
        let directive = match record.kind {
            DirectiveRecordKind::Rule {
                targets,
                operator,
                actions,
            } => Directive::SecRule {
                targets,
                operator,
                actions,
            },
            DirectiveRecordKind::Action { actions } => Directive::SecAction { actions },
            DirectiveRecordKind::Marker { marker } => Directive::SecMarker { marker },
            DirectiveRecordKind::ComponentSignature { signature } => Directive::SecComponentSignature { signature },
            DirectiveRecordKind::RuleUpdateTargetById {
                selector_start,
                selector_count,
                targets,
            } => {
                let selector_end = selector_start
                    .checked_add(selector_count)
                    .expect("private selector count must fit after validated parser arithmetic");
                let selectors = self
                    .update_selectors
                    .get(selector_start..selector_end)
                    .expect("private selector range must remain valid in an immutable directive document");
                Directive::SecRuleUpdateTargetById { selectors, targets }
            }
            DirectiveRecordKind::RuleRemoveById {
                selector_start,
                selector_count,
            } => {
                let selector_end = selector_start
                    .checked_add(selector_count)
                    .expect("private selector count must fit after validated parser arithmetic");
                let selectors = self
                    .update_selectors
                    .get(selector_start..selector_end)
                    .expect("private selector range must remain valid in an immutable directive document");
                Directive::SecRuleRemoveById { selectors }
            }
        };
        DirectiveView {
            logical_line_index: record.logical_line_index,
            span: record.span,
            directive,
        }
    }
}

/// Error returned by selected-directive envelope parsing.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DirectiveParseError {
    /// The directive count exceeds its configured limit.
    #[error("directive count exceeds the configured limit of {limit} at {position:?}")]
    DirectiveLimit { limit: usize, position: SourcePosition },
    /// A logical line has no directive-name token.
    #[error("logical line {logical_line} has no directive name at {position:?}")]
    MissingDirective {
        logical_line: usize,
        position: SourcePosition,
    },
    /// The directive name is quoted.
    #[error("directive name must be unquoted, not {actual:?}, at {position:?}")]
    QuotedDirectiveName {
        actual: QuoteStyle,
        position: SourcePosition,
    },
    /// The unquoted directive name is outside the selected profile.
    #[error("unsupported SecLang directive at {position:?}")]
    UnknownDirective { position: SourcePosition },
    /// The top-level token count is invalid for a selected directive.
    #[error("{kind:?} has {actual} top-level tokens, expected {expected:?}, at {position:?}")]
    Arity {
        kind: DirectiveKind,
        actual: usize,
        expected: DirectiveArity,
        position: SourcePosition,
    },
    /// A rule-ID directive exceeds its selector limit.
    #[error("rule ID directive has {actual} selectors, exceeding the configured limit of {limit} at {position:?}")]
    UpdateSelectorLimit {
        actual: usize,
        limit: usize,
        position: SourcePosition,
    },
    /// A field uses a forbidden top-level quote style.
    #[error("{kind:?} {field:?} has quote style {actual:?}, expected {expected:?}, at {position:?}")]
    QuoteStyle {
        kind: DirectiveKind,
        field: DirectiveField,
        actual: QuoteStyle,
        expected: QuoteRequirement,
        position: SourcePosition,
    },
    /// A field that must contain bytes is empty.
    #[error("{kind:?} {field:?} is empty at {position:?}")]
    EmptyField {
        kind: DirectiveKind,
        field: DirectiveField,
        position: SourcePosition,
    },
    /// A tokenized line cannot be associated with its retained source line.
    #[error("tokenized logical line {logical_line} is unavailable in the retained source")]
    InvalidTokenizedDocument { logical_line: usize },
    /// A normalized marker tail cannot be recovered from borrowed source text.
    #[error("cannot recover marker tail on logical line {logical_line} from bounded token metadata")]
    MarkerTailMapping { logical_line: usize },
    /// The returned document would exceed its charged owned-byte limit.
    #[error("directive document requires more than {limit} charged owned bytes at {position:?}")]
    OwnedByteLimit { limit: usize, position: SourcePosition },
    /// A bounded allocation failed.
    #[error("cannot allocate bounded SecLang {operation}; reduce the source or configured limits")]
    Allocation { operation: &'static str },
}

/// Parse the six selected typed directive envelopes without retaining tokenizer metadata.
pub fn parse_directives<'source>(
    tokenized: &TokenizedDocument<'source>,
    limits: DirectiveLimits,
) -> Result<DirectiveDocument<'source>, DirectiveParseError> {
    let source = tokenized.source();
    let mut document = DirectiveDocument {
        source,
        records: Vec::new(),
        update_selectors: Vec::new(),
        charged_bytes: 0,
    };

    for line_index in 0..tokenized.line_count() {
        let source_line = source
            .lines()
            .get(line_index)
            .ok_or(DirectiveParseError::InvalidTokenizedDocument {
                logical_line: line_index + 1,
            })?;
        if document.records.len() >= limits.max_directives {
            return Err(DirectiveParseError::DirectiveLimit {
                limit: limits.max_directives,
                position: source_line.span().start,
            });
        }
        let line = tokenized
            .line(line_index)
            .ok_or(DirectiveParseError::InvalidTokenizedDocument {
                logical_line: line_index + 1,
            })?;
        let tokens = line.tokens();
        let name = tokens.first().ok_or(DirectiveParseError::MissingDirective {
            logical_line: line_index + 1,
            position: source_line.span().start,
        })?;
        if name.quote_style() != QuoteStyle::Unquoted {
            return Err(DirectiveParseError::QuotedDirectiveName {
                actual: name.quote_style(),
                position: name.raw_span().start,
            });
        }
        let kind = recognize(name.value()).ok_or(DirectiveParseError::UnknownDirective {
            position: name.raw_span().start,
        })?;
        let record_kind = parse_record_kind(kind, tokens, source_line, line_index, limits)?;

        let selector_tokens = match &record_kind {
            PendingRecordKind::RuleUpdateTargetById { selectors, .. }
            | PendingRecordKind::RuleRemoveById { selectors } => *selectors,
            _ => &[],
        };
        let selector_bytes = size_of::<Token<'source>>().checked_mul(selector_tokens.len()).ok_or(
            DirectiveParseError::OwnedByteLimit {
                limit: limits.max_owned_bytes,
                position: name.raw_span().start,
            },
        )?;
        charge_owned(
            &mut document.charged_bytes,
            selector_bytes,
            limits.max_owned_bytes,
            name.raw_span().start,
        )?;
        document
            .update_selectors
            .try_reserve(selector_tokens.len())
            .map_err(|_| DirectiveParseError::Allocation {
                operation: "rule ID selector records",
            })?;
        charge_owned(
            &mut document.charged_bytes,
            size_of::<DirectiveRecord<'source>>(),
            limits.max_owned_bytes,
            name.raw_span().start,
        )?;
        document
            .records
            .try_reserve(1)
            .map_err(|_| DirectiveParseError::Allocation {
                operation: "directive records",
            })?;

        let selector_start = document.update_selectors.len();
        document.update_selectors.extend_from_slice(selector_tokens);
        let record_kind = record_kind.publish(selector_start);
        document.records.push(DirectiveRecord {
            logical_line_index: line_index,
            span: source_line.span(),
            kind: record_kind,
        });
    }
    Ok(document)
}

#[allow(clippy::large_enum_variant)] // Temporary flat values preserve transactional publication without heap indirection.
enum PendingRecordKind<'tokens, 'source> {
    Rule {
        targets: Token<'source>,
        operator: Token<'source>,
        actions: Option<Token<'source>>,
    },
    Action {
        actions: Token<'source>,
    },
    Marker {
        marker: MarkerText<'source>,
    },
    ComponentSignature {
        signature: Token<'source>,
    },
    RuleUpdateTargetById {
        selectors: &'tokens [Token<'source>],
        targets: Token<'source>,
    },
    RuleRemoveById {
        selectors: &'tokens [Token<'source>],
    },
}

impl<'source> PendingRecordKind<'_, 'source> {
    fn publish(self, selector_start: usize) -> DirectiveRecordKind<'source> {
        match self {
            Self::Rule {
                targets,
                operator,
                actions,
            } => DirectiveRecordKind::Rule {
                targets,
                operator,
                actions,
            },
            Self::Action { actions } => DirectiveRecordKind::Action { actions },
            Self::Marker { marker } => DirectiveRecordKind::Marker { marker },
            Self::ComponentSignature { signature } => DirectiveRecordKind::ComponentSignature { signature },
            Self::RuleUpdateTargetById { selectors, targets } => DirectiveRecordKind::RuleUpdateTargetById {
                selector_start,
                selector_count: selectors.len(),
                targets,
            },
            Self::RuleRemoveById { selectors } => DirectiveRecordKind::RuleRemoveById {
                selector_start,
                selector_count: selectors.len(),
            },
        }
    }
}

fn parse_record_kind<'tokens, 'source>(
    kind: DirectiveKind,
    tokens: &'tokens [Token<'source>],
    line: &'source super::LogicalLine,
    line_index: usize,
    limits: DirectiveLimits,
) -> Result<PendingRecordKind<'tokens, 'source>, DirectiveParseError> {
    let position = tokens[0].raw_span().start;
    match kind {
        DirectiveKind::SecRule => {
            require_arity(
                kind,
                tokens,
                DirectiveArity::Either(3, 4),
                |count| matches!(count, 3 | 4),
                position,
            )?;
            require_quote(kind, DirectiveField::Targets, tokens[1], QuoteRequirement::Unquoted)?;
            require_quote(kind, DirectiveField::Operator, tokens[2], QuoteRequirement::Double)?;
            let actions = tokens.get(3).copied();
            if let Some(actions) = actions {
                require_quote(kind, DirectiveField::Actions, actions, QuoteRequirement::Double)?;
            }
            Ok(PendingRecordKind::Rule {
                targets: tokens[1],
                operator: tokens[2],
                actions,
            })
        }
        DirectiveKind::SecAction => {
            require_arity(kind, tokens, DirectiveArity::Exact(2), |count| count == 2, position)?;
            let actions = tokens[1];
            require_quote(kind, DirectiveField::Actions, actions, QuoteRequirement::Double)?;
            require_non_empty(kind, DirectiveField::Actions, actions)?;
            Ok(PendingRecordKind::Action { actions })
        }
        DirectiveKind::SecMarker => {
            require_arity(kind, tokens, DirectiveArity::AtLeast(2), |count| count >= 2, position)?;
            let first = tokens[1];
            if tokens.len() == 2 {
                require_quote(kind, DirectiveField::Marker, first, QuoteRequirement::UnquotedOrDouble)?;
                require_non_empty(kind, DirectiveField::Marker, first)?;
                Ok(PendingRecordKind::Marker {
                    marker: MarkerText {
                        value: first.value(),
                        quote_style: first.quote_style(),
                        span: first.content_span(),
                    },
                })
            } else {
                require_quote(kind, DirectiveField::Marker, first, QuoteRequirement::Unquoted)?;
                let marker = marker_tail(line, tokens[0], first, line_index)?;
                Ok(PendingRecordKind::Marker { marker })
            }
        }
        DirectiveKind::SecComponentSignature => {
            require_arity(kind, tokens, DirectiveArity::Exact(2), |count| count == 2, position)?;
            let signature = tokens[1];
            require_quote(kind, DirectiveField::Signature, signature, QuoteRequirement::Double)?;
            require_non_empty(kind, DirectiveField::Signature, signature)?;
            Ok(PendingRecordKind::ComponentSignature { signature })
        }
        DirectiveKind::SecRuleUpdateTargetById => {
            require_arity(kind, tokens, DirectiveArity::AtLeast(3), |count| count >= 3, position)?;
            let selector_count = tokens.len() - 2;
            if selector_count > limits.max_update_selectors {
                return Err(DirectiveParseError::UpdateSelectorLimit {
                    actual: selector_count,
                    limit: limits.max_update_selectors,
                    position: tokens[1].raw_span().start,
                });
            }
            let selectors = &tokens[1..tokens.len() - 1];
            for selector in selectors {
                require_quote(
                    kind,
                    DirectiveField::UpdateSelector,
                    *selector,
                    QuoteRequirement::Unquoted,
                )?;
                require_non_empty(kind, DirectiveField::UpdateSelector, *selector)?;
            }
            let targets = tokens[tokens.len() - 1];
            require_quote(
                kind,
                DirectiveField::UpdateTargets,
                targets,
                QuoteRequirement::UnquotedOrDouble,
            )?;
            require_non_empty(kind, DirectiveField::UpdateTargets, targets)?;
            Ok(PendingRecordKind::RuleUpdateTargetById { selectors, targets })
        }
        DirectiveKind::SecRuleRemoveById => {
            require_arity(kind, tokens, DirectiveArity::AtLeast(2), |count| count >= 2, position)?;
            let selector_count = tokens.len() - 1;
            if selector_count > limits.max_update_selectors {
                return Err(DirectiveParseError::UpdateSelectorLimit {
                    actual: selector_count,
                    limit: limits.max_update_selectors,
                    position: tokens[1].raw_span().start,
                });
            }
            let selectors = &tokens[1..];
            for selector in selectors {
                require_quote(
                    kind,
                    DirectiveField::UpdateSelector,
                    *selector,
                    QuoteRequirement::Unquoted,
                )?;
                require_non_empty(kind, DirectiveField::UpdateSelector, *selector)?;
            }
            Ok(PendingRecordKind::RuleRemoveById { selectors })
        }
    }
}

fn marker_tail<'source>(
    line: &'source super::LogicalLine,
    name: Token<'source>,
    first: Token<'source>,
    line_index: usize,
) -> Result<MarkerText<'source>, DirectiveParseError> {
    let after_name = line
        .text()
        .strip_prefix(name.raw())
        .ok_or(DirectiveParseError::MarkerTailMapping {
            logical_line: line_index + 1,
        })?;
    let value = after_name.trim_start_matches([' ', '\t']);
    if !value.starts_with(first.raw()) {
        return Err(DirectiveParseError::MarkerTailMapping {
            logical_line: line_index + 1,
        });
    }
    Ok(MarkerText {
        value,
        quote_style: QuoteStyle::Unquoted,
        span: SourceSpan {
            start: first.raw_span().start,
            end: line.span().end,
        },
    })
}

fn recognize(name: &str) -> Option<DirectiveKind> {
    if name.eq_ignore_ascii_case("SecRule") {
        Some(DirectiveKind::SecRule)
    } else if name.eq_ignore_ascii_case("SecAction") {
        Some(DirectiveKind::SecAction)
    } else if name.eq_ignore_ascii_case("SecMarker") {
        Some(DirectiveKind::SecMarker)
    } else if name.eq_ignore_ascii_case("SecComponentSignature") {
        Some(DirectiveKind::SecComponentSignature)
    } else if name.eq_ignore_ascii_case("SecRuleUpdateTargetById") {
        Some(DirectiveKind::SecRuleUpdateTargetById)
    } else if name.eq_ignore_ascii_case("SecRuleRemoveById") {
        Some(DirectiveKind::SecRuleRemoveById)
    } else {
        None
    }
}

fn require_arity(
    kind: DirectiveKind,
    tokens: &[Token<'_>],
    expected: DirectiveArity,
    accepts: impl FnOnce(usize) -> bool,
    position: SourcePosition,
) -> Result<(), DirectiveParseError> {
    if accepts(tokens.len()) {
        Ok(())
    } else {
        Err(DirectiveParseError::Arity {
            kind,
            actual: tokens.len(),
            expected,
            position,
        })
    }
}

fn require_quote(
    kind: DirectiveKind,
    field: DirectiveField,
    token: Token<'_>,
    expected: QuoteRequirement,
) -> Result<(), DirectiveParseError> {
    let accepted = match expected {
        QuoteRequirement::Unquoted => token.quote_style() == QuoteStyle::Unquoted,
        QuoteRequirement::Double => token.quote_style() == QuoteStyle::Double,
        QuoteRequirement::UnquotedOrDouble => matches!(token.quote_style(), QuoteStyle::Unquoted | QuoteStyle::Double),
    };
    if accepted {
        Ok(())
    } else {
        Err(DirectiveParseError::QuoteStyle {
            kind,
            field,
            actual: token.quote_style(),
            expected,
            position: token.raw_span().start,
        })
    }
}

fn require_non_empty(kind: DirectiveKind, field: DirectiveField, token: Token<'_>) -> Result<(), DirectiveParseError> {
    if token.value().is_empty() {
        Err(DirectiveParseError::EmptyField {
            kind,
            field,
            position: token.content_span().start,
        })
    } else {
        Ok(())
    }
}

fn charge_owned(
    charged: &mut usize,
    amount: usize,
    limit: usize,
    position: SourcePosition,
) -> Result<(), DirectiveParseError> {
    let next = charged
        .checked_add(amount)
        .ok_or(DirectiveParseError::OwnedByteLimit { limit, position })?;
    if next > limit {
        return Err(DirectiveParseError::OwnedByteLimit { limit, position });
    }
    *charged = next;
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/seclang/directive_test.rs"]
mod tests;
