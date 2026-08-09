use axum::{
    extract::{Path, Query, State},
    response::Redirect,
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use evgl_domain::ProviderKind;
use evgl_provider_sdk::TokenSet;
use rand::{rngs::OsRng, RngCore};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::str::FromStr;
use uuid::Uuid;

use crate::{auth::User, error::ApiError, state::AppState, store};

#[derive(Serialize)]
pub struct ProviderView {
    pub capabilities: evgl_domain::ProviderCapabilities,
    pub configured: bool,
}

pub async fn providers(State(state): State<AppState>) -> Json<Vec<ProviderView>> {
    Json(
        state
            .providers
            .capabilities()
            .into_iter()
            .map(|(capabilities, configured)| ProviderView {
                capabilities,
                configured,
            })
            .collect(),
    )
}

pub async fn start(
    State(state): State<AppState>,
    User { id: user_id }: User,
    Path(provider): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let provider = ProviderKind::from_str(&provider)
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let adapter = state
        .providers
        .get(provider)
        .ok_or_else(|| ApiError::Conflict(format!("{provider} is not configured")))?;
    if !adapter.capabilities().oauth {
        return Err(ApiError::BadRequest(format!(
            "{provider} uses a manual or secret-based connection flow"
        )));
    }
    let state_token = random_urlsafe(32);
    let verifier = random_urlsafe(48);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let oauth = adapter.authorization_url(&state_token, Some(&challenge))?;
    store::create_oauth_session(
        &state.db,
        &hash_state(&state_token),
        user_id,
        provider,
        Some(&verifier),
        oauth.uses_pkce,
    )
    .await?;
    Ok(Json(json!({
        "provider": provider,
        "authorization_url": oauth.authorization_url,
        "expires_in_seconds": 600
    })))
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: String,
    error: Option<String>,
    error_description: Option<String>,
}

pub async fn callback(
    State(state): State<AppState>,
    Path(provider_path): Path<String>,
    Query(query): Query<CallbackQuery>,
) -> Result<Redirect, ApiError> {
    if let Some(error) = query.error {
        return Err(ApiError::Unauthorized(format!(
            "provider authorization failed: {error}: {}",
            query.error_description.unwrap_or_default()
        )));
    }
    let session = store::consume_oauth_session(&state.db, &hash_state(&query.state)).await?;
    let provider = ProviderKind::from_str(&session.provider)
        .map_err(|error| ApiError::Internal(error.into()))?;
    let path_provider = ProviderKind::from_str(&provider_path)
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    if provider != path_provider {
        return Err(ApiError::Unauthorized("OAuth provider mismatch".into()));
    }
    let adapter = state
        .providers
        .get(provider)
        .ok_or_else(|| ApiError::Conflict(format!("{provider} is not configured")))?;
    let code = query
        .code
        .ok_or_else(|| ApiError::BadRequest("missing OAuth code".into()))?;
    let tokens = adapter
        .exchange_code(
            &code,
            session
                .uses_pkce
                .then_some(session.pkce_verifier.as_deref())
                .flatten(),
        )
        .await?;
    let accounts = adapter.resolve_accounts(&tokens).await?;
    let mut connection_ids = Vec::with_capacity(accounts.len());
    for account in accounts {
        let account_tokens = account.token_override.unwrap_or_else(|| tokens.clone());
        let aad = format!("{}:{}:{}", session.user_id, provider, account.account_key);
        let envelope = state.vault.encrypt(aad.as_bytes(), &account_tokens)?;
        let row = store::upsert_connection(
            &state.db,
            session.user_id,
            provider,
            &account.account_key,
            &account.display_name,
            &account.metadata,
            &envelope,
        )
        .await?;
        connection_ids.push(row.id);
    }
    let mut destination = state
        .web_url
        .join("integrations")
        .map_err(|error| ApiError::Internal(error.into()))?;
    destination
        .query_pairs_mut()
        .append_pair("connected", provider.as_str())
        .append_pair("accounts", &connection_ids.len().to_string());
    Ok(Redirect::to(destination.as_str()))
}

#[derive(Deserialize)]
pub struct ManualConnectionInput {
    pub provider: ProviderKind,
    pub account_key: String,
    pub display_name: String,
    #[serde(default)]
    pub metadata: Value,
    pub secret: Option<String>,
}

pub async fn create_manual(
    State(state): State<AppState>,
    User { id: user_id }: User,
    Json(input): Json<ManualConnectionInput>,
) -> Result<Json<store::ConnectionRow>, ApiError> {
    let adapter = state
        .providers
        .get(input.provider)
        .ok_or_else(|| ApiError::Conflict(format!("{} is unavailable", input.provider)))?;
    if adapter.capabilities().oauth {
        return Err(ApiError::BadRequest(
            "use the OAuth start route for this provider".into(),
        ));
    }
    if input.account_key.trim().is_empty() || input.display_name.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "account_key and display_name are required".into(),
        ));
    }
    if input.provider == ProviderKind::GenericWebhook {
        let endpoint = input
            .metadata
            .get("endpoint")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::BadRequest("metadata.endpoint is required".into()))?;
        let parsed: url::Url = endpoint
            .parse()
            .map_err(|_| ApiError::BadRequest("metadata.endpoint is invalid".into()))?;
        validate_webhook_endpoint(&parsed)?;
        if input
            .secret
            .as_deref()
            .is_none_or(|secret| secret.len() < 32)
        {
            return Err(ApiError::BadRequest(
                "generic_webhook requires a secret containing at least 32 characters".into(),
            ));
        }
    }
    let secret = input.secret.unwrap_or_else(|| random_urlsafe(32));
    let tokens = TokenSet {
        access_token: SecretString::from(secret),
        refresh_token: None,
        expires_at: None,
        scopes: vec![],
        provider_data: Value::Null,
    };
    let aad = format!("{}:{}:{}", user_id, input.provider, input.account_key);
    let envelope = state.vault.encrypt(aad.as_bytes(), &tokens)?;
    let row = store::upsert_connection(
        &state.db,
        user_id,
        input.provider,
        &input.account_key,
        &input.display_name,
        &input.metadata,
        &envelope,
    )
    .await?;
    Ok(Json(row))
}

pub async fn list_connections(
    State(state): State<AppState>,
    User { id: user_id }: User,
) -> Result<Json<Vec<store::ConnectionRow>>, ApiError> {
    Ok(Json(store::list_connections(&state.db, user_id).await?))
}

pub async fn delete_connection(
    State(state): State<AppState>,
    User { id: user_id }: User,
    Path(id): Path<Uuid>,
) -> Result<axum::http::StatusCode, ApiError> {
    store::delete_connection(&state.db, user_id, id).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

fn validate_webhook_endpoint(endpoint: &url::Url) -> Result<(), ApiError> {
    fn blocked_ipv4(address: std::net::Ipv4Addr) -> bool {
        address.is_private()
            || address.is_loopback()
            || address.is_link_local()
            || address.is_broadcast()
            || address.is_documentation()
            || address.is_unspecified()
    }

    fn blocked_ipv6(address: std::net::Ipv6Addr) -> bool {
        address.is_loopback()
            || address.is_unspecified()
            || address.is_unique_local()
            || address.is_unicast_link_local()
            || address.to_ipv4_mapped().is_some_and(blocked_ipv4)
    }

    if endpoint.scheme() != "https" {
        return Err(ApiError::BadRequest(
            "webhook endpoint must use HTTPS".into(),
        ));
    }

    let blocked = match endpoint
        .host()
        .ok_or_else(|| ApiError::BadRequest("webhook endpoint must include a host".into()))?
    {
        url::Host::Domain(host) => {
            if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
                return Err(ApiError::BadRequest(
                    "webhook endpoint cannot target localhost".into(),
                ));
            }
            false
        }
        url::Host::Ipv4(address) => blocked_ipv4(address),
        url::Host::Ipv6(address) => blocked_ipv6(address),
    };

    if blocked {
        return Err(ApiError::BadRequest(
            "webhook endpoint cannot target a private or local address".into(),
        ));
    }
    Ok(())
}

fn random_urlsafe(bytes: usize) -> String {
    let mut buffer = vec![0_u8; bytes];
    OsRng.fill_bytes(&mut buffer);
    URL_SAFE_NO_PAD.encode(buffer)
}

fn hash_state(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(value.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::validate_webhook_endpoint;
    use url::Url;

    #[test]
    fn webhook_endpoint_validation_rejects_local_networks() {
        for value in [
            "http://events.example.com/hook",
            "https://localhost/hook",
            "https://127.0.0.1/hook",
            "https://192.168.1.2/hook",
            "https://[::1]/hook",
            "https://[::]/hook",
            "https://[fc00::1]/hook",
            "https://[fe80::1]/hook",
            "https://[::ffff:127.0.0.1]/hook",
        ] {
            assert!(
                validate_webhook_endpoint(&Url::parse(value).unwrap()).is_err(),
                "{value}",
            );
        }
    }

    #[test]
    fn webhook_endpoint_validation_accepts_public_https() {
        assert!(validate_webhook_endpoint(
            &Url::parse("https://events.example.com/hooks/evgl").unwrap(),
        )
        .is_ok());
    }
}
