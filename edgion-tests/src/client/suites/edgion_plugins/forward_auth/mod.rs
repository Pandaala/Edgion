// ForwardAuth plugin tests
//
// Required config files (in edgion-tests/integration/conf/EdgionPlugins/ForwardAuth/):
// - 01_EdgionPlugins_forward-auth-basic.yaml      # ForwardAuth: forward all headers
// - 02_EdgionPlugins_forward-auth-selective.yaml   # ForwardAuth: forward specific headers
// - HTTPRoute_default_forward-auth.yaml            # Routes for both test modes
//
// Also requires:
// - test_server started with --auth-port 30040     # Fake auth server
// - base config (in edgion-tests/integration/conf/EdgionPlugins/base/):
//   - Gateway.yaml                                 # Gateway for EdgionPlugins tests

mod forward_auth;

pub use forward_auth::ForwardAuthTestSuite;
