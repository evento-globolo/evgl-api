use axum::{
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use uuid::Uuid;

use crate::state::AppState;

#[derive(Clone, Copy)]
pub struct User {
    pub id: Uuid,
}

#[derive(Deserialize)]
struct Claims {
    sub: String,
    exp: usize,
}

impl FromRequestParts<AppState> for User {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let header = parts.headers.get("authorization")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or((StatusCode::UNAUTHORIZED, "missing bearer token"))?;
        let token = decode::<Claims>(
            header,
            &DecodingKey::from_secret(state.jwt_secret.as_bytes()),
            &Validation::new(Algorithm::HS256),
        ).map_err(|_| (StatusCode::UNAUTHORIZED, "invalid bearer token"))?;
        let _ = token.claims.exp;
        let id = token.claims.sub.parse()
            .map_err(|_| (StatusCode::UNAUTHORIZED, "token subject must be a UUID"))?;
        Ok(User { id })
    }
}
