package conformance_test

import (
	"testing"

	"k8s.io/apimachinery/pkg/util/sets"

	"sigs.k8s.io/gateway-api/conformance/utils/suite"
	"sigs.k8s.io/gateway-api/pkg/features"
)

func TestEnsureConformanceProfilesUsesDefaultsWhenUnset(t *testing.T) {
	opts := suite.ConformanceOptions{}

	ensureConformanceProfiles(&opts)

	got := sets.New(opts.ConformanceProfiles...)
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
		ConfigurableOptions: suite.ConfigurableOptions{ConformanceProfiles: want.UnsortedList()},
	}

	ensureConformanceProfiles(&opts)

	got := sets.New(opts.ConformanceProfiles...)
	if !got.Equal(want) {
		t.Fatalf("explicit conformance profiles should be preserved: got %v want %v", got.UnsortedList(), want.UnsortedList())
	}
}

func TestEnsureSupportedFeaturesFiltersToSelectedProfile(t *testing.T) {
	opts := suite.ConformanceOptions{
		ConfigurableOptions: suite.ConfigurableOptions{ConformanceProfiles: []suite.ConformanceProfileName{suite.GatewayGRPCConformanceProfileName}},
	}

	ensureSupportedFeatures(t, &opts)

	if !sets.New(opts.SupportedFeatures...).Has(features.SupportGRPCRoute) {
		t.Fatalf("expected gRPC feature to remain enabled")
	}
	if sets.New(opts.SupportedFeatures...).Has(features.SupportTLSRoute) {
		t.Fatalf("did not expect TLSRoute feature when only GATEWAY-GRPC is selected")
	}
	if sets.New(opts.SupportedFeatures...).Has(features.SupportTLSRouteModeTerminate) {
		t.Fatalf("did not expect TLS terminate feature when only GATEWAY-GRPC is selected")
	}
}

func TestV1StreamProfilesMapFeaturesWithoutAdvertising(t *testing.T) {
	for _, tc := range []struct {
		name    suite.ConformanceProfileName
		feature features.FeatureName
	}{
		{suite.GatewayTCPConformanceProfileName, features.SupportTCPRoute},
		{suite.GatewayUDPConformanceProfileName, features.SupportUDPRoute},
	} {
		t.Run(string(tc.name), func(t *testing.T) {
			if !featuresForProfile(tc.name).Has(tc.feature) {
				t.Fatalf("%s profile must map %s for explicit conformance runs", tc.name, tc.feature)
			}
			opts := suite.ConformanceOptions{
				ConfigurableOptions: suite.ConfigurableOptions{ConformanceProfiles: []suite.ConformanceProfileName{tc.name}},
			}
			ensureSupportedFeatures(t, &opts)
			if sets.New(opts.SupportedFeatures...).Has(tc.feature) {
				t.Fatalf("%s must remain unadvertised until %s passes", tc.feature, tc.name)
			}
		})
	}
}

func TestEnsureSupportedFeaturesPreservesExplicitFeatures(t *testing.T) {
	want := sets.New[features.FeatureName](features.SupportGRPCRoute, features.SupportTLSRoute)
	opts := suite.ConformanceOptions{
		ConfigurableOptions: suite.ConfigurableOptions{
			ConformanceProfiles: []suite.ConformanceProfileName{suite.GatewayGRPCConformanceProfileName},
			SupportedFeatures:   want.UnsortedList(),
		},
	}

	ensureSupportedFeatures(t, &opts)

	if !sets.New(opts.SupportedFeatures...).Equal(want) {
		t.Fatalf("explicit supported features should be preserved: got %v want %v", opts.SupportedFeatures, want.UnsortedList())
	}
}
