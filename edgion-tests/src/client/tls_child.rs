//! Direct-child lifetime ownership for the selected OpenSSL TLS test helpers.

use std::fmt;
use std::io;
use std::process::{Output, Stdio};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{ChildStdin, Command};
use tokio::sync::oneshot;
use tokio::time::{sleep_until, Instant};

pub(crate) const OPERATION_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub(crate) enum ChildError {
    Spawn(io::Error),
    Io(io::Error),
    Deadline,
    Terminate(io::Error),
    OwnerStopped,
}

impl ChildError {
    pub(crate) fn is_not_found(&self) -> bool {
        matches!(self, Self::Spawn(error) if error.kind() == io::ErrorKind::NotFound)
    }
}

impl fmt::Display for ChildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(error) => write!(formatter, "failed to spawn OpenSSL child: {error}"),
            Self::Io(error) => write!(formatter, "OpenSSL child input/output or wait failed: {error}"),
            Self::Deadline => formatter.write_str("OpenSSL operation deadline exceeded"),
            Self::Terminate(error) => write!(formatter, "failed to terminate OpenSSL child: {error}"),
            Self::OwnerStopped => formatter.write_str("OpenSSL child owner stopped before returning a result"),
        }
    }
}

impl std::error::Error for ChildError {}

struct Operation {
    result: oneshot::Receiver<Result<Output, ChildError>>,
    #[cfg(test)]
    reaped: oneshot::Receiver<io::Result<std::process::ExitStatus>>,
}

/// The requester owns only a receiver. Its cancellation cannot drop the Child
/// or cancel cleanup; the supervisor observes receiver closure instead.
pub(crate) async fn output(command: Command, input: Option<Vec<u8>>, deadline: Instant) -> Result<Output, ChildError> {
    start(command, input, deadline)
        .result
        .await
        .unwrap_or(Err(ChildError::OwnerStopped))
}

fn start(mut command: Command, input: Option<Vec<u8>>, deadline: Instant) -> Operation {
    let (mut sender, result) = oneshot::channel();
    #[cfg(test)]
    let (reaped_sender, reaped) = oneshot::channel();
    tokio::spawn(async move {
        if sender.is_closed() {
            return;
        }
        if Instant::now() >= deadline {
            let _ = sender.send(Err(ChildError::Deadline));
            return;
        }
        command
            .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let _ = sender.send(Err(ChildError::Spawn(error)));
                return;
            }
        };
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let result = {
            // All pipe handles live inside this scoped future. Drop it before
            // cleanup so neither backpressure nor a pending stdin write can
            // prevent termination. Child itself remains outside the future.
            let exchange = async {
                let ((), stdout, stderr, status) = tokio::try_join!(
                    write_input(stdin, input),
                    read_output(stdout),
                    read_output(stderr),
                    child.wait(),
                )?;
                Ok(Output { status, stdout, stderr })
            };
            tokio::select! {
                biased;
                _ = sender.closed() => None,
                _ = sleep_until(deadline) => Some(Err(ChildError::Deadline)),
                result = exchange => Some(result.map_err(ChildError::Io)),
            }
        };
        if let Some(Ok(output)) = result {
            #[cfg(test)]
            let _ = reaped_sender.send(Ok(output.status));
            let _ = sender.send(Ok(output));
            return;
        }

        // Report failure after signaling, without making the decision wait on
        // OS reaping. Even a failed send must leave this owner awaiting wait.
        let kill_result = child.start_kill();
        if let Some(Err(error)) = result {
            let error = kill_result.err().map(ChildError::Terminate).unwrap_or(error);
            let _ = sender.send(Err(error));
        }
        let wait_result = child.wait().await;
        #[cfg(test)]
        let _ = reaped_sender.send(wait_result);
        #[cfg(not(test))]
        let _ = wait_result;
    });
    Operation {
        result,
        #[cfg(test)]
        reaped,
    }
}

async fn write_input(stdin: Option<ChildStdin>, input: Option<Vec<u8>>) -> io::Result<()> {
    if let (Some(mut stdin), Some(input)) = (stdin, input) {
        stdin.write_all(&input).await?;
        stdin.shutdown().await?;
    }
    Ok(())
}

async fn read_output(stream: Option<impl AsyncRead + Unpin>) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    if let Some(mut stream) = stream {
        stream.read_to_end(&mut bytes).await?;
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "../../tests/unit/client/tls_child_test.rs"]
mod tests;
