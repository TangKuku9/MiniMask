//! Wire protocol for the tunnel: a length-prefixed handshake followed by a yamux
//! session. On each yamux stream the server sends a "connect" message carrying
//! the local address the client should dial, then bytes are copied both ways.

use anyhow::{anyhow, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const MAGIC: &[u8; 4] = b"MMSK";
pub const PROTO_VERSION: u8 = 1;
pub const STATUS_OK: u8 = 1;
pub const STATUS_FAIL: u8 = 0;

/// Client -> Server: announce itself.
pub async fn write_handshake<W>(w: &mut W, client_id: &str, token: &str) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    w.write_all(MAGIC).await?;
    w.write_all(&[PROTO_VERSION]).await?;
    write_lp_str(w, client_id).await?;
    write_lp_str(w, token).await?;
    w.flush().await?;
    Ok(())
}

/// Server side: read the client announcement.
pub async fn read_handshake<R>(r: &mut R) -> Result<(String, String)>
where
    R: AsyncReadExt + Unpin,
{
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic).await?;
    if &magic != MAGIC {
        return Err(anyhow!("invalid magic bytes"));
    }
    let mut ver = [0u8; 1];
    r.read_exact(&mut ver).await?;
    if ver[0] != PROTO_VERSION {
        return Err(anyhow!("unsupported protocol version {}", ver[0]));
    }
    let client_id = read_lp_str(r).await?;
    let token = read_lp_str(r).await?;
    Ok((client_id, token))
}

/// Server -> Client: accept or reject the handshake.
pub async fn write_status<W>(w: &mut W, ok: bool, msg: &str) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    w.write_all(&[if ok { STATUS_OK } else { STATUS_FAIL }])
        .await?;
    write_lp_str(w, msg).await?;
    w.flush().await?;
    Ok(())
}

pub async fn read_status<R>(r: &mut R) -> Result<(bool, String)>
where
    R: AsyncReadExt + Unpin,
{
    let mut s = [0u8; 1];
    r.read_exact(&mut s).await?;
    let msg = read_lp_str(r).await?;
    Ok((s[0] == STATUS_OK, msg))
}

/// Server -> Client (on a fresh yamux stream): the target address to dial.
pub async fn write_target<W>(w: &mut W, target: &str) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    write_lp_str(w, target).await?;
    w.flush().await?;
    Ok(())
}

pub async fn read_target<R>(r: &mut R) -> Result<String>
where
    R: AsyncReadExt + Unpin,
{
    read_lp_str(r).await
}

async fn write_lp_str<W>(w: &mut W, s: &str) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let b = s.as_bytes();
    if b.len() > u16::MAX as usize {
        return Err(anyhow!("string too long ({} bytes)", b.len()));
    }
    let len = b.len() as u16;
    w.write_all(&len.to_be_bytes()).await?;
    w.write_all(b).await?;
    Ok(())
}

async fn read_lp_str<R>(r: &mut R) -> Result<String>
where
    R: AsyncReadExt + Unpin,
{
    let mut len_b = [0u8; 2];
    r.read_exact(&mut len_b).await?;
    let len = u16::from_be_bytes(len_b) as usize;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    String::from_utf8(buf).map_err(|e| anyhow!("invalid utf8: {e}"))
}
