//! Tunnel subsystem: protocol framing, yamux session driver, TLS listener and
//! the proxy engine.

pub mod listener;
pub mod protocol;
pub mod proxy;
pub mod session;

pub use listener::{run_tunnel_listener, TunnelAcceptor};

use std::time::Duration;

/// Build a tuned yamux `Config` for both client and server sides.
///
/// Defaults in yamux 0.14 are reasonable but we tighten a few knobs for
/// long-lived tunnel connections:
/// - `max_num_streams`: explicit cap so a misbehaving peer can't exhaust
///   memory by opening unbounded streams.
/// - `max_connection_receive_window`: raised to 256 MiB so high-RTT links
///   (mobile, cross-continent) can saturate bandwidth.
/// - `split_send_size`: 64 KiB for fewer frames on high-throughput links.
///
/// Keepalive is handled internally by yamux (ping/pong based on activity)
/// and is not exposed via `Config`.
pub fn yamux_config() -> yamux::Config {
    let mut cfg = yamux::Config::default();
    cfg.set_max_num_streams(1024);
    cfg.set_max_connection_receive_window(Some(256 * 1024 * 1024));
    cfg.set_split_send_size(64 * 1024);
    cfg
}

/// Default idle timeout for proxy copy loops (both directions). A connection
/// that transfers no data for this duration is forcibly closed to prevent
/// stream exhaustion when a local service hangs.
pub const PROXY_IDLE_TIMEOUT: Duration = Duration::from_secs(300); // 5 minutes

