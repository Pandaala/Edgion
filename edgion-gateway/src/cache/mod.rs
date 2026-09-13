pub mod tiny_ufo;
pub use tiny_ufo::{
    validate_limits, LogicalWeight, TinyUfoCacheConfigError, TinyUfoCacheLimits, TinyUfoInsertOutcome,
    TinyUfoLookupStatus, TinyUfoTtlCache,
};
