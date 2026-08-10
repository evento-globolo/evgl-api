use std::{collections::HashMap, env, sync::Arc};

use anyhow::Context;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use sea_orm::{Database, DatabaseConnection};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::info;
use uuid::Uuid;

const MAX_TITLE_CHARS: usize = 200;
const MAX_DESCRIPTION_CHARS: usize = 20_000;
const MAX_LOCATION_CHARS: usize = 300;
const MAX_EVENT_CAPACITY: i32 = 1_000_000;

#[derive(Clone)]
struct AppState {
    db: Option<DatabaseConnection>,
    records: Arc<RwLock<HashMap<Uuid, Event>>>,
    events: broadcast::Sender<String>,
    supabase_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Event {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub title: String,
    pub description: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub venue: String,
    pub city: String,
    pub country: String,
    pub organizer_id: Uuid,
    pub capacity: i32,
}

#[derive(Debug, Deserialize)]
struct CreateEvent {
    pub title: String,
    pub description: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub venue: String,
    pub city: String,
    pub country: String,
    pub organizer_id: Uuid,
    pub capacity: i32,
}

impl CreateEvent {
    fn into_record(self, now: DateTime<Utc>) -> Result<Event, ValidationError> {
        if self.ends_at <= self.starts_at {
            return Err(ValidationError::new(
                "ends_at",
                "event end time must be after the start time",
            ));
        }
        if self.capacity <= 0 {
            return Err(ValidationError::new(
                "capacity",
                "event capacity must be greater than zero",
            ));
        }
        if self.capacity > MAX_EVENT_CAPACITY {
            return Err(ValidationError::new(
                "capacity",
                "event capacity exceeds the supported maximum",
            ));
        }
        if self.organizer_id.is_nil() {
            return Err(ValidationError::new(
                "organizer_id",
                "organizer id must not be nil",
            ));
        }

        Ok(Event {
            id: Uuid::new_v4(),
            created_at: now,
            updated_at: now,
            title: normalize_required(self.title, "title", MAX_TITLE_CHARS)?,
            description: normalize_bounded(
                self.description,
                "description",
                MAX_DESCRIPTION_CHARS,
            )?,
            starts_at: self.starts_at,
            ends_at: self.ends_at,
            venue: normalize_required(self.venue, "venue", MAX_LOCATION_CHARS)?,
            city: normalize_required(self.city, "city", MAX_LOCATION_CHARS)?,
            country: normalize_required(self.country, "country", MAX_LOCATION_CHARS)?,
            organizer_id: self.organizer_id,
            capacity: self.capacity,
        })
    }
}

fn normalize_required(
    value: String,
    field: &'static str,
    max_chars: usize,
) -> Result<String, ValidationError> {
    let normalized = value.trim();
    if normalized.is_empty() {
        return Err(ValidationError::new(field, "field must not be empty"));
    }
    if normalized.chars().count() > max_chars {
        return Err(ValidationError::new(
            field,
            "field exceeds the supported character limit",
        ));
    }
    Ok(normalized.to_owned())
}

fn normalize_bounded(
    value: String,
    field: &'static str,
    max_chars: usize,
) -> Result<String, ValidationError> {
    let normalized = value.trim();
    if normalized.chars().count() > max_chars {
        return Err(ValidationError::new(
            field,
            "field exceeds the supported character limit",
        ));
    }
    Ok(normalized.to_owned())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ValidationError {
    field: &'static str,
    message: &'static str,
}

impl ValidationError {
    const fn new(field: &'static str, message: &'static str) -> Self {
        Self { field, message }
    }
}

#[derive(Debug)]
enum ApiError {
    Validation(ValidationError),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            Self::Validation(error) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": "validation",
                    "field": error.field,
                    "message": error.message,
                })),
            )
                .into_response(),
        }
    }
}

#[derive(Debug, Serialize)]
struct Health {
    service: &'static str,
    status: &'static str,
    database_configured: bool,
    supabase_configured: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let db = match env::var("DATABASE_URL") {
        Ok(url) if !url.trim().is_empty() => {
            Some(Database::connect(url).await.context("connect database")?)
        }
        _ => None,
    };
    let (events, _) = broadcast::channel(512);
    let state = AppState {
        db,
        records: Arc::new(RwLock::new(HashMap::new())),
        events,
        supabase_url: env::var("SUPABASE_URL").ok(),
    };

    let app = Router::new()
        .route("/healthz", get(health))
        .route("/v1/events", get(list_records).post(create_record))
        .route("/v1/events/{id}", get(get_record))
        .route("/v1/ws", get(ws_upgrade))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into());
    let port = env::var("PORT").unwrap_or_else(|_| "8080".into());
    let listener = tokio::net::TcpListener::bind(format!("{host}:{port}")).await?;
    info!(address = %listener.local_addr()?, "Evento Globolo API listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn health(State(state): State<AppState>) -> Json<Health> {
    Json(Health {
        service: "evgl-api",
        status: "ok",
        database_configured: state.db.is_some(),
        supabase_configured: state.supabase_url.is_some(),
    })
}

async fn list_records(State(state): State<AppState>) -> Json<Vec<Event>> {
    Json(state.records.read().await.values().cloned().collect())
}

async fn get_record(Path(id): Path<Uuid>, State(state): State<AppState>) -> impl IntoResponse {
    match state.records.read().await.get(&id).cloned() {
        Some(record) => (StatusCode::OK, Json(record)).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn create_record(
    State(state): State<AppState>,
    Json(input): Json<CreateEvent>,
) -> Result<(StatusCode, Json<Event>), ApiError> {
    let record = input
        .into_record(Utc::now())
        .map_err(ApiError::Validation)?;
    state
        .records
        .write()
        .await
        .insert(record.id, record.clone());
    let _ = state
        .events
        .send(serde_json::to_string(&record).unwrap_or_default());
    Ok((StatusCode::CREATED, Json(record)))
}

async fn ws_upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| websocket(socket, state))
}

async fn websocket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let mut events = state.events.subscribe();
    let send_task = tokio::spawn(async move {
        while let Ok(event) = events.recv().await {
            if sender.send(Message::Text(event.into())).await.is_err() {
                break;
            }
        }
    });
    let receive_task = tokio::spawn(async move {
        while let Some(Ok(message)) = receiver.next().await {
            if matches!(message, Message::Close(_)) {
                break;
            }
        }
    });
    tokio::select! { _ = send_task => {}, _ = receive_task => {} }
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn valid_input() -> CreateEvent {
        CreateEvent {
            title: "  Rust in Lima  ".into(),
            description: "  A community systems event.  ".into(),
            starts_at: Utc
                .with_ymd_and_hms(2026, 9, 1, 18, 0, 0)
                .single()
                .expect("valid start time"),
            ends_at: Utc
                .with_ymd_and_hms(2026, 9, 1, 20, 0, 0)
                .single()
                .expect("valid end time"),
            venue: "  Centro  ".into(),
            city: "  Lima  ".into(),
            country: "  PE  ".into(),
            organizer_id: Uuid::new_v4(),
            capacity: 250,
        }
    }

    #[test]
    fn accepts_and_normalizes_valid_event() {
        let record = valid_input()
            .into_record(Utc::now())
            .expect("valid event should be accepted");

        assert_eq!(record.title, "Rust in Lima");
        assert_eq!(record.description, "A community systems event.");
        assert_eq!(record.venue, "Centro");
        assert_eq!(record.city, "Lima");
        assert_eq!(record.country, "PE");
    }

    #[test]
    fn rejects_end_at_or_before_start() {
        let mut input = valid_input();
        input.ends_at = input.starts_at;

        assert_eq!(
            input.into_record(Utc::now()),
            Err(ValidationError::new(
                "ends_at",
                "event end time must be after the start time",
            ))
        );
    }

    #[test]
    fn rejects_non_positive_capacity() {
        for capacity in [-1, 0] {
            let mut input = valid_input();
            input.capacity = capacity;

            assert_eq!(
                input.into_record(Utc::now()),
                Err(ValidationError::new(
                    "capacity",
                    "event capacity must be greater than zero",
                ))
            );
        }
    }

    #[test]
    fn rejects_nil_organizer() {
        let mut input = valid_input();
        input.organizer_id = Uuid::nil();

        assert_eq!(
            input.into_record(Utc::now()),
            Err(ValidationError::new(
                "organizer_id",
                "organizer id must not be nil",
            ))
        );
    }

    #[test]
    fn rejects_oversized_title() {
        let mut input = valid_input();
        input.title = "x".repeat(MAX_TITLE_CHARS + 1);

        assert_eq!(
            input.into_record(Utc::now()),
            Err(ValidationError::new(
                "title",
                "field exceeds the supported character limit",
            ))
        );
    }
}
