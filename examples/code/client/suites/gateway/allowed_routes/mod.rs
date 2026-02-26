// Gateway AllowedRoutes test module
mod all_namespaces;
mod kinds;
mod same_namespace;

pub use all_namespaces::AllowedRoutesAllNamespacesTestSuite;
pub use kinds::AllowedRoutesKindsTestSuite;
pub use same_namespace::AllowedRoutesSameNamespaceTestSuite;
