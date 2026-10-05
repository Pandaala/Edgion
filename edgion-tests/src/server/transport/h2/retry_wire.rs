//! Test-only wire faults that h2::server cannot express: a GOAWAY excluding
//! the accepted request, and a real TCP reset after application processing.

use futures::FutureExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};

pub(super) enum Fault {
    Flush,
    Goaway { last_stream: u32 },
    TcpReset,
}

pub(super) struct Command {
    pub fault: Fault,
    pub completed: oneshot::Sender<Result<(), String>>,
}

pub(super) struct Bridge(tokio::task::JoinHandle<()>);

impl Bridge {
    pub async fn finish(mut self) -> Result<(), String> {
        tokio::time::timeout(std::time::Duration::from_secs(2), &mut self.0)
            .await
            .map_err(|_| "H2 wire drain timed out".to_string())?
            .map_err(|error| error.to_string())
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) fn bridge(tcp: TcpStream) -> (DuplexStream, mpsc::Sender<Command>, Bridge) {
    // Accommodate the maximum scripted response plus frame/header overhead.
    // The flush probes use small replies; this is not a flow-control oracle
    // for arbitrary responses larger than the script's hard body cap.
    let (h2_io, wire_io) = tokio::io::duplex(2 * super::super::script::MAX_RESP_LEN);
    let (commands, mut incoming) = mpsc::channel::<Command>(1);
    let task = tokio::spawn(async move {
        let (mut socket_read, mut socket_write) = tcp.into_split();
        let (mut wire_read, mut wire_write) = tokio::io::split(wire_io);
        let outgoing = async {
            let mut buffer = [0u8; 16_384];
            let mut commands_open = true;
            loop {
                tokio::select! {
                    command = incoming.recv(), if commands_open => {
                        let Some(command) = command else { commands_open = false; continue };
                        let result = match command.fault {
                            Fault::Flush => {
                                // The h2 codec has already flushed into the duplex.
                                // Drain those ready bytes before acknowledging TCP flush.
                                let result = async {
                                    while let Some(read) = wire_read.read(&mut buffer).now_or_never() {
                                        let count = read?;
                                        if count == 0 { break; }
                                        socket_write.write_all(&buffer[..count]).await?;
                                    }
                                    socket_write.flush().await
                                }.await;
                                let _ = command.completed.send(result.map_err(|error| error.to_string()));
                                continue;
                            }
                            Fault::Goaway { last_stream } => {
                                let mut frame = vec![0, 0, 8, 7, 0, 0, 0, 0, 0];
                                frame.extend_from_slice(&last_stream.to_be_bytes());
                                frame.extend_from_slice(&0u32.to_be_bytes());
                                socket_write.write_all(&frame).await
                            }
                            Fault::TcpReset => {
                                // Zero linger is an abortive close, never a blocking linger.
                                #[allow(deprecated)]
                                socket_write.as_ref().set_linger(Some(std::time::Duration::ZERO))
                            }
                        };
                        let _ = command.completed.send(result.map_err(|error| error.to_string()));
                        return;
                    }
                    read = wire_read.read(&mut buffer) => {
                        let Ok(count) = read else { return };
                        if count == 0 || socket_write.write_all(&buffer[..count]).await.is_err() {
                            return;
                        }
                    }
                }
            }
        };
        tokio::pin!(outgoing);
        tokio::select! {
            _ = tokio::io::copy(&mut socket_read, &mut wire_write) => {
                // A peer write-half close must not cancel our queued response.
                drop(wire_write);
                outgoing.await;
            },
            _ = &mut outgoing => {},
        }
        // Drop both socket halves and the h2-facing duplex writer together.
        // A fault cannot leave an orphan reader or an origin stream hanging.
    });
    (h2_io, commands, Bridge(task))
}
