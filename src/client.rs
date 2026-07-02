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

/// Supervised client loop used by the GUI. Reconnects with backoff and reports
/// state/log events through `sink`. Returns once `cancel` fires.
pub async fn run_supervised(args: ClientArgs, sink: EventSink, cancel: CancelToken) -> Result<()> {
    sink.info(format!(
        "客户端启动，连接到 {} (TLS={}) 身份 {}",
        args.server, args.tls, args.id
    ));

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

        match result {
            Ok(()) => sink.warn("隧道已关闭，3 秒后重连"),
            Err(e) => sink.error(format!("隧道错误：{e}；3 秒后重连")),
        }

        if cancel.is_cancelled() {
            sink.info("已停止");
            return Ok(());
        }

        sink.state(ConnState::Reconnecting);
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(3)) => {}
            _ = cancel.cancelled() => {
                sink.info("已停止");
                return Ok(());
            }
        }
    }
}

async fn run_once(args: &ClientArgs, sink: &EventSink) -> Result<()> {
    sink.info(format!("正在连接 {} ...", args.server));
    let stream = TcpStream::connect(&args.server).await?;
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
        let connector = util::build_dangerous_tls_connector()?;
        let server_name = rustls::pki_types::ServerName::try_from(args.server_name.clone())
            .map_err(|e| anyhow!("非法的 server name '{}': {e}", args.server_name))?;
        Box::new(connector.connect(server_name, stream).await?)
    } else {
        Box::new(stream)
    };
    sink.info("传输通道就绪");

    sink.info(format!("发送握手 (id={})", args.id));
    protocol::write_handshake(&mut boxed, &args.id, &args.token).await?;
    sink.info("等待认证结果 ...");
    let (ok, msg) = match protocol::read_status(&mut boxed).await {
        Ok(v) => v,
        Err(e) => {
            // Provide a helpful hint when the server closes the connection
            // immediately after the handshake. This usually means the server
            // expects TLS but the client connected without --tls.
            if !args.tls {
                sink.warn("服务端在握手后立即关闭了连接");
                sink.warn("提示：服务端可能要求 TLS，请尝试勾选「启用 TLS」");
            }
            return Err(e);
        }
    };
    if !ok {
        return Err(anyhow!("服务端拒绝：{msg}"));
    }
    sink.info("认证成功，隧道已建立");
    sink.state(ConnState::Connected);

    let conn = yamux::Connection::new(boxed.compat(), yamux::Config::default(), yamux::Mode::Client);
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
    let target = protocol::read_target(&mut stream).await?;
    sink.info(format!("隧道 -> {target}"));
    let mut local = TcpStream::connect(&target).await?;
    let _ = tokio::io::copy_bidirectional(&mut stream, &mut local).await;
    Ok(())
}