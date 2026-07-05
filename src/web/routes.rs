//! REST API handlers for clients, mappings, logs, stats, sessions and settings.

use crate::error::{AppError, AppResult};
use crate::state::{AppState, Client, PortMapping};
use crate::tunnel::listener;
use crate::web::auth::AuthedUser;
use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::net::SocketAddr;

/// A client view that omits the secret token hash.
#[derive(Serialize)]
struct ClientView {
    id: String,
    name: String,
    token_prefix: String,
    enabled: bool,
    created_at: chrono::DateTime<chrono::Utc>,
    mappings: Vec<PortMapping>,
}

impl From<Client> for ClientView {
    fn from(c: Client) -> Self {
        Self {
            id: c.id,
            name: c.name,
            token_prefix: c.token_prefix,
            enabled: c.enabled,
            created_at: c.created_at,
            mappings: c.mappings,
        }
    }
}

// ---------------------------------------------------------------------------
// Clients
// ---------------------------------------------------------------------------

pub async fn list_clients(State(state): State<AppState>) -> AppResult<Json<Value>> {
    let views: Vec<ClientView> = state
        .clients
        .read()
        .await
        .list
        .iter()
        .map(|c| ClientView::from(c.clone()))
        .collect();
    Ok(Json(json!(views)))
}

#[derive(Deserialize)]
pub struct CreateClientReq {
    pub name: String,
}

pub async fn create_client(
    State(state): State<AppState>,
    user: AuthedUser,
    Json(req): Json<CreateClientReq>,
) -> AppResult<Json<Value>> {
    if req.name.trim().is_empty() {
        return Err(AppError::bad_request("name is required"));
    }
    let (client, token) = state
        .clients
        .write()
        .await
        .add_client(req.name.trim(), &state.token_pepper)
        .await?;
    state
        .log("info", "client", format!("{} created client {} ({})", user.username, client.id, client.name))
        .await;
    let view = ClientView::from(client);
    Ok(Json(json!({ "client": view, "token": token })))
}

#[derive(Deserialize)]
pub struct UpdateEnabledReq {
    pub enabled: bool,
}

pub async fn update_client(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(id): Path<String>,
    Json(req): Json<UpdateEnabledReq>,
) -> AppResult<Json<Value>> {
    let found = state
        .clients
        .write()
        .await
        .set_client_enabled(&id, req.enabled)
        .await?;
    if !found {
        return Err(AppError::NotFound);
    }
    if req.enabled {
        listener::start_client_listeners(&state, &id).await;
    } else {
        listener::remove_client_listeners(&state, &id).await;
    }
    state
        .log("info", "client", format!("{} set client {id} enabled={}", user.username, req.enabled))
        .await;
    Ok(Json(json!({ "ok": true })))
}

pub async fn delete_client(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    let found = state.clients.write().await.delete_client(&id).await?;
    if !found {
        return Err(AppError::NotFound);
    }
    listener::remove_client_listeners(&state, &id).await;
    state
        .log("warn", "client", format!("{} deleted client {id}", user.username))
        .await;
    Ok(Json(json!({ "ok": true })))
}

pub async fn regenerate_token(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    let token = state
        .clients
        .write()
        .await
        .regenerate_token(&id, &state.token_pepper)
        .await?
        .ok_or(AppError::NotFound)?;
    state
        .log("warn", "client", format!("{} regenerated token for client {id}", user.username))
        .await;
    Ok(Json(json!({ "token": token })))
}

// ---------------------------------------------------------------------------
// Mappings
// ---------------------------------------------------------------------------

pub async fn list_mappings(State(state): State<AppState>) -> AppResult<Json<Value>> {
    let views = state.clients.read().await.mapping_views();
    Ok(Json(json!(views)))
}

#[derive(Deserialize)]
pub struct CreateMappingReq {
    pub client_id: String,
    pub name: String,
    pub remote_port: u16,
    pub local_addr: String,
}

pub async fn create_mapping(
    State(state): State<AppState>,
    user: AuthedUser,
    Json(req): Json<CreateMappingReq>,
) -> AppResult<Json<Value>> {
    if req.name.trim().is_empty() {
        return Err(AppError::bad_request("name is required"));
    }
    if req.remote_port == 0 {
        return Err(AppError::bad_request("remote_port is required"));
    }
    req.local_addr
        .parse::<SocketAddr>()
        .map_err(|_| AppError::bad_request("local_addr must be IP:port, e.g. 127.0.0.1:8080"))?;

    let mapping = state
        .clients
        .write()
        .await
        .add_mapping(&req.client_id, req.name.trim(), req.remote_port, &req.local_addr)
        .await?
        .ok_or_else(|| AppError::Conflict("remote_port already in use or client not found".into()))?;
    if mapping.enabled {
        let _ = listener::add_listener(&state, &req.client_id, &mapping).await;
    }
    state
        .log("info", "mapping", format!("{} created mapping {} ({} -> :{})", user.username, mapping.id, mapping.local_addr, mapping.remote_port))
        .await;
    Ok(Json(json!(mapping)))
}

pub async fn update_mapping(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(id): Path<String>,
    Json(req): Json<UpdateEnabledReq>,
) -> AppResult<Json<Value>> {
    let found = state
        .clients
        .write()
        .await
        .set_mapping_enabled(&id, req.enabled)
        .await?;
    if !found {
        return Err(AppError::NotFound);
    }
    if req.enabled {
        if let Some((cid, m)) = state.clients.read().await.find_mapping(&id) {
            let _ = listener::add_listener(&state, &cid, &m).await;
        }
    } else {
        listener::remove_listener(&state, &id).await;
    }
    state
        .log("info", "mapping", format!("{} set mapping {id} enabled={}", user.username, req.enabled))
        .await;
    Ok(Json(json!({ "ok": true })))
}

pub async fn delete_mapping(
    State(state): State<AppState>,
    user: AuthedUser,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    let found = state.clients.write().await.delete_mapping(&id).await?;
    if !found {
        return Err(AppError::NotFound);
    }
    listener::remove_listener(&state, &id).await;
    state
        .log("warn", "mapping", format!("{} deleted mapping {id}", user.username))
        .await;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Logs / stats / sessions / settings
// ---------------------------------------------------------------------------

pub async fn list_logs(State(state): State<AppState>) -> AppResult<Json<Value>> {
    let logs = state.logs.list().await;
    Ok(Json(json!(logs)))
}

pub async fn get_stats(State(state): State<AppState>) -> AppResult<Json<Value>> {
    Ok(Json(crate::web::ws::build_stats_value(&state).await))
}

pub async fn list_sessions(State(state): State<AppState>) -> AppResult<Json<Value>> {
    let sessions: Vec<Value> = state
        .sessions
        .read()
        .await
        .values()
        .map(|s| {
            json!({
                "client_id": s.client_id,
                "remote_addr": s.remote_addr,
                "connected_at": s.connected_at,
                "active_conns": s.active_conns(),
            })
        })
        .collect();
    Ok(Json(json!(sessions)))
}

pub async fn get_settings(State(state): State<AppState>) -> AppResult<Json<Value>> {
    let username = state.auth.username().await;
    Ok(Json(json!({
        "username": username,
        "tunnel_bind": state.config.server.tunnel_bind,
        "web_bind": state.config.server.web_bind,
        "tunnel_tls": state.config.server.tunnel_tls,
        "web_tls": state.config.server.web_tls,
        "max_clients": state.config.security.max_clients,
        "max_conns_per_client": state.config.security.max_conns_per_client,
        "data_dir": state.config.server.data_dir.display().to_string(),
    })))
}

#[derive(Deserialize)]
pub struct ChangePasswordReq {
    pub old_password: String,
    pub new_password: String,
}

pub async fn change_password(
    State(state): State<AppState>,
    user: AuthedUser,
    Json(req): Json<ChangePasswordReq>,
) -> AppResult<Json<Value>> {
    if req.new_password.len() < 6 {
        return Err(AppError::bad_request("new password must be at least 6 characters"));
    }
    if !state.auth.verify(&req.old_password).await {
        return Err(AppError::bad_request("old password incorrect"));
    }
    state.auth.change_password(&req.new_password).await?;
    state
        .log("warn", "auth", format!("{} changed admin password", user.username))
        .await;
    Ok(Json(json!({ "ok": true })))
}
