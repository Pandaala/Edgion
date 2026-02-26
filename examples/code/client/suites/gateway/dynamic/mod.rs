// Gateway Dynamic Configuration Test Suite
//
// Tests dynamic updates of Gateway resources including:
// - Listener hostname constraints
// - AllowedRoutes configuration
// - HTTPRoute CRUD operations
//
// This module exports test suites for both initial and update phases.

mod initial_tests;
mod update_tests;

pub use initial_tests::InitialPhaseTestSuite;
pub use update_tests::UpdatePhaseTestSuite;
