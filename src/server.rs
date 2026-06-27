//! Server orchestration: load config, build state, start tunnel listener,
//! stats sampler and the web server (HTTP or HTTPS).

use crate::config::Config;
use crate::state::{AppState, AuditLogStore, AuthStore, ClientStore, Stats, WsBroadcaster};
use crate::tunnel::{self, TunnelAcceptor};
use crate::util;
use crate::web;
use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::RwLock;

pub async fn run(config_path: PathBuf) -> Result<()> {
    let mut config = Config::load_or_create(&config_path)?;
    std::fs::create_dir_all(&config.server.data_dir).ok();

    // Ensure a JWT signing secret exists.
    if config.auth.jwt_secret.is_empty() {
        let secret_path = config.server.data_dir.join("jwt_secret");
        config.auth.jwt_secret = if secret_path.exists() {
            std::fs::read_to_string(&secret_path)?.trim().to_string()
        } else {
            let s = util::gen_jwt_secret();
            std::fs::write(&secret_path, &s)?;
            s
        };
    }

    // Ensure a self-signed certificate if any TLS is enabled.
    let any_tls = config.server.tunnel_tls || config.server.web_tls;
    if any_tls {
        util::ensure_self_signed_cert(
            &config.server.cert_path,
            &config.server.key_path,
            &config.server.cert_san,
        )?;
    }

    let state = AppState {
        config: Arc::new(config.clone()),
        clients: Arc::new(RwLock::new(ClientStore::load(
            config.server.data_dir.join("clients.json"),
        )?)),
        sessions: Arc::new(RwLock::new(HashMap::new())),
        stats: Arc::new(Stats::default()),
        logs: Arc::new(AuditLogStore::new(500)),
        auth: Arc::new(AuthStore::load_or_seed(
            config.server.data_dir.join("auth.json"),
            &config.auth.admin_username,
            &config.auth.admin_password,
        )?),
        ws: Arc::new(WsBroadcaster::new()),
    };

    state
        .log("info", "system", format!("MiniMask server starting (data_dir={})", config.server.data_dir.display()))
        .await;

    // Tunnel listener.
    let tunnel_acceptor = if config.server.tunnel_tls {
        let acc = util::build_tls_acceptor(&config.server.cert_path, &config.server.key_path)?;
        TunnelAcceptor { tls: Some(acc) }
    } else {
        TunnelAcceptor { tls: None }
    };
    {
        let st = state.clone();
        tokio::spawn(async move {
            tunnel::run_tunnel_listener(st, tunnel_acceptor).await;
        });
    }

    // Stats sampler + WebSocket broadcaster (1Hz).
    {
        let st = state.clone();
        tokio::spawn(async move {
            stats_sampler(st).await;
        });
    }

    // Web server.
    let app = web::build_app(state.clone());
    let web_bind = config.server.web_bind.clone();
    let listener = TcpListener::bind(&web_bind).await?;
    if config.server.web_tls {
        tracing::info!("web UI on https://{web_bind}");
        let acceptor = util::build_tls_acceptor(&config.server.cert_path, &config.server.key_path)?;
        serve_https(listener, acceptor, app).await?;
    } else {
        tracing::info!("web UI on http://{web_bind}");
        axum::serve(listener, app).await?;
    }
    Ok(())
}

async fn stats_sampler(state: AppState) {
    let mut prev_in = state.stats.bytes_in.load(Ordering::Relaxed);
    let mut prev_out = state.stats.bytes_out.load(Ordering::Relaxed);
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    loop {
        interval.tick().await;
        let cur_in = state.stats.bytes_in.load(Ordering::Relaxed);
        let cur_out = state.stats.bytes_out.load(Ordering::Relaxed);
        let rate_in = cur_in.saturating_sub(prev_in);
        let rate_out = cur_out.saturating_sub(prev_out);
        prev_in = cur_in;
        prev_out = cur_out;
        state.stats.record_sample(rate_in, rate_out).await;
        let msg = crate::web::ws::build_stats_message(&state).await;
        state.ws.broadcast(msg).await;
    }
}

/// Serve the Axum app over HTTPS (manual TLS accept + hyper-util auto builder).
async fn serve_https(
    listener: TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
    app: axum::Router,
) -> Result<()> {
    use hyper::service::service_fn;
    use hyper_util::rt::{TokioExecutor, TokioIo};
    use tower::ServiceExt;

    loop {
        let (tcp, _addr) = match listener.accept().await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("web accept error: {e}");
                continue;
            }
        };
        let acc = acceptor.clone();
        let app = app.clone();
        tokio::spawn(async move {
            let tls = match acc.accept(tcp).await {
                Ok(t) => t,
                Err(e) => {
                    tracing::debug!("tls handshake failed: {e}");
                    return;
                }
            };
            let io = TokioIo::new(tls);
            let svc = service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                let app = app.clone();
                async move {
                    let req = req.map(axum::body::Body::new);
                    app.oneshot(req).await
                }
            });
            if let Err(e) = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                .serve_connection(io, svc)
                .await
            {
                tracing::debug!("https connection ended: {e}");
            }
        });
    }
}
