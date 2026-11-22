use async_trait::async_trait;
use dashmap::DashMap;
use reqwest::Client;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

use crate::auth::google::{GoogleVerifier, LiveGoogleVerifier};
use crate::autotrade::scheduler::SchedulerHandle;
use crate::config::Config;
use crate::crypto::Crypto;
use crate::errors::ApiResult;
use crate::integrations::toss::{TossClient, TossHttpOverride};
use crate::market::extras::ExtrasCache;
use crate::market::toss::TossMarketProvider;
use crate::realtime::hub::Hub;
use crate::storage::{init_storage, Store};
use crate::transport_crypto::TransportCrypto;
use crate::version::read_version;

#[async_trait]
pub trait NotifierHook: Send + Sync {
    async fn send_http(&self, url: &str, payload: &Value) -> Result<(), String>;
}

pub struct LiveNotifierHook {
    client: Client,
}

impl LiveNotifierHook {
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(4))
            .build()
            .expect("reqwest notifier client");
        Self { client }
    }
}

#[async_trait]
impl NotifierHook for LiveNotifierHook {
    async fn send_http(&self, url: &str, payload: &Value) -> Result<(), String> {
        let resp = self
            .client
            .post(url)
            .json(payload)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = resp.status();
        if status.as_u16() >= 400 {
            let txt = resp.text().await.unwrap_or_default();
            let preview: String = txt.chars().take(120).collect();
            return Err(format!("HTTP {}: {}", status.as_u16(), preview));
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub store: Arc<dyn Store>,
    pub crypto: Arc<Crypto>,
    pub transport: Arc<TransportCrypto>,
    pub http: Client,
    pub app_version: String,
    pub google_verifier: Arc<dyn GoogleVerifier>,
    pub notifier_hook: Arc<dyn NotifierHook>,
    pub toss_clients: Arc<DashMap<String, (String, Arc<TossClient>)>>,
    pub toss_override: Option<Arc<dyn TossHttpOverride>>,
    pub market_toss: Arc<TossMarketProvider>,
    pub market_extras: Arc<ExtrasCache>,
    pub realtime_hub: Arc<Hub>,
    pub scheduler: Arc<SchedulerHandle>,
}

impl AppState {
    pub fn new(config: Config) -> Result<Self, String> {
        config.validate_production()?;
        let store = init_storage(&config.storage_backend, &config.data_dir)?;
        let crypto = Arc::new(Crypto::from_config(&config)?);
        let http = Client::builder()
            .timeout(Duration::from_secs_f64(config.toss_timeout_sec.max(1.0)))
            .build()
            .map_err(|e| format!("reqwest client build failed: {e}"))?;
        let google_verifier: Arc<dyn GoogleVerifier> = Arc::new(LiveGoogleVerifier::new(http.clone()));
        let notifier_hook: Arc<dyn NotifierHook> = Arc::new(LiveNotifierHook::new());
        Ok(Self {
            config: Arc::new(config),
            store,
            crypto,
            transport: Arc::new(TransportCrypto::new()),
            http,
            app_version: read_version(),
            google_verifier,
            notifier_hook,
            toss_clients: Arc::new(DashMap::new()),
            toss_override: None,
            market_toss: Arc::new(TossMarketProvider::new()),
            market_extras: Arc::new(ExtrasCache::new()),
            realtime_hub: Arc::new(Hub::new()),
            scheduler: Arc::new(SchedulerHandle::new()),
        })
    }

    pub fn forget_toss_client(&self, user_id: &str, client_id: Option<&str>) {
        self.toss_clients.remove(user_id);
        if let Some(cid) = client_id {
            self.store
                .delete(crate::integrations::toss::TOKENS, &crate::integrations::toss::token_id(cid));
        }
    }

    pub fn get_toss_client(&self, user_id: &str) -> ApiResult<Arc<TossClient>> {
        let creds = crate::users::credentials::get_toss(self, user_id).ok_or_else(|| {
            crate::errors::ApiError::conflict(
                "toss_credentials_required",
                "토스증권 API 키를 먼저 등록하세요 (설정)",
            )
        })?;
        if let Some(hit) = self.toss_clients.get(user_id) {
            if hit.0 == creds.version {
                return Ok(hit.1.clone());
            }
        }
        let client = Arc::new(TossClient::new(
            self.config.toss_api_base_url.clone(),
            creds.client_id,
            creds.client_secret,
            self.store.clone(),
            self.crypto.clone(),
            self.http.clone(),
            self.toss_override.clone(),
        )?);
        self.toss_clients
            .insert(user_id.to_string(), (creds.version, client.clone()));
        Ok(client)
    }
}
