//! Companion tunnel client. Connects to the server, authenticates with a
//! client token, and bridges inbound yamux streams to local services. This
//! makes the whole system testable end-to-end from a single binary.

use crate::tunnel::protocol;
use crate::util;
use anyhow::{anyhow, Result};
use clap::Args;
use std::future::poll_fn;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_util::compat::{FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};

#[derive(Args, Debug, Clone)]
pub struct ClientArgs {
    /// Server address, e.g. 127.0.0.1:7443
    #[arg(long)]
    pub server: String,
    /// Client id (created in the Web UI)
    #[arg(long)]
    pub id: String,
    /// Client token (shown once when the client is created)
    #[arg(long)]
    pub token: String,
    /// Connect to the server over TLS
    #[arg(long)]
    pub tls: bool,
    /// SNI / server name used for TLS (must match a SAN in the server cert)
    #[arg(long, default_value = "localhost")]
    pub server_name: String,
}

pub async fn run(args: ClientArgs) -> Result<()> {
    tracing::info!(
        "MiniMask client connecting to {} (tls={}) as {}",
        args.server,
        args.tls,
        args.id
    );
    loop {
        match run_once(&args).await {
            Ok(()) => tracing::info!("tunnel closed; reconnecting in 3s"),
            Err(e) => tracing::warn!("tunnel error: {e}; reconnecting in 3s"),
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

async fn run_once(args: &ClientArgs) -> Result<()> {
    let stream = TcpStream::connect(&args.server).await?;
    let mut boxed: Box<dyn crate::util::AsyncStream + Send + Unpin> = if args.tls {
        let connector = util::build_dangerous_tls_connector()?;
        let server_name = rustls::pki_types::ServerName::try_from(args.server_name.clone())
            .map_err(|e| anyhow!("invalid server name '{}': {e}", args.server_name))?;
        Box::new(connector.connect(server_name, stream).await?)
    } else {
        Box::new(stream)
    };

    protocol::write_handshake(&mut boxed, &args.id, &args.token).await?;
    let (ok, msg) = protocol::read_status(&mut boxed).await?;
    if !ok {
        return Err(anyhow!("server rejected: {msg}"));
    }
    tracing::info!("authenticated; tunnel established");

    let conn = yamux::Connection::new(boxed.compat(), yamux::Config::default(), yamux::Mode::Client);
    run_client_session(conn).await;
    Ok(())
}

async fn run_client_session<S>(mut conn: yamux::Connection<S>)
where
    S: futures_util::io::AsyncRead + futures_util::io::AsyncWrite + Unpin + Send,
{
    loop {
        match poll_fn(|cx| conn.poll_next_inbound(cx)).await {
            Some(Ok(stream)) => {
                tokio::spawn(async move {
                    if let Err(e) = handle_client_stream(stream).await {
                        tracing::warn!("proxied stream ended: {e}");
                    }
                });
            }
            Some(Err(e)) => {
                tracing::warn!("yamux error: {e}");
                break;
            }
            None => {
                tracing::info!("server closed the tunnel");
                break;
            }
        }
    }
}

async fn handle_client_stream(stream: yamux::Stream) -> Result<()> {
    let mut stream = stream.compat();
    let target = protocol::read_target(&mut stream).await?;
    tracing::info!("tunnel -> {target}");
    let mut local = TcpStream::connect(&target).await?;
    let _ = tokio::io::copy_bidirectional(&mut stream, &mut local).await;
    Ok(())
}
