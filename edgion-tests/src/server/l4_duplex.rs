//! Explicitly controlled TCP/TLS fixture, owned by the shared test_server.

#[path = "l4_duplex_protocol.rs"]
mod protocol;

use anyhow::{anyhow, ensure, Result};
use axum::{extract::Path, http::StatusCode, Json};
use parking_lot::Mutex;
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Notify, Semaphore};

static SESSIONS: LazyLock<Mutex<HashMap<u64, Arc<State>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

struct State {
    written: AtomicUsize,
    pulses: AtomicUsize,
    verified: AtomicBool,
    commands: mpsc::Sender<u8>,
    drain: Notify,
    close: Notify,
}

#[derive(serde::Serialize)]
pub struct Observation {
    written: usize,
    pulses: usize,
    verified: bool,
}

pub async fn observe(Path(id): Path<u64>) -> Result<Json<Observation>, StatusCode> {
    let sessions = SESSIONS.lock();
    let state = sessions.get(&id).ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(Observation {
        written: state.written.load(Ordering::SeqCst),
        pulses: state.pulses.load(Ordering::SeqCst),
        verified: state.verified.load(Ordering::SeqCst),
    }))
}

pub async fn command(Path((id, action)): Path<(u64, String)>) -> Result<StatusCode, StatusCode> {
    let state = SESSIONS.lock().get(&id).cloned().ok_or(StatusCode::NOT_FOUND)?;
    match action.as_str() {
        "pulse" => state.commands.try_send(b'p').map_err(|_| StatusCode::CONFLICT)?,
        "finish" => state.commands.try_send(b'F').map_err(|_| StatusCode::CONFLICT)?,
        "drain" => state.drain.notify_one(),
        "close" => state.close.notify_one(),
        _ => return Err(StatusCode::BAD_REQUEST),
    }
    Ok(StatusCode::NO_CONTENT)
}

struct Registration(u64);
impl Drop for Registration {
    fn drop(&mut self) {
        SESSIONS.lock().remove(&self.0);
    }
}

pub async fn start(port: u16, identity: Option<(String, String)>) -> Result<()> {
    let tls = if let Some((cert, key)) = identity {
        let config = rustls::ServerConfig::builder().with_no_client_auth().with_single_cert(
            CertificateDer::pem_file_iter(cert)?.collect::<std::result::Result<Vec<_>, _>>()?,
            PrivateKeyDer::from_pem_file(key)?,
        )?;
        Some(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
    } else {
        None
    };
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    let slots = Arc::new(Semaphore::new(16));
    loop {
        let (socket, _) = listener.accept().await?;
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            continue;
        };
        socket.set_nodelay(true)?;
        let tls = tls.clone();
        tokio::spawn(async move {
            let _permit = permit;
            // Includes TLS handshake, registration, all I/O and cleanup. No detached pumps.
            let outcome = tokio::time::timeout(Duration::from_secs(60), async {
                if let Some(tls) = tls {
                    serve(tls.accept(socket).await?).await
                } else {
                    serve(socket).await
                }
            })
            .await;
            if !matches!(outcome, Ok(Ok(()))) {
                tracing::debug!(?outcome, "L4 fixture connection ended");
            }
        });
    }
}

async fn serve<S: AsyncRead + AsyncWrite + Unpin>(mut stream: S) -> Result<()> {
    let id = stream.read_u64().await?;
    let mode = stream.read_u8().await?;
    ensure!(mode <= 1, "unknown L4 fixture mode");
    let (commands, mut receiver) = mpsc::channel(8);
    let state = Arc::new(State {
        written: AtomicUsize::new(0),
        pulses: AtomicUsize::new(0),
        verified: AtomicBool::new(false),
        commands,
        drain: Notify::new(),
        close: Notify::new(),
    });
    {
        let mut sessions = SESSIONS.lock();
        ensure!(
            sessions.len() < 32 && !sessions.contains_key(&id),
            "fixture capacity or duplicate id"
        );
        sessions.insert(id, state.clone());
    }
    let _registration = Registration(id);
    stream.write_all(b"R").await?;
    stream.flush().await?;
    let (mut reader, mut writer) = tokio::io::split(stream);
    let exchange = async {
        if mode == 0 {
            let read = async {
                state.drain.notified().await;
                protocol::receive_payload(&mut reader).await?;
                state.verified.store(true, Ordering::SeqCst);
                Ok::<_, anyhow::Error>(())
            };
            let write = async {
                while let Some(byte) = receiver.recv().await {
                    writer.write_all(&[byte]).await?;
                    writer.flush().await?;
                    if byte == b'F' {
                        return Ok::<_, anyhow::Error>(());
                    }
                }
                Err(anyhow!("fixture command channel closed"))
            };
            tokio::try_join!(read, write)?;
        } else {
            let read = async {
                loop {
                    match reader.read_u8().await? {
                        b'p' => {
                            state.pulses.fetch_add(1, Ordering::SeqCst);
                        }
                        b'F' => return Ok::<_, anyhow::Error>(()),
                        _ => return Err(anyhow!("unexpected reverse byte")),
                    }
                }
            };
            let write = async {
                protocol::send_payload(&mut writer, &state.written).await?;
                state.verified.store(true, Ordering::SeqCst);
                Ok::<_, anyhow::Error>(())
            };
            tokio::try_join!(read, write)?;
            writer.write_all(b"F").await?;
            writer.flush().await?;
        }
        Ok(())
    };
    tokio::select! {
        result = exchange => result,
        _ = state.close.notified() => Ok(()),
    }
}
