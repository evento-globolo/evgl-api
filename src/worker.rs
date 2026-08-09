use chrono::Utc;
use evgl_domain::{JobUpdate, ProviderKind, PublicationStatus};
use evgl_provider_sdk::PublishContext;
use std::{str::FromStr, time::Duration};

use crate::{error::ApiError, state::AppState, store};

pub fn spawn(state: AppState, job: store::JobRow) {
    tokio::spawn(async move {
        if let Err(error) = run(state.clone(), job.clone()).await {
            tracing::error!(job_id = %job.id, %error, "cross-post job failed");
        }
    });
}

async fn run(state: AppState, job: store::JobRow) -> Result<(), ApiError> {
    let event = store::get_event(&state.db, job.user_id, job.event_id).await?;
    let targets = store::claim_targets(&state.db, job.id).await?;
    emit(
        &state,
        &job,
        None,
        PublicationStatus::Running,
        0,
        "job started",
        None,
    );

    for target in targets {
        let provider = match ProviderKind::from_str(&target.provider) {
            Ok(provider) => provider,
            Err(error) => {
                fail_target(
                    &state,
                    &job,
                    target.id,
                    None,
                    0,
                    &format!("invalid provider on queued target: {error}"),
                )
                .await?;
                continue;
            }
        };
        let connection = match store::get_connection(
            &state.db,
            job.user_id,
            target.connection_id,
        )
        .await
        {
            Ok(connection) => connection,
            Err(error) => {
                fail_target(
                    &state,
                    &job,
                    target.id,
                    Some(provider),
                    0,
                    &format!("provider connection is unavailable: {error}"),
                )
                .await?;
                continue;
            }
        };
        let Some(adapter) = state.providers.get(provider) else {
            fail_target(
                &state,
                &job,
                target.id,
                Some(provider),
                0,
                &format!("{provider} is not configured on this deployment"),
            )
            .await?;
            continue;
        };
        let tokens = match state.vault.decrypt(
            connection.aad().as_bytes(),
            &connection.token_envelope,
        ) {
            Ok(tokens) => tokens,
            Err(error) => {
                fail_target(
                    &state,
                    &job,
                    target.id,
                    Some(provider),
                    0,
                    &format!("could not decrypt provider credentials: {error}"),
                )
                .await?;
                continue;
            }
        };
        let context = PublishContext {
            account_key: connection.account_key.clone(),
            account_metadata: connection.metadata.clone(),
            target_options: target.options.clone(),
        };
        let mut published = None;
        let mut final_error = None;
        let mut final_attempt = 0_i32;
        for attempt in 1_i32..=3 {
            final_attempt = attempt;
            store::target_running(&state.db, target.id, attempt).await?;
            emit(
                &state,
                &job,
                Some(provider),
                PublicationStatus::Running,
                attempt as u32,
                "provider request started",
                None,
            );
            match adapter.publish(&tokens, &event, &context).await {
                Ok(publication) => {
                    let value = serde_json::to_value(&publication)
                        .map_err(|error| ApiError::Internal(error.into()))?;
                    store::target_complete(
                        &state.db,
                        target.id,
                        publication.status,
                        &value,
                    )
                    .await?;
                    emit(
                        &state,
                        &job,
                        Some(provider),
                        publication.status,
                        attempt as u32,
                        "provider request completed",
                        Some(publication.clone()),
                    );
                    published = Some(publication);
                    break;
                }
                Err(error) if attempt < 3 && is_retryable(&error) => {
                    emit(
                        &state,
                        &job,
                        Some(provider),
                        PublicationStatus::Retrying,
                        attempt as u32,
                        &error.to_string(),
                        None,
                    );
                    tokio::time::sleep(Duration::from_secs(2_u64.pow(attempt as u32))).await;
                }
                Err(error) => {
                    final_error = Some(error.to_string());
                    break;
                }
            }
        }
        if published.is_none() {
            let error = final_error.unwrap_or_else(|| "provider publish failed".into());
            fail_target(
                &state,
                &job,
                target.id,
                Some(provider),
                final_attempt,
                &error,
            )
            .await?;
        }
    }

    store::finish_job(&state.db, job.id).await?;
    let (current, _) = store::get_job(&state.db, job.user_id, job.id).await?;
    let status = match current.status.as_str() {
        "failed" => PublicationStatus::Failed,
        "action_required" => PublicationStatus::ActionRequired,
        _ => PublicationStatus::Published,
    };
    emit(&state, &job, None, status, 0, "job finished", None);
    Ok(())
}

async fn fail_target(
    state: &AppState,
    job: &store::JobRow,
    target_id: uuid::Uuid,
    provider: Option<ProviderKind>,
    attempt: i32,
    error: &str,
) -> Result<(), ApiError> {
    store::target_failed(&state.db, target_id, attempt, error).await?;
    emit(
        state,
        job,
        provider,
        PublicationStatus::Failed,
        attempt.max(0) as u32,
        error,
        None,
    );
    Ok(())
}

fn is_retryable(error: &evgl_provider_sdk::ProviderError) -> bool {
    match error {
        evgl_provider_sdk::ProviderError::Network(_) => true,
        evgl_provider_sdk::ProviderError::Remote { status, .. } => {
            *status == 429 || *status >= 500
        }
        _ => false,
    }
}

fn emit(
    state: &AppState,
    job: &store::JobRow,
    provider: Option<ProviderKind>,
    status: PublicationStatus,
    attempt: u32,
    message: &str,
    publication: Option<evgl_domain::Publication>,
) {
    let _ = state.channel(job.id).send(JobUpdate {
        job_id: job.id,
        event_id: job.event_id,
        provider,
        status,
        attempt,
        message: message.to_owned(),
        occurred_at: Utc::now(),
        publication,
    });
}
