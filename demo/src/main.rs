// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
//! The same-origin demo bridge: static assets, a bootstrap endpoint (session
//! capability token + initial data), validated state-changing commands, and a
//! bounded SSE snapshot stream. Loopback-only by construction.

mod scenario;

use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use scenario::{spawn_run, RunState, SharedRun};
use serde::Deserialize;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

/// Static assets, embedded: the page, the app modules, the styles.
mod statics {
    pub const HTML: &str = include_str!("../static/index.html");
    pub const APP_JS: &str = include_str!("../static/app.js");
    pub const VIEWS_JS: &str = include_str!("../static/views.js");
    pub const GRAPH_JS: &str = include_str!("../static/graph.js");
    pub const STYLES: &str = include_str!("../static/style.css");
}

/// Host stems a same-origin browser sends while we bind loopback.
const HOSTS: [&str; 4] = ["127.0.0.1", "localhost", "[::1]", "[::]"];

/// SSE publication cadence — bounded, coalesced snapshots at a fixed cadence.
const PUBLISH_MS: u64 = 500;

/// Everything the demo serves comes from itself; no external code, ever.
const CSP: &str =
    "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'";

#[derive(Clone)]
struct Bridge {
    shared: Arc<SharedRun>,
    token: Arc<String>,
}

#[derive(Deserialize)]
struct RunCommand {
    action: String,
}

#[derive(Deserialize)]
struct PurchaseCommand {
    offer_tx_id: String,
    /// The acting pharmacy; defaults to the contract owner.
    buyer: Option<String>,
    /// Units to buy off the offer (partial buys); defaults to the rest.
    quantity: Option<u64>,
}

fn host_header_ok(headers: &HeaderMap) -> bool {
    headers
        .get(header::HOST)
        .and_then(|host| host.to_str().ok())
        .map(|host| {
            let bare = host.split(':').next().unwrap_or("");
            HOSTS.contains(&bare)
        })
        .unwrap_or(false)
}

/// For state-changing commands the `Origin` check is the cross-origin drive-by
/// defense, together with the per-run token in the auth header (no cookies).
fn origin_ok(headers: &HeaderMap) -> bool {
    headers
        .get(header::ORIGIN)
        .and_then(|origin| origin.to_str().ok())
        .map(|origin| HOSTS.iter().any(|known| origin.contains(known)))
        .unwrap_or(false)
}

/// Attach the demo CSP; every response the bridge emits gets it.
fn with_csp(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    response
}

fn json_plain(value: serde_json::Value) -> Response {
    with_csp(Json(value).into_response())
}

fn error_json(status: StatusCode, message: &str) -> Response {
    let Ok(value) = serde_json::to_value(serde_json::json!({ "error": message })) else {
        return (status, message.to_owned()).into_response();
    };
    let mut response = json_plain(value);
    *response.status_mut() = status;
    response
}

fn asset_response(body: &'static str, content_type: &'static str) -> Response {
    let mut response = ([(header::CONTENT_TYPE, content_type)], body).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// GET / — the demo page.
async fn get_index(headers: HeaderMap) -> Response {
    if !host_header_ok(&headers) {
        return error_json(StatusCode::FORBIDDEN, "host mismatch");
    }
    asset_response(statics::HTML, "text/html; charset=utf-8")
}

/// GET /api/bootstrap: session token + initial data, same-origin only; the
/// token never travels as a query parameter, never in a log line.
async fn get_bootstrap(State(bridge): State<Bridge>, headers: HeaderMap) -> Response {
    if !host_header_ok(&headers) {
        return error_json(StatusCode::FORBIDDEN, "host mismatch");
    }
    let params = scenario::params_for_view(&bridge.shared).await;
    let companies = scenario::build_companies(&params);
    json_plain(serde_json::json!({
        "token": bridge.token.to_string(),
        "orgs": companies.iter().map(|c| (&c.id, &c.role)).collect::<Vec<_>>(),
        "params": params,
        "mode": scenario::MODE_LABEL,
    }))
}

/// GET /api/snapshot[?as=<org id>]: the bounded run snapshot. Without `as`
/// the view is public (commitments, never PDC cleartext); an `as=<org>` view
/// adds only the payloads the org genuinely holds, after the server-side
/// membership gate. UI hiding is not the enforcement — this filter is.
async fn get_snapshot(
    State(bridge): State<Bridge>,
    headers: HeaderMap,
    params: Query<HashMap<String, String>>,
) -> Response {
    if !host_header_ok(&headers) {
        return error_json(StatusCode::FORBIDDEN, "host mismatch");
    }
    let merged = match params.get("as").map(String::as_str) {
        None | Some("") => bridge.shared.snapshot_value().await,
        Some(view_as) => {
            scenario::org_snapshot(
                &bridge.shared,
                view_as,
                params.get("viewer").map(String::as_str),
            )
            .await
        }
    };
    json_plain(merged)
}

/// POST /api/run {action: "start"|"stop"|"reset"}: token + Origin gated.
async fn post_run(
    State(bridge): State<Bridge>,
    headers: HeaderMap,
    Json(command): Json<RunCommand>,
) -> Response {
    if !origin_ok(&headers) {
        return error_json(StatusCode::FORBIDDEN, "cross-origin command rejected");
    }
    let auth_ok = headers
        .get("x-glass-auth")
        .and_then(|value| value.to_str().ok())
        .map(|token| token == bridge.token.as_str())
        .unwrap_or(false);
    if !auth_ok {
        return error_json(StatusCode::FORBIDDEN, "missing or invalid session token");
    }
    let mut task_guard = bridge.shared.task.lock().await;
    match command.action.as_str() {
        "start" => {
            if task_guard.is_none() {
                let shared = Arc::clone(&bridge.shared);
                *task_guard = Some(spawn_run(shared));
                let mut state = bridge.shared.state.lock().await;
                state.status = "running".into();
            }
            json_plain(serde_json::json!({ "result": "running" }))
        }
        "stop" => {
            if let Some(task) = task_guard.take() {
                task.abort();
            }
            bridge.shared.state.lock().await.status = "stopped".into();
            json_plain(serde_json::json!({ "result": "stopped" }))
        }
        "reset" => {
            if let Some(task) = task_guard.take() {
                task.abort();
            }
            let mut state = bridge.shared.state.lock().await;
            *state = RunState::default();
            state.status = "idle".into();
            json_plain(serde_json::json!({ "result": "reset" }))
        }
        _ => error_json(
            StatusCode::BAD_REQUEST,
            "expected action: start | stop | reset",
        ),
    }
}

/// POST /api/purchase {offer_tx_id}: the human completes a pending offer as
/// the currently-viewed pharmacy (or the contract's buyer). Token + Origin
/// gated; a real PurchaseOrder goes through the buyer's own node.
async fn post_purchase(
    State(bridge): State<Bridge>,
    headers: HeaderMap,
    Json(command): Json<PurchaseCommand>,
) -> Response {
    if !origin_ok(&headers) {
        return error_json(StatusCode::FORBIDDEN, "cross-origin command rejected");
    }
    let auth_ok = headers
        .get("x-glass-auth")
        .and_then(|value| value.to_str().ok())
        .map(|token| token == bridge.token.as_str())
        .unwrap_or(false);
    if !auth_ok {
        return error_json(StatusCode::FORBIDDEN, "missing or invalid session token");
    }
    let buyer = match command.buyer.as_deref() {
        Some(wanted) => wanted.to_owned(),
        None => "pharmacy-1".to_owned(),
    };
    match scenario::complete_purchase(
        &bridge.shared,
        &command.offer_tx_id,
        &buyer,
        command.quantity.unwrap_or(u64::MAX),
    )
    .await
    {
        Ok(tx_id) => json_plain(serde_json::json!({ "result": "submitted", "tx": tx_id })),
        Err(error) => error_json(StatusCode::BAD_REQUEST, &error),
    }
}

/// POST /api/params: edit the simulation (company counts per role, evil
/// nodes, lots/round, interval). Token + Origin gated; topology changes
/// rebuild the federation at the next round boundary — mid run, live.
async fn post_params(
    State(bridge): State<Bridge>,
    headers: HeaderMap,
    Json(requested): Json<scenario::SimParams>,
) -> Response {
    if !origin_ok(&headers) {
        return error_json(StatusCode::FORBIDDEN, "cross-origin command rejected");
    }
    let auth_ok = headers
        .get("x-glass-auth")
        .and_then(|value| value.to_str().ok())
        .map(|token| token == bridge.token.as_str())
        .unwrap_or(false);
    if !auth_ok {
        return error_json(StatusCode::FORBIDDEN, "missing or invalid session token");
    }
    let applied = scenario::apply_params(&bridge.shared, requested).await;
    json_plain(serde_json::json!({ "params": applied }))
}

/// GET /api/events: the bounded SSE snapshot stream, one coalesced snapshot
/// per interval (never one event per transaction). A reconnecting client
/// refetches /api/snapshot — the presentation stream is not an audit log.
async fn get_events(State(bridge): State<Bridge>, headers: HeaderMap) -> Response {
    if !host_header_ok(&headers) {
        return error_json(StatusCode::FORBIDDEN, "host mismatch");
    }
    let shared = std::sync::Arc::clone(&bridge.shared);
    let stream = async_stream::stream! {
        let mut interval = tokio::time::interval(Duration::from_millis(PUBLISH_MS));
        loop {
            interval.tick().await;
            let body = shared.snapshot_value().await;
            let body = serde_json::to_string(&body).unwrap_or_default();
            yield Ok::<Event, Infallible>(Event::default().event("snapshot").data(body));
        }
    };
    let sse = Sse::new(stream).keep_alive(KeepAlive::default());
    with_csp(sse.into_response())
}

#[tokio::main]
async fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .init();
    let listen = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:18850".to_owned());
    let token = uuid::Uuid::new_v4().simple().to_string();
    let bridge = Bridge {
        shared: Arc::new(SharedRun::default()),
        token: Arc::new(token),
    };

    let app = Router::new()
        .route("/", get(get_index))
        .route(
            "/app.js",
            get(|| async { asset_response(statics::APP_JS, "text/javascript; charset=utf-8") }),
        )
        .route(
            "/views.js",
            get(|| async { asset_response(statics::VIEWS_JS, "text/javascript; charset=utf-8") }),
        )
        .route(
            "/graph.js",
            get(|| async { asset_response(statics::GRAPH_JS, "text/javascript; charset=utf-8") }),
        )
        .route(
            "/style.css",
            get(|| async { asset_response(statics::STYLES, "text/css; charset=utf-8") }),
        )
        .route("/api/bootstrap", get(get_bootstrap))
        .route("/api/snapshot", get(get_snapshot))
        .route("/api/events", get(get_events))
        .route("/api/run", post(post_run))
        .route("/api/params", post(post_params))
        .route("/api/purchase", post(post_purchase))
        .with_state(bridge);

    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .unwrap_or_else(|error| panic!("demo bridge cannot bind {listen}: {error}"));
    eprintln!("GlassChain demo: http://{listen}/");
    eprintln!("Demonstration only: {0}", scenario::MODE_LABEL);
    if let Err(error) = axum::serve(listener, app).await {
        eprintln!("demo bridge ended: {error}");
    }
}
