//! WebSocket endpoint that pushes live stats to dashboard clients.

use crate::state::AppState;
use crate::web::auth::AuthedUser;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use serde_json::json;

/// Build the JSON stats object broadcast to all connected dashboard clients.
/// Also used to send an initial snapshot on connect and by the REST `/api/stats`.
pub async fn build_stats_value(state: &AppState) -> serde_json::Value {
    let snapshot = state.stats.snapshot();
    let history = state.stats.history().await;
    let sessions: Vec<_> = state
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
    json!({
        "type": "stats",
        "bytes_in": snapshot.bytes_in,
        "bytes_out": snapshot.bytes_out,
        "total_conns": snapshot.total_conns,
        "active_conns": snapshot.active_conns,
        "history": history,
        "sessions": sessions,
    })
}

/// Build the JSON stats message broadcast to all connected dashboard clients.
/// Also used to send an initial snapshot on connect.
pub async fn build_stats_message(state: &AppState) -> String {
    build_stats_value(state).await.to_string()
}

pub async fn ws_handler(
    State(state): State<AppState>,
    _user: AuthedUser,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn handle_ws(mut socket: WebSocket, state: AppState) {
    let mut rx = state.ws.subscribe().await;

    // Send an immediate snapshot so the dashboard renders without waiting.
    let initial = build_stats_message(&state).await;
    if socket.send(Message::Text(initial.into())).await.is_err() {
        return;
    }

    loop {
        tokio::select! {
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(Message::Ping(_))) => { /* axum auto-pongs */ }
                Some(Ok(_)) => { /* ignore text/binary from client */ }
                Some(Err(_)) => break,
            },
            msg = rx.recv() => match msg {
                Some(text) => {
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                None => break,
            },
        }
    }
}
