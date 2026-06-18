package conformance_test

import (
	"testing"

	"k8s.io/apimachinery/pkg/util/sets"

	"sigs.k8s.io/gateway-api/pkg/features"
	"sigs.k8s.io/gateway-api/conformance/utils/suite"
)

func TestEnsureConformanceProfilesUsesDefaultsWhenUnset(t *testing.T) {
	opts := suite.ConformanceOptions{}

	ensureConformanceProfiles(&opts)

	got := opts.ConformanceProfiles
	want := sets.New[suite.ConformanceProfileName](
		suite.GatewayHTTPConformanceProfileName,
		suite.GatewayGRPCConformanceProfileName,
		suite.GatewayTLSConformanceProfileName,
	)

	if !got.Equal(want) {
		t.Fatalf("unexpected default conformance profiles: got %v want %v", got.UnsortedList(), want.UnsortedList())
	}
}

func TestEnsureConformanceProfilesPreservesExplicitSelection(t *testing.T) {
	want := sets.New[suite.ConformanceProfileName](suite.GatewayGRPCConformanceProfileName)
	opts := suite.ConformanceOptions{
		ConformanceProfiles: want.Clone(),
	}

	ensureConformanceProfiles(&opts)

	got := opts.ConformanceProfiles
	if !got.Equal(want) {
		t.Fatalf("explicit conformance profiles should be preserved: got %v want %v", got.UnsortedList(), want.UnsortedList())
	}
}

func TestEnsureSupportedFeaturesFiltersToSelectedProfile(t *testing.T) {
	opts := suite.ConformanceOptions{
		ConformanceProfiles: sets.New[suite.ConformanceProfileName](suite.GatewayGRPCConformanceProfileName),
	}

	ensureSupportedFeatures(t, &opts)

	if !opts.SupportedFeatures.Has(features.SupportGRPCRoute) {
		t.Fatalf("expected gRPC feature to remain enabled")
	}
	if opts.SupportedFeatures.Has(features.SupportTLSRoute) {
		t.Fatalf("did not expect TLSRoute feature when only GATEWAY-GRPC is selected")
	}
	if opts.SupportedFeatures.Has(features.SupportTLSRouteModeTerminate) {
		t.Fatalf("did not expect TLS terminate feature when only GATEWAY-GRPC is selected")
	}
}

func TestEnsureSupportedFeaturesPreservesExplicitFeatures(t *testing.T) {
	want := sets.New[features.FeatureName](features.SupportGRPCRoute, features.SupportTLSRoute)
	opts := suite.ConformanceOptions{
		ConformanceProfiles: sets.New[suite.ConformanceProfileName](suite.GatewayGRPCConformanceProfileName),
		SupportedFeatures:   want.Clone(),
	}

	ensureSupportedFeatures(t, &opts)

	if !opts.SupportedFeatures.Equal(want) {
		t.Fatalf("explicit supported features should be preserved: got %v want %v", opts.SupportedFeatures.UnsortedList(), want.UnsortedList())
	}
}
