package tal

import (
	"context"
	"net"

	"sigs.k8s.io/gateway-api/conformance/utils/config"
	"sigs.k8s.io/gateway-api/conformance/utils/roundtripper"
)

// NewRoundTripper returns a roundtripper.RoundTripper that rewrites the
// destination address of every HTTP/HTTPS/H2C connection through the
// provided Resolver before dialing.
//
// All request semantics (Host header, :authority, URL path, TLS config,
// request body) are preserved unchanged — only the TCP dial target is
// rewritten.
//
// When resolver is a directResolver the returned tripper behaves identically
// to a plain DefaultRoundTripper (no overhead).
func NewRoundTripper(resolver Resolver, debug bool, timeoutConfig config.TimeoutConfig) roundtripper.RoundTripper {
	return &roundtripper.DefaultRoundTripper{
		Debug:         debug,
		TimeoutConfig: timeoutConfig,
		CustomDialContext: func(ctx context.Context, network, addr string) (net.Conn, error) {
			resolved, err := resolver.Resolve(addr)
			if err != nil {
				return nil, err
			}
			return (&net.Dialer{}).DialContext(ctx, network, resolved)
		},
	}
}
