//! Real TCP/TLS backpressure coverage through the canonical shared gateway.

#[path = "../../server/l4_duplex_protocol.rs"]
mod protocol;

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::port_config::{PortConfig, PortKind};
use anyhow::{anyhow, ensure, Result};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{sleep, timeout, Instant};

pub struct L4DuplexTestSuite;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug)]
enum Transport {
    Tcp,
    Terminate,
    Passthrough,
}

struct Control {
    client: reqwest::Client,
    url: String,
}

impl Control {
    async fn command(&self, command: &str) -> Result<()> {
        self.client
            .post(format!("{}/{command}", self.url))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }
    async fn state(&self) -> Result<Value> {
        Ok(self
            .client
            .get(&self.url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }
}

struct LogProbe {
    path: PathBuf,
    offset: usize,
    listener: u16,
    peer: u16,
    tcp: bool,
    connected: Instant,
}

impl LogProbe {
    async fn entries(&self) -> Result<Vec<Value>> {
        let bytes = tokio::fs::read(&self.path).await?;
        ensure!(bytes.len() >= self.offset, "connection log truncated during test");
        let mut entries = Vec::new();
        // Only complete JSON lines are eligible; the logger may be appending.
        for line in bytes[self.offset..].split_inclusive(|byte| *byte == b'\n') {
            if !line.ends_with(b"\n") {
                continue;
            }
            let entry: Value = serde_json::from_slice(line)?;
            if entry["listener_port"].as_u64() == Some(self.listener as u64)
                && entry["direct_peer_port"].as_u64() == Some(self.peer as u64)
            {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    async fn idle_closed(&self, stalled: Instant) -> Result<()> {
        timeout(Duration::from_secs(8), async {
            loop {
                match self.entries().await {
                    Ok(entries) if !entries.is_empty() => {
                        ensure!(
                            stalled.elapsed() >= Duration::from_millis(1500),
                            "idle closure was premature"
                        );
                        ensure!(entries.len() == 1, "duplicate connection close logs: {entries:?}");
                        let duration_ms = if self.tcp {
                            entries[0]["duration_ms"].as_u64()
                        } else {
                            entries[0]["end_time"]
                                .as_u64()
                                .zip(entries[0]["ts"].as_u64())
                                .and_then(|(end, start)| end.checked_sub(start))
                        }
                        .ok_or_else(|| anyhow!("missing connection duration: {:?}", entries[0]))?;
                        let minimum_ms = stalled.duration_since(self.connected).as_millis() as u64 + 1500;
                        ensure!(
                            duration_ms >= minimum_ms,
                            "premature idle closure: {duration_ms}ms < {minimum_ms}ms"
                        );
                        let (key, expected) = if self.tcp {
                            ("status", "IdleTimeout")
                        } else {
                            ("err_log", "idle timeout")
                        };
                        ensure!(
                            entries[0][key].as_str() == Some(expected),
                            "unexpected close diagnostic: {:?}",
                            entries[0]
                        );
                        return Ok::<_, anyhow::Error>(());
                    }
                    Ok(_) => {}
                    Err(error) if !self.path.exists() => {
                        let _ = error;
                    }
                    Err(error) => return Err(error),
                }
                sleep(Duration::from_millis(50)).await;
            }
        })
        .await??;
        // The byte-stream closure is checked separately; also allow pending log batches to arrive.
        sleep(Duration::from_millis(300)).await;
        ensure!(self.entries().await?.len() == 1, "duplicate idle-close record");
        Ok(())
    }
}

async fn closed<R: AsyncRead + Unpin>(reader: &mut R) -> Result<()> {
    timeout(Duration::from_secs(5), async {
        let mut bytes = 0;
        let mut buf = [0u8; protocol::CHUNK];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) | Err(_) => return Ok::<_, anyhow::Error>(()),
                Ok(n) => {
                    bytes += n;
                    ensure!(bytes <= protocol::PAYLOAD, "unbounded bytes after idle closure");
                }
            }
        }
    })
    .await?
}

async fn wait_upload_stall(count: &AtomicUsize) -> Result<usize> {
    timeout(Duration::from_secs(5), async {
        let mut last = count.load(Ordering::SeqCst);
        let mut changed = Instant::now();
        loop {
            sleep(Duration::from_millis(25)).await;
            let now = count.load(Ordering::SeqCst);
            ensure!(
                now < protocol::PAYLOAD,
                "upload fit entirely in buffers; backpressure not established"
            );
            if now != last {
                last = now;
                changed = Instant::now();
            }
            if last > 0 && changed.elapsed() >= Duration::from_millis(250) {
                return Ok(last);
            }
        }
    })
    .await?
}

async fn wait_download_stall(control: &Control) -> Result<usize> {
    timeout(Duration::from_secs(5), async {
        let mut last = 0;
        let mut changed = Instant::now();
        loop {
            let now = control.state().await?["written"]
                .as_u64()
                .ok_or_else(|| anyhow!("missing write count"))? as usize;
            ensure!(
                now < protocol::PAYLOAD,
                "download fit entirely in buffers; backpressure not established"
            );
            if now != last {
                last = now;
                changed = Instant::now();
            }
            if last > 0 && changed.elapsed() >= Duration::from_millis(250) {
                return Ok(last);
            }
            sleep(Duration::from_millis(25)).await;
        }
    })
    .await?
}

async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    control: &Control,
    probe: &LogProbe,
    id: u64,
    download: bool,
    idle: bool,
) -> Result<()> {
    stream.write_u64(id).await?;
    stream.write_u8(u8::from(download)).await?;
    stream.flush().await?;
    protocol::expect_byte(&mut stream, b'R').await?;
    let (mut reader, mut writer) = tokio::io::split(stream);
    if download {
        let mut before = wait_download_stall(control).await?;
        let stalled = Instant::now();
        if idle {
            probe.idle_closed(stalled).await?;
            closed(&mut reader).await?;
        } else {
            timeout(Duration::from_secs(10), async {
                let mut stable_since = Instant::now();
                let mut pulse = 0u64;
                loop {
                    pulse += 1;
                    writer.write_all(b"p").await?;
                    writer.flush().await?;
                    timeout(Duration::from_secs(1), async {
                        loop {
                            if control.state().await?["pulses"].as_u64() == Some(pulse) {
                                return Ok::<_, anyhow::Error>(());
                            }
                            sleep(Duration::from_millis(20)).await;
                        }
                    })
                    .await??;
                    sleep(Duration::from_millis(400)).await;
                    let now = control.state().await?["written"]
                        .as_u64()
                        .ok_or_else(|| anyhow!("missing write count"))? as usize;
                    if sustained_stall(&mut before, &mut stable_since, now)? {
                        return Ok::<_, anyhow::Error>(());
                    }
                }
            })
            .await??;
            protocol::receive_payload(&mut reader).await?;
            writer.write_all(b"F").await?;
            writer.flush().await?;
            protocol::expect_byte(&mut reader, b'F').await?;
        }
    } else {
        let count = AtomicUsize::new(0);
        let send = async {
            let result = protocol::send_payload(&mut writer, &count).await;
            if idle {
                ensure!(result.is_err(), "stalled upload completed unexpectedly");
                std::future::pending::<Result<()>>().await
            } else {
                result.map_err(Into::into)
            }
        };
        let observe = async {
            let mut before = wait_upload_stall(&count).await?;
            let stalled = Instant::now();
            if idle {
                probe.idle_closed(stalled).await?;
                closed(&mut reader).await?;
            } else {
                timeout(Duration::from_secs(10), async {
                    let mut stable_since = Instant::now();
                    loop {
                        control.command("pulse").await?;
                        timeout(Duration::from_secs(1), protocol::expect_byte(&mut reader, b'p')).await??;
                        sleep(Duration::from_millis(400)).await;
                        if sustained_stall(&mut before, &mut stable_since, count.load(Ordering::SeqCst))? {
                            return Ok::<_, anyhow::Error>(());
                        }
                    }
                })
                .await??;
                control.command("drain").await?;
                timeout(Duration::from_secs(15), async {
                    loop {
                        if control.state().await?["verified"] == true {
                            return Ok::<_, anyhow::Error>(());
                        }
                        sleep(Duration::from_millis(20)).await;
                    }
                })
                .await??;
                control.command("finish").await?;
                protocol::expect_byte(&mut reader, b'F').await?;
            }
            Ok::<_, anyhow::Error>(())
        };
        if idle {
            tokio::select! { result = observe => result?, result = send => result? }
        } else {
            tokio::try_join!(send, observe)?;
        }
    }
    Ok(())
}

fn sustained_stall(last: &mut usize, since: &mut Instant, now: usize) -> Result<bool> {
    ensure!(
        now < protocol::PAYLOAD,
        "payload fit entirely in buffers; backpressure not established"
    );
    // TCP window updates may allow a final partial write after the first short
    // plateau. Require a full three-second plateau while reverse I/O continues.
    if now != *last {
        *last = now;
        *since = Instant::now();
    }
    Ok(since.elapsed() >= Duration::from_secs(3))
}

async fn run(ctx: &TestContext, transport: Transport, download: bool, idle: bool) -> Result<()> {
    let ports = PortConfig::load().map_err(anyhow::Error::msg)?;
    let (suite, kind) = match transport {
        Transport::Tcp => ("TCPRoute/DuplexBackpressure", PortKind::Tcp),
        Transport::Terminate => ("TCPRoute/DuplexBackpressure", PortKind::Tls),
        Transport::Passthrough => ("TCPRoute/DuplexBackpressure/Passthrough", PortKind::Tls),
    };
    let port = ports.require_port(suite, kind).map_err(anyhow::Error::msg)?;
    let tcp = TcpStream::connect((ctx.target_host.as_str(), port)).await?;
    tcp.set_nodelay(true)?;
    let path = if matches!(transport, Transport::Tcp) {
        ctx.access_log_path.with_file_name("tcp.log")
    } else {
        ctx.access_log_path.clone()
    };
    let mut probe = LogProbe {
        offset: match tokio::fs::metadata(&path).await {
            Ok(meta) => meta.len() as usize,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error.into()),
        },
        path,
        listener: port,
        peer: tcp.local_addr()?.port(),
        tcp: matches!(transport, Transport::Tcp),
        connected: Instant::now(),
    };
    let id = (u64::from(std::process::id()) << 32) | NEXT_ID.fetch_add(1, Ordering::SeqCst);
    let control = Control {
        client: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(2))
            .build()?,
        url: format!("http://{}:30001/l4-duplex/{id}", ctx.target_host),
    };
    let result = timeout(Duration::from_secs(40), async {
        match transport {
            Transport::Tcp => exchange(tcp, &control, &probe, id, download, idle).await,
            _ => {
                let connector = crate::suites::tls_route::basic::basic::make_tls_connector();
                let tls = connector
                    .connect(
                        rustls::pki_types::ServerName::try_from("duplex.sandbox.example.com")?,
                        tcp,
                    )
                    .await?;
                // Terminated TLS creates its gateway context after the handshake.
                probe.connected = Instant::now();
                exchange(tls, &control, &probe, id, download, idle).await
            }
        }
    })
    .await;
    // A failed/cancelled exchange must not leave a fixture waiting for a drain command.
    let response = control.client.post(format!("{}/close", control.url)).send().await?;
    ensure!(
        response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND,
        "fixture cleanup failed"
    );
    timeout(Duration::from_secs(3), async {
        loop {
            let response = control.client.get(&control.url).send().await?;
            if response.status() == reqwest::StatusCode::NOT_FOUND {
                return Ok::<_, anyhow::Error>(());
            }
            response.error_for_status()?;
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    result?
}

impl TestSuite for L4DuplexTestSuite {
    fn name(&self) -> &str {
        "TCPRoute/DuplexBackpressure"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let mut cases = Vec::new();
        for transport in [Transport::Tcp, Transport::Terminate, Transport::Passthrough] {
            for download in [false, true] {
                for idle in [false, true] {
                    cases.push(
                        TestCase::new(
                            format!(
                                "l4_{transport:?}_{}_{}",
                                if download { "download" } else { "upload" },
                                if idle { "idle_close" } else { "reverse_progress" }
                            ),
                            "Prove sustained backpressure, reverse progress and exact recovery, or unique idle closure",
                            move |ctx| {
                                Box::pin(async move {
                                    let start = Instant::now();
                                    match run(&ctx, transport, download, idle).await {
                                        Ok(()) => TestResult::passed_with_message(
                                            start.elapsed(),
                                            "L4 backpressure assertions passed".into(),
                                        ),
                                        Err(error) => TestResult::failed(start.elapsed(), format!("{error:#}")),
                                    }
                                })
                            },
                        )
                        .with_timeout(Duration::from_secs(50)),
                    );
                }
            }
        }
        cases
    }
}
