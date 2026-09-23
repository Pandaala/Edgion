// Command ext-proc-external is an independent Envoy ExtProc test fixture.
package main

import (
	"bytes"
	"context"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/sha256"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"encoding/pem"
	"errors"
	"flag"
	"fmt"
	"hash"
	"io"
	"log"
	"math/big"
	"net"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"regexp"
	"runtime/debug"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"syscall"
	"time"

	corev3 "github.com/envoyproxy/go-control-plane/envoy/config/core/v3"
	extprocmodev3 "github.com/envoyproxy/go-control-plane/envoy/extensions/filters/http/ext_proc/v3"
	extprocv3 "github.com/envoyproxy/go-control-plane/envoy/service/ext_proc/v3"
	typev3 "github.com/envoyproxy/go-control-plane/envoy/type/v3"
	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/stats"
	"google.golang.org/grpc/status"
)

const (
	fixtureIdentity = "edgion-ext-proc-external"
	protocolVersion = "v1"
	moduleVersion   = "v1.37.0"

	caseIDHeader  = "x-ext-proc-case-id"
	modeHeader    = "x-ext-proc-test-mode"
	barrierHeader = "x-ext-proc-barrier-id"

	maxCases                      = 128
	maxStreamsPerCase             = 16
	maxPhaseTransitionsPerStream  = 16
	maxBodyMessagesPerDirection   = 8_192
	maxObservedBodyBytes          = 32 << 20
	maxHeaderMessagesPerDirection = 64
	maxBarriers                   = 128
	maxBarrierWaiters             = 96
	maxConcurrentStreams          = 96
	maxGRPCMessageBytes           = 1 << 20
	maxAdminBodyBytes             = 1024
	longIdleDelay                 = 3 * time.Second
	barrierWaitTimeout            = 25 * time.Second
	maxTrackedTLSConnections      = 256
	tlsHandshakeTimeout           = 5 * time.Second
)

var caseIDPattern = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$`)

func verifyEnvoyModuleVersion() error {
	info, ok := debug.ReadBuildInfo()
	if !ok {
		return errors.New("Go build information is unavailable")
	}
	const modulePath = "github.com/envoyproxy/go-control-plane/envoy"
	for _, dependency := range info.Deps {
		if dependency.Path != modulePath {
			continue
		}
		if dependency.Replace != nil {
			dependency = dependency.Replace
		}
		if dependency.Version != moduleVersion {
			return fmt.Errorf("%s version is %q, want %q", modulePath, dependency.Version, moduleVersion)
		}
		return nil
	}
	return fmt.Errorf("%s is missing from Go build information", modulePath)
}

type leafMaterial struct {
	certificate tls.Certificate
	certPEM     []byte
	keyPEM      []byte
	fingerprint string
}

type tlsMaterials struct {
	serverCAFingerprint string
	clientCAFingerprint string
	clientCAPool        *x509.CertPool
	servers             map[string]*leafMaterial
	clients             map[string]*leafMaterial
}

func certificateFingerprint(raw []byte) string {
	sum := sha256.Sum256(raw)
	return hex.EncodeToString(sum[:])
}

func randomSerial() (*big.Int, error) {
	limit := new(big.Int).Lsh(big.NewInt(1), 128)
	return rand.Int(rand.Reader, limit)
}

func generateCA(commonName string, now time.Time) (*x509.Certificate, *ecdsa.PrivateKey, []byte, error) {
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		return nil, nil, nil, err
	}
	serial, err := randomSerial()
	if err != nil {
		return nil, nil, nil, err
	}
	template := &x509.Certificate{
		SerialNumber: serial,
		Subject:      pkix.Name{CommonName: commonName},
		NotBefore:    now.Add(-time.Hour), NotAfter: now.Add(24 * time.Hour),
		IsCA: true, BasicConstraintsValid: true,
		KeyUsage: x509.KeyUsageCertSign | x509.KeyUsageCRLSign | x509.KeyUsageDigitalSignature,
	}
	raw, err := x509.CreateCertificate(rand.Reader, template, template, &key.PublicKey, key)
	if err != nil {
		return nil, nil, nil, err
	}
	certificate, err := x509.ParseCertificate(raw)
	if err != nil {
		return nil, nil, nil, err
	}
	return certificate, key, pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: raw}), nil
}

func generateLeaf(
	commonName string,
	usage x509.ExtKeyUsage,
	ca *x509.Certificate,
	caKey *ecdsa.PrivateKey,
	now time.Time,
	serverDNSName string,
) (*leafMaterial, error) {
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		return nil, err
	}
	serial, err := randomSerial()
	if err != nil {
		return nil, err
	}
	template := &x509.Certificate{
		SerialNumber: serial,
		Subject:      pkix.Name{CommonName: commonName},
		NotBefore:    now.Add(-time.Hour),
		NotAfter:     now.Add(12 * time.Hour),
		KeyUsage:     x509.KeyUsageDigitalSignature,
		ExtKeyUsage:  []x509.ExtKeyUsage{usage},
	}
	if usage == x509.ExtKeyUsageServerAuth {
		template.IPAddresses = []net.IP{net.ParseIP("127.0.0.1")}
		if serverDNSName != "" {
			template.DNSNames = []string{serverDNSName}
		}
	}
	raw, err := x509.CreateCertificate(rand.Reader, template, ca, &key.PublicKey, caKey)
	if err != nil {
		return nil, err
	}
	keyRaw, err := x509.MarshalPKCS8PrivateKey(key)
	if err != nil {
		return nil, err
	}
	certPEM := pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: raw})
	keyPEM := pem.EncodeToMemory(&pem.Block{Type: "PRIVATE KEY", Bytes: keyRaw})
	pair, err := tls.X509KeyPair(certPEM, keyPEM)
	if err != nil {
		return nil, err
	}
	pair.Leaf, err = x509.ParseCertificate(raw)
	if err != nil {
		return nil, err
	}
	return &leafMaterial{certificate: pair, certPEM: certPEM, keyPEM: keyPEM, fingerprint: certificateFingerprint(raw)}, nil
}

func atomicWritePrivate(path string, content []byte) error {
	directory := filepath.Dir(path)
	file, err := os.CreateTemp(directory, ".ext-proc-material-*")
	if err != nil {
		return err
	}
	temporary := file.Name()
	keep := false
	defer func() {
		_ = file.Close()
		if !keep {
			_ = os.Remove(temporary)
		}
	}()
	if err := file.Chmod(0o600); err != nil {
		return err
	}
	if _, err := file.Write(content); err != nil {
		return err
	}
	if err := file.Sync(); err != nil {
		return err
	}
	if err := file.Close(); err != nil {
		return err
	}
	if err := os.Rename(temporary, path); err != nil {
		return err
	}
	if err := os.Chmod(path, 0o600); err != nil {
		return err
	}
	keep = true
	return nil
}

func secretYAML(leaf *leafMaterial) []byte {
	return []byte(fmt.Sprintf(`apiVersion: v1
kind: Secret
metadata:
  name: ext-proc-external-client
  namespace: edgion-default
type: kubernetes.io/tls
data:
  tls.crt: %s
  tls.key: %s
`, base64.StdEncoding.EncodeToString(leaf.certPEM), base64.StdEncoding.EncodeToString(leaf.keyPEM)))
}

func serverCASecretYAML(certificatePEM []byte) []byte {
	return []byte(fmt.Sprintf(`apiVersion: v1
kind: Secret
metadata:
  name: ext-proc-external-server-ca
  namespace: edgion-default
type: Opaque
data:
  ca.crt: %s
`, base64.StdEncoding.EncodeToString(certificatePEM)))
}

func generateTLSMaterials(directory string) (*tlsMaterials, error) {
	return generateTLSMaterialsWithDNS(directory, "")
}

func generateTLSMaterialsWithDNS(directory, serverDNSName string) (*tlsMaterials, error) {
	if directory == "" {
		return nil, errors.New("TLS material directory is required")
	}
	if !filepath.IsAbs(directory) {
		return nil, errors.New("TLS material directory must be an absolute path")
	}
	if err := os.MkdirAll(directory, 0o700); err != nil {
		return nil, fmt.Errorf("create TLS material directory: %w", err)
	}
	info, err := os.Lstat(directory)
	if err != nil {
		return nil, fmt.Errorf("inspect TLS material directory: %w", err)
	}
	if !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
		return nil, errors.New("TLS material path must be a directory, not a symlink")
	}
	if err := os.Chmod(directory, 0o700); err != nil {
		return nil, fmt.Errorf("secure TLS material directory: %w", err)
	}
	now := time.Now().UTC()
	serverCA, serverCAKey, serverCAPEM, err := generateCA("Edgion external ExtProc server CA", now)
	if err != nil {
		return nil, fmt.Errorf("generate server CA: %w", err)
	}
	clientCA, clientCAKey, clientCAPEM, err := generateCA("Edgion external ExtProc client CA", now)
	if err != nil {
		return nil, fmt.Errorf("generate client CA: %w", err)
	}
	materials := &tlsMaterials{
		serverCAFingerprint: certificateFingerprint(serverCA.Raw),
		clientCAFingerprint: certificateFingerprint(clientCA.Raw),
		clientCAPool:        x509.NewCertPool(),
		servers:             make(map[string]*leafMaterial, 2),
		clients:             make(map[string]*leafMaterial, 2),
	}
	materials.clientCAPool.AddCert(clientCA)
	for _, generation := range []string{"a", "b"} {
		materials.servers[generation], err = generateLeaf(
			"Edgion external ExtProc server "+strings.ToUpper(generation), x509.ExtKeyUsageServerAuth,
			serverCA, serverCAKey, now, serverDNSName,
		)
		if err != nil {
			return nil, fmt.Errorf("generate server leaf %s: %w", generation, err)
		}
		materials.clients[generation], err = generateLeaf(
			"Edgion external ExtProc client "+strings.ToUpper(generation), x509.ExtKeyUsageClientAuth,
			clientCA, clientCAKey, now, "",
		)
		if err != nil {
			return nil, fmt.Errorf("generate client leaf %s: %w", generation, err)
		}
	}
	files := map[string][]byte{
		"server-ca.crt":                         serverCAPEM,
		"client-ca.crt":                         clientCAPEM,
		"server-a.crt":                          materials.servers["a"].certPEM,
		"server-a.key":                          materials.servers["a"].keyPEM,
		"server-b.crt":                          materials.servers["b"].certPEM,
		"server-b.key":                          materials.servers["b"].keyPEM,
		"client-a.crt":                          materials.clients["a"].certPEM,
		"client-a.key":                          materials.clients["a"].keyPEM,
		"client-b.crt":                          materials.clients["b"].certPEM,
		"client-b.key":                          materials.clients["b"].keyPEM,
		"ext-proc-external-client-initial.yaml": secretYAML(materials.clients["a"]),
		"ext-proc-external-client-updated.yaml": secretYAML(materials.clients["b"]),
		"ext-proc-external-server-ca.yaml":      serverCASecretYAML(serverCAPEM),
	}
	for name, content := range files {
		if err := atomicWritePrivate(filepath.Join(directory, name), content); err != nil {
			return nil, fmt.Errorf("write TLS material %s: %w", name, err)
		}
	}
	return materials, nil
}

type connectionIdentity struct {
	transport             string
	generation            string
	serverLeafFingerprint string
	peerLeafFingerprint   string
}

type connectionIdentityContextKey struct{}

type trackedTLSConn struct {
	net.Conn
	id       uint64
	remote   string
	listener *rotatingTLSListener
	once     sync.Once
}

func (c *trackedTLSConn) Close() error {
	var err error
	c.once.Do(func() {
		err = c.Conn.Close()
		c.listener.unregister(c.id, c.remote)
	})
	return err
}

type trackedConnection struct {
	conn     *trackedTLSConn
	identity connectionIdentity
}

type rotatingTLSListener struct {
	base      net.Listener
	transport string
	mutualTLS bool
	materials *tlsMaterials

	mu       sync.Mutex
	current  string
	active   map[uint64]*trackedConnection
	byRemote map[string]connectionIdentity
	nextID   atomic.Uint64
	accepted atomic.Uint64
	rpcCalls atomic.Uint64
}

func newRotatingTLSListener(base net.Listener, transport string, mutualTLS bool, materials *tlsMaterials) *rotatingTLSListener {
	return &rotatingTLSListener{
		base: base, transport: transport, mutualTLS: mutualTLS, materials: materials,
		current: "a", active: make(map[uint64]*trackedConnection), byRemote: make(map[string]connectionIdentity),
	}
}

func (l *rotatingTLSListener) Addr() net.Addr { return l.base.Addr() }

func (l *rotatingTLSListener) Close() error {
	err := l.base.Close()
	l.mu.Lock()
	connections := make([]*trackedTLSConn, 0, len(l.active))
	for _, tracked := range l.active {
		connections = append(connections, tracked.conn)
	}
	l.mu.Unlock()
	for _, connection := range connections {
		_ = connection.Close()
	}
	return err
}

func (l *rotatingTLSListener) Accept() (net.Conn, error) {
	for {
		raw, err := l.base.Accept()
		if err != nil {
			return nil, err
		}
		// Count before the handshake, including rejected or short-lived dials.
		l.accepted.Add(1)
		l.mu.Lock()
		generation := l.current
		leaf := l.materials.servers[generation]
		l.mu.Unlock()
		config := &tls.Config{
			MinVersion:   tls.VersionTLS12,
			NextProtos:   []string{"h2"},
			Certificates: []tls.Certificate{leaf.certificate},
		}
		if l.mutualTLS {
			config.ClientAuth = tls.RequireAndVerifyClientCert
			config.ClientCAs = l.materials.clientCAPool
		}
		secure := tls.Server(raw, config)
		_ = raw.SetDeadline(time.Now().Add(tlsHandshakeTimeout))
		if err := secure.Handshake(); err != nil {
			_ = raw.Close()
			continue
		}
		_ = raw.SetDeadline(time.Time{})
		identity := connectionIdentity{
			transport: l.transport, generation: generation, serverLeafFingerprint: leaf.fingerprint,
		}
		state := secure.ConnectionState()
		if len(state.PeerCertificates) != 0 {
			identity.peerLeafFingerprint = certificateFingerprint(state.PeerCertificates[0].Raw)
		}
		id := l.nextID.Add(1)
		remote := secure.RemoteAddr().String()
		tracked := &trackedTLSConn{Conn: secure, id: id, remote: remote, listener: l}
		l.mu.Lock()
		if generation != l.current {
			l.mu.Unlock()
			_ = tracked.Close()
			continue
		}
		if len(l.active) >= maxTrackedTLSConnections {
			l.mu.Unlock()
			_ = tracked.Close()
			continue
		}
		l.active[id] = &trackedConnection{conn: tracked, identity: identity}
		l.byRemote[remote] = identity
		l.mu.Unlock()
		return tracked, nil
	}
}

func (l *rotatingTLSListener) unregister(id uint64, remote string) {
	l.mu.Lock()
	defer l.mu.Unlock()
	if tracked, ok := l.active[id]; ok {
		delete(l.active, id)
		if current, exists := l.byRemote[remote]; exists && current == tracked.identity {
			delete(l.byRemote, remote)
		}
	}
}

func (l *rotatingTLSListener) identityForRemote(remote string) (connectionIdentity, bool) {
	l.mu.Lock()
	defer l.mu.Unlock()
	identity, ok := l.byRemote[remote]
	return identity, ok
}

func (l *rotatingTLSListener) rotate(generation string) error {
	if generation != "a" && generation != "b" {
		return errors.New("TLS generation must be a or b")
	}
	l.mu.Lock()
	if l.current == generation {
		l.mu.Unlock()
		return nil
	}
	l.current = generation
	connections := make([]*trackedTLSConn, 0, len(l.active))
	for _, tracked := range l.active {
		if tracked.identity.generation != generation {
			connections = append(connections, tracked.conn)
		}
	}
	l.mu.Unlock()
	for _, connection := range connections {
		_ = connection.Close()
	}
	return nil
}

type connectionStatsHandler struct{ listener *rotatingTLSListener }

func (h connectionStatsHandler) TagRPC(ctx context.Context, _ *stats.RPCTagInfo) context.Context {
	return ctx
}
func (h connectionStatsHandler) HandleRPC(_ context.Context, event stats.RPCStats) {
	if _, ok := event.(*stats.Begin); ok {
		h.listener.rpcCalls.Add(1)
	}
}
func (h connectionStatsHandler) HandleConn(context.Context, stats.ConnStats) {}
func (h connectionStatsHandler) TagConn(ctx context.Context, info *stats.ConnTagInfo) context.Context {
	identity, ok := h.listener.identityForRemote(info.RemoteAddr.String())
	if !ok {
		return ctx
	}
	return context.WithValue(ctx, connectionIdentityContextKey{}, identity)
}

type tlsListenerSnapshot struct {
	Enabled               bool   `json:"enabled"`
	Ready                 bool   `json:"ready"`
	CurrentGeneration     string `json:"current_generation,omitempty"`
	CurrentLeafSHA256     string `json:"current_leaf_sha256,omitempty"`
	GenerationALeafSHA256 string `json:"generation_a_leaf_sha256,omitempty"`
	GenerationBLeafSHA256 string `json:"generation_b_leaf_sha256,omitempty"`
	ActiveConnections     int    `json:"active_connections"`
	AcceptedConnections   uint64 `json:"accepted_connections"`
	RPCCalls              uint64 `json:"rpc_calls"`
}

type tlsIdentitySnapshot struct {
	ServerCASHA256        string              `json:"server_ca_sha256"`
	ClientCASHA256        string              `json:"client_ca_sha256"`
	ClientALeafSHA256     string              `json:"client_a_leaf_sha256"`
	ClientBLeafSHA256     string              `json:"client_b_leaf_sha256"`
	FixtureStreamCapacity int                 `json:"fixture_stream_capacity"`
	TLS                   tlsListenerSnapshot `json:"tls"`
	MTLS                  tlsListenerSnapshot `json:"mtls"`
}

type tlsTransportManager struct {
	materials *tlsMaterials
	tls       *rotatingTLSListener
	mtls      *rotatingTLSListener
}

func (m *tlsTransportManager) listener(name string) (*rotatingTLSListener, bool) {
	if m == nil {
		return nil, false
	}
	switch name {
	case "tls":
		return m.tls, m.tls != nil
	case "mtls":
		return m.mtls, m.mtls != nil
	default:
		return nil, false
	}
}

func listenerSnapshot(listener *rotatingTLSListener, materials *tlsMaterials) tlsListenerSnapshot {
	if listener == nil {
		return tlsListenerSnapshot{}
	}
	listener.mu.Lock()
	defer listener.mu.Unlock()
	current := listener.current
	return tlsListenerSnapshot{
		Enabled: true, Ready: true, CurrentGeneration: current,
		CurrentLeafSHA256:     materials.servers[current].fingerprint,
		GenerationALeafSHA256: materials.servers["a"].fingerprint,
		GenerationBLeafSHA256: materials.servers["b"].fingerprint,
		ActiveConnections:     len(listener.active),
		AcceptedConnections:   listener.accepted.Load(),
		RPCCalls:              listener.rpcCalls.Load(),
	}
}

func (m *tlsTransportManager) snapshot() tlsIdentitySnapshot {
	if m == nil || m.materials == nil {
		return tlsIdentitySnapshot{FixtureStreamCapacity: maxConcurrentStreams}
	}
	return tlsIdentitySnapshot{
		ServerCASHA256:        m.materials.serverCAFingerprint,
		ClientCASHA256:        m.materials.clientCAFingerprint,
		ClientALeafSHA256:     m.materials.clients["a"].fingerprint,
		ClientBLeafSHA256:     m.materials.clients["b"].fingerprint,
		FixtureStreamCapacity: maxConcurrentStreams,
		TLS:                   listenerSnapshot(m.tls, m.materials), MTLS: listenerSnapshot(m.mtls, m.materials),
	}
}

type bodyObservation struct {
	messages int
	bytes    int
	hash     hash.Hash
	eos      int
}

func (o *bodyObservation) add(body []byte, eos bool) error {
	if o.messages >= maxBodyMessagesPerDirection {
		return errors.New("body message limit exceeded")
	}
	if len(body) > maxObservedBodyBytes-o.bytes {
		return errors.New("observed body byte limit exceeded")
	}
	o.messages++
	o.bytes += len(body)
	if o.hash == nil {
		o.hash = sha256.New()
	}
	_, _ = o.hash.Write(body)
	if eos {
		o.eos++
	}
	return nil
}

func (o *bodyObservation) digest() string {
	if o.hash == nil {
		sum := sha256.Sum256(nil)
		return hex.EncodeToString(sum[:])
	}
	return hex.EncodeToString(o.hash.Sum(nil))
}

type streamObservation struct {
	StreamID         uint64   `json:"stream_id"`
	Phases           []string `json:"phases"`
	Transport        string   `json:"transport"`
	TLSGeneration    string   `json:"tls_generation,omitempty"`
	ServerLeafSHA256 string   `json:"server_leaf_sha256,omitempty"`
	PeerLeafSHA256   string   `json:"peer_leaf_sha256,omitempty"`

	RequestHeaderMessages   int      `json:"request_header_messages"`
	ResponseHeaderMessages  int      `json:"response_header_messages"`
	ResponseStatuses        []uint16 `json:"response_statuses"`
	RequestOpaqueHeadersOK  bool     `json:"request_opaque_headers_ok"`
	ResponseOpaqueHeadersOK bool     `json:"response_opaque_headers_ok"`

	RequestBodyMessages int    `json:"request_body_messages"`
	RequestBodyBytes    int    `json:"request_body_bytes"`
	RequestBodySHA256   string `json:"request_body_sha256"`
	RequestEOSCount     int    `json:"request_eos_count"`

	ResponseBodyMessages int    `json:"response_body_messages"`
	ResponseBodyBytes    int    `json:"response_body_bytes"`
	ResponseBodySHA256   string `json:"response_body_sha256"`
	ResponseEOSCount     int    `json:"response_eos_count"`

	ResponseTrailerMessages int  `json:"response_trailer_messages"`
	ResponseTrailerFields   int  `json:"response_trailer_fields"`
	SawFixedResponseTrailer bool `json:"saw_fixed_response_trailer"`
	SawGRPCStatusTrailer    bool `json:"saw_grpc_status_trailer"`

	ClientHalfClosed bool `json:"client_half_closed"`
	Cancelled        bool `json:"cancelled"`
	Active           bool `json:"active"`

	requestBody  bodyObservation
	responseBody bodyObservation
}

func (o *streamObservation) snapshot() streamObservation {
	copy := *o
	copy.Phases = append([]string(nil), o.Phases...)
	// Keep the JSON contract stable for streams with no response-header phase:
	// consumers expect an array, not null.
	copy.ResponseStatuses = append([]uint16{}, o.ResponseStatuses...)
	copy.RequestBodyMessages = o.requestBody.messages
	copy.RequestBodyBytes = o.requestBody.bytes
	copy.RequestBodySHA256 = o.requestBody.digest()
	copy.RequestEOSCount = o.requestBody.eos
	copy.ResponseBodyMessages = o.responseBody.messages
	copy.ResponseBodyBytes = o.responseBody.bytes
	copy.ResponseBodySHA256 = o.responseBody.digest()
	copy.ResponseEOSCount = o.responseBody.eos
	copy.requestBody = bodyObservation{}
	copy.responseBody = bodyObservation{}
	return copy
}

type caseObservation struct {
	CaseID  string               `json:"case_id"`
	Streams []*streamObservation `json:"-"`
	touched uint64
}

type caseSnapshot struct {
	CaseID  string              `json:"case_id"`
	Streams []streamObservation `json:"streams"`
}

type observationStore struct {
	mu           sync.Mutex
	cases        map[string]*caseObservation
	nextStreamID uint64
	nextTouch    uint64
}

func newObservationStore() *observationStore {
	return &observationStore{cases: make(map[string]*caseObservation)}
}

func (s *observationStore) begin(caseID string) (*streamObservation, error) {
	s.mu.Lock()
	defer s.mu.Unlock()

	c, ok := s.cases[caseID]
	if !ok {
		if len(s.cases) >= maxCases && !s.evictOldestCompletedLocked() {
			return nil, errors.New("observation case capacity is exhausted by active cases")
		}
		c = &caseObservation{CaseID: caseID}
		s.cases[caseID] = c
	}
	if len(c.Streams) >= maxStreamsPerCase {
		return nil, errors.New("stream limit for case is exhausted")
	}
	s.nextStreamID++
	s.nextTouch++
	c.touched = s.nextTouch
	stream := &streamObservation{StreamID: s.nextStreamID, Active: true}
	c.Streams = append(c.Streams, stream)
	return stream, nil
}

func (s *observationStore) evictOldestCompletedLocked() bool {
	var candidate *caseObservation
	for _, c := range s.cases {
		active := false
		for _, stream := range c.Streams {
			if stream.Active {
				active = true
				break
			}
		}
		if !active && (candidate == nil || c.touched < candidate.touched) {
			candidate = c
		}
	}
	if candidate == nil {
		return false
	}
	delete(s.cases, candidate.CaseID)
	return true
}

func (s *observationStore) addPhase(stream *streamObservation, phase string, body []byte, eos bool) error {
	return s.addPhaseWithStatus(stream, phase, body, eos, nil)
}

func (s *observationStore) addPhaseWithStatus(
	stream *streamObservation,
	phase string,
	body []byte,
	eos bool,
	responseStatus *uint16,
) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	phaseChanged := len(stream.Phases) == 0 || stream.Phases[len(stream.Phases)-1] != phase
	if phaseChanged && len(stream.Phases) >= maxPhaseTransitionsPerStream {
		return errors.New("phase transition limit exceeded")
	}
	switch phase {
	case "request_headers":
		if stream.RequestHeaderMessages >= maxHeaderMessagesPerDirection {
			return errors.New("request header message limit exceeded")
		}
	case "response_headers":
		if stream.ResponseHeaderMessages >= maxHeaderMessagesPerDirection {
			return errors.New("response header message limit exceeded")
		}
		if responseStatus == nil {
			return errors.New("response header status is missing")
		}
	case "response_trailers":
		if stream.ResponseTrailerMessages >= maxHeaderMessagesPerDirection {
			return errors.New("response trailer message limit exceeded")
		}
	}
	switch phase {
	case "request_headers":
		stream.RequestHeaderMessages++
	case "response_headers":
		stream.ResponseHeaderMessages++
		stream.ResponseStatuses = append(stream.ResponseStatuses, *responseStatus)
	case "request_body":
		if err := stream.requestBody.add(body, eos); err != nil {
			return err
		}
	case "response_body":
		if err := stream.responseBody.add(body, eos); err != nil {
			return err
		}
	case "response_trailers":
		stream.ResponseTrailerMessages++
	}
	if phaseChanged {
		stream.Phases = append(stream.Phases, phase)
	}
	return nil
}

func (s *observationStore) observeResponseTrailers(
	stream *streamObservation,
	trailers *corev3.HeaderMap,
) error {
	if err := s.addPhase(stream, "response_trailers", nil, false); err != nil {
		return err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	fields := trailers.GetHeaders()
	if len(fields) > maxHeaderMessagesPerDirection-stream.ResponseTrailerFields {
		return errors.New("response trailer field limit exceeded")
	}
	stream.ResponseTrailerFields += len(fields)
	for _, field := range fields {
		switch strings.ToLower(field.GetKey()) {
		case "x-edgion-upstream-trailer":
			stream.SawFixedResponseTrailer = true
		case "grpc-status":
			stream.SawGRPCStatusTrailer = true
		}
	}
	return nil
}

func (s *observationStore) setConnectionIdentity(stream *streamObservation, identity connectionIdentity) {
	s.mu.Lock()
	defer s.mu.Unlock()
	stream.Transport = identity.transport
	stream.TLSGeneration = identity.generation
	stream.ServerLeafSHA256 = identity.serverLeafFingerprint
	stream.PeerLeafSHA256 = identity.peerLeafFingerprint
}

func (s *observationStore) setOpaqueHeaderResult(stream *streamObservation, request, ok bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if request {
		stream.RequestOpaqueHeadersOK = ok
	} else {
		stream.ResponseOpaqueHeadersOK = ok
	}
}

type barrierWaiter struct {
	id      uint64
	release chan struct{}
}

type barrierState struct {
	id          string
	waiters     []*barrierWaiter
	arrivals    uint64
	releases    uint64
	releasedAll bool
}

type barrierSnapshot struct {
	BarrierID             string `json:"barrier_id"`
	Waiting               int    `json:"waiting"`
	Arrivals              uint64 `json:"arrivals"`
	Releases              uint64 `json:"releases"`
	ReleasedAll           bool   `json:"released_all"`
	WaiterCapacity        int    `json:"waiter_capacity"`
	FixtureStreamCapacity int    `json:"fixture_stream_capacity"`
}

type barrierRegistry struct {
	mu          sync.Mutex
	barriers    map[string]*barrierState
	nextWaiter  uint64
	waitTimeout time.Duration
}

func newBarrierRegistry(waitTimeout time.Duration) *barrierRegistry {
	return &barrierRegistry{barriers: make(map[string]*barrierState), waitTimeout: waitTimeout}
}

func (r *barrierRegistry) wait(ctx context.Context, barrierID string) error {
	r.mu.Lock()
	barrier, ok := r.barriers[barrierID]
	if !ok {
		if len(r.barriers) >= maxBarriers {
			r.mu.Unlock()
			return status.Error(codes.ResourceExhausted, "barrier capacity exceeded")
		}
		barrier = &barrierState{id: barrierID}
		r.barriers[barrierID] = barrier
	}
	if barrier.releasedAll {
		barrier.arrivals++
		barrier.releases++
		r.mu.Unlock()
		return nil
	}
	if len(barrier.waiters) >= maxBarrierWaiters {
		r.mu.Unlock()
		return status.Error(codes.ResourceExhausted, "barrier waiter limit exceeded")
	}
	barrier.arrivals++
	r.nextWaiter++
	waiter := &barrierWaiter{id: r.nextWaiter, release: make(chan struct{})}
	barrier.waiters = append(barrier.waiters, waiter)
	r.mu.Unlock()

	timer := time.NewTimer(r.waitTimeout)
	defer timer.Stop()
	select {
	case <-waiter.release:
		return nil
	case <-ctx.Done():
		r.removeWaiter(barrierID, waiter.id)
		return ctx.Err()
	case <-timer.C:
		r.removeWaiter(barrierID, waiter.id)
		return status.Error(codes.DeadlineExceeded, "barrier wait timed out")
	}
}

func (r *barrierRegistry) removeWaiter(barrierID string, waiterID uint64) {
	r.mu.Lock()
	defer r.mu.Unlock()
	barrier, ok := r.barriers[barrierID]
	if !ok {
		return
	}
	for index, waiter := range barrier.waiters {
		if waiter.id == waiterID {
			barrier.waiters = append(barrier.waiters[:index], barrier.waiters[index+1:]...)
			return
		}
	}
}

func (r *barrierRegistry) releaseOne(barrierID string) error {
	r.mu.Lock()
	defer r.mu.Unlock()
	barrier, ok := r.barriers[barrierID]
	if !ok || len(barrier.waiters) == 0 {
		return errors.New("barrier has no waiting stream")
	}
	waiter := barrier.waiters[0]
	barrier.waiters = barrier.waiters[1:]
	barrier.releases++
	close(waiter.release)
	return nil
}

func (r *barrierRegistry) releaseAll(barrierID string) error {
	r.mu.Lock()
	defer r.mu.Unlock()
	barrier, ok := r.barriers[barrierID]
	if !ok {
		if len(r.barriers) >= maxBarriers {
			return errors.New("barrier capacity exceeded")
		}
		barrier = &barrierState{id: barrierID}
		r.barriers[barrierID] = barrier
	}
	barrier.releasedAll = true
	waiting := len(barrier.waiters)
	for _, waiter := range barrier.waiters {
		close(waiter.release)
	}
	barrier.releases += uint64(waiting)
	barrier.waiters = nil
	return nil
}

func (r *barrierRegistry) snapshot(barrierID string) (barrierSnapshot, bool) {
	r.mu.Lock()
	defer r.mu.Unlock()
	barrier, ok := r.barriers[barrierID]
	if !ok {
		return barrierSnapshot{}, false
	}
	return barrierSnapshot{
		BarrierID: barrier.id, Waiting: len(barrier.waiters), Arrivals: barrier.arrivals,
		Releases: barrier.releases, ReleasedAll: barrier.releasedAll,
		WaiterCapacity: maxBarrierWaiters, FixtureStreamCapacity: maxConcurrentStreams,
	}, true
}

func (r *barrierRegistry) reset(barrierID string) error {
	r.mu.Lock()
	defer r.mu.Unlock()
	barrier, ok := r.barriers[barrierID]
	if !ok {
		return nil
	}
	if len(barrier.waiters) != 0 {
		return errors.New("cannot reset a barrier with waiting streams")
	}
	delete(r.barriers, barrierID)
	return nil
}

func (s *observationStore) finish(stream *streamObservation, halfClosed, cancelled bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	stream.ClientHalfClosed = halfClosed
	stream.Cancelled = cancelled
	stream.Active = false
}

func (s *observationStore) snapshot(caseID string) (caseSnapshot, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	c, ok := s.cases[caseID]
	if !ok {
		return caseSnapshot{}, false
	}
	result := caseSnapshot{CaseID: caseID, Streams: make([]streamObservation, 0, len(c.Streams))}
	for _, stream := range c.Streams {
		result.Streams = append(result.Streams, stream.snapshot())
	}
	return result, true
}

func (s *observationStore) reset(caseID string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	c, ok := s.cases[caseID]
	if !ok {
		return nil
	}
	for _, stream := range c.Streams {
		if stream.Active {
			return errors.New("cannot reset a case with an active stream")
		}
	}
	delete(s.cases, caseID)
	return nil
}

type processor struct {
	extprocv3.UnimplementedExternalProcessorServer
	store    *observationStore
	barriers *barrierRegistry
	slots    chan struct{}
}

func (p *processor) Process(stream extprocv3.ExternalProcessor_ProcessServer) error {
	if p.slots != nil {
		select {
		case p.slots <- struct{}{}:
			defer func() { <-p.slots }()
		default:
			return status.Error(codes.ResourceExhausted, "concurrent stream limit exceeded")
		}
	}
	first, err := stream.Recv()
	if err != nil {
		return err
	}
	headers := first.GetRequestHeaders()
	if headers == nil {
		return status.Error(codes.InvalidArgument, "first message must be request_headers")
	}
	caseID := headerValue(headers.GetHeaders(), caseIDHeader)
	if !validCaseID(caseID) {
		return status.Error(codes.InvalidArgument, "invalid or missing x-ext-proc-case-id")
	}
	mode := headerValue(headers.GetHeaders(), modeHeader)
	if !validMode(mode) {
		return status.Error(codes.InvalidArgument, "invalid or missing x-ext-proc-test-mode")
	}
	barrierID := ""
	if isBarrierMode(mode) {
		barrierID = headerValue(headers.GetHeaders(), barrierHeader)
		if !validCaseID(barrierID) {
			return status.Error(codes.InvalidArgument, "invalid or missing x-ext-proc-barrier-id")
		}
		if p.barriers == nil {
			return status.Error(codes.FailedPrecondition, "barrier registry is unavailable")
		}
	}
	observation, err := p.store.begin(caseID)
	if err != nil {
		return status.Error(codes.ResourceExhausted, err.Error())
	}
	identity, ok := stream.Context().Value(connectionIdentityContextKey{}).(connectionIdentity)
	if !ok {
		identity = connectionIdentity{transport: "plain"}
	}
	p.store.setConnectionIdentity(observation, identity)
	halfClosed := false
	cancelled := false
	defer func() { p.store.finish(observation, halfClosed, cancelled) }()

	request := first
	barrierUsed := false
	for {
		if err := p.observeAndRespond(stream, observation, mode, barrierID, &barrierUsed, request); err != nil {
			if errors.Is(err, context.Canceled) || status.Code(err) == codes.Canceled || stream.Context().Err() != nil {
				cancelled = true
			}
			return err
		}
		request, err = stream.Recv()
		if errors.Is(err, io.EOF) {
			halfClosed = true
			return nil
		}
		if err != nil {
			if errors.Is(err, context.Canceled) || status.Code(err) == codes.Canceled || stream.Context().Err() != nil {
				cancelled = true
			}
			return err
		}
	}
}

func (p *processor) observeAndRespond(
	stream extprocv3.ExternalProcessor_ProcessServer,
	observation *streamObservation,
	mode string,
	barrierID string,
	barrierUsed *bool,
	request *extprocv3.ProcessingRequest,
) error {
	var phase string
	var body []byte
	var eos bool
	var response *extprocv3.ProcessingResponse
	var observedResponseStatus *uint16

	switch value := request.GetRequest().(type) {
	case *extprocv3.ProcessingRequest_RequestHeaders:
		phase = "request_headers"
		if mode == "opaque-headers" || mode == "opaque-immediate" {
			ok := opaqueRequestHeadersMatch(value.RequestHeaders.GetHeaders())
			p.store.setOpaqueHeaderResult(observation, true, ok)
			if !ok {
				return status.Error(codes.InvalidArgument, "opaque request headers did not match")
			}
			if mode == "opaque-immediate" {
				response = opaqueImmediateResponse()
			} else {
				response = opaqueRequestHeaderResponse()
			}
		} else if mode == "immediate" || mode == "disabled-immediate" {
			response = immediateResponse()
		} else if mode == "grpc-immediate" {
			response = grpcImmediateResponse()
		} else if mode == "mode-reduce-none" {
			response = headerResponseWithModeOverride(extprocmodev3.ProcessingMode_NONE)
		} else if mode == "mode-forbidden-expand" {
			response = headerResponseWithModeOverride(extprocmodev3.ProcessingMode_BUFFERED)
		} else {
			response = headerResponse(true)
		}
	case *extprocv3.ProcessingRequest_RequestBody:
		phase, body, eos = "request_body", value.RequestBody.GetBody(), value.RequestBody.GetEndOfStream()
		response = bodyResponse(true)
	case *extprocv3.ProcessingRequest_ResponseHeaders:
		phase = "response_headers"
		statusCode, err := extractResponseStatus(value.ResponseHeaders.GetHeaders())
		if err != nil {
			return status.Error(codes.InvalidArgument, err.Error())
		}
		observedResponseStatus = &statusCode
		if mode == "opaque-headers" {
			ok := opaqueResponseHeadersMatch(value.ResponseHeaders.GetHeaders())
			p.store.setOpaqueHeaderResult(observation, false, ok)
			if !ok {
				return status.Error(codes.InvalidArgument, "opaque response headers did not match")
			}
			response = opaqueResponseHeaderResponse()
		} else {
			response = headerResponse(false)
		}
	case *extprocv3.ProcessingRequest_ResponseBody:
		phase, body, eos = "response_body", value.ResponseBody.GetBody(), value.ResponseBody.GetEndOfStream()
		response = bodyResponse(false)
	case *extprocv3.ProcessingRequest_ResponseTrailers:
		if err := p.store.observeResponseTrailers(observation, value.ResponseTrailers.GetTrailers()); err != nil {
			return status.Error(codes.ResourceExhausted, err.Error())
		}
		return stream.Send(responseTrailerResponse())
	default:
		return status.Error(codes.Unimplemented, "unsupported ExtProc phase")
	}
	if err := p.store.addPhaseWithStatus(observation, phase, body, eos, observedResponseStatus); err != nil {
		return status.Error(codes.ResourceExhausted, err.Error())
	}
	if mode == "long-idle" && phase == "request_headers" {
		timer := time.NewTimer(longIdleDelay)
		defer timer.Stop()
		select {
		case <-timer.C:
		case <-stream.Context().Done():
			return stream.Context().Err()
		}
	}
	shouldWait := !*barrierUsed && ((mode == "barrier-headers" && phase == "request_headers") ||
		(mode == "barrier-request-body" && phase == "request_body"))
	if shouldWait {
		*barrierUsed = true
		if err := p.barriers.wait(stream.Context(), barrierID); err != nil {
			return err
		}
	}
	return stream.Send(response)
}

func validCaseID(value string) bool { return caseIDPattern.MatchString(value) }

func validMode(value string) bool {
	switch value {
	case "interop", "immediate", "grpc-immediate", "disabled-immediate", "mode-reduce-none",
		"mode-forbidden-expand", "trailer-mutate", "no-trailer", "long-idle", "barrier-headers",
		"barrier-request-body", "opaque-headers", "opaque-immediate":
		return true
	default:
		return false
	}
}

func isBarrierMode(mode string) bool {
	return mode == "barrier-headers" || mode == "barrier-request-body"
}

func headerValue(headers *corev3.HeaderMap, name string) string {
	if headers == nil {
		return ""
	}
	for _, header := range headers.GetHeaders() {
		if strings.EqualFold(header.GetKey(), name) {
			if len(header.GetRawValue()) != 0 {
				return string(header.GetRawValue())
			}
			return header.GetValue()
		}
	}
	return ""
}

func extractResponseStatus(headers *corev3.HeaderMap) (uint16, error) {
	value := headerValue(headers, ":status")
	statusCode, err := strconv.ParseUint(value, 10, 16)
	if err != nil || statusCode < 100 || statusCode > 599 {
		return 0, errors.New("response_headers has an invalid or missing :status")
	}
	return uint16(statusCode), nil
}

func setHeader(name, value string) *corev3.HeaderValueOption {
	return &corev3.HeaderValueOption{
		Header:       &corev3.HeaderValue{Key: name, RawValue: []byte(value)},
		AppendAction: corev3.HeaderValueOption_OVERWRITE_IF_EXISTS_OR_ADD,
	}
}

func rawHeaderAction(name string, value []byte, action corev3.HeaderValueOption_HeaderAppendAction) *corev3.HeaderValueOption {
	return &corev3.HeaderValueOption{
		Header:       &corev3.HeaderValue{Key: name, RawValue: value},
		AppendAction: action,
	}
}

func headerRawValues(headers *corev3.HeaderMap, name string) [][]byte {
	if headers == nil {
		return nil
	}
	values := make([][]byte, 0)
	for _, header := range headers.GetHeaders() {
		if !strings.EqualFold(header.GetKey(), name) {
			continue
		}
		value := header.GetRawValue()
		if len(value) == 0 {
			value = []byte(header.GetValue())
		}
		values = append(values, value)
	}
	return values
}

func rawValuesEqual(actual [][]byte, expected ...[]byte) bool {
	if len(actual) != len(expected) {
		return false
	}
	for index := range actual {
		if !bytes.Equal(actual[index], expected[index]) {
			return false
		}
	}
	return true
}

func opaqueRequestHeadersMatch(headers *corev3.HeaderMap) bool {
	return rawValuesEqual(headerRawValues(headers, "x-opaque-presence"), []byte("\xffrequest-first"), []byte("\x80request-second")) &&
		rawValuesEqual(headerRawValues(headers, "x-opaque-overwrite"), []byte("\xferequest-old"))
}

func opaqueResponseHeadersMatch(headers *corev3.HeaderMap) bool {
	return rawValuesEqual(headerRawValues(headers, "x-opaque-presence"), []byte("\xfdresponse-first"), []byte("\x80response-second")) &&
		rawValuesEqual(headerRawValues(headers, "x-opaque-overwrite"), []byte("\xfcresponse-old"))
}

func opaqueRequestHeaderResponse() *extprocv3.ProcessingResponse {
	mutation := &extprocv3.HeaderMutation{SetHeaders: []*corev3.HeaderValueOption{
		rawHeaderAction("x-opaque-presence", []byte("must-not-replace"), corev3.HeaderValueOption_ADD_IF_ABSENT),
		rawHeaderAction("x-opaque-overwrite", []byte("\x81request-overwrite"), corev3.HeaderValueOption_OVERWRITE_IF_EXISTS),
		rawHeaderAction("x-opaque-added", []byte("\x82request-add"), corev3.HeaderValueOption_ADD_IF_ABSENT),
		rawHeaderAction("x-opaque-added", []byte("\x83request-append"), corev3.HeaderValueOption_APPEND_IF_EXISTS_OR_ADD),
		setHeader("x-ext-proc-backend-opaque", "1"),
	}}
	return &extprocv3.ProcessingResponse{Response: &extprocv3.ProcessingResponse_RequestHeaders{
		RequestHeaders: &extprocv3.HeadersResponse{Response: &extprocv3.CommonResponse{
			Status: extprocv3.CommonResponse_CONTINUE, HeaderMutation: mutation,
		}},
	}}
}

func opaqueResponseMutation() *extprocv3.HeaderMutation {
	empty := rawHeaderAction("x-opaque-empty", []byte{}, corev3.HeaderValueOption_OVERWRITE_IF_EXISTS_OR_ADD)
	empty.KeepEmptyValue = true
	return &extprocv3.HeaderMutation{SetHeaders: []*corev3.HeaderValueOption{
		rawHeaderAction("x-opaque-presence", []byte("must-not-replace"), corev3.HeaderValueOption_ADD_IF_ABSENT),
		rawHeaderAction("x-opaque-overwrite", []byte("\x81response-overwrite"), corev3.HeaderValueOption_OVERWRITE_IF_EXISTS),
		rawHeaderAction("x-opaque-added", []byte("\x82response-add"), corev3.HeaderValueOption_ADD_IF_ABSENT),
		rawHeaderAction("x-opaque-added", []byte("\x83response-append"), corev3.HeaderValueOption_APPEND_IF_EXISTS_OR_ADD),
		empty,
	}}
}

func opaqueResponseHeaderResponse() *extprocv3.ProcessingResponse {
	return &extprocv3.ProcessingResponse{Response: &extprocv3.ProcessingResponse_ResponseHeaders{
		ResponseHeaders: &extprocv3.HeadersResponse{Response: &extprocv3.CommonResponse{
			Status: extprocv3.CommonResponse_CONTINUE, HeaderMutation: opaqueResponseMutation(),
		}},
	}}
}

func opaqueImmediateResponse() *extprocv3.ProcessingResponse {
	mutation := &extprocv3.HeaderMutation{SetHeaders: []*corev3.HeaderValueOption{
		rawHeaderAction("x-opaque-immediate", []byte("\x84immediate-first"), corev3.HeaderValueOption_OVERWRITE_IF_EXISTS_OR_ADD),
		rawHeaderAction("x-opaque-immediate", []byte("\x85immediate-second"), corev3.HeaderValueOption_APPEND_IF_EXISTS_OR_ADD),
	}}
	return &extprocv3.ProcessingResponse{Response: &extprocv3.ProcessingResponse_ImmediateResponse{
		ImmediateResponse: &extprocv3.ImmediateResponse{
			Status: &typev3.HttpStatus{Code: typev3.StatusCode_Forbidden}, Headers: mutation,
			Body: []byte("opaque immediate response"),
		},
	}}
}

func commonResponse(headerName, headerValue string, body []byte) *extprocv3.CommonResponse {
	response := &extprocv3.CommonResponse{Status: extprocv3.CommonResponse_CONTINUE}
	if headerName != "" {
		response.HeaderMutation = &extprocv3.HeaderMutation{SetHeaders: []*corev3.HeaderValueOption{setHeader(headerName, headerValue)}}
	}
	if body != nil {
		response.BodyMutation = &extprocv3.BodyMutation{Mutation: &extprocv3.BodyMutation_Body{Body: body}}
	}
	return response
}

func headerResponse(request bool) *extprocv3.ProcessingResponse {
	if request {
		return &extprocv3.ProcessingResponse{Response: &extprocv3.ProcessingResponse_RequestHeaders{RequestHeaders: &extprocv3.HeadersResponse{Response: commonResponse("x-ext-proc-req", "external-go", nil)}}}
	}
	return &extprocv3.ProcessingResponse{Response: &extprocv3.ProcessingResponse_ResponseHeaders{ResponseHeaders: &extprocv3.HeadersResponse{Response: commonResponse("x-ext-proc-resp", "external-go", nil)}}}
}

func headerResponseWithModeOverride(requestBodyMode extprocmodev3.ProcessingMode_BodySendMode) *extprocv3.ProcessingResponse {
	response := headerResponse(true)
	response.ModeOverride = &extprocmodev3.ProcessingMode{
		RequestHeaderMode:   extprocmodev3.ProcessingMode_DEFAULT,
		ResponseHeaderMode:  extprocmodev3.ProcessingMode_DEFAULT,
		RequestBodyMode:     requestBodyMode,
		ResponseBodyMode:    extprocmodev3.ProcessingMode_NONE,
		RequestTrailerMode:  extprocmodev3.ProcessingMode_DEFAULT,
		ResponseTrailerMode: extprocmodev3.ProcessingMode_DEFAULT,
	}
	return response
}

func bodyResponse(request bool) *extprocv3.ProcessingResponse {
	if request {
		return &extprocv3.ProcessingResponse{Response: &extprocv3.ProcessingResponse_RequestBody{RequestBody: &extprocv3.BodyResponse{Response: commonResponse("", "", []byte("processed-by-external-go"))}}}
	}
	return &extprocv3.ProcessingResponse{Response: &extprocv3.ProcessingResponse_ResponseBody{ResponseBody: &extprocv3.BodyResponse{Response: commonResponse("", "", []byte("response-by-external-go"))}}}
}

func immediateResponse() *extprocv3.ProcessingResponse {
	return &extprocv3.ProcessingResponse{Response: &extprocv3.ProcessingResponse_ImmediateResponse{ImmediateResponse: &extprocv3.ImmediateResponse{
		Status:  &typev3.HttpStatus{Code: typev3.StatusCode_Forbidden},
		Headers: &extprocv3.HeaderMutation{SetHeaders: []*corev3.HeaderValueOption{setHeader("x-ext-proc-denied", "1")}},
		Body:    []byte("blocked by external ext_proc"),
	}}}
}

func grpcImmediateResponse() *extprocv3.ProcessingResponse {
	return &extprocv3.ProcessingResponse{Response: &extprocv3.ProcessingResponse_ImmediateResponse{ImmediateResponse: &extprocv3.ImmediateResponse{
		Status:     &typev3.HttpStatus{Code: typev3.StatusCode_Forbidden},
		Body:       []byte("processor denied exact grpc request"),
		GrpcStatus: &extprocv3.GrpcStatus{Status: uint32(codes.PermissionDenied)},
	}}}
}

func responseTrailerResponse() *extprocv3.ProcessingResponse {
	return &extprocv3.ProcessingResponse{Response: &extprocv3.ProcessingResponse_ResponseTrailers{
		ResponseTrailers: &extprocv3.TrailersResponse{HeaderMutation: &extprocv3.HeaderMutation{
			SetHeaders: []*corev3.HeaderValueOption{setHeader("x-edgion-upstream-trailer", "processed-by-external-go")},
		}},
	}}
}

type adminHandler struct {
	store    *observationStore
	barriers *barrierRegistry
	tls      *tlsTransportManager
}

func (h adminHandler) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	r.Body = http.MaxBytesReader(w, r.Body, maxAdminBodyBytes)
	if r.URL.RawQuery != "" {
		http.Error(w, "query parameters are not supported", http.StatusBadRequest)
		return
	}
	if r.URL.Path == "/readyz" {
		if r.Method != http.MethodGet {
			methodNotAllowed(w, http.MethodGet)
			return
		}
		if !emptyBody(r) {
			http.Error(w, "request body must be empty", http.StatusBadRequest)
			return
		}
		_, tlsReady := h.tls.listener("tls")
		_, mtlsReady := h.tls.listener("mtls")
		writeJSON(w, http.StatusOK, map[string]any{
			"status": "ready", "fixture": fixtureIdentity, "protocol_version": protocolVersion,
			"go_control_plane_envoy": moduleVersion, "fixture_stream_capacity": maxConcurrentStreams,
			"tls_capable": true, "mtls_capable": true, "tls_ready": tlsReady, "mtls_ready": mtlsReady,
		})
		return
	}
	if r.URL.Path == "/v1/tls" || strings.HasPrefix(r.URL.Path, "/v1/tls/") {
		h.serveTLS(w, r)
		return
	}
	if strings.HasPrefix(r.URL.Path, "/v1/barriers/") {
		h.serveBarrier(w, r)
		return
	}

	const prefix = "/v1/cases/"
	if !strings.HasPrefix(r.URL.Path, prefix) {
		http.NotFound(w, r)
		return
	}
	caseID := strings.TrimPrefix(r.URL.Path, prefix)
	reset := strings.HasSuffix(caseID, "/reset")
	if reset {
		caseID = strings.TrimSuffix(caseID, "/reset")
	}
	if strings.Contains(caseID, "/") || !validCaseID(caseID) {
		http.Error(w, "invalid case ID", http.StatusBadRequest)
		return
	}
	if !emptyBody(r) {
		http.Error(w, "request body must be empty", http.StatusBadRequest)
		return
	}
	if reset {
		if r.Method != http.MethodPost {
			methodNotAllowed(w, http.MethodPost)
			return
		}
		if err := h.store.reset(caseID); err != nil {
			http.Error(w, err.Error(), http.StatusConflict)
			return
		}
		w.WriteHeader(http.StatusNoContent)
		return
	}
	if r.Method != http.MethodGet {
		methodNotAllowed(w, http.MethodGet)
		return
	}
	snapshot, ok := h.store.snapshot(caseID)
	if !ok {
		http.NotFound(w, r)
		return
	}
	writeJSON(w, http.StatusOK, snapshot)
}

func (h adminHandler) serveTLS(w http.ResponseWriter, r *http.Request) {
	if h.tls == nil || h.tls.materials == nil {
		http.Error(w, "TLS fixture listeners are disabled", http.StatusNotFound)
		return
	}
	if !emptyBody(r) {
		http.Error(w, "request body must be empty", http.StatusBadRequest)
		return
	}
	if r.URL.Path == "/v1/tls" {
		if r.Method != http.MethodGet {
			methodNotAllowed(w, http.MethodGet)
			return
		}
		writeJSON(w, http.StatusOK, h.tls.snapshot())
		return
	}
	parts := strings.Split(strings.TrimPrefix(r.URL.Path, "/v1/tls/"), "/")
	if len(parts) != 3 || parts[1] != "rotate" || (parts[2] != "a" && parts[2] != "b") {
		http.NotFound(w, r)
		return
	}
	listener, ok := h.tls.listener(parts[0])
	if !ok {
		http.NotFound(w, r)
		return
	}
	if r.Method != http.MethodPost {
		methodNotAllowed(w, http.MethodPost)
		return
	}
	if err := listener.rotate(parts[2]); err != nil {
		http.Error(w, "TLS rotation failed", http.StatusConflict)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (h adminHandler) serveBarrier(w http.ResponseWriter, r *http.Request) {
	if h.barriers == nil {
		http.Error(w, "barrier registry is unavailable", http.StatusServiceUnavailable)
		return
	}
	rest := strings.TrimPrefix(r.URL.Path, "/v1/barriers/")
	parts := strings.Split(rest, "/")
	if len(parts) < 1 || len(parts) > 2 || !validCaseID(parts[0]) {
		http.Error(w, "invalid barrier ID or path", http.StatusBadRequest)
		return
	}
	if !emptyBody(r) {
		http.Error(w, "request body must be empty", http.StatusBadRequest)
		return
	}
	barrierID := parts[0]
	if len(parts) == 1 {
		if r.Method != http.MethodGet {
			methodNotAllowed(w, http.MethodGet)
			return
		}
		snapshot, ok := h.barriers.snapshot(barrierID)
		if !ok {
			http.NotFound(w, r)
			return
		}
		writeJSON(w, http.StatusOK, snapshot)
		return
	}
	if parts[1] != "release-one" && parts[1] != "release-all" && parts[1] != "reset" {
		http.NotFound(w, r)
		return
	}
	if r.Method != http.MethodPost {
		methodNotAllowed(w, http.MethodPost)
		return
	}
	var err error
	switch parts[1] {
	case "release-one":
		err = h.barriers.releaseOne(barrierID)
	case "release-all":
		err = h.barriers.releaseAll(barrierID)
	case "reset":
		err = h.barriers.reset(barrierID)
	}
	if err != nil {
		http.Error(w, err.Error(), http.StatusConflict)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func emptyBody(r *http.Request) bool {
	var one [1]byte
	n, err := r.Body.Read(one[:])
	return n == 0 && errors.Is(err, io.EOF)
}

func methodNotAllowed(w http.ResponseWriter, allowed string) {
	w.Header().Set("Allow", allowed)
	http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
}

func writeJSON(w http.ResponseWriter, code int, value any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	if err := json.NewEncoder(w).Encode(value); err != nil {
		log.Printf("admin response write failed: %v", err)
	}
}

func validateListenAddress(raw string) error {
	return validateListenAddressForMode(raw, false)
}

func validateListenAddressForMode(raw string, allowClusterListen bool) error {
	host, port, err := net.SplitHostPort(raw)
	if err != nil {
		return fmt.Errorf("invalid listen address %q: %w", raw, err)
	}
	if host != "127.0.0.1" && !(allowClusterListen && host == "0.0.0.0") {
		return fmt.Errorf("listen address %q must use literal 127.0.0.1 or an explicitly enabled cluster wildcard", raw)
	}
	if port == "" {
		return fmt.Errorf("listen address %q has no port", raw)
	}
	portNumber, err := strconv.Atoi(port)
	if err != nil || portNumber < 1 || portNumber > 65535 {
		return fmt.Errorf("listen address %q has an invalid port", raw)
	}
	return nil
}

func listenBoth(grpcAddress, adminAddress string) (net.Listener, net.Listener, error) {
	grpcListener, err := net.Listen("tcp", grpcAddress)
	if err != nil {
		return nil, nil, fmt.Errorf("bind gRPC listener: %w", err)
	}
	adminListener, err := net.Listen("tcp", adminAddress)
	if err != nil {
		_ = grpcListener.Close()
		return nil, nil, fmt.Errorf("bind admin listener: %w", err)
	}
	return grpcListener, adminListener, nil
}

type runOptions struct {
	grpcAddress        string
	adminAddress       string
	tlsAddress         string
	mtlsAddress        string
	materialDirectory  string
	allowClusterListen bool
	serverDNSName      string
}

func run(ctx context.Context, grpcAddress, adminAddress string) error {
	return runWithOptions(ctx, runOptions{grpcAddress: grpcAddress, adminAddress: adminAddress})
}

func newFixtureGRPCServer(processor *processor, handler stats.Handler) *grpc.Server {
	options := []grpc.ServerOption{
		grpc.MaxRecvMsgSize(maxGRPCMessageBytes),
		grpc.MaxSendMsgSize(maxGRPCMessageBytes),
		grpc.MaxConcurrentStreams(maxConcurrentStreams),
	}
	if handler != nil {
		options = append(options, grpc.StatsHandler(handler))
	}
	server := grpc.NewServer(options...)
	extprocv3.RegisterExternalProcessorServer(server, processor)
	return server
}

func runWithOptions(ctx context.Context, options runOptions) error {
	if err := verifyEnvoyModuleVersion(); err != nil {
		return err
	}
	if err := validateListenAddressForMode(options.grpcAddress, options.allowClusterListen); err != nil {
		return err
	}
	if err := validateListenAddressForMode(options.adminAddress, options.allowClusterListen); err != nil {
		return err
	}
	for name, address := range map[string]string{"TLS": options.tlsAddress, "mTLS": options.mtlsAddress} {
		if address != "" {
			if err := validateListenAddressForMode(address, options.allowClusterListen); err != nil {
				return fmt.Errorf("%s listener: %w", name, err)
			}
		}
	}
	tlsEnabled := options.tlsAddress != "" || options.mtlsAddress != ""
	if tlsEnabled && options.materialDirectory == "" {
		return errors.New("--tls-material-dir is required when TLS or mTLS is enabled")
	}
	var materials *tlsMaterials
	var err error
	if tlsEnabled {
		materials, err = generateTLSMaterialsWithDNS(options.materialDirectory, options.serverDNSName)
		if err != nil {
			return err
		}
	}
	grpcListener, adminListener, err := listenBoth(options.grpcAddress, options.adminAddress)
	if err != nil {
		return err
	}
	bound := []net.Listener{grpcListener, adminListener}
	closeBound := func() {
		for _, listener := range bound {
			_ = listener.Close()
		}
	}
	var tlsBase, mtlsBase net.Listener
	if options.tlsAddress != "" {
		tlsBase, err = net.Listen("tcp", options.tlsAddress)
		if err != nil {
			closeBound()
			return fmt.Errorf("bind TLS gRPC listener: %w", err)
		}
		bound = append(bound, tlsBase)
	}
	if options.mtlsAddress != "" {
		mtlsBase, err = net.Listen("tcp", options.mtlsAddress)
		if err != nil {
			closeBound()
			return fmt.Errorf("bind mTLS gRPC listener: %w", err)
		}
		bound = append(bound, mtlsBase)
	}

	store := newObservationStore()
	barriers := newBarrierRegistry(barrierWaitTimeout)
	sharedProcessor := &processor{
		store: store, barriers: barriers, slots: make(chan struct{}, maxConcurrentStreams),
	}
	plainServer := newFixtureGRPCServer(sharedProcessor, nil)
	manager := &tlsTransportManager{materials: materials}
	servers := []*grpc.Server{plainServer}
	listeners := []net.Listener{grpcListener}
	if tlsBase != nil {
		manager.tls = newRotatingTLSListener(tlsBase, "tls", false, materials)
		servers = append(servers, newFixtureGRPCServer(sharedProcessor, connectionStatsHandler{listener: manager.tls}))
		listeners = append(listeners, manager.tls)
	}
	if mtlsBase != nil {
		manager.mtls = newRotatingTLSListener(mtlsBase, "mtls", true, materials)
		servers = append(servers, newFixtureGRPCServer(sharedProcessor, connectionStatsHandler{listener: manager.mtls}))
		listeners = append(listeners, manager.mtls)
	}
	adminServer := &http.Server{
		Handler:           adminHandler{store: store, barriers: barriers, tls: manager},
		ReadHeaderTimeout: 2 * time.Second,
		ReadTimeout:       3 * time.Second,
		WriteTimeout:      5 * time.Second,
		IdleTimeout:       10 * time.Second,
		MaxHeaderBytes:    8 << 10,
	}

	errorsCh := make(chan error, len(servers)+1)
	for index, server := range servers {
		server, listener := server, listeners[index]
		go func() { errorsCh <- server.Serve(listener) }()
	}
	go func() { errorsCh <- adminServer.Serve(adminListener) }()
	select {
	case <-ctx.Done():
		grpcStopped := make(chan struct{})
		go func() {
			var wait sync.WaitGroup
			wait.Add(len(servers))
			for _, server := range servers {
				server := server
				go func() { defer wait.Done(); server.GracefulStop() }()
			}
			wait.Wait()
			close(grpcStopped)
		}()
		select {
		case <-grpcStopped:
		case <-time.After(3 * time.Second):
			for _, server := range servers {
				server.Stop()
			}
		}
		shutdownCtx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		defer cancel()
		if err := adminServer.Shutdown(shutdownCtx); err != nil {
			return fmt.Errorf("admin shutdown: %w", err)
		}
		return nil
	case err := <-errorsCh:
		for _, server := range servers {
			server.Stop()
		}
		_ = adminServer.Close()
		if errors.Is(err, grpc.ErrServerStopped) || errors.Is(err, http.ErrServerClosed) {
			return errors.New("fixture listener stopped unexpectedly")
		}
		return fmt.Errorf("fixture listener failed: %w", err)
	}
}

func main() {
	grpcAddress := flag.String("grpc-listen", "", "address for the ExtProc gRPC listener")
	adminAddress := flag.String("admin-listen", "", "address for the readiness and observation listener")
	tlsAddress := flag.String("tls-listen", "", "optional address for the TLS ExtProc gRPC listener")
	mtlsAddress := flag.String("mtls-listen", "", "optional address for the mTLS ExtProc gRPC listener")
	materialDirectory := flag.String("tls-material-dir", "", "work-directory path for generated TLS test material")
	allowClusterListen := flag.Bool("allow-cluster-listen", false, "allow wildcard listeners in the isolated Kubernetes test fixture")
	serverDNSName := flag.String("server-dns-name", "", "optional DNS name in generated server certificates")
	flag.Parse()
	if *grpcAddress == "" || *adminAddress == "" {
		flag.Usage()
		os.Exit(2)
	}
	if *serverDNSName != "" && !*allowClusterListen {
		log.Fatal("--server-dns-name requires --allow-cluster-listen")
	}
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	if err := runWithOptions(ctx, runOptions{
		grpcAddress: *grpcAddress, adminAddress: *adminAddress, tlsAddress: *tlsAddress,
		mtlsAddress: *mtlsAddress, materialDirectory: *materialDirectory,
		allowClusterListen: *allowClusterListen, serverDNSName: *serverDNSName,
	}); err != nil {
		log.Printf("%s failed: %v", fixtureIdentity, err)
		os.Exit(1)
	}
}
