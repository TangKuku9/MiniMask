//! Authentication: JWT inside an HttpOnly cookie, a `require_auth` middleware
//! layer, and the login/logout/me handlers.

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::util;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, HeaderValue};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

/// The authenticated principal, stored in request extensions by `require_auth`.
#[derive(Clone, Debug)]
pub struct AuthedUser {
    pub username: String,
}

impl<S: Send + Sync> FromRequestParts<S> for AuthedUser {
    type Rejection = AppError;
    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<AuthedUser>()
            .cloned()
            .ok_or(AppError::Unauthorized)
    }
}

pub fn get_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let header = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in header.split(';') {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix(name) {
            if let Some(v) = rest.strip_prefix('=') {
                return Some(v.to_string());
            }
        }
    }
    None
}

fn set_auth_cookie(resp: &mut Response, token: &str, ttl_secs: u64, secure: bool) {
    let mut val = format!(
        "{}={}; HttpOnly; Path=/; SameSite=Strict; Max-Age={ttl_secs}",
        util::COOKIE_NAME,
        token
    );
    if secure {
        val.push_str("; Secure");
    }
    if let Ok(hv) = HeaderValue::from_str(&val) {
        resp.headers_mut().append(header::SET_COOKIE, hv);
    }
}

fn clear_auth_cookie(resp: &mut Response) {
    let val = format!(
        "{}=; HttpOnly; Path=/; SameSite=Strict; Max-Age=0",
        util::COOKIE_NAME
    );
    if let Ok(hv) = HeaderValue::from_str(&val) {
        resp.headers_mut().append(header::SET_COOKIE, hv);
    }
}

/// Middleware: reject requests without a valid JWT cookie.
pub async fn require_auth(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, AppError> {
    let token = get_cookie(req.headers(), util::COOKIE_NAME).ok_or(AppError::Unauthorized)?;
    let claims = util::verify_jwt(&token, &state.config.auth.jwt_secret)
        .map_err(|_| AppError::Unauthorized)?;
    req.extensions_mut().insert(AuthedUser {
        username: claims.sub,
    });
    Ok(next.run(req).await)
}

#[derive(Debug, Deserialize)]
pub struct LoginReq {
    pub username: String,
    pub password: String,
}

pub async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginReq>,
) -> AppResult<Response> {
    let username = state.auth.username().await;
    let ok = req.username == username && state.auth.verify(&req.password).await;
    if !ok {
        state
            .log("warn", "auth", format!("failed login attempt for '{}'", req.username))
            .await;
        return Err(AppError::Unauthorized);
    }
    let token = util::make_jwt(
        &username,
        &state.config.auth.jwt_secret,
        state.config.auth.token_ttl_hours,
    )?;
    state
        .log("info", "auth", format!("user {username} logged in"))
        .await;
    let secure = state.config.server.web_tls;
    let mut resp = Json(json!({ "username": username })).into_response();
    set_auth_cookie(
        &mut resp,
        &token,
        state.config.auth.token_ttl_hours * 3600,
        secure,
    );
    Ok(resp)
}

pub async fn logout() -> Response {
    let mut resp = Json(json!({ "ok": true })).into_response();
    clear_auth_cookie(&mut resp);
    resp
}

pub async fn me(user: AuthedUser) -> impl IntoResponse {
    Json(json!({ "username": user.username }))
}
