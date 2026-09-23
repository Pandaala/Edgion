// Dynamic (hot-reload) test scenario framework.
//
// A scenario is a Rust function that drives a configuration state machine:
// Phase₀ (assert) → Transition (apply / delete / wait) → Phase₁ (assert) → …
//
// Scenarios are registered via `registry::all()` and exposed to the test
// runner through `test_client --list-dynamic-scenarios` / `--dynamic-scenario <id>`.

pub mod apply;
pub mod ctx;
pub mod registry;
pub mod scenario;
pub mod scenarios;
pub mod status;
pub mod wait;

pub use ctx::ScenarioCtx;
pub use registry::all;
pub use scenario::{PhaseSuite, Scenario};
