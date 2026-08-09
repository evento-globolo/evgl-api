use axum::{
    extract::{
        Path, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::Response,
    Json,
};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use uuid::Uuid;

use crate::{auth::User, error::ApiError, state::AppState, store};

#[derive(Serialize)]
pub struct JobView {
    pub job: store::JobRow,
    pub targets: Vec<TargetView>,
}

#[derive(Serialize)]
pub struct TargetView {
    pub id: Uuid,
    pub connection_id: Uuid,
    pub provider: String,
    pub options: serde_json::Value,
    pub status: String,
    pub attempt: i32,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

pub async fn get(
    State(state): State<AppState>,
    User { id: user_id }: User,
    Path(job_id): Path<Uuid>,
) -> Result<Json<JobView>, ApiError> {
    let (job, targets) = store::get_job(&state.db, user_id, job_id).await?;
    Ok(Json(JobView {
        job,
        targets: targets
            .into_iter()
            .map(|target| TargetView {
                id: target.id,
                connection_id: target.connection_id,
                provider: target.provider,
                options: target.options,
                status: target.status,
                attempt: target.attempt,
                result: target.result,
                error: target.error,
            })
            .collect(),
    }))
}

pub async fn websocket(
    State(state): State<AppState>,
    User { id: user_id }: User,
    Path(job_id): Path<Uuid>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let _ = store::get_job(&state.db, user_id, job_id).await?;
    let receiver = state.channel(job_id).subscribe();
    Ok(ws.on_upgrade(move |socket| stream(socket, receiver)))
}

async fn stream(
    socket: WebSocket,
    mut receiver: tokio::sync::broadcast::Receiver<evgl_domain::JobUpdate>,
) {
    // Split the bidirectional socket so the select branches do not create two
    // simultaneous mutable borrows of the same WebSocket value.
    let (mut sender, mut incoming) = socket.split();
    loop {
        tokio::select! {
            update = receiver.recv() => match update {
                Ok(update) => match serde_json::to_string(&update) {
                    Ok(payload) => {
                        if sender.send(Message::Text(payload.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        tracing::error!(%error, "could not serialize job update");
                        break;
                    }
                },
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    let payload = serde_json::json!({
                        "type": "resync_required",
                        "skipped": skipped
                    }).to_string();
                    if sender.send(Message::Text(payload.into())).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            },
            message = incoming.next() => match message {
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(Message::Ping(data))) => {
                    if sender.send(Message::Pong(data)).await.is_err() {
                        break;
                    }
                }
                Some(Err(_)) => break,
                _ => {}
            }
        }
    }
}
