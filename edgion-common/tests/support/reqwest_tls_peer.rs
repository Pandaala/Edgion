//! Owned loopback TLS/H2 peer for canonical reqwest transport integration tests.
use bytes::Bytes;
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

pub struct Material {
    pub ca: String,
    pub cert: String,
    pub key: String,
}
impl Material {
    pub fn new() -> Self {
        use rcgen::*;
        let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::DigitalSignature];
        let key = KeyPair::generate().unwrap();
        let pem = ca.self_signed(&key).unwrap().pem();
        let issuer = Issuer::new(ca, key);
        let mut leaf = CertificateParams::new(vec!["localhost".into()]).unwrap();
        leaf.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth, ExtendedKeyUsagePurpose::ClientAuth];
        let key = KeyPair::generate().unwrap();
        Self {
            ca: pem,
            cert: leaf.signed_by(&key, &issuer).unwrap().pem(),
            key: key.serialize_pem(),
        }
    }
}
#[derive(Debug)]
pub struct Observed {
    pub sni: Option<String>,
    pub alpn: Vec<u8>,
    pub client_certificates: usize,
}
pub struct Peer {
    pub address: SocketAddr,
    task: Option<JoinHandle<Result<Observed, &'static str>>>,
}
impl Peer {
    /// None: no client auth; Some(false): optional; Some(true): required.
    pub async fn start(material: &Material, client_auth: Option<bool>) -> Self {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let builder = rustls::ServerConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .unwrap();
        let builder = if let Some(required) = client_auth {
            let mut roots = rustls::RootCertStore::empty();
            for certificate in CertificateDer::pem_slice_iter(material.ca.as_bytes()) {
                roots.add(certificate.unwrap()).unwrap();
            }
            let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider);
            let verifier = if required {
                verifier
            } else {
                verifier.allow_unauthenticated()
            };
            builder.with_client_cert_verifier(verifier.build().unwrap())
        } else {
            builder.with_no_client_auth()
        };
        let mut config = builder
            .with_single_cert(
                CertificateDer::pem_slice_iter(material.cert.as_bytes())
                    .collect::<Result<_, _>>()
                    .unwrap(),
                PrivateKeyDer::from_pem_slice(material.key.as_bytes()).unwrap(),
            )
            .unwrap();
        config.alpn_protocols = vec![b"h2".to_vec()];
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            tokio::time::timeout(Duration::from_secs(5), async {
                let (socket, _) = listener.accept().await.map_err(|_| "accept_failed")?;
                let stream = tokio_rustls::TlsAcceptor::from(Arc::new(config))
                    .accept(socket)
                    .await
                    .map_err(|_| "tls_failed")?;
                let observed = Observed {
                    sni: stream.get_ref().1.server_name().map(str::to_owned),
                    alpn: stream.get_ref().1.alpn_protocol().unwrap_or_default().to_vec(),
                    client_certificates: stream
                        .get_ref()
                        .1
                        .peer_certificates()
                        .map_or(0, |certificates| certificates.len()),
                };
                let mut connection = h2::server::handshake(stream).await.map_err(|_| "h2_failed")?;
                let (request, mut response) = connection
                    .accept()
                    .await
                    .ok_or("request_missing")?
                    .map_err(|_| "request_failed")?;
                drop(request);
                let mut response = response
                    .send_response(http::Response::builder().status(200).body(()).unwrap(), false)
                    .map_err(|_| "response_failed")?;
                response
                    .send_data(Bytes::from_static(b"ok"), true)
                    .map_err(|_| "body_failed")?;
                // Poll the connection to flush the response, then await client release.
                if let Some(request) = connection.accept().await {
                    request.map_err(|_| "connection_failed")?;
                    return Err("unexpected_additional_request");
                }
                Ok(observed)
            })
            .await
            .map_err(|_| "peer_deadline")?
        });
        Self {
            address,
            task: Some(task),
        }
    }
    pub fn url(&self) -> String {
        format!("https://localhost:{}/", self.address.port())
    }
    pub async fn finish(mut self) -> Result<Observed, &'static str> {
        let result = self.task.as_mut().unwrap().await.map_err(|_| "peer_task_failed")?;
        self.task.take();
        result
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
