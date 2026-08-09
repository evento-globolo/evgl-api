use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::{DateTime, Utc};
use evgl_domain::{CrossPostRequest, EventDraft, PublishTarget, Venue};
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
    Ok((StatusCode::CREATED, Json(event)))
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
    let key = headers.get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| ApiError::BadRequest("Idempotency-Key header is required".into()))?;
    let request = CrossPostRequest {
        event_id,
        targets: input.targets,
        idempotency_key: key.to_owned(),
    };
    request.validate().map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let _ = store::get_event(&state.db, user_id, event_id).await?;
    let enqueued = store::enqueue_job(
        &state.db, user_id, event_id, key, &request.targets
    ).await?;
    if enqueued.created {
        worker::spawn(state.clone(), enqueued.job.clone());
    }
    let status = if enqueued.created { StatusCode::ACCEPTED } else { StatusCode::OK };
    Ok((status, Json(enqueued.job)))
}
