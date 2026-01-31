// PluginCondition tests
//
// Required config files (in examples/test/conf/EdgionPlugins/PluginCondition/):
// - EdgionPlugins_default_condition-test.yaml
// - HTTPRoute_default_condition-test.yaml
// - EdgionPlugins_default_condition-all-types.yaml  (all condition types)
// - HTTPRoute_default_condition-all-types.yaml      (route for all condition types)

mod all_conditions;
mod conditions;

pub use all_conditions::AllConditionsTestSuite;
pub use conditions::PluginConditionTestSuite;
