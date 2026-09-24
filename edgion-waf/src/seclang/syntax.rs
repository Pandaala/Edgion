use std::iter::FusedIterator;

use thiserror::Error;

use super::action::{
    parse_action_syntax, ActionBuildError, ActionListKind, ActionMappingView, ActionSyntaxDocument, ActionSyntaxError,
    ActionSyntaxLimits,
};
use super::operator::{
    parse_operator_syntax, OperatorMappingView, OperatorSyntaxError, OperatorSyntaxLimits, OperatorView,
};
use super::target::{parse_target_syntax, DirectiveTargetView, TargetSyntaxError, TargetSyntaxLimits};
use super::{
    parse_directives, tokenize_document, Directive, DirectiveLimits, DirectiveParseError, DirectiveView,
    SourceDocument, TokenizeError, TokenizerLimits,
};

pub const DEFAULT_MAX_SYNTAX_RETAINED_BYTES: usize = 160 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyntaxLimits {
    pub tokenizer: TokenizerLimits,
    pub directives: DirectiveLimits,
    pub targets: TargetSyntaxLimits,
    pub operators: OperatorSyntaxLimits,
    pub actions: ActionSyntaxLimits,
    pub max_retained_bytes: usize,
}

impl Default for SyntaxLimits {
    fn default() -> Self {
        Self {
            tokenizer: TokenizerLimits::default(),
            directives: DirectiveLimits::default(),
            targets: TargetSyntaxLimits::default(),
            operators: OperatorSyntaxLimits::default(),
            actions: ActionSyntaxLimits::default(),
            max_retained_bytes: DEFAULT_MAX_SYNTAX_RETAINED_BYTES,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxInvariantKind {
    RecordCount,
    LogicalLine,
    NestedKind,
    ActionPresence,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SyntaxError {
    #[error("{0}")]
    Tokenizer(
        #[source]
        #[from]
        TokenizeError,
    ),
    #[error("{0}")]
    Directive(
        #[source]
        #[from]
        DirectiveParseError,
    ),
    #[error("{0}")]
    Target(
        #[source]
        #[from]
        TargetSyntaxError,
    ),
    #[error("{0}")]
    Operator(
        #[source]
        #[from]
        OperatorSyntaxError,
    ),
    #[error("{0}")]
    Action(#[source] ActionSyntaxError),
    #[error("retained syntax requires more than the configured limit of {limit} bytes")]
    RetainedByteLimit { limit: usize },
    #[error("private syntax invariant {kind:?} failed at directive index {index}")]
    Invariant { kind: SyntaxInvariantKind, index: usize },
}

#[derive(Debug)]
pub struct SyntaxDocument<'source> {
    actions: ActionSyntaxDocument<'source>,
    tokenizer_charged_bytes: usize,
}

impl<'source> SyntaxDocument<'source> {
    pub fn source(&self) -> &'source SourceDocument {
        self.actions.source()
    }

    pub fn len(&self) -> usize {
        self.actions.operators().directives().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn source_charged_bytes(&self) -> usize {
        self.source().charged_bytes()
    }

    pub fn tokenizer_charged_bytes(&self) -> usize {
        self.tokenizer_charged_bytes
    }

    pub fn directive_charged_bytes(&self) -> usize {
        self.actions.operators().directives().charged_bytes()
    }

    pub fn target_charged_bytes(&self) -> usize {
        self.actions.operators().targets().charged_bytes()
    }

    pub fn operator_charged_bytes(&self) -> usize {
        self.actions.operators().charged_bytes()
    }

    pub fn action_charged_bytes(&self) -> usize {
        self.actions.charged_bytes()
    }

    pub fn retained_charged_bytes(&self) -> usize {
        self.actions.retained_charged_bytes()
    }

    pub fn target_atom_count(&self) -> usize {
        self.actions.operators().targets().atom_count()
    }

    pub fn id_selector_count(&self) -> usize {
        self.actions.operators().targets().id_selector_count()
    }

    pub fn operator_count(&self) -> usize {
        self.actions.operators().operator_count()
    }

    pub fn decoded_operator_bytes(&self) -> usize {
        self.actions.operators().decoded_bytes()
    }

    pub fn action_list_count(&self) -> usize {
        self.actions.list_count()
    }

    pub fn action_entry_count(&self) -> usize {
        self.actions.entry_count()
    }

    pub fn get(&self, index: usize) -> Option<SyntaxDirectiveView<'_, 'source>> {
        let directives = self.actions.operators().directives();
        Some(SyntaxDirectiveView {
            directive: directives.get(index)?,
            targets: self.actions.operators().targets().get(index)?.targets(),
            operator: match self.actions.operators().get(index)? {
                OperatorMappingView::None => None,
                OperatorMappingView::Rule(operator) => Some(operator),
            },
            actions: match self.actions.get(index)? {
                ActionMappingView::None => None,
                ActionMappingView::List(actions) => Some(actions),
            },
        })
    }

    pub fn iter(&self) -> SyntaxIter<'_, 'source> {
        SyntaxIter {
            document: self,
            index: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SyntaxDirectiveView<'view, 'source> {
    directive: DirectiveView<'view, 'source>,
    targets: DirectiveTargetView<'view, 'source>,
    operator: Option<OperatorView<'view, 'source>>,
    actions: Option<super::action::ActionListView<'view, 'source>>,
}

impl<'view, 'source> SyntaxDirectiveView<'view, 'source> {
    pub fn directive(&self) -> DirectiveView<'view, 'source> {
        self.directive
    }

    pub fn targets(&self) -> DirectiveTargetView<'view, 'source> {
        self.targets
    }

    pub fn operator(&self) -> Option<OperatorView<'view, 'source>> {
        self.operator
    }

    pub fn actions(&self) -> Option<super::action::ActionListView<'view, 'source>> {
        self.actions
    }
}

pub struct SyntaxIter<'view, 'source> {
    document: &'view SyntaxDocument<'source>,
    index: usize,
}

impl<'view, 'source> Iterator for SyntaxIter<'view, 'source> {
    type Item = SyntaxDirectiveView<'view, 'source>;

    fn next(&mut self) -> Option<Self::Item> {
        let item = self.document.get(self.index)?;
        self.index += 1;
        Some(item)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.document.len().saturating_sub(self.index);
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for SyntaxIter<'_, '_> {}
impl FusedIterator for SyntaxIter<'_, '_> {}

pub fn parse_syntax<'source>(
    source: &'source SourceDocument,
    limits: SyntaxLimits,
) -> Result<SyntaxDocument<'source>, SyntaxError> {
    let tokenized = tokenize_document(source, limits.tokenizer)?;
    let tokenizer_charged_bytes = tokenized.charged_bytes();
    let directives = parse_directives(&tokenized, limits.directives)?;
    drop(tokenized);
    let targets = parse_target_syntax(&directives, limits.targets)?;
    let operators = parse_operator_syntax(directives, targets, limits.operators)?;
    let inherited = operators
        .retained_charged_bytes()
        .ok_or(SyntaxError::RetainedByteLimit {
            limit: limits.max_retained_bytes,
        })?;
    if inherited > limits.max_retained_bytes {
        return Err(SyntaxError::RetainedByteLimit {
            limit: limits.max_retained_bytes,
        });
    }
    let actions =
        parse_action_syntax(operators, limits.actions, limits.max_retained_bytes).map_err(|error| match error {
            ActionBuildError::Syntax(error) => SyntaxError::Action(error),
            ActionBuildError::RetainedByteLimit => SyntaxError::RetainedByteLimit {
                limit: limits.max_retained_bytes,
            },
        })?;
    validate(&actions)?;
    Ok(SyntaxDocument {
        actions,
        tokenizer_charged_bytes,
    })
}

fn validate(actions: &ActionSyntaxDocument<'_>) -> Result<(), SyntaxError> {
    let operators = actions.operators();
    let directives = operators.directives();
    let count = directives.len();
    if operators.mapping_count() != count
        || operators.targets().record_count() != count
        || actions.mapping_count() != count
    {
        return Err(SyntaxError::Invariant {
            kind: SyntaxInvariantKind::RecordCount,
            index: 0,
        });
    }
    for index in 0..count {
        let directive = directives.get(index).ok_or(SyntaxError::Invariant {
            kind: SyntaxInvariantKind::RecordCount,
            index,
        })?;
        let target = operators.targets().get(index).ok_or(SyntaxError::Invariant {
            kind: SyntaxInvariantKind::RecordCount,
            index,
        })?;
        let operator = operators.get(index).ok_or(SyntaxError::Invariant {
            kind: SyntaxInvariantKind::RecordCount,
            index,
        })?;
        let action = actions.get(index).ok_or(SyntaxError::Invariant {
            kind: SyntaxInvariantKind::RecordCount,
            index,
        })?;
        if target.logical_line_index() != directive.logical_line_index()
            || matches!(operator, OperatorMappingView::Rule(value) if value.logical_line_index() != directive.logical_line_index())
            || matches!(action, ActionMappingView::List(value) if value.logical_line_index() != directive.logical_line_index())
        {
            return Err(SyntaxError::Invariant {
                kind: SyntaxInvariantKind::LogicalLine,
                index,
            });
        }
        let nested_valid = match directive.directive() {
            Directive::SecRule { actions: envelope, .. } => {
                matches!(target.targets(), DirectiveTargetView::Rule { .. })
                    && matches!(operator, OperatorMappingView::Rule(_))
                    && !matches!(action, ActionMappingView::List(value) if value.kind() != ActionListKind::Rule)
                    && match (envelope.is_some(), action) {
                        (false, ActionMappingView::None) | (true, ActionMappingView::List(_)) => true,
                        _ => {
                            return Err(SyntaxError::Invariant {
                                kind: SyntaxInvariantKind::ActionPresence,
                                index,
                            })
                        }
                    }
            }
            Directive::SecAction { .. } => {
                matches!(target.targets(), DirectiveTargetView::None)
                    && matches!(operator, OperatorMappingView::None)
                    && match action {
                        ActionMappingView::List(value) => value.kind() == ActionListKind::Action,
                        ActionMappingView::None => {
                            return Err(SyntaxError::Invariant {
                                kind: SyntaxInvariantKind::ActionPresence,
                                index,
                            });
                        }
                    }
            }
            Directive::SecRuleUpdateTargetById { .. } => {
                matches!(target.targets(), DirectiveTargetView::Update { .. })
                    && matches!(operator, OperatorMappingView::None)
                    && matches!(action, ActionMappingView::None)
            }
            Directive::SecRuleRemoveById { .. } => {
                matches!(target.targets(), DirectiveTargetView::Remove { .. })
                    && matches!(operator, OperatorMappingView::None)
                    && matches!(action, ActionMappingView::None)
            }
            Directive::SecMarker { .. } | Directive::SecComponentSignature { .. } => {
                matches!(target.targets(), DirectiveTargetView::None)
                    && matches!(operator, OperatorMappingView::None)
                    && matches!(action, ActionMappingView::None)
            }
        };
        if !nested_valid {
            return Err(SyntaxError::Invariant {
                kind: SyntaxInvariantKind::NestedKind,
                index,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/seclang/syntax_test.rs"]
mod tests;
