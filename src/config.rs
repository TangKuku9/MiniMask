//! Configuration loading, validation and default generation.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub auth: AuthConfig,
    pub security: SecurityConfig,
    #[serde(default)]
    pub audit: AuditConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Address the tunnel clients connect to (TLS + yamux), e.g. "0.0.0.0:7443".
    pub tunnel_bind: String,
    /// Address the management Web UI / REST API listens on, e.g. "0.0.0.0:8080".
    pub web_bind: String,
    /// Directory for persistent state (clients.json, auth.json) and certs.
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
    /// Enable TLS for the tunnel listener.
    #[serde(default = "default_true")]
    pub tunnel_tls: bool,
    /// Enable HTTPS for the Web UI.
    #[serde(default)]
    pub web_tls: bool,
    /// TLS certificate PEM path (server cert signed by the CA).
    #[serde(default = "default_cert_path")]
    pub cert_path: PathBuf,
    /// TLS private key PEM path (server key).
    #[serde(default = "default_key_path")]
    pub key_path: PathBuf,
    /// CA certificate PEM path. This file is distributed to clients for
    /// CA pinning and must be kept in sync with the server cert above.
    #[serde(default = "default_ca_path")]
    pub ca_path: PathBuf,
    /// CA private key PEM path. Used only to sign the initial server cert;
    /// kept on disk so future rotations can sign new server certs.
    #[serde(default = "default_ca_key_path")]
    pub ca_key_path: PathBuf,
    /// Subject Alternative Names embedded in the auto-generated certificate.
    #[serde(default = "default_san")]
    pub cert_san: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    #[serde(default = "default_admin_user")]
    pub admin_username: String,
    #[serde(default = "default_admin_pass")]
    pub admin_password: String,
    #[serde(default)]
    pub jwt_secret: String,
    #[serde(default = "default_ttl")]
    pub token_ttl_hours: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    #[serde(default = "default_max_clients")]
    pub max_clients: usize,
    #[serde(default = "default_max_conns")]
    pub max_conns_per_client: usize,
}

/// Audit log configuration. The in-memory ring buffer is always maintained for
/// the Web UI, but entries can additionally be appended to a file for
/// long-term retention and compliance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    /// File path for persistent audit log (JSON-lines format). Set to empty
    /// to disable file persistence.
    #[serde(default = "default_audit_log_file")]
    pub log_file: PathBuf,
    /// Maximum file size in MiB before rotation. When the file exceeds this,
    /// it is renamed to `<file>.1` and a new file is started.
    #[serde(default = "default_audit_max_size_mb")]
    pub max_size_mb: u64,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            log_file: default_audit_log_file(),
            max_size_mb: default_audit_max_size_mb(),
        }
    }
}

fn default_audit_log_file() -> PathBuf {
    PathBuf::from("./data/audit.log")
}
fn default_audit_max_size_mb() -> u64 {
    10
}

fn default_data_dir() -> PathBuf {
    PathBuf::from("./data")
}
fn default_cert_path() -> PathBuf {
    PathBuf::from("./data/cert.pem")
}
fn default_key_path() -> PathBuf {
    PathBuf::from("./data/key.pem")
}
fn default_ca_path() -> PathBuf {
    PathBuf::from("./data/ca.pem")
}
fn default_ca_key_path() -> PathBuf {
    PathBuf::from("./data/ca_key.pem")
}
fn default_san() -> Vec<String> {
    vec!["localhost".to_string(), "127.0.0.1".to_string()]
}
fn default_true() -> bool {
    true
}
fn default_admin_user() -> String {
    "admin".to_string()
}
fn default_admin_pass() -> String {
    "admin".to_string()
}
fn default_ttl() -> u64 {
    24
}
fn default_max_clients() -> usize {
    100
}
fn default_max_conns() -> usize {
    512
}

impl Default for Config {
    fn default() -> Self {
        Config {
            server: ServerConfig {
                tunnel_bind: "0.0.0.0:7443".to_string(),
                web_bind: "0.0.0.0:8080".to_string(),
                data_dir: default_data_dir(),
                tunnel_tls: true,
                web_tls: false,
                cert_path: default_cert_path(),
                key_path: default_key_path(),
                ca_path: default_ca_path(),
                ca_key_path: default_ca_key_path(),
                cert_san: default_san(),
            },
            auth: AuthConfig {
                admin_username: default_admin_user(),
                admin_password: default_admin_pass(),
                jwt_secret: String::new(),
                token_ttl_hours: default_ttl(),
            },
            security: SecurityConfig {
                max_clients: default_max_clients(),
                max_conns_per_client: default_max_conns(),
            },
            audit: AuditConfig::default(),
        }
    }
}

impl Config {
    /// Load configuration from `config_path`. If it does not exist, write the
    /// default config there and return it.
    pub fn load_or_create(config_path: &Path) -> Result<Self> {
        if !config_path.exists() {
            tracing::info!("config file not found at {}, writing defaults", config_path.display());
            let default = toml::to_string_pretty(&Config::default())
                .context("serialize default config")?;
            let header = "# MiniMask server configuration.\n# This file is auto-created with these defaults on first run if missing.\n\n";
            if let Some(parent) = config_path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            std::fs::write(config_path, format!("{header}{default}"))
                .with_context(|| format!("write config to {}", config_path.display()))?;
        }
        let raw = std::fs::read_to_string(config_path)
            .with_context(|| format!("read config {}", config_path.display()))?;
        let cfg: Config = toml::from_str(&raw).context("parse config.toml")?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<()> {
        if self.server.tunnel_bind.is_empty() {
            anyhow::bail!("server.tunnel_bind must not be empty");
        }
        if self.server.web_bind.is_empty() {
            anyhow::bail!("server.web_bind must not be empty");
        }
        if self.auth.admin_username.is_empty() {
            anyhow::bail!("auth.admin_username must not be empty");
        }
        if self.auth.token_ttl_hours == 0 {
            anyhow::bail!("auth.token_ttl_hours must be > 0");
        }
        Ok(())
    }
}
