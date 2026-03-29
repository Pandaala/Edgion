package conformance_test

import (
	"os"
	"strconv"
	"testing"
	"time"

	"k8s.io/apimachinery/pkg/util/sets"

	"sigs.k8s.io/gateway-api/conformance"
	"sigs.k8s.io/gateway-api/conformance/utils/config"
	"sigs.k8s.io/gateway-api/conformance/utils/suite"
	"sigs.k8s.io/gateway-api/pkg/features"

	"edgion.com/gateway-api-conformance/tal"
)

func TestConformance(t *testing.T) {
	opts := conformance.DefaultOptions(t)

	if opts.SupportedFeatures == nil {
		opts.SupportedFeatures = sets.New[features.FeatureName]()
	}

	// Core features (required for all tests)
	opts.SupportedFeatures.Insert(features.SupportGateway)
	opts.SupportedFeatures.Insert(features.SupportHTTPRoute)
	opts.SupportedFeatures.Insert(features.SupportReferenceGrant)

	// A-class: HTTPRoute matching enhancements
	opts.SupportedFeatures.Insert(features.SupportHTTPRouteQueryParamMatching)
	opts.SupportedFeatures.Insert(features.SupportHTTPRouteMethodMatching)

	// A-class: HTTPRoute filters (response header, rewrite, mirror, timeout)
	opts.SupportedFeatures.Insert(features.SupportHTTPRouteResponseHeaderModification)
	opts.SupportedFeatures.Insert(features.SupportHTTPRouteHostRewrite)
	opts.SupportedFeatures.Insert(features.SupportHTTPRoutePathRewrite)
	opts.SupportedFeatures.Insert(features.SupportHTTPRouteRequestMirror)
	opts.SupportedFeatures.Insert(features.SupportHTTPRouteRequestTimeout)
	opts.SupportedFeatures.Insert(features.SupportHTTPRouteBackendTimeout)

	// A-class: HTTPRoute backend-level filters and protocol
	opts.SupportedFeatures.Insert(features.SupportHTTPRouteBackendRequestHeaderModification)
	opts.SupportedFeatures.Insert(features.SupportHTTPRouteBackendProtocolH2C)
	opts.SupportedFeatures.Insert(features.SupportHTTPRouteBackendProtocolWebSocket)
	opts.SupportedFeatures.Insert(features.SupportHTTPRouteNamedRouteRule)

	// A-class: Gateway features
	opts.SupportedFeatures.Insert(features.SupportGatewayHTTPListenerIsolation)
	// GatewayPort8080 requires dynamic listener binding at runtime, which is not supported.
	// The gateway binds ports at startup only; ports created via Gateway resources after
	// startup cannot be bound. This feature is permanently deferred.

	// B-class: HTTPRoute redirect (path/port/scheme)
	opts.SupportedFeatures.Insert(features.SupportHTTPRoutePathRedirect)
	opts.SupportedFeatures.Insert(features.SupportHTTPRoutePortRedirect)
	opts.SupportedFeatures.Insert(features.SupportHTTPRouteSchemeRedirect)

	// B-class: HTTPRoute redirect status codes (303/307/308)
	opts.SupportedFeatures.Insert(features.SupportHTTPRoute303RedirectStatusCode)
	opts.SupportedFeatures.Insert(features.SupportHTTPRoute307RedirectStatusCode)
	opts.SupportedFeatures.Insert(features.SupportHTTPRoute308RedirectStatusCode)

	// B-class: HTTPRoute percentage mirror + multiple mirrors
	opts.SupportedFeatures.Insert(features.SupportHTTPRouteRequestPercentageMirror)
	opts.SupportedFeatures.Insert(features.SupportHTTPRouteRequestMultipleMirrors)

	// B-class: HTTPRoute destination port matching (status-only tests)
	opts.SupportedFeatures.Insert(features.SupportHTTPRouteDestinationPortMatching)

	// B-class: HTTPRoute parentRef port matching
	// Disabled: requires dynamic listener port binding at runtime (8080/8090),
	// which Edgion does not support — ports are bound at startup only.
	// opts.SupportedFeatures.Insert(features.SupportHTTPRouteParentRefPort)

	// TLSRoute support (needed for HTTPRouteDisallowedKind status test)
	opts.SupportedFeatures.Insert(features.SupportTLSRoute)

	// CORS support — disabled: CORS preflight handling is incomplete
	// (simple requests work, but preflight OPTIONS with auth/method/header
	// negotiation fails on several sub-tests)
	// opts.SupportedFeatures.Insert(features.SupportHTTPRouteCORS)

	// HTTP 421 Misdirected Request detection (Gateway API spec)
	opts.SupportedFeatures.Insert(features.SupportGatewayHTTPSListenerDetectMisdirectedRequests)

	// GRPCRoute core feature — required for GATEWAY-GRPC conformance profile.
	opts.SupportedFeatures.Insert(features.SupportGRPCRoute)

	// F-class: Frontend client certificate validation (mTLS)
	// F1/F3/F4 are gated by SupportGatewayFrontendClientCertificateValidation
	// F2 additionally requires SupportGatewayFrontendClientCertificateValidationInsecureFallback
	opts.SupportedFeatures.Insert(features.SupportGatewayFrontendClientCertificateValidation)
	opts.SupportedFeatures.Insert(features.SupportGatewayFrontendClientCertificateValidationInsecureFallback)

	// Conformance profiles — declare which profiles Edgion claims to support.
	// This enables profile-based conformance reporting per GEP-1709.
	//
	// GATEWAY-HTTP: declared, core features implemented.
	// GATEWAY-GRPC: declared, core features implemented (exact/header/hostname/weight matching).
	// GATEWAY-TLS:  declared for testing. TLS Passthrough implemented; status condition work may be needed.
	opts.ConformanceProfiles = sets.New[suite.ConformanceProfileName](
		suite.GatewayHTTPConformanceProfileName,
		suite.GatewayGRPCConformanceProfileName,
		suite.GatewayTLSConformanceProfileName,
	)

	opts.AllowCRDsMismatch = true

	opts.TimeoutConfig = config.TimeoutConfig{
		CreateTimeout:                      10 * time.Second,
		DeleteTimeout:                      5 * time.Second,
		GetTimeout:                         5 * time.Second,
		GatewayMustHaveAddress:             10 * time.Second,
		GatewayMustHaveCondition:           10 * time.Second,
		GatewayStatusMustHaveListeners:     10 * time.Second,
		GatewayListenersMustHaveConditions: 10 * time.Second,
		GWCMustBeAccepted:                  30 * time.Second,
		HTTPRouteMustNotHaveParents:        10 * time.Second,
		HTTPRouteMustHaveCondition:         10 * time.Second,
		TLSRouteMustHaveCondition:          10 * time.Second,
		RouteMustHaveParents:               10 * time.Second,
		ManifestFetchTimeout:               5 * time.Second,
		MaxTimeToConsistency:               30 * time.Second,
		NamespacesMustBeReady:              60 * time.Second,
		RequestTimeout:                     2 * time.Second,
		LatestObservedGenerationSet:        10 * time.Second,
		DefaultTestTimeout:                 15 * time.Second,
		RequiredConsecutiveSuccesses:       3,
	}

	// Transport Access Layer (TAL): unified Gateway address resolver.
	// In port-forward mode (GATEWAY_PROXY_HOST set), all protocol adapters
	// rewrite only the TCP dial target — request semantics are never modified.
	resolver, proxyEnabled := tal.ResolverFromEnv()

	enableGRPCProxy := false
	if raw := os.Getenv("GATEWAY_PROXY_ENABLE_GRPC"); raw != "" {
		parsed, err := strconv.ParseBool(raw)
		if err != nil {
			t.Fatalf("invalid GATEWAY_PROXY_ENABLE_GRPC value %q: %v", raw, err)
		}
		enableGRPCProxy = parsed
	}

	if proxyEnabled {
		t.Logf("TAL: port-forward mode enabled (GATEWAY_PROXY_HOST=%s)", os.Getenv("GATEWAY_PROXY_HOST"))
		opts.RoundTripper = tal.NewRoundTripper(resolver, opts.Debug, opts.TimeoutConfig)

		if enableGRPCProxy {
			t.Log("TAL: gRPC proxy client enabled")
			opts.GRPCClient = newGRPCClient(resolver)
		}
	} else if enableGRPCProxy {
		t.Fatal("GATEWAY_PROXY_ENABLE_GRPC requires GATEWAY_PROXY_HOST to be set")
	}

	conformance.RunConformanceWithOptions(t, opts)
}
