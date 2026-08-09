use anyhow::{Context, Result};
use url::Url;

#[derive(Clone)]
pub struct Config {
    pub bind: String,
    pub public_url: Url,
    pub web_url: Url,
    pub database_url: String,
    pub jwt_secret: String,
    pub token_vault_key: String,
    pub eventbrite: Option<OAuthProviderConfig>,
    pub meetup: Option<OAuthProviderConfig>,
    pub meta: Option<OAuthProviderConfig>,
    pub meta_graph_version: String,
}

#[derive(Clone)]
pub struct OAuthProviderConfig {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: Url,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            bind: env_or("APP_BIND", "0.0.0.0:8080"),
            public_url: required("APP_PUBLIC_URL")?.parse().context("APP_PUBLIC_URL")?,
            web_url: required("APP_WEB_URL")?.parse().context("APP_WEB_URL")?,
            database_url: required("DATABASE_URL")?,
            jwt_secret: required("JWT_SECRET")?,
            token_vault_key: required("TOKEN_VAULT_KEY")?,
            eventbrite: oauth("EVENTBRITE")?,
            meetup: oauth("MEETUP")?,
            meta: oauth("META")?,
            meta_graph_version: env_or("META_GRAPH_VERSION", "v25.0"),
        })
    }
}

fn oauth(prefix: &str) -> Result<Option<OAuthProviderConfig>> {
    let client_id = std::env::var(format!("{prefix}_CLIENT_ID")).ok()
        .filter(|value| !value.trim().is_empty());
    let secret = std::env::var(format!("{prefix}_CLIENT_SECRET")).ok()
        .filter(|value| !value.trim().is_empty());
    let redirect = std::env::var(format!("{prefix}_REDIRECT_URI")).ok()
        .filter(|value| !value.trim().is_empty());
    match (client_id, secret, redirect) {
        (None, None, None) => Ok(None),
        (Some(client_id), Some(client_secret), Some(redirect_uri)) => Ok(Some(
            OAuthProviderConfig {
                client_id,
                client_secret,
                redirect_uri: redirect_uri.parse()
                    .with_context(|| format!("{prefix}_REDIRECT_URI"))?,
            },
        )),
        _ => anyhow::bail!(
            "{prefix}_CLIENT_ID, {prefix}_CLIENT_SECRET and {prefix}_REDIRECT_URI must be set together"
        ),
    }
}

fn required(key: &'static str) -> Result<String> {
    std::env::var(key).with_context(|| format!("missing {key}"))
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}
