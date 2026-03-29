package conformance_test

import (
	"context"
	"fmt"
	"net"
	"testing"
	"time"

	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/credentials/insecure"
	"google.golang.org/grpc/metadata"
	"google.golang.org/grpc/status"

	pb "sigs.k8s.io/gateway-api/conformance/echo-basic/grpcechoserver"
	confgrpc "sigs.k8s.io/gateway-api/conformance/utils/grpc"
)

// PortForwardGRPCClient routes conformance gRPC traffic through the same
// localhost port-forward proxy used by the HTTP round tripper.
type PortForwardGRPCClient struct {
	proxyHost     string
	httpPort      string
	httpsPort     string
	https8443Port string
}

func NewPortForwardGRPCClient(proxyHost, httpPort, httpsPort, https8443Port string) *PortForwardGRPCClient {
	return &PortForwardGRPCClient{
		proxyHost:     proxyHost,
		httpPort:      httpPort,
		httpsPort:     httpsPort,
		https8443Port: https8443Port,
	}
}

func (c *PortForwardGRPCClient) proxyAddress(addr string) (string, error) {
	_, port, err := net.SplitHostPort(addr)
	if err != nil {
		return "", err
	}

	switch port {
	case "80":
		return net.JoinHostPort(c.proxyHost, c.httpPort), nil
	case "443":
		return net.JoinHostPort(c.proxyHost, c.httpsPort), nil
	case "8443":
		return net.JoinHostPort(c.proxyHost, c.https8443Port), nil
	default:
		return "", fmt.Errorf("unexpected port in gateway address: %s", addr)
	}
}

func (c *PortForwardGRPCClient) dialOptions(req *confgrpc.RequestMetadata) []grpc.DialOption {
	dialOpts := []grpc.DialOption{
		grpc.WithTransportCredentials(insecure.NewCredentials()),
		grpc.WithContextDialer(func(ctx context.Context, addr string) (net.Conn, error) {
			targetAddr, err := c.proxyAddress(addr)
			if err != nil {
				return nil, err
			}
			return (&net.Dialer{}).DialContext(ctx, "tcp", targetAddr)
		}),
	}

	if req != nil && req.Authority != "" {
		dialOpts = append(dialOpts, grpc.WithAuthority(req.Authority))
	}

	return dialOpts
}

func (c *PortForwardGRPCClient) SendRPC(t *testing.T, address string, expected confgrpc.ExpectedResponse, timeout time.Duration) (*confgrpc.Response, error) {
	t.Helper()

	conn, err := grpc.NewClient(address, c.dialOptions(expected.RequestMetadata)...)
	if err != nil {
		return &confgrpc.Response{}, err
	}
	defer conn.Close()

	resp := &confgrpc.Response{
		Headers:  &metadata.MD{},
		Trailers: &metadata.MD{},
	}

	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()

	if expected.RequestMetadata != nil && len(expected.RequestMetadata.Metadata) > 0 {
		ctx = metadata.NewOutgoingContext(ctx, metadata.New(expected.RequestMetadata.Metadata))
	}

	stub := pb.NewGrpcEchoClient(conn)

	switch {
	case expected.EchoRequest != nil:
		resp.Response, err = stub.Echo(ctx, expected.EchoRequest, grpc.Header(resp.Headers), grpc.Trailer(resp.Trailers))
	case expected.EchoTwoRequest != nil:
		resp.Response, err = stub.EchoTwo(ctx, expected.EchoTwoRequest, grpc.Header(resp.Headers), grpc.Trailer(resp.Trailers))
	case expected.EchoThreeRequest != nil:
		resp.Response, err = stub.EchoThree(ctx, expected.EchoThreeRequest, grpc.Header(resp.Headers), grpc.Trailer(resp.Trailers))
	default:
		return resp, fmt.Errorf("no request specified")
	}

	if err != nil {
		resp.Code = status.Code(err)
		return resp, nil
	}

	resp.Code = codes.OK
	return resp, nil
}

func (c *PortForwardGRPCClient) Close() {}
