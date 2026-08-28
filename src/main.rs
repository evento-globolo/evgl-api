mod auth;
mod config;
mod error;
mod events;
mod jobs;
mod oauth;
mod registry;
mod state;
mod store;
mod worker;

use axum::{
    http::{HeaderName, Method, StatusCode},
    routing::{delete, get, post},
    Json, Router,
};
use config::Config;
use dashmap::DashMap;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use tower_http::{
    cors::{Any, CorsLayer},
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};

use crate::{registry::ProviderRegistry, state::AppState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .json()
        .init();

    let config = Config::from_env()?;
    let db = PgPoolOptions::new()
        .max_connections(20)
        .connect(&config.database_url)
        .await?;
    sqlx::migrate!().run(&db).await?;

    let state = AppState {
        db,
        jwt_secret: Arc::new(config.jwt_secret.clone()),
        vault: Arc::new(evgl_token_vault::TokenVault::from_base64_key(
            &config.token_vault_key,
        )?),
        providers: ProviderRegistry::build(&config)?,
        job_channels: Arc::new(DashMap::new()),
        web_url: config.web_url.clone(),
    };

    let request_id = HeaderName::from_static("x-request-id");
    let app = Router::new()
        .route(
            "/healthz",
            get(|| async { Json(serde_json::json!({ "status": "ok", "service": "evgl-api" })) }),
        )
        .route("/readyz", get(|| async { StatusCode::NO_CONTENT }))
        .route("/v1/providers", get(oauth::providers))
        .route("/v1/oauth/{provider}/start", post(oauth::start))
        .route("/v1/oauth/{provider}/callback", get(oauth::callback))
        .route("/v1/connections/manual", post(oauth::create_manual))
        .route("/v1/connections", get(oauth::list_connections))
        .route("/v1/connections/{id}", delete(oauth::delete_connection))
        .route("/v1/events", post(events::create))
        .route("/v1/events/{id}", get(events::get))
        .route("/v1/events/{id}/cross-post", post(events::cross_post))
        .route("/v1/jobs/{id}", get(jobs::get))
        .route("/v1/jobs/{id}/ws", get(jobs::websocket))
        .layer(PropagateRequestIdLayer::new(request_id.clone()))
        .layer(SetRequestIdLayer::new(request_id, MakeRequestUuid))
        .layer(TraceLayer::new_for_http())
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_headers(Any)
                .allow_methods([Method::GET, Method::POST, Method::DELETE]),
        )
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&config.bind).await?;
    tracing::info!(bind = %config.bind, public_url = %config.public_url, "Evento Globolo API listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutdown signal received");
}
