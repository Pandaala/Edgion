// ExtProc plugin tests
//
// Required config files (in edgion-tests/integration/conf/EdgionPlugins/ExtProc/):
// - 01_EdgionPlugins_ext-proc-basic.yaml       # ext_proc: request + response headers
// - 02_EdgionPlugins_ext-proc-failclose.yaml   # ext_proc: dead target, fail-close
// - 03_EdgionPlugins_ext-proc-failopen.yaml    # ext_proc: dead target, fail-open
// - HTTPRoute_default_ext-proc-*.yaml          # one route per config
//
// Also requires:
// - test_server started with --ext-proc-port 30041   # Fake ext_proc gRPC server
// - base config (in edgion-tests/integration/conf/EdgionPlugins/base/):
//   - Gateway.yaml                                    # Gateway for EdgionPlugins tests

mod ext_proc;

pub use ext_proc::ExtProcTestSuite;
