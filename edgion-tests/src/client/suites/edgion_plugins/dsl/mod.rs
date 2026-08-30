// DSL Plugin Test Module

mod body;
mod dsl;
mod dynamic;
mod grpc_error;

pub use body::DslBodyTestSuite;
pub use dsl::DslTestSuite;
pub use dynamic::{DslDynamicInitialTestSuite, DslDynamicUpdateTestSuite};
pub use grpc_error::DslGrpcErrorTestSuite;
