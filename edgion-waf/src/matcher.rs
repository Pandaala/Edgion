//! Bounded multi-pattern matching primitives.
//!
//! The phrase matcher is an in-crate Aho-Corasick implementation. Regular
//! expressions use the workspace's general-purpose `regex-automata` engine;
//! no external WAF engine is linked or embedded.

use crate::aho_corasick::{
    AhoCorasick, BuildError as AcBuildError, BuildLimits as AcBuildLimits, Builder as AcBuilder,
    MatchError as AcMatchError, MatchKind as AcMatchKind, PatternSet as AcPatternSet, StreamState as AcStreamState,
};
use crate::bitset::set_bit_indices;
use regex_automata::nfa::thompson::pikevm::{Cache as PikeCache, PikeVM};
use regex_automata::nfa::thompson::{self, WhichCaptures, NFA};
use regex_automata::util::captures::Captures;
use regex_automata::{Input, MatchKind, PatternSet as RegexPatternSet};
use regex_syntax::ast::{self, Ast};
use regex_syntax::hir::{self, Hir, HirKind};
use std::marker::PhantomData;
use std::mem::size_of;
use std::ops::Range;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use thiserror::Error;

/// Default maximum number of patterns in one compiled matcher.
pub const DEFAULT_MAX_PATTERNS: usize = 4_096;
/// Default maximum combined source bytes in one compiled matcher.
pub const DEFAULT_MAX_PATTERN_BYTES: usize = 4 * 1024 * 1024;
/// Default maximum number of states in one phrase automaton.
pub const DEFAULT_MAX_PHRASE_STATES: usize = 1_000_000;
/// Default maximum number of transitions in one phrase automaton.
pub const DEFAULT_MAX_PHRASE_TRANSITIONS: usize = 1_000_000;
/// Default maximum number of direct outputs in one phrase automaton.
pub const DEFAULT_MAX_PHRASE_OUTPUTS: usize = DEFAULT_MAX_PATTERNS;
/// Default maximum estimated immutable bytes in one phrase automaton.
pub const DEFAULT_MAX_PHRASE_COMPILED_BYTES: usize = 256 * 1024 * 1024;
/// Default maximum complete-DFA cells available to a phrase automaton.
pub const DEFAULT_MAX_PHRASE_DFA_CELLS: usize = 16 * 1024 * 1024;
/// Default maximum Thompson NFA size for one multi-regex matcher.
pub const DEFAULT_MAX_REGEX_NFA_BYTES: usize = 32 * 1024 * 1024;
/// Default maximum complete retained bytes for one regex program.
pub const DEFAULT_MAX_REGEX_PROGRAM_BYTES: usize = 96 * 1024 * 1024;
pub const DEFAULT_MAX_REGEX_SOURCE_BYTES: usize = 64 * 1024;
pub const DEFAULT_MAX_REGEX_GROUP_SOURCE_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_MAX_REGEX_NEST_DEPTH: u32 = 128;
const MAX_SUPPORTED_REGEX_NEST_DEPTH: u32 = DEFAULT_MAX_REGEX_NEST_DEPTH;
pub const DEFAULT_MAX_REGEX_AST_NODES: usize = 262_144;
pub const DEFAULT_MAX_REGEX_HIR_NODES: usize = 262_144;
pub const DEFAULT_MAX_REGEX_CLASS_RANGES: usize = 262_144;
pub const DEFAULT_MAX_REGEX_HIR_LITERAL_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_MAX_REGEX_REPETITION_WORK: usize = 1_000_000;
pub const DEFAULT_MAX_REGEX_SCRATCH_BYTES: usize = 64 * 1024 * 1024;

/// Compilation limits for one matching group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatcherLimits {
    /// Maximum number of phrase and regex patterns combined.
    pub max_patterns: usize,
    /// Maximum combined bytes across all pattern source strings.
    pub max_pattern_bytes: usize,
    /// Maximum states in either case-sensitive phrase automaton.
    pub max_phrase_states: usize,
    /// Maximum transitions in either case-sensitive phrase automaton.
    pub max_phrase_transitions: usize,
    /// Maximum direct outputs in either case-sensitive phrase automaton.
    pub max_phrase_outputs: usize,
    /// Maximum estimated immutable bytes in either phrase automaton.
    pub max_phrase_compiled_bytes: usize,
    /// Maximum complete-DFA transition cells in either phrase automaton.
    pub max_phrase_dfa_cells: usize,
    /// Maximum bytes used by the compiled regex Thompson NFA.
    pub max_regex_nfa_bytes: usize,
    /// Maximum complete retained bytes for a regex program.
    pub max_regex_program_bytes: usize,
    pub max_regex_source_bytes_per_pattern: usize,
    pub max_regex_source_bytes_per_group: usize,
    pub max_regex_nest_depth: u32,
    pub max_regex_ast_nodes: usize,
    pub max_regex_hir_nodes: usize,
    pub max_regex_class_ranges: usize,
    pub max_regex_hir_literal_bytes: usize,
    pub max_regex_repetition_work: usize,
    pub max_regex_scratch_bytes: usize,
}

impl Default for MatcherLimits {
    fn default() -> Self {
        Self {
            max_patterns: DEFAULT_MAX_PATTERNS,
            max_pattern_bytes: DEFAULT_MAX_PATTERN_BYTES,
            max_phrase_states: DEFAULT_MAX_PHRASE_STATES,
            max_phrase_transitions: DEFAULT_MAX_PHRASE_TRANSITIONS,
            max_phrase_outputs: DEFAULT_MAX_PHRASE_OUTPUTS,
            max_phrase_compiled_bytes: DEFAULT_MAX_PHRASE_COMPILED_BYTES,
            max_phrase_dfa_cells: DEFAULT_MAX_PHRASE_DFA_CELLS,
            max_regex_nfa_bytes: DEFAULT_MAX_REGEX_NFA_BYTES,
            max_regex_program_bytes: DEFAULT_MAX_REGEX_PROGRAM_BYTES,
            max_regex_source_bytes_per_pattern: DEFAULT_MAX_REGEX_SOURCE_BYTES,
            max_regex_source_bytes_per_group: DEFAULT_MAX_REGEX_GROUP_SOURCE_BYTES,
            max_regex_nest_depth: DEFAULT_MAX_REGEX_NEST_DEPTH,
            max_regex_ast_nodes: DEFAULT_MAX_REGEX_AST_NODES,
            max_regex_hir_nodes: DEFAULT_MAX_REGEX_HIR_NODES,
            max_regex_class_ranges: DEFAULT_MAX_REGEX_CLASS_RANGES,
            max_regex_hir_literal_bytes: DEFAULT_MAX_REGEX_HIR_LITERAL_BYTES,
            max_regex_repetition_work: DEFAULT_MAX_REGEX_REPETITION_WORK,
            max_regex_scratch_bytes: DEFAULT_MAX_REGEX_SCRATCH_BYTES,
        }
    }
}

/// Limits for one byte-oriented regular-expression program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegexProgramLimits {
    pub max_source_bytes: usize,
    pub max_nest_depth: u32,
    pub max_ast_nodes: usize,
    pub max_hir_nodes: usize,
    pub max_class_ranges: usize,
    pub max_hir_literal_bytes: usize,
    pub max_repetition_work: usize,
    pub max_nfa_bytes: usize,
    pub max_program_bytes: usize,
    pub max_capture_slots: usize,
    pub max_named_groups: usize,
    pub max_capture_name_bytes: usize,
    pub max_total_capture_name_bytes: usize,
    pub max_scratch_bytes: usize,
}

impl Default for RegexProgramLimits {
    fn default() -> Self {
        Self {
            max_source_bytes: DEFAULT_MAX_REGEX_SOURCE_BYTES,
            max_nest_depth: DEFAULT_MAX_REGEX_NEST_DEPTH,
            max_ast_nodes: DEFAULT_MAX_REGEX_AST_NODES,
            max_hir_nodes: DEFAULT_MAX_REGEX_HIR_NODES,
            max_class_ranges: DEFAULT_MAX_REGEX_CLASS_RANGES,
            max_hir_literal_bytes: DEFAULT_MAX_REGEX_HIR_LITERAL_BYTES,
            max_repetition_work: DEFAULT_MAX_REGEX_REPETITION_WORK,
            max_nfa_bytes: DEFAULT_MAX_REGEX_NFA_BYTES,
            max_program_bytes: DEFAULT_MAX_REGEX_PROGRAM_BYTES,
            max_capture_slots: 32,
            max_named_groups: 32,
            max_capture_name_bytes: 64,
            max_total_capture_name_bytes: 4 * 1024,
            max_scratch_bytes: DEFAULT_MAX_REGEX_SCRATCH_BYTES,
        }
    }
}

impl From<MatcherLimits> for RegexProgramLimits {
    fn from(limits: MatcherLimits) -> Self {
        Self {
            max_source_bytes: limits.max_regex_source_bytes_per_pattern,
            max_nest_depth: limits.max_regex_nest_depth,
            max_ast_nodes: limits.max_regex_ast_nodes,
            max_hir_nodes: limits.max_regex_hir_nodes,
            max_class_ranges: limits.max_regex_class_ranges,
            max_hir_literal_bytes: limits.max_regex_hir_literal_bytes,
            max_repetition_work: limits.max_regex_repetition_work,
            max_nfa_bytes: limits.max_regex_nfa_bytes,
            max_program_bytes: limits.max_regex_program_bytes,
            max_scratch_bytes: limits.max_regex_scratch_bytes,
            ..Self::default()
        }
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum RegexProgramError {
    #[error("regex source has {actual} bytes, exceeding the limit of {limit}")]
    SourceByteLimit { actual: usize, limit: usize },
    #[error("regex nesting exceeds the configured depth limit of {limit}")]
    NestLimit { limit: u32 },
    #[error("regex nesting limit {actual} exceeds the supported maximum of {limit}")]
    NestConfigurationLimit { actual: u32, limit: u32 },
    #[error("regex AST has {actual} nodes, exceeding the limit of {limit}")]
    AstNodeLimit { actual: usize, limit: usize },
    #[error("regex HIR has {actual} nodes, exceeding the limit of {limit}")]
    HirNodeLimit { actual: usize, limit: usize },
    #[error("regex HIR has {actual} class ranges, exceeding the limit of {limit}")]
    ClassRangeLimit { actual: usize, limit: usize },
    #[error("regex HIR has {actual} literal bytes, exceeding the limit of {limit}")]
    HirLiteralByteLimit { actual: usize, limit: usize },
    #[error("regex repetition work is {actual}, exceeding the limit of {limit}")]
    RepetitionWorkLimit { actual: usize, limit: usize },
    #[error("regex syntax projection exceeds the compile peak bound")]
    SyntaxPeakLimit,
    #[error("regex syntax projection did not bound translated HIR")]
    ProjectionInvariant,
    #[error("regex syntax is invalid for the supported byte profile")]
    Syntax,
    #[error("regex Thompson NFA exceeds the configured byte limit of {limit}")]
    NfaByteLimit { limit: usize },
    #[error("regex engine rejected the bounded intermediate program")]
    Engine,
    #[error("compiled regex requires {actual} bytes, exceeding the limit of {limit}")]
    ProgramByteLimit { actual: usize, limit: usize },
    #[error("regex has {actual} capture slots, exceeding the limit of {limit}")]
    CaptureSlotLimit { actual: usize, limit: usize },
    #[error("regex has {actual} named groups, exceeding the limit of {limit}")]
    NamedGroupLimit { actual: usize, limit: usize },
    #[error("regex capture name has {actual} bytes, exceeding the limit of {limit}")]
    CaptureNameLimit { actual: usize, limit: usize },
    #[error("regex capture names have {actual} bytes, exceeding the limit of {limit}")]
    CaptureNameByteLimit { actual: usize, limit: usize },
    #[error("regex requires {actual} scratch bytes, exceeding the limit of {limit}")]
    ScratchByteLimit { actual: usize, limit: usize },
    #[error("regex accounting arithmetic overflowed")]
    Arithmetic,
    #[error("cannot allocate bounded regex metadata")]
    Allocation,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum RegexScratchError {
    #[error("regex scratch belongs to another program")]
    ScratchShape,
}

const FOLLOW_EPSILON_UPPER_BYTES: usize = 32;
const MAX_UNICODE_CLASS_RANGES: usize = 16_384;
const MAX_BYTE_CLASS_RANGES: usize = 256;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ClassRangeProjection {
    out_ranges: usize,
    peak_ranges: usize,
}

#[derive(Debug, Clone, Copy)]
struct ProjectionFlags {
    case_insensitive: bool,
    unicode: bool,
    dot_matches_new_line: bool,
    swap_greed: bool,
    multi_line: bool,
    crlf: bool,
    ignore_whitespace: bool,
}

impl Default for ProjectionFlags {
    fn default() -> Self {
        Self {
            case_insensitive: false,
            unicode: true,
            dot_matches_new_line: false,
            swap_greed: false,
            multi_line: false,
            crlf: false,
            ignore_whitespace: false,
        }
    }
}

impl ProjectionFlags {
    fn merge(&mut self, flags: &ast::Flags) {
        use ast::Flag;
        for (flag, field) in [
            (Flag::CaseInsensitive, &mut self.case_insensitive),
            (Flag::Unicode, &mut self.unicode),
            (Flag::DotMatchesNewLine, &mut self.dot_matches_new_line),
            (Flag::SwapGreed, &mut self.swap_greed),
            (Flag::MultiLine, &mut self.multi_line),
            (Flag::CRLF, &mut self.crlf),
            (Flag::IgnoreWhitespace, &mut self.ignore_whitespace),
        ] {
            if let Some(value) = flags.flag_state(flag) {
                *field = value;
            }
        }
    }

    const fn universe(self) -> usize {
        if self.unicode {
            MAX_UNICODE_CLASS_RANGES
        } else {
            MAX_BYTE_CLASS_RANGES
        }
    }
}

fn checked_projection(out_ranges: usize, peak_ranges: usize) -> Result<ClassRangeProjection, RegexProgramError> {
    Ok(ClassRangeProjection {
        out_ranges,
        peak_ranges,
    })
}

fn combine_projection(
    left: ClassRangeProjection,
    right: ClassRangeProjection,
    out: usize,
) -> Result<ClassRangeProjection, RegexProgramError> {
    let right_live = left
        .out_ranges
        .checked_add(right.peak_ranges)
        .ok_or(RegexProgramError::Arithmetic)?;
    let operation = left
        .out_ranges
        .checked_add(right.out_ranges)
        .and_then(|value| value.checked_add(out))
        .ok_or(RegexProgramError::Arithmetic)?;
    checked_projection(out, left.peak_ranges.max(right_live).max(operation))
}

fn fold_projection(
    value: ClassRangeProjection,
    flags: ProjectionFlags,
) -> Result<ClassRangeProjection, RegexProgramError> {
    if !flags.case_insensitive {
        return Ok(value);
    }
    let out = flags.universe();
    checked_projection(
        out,
        value
            .peak_ranges
            .max(value.out_ranges.checked_add(out).ok_or(RegexProgramError::Arithmetic)?),
    )
}

fn negate_projection(
    value: ClassRangeProjection,
    flags: ProjectionFlags,
) -> Result<ClassRangeProjection, RegexProgramError> {
    let out = flags
        .universe()
        .min(value.out_ranges.checked_add(1).ok_or(RegexProgramError::Arithmetic)?);
    checked_projection(
        out,
        value
            .peak_ranges
            .max(value.out_ranges.checked_add(out).ok_or(RegexProgramError::Arithmetic)?),
    )
}

fn project_binary_class(
    operation: &ast::ClassSetBinaryOp,
    left: ClassRangeProjection,
    right: ClassRangeProjection,
    flags: ProjectionFlags,
) -> Result<ClassRangeProjection, RegexProgramError> {
    let mut peak = left.peak_ranges.max(
        left.out_ranges
            .checked_add(right.peak_ranges)
            .ok_or(RegexProgramError::Arithmetic)?,
    );
    let (left_folded, right_folded) = if flags.case_insensitive {
        let right_folded = fold_projection(right, flags)?;
        peak = peak.max(
            left.out_ranges
                .checked_add(right.out_ranges)
                .and_then(|value| value.checked_add(right_folded.out_ranges))
                .ok_or(RegexProgramError::Arithmetic)?,
        );
        let left_folded = fold_projection(left, flags)?;
        peak = peak.max(
            right_folded
                .out_ranges
                .checked_add(left.out_ranges)
                .and_then(|value| value.checked_add(left_folded.out_ranges))
                .ok_or(RegexProgramError::Arithmetic)?,
        );
        (left_folded, right_folded)
    } else {
        (left, right)
    };
    let out = match operation.kind {
        ast::ClassSetBinaryOpKind::Intersection => flags.universe().min(
            left_folded
                .out_ranges
                .checked_add(right_folded.out_ranges)
                .ok_or(RegexProgramError::Arithmetic)?,
        ),
        ast::ClassSetBinaryOpKind::Difference => flags.universe(),
        ast::ClassSetBinaryOpKind::SymmetricDifference => flags.universe().min(
            left_folded
                .out_ranges
                .checked_add(right_folded.out_ranges)
                .ok_or(RegexProgramError::Arithmetic)?,
        ),
    };
    let operation_peak = left_folded
        .out_ranges
        .checked_add(right_folded.out_ranges)
        .and_then(|value| value.checked_add(out))
        .ok_or(RegexProgramError::Arithmetic)?;
    checked_projection(out, peak.max(operation_peak))
}

fn project_bracketed_value(
    class: &ast::ClassBracketed,
    mut value: ClassRangeProjection,
    flags: ProjectionFlags,
) -> Result<ClassRangeProjection, RegexProgramError> {
    value = fold_projection(value, flags)?;
    if class.negated {
        value = negate_projection(value, flags)?;
    }
    Ok(value)
}

fn combine_ast_projections(children: &[ClassRangeProjection]) -> Result<ClassRangeProjection, RegexProgramError> {
    let mut result = ClassRangeProjection::default();
    for child in children {
        let live_peak = result
            .out_ranges
            .checked_add(child.peak_ranges)
            .ok_or(RegexProgramError::Arithmetic)?;
        result.peak_ranges = result.peak_ranges.max(live_peak);
        result.out_ranges = result
            .out_ranges
            .checked_add(child.out_ranges)
            .ok_or(RegexProgramError::Arithmetic)?;
    }
    Ok(result)
}

struct ClassRangeProjectionVisitor {
    flags: ProjectionFlags,
    values: Vec<ClassRangeProjection>,
    markers: Vec<usize>,
    saved_flags: Vec<ProjectionFlags>,
}

impl ClassRangeProjectionVisitor {
    fn new(
        source_units: usize,
        case_insensitive: bool,
        unicode: bool,
        dot_matches_new_line: bool,
    ) -> Result<Self, RegexProgramError> {
        let flags = ProjectionFlags {
            case_insensitive,
            unicode,
            dot_matches_new_line,
            ..ProjectionFlags::default()
        };
        let mut visitor = Self {
            flags,
            values: Vec::new(),
            markers: Vec::new(),
            saved_flags: Vec::new(),
        };
        visitor
            .values
            .try_reserve_exact(source_units)
            .map_err(|_| RegexProgramError::Allocation)?;
        visitor
            .markers
            .try_reserve_exact(source_units)
            .map_err(|_| RegexProgramError::Allocation)?;
        visitor
            .saved_flags
            .try_reserve_exact(source_units)
            .map_err(|_| RegexProgramError::Allocation)?;
        Ok(visitor)
    }

    fn push_value(&mut self, value: ClassRangeProjection) -> Result<(), RegexProgramError> {
        if self.values.len() == self.values.capacity() {
            return Err(RegexProgramError::ProjectionInvariant);
        }
        self.values.push(value);
        Ok(())
    }

    fn push_marker(&mut self) -> Result<(), RegexProgramError> {
        if self.markers.len() == self.markers.capacity() {
            return Err(RegexProgramError::ProjectionInvariant);
        }
        self.markers.push(self.values.len());
        Ok(())
    }

    fn collapse_ast(&mut self) -> Result<(), RegexProgramError> {
        let start = self.markers.pop().ok_or(RegexProgramError::ProjectionInvariant)?;
        let result = combine_ast_projections(&self.values[start..])?;
        self.values.truncate(start);
        self.push_value(result)
    }

    fn collapse_union(&mut self) -> Result<(), RegexProgramError> {
        let start = self.markers.pop().ok_or(RegexProgramError::ProjectionInvariant)?;
        let mut result = ClassRangeProjection::default();
        for right in self.values[start..].iter().copied() {
            let out = self.flags.universe().min(
                result
                    .out_ranges
                    .checked_add(right.out_ranges)
                    .ok_or(RegexProgramError::Arithmetic)?,
            );
            result = combine_projection(result, right, out)?;
        }
        self.values.truncate(start);
        self.push_value(result)
    }

    fn collapse_bracketed(&mut self, class: &ast::ClassBracketed) -> Result<(), RegexProgramError> {
        let start = self.markers.pop().ok_or(RegexProgramError::ProjectionInvariant)?;
        let expected = start.checked_add(1).ok_or(RegexProgramError::Arithmetic)?;
        if self.values.len() != expected {
            return Err(RegexProgramError::ProjectionInvariant);
        }
        let value = self.values.pop().ok_or(RegexProgramError::ProjectionInvariant)?;
        self.push_value(project_bracketed_value(class, value, self.flags)?)
    }
}

impl ast::Visitor for ClassRangeProjectionVisitor {
    type Output = ClassRangeProjection;
    type Err = RegexProgramError;

    fn finish(mut self) -> Result<Self::Output, Self::Err> {
        if !self.markers.is_empty() || !self.saved_flags.is_empty() || self.values.len() != 1 {
            return Err(RegexProgramError::ProjectionInvariant);
        }
        self.values.pop().ok_or(RegexProgramError::ProjectionInvariant)
    }

    fn visit_pre(&mut self, ast: &Ast) -> Result<(), Self::Err> {
        match ast {
            Ast::Flags(set) => self.flags.merge(&set.flags),
            Ast::Group(group) => {
                if self.saved_flags.len() == self.saved_flags.capacity() {
                    return Err(RegexProgramError::ProjectionInvariant);
                }
                self.saved_flags.push(self.flags);
                if let Some(group_flags) = group.flags() {
                    self.flags.merge(group_flags);
                }
            }
            Ast::Alternation(_) | Ast::Concat(_) => self.push_marker()?,
            _ => {}
        }
        Ok(())
    }

    fn visit_post(&mut self, ast: &Ast) -> Result<(), Self::Err> {
        match ast {
            Ast::Empty(_) | Ast::Assertion(_) | Ast::Flags(_) => {
                self.push_value(ClassRangeProjection::default())?;
            }
            Ast::Literal(_) => {
                let value = ClassRangeProjection {
                    out_ranges: 1,
                    peak_ranges: 1,
                };
                self.push_value(if self.flags.case_insensitive {
                    fold_projection(value, self.flags)?
                } else {
                    value
                })?;
            }
            Ast::Dot(_) | Ast::ClassUnicode(_) | Ast::ClassPerl(_) => {
                self.push_value(ClassRangeProjection {
                    out_ranges: self.flags.universe(),
                    peak_ranges: self.flags.universe(),
                })?;
            }
            Ast::ClassBracketed(class) => {
                let value = self.values.pop().ok_or(RegexProgramError::ProjectionInvariant)?;
                self.push_value(project_bracketed_value(class, value, self.flags)?)?;
            }
            Ast::Repetition(_) => {}
            Ast::Group(_) => {
                self.flags = self.saved_flags.pop().ok_or(RegexProgramError::ProjectionInvariant)?;
            }
            Ast::Alternation(_) | Ast::Concat(_) => self.collapse_ast()?,
        }
        Ok(())
    }

    fn visit_class_set_item_pre(&mut self, item: &ast::ClassSetItem) -> Result<(), Self::Err> {
        if matches!(item, ast::ClassSetItem::Bracketed(_) | ast::ClassSetItem::Union(_)) {
            self.push_marker()?;
        }
        Ok(())
    }

    fn visit_class_set_item_post(&mut self, item: &ast::ClassSetItem) -> Result<(), Self::Err> {
        match item {
            ast::ClassSetItem::Empty(_) => self.push_value(ClassRangeProjection::default())?,
            ast::ClassSetItem::Literal(_) | ast::ClassSetItem::Range(_) => {
                self.push_value(ClassRangeProjection {
                    out_ranges: 1,
                    peak_ranges: 1,
                })?;
            }
            ast::ClassSetItem::Ascii(_) => self.push_value(ClassRangeProjection {
                out_ranges: 16,
                peak_ranges: 16,
            })?,
            ast::ClassSetItem::Unicode(_) | ast::ClassSetItem::Perl(_) => {
                self.push_value(ClassRangeProjection {
                    out_ranges: self.flags.universe(),
                    peak_ranges: self.flags.universe(),
                })?;
            }
            ast::ClassSetItem::Bracketed(class) => self.collapse_bracketed(class)?,
            ast::ClassSetItem::Union(_) => self.collapse_union()?,
        }
        Ok(())
    }

    fn visit_class_set_binary_op_pre(&mut self, _operation: &ast::ClassSetBinaryOp) -> Result<(), Self::Err> {
        self.push_marker()
    }

    fn visit_class_set_binary_op_post(&mut self, operation: &ast::ClassSetBinaryOp) -> Result<(), Self::Err> {
        let start = self.markers.pop().ok_or(RegexProgramError::ProjectionInvariant)?;
        let expected = start.checked_add(2).ok_or(RegexProgramError::Arithmetic)?;
        if self.values.len() != expected {
            return Err(RegexProgramError::ProjectionInvariant);
        }
        let right = self.values.pop().ok_or(RegexProgramError::ProjectionInvariant)?;
        let left = self.values.pop().ok_or(RegexProgramError::ProjectionInvariant)?;
        self.push_value(project_binary_class(operation, left, right, self.flags)?)
    }
}

fn project_ast_ranges(
    ast: &Ast,
    source_units: usize,
    case_insensitive: bool,
    unicode: bool,
    dot_matches_new_line: bool,
) -> Result<ClassRangeProjection, RegexProgramError> {
    ast::visit(
        ast,
        ClassRangeProjectionVisitor::new(source_units, case_insensitive, unicode, dot_matches_new_line)?,
    )
}

#[derive(Debug)]
struct CaptureShapeToken {
    discriminator: u8,
}

#[repr(C)]
struct RegexNameMapEntryProjection {
    fields: [usize; 4],
}

#[repr(C)]
struct RegexPatternNameProjection {
    fields: [usize; 6],
}

fn align_up(value: usize, align: usize) -> Option<usize> {
    value
        .checked_add(align.checked_sub(1)?)
        .map(|value| value / align * align)
}

#[derive(Default)]
struct AstNodeCounter(usize);

impl ast::Visitor for AstNodeCounter {
    type Output = usize;
    type Err = RegexProgramError;

    fn finish(self) -> Result<Self::Output, Self::Err> {
        Ok(self.0)
    }

    fn visit_pre(&mut self, _ast: &Ast) -> Result<(), Self::Err> {
        self.0 = self.0.checked_add(1).ok_or(RegexProgramError::Arithmetic)?;
        Ok(())
    }
}

fn count_ast_nodes(ast: &Ast) -> Result<usize, RegexProgramError> {
    ast::visit(ast, AstNodeCounter::default())
}

#[derive(Default)]
struct HirMetrics {
    nodes: usize,
    ranges: usize,
    literal_bytes: usize,
    repetition_work: usize,
    capture_groups: usize,
    named_groups: usize,
    max_capture_name_bytes: usize,
    capture_name_bytes: usize,
}

struct HirMetricsVisitor {
    metrics: HirMetrics,
    subtree_nodes: Vec<usize>,
    markers: Vec<usize>,
}

impl HirMetricsVisitor {
    fn new(source_units: usize) -> Result<Self, RegexProgramError> {
        let mut visitor = Self {
            metrics: HirMetrics::default(),
            subtree_nodes: Vec::new(),
            markers: Vec::new(),
        };
        visitor
            .subtree_nodes
            .try_reserve_exact(source_units)
            .map_err(|_| RegexProgramError::Allocation)?;
        visitor
            .markers
            .try_reserve_exact(source_units)
            .map_err(|_| RegexProgramError::Allocation)?;
        Ok(visitor)
    }

    fn push_subtree(&mut self, nodes: usize) -> Result<(), RegexProgramError> {
        if self.subtree_nodes.len() == self.subtree_nodes.capacity() {
            return Err(RegexProgramError::ProjectionInvariant);
        }
        self.subtree_nodes.push(nodes);
        Ok(())
    }

    fn push_marker(&mut self) -> Result<(), RegexProgramError> {
        if self.markers.len() == self.markers.capacity() {
            return Err(RegexProgramError::ProjectionInvariant);
        }
        self.markers.push(self.subtree_nodes.len());
        Ok(())
    }
}

impl hir::Visitor for HirMetricsVisitor {
    type Output = HirMetrics;
    type Err = RegexProgramError;

    fn finish(self) -> Result<Self::Output, Self::Err> {
        if !self.markers.is_empty() || self.subtree_nodes.len() != 1 {
            return Err(RegexProgramError::ProjectionInvariant);
        }
        Ok(self.metrics)
    }

    fn visit_pre(&mut self, hir: &Hir) -> Result<(), Self::Err> {
        self.metrics.nodes = self.metrics.nodes.checked_add(1).ok_or(RegexProgramError::Arithmetic)?;
        match hir.kind() {
            HirKind::Literal(literal) => {
                self.metrics.literal_bytes = self
                    .metrics
                    .literal_bytes
                    .checked_add(literal.0.len())
                    .ok_or(RegexProgramError::Arithmetic)?;
            }
            HirKind::Class(class) => {
                let count = match class {
                    hir::Class::Unicode(class) => class.ranges().len(),
                    hir::Class::Bytes(class) => class.ranges().len(),
                };
                self.metrics.ranges = self
                    .metrics
                    .ranges
                    .checked_add(count)
                    .ok_or(RegexProgramError::Arithmetic)?;
            }
            HirKind::Capture(capture) => {
                self.metrics.capture_groups = self
                    .metrics
                    .capture_groups
                    .checked_add(1)
                    .ok_or(RegexProgramError::Arithmetic)?;
                if let Some(name) = &capture.name {
                    self.metrics.named_groups = self
                        .metrics
                        .named_groups
                        .checked_add(1)
                        .ok_or(RegexProgramError::Arithmetic)?;
                    self.metrics.max_capture_name_bytes = self.metrics.max_capture_name_bytes.max(name.len());
                    self.metrics.capture_name_bytes = self
                        .metrics
                        .capture_name_bytes
                        .checked_add(name.len())
                        .ok_or(RegexProgramError::Arithmetic)?;
                }
            }
            HirKind::Concat(_) | HirKind::Alternation(_) => self.push_marker()?,
            HirKind::Empty | HirKind::Look(_) | HirKind::Repetition(_) => {}
        }
        Ok(())
    }

    fn visit_post(&mut self, hir: &Hir) -> Result<(), Self::Err> {
        match hir.kind() {
            HirKind::Empty | HirKind::Look(_) | HirKind::Literal(_) | HirKind::Class(_) => self.push_subtree(1)?,
            HirKind::Capture(_) => {
                let child = self.subtree_nodes.pop().ok_or(RegexProgramError::ProjectionInvariant)?;
                self.push_subtree(child.checked_add(1).ok_or(RegexProgramError::Arithmetic)?)?;
            }
            HirKind::Repetition(repetition) => {
                let child = self.subtree_nodes.pop().ok_or(RegexProgramError::ProjectionInvariant)?;
                let minimum = usize::try_from(repetition.min)
                    .ok()
                    .and_then(|value| value.checked_mul(child))
                    .ok_or(RegexProgramError::Arithmetic)?;
                let additional = match repetition.max {
                    Some(maximum) => usize::try_from(maximum.saturating_sub(repetition.min))
                        .ok()
                        .and_then(|value| value.checked_mul(child))
                        .ok_or(RegexProgramError::Arithmetic)?,
                    None => child,
                };
                self.metrics.repetition_work = self
                    .metrics
                    .repetition_work
                    .checked_add(minimum)
                    .and_then(|value| value.checked_add(additional))
                    .ok_or(RegexProgramError::Arithmetic)?;
                self.push_subtree(child.checked_add(1).ok_or(RegexProgramError::Arithmetic)?)?;
            }
            HirKind::Concat(_) | HirKind::Alternation(_) => {
                let start = self.markers.pop().ok_or(RegexProgramError::ProjectionInvariant)?;
                let nodes = self.subtree_nodes[start..].iter().try_fold(1_usize, |total, child| {
                    total.checked_add(*child).ok_or(RegexProgramError::Arithmetic)
                })?;
                self.subtree_nodes.truncate(start);
                self.push_subtree(nodes)?;
            }
        }
        Ok(())
    }
}

fn hir_metrics(hir: &Hir, source_units: usize) -> Result<HirMetrics, RegexProgramError> {
    hir::visit(hir, HirMetricsVisitor::new(source_units)?)
}

fn check_hir(metrics: &HirMetrics, limits: RegexProgramLimits) -> Result<(), RegexProgramError> {
    if metrics.nodes > limits.max_hir_nodes {
        return Err(RegexProgramError::HirNodeLimit {
            actual: metrics.nodes,
            limit: limits.max_hir_nodes,
        });
    }
    if metrics.ranges > limits.max_class_ranges {
        return Err(RegexProgramError::ClassRangeLimit {
            actual: metrics.ranges,
            limit: limits.max_class_ranges,
        });
    }
    if metrics.literal_bytes > limits.max_hir_literal_bytes {
        return Err(RegexProgramError::HirLiteralByteLimit {
            actual: metrics.literal_bytes,
            limit: limits.max_hir_literal_bytes,
        });
    }
    if metrics.repetition_work > limits.max_repetition_work {
        return Err(RegexProgramError::RepetitionWorkLimit {
            actual: metrics.repetition_work,
            limit: limits.max_repetition_work,
        });
    }
    Ok(())
}

fn check_capture_hir(
    metrics: &HirMetrics,
    pattern_count: usize,
    limits: RegexProgramLimits,
) -> Result<(), RegexProgramError> {
    let capture_slots = metrics
        .capture_groups
        .checked_add(pattern_count)
        .ok_or(RegexProgramError::Arithmetic)?;
    if capture_slots > limits.max_capture_slots {
        return Err(RegexProgramError::CaptureSlotLimit {
            actual: capture_slots,
            limit: limits.max_capture_slots,
        });
    }
    if metrics.named_groups > limits.max_named_groups {
        return Err(RegexProgramError::NamedGroupLimit {
            actual: metrics.named_groups,
            limit: limits.max_named_groups,
        });
    }
    if metrics.max_capture_name_bytes > limits.max_capture_name_bytes {
        return Err(RegexProgramError::CaptureNameLimit {
            actual: metrics.max_capture_name_bytes,
            limit: limits.max_capture_name_bytes,
        });
    }
    if metrics.capture_name_bytes > limits.max_total_capture_name_bytes {
        return Err(RegexProgramError::CaptureNameByteLimit {
            actual: metrics.capture_name_bytes,
            limit: limits.max_total_capture_name_bytes,
        });
    }
    Ok(())
}

#[derive(Debug)]
struct RegexCore {
    vm: PikeVM,
    token: Arc<CaptureShapeToken>,
    retained_bytes: usize,
    program_peak_bound: usize,
    peak: usize,
    scratch_upper: usize,
}

impl RegexCore {
    fn compile_many(
        sources: &[&str],
        limits: RegexProgramLimits,
        captures: WhichCaptures,
        peak_budget: usize,
        case_insensitive: bool,
        unicode: bool,
        match_kind: MatchKind,
    ) -> Result<Self, RegexProgramError> {
        let mut bytes = 0_usize;
        for source in sources {
            if source.len() > limits.max_source_bytes {
                return Err(RegexProgramError::SourceByteLimit {
                    actual: source.len(),
                    limit: limits.max_source_bytes,
                });
            }
            bytes = bytes.checked_add(source.len()).ok_or(RegexProgramError::Arithmetic)?;
        }
        if limits.max_nest_depth > MAX_SUPPORTED_REGEX_NEST_DEPTH {
            return Err(RegexProgramError::NestConfigurationLimit {
                actual: limits.max_nest_depth,
                limit: MAX_SUPPORTED_REGEX_NEST_DEPTH,
            });
        }
        let mut hirs = Vec::new();
        let mut ast_node_total = 0_usize;
        let mut hir_total = HirMetrics::default();
        let mut live_class_ranges = 0_usize;
        let mut hir_peak_ranges = 0_usize;
        let normalization_peak = if unicode {
            0
        } else {
            sources
                .iter()
                .map(|source| source.len())
                .max()
                .unwrap_or(0)
                .checked_add(size_of::<String>())
                .ok_or(RegexProgramError::Arithmetic)?
        };
        if regex_ast_pre_bytes(sources.len(), bytes)?
            .checked_add(normalization_peak)
            .ok_or(RegexProgramError::Arithmetic)?
            > peak_budget
        {
            return Err(RegexProgramError::SyntaxPeakLimit);
        }
        for original_source in sources {
            let normalized_source = if unicode {
                None
            } else {
                Some(normalize_pcre_byte_escapes(original_source)?)
            };
            let source = normalized_source.as_deref().unwrap_or(original_source);
            let source_units = original_source
                .len()
                .checked_add(1)
                .ok_or(RegexProgramError::Arithmetic)?;
            let ast = ast::parse::ParserBuilder::new()
                .nest_limit(limits.max_nest_depth)
                .octal(false)
                .build()
                .parse(source)
                .map_err(|error| match error.kind() {
                    ast::ErrorKind::NestLimitExceeded(limit) => RegexProgramError::NestLimit { limit: *limit },
                    _ => RegexProgramError::Syntax,
                })?;
            let ast_nodes = count_ast_nodes(&ast)?;
            ast_node_total = ast_node_total
                .checked_add(ast_nodes)
                .ok_or(RegexProgramError::Arithmetic)?;
            if ast_node_total > limits.max_ast_nodes {
                return Err(RegexProgramError::AstNodeLimit {
                    actual: ast_node_total,
                    limit: limits.max_ast_nodes,
                });
            }
            let projection = project_ast_ranges(&ast, source_units, case_insensitive, unicode, !unicode)?;
            hir_peak_ranges = hir_peak_ranges.max(
                live_class_ranges
                    .checked_add(projection.peak_ranges)
                    .ok_or(RegexProgramError::Arithmetic)?,
            );
            live_class_ranges = live_class_ranges
                .checked_add(projection.out_ranges)
                .ok_or(RegexProgramError::Arithmetic)?;
            // The AST is available now but HIR translation has not started.
            // Freeze the input-shaped translation bound before invoking the
            // infallibly allocating upstream translator.
            if regex_peak_for_projection(sources.len(), bytes, hir_peak_ranges, normalization_peak, limits)?
                > peak_budget
            {
                return Err(RegexProgramError::SyntaxPeakLimit);
            }
            if hirs.capacity() == 0 {
                hirs.try_reserve_exact(sources.len())
                    .map_err(|_| RegexProgramError::Allocation)?;
            }
            let hir = hir::translate::TranslatorBuilder::new()
                .utf8(false)
                .unicode(unicode)
                .case_insensitive(case_insensitive)
                .dot_matches_new_line(!unicode)
                .build()
                .translate(source, &ast)
                .map_err(|_| RegexProgramError::Syntax)?;
            let metrics = hir_metrics(&hir, source_units)?;
            if metrics.ranges > projection.out_ranges {
                return Err(RegexProgramError::ProjectionInvariant);
            }
            hir_total.nodes = hir_total
                .nodes
                .checked_add(metrics.nodes)
                .ok_or(RegexProgramError::Arithmetic)?;
            hir_total.ranges = hir_total
                .ranges
                .checked_add(metrics.ranges)
                .ok_or(RegexProgramError::Arithmetic)?;
            hir_total.literal_bytes = hir_total
                .literal_bytes
                .checked_add(metrics.literal_bytes)
                .ok_or(RegexProgramError::Arithmetic)?;
            hir_total.repetition_work = hir_total
                .repetition_work
                .checked_add(metrics.repetition_work)
                .ok_or(RegexProgramError::Arithmetic)?;
            hir_total.capture_groups = hir_total
                .capture_groups
                .checked_add(metrics.capture_groups)
                .ok_or(RegexProgramError::Arithmetic)?;
            hir_total.named_groups = hir_total
                .named_groups
                .checked_add(metrics.named_groups)
                .ok_or(RegexProgramError::Arithmetic)?;
            hir_total.max_capture_name_bytes = hir_total.max_capture_name_bytes.max(metrics.max_capture_name_bytes);
            hir_total.capture_name_bytes = hir_total
                .capture_name_bytes
                .checked_add(metrics.capture_name_bytes)
                .ok_or(RegexProgramError::Arithmetic)?;
            check_hir(&hir_total, limits)?;
            if matches!(captures, WhichCaptures::All) {
                let pattern_count = hirs.len().checked_add(1).ok_or(RegexProgramError::Arithmetic)?;
                check_capture_hir(&hir_total, pattern_count, limits)?;
            }
            hirs.push(hir);
        }
        if hir_total.ranges > live_class_ranges {
            return Err(RegexProgramError::ProjectionInvariant);
        }
        let nfa = NFA::compiler()
            .configure(
                thompson::Config::new()
                    .utf8(false)
                    .which_captures(captures)
                    .nfa_size_limit(Some(limits.max_nfa_bytes)),
            )
            .build_many_from_hir(&hirs)
            .map_err(|error| {
                if error.size_limit().is_some() {
                    RegexProgramError::NfaByteLimit {
                        limit: limits.max_nfa_bytes,
                    }
                } else {
                    RegexProgramError::Engine
                }
            })?;
        let vm = PikeVM::builder()
            .configure(PikeVM::config().match_kind(match_kind))
            .build_from_nfa(nfa)
            .map_err(|_| RegexProgramError::Engine)?;
        let retained_bytes = vm
            .get_nfa()
            .memory_usage()
            .checked_add(4 * size_of::<AtomicUsize>())
            .and_then(|value| value.checked_add(size_of::<CaptureShapeToken>()))
            .ok_or(RegexProgramError::Arithmetic)?;
        let program_peak_bound = limits
            .max_nfa_bytes
            .checked_add(6 * size_of::<AtomicUsize>())
            .and_then(|value| {
                sources
                    .len()
                    .checked_mul(size_of::<RegexPatternNameProjection>())
                    .and_then(|term| value.checked_add(term))
            })
            .and_then(|value| value.checked_add(size_of::<CaptureShapeToken>()))
            .ok_or(RegexProgramError::Arithmetic)?;
        if program_peak_bound > limits.max_program_bytes {
            return Err(RegexProgramError::ProgramByteLimit {
                actual: program_peak_bound,
                limit: limits.max_program_bytes,
            });
        }
        let cache = vm.create_cache();
        let capture_slot_len = if matches!(captures, WhichCaptures::All) {
            vm.create_captures().slots().len()
        } else {
            0
        };
        let scratch_wrapper = if matches!(captures, WhichCaptures::All) {
            size_of::<RegexCaptureScratch>()
        } else {
            size_of::<RegexBooleanScratch>()
        };
        let base_scratch = scratch_wrapper
            .checked_add(cache.memory_usage())
            .and_then(|value| value.checked_add(capture_slot_len.checked_mul(size_of::<usize>())?))
            .ok_or(RegexProgramError::Arithmetic)?;
        let epsilon_stack = vm
            .get_nfa()
            .states()
            .len()
            .checked_mul(4 * FOLLOW_EPSILON_UPPER_BYTES)
            .ok_or(RegexProgramError::Arithmetic)?;
        let scratch_upper = base_scratch
            .checked_add(epsilon_stack)
            .ok_or(RegexProgramError::Arithmetic)?;
        if scratch_upper > limits.max_scratch_bytes {
            return Err(RegexProgramError::ScratchByteLimit {
                actual: scratch_upper,
                limit: limits.max_scratch_bytes,
            });
        }
        let peak = regex_peak_for_projection(sources.len(), bytes, hir_peak_ranges, normalization_peak, limits)?;
        Ok(Self {
            vm,
            token: Arc::new(CaptureShapeToken { discriminator: 1 }),
            retained_bytes,
            program_peak_bound,
            peak,
            scratch_upper,
        })
    }
}

/// One immutable boolean byte-oriented regex program.
#[derive(Debug)]
pub struct CompiledRegexProgram {
    core: RegexCore,
    execution_hint: RegexExecutionHint,
}

#[derive(Debug, Default)]
enum RegexExecutionHint {
    #[default]
    None,
    Guard(Vec<AhoCorasick>),
    Simple(SimpleRegex),
}

#[derive(Debug, Clone, Copy, Default)]
struct SimpleRegex {
    literal: [u8; 16],
    len: u8,
    anchors: u8,
}
const REGEX_SIMPLE_PROOF_BYTES: usize = 256;

pub(crate) const REGEX_GUARD_PROOF_BYTES: usize = 64 * (256 + size_of::<usize>() + size_of::<&[u8]>()) + 256;
// Bounds the current native prefilter's 64 KiB two-gram table, packed buffers,
// growth overlap and small profile state. Recheck when that builder changes.
const REGEX_GUARD_ACCELERATION_BUILD_BYTES: usize = 128 * 1024;

#[derive(Clone, Copy)]
pub(crate) struct RegexGuardBudget {
    pub owned: usize,
    pub peak: usize,
    pub work: usize,
}

#[derive(Default)]
pub(crate) struct RegexGuardOutcome {
    pub work: usize,
    pub peak: usize,
    pub owned: usize,
}

impl CompiledRegexProgram {
    fn exact_match(&self, input: &[u8]) -> Option<bool> {
        let RegexExecutionHint::Simple(simple) = &self.execution_hint else {
            return None;
        };
        let literal = &simple.literal[..usize::from(simple.len)];
        Some(match simple.anchors {
            3 => input == literal,
            1 => input.starts_with(literal),
            2 => input.ends_with(literal),
            _ => memchr::memmem::find(input, literal).is_some(),
        })
    }

    pub(crate) fn lower_simple(
        &mut self,
        budget: RegexGuardBudget,
    ) -> Result<(RegexGuardOutcome, bool), RegexProgramError> {
        if !matches!(self.execution_hint, RegexExecutionHint::None) {
            return Err(RegexProgramError::ProjectionInvariant);
        }
        if self.core.vm.pattern_len() != 1 || budget.peak < REGEX_SIMPLE_PROOF_BYTES || budget.work == 0 {
            return Ok((RegexGuardOutcome::default(), false));
        }
        self.prove_simple(budget.work)
    }

    #[inline(never)]
    fn prove_simple(&mut self, work: usize) -> Result<(RegexGuardOutcome, bool), RegexProgramError> {
        use regex_automata::nfa::thompson::State;
        use regex_automata::util::look::Look;
        let mut outcome = RegexGuardOutcome {
            peak: REGEX_SIMPLE_PROOF_BYTES,
            ..RegexGuardOutcome::default()
        };
        let mut simple = SimpleRegex::default();
        let nfa = self.core.vm.get_nfa();
        let mut state = nfa
            .start_pattern(regex_automata::PatternID::ZERO)
            .ok_or(RegexProgramError::ProjectionInvariant)?;
        let available = work.min(64);
        while outcome.work < available {
            outcome.work += 1;
            let trans = match nfa
                .states()
                .get(state.as_usize())
                .ok_or(RegexProgramError::ProjectionInvariant)?
            {
                State::Look {
                    look: Look::Start,
                    next,
                } if simple.len == 0 && simple.anchors & 2 == 0 => {
                    simple.anchors |= 1;
                    state = *next;
                    continue;
                }
                State::Look { look: Look::End, next } => {
                    simple.anchors |= 2;
                    state = *next;
                    continue;
                }
                State::Capture { next, .. } => {
                    state = *next;
                    continue;
                }
                State::Union { alternates } if alternates.len() == 1 => {
                    state = alternates[0];
                    continue;
                }
                State::Match { pattern_id } if *pattern_id == regex_automata::PatternID::ZERO => {
                    self.execution_hint = RegexExecutionHint::Simple(simple);
                    return Ok((outcome, true));
                }
                State::ByteRange { trans } if trans.start == trans.end => trans,
                State::Sparse(trans)
                    if trans.transitions.len() == 1 && trans.transitions[0].start == trans.transitions[0].end =>
                {
                    &trans.transitions[0]
                }
                _ => return Ok((outcome, false)),
            };
            if simple.anchors & 2 != 0 || usize::from(simple.len) == simple.literal.len() || outcome.work == available {
                return Ok((outcome, false));
            }
            outcome.work += 1;
            simple.literal[usize::from(simple.len)] = trans.start;
            simple.len += 1;
            state = trans.next;
        }
        Ok((outcome, false))
    }
    pub fn prebuild_peak(limits: RegexProgramLimits) -> Result<usize, RegexProgramError> {
        regex_peak(limits)
    }

    pub fn compile_one(source: &str, limits: RegexProgramLimits) -> Result<Self, RegexProgramError> {
        Self::compile_one_with_peak_budget(source, limits, usize::MAX)
    }

    pub(crate) fn compile_one_with_peak_budget(
        source: &str,
        limits: RegexProgramLimits,
        peak_budget: usize,
    ) -> Result<Self, RegexProgramError> {
        let core = RegexCore::compile_many(
            &[source],
            limits,
            WhichCaptures::None,
            peak_budget,
            false,
            true,
            MatchKind::LeftmostFirst,
        )?;
        let actual = core.retained_bytes;
        if actual > limits.max_program_bytes {
            return Err(RegexProgramError::ProgramByteLimit {
                actual,
                limit: limits.max_program_bytes,
            });
        }
        Ok(Self {
            core,
            execution_hint: RegexExecutionHint::None,
        })
    }

    pub(crate) fn compile_one_pcre_bytes_with_peak_budget(
        source: &str,
        limits: RegexProgramLimits,
        peak_budget: usize,
    ) -> Result<Self, RegexProgramError> {
        let core = RegexCore::compile_many(
            &[source],
            limits,
            WhichCaptures::None,
            peak_budget,
            false,
            false,
            MatchKind::LeftmostFirst,
        )?;
        let actual = core.retained_bytes;
        if actual > limits.max_program_bytes {
            return Err(RegexProgramError::ProgramByteLimit {
                actual,
                limit: limits.max_program_bytes,
            });
        }
        Ok(Self {
            core,
            execution_hint: RegexExecutionHint::None,
        })
    }

    pub(crate) fn compile_one_ascii_case_insensitive_with_peak_budget(
        source: &str,
        limits: RegexProgramLimits,
        peak_budget: usize,
    ) -> Result<Self, RegexProgramError> {
        let core = RegexCore::compile_many(
            &[source],
            limits,
            WhichCaptures::None,
            peak_budget,
            true,
            false,
            MatchKind::LeftmostFirst,
        )?;
        let actual = core.retained_bytes;
        if actual > limits.max_program_bytes {
            return Err(RegexProgramError::ProgramByteLimit {
                actual,
                limit: limits.max_program_bytes,
            });
        }
        Ok(Self {
            core,
            execution_hint: RegexExecutionHint::None,
        })
    }

    pub fn create_scratch(&self) -> RegexBooleanScratch {
        debug_assert_eq!(self.core.token.discriminator, 1);
        RegexBooleanScratch {
            token: Arc::clone(&self.core.token),
            cache: self.core.vm.create_cache(),
        }
    }

    pub fn is_match(&self, haystack: &[u8], scratch: &mut RegexBooleanScratch) -> Result<bool, RegexScratchError> {
        if !Arc::ptr_eq(&scratch.token, &self.core.token) {
            return Err(RegexScratchError::ScratchShape);
        }
        Ok(self
            .exact_match(haystack)
            .unwrap_or_else(|| self.may_match(haystack) && self.core.vm.is_match(&mut scratch.cache, haystack)))
    }

    fn may_match(&self, input: &[u8]) -> bool {
        input.len() < 64
            || match &self.execution_hint {
                RegexExecutionHint::Guard(guards) => guards.first().is_none_or(|guard| guard.is_match(input)),
                _ => true,
            }
    }

    /// Attach an optional guard only after its owner has admitted all required programs.
    pub(crate) fn add_literal_guard(
        &mut self,
        budget: RegexGuardBudget,
    ) -> Result<RegexGuardOutcome, RegexProgramError> {
        if !matches!(self.execution_hint, RegexExecutionHint::None) {
            return Err(RegexProgramError::ProjectionInvariant);
        }
        if budget.peak < REGEX_GUARD_PROOF_BYTES
            || budget.work == 0
            || self.core.vm.pattern_len() == 0
            || self.core.vm.pattern_len() > 64
        {
            return Ok(RegexGuardOutcome::default());
        }
        self.build_literal_guard(budget)
    }

    // Keep the proof frame out of callers that did not admit its fixed storage.
    #[inline(never)]
    fn build_literal_guard(&mut self, budget: RegexGuardBudget) -> Result<RegexGuardOutcome, RegexProgramError> {
        use regex_automata::nfa::thompson::State;
        let mut outcome = RegexGuardOutcome {
            peak: REGEX_GUARD_PROOF_BYTES,
            ..RegexGuardOutcome::default()
        };
        let mut literals = [[0_u8; 256]; 64];
        let mut lengths = [0_usize; 64];
        let nfa = self.core.vm.get_nfa();
        let count = nfa.pattern_len();
        let available = budget.work.min(8192);
        for pattern in 0..count {
            let mut state = nfa
                .start_pattern(regex_automata::PatternID::must(pattern))
                .ok_or(RegexProgramError::ProjectionInvariant)?;
            while lengths[pattern] < 256 {
                if outcome.work == available {
                    return Ok(outcome);
                }
                outcome.work += 1;
                let transition = match nfa
                    .states()
                    .get(state.as_usize())
                    .ok_or(RegexProgramError::ProjectionInvariant)?
                {
                    State::Look {
                        look: regex_automata::util::look::Look::Start,
                        ..
                    } if lengths[pattern] == 0 => return Ok(outcome),
                    State::Look { next, .. } | State::Capture { next, .. } => {
                        state = *next;
                        continue;
                    }
                    State::Union { alternates } if alternates.len() == 1 => {
                        state = alternates[0];
                        continue;
                    }
                    State::ByteRange { trans } if trans.start == trans.end => trans,
                    State::Sparse(trans)
                        if trans.transitions.len() == 1 && trans.transitions[0].start == trans.transitions[0].end =>
                    {
                        &trans.transitions[0]
                    }
                    _ => break,
                };
                if outcome.work == available {
                    return Ok(outcome);
                }
                outcome.work += 1;
                literals[pattern][lengths[pattern]] = transition.start;
                lengths[pattern] += 1;
                state = transition.next;
            }
            if lengths[pattern] < 3 {
                return Ok(outcome);
            }
        }
        let bytes = lengths[..count].iter().sum::<usize>();
        let wrapper = size_of::<AhoCorasick>();
        let Some(room) = budget.owned.checked_sub(wrapper) else {
            return Ok(outcome);
        };
        let limits = AcBuildLimits {
            max_patterns: count,
            max_pattern_bytes: bytes,
            max_states: bytes + 1,
            max_transitions: bytes,
            max_outputs: count,
            max_compiled_bytes: room.min(256 * 1024),
            max_dfa_cells: 0,
        };
        let projection = crate::aho_corasick::phrase_build_projection(count, bytes, limits)
            .and_then(|value| value.checked_add(wrapper))
            .and_then(|value| value.checked_add(REGEX_GUARD_PROOF_BYTES))
            .and_then(|value| value.checked_add(REGEX_GUARD_ACCELERATION_BUILD_BYTES))
            .ok_or(RegexProgramError::Arithmetic)?;
        if projection > budget.peak {
            return Ok(outcome);
        }
        outcome.peak = projection;
        let mut storage = Vec::new();
        if storage.try_reserve_exact(1).is_err() {
            return Ok(outcome);
        }
        let actual_wrapper = storage
            .capacity()
            .checked_mul(wrapper)
            .ok_or(RegexProgramError::Arithmetic)?;
        if actual_wrapper != wrapper {
            return Err(RegexProgramError::ProjectionInvariant);
        }
        let built = AcBuilder::new()
            .limits(limits)
            .automaton_kind(crate::aho_corasick::AutomatonKind::SparseNfa)
            .match_kind(AcMatchKind::Standard)
            .prefilter(true)
            .build((0..count).map(|i| &literals[i][..lengths[i]]));
        let guard = match built {
            Ok(guard) => guard,
            Err(AcBuildError::SizeOverflow { .. } | AcBuildError::IndexOverflow { .. }) => {
                return Err(RegexProgramError::Arithmetic)
            }
            Err(_) => return Ok(outcome),
        };
        let owned = guard
            .memory_usage()
            .checked_add(actual_wrapper)
            .ok_or(RegexProgramError::Arithmetic)?;
        if owned > budget.owned
            || guard.estimated_peak_build_memory_usage() > projection - REGEX_GUARD_PROOF_BYTES - wrapper
        {
            return Err(RegexProgramError::ProjectionInvariant);
        }
        let retained = self
            .core
            .retained_bytes
            .checked_add(owned)
            .ok_or(RegexProgramError::Arithmetic)?;
        let program_bound = self
            .core
            .program_peak_bound
            .checked_add(owned)
            .ok_or(RegexProgramError::Arithmetic)?;
        storage.push(guard);
        self.execution_hint = RegexExecutionHint::Guard(storage);
        self.core.retained_bytes = retained;
        self.core.program_peak_bound = program_bound;
        outcome.owned = owned;
        Ok(outcome)
    }

    pub fn memory_usage(&self) -> usize {
        self.core.retained_bytes
    }

    pub const fn conservative_program_memory_bound(&self) -> usize {
        self.core.program_peak_bound
    }

    pub const fn required_scratch_bytes(&self) -> usize {
        self.core.scratch_upper
    }

    pub const fn estimated_peak_build_memory_usage(&self) -> usize {
        self.core.peak
    }
}

#[derive(Debug)]
pub struct RegexBooleanScratch {
    token: Arc<CaptureShapeToken>,
    cache: PikeCache,
}

/// Caller-owned reusable capture state tied to one exact program instance.
#[derive(Debug)]
pub struct RegexCaptureScratch {
    token: Arc<CaptureShapeToken>,
    cache: PikeCache,
    captures: Captures,
}

/// Borrowed captured byte ranges. Unmatched optional groups return `None`.
#[derive(Debug)]
pub struct RegexCaptureResults<'haystack, 'program, 'scratch> {
    program: &'program CompiledCaptureRegexProgram,
    captures: &'scratch Captures,
    _haystack: PhantomData<&'haystack [u8]>,
}

impl RegexCaptureResults<'_, '_, '_> {
    pub fn overall(&self) -> Option<Range<usize>> {
        self.group(0)
    }

    pub fn group(&self, index: usize) -> Option<Range<usize>> {
        let span = self.captures.get_group(index)?;
        Some(span.start..span.end)
    }

    pub fn group_by_name(&self, name: &str) -> Option<Range<usize>> {
        let index = self
            .program
            .names
            .iter()
            .position(|candidate| candidate.as_deref() == Some(name))?;
        self.group(index)
    }
}

/// One immutable capture-bearing byte-oriented regex program.
#[derive(Debug)]
pub struct CompiledCaptureRegexProgram {
    core: RegexCore,
    names: Vec<Option<String>>,
    metadata_bytes: usize,
    retained_bytes: usize,
}

impl CompiledCaptureRegexProgram {
    pub fn compile_one(source: &str, limits: RegexProgramLimits) -> Result<Self, RegexProgramError> {
        Self::compile_one_with_peak_budget(source, limits, usize::MAX)
    }

    pub(crate) fn compile_one_with_peak_budget(
        source: &str,
        limits: RegexProgramLimits,
        peak_budget: usize,
    ) -> Result<Self, RegexProgramError> {
        Self::compile_one_with_options(source, limits, peak_budget, true)
    }

    pub(crate) fn compile_one_pcre_bytes_with_peak_budget(
        source: &str,
        limits: RegexProgramLimits,
        peak_budget: usize,
    ) -> Result<Self, RegexProgramError> {
        Self::compile_one_with_options(source, limits, peak_budget, false)
    }

    fn compile_one_with_options(
        source: &str,
        limits: RegexProgramLimits,
        peak_budget: usize,
        unicode: bool,
    ) -> Result<Self, RegexProgramError> {
        let mut core = RegexCore::compile_many(
            &[source],
            limits,
            WhichCaptures::All,
            peak_budget,
            false,
            unicode,
            MatchKind::LeftmostFirst,
        )?;
        let group_info = core.vm.get_nfa().group_info();
        let group_count = group_info.group_len(regex_automata::PatternID::ZERO);
        if group_count > limits.max_capture_slots {
            return Err(RegexProgramError::CaptureSlotLimit {
                actual: group_count,
                limit: limits.max_capture_slots,
            });
        }
        let mut names = Vec::new();
        names
            .try_reserve_exact(group_count)
            .map_err(|_| RegexProgramError::Allocation)?;
        let mut named_count = 0_usize;
        let mut name_bytes = 0_usize;
        for name in group_info.pattern_names(regex_automata::PatternID::ZERO) {
            let owned = if let Some(name) = name {
                named_count = named_count.checked_add(1).ok_or(RegexProgramError::Arithmetic)?;
                if named_count > limits.max_named_groups {
                    return Err(RegexProgramError::NamedGroupLimit {
                        actual: named_count,
                        limit: limits.max_named_groups,
                    });
                }
                if name.len() > limits.max_capture_name_bytes {
                    return Err(RegexProgramError::CaptureNameLimit {
                        actual: name.len(),
                        limit: limits.max_capture_name_bytes,
                    });
                }
                name_bytes = name_bytes
                    .checked_add(name.len())
                    .ok_or(RegexProgramError::Arithmetic)?;
                if name_bytes > limits.max_total_capture_name_bytes {
                    return Err(RegexProgramError::CaptureNameByteLimit {
                        actual: name_bytes,
                        limit: limits.max_total_capture_name_bytes,
                    });
                }
                let mut value = String::new();
                value
                    .try_reserve_exact(name.len())
                    .map_err(|_| RegexProgramError::Allocation)?;
                value.push_str(name);
                Some(value)
            } else {
                None
            };
            names.push(owned);
        }
        let metadata_bytes = group_count
            .checked_mul(size_of::<Option<String>>())
            .and_then(|bytes| bytes.checked_add(name_bytes))
            .ok_or(RegexProgramError::Arithmetic)?;
        let aligned_names = names.iter().try_fold(0_usize, |total, name| {
            let bytes = name.as_ref().map_or(0, String::len);
            align_up(bytes, std::mem::align_of::<usize>())
                .and_then(|bytes| total.checked_add(bytes))
                .ok_or(RegexProgramError::Arithmetic)
        })?;
        let auxiliary_peak = 4_usize
            .checked_mul(size_of::<AtomicUsize>())
            .and_then(|value| {
                named_count
                    .checked_mul(2 * size_of::<AtomicUsize>())
                    .and_then(|term| value.checked_add(term))
            })
            .and_then(|value| value.checked_add(aligned_names))
            .and_then(|value| {
                named_count
                    .checked_mul(4 * size_of::<RegexNameMapEntryProjection>())
                    .and_then(|term| value.checked_add(term))
            })
            .and_then(|value| value.checked_add(size_of::<RegexPatternNameProjection>()))
            .and_then(|value| {
                group_count
                    .checked_mul(size_of::<Option<std::sync::Arc<str>>>())
                    .and_then(|term| value.checked_add(term))
            })
            .ok_or(RegexProgramError::Arithmetic)?;
        let physical_bound = limits
            .max_nfa_bytes
            .checked_add(auxiliary_peak)
            .and_then(|value| value.checked_add(metadata_bytes))
            .ok_or(RegexProgramError::Arithmetic)?;
        if physical_bound > limits.max_program_bytes {
            return Err(RegexProgramError::ProgramByteLimit {
                actual: physical_bound,
                limit: limits.max_program_bytes,
            });
        }
        core.program_peak_bound = core.program_peak_bound.max(physical_bound);
        let actual = core
            .retained_bytes
            .checked_add(metadata_bytes)
            .ok_or(RegexProgramError::Arithmetic)?;
        if actual > limits.max_program_bytes {
            return Err(RegexProgramError::ProgramByteLimit {
                actual,
                limit: limits.max_program_bytes,
            });
        }
        Ok(Self {
            core,
            names,
            metadata_bytes,
            retained_bytes: actual,
        })
    }

    pub fn is_match(&self, haystack: &[u8], scratch: &mut RegexCaptureScratch) -> Result<bool, RegexScratchError> {
        if !Arc::ptr_eq(&scratch.token, &self.core.token) {
            return Err(RegexScratchError::ScratchShape);
        }
        Ok(self.core.vm.is_match(&mut scratch.cache, haystack))
    }

    pub fn capture_len(&self) -> usize {
        self.names.len()
    }

    pub fn capture_name(&self, index: usize) -> Option<&str> {
        self.names.get(index).and_then(Option::as_deref)
    }

    /// Creates admitted runtime scratch. Upstream allocation is infallible;
    /// process OOM may abort here or during the first search.
    pub fn create_scratch(&self) -> RegexCaptureScratch {
        debug_assert_eq!(self.core.token.discriminator, 1);
        RegexCaptureScratch {
            token: Arc::clone(&self.core.token),
            cache: self.core.vm.create_cache(),
            captures: self.core.vm.create_captures(),
        }
    }

    pub fn captures<'haystack, 'program, 'scratch>(
        &'program self,
        haystack: &'haystack [u8],
        scratch: &'scratch mut RegexCaptureScratch,
    ) -> Result<RegexCaptureResults<'haystack, 'program, 'scratch>, RegexScratchError> {
        if !Arc::ptr_eq(&scratch.token, &self.core.token) {
            return Err(RegexScratchError::ScratchShape);
        }
        self.core
            .vm
            .captures(&mut scratch.cache, haystack, &mut scratch.captures);
        Ok(RegexCaptureResults {
            program: self,
            captures: &scratch.captures,
            _haystack: PhantomData,
        })
    }

    pub fn engine_memory_usage(&self) -> usize {
        self.core.retained_bytes
    }

    pub const fn capture_metadata_memory_usage(&self) -> usize {
        self.metadata_bytes
    }

    pub fn memory_usage(&self) -> usize {
        self.retained_bytes
    }

    pub const fn conservative_program_memory_bound(&self) -> usize {
        self.core.program_peak_bound
    }

    pub const fn required_scratch_bytes(&self) -> usize {
        self.core.scratch_upper
    }

    pub const fn estimated_peak_build_memory_usage(&self) -> usize {
        self.core.peak
    }
}

fn regex_peak(limits: RegexProgramLimits) -> Result<usize, RegexProgramError> {
    regex_peak_for(1, limits.max_source_bytes, limits)
}

fn regex_ast_pre_bytes(patterns: usize, bytes: usize) -> Result<usize, RegexProgramError> {
    let ast_units = patterns.checked_add(bytes).ok_or(RegexProgramError::Arithmetic)?;
    ast_units
        .checked_mul(2)
        .and_then(|value| value.checked_mul(size_of::<Ast>() + size_of::<usize>()))
        .and_then(|value| {
            ast_units
                .checked_mul(size_of::<ClassRangeProjection>() + size_of::<usize>() + size_of::<ProjectionFlags>())
                .and_then(|term| value.checked_add(term))
        })
        .ok_or(RegexProgramError::Arithmetic)
}

fn regex_hir_pre_bytes(patterns: usize, bytes: usize, hir_peak_ranges: usize) -> Result<usize, RegexProgramError> {
    let ast_units = patterns.checked_add(bytes).ok_or(RegexProgramError::Arithmetic)?;
    ast_units
        .checked_mul(2)
        .and_then(|value| value.checked_mul(size_of::<Hir>() + size_of::<usize>()))
        .and_then(|value| value.checked_add(hir_peak_ranges.checked_mul(size_of::<hir::ClassUnicodeRange>())?))
        .and_then(|value| value.checked_add(bytes.checked_mul(4)?))
        .and_then(|value| value.checked_add(patterns.checked_mul(size_of::<&Hir>())?))
        .and_then(|value| {
            ast_units
                .checked_mul(2 * size_of::<usize>())
                .and_then(|term| value.checked_add(term))
        })
        .ok_or(RegexProgramError::Arithmetic)
}

fn regex_peak_for_projection(
    patterns: usize,
    bytes: usize,
    hir_peak_ranges: usize,
    normalization_peak: usize,
    limits: RegexProgramLimits,
) -> Result<usize, RegexProgramError> {
    let syntax = regex_ast_pre_bytes(patterns, bytes)?
        .checked_add(regex_hir_pre_bytes(patterns, bytes, hir_peak_ranges)?)
        .and_then(|value| value.checked_add(normalization_peak))
        .ok_or(RegexProgramError::Arithmetic)?;
    syntax
        .checked_add(
            limits
                .max_nfa_bytes
                .checked_mul(2)
                .ok_or(RegexProgramError::Arithmetic)?,
        )
        .and_then(|bytes| bytes.checked_add(limits.max_program_bytes))
        .and_then(|bytes| bytes.checked_add(limits.max_scratch_bytes))
        .ok_or(RegexProgramError::Arithmetic)
}

fn regex_peak_for(patterns: usize, bytes: usize, limits: RegexProgramLimits) -> Result<usize, RegexProgramError> {
    let source_units = patterns.checked_add(bytes).ok_or(RegexProgramError::Arithmetic)?;
    // One source unit can introduce at most one range-producing AST atom or
    // one combining operation. Three universe-sized vectors cover both live
    // operands and the operation result without inspecting source content.
    let hir_peak_ranges = source_units
        .checked_mul(3)
        .and_then(|value| value.checked_mul(MAX_UNICODE_CLASS_RANGES))
        .ok_or(RegexProgramError::Arithmetic)?;
    let normalization_peak = bytes
        .checked_add(size_of::<String>())
        .ok_or(RegexProgramError::Arithmetic)?;
    regex_peak_for_projection(patterns, bytes, hir_peak_ranges, normalization_peak, limits)
}

fn normalize_pcre_byte_escapes(source: &str) -> Result<String, RegexProgramError> {
    let bytes = source.as_bytes();
    let mut output = String::new();
    output
        .try_reserve_exact(bytes.len())
        .map_err(|_| RegexProgramError::Allocation)?;
    let mut index = 0_usize;
    while index < bytes.len() {
        if bytes[index] != b'\\' {
            let end = bytes[index..]
                .iter()
                .position(|byte| *byte == b'\\')
                .map_or(bytes.len(), |offset| index + offset);
            output.push_str(&source[index..end]);
            index = end;
            continue;
        }
        let run_start = index;
        while index < bytes.len() && bytes[index] == b'\\' {
            index += 1;
        }
        let run_len = index - run_start;
        for _ in 0..run_len {
            output.push('\\');
        }
        if run_len % 2 == 1
            && index + 4 < bytes.len()
            && bytes[index] == b'x'
            && bytes[index + 1] == b'{'
            && hex_nibble(bytes[index + 2]).is_some()
            && hex_nibble(bytes[index + 3]).is_some()
            && bytes[index + 4] == b'}'
        {
            output.push('x');
            output.push(char::from(bytes[index + 2]));
            output.push(char::from(bytes[index + 3]));
            index += 5;
        }
    }
    Ok(output)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Pattern counts classified before matcher allocation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MatcherPatternCounts {
    pub sensitive_phrases: usize,
    pub insensitive_phrases: usize,
    pub regexes: usize,
}

/// Source-byte counts classified before matcher allocation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MatcherPatternBytes {
    pub sensitive_phrases: usize,
    pub insensitive_phrases: usize,
    pub regexes: usize,
}

/// Stable checked pre-build matcher memory projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatcherBuildProjection {
    working_partition_bytes: usize,
    retained_map_bytes: usize,
    sensitive_phrase_peak: usize,
    insensitive_phrase_peak: usize,
    regex_peak: usize,
}

impl MatcherBuildProjection {
    pub fn for_counts(
        counts: MatcherPatternCounts,
        bytes: MatcherPatternBytes,
        limits: MatcherLimits,
    ) -> Result<Self, MatcherCompileError> {
        let phrases = counts
            .sensitive_phrases
            .checked_add(counts.insensitive_phrases)
            .ok_or(MatcherCompileError::PeakArithmetic)?;
        let patterns = phrases
            .checked_add(counts.regexes)
            .ok_or(MatcherCompileError::PeakArithmetic)?;
        if patterns > limits.max_patterns {
            return Err(MatcherCompileError::PatternLimit {
                actual: patterns,
                limit: limits.max_patterns,
            });
        }
        let pattern_bytes = bytes
            .sensitive_phrases
            .checked_add(bytes.insensitive_phrases)
            .and_then(|value| value.checked_add(bytes.regexes))
            .ok_or(MatcherCompileError::PeakArithmetic)?;
        if pattern_bytes > limits.max_pattern_bytes {
            return Err(MatcherCompileError::PatternByteLimit {
                actual: pattern_bytes,
                limit: limits.max_pattern_bytes,
            });
        }
        let working_partition_bytes = phrases
            .checked_mul(size_of::<(PatternId, &str)>())
            .and_then(|value| {
                counts
                    .regexes
                    .checked_mul(size_of::<RegexPatternEntry>() + size_of::<&str>())
                    .and_then(|regex| value.checked_add(regex))
            })
            .ok_or(MatcherCompileError::PeakArithmetic)?;
        let retained_map_bytes = patterns
            .checked_mul(size_of::<PatternId>())
            .and_then(|value| {
                counts
                    .regexes
                    .checked_mul(size_of::<usize>())
                    .and_then(|ends| value.checked_add(ends))
            })
            .ok_or(MatcherCompileError::PeakArithmetic)?;
        let ac_limits = AcBuildLimits {
            max_patterns: limits.max_patterns,
            max_pattern_bytes: limits.max_pattern_bytes,
            max_states: limits.max_phrase_states,
            max_transitions: limits.max_phrase_transitions,
            max_outputs: limits.max_phrase_outputs,
            max_compiled_bytes: limits.max_phrase_compiled_bytes,
            max_dfa_cells: limits.max_phrase_dfa_cells,
        };
        let sensitive_phrase_peak =
            crate::aho_corasick::phrase_build_projection(counts.sensitive_phrases, bytes.sensitive_phrases, ac_limits)
                .ok_or(MatcherCompileError::PeakArithmetic)?;
        let insensitive_phrase_peak = crate::aho_corasick::phrase_build_projection(
            counts.insensitive_phrases,
            bytes.insensitive_phrases,
            ac_limits,
        )
        .ok_or(MatcherCompileError::PeakArithmetic)?;
        let regex_peak = if counts.regexes == 0 {
            0
        } else {
            regex_peak_for(counts.regexes, bytes.regexes, limits.into())
                .map_err(|_| MatcherCompileError::PeakArithmetic)?
        };
        Ok(Self {
            working_partition_bytes,
            retained_map_bytes,
            sensitive_phrase_peak,
            insensitive_phrase_peak,
            regex_peak,
        })
    }

    pub const fn working_partition_bytes(self) -> usize {
        self.working_partition_bytes
    }
    pub const fn retained_map_bytes(self) -> usize {
        self.retained_map_bytes
    }
    pub const fn sensitive_phrase_peak(self) -> usize {
        self.sensitive_phrase_peak
    }
    pub const fn insensitive_phrase_peak(self) -> usize {
        self.insensitive_phrase_peak
    }
    pub const fn regex_peak(self) -> usize {
        self.regex_peak
    }
    pub fn conservative_peak_bytes(self) -> Result<usize, MatcherCompileError> {
        self.working_partition_bytes
            .checked_add(self.retained_map_bytes)
            .and_then(|value| value.checked_add(self.sensitive_phrase_peak))
            .and_then(|value| value.checked_add(self.insensitive_phrase_peak))
            .and_then(|value| value.checked_add(self.regex_peak))
            .ok_or(MatcherCompileError::PeakArithmetic)
    }
}

/// Stable identifier assigned from a pattern's source-order position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PatternId(usize);

impl PatternId {
    /// Return the zero-based source-order position.
    pub const fn as_usize(self) -> usize {
        self.0
    }
}

/// One pattern in a matching group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternSpec<'a> {
    /// Exact substring phrase matching, corresponding to the matching core of
    /// SecLang `@pm`.
    Phrase {
        /// Phrase bytes. Empty phrases are rejected.
        value: &'a str,
        /// Whether ASCII letters compare without case distinctions.
        ascii_case_insensitive: bool,
    },
    /// Exact regular-expression matching for the supported Rust-regex subset.
    Regex(&'a str),
}

impl PatternSpec<'_> {
    fn source_len(self) -> usize {
        match self {
            Self::Phrase { value, .. } | Self::Regex(value) => value.len(),
        }
    }
}

/// Error returned while compiling a matching group.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum MatcherCompileError {
    /// Checked pre-build memory projection overflowed.
    #[error("matcher pre-build memory projection overflowed")]
    PeakArithmetic,
    /// A bounded matcher working or retained vector could not be reserved.
    #[error("cannot allocate bounded matcher arena {operation}")]
    Allocation { operation: &'static str },
    /// The group exceeds its configured pattern-count bound.
    #[error("matcher has {actual} patterns, exceeding the configured limit of {limit}; split the rule group or raise the compile limit")]
    PatternLimit { actual: usize, limit: usize },
    /// The group exceeds its configured combined source-byte bound.
    #[error("matcher has {actual} pattern bytes, exceeding the configured limit of {limit}; reduce the rule set or raise the compile limit")]
    PatternByteLimit { actual: usize, limit: usize },
    #[error("regex pattern {pattern_id} has {actual} source bytes, exceeding the limit of {limit}")]
    RegexSourceByteLimit {
        pattern_id: usize,
        actual: usize,
        limit: usize,
    },
    #[error("regex patterns have {actual} cumulative source bytes, exceeding the limit of {limit}")]
    RegexSourceGroupByteLimit { actual: usize, limit: usize },
    /// An empty phrase was supplied.
    #[error("phrase pattern {pattern_id} is empty; remove it or use an operator with explicit empty-input semantics")]
    EmptyPhrase { pattern_id: usize },
    /// A phrase automaton exceeds its state bound.
    #[error("phrase automaton requires more than {limit} states; split the matching group or raise the compile limit")]
    PhraseStateLimit { limit: usize },
    /// A phrase automaton exceeds its transition bound.
    #[error(
        "phrase automaton requires more than {limit} transitions; split the matching group or raise the compile limit"
    )]
    PhraseTransitionLimit { limit: usize },
    /// A phrase automaton exceeds its direct-output bound.
    #[error("phrase automaton requires more than {limit} direct outputs; split the matching group or raise the compile limit")]
    PhraseOutputLimit { limit: usize },
    /// A phrase automaton exceeds its estimated immutable-byte bound.
    #[error("phrase automaton requires more than {limit} immutable bytes; split the matching group or raise the compile limit")]
    PhraseCompiledByteLimit { limit: usize },
    /// A phrase automaton failed for another bounded compilation reason.
    #[error("phrase automaton cannot be compiled: {message}")]
    PhraseBuild { message: String },
    /// One or more regular expressions cannot be compiled.
    #[error("regex pattern {pattern_id:?} cannot be compiled for the supported byte profile")]
    Regex { pattern_id: Option<usize> },
    /// A compiled regex exceeds its complete retained-program bound.
    #[error("compiled regex requires {actual} bytes, exceeding the configured limit of {limit}")]
    RegexProgramByteLimit { actual: usize, limit: usize },
}

/// Error returned when scratch memory belongs to a different matcher shape.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum MatcherScanError {
    /// The compiled phrase search mode cannot serve an all-pattern scan.
    #[error("compiled phrase matcher mode does not support the requested scan; rebuild the rule generation")]
    PhraseSearchMode,
    /// Scratch memory was created for another compiled matcher.
    #[error(
        "matcher scratch shape does not match the compiled matcher; create scratch from the active rule generation"
    )]
    ScratchShape,
    /// A compiled matcher contained an invalid internal pattern identifier.
    #[error("compiled matcher contains an invalid pattern identifier; discard the rule generation and recompile it")]
    InvalidPatternId,
}

#[derive(Debug)]
struct PhraseGroup {
    matcher: AhoCorasick,
    pattern_ids: Vec<PatternId>,
}

impl PhraseGroup {
    fn compile(
        patterns: &[(PatternId, &str)],
        pattern_ids: Vec<PatternId>,
        ascii_case_insensitive: bool,
        limits: MatcherLimits,
    ) -> Result<Option<Self>, MatcherCompileError> {
        if patterns.is_empty() {
            return Ok(None);
        }

        let mut builder = AcBuilder::new();
        builder
            .ascii_case_insensitive(ascii_case_insensitive)
            .limits(AcBuildLimits {
                max_patterns: limits.max_patterns,
                max_pattern_bytes: limits.max_pattern_bytes,
                max_states: limits.max_phrase_states,
                max_transitions: limits.max_phrase_transitions,
                max_outputs: limits.max_phrase_outputs,
                max_compiled_bytes: limits.max_phrase_compiled_bytes,
                max_dfa_cells: limits.max_phrase_dfa_cells,
            });
        let matcher = builder
            .build(patterns.iter().map(|(_, pattern)| pattern.as_bytes()))
            .map_err(map_phrase_build_error)?;
        Ok(Some(Self { matcher, pattern_ids }))
    }

    fn create_pattern_set(&self) -> AcPatternSet {
        self.matcher.create_pattern_set()
    }

    fn memory_usage(&self) -> usize {
        self.matcher
            .memory_usage()
            .saturating_add(self.pattern_ids.len().saturating_mul(size_of::<PatternId>()))
    }

    fn estimated_peak_build_memory_usage(&self) -> usize {
        self.matcher
            .estimated_peak_build_memory_usage()
            .saturating_add(self.pattern_ids.len().saturating_mul(size_of::<PatternId>()))
    }

    fn scan(
        &self,
        haystack: &[u8],
        phrase_matches: &mut AcPatternSet,
        matched: &mut [u64],
    ) -> Result<(), MatcherScanError> {
        self.matcher
            .scan_pattern_set(haystack, phrase_matches)
            .map_err(map_phrase_scan_error)?;
        for local_id in phrase_matches.iter() {
            let Some(pattern_id) = self.pattern_ids.get(local_id.as_usize()) else {
                return Err(MatcherScanError::InvalidPatternId);
            };
            set_match(matched, *pattern_id)?;
        }
        Ok(())
    }

    fn scan_reference(
        &self,
        haystack: &[u8],
        phrase_matches: &mut AcPatternSet,
        matched: &mut [u64],
    ) -> Result<(), MatcherScanError> {
        self.matcher
            .scan_pattern_set_reference(haystack, phrase_matches)
            .map_err(map_phrase_scan_error)?;
        for local_id in phrase_matches.iter() {
            let Some(pattern_id) = self.pattern_ids.get(local_id.as_usize()) else {
                return Err(MatcherScanError::InvalidPatternId);
            };
            set_match(matched, *pattern_id)?;
        }
        Ok(())
    }

    fn create_stream_state(&self) -> Result<PhraseStreamState, MatcherScanError> {
        Ok(PhraseStreamState {
            state: self
                .matcher
                .try_stream_state()
                .map_err(|_| MatcherScanError::PhraseSearchMode)?,
            matches: self.matcher.create_pattern_set(),
        })
    }

    fn scan_stream(
        &self,
        state: &mut PhraseStreamState,
        chunk: &[u8],
        end_of_stream: bool,
        matched: &mut [u64],
    ) -> Result<(), MatcherScanError> {
        self.matcher
            .try_stream_pattern_set_chunk(&mut state.state, chunk, end_of_stream, &mut state.matches)
            .map_err(map_phrase_scan_error)?;
        for local_id in state.matches.iter() {
            let Some(pattern_id) = self.pattern_ids.get(local_id.as_usize()) else {
                return Err(MatcherScanError::InvalidPatternId);
            };
            set_match(matched, *pattern_id)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct PhraseStreamState {
    state: AcStreamState,
    matches: AcPatternSet,
}

impl PhraseStreamState {
    fn memory_usage(&self) -> usize {
        self.state.memory_usage().saturating_add(self.matches.memory_usage())
    }
}

fn map_phrase_build_error(error: AcBuildError) -> MatcherCompileError {
    match error {
        AcBuildError::PatternLimit { actual, limit } => MatcherCompileError::PatternLimit { actual, limit },
        AcBuildError::PatternByteLimit { actual, limit } => MatcherCompileError::PatternByteLimit { actual, limit },
        AcBuildError::StateLimit { limit, .. } => MatcherCompileError::PhraseStateLimit { limit },
        AcBuildError::TransitionLimit { limit, .. } => MatcherCompileError::PhraseTransitionLimit { limit },
        AcBuildError::OutputLimit { limit, .. } => MatcherCompileError::PhraseOutputLimit { limit },
        AcBuildError::CompiledByteLimit { limit, .. } => MatcherCompileError::PhraseCompiledByteLimit { limit },
        other => MatcherCompileError::PhraseBuild {
            message: other.to_string(),
        },
    }
}

fn map_phrase_scan_error(error: AcMatchError) -> MatcherScanError {
    match error {
        AcMatchError::UnsupportedOverlapping { .. } => MatcherScanError::PhraseSearchMode,
        AcMatchError::PatternSetShape { .. } => MatcherScanError::ScratchShape,
        AcMatchError::InvalidPatternId { .. } => MatcherScanError::InvalidPatternId,
        _ => MatcherScanError::PhraseSearchMode,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CapturePhraseMatch {
    pattern_id: usize,
    start: usize,
    end: usize,
}

impl CapturePhraseMatch {
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "transaction execution consumes captured phrase spans")
    )]
    pub(crate) const fn pattern_id(self) -> usize {
        self.pattern_id
    }

    pub(crate) const fn start(self) -> usize {
        self.start
    }

    pub(crate) const fn end(self) -> usize {
        self.end
    }
}

/// Dedicated native phrase program for rules that need the selected match span.
#[derive(Debug)]
pub(crate) struct CompiledCapturePhraseProgram {
    matcher: AhoCorasick,
}

impl CompiledCapturePhraseProgram {
    pub(crate) fn compile(patterns: &[&str], limits: MatcherLimits) -> Result<Self, MatcherCompileError> {
        if patterns.len() > limits.max_patterns {
            return Err(MatcherCompileError::PatternLimit {
                actual: patterns.len(),
                limit: limits.max_patterns,
            });
        }
        let mut pattern_bytes = 0_usize;
        for (pattern_id, pattern) in patterns.iter().enumerate() {
            if pattern.is_empty() {
                return Err(MatcherCompileError::EmptyPhrase { pattern_id });
            }
            pattern_bytes = pattern_bytes
                .checked_add(pattern.len())
                .ok_or(MatcherCompileError::PeakArithmetic)?;
        }
        if pattern_bytes > limits.max_pattern_bytes {
            return Err(MatcherCompileError::PatternByteLimit {
                actual: pattern_bytes,
                limit: limits.max_pattern_bytes,
            });
        }

        let mut builder = AcBuilder::new();
        builder
            .ascii_case_insensitive(true)
            .match_kind(AcMatchKind::LeftmostLongest)
            .limits(AcBuildLimits {
                max_patterns: limits.max_patterns,
                max_pattern_bytes: limits.max_pattern_bytes,
                max_states: limits.max_phrase_states,
                max_transitions: limits.max_phrase_transitions,
                max_outputs: limits.max_phrase_outputs,
                max_compiled_bytes: limits.max_phrase_compiled_bytes,
                max_dfa_cells: limits.max_phrase_dfa_cells,
            });
        let matcher = builder
            .build(patterns.iter().map(|pattern| pattern.as_bytes()))
            .map_err(map_phrase_build_error)?;
        Ok(Self { matcher })
    }

    pub(crate) fn find(&self, haystack: &[u8]) -> Option<CapturePhraseMatch> {
        self.matcher.find(haystack).map(|matched| CapturePhraseMatch {
            pattern_id: matched.pattern_id().as_usize(),
            start: matched.start(),
            end: matched.end(),
        })
    }

    pub(crate) fn find_reference(&self, haystack: &[u8]) -> Option<CapturePhraseMatch> {
        self.matcher.find_reference(haystack).map(|matched| CapturePhraseMatch {
            pattern_id: matched.pattern_id().as_usize(),
            start: matched.start(),
            end: matched.end(),
        })
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "compiled generation views expose phrase cardinality")
    )]
    pub(crate) fn pattern_count(&self) -> usize {
        self.matcher.pattern_count()
    }

    pub(crate) fn memory_usage(&self) -> usize {
        self.matcher.memory_usage()
    }

    pub(crate) fn estimated_peak_build_memory_usage(&self) -> usize {
        self.matcher.estimated_peak_build_memory_usage()
    }
}

fn set_match(words: &mut [u64], pattern_id: PatternId) -> Result<(), MatcherScanError> {
    let word_index = pattern_id.0 / u64::BITS as usize;
    let bit_index = pattern_id.0 % u64::BITS as usize;
    let Some(word) = words.get_mut(word_index) else {
        return Err(MatcherScanError::InvalidPatternId);
    };
    *word |= 1_u64 << bit_index;
    Ok(())
}

#[derive(Clone, Copy)]
struct RegexPatternEntry<'source> {
    logical: PatternId,
    source: &'source str,
    physical: usize,
}

fn compare_regex_sources(a: &str, b: &str, remaining: &mut usize) -> Result<std::cmp::Ordering, ()> {
    *remaining = remaining.checked_sub(1).ok_or(())?;
    for (&a, &b) in a.as_bytes().iter().zip(b.as_bytes()) {
        *remaining = remaining.checked_sub(1).ok_or(())?;
        if a != b {
            return Ok(a.cmp(&b));
        }
    }
    Ok(a.len().cmp(&b.len()))
}

// Failure leaves entries permuted. The caller must rebuild from the original
// specs before compiling the unshared fallback; partial IDs are never published.
fn canonicalize_regex_entries(entries: &mut [RegexPatternEntry<'_>], mut remaining: usize) -> Result<usize, ()> {
    crate::bounded_sort::heapsort_by(entries, |a, b| {
        Ok(compare_regex_sources(a.source, b.source, &mut remaining)?.then(a.logical.cmp(&b.logical)))
    })?;
    let Some(first) = entries.first_mut() else {
        return Ok(0);
    };
    first.physical = 0;
    let mut previous = first.source;
    let mut physical = 0_usize;
    for entry in &mut entries[1..] {
        if compare_regex_sources(previous, entry.source, &mut remaining)? != std::cmp::Ordering::Equal {
            physical = physical.checked_add(1).ok_or(())?;
            previous = entry.source;
        }
        entry.physical = physical;
    }
    physical.checked_add(1).ok_or(())
}

/// Immutable compiled matcher for one semantically equivalent input group.
#[derive(Debug)]
pub struct CompiledMatcher {
    pattern_count: usize,
    sensitive_phrases: Option<PhraseGroup>,
    insensitive_phrases: Option<PhraseGroup>,
    regex: Option<CompiledRegexProgram>,
    regex_pattern_ids: Vec<PatternId>,
    // Empty for one-to-one mapping; otherwise each physical pattern owns a slice
    // ending here in the flat logical-ID vector.
    regex_fanout_ends: Vec<usize>,
}

impl CompiledMatcher {
    pub(crate) fn regex_program_mut(&mut self) -> Option<&mut CompiledRegexProgram> {
        self.regex.as_mut()
    }
    /// Compile a bounded group and assign stable IDs in source order.
    pub fn compile(patterns: &[PatternSpec<'_>], limits: MatcherLimits) -> Result<Self, MatcherCompileError> {
        Self::compile_with_regex_unicode(patterns, limits, true)
    }

    pub(crate) fn compile_pcre_bytes(
        patterns: &[PatternSpec<'_>],
        limits: MatcherLimits,
    ) -> Result<Self, MatcherCompileError> {
        Self::compile_with_regex_unicode(patterns, limits, false)
    }

    fn compile_with_regex_unicode(
        patterns: &[PatternSpec<'_>],
        limits: MatcherLimits,
        regex_unicode: bool,
    ) -> Result<Self, MatcherCompileError> {
        Self::compile_with_regex_unicode_and_work_limit(patterns, limits, regex_unicode, usize::MAX)
    }

    fn compile_with_regex_unicode_and_work_limit(
        patterns: &[PatternSpec<'_>],
        limits: MatcherLimits,
        regex_unicode: bool,
        canonical_work_limit: usize,
    ) -> Result<Self, MatcherCompileError> {
        if patterns.len() > limits.max_patterns {
            return Err(MatcherCompileError::PatternLimit {
                actual: patterns.len(),
                limit: limits.max_patterns,
            });
        }
        let mut regex_source_bytes = 0_usize;
        for (pattern_id, pattern) in patterns.iter().enumerate() {
            let PatternSpec::Regex(source) = pattern else {
                continue;
            };
            if source.len() > limits.max_regex_source_bytes_per_pattern {
                return Err(MatcherCompileError::RegexSourceByteLimit {
                    pattern_id,
                    actual: source.len(),
                    limit: limits.max_regex_source_bytes_per_pattern,
                });
            }
            regex_source_bytes = match regex_source_bytes.checked_add(source.len()) {
                Some(value) => value,
                None => usize::MAX,
            };
            if regex_source_bytes > limits.max_regex_source_bytes_per_group {
                return Err(MatcherCompileError::RegexSourceGroupByteLimit {
                    actual: regex_source_bytes,
                    limit: limits.max_regex_source_bytes_per_group,
                });
            }
        }
        let pattern_bytes = patterns
            .iter()
            .try_fold(0_usize, |total, pattern| total.checked_add(pattern.source_len()));
        let Some(pattern_bytes) = pattern_bytes else {
            return Err(MatcherCompileError::PatternByteLimit {
                actual: usize::MAX,
                limit: limits.max_pattern_bytes,
            });
        };
        if pattern_bytes > limits.max_pattern_bytes {
            return Err(MatcherCompileError::PatternByteLimit {
                actual: pattern_bytes,
                limit: limits.max_pattern_bytes,
            });
        }

        let mut counts = MatcherPatternCounts::default();
        let mut bytes = MatcherPatternBytes::default();
        for pattern in patterns {
            match pattern {
                PatternSpec::Phrase {
                    value,
                    ascii_case_insensitive,
                } => {
                    let (count, source_bytes) = if *ascii_case_insensitive {
                        (&mut counts.insensitive_phrases, &mut bytes.insensitive_phrases)
                    } else {
                        (&mut counts.sensitive_phrases, &mut bytes.sensitive_phrases)
                    };
                    *count += 1;
                    *source_bytes += value.len();
                }
                PatternSpec::Regex(source) => {
                    counts.regexes += 1;
                    bytes.regexes += source.len();
                }
            }
        }
        MatcherBuildProjection::for_counts(counts, bytes, limits)?;

        let mut sensitive_phrases = Vec::new();
        let mut insensitive_phrases = Vec::new();
        let mut regex_sources = Vec::new();
        let mut regex_entries = Vec::new();
        let mut sensitive_pattern_ids = Vec::new();
        let mut insensitive_pattern_ids = Vec::new();
        let mut regex_pattern_ids = Vec::new();
        let mut regex_fanout_ends = Vec::new();
        reserve_matcher(
            &mut sensitive_phrases,
            counts.sensitive_phrases,
            "sensitive phrase references",
        )?;
        reserve_matcher(
            &mut insensitive_phrases,
            counts.insensitive_phrases,
            "insensitive phrase references",
        )?;
        reserve_matcher(&mut regex_sources, counts.regexes, "regex references")?;
        reserve_matcher(&mut regex_entries, counts.regexes, "regex source entries")?;
        reserve_matcher(
            &mut sensitive_pattern_ids,
            counts.sensitive_phrases,
            "sensitive phrase identifiers",
        )?;
        reserve_matcher(
            &mut insensitive_pattern_ids,
            counts.insensitive_phrases,
            "insensitive phrase identifiers",
        )?;
        reserve_matcher(&mut regex_pattern_ids, counts.regexes, "regex identifiers")?;
        for (index, pattern) in patterns.iter().enumerate() {
            let pattern_id = PatternId(index);
            match pattern {
                PatternSpec::Phrase {
                    value,
                    ascii_case_insensitive,
                } => {
                    if value.is_empty() {
                        return Err(MatcherCompileError::EmptyPhrase { pattern_id: index });
                    }
                    if *ascii_case_insensitive {
                        insensitive_phrases.push((pattern_id, *value));
                        insensitive_pattern_ids.push(pattern_id);
                    } else {
                        sensitive_phrases.push((pattern_id, *value));
                        sensitive_pattern_ids.push(pattern_id);
                    }
                }
                PatternSpec::Regex(source) => {
                    regex_entries.push(RegexPatternEntry {
                        logical: pattern_id,
                        source,
                        physical: 0,
                    });
                }
            }
        }

        let first_regex_id = regex_entries.first().map(|entry| entry.logical);
        let logarithm = if counts.regexes <= 1 {
            0
        } else {
            usize::BITS as usize - (counts.regexes - 1).leading_zeros() as usize
        };
        let canonical_budget = regex_source_bytes
            .checked_add(counts.regexes)
            .and_then(|value| value.checked_mul(2 * logarithm + 4))
            .ok_or(MatcherCompileError::PeakArithmetic)?;
        let physical_count =
            match canonicalize_regex_entries(&mut regex_entries, canonical_budget.min(canonical_work_limit)) {
                Ok(count) => count,
                Err(()) => {
                    regex_entries.clear();
                    for (index, pattern) in patterns.iter().enumerate() {
                        if let PatternSpec::Regex(source) = pattern {
                            let physical = regex_entries.len();
                            regex_entries.push(RegexPatternEntry {
                                logical: PatternId(index),
                                source,
                                physical,
                            });
                        }
                    }
                    counts.regexes
                }
            };
        let fanout = physical_count < counts.regexes;
        if fanout {
            reserve_matcher(&mut regex_fanout_ends, counts.regexes, "regex fanout offsets")?;
        }
        let mut previous = None;
        for entry in &regex_entries {
            if previous != Some(entry.physical) {
                if fanout && previous.is_some() {
                    regex_fanout_ends.push(regex_pattern_ids.len());
                }
                regex_sources.push(entry.source);
                previous = Some(entry.physical);
            }
            regex_pattern_ids.push(entry.logical);
        }
        if fanout {
            regex_fanout_ends.push(regex_pattern_ids.len());
        }

        let sensitive_phrases = PhraseGroup::compile(&sensitive_phrases, sensitive_pattern_ids, false, limits)?;
        let insensitive_phrases = PhraseGroup::compile(&insensitive_phrases, insensitive_pattern_ids, true, limits)?;
        let regex = compile_regex(&regex_sources, first_regex_id, limits, regex_unicode)?;

        Ok(Self {
            pattern_count: patterns.len(),
            sensitive_phrases,
            insensitive_phrases,
            regex,
            regex_pattern_ids,
            regex_fanout_ends,
        })
    }

    pub(crate) fn create_stream_state(&self) -> Result<MatcherStreamState, MatcherScanError> {
        if self.regex.is_some() {
            return Err(MatcherScanError::PhraseSearchMode);
        }
        Ok(MatcherStreamState {
            pattern_count: self.pattern_count,
            sensitive: self
                .sensitive_phrases
                .as_ref()
                .map(PhraseGroup::create_stream_state)
                .transpose()?,
            insensitive: self
                .insensitive_phrases
                .as_ref()
                .map(PhraseGroup::create_stream_state)
                .transpose()?,
            matched_words: vec![0; self.pattern_count.div_ceil(u64::BITS as usize)],
        })
    }

    pub(crate) fn scan_stream_chunk<'state>(
        &self,
        state: &'state mut MatcherStreamState,
        chunk: &[u8],
        end_of_stream: bool,
    ) -> Result<MatchResults<'state>, MatcherScanError> {
        if state.pattern_count != self.pattern_count {
            return Err(MatcherScanError::ScratchShape);
        }
        match (&self.sensitive_phrases, &mut state.sensitive) {
            (Some(group), Some(stream)) => {
                group.scan_stream(stream, chunk, end_of_stream, &mut state.matched_words)?;
            }
            (None, None) => {}
            _ => return Err(MatcherScanError::ScratchShape),
        }
        match (&self.insensitive_phrases, &mut state.insensitive) {
            (Some(group), Some(stream)) => {
                group.scan_stream(stream, chunk, end_of_stream, &mut state.matched_words)?;
            }
            (None, None) => {}
            _ => return Err(MatcherScanError::ScratchShape),
        }
        Ok(MatchResults {
            matched_words: &state.matched_words,
            pattern_count: state.pattern_count,
        })
    }

    /// Return the number of patterns and therefore the valid ID range.
    pub const fn pattern_count(&self) -> usize {
        self.pattern_count
    }

    fn regex_physical_pattern_count(&self) -> usize {
        if self.regex_fanout_ends.is_empty() {
            self.regex_pattern_ids.len()
        } else {
            self.regex_fanout_ends.len()
        }
    }

    /// Return stable retained matcher and pattern-map bytes.
    pub fn memory_usage(&self) -> usize {
        self.sensitive_phrases
            .as_ref()
            .map_or(0, PhraseGroup::memory_usage)
            .saturating_add(self.insensitive_phrases.as_ref().map_or(0, PhraseGroup::memory_usage))
            .saturating_add(self.regex.as_ref().map_or(0, CompiledRegexProgram::memory_usage))
            .saturating_add(self.regex_pattern_ids.capacity().saturating_mul(size_of::<PatternId>()))
            .saturating_add(self.regex_fanout_ends.capacity().saturating_mul(size_of::<usize>()))
    }

    /// Return the logical bytes allocated by one reusable scan scratch.
    pub fn required_scratch_bytes(&self) -> usize {
        let word_bytes = self
            .pattern_count
            .div_ceil(u64::BITS as usize)
            .saturating_mul(size_of::<u64>());
        let sensitive_bytes = self.sensitive_phrases.as_ref().map_or(0, |group| {
            size_of::<AcPatternSet>().saturating_add(
                group
                    .pattern_ids
                    .len()
                    .div_ceil(u64::BITS as usize)
                    .saturating_mul(size_of::<u64>()),
            )
        });
        let insensitive_bytes = self.insensitive_phrases.as_ref().map_or(0, |group| {
            size_of::<AcPatternSet>().saturating_add(
                group
                    .pattern_ids
                    .len()
                    .div_ceil(u64::BITS as usize)
                    .saturating_mul(size_of::<u64>()),
            )
        });
        let regex_bytes = self.regex.as_ref().map_or(0, |regex| {
            regex
                .required_scratch_bytes()
                .saturating_add(self.regex_physical_pattern_count())
        });
        size_of::<MatchScratch>()
            .saturating_add(word_bytes)
            .saturating_add(sensitive_bytes)
            .saturating_add(insensitive_bytes)
            .saturating_add(regex_bytes)
    }

    /// Return a conservative post-build diagnostic peak report.
    pub fn estimated_peak_build_memory_usage(&self) -> usize {
        self.sensitive_phrases
            .as_ref()
            .map_or(0, PhraseGroup::estimated_peak_build_memory_usage)
            .saturating_add(
                self.insensitive_phrases
                    .as_ref()
                    .map_or(0, PhraseGroup::estimated_peak_build_memory_usage),
            )
            .saturating_add(self.regex.as_ref().map_or(0, CompiledRegexProgram::memory_usage))
            .saturating_add(self.regex_pattern_ids.capacity().saturating_mul(size_of::<PatternId>()))
            .saturating_add(self.regex_fanout_ends.capacity().saturating_mul(size_of::<usize>()))
    }

    /// Allocate reusable scratch sized for this compiled generation.
    pub fn create_scratch(&self) -> MatchScratch {
        MatchScratch {
            pattern_count: self.pattern_count,
            matched_words: vec![0; self.pattern_count.div_ceil(u64::BITS as usize)],
            sensitive_phrase_matches: self.sensitive_phrases.as_ref().map(PhraseGroup::create_pattern_set),
            insensitive_phrase_matches: self.insensitive_phrases.as_ref().map(PhraseGroup::create_pattern_set),
            regex_scratch: self.regex.as_ref().map(CompiledRegexProgram::create_scratch),
            regex_matches: RegexPatternSet::new(self.regex_physical_pattern_count()),
        }
    }

    /// Match all patterns against one byte-oriented value.
    ///
    /// Results are a source-order bitset and remain valid until the scratch is
    /// reused. No pattern is compiled on this path.
    pub fn scan<'a>(
        &self,
        haystack: &[u8],
        scratch: &'a mut MatchScratch,
    ) -> Result<MatchResults<'a>, MatcherScanError> {
        self.scan_inner(haystack, scratch, true)
    }

    /// Match all patterns through the scalar reference path.
    ///
    /// Phrase matching bypasses optional candidate acceleration. Regex matching
    /// uses the same bounded Thompson NFA as the normal path.
    pub fn scan_reference<'a>(
        &self,
        haystack: &[u8],
        scratch: &'a mut MatchScratch,
    ) -> Result<MatchResults<'a>, MatcherScanError> {
        self.scan_inner(haystack, scratch, false)
    }

    fn scan_inner<'a>(
        &self,
        haystack: &[u8],
        scratch: &'a mut MatchScratch,
        accelerated: bool,
    ) -> Result<MatchResults<'a>, MatcherScanError> {
        if scratch.pattern_count != self.pattern_count
            || scratch.regex_matches.capacity() != self.regex_physical_pattern_count()
        {
            return Err(MatcherScanError::ScratchShape);
        }
        scratch.matched_words.fill(0);
        scratch.regex_matches.clear();

        scan_phrase_group(
            self.sensitive_phrases.as_ref(),
            scratch.sensitive_phrase_matches.as_mut(),
            haystack,
            &mut scratch.matched_words,
            accelerated,
        )?;
        scan_phrase_group(
            self.insensitive_phrases.as_ref(),
            scratch.insensitive_phrase_matches.as_mut(),
            haystack,
            &mut scratch.matched_words,
            accelerated,
        )?;
        if let (Some(regex), Some(regex_scratch)) = (&self.regex, scratch.regex_scratch.as_mut()) {
            if !Arc::ptr_eq(&regex_scratch.token, &regex.core.token) {
                return Err(MatcherScanError::ScratchShape);
            }
            if let Some(matched) = accelerated.then(|| regex.exact_match(haystack)).flatten() {
                if matched {
                    scratch
                        .regex_matches
                        .try_insert(regex_automata::PatternID::ZERO)
                        .map_err(|_| MatcherScanError::ScratchShape)?;
                }
            } else if !accelerated || regex.may_match(haystack) {
                regex.core.vm.which_overlapping_matches(
                    &mut regex_scratch.cache,
                    &Input::new(haystack),
                    &mut scratch.regex_matches,
                );
            }
            for local_id in scratch.regex_matches.iter() {
                let physical = local_id.as_usize();
                let range = if self.regex_fanout_ends.is_empty() {
                    physical..physical + 1
                } else {
                    let start = if physical == 0 {
                        0
                    } else {
                        *self
                            .regex_fanout_ends
                            .get(physical - 1)
                            .ok_or(MatcherScanError::InvalidPatternId)?
                    };
                    let end = *self
                        .regex_fanout_ends
                        .get(physical)
                        .ok_or(MatcherScanError::InvalidPatternId)?;
                    start..end
                };
                for &pattern_id in self
                    .regex_pattern_ids
                    .get(range)
                    .ok_or(MatcherScanError::InvalidPatternId)?
                {
                    set_match(&mut scratch.matched_words, pattern_id)?;
                }
            }
        }

        Ok(MatchResults {
            matched_words: &scratch.matched_words,
            pattern_count: self.pattern_count,
        })
    }
}

fn scan_phrase_group(
    group: Option<&PhraseGroup>,
    phrase_matches: Option<&mut AcPatternSet>,
    haystack: &[u8],
    matched: &mut [u64],
    accelerated: bool,
) -> Result<(), MatcherScanError> {
    match (group, phrase_matches) {
        (Some(group), Some(phrase_matches)) if accelerated => group.scan(haystack, phrase_matches, matched),
        (Some(group), Some(phrase_matches)) => group.scan_reference(haystack, phrase_matches, matched),
        (None, None) => Ok(()),
        _ => Err(MatcherScanError::ScratchShape),
    }
}

fn compile_regex(
    patterns: &[&str],
    first_pattern_id: Option<PatternId>,
    limits: MatcherLimits,
    unicode: bool,
) -> Result<Option<CompiledRegexProgram>, MatcherCompileError> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let core = RegexCore::compile_many(
        patterns,
        limits.into(),
        WhichCaptures::None,
        usize::MAX,
        false,
        unicode,
        MatchKind::All,
    )
    .map_err(|error| match error {
        RegexProgramError::ProgramByteLimit { actual, limit } => {
            MatcherCompileError::RegexProgramByteLimit { actual, limit }
        }
        _ => MatcherCompileError::Regex {
            pattern_id: first_pattern_id.map(PatternId::as_usize),
        },
    })?;
    let actual = core.retained_bytes;
    if actual > limits.max_regex_program_bytes {
        return Err(MatcherCompileError::RegexProgramByteLimit {
            actual,
            limit: limits.max_regex_program_bytes,
        });
    }
    Ok(Some(CompiledRegexProgram {
        core,
        execution_hint: RegexExecutionHint::None,
    }))
}

fn reserve_matcher<T>(
    records: &mut Vec<T>,
    additional: usize,
    operation: &'static str,
) -> Result<(), MatcherCompileError> {
    records
        .try_reserve_exact(additional)
        .map_err(|_| MatcherCompileError::Allocation { operation })
}

/// Reusable, matcher-generation-specific scan memory.
#[derive(Debug)]
pub struct MatchScratch {
    pattern_count: usize,
    matched_words: Vec<u64>,
    sensitive_phrase_matches: Option<AcPatternSet>,
    insensitive_phrase_matches: Option<AcPatternSet>,
    regex_scratch: Option<RegexBooleanScratch>,
    regex_matches: RegexPatternSet,
}

impl MatchScratch {
    /// Query the most recent successful scan. Callers must establish that this
    /// scratch still belongs to that scan's matcher and input before reuse.
    pub(crate) fn contains_any_in_range(
        &self,
        expected_patterns: usize,
        range: std::ops::Range<usize>,
    ) -> Option<bool> {
        if self.pattern_count != expected_patterns {
            return None;
        }
        MatchResults {
            matched_words: &self.matched_words,
            pattern_count: self.pattern_count,
        }
        .contains_any_in_range(range)
    }
}

#[derive(Debug)]
pub(crate) struct MatcherStreamState {
    pattern_count: usize,
    sensitive: Option<PhraseStreamState>,
    insensitive: Option<PhraseStreamState>,
    matched_words: Vec<u64>,
}

impl MatcherStreamState {
    pub(crate) fn memory_usage(&self) -> usize {
        self.matched_words
            .capacity()
            .saturating_mul(size_of::<u64>())
            .saturating_add(self.sensitive.as_ref().map_or(0, PhraseStreamState::memory_usage))
            .saturating_add(self.insensitive.as_ref().map_or(0, PhraseStreamState::memory_usage))
    }
}

/// Borrowed result view in stable source order.
#[derive(Debug, Clone, Copy)]
pub struct MatchResults<'a> {
    matched_words: &'a [u64],
    pattern_count: usize,
}

impl MatchResults<'_> {
    /// Return whether a specific source-order pattern matched.
    pub fn contains(self, pattern_id: PatternId) -> bool {
        if pattern_id.0 >= self.pattern_count {
            return false;
        }
        let word_index = pattern_id.0 / u64::BITS as usize;
        let bit_index = pattern_id.0 % u64::BITS as usize;
        self.matched_words
            .get(word_index)
            .is_some_and(|word| word & (1_u64 << bit_index) != 0)
    }

    /// Query only a member's pattern range; invalid ranges are invariant errors
    /// for callers, not silently truncated nonmatches.
    pub(crate) fn contains_any_in_range(self, range: std::ops::Range<usize>) -> Option<bool> {
        if range.start > range.end || range.end > self.pattern_count {
            return None;
        }
        if range.is_empty() {
            return Some(false);
        }
        let bits = u64::BITS as usize;
        let first = range.start / bits;
        let last = (range.end - 1) / bits;
        let first_mask = u64::MAX << (range.start % bits);
        let last_mask = u64::MAX >> (bits - 1 - (range.end - 1) % bits);
        Some(
            self.matched_words
                .iter()
                .enumerate()
                .skip(first)
                .take(last - first + 1)
                .any(|(index, &word)| {
                    let word = if index == first { word & first_mask } else { word };
                    let word = if index == last { word & last_mask } else { word };
                    word != 0
                }),
        )
    }

    /// Iterate over every matching ID exactly once in source order.
    pub fn iter(&self) -> impl Iterator<Item = PatternId> + '_ {
        set_bit_indices(self.matched_words, self.pattern_count).map(PatternId)
    }

    /// Return whether no pattern matched.
    pub fn is_empty(self) -> bool {
        self.matched_words.iter().all(|word| *word == 0)
    }
}

#[cfg(test)]
#[path = "../tests/unit/matcher_test.rs"]
mod tests;
