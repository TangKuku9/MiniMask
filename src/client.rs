//! Companion tunnel client. Connects to the server, authenticates with a
//! client token, and bridges inbound yamux streams to local services. This
//! makes the whole system testable end-to-end from a single binary.
//!
//! The client can be driven in two ways:
//!   * from the CLI via [`run`] (logs go to stderr), and
//!   * from the embedded GUI via [`run_supervised`], which streams structured
//!     log events over a channel and supports cooperative cancellation.

use crate::tunnel::protocol;
use crate::util;
use anyhow::{anyhow, Result};
use clap::Args;
use std::future::poll_fn;
use std::sync::mpsc::Sender;
use std::sync::Arc;
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
    /// Path to the pinned CA certificate (PEM). Required when `--tls` is set
    /// unless `--insecure-skip-verify` is used. Defaults to `./data/ca.pem`.
    #[arg(long, default_value = "./data/ca.pem")]
    pub ca_path: String,
    /// Skip TLS certificate verification entirely. INSECURE — only for local
    /// debugging. Hidden from help output to avoid accidental use.
    #[arg(long, hide = true, default_value_t = false)]
    pub insecure_skip_verify: bool,
}

/// Log severity for events emitted by the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// Connection lifecycle state, surfaced to the GUI for a status badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    /// Establishing the TCP/TLS connection and authenticating.
    Connecting,
    /// Authenticated; tunnel is up and forwarding.
    Connected,
    /// Tunnel dropped; waiting before the next attempt.
    Reconnecting,
}

/// A structured event emitted by the supervised client loop.
#[derive(Debug, Clone)]
pub enum ClientEvent {
    Log { level: LogLevel, message: String },
    State(ConnState),
}

/// A sink the client uses to report events. When present, events are pushed to
/// the channel; otherwise the client falls back to stderr (CLI mode).
#[derive(Clone)]
pub struct EventSink {
    tx: Option<Sender<ClientEvent>>,
}

impl EventSink {
    /// A sink that only writes to stderr (used by the CLI).
    pub fn stderr() -> Self {
        Self { tx: None }
    }

    /// A sink that forwards events to a std channel (used by the GUI). Events
    /// are also echoed to stderr so console debugging still works.
    pub fn channel(tx: Sender<ClientEvent>) -> Self {
        Self { tx: Some(tx) }
    }

    fn log(&self, level: LogLevel, message: impl Into<String>) {
        let message = message.into();
        let tag = match level {
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        };
        eprintln!("[minimask] {tag}: {message}");
        if let Some(tx) = &self.tx {
            let _ = tx.send(ClientEvent::Log { level, message });
        }
    }

    fn info(&self, message: impl Into<String>) {
        self.log(LogLevel::Info, message);
    }

    fn warn(&self, message: impl Into<String>) {
        self.log(LogLevel::Warn, message);
    }

    fn error(&self, message: impl Into<String>) {
        self.log(LogLevel::Error, message);
    }

    fn state(&self, state: ConnState) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(ClientEvent::State(state));
        }
    }
}

/// CLI entry point. Runs forever, reconnecting on failure. Logs to stderr.
pub async fn run(args: ClientArgs) -> Result<()> {
    let sink = EventSink::stderr();
    let cancel = CancelToken::new();
    run_supervised(args, sink, cancel).await
}

/// A lightweight cancellation token shared between the GUI and the client task.
#[derive(Clone, Default)]
pub struct CancelToken {
    inner: Arc<tokio::sync::Notify>,
    flag: Arc<std::sync::atomic::AtomicBool>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// Request cancellation; wakes any task waiting in [`cancelled`].
    pub fn cancel(&self) {
        self.flag.store(true, std::sync::atomic::Ordering::SeqCst);
        self.inner.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(std::sync::atomic::Ordering::SeqCst)
    }

    async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        self.inner.notified().await;
    }
}

/// Supervised client loop used by the GUI. Reconnects with exponential backoff
/// (0.5s → 30s, ±10% jitter) and reports state/log events through `sink`.
/// Returns once `cancel` fires.
pub async fn run_supervised(args: ClientArgs, sink: EventSink, cancel: CancelToken) -> Result<()> {
    sink.info(format!(
        "客户端启动，连接到 {} (TLS={}) 身份 {}",
        args.server, args.tls, args.id
    ));

    // Exponential backoff parameters (P0-3).
    const INITIAL_BACKOFF: Duration = Duration::from_millis(500);
    const MAX_BACKOFF: Duration = Duration::from_secs(30);
    const JITTER_RATIO: f64 = 0.10; // ±10%

    let mut backoff = INITIAL_BACKOFF;

    loop {
        if cancel.is_cancelled() {
            sink.info("已停止");
            return Ok(());
        }

        sink.state(ConnState::Connecting);
        let result = tokio::select! {
            r = run_once(&args, &sink) => r,
            _ = cancel.cancelled() => {
                sink.info("收到停止信号，正在断开");
                return Ok(());
            }
        };

        let succeeded = matches!(result, Ok(()));
        match &result {
            Ok(()) => sink.warn(format!("隧道已关闭，{:.2?} 后重连", backoff)),
            Err(e) => sink.error(format!("隧道错误：{e}；{:.2?} 后重连", backoff)),
        }

        if cancel.is_cancelled() {
            sink.info("已停止");
            return Ok(());
        }

        // Sleep with jitter, then grow the backoff for the next round.
        // On a successful (but later closed) connection we still back off briefly
        // but reset to the initial value so transient disconnects recover fast.
        let sleep_dur = jitter(backoff, JITTER_RATIO);
        sink.state(ConnState::Reconnecting);
        tokio::select! {
            _ = tokio::time::sleep(sleep_dur) => {}
            _ = cancel.cancelled() => {
                sink.info("已停止");
                return Ok(());
            }
        }

        if succeeded {
            // Connection was established cleanly; reset backoff so the next
            // reconnect attempt is fast.
            backoff = INITIAL_BACKOFF;
        } else {
            // Grow backoff: backoff = min(backoff * 2, MAX_BACKOFF).
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    }
}

/// Apply symmetric ±`ratio` jitter to `d`. Returns a duration in
/// `[d * (1 - ratio), d * (1 + ratio)]`.
fn jitter(d: Duration, ratio: f64) -> Duration {
    use rand::Rng;
    let millis = d.as_millis() as f64;
    let factor = 1.0 + (rand::thread_rng().gen_range(-ratio..=ratio));
    let jittered = (millis * factor).max(0.0) as u64;
    Duration::from_millis(jittered)
}

async fn run_once(args: &ClientArgs, sink: &EventSink) -> Result<()> {
    sink.info(format!("正在连接 {} ...", args.server));

    // P0-2: TCP connect timeout (10s).
    let stream = tokio::time::timeout(
        Duration::from_secs(10),
        TcpStream::connect(&args.server),
    )
    .await
    .map_err(|_| anyhow!("连接 {} 超时（10s）", args.server))??;
    sink.info(format!("TCP 已连接到 {}", args.server));

    // Enable TCP keepalive to prevent NAT/firewall idle timeouts from killing
    // the otherwise-idle tunnel connection.
    {
        let sock_ref = socket2::SockRef::from(&stream);
        let ka = socket2::TcpKeepalive::new().with_time(Duration::from_secs(30));
        let _ = sock_ref.set_tcp_keepalive(&ka);
    }

    let mut boxed: Box<dyn crate::util::AsyncStream + Send + Unpin> = if args.tls {
        sink.info(format!("开始 TLS 握手 (server_name={})", args.server_name));
        // P0-1: use CA pinning by default; only fall back to the dangerous
        // connector when the user explicitly opts in via --insecure-skip-verify.
        let connector = if args.insecure_skip_verify {
            sink.warn("已禁用 TLS 证书校验（--insecure-skip-verify），仅用于本地调试");
            util::build_dangerous_tls_connector()
        } else {
            let ca_path = std::path::Path::new(&args.ca_path);
            util::build_tls_connector_with_ca(ca_path).map_err(|e| {
                anyhow!(
                    "加载 CA 证书失败 ({}): {e}\n提示：请从服务端 data/ca.pem 拷贝到客户端，或使用 --insecure-skip-verify 调试",
                    args.ca_path
                )
            })
        }?;
        let server_name = rustls::pki_types::ServerName::try_from(args.server_name.clone())
            .map_err(|e| anyhow!("非法的 server name '{}': {e}", args.server_name))?;
        // P0-2: TLS handshake timeout (15s).
        let tls_stream = tokio::time::timeout(
            Duration::from_secs(15),
            connector.connect(server_name, stream),
        )
        .await
        .map_err(|_| anyhow!("TLS 握手超时（15s）"))??;
        Box::new(tls_stream)
    } else {
        Box::new(stream)
    };
    sink.info("传输通道就绪");

    // P0-2: handshake write + status read timeout (15s total).
    sink.info(format!("发送握手 (id={})", args.id));
    let (ok, msg) = match tokio::time::timeout(
        Duration::from_secs(15),
        async {
            protocol::write_handshake(&mut boxed, &args.id, &args.token).await?;
            sink.info("等待认证结果 ...");
            protocol::read_status(&mut boxed).await
        },
    )
    .await
    {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            // Provide a helpful hint when the server closes the connection
            // immediately after the handshake. This usually means the server
            // expects TLS but the client connected without --tls.
            if !args.tls {
                sink.warn("服务端在握手后立即关闭了连接");
                sink.warn("提示：服务端可能要求 TLS，请尝试勾选「启用 TLS」");
            }
            return Err(e);
        }
        Err(_) => {
            return Err(anyhow!("握手超时（15s）"));
        }
    };
    if !ok {
        return Err(anyhow!("服务端拒绝：{msg}"));
    }
    sink.info("认证成功，隧道已建立");
    sink.state(ConnState::Connected);

    let conn = yamux::Connection::new(boxed.compat(), crate::tunnel::yamux_config(), yamux::Mode::Client);
    run_client_session(conn, sink).await;
    Ok(())
}

async fn run_client_session<S>(mut conn: yamux::Connection<S>, sink: &EventSink)
where
    S: futures_util::io::AsyncRead + futures_util::io::AsyncWrite + Unpin + Send,
{
    loop {
        match poll_fn(|cx| conn.poll_next_inbound(cx)).await {
            Some(Ok(stream)) => {
                let sink = sink.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_client_stream(stream, &sink).await {
                        sink.warn(format!("代理流结束：{e}"));
                    }
                });
            }
            Some(Err(e)) => {
                sink.warn(format!("yamux 错误：{e}"));
                break;
            }
            None => {
                sink.info("服务端关闭了隧道");
                break;
            }
        }
    }
}

async fn handle_client_stream(stream: yamux::Stream, sink: &EventSink) -> Result<()> {
    let mut stream = stream.compat();
    // P0-2: read target with a timeout so a malformed open doesn't hang forever.
    let target = tokio::time::timeout(Duration::from_secs(15), protocol::read_target(&mut stream))
        .await
        .map_err(|_| anyhow!("读取目标地址超时（15s）"))??;
    sink.info(format!("隧道 -> {target}"));
    // P0-2: local dial timeout (5s) so a dead local service can't pin a stream.
    let mut local = tokio::time::timeout(
        Duration::from_secs(5),
        TcpStream::connect(&target),
    )
    .await
    .map_err(|_| anyhow!("本地拨号 {target} 超时（5s）"))??;
    // P1-9: copy with idle timeout so a hung local service or idle visitor
    // can't pin a yamux stream forever.
    let _ = copy_bidirectional_idle(&mut stream, &mut local, crate::tunnel::PROXY_IDLE_TIMEOUT).await;
    Ok(())
}

/// Like `tokio::io::copy_bidirectional` but aborts if neither direction
/// transfers data within `idle`. The timer resets on every successful read
/// on either side.
async fn copy_bidirectional_idle<A, B>(a: &mut A, b: &mut B, idle: Duration) -> std::io::Result<(u64, u64)>
where
    A: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    B: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut a_buf = [0u8; 16384];
    let mut b_buf = [0u8; 16384];
    let mut a_to_b: u64 = 0;
    let mut b_to_a: u64 = 0;

    loop {
        tokio::select! {
            // biased: check for data first, then idle timeout
            biased;
            r = a.read(&mut a_buf) => {
                let n = r?;
                if n == 0 { break; }
                b.write_all(&a_buf[..n]).await?;
                a_to_b += n as u64;
            }
            r = b.read(&mut b_buf) => {
                let n = r?;
                if n == 0 { break; }
                a.write_all(&b_buf[..n]).await?;
                b_to_a += n as u64;
            }
            _ = tokio::time::sleep(idle) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("idle timeout after {:?}", idle),
                ));
            }
        }
    }
    Ok((a_to_b, b_to_a))
}