//! HTTP + WebSocket gateway in front of [`aggora_state::CoinState`].
//!
//! The router maps every public/admin/operator-signed endpoint listed in the spec (G.2) to a
//! handler. Each handler:
//!
//! 1. Deserializes the body into a typed [`aggora_types`] request.
//! 2. Applies pre-validation that doesn't need state: rate limits keyed by `ConnectInfo` IP or
//!    by the wallet id in the body, so a flood of unauthorized traffic can't burn Ed25519
//!    verifies on the validator.
//! 3. Calls [`operator_auth`] for operator-signed paths, which delegates to
//!    [`CoinState::verify_operator_request`].
//! 4. Forwards to the matching [`CoinState`] method; any [`StateError`] is mapped to the spec
//!    G.4 error code + HTTP status via [`state_error`].
//!
//! Responses always wrap into [`ApiResponse`] so clients see a uniform `{success,data,error,
//! tick,iteration}` envelope.

use aggora_state::{CoinState, StateError};
use aggora_types::*;
use axum::{
    extract::{ConnectInfo, Path, State, WebSocketUpgrade},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{net::SocketAddr, time::Duration};
use tokio::net::TcpListener;
use tower_http::{cors::CorsLayer, trace::TraceLayer};

#[derive(Debug, Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<ApiErrorBody>,
    pub tick: Tick,
    pub iteration: IterationId,
}

#[derive(Debug, Serialize)]
pub struct ApiErrorBody {
    pub code: String,
    pub message: String,
}

pub fn router(state: CoinState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/v1/health", get(healthz))
        .route("/api/v1/operator/genesis", get(genesis_operator))
        .route("/api/v1/wallet", post(create_wallet))
        .route("/api/v1/charge", post(charge))
        .route("/api/v1/transaction", post(transfer))
        .route("/api/v1/burn", post(burn))
        .route("/api/v1/wallet/:id", get(get_wallet))
        .route("/api/v1/wallet/:id/transactions", get(wallet_transactions))
        .route("/api/v1/transaction/:id", get(get_transaction))
        .route("/api/v1/iteration/current", get(current_iteration))
        .route("/api/v1/iteration/:id", get(get_iteration))
        .route("/api/v1/stats", get(stats))
        .route("/api/v1/stats/gini", get(gini_history))
        .route("/api/v1/stats/supply", get(supply_history))
        .route("/api/v1/events", get(events))
        .route("/admin/parameters", get(admin_parameters).put(update_parameters))
        .route("/admin/simulation/start", post(simulation_start))
        .route("/admin/simulation/stop", post(simulation_stop))
        .route("/admin/simulation/status", get(simulation_status))
        .route("/admin/snapshot", post(admin_snapshot))
        .route("/admin/operator", post(admin_operator_placeholder))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

pub async fn serve(state: CoinState, bind: SocketAddr) -> anyhow::Result<()> {
    let listener = TcpListener::bind(bind).await?;
    tracing::info!(%bind, "aggora coin REST API listening");
    // `with_connect_info::<SocketAddr>()` makes the client's source address available to
    // handlers via the `ConnectInfo` extractor, which is what the rate-limiter keys on.
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

async fn healthz(State(state): State<CoinState>) -> Response {
    ok(&state, StatusCode::OK, json!({"status": "ok", "service": "aggora-coin"})).await
}

async fn genesis_operator(State(state): State<CoinState>) -> Response {
    ok(
        &state,
        StatusCode::OK,
        json!({
            "operator_id": state.operator_id(),
            "public_key": state.operator_public_key(),
        }),
    )
    .await
}

async fn create_wallet(
    State(state): State<CoinState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match serde_json::from_slice::<WalletCreateRequest>(&body) {
        Ok(request) => request,
        Err(err) => return error(&state, StatusCode::BAD_REQUEST, "BAD_REQUEST", err.to_string()).await,
    };
    // Per-IP daily limit applies before signature verification: a flood of unauthorized requests
    // shouldn't be able to consume CPU on Ed25519 verifies from a single source.
    if let Err(err) = state.check_wallet_rate_limit(Some(addr.ip())).await {
        return state_error(&state, err).await;
    }
    let (operator, sig) = match operator_auth(&state, "POST", "/api/v1/wallet", &headers, &body, false).await {
        Ok(auth) => auth,
        Err(err) => return state_error(&state, err).await,
    };
    match state.create_wallet(request, &operator, sig).await {
        Ok(response) => ok(&state, StatusCode::CREATED, response).await,
        Err(err) => state_error(&state, err).await,
    }
}

async fn charge(State(state): State<CoinState>, headers: HeaderMap, body: Bytes) -> Response {
    let request = match serde_json::from_slice::<ChargeRequest>(&body) {
        Ok(request) => request,
        Err(err) => return error(&state, StatusCode::BAD_REQUEST, "BAD_REQUEST", err.to_string()).await,
    };
    let (operator, sig) = match operator_auth(&state, "POST", "/api/v1/charge", &headers, &body, false).await {
        Ok(auth) => auth,
        Err(err) => return state_error(&state, err).await,
    };
    match state.charge(request, &operator, sig).await {
        Ok(response) => ok(&state, StatusCode::ACCEPTED, response).await,
        Err(err) => state_error(&state, err).await,
    }
}

async fn transfer(State(state): State<CoinState>, body: Bytes) -> Response {
    let request = match serde_json::from_slice::<TransferRequest>(&body) {
        Ok(request) => request,
        Err(err) => return error(&state, StatusCode::BAD_REQUEST, "BAD_REQUEST", err.to_string()).await,
    };
    // Per-wallet per-minute limit, applied before signature verification so a hostile sender
    // can't burn the validator's CPU by spamming bogus transfers from one wallet id.
    if let Err(err) = state.check_transfer_rate_limit(&request.from).await {
        return state_error(&state, err).await;
    }
    match state.transfer(request).await {
        Ok(response) => ok(&state, StatusCode::ACCEPTED, response).await,
        Err(err) => state_error(&state, err).await,
    }
}

async fn burn(State(state): State<CoinState>, body: Bytes) -> Response {
    let request = match serde_json::from_slice::<BurnRequest>(&body) {
        Ok(request) => request,
        Err(err) => return error(&state, StatusCode::BAD_REQUEST, "BAD_REQUEST", err.to_string()).await,
    };
    match state.burn(request).await {
        Ok(response) => ok(&state, StatusCode::ACCEPTED, response).await,
        Err(err) => state_error(&state, err).await,
    }
}

async fn get_wallet(State(state): State<CoinState>, Path(id): Path<String>) -> Response {
    match state.storage().get_wallet(&id) {
        Ok(Some(wallet)) => ok(&state, StatusCode::OK, wallet).await,
        Ok(None) => state_error(&state, StateError::WalletNotFound).await,
        Err(err) => state_error(&state, StateError::Internal(err)).await,
    }
}

async fn wallet_transactions(State(state): State<CoinState>, Path(id): Path<String>) -> Response {
    match state.storage().list_transactions_for_wallet(&id, 100) {
        Ok(transactions) => ok(&state, StatusCode::OK, transactions).await,
        Err(err) => state_error(&state, StateError::Internal(err)).await,
    }
}

async fn get_transaction(State(state): State<CoinState>, Path(id): Path<String>) -> Response {
    match state.storage().get_transaction(&id) {
        Ok(Some(tx)) => ok(&state, StatusCode::OK, tx).await,
        Ok(None) => error(&state, StatusCode::NOT_FOUND, "TRANSACTION_NOT_FOUND", "transaction not found").await,
        Err(err) => state_error(&state, StateError::Internal(err)).await,
    }
}

async fn current_iteration(State(state): State<CoinState>) -> Response {
    let system = state.system().await;
    if system.current_iteration == 0 {
        return ok(&state, StatusCode::OK, json!({"iteration_id": 0, "status": "genesis"})).await;
    }
    match state.storage().get_iteration(system.current_iteration) {
        Ok(Some(iteration)) => ok(&state, StatusCode::OK, iteration).await,
        Ok(None) => ok(&state, StatusCode::OK, json!({"iteration_id": system.current_iteration})).await,
        Err(err) => state_error(&state, StateError::Internal(err)).await,
    }
}

async fn get_iteration(State(state): State<CoinState>, Path(id): Path<u64>) -> Response {
    match state.storage().get_iteration(id) {
        Ok(Some(iteration)) => ok(&state, StatusCode::OK, iteration).await,
        Ok(None) => error(&state, StatusCode::NOT_FOUND, "ITERATION_NOT_FOUND", "iteration not found").await,
        Err(err) => state_error(&state, StateError::Internal(err)).await,
    }
}

async fn stats(State(state): State<CoinState>) -> Response {
    match state.stats().await {
        Ok(stats) => ok(&state, StatusCode::OK, stats).await,
        Err(err) => state_error(&state, err).await,
    }
}

async fn gini_history(State(state): State<CoinState>) -> Response {
    match state.storage().gini_history() {
        Ok(points) => ok(&state, StatusCode::OK, points).await,
        Err(err) => state_error(&state, StateError::Internal(err)).await,
    }
}

async fn supply_history(State(state): State<CoinState>) -> Response {
    match state.storage().supply_history() {
        Ok(points) => ok(&state, StatusCode::OK, points).await,
        Err(err) => state_error(&state, StateError::Internal(err)).await,
    }
}

async fn admin_parameters(State(state): State<CoinState>, headers: HeaderMap, body: Bytes) -> Response {
    if let Err(err) = operator_auth(&state, "GET", "/admin/parameters", &headers, &body, true).await {
        return state_error(&state, err).await;
    }
    ok(&state, StatusCode::OK, state.parameters().await).await
}

async fn update_parameters(State(state): State<CoinState>, headers: HeaderMap, body: Bytes) -> Response {
    let parameters = match serde_json::from_slice::<SystemParameters>(&body) {
        Ok(parameters) => parameters,
        Err(err) => return error(&state, StatusCode::BAD_REQUEST, "BAD_REQUEST", err.to_string()).await,
    };
    if let Err(err) = operator_auth(&state, "PUT", "/admin/parameters", &headers, &body, true).await {
        return state_error(&state, err).await;
    }
    match state.update_parameters(parameters).await {
        Ok(()) => ok(&state, StatusCode::OK, state.parameters().await).await,
        Err(err) => state_error(&state, err).await,
    }
}

#[derive(Debug, Deserialize)]
struct SimulationStartRequest {
    #[serde(default = "default_one")]
    iterations: u64,
}

fn default_one() -> u64 {
    1
}

async fn simulation_start(State(state): State<CoinState>, headers: HeaderMap, body: Bytes) -> Response {
    let request = if body.is_empty() {
        SimulationStartRequest { iterations: 1 }
    } else {
        match serde_json::from_slice::<SimulationStartRequest>(&body) {
            Ok(request) => request,
            Err(err) => return error(&state, StatusCode::BAD_REQUEST, "BAD_REQUEST", err.to_string()).await,
        }
    };
    if let Err(err) = operator_auth(&state, "POST", "/admin/simulation/start", &headers, &body, true).await {
        return state_error(&state, err).await;
    }
    match state.run_simulation_iterations(request.iterations.max(1)).await {
        Ok(status) => ok(&state, StatusCode::ACCEPTED, status).await,
        Err(err) => state_error(&state, err).await,
    }
}

async fn simulation_stop(State(state): State<CoinState>, headers: HeaderMap, body: Bytes) -> Response {
    if let Err(err) = operator_auth(&state, "POST", "/admin/simulation/stop", &headers, &body, true).await {
        return state_error(&state, err).await;
    }
    let status = state.set_simulation_running(false, "stopped").await;
    ok(&state, StatusCode::OK, status).await
}

async fn simulation_status(State(state): State<CoinState>) -> Response {
    ok(&state, StatusCode::OK, state.simulation_status().await).await
}

async fn admin_snapshot(State(state): State<CoinState>, headers: HeaderMap, body: Bytes) -> Response {
    if let Err(err) = operator_auth(&state, "POST", "/admin/snapshot", &headers, &body, true).await {
        return state_error(&state, err).await;
    }
    match state.create_manual_snapshot().await {
        Ok(snapshot) => ok(&state, StatusCode::CREATED, snapshot).await,
        Err(err) => state_error(&state, err).await,
    }
}

async fn admin_operator_placeholder(State(state): State<CoinState>, headers: HeaderMap, body: Bytes) -> Response {
    if let Err(err) = operator_auth(&state, "POST", "/admin/operator", &headers, &body, true).await {
        return state_error(&state, err).await;
    }
    error(
        &state,
        StatusCode::NOT_IMPLEMENTED,
        "OPERATOR_CHANGE_DISABLED",
        "operator changes are disabled in v1.0 single-operator prototype",
    )
    .await
}

async fn events(State(state): State<CoinState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| async move {
        let (mut sender, mut receiver) = socket.split();
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let payload = match state.stats().await {
                        Ok(stats) => json!({"event": "stats", "data": stats}).to_string(),
                        Err(err) => json!({"event": "error", "message": err.to_string()}).to_string(),
                    };
                    if sender.send(axum::extract::ws::Message::Text(payload)).await.is_err() {
                        break;
                    }
                }
                message = receiver.next() => {
                    if message.is_none() {
                        break;
                    }
                }
            }
        }
    })
}

async fn operator_auth(
    state: &CoinState,
    method: &str,
    path: &str,
    headers: &HeaderMap,
    body: &[u8],
    admin_only: bool,
) -> Result<(Operator, String), StateError> {
    let timestamp = header_str(headers, "x-operator-timestamp").and_then(|value| value.parse::<i64>().ok());
    let signature = header_str(headers, "x-operator-signature");
    let operator_id = header_str(headers, "x-operator-id");
    let operator_public_key = header_str(headers, "x-operator-public-key");
    let operator = state
        .verify_operator_request(
            method,
            path,
            body,
            timestamp,
            signature.as_deref(),
            operator_id.as_deref(),
            operator_public_key.as_deref(),
            admin_only,
        )
        .await?;
    Ok((operator, signature.unwrap_or_default()))
}

fn header_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get(name)?.to_str().ok().map(ToString::to_string)
}

async fn ok<T: Serialize>(state: &CoinState, status: StatusCode, data: T) -> Response {
    let system = state.system().await;
    (
        status,
        Json(ApiResponse {
            success: true,
            data: Some(data),
            error: None,
            tick: system.current_tick,
            iteration: system.current_iteration,
        }),
    )
        .into_response()
}

async fn error(state: &CoinState, status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    let system = state.system().await;
    (
        status,
        Json(ApiResponse::<serde_json::Value> {
            success: false,
            data: None,
            error: Some(ApiErrorBody {
                code: code.to_string(),
                message: message.into(),
            }),
            tick: system.current_tick,
            iteration: system.current_iteration,
        }),
    )
        .into_response()
}

async fn state_error(state: &CoinState, err: StateError) -> Response {
    let status = match err {
        StateError::InvalidSignature => StatusCode::UNAUTHORIZED,
        StateError::OperatorUnauthorized => StatusCode::FORBIDDEN,
        StateError::InvalidNonce | StateError::InsufficientBalance | StateError::BadRequest(_) | StateError::WalletAlreadyExists => {
            StatusCode::BAD_REQUEST
        }
        StateError::WalletNotFound => StatusCode::NOT_FOUND,
        StateError::IterationInProgress => StatusCode::CONFLICT,
        StateError::RateLimitExceeded => StatusCode::TOO_MANY_REQUESTS,
        StateError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let code = err.code().to_string();
    error(state, status, &code, err.to_string()).await
}
