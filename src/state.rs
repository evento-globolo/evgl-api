use dashmap::DashMap;
use evgl_domain::JobUpdate;
use evgl_token_vault::TokenVault;
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::broadcast;
use url::Url;
use uuid::Uuid;

use crate::registry::ProviderRegistry;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub jwt_secret: Arc<String>,
    pub vault: Arc<TokenVault>,
    pub providers: ProviderRegistry,
    pub job_channels: Arc<DashMap<Uuid, broadcast::Sender<JobUpdate>>>,
    pub web_url: Url,
}

impl AppState {
    pub fn channel(&self, job_id: Uuid) -> broadcast::Sender<JobUpdate> {
        self.job_channels
            .entry(job_id)
            .or_insert_with(|| {
                let (sender, _) = broadcast::channel(128);
                sender
            })
            .clone()
    }
}
