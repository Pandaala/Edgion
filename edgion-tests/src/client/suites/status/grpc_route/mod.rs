//! GRPCRoute Status Test Suite
//!
//! Tests the status system for GRPCRoute resources:
//! 1. Basic route with valid backend → Accepted=True, ResolvedRefs=True
//! 2. Route with non-existent backend → ResolvedRefs=False(BackendNotFound)
//! 3. Route with invalid backend kind → ResolvedRefs=False(InvalidKind)
//!
//! Required config files (in edgion-tests/integration/conf/Status/GRPCRoute/):
//! - Gateway_edgion-test_grpc-status-gateway.yaml
//! - Service_edgion-default_grpc-status-svc.yaml
//! - EndpointSlice_edgion-default_grpc-status-svc.yaml
//! - GRPCRoute_edgion-default_grpc-basic-route.yaml
//! - GRPCRoute_edgion-default_grpc-nonexistent-backend.yaml
//! - GRPCRoute_edgion-default_grpc-invalid-kind.yaml

mod status_test;

pub use status_test::GrpcRouteStatusTestSuite;
