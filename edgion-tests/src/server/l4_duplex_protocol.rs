//! Bounded wire protocol shared by the L4 fixture and its integration client.

use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const PAYLOAD: usize = 32 * 1024 * 1024;
pub const CHUNK: usize = 16 * 1024;

fn byte_at(offset: usize) -> u8 {
    (offset as u64).wrapping_mul(6364136223846793005).rotate_right(23) as u8
}

pub async fn send_payload<W: AsyncWrite + Unpin>(writer: &mut W, count: &AtomicUsize) -> io::Result<()> {
    let mut buf = [0u8; CHUNK];
    for offset in (0..PAYLOAD).step_by(CHUNK) {
        for (index, byte) in buf.iter_mut().enumerate() {
            *byte = byte_at(offset + index);
        }
        let mut written = 0;
        while written < buf.len() {
            let n = writer.write(&buf[written..]).await?;
            if n == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            written += n;
            count.fetch_add(n, Ordering::SeqCst);
        }
    }
    writer.write_all(b"E").await?;
    writer.flush().await
}

pub async fn receive_payload<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<()> {
    let mut buf = [0u8; CHUNK];
    for offset in (0..PAYLOAD).step_by(CHUNK) {
        reader.read_exact(&mut buf).await?;
        if buf
            .iter()
            .enumerate()
            .any(|(index, byte)| *byte != byte_at(offset + index))
        {
            return Err(io::Error::other(format!("payload mismatch at block {offset}")));
        }
    }
    expect_byte(reader, b'E').await
}

pub async fn expect_byte<R: AsyncRead + Unpin>(reader: &mut R, expected: u8) -> io::Result<()> {
    let actual = reader.read_u8().await?;
    if actual != expected {
        return Err(io::Error::other(format!("expected byte {expected}, got {actual}")));
    }
    Ok(())
}
