//! Yamux session driver (server side).
//!
//! A single task owns the `Connection` and continuously polls
//! `poll_next_inbound` to drive I/O for all open streams. Public-port listeners
//! request new outbound streams through an mpsc channel; the driver opens them
//! via `poll_new_outbound` and spawns a proxy task per connection.

use crate::state::{AppState, ProxyRequest};
use crate::tunnel::proxy;
use serde_json::json;
use std::future::poll_fn;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Drive a server-side yamux connection until it closes.
///
/// `listeners` are the public-port accept-loop tasks started for this client;
/// they are aborted when the session ends so the exposed ports stop accepting.
pub async fn drive_server<S>(
    mut conn: yamux::Connection<S>,
    mut proxy_rx: mpsc::Receiver<ProxyRequest>,
    client_id: String,
    session_id: String,
    state: AppState,
    active_conns: Arc<AtomicU64>,
) where
    S: futures_util::io::AsyncRead + futures_util::io::AsyncWrite + Unpin + Send,
{
    loop {
        // Poll inbound streams (also pumps data for existing streams).
        let inbound = poll_fn(|cx| conn.poll_next_inbound(cx));
        tokio::select! {
            biased;
            res = inbound => match res {
                Some(Ok(_stream)) => {
                    // The server never expects the client to open streams; drop it.
                    tracing::debug!("tunnel: unexpected inbound stream from {client_id}, closing");
                }
                Some(Err(e)) => {
                    tracing::warn!("tunnel: yamux error on {client_id}: {e}");
                    break;
                }
                None => {
                    tracing::info!("tunnel: client {client_id} closed the connection");
                    break;
                }
            },
            req = proxy_rx.recv() => {
                let Some(req) = req else { break };
                match poll_fn(|cx| conn.poll_new_outbound(cx)).await {
                    Ok(stream) => {
                        let stats = state.stats.clone();
                        let ac = active_conns.clone();
                        let ProxyRequest { target, conn } = req;
                        tokio::spawn(async move {
                            ac.fetch_add(1, Ordering::Relaxed);
                            stats.total_conns.fetch_add(1, Ordering::Relaxed);
                            stats.active_conns.fetch_add(1, Ordering::Relaxed);
                            proxy::proxy_connection(stream, conn, &target, stats.clone()).await;
                            ac.fetch_sub(1, Ordering::Relaxed);
                            stats.active_conns.fetch_sub(1, Ordering::Relaxed);
                        });
                    }
                    Err(e) => {
                        tracing::warn!("tunnel: open stream to {client_id} failed: {e}; closing");
                        // req.conn is dropped here -> visitor connection closed
                        break;
                    }
                }
            }
        }
    }

    // --- cleanup on disconnect ---
    // Public-port listeners self-terminate once the proxy channel closes
    // (proxy_rx is dropped when this function returns), so we only remove the
    // session entry if it still belongs to this connection (a reconnect may
    // have replaced it with a newer session_id).
    {
        let mut sessions = state.sessions.write().await;
        if let Some(info) = sessions.get(&client_id) {
            if info.session_id == session_id {
                sessions.remove(&client_id);
            }
        }
    }
    state
        .log("info", "tunnel", format!("client {client_id} disconnected"))
        .await;
    state
        .ws
        .broadcast(json!({ "type": "session_change" }).to_string())
        .await;
}
