// Package tal implements the Transport Access Layer (TAL) for Gateway API
// conformance testing. TAL separates "how to reach the Gateway" from "what
// protocol/semantics to test", enabling conformance tests to run in
// environments where the Gateway address (ClusterIP / Pod IP) is not
// directly reachable from the test process (e.g. OrbStack, port-forward).
//
// TAL only rewrites the destination socket address. It never modifies request
// semantics: Host header, :authority, URL path, TLS parameters, request body,
// or test assertions remain untouched.
package tal

import (
	"fmt"
	"net"
	"os"
	"strings"
)

// Resolver maps a Gateway address (host:port) to a locally reachable address.
// In direct mode it returns the original address unchanged.
type Resolver interface {
	// Resolve returns the local address to dial instead of gatewayAddr.
	// gatewayAddr is always in "host:port" form as provided by the conformance
	// framework. The returned address is also in "host:port" form.
	Resolve(gatewayAddr string) (string, error)
}

// PortMap maps well-known Gateway ports to local proxy ports.
// Key: gateway port string (e.g. "80"), Value: local port string (e.g. "18080").
type PortMap map[string]string

// portForwardResolver rewrites Gateway addresses to a fixed proxy host using
// the configured PortMap.
type portForwardResolver struct {
	proxyHost string
	portMap   PortMap
	debug     bool
}

// directResolver returns the original address unchanged (no proxy needed).
type directResolver struct{}

func (d *directResolver) Resolve(gatewayAddr string) (string, error) {
	return gatewayAddr, nil
}

// NewDirectResolver returns a Resolver that dials Gateway addresses directly.
// Use this in environments where Gateway IPs are reachable from the test process.
func NewDirectResolver() Resolver {
	return &directResolver{}
}

// NewPortForwardResolver constructs a Resolver that redirects all Gateway
// traffic through a local proxy (e.g. kubectl port-forward).
//
//   - proxyHost: local host to dial (e.g. "127.0.0.1")
//   - portMap:   mapping from Gateway port → local port
//   - debug:     when true, each Resolve call logs the address rewrite
func NewPortForwardResolver(proxyHost string, portMap PortMap, debug bool) Resolver {
	return &portForwardResolver{
		proxyHost: proxyHost,
		portMap:   portMap,
		debug:     debug,
	}
}

// Resolve implements Resolver for port-forward mode.
// It returns an error (rather than silently falling back to direct) if the
// Gateway port is not present in the PortMap, so misconfigurations are
// surfaced immediately.
func (r *portForwardResolver) Resolve(gatewayAddr string) (string, error) {
	_, gwPort, err := net.SplitHostPort(gatewayAddr)
	if err != nil {
		return "", fmt.Errorf("[TAL] failed to parse gateway address %q: %w", gatewayAddr, err)
	}

	localPort, ok := r.portMap[gwPort]
	if !ok {
		return "", fmt.Errorf(
			"[TAL] no port-forward mapping for gateway address %s (port %s not in port map %v)",
			gatewayAddr, gwPort, r.portMap,
		)
	}

	resolved := net.JoinHostPort(r.proxyHost, localPort)
	if r.debug {
		fmt.Printf("[TAL] resolved %s -> %s\n", gatewayAddr, resolved)
	}
	return resolved, nil
}

// DefaultPortMap returns the standard Gateway port map used by Edgion
// conformance tests:
//
//	80   → httpPort   (HTTP)
//	443  → httpsPort  (HTTPS / HTTP2)
//	8443 → https8443Port (HTTPS on alternate port)
func DefaultPortMap(httpPort, httpsPort, https8443Port string) PortMap {
	return PortMap{
		"80":   httpPort,
		"443":  httpsPort,
		"8443": https8443Port,
	}
}

// ResolverFromEnv builds a Resolver from environment variables.
//
// If GATEWAY_PROXY_HOST is set, port-forward mode is enabled:
//   - GATEWAY_PROXY_PORT_MAP (preferred): "80:18080,443:18443,8443:28443"
//   - Individual variables (fallback):
//     GATEWAY_PROXY_HTTP_PORT      (default 18080)
//     GATEWAY_PROXY_HTTPS_PORT     (default 18443)
//     GATEWAY_PROXY_HTTPS_8443_PORT (default 28443)
//
// If GATEWAY_PROXY_HOST is not set, direct mode is used.
//
// TAL_DEBUG=true enables address-rewrite logging.
func ResolverFromEnv() (Resolver, bool) {
	proxyHost := os.Getenv("GATEWAY_PROXY_HOST")
	if proxyHost == "" {
		return NewDirectResolver(), false
	}

	debug := os.Getenv("TAL_DEBUG") == "true"

	var portMap PortMap

	if raw := os.Getenv("GATEWAY_PROXY_PORT_MAP"); raw != "" {
		pm, err := parsePortMap(raw)
		if err != nil {
			// Treat parse errors as fatal configuration mistakes.
			panic(fmt.Sprintf("[TAL] invalid GATEWAY_PROXY_PORT_MAP %q: %v", raw, err))
		}
		portMap = pm
	} else {
		httpPort := envOr("GATEWAY_PROXY_HTTP_PORT", "18080")
		httpsPort := envOr("GATEWAY_PROXY_HTTPS_PORT", "18443")
		https8443Port := envOr("GATEWAY_PROXY_HTTPS_8443_PORT", "28443")
		portMap = DefaultPortMap(httpPort, httpsPort, https8443Port)
	}

	return NewPortForwardResolver(proxyHost, portMap, debug), true
}

// parsePortMap parses a comma-separated "gwPort:localPort" string into a PortMap.
// Example: "80:18080,443:18443,8443:28443"
func parsePortMap(raw string) (PortMap, error) {
	pm := make(PortMap)
	for _, entry := range strings.Split(raw, ",") {
		entry = strings.TrimSpace(entry)
		if entry == "" {
			continue
		}
		parts := strings.SplitN(entry, ":", 2)
		if len(parts) != 2 || parts[0] == "" || parts[1] == "" {
			return nil, fmt.Errorf("expected \"gwPort:localPort\", got %q", entry)
		}
		pm[parts[0]] = parts[1]
	}
	if len(pm) == 0 {
		return nil, fmt.Errorf("empty port map")
	}
	return pm, nil
}

func envOr(key, defaultVal string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return defaultVal
}
