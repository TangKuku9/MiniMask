//! MiniMask: a lightweight, stable and secure reverse-tunnel server with an
//! embedded Web UI. Single binary; supports a `server` and a `client` subcommand.
//!
//! Dual-mode entry point:
//!   * Run with any subcommand / from a terminal → command-line behavior.
//!   * Double-click the executable (Windows) → a graphical client UI opens with
//!     no console window at all.
//!
//! To avoid a stray black console window when double-clicking, the binary is
//! compiled for the Windows GUI subsystem. When run from a terminal we attach
//! to the parent process' console via `AttachConsole` so CLI output still
//! appears, and that same call tells us whether we were launched from a shell
//! (has a parent console) or by double-clicking (no parent console → open GUI).
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(all(unix, not(target_env = "msvc")))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

mod client;
mod config;
mod error;
mod gui;
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
    /// Force the graphical client UI (same as double-clicking the executable).
    #[arg(long, global = true)]
    gui: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the tunnel server + Web UI (default when launched from a console)
    Server {
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
    },
    /// Run a tunnel client (connect to a MiniMask server)
    Client(client::ClientArgs),
    /// Open the graphical client UI
    Gui,
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

fn main() -> anyhow::Result<()> {
    // On Windows the binary is a GUI-subsystem app (so double-clicking never
    // pops a black console). When launched from a terminal, attach to the
    // parent console so CLI output is visible. The return value also tells us
    // whether we were started from a shell (true) or by double-click (false).
    #[cfg(windows)]
    let from_terminal = attach_parent_console();
    #[cfg(not(windows))]
    let from_terminal = true;

    // On Windows, set the console output codepage to UTF-8 so that error
    // messages (which may contain non-ASCII characters) are displayed correctly.
    #[cfg(windows)]
    {
        if from_terminal {
            let _ = enable_vt_processing();
        }
    }

    util::install_crypto_provider();

    let cli = Cli::parse();

    // Decide whether to open the GUI. This happens when the user either:
    //   * passes `--gui` or the `gui` subcommand, or
    //   * double-clicks the executable (no subcommand + no parent console).
    let explicit_gui = cli.gui || matches!(cli.command, Some(Commands::Gui));
    let auto_gui = cli.command.is_none() && !cli.gui && !from_terminal;

    if explicit_gui || auto_gui {
        return gui::run();
    }

    // Command-line / server path: logs to stderr, needs a tokio runtime.
    init_tracing();
    let command = cli
        .command
        .unwrap_or(Commands::Server { config: PathBuf::from("config.toml") });

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(run_command(command))
}

async fn run_command(command: Commands) -> anyhow::Result<()> {
    match command {
        Commands::Server { config } => server::run(config).await,
        Commands::Client(args) => client::run(args).await,
        Commands::Gui => gui::run(),
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

/// Attempt to attach to the parent process' console (the terminal that
/// launched us). Returns `true` if a parent console was available — i.e. we
/// were started from a shell — and `false` if there was none (typical of a
/// double-click from Explorer), in which case we should open the GUI.
#[cfg(windows)]
fn attach_parent_console() -> bool {
    use windows_sys::Win32::System::Console::AttachConsole;
    // ATTACH_PARENT_PROCESS == 0xFFFF_FFFF (u32::MAX). Using the literal avoids
    // a constant-resolution quirk in some windows-sys versions.
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS) != 0 }
}

/// Enable UTF-8 and virtual terminal processing on the Windows console so that
/// non-ASCII error messages and tracing logs are displayed correctly.
#[cfg(windows)]
fn enable_vt_processing() -> std::io::Result<()> {
    use windows_sys::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, SetConsoleMode, SetConsoleOutputCP,
        ENABLE_VIRTUAL_TERMINAL_PROCESSING, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE,
    };

    unsafe {
        // Set both stdout and stderr codepage to UTF-8 (65001).
        SetConsoleOutputCP(65001);

        // Enable virtual terminal processing (ANSI escape sequences) on stdout
        // and stderr so that tracing's colored output works.
        for handle in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let h = GetStdHandle(handle);
            if h == 0 || h == -1 {
                continue;
            }
            let mut mode = 0u32;
            if GetConsoleMode(h, &mut mode) != 0 {
                SetConsoleMode(h, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
            }
        }
    }
    Ok(())
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "minimask=info,tower_http=info".into()),
        )
        // Output to stderr to avoid Windows stdout block-buffering issues when
        // the process is not attached to an interactive TTY (e.g. release builds
        // launched from PowerShell). stderr is unbuffered by default.
        .with_writer(std::io::stderr)
        .init();
}