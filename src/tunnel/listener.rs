//! Tunnel listener: accepts tunnel clients (optionally over TLS), performs the
//! handshake, and for each connected client starts public-port listeners that
//! forward incoming traffic through the client's yamux session.

use crate::state::{AppState, PortMapping, ProxyRequest, SessionInfo};
use crate::tunnel::{protocol, session};
use anyhow::{anyhow, Result};
use chrono::Utc;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_util::compat::TokioAsyncReadCompatExt;

pub struct TunnelAcceptor {
    pub tls: Option<tokio_rustls::TlsAcceptor>,
}

pub async fn run_tunnel_listener(state: AppState, acceptor: TunnelAcceptor) {
    let bind = state.config.server.tunnel_bind.clone();
    let listener = match TcpListener::bind(&bind).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("failed to bind tunnel listener {bind}: {e}");
            return;
        }
    };
    tracing::info!("tunnel listener on {bind} (tls={})", acceptor.tls.is_some());
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                let state = state.clone();
                let tls = acceptor.tls.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_tunnel(stream, addr, state, tls).await {
                        tracing::debug!("tunnel connection from {addr} ended: {e}");
                    }
                });
            }
            Err(e) => {
                tracing::warn!("tunnel accept error: {e}");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

async fn handle_tunnel(
    stream: TcpStream,
    addr: std::net::SocketAddr,
    state: AppState,
    tls: Option<tokio_rustls::TlsAcceptor>,
) -> Result<()> {
    // Unify TLS/plain into a single boxed tokio I/O object.
    let mut boxed: Box<dyn crate::util::AsyncStream + Send + Unpin> =
        match tls {
            Some(acc) => Box::new(acc.accept(stream).await?),
            None => Box::new(stream),
        };

    // --- handshake ---
    let (_client_id, token) = protocol::read_handshake(&mut boxed).await?;
    let verified = state.clients.read().await.verify_token(&token);
    let client_id = match verified {
        Some((id, _enabled)) => id,
        None => {
            let _ = protocol::write_status(&mut boxed, false, "invalid token").await;
            state.log("warn", "tunnel", format!("rejected auth from {addr}")).await;
            return Err(anyhow!("invalid client token from {addr}"));
        }
    };

    // Enforce max concurrent clients.
    {
        let sessions = state.sessions.read().await;
        if sessions.len() >= state.config.security.max_clients {
            let _ = protocol::write_status(&mut boxed, false, "max clients reached").await;
            state
                .log("warn", "tunnel", format!("max clients reached, rejecting {client_id}"))
                .await;
            return Err(anyhow!("max clients reached"));
        }
    }

    protocol::write_status(&mut boxed, true, "ok").await?;

    // --- start yamux session ---
    let conn = yamux::Connection::new(boxed.compat(), yamux::Config::default(), yamux::Mode::Server);

    let (proxy_tx, proxy_rx) = mpsc::channel::<ProxyRequest>(256);
    let active_conns = Arc::new(AtomicU64::new(0));
    let session_id = uuid::Uuid::new_v4().to_string();
    let listeners = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));

    {
        let mut sessions = state.sessions.write().await;
        sessions.insert(
            client_id.clone(),
            SessionInfo {
                client_id: client_id.clone(),
                session_id: session_id.clone(),
                remote_addr: addr.to_string(),
                connected_at: Utc::now(),
                active_conns: active_conns.clone(),
                proxy_tx: proxy_tx.clone(),
                listeners: listeners.clone(),
            },
        );
    }
    state.log("info", "tunnel", format!("client {client_id} connected from {addr}")).await;
    state.ws.broadcast(json!({ "type": "session_change" }).to_string()).await;

    // Start public listeners for already-enabled mappings.
    start_client_listeners(&state, &client_id).await;

    session::drive_server(conn, proxy_rx, client_id, session_id, state, active_conns).await;
    Ok(())
}

/// Start a public listener for every currently-enabled mapping of a connected
/// client. Called right after the session is registered, and when a client is
/// re-enabled.
pub async fn start_client_listeners(state: &AppState, client_id: &str) {
    let mappings: Vec<PortMapping> = {
        let clients = state.clients.read().await;
        let Some(client) = clients.find(client_id) else {
            return;
        };
        client.mappings.iter().filter(|m| m.enabled).cloned().collect()
    };
    for m in mappings {
        add_listener(state, client_id, &m).await;
    }
}

/// Bind a public port and forward it through the client's session. Returns
/// `false` if the client is offline (the listener will start on connect) or the
/// port could not be bound. Used for hot-reload of mappings.
pub async fn add_listener(state: &AppState, client_id: &str, mapping: &PortMapping) -> bool {
    let (proxy_tx, listeners, active_conns, max_conns) = {
        let sessions = state.sessions.read().await;
        let Some(info) = sessions.get(client_id) else {
            return false;
        };
        (
            info.proxy_tx.clone(),
            info.listeners.clone(),
            info.active_conns.clone(),
            state.config.security.max_conns_per_client,
        )
    };
    let bind_addr = format!("0.0.0.0:{}", mapping.remote_port);
    let listener = match TcpListener::bind(&bind_addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!("failed to bind public port :{}: {e}", mapping.remote_port);
            return false;
        }
    };
    tracing::info!(
        "public port :{} -> client {client_id} local {}",
        mapping.remote_port,
        mapping.local_addr
    );
    let cid = client_id.to_string();
    let local_addr = mapping.local_addr.clone();
    let handle = tokio::spawn(async move {
        public_accept_loop(listener, proxy_tx, local_addr, cid, active_conns, max_conns).await;
    });
    listeners.lock().await.insert(mapping.id.clone(), handle);
    true
}

/// Stop the listener for a mapping (by id) across all sessions. The mapping id
/// is globally unique, so only one session can hold it.
pub async fn remove_listener(state: &AppState, mapping_id: &str) {
    let listener_arcs: Vec<_> = {
        let sessions = state.sessions.read().await;
        sessions.values().map(|i| i.listeners.clone()).collect()
    };
    for listeners in listener_arcs {
        if let Some(handle) = listeners.lock().await.remove(mapping_id) {
            handle.abort();
            tracing::info!("stopped listener for mapping {mapping_id}");
            return;
        }
    }
}

/// Stop all listeners for a client (e.g. when the client is disabled/deleted).
pub async fn remove_client_listeners(state: &AppState, client_id: &str) {
    let listeners = {
        let sessions = state.sessions.read().await;
        sessions.get(client_id).map(|i| i.listeners.clone())
    };
    if let Some(listeners) = listeners {
        let mut map = listeners.lock().await;
        for (_, handle) in map.drain() {
            handle.abort();
        }
    }
}

async fn public_accept_loop(
    listener: TcpListener,
    tx: mpsc::Sender<ProxyRequest>,
    local_addr: String,
    client_id: String,
    active_conns: Arc<AtomicU64>,
    max_conns: usize,
) {
    loop {
        match listener.accept().await {
            Ok((conn, peer)) => {
                if max_conns != 0 && active_conns.load(Ordering::Relaxed) >= max_conns as u64 {
                    tracing::warn!("client {client_id} at conn limit, dropping {peer}");
                    continue;
                }
                let req = ProxyRequest { target: local_addr.clone(), conn };
                match tx.try_send(req) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        tracing::debug!("proxy channel full for {client_id}, dropping {peer}");
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        tracing::info!("proxy channel closed for {client_id}; stopping listener");
                        break;
                    }
                }
            }
            Err(e) => {
                tracing::warn!("public accept error: {e}");
                break;
            }
        }
    }
}
