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

func defaultConformanceProfiles() sets.Set[suite.ConformanceProfileName] {
	return sets.New[suite.ConformanceProfileName](
		suite.GatewayHTTPConformanceProfileName,
		suite.GatewayGRPCConformanceProfileName,
		suite.GatewayTLSConformanceProfileName,
	)
}

func declaredSupportedFeatures() sets.Set[features.FeatureName] {
	supported := sets.New[features.FeatureName]()

	// Core features (required for all tests)
	supported.Insert(features.SupportGateway)
	supported.Insert(features.SupportHTTPRoute)
	supported.Insert(features.SupportReferenceGrant)

	// A-class: HTTPRoute matching enhancements
	supported.Insert(features.SupportHTTPRouteQueryParamMatching)
	supported.Insert(features.SupportHTTPRouteMethodMatching)

	// A-class: HTTPRoute filters (response header, rewrite, mirror, timeout)
	supported.Insert(features.SupportHTTPRouteResponseHeaderModification)
	supported.Insert(features.SupportHTTPRouteHostRewrite)
	supported.Insert(features.SupportHTTPRoutePathRewrite)
	supported.Insert(features.SupportHTTPRouteRequestMirror)
	supported.Insert(features.SupportHTTPRouteRequestTimeout)
	supported.Insert(features.SupportHTTPRouteBackendTimeout)

	// A-class: HTTPRoute backend-level filters and protocol
	supported.Insert(features.SupportHTTPRouteBackendRequestHeaderModification)
	supported.Insert(features.SupportHTTPRouteBackendProtocolH2C)
	supported.Insert(features.SupportHTTPRouteBackendProtocolWebSocket)
	supported.Insert(features.SupportHTTPRouteNamedRouteRule)

	// A-class: Gateway features
	supported.Insert(features.SupportGatewayHTTPListenerIsolation)

	// B-class: HTTPRoute redirect (path/port/scheme)
	supported.Insert(features.SupportHTTPRoutePathRedirect)
	supported.Insert(features.SupportHTTPRoutePortRedirect)
	supported.Insert(features.SupportHTTPRouteSchemeRedirect)

	// B-class: HTTPRoute redirect status codes (303/307/308)
	supported.Insert(features.SupportHTTPRoute303RedirectStatusCode)
	supported.Insert(features.SupportHTTPRoute307RedirectStatusCode)
	supported.Insert(features.SupportHTTPRoute308RedirectStatusCode)

	// B-class: HTTPRoute percentage mirror + multiple mirrors
	supported.Insert(features.SupportHTTPRouteRequestPercentageMirror)
	supported.Insert(features.SupportHTTPRouteRequestMultipleMirrors)

	// B-class: HTTPRoute destination port matching (status-only tests)
	supported.Insert(features.SupportHTTPRouteDestinationPortMatching)

	// TLSRoute support (needed for HTTPRouteDisallowedKind status test)
	supported.Insert(features.SupportTLSRoute)

	// TLS Terminate: gateway terminates TLS, forwards plaintext TCP to backends.
	// Mixed mode (Terminate + Passthrough on same port) is NOT supported.
	supported.Insert(features.SupportTLSRouteModeTerminate)

	// HTTP 421 Misdirected Request detection (Gateway API spec)
	supported.Insert(features.SupportGatewayHTTPSListenerDetectMisdirectedRequests)

	// CORS support (preflight + simple requests)
	supported.Insert(features.SupportHTTPRouteCORS)

	// GRPCRoute core feature — required for GATEWAY-GRPC conformance profile.
	supported.Insert(features.SupportGRPCRoute)

	return supported
}

func ensureConformanceProfiles(opts *suite.ConformanceOptions) {
	if opts.ConformanceProfiles == nil || opts.ConformanceProfiles.Len() == 0 {
		opts.ConformanceProfiles = defaultConformanceProfiles()
	}
}

func featuresForProfile(profile suite.ConformanceProfileName) sets.Set[features.FeatureName] {
	switch profile {
	case suite.GatewayHTTPConformanceProfileName:
		return sets.New[features.FeatureName]().
			Insert(suite.GatewayHTTPConformanceProfile.CoreFeatures.UnsortedList()...).
			Insert(suite.GatewayHTTPConformanceProfile.ExtendedFeatures.UnsortedList()...)
	case suite.GatewayGRPCConformanceProfileName:
		return sets.New[features.FeatureName]().
			Insert(suite.GatewayGRPCConformanceProfile.CoreFeatures.UnsortedList()...).
			Insert(suite.GatewayGRPCConformanceProfile.ExtendedFeatures.UnsortedList()...)
	case suite.GatewayTLSConformanceProfileName:
		return sets.New[features.FeatureName]().
			Insert(suite.GatewayTLSConformanceProfile.CoreFeatures.UnsortedList()...).
			Insert(suite.GatewayTLSConformanceProfile.ExtendedFeatures.UnsortedList()...)
	case suite.MeshHTTPConformanceProfileName:
		return sets.New[features.FeatureName]().
			Insert(suite.MeshHTTPConformanceProfile.CoreFeatures.UnsortedList()...).
			Insert(suite.MeshHTTPConformanceProfile.ExtendedFeatures.UnsortedList()...)
	case suite.MeshGRPCConformanceProfileName:
		return sets.New[features.FeatureName]().
			Insert(suite.MeshGRPCConformanceProfile.CoreFeatures.UnsortedList()...).
			Insert(suite.MeshGRPCConformanceProfile.ExtendedFeatures.UnsortedList()...)
	default:
		return sets.New[features.FeatureName]()
	}
}

func filterSupportedFeaturesForProfiles(
	declared sets.Set[features.FeatureName],
	profiles sets.Set[suite.ConformanceProfileName],
) sets.Set[features.FeatureName] {
	if profiles == nil || profiles.Len() == 0 {
		return declared.Clone()
	}

	allowed := sets.New[features.FeatureName]()
	for _, profile := range profiles.UnsortedList() {
		allowed.Insert(featuresForProfile(profile).UnsortedList()...)
	}

	filtered := sets.New[features.FeatureName]()
	for _, feature := range declared.UnsortedList() {
		if allowed.Has(feature) {
			filtered.Insert(feature)
		}
	}
	return filtered
}

func ensureSupportedFeatures(opts *suite.ConformanceOptions) {
	if opts.SupportedFeatures != nil && opts.SupportedFeatures.Len() > 0 {
		return
	}

	declared := declaredSupportedFeatures()
	opts.SupportedFeatures = filterSupportedFeaturesForProfiles(declared, opts.ConformanceProfiles)
}

func TestConformance(t *testing.T) {
	opts := conformance.DefaultOptions(t)

	// Conformance profiles — declare which profiles Edgion claims to support.
	// This enables profile-based conformance reporting per GEP-1709.
	//
	// GATEWAY-HTTP: declared, core features implemented.
	// GATEWAY-GRPC: declared, core features implemented (exact/header/hostname/weight matching).
	// GATEWAY-TLS:  declared for testing. TLS Passthrough implemented; status condition work may be needed.
	ensureConformanceProfiles(&opts)
	ensureSupportedFeatures(&opts)

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
