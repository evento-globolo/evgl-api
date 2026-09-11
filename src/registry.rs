use evgl_domain::{ProviderCapabilities, ProviderKind};
use evgl_provider_sdk::{OAuthClient, ProviderAdapter};
use evgl_providers::{
    CraigslistAdapter, EventbriteAdapter, GenericWebhookAdapter, MeetupAdapter,
    MetaFacebookPageAdapter,
};
use secrecy::SecretString;
use std::{collections::HashMap, sync::Arc};

use crate::config::{Config, OAuthProviderConfig};

#[derive(Clone)]
pub struct ProviderRegistry {
    adapters: Arc<HashMap<ProviderKind, Arc<dyn ProviderAdapter>>>,
}

impl ProviderRegistry {
    pub fn build(config: &Config) -> anyhow::Result<Self> {
        let mut adapters: HashMap<ProviderKind, Arc<dyn ProviderAdapter>> = HashMap::new();
        adapters.insert(ProviderKind::Craigslist, Arc::new(CraigslistAdapter));
        adapters.insert(
            ProviderKind::GenericWebhook,
            Arc::new(GenericWebhookAdapter::default()),
        );

        if let Some(provider) = &config.eventbrite {
            adapters.insert(
                ProviderKind::Eventbrite,
                Arc::new(EventbriteAdapter::new(oauth(provider))?),
            );
        }
        if let Some(provider) = &config.meetup {
            adapters.insert(
                ProviderKind::Meetup,
                Arc::new(MeetupAdapter::new(oauth(provider))?),
            );
        }
        if let Some(provider) = &config.meta {
            adapters.insert(
                ProviderKind::MetaFacebookPage,
                Arc::new(MetaFacebookPageAdapter::new(
                    oauth(provider),
                    config.meta_graph_version.clone(),
                )),
            );
        }
        Ok(Self {
            adapters: Arc::new(adapters),
        })
    }

    pub fn get(&self, provider: ProviderKind) -> Option<Arc<dyn ProviderAdapter>> {
        self.adapters.get(&provider).cloned()
    }

    pub fn capabilities(&self) -> Vec<(ProviderCapabilities, bool)> {
        ProviderKind::ALL
            .into_iter()
            .map(|provider| {
                if let Some(adapter) = self.get(provider) {
                    (adapter.capabilities(), true)
                } else {
                    (static_capability(provider), false)
                }
            })
            .collect()
    }
}

fn oauth(config: &OAuthProviderConfig) -> OAuthClient {
    OAuthClient {
        client_id: config.client_id.clone(),
        client_secret: SecretString::from(config.client_secret.clone()),
        redirect_uri: config.redirect_uri.clone(),
    }
}

fn static_capability(provider: ProviderKind) -> ProviderCapabilities {
    use evgl_domain::DeliveryMode;
    match provider {
        ProviderKind::Eventbrite => ProviderCapabilities {
            provider,
            delivery_mode: DeliveryMode::NativeEvent,
            oauth: true,
            create: true,
            update: false,
            delete: false,
            publish: true,
            webhooks: true,
            requires_manual_step: false,
            notes: vec!["OAuth credentials are not configured on this deployment.".into()],
        },
        ProviderKind::Meetup => ProviderCapabilities {
            provider,
            delivery_mode: DeliveryMode::NativeEvent,
            oauth: true,
            create: true,
            update: false,
            delete: false,
            publish: true,
            webhooks: false,
            requires_manual_step: false,
            notes: vec!["OAuth credentials are not configured on this deployment.".into()],
        },
        ProviderKind::MetaFacebookPage => ProviderCapabilities {
            provider,
            delivery_mode: DeliveryMode::DistributionPost,
            oauth: true,
            create: true,
            update: false,
            delete: false,
            publish: true,
            webhooks: true,
            requires_manual_step: false,
            notes: vec!["Meta OAuth credentials are not configured on this deployment.".into()],
        },
        ProviderKind::Craigslist => ProviderCapabilities {
            provider,
            delivery_mode: DeliveryMode::ManualHandoff,
            oauth: false,
            create: false,
            update: false,
            delete: false,
            publish: false,
            webhooks: false,
            requires_manual_step: true,
            notes: vec!["General automated posting is intentionally disabled.".into()],
        },
        ProviderKind::GenericWebhook => ProviderCapabilities {
            provider,
            delivery_mode: DeliveryMode::SignedWebhook,
            oauth: false,
            create: true,
            update: false,
            delete: false,
            publish: true,
            webhooks: false,
            requires_manual_step: false,
            notes: vec!["Requires an HTTPS endpoint and HMAC secret.".into()],
        },
    }
}
