//! Web subsystem: embedded SPA, JWT auth, REST routes and WebSocket stats.

pub mod auth;
pub mod embed;
pub mod routes;
pub mod ws;

use crate::state::AppState;
use axum::middleware;
use axum::routing::{get, patch, post};
use axum::Router;
use tower_http::trace::TraceLayer;

/// Build the full Axum application router.
pub fn build_app(state: AppState) -> Router {
    // Routes protected by the JWT cookie middleware.
    let protected = Router::new()
        .route("/api/auth/me", get(auth::me))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/clients", get(routes::list_clients).post(routes::create_client))
        .route("/api/clients/{id}", patch(routes::update_client).delete(routes::delete_client))
        .route("/api/clients/{id}/regenerate-token", post(routes::regenerate_token))
        .route("/api/mappings", get(routes::list_mappings).post(routes::create_mapping))
        .route("/api/mappings/{id}", patch(routes::update_mapping).delete(routes::delete_mapping))
        .route("/api/logs", get(routes::list_logs))
        .route("/api/stats", get(routes::get_stats))
        .route("/api/sessions", get(routes::list_sessions))
        .route("/api/settings", get(routes::get_settings))
        .route("/api/settings/password", post(routes::change_password))
        .route("/api/ws", get(ws::ws_handler))
        .layer(middleware::from_fn_with_state(state.clone(), auth::require_auth));

    // Public routes (no auth).
    let public = Router::new().route("/api/auth/login", post(auth::login));

    Router::new()
        .merge(public)
        .merge(protected)
        .fallback(embed::serve_embed)
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}
