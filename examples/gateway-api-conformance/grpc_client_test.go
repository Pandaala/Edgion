package conformance_test

// TAL gRPC client — wraps tal.Resolver to route all gRPC conformance traffic
// through the configured address resolver (port-forward or direct).
//
// This file lives in the conformance_test package (not in tal/) because the
// confgrpc.Client interface accepts *testing.T, making it test-only by design.

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

	"edgion.com/gateway-api-conformance/tal"
)

// grpcClient implements confgrpc.Client, routing connections through a
// tal.Resolver. Only the TCP dial target is rewritten; Authority and
// metadata are preserved unchanged.
type grpcClient struct {
	resolver tal.Resolver
}

func newGRPCClient(resolver tal.Resolver) *grpcClient {
	return &grpcClient{resolver: resolver}
}

func (c *grpcClient) dialOptions(req *confgrpc.RequestMetadata) []grpc.DialOption {
	dialOpts := []grpc.DialOption{
		grpc.WithTransportCredentials(insecure.NewCredentials()),
		grpc.WithContextDialer(func(ctx context.Context, addr string) (net.Conn, error) {
			resolved, err := c.resolver.Resolve(addr)
			if err != nil {
				return nil, fmt.Errorf("[TAL] grpc dial: %w", err)
			}
			return (&net.Dialer{}).DialContext(ctx, "tcp", resolved)
		}),
	}

	if req != nil && req.Authority != "" {
		dialOpts = append(dialOpts, grpc.WithAuthority(req.Authority))
	}

	return dialOpts
}

// SendRPC implements confgrpc.Client.
func (c *grpcClient) SendRPC(t *testing.T, address string, expected confgrpc.ExpectedResponse, timeout time.Duration) (*confgrpc.Response, error) {
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

// Close implements confgrpc.Client.
func (c *grpcClient) Close() {}
