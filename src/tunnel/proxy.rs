//! Bidirectional proxy engine with live byte counting (server side).

use crate::state::Stats;
use crate::tunnel::protocol;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::compat::FuturesAsyncReadCompatExt;

/// Proxy a visitor's TCP connection through a yamux `Stream` toward the tunneled
/// client. The `target` (local address on the client side) is sent first so the
/// client knows what to dial. Bytes are counted live into the shared stats.
pub async fn proxy_connection(
    stream: yamux::Stream,
    conn: tokio::net::TcpStream,
    target: &str,
    stats: Arc<Stats>,
) {
    // yamux::Stream implements futures::io::{AsyncRead, AsyncWrite}; convert to
    // tokio traits so we can split & copy with the visitor's TcpStream.
    let mut stream = stream.compat();

    if let Err(e) = protocol::write_target(&mut stream, target).await {
        tracing::warn!("tunnel: write target {target} failed: {e}");
        return;
    }

    let (mut s_read, mut s_write) = tokio::io::split(stream);
    let (mut c_read, mut c_write) = tokio::io::split(conn);

    let out = &stats.bytes_out; // visitor -> client
    let inc = &stats.bytes_in; // client -> visitor

    // Stop as soon as either direction ends; dropping the futures closes both
    // halves and therefore the connection.
    tokio::select! {
        res = copy_count(&mut c_read, &mut s_write, out) => {
            if let Err(e) = res { tracing::debug!("proxy copy out ended: {e}"); }
        }
        res = copy_count(&mut s_read, &mut c_write, inc) => {
            if let Err(e) = res { tracing::debug!("proxy copy in ended: {e}"); }
        }
    }
}

async fn copy_count<R, W>(
    r: &mut R,
    w: &mut W,
    counter: &AtomicU64,
) -> std::io::Result<u64>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    let mut buf = [0u8; 16384];
    let mut total = 0u64;
    loop {
        let n = r.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        w.write_all(&buf[..n]).await?;
        w.flush().await?;
        counter.fetch_add(n as u64, Ordering::Relaxed);
        total += n as u64;
    }
    Ok(total)
}
