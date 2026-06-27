//! MiniMask: a lightweight, stable and secure reverse-tunnel server with an
//! embedded Web UI. Single binary; supports a `server` and a `client` subcommand.

#[cfg(all(unix, not(target_env = "msvc")))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

mod client;
mod config;
mod error;
mod server;
mod state;
mod tunnel;
mod util;
mod web;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "minimask",
    version,
    about = "Lightweight, stable & secure reverse-tunnel server with an embedded Web UI"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the tunnel server + Web UI (default)
    Server {
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
    },
    /// Run a tunnel client (connect to a MiniMask server)
    Client(client::ClientArgs),
    /// Hash a password with argon2 (for manual config editing)
    HashPassword { password: String },
    /// Generate a self-signed certificate + key pair
    GenCert {
        #[arg(long, default_value = "cert.pem")]
        out_cert: PathBuf,
        #[arg(long, default_value = "key.pem")]
        out_key: PathBuf,
        #[arg(long, help = "subject alternative names (DNS/IP)")]
        san: Vec<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    util::install_crypto_provider();
    init_tracing();

    let cli = Cli::parse();
    let command = cli
        .command
        .unwrap_or(Commands::Server { config: PathBuf::from("config.toml") });

    match command {
        Commands::Server { config } => server::run(config).await,
        Commands::Client(args) => client::run(args).await,
        Commands::HashPassword { password } => {
            println!("{}", util::hash_password(&password)?);
            Ok(())
        }
        Commands::GenCert { out_cert, out_key, san } => {
            let san = if san.is_empty() {
                vec!["localhost".to_string(), "127.0.0.1".to_string()]
            } else {
                san
            };
            let (cert, key) = util::gen_self_signed_cert(&san)?;
            if let Some(p) = out_cert.parent() {
                std::fs::create_dir_all(p).ok();
            }
            if let Some(p) = out_key.parent() {
                std::fs::create_dir_all(p).ok();
            }
            std::fs::write(&out_cert, cert)?;
            std::fs::write(&out_key, key)?;
            println!("certificate -> {}", out_cert.display());
            println!("key         -> {}", out_key.display());
            Ok(())
        }
    }
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "minimask=info,tower_http=info".into()),
        )
        .init();
}

