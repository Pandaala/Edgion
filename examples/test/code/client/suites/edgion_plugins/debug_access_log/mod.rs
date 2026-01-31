// DebugAccessLog plugin tests
//
// Required config files (in examples/test/conf/EdgionPlugins/DebugAccessLog/):
// - EdgionPlugins_default_debug-access-log.yaml
// - HTTPRoute_default_plugin-logs-test.yaml

mod logs;

pub use logs::PluginLogsTestSuite;
