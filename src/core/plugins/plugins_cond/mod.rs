//! Common plugin structures and utilities
//!
//! This module provides shared types and utilities for plugin conditions,
//! enabling conditional execution of plugins based on various criteria.

mod conditions;
mod evaluator;

pub use conditions::{
    Condition, ConditionSource, ExcludeCondition, IncludeCondition, KeyExistCondition, KeyMatchCondition,
    PluginConditions, ProbabilityCondition, TimeRangeCondition,
};
pub use evaluator::{ConditionContext, ConditionEvalResult, EvaluationResult};
