use chrono::{DateTime, Utc};
use evgl_domain::{EventDraft, ProviderKind, PublicationStatus, PublishTarget};
use serde::Serialize;
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::error::ApiError;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct ConnectionRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub provider: String,
    pub account_key: String,
    pub display_name: String,
    pub metadata: Value,
    #[serde(skip_serializing)]
    pub token_envelope: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ConnectionRow {
    pub fn aad(&self) -> String {
        format!("{}:{}:{}", self.user_id, self.provider, self.account_key)
    }
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct JobRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub event_id: Uuid,
    pub idempotency_key: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct TargetRow {
    pub id: Uuid,
    pub job_id: Uuid,
    pub connection_id: Uuid,
    pub provider: String,
    pub options: Value,
    pub status: String,
    pub attempt: i32,
    pub result: Option<Value>,
    pub error: Option<String>,
}

pub async fn create_oauth_session(
    db: &PgPool,
    state_hash: &str,
    user_id: Uuid,
    provider: ProviderKind,
    pkce_verifier: Option<&str>,
    uses_pkce: bool,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO oauth_sessions
         (state_hash, user_id, provider, pkce_verifier, uses_pkce, expires_at)
         VALUES ($1, $2, $3, $4, $5, now() + interval '10 minutes')",
    )
    .bind(state_hash)
    .bind(user_id)
    .bind(provider.as_str())
    .bind(pkce_verifier)
    .bind(uses_pkce)
    .execute(db)
    .await?;
    Ok(())
}

#[derive(FromRow)]
pub struct OAuthSession {
    pub user_id: Uuid,
    pub provider: String,
    pub pkce_verifier: Option<String>,
    pub uses_pkce: bool,
}

pub async fn consume_oauth_session(
    db: &PgPool,
    state_hash: &str,
) -> Result<OAuthSession, ApiError> {
    let mut tx = db.begin().await?;
    let session = sqlx::query_as::<_, OAuthSession>(
        "DELETE FROM oauth_sessions
         WHERE state_hash = $1 AND expires_at > now()
         RETURNING user_id, provider, pkce_verifier, uses_pkce",
    )
    .bind(state_hash)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::Unauthorized("OAuth state is invalid or expired".into()))?;
    tx.commit().await?;
    Ok(session)
}

pub async fn upsert_connection(
    db: &PgPool,
    user_id: Uuid,
    provider: ProviderKind,
    account_key: &str,
    display_name: &str,
    metadata: &Value,
    token_envelope: &str,
) -> Result<ConnectionRow, ApiError> {
    Ok(sqlx::query_as::<_, ConnectionRow>(
        "INSERT INTO provider_connections
         (id, user_id, provider, account_key, display_name, metadata, token_envelope)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (user_id, provider, account_key) DO UPDATE SET
           display_name = EXCLUDED.display_name,
           metadata = EXCLUDED.metadata,
           token_envelope = EXCLUDED.token_envelope,
           updated_at = now()
         RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(user_id)
    .bind(provider.as_str())
    .bind(account_key)
    .bind(display_name)
    .bind(metadata)
    .bind(token_envelope)
    .fetch_one(db)
    .await?)
}

pub async fn list_connections(db: &PgPool, user_id: Uuid) -> Result<Vec<ConnectionRow>, ApiError> {
    Ok(sqlx::query_as::<_, ConnectionRow>(
        "SELECT * FROM provider_connections WHERE user_id = $1 ORDER BY provider, display_name",
    )
    .bind(user_id)
    .fetch_all(db)
    .await?)
}

pub async fn get_connection(
    db: &PgPool,
    user_id: Uuid,
    id: Uuid,
) -> Result<ConnectionRow, ApiError> {
    sqlx::query_as::<_, ConnectionRow>(
        "SELECT * FROM provider_connections WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("connection not found".into()))
}

pub async fn delete_connection(db: &PgPool, user_id: Uuid, id: Uuid) -> Result<(), ApiError> {
    let result = sqlx::query("DELETE FROM provider_connections WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .execute(db)
        .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("connection not found".into()));
    }
    Ok(())
}

pub async fn create_event(db: &PgPool, event: &EventDraft) -> Result<(), ApiError> {
    event
        .validate()
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    sqlx::query("INSERT INTO events (id, owner_id, document) VALUES ($1, $2, $3)")
        .bind(event.id)
        .bind(event.owner_id)
        .bind(serde_json::to_value(event).map_err(|error| ApiError::Internal(error.into()))?)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn get_event(db: &PgPool, user_id: Uuid, event_id: Uuid) -> Result<EventDraft, ApiError> {
    let document: Option<Value> =
        sqlx::query_scalar("SELECT document FROM events WHERE id = $1 AND owner_id = $2")
            .bind(event_id)
            .bind(user_id)
            .fetch_optional(db)
            .await?;
    let document = document.ok_or_else(|| ApiError::NotFound("event not found".into()))?;
    serde_json::from_value(document).map_err(|error| ApiError::Internal(error.into()))
}

pub async fn list_events(db: &PgPool, user_id: Uuid) -> Result<Vec<EventDraft>, ApiError> {
    let documents: Vec<Value> = sqlx::query_scalar(
        "SELECT document FROM events WHERE owner_id = $1 ORDER BY created_at DESC LIMIT 100",
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    documents
        .into_iter()
        .map(|document| {
            serde_json::from_value(document).map_err(|error| ApiError::Internal(error.into()))
        })
        .collect()
}

pub struct EnqueueResult {
    pub job: JobRow,
    pub created: bool,
}

pub async fn enqueue_job(
    db: &PgPool,
    user_id: Uuid,
    event_id: Uuid,
    idempotency_key: &str,
    targets: &[PublishTarget],
) -> Result<EnqueueResult, ApiError> {
    let mut tx = db.begin().await?;
    if let Some(existing) = sqlx::query_as::<_, JobRow>(
        "SELECT * FROM cross_post_jobs WHERE user_id = $1 AND idempotency_key = $2",
    )
    .bind(user_id)
    .bind(idempotency_key)
    .fetch_optional(&mut *tx)
    .await?
    {
        tx.commit().await?;
        return Ok(EnqueueResult {
            job: existing,
            created: false,
        });
    }
    let job_id = Uuid::new_v4();
    let job = sqlx::query_as::<_, JobRow>(
        "INSERT INTO cross_post_jobs
         (id, user_id, event_id, idempotency_key, status)
         VALUES ($1, $2, $3, $4, 'queued') RETURNING *",
    )
    .bind(job_id)
    .bind(user_id)
    .bind(event_id)
    .bind(idempotency_key)
    .fetch_one(&mut *tx)
    .await?;
    for target in targets {
        let connection: Option<String> = sqlx::query_scalar(
            "SELECT provider FROM provider_connections WHERE id = $1 AND user_id = $2",
        )
        .bind(target.connection_id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        let connection_provider = connection.ok_or_else(|| {
            ApiError::BadRequest(format!("connection {} was not found", target.connection_id))
        })?;
        if connection_provider != target.provider.as_str() {
            return Err(ApiError::BadRequest(format!(
                "connection {} belongs to {}, not {}",
                target.connection_id, connection_provider, target.provider
            )));
        }
        sqlx::query(
            "INSERT INTO cross_post_targets
             (id, job_id, connection_id, provider, options, status)
             VALUES ($1, $2, $3, $4, $5, 'queued')",
        )
        .bind(Uuid::new_v4())
        .bind(job_id)
        .bind(target.connection_id)
        .bind(target.provider.as_str())
        .bind(&target.options)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(EnqueueResult { job, created: true })
}

pub async fn get_job(
    db: &PgPool,
    user_id: Uuid,
    job_id: Uuid,
) -> Result<(JobRow, Vec<TargetRow>), ApiError> {
    let job =
        sqlx::query_as::<_, JobRow>("SELECT * FROM cross_post_jobs WHERE id = $1 AND user_id = $2")
            .bind(job_id)
            .bind(user_id)
            .fetch_optional(db)
            .await?
            .ok_or_else(|| ApiError::NotFound("job not found".into()))?;
    let targets = sqlx::query_as::<_, TargetRow>(
        "SELECT * FROM cross_post_targets WHERE job_id = $1 ORDER BY id",
    )
    .bind(job_id)
    .fetch_all(db)
    .await?;
    Ok((job, targets))
}

pub async fn claim_targets(db: &PgPool, job_id: Uuid) -> Result<Vec<TargetRow>, ApiError> {
    sqlx::query("UPDATE cross_post_jobs SET status = 'running', updated_at = now() WHERE id = $1")
        .bind(job_id)
        .execute(db)
        .await?;
    Ok(sqlx::query_as::<_, TargetRow>(
        "SELECT * FROM cross_post_targets WHERE job_id = $1 ORDER BY id",
    )
    .bind(job_id)
    .fetch_all(db)
    .await?)
}

pub async fn target_running(db: &PgPool, target_id: Uuid, attempt: i32) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE cross_post_targets SET status = 'running', attempt = $2, error = NULL WHERE id = $1"
    ).bind(target_id).bind(attempt).execute(db).await?;
    Ok(())
}

pub async fn target_complete(
    db: &PgPool,
    target_id: Uuid,
    status: PublicationStatus,
    result: &Value,
) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE cross_post_targets SET status = $2, result = $3, error = NULL WHERE id = $1",
    )
    .bind(target_id)
    .bind(status_name(status))
    .bind(result)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn target_failed(
    db: &PgPool,
    target_id: Uuid,
    attempt: i32,
    error: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE cross_post_targets SET status = 'failed', attempt = $2, error = $3 WHERE id = $1",
    )
    .bind(target_id)
    .bind(attempt)
    .bind(error)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn finish_job(db: &PgPool, job_id: Uuid) -> Result<(), ApiError> {
    let failures: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cross_post_targets WHERE job_id = $1 AND status = 'failed'",
    )
    .bind(job_id)
    .fetch_one(db)
    .await?;
    let actions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cross_post_targets WHERE job_id = $1 AND status = 'action_required'",
    )
    .bind(job_id)
    .fetch_one(db)
    .await?;
    let status = if failures > 0 {
        "failed"
    } else if actions > 0 {
        "action_required"
    } else {
        "published"
    };
    sqlx::query("UPDATE cross_post_jobs SET status = $2, updated_at = now() WHERE id = $1")
        .bind(job_id)
        .bind(status)
        .execute(db)
        .await?;
    Ok(())
}

fn status_name(status: PublicationStatus) -> &'static str {
    match status {
        PublicationStatus::Queued => "queued",
        PublicationStatus::Running => "running",
        PublicationStatus::Published => "published",
        PublicationStatus::ActionRequired => "action_required",
        PublicationStatus::Retrying => "retrying",
        PublicationStatus::Failed => "failed",
    }
}
