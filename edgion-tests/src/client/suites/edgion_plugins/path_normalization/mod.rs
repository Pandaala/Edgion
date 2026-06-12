// PathNormalization test suites
//
// Scenario 01 (default strip mode): PathNormalizationTestSuite
// Scenario 02 (preserve_path_parameters opt-out): PathNormalizationPreserveParamTestSuite
// Scenario 03 (upstream forwarding decision): PathNormalizationForwardViewTestSuite

mod forward_view;
mod path_normalization;
mod preserve_param;

pub use forward_view::PathNormalizationForwardViewTestSuite;
pub use path_normalization::PathNormalizationTestSuite;
pub use preserve_param::PathNormalizationPreserveParamTestSuite;
