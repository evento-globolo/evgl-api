use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, State, WebSocketUpgrade,
    },
    http::{HeaderMap, StatusCode},
    response::Response,
    Json,
};
use chrono::{DateTime, Utc};
use evgl_domain::{CrossPostRequest, EventDraft, PublishTarget, Venue};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::collections::BTreeMap;
use url::Url;
use uuid::Uuid;

use crate::{auth::User, error::ApiError, state::AppState, store, worker};

#[derive(Deserialize)]
pub struct CreateEventInput {
    pub title: String,
    pub summary: String,
    pub description_html: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub timezone: String,
    pub canonical_url: Url,
    pub online_url: Option<Url>,
    pub venue: Option<Venue>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, serde_json::Value>,
}

pub async fn create(
    State(state): State<AppState>,
    User { id: user_id }: User,
    Json(input): Json<CreateEventInput>,
) -> Result<(StatusCode, Json<EventDraft>), ApiError> {
    let event = EventDraft {
        id: Uuid::new_v4(),
        owner_id: user_id,
        title: input.title,
        summary: input.summary,
        description_html: input.description_html,
        starts_at: input.starts_at,
        ends_at: input.ends_at,
        timezone: input.timezone,
        canonical_url: input.canonical_url,
        online_url: input.online_url,
        venue: input.venue,
        tags: input.tags,
        metadata: input.metadata,
    };
    store::create_event(&state.db, &event).await?;
    let _ = state.event_channel.send(event.clone());
    Ok((StatusCode::CREATED, Json(event)))
}

pub async fn list(
    State(state): State<AppState>,
    User { id: user_id }: User,
) -> Result<Json<Vec<EventDraft>>, ApiError> {
    Ok(Json(store::list_events(&state.db, user_id).await?))
}

pub async fn get(
    State(state): State<AppState>,
    User { id: user_id }: User,
    Path(id): Path<Uuid>,
) -> Result<Json<EventDraft>, ApiError> {
    Ok(Json(store::get_event(&state.db, user_id, id).await?))
}

#[derive(Deserialize)]
pub struct CrossPostInput {
    pub targets: Vec<PublishTarget>,
}

pub async fn cross_post(
    State(state): State<AppState>,
    User { id: user_id }: User,
    Path(event_id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<CrossPostInput>,
) -> Result<(StatusCode, Json<store::JobRow>), ApiError> {
    let key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| ApiError::BadRequest("Idempotency-Key header is required".into()))?;
    let request = CrossPostRequest {
        event_id,
        targets: input.targets,
        idempotency_key: key.to_owned(),
    };
    request
        .validate()
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let _ = store::get_event(&state.db, user_id, event_id).await?;
    let enqueued = store::enqueue_job(&state.db, user_id, event_id, key, &request.targets).await?;
    if enqueued.created {
        worker::spawn(state.clone(), enqueued.job.clone());
    }
    let status = if enqueued.created {
        StatusCode::ACCEPTED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(enqueued.job)))
}

pub async fn websocket(
    State(state): State<AppState>,
    User { id: user_id }: User,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let receiver = state.event_channel.subscribe();
    Ok(ws.on_upgrade(move |socket| stream_events(socket, receiver, user_id)))
}

async fn stream_events(
    socket: WebSocket,
    mut receiver: tokio::sync::broadcast::Receiver<EventDraft>,
    user_id: Uuid,
) {
    let (mut sender, mut incoming) = socket.split();
    loop {
        tokio::select! {
            update = receiver.recv() => match update {
                Ok(event) if event.owner_id == user_id => {
                    let payload = serde_json::json!({
                        "event_id": event.id,
                        "event_type": "event.created",
                        "occurred_at": Utc::now(),
                        "data": event,
                    });
                    match serde_json::to_string(&payload) {
                        Ok(payload) => {
                            if sender.send(Message::Text(payload.into())).await.is_err() {
                                break;
                            }
                        }
                        Err(error) => {
                            tracing::error!(%error, "could not serialize event update");
                            break;
                        }
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    let payload = serde_json::json!({
                        "type": "resync_required",
                        "skipped": skipped,
                    }).to_string();
                    if sender.send(Message::Text(payload.into())).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            },
            message = incoming.next() => match message {
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(Message::Ping(data)))
                    if sender.send(Message::Pong(data.clone())).await.is_err() => break,
                Some(Err(_)) => break,
                _ => {}
            }
        }
    }
}
