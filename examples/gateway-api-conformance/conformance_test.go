package conformance_test

import (
	"context"
	"fmt"
	"net"
	"os"
	"testing"
	"time"

	"k8s.io/apimachinery/pkg/util/sets"

	"sigs.k8s.io/gateway-api/conformance"
	"sigs.k8s.io/gateway-api/conformance/utils/config"
	"sigs.k8s.io/gateway-api/conformance/utils/roundtripper"
	"sigs.k8s.io/gateway-api/pkg/features"
)

func TestConformance(t *testing.T) {
	proxyHost := os.Getenv("GATEWAY_PROXY_HOST")
	if proxyHost == "" {
		conformance.RunConformance(t)
		return
	}

	httpPort := os.Getenv("GATEWAY_PROXY_HTTP_PORT")
	if httpPort == "" {
		httpPort = "8080"
	}
	httpsPort := os.Getenv("GATEWAY_PROXY_HTTPS_PORT")
	if httpsPort == "" {
		httpsPort = "8443"
	}

	t.Logf("Using port-forward proxy: %s (HTTP: %s, HTTPS: %s)", proxyHost, httpPort, httpsPort)

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
	opts.SupportedFeatures.Insert(features.SupportHTTPRouteRequestMultipleMirrors)
	opts.SupportedFeatures.Insert(features.SupportHTTPRouteRequestTimeout)
	opts.SupportedFeatures.Insert(features.SupportHTTPRouteBackendTimeout)

	// A-class: HTTPRoute backend-level filters and protocol
	opts.SupportedFeatures.Insert(features.SupportHTTPRouteBackendRequestHeaderModification)
	opts.SupportedFeatures.Insert(features.SupportHTTPRouteBackendProtocolH2C)
	opts.SupportedFeatures.Insert(features.SupportHTTPRouteBackendProtocolWebSocket)
	opts.SupportedFeatures.Insert(features.SupportHTTPRouteNamedRouteRule)

	// A-class: Gateway features
	opts.SupportedFeatures.Insert(features.SupportGatewayHTTPListenerIsolation)
	opts.SupportedFeatures.Insert(features.SupportGatewayPort8080)

	// B-class: HTTPRoute redirect (path/port/scheme)
	opts.SupportedFeatures.Insert(features.SupportHTTPRoutePathRedirect)
	opts.SupportedFeatures.Insert(features.SupportHTTPRoutePortRedirect)
	opts.SupportedFeatures.Insert(features.SupportHTTPRouteSchemeRedirect)

	// B-class: HTTPRoute percentage mirror
	opts.SupportedFeatures.Insert(features.SupportHTTPRouteRequestPercentageMirror)

	// B-class: HTTPRoute parentRef port matching
	opts.SupportedFeatures.Insert(features.SupportHTTPRouteParentRefPort)

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
		RequiredConsecutiveSuccesses:        3,
	}

	opts.RoundTripper = &roundtripper.DefaultRoundTripper{
		Debug:         opts.Debug,
		TimeoutConfig: opts.TimeoutConfig,
		CustomDialContext: func(ctx context.Context, network, addr string) (net.Conn, error) {
			_, port, err := net.SplitHostPort(addr)
			if err != nil {
				return nil, err
			}
			switch port {
			case "80":
				addr = net.JoinHostPort(proxyHost, httpPort)
			case "443":
				addr = net.JoinHostPort(proxyHost, httpsPort)
			default:
				return nil, fmt.Errorf("unexpected port in gateway address: %s", addr)
			}
			return (&net.Dialer{}).DialContext(ctx, network, addr)
		},
	}

	conformance.RunConformanceWithOptions(t, opts)
}
