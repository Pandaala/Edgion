mod basic_auth;
mod dynamic;
pub use basic_auth::BasicAuthTestSuite;
pub use dynamic::{
    BasicAuthDynamicGroupedInitialTestSuite, BasicAuthDynamicGroupedUpdateTestSuite, BasicAuthDynamicInitialTestSuite,
    BasicAuthDynamicUpdateTestSuite,
};
