package main

import (
	"context"
	"crypto/sha256"
	"crypto/tls"
	"crypto/x509"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	corev3 "github.com/envoyproxy/go-control-plane/envoy/config/core/v3"
	extprocmodev3 "github.com/envoyproxy/go-control-plane/envoy/extensions/filters/http/ext_proc/v3"
	extprocv3 "github.com/envoyproxy/go-control-plane/envoy/service/ext_proc/v3"
	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/credentials"
	"google.golang.org/grpc/metadata"
	"google.golang.org/grpc/status"
)

type fakeProcessStream struct {
	ctx      context.Context
	requests []*extprocv3.ProcessingRequest
	sent     []*extprocv3.ProcessingResponse
}

func (s *fakeProcessStream) Send(response *extprocv3.ProcessingResponse) error {
	s.sent = append(s.sent, response)
	return nil
}

func (s *fakeProcessStream) Recv() (*extprocv3.ProcessingRequest, error) {
	if len(s.requests) == 0 {
		return nil, io.EOF
	}
	request := s.requests[0]
	s.requests = s.requests[1:]
	return request, nil
}

func (s *fakeProcessStream) SetHeader(metadata.MD) error  { return nil }
func (s *fakeProcessStream) SendHeader(metadata.MD) error { return nil }
func (s *fakeProcessStream) SetTrailer(metadata.MD)       {}
func (s *fakeProcessStream) Context() context.Context     { return s.ctx }
func (s *fakeProcessStream) SendMsg(any) error            { return nil }
func (s *fakeProcessStream) RecvMsg(any) error            { return nil }

func requestHeaders(caseID, mode string) *extprocv3.ProcessingRequest {
	return requestHeadersWithBarrier(caseID, mode, "")
}

func requestHeadersWithBarrier(caseID, mode, barrierID string) *extprocv3.ProcessingRequest {
	headers := []*corev3.HeaderValue{
		{Key: caseIDHeader, RawValue: []byte(caseID)},
		{Key: modeHeader, Value: mode},
	}
	if barrierID != "" {
		headers = append(headers, &corev3.HeaderValue{Key: barrierHeader, Value: barrierID})
	}
	return &extprocv3.ProcessingRequest{Request: &extprocv3.ProcessingRequest_RequestHeaders{RequestHeaders: &extprocv3.HttpHeaders{
		Headers: &corev3.HeaderMap{Headers: headers},
	}}}
}

func requestBody(body string, eos bool) *extprocv3.ProcessingRequest {
	return &extprocv3.ProcessingRequest{Request: &extprocv3.ProcessingRequest_RequestBody{RequestBody: &extprocv3.HttpBody{Body: []byte(body), EndOfStream: eos}}}
}

func responseHeaders() *extprocv3.ProcessingRequest {
	return responseHeadersStatus(200)
}

func responseHeadersStatus(statusCode uint16) *extprocv3.ProcessingRequest {
	return &extprocv3.ProcessingRequest{Request: &extprocv3.ProcessingRequest_ResponseHeaders{ResponseHeaders: &extprocv3.HttpHeaders{
		Headers: &corev3.HeaderMap{Headers: []*corev3.HeaderValue{{Key: ":status", Value: fmtInt(int(statusCode))}}},
	}}}
}

func responseBody(body string, eos bool) *extprocv3.ProcessingRequest {
	return &extprocv3.ProcessingRequest{Request: &extprocv3.ProcessingRequest_ResponseBody{ResponseBody: &extprocv3.HttpBody{Body: []byte(body), EndOfStream: eos}}}
}

func responseTrailers() *extprocv3.ProcessingRequest {
	return &extprocv3.ProcessingRequest{Request: &extprocv3.ProcessingRequest_ResponseTrailers{ResponseTrailers: &extprocv3.HttpTrailers{
		Trailers: &corev3.HeaderMap{Headers: []*corev3.HeaderValue{
			{Key: "x-edgion-upstream-trailer", RawValue: []byte("fixed-upstream-value")},
			{Key: "grpc-status", RawValue: []byte("0")},
		}},
	}}}
}

func TestProcessRecordsPhasesBodiesAndHalfClose(t *testing.T) {
	store := newObservationStore()
	stream := &fakeProcessStream{
		ctx: context.Background(),
		requests: []*extprocv3.ProcessingRequest{
			requestHeaders("case-1", "interop"),
			requestBody("request", true),
			responseHeaders(),
			responseBody("response", true),
		},
	}
	if err := (&processor{store: store}).Process(stream); err != nil {
		t.Fatalf("Process() error = %v", err)
	}
	if len(stream.sent) != 4 {
		t.Fatalf("sent responses = %d, want 4", len(stream.sent))
	}
	if got := stream.sent[0].GetRequestHeaders().GetResponse().GetHeaderMutation().GetSetHeaders()[0].GetHeader().GetRawValue(); string(got) != "external-go" {
		t.Fatalf("request header mutation = %q", got)
	}
	if got := stream.sent[1].GetRequestBody().GetResponse().GetBodyMutation().GetBody(); string(got) != "processed-by-external-go" {
		t.Fatalf("request body mutation = %q", got)
	}
	if got := stream.sent[2].GetResponseHeaders().GetResponse().GetHeaderMutation().GetSetHeaders()[0].GetHeader().GetRawValue(); string(got) != "external-go" {
		t.Fatalf("response header mutation = %q", got)
	}
	if got := stream.sent[3].GetResponseBody().GetResponse().GetBodyMutation().GetBody(); string(got) != "response-by-external-go" {
		t.Fatalf("response body mutation = %q", got)
	}

	snapshot, ok := store.snapshot("case-1")
	if !ok || len(snapshot.Streams) != 1 {
		t.Fatalf("snapshot = %+v, present = %v", snapshot, ok)
	}
	got := snapshot.Streams[0]
	wantPhases := []string{"request_headers", "request_body", "response_headers", "response_body"}
	if strings.Join(got.Phases, ",") != strings.Join(wantPhases, ",") {
		t.Fatalf("phases = %v, want %v", got.Phases, wantPhases)
	}
	requestSum := sha256.Sum256([]byte("request"))
	responseSum := sha256.Sum256([]byte("response"))
	if got.RequestBodyMessages != 1 || got.RequestBodyBytes != 7 || got.RequestEOSCount != 1 || got.RequestBodySHA256 != hex.EncodeToString(requestSum[:]) {
		t.Fatalf("request body observation = %+v", got)
	}
	if got.ResponseBodyMessages != 1 || got.ResponseBodyBytes != 8 || got.ResponseEOSCount != 1 || got.ResponseBodySHA256 != hex.EncodeToString(responseSum[:]) {
		t.Fatalf("response body observation = %+v", got)
	}
	if got.RequestHeaderMessages != 1 || got.ResponseHeaderMessages != 1 || !slices.Equal(got.ResponseStatuses, []uint16{200}) {
		t.Fatalf("header observation = %+v", got)
	}
	if !got.ClientHalfClosed || got.Cancelled || got.Active {
		t.Fatalf("terminal state = half_closed:%v cancelled:%v active:%v", got.ClientHalfClosed, got.Cancelled, got.Active)
	}
}

func TestImmediateResponseContract(t *testing.T) {
	response := immediateResponse().GetImmediateResponse()
	if response.GetStatus().GetCode() != 403 || string(response.GetBody()) != "blocked by external ext_proc" {
		t.Fatalf("immediate response = %+v", response)
	}
	header := response.GetHeaders().GetSetHeaders()[0].GetHeader()
	if header.GetKey() != "x-ext-proc-denied" || string(header.GetRawValue()) != "1" {
		t.Fatalf("immediate response header = %+v", header)
	}
	snapshot := (&streamObservation{}).snapshot()
	encoded, err := json.Marshal(snapshot)
	if err != nil {
		t.Fatalf("marshal empty response-status snapshot: %v", err)
	}
	if !strings.Contains(string(encoded), `"response_statuses":[]`) {
		t.Fatalf("empty response statuses must be a JSON array: %s", encoded)
	}
}

func TestOfficialEnvoyTypesPinNewWireFields(t *testing.T) {
	immediateFields := (&extprocv3.ImmediateResponse{}).ProtoReflect().Descriptor().Fields()
	if field := immediateFields.ByName("grpc_status"); field == nil || field.Number() != 4 {
		t.Fatalf("ImmediateResponse.grpc_status descriptor = %v, want field 4", field)
	}
	responseFields := (&extprocv3.ProcessingResponse{}).ProtoReflect().Descriptor().Fields()
	if field := responseFields.ByName("mode_override"); field == nil || field.Number() != 9 {
		t.Fatalf("ProcessingResponse.mode_override descriptor = %v, want field 9", field)
	}
	modeFields := (&extprocmodev3.ProcessingMode{}).ProtoReflect().Descriptor().Fields()
	if field := modeFields.ByName("response_trailer_mode"); field == nil || field.Number() != 6 {
		t.Fatalf("ProcessingMode.response_trailer_mode descriptor = %v, want field 6", field)
	}
	requestFields := (&extprocv3.ProcessingRequest{}).ProtoReflect().Descriptor().Fields()
	if field := requestFields.ByName("response_trailers"); field == nil || field.Number() != 7 {
		t.Fatalf("ProcessingRequest.response_trailers descriptor = %v, want field 7", field)
	}
}

func TestNewOfficialResponsesAndTrailerObservation(t *testing.T) {
	grpcResponse := grpcImmediateResponse().GetImmediateResponse()
	if grpcResponse.GetGrpcStatus().GetStatus() != uint32(codes.PermissionDenied) ||
		string(grpcResponse.GetBody()) != "processor denied exact grpc request" {
		t.Fatalf("exact gRPC ImmediateResponse = %+v", grpcResponse)
	}

	reduction := headerResponseWithModeOverride(extprocmodev3.ProcessingMode_NONE).GetModeOverride()
	if reduction.GetRequestBodyMode() != extprocmodev3.ProcessingMode_NONE {
		t.Fatalf("reduction override = %+v", reduction)
	}
	expansion := headerResponseWithModeOverride(extprocmodev3.ProcessingMode_BUFFERED).GetModeOverride()
	if expansion.GetRequestBodyMode() != extprocmodev3.ProcessingMode_BUFFERED {
		t.Fatalf("expansion override = %+v", expansion)
	}

	store := newObservationStore()
	stream := &fakeProcessStream{
		ctx: context.Background(),
		requests: []*extprocv3.ProcessingRequest{
			requestHeaders("trailer-case", "trailer-mutate"),
			responseHeaders(),
			responseTrailers(),
		},
	}
	if err := (&processor{store: store}).Process(stream); err != nil {
		t.Fatalf("Process() error = %v", err)
	}
	if len(stream.sent) != 3 {
		t.Fatalf("sent responses = %d, want 3", len(stream.sent))
	}
	mutation := stream.sent[2].GetResponseTrailers().GetHeaderMutation().GetSetHeaders()
	if len(mutation) != 1 || mutation[0].GetHeader().GetKey() != "x-edgion-upstream-trailer" ||
		string(mutation[0].GetHeader().GetRawValue()) != "processed-by-external-go" {
		t.Fatalf("response trailer mutation = %+v", mutation)
	}
	snapshot, ok := store.snapshot("trailer-case")
	if !ok || len(snapshot.Streams) != 1 {
		t.Fatalf("snapshot = %+v, present = %v", snapshot, ok)
	}
	got := snapshot.Streams[0]
	if got.ResponseTrailerMessages != 1 || got.ResponseTrailerFields != 2 ||
		!got.SawFixedResponseTrailer || !got.SawGRPCStatusTrailer ||
		!slices.Equal(got.Phases, []string{"request_headers", "response_headers", "response_trailers"}) {
		t.Fatalf("response trailer observation = %+v", got)
	}
}

func TestLongIdleStopsWhenStreamIsCancelled(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	store := newObservationStore()
	observation, err := store.begin("cancel-case")
	if err != nil {
		t.Fatal(err)
	}
	stream := &fakeProcessStream{ctx: ctx}
	started := time.Now()
	barrierUsed := false
	err = (&processor{store: store}).observeAndRespond(
		stream, observation, "long-idle", "", &barrierUsed, requestHeaders("cancel-case", "long-idle"),
	)
	if err == nil || time.Since(started) > time.Second {
		t.Fatalf("cancelled delay returned %v after %v", err, time.Since(started))
	}
}

func TestCaseIDAndModeValidation(t *testing.T) {
	for _, value := range []string{"a", "Case_1.test", strings.Repeat("a", 64)} {
		if !validCaseID(value) {
			t.Errorf("validCaseID(%q) = false", value)
		}
	}
	for _, value := range []string{"", "-starts-with-symbol", "has/slash", strings.Repeat("a", 65), "nonascii-é"} {
		if validCaseID(value) {
			t.Errorf("validCaseID(%q) = true", value)
		}
	}
	for _, value := range []string{
		"interop", "immediate", "grpc-immediate", "disabled-immediate", "mode-reduce-none",
		"mode-forbidden-expand", "trailer-mutate", "no-trailer", "long-idle", "barrier-headers",
		"barrier-request-body",
	} {
		if !validMode(value) {
			t.Errorf("validMode(%q) = false", value)
		}
	}
	if validMode("unknown") {
		t.Error("validMode(unknown) = true")
	}
}

func TestObservationStoreEvictsOnlyCompletedCases(t *testing.T) {
	store := newObservationStore()
	active := make([]*streamObservation, 0, maxCases)
	for i := 0; i < maxCases; i++ {
		stream, err := store.begin("case-" + strings.Repeat("x", 3) + string(rune('A'+i%26)) + fmtInt(i))
		if err != nil {
			t.Fatalf("begin case %d: %v", i, err)
		}
		active = append(active, stream)
	}
	if _, err := store.begin("overflow-active"); err == nil {
		t.Fatal("active capacity overflow succeeded")
	}
	store.finish(active[0], false, false)
	if _, err := store.begin("replacement"); err != nil {
		t.Fatalf("completed case was not evicted: %v", err)
	}
}

func fmtInt(value int) string {
	const digits = "0123456789"
	if value < 10 {
		return string(digits[value])
	}
	return fmtInt(value/10) + string(digits[value%10])
}

func TestResetRejectsActiveStream(t *testing.T) {
	store := newObservationStore()
	stream, err := store.begin("reset-case")
	if err != nil {
		t.Fatal(err)
	}
	if err := store.reset("reset-case"); err == nil {
		t.Fatal("reset active case succeeded")
	}
	store.finish(stream, true, false)
	if err := store.reset("reset-case"); err != nil {
		t.Fatalf("reset completed case: %v", err)
	}
}

func TestObservationEventAndStreamLimits(t *testing.T) {
	store := newObservationStore()
	stream, err := store.begin("limit-case")
	if err != nil {
		t.Fatal(err)
	}
	for i := 0; i < maxPhaseTransitionsPerStream; i++ {
		phase := "request_headers"
		if i%2 == 1 {
			phase = "response_headers"
		}
		var err error
		if phase == "response_headers" {
			statusCode := uint16(200)
			err = store.addPhaseWithStatus(stream, phase, nil, false, &statusCode)
		} else {
			err = store.addPhase(stream, phase, nil, false)
		}
		if err != nil {
			t.Fatalf("phase transition %d rejected: %v", i, err)
		}
	}
	if err := store.addPhase(stream, "request_body", nil, false); err == nil {
		t.Fatal("phase transition beyond limit succeeded")
	}

	bodyMessages := &bodyObservation{messages: maxBodyMessagesPerDirection}
	if err := bodyMessages.add(nil, false); err == nil {
		t.Fatal("body message beyond limit succeeded")
	}
	bodyBytes := &bodyObservation{bytes: maxObservedBodyBytes}
	if err := bodyBytes.add([]byte("x"), false); err == nil {
		t.Fatal("body bytes beyond limit succeeded")
	}

	streamCase := newObservationStore()
	for i := 0; i < maxStreamsPerCase; i++ {
		stream, err := streamCase.begin("stream-limit")
		if err != nil {
			t.Fatalf("stream %d rejected: %v", i, err)
		}
		streamCase.finish(stream, true, false)
	}
	if _, err := streamCase.begin("stream-limit"); err == nil {
		t.Fatal("stream beyond per-case limit succeeded")
	}
}

func TestRepeatedBodyMessagesCompactPhasesButKeepIndependentCounts(t *testing.T) {
	store := newObservationStore()
	stream, err := store.begin("compact-case")
	if err != nil {
		t.Fatal(err)
	}
	for i := 0; i < 100; i++ {
		if err := store.addPhase(stream, "request_body", []byte("ab"), i == 99); err != nil {
			t.Fatal(err)
		}
	}
	statusCode := uint16(200)
	if err := store.addPhaseWithStatus(stream, "response_headers", nil, false, &statusCode); err != nil {
		t.Fatal(err)
	}
	for i := 0; i < 3; i++ {
		if err := store.addPhase(stream, "response_body", []byte("xyz"), i == 2); err != nil {
			t.Fatal(err)
		}
	}
	snapshot, _ := store.snapshot("compact-case")
	got := snapshot.Streams[0]
	if strings.Join(got.Phases, ",") != "request_body,response_headers,response_body" {
		t.Fatalf("compacted phases = %v", got.Phases)
	}
	if got.RequestBodyMessages != 100 || got.RequestBodyBytes != 200 || got.RequestEOSCount != 1 {
		t.Fatalf("request counters = %+v", got)
	}
	if got.ResponseBodyMessages != 3 || got.ResponseBodyBytes != 9 || got.ResponseEOSCount != 1 {
		t.Fatalf("response counters = %+v", got)
	}
	if maxConcurrentStreams <= 64 {
		t.Fatalf("fixture concurrency cap %d leaves no headroom above Edgion's cap", maxConcurrentStreams)
	}
}

func TestConsecutiveResponseHeadersKeepExactMessagesAndStatuses(t *testing.T) {
	store := newObservationStore()
	stream := &fakeProcessStream{ctx: context.Background(), requests: []*extprocv3.ProcessingRequest{
		requestHeaders("retry-status-case", "interop"),
		responseHeadersStatus(503),
		responseHeadersStatus(200),
	}}
	if err := (&processor{store: store}).Process(stream); err != nil {
		t.Fatal(err)
	}
	snapshot, _ := store.snapshot("retry-status-case")
	got := snapshot.Streams[0]
	if !slices.Equal(got.Phases, []string{"request_headers", "response_headers"}) {
		t.Fatalf("phase transitions = %v", got.Phases)
	}
	if got.ResponseHeaderMessages != 2 || !slices.Equal(got.ResponseStatuses, []uint16{503, 200}) {
		t.Fatalf("response header observation = %+v", got)
	}
	snapshot.Streams[0].ResponseStatuses[0] = 418
	second, _ := store.snapshot("retry-status-case")
	if !slices.Equal(second.Streams[0].ResponseStatuses, []uint16{503, 200}) {
		t.Fatalf("snapshot mutation changed stored statuses: %v", second.Streams[0].ResponseStatuses)
	}
}

func TestHeaderObservationCapsFailResourceExhausted(t *testing.T) {
	store := newObservationStore()
	requests := []*extprocv3.ProcessingRequest{requestHeaders("header-cap-case", "interop")}
	for i := 0; i <= maxHeaderMessagesPerDirection; i++ {
		requests = append(requests, responseHeadersStatus(200))
	}
	stream := &fakeProcessStream{ctx: context.Background(), requests: requests}
	err := (&processor{store: store}).Process(stream)
	if status.Code(err) != codes.ResourceExhausted {
		t.Fatalf("response header cap returned %v", err)
	}
	snapshot := terminalStream(t, store, "header-cap-case")
	if snapshot.ResponseHeaderMessages != maxHeaderMessagesPerDirection || len(snapshot.ResponseStatuses) != maxHeaderMessagesPerDirection {
		t.Fatalf("capped response header observation = %+v", snapshot)
	}

	requestStore := newObservationStore()
	requestStream, err := requestStore.begin("request-header-cap")
	if err != nil {
		t.Fatal(err)
	}
	requestStream.RequestHeaderMessages = maxHeaderMessagesPerDirection
	if err := requestStore.addPhase(requestStream, "request_headers", nil, false); err == nil {
		t.Fatal("request header message beyond cap succeeded")
	}
}

func TestProcessorRejectsGlobalConcurrencyOverflow(t *testing.T) {
	slots := make(chan struct{}, 1)
	slots <- struct{}{}
	stream := &fakeProcessStream{ctx: context.Background()}
	err := (&processor{store: newObservationStore(), slots: slots}).Process(stream)
	if err == nil {
		t.Fatal("processor accepted stream beyond global concurrency limit")
	}
}

func waitForBarrier(t *testing.T, registry *barrierRegistry, barrierID string, waiting int) barrierSnapshot {
	t.Helper()
	deadline := time.Now().Add(time.Second)
	for {
		if snapshot, ok := registry.snapshot(barrierID); ok && snapshot.Waiting == waiting {
			return snapshot
		}
		if time.Now().After(deadline) {
			snapshot, _ := registry.snapshot(barrierID)
			t.Fatalf("barrier %q did not reach waiting=%d: %+v", barrierID, waiting, snapshot)
		}
		time.Sleep(time.Millisecond)
	}
}

func TestBarrierRegistryReleaseOneReleaseAllAndReset(t *testing.T) {
	registry := newBarrierRegistry(time.Second)
	results := make(chan error, 2)
	for i := 0; i < 2; i++ {
		go func() { results <- registry.wait(context.Background(), "shared-barrier") }()
	}
	snapshot := waitForBarrier(t, registry, "shared-barrier", 2)
	if snapshot.Arrivals != 2 || snapshot.Releases != 0 || snapshot.ReleasedAll {
		t.Fatalf("initial barrier snapshot = %+v", snapshot)
	}
	if err := registry.reset("shared-barrier"); err == nil {
		t.Fatal("reset with waiters succeeded")
	}
	if err := registry.releaseOne("shared-barrier"); err != nil {
		t.Fatal(err)
	}
	select {
	case err := <-results:
		if err != nil {
			t.Fatalf("released waiter returned %v", err)
		}
	case <-time.After(time.Second):
		t.Fatal("release-one did not wake a waiter")
	}
	snapshot = waitForBarrier(t, registry, "shared-barrier", 1)
	if snapshot.Releases != 1 || snapshot.ReleasedAll {
		t.Fatalf("release-one snapshot = %+v", snapshot)
	}
	if err := registry.releaseAll("shared-barrier"); err != nil {
		t.Fatal(err)
	}
	select {
	case err := <-results:
		if err != nil {
			t.Fatalf("release-all waiter returned %v", err)
		}
	case <-time.After(time.Second):
		t.Fatal("release-all did not wake remaining waiter")
	}
	snapshot = waitForBarrier(t, registry, "shared-barrier", 0)
	if snapshot.Releases != 2 || !snapshot.ReleasedAll {
		t.Fatalf("release-all snapshot = %+v", snapshot)
	}
	if err := registry.wait(context.Background(), "shared-barrier"); err != nil {
		t.Fatalf("arrival after release-all blocked: %v", err)
	}
	snapshot, _ = registry.snapshot("shared-barrier")
	if snapshot.Arrivals != 3 || snapshot.Releases != 3 || snapshot.Waiting != 0 {
		t.Fatalf("open barrier snapshot = %+v", snapshot)
	}
	if err := registry.reset("shared-barrier"); err != nil {
		t.Fatalf("reset released barrier: %v", err)
	}
	if _, ok := registry.snapshot("shared-barrier"); ok {
		t.Fatal("reset barrier still exists")
	}
	if err := registry.releaseAll("cleanup-before-arrival"); err != nil {
		t.Fatalf("release-all before arrival: %v", err)
	}
	if err := registry.wait(context.Background(), "cleanup-before-arrival"); err != nil {
		t.Fatalf("late arrival did not pass pre-opened cleanup barrier: %v", err)
	}
	snapshot, _ = registry.snapshot("cleanup-before-arrival")
	if !snapshot.ReleasedAll || snapshot.Waiting != 0 || snapshot.Arrivals != 1 || snapshot.Releases != 1 {
		t.Fatalf("pre-opened cleanup barrier snapshot = %+v", snapshot)
	}
}

func TestBarrierRegistryCancellationAndTimeoutRemoveWaiters(t *testing.T) {
	registry := newBarrierRegistry(time.Second)
	ctx, cancel := context.WithCancel(context.Background())
	result := make(chan error, 1)
	go func() { result <- registry.wait(ctx, "cancel-barrier") }()
	waitForBarrier(t, registry, "cancel-barrier", 1)
	cancel()
	if err := <-result; !errors.Is(err, context.Canceled) {
		t.Fatalf("cancelled wait returned %v", err)
	}
	snapshot := waitForBarrier(t, registry, "cancel-barrier", 0)
	if snapshot.Arrivals != 1 || snapshot.Releases != 0 {
		t.Fatalf("cancelled barrier snapshot = %+v", snapshot)
	}

	timeoutRegistry := newBarrierRegistry(10 * time.Millisecond)
	err := timeoutRegistry.wait(context.Background(), "timeout-barrier")
	if status.Code(err) != codes.DeadlineExceeded {
		t.Fatalf("timed out wait returned %v", err)
	}
	snapshot = waitForBarrier(t, timeoutRegistry, "timeout-barrier", 0)
	if snapshot.Arrivals != 1 || snapshot.Releases != 0 {
		t.Fatalf("timed out barrier snapshot = %+v", snapshot)
	}
}

func TestBarrierRegistryBounds(t *testing.T) {
	registry := newBarrierRegistry(time.Second)
	for i := 0; i < maxBarriers; i++ {
		id := "barrier-" + fmtInt(i)
		registry.barriers[id] = &barrierState{id: id}
	}
	if err := registry.wait(context.Background(), "barrier-overflow"); status.Code(err) != codes.ResourceExhausted {
		t.Fatalf("barrier capacity overflow returned %v", err)
	}

	waiterRegistry := newBarrierRegistry(time.Second)
	waiters := make([]*barrierWaiter, maxBarrierWaiters)
	for i := range waiters {
		waiters[i] = &barrierWaiter{id: uint64(i + 1), release: make(chan struct{})}
	}
	waiterRegistry.barriers["waiter-overflow"] = &barrierState{id: "waiter-overflow", waiters: waiters}
	if err := waiterRegistry.wait(context.Background(), "waiter-overflow"); status.Code(err) != codes.ResourceExhausted {
		t.Fatalf("waiter capacity overflow returned %v", err)
	}
}

func TestProcessorBarrierModesRequireIDAndBlockSelectedPhase(t *testing.T) {
	missing := &fakeProcessStream{
		ctx:      context.Background(),
		requests: []*extprocv3.ProcessingRequest{requestHeaders("missing-barrier", "barrier-headers")},
	}
	err := (&processor{store: newObservationStore(), barriers: newBarrierRegistry(time.Second)}).Process(missing)
	if status.Code(err) != codes.InvalidArgument {
		t.Fatalf("missing barrier ID returned %v", err)
	}

	for _, test := range []struct {
		name       string
		barrierID  string
		requests   []*extprocv3.ProcessingRequest
		sentAtWait int
	}{
		{
			name: "headers", barrierID: "headers-barrier", sentAtWait: 0,
			requests: []*extprocv3.ProcessingRequest{requestHeadersWithBarrier("headers-case", "barrier-headers", "headers-barrier")},
		},
		{
			name: "request-body", barrierID: "body-barrier", sentAtWait: 1,
			requests: []*extprocv3.ProcessingRequest{
				requestHeadersWithBarrier("body-case", "barrier-request-body", "body-barrier"),
				requestBody("payload-one", false), requestBody("payload-two", true),
			},
		},
	} {
		t.Run(test.name, func(t *testing.T) {
			registry := newBarrierRegistry(time.Second)
			stream := &fakeProcessStream{ctx: context.Background(), requests: test.requests}
			processor := &processor{store: newObservationStore(), barriers: registry}
			result := make(chan error, 1)
			go func() { result <- processor.Process(stream) }()
			waitForBarrier(t, registry, test.barrierID, 1)
			if len(stream.sent) != test.sentAtWait {
				t.Fatalf("responses before release = %d, want %d", len(stream.sent), test.sentAtWait)
			}
			if err := registry.releaseOne(test.barrierID); err != nil {
				t.Fatal(err)
			}
			select {
			case err := <-result:
				if err != nil {
					t.Fatalf("Process() after release = %v", err)
				}
			case <-time.After(time.Second):
				t.Fatal("Process() did not complete after release")
			}
			if len(stream.sent) != len(test.requests) {
				t.Fatalf("responses after release = %d, want %d", len(stream.sent), len(test.requests))
			}
		})
	}
}

func TestAdminContract(t *testing.T) {
	store := newObservationStore()
	handler := adminHandler{store: store}

	request := httptest.NewRequest(http.MethodGet, "/readyz", nil)
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, request)
	if response.Code != http.StatusOK {
		t.Fatalf("ready status = %d", response.Code)
	}
	var ready struct {
		Status                string `json:"status"`
		Fixture               string `json:"fixture"`
		ProtocolVersion       string `json:"protocol_version"`
		GoControlPlaneEnvoy   string `json:"go_control_plane_envoy"`
		FixtureStreamCapacity int    `json:"fixture_stream_capacity"`
		TLSCapable            bool   `json:"tls_capable"`
		MTLSCapable           bool   `json:"mtls_capable"`
		TLSReady              bool   `json:"tls_ready"`
		MTLSReady             bool   `json:"mtls_ready"`
	}
	if err := json.Unmarshal(response.Body.Bytes(), &ready); err != nil {
		t.Fatal(err)
	}
	if ready.Fixture != fixtureIdentity || ready.ProtocolVersion != protocolVersion || ready.GoControlPlaneEnvoy != moduleVersion || ready.Status != "ready" || ready.FixtureStreamCapacity != maxConcurrentStreams || !ready.TLSCapable || !ready.MTLSCapable || ready.TLSReady || ready.MTLSReady {
		t.Fatalf("ready response = %v", ready)
	}

	request = httptest.NewRequest(http.MethodPost, "/v1/cases/admin-case/reset", strings.NewReader("not-empty"))
	response = httptest.NewRecorder()
	handler.ServeHTTP(response, request)
	if response.Code != http.StatusBadRequest {
		t.Fatalf("non-empty reset status = %d", response.Code)
	}

	request = httptest.NewRequest(http.MethodDelete, "/v1/cases/admin-case", nil)
	response = httptest.NewRecorder()
	handler.ServeHTTP(response, request)
	if response.Code != http.StatusMethodNotAllowed || response.Header().Get("Allow") != http.MethodGet {
		t.Fatalf("invalid method response = %d, Allow=%q", response.Code, response.Header().Get("Allow"))
	}
}

func TestBarrierAdminContract(t *testing.T) {
	registry := newBarrierRegistry(time.Second)
	handler := adminHandler{store: newObservationStore(), barriers: registry}
	result := make(chan error, 1)
	go func() { result <- registry.wait(context.Background(), "admin-barrier") }()
	waitForBarrier(t, registry, "admin-barrier", 1)

	request := httptest.NewRequest(http.MethodGet, "/v1/barriers/admin-barrier", nil)
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, request)
	if response.Code != http.StatusOK {
		t.Fatalf("barrier snapshot status = %d", response.Code)
	}
	var snapshot barrierSnapshot
	if err := json.Unmarshal(response.Body.Bytes(), &snapshot); err != nil {
		t.Fatal(err)
	}
	if snapshot.BarrierID != "admin-barrier" || snapshot.Waiting != 1 || snapshot.Arrivals != 1 {
		t.Fatalf("barrier admin snapshot = %+v", snapshot)
	}
	if snapshot.WaiterCapacity != maxBarrierWaiters || snapshot.FixtureStreamCapacity != maxConcurrentStreams {
		t.Fatalf("barrier capacity identity = %+v", snapshot)
	}

	request = httptest.NewRequest(http.MethodPost, "/v1/barriers/admin-barrier/release-one", nil)
	response = httptest.NewRecorder()
	handler.ServeHTTP(response, request)
	if response.Code != http.StatusNoContent {
		t.Fatalf("barrier release-one status = %d: %s", response.Code, response.Body.String())
	}
	if err := <-result; err != nil {
		t.Fatalf("admin-released waiter returned %v", err)
	}

	request = httptest.NewRequest(http.MethodPost, "/v1/barriers/admin-barrier/reset", nil)
	response = httptest.NewRecorder()
	handler.ServeHTTP(response, request)
	if response.Code != http.StatusNoContent {
		t.Fatalf("barrier reset status = %d: %s", response.Code, response.Body.String())
	}

	for _, test := range []struct {
		method string
		path   string
		body   string
		status int
	}{
		{http.MethodGet, "/v1/barriers/missing", "", http.StatusNotFound},
		{http.MethodPost, "/v1/barriers/bad/id/release-one", "", http.StatusBadRequest},
		{http.MethodPost, "/v1/barriers/valid/release-one", "x", http.StatusBadRequest},
		{http.MethodDelete, "/v1/barriers/valid", "", http.StatusMethodNotAllowed},
		{http.MethodGet, "/v1/barriers/valid?query=1", "", http.StatusBadRequest},
	} {
		request = httptest.NewRequest(test.method, test.path, strings.NewReader(test.body))
		response = httptest.NewRecorder()
		handler.ServeHTTP(response, request)
		if response.Code != test.status {
			t.Errorf("%s %s status = %d, want %d", test.method, test.path, response.Code, test.status)
		}
	}
}

func TestListenBothClosesFirstListenerWhenSecondBindFails(t *testing.T) {
	occupied, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer occupied.Close()
	reserved, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	grpcAddress := reserved.Addr().String()
	reserved.Close()
	if _, _, err := listenBoth(grpcAddress, occupied.Addr().String()); err == nil {
		t.Fatal("listenBoth succeeded with occupied admin address")
	}
	rebound, err := net.Listen("tcp", grpcAddress)
	if err != nil {
		t.Fatalf("first listener was not closed: %v", err)
	}
	rebound.Close()
}

func TestValidateListenAddress(t *testing.T) {
	for _, address := range []string{"0.0.0.0:9000", "localhost:9000", "127.0.0.1:0", "127.0.0.1:65536", "bad"} {
		if validateListenAddress(address) == nil {
			t.Errorf("validateListenAddress(%q) succeeded", address)
		}
	}
	if err := validateListenAddress("127.0.0.1:9000"); err != nil {
		t.Fatalf("valid loopback address rejected: %v", err)
	}
}

type testTLSFixture struct {
	materials         *tlsMaterials
	materialDirectory string
	listener          *rotatingTLSListener
	store             *observationStore
	server            *grpc.Server
}

func startTestTLSFixture(t *testing.T, mutualTLS bool) *testTLSFixture {
	t.Helper()
	materialDirectory := filepath.Join(t.TempDir(), "materials")
	materials, err := generateTLSMaterials(materialDirectory)
	if err != nil {
		t.Fatal(err)
	}
	base, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	transport := "tls"
	if mutualTLS {
		transport = "mtls"
	}
	listener := newRotatingTLSListener(base, transport, mutualTLS, materials)
	store := newObservationStore()
	processor := &processor{
		store: store, barriers: newBarrierRegistry(time.Second), slots: make(chan struct{}, maxConcurrentStreams),
	}
	server := newFixtureGRPCServer(processor, connectionStatsHandler{listener: listener})
	go func() { _ = server.Serve(listener) }()
	fixture := &testTLSFixture{
		materials: materials, materialDirectory: materialDirectory, listener: listener, store: store, server: server,
	}
	t.Cleanup(func() {
		server.Stop()
		_ = listener.Close()
	})
	return fixture
}

func serverRootPool(t *testing.T, materialDirectory string) *x509.CertPool {
	t.Helper()
	serverCAPEM, err := os.ReadFile(filepath.Join(materialDirectory, "server-ca.crt"))
	if err != nil {
		t.Fatal(err)
	}
	pool := x509.NewCertPool()
	if !pool.AppendCertsFromPEM(serverCAPEM) {
		t.Fatal("server CA PEM did not parse")
	}
	return pool
}

func dialFixture(t *testing.T, address string, roots *x509.CertPool, client *leafMaterial) *grpc.ClientConn {
	t.Helper()
	config := &tls.Config{MinVersion: tls.VersionTLS12, RootCAs: roots}
	if client != nil {
		config.Certificates = []tls.Certificate{client.certificate}
	}
	connection, err := grpc.NewClient(address, grpc.WithTransportCredentials(credentials.NewTLS(config)))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = connection.Close() })
	return connection
}

func processOnce(ctx context.Context, connection *grpc.ClientConn, caseID string) error {
	client := extprocv3.NewExternalProcessorClient(connection)
	stream, err := client.Process(ctx)
	if err != nil {
		return err
	}
	if err := stream.Send(requestHeaders(caseID, "interop")); err != nil {
		return err
	}
	if err := stream.CloseSend(); err != nil {
		return err
	}
	response, err := stream.Recv()
	if err != nil {
		return err
	}
	if response.GetRequestHeaders() == nil {
		return errors.New("processor returned a non-header verdict")
	}
	_, err = stream.Recv()
	if errors.Is(err, io.EOF) {
		return nil
	}
	return err
}

func terminalStream(t *testing.T, store *observationStore, caseID string) streamObservation {
	t.Helper()
	deadline := time.Now().Add(time.Second)
	for {
		if snapshot, ok := store.snapshot(caseID); ok && len(snapshot.Streams) == 1 && !snapshot.Streams[0].Active {
			return snapshot.Streams[0]
		}
		if time.Now().After(deadline) {
			t.Fatalf("case %q did not become terminal", caseID)
		}
		time.Sleep(time.Millisecond)
	}
}

func TestTLSMaterialPermissionsAndFingerprints(t *testing.T) {
	directory := filepath.Join(t.TempDir(), "materials")
	materials, err := generateTLSMaterials(directory)
	if err != nil {
		t.Fatal(err)
	}
	info, err := os.Stat(directory)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm() != 0o700 {
		t.Fatalf("material directory mode = %#o", info.Mode().Perm())
	}
	entries, err := os.ReadDir(directory)
	if err != nil {
		t.Fatal(err)
	}
	if len(entries) != 13 {
		t.Fatalf("material file count = %d, want 13", len(entries))
	}
	for _, entry := range entries {
		info, err := entry.Info()
		if err != nil {
			t.Fatal(err)
		}
		if info.Mode().Perm() != 0o600 {
			t.Errorf("%s mode = %#o", entry.Name(), info.Mode().Perm())
		}
	}
	if materials.servers["a"].fingerprint == materials.servers["b"].fingerprint {
		t.Fatal("server A/B fingerprints are equal")
	}
	if materials.clients["a"].fingerprint == materials.clients["b"].fingerprint {
		t.Fatal("client A/B fingerprints are equal")
	}
	for _, name := range []string{"ext-proc-external-client-initial.yaml", "ext-proc-external-client-updated.yaml"} {
		content, err := os.ReadFile(filepath.Join(directory, name))
		if err != nil {
			t.Fatal(err)
		}
		text := string(content)
		if !strings.Contains(text, "name: ext-proc-external-client") || !strings.Contains(text, "namespace: edgion-default") {
			t.Fatalf("%s lacks fixed Secret identity", name)
		}
		if strings.Contains(text, "BEGIN") {
			t.Fatalf("%s contains unencoded PEM", name)
		}
	}
	serverCA, err := os.ReadFile(filepath.Join(directory, "server-ca.crt"))
	if err != nil {
		t.Fatal(err)
	}
	serverCASecret, err := os.ReadFile(filepath.Join(directory, "ext-proc-external-server-ca.yaml"))
	if err != nil {
		t.Fatal(err)
	}
	secretText := string(serverCASecret)
	if !strings.Contains(secretText, "apiVersion: v1") ||
		!strings.Contains(secretText, "kind: Secret") ||
		!strings.Contains(secretText, "name: ext-proc-external-server-ca") ||
		!strings.Contains(secretText, "namespace: edgion-default") ||
		!strings.Contains(secretText, "type: Opaque") {
		t.Fatal("server CA Secret lacks its fixed API identity")
	}
	if strings.Contains(secretText, "BEGIN") || strings.Contains(secretText, "PRIVATE KEY") ||
		strings.Contains(secretText, "tls.key:") || strings.Contains(secretText, "tls.crt:") {
		t.Fatal("server CA Secret contains unencoded or private material")
	}
	const caPrefix = "  ca.crt: "
	var encodedCA string
	for _, line := range strings.Split(secretText, "\n") {
		if strings.HasPrefix(line, caPrefix) {
			encodedCA = strings.TrimPrefix(line, caPrefix)
			break
		}
	}
	decodedCA, err := base64.StdEncoding.DecodeString(encodedCA)
	if err != nil {
		t.Fatalf("decode server CA Secret: %v", err)
	}
	if !slices.Equal(decodedCA, serverCA) {
		t.Fatal("server CA Secret data.ca.crt differs from server-ca.crt")
	}
}

func TestTLSMaterialDirectoryMustBeAbsolute(t *testing.T) {
	if _, err := generateTLSMaterials(filepath.Join("relative", "materials")); err == nil ||
		!strings.Contains(err.Error(), "absolute path") {
		t.Fatalf("relative TLS material directory returned %v", err)
	}
}

func TestTLSProcessUsesExactAcceptedServerLeaf(t *testing.T) {
	fixture := startTestTLSFixture(t, false)
	roots := serverRootPool(t, fixture.materialDirectory)
	connection := dialFixture(t, fixture.listener.Addr().String(), roots, nil)
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	if err := processOnce(ctx, connection, "tls-exact-leaf"); err != nil {
		t.Fatal(err)
	}
	stream := terminalStream(t, fixture.store, "tls-exact-leaf")
	if stream.Transport != "tls" || stream.TLSGeneration != "a" || stream.ServerLeafSHA256 != fixture.materials.servers["a"].fingerprint || stream.PeerLeafSHA256 != "" {
		t.Fatalf("TLS stream identity = %+v", stream)
	}
}

func TestMTLSAttributesExactClientAAndBLeafs(t *testing.T) {
	fixture := startTestTLSFixture(t, true)
	roots := serverRootPool(t, fixture.materialDirectory)
	for _, generation := range []string{"a", "b"} {
		connection := dialFixture(t, fixture.listener.Addr().String(), roots, fixture.materials.clients[generation])
		caseID := "mtls-client-" + generation
		ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
		if err := processOnce(ctx, connection, caseID); err != nil {
			cancel()
			t.Fatal(err)
		}
		cancel()
		stream := terminalStream(t, fixture.store, caseID)
		if stream.Transport != "mtls" || stream.ServerLeafSHA256 != fixture.materials.servers["a"].fingerprint || stream.PeerLeafSHA256 != fixture.materials.clients[generation].fingerprint {
			t.Fatalf("mTLS %s stream identity = %+v", generation, stream)
		}
	}
}

func TestTLSRejectsUntrustedServerAndMTLSRejectsMissingOrUntrustedClient(t *testing.T) {
	tlsFixture := startTestTLSFixture(t, false)
	otherMaterialsDirectory := filepath.Join(t.TempDir(), "other-materials")
	otherMaterials, err := generateTLSMaterials(otherMaterialsDirectory)
	if err != nil {
		t.Fatal(err)
	}
	wrongRoots := serverRootPool(t, otherMaterialsDirectory)
	wrongServerConnection := dialFixture(t, tlsFixture.listener.Addr().String(), wrongRoots, nil)
	ctx, cancel := context.WithTimeout(context.Background(), 500*time.Millisecond)
	if err := processOnce(ctx, wrongServerConnection, "untrusted-server"); err == nil {
		cancel()
		t.Fatal("TLS connection trusted an unrelated server CA")
	}
	cancel()

	mtlsFixture := startTestTLSFixture(t, true)
	roots := serverRootPool(t, mtlsFixture.materialDirectory)
	missingClient := dialFixture(t, mtlsFixture.listener.Addr().String(), roots, nil)
	ctx, cancel = context.WithTimeout(context.Background(), 500*time.Millisecond)
	if err := processOnce(ctx, missingClient, "missing-client"); err == nil {
		cancel()
		t.Fatal("mTLS connection accepted no client certificate")
	}
	cancel()

	untrustedClient := dialFixture(t, mtlsFixture.listener.Addr().String(), roots, otherMaterials.clients["a"])
	ctx, cancel = context.WithTimeout(context.Background(), 500*time.Millisecond)
	if err := processOnce(ctx, untrustedClient, "untrusted-client"); err == nil {
		cancel()
		t.Fatal("mTLS connection accepted an unrelated client CA")
	}
	cancel()
}

func TestTLSServerRotationDrainsOldGenerationAndAttributesAtoBtoA(t *testing.T) {
	fixture := startTestTLSFixture(t, false)
	roots := serverRootPool(t, fixture.materialDirectory)
	connectionA := dialFixture(t, fixture.listener.Addr().String(), roots, nil)
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	if err := processOnce(ctx, connectionA, "rotate-a-first"); err != nil {
		cancel()
		t.Fatal(err)
	}
	cancel()
	first := terminalStream(t, fixture.store, "rotate-a-first")
	if first.ServerLeafSHA256 != fixture.materials.servers["a"].fingerprint {
		t.Fatalf("initial leaf = %s", first.ServerLeafSHA256)
	}
	if snapshot := listenerSnapshot(fixture.listener, fixture.materials); snapshot.ActiveConnections != 1 {
		t.Fatalf("active connections before idempotent rotation = %d", snapshot.ActiveConnections)
	}
	if err := fixture.listener.rotate("a"); err != nil {
		t.Fatal(err)
	}
	ctx, cancel = context.WithTimeout(context.Background(), 2*time.Second)
	if err := processOnce(ctx, connectionA, "rotate-a-idempotent"); err != nil {
		cancel()
		t.Fatal(err)
	}
	cancel()
	if snapshot := listenerSnapshot(fixture.listener, fixture.materials); snapshot.ActiveConnections != 1 {
		t.Fatalf("idempotent rotation drained current connection: %d", snapshot.ActiveConnections)
	}

	if err := fixture.listener.rotate("b"); err != nil {
		t.Fatal(err)
	}
	if snapshot := listenerSnapshot(fixture.listener, fixture.materials); snapshot.ActiveConnections != 0 || snapshot.CurrentGeneration != "b" {
		t.Fatalf("A->B rotation snapshot = %+v", snapshot)
	}
	connectionB := dialFixture(t, fixture.listener.Addr().String(), roots, nil)
	ctx, cancel = context.WithTimeout(context.Background(), 2*time.Second)
	if err := processOnce(ctx, connectionB, "rotate-b"); err != nil {
		cancel()
		t.Fatal(err)
	}
	cancel()
	second := terminalStream(t, fixture.store, "rotate-b")
	if second.ServerLeafSHA256 != fixture.materials.servers["b"].fingerprint || second.ServerLeafSHA256 == first.ServerLeafSHA256 {
		t.Fatalf("rotated B leaf = %+v", second)
	}

	if err := fixture.listener.rotate("a"); err != nil {
		t.Fatal(err)
	}
	if snapshot := listenerSnapshot(fixture.listener, fixture.materials); snapshot.ActiveConnections != 0 || snapshot.CurrentGeneration != "a" {
		t.Fatalf("B->A rotation snapshot = %+v", snapshot)
	}
	connectionA2 := dialFixture(t, fixture.listener.Addr().String(), roots, nil)
	ctx, cancel = context.WithTimeout(context.Background(), 2*time.Second)
	if err := processOnce(ctx, connectionA2, "rotate-a-return"); err != nil {
		cancel()
		t.Fatal(err)
	}
	cancel()
	third := terminalStream(t, fixture.store, "rotate-a-return")
	if third.ServerLeafSHA256 != first.ServerLeafSHA256 || third.TLSGeneration != "a" {
		t.Fatalf("returned A leaf = %+v", third)
	}
}

func TestTLSAdminIdentityAndStrictRotationContract(t *testing.T) {
	materials, err := generateTLSMaterials(filepath.Join(t.TempDir(), "materials"))
	if err != nil {
		t.Fatal(err)
	}
	base, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	listener := newRotatingTLSListener(base, "tls", false, materials)
	defer listener.Close()
	manager := &tlsTransportManager{materials: materials, tls: listener}
	handler := adminHandler{store: newObservationStore(), barriers: newBarrierRegistry(time.Second), tls: manager}

	request := httptest.NewRequest(http.MethodGet, "/v1/tls", nil)
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, request)
	if response.Code != http.StatusOK {
		t.Fatalf("TLS identity status = %d", response.Code)
	}
	body := response.Body.String()
	if strings.Contains(body, "BEGIN") || strings.Contains(body, "PRIVATE KEY") {
		t.Fatal("TLS identity response leaked PEM material")
	}
	var identity tlsIdentitySnapshot
	if err := json.Unmarshal([]byte(body), &identity); err != nil {
		t.Fatal(err)
	}
	if !identity.TLS.Ready || identity.TLS.CurrentGeneration != "a" || identity.FixtureStreamCapacity != 96 || identity.MTLS.Enabled {
		t.Fatalf("TLS identity = %+v", identity)
	}
	if identity.TLS.GenerationALeafSHA256 != materials.servers["a"].fingerprint || identity.TLS.GenerationBLeafSHA256 != materials.servers["b"].fingerprint || identity.ClientALeafSHA256 != materials.clients["a"].fingerprint || identity.ClientBLeafSHA256 != materials.clients["b"].fingerprint {
		t.Fatalf("TLS public fingerprints = %+v", identity)
	}

	request = httptest.NewRequest(http.MethodGet, "/readyz", nil)
	response = httptest.NewRecorder()
	handler.ServeHTTP(response, request)
	var ready map[string]any
	if response.Code != http.StatusOK || json.Unmarshal(response.Body.Bytes(), &ready) != nil || ready["tls_ready"] != true || ready["mtls_ready"] != false {
		t.Fatalf("secure listener readiness = %d %s", response.Code, response.Body.String())
	}

	request = httptest.NewRequest(http.MethodPost, "/v1/tls/tls/rotate/b", nil)
	response = httptest.NewRecorder()
	handler.ServeHTTP(response, request)
	if response.Code != http.StatusNoContent || listenerSnapshot(listener, materials).CurrentGeneration != "b" {
		t.Fatalf("TLS rotation response = %d, snapshot=%+v", response.Code, listenerSnapshot(listener, materials))
	}

	for _, test := range []struct {
		method string
		path   string
		body   string
		status int
	}{
		{http.MethodGet, "/v1/tls/tls/rotate/a", "", http.StatusMethodNotAllowed},
		{http.MethodPost, "/v1/tls/tls/rotate/c", "", http.StatusNotFound},
		{http.MethodPost, "/v1/tls/mtls/rotate/a", "", http.StatusNotFound},
		{http.MethodPost, "/v1/tls/tls/rotate/a", "pem", http.StatusBadRequest},
		{http.MethodGet, "/v1/tls?query=1", "", http.StatusBadRequest},
	} {
		request = httptest.NewRequest(test.method, test.path, strings.NewReader(test.body))
		response = httptest.NewRecorder()
		handler.ServeHTTP(response, request)
		if response.Code != test.status {
			t.Errorf("%s %s status = %d, want %d", test.method, test.path, response.Code, test.status)
		}
	}
}

func TestPinnedEnvoyModuleMatchesBuildInformation(t *testing.T) {
	if err := verifyEnvoyModuleVersion(); err != nil {
		t.Fatal(err)
	}
}
