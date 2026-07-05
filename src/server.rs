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

    // P2-14: Ensure a token pepper exists. Mixed into client token hashes so a
    // leaked `clients.json` alone cannot be used to verify candidate tokens.
    // Stored separately from `clients.json` to raise the bar for exfiltration.
    let token_pepper = {
        let pepper_path = config.server.data_dir.join("token_pepper");
        if pepper_path.exists() {
            std::fs::read_to_string(&pepper_path)?.trim().to_string()
        } else {
            let s = util::gen_jwt_secret(); // 48 random bytes, URL-safe base64
            std::fs::write(&pepper_path, &s)?;
            s
        }
    };

    // Ensure a CA + server certificate if any TLS is enabled. The CA cert
    // (ca.pem) is later distributed to clients for CA pinning.
    let any_tls = config.server.tunnel_tls || config.server.web_tls;
    if any_tls {
        util::ensure_ca_and_server_cert(
            &config.server.ca_path,
            &config.server.ca_key_path,
            &config.server.cert_path,
            &config.server.key_path,
            &config.server.cert_san,
        )?;
        tracing::info!(
            "CA certificate at {} — distribute this to clients for CA pinning",
            config.server.ca_path.display()
        );
    }

    let state = AppState {
        config: Arc::new(config.clone()),
        clients: Arc::new(RwLock::new(ClientStore::load(
            config.server.data_dir.join("clients.json"),
        )?)),
        sessions: Arc::new(RwLock::new(HashMap::new())),
        stats: Arc::new(Stats::default()),
        logs: Arc::new({
            // P1-6: audit log with in-memory ring buffer + file persistence.
            let store = AuditLogStore::new(500);
            // Only enable file persistence if log_file is non-empty.
            if config.audit.log_file.as_os_str().is_empty() {
                store
            } else {
                store.with_file(config.audit.log_file.clone(), config.audit.max_size_mb)
            }
        }),
        auth: Arc::new(AuthStore::load_or_seed(
            config.server.data_dir.join("auth.json"),
            &config.auth.admin_username,
            &config.auth.admin_password,
        )?),
        ws: Arc::new(WsBroadcaster::new()),
        token_pepper: Arc::new(token_pepper),
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
    let tunnel_handle = {
        let st = state.clone();
        tokio::spawn(async move {
            tunnel::run_tunnel_listener(st, tunnel_acceptor).await;
        })
    };

    // Stats sampler + WebSocket broadcaster (1Hz).
    {
        let st = state.clone();
        tokio::spawn(async move {
            stats_sampler(st).await;
        });
    }

    // P1-10: Graceful shutdown signal — Ctrl+C or SIGTERM triggers shutdown.
    let shutdown_signal = async {
        let ctrl_c = tokio::signal::ctrl_c();
        tokio::pin!(ctrl_c);
        // Also listen for SIGTERM on Unix (no-op on Windows).
        #[cfg(unix)]
        {
            let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("install SIGTERM handler");
            tokio::select! {
                _ = &mut ctrl_c => {}
                _ = sigterm.recv() => {}
            }
        }
        #[cfg(not(unix))]
        {
            let _ = ctrl_c.await;
        }
        tracing::info!("收到停止信号，正在优雅关闭...");
    };

    // Web server.
    let app = web::build_app(state.clone());
    let web_bind = config.server.web_bind.clone();
    let listener = TcpListener::bind(&web_bind).await?;
    if config.server.web_tls {
        tracing::info!("web UI on https://{web_bind}");
        let acceptor = util::build_tls_acceptor(&config.server.cert_path, &config.server.key_path)?;
        serve_https(listener, acceptor, app, shutdown_signal).await?;
    } else {
        tracing::info!("web UI on http://{web_bind}");
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal)
            .await?;
    }

    // P1-10: Stop the tunnel listener and give existing sessions a brief drain
    // period before hard-stopping.
    tracing::info!("停止隧道监听器，等待现有连接排空 (2s)...");
    tunnel_handle.abort();
    // Allow in-flight tunnel sessions to finish their current I/O.
    tokio::time::sleep(Duration::from_secs(2)).await;

    tracing::info!("MiniMask 服务器已关闭");
    Ok(())
}

async fn stats_sampler(state: AppState) {
    let mut prev_in = state.stats.bytes_in.load(Ordering::Relaxed);
    let mut prev_out = state.stats.bytes_out.load(Ordering::Relaxed);
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    // P2-12: counter to avoid calling `subscriber_count()` (which takes a
    // mutex) every single tick. We check roughly every 5 ticks when no
    // subscribers are believed to be present.
    let mut idle_ticks: u32 = 0;
    let mut has_subs = false;
    loop {
        interval.tick().await;
        let cur_in = state.stats.bytes_in.load(Ordering::Relaxed);
        let cur_out = state.stats.bytes_out.load(Ordering::Relaxed);
        let rate_in = cur_in.saturating_sub(prev_in);
        let rate_out = cur_out.saturating_sub(prev_out);
        prev_in = cur_in;
        prev_out = cur_out;
        state.stats.record_sample(rate_in, rate_out).await;

        // Refresh subscriber presence at most every 5s when idle, every 1s
        // when we know someone is watching.
        if has_subs || idle_ticks >= 5 {
            has_subs = state.ws.subscriber_count().await > 0;
            idle_ticks = 0;
        } else {
            idle_ticks += 1;
        }
        if has_subs {
            let msg = crate::web::ws::build_stats_message(&state).await;
            state.ws.broadcast(msg).await;
        }
    }
}

/// Serve the Axum app over HTTPS (manual TLS accept + hyper-util auto builder).
/// Stops accepting new connections when `shutdown` completes (P1-10).
async fn serve_https(
    listener: TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
    app: axum::Router,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<()> {
    use hyper::service::service_fn;
    use hyper_util::rt::{TokioExecutor, TokioIo};
    use tower::ServiceExt;

    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                let (tcp, _addr) = match accept_result {
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
            _ = &mut shutdown => {
                tracing::info!("HTTPS 服务器停止接受新连接");
                break;
            }
        }
    }
    Ok(())
}
