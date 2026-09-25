package conformance_test

import (
	_ "embed"
	"os"
	"strconv"
	"strings"
	"testing"
	"time"

	"k8s.io/apimachinery/pkg/util/sets"

	"sigs.k8s.io/gateway-api/conformance"
	"sigs.k8s.io/gateway-api/conformance/utils/config"
	"sigs.k8s.io/gateway-api/conformance/utils/suite"
	"sigs.k8s.io/gateway-api/pkg/features"

	"edgion.com/gateway-api-conformance/tal"
)

// supportedFeaturesFile is the canonical, cross-language feature list. The
// single source of truth lives in edgion-resources (SUPPORTED_FEATURES); a Rust
// unit test (shared_list_file_matches_supported_features) keeps this file
// byte-equal to it so the controller status and the conformance declaration
// cannot silently drift. See the file's header comment for details.
const supportedFeaturesFile = "supported_features.txt"

// supportedFeaturesData embeds the canonical list at compile time so the test
// binary needs no runtime file dependency (it runs inside a pod for in-cluster
// conformance, where the source tree is not present).
//
//go:embed supported_features.txt
var supportedFeaturesData string

func defaultConformanceProfiles() []suite.ConformanceProfileName {
	return []suite.ConformanceProfileName{
		suite.GatewayHTTPConformanceProfileName,
		suite.GatewayGRPCConformanceProfileName,
		suite.GatewayTLSConformanceProfileName,
	}
}

// declaredSupportedFeatures derives the conformance feature set from the
// canonical shared list (supported_features.txt) instead of a hand-maintained
// list, so it cannot drift from what the controller advertises. An unknown
// feature name fails the test as a typo.
func declaredSupportedFeatures(t *testing.T) sets.Set[features.FeatureName] {
	t.Helper()

	validNames := features.SetsToNamesSet(features.AllFeatures)
	supported := sets.New[features.FeatureName]()
	for _, line := range strings.Split(supportedFeaturesData, "\n") {
		name := strings.TrimSpace(line)
		if name == "" || strings.HasPrefix(name, "#") {
			continue
		}
		fn := features.FeatureName(name)
		if !validNames.Has(fn) {
			t.Fatalf("supported feature %q in %s is not a known Gateway API conformance feature", name, supportedFeaturesFile)
		}
		supported.Insert(fn)
	}
	return supported
}

func ensureConformanceProfiles(opts *suite.ConformanceOptions) {
	if len(opts.ConformanceProfiles) == 0 {
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
	case suite.GatewayTCPConformanceProfileName:
		return sets.New[features.FeatureName]().
			Insert(suite.GatewayTCPConformanceProfile.CoreFeatures.UnsortedList()...).
			Insert(suite.GatewayTCPConformanceProfile.ExtendedFeatures.UnsortedList()...)
	case suite.GatewayUDPConformanceProfileName:
		return sets.New[features.FeatureName]().
			Insert(suite.GatewayUDPConformanceProfile.CoreFeatures.UnsortedList()...).
			Insert(suite.GatewayUDPConformanceProfile.ExtendedFeatures.UnsortedList()...)
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

func ensureSupportedFeatures(t *testing.T, opts *suite.ConformanceOptions) {
	if len(opts.SupportedFeatures) > 0 {
		return
	}

	declared := declaredSupportedFeatures(t)
	opts.SupportedFeatures = filterSupportedFeaturesForProfiles(declared, sets.New(opts.ConformanceProfiles...)).UnsortedList()
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
	ensureSupportedFeatures(t, &opts)

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
		MaxTimeToConsistency:               120 * time.Second,
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

	// Allow overriding base manifests for split-profile runs. TLS-only excludes
	// HTTP Gateways that conflict on port 443; HTTP/GRPC keeps dedicated Gateway
	// annotations present when the suite reapplies its base resources.
	if bm := os.Getenv("EDGION_BASE_MANIFESTS"); bm != "" {
		t.Logf("Using custom base manifests: %s", bm)
		opts.BaseManifests = bm
	}

	conformance.RunConformanceWithOptions(t, opts)
}
