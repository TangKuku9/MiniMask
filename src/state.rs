//! Global shared application state, data models and in-memory stores.

use crate::util;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::task::JoinHandle;

pub type ProxyTx = mpsc::Sender<ProxyRequest>;

/// A request handed from a public-port listener to a tunnel session's driver,
/// asking it to open a yamux stream toward the client and proxy this connection.
pub struct ProxyRequest {
    pub target: String,
    pub conn: TcpStream,
}

// ===========================================================================
// Data models
// ===========================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortMapping {
    pub id: String,
    pub name: String,
    pub remote_port: u16,
    /// Local address the client should dial, e.g. "127.0.0.1:8080".
    pub local_addr: String,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Client {
    pub id: String,
    pub name: String,
    /// SHA-256 hex of the client token (never the plaintext).
    pub token_hash: String,
    /// First chars of the token, for display only.
    pub token_prefix: String,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub mappings: Vec<PortMapping>,
}

/// A flattened view of a mapping joined with its owning client, for the UI.
#[derive(Debug, Clone, Serialize)]
pub struct MappingView {
    pub id: String,
    pub name: String,
    pub remote_port: u16,
    pub local_addr: String,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub client_id: String,
    pub client_name: String,
}

// ===========================================================================
// Client store (persisted to clients.json)
// ===========================================================================

pub struct ClientStore {
    pub list: Vec<Client>,
    path: PathBuf,
}

impl ClientStore {
    pub fn load(path: PathBuf) -> Result<Self> {
        if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("read {}", path.display()))?;
            let list: Vec<Client> = serde_json::from_str(&raw)
                .with_context(|| format!("parse {}", path.display()))?;
            Ok(Self { list, path })
        } else {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            // P1-7: initial creation uses sync I/O (startup only, not hot path).
            let list = Vec::new();
            let json = serde_json::to_string_pretty(&list).context("serialize clients")?;
            std::fs::write(&path, json).context("write clients")?;
            Ok(Self { list, path })
        }
    }

    /// Persist the store to disk using async I/O (tokio::fs).
    ///
    /// P1-7: previously this used `std::fs` (blocking) and was called from
    /// within `RwLock<ClientStore>` write guards, blocking the executor thread
    /// on slow disks. Now uses `tokio::fs` which offloads I/O to a blocking
    /// thread pool, so the executor stays responsive.
    pub async fn persist(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        let json = serde_json::to_string_pretty(&self.list).context("serialize clients")?;
        let tmp = self.path.with_extension("json.tmp");
        tokio::fs::write(&tmp, json)
            .await
            .with_context(|| format!("write clients tmp {}", tmp.display()))?;
        tokio::fs::rename(&tmp, &self.path)
            .await
            .with_context(|| format!("rename clients to {}", self.path.display()))?;
        Ok(())
    }

    pub fn find(&self, id: &str) -> Option<&Client> {
        self.list.iter().find(|c| c.id == id)
    }

    /// Find a mapping by id across all clients, returning the owning client id.
    pub fn find_mapping(&self, mapping_id: &str) -> Option<(String, PortMapping)> {
        for c in &self.list {
            if let Some(m) = c.mappings.iter().find(|m| m.id == mapping_id) {
                return Some((c.id.clone(), m.clone()));
            }
        }
        None
    }

    pub fn verify_token(&self, token: &str, pepper: &str) -> Option<(String, bool)> {
        let h = util::sha256_hex_with_pepper(token, pepper);
        self.list
            .iter()
            .find(|c| c.token_hash == h && c.enabled)
            .map(|c| (c.id.clone(), c.enabled))
    }

    pub fn remote_port_taken(&self, port: u16, ignore_mapping_id: Option<&str>) -> bool {
        self.list.iter().any(|c| {
            c.mappings.iter().any(|m| {
                m.remote_port == port && Some(m.id.as_str()) != ignore_mapping_id
            })
        })
    }

    pub async fn add_client(&mut self, name: &str, pepper: &str) -> Result<(Client, String)> {
        let token = util::gen_token();
        let client = Client {
            id: util::gen_client_id(),
            name: name.to_string(),
            token_hash: util::sha256_hex_with_pepper(&token, pepper),
            token_prefix: token.chars().take(12).collect(),
            enabled: true,
            created_at: Utc::now(),
            mappings: Vec::new(),
        };
        self.list.push(client.clone());
        self.persist().await?;
        Ok((client, token))
    }

    pub async fn delete_client(&mut self, id: &str) -> Result<bool> {
        let before = self.list.len();
        self.list.retain(|c| c.id != id);
        let changed = self.list.len() != before;
        if changed {
            self.persist().await?;
        }
        Ok(changed)
    }

    pub async fn set_client_enabled(&mut self, id: &str, enabled: bool) -> Result<bool> {
        if let Some(c) = self.list.iter_mut().find(|c| c.id == id) {
            c.enabled = enabled;
            self.persist().await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn regenerate_token(&mut self, id: &str, pepper: &str) -> Result<Option<String>> {
        let token = util::gen_token();
        if let Some(c) = self.list.iter_mut().find(|c| c.id == id) {
            c.token_hash = util::sha256_hex_with_pepper(&token, pepper);
            c.token_prefix = token.chars().take(12).collect();
            self.persist().await?;
            Ok(Some(token))
        } else {
            Ok(None)
        }
    }

    pub async fn add_mapping(
        &mut self,
        client_id: &str,
        name: &str,
        remote_port: u16,
        local_addr: &str,
    ) -> Result<Option<PortMapping>> {
        if self.remote_port_taken(remote_port, None) {
            return Ok(None);
        }
        if let Some(c) = self.list.iter_mut().find(|c| c.id == client_id) {
            let m = PortMapping {
                id: util::gen_mapping_id(),
                name: name.to_string(),
                remote_port,
                local_addr: local_addr.to_string(),
                enabled: true,
                created_at: Utc::now(),
            };
            c.mappings.push(m.clone());
            self.persist().await?;
            Ok(Some(m))
        } else {
            Ok(None)
        }
    }

    pub async fn delete_mapping(&mut self, id: &str) -> Result<bool> {
        let mut changed = false;
        for c in self.list.iter_mut() {
            let before = c.mappings.len();
            c.mappings.retain(|m| m.id != id);
            if c.mappings.len() != before {
                changed = true;
            }
        }
        if changed {
            self.persist().await?;
        }
        Ok(changed)
    }

    pub async fn set_mapping_enabled(&mut self, id: &str, enabled: bool) -> Result<bool> {
        let mut found = false;
        for c in self.list.iter_mut() {
            if let Some(m) = c.mappings.iter_mut().find(|m| m.id == id) {
                m.enabled = enabled;
                found = true;
            }
        }
        if found {
            self.persist().await?;
        }
        Ok(found)
    }

    pub fn mapping_views(&self) -> Vec<MappingView> {
        let mut out = Vec::new();
        for c in &self.list {
            for m in &c.mappings {
                out.push(MappingView {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    remote_port: m.remote_port,
                    local_addr: m.local_addr.clone(),
                    enabled: m.enabled,
                    created_at: m.created_at,
                    client_id: c.id.clone(),
                    client_name: c.name.clone(),
                });
            }
        }
        out
    }
}

// ===========================================================================
// Live tunnel sessions (in-memory)
// ===========================================================================

pub struct SessionInfo {
    pub client_id: String,
    pub session_id: String,
    pub remote_addr: String,
    pub connected_at: DateTime<Utc>,
    pub active_conns: Arc<AtomicU64>,
    pub proxy_tx: ProxyTx,
    pub listeners: Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
}

impl SessionInfo {
    pub fn active_conns(&self) -> u64 {
        self.active_conns.load(Ordering::Relaxed)
    }
}

pub type SessionStore = HashMap<String, SessionInfo>;

// ===========================================================================
// Stats (live counters + history for the dashboard chart)
// ===========================================================================

#[derive(Debug, Clone, Serialize)]
pub struct Sample {
    pub ts: DateTime<Utc>,
    pub rate_in: u64,
    pub rate_out: u64,
    pub active_conns: u64,
}

#[derive(Default)]
pub struct Stats {
    /// Bytes flowing visitor -> client (toward the tunneled service).
    pub bytes_out: AtomicU64,
    /// Bytes flowing client -> visitor (responses back out).
    pub bytes_in: AtomicU64,
    pub total_conns: AtomicU64,
    pub active_conns: AtomicU64,
    pub history: RwLock<Vec<Sample>>,
}

impl Stats {
    pub fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            bytes_in: self.bytes_in.load(Ordering::Relaxed),
            bytes_out: self.bytes_out.load(Ordering::Relaxed),
            total_conns: self.total_conns.load(Ordering::Relaxed),
            active_conns: self.active_conns.load(Ordering::Relaxed),
        }
    }

    pub async fn history(&self) -> Vec<Sample> {
        self.history.read().await.clone()
    }

    pub async fn record_sample(&self, rate_in: u64, rate_out: u64) {
        let s = Sample {
            ts: Utc::now(),
            rate_in,
            rate_out,
            active_conns: self.active_conns.load(Ordering::Relaxed),
        };
        let mut h = self.history.write().await;
        h.push(s);
        if h.len() > 120 {
            let drop_n = h.len() - 120;
            h.drain(0..drop_n);
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StatsSnapshot {
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub total_conns: u64,
    pub active_conns: u64,
}

// ===========================================================================
// Audit log (in-memory ring buffer + optional file persistence)
// ===========================================================================

#[derive(Debug, Clone, Serialize)]
pub struct AuditLog {
    pub ts: DateTime<Utc>,
    pub level: String,
    pub category: String,
    pub message: String,
}

pub struct AuditLogStore {
    logs: Mutex<VecDeque<AuditLog>>,
    cap: usize,
    /// Optional file persistence target (JSON-lines, with size-based rotation).
    file: Option<AuditLogFile>,
}

struct AuditLogFile {
    path: PathBuf,
    max_bytes: u64,
}

impl AuditLogStore {
    /// Create an in-memory ring buffer with the given capacity.
    pub fn new(cap: usize) -> Self {
        Self {
            logs: Mutex::new(VecDeque::with_capacity(cap)),
            cap,
            file: None,
        }
    }

    /// Enable file persistence. Each `add()` call will append a JSON line to
    /// `path`. When the file exceeds `max_size_mb`, it is rotated to
    /// `<path>.1` and a new file is started.
    pub fn with_file(mut self, path: PathBuf, max_size_mb: u64) -> Self {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        self.file = Some(AuditLogFile {
            path,
            max_bytes: max_size_mb.saturating_mul(1024 * 1024),
        });
        self
    }

    pub async fn add(&self, level: &str, category: &str, message: impl Into<String>) {
        let entry = AuditLog {
            ts: Utc::now(),
            level: level.to_string(),
            category: category.to_string(),
            message: message.into(),
        };
        match level {
            "warn" => tracing::warn!("[{category}] {}", entry.message),
            "error" => tracing::error!("[{category}] {}", entry.message),
            _ => tracing::info!("[{category}] {}", entry.message),
        }
        // Push to in-memory ring buffer (brief lock).
        {
            let mut g = self.logs.lock().await;
            if g.len() >= self.cap {
                g.pop_front();
            }
            g.push_back(entry.clone());
        }
        // P1-6: append to file outside the in-memory lock so the ring buffer
        // stays responsive even when disk I/O is slow.
        if let Some(file) = &self.file {
            if let Err(e) = file.append(&entry).await {
                tracing::warn!("audit log file write failed: {e}");
            }
        }
    }

    pub async fn list(&self) -> Vec<AuditLog> {
        self.logs.lock().await.iter().cloned().collect()
    }
}

impl AuditLogFile {
    async fn append(&self, entry: &AuditLog) -> Result<()> {
        use tokio::io::AsyncWriteExt;

        // Rotate if the file is too large. Check size before writing to avoid
        // rotating on every single entry.
        if let Ok(meta) = tokio::fs::metadata(&self.path).await {
            if meta.len() >= self.max_bytes {
                let backup = self.path.with_extension("log.1");
                // Best-effort rotation; if rename fails we just keep appending.
                let _ = tokio::fs::rename(&self.path, &backup).await;
            }
        }

        let line = serde_json::to_string(entry).context("serialize audit log")? + "\n";
        let mut f = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await
            .with_context(|| format!("open audit log {}", self.path.display()))?;
        f.write_all(line.as_bytes())
            .await
            .with_context(|| format!("write audit log {}", self.path.display()))?;
        Ok(())
    }
}

// ===========================================================================
// Auth store (persisted to auth.json)
// ===========================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AuthFile {
    username: String,
    password_hash: String,
}

pub struct AuthStore {
    inner: RwLock<AuthFile>,
    path: PathBuf,
}

impl AuthStore {
    pub fn load_or_seed(path: PathBuf, username: &str, password: &str) -> Result<Self> {
        if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("read {}", path.display()))?;
            let f: AuthFile = serde_json::from_str(&raw)
                .with_context(|| format!("parse {}", path.display()))?;
            Ok(Self { inner: RwLock::new(f), path })
        } else {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            let f = AuthFile {
                username: username.to_string(),
                password_hash: util::hash_password(password)?,
            };
            // P1-7: initial creation uses sync I/O (startup only, not hot path).
            let json = serde_json::to_string_pretty(&f).context("serialize auth")?;
            std::fs::write(&path, json).with_context(|| format!("write auth {}", path.display()))?;
            Ok(Self {
                inner: RwLock::new(f),
                path,
            })
        }
    }

    /// Persist auth state using async I/O (tokio::fs).
    ///
    /// P1-7: previously used `std::fs` (blocking) under the write lock.
    async fn persist_inner(&self, f: &AuthFile) -> Result<()> {
        let json = serde_json::to_string_pretty(f).context("serialize auth")?;
        let tmp = self.path.with_extension("json.tmp");
        tokio::fs::write(&tmp, json)
            .await
            .with_context(|| format!("write auth tmp {}", tmp.display()))?;
        tokio::fs::rename(&tmp, &self.path)
            .await
            .with_context(|| format!("rename auth to {}", self.path.display()))?;
        Ok(())
    }

    pub async fn username(&self) -> String {
        self.inner.read().await.username.clone()
    }

    pub async fn verify(&self, password: &str) -> bool {
        util::verify_password(password, &self.inner.read().await.password_hash)
    }

    pub async fn change_password(&self, new_password: &str) -> Result<()> {
        let f = {
            let mut g = self.inner.write().await;
            g.password_hash = util::hash_password(new_password)?;
            g.clone()
        };
        self.persist_inner(&f).await
    }
}

// ===========================================================================
// WebSocket broadcaster (push live stats to dashboard clients)
// ===========================================================================

pub struct WsBroadcaster {
    clients: Mutex<Vec<mpsc::UnboundedSender<String>>>,
}

impl WsBroadcaster {
    pub fn new() -> Self {
        Self {
            clients: Mutex::new(Vec::new()),
        }
    }

    pub async fn subscribe(&self) -> mpsc::UnboundedReceiver<String> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.clients.lock().await.push(tx);
        rx
    }

    /// Current number of live subscribers. Used by the stats sampler to skip
    /// broadcasting when no dashboard is watching (P2-12).
    pub async fn subscriber_count(&self) -> usize {
        let mut g = self.clients.lock().await;
        // Prune disconnected senders so the count is accurate.
        g.retain(|s| !s.is_closed());
        g.len()
    }

    pub async fn broadcast(&self, msg: String) {
        let mut g = self.clients.lock().await;
        g.retain(|s| s.send(msg.clone()).is_ok());
    }
}

// ===========================================================================
// AppState
// ===========================================================================

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<crate::config::Config>,
    pub clients: Arc<RwLock<ClientStore>>,
    pub sessions: Arc<RwLock<SessionStore>>,
    pub stats: Arc<Stats>,
    pub logs: Arc<AuditLogStore>,
    pub auth: Arc<AuthStore>,
    pub ws: Arc<WsBroadcaster>,
    /// Server-side pepper mixed into client token hashes (P2-14). Prevents
    /// rainbow-table attacks in case `clients.json` is leaked: an attacker
    /// would need the pepper (stored separately in `data/token_pepper`) to
    /// mount even a brute-force attack against individual tokens.
    pub token_pepper: Arc<String>,
}

impl AppState {
    pub async fn log(&self, level: &str, category: &str, message: impl Into<String>) {
        self.logs.add(level, category, message).await;
    }
}
