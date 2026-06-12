// JWT Auth plugin tests
//
// Required config files (in examples/test/conf/EdgionPlugins/JwtAuth/):
// - Secret_default_jwt-secret.yaml            # JWT secret for HS256
// - EdgionPlugins_default_jwt-auth.yaml       # JwtAuth plugin config
// - HTTPRoute_default_jwt-auth-test.yaml      # Route with host: jwt-test.example.com

mod jwt_auth;

pub use jwt_auth::JwtAuthTestSuite;
