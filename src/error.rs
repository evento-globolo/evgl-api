use axum::{http::StatusCode, response::{IntoResponse, Response}, Json};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Provider(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            Self::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            Self::Unauthorized(_) => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Self::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            Self::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            Self::Provider(_) => (StatusCode::BAD_GATEWAY, "provider_error"),
            Self::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
        };
        tracing::error!(error = %self, ?status, "request failed");
        (status, Json(json!({ "error": code, "message": self.to_string() }))).into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self { Self::Internal(error.into()) }
}

impl From<evgl_provider_sdk::ProviderError> for ApiError {
    fn from(error: evgl_provider_sdk::ProviderError) -> Self {
        Self::Provider(error.to_string())
    }
}

impl From<evgl_token_vault::VaultError> for ApiError {
    fn from(error: evgl_token_vault::VaultError) -> Self {
        Self::Internal(error.into())
    }
}
